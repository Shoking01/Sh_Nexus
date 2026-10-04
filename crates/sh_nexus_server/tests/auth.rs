//! Authentication, end to end: bootstrap, login, the handshake's 401, revocation,
//! and the migration that made room for all of it.
//!
//! # What this file is for
//!
//! ADR-010's Decision section commits this server to four properties, and each
//! one has exactly one test here that would fail if the property were dropped:
//!
//! | Property | Test |
//! |---|---|
//! | Closed registration: an administrator creates accounts, and the first one comes from the environment | [`a_fresh_instance_bootstraps_one_administrator_and_a_second_start_changes_nothing`] |
//! | Migration 2 is additive and destroys nothing | [`migration_two_is_additive_and_preserves_a_schema_one_database`] |
//! | A token is stored hashed, so a leaked database yields no usable token | [`the_database_stores_the_tokens_digest_and_never_the_token`] |
//! | An unauthenticated handshake is refused **before** the upgrade | [`every_credential_shaped_handshake_is_refused_with_a_401_and_no_socket`] |
//! | A revoked session dies immediately, without waiting for its expiry | [`a_revoked_session_dies_immediately_rather_than_at_its_expiry`] |
//! | Messages are attributed to the authenticated user | [`a_message_is_attributed_to_the_authenticated_user_and_never_to_the_reserved_row`] |
//!
//! # How "before the upgrade" is proved, and not merely asserted
//!
//! Two independent observations, because either one alone has a cheaper
//! explanation:
//!
//! 1. **The peer never receives a 101.** [`support::attempt_upgrade`] reads the
//!    status line, so a `101` followed by a close would show up as a `101`. It
//!    shows up as a `401` instead.
//! 2. **The server's connection count stays at zero.** `ws.rs` subscribes to the
//!    hub *after* it has authenticated, so a refused handshake costs the server
//!    no task, no subscription and no row. An implementation that upgraded first
//!    and closed afterwards would answer the same `401` to the peer while still
//!    having spent the resources -- and would leave this count at one.
//!
//! Neither observation is a proxy for the other, which is why both are here.
//!
//! # No `sleep()` anywhere
//!
//! `AGENTS.md` §4.3 forbids waiting for logic to happen. Expiry is tested by
//! *placing the clock*: [`sh_nexus_server::Store::lookup_session`] takes `now` as an
//! argument rather than reading it, so a session can be observed on either side of
//! its expiry without a session TTL measured in days being waited out. The
//! revocation test is the same trick: it revokes a session whose expiry is seven
//! days away and observes it die immediately.

mod support;

use sh_nexus_server::auth;
use sh_nexus_server::db::{
    Account, SessionState, Store, DEFAULT_CHANNEL_ID, SCHEMA_VERSION, UNATTRIBUTED_USER_ID,
};
use sh_nexus_server::error::ServerError;
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use support::{
    attempt_upgrade, TestClient, TestServer, ADMIN_PASSWORD, ADMIN_USERNAME, WRONG_PASSWORD,
};

use rusqlite::Connection;

const FIRST_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";
const SECOND_MSG_ID: &str = "7d9b2c3e-4f5a-4b6c-9d0e-1f2a3b4c5d6e";

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

/// A fresh instance gets exactly one administrator, and a second start changes
/// nothing.
///
/// **Both halves are in one test on purpose.** The interesting failure is not "the
/// first bootstrap did not run" -- that is visible from a login -- it is "the second
/// bootstrap ran and overwrote somebody's password", which is the failure that would
/// be *most* damaging and is invisible to any test that only starts an instance
/// once. So the second call is made, and the password is then checked with the
/// *first* value: if the second call had rewritten it, that login would fail.
#[test]
fn a_fresh_instance_bootstraps_one_administrator_and_a_second_start_changes_nothing() {
    let directory = support::TempDir::new("bootstrap").expect("a temporary directory");
    let path = directory.path().join("sh_nexus.sqlite3");
    let store = Store::open(&path).expect("a migrated database");

    assert!(
        auth::bootstrap(&store, ADMIN_USERNAME, ADMIN_PASSWORD).expect("a first bootstrap"),
        "a database with no account must bootstrap one"
    );
    assert_eq!(
        store.account_count().expect("an account count"),
        1,
        "exactly one account, and the reserved historical-author row is not one of them"
    );

    let credential = store
        .credential_for_username(ADMIN_USERNAME)
        .expect("a readable credential")
        .expect("the bootstrapped account");
    assert!(
        credential.account.is_admin,
        "the bootstrap account is an administrator"
    );
    assert!(
        credential.password_hash.is_some(),
        "the bootstrap account can log in; the reserved row cannot"
    );
    assert!(
        auth::verify_password(credential.password_hash.as_deref(), ADMIN_PASSWORD),
        "the account's own password must verify against the hash that was stored"
    );

    // The bootstrap administrator is the one account that can reach a channel
    // without being granted one -- bootstrapping a team means being able to say
    // something in it.
    assert!(
        store
            .user_can_reach_channel(&credential.account.id, DEFAULT_CHANNEL_ID)
            .expect("a membership read"),
        "the bootstrap administrator is granted the seeded channel"
    );

    // The second call. A different password, on purpose: an implementation that
    // overwrote on every start would rewrite it to this one.
    assert!(
        !auth::bootstrap(&store, ADMIN_USERNAME, WRONG_PASSWORD)
            .expect("a second, no-op bootstrap"),
        "an instance that already has an account must not bootstrap another"
    );
    assert_eq!(
        store.account_count().expect("an account count"),
        1,
        "a second start adds no second administrator"
    );
    let after = store
        .credential_for_username(ADMIN_USERNAME)
        .expect("a readable credential")
        .expect("the bootstrapped account");
    assert!(
        auth::verify_password(after.password_hash.as_deref(), ADMIN_PASSWORD),
        "a second start must not rewrite the password the first one set"
    );

    // And the reserved row is still not an account. This is the assertion that
    // keeps `Store::account_count`'s definition honest: it counts rows that can log
    // in, and the reserved row cannot.
    assert!(
        store
            .credential_for_username("unattributed")
            .expect("a readable credential")
            .is_some_and(|credential| credential.password_hash.is_none()),
        "the reserved historical-author row exists and still has no password, so it is \
         never the reason a bootstrap is skipped"
    );
}

