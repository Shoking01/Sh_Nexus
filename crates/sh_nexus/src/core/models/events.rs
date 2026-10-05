//! The domain events `network/` emits and `state/` consumes.
//!
//! # The seam this enum is
//!
//! `AGENTS.md` §3.2 assigns `network/` two jobs: "parses wire formats into
//! `core::models`. Emits domain events; **never touches GPUI state directly**."
//! And §7.3 forbids blocking `cx.update_global` from a non-UI thread. Those two
//! rules are in tension -- something has to carry a message from a tokio task to
//! the main thread, and the module that produces the message is forbidden from
//! calling the function that delivers it.
//!
//! `PLAN.md` §4 resolves it with a named owner: `state/bridge.rs` is the only
//! module that calls `cx.update_global`, and `network/` emits plain values. This
//! enum is what "plain values" means. A `DomainEvent` is a fully-typed,
//! fully-validated description of something that happened, carrying no `Entity`,
//! no `Rc`, no `Window`, and no `App` -- so it can be built on a tokio worker
//! thread, sent across a channel, and consumed on the main thread without either
//! side knowing about the other's runtime.
//!
//! # Why an enum and not a trait object
//!
//! A `Box<dyn DomainEventHandler>` would be extensible without editing this file.
//! That is a real property, and it is the wrong one here, for three reasons:
//!
//! 1. **It cannot be exhaustive, so a new event cannot be handled on purpose.**
//!    `state/actions.rs` would be written against a trait, every implementor
//!    would compile unchanged when a variant is added, and the new event would
//!    be silently dropped. `AGENTS.md` §3.2's "all mutations go through
//!    `actions.rs` so they are auditable" depends on the set of things that can
//!    happen being a closed, reviewable list.
//! 2. **It is a dynamic dispatch per event on the UI thread.** `AGENTS.md` §1's
//!    first priority is responsiveness, and a vtable call per message is
//!    measurable at the 10,000-message scale §6.2's <8ms frame budget assumes.
//! 3. **There is no plugin surface.** This is not an extensible application. The
//!    protocol is one enum, the server is one server, and the set of things that
//!    can happen over a WebSocket is exactly what `PLAN.md` §6 lists.
//!
//! # The variants, and why there are exactly these
//!
//! The set is derived from `PLAN.md` §6's server frames, one variant per frame
//! plus three that no frame produces. Going from seven frames to eight events, and
//! then stopping, is the interesting part -- see "What is not here" below.
//!
//! # The three that no frame produces
//!
//! [`DomainEvent::ConnectionStateChanged`] is produced by the reconnect state
//! machine from its own state, [`DomainEvent::ResyncRequested`] by the client from
//! a cursor it already holds, and [`DomainEvent::SendUndelivered`] by
//! `network/ws.rs` from its own knowledge that a write failed. **All three are
//! facts about the local process rather than facts a peer sent**, and that is the
//! common thread: each exists because the client genuinely knows something the
//! wire cannot tell it. None of them is a server's opinion of the client.
//!
//! # What is *not* here, and why
//!
//! This is the half of the design that matters. Every variant below has a server
//! frame that can produce it, and **no** variant exists without one (bar the three
//! noted above). A variant nothing can construct is not a harmless extension
//! point: it is a shape that `state/`, the UI and the tests must all handle, a
//! case every `match` must cover, and a promise that a payload will arrive. Four
//! were considered and rejected:
//!
//! | Rejected | Why not |
//! |---|---|
//! | `ChannelUpdated`, `UserUpdated` | No frame carries them. `PLAN.md` §6 has no such frame, so a server could never produce one; the data arrives via `GET /channels` and `GET /users/:id` on demand, not pushed. |
//! | `MessageEdited` | An edit arrives as a `message.new` carrying the whole message with `edited_at` set and the original `id` and `client_msg_id`. It is a [`DomainEvent::MessageReceived`]; `state/` reconciles it. A separate variant would mean two paths for one event, and the reconciliation would have to be written twice. |
//! | `MessageDeleted` | `PLAN.md` §6 has no delete frame and §8.1's flows do not require deletion. A retraction is a protocol addition, and when it happens it arrives as a new `message.*` frame plus a new [`DomainEvent`] variant, in that order. |
//! | `ThreadReplied` | A thread reply is an ordinary `message.new` whose `thread_id` is `Some`. Modelling it separately would put thread knowledge in the transport layer, which is exactly the layering §3.2 forbids. |
//! | `Error(String)` | A free-floating error event is the one variant that would let the UI render something unactionable. Every `PLAN.md` §6 error frame has a *specific* destination: `message.error` names a send, and the bare `error` frame is about the connection, so it becomes [`ConnectionState::Rejected`] with the code and detail attached. The user can act on both; they cannot act on "something went wrong". |
//! | `UnreadChanged` | Client state, per §3.2. It is *computed* by `state/actions.rs` when it applies a [`DomainEvent::MessageReceived`] for a channel that is not currently selected, so an event for it would be a loop: `network/` cannot know whether a message was unread until `state/` has applied it. |

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::core::models::message::Message;
use crate::core::models::user::UserStatus;

