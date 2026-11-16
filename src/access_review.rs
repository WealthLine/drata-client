//! An Access Review application's users, validated and turned into the body
//! Drata accepts.
//!
//! [`UserSet`] needs no key and makes no call, so it is also the dry run: its
//! [`body`](UserSet::body) is exactly what the client sends.

use std::{collections::HashMap, fmt};

use serde::Deserialize;
use serde_json::Value;

use crate::records::MAX_BODY_BYTES;

/// The most users Drata accepts in one application.
pub const MAX_USERS: usize = 4_000;

/// The longest `identityId`, `username` or `email` Drata accepts, in
/// characters.
const MAX_FIELD_CHARS: usize = 191;

/// The most roles Drata accepts on one user.
const MAX_ROLES: usize = 300;

/// An Access Review application's ID in Drata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(u64);

impl ApplicationId {
    /// Wraps the numeric ID Drata shows for the application.
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

/// One user to load into an Access Review application.
///
/// Its `Debug` output shows only how many roles it has, since the rest is a
/// person's name and email address.
#[derive(Clone, PartialEq, Eq)]
pub struct ReviewUser {
    identity_id: String,
    username: String,
    email: String,
    roles: Vec<String>,
}

impl ReviewUser {
    /// A user with no roles. `identity_id` is the user's stable ID in the
    /// source system, unique within the application.
    #[must_use]
    pub fn new(
        identity_id: impl Into<String>,
        username: impl Into<String>,
        email: impl Into<String>,
    ) -> Self {
        Self {
            identity_id: identity_id.into(),
            username: username.into(),
            email: email.into(),
            roles: Vec::new(),
        }
    }

    /// Sets the user's roles: free-form labels, as the source system reports
    /// them, such as `Owner` or `ReadOnly`.
    #[must_use]
    pub fn with_roles<R: Into<String>>(mut self, roles: impl IntoIterator<Item = R>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// The user as JSON, fields in the order Drata documents them, leaving
    /// out `roles` when there are none. `Value`'s `Display` escapes each
    /// string and cannot fail.
    fn to_json(&self) -> String {
        let mut json = format!(
            r#"{{"identityId":{},"username":{},"email":{}"#,
            Value::from(self.identity_id.as_str()),
            Value::from(self.username.as_str()),
            Value::from(self.email.as_str()),
        );
        if !self.roles.is_empty() {
            json.push_str(r#","roles":"#);
            json.push_str(&Value::from(self.roles.clone()).to_string());
        }
        json.push('}');
        json
    }
}

/// Counts only: the fields are a person's name and email address.
impl fmt::Debug for ReviewUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReviewUser")
            .field("roles", &self.roles.len())
            .finish_non_exhaustive()
    }
}

/// An application's full user set, validated against the rules Drata
/// enforces, and the request body built from it.
///
/// Building one needs no key and makes no call, which makes
/// [`body`](Self::body) the dry run.
pub struct UserSet {
    users: Vec<ReviewUser>,
    body: String,
}

impl UserSet {
    /// Validates `users` and builds the request body.
    ///
    /// An empty set is valid here: whether to send one is decided when it is
    /// loaded.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TooManyUsers`](crate::Error::TooManyUsers) for more
    /// than [`MAX_USERS`],
    /// [`Error::UserSetTooLarge`](crate::Error::UserSetTooLarge) for a body
    /// over Drata's 5 MB limit, and
    /// [`Error::InvalidUser`](crate::Error::InvalidUser), naming the user's
    /// position and never their details, for an empty `identity_id`, one
    /// already used in the set, a field over 191 characters, or more than 300
    /// roles.
    pub fn new(users: Vec<ReviewUser>) -> crate::Result<Self> {
        if users.len() > MAX_USERS {
            return Err(crate::Error::TooManyUsers { count: users.len() });
        }
        let mut first_index_of_id: HashMap<&str, usize> = HashMap::new();
        for (index, user) in users.iter().enumerate() {
            let invalid = |problem| crate::Error::InvalidUser { index, problem };
            if user.identity_id.is_empty() {
                return Err(invalid(UserProblem::EmptyIdentityId));
            }
            if let Some(&first) = first_index_of_id.get(user.identity_id.as_str()) {
                return Err(invalid(UserProblem::DuplicateIdentityId { first }));
            }
            for (field, value) in [
                ("identityId", &user.identity_id),
                ("username", &user.username),
                ("email", &user.email),
            ] {
                if value.chars().count() > MAX_FIELD_CHARS {
                    return Err(invalid(UserProblem::TooLong { field }));
                }
            }
            if user.roles.len() > MAX_ROLES {
                return Err(invalid(UserProblem::TooManyRoles {
                    count: user.roles.len(),
                }));
            }
            first_index_of_id.insert(&user.identity_id, index);
        }
        let identities: Vec<String> = users.iter().map(ReviewUser::to_json).collect();
        let body = format!(r#"{{"identities":[{}]}}"#, identities.join(","));
        if body.len() > MAX_BODY_BYTES {
            return Err(crate::Error::UserSetTooLarge { bytes: body.len() });
        }
        Ok(Self { users, body })
    }

    /// How many users the set holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.users.len()
    }

    /// Whether the set holds no users.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }

    /// The request body, `{"identities":[...]}`, exactly as the client sends
    /// it. This is the dry run.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// Counts only: users are people's names and email addresses.
impl fmt::Debug for UserSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserSet")
            .field("users", &self.users.len())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// What is wrong with a user. It never quotes the user's details.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UserProblem {
    /// The user's `identity_id` is empty.
    EmptyIdentityId,
    /// Another user in the set, at `first`, has the same `identity_id`.
    DuplicateIdentityId {
        /// The position of the first user with this `identity_id`.
        first: usize,
    },
    /// The named field is longer than Drata's 191 characters.
    TooLong {
        /// `identityId`, `username` or `email`.
        field: &'static str,
    },
    /// The user has more than Drata's 300 roles.
    TooManyRoles {
        /// How many roles the user has.
        count: usize,
    },
}

impl fmt::Display for UserProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIdentityId => f.write_str("its identity ID is empty"),
            Self::DuplicateIdentityId { first } => {
                write!(f, "its identity ID is already used by the user at {first}")
            }
            Self::TooLong { field } => write!(
                f,
                "its `{field}` is over Drata's {MAX_FIELD_CHARS}-character limit"
            ),
            Self::TooManyRoles { count } => {
                write!(f, "it has {count} roles, over Drata's limit of {MAX_ROLES}")
            }
        }
    }
}

#[cfg(test)]
mod tests;
