//! Wall-clock access, and why it does not go through `chrono/clock`.
//!
//! # The trap this module exists to avoid
//!
//! `chrono` is declared once in the workspace root with
//! `default-features = false, features = ["std", "serde"]`. The `clock` feature --
//! the one that provides `Utc::now()` -- is added by
//! `crates/sh_nexus/Cargo.toml` and **by nothing else**.
//!
//! Cargo unifies features per crate version across a *workspace* build, so
//! `Utc::now()` compiles under `cargo test --workspace` and under
//! `cargo build --workspace`, and **fails to compile** under
//! `cargo build -p sh_nexus_server`. A dependency that exists only until someone
//! runs the narrower command is not a dependency; it is a build that passes CI
//! and fails for the person deploying the server. The root `Cargo.toml` says as
//! much about `chrono/clock` on the client crate -- "the honest part: `Utc::now()`
//! compiled here before `clock` was declared, because gpui enables chrono's
//! `default` features and features unify per crate version" -- and this module is
//! the server declining to repeat that trick in the other direction.
//!
//! So the clock is read from [`std::time::SystemTime`] and converted with
//! chrono's *pure arithmetic*, which needs no feature at all:
//! [`DateTime::from_timestamp_millis`] is a `const fn` in chrono's core.
//!
//! # Why milliseconds, and why not a formatted string
//!
//! `AGENTS.md` §2.1 requires `chrono::DateTime<Utc>` for every timestamp and
//! forbids storing raw strings; the wire layer converts to RFC-3339 on the way
//! out. Storage is the one place where *ordering* matters as much as the instant,
//! and a `TEXT` column of RFC-3339 values sorts lexicographically only because
//! this server happens to write one fixed offset. That is a property of the
//! writer, not of the column, and it stops holding the moment a second writer
//! appears. An `INTEGER` of milliseconds is ordered by SQLite itself, on every
//! platform, forever.
//!
//! The cost is a conversion on the way in and on the way out, and both are
//! covered by tests in `crates/sh_nexus_server/tests/persistence.rs`.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

use crate::error::{Result, ServerError};

/// The current instant, in milliseconds since the Unix epoch.
///
/// # What the two unreachable branches are for
///
/// `SystemTime::duration_since(UNIX_EPOCH)` fails only on a clock set before
/// 1970, and `i64::try_from` on the elapsed milliseconds fails only after the
/// year 292 million. Neither is reachable on a machine that has ever had a
/// correct clock, and neither has a correct answer, so both resolve to a value
/// that is *outside* the client's plausible timestamp window
/// (`network/mapping.rs`: 2000-01-01 to 2100-01-01). The consequence is
/// deliberate: a message accepted under a broken clock produces a timestamp the
/// client's own boundary refuses, rather than a plausible-looking 1970 date that
/// a human would read and trust.
///
/// A `debug!` and not an `error!`, because the resulting refusal is already
/// logged at `error!` by the caller and two logs for one fault is one too many.
pub fn now_unix_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(error) => {
            debug_assert!(
                false,
                "the system clock is before the Unix epoch: {}",
                error.duration().as_secs()
            );
            0
        }
    }
}

/// Converts a stored millisecond timestamp into the instant the wire carries.
///
/// # Errors
///
/// [`ServerError::TimestampOutOfRange`] if `unix_millis` is not representable.
/// Chrono's constructors return `Option` rather than clamping, and this preserves
/// that: an out-of-range value in a hand-edited file is a fact to report, not a
/// value to round to the nearest representable instant.
pub fn from_unix_millis(unix_millis: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_millis(unix_millis)
        .ok_or(ServerError::TimestampOutOfRange { unix_millis })
}

/// The same instant as a stored millisecond timestamp.
///
/// Exact for every instant chrono can represent in the plausible window: a
/// `DateTime<Utc>` holds nanoseconds, and milliseconds are within that range
/// losslessly in both directions. The inverse of [`from_unix_millis`].
pub fn to_unix_millis(instant: DateTime<Utc>) -> i64 {
    instant.timestamp_millis()
}
