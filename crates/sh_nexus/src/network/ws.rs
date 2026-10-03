//! The client's WebSocket transport: one socket, one worker thread, one inbox.
//!
//! `AGENTS.md` §3.1 names `network/websocket.rs` as a module, and this is it.
//! It is the first file in this crate that opens a socket, and the reason it
//! exists is stated in `AGENTS.md` §8.1's Real-time Flow: *"Two clients, a send
//! crossing the wire"*. Until now the client had a boundary
//! ([`mapping`](crate::network::mapping)) and a seam
//! ([`bridge`](crate::state::bridge)) but nothing that spoke the protocol, so
//! that flow could not be written and the mapping had no producer to be tested
//! against.
//!
//! # The thread rule, concretely
//!
//! `docs/ARCHITECTURE.md` ADR-009 says a worker thread *cannot* call
//! `cx.update_global`, because `Context<'a, T>` holds `&'a mut App` and `App`
//! holds `Rc`s, so `Context` is `!Send` and the program does not compile. This
//! module is written so that this is the only thing it could possibly do:
//!
//! - [`WsTransport::start`] returns as soon as the worker thread exists. It does
//!   not wait for a socket. A caller on the main thread that blocked for the
//!   connect would put up to [`CONNECT_TIMEOUT`] of network wait inside the frame
//!   loop, which `AGENTS.md` §2.3 forbids outright.
//! - The thread holds exactly three things: an [`EventSender`], a receiver and a
//!   block of counters. `EventSender` is `Send + Sync + Clone`, holds a bounded
//!   channel and nothing else, and has no method that returns a reference to
//!   anything this crate owns. So the value that crosses the thread is a finished
//!   [`DomainEvent`] and the state never does — ADR-009's *"values cross, state
//!   stays"*, expressed as a signature.
//! - This file names no context type and no mutating context method at all.
//!   `network_cannot_reach_the_main_thread_context` in
//!   `crates/sh_nexus/tests/bridge.rs` and `network_names_no_gpui` in
//!   `crates/sh_nexus/tests/layer_boundary.rs` scan this directory for those
//!   tokens and fail the build on any of them.
//!
//! # Why one `tokio` runtime per transport, on its own OS thread
//!
//! A current-thread runtime inside one named `std::thread` per transport. The
//! alternatives were a shared multi-thread runtime (which needs a process-wide
//! owner, and `network/` is not allowed to own application lifecycle), a task on
//! GPUI's executor (which would put a socket read on the thread that draws
//! frames, and `AGENTS.md` §6.2's frame budget for that thread is `< 8 ms`),
//! and blocking reads on a dedicated thread with no async at all (which cannot
//! express a read timeout and a write at the same time).
//!
//! A current-thread runtime on its own OS thread has one property the others do
//! not: **exactly one thread can touch the socket, so there is no lock on it.**
//! The read half and the write half are two branches of one `tokio::select!`, and
//! [`WsTransport::send_message`] never touches the socket at all — it puts a
//! frame on a bounded queue and returns.
//!
//! # The bound on the outbound queue, and what a full one does
//!
//! [`MAX_OUTBOUND_FRAMES`] frames, reported as [`TransportError::OutboundFull`]
//! and never grown past. `AGENTS.md` §7.1 forbids unbounded in-memory state, and
//! an unbounded outbound queue would break it in the one place a user's typing
//! outruns a broken network. The refusal is the same shape as
//! [`bridge`](crate::state::bridge)'s: a reported value, not a block and not a
//! silent drop. It cannot hand the frame back the way `EventSender::deliver`
//! hands an event back, and that asymmetry is the reason this file has one queue
//! and not two.
//!
//! # What this module does not do
//!
//! | Not here | Named as |
//! |---|---|
//! | Authentication, tokens, a login frame | ADR-010's next PR. The server accepts unauthenticated sends, so there is nothing to send. |
//! | TLS. The URL must be `ws://`; `wss://` needs a TLS feature and a root store, and `AGENTS.md` §7.4's "certificate validation must never be disabled in release builds" is a rule about a stack this client does not have yet. | ADR-010 |
//! | The outbox, so a `Pending` send survives a disconnect | `PLAN.md` §7's persistence work. This queues what it was handed while running; it does not persist. |
//! | A `reaction.remove`, because `PLAN.md` §6 has no such frame | A protocol addition, not a client omission. |
//! | Any log line | See below. |
//!
//! # Why there is no `tracing` call in this file
//!
//! `AGENTS.md` §7.5 requires `tracing` and §7.1 bans `println!`, and this file
//! obeys both by making **no log line at all**: `crates/sh_nexus` does not
//! declare `tracing`, and adding it is an `AGENTS.md` §7.2 audit this work unit
//! does not carry. So instead of logging, the transport *counts*, and the counts
//! are public: [`WsTransport::stats`] returns a [`TransportStats`] snapshot in
//! which `events_refused`, `frames_dropped`, `writes_failed`, `connect_timeouts`
//! and `keepalive_abandoned` are exactly the five conditions that would have been
//! log lines. A drop is never silent — it is a number a caller can poll — and no
//! line can leak message content, because there are no lines. What the *server*
//! logs about its half of the same conditions is `sh_nexus_server/src/ws.rs`.
//!
//! # An example
//!
//! The production shape: install the seam on the main thread, start the
//! transport, and let the frames do the rest.
//!
//! ```
//! use gpui::App;
//! use sh_nexus::network::ws::{epoch_cursor, TransportConfig, WsTransport};
//!
//! fn on_the_main_thread(cx: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let events = sh_nexus::state::bridge::install(cx, "u_me")?;
//!     let transport = WsTransport::start(
//!         TransportConfig::new("ws://127.0.0.1:8484/ws"),
//!         events,
//!     )?;
//!     // Every line above returned without waiting on a socket.
//!     transport.request_resync("general", epoch_cursor())?;
//!     Ok(())
//! }
//! # let _ = on_the_main_thread;
//! # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use crate::core::models::events::{ConnectionState, DomainEvent};
use crate::state::bridge::{Delivery, EventSender};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope, ServerFrame};
use sh_nexus_wire::version::{PROTOCOL_VERSION, UNSUPPORTED_VERSION_CODE};
use sh_nexus_wire::WireError;

/// The socket type [`connect_async`] produces, named once.
///
/// `MaybeTlsStream` is the handshake's own sum type. This build enables no TLS
/// feature on `tokio-tungstenite`, so only its `Plain` arm is ever constructed —
/// naming the type is still necessary, because a bare `TcpStream` is not what the
/// function returns and the compiler would say so.
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

// ---------------------------------------------------------------------------
// Timeouts and bounds
// ---------------------------------------------------------------------------

/// How long one connect attempt may take before it is abandoned: 5 seconds.
///
/// `AGENTS.md` §7.4 names it, and it is a per-attempt bound rather than a
/// whole-reconnection bound because the difference is the difference between "the
/// server is not there" and "the server accepted a socket and then stopped
/// answering". The first should be retried on the backoff schedule; the second
/// is a hung handshake that would otherwise hold a worker thread open
/// indefinitely.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the read half may be silent before the client pings: 30 seconds.
///
/// `AGENTS.md` §7.4: *"read (30s keepalive → ping)"*. Two consecutive silent
/// intervals abandon the connection rather than ping again, so a peer that
/// answers neither pings nor frames ends the connection in bounded time instead
/// of sitting on a half-open socket until the operating system notices.
///
/// **Not a per-message timeout, and it never fires while frames are arriving.** A
/// busy channel resets it on every frame. What it detects is a socket that has
/// stopped being readable, which is the one failure a client cannot otherwise
/// notice at all.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// How many silent keepalive intervals make a connection abandoned: 2.
///
/// One interval of grace in which to answer a ping, one more in which the answer
/// must arrive. With the default interval that is 60 seconds.
pub const KEEPALIVE_ABANDON_FACTOR: u32 = 2;

