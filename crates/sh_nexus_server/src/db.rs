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
//! Four tables and one migration, and two of them are not exercised by this
//! milestone. That is the interesting part, so it is stated rather than implied:
//!
//! | Table | Exercised here | Why it exists anyway |
//! |---|---|---|
//! | `users` | one seeded row | Auth is ADR-010's next PR. `messages.user_id` has a foreign key, so the row has to exist for the milestone's own foreign keys to mean anything. |
//! | `channels` | one seeded row | Channel provisioning is the REST milestone's job. A send naming an unknown channel is refused, not silently created. |
//! | `messages` | the whole round trip | The one working path. |
//! | `read_cursors` | **no** | Required by ADR-010 "from the first migration". |
//! | `channel_members` | **no** | Membership is an authorization concern, which is the auth milestone. |
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
//! # The author of every message in this milestone
//!
//! [`UNATTRIBUTED_USER_ID`] is the author `messages.user_id` points at, because
//! `PLAN.md` §6's `message.send` carries only `channel_id` and `content` and
//! authorship is meant to come from an authenticated session -- which is the
//! milestone ADR-010 defers. So the server has no identity to record and this is
//! the honest value: a reserved row that no human can be, named
//! `unattributed`, so a transcript rendered from this milestone's file says
//! plainly that the server does not know who wrote it.
//!
//! The alternative -- a blank `user_id` -- was rejected for a concrete reason
//! rather than taste: `WireMessage`'s own boundary
//! (`crates/sh_nexus/src/network/mapping.rs`) rejects a blank `user_id`, so a
//! blank one makes this server emit a `message.ack` its own client refuses to
//! decode, and the resulting bug would present as a client defect.

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
pub const SCHEMA_VERSION: i64 = 1;

/// The id every message is attributed to while there is no authentication.
///
/// See the module docs. Not a user, not an account, and not a fallback for a
/// real one: a value that means "the server does not know".
pub const UNATTRIBUTED_USER_ID: &str = "u_unattributed";

/// The login handle of [`UNATTRIBUTED_USER_ID`].
pub const UNATTRIBUTED_USERNAME: &str = "unattributed";

/// The display name of [`UNATTRIBUTED_USER_ID`].
///
/// Renders in a transcript as a word rather than as an empty bubble, because
/// `network/mapping.rs` documents an empty display name as "a message attributed
/// to nobody, which is worse than the message not arriving".
pub const UNATTRIBUTED_DISPLAY_NAME: &str = "Unattributed (no authentication yet)";

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

/// A message row, read by `client_msg_id`.
///
/// The read is keyed on `client_msg_id` and not on the id this call just minted,
/// so the duplicate path returns the *original* row. That is the whole point: an
/// ack that carried a fresh id would tell the client its replayed send became a
/// second message.
const SELECT_BY_CLIENT_MSG_ID: &str = "SELECT id, client_msg_id, channel_id, user_id, \
     content, accepted_at_unix_ms FROM messages WHERE client_msg_id = ?1";

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
    /// * `content` - the message body, stored verbatim.
    ///
    /// # Errors
    ///
    /// [`ServerError::UnknownChannel`] if `channel_id` names no channel, and
    /// [`ServerError::Sqlite`] for a driver failure. [`ServerError::LockPoisoned`]
    /// if an earlier panic left the connection unusable.
    pub fn accept_message(
        &self,
        client_msg_id: &str,
        channel_id: &str,
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
                UNATTRIBUTED_USER_ID,
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

    /// Inserts the two rows the round trip needs, if they are not already there.
    ///
    /// `INSERT OR IGNORE` rather than `INSERT OR REPLACE`, and the difference is
    /// the whole reason this is safe to run on every open: `OR REPLACE` deletes
    /// the conflicting row and inserts a new one, which on `users` would cascade
    /// into `messages.user_id` and on `channels` would reset
    /// `last_message_at` -- the resume cursor -- on a restart.
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
/// - Otherwise: apply every migration the file has not had, in one transaction,
///   stamping the version last so a crash mid-migration leaves the file at the
///   version it had reached rather than at a version it did not.
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
    transaction.execute_batch(MIGRATION_1)?;
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
fn channel_exists(transaction: &Transaction<'_>, channel_id: &str) -> Result<bool> {
    let found: Option<i64> = transaction
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
/// **Not** `Uuid::new_v4()`, and the reason is the same one `time.rs` gives for
/// not using `Utc::now()`: `crates/sh_nexus_server/Cargo.toml` declares `uuid`
/// with the workspace default, and the workspace default is features `["std"]` --
/// `v4` is added by `crates/sh_nexus/Cargo.toml`. A `Uuid::new_v4()` here would
/// compile under `cargo build --workspace` and fail under
/// `cargo build -p sh_nexus_server`, which is the deployment command.
///
/// So the id is built from two numbers this process already has: the acceptance
/// instant in milliseconds, and a per-process sequence that increases on every
/// insert. That gives what the protocol actually asks for -- "server-assigned",
/// non-blank, and unique -- plus two properties worth having: ids sort in
/// acceptance order, which makes `ORDER BY id` a tie-break that agrees with
/// `ORDER BY accepted_at`, and they are reproducible, so a test can assert on one.
///
/// Not a secret and not an authorization token: without authentication there is
/// nothing to authorize with, and ADR-010 puts local accounts behind a credential
/// this milestone does not have. The uniqueness argument is 41 bits of wall clock
/// plus 64 bits of counter, which is wider than any of this milestone's flows
/// need and is documented as the first thing to revisit when a real id generator
/// arrives.
fn mint_message_id(accepted_at_unix_millis: i64, sequence: u64) -> String {
    // `unsigned_abs` rather than a cast: the value cannot be negative for a clock
    // this server can be running on, and a total function that is wrong for an
    // impossible input beats one that panics on it.
    let millis = u128::from(accepted_at_unix_millis.unsigned_abs());
    Uuid::from_u128((millis << 64) | u128::from(sequence))
        .hyphenated()
        .to_string()
}
