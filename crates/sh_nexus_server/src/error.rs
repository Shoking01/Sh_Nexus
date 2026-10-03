//! The server's error type.
//!
//! # Why this is neither `ShNexusError` nor `WireError`
//!
//! `AGENTS.md` §3.3 makes `ShNexusError` the project's global error enum, and
//! §2.2 mandates `thiserror` to derive it. Neither applies here, for one reason
//! each, and both reasons are about the dependency graph rather than taste:
//!
//! - `ShNexusError` lives in `sh_nexus`, the **client**. Reusing it would make
//!   the server depend on the client, which is the back-edge that would destroy
//!   ADR-002's "the protocol is defined once" guarantee. `sh_nexus_wire` makes
//!   the same argument about its own [`WireError`], in
//!   `crates/sh_nexus_wire/src/error.rs`.
//! - `thiserror` is **not** in `crates/sh_nexus_server/Cargo.toml`, and adding a
//!   dependency needs a `AGENTS.md` §7.2 audit and a change to a manifest this
//!   milestone does not touch. Nine variants with hand-written `Display` arms is
//!   the cheap honest answer here; if the enum grows materially, the audit for
//!   `thiserror` is a small, self-contained pull request.
//!
//! [`WireError`]: sh_nexus_wire::WireError
//!
//! # What is a fault and what is a refusal
//!
//! The two are different and the difference is load-bearing:
//!
//! | | Meaning | The peer's frame |
//! |---|---|---|
//! | [`ServerError`] | The server could not do its job | `message.error` with `storage_failure`, or the connection drops |
//! | [`Refusal`](crate::message::Refusal) | The request was invalid | `message.error` with the specific code |
//!
//! A user who sends a blank channel must be told *which* field was blank, not
//! that "storage failed". A user whose send failed because the disk is full must
//! be told *that*, and it must not read as though they typed something wrong. ADR-010
//! records the same instinct from the other direction: an opaque code the user can
//! report beats a silently dropped explanation.
//!
//! # What is never in here
//!
//! No message content, and no `client_msg_id`. `AGENTS.md` §7.5 forbids logging
//! payload content, and `crates/sh_nexus_wire/src/dto.rs` records *why* a
//! `client_msg_id` is treated as hostile too: it is the one id in the system that
//! is peer-supplied and length-unbounded, so an error that could reach a log
//! carries its length rather than its value. The same reasoning is applied
//! consistently in `message.rs`.

use std::error::Error as StdError;
use std::fmt;
use std::io;

use rusqlite::Error as SqliteError;
use sh_nexus_wire::WireError;

/// The result of a fallible server operation.
pub type Result<T> = std::result::Result<T, ServerError>;

/// Everything that can go wrong inside the server.
#[derive(Debug)]
pub enum ServerError {
    /// A SQLite operation failed. Wrapped rather than stringified so the driver
    /// code stays visible in a `warn!`.
    Sqlite(SqliteError),

    /// A socket or file operation failed: binding the listener, opening the
    /// database file, reading a request.
    Io(io::Error),

    /// A server frame could not be serialized. It cannot fail for any value
    /// `sh_nexus_wire`'s types can hold -- see `ClientEnvelope::encode` -- and
    /// it is a variant rather than an `expect` because `AGENTS.md` §2.1 forbids a
    /// panic in a production path.
    Encode(WireError),

    /// The environment named an unusable bind address or database path.
    ///
    /// Reported rather than defaulted, because a self-hosted operator who has
    /// misspelled `SH_NEXUS_BIND` needs to be told which variable was wrong. The
    /// message is the operator-facing one and is logged at `error!`.
    Configuration(String),

    /// A lock was poisoned by a panic in another thread.
    ///
    /// **Not** recovered with `into_inner`. A poisoned SQLite mutex means a
    /// thread died mid-statement, so the connection's transaction state is
    /// whatever it was at the instant of the panic, and using it would turn one
    /// crash into silent corruption. Failing the operation is the recoverable
    /// half; the process staying up is the operator's decision.
    LockPoisoned {
        /// What was being locked, as a compile-time constant naming it. An id,
        /// never user text.
        resource: &'static str,
    },

