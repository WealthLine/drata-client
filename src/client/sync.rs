//! Full replace of a resource through a v2 session.

use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::{Map, Value};

use super::{
    Client, ConnectionId, PAGE_SIZE, ResourceId, decode,
    records::{ItemResults, Upserted, encode_component, records_path},
};
use crate::{
    Batch, Error, Result,
    transport::{Method, Transport},
};

/// What [`Client::sync`] and [`Client::load_access_review_users`] do with an
/// empty batch or user set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptySnapshot {
    /// Refuse it. A source read that failed quietly must never empty Drata.
    Refuse,
    /// Send it, deleting every record the resource holds, or removing every
    /// user from the application.
    Allow,
}

/// What a [`Client::sync`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Synced {
    /// Records in the snapshot that the resource did not hold.
    pub created: usize,
    /// Records the resource held with different data.
    pub updated: usize,
    /// Records the resource held with the same data.
    pub unchanged: usize,
    /// Records the resource held that the snapshot left out, now deleted.
    pub deleted: usize,
    /// Sessions a crashed earlier run left in progress, cancelled first.
    pub abandoned_sessions_cancelled: usize,
}

impl<T: Transport> Client<T> {
    /// Makes the resource hold exactly `batch`, replacing everything else.
    ///
    /// It cancels any session a crashed earlier run left in progress, reads
    /// the records the resource holds, uploads `batch` into a new session,
    /// checking every record Drata answers for, and completes the session
    /// only when every record was accepted. Completing is the one step that
    /// changes what Drata shows, and it deletes every record not in `batch`.
    ///
    /// Any error before completion cancels the session, so Drata keeps its
    /// previous records. If the returned future is dropped before it
    /// finishes, the session is left in progress and never completed, and
    /// the next `sync` cancels it.
    ///
    /// **Use only `sync` on a resource it keeps.** Records written through
    /// [`direct_records`](Self::direct_records) are deleted by the next
    /// completion.
    ///
    /// The key needs Custom Connections Data: Create Data, and Custom
    /// Connections Management: List Custom Connections.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptySnapshot`] for an empty batch unless `empty` is
    /// [`EmptySnapshot::Allow`], [`Error::RecordsRejected`] if Drata refused
    /// any record, [`Error::SessionNotActive`] if completing did not make the
    /// session live, and the usual errors if Drata refuses the key, does not
    /// know the connection or resource, keeps failing after every retry, or
    /// answers with unexpected JSON.
    pub async fn sync(
        &self,
        connection: ConnectionId,
        resource: ResourceId,
        batch: &Batch,
        empty: EmptySnapshot,
    ) -> Result<Synced> {
        self.sync_in_session(connection, resource, batch, empty, &new_session_id())
            .await
    }

    /// [`sync`](Self::sync), with the session ID given, so tests can name it.
    pub(crate) async fn sync_in_session(
        &self,
        connection: ConnectionId,
        resource: ResourceId,
        batch: &Batch,
        empty: EmptySnapshot,
        session: &str,
    ) -> Result<Synced> {
        if batch.is_empty() && empty == EmptySnapshot::Refuse {
            return Err(Error::EmptySnapshot);
        }
        let sessions = SessionPaths::new(connection, resource);
        let mut synced = Synced {
            abandoned_sessions_cancelled: self.cancel_in_progress(&sessions).await?,
            ..Synced::default()
        };
        let before: HashMap<String, Map<String, Value>> = self
            .list_records(connection, resource)
            .await?
            .into_iter()
            .map(|record| (record.id, record.attributes))
            .collect();

        if let Err(error) = self.upload(&sessions, session, batch).await {
            // The upload's own error is the one worth reporting; a failed
            // cancel leaves a session the next sync cancels.
            let _cancelled = self.act(&sessions, session, "cancel").await;
            return Err(error);
        }
        let status = self.act(&sessions, session, "complete").await?;
        if status != "ACTIVE" {
            return Err(Error::SessionNotActive { status });
        }

        let mut seen = 0;
        for record in batch.records() {
            let Some(id) = record.get("id").and_then(Value::as_str) else {
                continue;
            };
            match before.get(id) {
                None => synced.created += 1,
                Some(attributes) if same_data(attributes, record) => synced.unchanged += 1,
                Some(_) => synced.updated += 1,
            }
            if before.contains_key(id) {
                seen += 1;
            }
        }
        synced.deleted = before.len() - seen;
        Ok(synced)
    }

    /// Cancels every session left in progress, returning how many.
    async fn cancel_in_progress(&self, sessions: &SessionPaths) -> Result<usize> {
        let path = format!("{}?size={PAGE_SIZE}&status=IN_PROGRESS", sessions.list);
        let stale: Vec<String> = self
            .list_all::<SessionSummary>(&path, "list custom data sessions")
            .await?
            .into_iter()
            .filter(|session| session.status == "IN_PROGRESS")
            .map(|session| session.session_id)
            .collect();
        for session in &stale {
            self.act(sessions, session, "cancel").await?;
        }
        Ok(stale.len())
    }

    /// Uploads every body into `session`, checking each record's status.
    async fn upload(&self, sessions: &SessionPaths, session: &str, batch: &Batch) -> Result<()> {
        let path = sessions.upload(session);
        let mut tally = Upserted::default();
        let mut offset = 0;
        for body in batch.bodies() {
            let answer = self
                .call(Method::Post, &path, Some(body.clone().into_bytes()))
                .await?;
            let results: ItemResults = decode(&answer, "upload custom data to a session")?;
            let results = results.into_vec();
            tally.tally(&results, offset)?;
            offset += results.len();
        }
        Ok(())
    }

    /// Completes or cancels `session`, returning its status afterwards.
    async fn act(&self, sessions: &SessionPaths, session: &str, action: &str) -> Result<String> {
        let body = format!(r#"{{"action":"{action}"}}"#).into_bytes();
        let answer = self
            .call(Method::Post, &sessions.action(session), Some(body))
            .await?;
        let acted: SessionAction = decode(&answer, "perform a session action")?;
        Ok(acted.status)
    }
}

/// Whether a stored record's attributes match the record about to be sent,
/// ignoring the `id`, which Drata may or may not echo in the attributes.
fn same_data(stored: &Map<String, Value>, sent: &Map<String, Value>) -> bool {
    let without_id = |map: &Map<String, Value>| {
        map.iter()
            .filter(|(field, _)| field.as_str() != "id")
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect::<Map<String, Value>>()
    };
    without_id(stored) == without_id(sent)
}

/// A session ID unique to this run: the time in milliseconds and the process
/// ID, which fit Drata's 3 to 64 letters, digits, hyphens and underscores.
fn new_session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("sync-{millis}-{}", std::process::id())
}

/// The session paths of one resource.
struct SessionPaths {
    list: String,
}

impl SessionPaths {
    fn new(connection: ConnectionId, resource: ResourceId) -> Self {
        let records = records_path(connection, resource);
        let base = records.trim_end_matches("/records");
        Self {
            list: format!("{base}/sessions"),
        }
    }

    fn upload(&self, session: &str) -> String {
        format!("{}/{}", self.list, encode_component(session))
    }

    fn action(&self, session: &str) -> String {
        format!("{}/actions", self.upload(session))
    }
}

/// The parts of a session the client reads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionSummary {
    session_id: String,
    status: String,
}

/// Drata's answer to a session action.
#[derive(Deserialize)]
struct SessionAction {
    status: String,
}

#[cfg(test)]
mod tests;