/// The first reconnection delay: 1 second.
pub const BACKOFF_INITIAL: Duration = Duration::from_secs(1);

/// The longest a reconnection delay may ever be: 60 seconds.
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Jitter, as parts per thousand of the nominal delay, in both directions:
/// 200 permille is 20%.
pub const JITTER_RANGE_PERMILLE: i32 = 200;

/// How many distinct values [`draw_jitter`] can return: 401.
///
/// `2 * JITTER_RANGE_PERMILLE + 1`, so the draw's range is exactly
/// `-200 ..= 200` permille and every value in it is reachable.
pub const JITTER_BUCKETS: u16 = (2 * JITTER_RANGE_PERMILLE + 1) as u16;

/// How many frames may sit in the outbound queue before a send is refused.
pub const MAX_OUTBOUND_FRAMES: usize = 256;

/// The protocol major version every frame this transport writes carries.
///
/// **A function rather than a constant, and that is `AGENTS.md` §7.4 made
/// checkable.** [`ClientEnvelope::new`] stamps the version itself, so no call site
/// in this crate can forget to — but a stamp nobody can read is not a rule, it is
/// an implementation detail, and a test can only assert what it can reach.
pub fn protocol_version() -> u16 {
    PROTOCOL_VERSION
}

/// The cursor a client that holds nothing for a channel asks from.
///
/// 2000-01-01T00:00:00Z, which is
/// [`TIMESTAMP_FLOOR_UNIX_SECS`](crate::network::mapping::TIMESTAMP_FLOOR_UNIX_SECS)
/// — the earliest instant this client's boundary will accept. Asking from it
/// means "everything", which is what a channel with no history yet needs.
///
/// **`DateTime<Utc>`, not `Option`, and that is deliberate.** The wire frame's
/// `after` is not optional, so a caller with no cursor must still name an instant;
/// making "no cursor" a named value rather than an absent branch means the case
/// is something somebody chose. `Option::None` here would be inexpressible on the
/// wire anyway, so the honest shape is a constant and not a `None`.
///
/// # Example
///
/// ```
/// use sh_nexus::network::ws::epoch_cursor;
/// use sh_nexus::network::mapping::TIMESTAMP_FLOOR_UNIX_SECS;
///
/// assert_eq!(epoch_cursor().timestamp(), TIMESTAMP_FLOOR_UNIX_SECS);
/// ```
pub fn epoch_cursor() -> DateTime<Utc> {
    DateTime::from_timestamp(946_684_800, 0).unwrap_or(DateTime::UNIX_EPOCH)
}

// ---------------------------------------------------------------------------
// The backoff schedule, as a pure function
// ---------------------------------------------------------------------------

/// The un-jittered delay before reconnection attempt `attempt`: 1s, 2s, 4s, ...
///
/// # Arguments
///
/// * `attempt` - 1-based. `attempt == 1` is the first retry, so the first retry
///   waits [`BACKOFF_INITIAL`]. Zero is treated as one rather than rejected,
///   because the loop that calls this holds a counter that starts at zero, and
///   clamping here means the caller cannot get the schedule wrong by being early.
///
/// # Returns
///
/// `BACKOFF_INITIAL * 2^(attempt - 1)`, saturating and then clamped to
/// [`BACKOFF_MAX`]. With the default constants: 1s, 2s, 4s, 8s, 16s, 32s, and
/// 60s from the seventh attempt onward — the doubling reaches 64s on the seventh
/// and the cap is what makes the sequence's final value 60s rather than 64s.
///
/// # Example
///
/// ```
/// use sh_nexus::network::ws::nominal_backoff;
/// use std::time::Duration;
///
/// assert_eq!(nominal_backoff(1), Duration::from_secs(1));
/// assert_eq!(nominal_backoff(3), Duration::from_secs(4));
/// assert_eq!(nominal_backoff(6), Duration::from_secs(32));
/// // Capped, and capped downward from 64s rather than upward from 60s.
/// assert_eq!(nominal_backoff(7), Duration::from_secs(60));
/// assert_eq!(nominal_backoff(64), Duration::from_secs(60));
/// ```
pub fn nominal_backoff(attempt: u32) -> Duration {
    // Thirty-one doublings of a one-second base is about 68 years in milliseconds,
    // inside `u128` and outside any plausible `Duration`. Bounding the exponent
    // here means the multiplication below cannot overflow at all, so the only
    // `unwrap_or` left in the function is a conversion that cannot fail.
    let doublings = attempt.saturating_sub(1).min(31);
    let scaled = BACKOFF_INITIAL
        .as_millis()
        .saturating_mul(1_u128 << doublings);
    let capped = scaled.min(BACKOFF_MAX.as_millis());
    Duration::from_millis(u64::try_from(capped).unwrap_or(u64::MAX))
}

/// The jittered delay before reconnection attempt `attempt`.
///
/// # Why jitter is a *parameter* and not something this function draws
///
/// `AGENTS.md` §4.2 requires the schedule, the jitter, the reset-on-success rule
/// and the max-attempt rule to be *tested*, and §4.3 forbids waiting for time to
/// do it. A function that drew its own jitter could only be tested by asserting a
/// range over many calls, which is a weaker claim than asserting the mapping. So
/// the drawn value is an argument: a test states the permille it wants and gets
/// the exact delay it asked for, and [`draw_jitter`] — the only caller that wants
/// randomness — is a separate, one-line function that can be reviewed on its own.
///
/// # Arguments
///
/// * `attempt` - 1-based; see [`nominal_backoff`].
/// * `jitter_permille` - parts per thousand of the nominal delay, positive to
///   wait *longer*. Values outside `-200 ..= 200` are **clamped, not rejected**:
///   a clamped delay is still a bounded delay, and a `Result` here would put an
///   error path on a function whose only job is arithmetic.
///
/// # Returns
///
/// The nominal delay scaled by `1 + jitter_permille / 1000`, never above
/// [`BACKOFF_MAX`] and never below zero.
///
/// The cap is applied **after** the jitter, so jitter may shorten a capped delay
/// (60s nominal may become 48s) but may never lengthen one past 60s. Otherwise a
/// client would exceed the ceiling §7.4 names precisely when it has reached the
/// cap — which is exactly when it is under the most pressure to retry slowly.
///
/// # Example
///
/// ```
/// use sh_nexus::network::ws::{backoff_delay, nominal_backoff};
/// use std::time::Duration;
///
/// assert_eq!(backoff_delay(3, 0), Duration::from_secs(4));
/// // +20% and -20% of a one-second first retry.
/// assert_eq!(backoff_delay(1, 200), Duration::from_millis(1_200));
/// assert_eq!(backoff_delay(1, -200), Duration::from_millis(800));
/// // A capped attempt: jitter may shorten it, never lengthen it past the cap.
/// assert_eq!(backoff_delay(99, 200), Duration::from_secs(60));
/// // Out-of-range jitter is clamped, so the answer is still bounded.
/// assert_eq!(backoff_delay(1, 9_000), Duration::from_millis(1_200));
/// assert_eq!(nominal_backoff(4) * 2, Duration::from_secs(16));
/// ```
pub fn backoff_delay(attempt: u32, jitter_permille: i32) -> Duration {
    let nominal = i128::try_from(nominal_backoff(attempt).as_millis()).unwrap_or(i128::MAX);
    let clamped = i128::from(jitter_permille.clamp(-JITTER_RANGE_PERMILLE, JITTER_RANGE_PERMILLE));
    let scaled = (nominal * (1_000 + clamped)) / 1_000;
    let capped = scaled.clamp(0, i128::from(BACKOFF_MAX.as_millis() as i64));
    Duration::from_millis(u64::try_from(capped).unwrap_or(0))
}

