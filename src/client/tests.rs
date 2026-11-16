//! The client against a scripted transport: errors, redaction and retries.

use std::time::Duration;

use super::{ConnectionId, RetryPolicy};
use crate::{
    DrataMessage, Error,
    testing::{KEY, RECORD_EMAIL, Scripted, Seen, answer, client, every_rendering, no_answer, run},
    transport::{Method, Response},
};

const CONNECTION: &str = r#"{
    "id": 42,
    "clientAlias": "PostHog",
    "description": null,
    "createdAt": "2026-11-11T00:00:00Z",
    "updatedAt": "2026-11-11T00:00:00Z",
    "customResources": [
        {"id": 7, "name": "members", "schema": {}, "createdAt": "x", "updatedAt": "x"},
        {"id": 8, "name": "settings", "schema": {}, "createdAt": "x", "updatedAt": "x"}
    ]
}"#;

#[test]
fn reads_a_connection_and_its_resources() {
    let transport = Scripted::new([answer(200, CONNECTION)]);
    let connection = run(client(&transport).connection(ConnectionId::new(42))).unwrap();

    assert_eq!(
        connection.id.get(),
        42,
        "the connection's id should be read"
    );
    assert_eq!(
        connection.client_alias, "PostHog",
        "the alias should be read"
    );
    let names: Vec<_> = connection
        .resources
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["members", "settings"],
        "both resources should be read"
    );
    assert_eq!(
        transport.seen(),
        [Seen {
            method: Method::Get,
            url: "https://drata.test/v2/custom-connections/42?expand[]=customResources".into(),
            authorization: format!("Bearer {KEY}"),
            body: None,
        }],
        "one GET with the key as a bearer token should be sent"
    );
}

#[test]
fn the_key_appears_in_no_error_or_debug_output() {
    let refused = r#"{"name":"Unauthorized","statusCode":401,"message":"invalid key","code":1}"#;
    let transport = Scripted::new([answer(401, refused)]);
    let client = client(&transport);
    let error = run(client.connection(ConnectionId::new(42))).unwrap_err();

    assert!(
        matches!(error, Error::Unauthorized { status: 401, .. }),
        "a 401 should be Unauthorized, got {error:?}"
    );
    let printed = format!("{}\n{client:?}", every_rendering(&error));
    assert!(
        !printed.contains(KEY),
        "the key should be in no rendering of the error or the client"
    );
}

