//! `sync` against a scripted transport: the session's whole life.

use serde::Serialize;

use super::{EmptySnapshot, Synced};
use crate::{
    Batch, ConnectionId, CustomRecord, Error, ResourceId,
    testing::{Scripted, answer, client, run, run_until_stalled, sent_body},
    transport::Method,
};

const CONNECTION: ConnectionId = ConnectionId::new(42);
const RESOURCE: ResourceId = ResourceId::new(7);
const BASE: &str = "https://drata.test/v2/custom-connections/42/resources/7";
const SESSION: &str = "sync-test";

#[derive(Serialize)]
struct Member {
    email: String,
    role: String,
}

impl CustomRecord for Member {
    const DISPLAY_NAME_KEY: &'static str = "email";

    fn key(&self) -> String {
        self.email.clone()
    }
}

fn batch(members: &[(&str, &str)]) -> Batch {
    let members: Vec<Member> = members
        .iter()
        .map(|(email, role)| Member {
            email: (*email).to_owned(),
            role: (*role).to_owned(),
        })
        .collect();
    Batch::new(&members).unwrap()
}

const NO_SESSIONS: &str = r#"{"data":[],"pagination":{"cursor":null}}"#;
const COMPLETED: &str = r#"{"sessionId":"sync-test","status":"ACTIVE","action":"complete","connectionId":42,"resourceId":7}"#;
const CANCELLED: &str = r#"{"sessionId":"sync-test","status":"CANCELED","action":"cancel","connectionId":42,"resourceId":7}"#;

/// The live records before the sync: a is unchanged, b changes role, c leaves.
const LIVE: &str = r#"{"data":[
    {"id":"a","attributes":{"email":"a","role":"admin","id":"a"},"sessionId":"old","createdAt":"x","updatedAt":"x"},
    {"id":"b","attributes":{"email":"b","role":"member"},"sessionId":"old","createdAt":"x","updatedAt":"x"},
    {"id":"c","attributes":{"email":"c","role":"member"},"sessionId":"old","createdAt":"x","updatedAt":"x"}
], "pagination":{"cursor":null}}"#;

const UPLOADED: &str =
    r#"[{"statusCode":201,"data":{}},{"statusCode":201,"data":{}},{"statusCode":201,"data":{}}]"#;

fn snapshot() -> Batch {
    batch(&[("a", "admin"), ("b", "admin"), ("d", "member")])
}

fn requests(transport: &Scripted) -> Vec<(Method, String)> {
    transport
        .seen()
        .into_iter()
        .map(|seen| (seen.method, seen.url.replace(BASE, "")))
        .collect()
}

#[test]
fn a_sync_uploads_into_a_session_then_completes_it() {
    let transport = Scripted::new([
        answer(200, NO_SESSIONS),
        answer(200, LIVE),
        answer(200, UPLOADED),
        answer(200, COMPLETED),
    ]);
    let client = client(&transport);
    let synced = run(client.sync_in_session(
        CONNECTION,
        RESOURCE,
        &snapshot(),
        EmptySnapshot::Refuse,
        SESSION,
    ))
    .unwrap();

    assert_eq!(
        synced,
        Synced {
            created: 1,
            updated: 1,
            unchanged: 1,
            deleted: 1,
            abandoned_sessions_cancelled: 0,
        },
        "d is new, b changed, a is the same and c left"
    );
    assert_eq!(
        requests(&transport),
        [
            (
                Method::Get,
                "/sessions?size=500&status=IN_PROGRESS".to_owned()
            ),
            (Method::Get, "/records?size=500".to_owned()),
            (Method::Post, "/sessions/sync-test".to_owned()),
            (Method::Post, "/sessions/sync-test/actions".to_owned()),
        ],
        "list sessions, list records, upload, complete"
    );
    assert_eq!(
        sent_body(&transport, 3),
        r#"{"action":"complete"}"#,
        "the last call should complete the session"
    );
}