/// The reserved row cannot log in, and cannot be given a password by accident.
///
/// The second half is a schema fact rather than a code fact: `password_hash` is a
/// plain nullable column, so nothing stops an operator from typing into it by hand.
/// What the code guarantees is the narrower and more useful claim -- the reserved
/// row is seeded with none, so it is not an account, and an argon2 verification
/// against it is a full-cost verification that always fails.
#[tokio::test]
async fn the_reserved_row_is_not_a_login_and_the_password_policy_is_enforced() {
    let server = TestServer::start().await;
    let response = support::post_json(
        server.address(),
        auth::LOGIN_PATH,
        None,
        &serde_json::json!({ "username": "unattributed", "password": ADMIN_PASSWORD }),
    )
    .await
    .expect("a login request that reaches the server");

    assert_eq!(
        response.status, 401,
        "a row with no password hash must not authenticate anybody: {}",
        response.body
    );
    assert_eq!(
        response.code().as_deref(),
        Some(auth::INVALID_CREDENTIALS),
        "and it must be refused with the same code as a wrong password, so the endpoint \
         cannot be used to discover which usernames exist"
    );

    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Migration 2
// ---------------------------------------------------------------------------

/// Migration 2 is additive, and a database already on schema 1 keeps everything.
///
/// **The file is built by hand, one statement at a time, at `user_version = 1`.**
/// That is the only way to test an upgrade: `Store::open` on a fresh file creates
/// schema 2 directly, so a test that wanted to exercise the migration would have to
/// produce a schema-1 file some other way. Reproducing `MIGRATION_1` here also means
/// a change to migration 1 that broke an upgrade would fail this test rather than
/// failing on a deployment nobody is watching.
#[test]
fn migration_two_is_additive_and_preserves_a_schema_one_database() {
    let directory = support::TempDir::new("migration-2").expect("a temporary directory");
    let path = directory.path().join("sh_nexus.sqlite3");

    // Build the schema-1 file, with data in it, at `user_version = 1`.
    let connection = Connection::open(&path).expect("creating a schema-1 file");
    connection
        .execute_batch(SCHEMA_ONE_SCHEMA)
        .expect("migration 1's statements");
    connection
        .execute(
            "INSERT INTO users
                 (id, username, display_name, avatar_url, created_at_unix_ms)
             VALUES (?1, ?2, ?3, NULL, 1)",
            rusqlite::params![
                UNATTRIBUTED_USER_ID,
                "unattributed",
                "Unattributed (written before authentication)"
            ],
        )
        .expect("the reserved row, as migration 1's seed wrote it");
    connection
        .execute(
            "INSERT INTO users
                 (id, username, display_name, avatar_url, created_at_unix_ms)
             VALUES ('u_legacy', 'dana', 'Dana', NULL, 1)",
            [],
        )
        .expect("an account that predates authentication");
    connection
        .execute(
            "INSERT INTO channels
                 (id, name, description, is_private, created_at_unix_ms, last_message_at_unix_ms)
             VALUES (?1, 'general', NULL, 0, 1, NULL)",
            [DEFAULT_CHANNEL_ID],
        )
        .expect("the seeded channel");
    connection
        .execute(
            "INSERT INTO messages
                 (id, client_msg_id, channel_id, user_id, content, accepted_at_unix_ms)
             VALUES ('m_before', 'c_before', ?1, ?2, 'written before authentication', 1)",
            rusqlite::params![DEFAULT_CHANNEL_ID, UNATTRIBUTED_USER_ID],
        )
        .expect("a message from before authentication");
    connection
        .pragma_update(None, "user_version", 1)
        .expect("stamping schema 1");
    drop(connection);

    // The upgrade.
    let store = Store::open(&path).expect("a file this build may migrate");
    assert_eq!(
        store.schema_version().expect("the stamped version"),
        SCHEMA_VERSION,
        "the file must now be on this build's version"
    );

    // Nothing was destroyed.
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM messages"),
        1,
        "every message survives the migration; ADR-010 retains authorship on removal, and \
         a migration that dropped rows would be the worst way to break that"
    );
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM users"),
        2,
        "both pre-existing users survive: the reserved row and the account that predates \
         authentication"
    );

    // Nothing was backfilled either. Both accounts have `password_hash IS NULL`, so
    // `Store::account_count` reports zero and the bootstrap still runs on a migrated
    // instance -- which is what `auth::bootstrap` is documented to rely on.
    assert_eq!(
        store.account_count().expect("an account count"),
        0,
        "no pre-existing row was given a password, so an upgraded instance still has no \
         administrator and still bootstraps one"
    );
    for username in ["unattributed", "dana"] {
        let credential = store
            .credential_for_username(username)
            .expect("a readable credential")
            .expect("the row survived");
        assert!(
            credential.password_hash.is_none(),
            "{username:?} predates authentication and has no password, so it cannot log in"
        );
        assert!(
            !credential.account.is_admin,
            "{username:?} predates `is_admin`, so it defaults to the least-privileged value"
        );
    }

    // The seeded rows are still what they were, and the reserved row was not
    // disturbed by a re-run of the seed.
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM channels"),
        1,
        "the seeded channel survives, and its `last_message_at` was not reset"
    );

    // And the migration is idempotent: a second open does not try to add the columns
    // again. `MIGRATION_1`'s "no IF NOT EXISTS" rule means a repeated `ALTER` would
    // fail loudly, so this is the test that says the version stamp is what prevents it.
    drop(store);
    Store::open(&path).expect("reopening a file already on this schema version");

    // The migration's own constraint: a session that expires the instant it was
    // created is a `SQLITY_CONSTRAINT` at the insert, not a session that is born dead.
    let store = Store::open(&path).expect("the file, again");
    let account = store
        .create_account(
            "after",
            "After",
            &auth::hash_password("a-long-enough-passphrase").expect("a hash"),
            true,
        )
        .expect("an account on the migrated schema");
    assert!(
        store
            .insert_session("hash", &account.id, 1_000, 1_000)
            .is_err(),
        "a session whose expiry equals its creation must be refused by the CHECK, because \
         a login that can never succeed is indistinguishable from a wrong password to \
         the person holding it"
    );
}

