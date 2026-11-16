//! Batches: validation, the dry run and splitting under Drata's limit.

use serde::Serialize;

use super::{Batch, CustomRecord, MAX_BODY_BYTES, RecordProblem};
use crate::{Error, testing::RECORD_EMAIL};

/// A record shaped like a connector's members: nested roles and groups.
#[derive(Serialize)]
struct Member {
    email: String,
    name: String,
    role: String,
    groups: Vec<Group>,
}

#[derive(Serialize)]
struct Group {
    name: String,
    role: String,
}

impl CustomRecord for Member {
    const DISPLAY_NAME_KEY: &'static str = "email";

    fn key(&self) -> String {
        self.email.clone()
    }
}

fn member(email: &str) -> Member {
    Member {
        email: email.to_owned(),
        name: "Alice Example".to_owned(),
        role: "admin".to_owned(),
        groups: vec![Group {
            name: "Owners".to_owned(),
            role: "member".to_owned(),
        }],
    }
}

fn problem(error: &Error) -> Option<(usize, RecordProblem)> {
    match error {
        Error::InvalidRecord { index, problem } => Some((*index, problem.clone())),
        _ => None,
    }
}

#[test]
fn the_dry_run_matches_the_committed_fixture() {
    let batch = Batch::new(&[member(RECORD_EMAIL), member("bob@example.com")]).unwrap();

    assert_eq!(
        batch.bodies(),
        [include_str!("fixtures/members.body.json").trim_end()],
        "the one body should match the fixture byte for byte"
    );
    assert_eq!(
        batch.sample_data().unwrap(),
        include_str!("fixtures/members.sample.json").trim_end(),
        "the sample data should be the first record, pretty-printed"
    );
    assert_eq!(batch.len(), 2, "both records should be in the batch");
}

#[test]
fn a_record_keeps_an_id_field_equal_to_its_key() {
    #[derive(Serialize)]
    struct WithId {
        id: String,
        name: String,
    }
    impl CustomRecord for WithId {
        const DISPLAY_NAME_KEY: &'static str = "name";
        fn key(&self) -> String {
            self.id.clone()
        }
    }

    struct Rekeyed(WithId);
    impl Serialize for Rekeyed {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.serialize(serializer)
        }
    }
    impl CustomRecord for Rekeyed {
        const DISPLAY_NAME_KEY: &'static str = "name";
        fn key(&self) -> String {
            "u-2".to_owned()
        }
    }

    let same = WithId {
        id: "u-1".to_owned(),
        name: "Alice".to_owned(),
    };
    let body = Batch::new(&[same]).unwrap().bodies().join("");
    assert_eq!(
        body, r#"{"data":[{"id":"u-1","name":"Alice"}]}"#,
        "the id should be kept"
    );

    let differs = WithId {
        id: "u-1".to_owned(),
        name: "Alice".to_owned(),
    };
    let error = Batch::new(&[Rekeyed(differs)]).unwrap_err();
    assert_eq!(
        problem(&error),
        Some((0, RecordProblem::IdDiffersFromKey)),
        "an id that differs from the key should be refused"
    );
}

#[test]
fn invalid_records_are_refused_by_position_without_their_contents() {
    let mut no_name = member(RECORD_EMAIL);
    no_name.email = String::new();
    let cases = [
        (
            Batch::new(&[member(RECORD_EMAIL), member(RECORD_EMAIL)]),
            (1, RecordProblem::DuplicateKey { first: 0 }),
        ),
        (Batch::new(&[no_name]), (0, RecordProblem::EmptyKey)),
    ];
    for (outcome, expected) in cases {
        let error = outcome.unwrap_err();
        assert_eq!(problem(&error), Some(expected), "got {error:?}");
        assert!(
            !format!("{error}{error:?}").contains(RECORD_EMAIL),
            "the error should not quote the record"
        );
    }
}

#[test]
fn a_record_must_be_an_object_with_its_display_name() {
    #[derive(Serialize)]
    struct Bare(String);
    impl CustomRecord for Bare {
        const DISPLAY_NAME_KEY: &'static str = "email";
        fn key(&self) -> String {
            self.0.clone()
        }
    }
    #[derive(Serialize)]
    struct NoEmail {
        name: String,
    }
    impl CustomRecord for NoEmail {
        const DISPLAY_NAME_KEY: &'static str = "email";
        fn key(&self) -> String {
            self.name.clone()
        }
    }

    let bare = Batch::new(&[Bare(RECORD_EMAIL.to_owned())]).unwrap_err();
    assert_eq!(
        problem(&bare),
        Some((0, RecordProblem::NotAnObject)),
        "got {bare:?}"
    );
    let missing = Batch::new(&[NoEmail {
        name: "Alice".to_owned(),
    }])
    .unwrap_err();
    assert_eq!(
        problem(&missing),
        Some((0, RecordProblem::DisplayName("email"))),
        "got {missing:?}"
    );
}

#[test]
fn bodies_are_split_under_the_limit_and_keep_every_record() {
    let padding = "x".repeat(400_000);
    let members: Vec<Member> = (0..30)
        .map(|n| {
            let mut m = member(&format!("user{n}@example.com"));
            m.name.clone_from(&padding);
            m
        })
        .collect();
    let batch = Batch::new(&members).unwrap();

    assert!(
        batch.bodies().len() > 1,
        "30 records of 400 KB should need several bodies"
    );
    for body in batch.bodies() {
        assert!(
            body.len() <= MAX_BODY_BYTES,
            "a body of {} bytes is over the limit",
            body.len()
        );
    }
    let sent: usize = batch
        .bodies()
        .iter()
        .map(|body| {
            let value: serde_json::Value = serde_json::from_str(body).unwrap();
            value["data"].as_array().map_or(0, Vec::len)
        })
        .sum();
    assert_eq!(sent, 30, "every record should be in exactly one body");
}

#[test]
fn a_record_too_large_alone_is_refused() {
    let mut huge = member(RECORD_EMAIL);
    huge.name = "x".repeat(MAX_BODY_BYTES);
    let error = Batch::new(&[huge]).unwrap_err();

    assert!(
        matches!(
            problem(&error),
            Some((0, RecordProblem::TooLarge { bytes })) if bytes > MAX_BODY_BYTES
        ),
        "got {error:?}"
    );
}

#[test]
fn debug_shows_counts_only() {
    let batch = Batch::new(&[member(RECORD_EMAIL)]).unwrap();
    assert_eq!(
        format!("{batch:?}"),
        "Batch { records: 1, bodies: 1 }",
        "Debug should not print records"
    );
}