/// The whole range [`backoff_delay`] can return for `attempt`.
///
/// **The schedule's promise, stated once and separately checkable.**
/// `AGENTS.md` §4.2 asks for the backoff schedule to be *tested*; a test that
/// re-derives the window from the same arithmetic it is testing cannot fail, so
/// the window is its own function and the tests assert against that.
///
/// A caller that wants "how long might this take at worst" reads `window.end()`;
/// one that wants to bound a budget reads `window.start()`.
///
/// # Example
///
/// ```
/// use sh_nexus::network::ws::backoff_window;
/// use std::time::Duration;
///
/// let window = backoff_window(1);
/// assert_eq!(*window.start(), Duration::from_millis(800));
/// assert_eq!(*window.end(), Duration::from_millis(1_200));
/// ```
pub fn backoff_window(attempt: u32) -> RangeInclusive<Duration> {
    backoff_delay(attempt, -JITTER_RANGE_PERMILLE)..=backoff_delay(attempt, JITTER_RANGE_PERMILLE)
}

/// Draws one jitter value in `-200 ..= 200` permille.
///
/// # Where the randomness comes from
///
/// **`Uuid::new_v4()`'s bytes, and nothing new is declared for it.** `uuid` is
/// already a dependency of this crate with the `v4` feature enabled, and `v4`
/// implies `rng`, which is a `getrandom` call per generated id. So the jitter draw
/// and the `client_msg_id` that `AGENTS.md` §7.4 requires on every frame come
/// from one entropy source, at the cost of one further call per reconnection
/// attempt — at most one per second, on a thread that is about to sleep.
///
/// **Sixteen bits of it, mapped down to [`JITTER_BUCKETS`] buckets.** That is not
/// a uniformly random permille and it does not need to be: jitter exists to
/// de-correlate a fleet of clients that all lost the server at the same instant,
/// and a value drawn from 65 536 random bits serves that for any bucket count. It
/// would be wrong for anything that must hide a value, and nothing here does. The
/// modulo leaves a bias of at most one bucket, recorded rather than removed because
/// removing it would need a crate this one does not have.
pub fn draw_jitter() -> i32 {
    let drawn = Uuid::new_v4();
    let bytes = drawn.as_bytes();
    let raw = u16::from_be_bytes([bytes[0], bytes[1]]);
    i32::from(raw % JITTER_BUCKETS) - JITTER_RANGE_PERMILLE
}

// ---------------------------------------------------------------------------
// The keepalive decision, as a pure function
// ---------------------------------------------------------------------------

/// What the read half wants to do about a silent connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeepaliveAction {
    /// A frame arrived recently enough that nothing is due.
    Idle,
    /// Nothing has arrived for one interval: ping the peer.
    Ping,
    /// Nothing has arrived for two: the connection is presumed dead, so end it
    /// and let the backoff schedule reconnect.
    Abandon,
}

/// Decides what a silent read half should do, given how long it has been silent.
///
/// # Why this is a function and not a comment on the loop
///
/// `AGENTS.md` §4.3 forbids `sleep()` in a unit test and §4.2 requires the
/// keepalive to be tested. A 30-second threshold inside a `tokio::select!` can
/// only be tested by waiting 30 seconds; a function of an elapsed `Duration` can
/// be tested in microseconds, exhaustively, with no clock and no socket.
///
/// # Arguments
///
/// * `since_last_receive` - how long since a frame, a ping answer or a pong.
/// * `interval` - the keepalive interval; [`KEEPALIVE_INTERVAL`] in production.
///
/// # Returns
///
/// [`KeepaliveAction::Idle`] below one interval, [`KeepaliveAction::Ping`]
/// between one and [`KEEPALIVE_ABANDON_FACTOR`] intervals, and
/// [`KeepaliveAction::Abandon`] at or beyond that.
///
/// # Example
///
/// ```
/// use sh_nexus::network::ws::{keepalive_action, KeepaliveAction, KEEPALIVE_INTERVAL};
/// use std::time::Duration;
///
/// assert_eq!(keepalive_action(Duration::ZERO, KEEPALIVE_INTERVAL), KeepaliveAction::Idle);
/// assert_eq!(
///     keepalive_action(Duration::from_secs(30), KEEPALIVE_INTERVAL),
///     KeepaliveAction::Ping,
/// );
/// assert_eq!(
///     keepalive_action(Duration::from_secs(60), KEEPALIVE_INTERVAL),
///     KeepaliveAction::Abandon,
/// );
/// ```
pub fn keepalive_action(since_last_receive: Duration, interval: Duration) -> KeepaliveAction {
    if since_last_receive < interval {
        KeepaliveAction::Idle
    } else if since_last_receive < interval.saturating_mul(KEEPALIVE_ABANDON_FACTOR) {
        KeepaliveAction::Ping
    } else {
        KeepaliveAction::Abandon
    }
}

// ---------------------------------------------------------------------------
// The frame policy
// ---------------------------------------------------------------------------

/// What one unreadable server frame does to the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameFailure {
    /// One frame could not be read. Counted in
    /// [`TransportStats::frames_dropped`] and the connection stays up — which is
    /// `AGENTS.md` §3.3's rule that a network failure is recoverable rather than
    /// fatal, applied per frame.
    Transient,
    /// The peer announced a major version this build does not speak, which
    /// [`sh_nexus_wire::version::negotiate`] refuses before the frame's body is
    /// read. Terminal: a `Rejected` connection state is emitted and the transport
    /// stops, because reconnecting cannot succeed and a client that tries sits in
    /// a loop against a server that refuses it identically every time.
    Rejected(ConnectionState),
}

/// Decodes one server frame's text.
///
/// # Why the version check is this function's job to expose and not the caller's
///
/// `AGENTS.md` §7.4 requires an unknown major version to be "rejected
/// explicitly", and `sh_nexus_wire` does the rejecting inside
/// [`ServerEnvelope::decode`] — *before* it reads a single field of the body,
/// which is the whole reason that crate puts `v` on the envelope. What the wire
/// crate cannot decide is what this client does next, so it hands the refusal
/// here and the loop turns it into a policy. Keeping that split is what lets the
/// wire crate stay a pure transformation and this module keep the decision.
///
/// # Arguments
///
/// * `text` - the UTF-8 contents of one server text frame.
///
/// # Returns
///
/// The decoded [`ServerFrame`], or the [`WireError`] that stopped it.
///
/// # Errors
///
/// Whatever [`ServerEnvelope::decode`] returns: an unknown major version, an
/// unknown frame type, a frame travelling the wrong way, or malformed JSON. All
/// four are reported rather than guessed at; [`classify_frame_error`] is what
/// turns them into a policy.
///
/// # Example
///
/// ```
/// use sh_nexus::network::ws::{classify_frame_error, decode_server_frame, FrameFailure};
/// use sh_nexus_wire::version::PROTOCOL_VERSION;
///
/// // One major version ahead: refused, not half-understood.
/// let future = format!(
///     r#"{{"v":{},"type":"typing.update","user_id":"u_1","channel_id":"c_1","active":true}}"#,
///     PROTOCOL_VERSION + 1,
/// );
/// let error = decode_server_frame(&future).expect_err("an unknown major must be refused");
/// assert!(matches!(classify_frame_error(&error), FrameFailure::Rejected(_)));
///
/// // A frame type this client does not know: refused, connection kept.
/// let unknown = format!(r#"{{"v":{PROTOCOL_VERSION},"type":"something.new"}}"#);
/// let error = decode_server_frame(&unknown).expect_err("an unknown type must be refused");
/// assert_eq!(classify_frame_error(&error), FrameFailure::Transient);
/// ```
pub fn decode_server_frame(text: &str) -> Result<ServerFrame, WireError> {
    Ok(ServerEnvelope::decode(text)?.frame)
}

