//! Persistence, tested on the files rather than through a socket.
//!
//! These are the properties `AGENTS.md` §4.2 requires of `repository.rs` on the
//! client and that ADR-010 requires of the server's schema: versioned migrations,
//! a round trip, and foreign key integrity. They are integration tests on the
//! public [`Store`] API rather than unit tests inside `db.rs`, so the assertions
//! are about what a caller can observe -- which is the only thing a test can
//! usefully pin down.

mod support;

use std::path::PathBuf;

use rusqlite::Connection;
use sh_nexus_server::db::{Store, DEFAULT_CHANNEL_ID, SCHEMA_VERSION, UNATTRIBUTED_USER_ID};
use sh_nexus_server::error::ServerError;
use support::TempDir;

const FIRST_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";
const SECOND_ID: &str = "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e";

/// Opens a store on a fresh file inside a fresh directory.
fn open(directory: &TempDir) -> (Store, PathBuf) {
    let path = directory.path().join("sh_nexus.sqlite3");
    let store = Store::open(&path).expect("a store on a fresh file");
    (store, path)
}

#[test]
fn a_fresh_database_is_on_this_builds_schema_version() {
    let directory = TempDir::new("schema-version").expect("a temporary directory");
    let (store, _) = open(&directory);

    assert_eq!(
        store.schema_version().expect("the stamped version"),
        SCHEMA_VERSION,
        "the file must carry the version this build migrated it to"
    );
    assert_eq!(
        store.schema_version().expect("a second read"),
        SCHEMA_VERSION
    );
}

#[test]
fn adr_010_requires_read_cursors_to_exist_from_the_first_migration() {
    let directory = TempDir::new("read-cursors").expect("a temporary directory");
    let (store, path) = open(&directory);
    store
        .accept_message(FIRST_ID, DEFAULT_CHANNEL_ID, "hello team")
        .expect("an accepted message");

    let connection = Connection::open(&path).expect("a reader connection");

    // The table is not read by anything in this milestone. That is the point: ADR-010
    // decides per-`(user, channel)` read state "from the first migration", and a
    // data migration over live rows is the expensive version of this insert.
    connection
        .execute(
            "INSERT INTO read_cursors (user_id, channel_id, last_read_at_unix_ms)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![UNATTRIBUTED_USER_ID, DEFAULT_CHANNEL_ID, 1_i64],
        )
        .expect("read_cursors must accept the per-(user, channel) row ADR-010 describes");

    // One row per pair, which is the whole shape of the decision: a second write
    // for the same pair is an update, not a second row.
    connection
        .execute(
            "INSERT INTO read_cursors (user_id, channel_id, last_read_at_unix_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id, channel_id) DO UPDATE
             SET last_read_at_unix_ms = excluded.last_read_at_unix_ms",
            rusqlite::params![UNATTRIBUTED_USER_ID, DEFAULT_CHANNEL_ID, 2_i64],
        )
        .expect("read_cursors must be keyed on the pair");

    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM read_cursors WHERE user_id = ?1 AND channel_id = ?2",
            rusqlite::params![UNATTRIBUTED_USER_ID, DEFAULT_CHANNEL_ID],
            |row| row.get(0),
        )
        .expect("the cursor count");
    assert_eq!(rows, 1, "read state is per (user, channel), not per user");

    // And a cursor for a channel that does not exist is refused, because the
    // foreign key is on and `db.rs` turns it on explicitly.
    let orphan = connection.execute(
        "INSERT INTO read_cursors (user_id, channel_id, last_read_at_unix_ms)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![UNATTRIBUTED_USER_ID, "c_nowhere", 1_i64],
    );
    assert!(
        orphan.is_err(),
        "a cursor for an unknown channel would make the unread count for it \
         meaningless rather than wrong-looking"
    );
}

#[test]
fn channel_members_exists_and_enforces_its_foreign_keys() {
    let directory = TempDir::new("channel-members").expect("a temporary directory");
    let (store, path) = open(&directory);
    store
        .accept_message(FIRST_ID, DEFAULT_CHANNEL_ID, "hello team")
        .expect("an accepted message");

    let connection = Connection::open(&path).expect("a reader connection");
    connection
        .execute(
            "INSERT INTO channel_members (user_id, channel_id) VALUES (?1, ?2)",
            rusqlite::params![UNATTRIBUTED_USER_ID, DEFAULT_CHANNEL_ID],
        )
        .expect("channel_members must accept the row its schema describes");

    assert!(
        connection
            .execute(
                "INSERT INTO channel_members (user_id, channel_id) VALUES (?1, ?2)",
                rusqlite::params![UNATTRIBUTED_USER_ID, "c_nowhere"],
            )
            .is_err(),
        "membership of a channel that does not exist is not a row"
    );
}