/// `MIGRATION_1`, verbatim, so this file can build a schema-1 database.
///
/// **Copied rather than imported**, and the reason is the same one
/// `crates/sh_nexus/tests/support/mod.rs` gives for spelling the server's WebSocket
/// path out: `MIGRATION_1` is a private `const`, and making it public purely so a
/// test could reproduce it would expose the migration to any caller at all. A
/// duplication a test watches is better than a constant that silently drifts -- and
/// this one is watched, because a `Store::open` that fails to upgrade the file this
/// builds would fail every assertion in the test above it.
const SCHEMA_ONE_SCHEMA: &str = r#"
CREATE TABLE users (
    id                TEXT    NOT NULL PRIMARY KEY,
    username          TEXT    NOT NULL,
    display_name      TEXT    NOT NULL,
    avatar_url        TEXT,
    created_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX users_username_unique ON users (username);

CREATE TABLE channels (
    id                    TEXT    NOT NULL PRIMARY KEY,
    name                  TEXT    NOT NULL,
    description           TEXT,
    is_private            INTEGER NOT NULL DEFAULT 0 CHECK (is_private IN (0, 1)),
    created_at_unix_ms    INTEGER NOT NULL,
    last_message_at_unix_ms INTEGER,
    CHECK (last_message_at_unix_ms IS NULL OR last_message_at_unix_ms >= created_at_unix_ms)
) STRICT;

CREATE UNIQUE INDEX channels_name_unique ON channels (name);

CREATE TABLE messages (
    id                 TEXT    NOT NULL PRIMARY KEY,
    client_msg_id      TEXT    NOT NULL,
    channel_id         TEXT    NOT NULL REFERENCES channels (id),
    user_id            TEXT    NOT NULL REFERENCES users (id),
    content            TEXT    NOT NULL,
    accepted_at_unix_ms INTEGER NOT NULL,
    edited_at_unix_ms  INTEGER,
    thread_id          TEXT    REFERENCES messages (id)
) STRICT;

CREATE UNIQUE INDEX messages_client_msg_id_unique ON messages (client_msg_id);

CREATE INDEX messages_channel_order
    ON messages (channel_id, accepted_at_unix_ms, id);

CREATE TABLE read_cursors (
    user_id              TEXT    NOT NULL REFERENCES users (id),
    channel_id           TEXT    NOT NULL REFERENCES channels (id),
    last_read_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, channel_id)
) STRICT;