/// Turns a decode refusal into a policy.
///
/// # Errors
///
/// None. Whether a refusal is "this peer is incompatible" or "this one frame is
/// unreadable" is a fact about [`WireError`], not a judgement call, so it belongs
/// in a total function rather than in a branch somebody has to remember.
///
/// # Arguments
///
/// * `error` - the refusal [`decode_server_frame`] returned.
///
/// # Returns
///
/// [`FrameFailure::Rejected`] carrying
/// `ConnectionState::Rejected { code: UNSUPPORTED_VERSION_CODE, .. }` for
/// [`WireError::UnsupportedVersion`], and [`FrameFailure::Transient`] for
/// everything else.
///
/// The code that goes into the state is the one `sh_nexus_wire` defines, so the
/// user sees the protocol's own vocabulary rather than a rephrasing of it — and
/// the detail is the wire crate's peer-facing sentence, which explicitly does not
/// suggest reconnecting.
pub fn classify_frame_error(error: &WireError) -> FrameFailure {
    match error {
        WireError::UnsupportedVersion(rejection) => {
            FrameFailure::Rejected(ConnectionState::Rejected {
                code: UNSUPPORTED_VERSION_CODE.to_owned(),
                detail: rejection.detail(),
            })
        }
        WireError::MalformedPayload(_)
        | WireError::InvalidUtf8(_)
        | WireError::UnknownFrameType { .. }
        | WireError::WrongDirection { .. } => FrameFailure::Transient,
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// How one transport behaves.
///
/// Every timeout is a field rather than a constant so a test can shorten a wait
/// without shortening the *contract*: [`CONNECT_TIMEOUT`] and
/// [`KEEPALIVE_INTERVAL`] are the values production uses and the values the pure
/// schedule is documented against, and a test that needs a fast loop sets its own
/// through [`TransportConfig::with_keepalive_interval`]. A constant a test cannot
/// change is a constant a test stops asserting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportConfig {
    url: String,
    connect_timeout: Duration,
    keepalive_interval: Duration,
    max_attempts: Option<u32>,
    outbound_capacity: usize,
}

impl TransportConfig {
    /// A transport that talks to `url` and retries until told otherwise.
    ///
    /// # Arguments
    ///
    /// * `url` - the `ws://` endpoint. A `wss://` URL will fail at connect time
    ///   because this build enables no TLS feature; see the module docs.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::network::ws::{TransportConfig, CONNECT_TIMEOUT, KEEPALIVE_INTERVAL};
    ///
    /// let config = TransportConfig::new("ws://127.0.0.1:8484/ws");
    /// assert_eq!(config.url(), "ws://127.0.0.1:8484/ws");
    /// assert_eq!(config.connect_timeout(), CONNECT_TIMEOUT);
    /// assert_eq!(config.keepalive_interval(), KEEPALIVE_INTERVAL);
    /// assert_eq!(config.max_attempts(), None);
    /// ```
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            connect_timeout: CONNECT_TIMEOUT,
            keepalive_interval: KEEPALIVE_INTERVAL,
            max_attempts: None,
            outbound_capacity: MAX_OUTBOUND_FRAMES,
        }
    }

    /// Replaces the per-attempt connect timeout.
    ///
    /// # Arguments
    ///
    /// * `timeout` - the bound for one attempt. Setting it to zero makes every
    ///   connect time out immediately, which is a legitimate way to test the
    ///   timeout path itself.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Replaces the keepalive interval, and with it the abandon point.
    ///
    /// # Arguments
    ///
    /// * `interval` - silence before a ping. The abandon point is this times
    ///   [`KEEPALIVE_ABANDON_FACTOR`].
    pub fn with_keepalive_interval(mut self, interval: Duration) -> Self {
        self.keepalive_interval = interval;
        self
    }

    /// Gives up after `attempts` consecutive failures.
    ///
    /// `AGENTS.md` §4.2 lists "max-attempt behavior" as a required test, and a
    /// behaviour nothing can reach cannot be tested. `None` — the default — retries
    /// indefinitely, which is right for a desktop client whose server may be down
    /// for an afternoon.
    ///
    /// **Counted in consecutive failures, and a successful connect resets the
    /// counter.** A client that has been up for an hour gets the full allowance
    /// again after one blip. The two rules — this one and the backoff's
    /// reset-on-success — would otherwise disagree, and then the first few
    /// retries of a client's second outage would be cheaper than those of its
    /// first.
    ///
    /// # Arguments
    ///
    /// * `attempts` - how many failed attempts to allow. Zero refuses to try.
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = Some(attempts);
        self
    }

    /// Replaces the outbound queue's bound.
    ///
    /// # Arguments
    ///
    /// * `capacity` - frames that may be queued. A `mpsc::channel` panics on zero,
    ///   so zero is refused here as [`TransportError::OutboundFull`] rather than
    ///   reaching it.
    pub fn with_outbound_capacity(mut self, capacity: usize) -> Self {
        self.outbound_capacity = capacity;
        self
    }

    /// The endpoint this transport connects to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The per-attempt connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// The keepalive interval.
    pub fn keepalive_interval(&self) -> Duration {
        self.keepalive_interval
    }

    /// The consecutive-failure allowance, or `None` for unlimited.
    pub fn max_attempts(&self) -> Option<u32> {
        self.max_attempts
    }

    /// The outbound queue's bound.
    pub fn outbound_capacity(&self) -> usize {
        self.outbound_capacity
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything a caller of [`WsTransport`] can be told, and nothing else.
///
/// **No `tungstenite::Error` payload, and that is `errors.rs`'s decision rather
/// than this file's.** `docs/ARCHITECTURE.md` ADR-007 records why
/// `ShNexusError::WebSocket` and `ShNexusError::Network` carry a `String` today:
/// the typed `#[from]` payloads need a dependency audit that has not happened.
/// Putting a `tungstenite::Error` into *this* enum instead would put the same
/// payload into the crate through a different door, so it does not happen either.
/// Socket failures inside the loop are counted in [`TransportStats`] rather than
/// returned; this type is only what a *caller* can be told.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransportError {
    /// The worker thread could not be started.
    ///
    /// Named as its own variant rather than folded into a `String` because it is
    /// the one failure here that is about this machine rather than the network: it
    /// means the process refused another thread, and it will refuse again on the
    /// next attempt.
    #[error("could not start the websocket worker thread: {0}")]
    Thread(String),

    /// The outbound queue is at its bound, or its bound was set to zero.
    ///
    /// **The frame is gone and that is the point.** A refusal the caller cannot
    /// see is a message that vanished, and a queue that grew instead would be
    /// `AGENTS.md` §7.1's "unbounded growth of in-memory state" reached by a user's
    /// typing outrunning a broken network. The optimistic row in the client is
    /// what the caller falls back to.
    #[error("the outbound queue is full at {capacity} frames")]
    OutboundFull {
        /// The bound that was reached, which is zero for a misconfigured queue.
        capacity: usize,
    },

    /// The transport has stopped, so no further frame will be sent.
    #[error("the transport is closed and will send nothing further")]
    ShutDown,
}

// ---------------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------------

/// The atomics behind [`TransportStats`], shared by the handle and the worker.
///
/// Private, and separate from the snapshot type so a caller can hold a
/// [`TransportStats`] without holding a lock. `AGENTS.md` §7.1's "no unbounded
/// growth" and `bridge.rs`'s no-`Mutex` argument are both satisfied by a fixed set
/// of counters written with `Relaxed` ordering and read as a snapshot.
#[derive(Debug, Default)]
struct Counters {
    connects: AtomicU64,
    connect_failures: AtomicU64,
    connect_timeouts: AtomicU64,
    runtime_failures: AtomicU64,
    frames_read: AtomicU64,
    frames_sent: AtomicU64,
    frames_dropped: AtomicU64,
    events_delivered: AtomicU64,
    events_refused: AtomicU64,
    writes_failed: AtomicU64,
    resyncs_sent: AtomicU64,
    pings_sent: AtomicU64,
    keepalive_abandoned: AtomicU64,
}