#[test]
fn every_byte_of_a_message_body_round_trips_verbatim() {
    let directory = TempDir::new("round-trip").expect("a temporary directory");
    let (store, path) = open(&directory);

    // The bodies that break a careless implementation: a parameter that is not a
    // parameter, an apostrophe that would end a quoted literal, a quote that would
    // end one, a newline, a tab, and non-ASCII text. Every query in `db.rs` is
    // parameterized, and this is the test that says so.
    //
    // The empty body is deliberately absent: it is a *refusal*, decided in
    // `message.rs` before the store is reached, and
    // `tests/refusals.rs::an_empty_content_is_refused_with_a_reason` is where it
    // belongs. The store is a container and validates nothing -- which is the
    // point of putting validation in one place.
    let bodies = [
        "hello team",
        "'; DROP TABLE messages; --",
        "he said \"hi\" and left",
        "line one\r\nline two\ttabbed",
        "emoji \u{1f44d} and accents: áéíóú ñ",
        "backtick ` and dollar $ and brace {}",
    ];

    for (index, content) in bodies.iter().enumerate() {
        let id = format!("id-{index}");
        match store.accept_message(&id, DEFAULT_CHANNEL_ID, content) {
            Ok(sh_nexus_server::AcceptOutcome::Inserted(message)) => {
                assert_eq!(
                    &message.content, content,
                    "the body must survive verbatim, or the transcript a user reads is \
                     not the message they sent"
                );
            }
            other => panic!("expected {content:?} to be inserted, got {other:?}"),
        }
    }

    assert_eq!(
        support::query_i64(
            &path,
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            DEFAULT_CHANNEL_ID
        ),
        bodies.len() as i64,
        "every insert is its own row"
    );

    // And the injection attempt is data, not an instruction: the table is still
    // there and the row is still readable.
    let quoted = support::query_count(&path, "SELECT COUNT(*) FROM messages");
    assert_eq!(quoted, bodies.len() as i64);
}

#[test]
fn messages_are_ordered_by_acceptance_then_by_id() {
    let directory = TempDir::new("ordering").expect("a temporary directory");
    let (store, path) = open(&directory);

    // Inserted inside one millisecond, deliberately. `messages_channel_order` is
    // `(channel_id, accepted_at_unix_ms, id)`, and the tie-break has to be total or
    // a resync would be able to return the same page twice.
    for index in 0..5 {
        store
            .accept_message(
                &format!("id-{index}"),
                DEFAULT_CHANNEL_ID,
                &format!("message {index}"),
            )
            .expect("an accepted message");
    }

    let connection = Connection::open(&path).expect("a reader connection");
    let mut statement = connection
        .prepare(
            "SELECT content FROM messages
             WHERE channel_id = ?1
             ORDER BY accepted_at_unix_ms ASC, id ASC",
        )
        .expect("the ordering query");
    let ordered: Vec<String> = statement
        .query_map([DEFAULT_CHANNEL_ID], |row| row.get(0))
        .expect("the rows")
        .map(|row| row.expect("a content column"))
        .collect();

    assert_eq!(
        ordered,
        vec![
            "message 0",
            "message 1",
            "message 2",
            "message 3",
            "message 4",
        ],
        "insertion order is acceptance order, and the id tie-break agrees with it"
    );
}

#[test]
fn reopening_an_existing_database_preserves_its_rows_and_its_version() {
    let directory = TempDir::new("reopen").expect("a temporary directory");
    let (store, path) = open(&directory);
    store
        .accept_message(FIRST_ID, DEFAULT_CHANNEL_ID, "hello team")
        .expect("an accepted message");
    store
        .accept_message(SECOND_ID, DEFAULT_CHANNEL_ID, "still here")
        .expect("a second accepted message");
    drop(store);

    let reopened = Store::open(&path).expect("reopening a migrated file");
    assert_eq!(
        reopened
            .schema_version()
            .expect("the version after reopening"),
        SCHEMA_VERSION,
        "reopening must not re-run migration 1: the file is already on this version"
    );
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM messages"),
        2,
        "every message row survives the reopen"
    );
    drop(reopened);

    // Deleting a row and reopening must not bring it back. `Store::open` runs the
    // seed on every start, and `INSERT OR IGNORE` is what makes that safe: the
    // alternative, `INSERT OR REPLACE`, would delete and reinsert the row it
    // touches, which on `users` cascades into `messages.user_id`.
    let connection = Connection::open(&path).expect("a writer connection");
    connection
        .execute("DELETE FROM messages WHERE client_msg_id = ?1", [SECOND_ID])
        .expect("deleting a message row");
    drop(connection);

    Store::open(&path).expect("reopening after a deletion");

    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM messages"),
        1,
        "a deleted row must stay deleted across a reopen"
    );
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM users"),
        1,
        "the seed runs again on every open and must not duplicate the row it finds"
    );
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM channels"),
        1,
        "nor the channel, whose `last_message_at` is the resume cursor a reinsert \
         would reset"
    );
}

