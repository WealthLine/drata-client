//! Calls on a resource's records: list, and direct upsert and delete.

use std::fmt;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::{Client, ConnectionId, PAGE_SIZE, ResourceId, decode};
use crate::{
    Batch, DrataMessage, Error, Result,
    transport::{Method, Transport},
};

impl<T: Transport> Client<T> {
    /// Lists every record a resource holds, following Drata's pages.
    ///
    /// The key needs the Custom Connections Management: List Custom
    /// Connections permission.
    ///
    /// # Errors
    ///
    /// Returns an error if Drata refuses the key, does not know the
    /// connection or resource, keeps failing after every retry, or answers
    /// with JSON that is not a page of records.
    pub async fn list_records(
        &self,
        connection: ConnectionId,
        resource: ResourceId,
    ) -> Result<Vec<StoredRecord>> {
        let path = format!("{}?size={PAGE_SIZE}", records_path(connection, resource));
        self.list_all(&path, "list custom data records").await
    }

    /// Direct calls on one resource's records, outside a session.
    ///
    /// **Do not use these on a resource that any session writes to**,
    /// including the library's full-replace sync. Completing a session deletes
    /// every record not in it, including records upserted here, and a direct
    /// upload made while a session is in progress is grouped into that session.
    /// Use one or the other on a resource, never both.
    #[must_use]
    pub const fn direct_records(
        &self,
        connection: ConnectionId,
        resource: ResourceId,
    ) -> DirectRecords<'_, T> {
        DirectRecords {
            client: self,
            connection,
            resource,
        }
    }
}

/// Direct upsert and delete on one resource, from
/// [`Client::direct_records`].
#[derive(Debug)]
pub struct DirectRecords<'a, T> {
    client: &'a Client<T>,
    connection: ConnectionId,
    resource: ResourceId,
}

impl<T: Transport> DirectRecords<'_, T> {
    /// Creates or updates every record in `batch`, matched by `id`, one
    /// request per body.
    ///
    /// Drata answers 200 even when some records fail, with a status for each,
    /// so every record's status is checked. Bodies already sent stay applied
    /// when a later one fails.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RecordsRejected`] if Drata refused any record, and the
    /// usual errors if it refuses the key, does not know the connection or
    /// resource, keeps failing after every retry, or answers with JSON that is
    /// not a list of record results.
    pub async fn upsert(&self, batch: &Batch) -> Result<Upserted> {
        let path = records_path(self.connection, self.resource);
        let mut upserted = Upserted::default();
        let mut offset = 0;
        for body in batch.bodies() {
            let answer = self
                .client
                .call(Method::Post, &path, Some(body.clone().into_bytes()))
                .await?;
            let results: ItemResults = decode(&answer, "upsert custom data records")?;
            let results = results.into_vec();
            upserted.tally(&results, offset)?;
            offset += results.len();
        }
        Ok(upserted)
    }

    /// Deletes the record with `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if Drata refuses the key, does not know the record,
    /// or keeps failing after every retry.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let path = format!(
            "{}/{}",
            records_path(self.connection, self.resource),
            encode_component(id)
        );
        self.client.call(Method::Delete, &path, None).await?;
        Ok(())
    }
}

/// What an upsert did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Upserted {
    /// Records Drata created, answered 201.
    pub created: usize,
    /// Records Drata updated, answered 200.
    pub updated: usize,
}

impl Upserted {
    /// Counts one body's results, or returns every failure in it.
    pub(super) fn tally(&mut self, results: &[ItemResult], offset: usize) -> Result<()> {
        let failures: Vec<RecordFailure> = results
            .iter()
            .enumerate()
            .filter(|(_, result)| !matches!(result.status_code, 200 | 201))
            .map(|(index, result)| RecordFailure {
                index: offset + index,
                status: result.status_code,
                message: result.error.as_ref().and_then(error_message),
                code: result.error.as_ref().and_then(error_code),
            })
            .collect();
        if !failures.is_empty() {
            return Err(Error::RecordsRejected { failures });
        }
        for result in results {
            if result.status_code == 201 {
                self.created += 1;
            } else {
                self.updated += 1;
            }
        }
        Ok(())
    }
}

/// One record Drata refused within an answer that was otherwise 200.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordFailure {
    /// The record's position in the batch.
    pub index: usize,
    /// The status Drata gave this record, such as 400.
    pub status: u16,
    /// Drata's numeric error code, such as 28022 for a schema failure.
    pub code: Option<i64>,
    /// Drata's message, such as which schema rule failed. Its `Debug` output
    /// shows only its length, since it can quote a value from the record.
    pub message: Option<DrataMessage>,
}

/// A record as Drata stores it, from [`Client::list_records`].
///
/// Its `Debug` output shows the ID and not the attributes, which hold
/// people's names and email addresses.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct StoredRecord {
    /// The record's ID, the key it was sent with.
    pub id: String,
    /// The record's data, as sent.
    pub attributes: Map<String, Value>,
    /// The session the record arrived in, if any.
    pub session_id: Option<String>,
}

impl fmt::Debug for StoredRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredRecord")
            .field("id", &self.id)
            .field("attributes", &self.attributes.len())
            .field("session_id", &self.session_id)
            .finish()
    }
}

/// Drata answers an array upload with an array, and may answer a single
/// record with an object.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum ItemResults {
    Many(Vec<ItemResult>),
    One(ItemResult),
}

impl ItemResults {
    pub(super) fn into_vec(self) -> Vec<ItemResult> {
        match self {
            Self::Many(results) => results,
            Self::One(result) => vec![result],
        }
    }
}

/// One record's result within an upsert's answer.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemResult {
    status_code: u16,
    #[serde(default)]
    error: Option<Value>,
}

/// Drata's documents show a record's error both as `{"message"}` and as
/// `{"error": {"message"}}`, so both are read.
fn error_field<'a>(error: &'a Value, field: &str) -> Option<&'a Value> {
    error
        .get(field)
        .or_else(|| error.get("error").and_then(|inner| inner.get(field)))
}

fn error_message(error: &Value) -> Option<DrataMessage> {
    error_field(error, "message")
        .and_then(Value::as_str)
        .map(|text| DrataMessage::new(text.to_owned()))
}

fn error_code(error: &Value) -> Option<i64> {
    error_field(error, "code").and_then(Value::as_i64)
}

/// The path of a resource's records.
pub(super) fn records_path(connection: ConnectionId, resource: ResourceId) -> String {
    format!(
        "/custom-connections/{}/resources/{}/records",
        connection.get(),
        resource.get()
    )
}

/// Percent-encodes everything but RFC 3986's unreserved characters, for a
/// cursor or record ID inside a URL.
pub(super) fn encode_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            for nibble in [byte >> 4, byte & 0x0F] {
                let digit = char::from_digit(u32::from(nibble), 16)
                    .expect("a nibble is below 16, so it is a hex digit");
                encoded.push(digit.to_ascii_uppercase());
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests;
