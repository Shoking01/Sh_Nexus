//! # `sh_nexus_server` -- the self-hosted instance described by ADR-010
//!
//! One working path: a `message.send` from a client is validated, persisted,
//! acknowledged to its sender, and broadcast to every other connected client.
//! Everything else `PLAN.md` describes is ADR-010's next PR and is **not**
//! written here -- not as a `todo!()`, which would compile and then panic on the
//! first user who reached it, but as frames this server decodes, logs at `warn!`,
//! and drops.
//!
//! ## What is deliberately absent, and why
//!
//! | Not built | Named as |
//! |---|---|
//! | Authentication, tokens, login, registration | ADR-010's next PR |
//! | REST endpoints, channel and user provisioning | ADR-010's next PR |
//! | Presence, typing, reactions | ADR-010's next PR |
//! | Resync, and the per-channel resume cursor's reader | ADR-010's next PR |
//!
//! One of those absences has a visible consequence worth naming here rather than
//! in a comment three files down: **`message.send` carries no author.** The frame
//! has `channel_id` and `content` and nothing else, because `PLAN.md` §6 meant
//! authorship to come from an authenticated session. So every message this
//! milestone stores is attributed to a reserved
//! [`UNATTRIBUTED_USER_ID`](db::UNATTRIBUTED_USER_ID) row. `db.rs` has the
//! reasoning, and it is not "the blank id would have been tidier": a blank
//! `user_id` makes this server emit a `message.ack` that the client's own boundary
//! rejects, so the blank version fails in a way that looks like a client bug.
//!
//! ## The modules
//!
//! | Module | Role |
//! |---|---|
//! | [`db`] | SQLite, WAL, migrations, and the idempotent accept transaction. |
//! | [`message`] | Validation, and the decision between ack, dedupe-ack and refusal. |
//! | [`ws`] | The WebSocket loop, version negotiation, and frame limits. |
//! | [`hub`] | Fan-out to every other connection. |
//! | [`time`] | The clock, and why it does not come from `chrono/clock`. |
//! | [`error`] | [`ServerError`], and why it is neither `ShNexusError` nor `WireError`. |
//!
//! ## What this crate does NOT depend on
//!
//! `sh_nexus`, the client. Not as a discipline anybody has to remember: a
//! dependency on the client would make the protocol depend on one of its two
//! implementations, which is the guarantee ADR-002 exists to provide, and the wire
//! crate's own `tests/dependency_direction.rs` asserts the same rule from the
//! other side. The two crates meet at `sh_nexus_wire` and nowhere else.
//!
//! ## An example
//!
//! The fan-out, which is the only part of the server with no I/O and therefore the
//! only part a doctest can show honestly. The database and the socket are
//! exercised in `crates/sh_nexus_server/tests/`, against a real file and a real
//! socket, because a doctest that created a SQLite file to delete it again would be
//! leaving WAL sidecars behind.
//!
//! ```
//! use sh_nexus_server::Hub;
//! use sh_nexus_wire::{ServerEnvelope, ServerFrame};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let hub = Hub::new();
//! let (alice, mut alice_frames) = hub.connect();
//! let (_bob, mut bob_frames) = hub.connect();
//! assert_eq!(hub.connection_count(), 2);
//!
//! let reached = hub.broadcast(
//!     alice,
//!     ServerEnvelope::new(ServerFrame::TypingUpdate {
//!         user_id: "u_alice".to_owned(),
//!         channel_id: "c_general".to_owned(),
//!         active: true,
//!     }),
//! );
//! assert_eq!(reached, 2, "both subscribers receive it");
//!
//! // The origin is *not* filtered here, and cannot be: `tokio::sync::broadcast`
//! // delivers to every receiver and offers no way to except one. Skipping the
//! // sender's own frame is the connection loop's job -- `ws.rs` compares
//! // `Delivery::origin` against its own `ConnectionId` -- which is also why
//! // `Delivery` carries the tag. `ws.rs`'s doc says what the skip is for; the two
//! // tests that prove it are `tests/message_round_trip.rs`.
//! assert_eq!(
//!     alice_frames.try_recv().expect("alice is subscribed").origin,
//!     alice,
//! );
//!
//! // Dropping a subscription is the only unsubscribe there is, which is why
//! // `Hub::connect` has no matching `disconnect` to forget.
//! drop(bob_frames);
//! assert_eq!(hub.connection_count(), 1);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod db;
pub mod error;
pub mod hub;
pub mod message;
pub mod time;
pub mod ws;

use axum::routing::get;
use axum::Router;

pub use crate::db::{AcceptOutcome, Store, DEFAULT_CHANNEL_ID, SCHEMA_VERSION};
pub use crate::error::{Result, ServerError};
pub use crate::hub::{ConnectionId, Hub};

/// The path the WebSocket endpoint is served on.
///
/// One route, and that is the whole HTTP surface of this milestone. It is a
/// constant rather than a string in the router so that `docs/API.md`, the test
/// harness and the log line cannot disagree about it.
pub const WS_PATH: &str = "/ws";

/// Everything a request handler needs.
///
/// Cloning is cheap and required: axum clones the state into each task it spawns,
/// and the `WebSocketUpgrade::on_upgrade` callback outlives the request that
/// created it. Both fields are themselves cheap to clone -- an `Arc` in
/// [`Store`], an `Arc` in [`Hub`] -- so the clone costs two refcounts and no deep
/// copy (`AGENTS.md` §2.3).
#[derive(Clone)]
pub struct AppState {
    /// Persistence, and the only component that knows what a channel is.
    pub store: Store,
    /// Fan-out to every other connection.
    pub hub: Hub,
}

impl std::fmt::Debug for AppState {
    /// Both halves have their own `Debug` impls, and this one adds nothing: it
    /// exists so a `warn!` with `{state:?}` is possible without either half having
    /// to be careful about what it prints.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("store", &self.store)
            .field("hub", &self.hub)
            .finish()
    }
}

impl AppState {
    /// Builds the state the router and the connection tasks share.
    pub fn new(store: Store, hub: Hub) -> Self {
        Self { store, hub }
    }
}

/// The whole route table.
///
/// # Returns
///
/// A router with exactly one route, [`WS_PATH`], upgrading to a WebSocket.
///
/// Public and used by the integration tests, which bind a real listener against
/// exactly this router: a test that assembled its own would be testing a
/// different server. There is deliberately no fallback route, no health endpoint
/// and no method-not-allowed handler -- axum answers those, and hand-written ones
/// would be untested code answering requests nobody sends during this milestone.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(WS_PATH, get(ws::handler))
        .with_state(state)
}