/// Something that happened, in terms the application understands.
///
/// Every variant is a fully-validated domain value. There is no variant that
/// carries a wire type, a JSON string, or an unparsed id: by the time a
/// `DomainEvent` exists, `AGENTS.md` §2.1's "validated against schemas before
/// touching state" has already happened in `sh_nexus::network::mapping`.
///
/// Cloning is cheap enough to be irrelevant at event rates -- one event per
/// message, not one per keystroke -- and `Clone` is derived because the events
/// cross a channel boundary, where ownership is transferred rather than shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    /// A message arrived and belongs in the channel's history.
    ///
    /// Covers three wire situations, deliberately, because they are the same
    /// situation from the client's point of view:
    ///
    /// - `message.new` from another user.
    /// - `message.new` echoing this client's own message, which is the
    ///   reconciliation point for an optimistic send.
    /// - `message.new` carrying an edited message, recognised by an `id` and a
    ///   `client_msg_id` the client already holds.
    ///
    /// Collapsing them is what keeps ordering and dedup in one place:
    /// `core/ordering.rs` decides, once, whether a `Message` is new, a
    /// duplicate, an edit or a gap-filler. Three variants would mean three
    /// places to get that decision wrong.
    MessageReceived(Message),

    /// The server accepted a send.
    ///
    /// Carries the `client_msg_id` **and** the stored `Message`, because the
    /// server is the authority on both `id` and `timestamp`. `state/actions.rs`
    /// uses the id to find the optimistic row and replaces it wholesale, rather
    /// than patching it -- which is what makes the Optimistic Send Flow
    /// (`AGENTS.md` §8.1) a single operation with no partial state.
    MessageAcked {
        /// The `client_msg_id` of the `message.send` being acknowledged.
        client_msg_id: Uuid,
        /// The message as the server stored it.
        message: Message,
    },

    /// The server rejected a send.
    ///
    /// Terminal for that send: the message moves to `DeliveryState::Failed` in
    /// `state/app_state.rs` and stays visible for the user to retry or discard.
    /// `PLAN.md` §7: "Failures are never silently dropped."
    ///
    /// A failed optimistic send is therefore *visible*, which is the difference
    /// between a chat client and a guessing machine: the user can see that their
    /// message did not arrive.
    ///
    /// **The counterpart of [`DomainEvent::SendUndelivered`], and the two must not
    /// be merged.** This one is the server's answer: it refused, so the row is
    /// `Failed` and the user's next move is a retry. That one is a local write
    /// that failed: nothing refused anything, so the row stays `Pending` and is
    /// silently re-driven when the socket returns.
    MessageSendFailed {
        /// The `client_msg_id` of the `message.send` being rejected.
        client_msg_id: Uuid,
        /// Machine-readable reason, passed through from the server. May be a code
        /// this client does not recognise -- an unrecognised code is surfaced, not
        /// discarded, because an opaque code the user can report beats a silently
        /// dropped explanation.
        code: String,
        /// Human-readable detail, shown to the user.
        detail: String,
    },

    /// A send that never left this process, and is therefore still owed.
    ///
    /// Produced by `network/ws.rs` when a `message.send` write fails. **That is a
    /// fact about the local transport and not about the message at all** — the
    /// server never refused anything, and never saw the frame; the bytes simply
    /// did not reach the socket.
    ///
    /// **Not [`DomainEvent::MessageSendFailed`], and the difference is the whole
    /// reason this is a separate variant.** A `message.error` is terminal: the row
    /// becomes `Failed` and the user gets a badge to retry. A failed write is
    /// *transient* — the message was never refused, it just never arrived — so a
    /// `Failed` row would put a red badge on screen for a blip that is about to fix
    /// itself, and hand the user a retry that is not needed. `PLAN.md` §7's "failures
    /// are never silently dropped" is about *telling someone*, and there is nobody
    /// to tell until the reconnection either works or does not.
    ///
    /// **What the row does instead is nothing visible.** It stays `Pending`, and
    /// `state/actions.rs` puts the identity back in the outbox, so the next flush
    /// drives it once the socket is genuinely back. Re-driving is safe because the
    /// server dedupes on `client_msg_id` and answers a duplicate with the same
    /// `message.ack` — a frame that failed mid-write may or may not have been
    /// stored, and either way the retry terminates and creates no second row.
    ///
    /// **Only `message.send` produces this.** A `typing.start` or a `reaction.add`
    /// that fails to write is *not* owed: both are ephemeral, and re-sending a
    /// stale typing indicator is worse than losing it. Re-driving is correct only
    /// for the one frame kind that carries durable, user-authored content.
    SendUndelivered {
        /// The `client_msg_id` of the `message.send` whose write failed.
        ///
        /// **The optimistic row's own identity, unchanged** — the retry travels
        /// under the id the server will dedupe on, so it is recognisable as the
        /// same message rather than a second one.
        client_msg_id: Uuid,
    },

    /// Somebody reacted to a message.
    ///
    /// An *increment*, not a replacement: the frame names one user, one emoji and
    /// one message, and `state/` merges it into the message's reaction list. A
    /// replacement would mean the server recomputes and resends every reaction on
    /// every reaction, which is a round trip per emoji press.
    ///
    /// Idempotent by construction on the consuming side: applying the same
    /// `ReactionUpdated` twice inserts one entry, because the merge is a set
    /// insert. That matters because a resync after a reconnect can legitimately
    /// replay reactions the client already has.
    ReactionUpdated {
        /// The message that was reacted to.
        message_id: String,
        /// The emoji used.
        emoji: String,
        /// Who reacted.
        user_id: String,
    },

    /// Somebody started or stopped typing.
    ///
    /// Advisory in the strongest sense: the client is free to drop any of these
    /// without consequence, and the server's inactivity timeout is the backstop
    /// for a `true` with no matching `false` (`AGENTS.md` §8.1's Typing
    /// Indicator Flow requires indicators to clear, and a timeout that never
    /// fires because the network died is the case that makes a stuck indicator
    /// permanent).
    ///
    /// A *set* of typing users is client state, not an event, so this carries
    /// one user's transition and `state/` owns the set -- including bounding it,
    /// which §7.1 requires and which only the owner can do.
    TypingUpdated {
        /// The channel being typed in.
        channel_id: String,
        /// Who is typing.
        user_id: String,
        /// `true` on start, `false` on stop.
        active: bool,
    },

    /// Somebody's presence changed.
    PresenceUpdated {
        /// Whose presence changed.
        user_id: String,
        /// The new presence.
        status: UserStatus,
    },

    /// The connection's state changed.
    ///
    /// One of the three events with no frame behind it: it is produced by
    /// `network/reconnect.rs` from the state of its own state machine, and it is
    /// the one thing the UI must always be able to show, because §3.3 requires
    /// network failures to surface as a recoverable state rather than as silence.
    ConnectionStateChanged(ConnectionState),

    /// A per-channel catch-up is required, from this instant forward.
    ///
    /// Produced by `network/reconnect.rs` after a successful reconnect, once per
    /// channel the client is a member of, each carrying that channel's own
    /// `last_message_at`. It is a *request* -- `state/` and `network/` between
    /// them turn it into a `resync` frame -- because the cursor comes from local
    /// state, so it is not a thing a frame can carry.
    ///
    /// **The per-channel scope is the point, and it is enforced by the type.**
    /// There is no way to construct this event without a `channel_id`, so a
    /// global resync is inexpressible rather than merely discouraged. A single
    /// global cursor cannot reconstruct per-channel history: each channel has its
    /// own `last_message_at` and its own unread count, so one cursor is either
    /// too old -- replaying messages the user has already read -- or too new,
    /// skipping messages in a channel the user had not caught up on. Both are
    /// failures, and `AGENTS.md` §8.1's Reconnect Flow requires "no duplicates,
    /// no gaps", which is unsatisfiable without a per-channel cursor.
    /// `PLAN.md` Rev 2 had a global one; ADR-004 records it as a real defect.
    ResyncRequested {
        /// The channel to catch up.
        channel_id: String,
        /// The cursor: the client holds messages up to and including this
        /// instant and wants everything strictly after it.
        after: DateTime<Utc>,
    },
}

