//! How a request reaches Drata.
//!
//! The client is generic over [`Transport`], so the HTTP stack is the caller's
//! choice: [`NativeTransport`] for a normal binary, and later a `wasm32`
//! transport, without a change to the client's API.

mod native;

use std::{error::Error as StdError, fmt, future::Future, time::Duration};

pub use native::NativeTransport;

use crate::ApiKey;

/// The HTTP methods the client uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// `GET`
    Get,
    /// `POST`
    Post,
    /// `PUT`
    Put,
    /// `DELETE`
    Delete,
}

/// One request to Drata.
///
/// Its `Debug` output names the method and URL and only the length of the
/// body, because a body holds the caller's records, and the key is redacted by
/// [`ApiKey`] itself.
pub struct Request<'a> {
    /// The method.
    pub method: Method,
    /// The full URL, including the query.
    pub url: String,
    /// The key, sent as `Authorization: Bearer <key>`.
    pub key: &'a ApiKey,
    /// A JSON body, if the call has one.
    pub body: Option<Vec<u8>>,
}

impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("key", self.key)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// Drata's answer, whatever its status.
///
/// Its `Debug` output shows only the length of the body, which can hold
/// records.
pub struct Response {
    /// The HTTP status code.
    pub status: u16,
    /// The `Retry-After` header, when it is given in seconds.
    pub retry_after: Option<Duration>,
    /// The body.
    pub body: Vec<u8>,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("retry_after", &self.retry_after)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// A request that did not get an HTTP answer: no connection, a timeout, or
/// a TLS failure.
#[derive(Debug, thiserror::Error)]
#[error("sending the request to Drata")]
pub struct TransportError {
    #[source]
    source: Box<dyn StdError + Send + Sync>,
}

impl TransportError {
    /// Wraps the HTTP stack's own error.
    #[must_use]
    pub fn new(source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// Sends requests to Drata and waits between retries.
///
/// The returned futures carry no `Send` bound, so a single-threaded `wasm32`
/// transport can implement this trait too.
pub trait Transport {
    /// Sends one request and returns Drata's answer, whatever its status.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] only when no HTTP answer arrived. An error
    /// status such as 404 or 500 is an `Ok` [`Response`].
    fn send(&self, request: Request<'_>) -> impl Future<Output = Result<Response, TransportError>>;

    /// Waits before the client retries a request.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()>;
}
