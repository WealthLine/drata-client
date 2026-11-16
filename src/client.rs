//! The client, its retry policy and the calls it makes.

use std::time::Duration;

use serde::{Deserialize, de::DeserializeOwned};

use crate::{
    ApiKey, DrataMessage, Error, NativeTransport, Result, TransportError,
    transport::{Method, Request, Response, Transport},
};

/// A Drata region, each with its own API host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Region {
    /// `public-api.drata.com`
    Us,
    /// `public-api.eu.drata.com`
    Eu,
    /// `public-api.apac.drata.com`
    Apac,
}

impl Region {
    /// The v2 API's base URL in this region.
    #[must_use]
    pub const fn base_url(self) -> &'static str {
        match self {
            Self::Us => "https://public-api.drata.com/public/v2",
            Self::Eu => "https://public-api.eu.drata.com/public/v2",
            Self::Apac => "https://public-api.apac.drata.com/public/v2",
        }
    }
}

/// When the client retries a request.
///
/// It retries a 429, a 5xx and a request that got no answer, and nothing else.
/// It waits for Drata's `Retry-After` when one is given, otherwise for a
/// backoff that doubles from `initial_backoff`, and never longer than
/// `max_backoff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl RetryPolicy {
    /// A policy of `max_retries` retries after the first attempt.
    #[must_use]
    pub const fn new(max_retries: u32, initial_backoff: Duration, max_backoff: Duration) -> Self {
        Self {
            max_retries,
            initial_backoff,
            max_backoff,
        }
    }

    /// No retries.
    #[must_use]
    pub const fn none() -> Self {
        Self::new(0, Duration::ZERO, Duration::ZERO)
    }

    /// The wait before retry number `retry`, counting from zero.
    fn backoff(&self, retry: u32, retry_after: Option<Duration>) -> Duration {
        let doubled = self.initial_backoff.saturating_mul(1_u32 << retry.min(16));
        retry_after.unwrap_or(doubled).min(self.max_backoff)
    }
}

impl Default for RetryPolicy {
    /// Three retries, waiting 1, 2 and 4 seconds, or Drata's `Retry-After` up
    /// to 30 seconds.
    fn default() -> Self {
        Self::new(3, Duration::from_secs(1), Duration::from_secs(30))
    }
}

/// A client for Drata's custom connections API.
///
/// Its `Debug` output redacts the key.
#[derive(Debug, Clone)]
pub struct Client<T = NativeTransport> {
    transport: T,
    key: ApiKey,
    base_url: String,
    retry: RetryPolicy,
}

impl Client<NativeTransport> {
    /// A client on the native transport, for `region`, with the default retry
    /// policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the native transport cannot be built; see
    /// [`NativeTransport::new`].
    pub fn native(key: ApiKey, region: Region) -> Result<Self> {
        let transport = NativeTransport::new().map_err(Error::Transport)?;
        Ok(Self::new(transport, key, region))
    }
}

impl<T: Transport> Client<T> {
    /// A client on `transport`, for `region`, with the default retry policy.
    #[must_use]
    pub fn new(transport: T, key: ApiKey, region: Region) -> Self {
        Self {
            transport,
            key,
            base_url: region.base_url().to_owned(),
            retry: RetryPolicy::default(),
        }
    }

    /// Sends requests to `base_url` instead of the region's, for tests. It is
    /// the v2 base, ending in `/public/v2`, with no trailing slash.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Replaces the retry policy.
    #[must_use]
    pub const fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Reads a custom connection and its resources.
    ///
    /// The key needs the Custom Connections Management: List Custom
    /// Connections permission.
    ///
    /// # Errors
    ///
    /// Returns an error if Drata refuses the key, does not know the
    /// connection, keeps failing after every retry, or answers with JSON that
    /// is not a connection.
    pub async fn connection(&self, id: ConnectionId) -> Result<Connection> {
        let path = format!("/custom-connections/{}?expand[]=customResources", id.get());
        let body = self.call(Method::Get, &path, None).await?;
        decode(&body, "get custom connection")
    }