/// The state of the connection to the server.
///
/// Not `Copy`: [`ConnectionState::Rejected`] carries the server's explanation,
/// and a connection state that has to be cloned to be logged is a connection
/// state that will be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No socket, and none being attempted. The initial state, and the state
    /// after a [`ConnectionState::Rejected`], because reconnecting after a
    /// version rejection cannot succeed.
    Disconnected,

    /// A connection attempt is in flight. The first attempt after a
    /// disconnect; subsequent ones are [`ConnectionState::Reconnecting`], so the
    /// UI can distinguish "connecting" from "retrying" and say so.
    Connecting,

    /// Connected and the protocol version negotiated. The only state in which
    /// sending is meaningful.
    Connected,

    /// A reconnection attempt is in flight after a failure.
    Reconnecting {
        /// Which attempt this is, counting from 1.
        ///
        /// Carried so the UI can show it, and so the backoff's "max attempts"
        /// rule is observable rather than invisible. `AGENTS.md` §4.2 requires
        /// "max-attempt behavior" to be tested, and a test can only assert on
        /// something the state exposes.
        attempt: u32,
    },

    /// The peer refused this client, and retrying will not help.
    ///
    /// Set for a version rejection (`PLAN.md` §6: the server responds with an
    /// `error` frame and closes) and for a connection closed for a reason that
    /// cannot be retried. **Distinct from [`ConnectionState::Disconnected`]
    /// because the user-facing behaviour differs**: `Disconnected` invites a
    /// retry, `Rejected` must not, or the client will sit in a reconnect loop
    /// against a server that will refuse it identically every time.
    ///
    /// The code and detail come from the server's `error` frame and are shown to
    /// the user, because "the connection failed" with no reason is unactionable.
    Rejected {
        /// Machine-readable reason. May be a code this client does not
        /// recognise.
        code: String,
        /// Human-readable detail, shown to the user.
        detail: String,
    },
}