impl Counters {
    /// Adds one. `Relaxed` throughout, deliberately.
    ///
    /// These are statistics, not synchronisation: nothing branches on them inside
    /// the loop, so ordering them against anything would cost fences on a path
    /// that runs once per frame to produce a number no control flow reads.
    fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> TransportStats {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        TransportStats {
            connects: read(&self.connects),
            connect_failures: read(&self.connect_failures),
            connect_timeouts: read(&self.connect_timeouts),
            runtime_failures: read(&self.runtime_failures),
            frames_read: read(&self.frames_read),
            frames_sent: read(&self.frames_sent),
            frames_dropped: read(&self.frames_dropped),
            events_delivered: read(&self.events_delivered),
            events_refused: read(&self.events_refused),
            writes_failed: read(&self.writes_failed),
            resyncs_sent: read(&self.resyncs_sent),
            pings_sent: read(&self.pings_sent),
            keepalive_abandoned: read(&self.keepalive_abandoned),
        }
    }
}

/// What a transport has done, as a plain copyable snapshot.
///
/// # Why this exists instead of log lines
///
/// The module docs give the reason: this crate declares no `tracing`, and
/// `AGENTS.md` §7.1 bans `println!`, so a dropped frame would otherwise have
/// nowhere to go but silence. Every condition that deserves a line has a counter
/// here, which means a caller — or a test — can ask.
///
/// Six fields are worth watching: `events_refused` (the inbox is full),
/// `frames_dropped` (a frame could not be read), `writes_failed` (a frame could
/// not be written), `connect_timeouts` (a handshake that never answered),
/// `runtime_failures` (no async runtime could be built at all) and
/// `keepalive_abandoned` (a socket that stopped being readable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransportStats {
    /// Successful connects, including the first one.
    pub connects: u64,
    /// Connect attempts that failed before a socket existed.
    pub connect_failures: u64,
    /// Connect attempts that hit the per-attempt timeout.
    pub connect_timeouts: u64,
    /// Attempts abandoned because no `tokio` runtime could be built.
    pub runtime_failures: u64,
    /// Server text frames read.
    pub frames_read: u64,
    /// Client text frames written.
    pub frames_sent: u64,
    /// Server frames discarded without reaching the inbox: undecodable, of an
    /// unknown type, or binary on a text-only protocol.
    pub frames_dropped: u64,
    /// Domain events accepted by the inbox.
    pub events_delivered: u64,
    /// Domain events the inbox refused because it was full.
    pub events_refused: u64,
    /// Frames whose write failed.
    pub writes_failed: u64,
    /// `resync` frames written, including every replay after a reconnect.
    pub resyncs_sent: u64,
    /// Keepalive pings written.
    pub pings_sent: u64,
    /// Connections ended because the keepalive gave up on them.
    pub keepalive_abandoned: u64,
}

impl fmt::Display for TransportStats {
    /// One line, and no frame contents.
    ///
    /// These are counts. There is nothing here that could be a message, which is
    /// why this type can be printed at all.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} connects, {} frames read, {} frames sent, {} delivered, {} refused, \
             {} dropped, {} resyncs",
            self.connects,
            self.frames_read,
            self.frames_sent,
            self.events_delivered,
            self.events_refused,
            self.frames_dropped,
            self.resyncs_sent,
        )
    }
}

// ---------------------------------------------------------------------------
// The handle
// ---------------------------------------------------------------------------

/// A handle to a running WebSocket transport.
///
/// `Send + Sync + Clone`, and holding **no interior mutable state of its own**
/// beyond an `mpsc::Sender`, an `Arc<Notify>` and an `Arc<AtomicBool>`. That is
/// what lets the main thread keep one while a worker thread owns the socket, and
/// it is why there is no lock anywhere in this module.
///
/// # What this handle cannot do
///
/// It cannot read a frame, cannot reach the socket, and cannot wait for anything.
/// The verbs are [`WsTransport::send_message`], [`WsTransport::request_resync`]
/// and [`WsTransport::shutdown`]; everything else arrives through the inbox as a
/// [`DomainEvent`].
#[derive(Clone)]
pub struct WsTransport {
    outbound: mpsc::Sender<Outbound>,
    shutdown: Arc<Notify>,
    closed: Arc<AtomicBool>,
    counters: Arc<Counters>,
}

impl WsTransport {
    /// Starts the worker thread and returns immediately.
    ///
    /// # Why it does not wait for a socket
    ///
    /// `AGENTS.md` §2.3 forbids blocking the frame loop, and the caller of this
    /// function is the main thread. Waiting for the connect here would put up to
    /// [`CONNECT_TIMEOUT`] of network wait inside a frame. So the caller learns
    /// the outcome the same way it learns everything else: as a
    /// [`DomainEvent::ConnectionStateChanged`] event, which arrives as
    /// [`ConnectionState::Connecting`] first and [`ConnectionState::Connected`] or
    /// [`ConnectionState::Disconnected`] later.
    ///
    /// # Arguments
    ///
    /// * `config` - the transport's behaviour. See [`TransportConfig`].
    /// * `events` - the producing half of the seam. Cloned into the worker; the
    ///   caller keeps its own copy for whatever ends the inbox's life.
    ///
    /// # Returns
    ///
    /// A handle. The only failure is the thread itself.
    ///
    /// # Errors
    ///
    /// [`TransportError::Thread`], and
    /// [`TransportError::OutboundFull`] if the configuration asked for a
    /// zero-capacity queue. Nothing *on the network* can fail here: a refused
    /// connection is a [`ConnectionState::Disconnected`] event and a refused
    /// version is a [`ConnectionState::Rejected`] one, both of which reach the
    /// caller through the seam this crate already has.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use sh_nexus::network::ws::{epoch_cursor, TransportConfig, WsTransport};
    /// use sh_nexus::state::bridge::EventSender;
    ///
    /// fn start_one(
    ///     events: EventSender,
    /// ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ///     let transport = WsTransport::start(
    ///         TransportConfig::new("ws://127.0.0.1:8484/ws"),
    ///         events,
    ///     )?;
    ///     transport.request_resync("general", epoch_cursor())?;
    ///     Ok(())
    /// }
    /// # let _ = start_one;
    /// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    /// ```
    pub fn start(config: TransportConfig, events: EventSender) -> Result<Self, TransportError> {
        if config.outbound_capacity == 0 {
            // Checked here rather than left to `mpsc::channel`, which panics on
            // zero. `AGENTS.md` §2.1 forbids a panic on a path a caller can reach,
            // and a configuration a caller wrote is exactly such a path.
            return Err(TransportError::OutboundFull { capacity: 0 });
        }

        let (outbound, inbound) = mpsc::channel(config.outbound_capacity);
        let shutdown = Arc::new(Notify::new());
        let closed = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());

        let session = Session {
            config: Arc::new(config),
            events,
            outbound: inbound,
            cursors: BTreeMap::new(),
            counters: Arc::clone(&counters),
            shutdown: Arc::clone(&shutdown),
            closed: Arc::clone(&closed),
        };

