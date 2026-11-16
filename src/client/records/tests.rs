//! The record calls against a scripted transport.

use serde::Serialize;

use super::encode_component;
use crate::{
    Batch, ConnectionId, CustomRecord, DrataMessage, Error, RecordFailure, ResourceId, Upserted,
    testing::{RECORD_EMAIL, Scripted, answer, client, every_rendering, run, sent_body},
    transport::Method,
};

const CONNECTION: ConnectionId = ConnectionId::new(42);
const RESOURCE: ResourceId = ResourceId::new(7);
const RECORDS_URL: &str = "https://drata.test/v2/custom-connections/42/resources/7/records";

#[derive(Serialize)]
struct Member {
    email: String,
}

impl CustomRecord for Member {
    const DISPLAY_NAME_KEY: &'static str = "email";

    fn key(&self) -> String {
        self.email.clone()
    }
}

fn batch(emails: &[&str]) -> Batch {
    let members: Vec<Member> = emails
        .iter()
        .map(|email| Member {
            email: (*email).to_owned(),
        })
        .collect();
    Batch::new(&members).unwrap()
}

#[test]
fn list_records_follows_the_cursor_to_the_last_page() {
    let transport = Scripted::new([
        answer(
            200,
            r#"{"data":[{"id":"a","attributes":{"email":"a"},"sessionId":null,"createdAt":"x","updatedAt":"x"}],
               "pagination":{"cursor":"page/2+="}}"#,
        ),
        answer(
            200,
            r#"{"data":[{"id":"b","attributes":{"email":"b"},"sessionId":"s-1","createdAt":"x","updatedAt":"x"}],
               "pagination":{"cursor":null}}"#,
        ),
    ]);
    let records = run(client(&transport).list_records(CONNECTION, RESOURCE)).unwrap();

    let ids: Vec<_> = records.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        ["a", "b"],
        "records from both pages should be returned in order"
    );
    assert_eq!(
        records[1].session_id.as_deref(),
        Some("s-1"),
        "the session should be read"
    );
    let urls: Vec<_> = transport.seen().into_iter().map(|seen| seen.url).collect();
    assert_eq!(
        urls,
        [
            format!("{RECORDS_URL}?size=500"),
            format!("{RECORDS_URL}?size=500&cursor=page%2F2%2B%3D"),
        ],
        "the second request should carry the encoded cursor"
    );
}

#[test]
fn a_stored_records_debug_omits_its_attributes() {
    let transport = Scripted::new([answer(
        200,
        &format!(
            r#"{{"data":[{{"id":"u-1","attributes":{{"email":"{RECORD_EMAIL}"}},"sessionId":null,"createdAt":"x","updatedAt":"x"}}],"pagination":{{"cursor":null}}}}"#
        ),
    )]);
    let records = run(client(&transport).list_records(CONNECTION, RESOURCE)).unwrap();

    assert!(
        !format!("{records:?}").contains(RECORD_EMAIL),
        "Debug should not print a record's attributes"
    );
}

#[test]
fn upsert_sends_the_batch_and_counts_created_and_updated() {
    let transport = Scripted::new([answer(
        200,
        r#"[{"id":"a","statusCode":201,"data":{}},{"id":"b","statusCode":200,"data":{}}]"#,
    )]);
    let client = client(&transport);
    let upserted = run(client
        .direct_records(CONNECTION, RESOURCE)
        .upsert(&batch(&["a", "b"])))
    .unwrap();

    assert_eq!(
        upserted,
        Upserted {
            created: 1,
            updated: 1
        },
        "one 201 and one 200 should count as one created and one updated"
    );
    let seen = transport.seen();
    assert_eq!(seen[0].method, Method::Post, "an upsert is a POST");
    assert_eq!(seen[0].url, RECORDS_URL, "to the resource's records");
    assert_eq!(
        sent_body(&transport, 0),
        r#"{"data":[{"email":"a","id":"a"},{"email":"b","id":"b"}]}"#,
        "the body should be the batch's dry-run body"
    );
}

#[test]
fn a_record_refused_inside_a_200_is_an_error() {
    let refusal = format!("Schema validation failed: `{RECORD_EMAIL}` is not allowed");
    let transport = Scripted::new([answer(
        200,
        &format!(
            r#"[{{"id":"a","statusCode":201,"data":{{}}}},
                {{"statusCode":400,"data":{{}},"error":{{"error":{{"message":"{refusal}","code":28022}}}}}}]"#
        ),
    )]);
    let client = client(&transport);
    let error = run(client
        .direct_records(CONNECTION, RESOURCE)
        .upsert(&batch(&[RECORD_EMAIL, "b"])))
    .unwrap_err();

    let Error::RecordsRejected { failures } = &error else {
        panic!("a refused record should be RecordsRejected, got {error:?}");
    };
    assert_eq!(
        failures.as_slice(),
        [RecordFailure {
            index: 1,
            status: 400,
            code: Some(28022),
            message: Some(DrataMessage::new(refusal.clone())),
        }],
        "the failure should name the record's position, status, code and message"
    );
    assert_eq!(
        error.drata_message().map(DrataMessage::as_str),
        Some(refusal.as_str()),
        "Drata's message should be readable on request"
    );
    let shown = every_rendering(&error);
    assert!(
        !shown.contains(RECORD_EMAIL),
        "the error should not quote a record, even through Drata's message: {shown}"
    );
    assert!(
        shown.contains("the first at 1 with status 400 (code 28022)"),
        "the error should name the first failure's position, status and code: {shown}"
    );
}

#[test]
fn delete_encodes_the_record_id_into_the_path() {
    let transport = Scripted::new([answer(204, "")]);
    let client = client(&transport);
    run(client
        .direct_records(CONNECTION, RESOURCE)
        .delete("alice@example.com"))
    .unwrap();

    let seen = transport.seen();
    assert_eq!(seen[0].method, Method::Delete, "a delete is a DELETE");
    assert_eq!(
        seen[0].url,
        format!("{RECORDS_URL}/alice%40example.com"),
        "the id should be percent-encoded"
    );
}

#[test]
fn encoding_keeps_only_unreserved_characters() {
    assert_eq!(
        encode_component("aZ09-._~ /+=@é"),
        "aZ09-._~%20%2F%2B%3D%40%C3%A9",
        "everything but RFC 3986's unreserved characters should be encoded"
    );
}
