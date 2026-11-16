//! The crate's error type.

use std::{fmt, time::Duration};

use crate::{RecordFailure, RecordProblem, TransportError, UserProblem, access_review::MAX_USERS};

/// Everything that can go wrong talking to Drata.
///
/// No variant carries the API key or a request body. Drata's own message is
/// kept as a [`DrataMessage`], which neither the error's text nor its `Debug`
/// output shows, since Drata can quote a value from the request in it; read
/// it with [`Error::drata_message`]. A decoding failure keeps only where in
/// the answer it happened, since `serde_json`'s message can quote a value
/// from it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Drata refused the key: 401, or 403 when the key lacks a permission.
    #[error("Drata refused the API key with HTTP {status}")]
    Unauthorized {
        /// 401 or 403.
        status: u16,
        /// Drata's message, if it sent one.
        message: Option<DrataMessage>,
    },

    /// Drata kept answering 429 after every retry.
    #[error("Drata rate-limited the client after every retry")]
    RateLimited {
        /// The last `Retry-After` Drata sent, if any.
        retry_after: Option<Duration>,
    },

    /// Drata refused the request body as too large, with 413. Drata's limit is
    /// 5 MB per request.
    #[error("Drata refused the request as too large with HTTP 413")]
    PayloadTooLarge,

    /// Drata refused the request with another 4xx status, such as 400 for a
    /// record that fails the resource's schema, 402 when the plan lacks custom
    /// connections, or 404 for an unknown connection or resource.
    #[error("Drata refused the request with HTTP {status}{}", code_suffix(*code))]
    Rejected {
        /// The status code.
        status: u16,
        /// Drata's numeric error code, if it sent one.
        code: Option<i64>,
        /// Drata's message, if it sent one.
        message: Option<DrataMessage>,
    },

    /// Drata kept failing with a 5xx status after every retry.
    #[error("Drata failed with HTTP {status} after every retry")]
    Server {
        /// The last status code.
        status: u16,
        /// Drata's message, if it sent one.
        message: Option<DrataMessage>,
    },

    /// Drata answered with a status the client does not expect, such as a
    /// redirect.
    #[error("Drata answered with unexpected HTTP {status}")]
    UnexpectedStatus {
        /// The status code.
        status: u16,
    },

    /// No HTTP answer arrived after every retry.
    #[error("reaching Drata")]
    Transport(#[source] TransportError),

    /// A record cannot be sent. Only its position is given, never its
    /// contents.
    #[error("checking the record at {index}: {problem}")]
    InvalidRecord {
        /// The record's position in the batch.
        index: usize,
        /// What is wrong with it.
        problem: RecordProblem,
    },

    /// The batch or user set was empty and the caller did not allow an empty
    /// snapshot.
    #[error("refusing to send an empty snapshot, which would remove everything Drata holds")]
    EmptySnapshot,

    /// A user cannot be loaded into an Access Review application. Only their
    /// position is given, never their details.
    #[error("checking the user at {index}: {problem}")]
    InvalidUser {
        /// The user's position in the set.
        index: usize,
        /// What is wrong with them.
        problem: UserProblem,
    },

    /// The user set holds more users than Drata accepts in one application.
    #[error("the user set holds {count} users, over Drata's limit of {MAX_USERS}")]
    TooManyUsers {
        /// How many users the set holds.
        count: usize,
    },

    /// The user set's body is over Drata's 5 MB limit.
    #[error("the user set is {bytes} bytes as JSON, over Drata's 5 MB limit")]
    UserSetTooLarge {
        /// The body's size.
        bytes: usize,
    },

    /// Completing a session did not make it the live set.
    #[error("completing the session left it `{status}`, not `ACTIVE`")]
    SessionNotActive {
        /// The status Drata gave the session.
        status: String,
    },

    /// Drata answered an upload with 200 but refused some of its records.
    #[error(
        "Drata refused {} record(s), the first at {}{}",
        failures.len(),
        failures.first().map_or(0, |failure| failure.index),
        failures.first().map_or_else(String::new, |failure| format!(
            " with status {}{}",
            failure.status,
            code_suffix(failure.code)
        ))
    )]
    RecordsRejected {
        /// Every refused record.
        failures: Vec<RecordFailure>,
    },

    /// Drata's answer was not the JSON the client expected.
    #[error("decoding Drata's answer to {call} at line {line}, column {column}")]
    Decode {
        /// The call whose answer failed to decode.
        call: &'static str,
        /// The line in the answer.
        line: usize,
        /// The column in the answer.
        column: usize,
    },
}

/// The crate's result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Drata's own message about the error, if it sent one: for
    /// [`Unauthorized`](Self::Unauthorized), [`Rejected`](Self::Rejected) and
    /// [`Server`](Self::Server), and the first refused record's for
    /// [`RecordsRejected`](Self::RecordsRejected).
    ///
    /// It can quote a value from the request, such as a person's email
    /// address, so decide where it may go before logging
    /// [`as_str`](DrataMessage::as_str).
    #[must_use]
    pub fn drata_message(&self) -> Option<&DrataMessage> {
        match self {
            Self::Unauthorized { message, .. }
            | Self::Rejected { message, .. }
            | Self::Server { message, .. } => message.as_ref(),
            Self::RecordsRejected { failures } => failures
                .first()
                .and_then(|failure| failure.message.as_ref()),
            _ => None,
        }
    }
}

/// Drata's own text explaining an error, such as which schema rule a record
/// failed.
///
/// Drata can quote a value from the request in it, so it has no `Display`,
/// and its `Debug` output shows only its length. Read the text with
/// [`as_str`](Self::as_str).
#[derive(Clone, PartialEq, Eq)]
pub struct DrataMessage(String);

impl DrataMessage {
    /// Wraps the text Drata sent.
    pub(crate) const fn new(text: String) -> Self {
        Self(text)
    }

    /// The text Drata sent.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Its length only: the text can quote a value from the request.
impl fmt::Debug for DrataMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DrataMessage({} chars)", self.0.chars().count())
    }
}

/// Formats Drata's optional numeric error code as a ` (code n)` suffix.
fn code_suffix(code: Option<i64>) -> String {
    code.map_or_else(String::new, |code| format!(" (code {code})"))
}