        let worker_counters = Arc::clone(&counters);
        let worker_closed = Arc::clone(&closed);
        std::thread::Builder::new()
            .name("sh_nexus-ws".to_owned())
            .spawn(move || {
                // A runtime failure is counted rather than reported, and the
                // reason it is not reported is the point: the only channel this
                // thread has to the rest of the program *is* the inbox, and the
                // inbox needs no runtime — so the honest report is the counter,
                // and `is_closed` becomes true immediately, which is the signal a
                // caller actually has. Retaining the message would mean a lock,
                // and `AGENTS.md` §2.3 puts no lock on a per-frame path.
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        runtime.block_on(session.run());
                        worker_closed.store(true, Ordering::Release);
                    }
                    Err(_) => {
                        Counters::bump(&worker_counters.runtime_failures);
                        worker_closed.store(true, Ordering::Release);
                    }
                }
            })
            .map_err(|error| TransportError::Thread(error.to_string()))?;

        Ok(Self {
            outbound,
            shutdown,
            closed,
            counters,
        })
    }

    /// Queues a `message.send` carrying the optimistic row's own identity.
    ///
    /// # Why `client_msg_id` is an argument and not generated here
    ///
    /// `AGENTS.md` §7.4 requires a client-generated UUID on every frame so the
    /// server can dedupe a replayed send after a reconnect. The identity that
    /// dedupes is the one the **optimistic row already has** — it is what
    /// `actions::begin_send` put on screen, and what `message.ack` will be matched
    /// against. Minting a second id here would give the server one identity and
    /// the user's screen another, and the send would never reconcile. So the
    /// caller mints it, exactly as `bridge::try_begin_send` requires.
    ///
    /// # Arguments
    ///
    /// * `client_msg_id` - the same `Uuid` the optimistic row carries.
    /// * `channel_id` - the target channel.
    /// * `content` - the message body, as authored.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the frame is queued — **not** once the server has it. There
    /// is no acknowledgement here; `message.ack` is that, and it arrives through
    /// the inbox as [`DomainEvent::MessageAcked`].
    ///
    /// # Errors
    ///
    /// [`TransportError::OutboundFull`] if the queue is at
    /// [`MAX_OUTBOUND_FRAMES`], and [`TransportError::ShutDown`] if the transport
    /// has stopped. Neither blocks: `AGENTS.md` §2.3 forbids blocking the frame
    /// loop, and this is called from it.
    pub fn send_message(
        &self,
        client_msg_id: Uuid,
        channel_id: &str,
        content: &str,
    ) -> Result<(), TransportError> {
        self.queue(Outbound::Once {
            id: client_msg_id,
            frame: ClientFrame::MessageSend {
                channel_id: channel_id.to_owned(),
                content: content.to_owned(),
            },
        })
    }

    /// Records a per-channel resume cursor and sends `resync` for it.
    ///
    /// # The rule this implements
    ///
    /// `AGENTS.md` §7.4: *"Reconnection must resume from the last known
    /// `last_message_at` cursor — never rely solely on 'live' delivery during a
    /// gap."* The cursor is per channel because each channel has its own
    /// `last_message_at` and its own unread count; one global cursor is either too
    /// old — replaying what the user read — or too new, skipping a channel they
    /// had not caught up on, and §8.1's Reconnect Flow requires no duplicates
    /// *and* no gaps.
    ///
    /// # Why the cursor is remembered here and not read from local state
    ///
    /// The cursor lives in `AppState`, and `AppState` is main-thread-only
    /// (`docs/ARCHITECTURE.md` ADR-009). This thread cannot read it, and
    /// [`DomainEvent::ResyncRequested`] exists precisely because the answer is
    /// local state. So the caller reads it on the main thread and hands it over
    /// here — and once handed over it is remembered, so **every** future reconnect
    /// replays it without the caller being asked again. That is the difference
    /// between sending a resync and reconnecting that resumes.
    ///
    /// # Arguments
    ///
    /// * `channel_id` - the channel to catch up.
    /// * `after` - the inclusive cursor: everything strictly later is wanted.
    ///   [`epoch_cursor`] when this client holds nothing for the channel.
    ///
    /// # Returns
    ///
    /// `Ok(())` once queued. No server answers a `resync` in this milestone —
    /// `sh_nexus_server/src/ws.rs` decodes it, logs it at `warn!` and keeps the
    /// connection — so there is nothing to wait for and the frame's effect is
    /// counted in [`TransportStats::resyncs_sent`] instead.
    ///
    /// # Errors
    ///
    /// As [`WsTransport::send_message`]. A refusal loses the *request*, not the
    /// cursor: a later call with the same channel replaces what was remembered,
    /// so a user who switches channels during an outage converges rather than
    /// accumulating one stale cursor per attempt.
    pub fn request_resync(
        &self,
        channel_id: &str,
        after: DateTime<Utc>,
    ) -> Result<(), TransportError> {
        self.queue(Outbound::Resync {
            channel_id: channel_id.to_owned(),
            after,
        })
    }

    /// Queues `typing.start` for `channel_id`.
    ///
    /// A method pair rather than one with a boolean, because the wire has two frame
    /// types and the boolean would have to be mapped back to one here.
    /// `typing.start` with no matching stop is a stuck indicator, which §8.1's
    /// Typing Indicator Flow forbids; [`WsTransport::stop_typing`] is the other
    /// half.
    ///
    /// # Arguments
    ///
    /// * `channel_id` - the channel being typed in.
    ///
    /// # Errors
    ///
    /// As [`WsTransport::send_message`].
    pub fn start_typing(&self, channel_id: &str) -> Result<(), TransportError> {
        self.queue(Outbound::Once {
            id: Uuid::new_v4(),
            frame: ClientFrame::TypingStart {
                channel_id: channel_id.to_owned(),
            },
        })
    }

    /// Queues `typing.stop` for `channel_id`. See [`WsTransport::start_typing`].
    ///
    /// # Arguments
    ///
    /// * `channel_id` - the channel the user stopped typing in.
    ///
    /// # Errors
    ///
    /// As [`WsTransport::send_message`].
    pub fn stop_typing(&self, channel_id: &str) -> Result<(), TransportError> {
        self.queue(Outbound::Once {
            id: Uuid::new_v4(),
            frame: ClientFrame::TypingStop {
                channel_id: channel_id.to_owned(),
            },
        })
    }

    /// Queues a `reaction.add`.
    ///
    /// # Arguments
    ///
    /// * `message_id` - the message being reacted to.
    /// * `emoji` - the emoji, byte-for-byte. Not normalised, because a reaction key
    ///   has to be identical across clients for two of them to agree they are
    ///   showing the same reaction.
    ///
    /// # Errors
    ///
    /// As [`WsTransport::send_message`].
    pub fn add_reaction(&self, message_id: &str, emoji: &str) -> Result<(), TransportError> {
        self.queue(Outbound::Once {
            id: Uuid::new_v4(),
            frame: ClientFrame::ReactionAdd {
                message_id: message_id.to_owned(),
                emoji: emoji.to_owned(),
            },
        })
    }

    /// What this transport has done. See [`TransportStats`].
    ///
    /// Cheap, lock-free and safe to call from the frame path: one relaxed atomic
    /// load per field, and nothing here can block.
    pub fn stats(&self) -> TransportStats {
        self.counters.snapshot()
    }

    /// Stops the transport, eventually and without blocking.
    ///
    /// # Why it does not block, and what "eventually" costs
    ///
    /// The caller is the main thread, and joining a thread that is inside a
    /// 30-second read timeout is a 30-second frame. So this sets a flag, wakes the
    /// loop through a [`Notify`] and returns. The worker finishes its current
    /// iteration, leaves the loop, and the thread ends.
    ///
    /// The consequence worth naming: a frame queued but not yet written is
    /// dropped. That is the right trade at shutdown — the alternative is blocking
    /// the interface for the length of a network timeout — and it is safe because
    /// every queued frame carries an identity the optimistic row already has, so a
    /// send that did not leave is a `Pending` row the user can retry rather than a
    /// lost message. [`TransportError::ShutDown`] on every later call is how a
    /// caller learns the transport is gone.
    ///
    /// Idempotent: calling it twice, or from two clones, is one call.
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        // `notify_one` rather than `notify_waiters`: the former stores a permit
        // when nobody is waiting yet, which closes the window between the loop
        // checking the flag and registering its interest. The flag check is kept
        // anyway, because a correctness argument should not rest on one wake-up
        // winning a race.
        self.shutdown.notify_one();
    }

    /// Whether this transport will send nothing further.
    ///
    /// True after [`WsTransport::shutdown`], and also once the worker has ended on
    /// its own — a refused version, or the max-attempt allowance running out —
    /// because in both cases no further frame will be sent.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Puts one item on the outbound queue, or says why it could not.
    fn queue(&self, item: Outbound) -> Result<(), TransportError> {
        if self.is_closed() {
            return Err(TransportError::ShutDown);
        }
        match self.outbound.try_send(item) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(TransportError::OutboundFull {
                capacity: self.outbound.max_capacity(),
            }),
            // The worker ended without being asked — a refused version, or a
            // runtime that could not be built — so the queue is not full, it is
            // closed. Reporting that as a full queue would send a caller looking
            // for a backlog that does not exist.
            Err(TrySendError::Closed(_)) => Err(TransportError::ShutDown),
        }
    }
}

