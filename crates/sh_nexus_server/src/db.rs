//! Persistence: one SQLite file, WAL mode, versioned migrations.
//!
//! # Why SQLite and why one file
//!
//! ADR-010 decides it, and the decision is what makes this module's shape what
//! it is: a team is tens to hundreds of users, the whole deployment is a binary
//! and a database file, and **a backup is a file copy**. There is no pool to
//! configure, no host to provision, and no separate lifecycle to reason about.
//!
//! # The connection model, and why `Store` is not `async`
//!
//! [`Store`] holds one [`rusqlite::Connection`] behind a
//! [`std::sync::Mutex`]. rusqlite is synchronous, and this module does not
//! pretend otherwise: every method here blocks, takes a lock, and returns.
//!
//! That is a deliberate choice over an `async` facade, and the reason is
//! visibility. `AGENTS.md` §2.3 forbids blocking work on a thread that has
//! something else to do, so the caller must run these calls somewhere it is
//! acceptable to block -- `tokio::task::spawn_blocking`, which is what
//! [`crate::message`] does. If `Store::accept_message` were `async` and awaited
//! the lock internally, the blocking would still happen; it would just be
//! invisible at the call site, on a runtime worker thread, with nothing in the
//! signature saying so. The synchronous signature is the reminder.
//!
//! The mutex means writes are serialized inside one process. It is *not* what
//! makes dedupe correct: that is the unique index and the single transaction
//! below, either of which holds with two processes on one file. The mutex is a
//! throughput property. Keeping those two apart matters, because the property
//! `AGENTS.md` §7.4 asks for has to survive the next deployment change.
//!
//! # Schema
//!
//! Five tables and two migrations, and two of them are not exercised by every
//! caller. That is the interesting part, so it is stated rather than implied:
//!
//! | Table | Exercised here | Why it exists anyway |
//! |---|---|---|
//! | `users` | one seeded row plus every real account | `messages.user_id` has a foreign key, so the row has to exist for the milestone's own foreign keys to mean anything. |
//! | `channels` | one seeded row | Channel provisioning is the REST milestone's job. A send naming an unknown channel is refused, not silently created. |
//! | `messages` | the whole round trip | The one working path. |
//! | `sessions` | every WebSocket handshake | A revoked session has to be distinguishable from a live one without reading the token, which is why the row is keyed on the hash. |
//! | `read_cursors` | **no** | Required by ADR-010 "from the first migration". |
//! | `channel_members` | yes, by `message.rs`'s authorization check **and by the catch-up read** | Membership is an authorization concern, and reading a channel's history is as much a question of membership as writing to it is. |
//!
//! **The catch-up read is [`Store::read_since`], and it is the second way
//! `messages` is exercised.** It is keyed on `(channel_id, accepted_at_unix_ms)`
//! rather than on an id, because `AGENTS.md` §7.4's resume cursor is a time and
//! nothing in this schema is a per-channel sequence number — the channel's
//! `last_message_at_unix_ms` column is the upper bound, and the cursor a client
//! holds is a lower one. **The two are deliberately the same column in opposite
//! directions**, and that is what makes "resume from `last_message_at`" a
//! statement about storage rather than about a convention.
//!
//! **`read_cursors` is the one to read the ADR for.** ADR-010's decision is
//! per-`(user, channel)` read state, and its stated reason is that the client's
//! `AppState` unread set is one global `BTreeSet<(String, Uuid)>` with no user in
//! it -- correct for a client, which only ever holds *this* user's unread, and
//! wrong for a server, where a global flag means user B sees as read what user A
//! read. Retro-fitting that after the first migration is a data migration over
//! live rows; putting it in migration 1 costs one `CREATE TABLE`.
//!
//! It is deliberately **not** read or written anywhere in this milestone. A table
//! nothing queries is not a feature; it is the shape of the decision, taken at
//! the only moment it is cheap.
//!
//! # The reserved author, and why the row outlives the role
//!
//! [`UNATTRIBUTED_USER_ID`] used to be the author of *every* message, because
//! `PLAN.md` §6's `message.send` carries only `channel_id` and `content` and
//! authorship was meant to come from an authenticated session. There is one now,
//! so the placeholder is **no longer the author of anything**: [`Store::accept_message`]
//! takes the author's id from the connection's session and stores that.
//!
//! **The row itself is still seeded, and that is a decision with a reason rather
//! than an oversight.** ADR-010 decides that *"message authorship is retained
//! when a user is removed"* -- teams want the audit trail, and deleting a person
//! must not silently rewrite history. A database migrated from schema 1 holds
//! `messages` rows whose `user_id` points at this row, and `foreign_keys` is on.
//! Stopping the seed would leave those files with a dangling reference that the
//! next `INSERT INTO messages` in the same transaction would not even notice,
//! and the transcript would render a blank author -- which is the one outcome
//! `network/mapping.rs` documents as worse than the message not arriving.
//!
//! So the row is kept as a **historical author and nothing else**: it has no
//! `password_hash` (migration 2's column is nullable for exactly this row), so it
//! can never log in, and no new message is ever attributed to it.
//!
//! The alternative that was rejected -- deleting the row -- fails ADR-010's
//! retention rule outright, and the alternative before that -- a blank `user_id`
//! -- was rejected for a concrete reason rather than taste: `WireMessage`'s own
//! boundary (`crates/sh_nexus/src/network/mapping.rs`) rejects a blank
//! `user_id`, so a blank one makes this server emit a `message.ack` its own
//! client refuses to decode, and the resulting bug would present as a client
//! defect.
//!
//! # Why `sessions` is keyed on a hash and not on the token
//!
//! The key of the `sessions` table is `token_hash`, a SHA-256 digest, and never
//! the token. That single choice is what makes ADR-010's revocation requirement
//! cheap: a backup of `sh_nexus.db` is a file copy, so a file copy that contained
//! live tokens would be a file copy that contained every session on the
//! instance. With the hash as the key, a leaked file yields digests, and a digest
//! cannot be presented in an `Authorization` header.
//!
//! It also means the lookup is a single indexed `SELECT` rather than a
//! "compare every row" pass, which is what lets the same fast hash be used for
//! every WebSocket handshake. `auth.rs` gives the full argument for why that hash
//! is `sha2` and not `argon2`, which is the opposite of the answer for a
//! password.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sh_nexus_wire::dto::WireMessage;
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::{Result, ServerError};
use crate::time;

/// The schema version this build reads and writes.
///
/// Stored in SQLite's `user_version` pragma rather than in a table of its own:
/// the pragma lives in the database header, is transactional, and cannot be
/// out of step with the schema it describes -- a `schema_version` table can be,
/// because a transaction that inserts the version and the schema it stamps is
/// still two statements someone can interrupt.
pub const SCHEMA_VERSION: i64 = 2;

/// The id every message was attributed to before this build had authentication.
///
/// See the module docs on why the row outlives the role. **Not a user, not an
/// account, and not a login**: no message is attributed to it by this build.
pub const UNATTRIBUTED_USER_ID: &str = "u_unattributed";

/// The login handle of [`UNATTRIBUTED_USER_ID`].
pub const UNATTRIBUTED_USERNAME: &str = "unattributed";

