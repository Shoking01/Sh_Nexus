//! Fan-out: one broadcast per instance, no per-connection state.
//!
//! # Why a `tokio::sync::broadcast` channel and not a registry of senders
//!
//! A chat server has to answer "who else is listening?" on every accepted send.
//! The two obvious answers both need bookkeeping that outlives a connection:
//!
//! - A `HashMap<ConnectionId, mpsc::Sender<..>>` needs an entry removed when a
//!   connection dies, and a peer that vanishes without a close frame leaves that
//!   entry until something notices. `AGENTS.md` §7.1 forbids unbounded growth of
//!   in-memory state, so the registry needs its own reaping, and the reaping is a
//!   second failure mode.
//! - A broadcast channel drops a sender when it is dropped and has no entry at
//!   all. A connection that dies takes its receiver with it and the channel
//!   forgets it. `tokio::sync::broadcast::Sender::send` also reports how many
//!   receivers got the value, which is the number worth logging about a message
//!   that reached nobody.
//!
//! The cost of the broadcast is that a *slow* receiver does not hold anything up
//! -- it is lagged instead, and [`Delivery`] carries the origin so a connection
//! can skip its own echo. See [`Delivery::origin`] for why the echo is skipped.
//!
//! # Bounded, on purpose
//!
//! [`Hub::CAPACITY`] frames. A channel with no bound would let one connection
//! that stops reading grow the instance's memory until the process is killed, and
//! `AGENTS.md` §7.1 names unbounded growth of in-memory state as a prohibition
//! rather than a smell. When a connection falls behind by more than the capacity,
//! `recv` yields `RecvError::Lagged` and `ws.rs` logs it and carries on: a
//! dropped frame is a visible gap in a transcript, which
//! `crates/sh_nexus_wire/src/error.rs` argues is strictly better than a
//! fabricated one.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sh_nexus_wire::ServerEnvelope;
use tokio::sync::broadcast;

/// How many undelivered frames a connection may fall behind by.
///
/// Sized against the two flows this milestone has. A burst of sends from one
/// client, and a `message.new` per send on every other connection, is a
/// multiplicative factor equal to the number of clients -- so at a hundred
/// clients a thousand sends in quick succession is ten frames each, and 256 is
/// twenty-five times that. A tuning knob rather than a derived number because the
/// right value depends on the deployment ADR-010 describes, and a self-hosted
/// operator is the only one who knows it.
pub const CAPACITY: usize = 256;

/// Identifies one WebSocket connection for the life of that connection.
///
/// Not a user. There is no user identity in this milestone
/// (see `db.rs`'s module docs), and an id that outlived its connection would
/// invite code to treat it as something it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId(u64);

impl fmt::Display for ConnectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl ConnectionId {
    /// The raw counter value.
    ///
    /// Public only so a log line can carry it as a number; there is no operation
    /// that takes one, because nothing may address a connection by id.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One broadcast frame, tagged with the connection that caused it.
///
/// The tag exists because the origin's frame is **not** filtered before the send.
/// `tokio::sync::broadcast` delivers to every receiver and has no way to except
/// one, so filtering a per-connection registry would mean giving up the property
/// this module exists for -- that a dead connection is forgotten because its
/// receiver was dropped, with no reaping code. Instead the origin's delivery sits
/// in its own queue and [`crate::ws`] discards it.
///
/// The reason to discard it is worth stating. `ServerFrame::MessageNew`'s
/// documentation says the frame reaches the receiving user's own messages,
/// echoed; **this milestone does not echo to the sender**, and the difference is
/// deliberate: the sender already holds the row, because the `message.ack` on its
/// own socket carries the stored `WireMessage`. An echo would deliver the same
/// message twice to the one client that definitely has it, and its only consumer
/// would be the client's own dedup.
///
/// That is a divergence from the frame's documentation rather than a violation of
/// it -- nothing says a server *must* echo, and `PLAN.md` §6 describes the frame's
/// use for resync and history replay, where an echo is exactly right. It is
/// recorded here because a future milestone that adds resync will have to decide
/// deliberately whether the echo resumes.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// The connection whose frame produced this delivery. Compare against the
    /// connection's own id to skip its echo.
    pub origin: ConnectionId,
    /// The frame to send to every other connection.
    pub envelope: ServerEnvelope,
}

/// The instance's fan-out point.
///
/// Cheap to clone and shared through `Arc`, so `AppState` can be cloned into
/// every axum task without a second handle to keep in step with the first.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    next_id: AtomicU64,
    frames: broadcast::Sender<Delivery>,
}

impl fmt::Debug for Hub {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Hub")
            .field("connections", &self.connection_count())
            .field("capacity", &CAPACITY)
            .finish_non_exhaustive()
    }
}

impl Hub {
    /// A hub with room for [`CAPACITY`] undelivered frames per connection.
    pub fn new() -> Self {
        let (frames, _receiver) = broadcast::channel(CAPACITY);
        Self {
            inner: Arc::new(Inner {
                // Starts at 1 so the first connection is 1 rather than 0, and 0 is
                // never a valid id -- a "connection 0" in a log is a question
                // nobody can answer.
                next_id: AtomicU64::new(1),
                frames,
            }),
        }
    }

    /// Subscribes a new connection and names it.
    ///
    /// Called **before** the WebSocket upgrade response is built, not after the
    /// socket exists. That ordering is a correctness property, not a style
    /// preference: a peer cannot send a frame before it has read the 101, so
    /// subscribing first closes the window in which a connected client is not yet
    /// listening. Without it, a `message.new` sent by a second client in that
    /// window would be lost for the first client with no error on either side --
    /// and `AGENTS.md`'s first priority is that a chat app never loses a message.
    ///
    /// There is no matching `disconnect`: the receiver returned here is dropped
    /// when the connection task ends, which releases the subscription with it.
    /// That is the property `broadcast` was chosen for, and it is why there is no
    /// reaping code here at all.
    ///
    /// # Returns
    ///
    /// The connection's id and its delivery stream.
    pub fn connect(&self) -> (ConnectionId, broadcast::Receiver<Delivery>) {
        let id = ConnectionId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        (id, self.inner.frames.subscribe())
    }

    /// Publishes `envelope` to every connection **including** `origin`.
    ///
    /// Including, not excluding: `broadcast` cannot except a receiver, and the
    /// origin's own copy is dropped one layer up. `origin` is still tagged onto
    /// the [`Delivery`] so that layer can tell whose frame it is.
    ///
    /// # Returns
    ///
    /// How many connections received it. Zero is an ordinary outcome -- a server
    /// with one client connected has nobody to tell -- and is not an error, so it
    /// is returned rather than logged from here; the caller decides what a message
    /// that reached nobody is worth saying.
    pub fn broadcast(&self, origin: ConnectionId, envelope: ServerEnvelope) -> usize {
        self.inner
            .frames
            .send(Delivery { origin, envelope })
            .unwrap_or_default()
    }

    /// How many connections are currently subscribed.
    ///
    /// The number a self-hosted operator asks first, and the one this crate's own
    /// integration tests wait on instead of sleeping: it is how a test knows every
    /// connection it opened is listening, without a timer and without reaching
    /// into the server's internals.
    pub fn connection_count(&self) -> usize {
        self.inner.frames.receiver_count()
    }
}

impl Default for Hub {
    /// Present because `Hub::new` takes no arguments, and `clippy::new_without_default`
    /// is right that a caller should be able to write `Hub::default()`.
    fn default() -> Self {
        Self::new()
    }
}