#[test]
fn a_session_left_by_a_crashed_run_is_cancelled_first() {
    let transport = Scripted::new([
        answer(
            200,
            r#"{"data":[{"id":1,"sessionId":"sync-old","status":"IN_PROGRESS","createdAt":"x","updatedAt":"x","activatedAt":null,"canceledAt":null}],"pagination":{"cursor":null}}"#,
        ),
        answer(200, CANCELLED),
        answer(200, r#"{"data":[],"pagination":{"cursor":null}}"#),
        answer(200, UPLOADED),
        answer(200, COMPLETED),
    ]);
    let client = client(&transport);
    let synced = run(client.sync_in_session(
        CONNECTION,
        RESOURCE,
        &snapshot(),
        EmptySnapshot::Refuse,
        SESSION,
    ))
    .unwrap();

    assert_eq!(
        synced.abandoned_sessions_cancelled, 1,
        "the old session should be cancelled"
    );
    assert_eq!(
        synced.created, 3,
        "every record is new to an empty resource"
    );
    assert_eq!(
        requests(&transport)[1],
        (Method::Post, "/sessions/sync-old/actions".to_owned()),
        "the old session should be cancelled before anything is uploaded"
    );
    assert_eq!(
        sent_body(&transport, 1),
        r#"{"action":"cancel"}"#,
        "with a cancel"
    );
}

#[test]
fn a_refused_record_cancels_the_session_and_never_completes_it() {
    let transport = Scripted::new([
        answer(200, NO_SESSIONS),
        answer(200, LIVE),
        answer(
            200,
            r#"[{"statusCode":201,"data":{}},{"statusCode":400,"data":{},"error":{"message":"Schema validation failed","code":28022}},{"statusCode":201,"data":{}}]"#,
        ),
        answer(200, CANCELLED),
    ]);
    let client = client(&transport);
    let error = run(client.sync_in_session(
        CONNECTION,
        RESOURCE,
        &snapshot(),
        EmptySnapshot::Refuse,
        SESSION,
    ))
    .unwrap_err();

    assert!(
        matches!(error, Error::RecordsRejected { .. }),
        "a refused record should fail the sync, got {error:?}"
    );
    assert_eq!(
        sent_body(&transport, 3),
        r#"{"action":"cancel"}"#,
        "the session should be cancelled, not completed"
    );
    assert_eq!(
        transport.seen().len(),
        4,
        "nothing should follow the cancel"
    );
}

#[test]
fn a_failed_upload_reports_its_own_error_even_if_cancelling_fails() {
    let transport = Scripted::new([
        answer(200, NO_SESSIONS),
        answer(200, LIVE),
        answer(400, r#"{"message":"bad body","code":1}"#),
        answer(404, r#"{"message":"no such session"}"#),
    ]);
    let client = client(&transport);
    let error = run(client.sync_in_session(
        CONNECTION,
        RESOURCE,
        &snapshot(),
        EmptySnapshot::Refuse,
        SESSION,
    ))
    .unwrap_err();

    assert!(
        matches!(error, Error::Rejected { status: 400, .. }),
        "the upload's 400 should be reported, not the cancel's 404, got {error:?}"
    );
}

#[test]
fn a_sync_dropped_mid_upload_never_completes_its_session() {
    let transport = Scripted::hanging_from(2, [answer(200, NO_SESSIONS), answer(200, LIVE)]);
    let client = client(&transport);
    let finished = run_until_stalled(client.sync_in_session(
        CONNECTION,
        RESOURCE,
        &snapshot(),
        EmptySnapshot::Refuse,
        SESSION,
    ));

    assert!(
        finished.is_none(),
        "the sync should be dropped while uploading"
    );
    assert!(
        transport
            .seen()
            .iter()
            .all(|seen| !seen.url.ends_with("/actions")),
        "a dropped sync should never complete, or even act on, its session"
    );
}

#[test]
fn a_session_that_does_not_become_active_is_an_error() {
    let transport = Scripted::new([
        answer(200, NO_SESSIONS),
        answer(200, LIVE),
        answer(200, UPLOADED),
        answer(
            200,
            r#"{"sessionId":"sync-test","status":"IN_PROGRESS","action":"complete","connectionId":42,"resourceId":7}"#,
        ),
    ]);
    let client = client(&transport);
    let error = run(client.sync_in_session(
        CONNECTION,
        RESOURCE,
        &snapshot(),
        EmptySnapshot::Refuse,
        SESSION,
    ))
    .unwrap_err();

    assert!(
        matches!(&error, Error::SessionNotActive { status } if status == "IN_PROGRESS"),
        "got {error:?}"
    );
}

#[test]
fn an_empty_snapshot_is_refused_before_any_call_unless_allowed() {
    let empty = Batch::new::<Member>(&[]).unwrap();
    let refused_transport = Scripted::new([]);
    let refused = run(client(&refused_transport).sync_in_session(
        CONNECTION,
        RESOURCE,
        &empty,
        EmptySnapshot::Refuse,
        SESSION,
    ))
    .unwrap_err();
    assert!(matches!(refused, Error::EmptySnapshot), "got {refused:?}");
    assert!(
        refused_transport.seen().is_empty(),
        "nothing should be sent"
    );

    let allowed_transport = Scripted::new([
        answer(200, NO_SESSIONS),
        answer(200, LIVE),
        answer(200, COMPLETED),
    ]);
    let synced = run(client(&allowed_transport).sync_in_session(
        CONNECTION,
        RESOURCE,
        &empty,
        EmptySnapshot::Allow,
        SESSION,
    ))
    .unwrap();
    assert_eq!(
        synced.deleted, 3,
        "an allowed empty snapshot deletes every record"
    );
}