/// The display name of [`UNATTRIBUTED_USER_ID`].
///
/// Renders in a transcript as a word rather than as an empty bubble, because
/// `network/mapping.rs` documents an empty display name as "a message attributed
/// to nobody, which is worse than the message not arriving". **Only a transcript
/// that contains a message from before schema 2 will ever show it.**
pub const UNATTRIBUTED_DISPLAY_NAME: &str = "Unattributed (written before authentication)";

/// The id of the one channel this milestone seeds.
///
/// See the module docs on why seeding beats implicit creation.
pub const DEFAULT_CHANNEL_ID: &str = "c_general";

/// The display name of [`DEFAULT_CHANNEL_ID`].
pub const DEFAULT_CHANNEL_NAME: &str = "general";

/// How long a statement waits for SQLite's write lock before failing.
///
/// Five seconds. Long enough that a burst of sends from a few dozen clients
/// queues rather than errors, short enough that a genuinely stuck process is
/// reported rather than hanging a request until the client gives up. The client's
/// own send timeout is the outer bound; this is only the inner one.
pub const BUSY_TIMEOUT_MS: i64 = 5_000;

/// What was being locked, for [`ServerError::LockPoisoned`].
///
/// A compile-time constant, so the message is greppable and the type system
/// refuses a caller's user text in that position.
const CONNECTION: &str = "SQLite connection";

/// Migration 1: the whole schema.
///
/// **No `IF NOT EXISTS`, on purpose.** A migration is allowed to fail loudly when
/// the file is not in the state it expects, because a migration that skips a
/// statement it did not apply leaves a schema that is *partly* migrated and
/// fully plausible. The pre-flight check in [`migrate`] is what turns "this file
/// has no version stamp but does have tables" into a refusal instead of a
/// mid-migration error.
///
/// `STRICT` on every table because it is enforced by SQLite, not by a
/// convention: a `TEXT` id column in a strict table cannot hold the integer 1,
/// and a `channel_id` that is sometimes an integer and sometimes a string is a
/// foreign key that silently stops matching.
const MIGRATION_1: &str = r#"
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

/// Migration 2: credentials and sessions. **Strictly additive.**
///
/// # What it adds, and the two columns that carry all the weight
///
/// ```text
/// ALTER TABLE users ADD COLUMN password_hash TEXT;   -- nullable
/// ALTER TABLE users ADD COLUMN is_admin      INTEGER NOT NULL DEFAULT 0
///                                           CHECK (is_admin IN (0, 1));
/// CREATE TABLE sessions (
///     token_hash         TEXT    NOT NULL PRIMARY KEY,
///     user_id            TEXT    NOT NULL REFERENCES users (id),
///     created_at_unix_ms INTEGER NOT NULL,
///     expires_at_unix_ms INTEGER NOT NULL,
///     revoked_at_unix_ms INTEGER,
///     CHECK (expires_at_unix_ms > created_at_unix_ms)
/// ) STRICT;
/// CREATE INDEX sessions_user  ON sessions (user_id);
/// CREATE INDEX sessions_expiry ON sessions (expires_at_unix_ms);
/// ```
///
/// # What it does to an existing database, exactly
///
/// **Nothing is rewritten, nothing is deleted, and every row survives.** Both
/// `ALTER`s are `ADD COLUMN`, which SQLite applies in place by appending to the
/// row format's null-bitmap; a table that was `STRICT` stays `STRICT`, and the
/// `CHECK` constraints migration 1 declared still apply to every column that
/// existed before this one. Concretely, on a file migrated from schema 1:
///
/// - `users.password_hash` is `NULL` for all existing rows, so **no existing
///   account can log in until an administrator sets its password.** That is the
///   correct outcome rather than a gap: there is no password to recover, and
///   inventing one would let anybody who guessed a username in.
/// - `users.is_admin` is `0` for all existing rows, so a file that had accounts
///   before this migration has **no administrator** unless an operator is made
///   one. `auth::bootstrap` does not help there -- it only runs on an instance
///   with no account at all -- which is why [`Store::promote_to_administrator`]
///   exists and why its existence is stated here rather than discovered.
/// - `sessions` starts empty, and no `messages` row changes. The reserved
///   [`UNATTRIBUTED_USER_ID`] row is untouched, which is what keeps the
///   transcripts of a pre-auth instance readable (see the module docs).
///
/// **The version stamp is written last, inside the same transaction**, so a
/// crash between the two `ALTER`s leaves the file at schema 1 and the next start
/// retries the whole migration rather than half-applying it. That is what
/// [`migrate`]'s per-migration conditionals are for.
///
/// # Why `is_admin` is here at all, since it was not asked for by name
///
/// ADR-010 decides "an administrator creates accounts; the first account is
/// created at startup or from an operator-supplied environment variable". An
/// administrator that has to be **inferred** -- the earliest `created_at`, or
/// the row named `admin`, or whichever account logged in first -- is a privilege
/// rule that cannot be audited by opening the file, and ADR-010's strongest
/// argument for this whole design is precisely that a team can verify the server's
/// behaviour by reading `sh_nexus.db` (its Context section calls this "a
/// stronger property than any policy statement"). So the flag is a stored column,
/// defaulted to the least-privileged value, and it is checkable with one query:
///
/// ```sql
/// SELECT id, username FROM users WHERE is_admin = 1;
/// ```
///
/// The alternative -- "the first account created is the administrator, inferred
/// from `MIN(created_at_unix_ms)`" -- was rejected for a concrete reason: two
/// accounts created inside the same millisecond tie, and a tie broken by `id` is
/// a privilege grant decided by a UUID. The rule would also be unrecoverable: an
/// administrator could not transfer the role, because transferring it would mean
/// deleting the row that *is* the rule.
///
/// # Why `CHECK (expires_at_unix_ms > created_at_unix_ms)`
///
/// A session that expires the instant it was created is not a session, it is a
/// bug that reads as a login failure for every user. The constraint turns that
/// class of mistake into a `SQLITE_CONSTRAINT` at the only place it can be
/// introduced -- the insert -- rather than into a support question later. It is
/// enforced by SQLite rather than by a convention, which is the same argument
/// `STRICT` makes for the id columns in migration 1.
const MIGRATION_2: &str = r#"
ALTER TABLE users ADD COLUMN password_hash TEXT;

ALTER TABLE users ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0 CHECK (is_admin IN (0, 1));

CREATE TABLE sessions (
    token_hash         TEXT    NOT NULL PRIMARY KEY,
    user_id            TEXT    NOT NULL REFERENCES users (id),
    created_at_unix_ms INTEGER NOT NULL,
    expires_at_unix_ms INTEGER NOT NULL,
    revoked_at_unix_ms INTEGER,
    CHECK (expires_at_unix_ms > created_at_unix_ms)
) STRICT;

CREATE INDEX sessions_user ON sessions (user_id);

CREATE INDEX sessions_expiry ON sessions (expires_at_unix_ms);
"#;

/// What [`Store::accept_message`] did with a send.
///
/// Two outcomes, not one, because the caller has to behave differently for each:
/// a duplicate is acknowledged again but **not** rebroadcast, and collapsing them
/// into a single `WireMessage` would make the second behaviour a decision the
/// caller has to reconstruct from a value comparison.
#[derive(Debug, Clone)]
pub enum AcceptOutcome {
    /// The send was new. The message is stored, and this is the row.
    Inserted(WireMessage),
    /// The send had been accepted before, under the same `client_msg_id`. The
    /// message is the row that already existed, byte for byte.
    Duplicate(WireMessage),
}

