//! User sets: validation, the dry run and redaction.

use super::{MAX_USERS, ReviewUser, UserProblem, UserSet};
use crate::{Error, testing::RECORD_EMAIL};

fn alice() -> ReviewUser {
    ReviewUser::new("U-ALICE", "alice", RECORD_EMAIL).with_roles(["Owner", "Administrators"])
}

fn bob() -> ReviewUser {
    ReviewUser::new("U-BOB", "bob", "bob@example.com")
}

fn problem(error: &Error) -> Option<(usize, UserProblem)> {
    match error {
        Error::InvalidUser { index, problem } => Some((*index, problem.clone())),
        _ => None,
    }
}

#[test]
fn the_dry_run_matches_the_committed_fixture() {
    let set = UserSet::new(vec![alice(), bob()]).unwrap();

    assert_eq!(
        set.body(),
        include_str!("fixtures/users.body.json").trim_end(),
        "the body should match the fixture byte for byte, with no `roles` for a user without any"
    );
    assert_eq!(set.len(), 2, "both users should be in the set");
}

#[test]
fn strings_are_escaped_as_json() {
    let set = UserSet::new(vec![ReviewUser::new(r#"id"\"#, "o'brien", "ü@example.com")]).unwrap();

    let parsed: serde_json::Value = serde_json::from_str(set.body()).unwrap();
    assert_eq!(
        parsed["identities"][0]["identityId"], r#"id"\"#,
        "quotes and backslashes should survive a round trip"
    );
    assert_eq!(
        parsed["identities"][0]["email"], "ü@example.com",
        "non-ASCII text should survive a round trip"
    );
}

#[test]
fn an_empty_set_is_valid_until_it_is_loaded() {
    let set = UserSet::new(Vec::new()).unwrap();

    assert!(set.is_empty(), "an empty set should report itself empty");
    assert_eq!(
        set.body(),
        r#"{"identities":[]}"#,
        "an empty set's body should be an empty list"
    );
}

#[test]
fn invalid_users_are_named_by_position() {
    let cases = [
        (
            vec![alice(), ReviewUser::new("", "x", "x@example.com")],
            (1, UserProblem::EmptyIdentityId),
        ),
        (
            vec![
                alice(),
                bob(),
                ReviewUser::new("U-ALICE", "a2", "a2@example.com"),
            ],
            (2, UserProblem::DuplicateIdentityId { first: 0 }),
        ),
        (
            vec![ReviewUser::new("U-1", "u".repeat(192), "u@example.com")],
            (0, UserProblem::TooLong { field: "username" }),
        ),
        (
            vec![
                bob(),
                alice().with_roles((0..301).map(|n| format!("role-{n}"))),
            ],
            (1, UserProblem::TooManyRoles { count: 301 }),
        ),
    ];
    for (users, expected) in cases {
        let error = UserSet::new(users).unwrap_err();
        assert_eq!(
            problem(&error),
            Some(expected.clone()),
            "the set should be refused as {expected:?}"
        );
        let shown = format!("{error} {error:?}");
        assert!(
            !shown.contains(RECORD_EMAIL) && !shown.contains("alice"),
            "the error should not quote a user: {shown}"
        );
    }
}

#[test]
fn a_191_character_field_is_accepted() {
    let users = vec![ReviewUser::new("U-1", "u".repeat(191), "u@example.com")];

    assert!(
        UserSet::new(users).is_ok(),
        "Drata's limit is 191 characters, inclusive"
    );
}

#[test]
fn more_than_4000_users_are_refused() {
    let users: Vec<_> = (0..=MAX_USERS)
        .map(|n| ReviewUser::new(format!("U-{n}"), format!("user{n}"), "u@example.com"))
        .collect();

    let error = UserSet::new(users).unwrap_err();
    assert!(
        matches!(error, Error::TooManyUsers { count: 4_001 }),
        "4,001 users should be refused, got {error:?}"
    );
}

#[test]
fn a_body_over_5_mb_is_refused() {
    let roles: Vec<String> = (0..300).map(|n| format!("{n:0>190}")).collect();
    let users: Vec<_> = (0..100)
        .map(|n| ReviewUser::new(format!("U-{n}"), "u", "u@example.com").with_roles(roles.clone()))
        .collect();

    let error = UserSet::new(users).unwrap_err();
    assert!(
        matches!(error, Error::UserSetTooLarge { bytes } if bytes > 5_000_000),
        "a body over 5,000,000 bytes should be refused, got {error:?}"
    );
}

#[test]
fn debug_output_shows_counts_and_no_user_details() {
    let user = alice();
    let set = UserSet::new(vec![alice(), bob()]).unwrap();

    let shown = format!("{user:?} {set:?}");
    for detail in ["U-ALICE", "alice", RECORD_EMAIL, "Owner"] {
        assert!(
            !shown.contains(detail),
            "Debug should not show `{detail}`: {shown}"
        );
    }
    assert!(
        shown.contains("roles: 2") && shown.contains("users: 2"),
        "Debug should show counts: {shown}"
    );
}
