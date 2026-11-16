//! A connector's records, validated and turned into the bodies Drata accepts.
//!
//! [`Batch`] needs no key and makes no call, so it is also the dry run: the
//! bodies it builds are exactly what the client sends.

use std::{collections::HashMap, fmt};

use serde::Serialize;
use serde_json::Value;

/// Drata's limit is 5 MB per request. Taken as 5,000,000 bytes, the smaller
/// reading of "5 MB", so a body is never near the edge.
pub(crate) const MAX_BODY_BYTES: usize = 5_000_000;

/// The record field Drata matches records by.
const ID_FIELD: &str = "id";

/// The bytes around the records in every body: `{"data":[` and `]}`.
const BODY_FRAME_BYTES: usize = r#"{"data":[]}"#.len();

/// One kind of record a connector pushes, matching one custom resource in
/// Drata. Implement it on the record type.
///
/// The record serializes to a JSON object. The client adds an `id` field
/// holding [`key`](Self::key), which is how Drata tells one record from the
/// next, so a record with its own `id` field must give it the same value.
pub trait CustomRecord: Serialize {
    /// The top-level field Drata labels each record with in its interface,
    /// such as `email`. It must hold a string or a number in every record, and
    /// it is the resource's `displayNameKey` in Drata.
    const DISPLAY_NAME_KEY: &'static str;

    /// The record's stable identity, such as an email address or a user ID.
    /// It must be unique within one batch and must not be empty.
    fn key(&self) -> String;
}

/// Records validated against the rules Drata enforces, and the request bodies
/// built from them.
///
/// Building a batch needs no key and makes no call, which makes
/// [`bodies`](Self::bodies) the dry run: they are exactly what the client
/// sends, split so each stays under Drata's 5 MB limit.
pub struct Batch {
    records: Vec<Value>,
    bodies: Vec<String>,
}

impl Batch {
    /// Validates `records` and builds the request bodies.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRecord`](crate::Error::InvalidRecord), naming
    /// the record's position and never its contents, if a record does not
    /// serialize to a JSON object, lacks a string or number at
    /// [`CustomRecord::DISPLAY_NAME_KEY`], has an empty key or one already
    /// used in the batch, has an `id` field that differs from its key, or is
    /// too large to send on its own.
    pub fn new<R: CustomRecord>(records: &[R]) -> crate::Result<Self> {
        let mut prepared = Vec::with_capacity(records.len());
        let mut first_index_of_key: HashMap<String, usize> = HashMap::new();
        for (index, record) in records.iter().enumerate() {
            let invalid = |problem| crate::Error::InvalidRecord { index, problem };
            let key = record.key();
            if key.is_empty() {
                return Err(invalid(RecordProblem::EmptyKey));
            }
            if let Some(&first) = first_index_of_key.get(&key) {
                return Err(invalid(RecordProblem::DuplicateKey { first }));
            }
            let Ok(Value::Object(mut object)) = serde_json::to_value(record) else {
                return Err(invalid(RecordProblem::NotAnObject));
            };
            match object.get(R::DISPLAY_NAME_KEY) {
                Some(Value::String(_) | Value::Number(_)) => {}
                _ => return Err(invalid(RecordProblem::DisplayName(R::DISPLAY_NAME_KEY))),
            }
            match object.get(ID_FIELD) {
                None => {
                    object.insert(ID_FIELD.to_owned(), Value::String(key.clone()));
                }
                Some(Value::String(id)) if *id == key => {}
                Some(_) => return Err(invalid(RecordProblem::IdDiffersFromKey)),
            }
            first_index_of_key.insert(key, index);
            prepared.push(Value::Object(object));
        }
        let bodies = split_into_bodies(&prepared)?;
        Ok(Self {
            records: prepared,
            bodies,
        })
    }

    /// How many records the batch holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the batch holds no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The validated records, `id` included, in order.
    pub(crate) fn records(&self) -> impl Iterator<Item = &serde_json::Map<String, Value>> {
        self.records.iter().filter_map(Value::as_object)
    }

    /// The request bodies, in order, each `{"data":[...]}` and under 5 MB.
    /// This is the dry run.
    #[must_use]
    pub fn bodies(&self) -> &[String] {
        &self.bodies
    }

    /// Every record as one pretty-printed JSON array, `id` included, for a
    /// connector to print or commit.
    #[must_use]
    pub fn to_pretty_json(&self) -> String {
        format!("{:#}", Value::Array(self.records.clone()))
    }

    /// The first record, pretty-printed, for the `sampleData` field when the
    /// custom resource is created in Drata. `None` for an empty batch.
    #[must_use]
    pub fn sample_data(&self) -> Option<String> {
        self.records.first().map(|record| format!("{record:#}"))
    }
}

/// Counts only: records hold people's names and email addresses.
impl fmt::Debug for Batch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Batch")
            .field("records", &self.records.len())
            .field("bodies", &self.bodies.len())
            .finish()
    }
}

/// Packs records into as few bodies as fit under [`MAX_BODY_BYTES`].
fn split_into_bodies(records: &[Value]) -> crate::Result<Vec<String>> {
    let mut bodies = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_bytes = BODY_FRAME_BYTES;
    for (index, record) in records.iter().enumerate() {
        let json = record.to_string();
        if BODY_FRAME_BYTES + json.len() > MAX_BODY_BYTES {
            return Err(crate::Error::InvalidRecord {
                index,
                problem: RecordProblem::TooLarge { bytes: json.len() },
            });
        }
        let separator = usize::from(!current.is_empty());
        if current_bytes + separator + json.len() > MAX_BODY_BYTES {
            bodies.push(frame(&current));
            current.clear();
            current_bytes = BODY_FRAME_BYTES;
        }
        current_bytes += usize::from(!current.is_empty()) + json.len();
        current.push(json);
    }
    if !current.is_empty() {
        bodies.push(frame(&current));
    }
    Ok(bodies)
}

/// Wraps serialized records as `{"data":[...]}`.
fn frame(records: &[String]) -> String {
    format!(r#"{{"data":[{}]}}"#, records.join(","))
}

/// What is wrong with a record. It never quotes the record's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordProblem {
    /// The record does not serialize to a JSON object.
    NotAnObject,
    /// The display-name field, named here, is missing or is not a string or
    /// number.
    DisplayName(&'static str),
    /// The record's key is empty.
    EmptyKey,
    /// Another record in the batch, at `first`, has the same key.
    DuplicateKey {
        /// The position of the first record with this key.
        first: usize,
    },
    /// The record has an `id` field that differs from its key.
    IdDiffersFromKey,
    /// The record alone is larger than Drata accepts in one request.
    TooLarge {
        /// The record's size as JSON.
        bytes: usize,
    },
}

impl fmt::Display for RecordProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObject => f.write_str("it does not serialize to a JSON object"),
            Self::DisplayName(field) => {
                write!(
                    f,
                    "its `{field}` field is missing or not a string or number"
                )
            }
            Self::EmptyKey => f.write_str("its key is empty"),
            Self::DuplicateKey { first } => {
                write!(f, "its key is already used by the record at {first}")
            }
            Self::IdDiffersFromKey => f.write_str("its `id` field differs from its key"),
            Self::TooLarge { bytes } => write!(
                f,
                "it is {bytes} bytes as JSON, over Drata's {MAX_BODY_BYTES}-byte limit"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