impl fmt::Debug for WsTransport {
    /// The shape, never the contents.
    ///
    /// A `Debug` on a sender of queued frames would print them, and a queued frame
    /// carries message content — which `AGENTS.md` §7.5 forbids reaching a log, and
    /// a `{:?}` inside a panic message is a log.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WsTransport")
            .field("closed", &self.is_closed())
            .field("stats", &self.stats())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// One queued frame, the identity it travels under, and whether the loop must
/// remember it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outbound {
    /// A frame that is written once and forgotten.
    Once {
        /// The envelope's `client_msg_id`.
        ///
        /// **Not always fresh, and that is the whole of `AGENTS.md` §7.4's dedupe
        /// requirement.** A `message.send` carries the identity the optimistic row
        /// already has, so that the server's replay of it is recognisable; every
        /// other frame carries a fresh `Uuid::new_v4()`, because nothing
        /// acknowledges them and nothing replays them.
        id: Uuid,
        /// The frame itself.
        frame: ClientFrame,
    },
    /// A `resync`, whose cursor is remembered so that **every** future connect
    /// replays it. See [`WsTransport::request_resync`].
    Resync {
        /// The channel to catch up.
        channel_id: String,
        /// The inclusive cursor.
        after: DateTime<Utc>,
    },
}

/// What one connect-and-serve attempt produced.
enum Outcome {
    /// A connection was established and has now ended for a reason worth
    /// retrying: a dropped socket, a peer close, or a silent peer.
    Established,
    /// The connect never produced a socket.
    Failed,
    /// The peer is incompatible. Terminal.
    Rejected(ConnectionState),
    /// The transport was shut down. Terminal, and not a failure.
    Stopped,
}

/// Everything the worker thread owns.
///
/// The fields are destructured apart before the read loop's `select!`, because
/// `tokio::select!` builds every branch future before polling any of them, and
/// two of those futures hold borrows of different fields. Splitting them into
/// local bindings is what lets the compiler see that the `outbound` borrow and the
/// `cursors` borrow do not overlap.
struct Session {
    config: Arc<TransportConfig>,
    events: EventSender,
    outbound: mpsc::Receiver<Outbound>,
    cursors: BTreeMap<String, DateTime<Utc>>,
    counters: Arc<Counters>,
    shutdown: Arc<Notify>,
    closed: Arc<AtomicBool>,
}

impl Session {
    /// The whole transport: connect, serve, back off, repeat.
    ///
    /// # The two counters, and why they are separate
    ///
    /// `attempt` is the backoff step. A connection that was *established* sets it
    /// to zero, which is `AGENTS.md` §4.2's "reset on success": a client that was
    /// up for an hour and then dropped should get the 1-second first retry, not
    /// the tenth-minute one. `ever_connected` only decides whether the state
    /// emitted before an attempt says `Connecting` — nothing has worked yet — or
    /// `Reconnecting { attempt }`, because a UI that cannot tell those apart
    /// cannot say "connecting" versus "retrying", and §3.3 requires the difference
    /// to be visible.
    ///
    /// The delay after a *successful* connection is `backoff_delay(1, jitter)`
    /// rather than zero, because a server that accepts and immediately drops would
    /// otherwise be reconnected to as fast as it can close a socket.
    async fn run(mut self) {
        let mut attempt: u32 = 0;
        let mut ever_connected = false;

        loop {
            if self.closed.load(Ordering::Acquire) {
                return;
            }

            self.emit_connection(if attempt == 0 && !ever_connected {
                ConnectionState::Connecting
            } else {
                ConnectionState::Reconnecting {
                    attempt: attempt.max(1),
                }
            });

            if self
                .config
                .max_attempts
                .is_some_and(|limit| attempt >= limit)
            {
                // Reported rather than silent, so "the client gave up" and "the
                // client is still trying" are different things to whoever is
                // watching.
                self.emit_connection(ConnectionState::Disconnected);
                return;
            }

            match self.connect_and_serve().await {
                Outcome::Established => {
                    attempt = 0;
                    ever_connected = true;
                    self.emit_connection(ConnectionState::Disconnected);
                }
                Outcome::Rejected(state) => {
                    // Terminal, and the reason this is not a retry loop: §7.4 says
                    // reject explicitly, and a `Rejected` state the user can see is
                    // what "explicitly" means in a user interface.
                    self.emit_connection(state);
                    return;
                }
                Outcome::Failed => {
                    attempt = attempt.saturating_add(1);
                }
                Outcome::Stopped => return,
            }

            let delay = backoff_delay(attempt.max(1), draw_jitter());
            if !self.sleep_or_stop(delay).await {
                return;
            }
        }
    }

    /// Waits `delay`, or returns `false` if the transport was shut down.
    ///
    /// The shutdown branch races the timer rather than being checked after it,
    /// because a shutdown that had to wait out a 60-second backoff would leave the
    /// thread — and its socket — alive for a minute after the user closed the
    /// application.
    async fn sleep_or_stop(&self, delay: Duration) -> bool {
        let stopping = self.shutdown.notified();
        tokio::pin!(stopping);
        tokio::select! {
            _ = tokio::time::sleep(delay) => true,
            _ = &mut stopping => false,
        }
    }

    /// Connects, replays the cursors, serves, and reports what happened.
    async fn connect_and_serve(&mut self) -> Outcome {
        let mut socket = match tokio::time::timeout(
            self.config.connect_timeout,
            connect_async(self.config.url.as_str()),
        )
        .await
        {
            Ok(Ok((socket, _response))) => socket,
            // The handshake answered with something other than a 101, or the host
            // did not resolve. Both are "not now", and both are counted.
            Ok(Err(_)) => {
                Counters::bump(&self.counters.connect_failures);
                return Outcome::Failed;
            }
            // §7.4's connect timeout, per attempt rather than for a whole
            // reconnection: the difference between a server that is not there and
            // one that accepted a socket and stopped answering.
            Err(_) => {
                Counters::bump(&self.counters.connect_timeouts);
                return Outcome::Failed;
            }
        };

        Counters::bump(&self.counters.connects);

        // The cursors go out **before** the loop starts reading, and therefore
        // before any live frame is processed. A catch-up that ran concurrently with
        // live delivery could interleave, and the result would then depend on how
        // fast the two happened to arrive rather than on anything the client
        // controls. Collected first because the borrow of `cursors` has to end
        // before the socket is written to.
        let replay: Vec<ClientFrame> = self
            .cursors
            .iter()
            .map(|(channel_id, after)| ClientFrame::Resync {
                channel_id: channel_id.clone(),
                after: *after,
            })
            .collect();
        for frame in replay {
            if !self.write(&mut socket, Uuid::new_v4(), frame).await {
                return Outcome::Established;
            }
        }

        self.emit_connection(ConnectionState::Connected);
        self.serve(socket).await
    }

    /// The read, write and keepalive loop for one established connection.
    async fn serve(&mut self, mut socket: Socket) -> Outcome {
        let Session {
            config,
            events,
            counters,
            shutdown,
            cursors,
            outbound,
            ..
        } = self;

        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + config.keepalive_interval,
            config.keepalive_interval,
        );
        // `Delay` rather than `Burst`: a loop starved by a long inbound burst must
        // not then fire a burst of ticks and abandon a connection that was fine.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut silent_for = Duration::ZERO;
        let stopping = shutdown.notified();
        tokio::pin!(stopping);