    /// Follows Drata's cursor from `path`, which already carries its query
    /// string, to the last page, and returns every item in order.
    async fn list_all<D: DeserializeOwned>(
        &self,
        path: &str,
        call: &'static str,
    ) -> Result<Vec<D>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut page_path = path.to_owned();
            if let Some(cursor) = &cursor {
                page_path.push_str("&cursor=");
                page_path.push_str(&encode_component(cursor));
            }
            let body = self.call(Method::Get, &page_path, None).await?;
            let page: Page<D> = decode(&body, call)?;
            items.extend(page.data);
            match page.pagination.cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => return Ok(items),
            }
        }
    }

    /// Sends one call, retrying as the policy allows, and returns the body of
    /// a 2xx answer.
    async fn call(&self, method: Method, path: &str, body: Option<Vec<u8>>) -> Result<Vec<u8>> {
        let mut retry = 0;
        loop {
            let request = Request {
                method,
                url: format!("{}{path}", self.base_url),
                key: &self.key,
                body: body.clone(),
            };
            let outcome = self.transport.send(request).await;
            if retry < self.retry.max_retries
                && let Some(wait) = self.wait_before_retry(retry, &outcome)
            {
                self.transport.sleep(wait).await;
                retry += 1;
                continue;
            }
            return classify(outcome?);
        }
    }

    /// How long to wait before retrying this outcome, or `None` if it is not
    /// retried.
    fn wait_before_retry(
        &self,
        retry: u32,
        outcome: &Result<Response, TransportError>,
    ) -> Option<Duration> {
        match outcome {
            Err(_) => Some(self.retry.backoff(retry, None)),
            Ok(response) if response.status == 429 || response.status >= 500 => {
                Some(self.retry.backoff(retry, response.retry_after))
            }
            Ok(_) => None,
        }
    }
}

impl From<TransportError> for Error {
    fn from(source: TransportError) -> Self {
        Self::Transport(source)
    }
}

/// Turns a final answer into its body or an error.
fn classify(response: Response) -> Result<Vec<u8>> {
    let status = response.status;
    if (200..300).contains(&status) {
        return Ok(response.body);
    }
    let drata = DrataError::read(&response.body);
    Err(match status {
        401 | 403 => Error::Unauthorized {
            status,
            message: drata.message.map(DrataMessage::new),
        },
        413 => Error::PayloadTooLarge,
        429 => Error::RateLimited {
            retry_after: response.retry_after,
        },
        400..=499 => Error::Rejected {
            status,
            code: drata.code,
            message: drata.message.map(DrataMessage::new),
        },
        500..=599 => Error::Server {
            status,
            message: drata.message.map(DrataMessage::new),
        },
        _ => Error::UnexpectedStatus { status },
    })
}

/// The parts of Drata's error body the client keeps.
#[derive(Debug, Default, Deserialize)]
struct DrataError {
    message: Option<String>,
    code: Option<i64>,
}

impl DrataError {
    /// Reads Drata's error body, or nothing if it is not the usual JSON.
    fn read(body: &[u8]) -> Self {
        serde_json::from_slice(body).unwrap_or_default()
    }
}

/// The largest page Drata serves.
const PAGE_SIZE: u32 = 500;

/// One page of a cursor-paged listing.
#[derive(Deserialize)]
struct Page<D> {
    data: Vec<D>,
    pagination: Pagination,
}

/// The cursor to the next page, `None` or empty on the last.
#[derive(Deserialize)]
struct Pagination {
    cursor: Option<String>,
}

/// Decodes an answer, keeping only the position of a failure.
fn decode<D: DeserializeOwned>(body: &[u8], call: &'static str) -> Result<D> {
    serde_json::from_slice(body).map_err(|source| Error::Decode {
        call,
        line: source.line(),
        column: source.column(),
    })
}

/// A custom connection's ID in Drata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(transparent)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// Wraps the numeric ID Drata shows for the connection.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The numeric ID.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A custom resource's ID in Drata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(transparent)]
pub struct ResourceId(u64);

impl ResourceId {
    /// Wraps the numeric ID Drata shows for the resource.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The numeric ID.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A custom connection, as [`Client::connection`] reads it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Connection {
    /// The connection's ID.
    pub id: ConnectionId,
    /// The connection's name in Drata.
    pub client_alias: String,
    /// The connection's description, if it has one.
    #[serde(default)]
    pub description: Option<String>,
    /// The connection's resources.
    #[serde(default, rename = "customResources")]
    pub resources: Vec<Resource>,
}

/// A custom resource within a connection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct Resource {
    /// The resource's ID.
    pub id: ResourceId,
    /// The resource's name.
    pub name: String,
}

mod access_review;
mod records;
mod sync;

pub use access_review::{ReviewUserRecord, UsersLoaded};
use records::encode_component;
pub use records::{DirectRecords, RecordFailure, StoredRecord, Upserted};
pub use sync::{EmptySnapshot, Synced};

#[cfg(test)]
mod tests;