    /// The database file was written by a newer build.
    ///
    /// Refused rather than opened. A build that has never heard of a schema
    /// version cannot know which of its assumptions still hold, and SQLite will
    /// happily let it write into a layout it misreads. This is the "never
    /// destroy user data silently" rule with teeth: the refusal is the only
    /// outcome that loses nothing.
    SchemaTooNew {
        /// The `user_version` found in the file.
        found: i64,
        /// The highest version this build knows how to read.
        supported: i64,
    },

    /// The database file has tables but no `user_version`.
    ///
    /// Almost always a file this server did not create -- copied in from another
    /// tool, or half-restored from a backup. Running migration 1 against it
    /// would either fail on an existing table or, worse, succeed on a schema
    /// that only looks compatible.
    UnversionedSchema,

    /// WAL could not be enabled.
    ///
    /// Named as an error because ADR-010 chooses WAL as part of the deployment
    /// story, and running without it is a different (and unmeasured) database
    /// configuration rather than a silently degraded one.
    JournalModeUnavailable {
        /// The mode SQLite reported back after being asked for WAL.
        reported: String,
    },

    /// A send named a channel that does not exist.
    ///
    /// A refusal dressed as an error because it originates in the store, which is
    /// the only component that knows what a channel is. `message.rs` maps it onto
    /// the `unknown_channel` code. The id is carried because a channel id is
    /// server-issued, bounded text and quoting it is the whole diagnostic.
    UnknownChannel {
        /// The channel the send named.
        channel_id: String,
    },

    /// A stored millisecond timestamp cannot be represented as a
    /// `DateTime<Utc>`.
    ///
    /// Reachable only from a corrupt or hand-edited file, since every value this
    /// server writes comes from `SystemTime::now()`. Surfaced rather than clamped
    /// to a plausible instant, because a fabricated timestamp would put a message
    /// at a time nobody sent it from.
    TimestampOutOfRange {
        /// The value that could not be converted.
        unix_millis: i64,
    },
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(inner) => write!(formatter, "database error: {inner}"),
            Self::Io(inner) => write!(formatter, "io error: {inner}"),
            Self::Encode(inner) => write!(formatter, "frame encoding error: {inner}"),
            Self::Configuration(detail) => write!(formatter, "configuration error: {detail}"),
            Self::LockPoisoned { resource } => {
                write!(
                    formatter,
                    "the {resource} lock was poisoned by an earlier panic"
                )
            }
            Self::SchemaTooNew { found, supported } => write!(
                formatter,
                "database schema version {found} was written by a newer build; \
                 this build reads up to version {supported}"
            ),
            Self::UnversionedSchema => write!(
                formatter,
                "database file has tables but no schema version stamp; refusing to \
                 migrate a file this build did not create"
            ),
            Self::JournalModeUnavailable { reported } => write!(
                formatter,
                "WAL journal mode was requested but SQLite reported {reported:?}"
            ),
            Self::UnknownChannel { channel_id } => {
                write!(formatter, "no such channel: {channel_id}")
            }
            Self::TimestampOutOfRange { unix_millis } => write!(
                formatter,
                "stored timestamp {unix_millis} ms since the epoch is not a representable instant"
            ),
        }
    }
}

impl StdError for ServerError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Sqlite(inner) => Some(inner),
            Self::Io(inner) => Some(inner),
            Self::Encode(inner) => Some(inner),
            Self::Configuration(_)
            | Self::LockPoisoned { .. }
            | Self::SchemaTooNew { .. }
            | Self::UnversionedSchema
            | Self::JournalModeUnavailable { .. }
            | Self::UnknownChannel { .. }
            | Self::TimestampOutOfRange { .. } => None,
        }
    }
}

impl From<SqliteError> for ServerError {
    fn from(error: SqliteError) -> Self {
        Self::Sqlite(error)
    }
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WireError> for ServerError {
    fn from(error: WireError) -> Self {
        Self::Encode(error)
    }
}