        loop {
            // `biased` is deliberate and the order is deliberate. Shutdown and the
            // keepalive clock are *time*, and a socket under load can keep the read
            // branch permanently ready; with a random order a busy connection could
            // postpone its own ping indefinitely and then abandon itself for having
            // been quiet. Ordering the clock above the read means a busy connection
            // still pings, and still notices a peer that stopped.
            tokio::select! {
                biased;
                _ = &mut stopping => return Outcome::Stopped,
                _ = ticker.tick() => {
                    silent_for = silent_for.saturating_add(config.keepalive_interval);
                    match keepalive_action(silent_for, config.keepalive_interval) {
                        KeepaliveAction::Idle => {}
                        KeepaliveAction::Ping => {
                            Counters::bump(&counters.pings_sent);
                            if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                                Counters::bump(&counters.writes_failed);
                                return Outcome::Established;
                            }
                        }
                        KeepaliveAction::Abandon => {
                            Counters::bump(&counters.keepalive_abandoned);
                            return Outcome::Established;
                        }
                    }
                }
                queued = outbound.recv() => match queued {
                    // Every handle is gone, so nothing can ever be sent again.
                    None => return Outcome::Stopped,
                    Some(item) => {
                        let (id, frame, remember) = split_outbound(item);
                        if remember {
                            if let ClientFrame::Resync { channel_id, after } = &frame {
                                cursors.insert(channel_id.clone(), *after);
                            }
                        }
                        if !write_frame(&mut socket, id, frame, counters).await {
                            return Outcome::Established;
                        }
                    }
                },
                incoming = socket.next() => match incoming {
                    // End of stream, or a transport error. Both mean the same thing
                    // to this loop: whatever was sent is lost and the socket must be
                    // replaced. Which one it was is a counter on the next attempt,
                    // not a branch here.
                    None => return Outcome::Established,
                    Some(Err(_)) => return Outcome::Established,
                    Some(Ok(message)) => {
                        silent_for = Duration::ZERO;
                        if let Outcome::Rejected(state) = on_message(events, counters, &message) {
                            return Outcome::Rejected(state);
                        }
                    }
                },
            }
        }
    }

    /// Writes one frame, counting a failure and reporting whether to keep going.
    async fn write(&self, socket: &mut Socket, id: Uuid, frame: ClientFrame) -> bool {
        write_frame(socket, id, frame, &self.counters).await
    }

    /// Emits a connection state and counts a refusal.
    ///
    /// A refused *connection state* is counted in the same place as a refused
    /// message event, deliberately: both mean the main thread is not keeping up
    /// with this socket, and a UI that watched only one of them would see a
    /// healthy client in one case and a healthy network in the other.
    fn emit_connection(&self, state: ConnectionState) {
        let outcome = self
            .events
            .deliver(DomainEvent::ConnectionStateChanged(state));
        if matches!(outcome, Delivery::Refused(_)) {
            Counters::bump(&self.counters.events_refused);
        }
    }
}

/// Splits an [`Outbound`] into the frame to write, the identity it travels under,
/// and whether to remember it.
///
/// A function rather than a method because the read loop holds disjoint borrows of
/// the [`Session`]'s fields, and a method call would want all of them at once.
///
/// **A remembered `resync` gets a fresh identity on every replay**, which is
/// correct and worth stating: the envelope's `client_msg_id` is a *frame*
/// identity, not a *request* identity. Nothing acknowledges a `resync` and
/// nothing dedupes one, and reusing a single id across replays would put the same
/// identity on two distinct frames — which is the exact shape of the bug §7.4's
/// dedupe exists to prevent, one layer up.
fn split_outbound(item: Outbound) -> (Uuid, ClientFrame, bool) {
    match item {
        Outbound::Once { id, frame } => (id, frame, false),
        Outbound::Resync { channel_id, after } => (
            Uuid::new_v4(),
            ClientFrame::Resync { channel_id, after },
            true,
        ),
    }
}

/// Encodes and writes one client frame, counting both outcomes.
///
/// **Every frame goes through [`ClientEnvelope::new`], and that is the whole of
/// `AGENTS.md` §7.4's client-side rules.** `v` is stamped from
/// [`PROTOCOL_VERSION`] by the envelope's own constructor, so a frame cannot go
/// out unversioned, and `client_msg_id` is supplied by the caller — the
/// optimistic row's identity for a `message.send`, a fresh `Uuid::new_v4()` for
/// everything else. Both are rendered as text because the field is a `String` on
/// an untrusted wire and this is the only place that could put something that is
/// not a UUID in it. A frame that arrived without the field would not decode at
/// all, so this is not a nicety.
async fn write_frame(
    socket: &mut Socket,
    id: Uuid,
    frame: ClientFrame,
    counters: &Counters,
) -> bool {
    let envelope = ClientEnvelope::new(id.hyphenated().to_string(), frame);
    // An encode failure is impossible for any value these types can hold — see
    // `ClientEnvelope::encode` for why it is a `Result` anyway — and is reported
    // rather than unwrapped because `AGENTS.md` §2.1 forbids a panic in a network
    // send path.
    let payload = match envelope.encode() {
        Ok(payload) => payload,
        Err(_) => {
            Counters::bump(&counters.frames_dropped);
            return true;
        }
    };

    let is_resync = matches!(envelope.frame, ClientFrame::Resync { .. });
    match socket.send(Message::Text(payload.into())).await {
        Ok(()) => {
            Counters::bump(&counters.frames_sent);
            if is_resync {
                Counters::bump(&counters.resyncs_sent);
            }
            true
        }
        Err(_) => {
            Counters::bump(&counters.writes_failed);
            false
        }
    }
}

/// Decodes one incoming message, maps it, and hands it to the inbox.
///
/// Returns [`Outcome::Rejected`] only for the one case §7.4 makes terminal.
/// Everything else — a frame this build cannot read, a frame whose payload fails
/// the boundary's validation, a binary frame on a text-only protocol — is counted
/// and dropped, because `AGENTS.md` §3.3 requires a network failure to be a
/// recoverable state rather than the end of the connection.
///
/// **The [`DomainEvent`] is produced by the boundary, not here.**
/// `network::mapping`'s `TryFrom<ServerFrame> for DomainEvent` has validated every
/// id, timestamp and attachment size by the time the conversion returns, which is
/// what lets this function hand the value straight to [`EventSender::deliver`]
/// with no further checking.
fn on_message(events: &EventSender, counters: &Counters, message: &Message) -> Outcome {
    let text = match message {
        Message::Text(text) => text.as_str(),
        // §3.3's rule, applied to a frame type: a binary frame is not a frame this
        // protocol has, and refusing it must not end a connection.
        Message::Binary(_) => {
            Counters::bump(&counters.frames_dropped);
            return Outcome::Established;
        }
        // A pong is the keepalive's answer, and tungstenite has already matched it
        // to its ping. There is nothing further to do with either, and both reset
        // the silence clock, which is the entire point of §7.4's keepalive.
        Message::Ping(_) | Message::Pong(_) => return Outcome::Established,
        Message::Close(_) => return Outcome::Established,
        Message::Frame(_) => {
            Counters::bump(&counters.frames_dropped);
            return Outcome::Established;
        }
    };

    Counters::bump(&counters.frames_read);

    let frame = match decode_server_frame(text) {
        Ok(frame) => frame,
        Err(error) => {
            Counters::bump(&counters.frames_dropped);
            return match classify_frame_error(&error) {
                FrameFailure::Transient => Outcome::Established,
                FrameFailure::Rejected(state) => Outcome::Rejected(state),
            };
        }
    };

    match DomainEvent::try_from(frame) {
        Ok(event) => match events.deliver(event) {
            Delivery::Queued => {
                Counters::bump(&counters.events_delivered);
                Outcome::Established
            }
            // A full inbox is `bridge.rs`'s decision and it hands the event back;
            // this loop has nothing to do with the returned value except say so,
            // because the alternative — retrying later — would need a queue in
            // front of a queue, which is the unbounded growth §7.1 forbids.
            Delivery::Refused(_) => {
                Counters::bump(&counters.events_refused);
                Outcome::Established
            }
        },
        Err(_) => {
            Counters::bump(&counters.frames_dropped);
            Outcome::Established
        }
    }
}
