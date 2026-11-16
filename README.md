# drata-client

A Rust client for [Drata](https://drata.com)'s custom connections and Access
Review APIs. A connector reads its own system, turns what it finds into records,
and hands them to this crate, which keeps a Drata custom resource holding
exactly those records. Built and used by [WealthReach](https://wealthreach.ai)
for its own Drata connectors.

This file is also the crate's documentation, so the examples below compile as
doc tests.

## Using it

```toml
[dependencies]
drata-client = "0.1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt"] }
```

Implement `CustomRecord` on the record type, build a `Batch`, print it as the
dry run, then `sync`:

```rust,no_run
use drata_client::{
    ApiKey, Batch, Client, ConnectionId, CustomRecord, EmptySnapshot, Region, ResourceId,
};
use serde::Serialize;

#[derive(Serialize)]
struct Member {
    email: String,
    role: String,
    groups: Vec<String>,
}

impl CustomRecord for Member {
    // The field Drata labels each record with: the resource's displayNameKey.
    const DISPLAY_NAME_KEY: &'static str = "email";

    // A stable identity, sent as the record's `id`.
    fn key(&self) -> String {
        self.email.clone()
    }
}

async fn push(members: &[Member], drata_key: String) -> drata_client::Result<()> {
    // Validates every record and builds the exact request bodies. No key, no
    // call: this is the dry run, and its first record is the resource's
    // sample data in Drata.
    let batch = Batch::new(members)?;
    for body in batch.bodies() {
        println!("{body}");
    }

    // The caller reads the key from wherever it keeps secrets.
    let client = Client::native(ApiKey::new(drata_key), Region::Us)?;
    let synced = client
        .sync(
            ConnectionId::new(42),
            ResourceId::new(7),
            &batch,
            EmptySnapshot::Refuse,
        )
        .await?;
    eprintln!(
        "created {}, updated {}, unchanged {}, deleted {}",
        synced.created, synced.updated, synced.unchanged, synced.deleted
    );
    Ok(())
}
```

## What `sync` promises

- **The resource ends up holding exactly the batch.** Records left out are
  deleted, so someone removed at the source leaves Drata on the next run.
- **Nothing changes in Drata unless every record was accepted.** The batch goes
  into a new v2 session, and the session is completed only after Drata accepted
  every record. Any error cancels it, and Drata keeps its previous records.
- **A run killed midway never completes its session**, and the next `sync`
  cancels the session it left.
- **An empty batch is refused** unless the caller passes `EmptySnapshot::Allow`,
  so a failed read at the source cannot empty Drata.
- **The key and the records stay out of errors and `Debug` output.** An invalid
  record is named by its position, never its contents. Drata's own message can
  quote a value from the request, so errors print only its status and code;
  `Error::drata_message` returns the text for a caller that decides where it may
  go.

Use only `sync` on a resource it keeps. `Client::direct_records` exists for
resources no session touches, and completing a session deletes anything written
there.

## The Drata key

Each connector gets its own key, holding exactly:

| Permission                                             | Why                                          |
| ------------------------------------------------------ | -------------------------------------------- |
| Custom Connections Data: Create Data                   | Uploading to a session, completing it        |
| Custom Connections Data: Create and Update Data        | Updating records                             |
| Custom Connections Data: Delete Data                   | Deleting records                             |
| Custom Connections Management: List Custom Connections | Cancelling a crashed run's session, counting |

Listing records and sessions needs the last permission. Without it, `sync`
cannot cancel a session a crashed run left in progress. See Drata's
[custom connections recipe](https://developers.drata.com/developer-portal/v2/recipes/custom-connections/).

## Loading Access Review users

Drata's Access Reviews sit outside custom connections, and the client can load
the users of an application managed by hand in Drata. Each load replaces the
whole set: users left out are removed from the application and its open review.

```rust,no_run
use drata_client::{
    ApiKey, ApplicationId, Client, EmptySnapshot, Region, ReviewUser, UserSet, UsersLoaded,
};

async fn load(drata_key: String) -> drata_client::Result<()> {
    // Validated with no key and no call; `body()` is the dry run.
    let users = UserSet::new(vec![
        ReviewUser::new("U-1", "alice", "alice@example.com").with_roles(["Owner"]),
    ])?;
    println!("{}", users.body());

    let client = Client::native(ApiKey::new(drata_key), Region::Us)?;
    match client
        .load_access_review_users(ApplicationId::new(12), &users, EmptySnapshot::Refuse)
        .await?
    {
        UsersLoaded::Saved { count } => eprintln!("loaded {count} users"),
        // Reviewers have recorded decisions: skip today, load after the review.
        UsersLoaded::ReviewLocked => eprintln!("review in progress; not loaded"),
        _ => {}
    }
    Ok(())
}
```

- **An empty set is refused** unless the caller passes `EmptySnapshot::Allow`,
  as for `sync`.
- **Drata's 409** comes back as `UsersLoaded::ReviewLocked`. It means reviewers
  have recorded decisions, or the application was completed in a review period
  still open, so a daily job skips the day rather than failing.
- **`Client::access_review_users`** lists the users the application holds, for
  comparing before a load.
- **The key needs** `access-review-application-users-put` to load and
  `access-review-application-users-get` to list, and the account needs the
  Access Review feature.

## Developing

The toolchain comes from `flake.nix`. With Nix and flakes enabled:

```sh
nix develop -c cargo nextest run
nix develop -c cargo test --doc
```

CI runs `cargo fmt --check`, `cargo clippy`, the tests, `cargo doc` and
`cargo deny check` through the same shell.

## Contributing and license

Issues and pull requests are welcome; see
[CONTRIBUTING.md](https://github.com/WealthLine/drata-client/blob/main/CONTRIBUTING.md).
Licensed under the
[MIT license](https://github.com/WealthLine/drata-client/blob/main/LICENSE).