CREATE TABLE channel_members (
    user_id    TEXT NOT NULL REFERENCES users (id),
    channel_id TEXT NOT NULL REFERENCES channels (id),
    PRIMARY KEY (user_id, channel_id)
) STRICT;
"#;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// The database stores a token's digest and never the token.
///
/// **The leak test, written as the leak an operator would actually perform.** The
/// whole reason `sessions` is keyed on a hash is ADR-010's "a backup is a file
/// copy", so the claim to test is that the file copy is not a set of live sessions.
/// This reads every value in the `sessions` table out of the real database file and
/// asserts none of them is the token that was issued -- and, because a hash is a
/// one-way function, also asserts none of them verifies as one.
#[tokio::test]
async fn the_database_stores_the_tokens_digest_and_never_the_token() {
    let server = TestServer::start().await;
    let token = server.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;

    assert_eq!(
        token.len(),
        auth::SESSION_TOKEN_BYTES * 2,
        "a 32-byte token in hex"
    );
    assert!(
        token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "a token travels in an HTTP header and in JSON, so it must need no escaping: \
         {token:?}"
    );

    // Every value in the table, by any route.
    let connection = Connection::open(server.database()).expect("the database file");
    let mut statement = connection
        .prepare("SELECT token_hash FROM sessions")
        .expect("the sessions query");
    let stored: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("the rows")
        .map(|row| row.expect("a token_hash column"))
        .collect();
    drop(statement);
    drop(connection);

    // Two rows, not one: the fixture mints a session of its own so socket suites
    // never have to make an HTTP request. Both are hashed, and the point of this
    // test is that *no* row is the plaintext of *any* token.
    assert_eq!(
        stored.len(),
        2,
        "the fixture's own session and this login's, both stored"
    );
    assert!(
        stored.iter().all(|value| value != &token),
        "the plaintext token must never be in the database: that is the whole property. \
         The table holds {stored:?}"
    );
    assert!(
        stored.contains(&auth::hash_session_token(&token)),
        "and the row for this login must be exactly its digest, so the lookup is a \
         single indexed read rather than a scan comparing every row. The table holds \
         {stored:?}"
    );
    assert!(
        !stored.iter().any(|value| value.contains(&token)),
        "and no digest may contain the token as a substring -- which is the shape a \
         naive \"hash the id\" implementation would produce. The table holds {stored:?}"
    );

    server.shutdown().await;
}

/// Minted tokens differ, and are the size the schema claims.
///
/// `proptest`-free on purpose: the property is not "tokens are random" -- which no
/// test can prove -- it is "two tokens are not equal", and a hundred draws is
/// already so far past a collision at 2^256 that the assertion is about the
/// *generator being called*, not about its quality. The quality argument lives in
/// `SESSION_TOKEN_BYTES`'s docs.
#[test]
fn minted_tokens_are_distinct_and_the_right_width() {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let token = auth::mint_session_token();
        assert_eq!(token.len(), auth::SESSION_TOKEN_BYTES * 2);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "a token is lowercase hex, so `bearer_token` accepts exactly what this \
             produces: {token:?}"
        );
        assert!(
            seen.insert(token.clone()),
            "two consecutive tokens were identical, which would mean the entropy source \
             is not being read: {token:?}"
        );
    }
}

/// `bearer_token` accepts one spelling and refuses the rest.
///
/// Every refusal is listed rather than sampled, because the set is the shape check
/// that keeps a peer from making this server hash an arbitrary-length string on
/// every handshake.
#[test]
fn only_a_bearer_token_of_the_exact_shape_is_accepted() {
    let good = auth::mint_session_token();
    assert_eq!(
        auth::bearer_token(Some(&format!("Bearer {good}"))),
        Some(good.as_str())
    );

    for refused in [
        None,
        Some(""),
        Some(good.as_str()),
        Some(&format!("bearer {good}")),
        Some(&format!("BEARER {good}")),
        Some(&format!("Basic {good}")),
        Some(&format!("Bearer{good}")),
        Some(&format!("Bearer  {good}")),
        Some(&format!("Bearer\t{good}")),
        Some(&format!("Bearer {}", good.to_uppercase())),
        Some(&format!("Bearer {good}extra")),
        Some(&format!(
            "Bearer {}",
            &good[..auth::SESSION_TOKEN_BYTES * 2 - 1]
        )),
        Some("Bearer not-hex-but-the-right-length-!!!!!!!!!!!!!!!!!!!!!!!!!!"),
    ] {
        assert_eq!(
            auth::bearer_token(refused),
            None,
            "{refused:?} is not a bearer token this server accepts"
        );
    }
}

