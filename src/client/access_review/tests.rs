//! The Access Review calls against a scripted transport.

use super::UsersLoaded;
use crate::{
    ApplicationId, EmptySnapshot, Error, ReviewUser, UserSet,
    testing::{KEY, RECORD_EMAIL, Scripted, answer, client, every_rendering, run, sent_body},
    transport::Method,
};

const APPLICATION: ApplicationId = ApplicationId::new(12);
const USERS_URL: &str = "https://drata.test/v2/access-review-applications/12/users";

fn users() -> UserSet {
    UserSet::new(vec![
        ReviewUser::new("U-ALICE", "alice", RECORD_EMAIL).with_roles(["Owner"]),
        ReviewUser::new("U-BOB", "bob", "bob@example.com"),
    ])
    .unwrap()
}

#[test]
fn a_load_puts_the_dry_run_body_and_reads_the_saved_count() {
    let transport = Scripted::new([answer(200, r#"{"savedCount":2}"#)]);
    let set = users();

    let loaded =
        run(client(&transport).load_access_review_users(APPLICATION, &set, EmptySnapshot::Refuse))
            .unwrap();

    assert_eq!(
        loaded,
        UsersLoaded::Saved { count: 2 },
        "Drata's savedCount should come back"
    );
    let seen = transport.seen();
    assert_eq!(seen.len(), 1, "a load should be one request");
    assert_eq!(seen[0].method, Method::Put, "a load should be a PUT");
    assert_eq!(seen[0].url, USERS_URL, "a load should go to the users path");
    assert_eq!(
        seen[0].authorization,
        format!("Bearer {KEY}"),
        "the key should be sent as a bearer token"
    );
    assert_eq!(
        sent_body(&transport, 0),
        set.body(),
        "the body sent should be the dry run byte for byte"
    );
}

#[test]
fn a_409_is_a_locked_review_not_an_error_and_is_not_retried() {
    let transport = Scripted::new([answer(
        409,
        r#"{"name":"ConflictException","statusCode":409,"code":0,
            "message":"This Application has already been completed while its review period is still open, or reviewers have already recorded decisions in it."}"#,
    )]);

    let loaded = run(client(&transport).load_access_review_users(
        APPLICATION,
        &users(),
        EmptySnapshot::Refuse,
    ))
    .unwrap();

    assert_eq!(
        loaded,
        UsersLoaded::ReviewLocked,
        "a 409 should say the review is locked"
    );
    assert_eq!(transport.seen().len(), 1, "a 409 should not be retried");
    assert!(transport.slept().is_empty(), "a 409 should not wait");
}

#[test]
fn other_refusals_stay_errors_and_quote_no_user() {
    let transport = Scripted::new([answer(
        400,
        r#"{"name":"BadRequestException","statusCode":400,"code":1,"message":"identities must be an array"}"#,
    )]);

    let error = run(client(&transport).load_access_review_users(
        APPLICATION,
        &users(),
        EmptySnapshot::Refuse,
    ))
    .unwrap_err();

    assert!(
        matches!(error, Error::Rejected { status: 400, .. }),
        "a 400 should be a rejection, got {error:?}"
    );
    let shown = every_rendering(&error);
    for detail in [KEY, RECORD_EMAIL, "U-ALICE", "alice"] {
        assert!(
            !shown.contains(detail),
            "the error should not show `{detail}`: {shown}"
        );
    }
}

#[test]
fn an_empty_set_is_refused_before_any_call_unless_allowed() {
    let empty = UserSet::new(Vec::new()).unwrap();

    let refused = Scripted::new([]);
    let error =
        run(client(&refused).load_access_review_users(APPLICATION, &empty, EmptySnapshot::Refuse))
            .unwrap_err();
    assert!(
        matches!(error, Error::EmptySnapshot),
        "an empty set should be refused, got {error:?}"
    );
    assert!(
        refused.seen().is_empty(),
        "a refused empty set should send nothing"
    );

    let allowed = Scripted::new([answer(200, r#"{"savedCount":0}"#)]);
    let loaded =
        run(client(&allowed).load_access_review_users(APPLICATION, &empty, EmptySnapshot::Allow))
            .unwrap();
    assert_eq!(
        loaded,
        UsersLoaded::Saved { count: 0 },
        "an allowed empty set removes every user"
    );
    assert_eq!(
        sent_body(&allowed, 0),
        r#"{"identities":[]}"#,
        "an allowed empty set should send an empty list"
    );
}

#[test]
fn listing_users_follows_the_cursor_to_the_last_page() {
    let transport = Scripted::new([
        answer(
            200,
            r#"{"data":[{"id":1,"identityId":"U-ALICE","username":"alice","email":"alice@example.com",
                         "roles":["Owner"],"hasAdminAccess":true,
                         "createdAt":"2026-11-11T00:00:00.000Z","updatedAt":"2026-11-11T00:00:00.000Z"}],
               "pagination":{"cursor":"next/page=","totalCount":null}}"#,
        ),
        answer(
            200,
            r#"{"data":[{"id":2,"identityId":null,"username":null,"email":null,
                         "roles":[],"hasAdminAccess":false,
                         "createdAt":"2026-11-11T00:00:00.000Z","updatedAt":"2026-11-11T00:00:00.000Z"}],
               "pagination":{"cursor":null}}"#,
        ),
    ]);

    let listed = run(client(&transport).access_review_users(APPLICATION)).unwrap();

    let ids: Vec<_> = listed.iter().map(|user| user.id).collect();
    assert_eq!(
        ids,
        [1, 2],
        "users from both pages should come back in order"
    );
    assert_eq!(
        listed[0].identity_id.as_deref(),
        Some("U-ALICE"),
        "the identity ID should be read"
    );
    assert!(listed[0].has_admin_access, "admin access should be read");
    assert_eq!(
        listed[1].email, None,
        "a disconnected user's missing email should be read as None"
    );
    let urls: Vec<_> = transport.seen().into_iter().map(|seen| seen.url).collect();
    assert_eq!(
        urls,
        [
            format!("{USERS_URL}?size=500"),
            format!("{USERS_URL}?size=500&cursor=next%2Fpage%3D"),
        ],
        "the second request should carry the encoded cursor"
    );

    let shown = format!("{listed:?}");
    for detail in ["U-ALICE", "alice", "Owner"] {
        assert!(
            !shown.contains(detail),
            "Debug should not show `{detail}`: {shown}"
        );
    }
}
