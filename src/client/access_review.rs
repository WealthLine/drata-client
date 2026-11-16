//! Calls on an Access Review application's users: load the full set, and
//! list it.

use std::fmt;

use serde::Deserialize;

use super::{Client, PAGE_SIZE, decode};
use crate::{
    ApplicationId, EmptySnapshot, Error, Result, UserSet,
    transport::{Method, Transport},
};

impl<T: Transport> Client<T> {
    /// Replaces an Access Review application's users with `users`.
    ///
    /// Drata treats the set as complete: users left out are removed from the
    /// application and its open review, and Drata keeps their name and email
    /// as disconnected identities. The application must be one managed by
    /// hand in Drata, which includes one added under Access Reviews >
    /// Applications.
    ///
    /// Once reviewers have recorded decisions, or the application was
    /// completed while its review period is still open, Drata refuses the load
    /// with 409. That comes back as [`UsersLoaded::ReviewLocked`], not as an
    /// error, so a daily job can skip the day and load again once the review
    /// period is completed.
    ///
    /// The key needs Access Review: Load Access Review Application User Set
    /// (`access-review-application-users-put`), and the account needs the
    /// Access Review feature.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptySnapshot`] for an empty set unless `empty` is
    /// [`EmptySnapshot::Allow`], and the usual errors if Drata refuses the key
    /// or the body, does not know the application, keeps failing after every
    /// retry, or answers with unexpected JSON.
    pub async fn load_access_review_users(
        &self,
        application: ApplicationId,
        users: &UserSet,
        empty: EmptySnapshot,
    ) -> Result<UsersLoaded> {
        if users.is_empty() && empty == EmptySnapshot::Refuse {
            return Err(Error::EmptySnapshot);
        }
        let body = users.body().as_bytes().to_vec();
        match self
            .call(Method::Put, &users_path(application), Some(body))
            .await
        {
            Ok(answer) => {
                let saved: SavedCount = decode(&answer, "load access review application users")?;
                Ok(UsersLoaded::Saved {
                    count: saved.saved_count,
                })
            }
            Err(Error::Rejected { status: 409, .. }) => Ok(UsersLoaded::ReviewLocked),
            Err(error) => Err(error),
        }
    }

    /// Lists the users an Access Review application holds, following Drata's
    /// pages, so a caller can compare before loading.
    ///
    /// The key needs Access Review: List Access Review Application Users
    /// (`access-review-application-users-get`), and the account needs the
    /// Access Review feature.
    ///
    /// # Errors
    ///
    /// Returns an error if Drata refuses the key, does not know the
    /// application, keeps failing after every retry, or answers with JSON
    /// that is not a page of users.
    pub async fn access_review_users(
        &self,
        application: ApplicationId,
    ) -> Result<Vec<ReviewUserRecord>> {
        let path = format!("{}?size={PAGE_SIZE}", users_path(application));
        self.list_all(&path, "list access review application users")
            .await
    }
}

/// What [`Client::load_access_review_users`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[must_use = "a locked review means the users were not loaded"]
pub enum UsersLoaded {
    /// Drata replaced the user set.
    Saved {
        /// How many users the application holds now, as Drata counts them.
        count: usize,
    },
    /// Drata refused with 409: reviewers have recorded decisions, or the
    /// application was completed while its review period is still open.
    /// Nothing changed; load again after the review period is completed.
    ReviewLocked,
}

/// A user as Drata holds it, from [`Client::access_review_users`].
///
/// Its `Debug` output shows Drata's ID and the number of roles, not the
/// identity ID, username or email, any of which can be an email address.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ReviewUserRecord {
    /// Drata's ID for the user within the application.
    pub id: u64,
    /// The ID the user was loaded with, if Drata still holds one.
    pub identity_id: Option<String>,
    /// The user's username, if Drata still holds one.
    pub username: Option<String>,
    /// The user's email address, if Drata still holds one.
    pub email: Option<String>,
    /// The user's roles.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Whether Drata reads any of the roles as granting admin access.
    #[serde(default)]
    pub has_admin_access: bool,
}

impl fmt::Debug for ReviewUserRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReviewUserRecord")
            .field("id", &self.id)
            .field("roles", &self.roles.len())
            .field("has_admin_access", &self.has_admin_access)
            .finish_non_exhaustive()
    }
}

/// Drata's answer to a load.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedCount {
    saved_count: usize,
}

/// The path of an application's users.
fn users_path(application: ApplicationId) -> String {
    format!("/access-review-applications/{}/users", application.get())
}

#[cfg(test)]
mod tests;