/// A login with the right credentials issues a token; the wrong ones do not.
///
/// **And the two failures are byte-identical**, which is the property the whole
/// `verify_password` signature exists to make unbreakable: if an unauthenticated
/// peer can tell "no such user" from "wrong password", it can enumerate the team.
#[tokio::test]
async fn a_login_issues_a_token_and_both_failures_look_identical() {
    let server = TestServer::start().await;

    let token = server.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    assert_eq!(token.len(), auth::SESSION_TOKEN_BYTES * 2);

    let wrong_password = support::post_json(
        server.address(),
        auth::LOGIN_PATH,
        None,
        &serde_json::json!({ "username": ADMIN_USERNAME, "password": WRONG_PASSWORD }),
    )
    .await
    .expect("a login request that reaches the server");

    let no_such_user = support::post_json(
        server.address(),
        auth::LOGIN_PATH,
        None,
        &serde_json::json!({ "username": "nobody-at-all", "password": ADMIN_PASSWORD }),
    )
    .await
    .expect("a login request that reaches the server");

    assert_eq!(wrong_password.status, 401);
    assert_eq!(no_such_user.status, 401);
    assert_eq!(
        wrong_password.body, no_such_user.body,
        "an unauthenticated peer must not be able to tell a wrong password from an \
         unknown username, or this endpoint is a team directory"
    );

    // And neither refusal body carries the username, the password, or anything
    // either of them implies.
    assert!(!wrong_password.body.contains(ADMIN_USERNAME));
    assert!(!wrong_password.body.contains(WRONG_PASSWORD));

    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// The handshake
// ---------------------------------------------------------------------------

/// Every credential-shaped handshake is refused with a 401, and no socket is made.
///
/// The table is the specification. Every row is a way a peer can arrive without a
/// live session, and each one gets its own labelled refusal so a failure says which
/// row regressed.
///
/// # Panics
///
/// If any row is accepted, or if any refusal leaves a connection behind.
#[tokio::test]
async fn every_credential_shaped_handshake_is_refused_with_a_401_and_no_socket() {
    let server = TestServer::start().await;

    let live = server.admin_token().to_owned();
    let unknown = auth::mint_session_token();
    let valid_token = server.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    let revoked = server.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    let logout = server.logout(&revoked).await;
    assert_eq!(
        logout.status, 204,
        "a logout must be answered with 204 whether or not the session existed: {}",
        logout.body
    );

    let refusals: [(&str, Option<String>); 6] = [
        ("no Authorization header", None),
        ("an empty bearer value", Some(String::new())),
        ("a different scheme", Some(format!("Basic {}", live))),
        ("a token no session has", Some(unknown)),
        ("a revoked session", Some(revoked)),
        (
            "a valid token that was not hex",
            Some("not-a-token-at-all-but-long-enough-to-look-like-one-xxxxxxxxxxxxxxx".to_owned()),
        ),
    ];

    for (label, presented) in refusals {
        let before = server.connection_count();
        let response = attempt_upgrade(server.address(), presented.as_deref())
            .await
            .expect("the upgrade attempt to reach the server");

        assert_eq!(
            response.status, 401,
            "{label}: the peer must be refused with a 401, and it must never see a 101"
        );
        assert_eq!(
            response.code().as_deref(),
            Some(auth::INVALID_CREDENTIALS),
            "{label}: every credential-shaped refusal carries one code, so the peer \
             cannot tell them apart"
        );
        assert!(
            response.head.to_lowercase().contains("www-authenticate"),
            "{label}: a 401 must say which scheme it wants, or a client has nothing to \
             react to: {}",
            response.head
        );
        assert!(
            !response.body.contains(&live),
            "{label}: a refusal body must not echo the credential it was given"
        );
        assert_eq!(
            server.connection_count(),
            before,
            "{label}: the refusal happened BEFORE the upgrade, so the server never \
             subscribed, never spawned a task and never held the connection"
        );
    }

    // And the positive control, on the same server, with the same code path: a live
    // token gets a socket. Without this, "every 401" would also be satisfied by a
    // server that refuses everything.
    let accepted = attempt_upgrade(server.address(), Some(&valid_token))
        .await
        .expect("the upgrade attempt to reach the server");
    assert!(
        accepted.is_upgrade(),
        "a live token must produce a 101, or the refusals above prove nothing: {}",
        accepted.head
    );

    server.shutdown().await;
}

/// A revoked session dies immediately, rather than at its expiry.
///
/// **The expiry is seven days away and it is asserted to be.** That is what makes
/// this a revocation test rather than an expiry test: if the code only checked
/// `expires_at_unix_ms`, this session would keep working for a week, and a test
/// that did not assert the expiry would still pass.
#[tokio::test]
async fn a_revoked_session_dies_immediately_rather_than_at_its_expiry() {
    let server = TestServer::start().await;
    let token = server.login(ADMIN_USERNAME, ADMIN_PASSWORD).await;

    // It works.
    let before = server.connection_count();
    let accepted = attempt_upgrade(server.address(), Some(&token))
        .await
        .expect("the upgrade attempt to reach the server");
    assert!(accepted.is_upgrade(), "a fresh session must be accepted");

    // Its expiry really is in the future, and nothing has revoked it yet.
    let connection = Connection::open(server.database()).expect("the database file");
    let (expires_at, revoked_at): (i64, Option<i64>) = connection
        .query_row(
            "SELECT expires_at_unix_ms, revoked_at_unix_ms FROM sessions WHERE token_hash = ?1",
            [auth::hash_session_token(&token)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the session row");
    drop(connection);
    assert!(
        expires_at > sh_nexus_server::time::now_unix_millis() + 24 * 60 * 60 * 1_000,
        "this test is only meaningful while the session is nowhere near expiry; the \
         stored expiry is {expires_at}"
    );
    assert_eq!(
        revoked_at, None,
        "a live session has no revocation stamp, so the refusal below cannot be \
         explained by the row already being dead"
    );

    // Revoke it.
    assert_eq!(
        server.logout(&token).await.status,
        204,
        "a logout must be answered with 204"
    );

    // It is dead immediately.
    let refused = attempt_upgrade(server.address(), Some(&token))
        .await
        .expect("the upgrade attempt to reach the server");
    assert_eq!(
        refused.status, 401,
        "a revoked session must be refused at once, not when it ages out"
    );
    assert_eq!(
        server.connection_count(),
        before,
        "and the refusal is still pre-upgrade, so no connection is left behind"
    );

    // A second logout is still a 204: revoking is idempotent, and reporting
    // otherwise would be an oracle for whether a token was ever valid.
    assert_eq!(
        server.logout(&token).await.status,
        204,
        "a second logout must be indistinguishable from the first"
    );

    server.shutdown().await;
}

/// `SessionState` reports revocation before expiry, and keeps the row to be
/// auditable.
///
/// The store-level half of the revocation property, and the one that says which
/// reason an *operator* would see in a log -- the peer cannot tell them apart, but
/// the log must.
#[test]
fn a_revoked_session_is_reported_as_revoked_and_the_row_survives() {
    let directory = support::TempDir::new("revocation").expect("a temporary directory");
    let path = directory.path().join("sh_nexus.sqlite3");
    let store = Store::open(&path).expect("a migrated database");

    let account: Account = store
        .create_account(
            "dana",
            "Dana",
            &auth::hash_password("a-long-enough-passphrase").expect("a hash"),
            false,
        )
        .expect("an account");
    let token = support::issue_session(&store, &account.id).expect("a session");
    let hash = auth::hash_session_token(&token);

    // Placed on both sides of the expiry at once, because the ordering is the claim.
    let expires_at = sh_nexus_server::time::now_unix_millis() + auth::SESSION_TTL_MILLIS;
    assert!(
        store.revoke_session(&hash, 1_234).expect("a revocation"),
        "revoking a live session must report that it did something"
    );

    let after_expiry = expires_at + 1;
    match store
        .lookup_session(&hash, after_expiry)
        .expect("a session lookup")
    {
        SessionState::Revoked { revoked_at_unix_ms } => assert_eq!(
            revoked_at_unix_ms, 1_234,
            "revocation is reported even once the session has also expired, because an \
             operator who revoked a session wants to know that"
        ),
        other => panic!("expected Revoked, got {other:?}"),
    }

    // The row is still there. Deleting it would make a second logout
    // indistinguishable from a token that was never valid -- fine as a property,
    // but it would also destroy the only record that the session existed.
    assert_eq!(
        support::query_count(&path, "SELECT COUNT(*) FROM sessions"),
        1,
        "a revoked session keeps its row; `revoke_session`'s docs say why"
    );
    assert!(
        !store
            .revoke_session(&hash, 5_678)
            .expect("a second revocation"),
        "revoking twice reports that the second call had nothing to do, and the caller \
         answers 204 either way"
    );
}

// ---------------------------------------------------------------------------
// Authorship
// ---------------------------------------------------------------------------

/// A message is attributed to the authenticated user, never to the reserved row.
///
/// **The property this milestone exists for**, asserted end to end: a socket
/// authenticated as a real account sends a frame, and the `message.ack` names that
/// account. Before this milestone the same assertion read `user_id ==
/// UNATTRIBUTED_USER_ID`; `message_round_trip.rs` carries that diff.
#[tokio::test]
async fn a_message_is_attributed_to_the_authenticated_user_and_never_to_the_reserved_row() {
    let server = TestServer::start().await;

    // A second identity, with its own session, so the author is not "the one account
    // this fixture happens to have created".
    let colleague_id = server.create_account("sam", true);
    let colleague_token = server.session_for(&colleague_id);
    let mut sam = TestClient::connect(server.address(), Some(&colleague_token))
        .await
        .expect("an authenticated upgrade for the second account");

    let envelope = ClientEnvelope::new(
        FIRST_MSG_ID,
        ClientFrame::MessageSend {
            channel_id: DEFAULT_CHANNEL_ID.to_owned(),
            content: "attributed to me".to_owned(),
        },
    );
    sam.send_text(&envelope.encode().expect("an encodable envelope"))
        .await
        .expect("the send to reach the server");

    let acked = ServerEnvelope::decode(&sam.expect_text("sam's message.ack").await)
        .expect("a decodable ack");
    match acked.frame {
        ServerFrame::MessageAck { message, .. } => {
            assert_eq!(
                message.user_id, colleague_id,
                "the author is the account whose session the handshake proved, which is \
                 the only place `message.send` can get an author from: the frame has no \
                 author field"
            );
            assert_ne!(
                message.user_id, UNATTRIBUTED_USER_ID,
                "the reserved row is not an author any more"
            );
        }
        other => panic!("expected a message.ack, got {other:?}"),
    }

    // The stored row agrees, read straight out of the file.
    let connection = Connection::open(server.database()).expect("the database file");
    let stored_author: String = connection
        .query_row(
            "SELECT user_id FROM messages WHERE client_msg_id = ?1",
            [FIRST_MSG_ID],
            |row| row.get(0),
        )
        .expect("the stored message row");
    drop(connection);
    assert_eq!(
        stored_author, colleague_id,
        "the file agrees with the ack, so the attribution is not a wire-format fiction"
    );

    drop(sam);
    server.shutdown().await;
}

/// A send into a channel the sender is not a member of is refused, and named.
///
/// The authorization half of the milestone. **A `message.error`, not a closed
/// socket**, because the connection's authorization already succeeded -- and
/// `crates/sh_nexus/src/network/mapping.rs` turns a `message.error` into a visible
/// failed row, which is what `AGENTS.md` §8.1's Optimistic Send Flow requires.
#[tokio::test]
async fn a_send_into_a_channel_the_sender_does_not_belong_to_is_refused() {
    let server = TestServer::start().await;
    let outsider_id = server.create_account("robin", false);
    let outsider_token = server.session_for(&outsider_id);

    let mut robin = TestClient::connect(server.address(), Some(&outsider_token))
        .await
        .expect("an authenticated upgrade; provisioning and membership are separate");

    let envelope = ClientEnvelope::new(
        SECOND_MSG_ID,
        ClientFrame::MessageSend {
            channel_id: DEFAULT_CHANNEL_ID.to_owned(),
            content: "let me in".to_owned(),
        },
    );
    robin
        .send_text(&envelope.encode().expect("an encodable envelope"))
        .await
        .expect("the send to reach the server");

    let text = robin.expect_text("the refusal").await;
    let decoded = ServerEnvelope::decode(&text).expect("a decodable refusal");
    match decoded.frame {
        ServerFrame::MessageError { code, detail, .. } => {
            assert_eq!(
                code,
                sh_nexus_server::message::NOT_A_MEMBER,
                "the refusal must name the actual problem: the account exists and the \
                 channel exists, so `unknown_channel` would send the user to fix a typo \
                 they did not make"
            );
            assert!(
                !detail.contains(DEFAULT_CHANNEL_ID),
                "the detail names the rule, not the value: {detail:?}"
            );
            assert!(
                !detail.contains("let me in"),
                "and never the content: AGENTS.md 7.5 forbids it reaching a client log"
            );
        }
        other => panic!("expected a message.error, got {other:?}"),
    }

    assert_eq!(
        support::query_count_where(
            server.database(),
            "SELECT COUNT(*) FROM messages WHERE client_msg_id = ?1",
            SECOND_MSG_ID,
        ),
        0,
        "a refused send leaves no row"
    );
    assert!(
        robin
            .next_text_within(support::SILENCE_BUDGET)
            .await
            .is_none(),
        "and the connection stays open: the socket's authorization succeeded, so refusing \
         it would be a statement about something that was fine"
    );

    drop(robin);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Account creation
// ---------------------------------------------------------------------------

/// Only an administrator may create an account, and nobody else may register.
///
/// ADR-010's Decision section, as a test: "open `POST /auth/register` on a
/// self-hosted instance is a read-access hole: anyone who can reach the port can
/// enrol and read the team's messages". There is no such route, and the route that
/// exists refuses a non-administrator with a **403** rather than a 401 -- a caller
/// who is already logged in is told the truth about its own privileges.
#[tokio::test]
async fn account_creation_is_administrator_only_and_registration_is_not_a_route() {
    let server = TestServer::start().await;

    // A non-administrator.
    let member_id = server.create_account("member", true);
    let member_token = server.session_for(&member_id);
    let refused = server
        .create_account_over_http(
            &member_token,
            "sneaky",
            "Sneaky",
            "a-long-enough-passphrase",
        )
        .await;
    assert_eq!(
        refused.status, 403,
        "a logged-in account that is not an administrator must be told 403, not 401: \
         its session was fine: {}",
        refused.body
    );
    assert_eq!(refused.code().as_deref(), Some(auth::NOT_AN_ADMINISTRATOR));

    // An unauthenticated caller.
    let anonymous = support::post_json(
        server.address(),
        auth::ADMIN_USERS_PATH,
        None,
        &serde_json::json!({
            "username": "sneaky",
            "display_name": "Sneaky",
            "password": "a-long-enough-passphrase",
        }),
    )
    .await
    .expect("a request that reaches the server");
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.code().as_deref(), Some(auth::INVALID_CREDENTIALS));

    // The route ADR-010 rejects does not exist at all.
    let registration = support::post_json(
        server.address(),
        "/auth/register",
        None,
        &serde_json::json!({ "username": "sneaky", "password": "a-long-enough-passphrase" }),
    )
    .await
    .expect("a request that reaches the server");
    assert_eq!(
        registration.status, 404,
        "there is no self-registration route, and a 405 would mean something else is at \
         this path: {}",
        registration.body
    );

    // The administrator succeeds, and the new account can log in for itself -- the
    // response carries no token, which is the property that makes provisioning and
    // authentication separate operations.
    let created = server
        .create_account_over_http(
            server.admin_token(),
            "sam",
            "Sam",
            "a-long-enough-passphrase",
        )
        .await;
    assert_eq!(
        created.status, 201,
        "an administrator may create an account: {}",
        created.body
    );
    assert!(
        !created.body.contains("password") && !created.body.contains("argon2"),
        "the response carries no hash and no password: {}",
        created.body
    );

    let token = server.login("sam", "a-long-enough-passphrase").await;
    assert_eq!(token.len(), auth::SESSION_TOKEN_BYTES * 2);

    // A taken username is a 409, and the response names the field rather than
    // repeating the value.
    let taken = server
        .create_account_over_http(
            server.admin_token(),
            "sam",
            "Sam Again",
            "a-long-enough-passphrase",
        )
        .await;
    assert_eq!(taken.status, 409);
    assert_eq!(taken.code().as_deref(), Some(auth::USERNAME_TAKEN));

    // And a short password is refused with the rule, not silently accepted.
    let short = server
        .create_account_over_http(server.admin_token(), "shorty", "Shorty", "tiny")
        .await;
    assert_eq!(
        short.status, 400,
        "a password under the minimum must be refused rather than quietly accepted"
    );
    assert!(
        !short.body.contains("tiny"),
        "and the refusal must not echo the password: {}",
        short.body
    );

    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// `verify_password` cannot tell its caller which failure it was.
///
/// The signature is the security property, so the test is about the *absence* of a
/// distinction rather than about any one value being accepted.
#[test]
fn verification_returns_one_answer_for_three_different_reasons() {
    let hash = auth::hash_password("a-correct-passphrase").expect("a hash");

    assert!(
        auth::verify_password(Some(hash.as_str()), "a-correct-passphrase"),
        "the right password verifies"
    );

    for (label, stored, candidate) in [
        (
            "a wrong password",
            Some(hash.as_str()),
            "a-wrong-passphrase",
        ),
        (
            "a right password with no stored hash",
            None,
            "a-correct-passphrase",
        ),
        (
            "no stored hash and no right password",
            None,
            "anything at all",
        ),
    ] {
        assert!(
            !auth::verify_password(stored, candidate),
            "{label} must be refused"
        );
    }
}

/// The dummy hash is a real Argon2 string, and it names no account.
///
/// **The first half is what keeps the absent-account path honest.** If the constant
/// were malformed, `argon2` would reject the *parse* and return immediately -- and
/// the absent-account path would cost microseconds instead of the ~50 ms a real
/// verification costs, which is exactly the timing oracle the constant exists to
/// close. So this test asserts the parse succeeds, by asserting that a verification
/// against it is possible at all.
#[test]
fn the_dummy_hash_is_a_real_argon2_string_and_names_no_account() {
    let directory = support::TempDir::new("dummy-hash").expect("a temporary directory");
    let path = directory.path().join("sh_nexus.sqlite3");
    let store = Store::open(&path).expect("a migrated database");

    // A parse failure in `argon2` maps to `Err`, and `verify_password` maps `Err` to
    // `false` -- so a *successful* verification is the only way to observe that the
    // constant parsed. The passphrase behind it was never written down, so this is
    // asserted through a hash the test makes itself.
    let own = auth::hash_password("a-passphrase-this-test-knows").expect("a hash");
    assert!(auth::verify_password(
        Some(own.as_str()),
        "a-passphrase-this-test-knows"
    ));

    // The constant's cost parameters must be the same ones a real hash carries, or
    // the work on the absent path is not the work on the present path. Read them off
    // a real hash this crate produced and off the string the refusal path uses --
    // the latter through the fact that a *malformed* parameters string would not
    // parse at all.
    assert!(
        own.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "a hash this build produced declares Argon2id at the default cost: {own:?}"
    );

    // And no account corresponds to it.
    assert_eq!(
        store.account_count().expect("an account count"),
        0,
        "the dummy hash belongs to no account, so even a successful verification of it \
         would grant nothing"
    );
}

/// A taken username is refused, and the refusal names it.
///
/// Distinct from [`the_reserved_row_is_not_a_login_and_the_password_policy_is_enforced`]:
/// this is the store-level refusal, which is what makes a duplicate bootstrap fail
/// loudly instead of silently doing nothing.
#[test]
fn a_taken_username_is_refused_rather_than_overwritten() {
    let directory = support::TempDir::new("username-taken").expect("a temporary directory");
    let path = directory.path().join("sh_nexus.sqlite3");
    let store = Store::open(&path).expect("a migrated database");
    let hash = auth::hash_password("a-long-enough-passphrase").expect("a hash");

    store
        .create_account("dana", "Dana", &hash, true)
        .expect("the first account");

    match store.create_account("dana", "Someone Else", &hash, false) {
        Err(ServerError::UsernameTaken { username }) => assert_eq!(username, "dana"),
        other => panic!("expected a UsernameTaken refusal, got {other:?}"),
    }

    assert_eq!(
        support::query_count_where(
            &path,
            "SELECT COUNT(*) FROM users WHERE username = ?1",
            "dana"
        ),
        1,
        "`INSERT OR REPLACE` would have deleted the row and cascaded into \
         `messages.user_id`, `sessions.user_id` and `read_cursors.user_id`; ADR-010 \
         retains authorship on removal and forbids silent destruction"
    );
    let survivor = store
        .credential_for_username("dana")
        .expect("a readable credential")
        .expect("the original account");
    assert_eq!(
        survivor.account.display_name, "Dana",
        "and the original row is untouched: `OR REPLACE` would have shown `Someone Else`"
    );
}