/// One message as this milestone stores it.
///
/// A private struct rather than reading straight into a [`WireMessage`], because
/// reading a row happens inside a `rusqlite` closure whose error type is
/// `rusqlite::Error`, and the timestamp conversion can fail with
/// [`ServerError`]. Splitting the read from the conversion lets each layer fail
/// with its own error instead of flattening one into the other.
#[derive(Debug)]
struct StoredMessage {
    id: String,
    client_msg_id: String,
    channel_id: String,
    user_id: String,
    content: String,
    accepted_at_unix_ms: i64,
}

impl StoredMessage {
    /// Projects the stored row onto the protocol's message shape.
    ///
    /// `edited_at`, `thread_id`, `reactions` and `attachments` are absent rather
    /// than null-and-guessed because **no table exists for them yet**: this
    /// milestone's schema has no `message_edits`, no `reactions`, and no
    /// `attachments`. Emitting an empty list would claim the server knows a
    /// message has no reactions, which is a different statement from this server
    /// not implementing reactions -- and it would be a false one the moment the
    /// next milestone lands.
    fn into_wire_message(self) -> Result<WireMessage> {
        Ok(WireMessage {
            timestamp: time::from_unix_millis(self.accepted_at_unix_ms)?,
            id: self.id,
            client_msg_id: self.client_msg_id,
            channel_id: self.channel_id,
            user_id: self.user_id,
            content: self.content,
            edited_at: None,
            reactions: Vec::new(),
            thread_id: None,
            attachments: Vec::new(),
        })
    }
}

/// What a send's target channel allows.
///
/// Three answers, and the split between the first two is a *user-facing* decision
/// rather than a security one: see [`Store::channel_access`] for why existence is
/// checked first and what asking in that order does and does not reveal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAccess {
    /// The channel exists and the user is a member of it. The send may proceed.
    Allowed,
    /// The channel exists and the user is not a member of it.
    NotAMember,
    /// No channel on this instance has that id.
    UnknownChannel,
}

/// A message row, read by `client_msg_id`.
///
/// The read is keyed on `client_msg_id` and not on the id this call just minted,
/// so the duplicate path returns the *original* row. That is the whole point: an
/// ack that carried a fresh id would tell the client its replayed send became a
/// second message.
const SELECT_BY_CLIENT_MSG_ID: &str = "SELECT id, client_msg_id, channel_id, user_id, \
     content, accepted_at_unix_ms FROM messages WHERE client_msg_id = ?1";

/// The catch-up read behind a `resync`: one channel, one lower bound, one page.
///
/// **`accepted_at_unix_ms >= ?2` and not `>`, and that is the whole reason this
/// method exists in this shape.** The acceptance clock has millisecond
/// resolution, so two messages can share an instant; a strict bound read from a
/// cursor pointing at the first of them skips the second permanently, because the
/// cursor has already moved past it and nothing will ask again.
/// `sh_nexus_wire::frame::ClientFrame::Resync`'s `after` carries the full argument,
/// and the client-side rule that terminates the resulting overlap — advance to the
/// batch maximum *including duplicates*, ask again only while a batch comes back
/// full — is what makes the inclusive bound cost one duplicate row rather than an
/// unbounded loop.
///
/// **`ORDER BY accepted_at_unix_ms, id` is a total order, and it is exact rather
/// than a tie-break guess.** `mint_message_id` packs
/// `(accepted_at_unix_ms << 64) | sequence` with a per-store atomic counter, so the
/// id's high bits are the clock and its low bits are acceptance order within the
/// millisecond. Sorting by the pair therefore agrees with acceptance order in every
/// case, which is what lets a replayed batch arrive in the order it was accepted.
/// `messages_channel_order` is exactly this index.
const SELECT_SINCE: &str = "SELECT id, client_msg_id, channel_id, user_id, \
     content, accepted_at_unix_ms FROM messages \
     WHERE channel_id = ?1 AND accepted_at_unix_ms >= ?2 \
     ORDER BY accepted_at_unix_ms, id LIMIT ?3";

/// One account that can log in.
///
/// **Not the reserved [`UNATTRIBUTED_USER_ID`] row**, which has no
/// `password_hash` and is therefore not an account by this definition. The
/// distinction is the reason this is a struct rather than a row type: the
/// `users` table holds one kind of row that is a historical artefact and another
/// that is a person, and the difference is a nullable column rather than a table.
///
/// `Serialize` is derived, and it is safe because every field here is either
/// server-issued or already public on the instance: the only field that would not
/// be is `password_hash`, and that one is deliberately **not** on this type -- it
/// lives on [`Credential`], which is the crate's one hand-written `Debug`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Account {
    /// The `users.id` value, server-issued and stable.
    pub id: String,
    /// The login handle. Unique, case-sensitively, by `users_username_unique`.
    pub username: String,
    /// The name to render in a transcript.
    pub display_name: String,
    /// Whether this account may create other accounts.
    ///
    /// One bit, and deliberately the only privilege this milestone models: ADR-010
    /// records "no OAuth, no SSO" and closed registration, so the entire
    /// authorization question for a new account is "may this one create it".
    pub is_admin: bool,
}

/// What a login needs to decide, read by username.
///
/// A separate type from [`Account`] on purpose: this is the **only** struct in the
/// server that holds a password hash, so it is the only one whose `Debug` has to
/// be hand-written, and `Account` is then free to derive one because it has
/// nothing sensitive in it.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    /// The account the password belongs to.
    pub account: Account,
    /// The stored PHC string, or `None` for a row that cannot log in.
    ///
    /// `Option` rather than an empty string, because the two are different facts:
    /// "this row was never given a password" (the reserved historical author) and
    /// "this row was given an empty password" are not the same statement, and
    /// [`crate::auth::verify_password`] is written so that only one of them can be
    /// true.
    pub password_hash: Option<String>,
}

impl std::fmt::Debug for Credential {
    /// **Prints whether a hash exists and never what it is.**
    ///
    /// Hand-written for the same reason [`Store`]'s is, and with more at stake: an
    /// `Argon2` PHC string is not a secret in the way a password is, but it is
    /// enough to mount an offline dictionary attack against whatever the person
    /// chose, and a `Debug` impl that can reach a `warn!` is an impl that has to
    /// be trusted not to. Trusting a derive here would be trusting a format
    /// change.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("account", &self.account)
            .field(
                "password_hash",
                &self.password_hash.as_ref().map(|_| "<argon2>"),
            )
            .finish()
    }
}