#[test]
fn a_file_from_a_newer_build_is_refused_rather_than_opened() {
    let directory = TempDir::new("too-new").expect("a temporary directory");
    let (store, path) = open(&directory);
    store
        .accept_message(FIRST_ID, DEFAULT_CHANNEL_ID, "hello team")
        .expect("an accepted message");
    drop(store);

    let connection = Connection::open(&path).expect("a writer connection");
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("stamping a future version");
    connection
        .execute(
            "INSERT INTO messages
                 (id, client_msg_id, channel_id, user_id, content, accepted_at_unix_ms)
             VALUES ('future-row', 'future-client-id', ?1, ?2, 'from the future', 1)",
            rusqlite::params![DEFAULT_CHANNEL_ID, UNATTRIBUTED_USER_ID],
        )
        .expect("a row only the future build could have written");
    drop(connection);

    match Store::open(&path) {
        Err(ServerError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, SCHEMA_VERSION + 1);
            assert_eq!(supported, SCHEMA_VERSION);
        }
        Err(other) => panic!("expected a SchemaTooNew refusal, got {other}"),
        Ok(_) => panic!(
            "a build that has never heard of a schema version must refuse the file, \
             not open it: SQLite will happily let it write into a layout it misreads"
        ),
    }

    assert_eq!(
        support::query_i64(
            &path,
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            "future-client-id",
        ),
        1,
        "the refusal must leave the file exactly as it was"
    );
}

#[test]
fn a_file_with_tables_but_no_version_stamp_is_refused() {
    let directory = TempDir::new("unversioned").expect("a temporary directory");
    let path = directory.path().join("not-ours.sqlite3");
    let connection = Connection::open(&path).expect("creating a file");
    connection
        .execute_batch(
            "CREATE TABLE messages (
                 id TEXT NOT NULL PRIMARY KEY,
                 content TEXT NOT NULL
             );
             INSERT INTO messages (id, content) VALUES ('m_1', 'someone else''s data');",
        )
        .expect("a table that is not ours");
    drop(connection);

    match Store::open(&path) {
        Err(ServerError::UnversionedSchema) => {}
        Err(other) => panic!("expected an UnversionedSchema refusal, got {other}"),
        Ok(_) => panic!(
            "a file with tables and no version stamp was not made by this build, and \
             running migration 1 against it would either fail mid-way or -- worse -- \
             succeed against a schema that only looks compatible"
        ),
    }

    assert_eq!(
        support::query_i64(&path, "SELECT COUNT(*) FROM messages WHERE id = ?1", "m_1"),
        1,
        "the refusal must not have touched the other tool's data"
    );
}

#[test]
fn a_send_naming_an_unknown_channel_is_refused_by_the_store() {
    let directory = TempDir::new("unknown-channel").expect("a temporary directory");
    let (store, _) = open(&directory);

    match store.accept_message(FIRST_ID, "c_nowhere", "hello team") {
        Err(ServerError::UnknownChannel { channel_id }) => {
            assert_eq!(channel_id, "c_nowhere");
        }
        other => panic!("expected an UnknownChannel refusal, got {other:?}"),
    }
}

#[test]
fn a_store_reports_the_file_it_opened() {
    let directory = TempDir::new("debug").expect("a temporary directory");
    let (store, _path) = open(&directory);

    // `Store`'s `Debug` prints the file and nothing else, because a `Debug` impl
    // that can reach a log line has to be trusted not to print the connection's
    // configuration. The database file is the one thing an operator needs.
    let rendered = format!("{store:?}");
    assert!(
        rendered.contains("sh_nexus.sqlite3"),
        "the store's Debug must name its file: {rendered}"
    );
    assert!(
        !rendered.contains("journal"),
        "and must not dump the connection's configuration: {rendered}"
    );
}

#[test]
fn opening_a_file_whose_directory_does_not_exist_reports_the_path() {
    let directory = TempDir::new("missing-parent").expect("a temporary directory");
    let path = directory
        .path()
        .join("no-such-directory")
        .join("sh_nexus.sqlite3");

    match Store::open(&path) {
        // `rusqlite` reports SQLite's `SQLITE_CANTOPEN` rather than the operating
        // system's error, so this is `ServerError::Sqlite` and not
        // `ServerError::Io`. What matters to an operator is the other half: the
        // message carries the full path, so a wrong `SH_NEXUS_DB` is diagnosable
        // without guessing.
        Err(ServerError::Sqlite(inner)) => {
            let rendered = inner.to_string();
            assert!(
                rendered.contains("no-such-directory"),
                "the refusal must name the path it could not open, because the \
                 alternative is an operator guessing which of two variables is wrong: \
                 {rendered}"
            );
        }
        Err(other) => panic!("expected a Sqlite error for a missing parent directory, got {other}"),
        Ok(_) => panic!(
            "`Connection::open` creates a file, not a directory, so a missing parent \
             must fail rather than be created"
        ),
    }

    assert!(!path.exists());
}