#[test]
fn a_record_body_appears_in_no_error() {
    let transport = Scripted::new([answer(
        400,
        r#"{"name":"BadRequest","statusCode":400,"message":"Schema validation failed","code":28022}"#,
    )]);
    let body = format!(r#"{{"data":{{"email":"{RECORD_EMAIL}"}}}}"#).into_bytes();
    let error = run(client(&transport).call(Method::Post, "/records", Some(body))).unwrap_err();

    assert!(
        matches!(
            error,
            Error::Rejected {
                status: 400,
                code: Some(28022),
                ..
            }
        ),
        "a 400 should be Rejected with Drata's code, got {error:?}"
    );
    assert!(
        !every_rendering(&error).contains(RECORD_EMAIL),
        "the request body should be in no rendering of the error"
    );
}

#[test]
fn drata_messages_appear_in_no_error_output_but_can_be_read() {
    let quoted = format!("`{RECORD_EMAIL}` is not a valid value");
    let body = format!(r#"{{"message":"{quoted}","code":7}}"#);
    for (status, expected) in [(403, "Unauthorized"), (400, "Rejected"), (502, "Server")] {
        let transport = Scripted::new((0..4).map(|_| answer(status, &body)));
        let error = run(client(&transport).connection(ConnectionId::new(42))).unwrap_err();

        let variant = match &error {
            Error::Unauthorized { .. } => "Unauthorized",
            Error::Rejected { .. } => "Rejected",
            Error::Server { .. } => "Server",
            _ => "another variant",
        };
        assert_eq!(variant, expected, "HTTP {status} should be {expected}");
        assert_eq!(
            error.drata_message().map(DrataMessage::as_str),
            Some(quoted.as_str()),
            "HTTP {status}: Drata's message should be readable on request"
        );
        let shown = every_rendering(&error);
        assert!(
            !shown.contains(RECORD_EMAIL),
            "HTTP {status}: no rendering should show Drata's message: {shown}"
        );
        assert!(
            shown.contains(&format!("DrataMessage({} chars)", quoted.chars().count())),
            "HTTP {status}: Debug should show the message's length: {shown}"
        );
    }
}

#[test]
fn a_decoding_failure_does_not_quote_the_answer() {
    let transport = Scripted::new([answer(200, &format!(r#"{{"id":"{RECORD_EMAIL}"}}"#))]);
    let error = run(client(&transport).connection(ConnectionId::new(42))).unwrap_err();

    assert!(
        matches!(error, Error::Decode { .. }),
        "an answer that is not a connection should be Decode, got {error:?}"
    );
    assert!(
        !every_rendering(&error).contains(RECORD_EMAIL),
        "a decoding error should not quote a value from the answer"
    );
}

#[test]
fn each_status_maps_to_its_variant() {
    type Expected = fn(&Error) -> bool;
    let cases: [(u16, Expected); 6] = [
        (403, |e| {
            matches!(e, Error::Unauthorized { status: 403, .. })
        }),
        (402, |e| matches!(e, Error::Rejected { status: 402, .. })),
        (404, |e| matches!(e, Error::Rejected { status: 404, .. })),
        (413, |e| matches!(e, Error::PayloadTooLarge)),
        (422, |e| matches!(e, Error::Rejected { status: 422, .. })),
        (302, |e| {
            matches!(e, Error::UnexpectedStatus { status: 302 })
        }),
    ];
    for (status, expected) in cases {
        let transport = Scripted::new([answer(status, "not json")]);
        let error = run(client(&transport).connection(ConnectionId::new(1))).unwrap_err();
        assert!(expected(&error), "HTTP {status} mapped to {error:?}");
        assert_eq!(
            transport.seen().len(),
            1,
            "HTTP {status} should not be retried"
        );
    }
}

#[test]
fn a_429_waits_for_retry_after_then_succeeds() {
    let transport = Scripted::new([
        Ok(Response {
            status: 429,
            retry_after: Some(Duration::from_secs(7)),
            body: Vec::new(),
        }),
        answer(200, CONNECTION),
    ]);
    run(client(&transport).connection(ConnectionId::new(42))).unwrap();

    assert_eq!(
        transport.slept(),
        [Duration::from_secs(7)],
        "the client should wait exactly Retry-After"
    );
}

#[test]
fn retry_after_is_capped_by_the_policy() {
    let transport = Scripted::new([
        Ok(Response {
            status: 429,
            retry_after: Some(Duration::from_secs(3600)),
            body: Vec::new(),
        }),
        answer(200, CONNECTION),
    ]);
    run(client(&transport).connection(ConnectionId::new(42))).unwrap();

    assert_eq!(
        transport.slept(),
        [Duration::from_secs(30)],
        "a long Retry-After should be capped at the policy's maximum"
    );
}

#[test]
fn server_errors_and_lost_answers_back_off_then_succeed() {
    let transport = Scripted::new([
        answer(503, ""),
        no_answer(),
        answer(500, ""),
        answer(200, CONNECTION),
    ]);
    run(client(&transport).connection(ConnectionId::new(42))).unwrap();

    assert_eq!(
        transport.slept(),
        [1, 2, 4].map(Duration::from_secs),
        "the backoff should double from one second"
    );
}

#[test]
fn a_persistent_5xx_fails_after_every_retry() {
    let transport = Scripted::new((0..4).map(|_| answer(502, r#"{"message":"bad gateway"}"#)));
    let error = run(client(&transport).connection(ConnectionId::new(42))).unwrap_err();

    assert!(
        matches!(error, Error::Server { status: 502, .. }),
        "a persistent 502 should be Server, got {error:?}"
    );
    assert_eq!(transport.seen().len(), 4, "one attempt and three retries");
}

#[test]
fn a_persistent_429_is_rate_limited() {
    let transport = Scripted::new((0..4).map(|_| answer(429, "")));
    let error = run(client(&transport).connection(ConnectionId::new(42))).unwrap_err();

    assert!(
        matches!(error, Error::RateLimited { .. }),
        "a persistent 429 should be RateLimited, got {error:?}"
    );
}

#[test]
fn no_answer_at_all_is_a_transport_error() {
    let transport = Scripted::new([no_answer()]);
    let error = run(client(&transport)
        .with_retry(RetryPolicy::none())
        .connection(ConnectionId::new(42)))
    .unwrap_err();

    assert!(
        matches!(error, Error::Transport(_)),
        "no answer should be Transport, got {error:?}"
    );
    assert!(
        transport.slept().is_empty(),
        "no retry policy means no wait"
    );
}