/// What a presented session token turned out to be.
///
/// Four outcomes, not two, and the split is what makes the *logs* honest without
/// making the *response* distinguishable: [`crate::auth`] answers all four with
/// one 401, because a client that can tell "your token expired" from "your token
/// was revoked" learns something about a token it presented, and
/// `AGENTS.md` §7.5's rule is that this server does not explain itself to a peer.
/// The operator gets the distinction in a log line carrying an id and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The token names a session that is neither revoked nor past its expiry.
    Live {
        /// The authenticated user. This is what a message is attributed to.
        user_id: String,
        /// Whether that user may create accounts.
        is_admin: bool,
        /// When the session stops being live, for the log line and for nothing else.
        expires_at_unix_ms: i64,
    },
    /// The token names a session that was explicitly revoked.
    ///
    /// **Checked before expiry, and that order is the point.** A revoked session
    /// dies immediately, which is the whole reason ADR-010 rejected a
    /// self-contained token format: expelling a member has to kill the socket they
    /// are holding, and a token that only stopped working when it aged out would
    /// leave it alive for the rest of its lifetime.
    Revoked {
        /// When it was revoked, so a log line can say how long it survived.
        revoked_at_unix_ms: i64,
    },
    /// The token names a session that is past `expires_at_unix_ms`.
    Expired {
        /// When it stopped being live.
        expires_at_unix_ms: i64,
    },
    /// No session has this token hash.
    ///
    /// Also the answer for a token that was never valid, and the two are the same
    /// outcome on purpose: there is nothing an operator can do about either.
    Absent,
}

/// The handle every other module holds to the database.
///
/// Cheap to clone (`Arc` inside, per `AGENTS.md` §2.3) and safe to share across
/// axum's worker tasks, which is what `#[derive(Clone)]` plus `Send + Sync` on
/// the inner `Mutex<Connection>` buys.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    sequence: Arc<AtomicU64>,
}

