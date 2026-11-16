#![doc = include_str!("../README.md")]

mod access_review;
mod client;
mod error;
mod key;
mod records;
#[cfg(test)]
mod testing;
mod transport;

pub use access_review::{ApplicationId, MAX_USERS, ReviewUser, UserProblem, UserSet};
pub use client::{
    Client, Connection, ConnectionId, DirectRecords, EmptySnapshot, RecordFailure, Region,
    Resource, ResourceId, RetryPolicy, ReviewUserRecord, StoredRecord, Synced, Upserted,
    UsersLoaded,
};
pub use error::{DrataMessage, Error, Result};
pub use key::ApiKey;
pub use records::{Batch, CustomRecord, RecordProblem};
pub use transport::{Method, NativeTransport, Request, Response, Transport, TransportError};