impl std::fmt::Debug for Store {
    /// Hand-written rather than derived: the derived form would print the whole
    /// connection's configuration, and a `Debug` impl that can reach a log line
    /// is a `Debug` impl that has to be trusted not to. This one prints the file
    /// and nothing else -- which is what an operator needs and is not a secret.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("file", &self.path())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Opens (or creates) the database file, applies migrations, and seeds the
    /// two rows the round trip needs.
    ///
    /// **Blocking.** See the module docs on why the signature says so.
    ///
    /// # Arguments
    ///
    /// * `path` - the SQLite file. Its parent directory must already exist;
    ///   `Connection::open` creates the file, not the directory, and reporting
    ///   "unable to open database file" for a missing directory sends an
    ///   operator looking in the wrong place.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a file that cannot be opened or a schema that
    /// cannot be read -- including the common operator mistake of a
    /// `SH_NEXUS_DB` whose parent directory does not exist, which the driver's
    /// "unable to open database file" reports with the full path in the message.
    /// That is [`ServerError::Sqlite`] and not [`ServerError::Io`]: `rusqlite`
    /// surfaces SQLite's own `SQLITE_CANTOPEN` rather than the operating
    /// system's error, so there is no `io::Error` to wrap.
    ///
    /// [`ServerError::JournalModeUnavailable`] if WAL cannot be enabled,
    /// [`ServerError::SchemaTooNew`] or [`ServerError::UnversionedSchema`] if the
    /// file is not one this build may migrate, and [`ServerError::LockPoisoned`]
    /// if an earlier panic left the connection unusable.
    pub fn open(path: &Path) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        configure(&connection)?;
        migrate(&mut connection)?;

        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
            sequence: Arc::new(AtomicU64::new(0)),
        };
        store.seed()?;
        info!(
            database = %path.display(),
            schema_version = SCHEMA_VERSION,
            "database ready"
        );
        Ok(store)
    }

    /// The database file's schema version, as stored.
    ///
    /// Public because it is the first question an operator has about a
    /// self-hosted instance -- "is my file on the schema this build expects?" --
    /// and because a backup restored onto the wrong version is otherwise
    /// invisible until a send fails.
    pub fn schema_version(&self) -> Result<i64> {
        let connection = self.lock()?;
        read_user_version(&connection)
    }

    /// The path the store was opened on, for diagnostics.
    fn path(&self) -> Option<String> {
        self.lock()
            .ok()
            .and_then(|connection| connection.path().map(str::to_owned))
    }

    /// Accepts a send, or reports that this `client_msg_id` was already accepted.
    ///
    /// This is the milestone's one correctness-critical operation, and the dedupe
    /// is enforced by the database rather than by the code around it:
    ///
    /// ```text
    /// BEGIN IMMEDIATE
    ///   -- the channel must exist, or the send is refused
    ///   SELECT 1 FROM channels WHERE id = ?1
    ///   INSERT INTO messages (...) VALUES (...) ON CONFLICT(client_msg_id) DO NOTHING
    ///   -- rows changed == 1 means new; 0 means this id is already stored
    ///   UPDATE channels SET last_message_at_unix_ms = ?2 WHERE id = ?1   -- only if new
    ///   SELECT ... FROM messages WHERE client_msg_id = ?1
    /// COMMIT
    /// ```
    ///
    /// Three properties follow, and each of them is the reason the code is shaped
    /// this way rather than a read-then-write:
    ///
    /// 1. **No duplicate row.** The unique index on `client_msg_id` is what
    ///    decides, and `ON CONFLICT DO NOTHING` makes losing the race an ordinary
    ///    outcome rather than an error. A `SELECT` before the `INSERT` would be a
    ///    time-of-check-to-time-of-use bug under two concurrent replays: both read
    ///    "not present" and both insert.
    /// 2. **The duplicate resolves to the original row.** The final `SELECT`
    ///    happens inside the same transaction, so the winner of the race has
    ///    committed and the row it wrote is visible. The ack therefore carries the
    ///    id and timestamp the message was *first* accepted at, which is what lets
    ///    the client reconcile a replay against a row it may already be showing.
    /// 3. **`last_message_at` moves only for a new message.** The channel's resume
    ///    cursor is the upper bound of a resync
    ///    (`sh_nexus_wire::dto::WireChannel::last_message_at`), and advancing it
    ///    for a replay would make a client that resyncs from it skip the message
    ///    the replay was about.
    ///
    /// `BEGIN IMMEDIATE` rather than the default deferred transaction because the
    /// statement sequence reads before it writes, and a deferred transaction that
    /// reads and then writes can fail to upgrade its lock at the worst possible
    /// moment.
    ///
    /// # Arguments
    ///
    /// * `client_msg_id` - the peer-generated id, as untrusted text. Deduplicated
    ///   exactly as received: not trimmed, not case-folded, not normalised. The
    ///   client's boundary is what guarantees it is a UUID
    ///   (`network/mapping.rs::parse_client_msg_id`), and a server that "fixed" a
    ///   malformed id into a different one would key the dedupe on something the
    ///   sender never sent.
    /// * `channel_id` - the target channel. Must already exist.
    /// * `user_id` - the author, **from the connection's session and never from
    ///   the frame**. `PLAN.md` §6's `message.send` carries no author, so the only
    ///   value available here is the one `ws.rs` got from the handshake's
    ///   `Authorization` header; taking it from the frame instead would make
    ///   attribution a claim rather than a fact, which is the entire difference
    ///   between an authenticated server and an anonymous one. The foreign key
    ///   enforces that it names a real row, so a connection whose session named
    ///   nobody cannot get this far.
    /// * `content` - the message body, stored verbatim.
    ///
    /// # Errors
    ///
    /// [`ServerError::UnknownChannel`] if `channel_id` names no channel, and
    /// [`ServerError::Sqlite`] for a driver failure -- including the foreign-key
    /// violation an unknown `user_id` produces.
    /// [`ServerError::LockPoisoned`] if an earlier panic left the connection
    /// unusable.
    pub fn accept_message(
        &self,
        client_msg_id: &str,
        channel_id: &str,
        user_id: &str,
        content: &str,
    ) -> Result<AcceptOutcome> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if !channel_exists(&transaction, channel_id)? {
            debug!(
                channel_id,
                "refused a send naming a channel that does not exist"
            );
            return Err(ServerError::UnknownChannel {
                channel_id: channel_id.to_owned(),
            });
        }

        let accepted_at = time::now_unix_millis();
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let id = mint_message_id(accepted_at, sequence);

        let inserted = transaction.execute(
            "INSERT INTO messages
                 (id, client_msg_id, channel_id, user_id, content, accepted_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(client_msg_id) DO NOTHING",
            params![
                &id,
                client_msg_id,
                channel_id,
                user_id,
                content,
                accepted_at
            ],
        )?;

        let outcome = if inserted == 0 {
            AcceptOutcome::Duplicate(read_by_client_msg_id(&transaction, client_msg_id)?)
        } else {
            transaction.execute(
                "UPDATE channels SET last_message_at_unix_ms = ?1 WHERE id = ?2",
                params![accepted_at, channel_id],
            )?;
            AcceptOutcome::Inserted(read_by_client_msg_id(&transaction, client_msg_id)?)
        };

        transaction.commit()?;
        Ok(outcome)
    }

    /// Locks the connection, mapping poisoning rather than recovering from it.
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| ServerError::LockPoisoned {
                resource: CONNECTION,
            })
    }

    // -----------------------------------------------------------------------
    // The catch-up read
    // -----------------------------------------------------------------------

    /// Every message in `channel_id` accepted **at or after** `after_unix_ms`, in
    /// acceptance order, at most `limit` of them.
    ///
    /// **This is the per-channel resume cursor's reader, and the one method in
    /// this module that reads rather than writes.** `AGENTS.md` §7.4 asks for
    /// reconnection to "resume from the last known `last_message_at` cursor — never
    /// rely solely on 'live' delivery during a gap", and a single global cursor
    /// cannot do that: each channel has its own `last_message_at`, so one cursor is
    /// either too old (replaying what the user read) or too new (skipping a channel
    /// they had not caught up on). The cursor is an argument, per channel, because
    /// the shape of the answer has to be per channel too.
    ///
    /// ## The bound, and why the caller owns it
    ///
    /// `limit` is the caller's, not this method's, and the reason is that the
    /// caller is the one that has to write the rest of the batch back down a
    /// socket: `ws.rs` passes the same 256 that `Hub::CAPACITY` and the client's
    /// `MAX_OUTBOUND_FRAMES` use, so one catch-up batch costs one bounded buffer on
    /// each side. **A client that missed 10 000 messages therefore takes 40 round
    /// trips, which is correct and bounded** — `AGENTS.md` §7.1 forbids the
    /// unbounded alternative, and a reader with no limit is that alternative.
    ///
    /// ## An unknown channel is an error, not an empty batch
    ///
    /// **This is the failure the whole unit exists to close.** A channel this
    /// instance does not have and a channel the client has already caught up with
    /// return the same zero rows if existence is not checked, and a catch-up that
    /// "succeeded" while recovering nothing is worse than no catch-up at all,
    /// because it is invisible. The refusal is [`ServerError::UnknownChannel`],
    /// which is the same variant and therefore the same `unknown_channel` code
    /// [`crate::message`] reports for a send naming a channel that does not exist.
    ///
    /// The check costs one indexed `SELECT` and duplicates the existence half of
    /// [`Store::channel_access`], which `ws.rs` asks first for the membership
    /// question. **It is here anyway because this method is `pub`:** a caller that
    /// reached it without asking about membership must still not be told "you are
    /// up to date" about a channel that does not exist.
    ///
    /// # Arguments
    ///
    /// * `channel_id` - the channel to catch up. Must already exist.
    /// * `after_unix_ms` - the **inclusive** lower bound; see [`SELECT_SINCE`] for
    ///   why inclusive, and the client's advancement rule for why that terminates.
    /// * `limit` - the largest batch to return. A returned batch of exactly `limit`
    ///   rows means there may be more, which is how a truncated batch is
    ///   distinguishable from a caught-up channel.
    ///
    /// # Errors
    ///
    /// [`ServerError::UnknownChannel`] if `channel_id` names no channel,
    /// [`ServerError::Sqlite`] for a driver failure, and
    /// [`ServerError::TimestampOutOfRange`] if a stored millisecond value is not a
    /// representable instant. [`ServerError::LockPoisoned`] if an earlier panic
    /// left the connection unusable.
    ///
    /// **An empty batch is not an error.** A channel with nothing at or after the
    /// cursor is a caught-up channel, which is an ordinary answer to the question
    /// that was asked.
    pub fn read_since(
        &self,
        channel_id: &str,
        after_unix_ms: i64,
        limit: usize,
    ) -> Result<Vec<WireMessage>> {
        let connection = self.lock()?;

        if !channel_exists(&connection, channel_id)? {
            debug!(
                channel_id,
                after_unix_ms, "refused a catch-up naming a channel that does not exist"
            );
            return Err(ServerError::UnknownChannel {
                channel_id: channel_id.to_owned(),
            });
        }

        // `usize` is cast rather than bound directly because `rusqlite` has no
        // `ToSql` for it, and because a page bound that could exceed `i64::MAX`
        // would need a platform this server does not build for. The bound is a
        // named constant at every call site, so the cast is a formality rather
        // than a truncation risk.
        let mut statement = connection.prepare(SELECT_SINCE)?;
        let rows =
            statement.query_map(params![channel_id, after_unix_ms, limit as i64], |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    client_msg_id: row.get(1)?,
                    channel_id: row.get(2)?,
                    user_id: row.get(3)?,
                    content: row.get(4)?,
                    accepted_at_unix_ms: row.get(5)?,
                })
            })?;

        // Collected rather than mapped lazily, because the timestamp conversion
        // can fail with `ServerError` while the row closure's error type is
        // `rusqlite::Error` -- the same split `StoredMessage`'s own docs give.
        let mut batch = Vec::new();
        for row in rows {
            batch.push(row?.into_wire_message()?);
        }
        Ok(batch)
    }

    // -----------------------------------------------------------------------
    // Accounts
    //
    // Every method below is synchronous and blocking, for the same reason the
    // message path is: `db.rs`'s module docs. `auth.rs` puts them on a blocking
    // thread with `spawn_blocking`, which is the whole reason `Store`'s methods
    // are not `async`.
    // -----------------------------------------------------------------------

    /// How many accounts on this instance can log in.
    ///
    /// **Counted as "has a `password_hash`", not as "rows in `users`".** That is
    /// what makes the bootstrap decision work on a database migrated from schema
    /// 1: such a file has the reserved [`UNATTRIBUTED_USER_ID`] row and nothing
    /// else, and counting rows would report an account that cannot log in and skip
    /// the bootstrap forever. Counting the rows that *can* log in is the property
    /// the bootstrap actually needs -- "this instance has nobody to administer
    /// it" -- and it is the same question on a fresh file and on a migrated one.
    pub fn account_count(&self) -> Result<i64> {
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM users WHERE password_hash IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Creates an account, refusing rather than overwriting a taken username.
    ///
    /// `INSERT` and not `INSERT OR IGNORE` and not `INSERT OR REPLACE`, and all
    /// three refusals are load-bearing:
    ///
    /// - `OR IGNORE` would make a taken username a **silent no-op**, and
    ///   [`crate::auth`]'s bootstrap would then report success for an
    ///   administrator it did not create.
    /// - `OR REPLACE` deletes the conflicting row, which on `users` cascades into
    ///   `messages.user_id`, `sessions.user_id` and `read_cursors.user_id`. That is
    ///   ADR-010's "never destroy user data silently" violated in one clause.
    /// - The existence check below is what turns the unique-index violation into a
    ///   **typed** [`ServerError::UsernameTaken`] rather than a raw
    ///   `SQLITE_CONSTRAINT`. It runs inside the same transaction as the `INSERT`,
    ///   and the `INSERT`'s own constraint remains the authority: if the check and
    ///   the insert ever disagreed, the insert would lose and the caller would get
    ///   [`ServerError::Sqlite`] -- which is a server fault, and is what a lost race
    ///   genuinely is. The alternative -- matching on the driver's extended error
    ///   code -- would make this function's error vocabulary depend on a SQLite
    ///   constant number.
    ///
    /// The `is_admin` argument rather than an implicit rule is the reason
    /// `MIGRATION_2`'s comment can claim the privilege is auditable with one
    /// query: there is exactly one place in the server that writes the flag, and
    /// it is this function's parameter.
    ///
    /// # Errors
    ///
    /// [`ServerError::UsernameTaken`] if `username` already exists, and
    /// [`ServerError::Sqlite`] for anything else the driver reports.
    pub fn create_account(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
        is_admin: bool,
    ) -> Result<Account> {
        let now = time::now_unix_millis();
        let id = mint_user_id(now);
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let taken: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM users WHERE username = ?1",
                [username],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(ServerError::UsernameTaken {
                username: username.to_owned(),
            });
        }

        transaction.execute(
            "INSERT INTO users
                 (id, username, display_name, avatar_url, created_at_unix_ms,
                  password_hash, is_admin)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
            params![&id, username, display_name, now, password_hash, is_admin],
        )?;

        transaction.commit()?;
        Ok(Account {
            id,
            username: username.to_owned(),
            display_name: display_name.to_owned(),
            is_admin,
        })
    }

    /// Raises an existing account to administrator.
    ///
    /// # Why this exists, when `create_account` could have been used
    ///
    /// Migration 2 gives every pre-existing row `is_admin = 0`, so an instance
    /// that had accounts before this build has no administrator and no way to
    /// become one through the API -- the API that makes accounts is the API only
    /// administrators may call. This is the break-glass door, and it is a
    /// separate named function rather than a flag on `create_account` so that
    /// "an operator granted themselves the role on the command line" is a thing
    /// the code says out loud.
    ///
    /// It is idempotent, so running it twice is one promotion.
    ///
    /// # Errors
    ///
    /// [`ServerError::UnknownAccount`] if no row has this username, and
    /// [`ServerError::Sqlite`] for anything else.
    pub fn promote_to_administrator(&self, username: &str) -> Result<()> {
        let connection = self.lock()?;
        let promoted = connection.execute(
            "UPDATE users SET is_admin = 1 WHERE username = ?1",
            [username],
        )?;
        if promoted == 0 {
            return Err(ServerError::UnknownAccount {
                username: username.to_owned(),
            });
        }
        info!(
            username_len = username.len(),
            "granted the administrator role to an existing account"
        );
        Ok(())
    }

    /// Reads one account's credential by login handle.
    ///
    /// `None` for a username that does not exist, which
    /// [`crate::auth::verify_password`] is careful to make indistinguishable from
    /// a wrong password: see its docs for the work it does to equalise the two.
    ///
    /// `username` is matched **exactly**, including case. `users_username_unique`
    /// is a `BINARY` index and matching the way the unique index does is the only
    /// way a login can be sure that the account it authenticated is the account
    /// the uniqueness rule protects -- case-insensitive login on a case-sensitive
    /// index means two spellings that are distinct rows but one handle.
    pub fn credential_for_username(&self, username: &str) -> Result<Option<Credential>> {
        let connection = self.lock()?;
        let found = connection
            .query_row(
                "SELECT id, username, display_name, password_hash, is_admin
                 FROM users WHERE username = ?1",
                [username],
                |row| {
                    Ok(Credential {
                        account: Account {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            display_name: row.get(2)?,
                            is_admin: row.get::<_, i64>(4)? != 0,
                        },
                        password_hash: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(found)
    }

    // -----------------------------------------------------------------------
    // Sessions
    // -----------------------------------------------------------------------

    /// Records a session, keyed on `token_hash`.
    ///
    /// The hash is the primary key, so two logins can never collide by accident
    /// and `INSERT` is enough -- no upsert, no `OR REPLACE`, nothing that could
    /// quietly extend an existing session's life. A caller that wants a new
    /// session mints a new token.
    ///
    /// `expires_at_unix_ms` must be strictly greater than `created_at_unix_ms`;
    /// migration 2's `CHECK` says so, so a caller that passes equal instants gets
    /// a [`ServerError::Sqlite`] naming the constraint rather than a session that
    /// is born dead.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure, including the foreign key a
    /// session for an unknown `user_id` would violate.
    pub fn insert_session(
        &self,
        token_hash: &str,
        user_id: &str,
        created_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<()> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO sessions
                 (token_hash, user_id, created_at_unix_ms, expires_at_unix_ms, revoked_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![token_hash, user_id, created_at_unix_ms, expires_at_unix_ms],
        )?;
        Ok(())
    }

    /// Decides what a presented session token hash is worth.
    ///
    /// **One indexed read, and the order of the checks is the security property.**
    /// A revoked session is reported as [`SessionState::Revoked`] even when its
    /// `expires_at_unix_ms` is also in the past, so the log line names the reason
    /// an operator actually acted on rather than the one that happened to fire
    /// first. `revoked_at_unix_ms IS NULL` is therefore the first predicate.
    ///
    /// `now_unix_millis` is an argument rather than a call to
    /// [`crate::time::now_unix_millis`] so a test can place itself on either side
    /// of an expiry without a `sleep()`, which `AGENTS.md` §4.3 forbids and which
    /// a session TTL measured in days would otherwise make unavoidable.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure. A foreign-key-broken session
    /// row is reported as [`SessionState::Absent`], because a session pointing at
    /// a user that is not there is not a session.
    pub fn lookup_session(&self, token_hash: &str, now_unix_millis: i64) -> Result<SessionState> {
        let connection = self.lock()?;
        let found = connection
            .query_row(
                "SELECT s.user_id, s.created_at_unix_ms, s.expires_at_unix_ms,
                        s.revoked_at_unix_ms, u.is_admin
                 FROM sessions AS s
                 JOIN users AS u ON u.id = s.user_id
                 WHERE s.token_hash = ?1",
                [token_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, i64>(4)? != 0,
                    ))
                },
            )
            .optional()?;

        let Some((user_id, created_at, expires_at, revoked_at, is_admin)) = found else {
            return Ok(SessionState::Absent);
        };

        // Reported even when the session has also expired: an operator who revoked
        // a session wants to know that, and the session outliving its own
        // expiry is a fact about the revocation, not about the clock.
        if let Some(revoked_at) = revoked_at {
            debug!(
                user_id = %user_id,
                lifetime_ms = revoked_at.saturating_sub(created_at),
                "a revoked session was presented"
            );
            return Ok(SessionState::Revoked {
                revoked_at_unix_ms: revoked_at,
            });
        }
        if now_unix_millis >= expires_at {
            return Ok(SessionState::Expired {
                expires_at_unix_ms: expires_at,
            });
        }
        Ok(SessionState::Live {
            user_id,
            is_admin,
            expires_at_unix_ms: expires_at,
        })
    }

    /// Marks a session revoked, and says whether there was one to revoke.
    ///
    /// **A row is kept rather than deleted, and that is what makes the operation
    /// idempotent and auditable.** Deleting would make a second `logout` for the
    /// same token indistinguishable from a token that was never valid -- which is
    /// a property, but it also destroys the only record that the session existed.
    /// Writing `revoked_at_unix_ms` keeps both: the answer is still "this session
    /// is not live" and there is still a row saying when it stopped being so.
    ///
    /// Idempotent, because `WHERE revoked_at_unix_ms IS NULL` makes a second call
    /// a no-op that returns `true` -- the session was revoked, by this call or an
    /// earlier one, and both answers are the same answer.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure.
    pub fn revoke_session(&self, token_hash: &str, now_unix_millis: i64) -> Result<bool> {
        let connection = self.lock()?;
        let revoked = connection.execute(
            "UPDATE sessions SET revoked_at_unix_ms = ?1
             WHERE token_hash = ?2 AND revoked_at_unix_ms IS NULL",
            params![now_unix_millis, token_hash],
        )?;
        Ok(revoked > 0)
    }

    /// Removes every session that is past its expiry.
    ///
    /// **Not called from a request path and not called on a timer**, deliberately:
    /// an expired session is already refused by [`Store::lookup_session`], so
    /// deleting it changes nothing a peer can observe, and a sweep implies a
    /// background task this server does not have. It exists so an operator -- or
    /// the tests -- can say how much of the file is dead weight, and so the
    /// retention story is a function somebody can call rather than a hope.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure.
    pub fn purge_expired_sessions(&self, now_unix_millis: i64) -> Result<usize> {
        let connection = self.lock()?;
        let removed = connection.execute(
            "DELETE FROM sessions WHERE expires_at_unix_ms <= ?1",
            [now_unix_millis],
        )?;
        Ok(removed)
    }

    // -----------------------------------------------------------------------
    // Channel membership
    // -----------------------------------------------------------------------

    /// Whether `user_id` may write in `channel_id`.
    ///
    /// `channel_members` was created by migration 1 and left empty by every build
    /// before this one, so on a database migrated from schema 1 **every send is
    /// refused** until somebody inserts a membership row. That is stated here
    /// rather than discovered: it is the correct answer (an empty membership table
    /// means nobody has been granted anything) and
    /// [`crate::auth::bootstrap`] is what puts the first administrator into
    /// [`DEFAULT_CHANNEL_ID`].
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure.
    pub fn user_can_reach_channel(&self, user_id: &str, channel_id: &str) -> Result<bool> {
        let connection = self.lock()?;
        let found: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM channel_members WHERE user_id = ?1 AND channel_id = ?2",
                params![user_id, channel_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Whether a send may proceed, and why not when it may not.
    ///
    /// **Existence is decided before membership, and the order is the whole
    /// design of this function.** A user who types a channel that does not exist
    /// has made a typo; a user who types a channel they are not a member of has
    /// made a request they cannot satisfy. Both need *different* sentences -- one
    /// sends them to fix an id, the other sends them to an administrator -- so
    /// asking the membership question first would answer "ask an administrator for
    /// access" about a channel that does not exist.
    ///
    /// It leaks nothing to ask in this order. An authenticated peer that learns
    /// "no such channel" learns a fact every other client will get from `GET
    /// /channels` the moment the REST milestone lands, and it does **not** learn
    /// whether any particular other user is a member of a channel that does exist:
    /// that question is answered with [`ChannelAccess::NotAMember`] either way.
    ///
    /// # Arguments
    ///
    /// * `user_id` - the authenticated account, from the handshake.
    /// * `channel_id` - as untrusted text from the frame.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure, which the caller reports as a
    /// server fault and not as a refusal: a check that could not run is not an
    /// answer.
    pub fn channel_access(&self, user_id: &str, channel_id: &str) -> Result<ChannelAccess> {
        let connection = self.lock()?;
        let exists: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM channels WHERE id = ?1",
                [channel_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(ChannelAccess::UnknownChannel);
        }

        let member: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM channel_members WHERE user_id = ?1 AND channel_id = ?2",
                params![user_id, channel_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(if member.is_some() {
            ChannelAccess::Allowed
        } else {
            ChannelAccess::NotAMember
        })
    }

    /// Grants `user_id` membership of `channel_id`.
    ///
    /// `INSERT OR IGNORE`, because membership is a set and re-granting one is not
    /// an event. **Not** `OR REPLACE`: the table's primary key is the pair and
    /// there is no column an upsert would have to overwrite, so an upsert here
    /// would be the same `OR REPLACE` reflex `create_account` rejects, with the
    /// same cascade.
    ///
    /// # Errors
    ///
    /// [`ServerError::Sqlite`] for a driver failure, including the foreign keys on
    /// either id.
    pub fn add_channel_member(&self, user_id: &str, channel_id: &str) -> Result<()> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT OR IGNORE INTO channel_members (user_id, channel_id) VALUES (?1, ?2)",
            params![user_id, channel_id],
        )?;
        Ok(())
    }

    /// Inserts the two rows the round trip needs, if they are not already there.
    ///
    /// `INSERT OR IGNORE` rather than `INSERT OR REPLACE`, and the difference is
    /// the whole reason this is safe to run on every open: `OR REPLACE` deletes
    /// the conflicting row and inserts a new one, which on `users` would cascade
    /// into `messages.user_id` and on `channels` would reset
    /// `last_message_at` -- the resume cursor -- on a restart.
    ///
    /// **The seeded user is not an account.** The insert names no
    /// `password_hash`, so migration 2's nullable column stays `NULL` and there
    /// is nothing anybody could present to log in as it. The module docs give the
    /// reason the row is still created at all: ADR-010 retains message authorship
    /// after a removal, and a database migrated from schema 1 has `messages` rows
    /// pointing at it under an enforced foreign key.
    fn seed(&self) -> Result<()> {
        let now = time::now_unix_millis();
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        transaction.execute(
            "INSERT OR IGNORE INTO users
                 (id, username, display_name, avatar_url, created_at_unix_ms)
             VALUES (?1, ?2, ?3, NULL, ?4)",
            params![
                UNATTRIBUTED_USER_ID,
                UNATTRIBUTED_USERNAME,
                UNATTRIBUTED_DISPLAY_NAME,
                now
            ],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO channels
                 (id, name, description, is_private, created_at_unix_ms, last_message_at_unix_ms)
             VALUES (?1, ?2, NULL, 0, ?3, NULL)",
            params![DEFAULT_CHANNEL_ID, DEFAULT_CHANNEL_NAME, now],
        )?;

        transaction.commit()?;
        Ok(())
    }
}

/// Applies the connection-level settings this server depends on.
///
/// Three, and each one is off by default in SQLite:
///
/// - `foreign_keys` is **off** unless asked for, per connection. With it off,
///   every `REFERENCES` clause in `MIGRATION_1` is documentation rather than a
///   constraint, and the message path would happily attribute a message to a
///   channel that does not exist.
/// - `journal_mode` to WAL, which ADR-010 chooses. Checked rather than assumed:
///   SQLite reports the mode it actually ended up in, and a filesystem that
///   cannot do WAL (some network mounts) would otherwise produce a server whose
///   concurrency behaviour nobody measured.
/// - `busy_timeout`, so a concurrent writer waits instead of failing
///   immediately with `SQLITE_BUSY`.
fn configure(connection: &Connection) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;

    let reported: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !reported.eq_ignore_ascii_case("wal") {
        return Err(ServerError::JournalModeUnavailable { reported });
    }
    Ok(())
}

/// Brings the file up to [`SCHEMA_VERSION`], or refuses to touch it.
///
/// The three refusals are the interesting half:
///
/// - `user_version` above [`SCHEMA_VERSION`]: a newer build wrote this file.
/// - `user_version` of zero with tables already present: this is not a file this
///   build created, so migration 1 is not the thing that should run against it.
/// - Otherwise: apply every migration the file has **not** had, in one
///   transaction, stamping the version last so a crash mid-migration leaves the
///   file at the version it had reached rather than at a version it did not.
///
/// **The per-migration conditionals are what make an upgrade additive rather than
/// a replay.** A file at version 1 runs only [`MIGRATION_2`], so migration 1's
/// `CREATE TABLE users` is not executed a second time; a fresh file runs both. The
/// alternative -- always running the whole batch -- worked only while there was
/// one migration, and would have failed loudly on the second, which is
/// [`MIGRATION_1`]'s `no IF NOT EXISTS` rule working as intended rather than as an
/// obstacle.
///
/// Note what is *not* here: nothing rewrites, backfills, or deletes an existing
/// row. `MIGRATION_2` is two `ALTER TABLE ... ADD COLUMN`s and one `CREATE TABLE`.
/// A row that existed before authentication existed is still exactly the row it
/// was, with two new columns holding their defaults.
fn migrate(connection: &mut Connection) -> Result<()> {
    let found = read_user_version(connection)?;

    if found > SCHEMA_VERSION {
        return Err(ServerError::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    if found == 0 && table_exists(connection, "messages")? {
        return Err(ServerError::UnversionedSchema);
    }

    let transaction = connection.transaction()?;
    if found < 1 {
        transaction.execute_batch(MIGRATION_1)?;
    }
    if found < 2 {
        transaction.execute_batch(MIGRATION_2)?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

/// Reads the file's schema version.
fn read_user_version(connection: &Connection) -> Result<i64> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Whether `name` is a table in this file.
///
/// Parameterized, like every other statement here. It is not an injection risk
/// today because both call sites pass a literal, and the rule is kept because the
/// next call site will not.
fn table_exists(connection: &Connection, name: &str) -> Result<bool> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Whether the send's target channel exists.
///
/// `&Connection` rather than `&Transaction`, so the same one-liner serves the send
/// path — which holds an open transaction — and [`Store::read_since`], which does
/// not. `Transaction` derefs to `Connection`, so `accept_message` passes `&transaction`
/// unchanged and the existence rule stays written once.
fn channel_exists(connection: &Connection, channel_id: &str) -> Result<bool> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM channels WHERE id = ?1",
            [channel_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Reads the stored row for `client_msg_id`.
///
/// Returns [`ServerError::Sqlite`] wrapping `QueryReturnedNoRows` if it is
/// absent, which the `ON CONFLICT` path makes unreachable: the row the conflict
/// matched is committed before this transaction can read it. It is still a
/// returned error rather than a panic, because "unreachable" is a claim and this
/// is the code that would be wrong if the claim ever became false.
fn read_by_client_msg_id(
    transaction: &Transaction<'_>,
    client_msg_id: &str,
) -> Result<WireMessage> {
    let stored = transaction.query_row(SELECT_BY_CLIENT_MSG_ID, [client_msg_id], |row| {
        Ok(StoredMessage {
            id: row.get(0)?,
            client_msg_id: row.get(1)?,
            channel_id: row.get(2)?,
            user_id: row.get(3)?,
            content: row.get(4)?,
            accepted_at_unix_ms: row.get(5)?,
        })
    })?;
    stored.into_wire_message()
}

/// Mints the server-assigned `id` for an accepted message.
///
/// **Not** `Uuid::new_v4()`, and the reason changed when this milestone added
/// authentication. `uuid/v4` *is* enabled for this crate now -- `auth.rs` needs
/// operating-system entropy for session tokens and `uuid/v4` is the only entropy
/// source already in the tree that costs no new crate -- so the old argument ("the
/// workspace default is `features = ["std"]` and `v4` would not compile under
/// `cargo build -p sh_nexus_server`") no longer holds and is not repeated here.
///
/// The reason is the one that survives: a v4 id's high bits are random, so
/// `ORDER BY id` would **disagree** with `ORDER BY accepted_at`. `messages_channel_order`
/// is `(channel_id, accepted_at_unix_ms, id)` and `persistence.rs` asserts that
/// five messages inserted inside one millisecond come back in insertion order --
/// which is only true because the id's high bits are the clock. An id nobody can
/// predict is not worth losing that for: message ids are not secrets, and this
/// server does not use them as authorization material.
fn mint_message_id(accepted_at_unix_millis: i64, sequence: u64) -> String {
    // `unsigned_abs` rather than a cast: the value cannot be negative for a clock
    // this server can be running on, and a total function that is wrong for an
    // impossible input beats one that panics on it.
    let millis = u128::from(accepted_at_unix_millis.unsigned_abs());
    Uuid::from_u128((millis << 64) | u128::from(sequence))
        .hyphenated()
        .to_string()
}

/// Mints the server-assigned `id` for a new account.
///
/// **Milliseconds alone, and the reason is different from
/// [`mint_message_id`]'s.** A message id is followed by a per-insert counter
/// because several messages can be accepted in the same millisecond and the tie
/// has to break in acceptance order. An account id has no such neighbour: one
/// account creation per millisecond on a per-team instance is not a load this
/// server will see, and if it somehow were, `users.id`'s primary key would reject
/// the collision rather than silently merge two people.
///
/// It is `u_`-prefixed like every other id in the protocol, so it is the same
/// shape as a client-side `u_me` and nothing has to learn a second spelling.
fn mint_user_id(created_at_unix_millis: i64) -> String {
    let millis = u128::from(created_at_unix_millis.unsigned_abs());
    format!("u_{millis:013x}")
}
