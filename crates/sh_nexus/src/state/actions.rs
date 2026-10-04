//! Every mutation of the application state, and every decision about when to
//! make one.
//!
//! `AGENTS.md` §3.2: *"All mutations go through `actions.rs` so they are
//! auditable and testable."* This module is the whole of that: `AppState`'s
//! moving parts are `pub(crate)` primitives in `state/app_state.rs`, so a
//! decision about *whether* to apply something, *which* copy to keep, and *what
//! an unread count should become* exists in exactly one place and one `match`.
//!
//! # 1. Nothing here fails; things are refused, and refusals are reported
//!
//! There is no `Result` in this module's API and no error type to convert into
//! `ShNexusError`. **An action that cannot apply what it was asked to apply is
//! not a failure of the program, it is a policy outcome** — and the precedent is
//! `core/cache.rs`'s `InsertOutcome`, which reports a refusal as a value for the
//! same reason.
//!
//! The difference from that precedent is one line and it is load-bearing:
//! **every** function here returns a `#[must_use]` outcome, so a refusal cannot
//! be discarded by a caller who forgot to look — which is the exact failure
//! `PLAN.md` §7's "failures are never silently dropped" is about.
//!
//! # 2. The two hard cases in the optimistic send, and what was decided
//!
//! `AGENTS.md` §8.1's Optimistic Send Flow is: *"Message appears instantly
//! (pending state) → ACK updates state → Failure rolls back with visible
//! error."* Two things in that sentence are underdetermined, and both are
//! decided here with the reasoning stated.
//!
//! ## 2.1 A rollback is a *transition*, not a removal
//!
//! **Decision: a `MessageSendFailed` marks the row
//! [`DeliveryState::Failed`] and keeps it — the text, the position and all.**
//!
//! `PLAN.md` §7 settles the first half: *"A send that fails terminally
//! transitions to `DeliveryState::Failed` and stays visible for retry. Failures
//! are never silently dropped."* Removing the row is a silent drop of the user's
//! text, and that is the outcome `AGENTS.md` §1 ("Reliability … no lost
//! messages") ranks second.
//!
//! ## 2.2 …and a *later* ACK for the same send is not ignored
//!
//! **Decision: an ACK that arrives after the failure **upgrades** the row to
//! [`DeliveryState::Acked`] and installs the server's `id`.**
//!
//! The reasoning, in the order that decided it:
//!
//! 1. **The alternative is not available.** "Keep `Failed` and also insert the
//!    acked message" would hold **two rows for one `client_msg_id`**, and
//!    `core/ordering.rs` §2's whole reason for keying identity on
//!    `client_msg_id` is that a duplicate is a reshuffle. The next reconcile
//!    would collapse them, so the user would see the message appear and then
//!    vanish. One row per `client_msg_id` is not a preference here; it is the
//!    invariant the dedup rests on.
//! 2. **The server is the authority on whether a message exists.** A
//!    `message.ack` carries the *stored* message, and a stored message is
//!    evidence the send succeeded. Losing a message the server has is worse than
//!    briefly showing a failure marker on one that did not fail.
//! 3. **It is idempotent.** Replaying the ACK is a no-op: the second one finds
//!    two equal copies and `core/ordering.rs` reports `Collapsed`.
//!
//! **The cost, stated rather than swallowed.** `PLAN.md` §7 calls the failure
//! *terminal*, and it is no longer terminal once a contradicting ACK arrives. The
//! user's history briefly shows a message marked failed and then accepted, and
//! [`retry_send`] / [`discard_failed_send`] become no-ops after the upgrade.
//! **If the owner prefers terminality**, the alternative is to keep `Failed` and
//! insert the acked row anyway, at the cost of the one-row-per-identity
//! invariant — and that is a trade to decide on purpose rather than inherit from
//! a default.
//!
//! # 3. A resync that returns an optimistic message
//!
//! **Decision: the resync echo and the ACK go through the *same* function, and
//! both reconcile by `client_msg_id` — never by position and never by content.**
//!
//! `PLAN.md` §7 and `AGENTS.md` §7.4 make this the load-bearing case: a send
//! queued in the outbox while offline is re-sent on reconnect, the server
//! deduplicates on `client_msg_id`, and the resync then delivers the stored
//! message back. So the client's own optimistic row and the server's stored
//! message meet, they share a `client_msg_id`, and they differ in exactly the
//! fields `state/app_state.rs`'s module docs §4 describes — an empty `id` against
//! a real one.
//!
//! `core/ordering.rs` already knows which of the two is the later truth: its
//! `precedence` makes a real id beat an empty one, so a real id wins **by rule
//! and not by lexicographic luck**. This module hands both copies to
//! [`ordering::reconcile`] and reports what [`IngestOutcome`] said, rather than
//! re-deciding it. **The merge is also where an empty reaction group is dropped**,
//! which is the promise `sh_nexus::network::mapping` makes on `WireReaction` and
//! which no other layer is positioned to keep.
//!
//! **A live `message.new` for a message this client already holds is the same
//! case** — `core/models/events.rs` on `MessageReceived` says so explicitly, and
//! the shared path is what keeps ordering and dedup in one place.
//!
//! # 4. An ACK with no local row is adopted, not dropped
//!
//! **Decision: an ACK whose `client_msg_id` this client does not hold is
//! *adopted* — the stored message is inserted, marked
//! [`DeliveryState::Acked`], and the unread rule is applied to it.**
//!
//! The client may not hold the row because it restarted, because the user
//! discarded the failed send, or because the row is in a channel this client has
//! not loaded. **In every one of those cases the server holds a message the user
//! wrote and the client would otherwise never show it** — a lost message, which
//! is the failure this whole layer exists to prevent. The ACK carries the full
//! stored message, so adopting costs one insertion and needs nothing else.
//!
//! **And the fourth reason is the one this decision depends on not happening:
//! eviction.** `acknowledge` asks [`AppState::channel_holding`] *before* it
//! gets here, so a message that was evicted while its ACK was in flight answers
//! `None` and lands on this path — a silent reordering and a row that looks
//! duplicated, for a message the user watched vanish before the server answered.
//! That is why `AppState::evict_one_over_cap` never takes a row with a send in
//! flight: adoption is the right answer for a message the client never saw, and
//! the wrong one for a message it is still waiting on.
//!
//! # 5. A `ResyncRequested` is a question this layer answers
//!
//! `core/models/events.rs` calls `ResyncRequested` a *request*, because *"the
//! cursor comes from local state, so it is not a thing a frame can carry."*
//! [`AppState::resync_cursor`] is the answer and [`AppState::sync_expectation`]
//! hands `core/ordering.rs` the shape it takes. **`apply_event` therefore records
//! the cursor and does nothing else** — it does not fetch, and it does not
//! fabricate a watermark, because `core/ordering.rs` §4 is explicit that on the
//! protocol as specified today every resync is `Unverifiable` and that is the
//! correct answer.
//!
//! # 6. Purity
//!
//! No `gpui`, no `tokio`, no I/O, no clock, no interior mutability. Checked
//! mechanically by `state_app_state_and_actions_are_pure` and
//! `state_app_state_and_actions_hold_no_interior_mutability` in
//! `crates/sh_nexus/tests/state_actions.rs`, **which name these two files
//! explicitly** rather than the whole directory — work unit 1E-2's
//! `state/bridge.rs` will import `gpui` legitimately, and a guard that has to be
//! relaxed is a guard nobody reads.
//!
//! # 7. The outbox: an entry leaves on the server's answer, never on a write
//!
//! `PLAN.md` §7 is the specification and this is the implementation of its first,
//! second, third and fourth bullets.
//!
//! **The one idea, stated once:** a queued send is retired when the server
//! **acknowledges** it or **refuses** it terminally — *never* because a write
//! succeeded. `network/ws.rs`'s worker consumes an item off its outbound queue
//! and returns without re-queueing it when `write_frame` fails, so
//! `MAX_OUTBOUND_FRAMES` is backpressure between the enqueue and the write and
//! **not a queue that survives a disconnection**. A successful write means the
//! bytes left this process; it does not mean the server stored the row, and the
//! code proves the round trip can fail after that point. **The three doors out are
//! [`acknowledge`], [`fail_send`], and the `Acked` upgrade inside [`ingest`] — and
//! all three are the server saying it stored or refused the row.** The third was
//! not in the original design and the proptest in `tests/state_actions.rs` is what
//! found it: a resync echo carries the server's *stored* copy of a queued send, so
//! leaving the entry behind would re-drive a send the client can already see
//! stored, on every reconnect, against an entry nothing else would ever retire.
//!
//! **Re-driving is safe, which is what licenses a blunt flush.** The server
//! deduplicates on `client_msg_id` (`Store::accept_message`, `ON CONFLICT DO
//! NOTHING`) and **answers a duplicate with a `message.ack`** — see
//! `sh_nexus_server`'s `message::Outcome::into_envelope`, which maps `Duplicate`
//! to the same frame as `Accepted` and explains that a replay "has to be
//! answered or the client leaves the message pending forever". So every re-drive
//! terminates: §7's third bullet is not merely tolerated, it is what guarantees
//! the queue drains.
//!
//! ## 7.1 What is queued, and what is not
//!
//! **A send composed while [`AppState::can_send`] was false.** `begin_send`
//! already computed that flag before this queue existed and returned it in
//! `SendOutcome::Pending`; that is the entire hook, and no `ui/` change was
//! needed to use it — `InputBar::send` only rejects an empty draft, so an
//! offline send already produced a `Pending` row.
//!
//! **A connected send is deliberately not queued**, and the reason is the
//! transport rather than the outbox: `MessageList::enqueue` hands that frame to
//! the socket immediately, so queuing it too would put two `message.send` frames
//! on the wire for one message. The server drops the second as a duplicate, so
//! no transcript is harmed — but it is a doubled frame per send, for nothing.
//!
//! **The residual hole this leaves, stated rather than hidden.** A send handed to
//! a *connected* transport whose write later fails is still not re-driven, because
//! nothing puts it in the queue: `begin_send` runs before the frame is pushed, and
//! its `offline` flag was already false. **Closing that hole means the composer
//! must ask the outbox instead of the transport — a `ui/` change, and `ui/` is
//! not this layer's.** What this unit fixes is the case `PLAN.md` §7 is about: a
//! send composed *while disconnected*, which previously had no re-drive at all.
//!
//! ## 7.2 Order, and where a retry goes
//!
//! **Enqueue order, always.** §7 says the reconnect *"flushes the outbox in
//! enqueue order"*, so the queue is a `VecDeque` and [`flush_outbox`] walks it
//! front to back. **A retry goes to the BACK**: it is a new attempt at an old
//! message, and putting it at the front would let one repeatedly-retried message
//! starve everything queued behind it while reordering this client's history
//! against every other client's.
//!
//! ## 7.3 The bound, and the accepted deviation
//!
//! **At [`MAX_OUTBOX_ENTRIES`] the enqueue is refused** and the row reads
//! `Failed` with a reason naming the bound. Not the oldest dropped, not the
//! newest: §7's *"failures are never silently dropped"* forbids both.
//!
//! **§7 says "the client resyncs per channel … then flushes the outbox", and the
//! resync does not happen.** `request_resync` has no production caller — it is
//! referenced only from doctests in `network/ws.rs`, and `resyncs_sent` is a
//! counter nothing drives — so the flush runs without one. **This is safe rather
//! than convenient:** the server does not echo to the sender, and
//! `core::ordering` dedupes by `client_msg_id`, so resync-then-flush and
//! flush-then-resync converge on the same held set with no duplicate and no gap.
//! **What is therefore NOT provided is the ordering guarantee §7 asks for** —
//! where this client's own message lands among other people's. Closing it belongs
//! with the resync work.
//!
//! ## 7.4 Durability
//!
//! **This queue is in memory, and a client that exits before the connection
//! returns loses what it held.** `PLAN.md` §7's outbox is persistence and
//! persistence is `db/` in Phase 3; what lands here is the part that can be
//! correct without I/O. The two are not the same guarantee and only one of them
//! is made.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::core::markdown;
use crate::core::models::channel::Channel;
use crate::core::models::events::DomainEvent;
use crate::core::models::message::{Message, Reaction};
use crate::core::ordering::{self, DifferingField, IngestOutcome};
use crate::state::app_state::{
    AppState, DeliveryState, DifferingFields, SendFailure, TypingUsers, MAX_OUTBOX_ENTRIES,
    MAX_TYPING_CHANNELS,
};

/// The machine code a client-side outbox refusal carries.
///
/// **Namespaced to this client rather than to the server, and for the same reason
/// `MessageList::enqueue` uses `transport.enqueue_failed`:** a badge that reads
/// `server.rate_limited` when this machine's own queue was full sends the user
/// looking at the server for a condition the server never saw.
const OUTBOX_FULL_CODE: &str = "client.outbox_full";

/// What an action did, and what it did not do.
///
/// **`#[must_use]` through the type**, so a refusal cannot be dropped by a caller
/// who forgot to look — which is the exact failure `PLAN.md` §7's "failures are
/// never silently dropped" is about. Every variant is a report about a *policy
/// decision*, never a program failure; see the module docs, §1.
///
/// # Example
///
/// ```
/// use sh_nexus::state::actions::{ApplyOutcome, IgnoreReason};
/// use uuid::Uuid;
///
/// // A refusal carries its reason, so a log line can say which one.
/// assert_eq!(
///     ApplyOutcome::Ignored(IgnoreReason::AlreadyHeld {
///         client_msg_id: Uuid::nil()
///     })
///     .to_string(),
///     "ignored: the message 00000000-0000-0000-0000-000000000000 is already held",
/// );
/// assert_eq!(ApplyOutcome::Applied.to_string(), "applied");
/// ```
#[must_use = "an action's outcome carries the refusals; discarding it is how a dropped message becomes invisible"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The event changed the state in the way it was meant to.
    Applied,
    /// The event named a message this client did not hold, and the message was
    /// **adopted**: inserted, and marked acknowledged if it was one of this
    /// client's sends.
    ///
    /// A distinct outcome rather than a flavour of `Applied` because it is the
    /// one case where the client was *behind* the server — the send happened, the
    /// client had no row for it, and this is the repair. Module docs, §4.
    Adopted {
        /// The `client_msg_id` the server named.
        client_msg_id: Uuid,
    },
    /// A message this client already held was replaced by a later copy, and the
    /// fields that disagreed are named.
    ///
    /// **A report, not a resolution.** `core/ordering.rs` §3 is explicit that a
    /// disagreement is never silently merged: the two legitimate causes are the
    /// ACK handoff and an edit, and the causes it *cannot* identify look
    /// identical from here. Naming the fields is what lets the bridge log a
    /// server defect instead of swallowing it.
    Merged {
        /// Which fields the two copies disagreed about, by name.
        ///
        /// Empty for a move between channels whose only disagreement is which
        /// channel the message is in.
        fields: DifferingFields,
    },
    /// Nothing was changed, and this is why.
    Ignored(IgnoreReason),
}

/// Why an action changed nothing.
///
/// **Named rather than a `bool`**, for the same reason `core/ordering.rs`'s
/// `IngestOutcome` is an enum: a caller that has to log or surface the refusal
/// needs to say *which* refusal, and a bare `false` is the thing that made this
/// project's bugs unreproducible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreReason {
    /// The channel id was empty or only whitespace.
    ///
    /// `sh_nexus::network::mapping` refuses a blank id at the wire boundary, and
    /// the same rule is applied here because `AGENTS.md` §2.1 makes every public
    /// function validate its preconditions. A blank channel id reaching this
    /// layer means a caller built it, not a server sent it.
    BlankChannelId,
    /// The message body was empty or only whitespace.
    ///
    /// The same rule `sh_nexus::network::mapping` applies to an incoming body:
    /// *"a message with no body and no attachment is not a message."* Sending one
    /// would put a row on screen that the server is about to reject.
    EmptyContent,
    /// The message is already held, in every field.
    AlreadyHeld {
        /// The identity that was already present.
        client_msg_id: Uuid,
    },
    /// The reaction named a message this client does not hold.
    ///
    /// The increment is **dropped and reported**. A reaction cannot be buffered
    /// without knowing the message, and inventing a placeholder row for one is
    /// the "fill the gap" behaviour `core/ordering.rs` §4 rejects in stronger
    /// words.
    NoSuchMessage {
        /// The server's id for the message, as the event reported it.
        message_id: String,
    },
    /// The send is not held by this client at all.
    NotHeld {
        /// The `client_msg_id` of the send.
        client_msg_id: Uuid,
    },
    /// The send is held but is not in a state this action can act on.
    NotFailed {
        /// The `client_msg_id` of the send.
        client_msg_id: Uuid,
    },
    /// A send with this `client_msg_id` is already outstanding.
    ///
    /// Which means either the caller retried without checking, or a
    /// `client_msg_id` was reused — and a reused one would make the server's
    /// dedup (`AGENTS.md` §7.4) discard the second send as a replay of the
    /// first. **The send is refused rather than reconciled, because the honest
    /// reading is that the caller has a bug and a second row would be a lie.**
    AlreadyPending {
        /// The identity that is already outstanding.
        client_msg_id: Uuid,
    },
    /// The outbox is at [`MAX_OUTBOX_ENTRIES`] and could not take this send.
    ///
    /// **Named for the bound rather than a `bool`, because "it did not work" is
    /// not an answer a user can act on.** The reason this is an [`IgnoreReason`]
    /// at all is that the *only* caller is `retry_send`: a first send at capacity
    /// still produces its row — visible, and reading
    /// [`DeliveryState::Failed`] with a [`SendFailure`] naming the bound — so it
    /// reports through [`SendOutcome::Failed`] instead. This variant is what a
    /// *retry* reports, where there is no new row to describe and the honest
    /// answer is that nothing changed.
    OutboxFull {
        /// The identity that could not be queued.
        client_msg_id: Uuid,
    },
    /// The channel the user tried to select is not in the loaded list.
    ///
    /// **Selection is refused rather than accepted**, because accepting it would
    /// clear the unread count of a channel the user cannot see and did not read.
    UnknownChannel {
        /// The channel id that was asked for.
        channel_id: String,
    },
    /// The channel is already at
    /// [`MAX_TYPING_USERS_PER_CHANNEL`](crate::state::app_state::MAX_TYPING_USERS_PER_CHANNEL).
    ///
    /// The bound is this client's memory, not the server's: a missing indicator
    /// on a thirty-third simultaneous typist, reported, beats an unbounded set.
    TypingSetFull {
        /// The channel whose set is full.
        channel_id: String,
    },
    /// This client is already holding typing sets for
    /// [`MAX_TYPING_CHANNELS`] channels, so a new one is refused.
    TypingChannelLimit {
        /// The channel that would have needed a new entry.
        channel_id: String,
    },
    /// A typing *stop* arrived for a channel nobody is typing in.
    ///
    /// Harmless in itself — the server's inactivity timeout is the backstop, and
    /// `core/models/events.rs` is explicit that the client may drop any of these
    /// — and reported because "the client believed nobody was typing" is a fact
    /// worth having when a stuck indicator has to be explained.
    NobodyTyping {
        /// The channel the stop named.
        channel_id: String,
    },
    /// The parsed document is larger than the whole segment-cache budget, so the
    /// cache refused it.
    ///
    /// **The message is not affected** — only the caching is. `core/cache.rs` §12
    /// calls this the honest cost of the refusal rule: the document is re-parsed
    /// every time it is asked for, and no amount of cleverness inside the cache
    /// removes that.
    RenderedDocumentTooLarge,
}

impl fmt::Display for IgnoreReason {
    /// A single line, safe to put in a `tracing` record.
    ///
    /// **Only ids and reasons, never message content** — `AGENTS.md` §7.5. A
    /// `Uuid` and a channel id are the two things a developer needs to reproduce a
    /// refusal, and a `String` payload here would be one refactor away from
    /// printing a body.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankChannelId => write!(formatter, "the channel id is blank"),
            Self::EmptyContent => write!(formatter, "the message body is empty"),
            Self::AlreadyHeld { client_msg_id } => {
                write!(formatter, "the message {client_msg_id} is already held")
            }
            Self::NoSuchMessage { message_id } => {
                write!(formatter, "no held message has the server id {message_id}")
            }
            Self::NotHeld { client_msg_id } => {
                write!(formatter, "no held send has the client id {client_msg_id}")
            }
            Self::NotFailed { client_msg_id } => {
                write!(formatter, "the send {client_msg_id} did not fail")
            }
            Self::AlreadyPending { client_msg_id } => {
                write!(formatter, "the send {client_msg_id} is already outstanding")
            }
            Self::OutboxFull { client_msg_id } => {
                write!(
                    formatter,
                    "the outbox for {client_msg_id} is full at {MAX_OUTBOX_ENTRIES} queued sends"
                )
            }
            Self::UnknownChannel { channel_id } => {
                write!(formatter, "the channel {channel_id} is not loaded")
            }
            Self::TypingSetFull { channel_id } => {
                write!(formatter, "the typing set for {channel_id} is full")
            }
            Self::TypingChannelLimit { channel_id } => {
                write!(
                    formatter,
                    "the typing channel limit is reached at {channel_id}"
                )
            }
            Self::NobodyTyping { channel_id } => {
                write!(formatter, "nobody was typing in {channel_id}")
            }
            Self::RenderedDocumentTooLarge => {
                write!(
                    formatter,
                    "the parsed document exceeds the segment cache budget"
                )
            }
        }
    }
}

impl fmt::Display for ApplyOutcome {
    /// A single line naming what happened, for a `tracing` record or an
    /// assertion message.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Applied => write!(formatter, "applied"),
            Self::Adopted { client_msg_id } => {
                write!(formatter, "adopted the unheld message {client_msg_id}")
            }
            Self::Merged { fields } if fields.is_empty() => {
                write!(formatter, "merged with no differing field")
            }
            Self::Merged { fields } => {
                let names: Vec<&str> = fields.iter().map(|field| field.as_str()).collect();
                write!(formatter, "merged; fields differ: {}", names.join(", "))
            }
            Self::Ignored(reason) => write!(formatter, "ignored: {reason}"),
        }
    }
}

/// What an optimistic send produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// The message is on screen, waiting for the server.
    Pending {
        /// The identity the send will be reconciled by.
        client_msg_id: Uuid,
        /// Whether the connection cannot carry it yet.
        ///
        /// **`true` means the outbox owns it** — `PLAN.md` §7 — and the composer
        /// shows "queued" rather than "sent". The row is created either way: the
        /// Optimistic Send Flow is connection-independent, and a client that hid
        /// the message until the socket was up would fail the flow it is named
        /// for. **With the outbox in `AppState`, "owns it" is now a fact rather
        /// than a signal**, and this flag is the composer's way of asking.
        offline: bool,
    },
    /// The message is on screen and [`DeliveryState::Failed`]: the outbox was at
    /// [`MAX_OUTBOX_ENTRIES`] and could not take it.
    ///
    /// **A third variant rather than a `Pending` that quietly reads `Failed`.**
    /// The row exists, so `Ignored` would be a lie about that; and the row does
    /// *not* read `Pending`, so returning `Pending` would make
    /// [`is_pending`](Self::is_pending) answer a question the state disagrees
    /// with. `AGENTS.md` §5.2's "clear, actionable error" is what this carries:
    /// the reason names the bound, and the user's retry is the way out.
    Failed {
        /// The identity of the row that is on screen and failed.
        client_msg_id: Uuid,
        /// Why it failed — the bound, in this module's only case.
        reason: SendFailure,
    },
    /// Nothing was created, and this is why.
    Ignored(IgnoreReason),
}

impl SendOutcome {
    /// Whether the message is now on screen awaiting the server.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    /// Whether the send is on screen but cannot be transmitted yet.
    pub fn is_offline(&self) -> bool {
        matches!(self, Self::Pending { offline: true, .. })
    }

    /// Whether a row was created, whatever state it is in.
    ///
    /// **The question `MessageList::begin_send` actually asks**, which it answers
    /// from the state rather than from this enum — see that method's own note
    /// about pattern-matching a refusal taxonomy to reach a boolean. Exposed here
    /// so a caller that has the outcome does not have to spell the same `match`.
    pub fn is_on_screen(&self) -> bool {
        matches!(self, Self::Pending { .. } | Self::Failed { .. })
    }
}

impl fmt::Display for SendOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending {
                client_msg_id,
                offline: true,
            } => {
                write!(formatter, "queued offline as {client_msg_id}")
            }
            Self::Pending {
                client_msg_id,
                offline: false,
            } => write!(formatter, "sent as {client_msg_id}"),
            Self::Failed {
                client_msg_id,
                reason,
            } => write!(formatter, "not queued: {client_msg_id} failed: {reason}"),
            Self::Ignored(reason) => write!(formatter, "not sent: {reason}"),
        }
    }
}

/// One of this client's queued sends, resolved into what a frame needs.
///
/// **The outbox holds identities; this is the resolution step.** `state/` has no
/// socket and no frame type, so the thing a caller needs in order to transmit a
/// queued send is three owned values and nothing else — and reading the body from
/// the held row here, rather than storing it in the queue, is what keeps one copy
/// of every queued message.
///
/// **Owned rather than borrowed because the seam cannot lend a borrow out.**
/// `bridge::try_read` hands the state to a closure whose return type cannot
/// borrow from it (see that function's documentation), so a flush that produced
/// `Vec<(&Uuid, &str, &str)>` could not be returned from one at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedSend {
    client_msg_id: Uuid,
    channel_id: String,
    content: String,
}

impl QueuedSend {
    /// The identity the server will reconcile this send by.
    ///
    /// **The one field the server sees in the frame's envelope**, and therefore
    /// the one the dedup keys on (`AGENTS.md` §7.4).
    pub fn client_msg_id(&self) -> Uuid {
        self.client_msg_id
    }

    /// The channel this send was composed in.
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    /// The body to transmit, read from the held row at flush time.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Everything the outbox would put on the wire this time, **in enqueue order**.
///
/// `PLAN.md` §7's *"flushes the outbox in enqueue order"*, made into a function
/// a caller can assert on. **This does not transmit and does not dequeue** — it
/// resolves identities into frames and hands them back, because `state/` may not
/// reach `network/` and because the entry's whole meaning is that leaving is the
/// server's decision, not the caller's.
///
/// # Refusals rather than errors
///
/// **Nothing here can fail.** A connection that cannot carry a send yields an
/// empty list (see below), and an entry naming a row that is no longer held is
/// dropped with its identity retired. See the module docs, §1: a policy outcome
/// is a value.
///
/// # Why it produces nothing unless [`AppState::can_send`] holds
///
/// **The gate is a decision, and it belongs here rather than at the caller.** A
/// flush triggered on every drain tick would otherwise rebuild and copy the whole
/// queue once every [`DRAIN_INTERVAL`](crate::app::DRAIN_INTERVAL) on a client
/// with no server — pure CPU on the main thread to produce frames nobody sends.
/// The empty list *is* the report: nothing was driven, and the queue is untouched.
///
/// # Errors
///
/// None. See the module docs, §1.
pub fn flush_outbox(state: &AppState) -> Vec<QueuedSend> {
    if !state.can_send() {
        return Vec::new();
    }
    state
        .outbox_ids()
        .into_iter()
        .filter_map(|client_msg_id| {
            let channel_id = state.outgoing_channel(&client_msg_id)?.to_owned();
            // **A held row is a precondition, and the invariant says it holds.**
            // The outbox skips nothing eviction may take, so every entry names a
            // row that is `Pending` or `Failed` and therefore still present. The
            // `?` is here so a future path that breaks the invariant costs one
            // undriven send rather than a panic on the main thread -- and
            // `AGENTS.md` §2.1 is what asks for the value over the `expect`.
            let content = state.message(&channel_id, &client_msg_id)?.content.clone();
            Some(QueuedSend {
                client_msg_id,
                channel_id,
                content,
            })
        })
        .collect()
}

/// The single entry point for everything the network delivers.
///
/// **Exhaustive over [`DomainEvent`], and that is the point.** The enum is closed
/// by construction (`core/models/events.rs` argues for that at length), so
/// adding a variant is a compile error here until it is handled on purpose —
/// which is what `AGENTS.md` §3.2's "auditable" means in practice. A
/// `Box<dyn Handler>` would compile unchanged when a variant was added and drop
/// the event on the floor.
///
/// # Errors
///
/// None. See the module docs, §1: refusals are reported through
/// [`ApplyOutcome`], not signalled as failures.
///
/// # Example
///
/// ```
/// use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
/// use sh_nexus::state::actions::{apply_event, ApplyOutcome};
/// use sh_nexus::state::app_state::AppState;
///
/// let mut state = AppState::new("u_me");
/// assert_eq!(
///     apply_event(
///         &mut state,
///         DomainEvent::ConnectionStateChanged(ConnectionState::Connected)
///     ),
///     ApplyOutcome::Applied,
/// );
/// assert!(state.can_send());
/// ```
pub fn apply_event(state: &mut AppState, event: DomainEvent) -> ApplyOutcome {
    match event {
        DomainEvent::MessageReceived(message) => ingest(state, message),

        DomainEvent::MessageAcked {
            client_msg_id,
            message,
        } => acknowledge(state, client_msg_id, message),

        DomainEvent::MessageSendFailed {
            client_msg_id,
            code,
            detail,
        } => fail_send(state, client_msg_id, code, detail),

        DomainEvent::ReactionUpdated {
            message_id,
            emoji,
            user_id,
        } => apply_reaction(state, &message_id, emoji, user_id),

        DomainEvent::TypingUpdated {
            channel_id,
            user_id,
            active,
        } => apply_typing(state, &channel_id, &user_id, active),

        DomainEvent::PresenceUpdated { user_id, status } => {
            state.set_presence(user_id, status);
            ApplyOutcome::Applied
        }

        DomainEvent::ConnectionStateChanged(connection) => {
            state.set_connection(connection);
            ApplyOutcome::Applied
        }

        DomainEvent::ResyncRequested { channel_id, after } => {
            // A question, not an instruction: the cursor is local state, and
            // `AppState::resync_cursor` is the answer. Module docs, section 5.
            state.record_resync_cursor(&channel_id, after);
            ApplyOutcome::Applied
        }
    }
}

/// Replaces the loaded channel list, keeping every message.
///
/// **The shape of `GET /channels`**, and the reason it is a function here rather
/// than a method on `AppState`: a bulk load is a *decision* — a message for a
/// channel missing from the response is kept, not dropped — and `AGENTS.md` §3.2
/// puts decisions in this module.
///
/// # Errors
///
/// None. See the module docs, §1.
pub fn set_channels(state: &mut AppState, channels: Vec<Arc<Channel>>) {
    state.set_channels(channels);
}

/// Shows a channel, and marks it read.
///
/// **Clears the unread count to zero**, which is what `AGENTS.md` §8.1's Channel
/// Switch Flow means by *"unread badge clears"*. The rule for what counts in the
/// first place is in `state/app_state.rs`'s module docs, §5.
///
/// **A re-selection of the channel already shown still clears the count**, and
/// that is deliberate: a user who clicks the channel they are already in is
/// saying "show me what I missed", and refusing to clear it would make the badge
/// impossible to clear without switching away and back.
///
/// # Errors
///
/// None as such; a blank id or a channel that is not loaded is an
/// [`IgnoreReason`] refusal. See the module docs, §1.
///
/// # Example
///
/// ```
/// use sh_nexus::state::actions::{select_channel, ApplyOutcome, IgnoreReason};
/// use sh_nexus::state::app_state::AppState;
///
/// let mut state = AppState::new("u_me");
/// // Selecting a channel this client has not loaded is refused, not accepted:
/// // accepting it would clear a count the user never read away.
/// assert_eq!(
///     select_channel(&mut state, "c_1"),
///     ApplyOutcome::Ignored(IgnoreReason::UnknownChannel {
///         channel_id: "c_1".to_owned()
///     }),
/// );
/// ```
pub fn select_channel(state: &mut AppState, channel_id: &str) -> ApplyOutcome {
    if channel_id.trim().is_empty() {
        return ApplyOutcome::Ignored(IgnoreReason::BlankChannelId);
    }
    if !state.has_channel(channel_id) {
        return ApplyOutcome::Ignored(IgnoreReason::UnknownChannel {
            channel_id: channel_id.to_owned(),
        });
    }
    state.set_selected(Some(channel_id.to_owned()));
    state.clear_unread(channel_id);
    ApplyOutcome::Applied
}

/// Puts a failed send back in flight, without changing its identity.
///
/// **The same `client_msg_id` is reused, deliberately.** `PLAN.md` §7 says the
/// server "deduplicates on `client_msg_id`, so a flush that partially succeeded
/// before a dropped connection does not duplicate messages", so a retry that
/// minted a *new* identity would defeat exactly the mechanism that protects the
/// user from a duplicated message. The text, the channel and the optimistic
/// timestamp are all unchanged, and the row stays in place.
///
/// # Errors
///
/// None as such; a send that is not held, or is not [`DeliveryState::Failed`], is
/// a refusal. See the module docs, §1.
///
/// # Example
///
/// ```
/// use sh_nexus::state::actions::{retry_send, ApplyOutcome, IgnoreReason};
/// use sh_nexus::state::app_state::AppState;
/// use uuid::Uuid;
///
/// let mut state = AppState::new("u_me");
/// let never_sent = Uuid::from_u128(7);
/// // Retrying something this client never sent is reported, not invented.
/// assert_eq!(
///     retry_send(&mut state, never_sent),
///     ApplyOutcome::Ignored(IgnoreReason::NotHeld {
///         client_msg_id: never_sent
///     }),
/// );
/// ```
pub fn retry_send(state: &mut AppState, client_msg_id: Uuid) -> ApplyOutcome {
    let Some(channel_id) = state.outgoing_channel(&client_msg_id).map(str::to_owned) else {
        return ApplyOutcome::Ignored(IgnoreReason::NotHeld { client_msg_id });
    };
    if state.delivery(&client_msg_id) != Some(DeliveryState::Failed) {
        return ApplyOutcome::Ignored(IgnoreReason::NotFailed { client_msg_id });
    }
    // **The outbox is asked before the delivery state moves, and a refusal leaves
    // the row `Failed`.** Re-queuing is the point of the gesture, so promoting the
    // row to `Pending` first would put it on screen reading `sending…` with
    // nothing behind it — a lie a user would wait on. §7's ordering puts a retry at
    // the **back** of the queue; see `AppState::enqueue_outbox`.
    //
    // **An identity already queued is accepted here rather than refused**, because
    // `enqueue_outbox` reports one refusal for both conditions and this is the
    // case where refusing would be wrong: a send can be `Failed` while still
    // queued (a flush the transport refused keeps it), and the user asking again
    // should not be told the outbox is full.
    if state.outbox_holds(&client_msg_id) {
        state.set_delivery(client_msg_id, channel_id, DeliveryState::Pending);
        return ApplyOutcome::Applied;
    }
    if !state.enqueue_outbox(client_msg_id) {
        return ApplyOutcome::Ignored(IgnoreReason::OutboxFull { client_msg_id });
    }
    state.set_delivery(client_msg_id, channel_id, DeliveryState::Pending);
    ApplyOutcome::Applied
}

/// Removes a failed send, because the user gave up on it.
///
/// **The one path in this layer that removes a message**, and it is explicit:
/// `PLAN.md` §7's *"failures are never silently dropped"* is about not losing
/// them without telling anyone, and a user pressing "discard" is the telling.
///
/// **Nothing else decrements an unread count**, and that is what keeps the
/// count's upper bound true: only messages this client did **not** author are
/// counted (`state/app_state.rs` module docs, §5), and the only messages it
/// removes are its own. The cached segments go with the row, because `AGENTS.md`
/// §7.1 is a bound on memory and a dead parse is memory.
///
/// # There is no UI for this, and that is a decision rather than an omission
///
/// **`retry_send` has a door in `state/bridge.rs` and a badge that reaches it;
/// this has neither.** Work unit 3D settled that, and the reasoning is worth
/// keeping next to the function rather than in a task file that nothing reads:
///
/// - **It is destructive and unrecoverable.** `retry_send` moves a row between
///   two states that hold the same text; this deletes the text, and `PLAN.md`
///   §7's "failures are never silently dropped" is precisely the rule a one-click
///   delete would strain. The sibling affordance sits on a badge that says
///   `failed: <the server's words>`, so a click there is a considered act; a
///   second button beside it is not, and a mis-click there is a lost message the
///   user wrote.
/// - **It needs a gesture of its own, and there is no design for one.** A
///   destructive action wants confirmation, or an undo, or a deliberate
///   long-press — and this crate has no menu, no modal, no toast and no undo
///   stack yet (`ui/` is a message list, a rail and an input bar). Choosing among
///   those is a design decision with a real trade-off, and taking it as a side
///   effect of "make the failed badge clickable" is how the wrong one gets taken.
/// - **A door with no caller is dead code with a doc comment.** `bridge.rs` §5
///   rejects that on principle, so adding a door here would be a way to publish
///   the affordance without the design.
///
/// What *is* asserted, so this stays a decision rather than drifting into an
/// oversight: `tests/layer_boundary.rs`
/// (`the_destructive_discard_has_no_ui_caller`) fails the build if any file
/// under `src/ui/` names this function, or names a `bridge` door for it. Wiring
/// it is a work unit of its own, and it starts by removing that test.
///
/// # Errors
///
/// None as such; a send that is not held, or is not [`DeliveryState::Failed`], is
/// a refusal. See the module docs, §1.
pub fn discard_failed_send(state: &mut AppState, client_msg_id: Uuid) -> ApplyOutcome {
    let Some(channel_id) = state.outgoing_channel(&client_msg_id).map(str::to_owned) else {
        return ApplyOutcome::Ignored(IgnoreReason::NotHeld { client_msg_id });
    };
    if state.delivery(&client_msg_id) != Some(DeliveryState::Failed) {
        return ApplyOutcome::Ignored(IgnoreReason::NotFailed { client_msg_id });
    }
    state.remove_message(&channel_id, &client_msg_id);
    state.clear_outgoing(&client_msg_id);
    state.forget_rendered(&client_msg_id);
    // **Defence, and the reason it is not optional: the outbox's invariant is
    // that every entry names a held row.** Nothing reaches this with a queued
    // identity today -- a terminal `message.error` already retired it, and a
    // capacity refusal never queued one -- so this call is unreachable as written.
    // **It stays because the invariant is structural only if every path that
    // removes a row also removes its entry**, and this is the path that removes a
    // row. `AGENTS.md` §7.1 forbids a bound whose enforcement depends on every
    // caller remembering, which is the same argument
    // `AppState::insert_message` makes for applying the history cap itself rather
    // than at its call sites.
    state.dequeue_outbox(&client_msg_id);
    ApplyOutcome::Applied
}

/// Parses a message's Markdown and caches it, at a cost the caller declares.
///
/// **The parse belongs here rather than in `ui/`**, because `ui/` is presentation
/// and `AGENTS.md` §3.2 gives it no business logic, and because the cache is keyed
/// by a `client_msg_id` that only this layer holds. The declared cost is the
/// caller's because `core/cache.rs` §5 makes the caller the only party that knows
/// what a rendered message weighs.
///
/// **A cache refusal does not affect the message.** A document too large for the
/// budget is re-parsed on every render, which is `core/cache.rs` §12's stated cost
/// of the refusal rule and the honest price of a ceiling.
///
/// # Errors
///
/// None as such; a refusal is [`IgnoreReason::RenderedDocumentTooLarge`]. See the
/// module docs, §1.
///
/// # Example
///
/// ```
/// use sh_nexus::state::actions::{render_and_cache, ApplyOutcome};
/// use sh_nexus::state::app_state::AppState;
/// use uuid::Uuid;
///
/// let mut state = AppState::new("u_me");
/// let id = Uuid::from_u128(1);
/// assert_eq!(
///     render_and_cache(&mut state, id, "**bold** and `code`", 64),
///     ApplyOutcome::Applied,
/// );
/// ```
pub fn render_and_cache(
    state: &mut AppState,
    client_msg_id: Uuid,
    source: &str,
    declared_cost: u64,
) -> ApplyOutcome {
    let document = Arc::new(markdown::parse(source));
    if state.cache_rendered(client_msg_id, document, declared_cost) {
        ApplyOutcome::Applied
    } else {
        ApplyOutcome::Ignored(IgnoreReason::RenderedDocumentTooLarge)
    }
}

/// Puts this client's own message on screen before the server has seen it.
///
/// **The Optimistic Send Flow of `AGENTS.md` §8.1**, and the message it produces
/// is the one carrying the `state/app_state.rs` module-docs-§4 convention: an
/// **empty** [`Message::id`] and a present [`Message::client_msg_id`].
///
/// # Why the clock and the identity are parameters
///
/// This layer may not read a clock (`Utc::now`) and may not generate a random
/// `Uuid` (the `uuid` crate is configured without its `rng` feature, precisely
/// because no generator existed when that was decided). So *when the user pressed
/// Enter* and *which identity the send carries* are supplied by the caller that
/// owns the clock and the generator. **The cost is stated:** a caller that
/// supplies a wrong clock writes a wrong optimistic timestamp, and the ACK
/// replaces it with the server's — which is exactly why `Message::timestamp` is
/// documented as the server's *acceptance* time and the ordering key is defined on
/// it. A client-local timestamp is an estimate until then, and treating it as a
/// fact would be a second, invisible ordering.
///
/// # Errors
///
/// None as such; a blank channel id, an empty body, or a `client_msg_id` that is
/// already outstanding is a refusal. See the module docs, §1.
///
/// # Example
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
/// use sh_nexus::state::actions::{apply_event, begin_send, SendOutcome};
/// use sh_nexus::state::app_state::AppState;
/// use uuid::Uuid;
///
/// # fn at(second: i64) -> chrono::DateTime<chrono::Utc> {
/// #     Utc.timestamp_opt(1_789_000_000 + second, 0)
/// #         .single()
/// #         .expect("in range")
/// # }
/// let mut state = AppState::new("u_me");
/// apply_event(
///     &mut state,
///     DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
/// );
///
/// let id = Uuid::from_u128(1);
/// let sent = begin_send(&mut state, "c_1", "hello team", id, at(0));
/// assert_eq!(sent.is_pending(), true);
/// assert_eq!(sent.is_offline(), false, "the connection is up");
///
/// // The optimistic row has NO server id, and that is the convention.
/// let row = state.message("c_1", &id).expect("the row is on screen");
/// assert_eq!(row.id, "");
/// assert_eq!(row.content, "hello team");
/// # let _ = &mut state;
/// ```
pub fn begin_send(
    state: &mut AppState,
    channel_id: &str,
    content: &str,
    client_msg_id: Uuid,
    at: DateTime<Utc>,
) -> SendOutcome {
    if channel_id.trim().is_empty() {
        return SendOutcome::Ignored(IgnoreReason::BlankChannelId);
    }
    if content.trim().is_empty() {
        return SendOutcome::Ignored(IgnoreReason::EmptyContent);
    }
    if state.has_outgoing(&client_msg_id) {
        return SendOutcome::Ignored(IgnoreReason::AlreadyPending { client_msg_id });
    }
    // The identity is already held, in *any* channel. A `client_msg_id` is
    // globally unique, so this is either a caller that reused one -- which the
    // server's dedup would silently swallow as a replay, per `AGENTS.md` §7.4 --
    // or a message the client already has under that identity. Either way the
    // honest answer is a refusal, and the proptest in `tests/state_actions.rs`
    // found the first version of this function checking only the send map, which
    // let a send create a **second row for one identity** in a different channel.
    if state.channel_holding(&client_msg_id).is_some() {
        return SendOutcome::Ignored(IgnoreReason::AlreadyHeld { client_msg_id });
    }

    let offline = !state.can_send();
    let message = Message {
        // The convention: no server id exists yet, and inventing one is the
        // failure `state/app_state.rs` module docs section 4 is about.
        id: String::new(),
        client_msg_id,
        channel_id: channel_id.to_owned(),
        user_id: state.self_user_id().to_owned(),
        content: content.to_owned(),
        // A client-local estimate, replaced by the server's on the ACK.
        timestamp: at,
        edited_at: None,
        reactions: smallvec::SmallVec::new(),
        thread_id: None,
        attachments: smallvec::SmallVec::new(),
    };

    if state.insert_message(message).is_none() {
        // Unreachable: the identity was checked against every channel above. Kept
        // as a refusal rather than an `expect`, per `AGENTS.md` §2.1, and it costs
        // one region.
        return SendOutcome::Ignored(IgnoreReason::AlreadyHeld { client_msg_id });
    }
    state.set_delivery(client_msg_id, channel_id.to_owned(), DeliveryState::Pending);

    // **An offline send goes into the outbox here, and this is the whole hook.**
    // `offline` is `!state.can_send()`, computed before the row existed and
    // returned to the caller ever since -- `PLAN.md` §7's first bullet, waiting
    // for a queue to exist. Module docs, section 7.
    //
    // **A connected send is deliberately NOT queued**, and the reason is the
    // transport rather than the outbox: `MessageList::enqueue` hands that frame to
    // the socket immediately, so queuing it too would put two `message.send`
    // frames on the wire for one message. The server would answer the second as a
    // duplicate and drop it (`AGENTS.md` §7.4), so nothing would be duplicated in
    // any client's transcript -- but it would be a doubled frame per send, forever,
    // for no gain. **The cost of that choice is stated in the module docs,
    // section 7: a write that fails after the frame left the client is still not
    // re-driven**, because closing that needs the composer to ask the outbox
    // instead of the transport, which is a `ui/` change this layer does not own.
    if !offline {
        return SendOutcome::Pending {
            client_msg_id,
            offline: false,
        };
    }

    if state.enqueue_outbox(client_msg_id) {
        return SendOutcome::Pending {
            client_msg_id,
            offline: true,
        };
    }

    // **At the bound: the row stays visible and reads `Failed`.** Dropping the
    // oldest queued send would lose a message the user wrote and still believes
    // is waiting, and dropping this one would make their latest words vanish while
    // they watch. So the refusal is the same shape as any other terminal failure
    // -- visible, explained, retryable -- and the explanation names the bound, per
    // `AGENTS.md` §5.2's "clear, actionable error".
    let reason = SendFailure::new(
        OUTBOX_FULL_CODE,
        format!(
            "this machine already has {MAX_OUTBOX_ENTRIES} sends waiting for a \
             connection; this one was not queued. Retry once the connection returns."
        ),
    );
    state.record_failure(client_msg_id, reason.clone());
    state.set_delivery(client_msg_id, channel_id.to_owned(), DeliveryState::Failed);
    SendOutcome::Failed {
        client_msg_id,
        reason,
    }
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

/// The one place a message enters the state.
///
/// **A live `message.new`, a `message.ack` and a resync echo of a pending send
/// all arrive here**, and that is the point: `core/models/events.rs` on
/// `MessageReceived` says those three wire situations are *"the same situation
/// from the client's point of view"*, and three code paths would mean three
/// places to get the identity decision wrong.
///
/// # The identity decision, and why it is not a reimplementation
///
/// Whether this is a new message or a second copy of one this client holds is
/// decided by **`client_msg_id` and nothing else** — never by position and never
/// by content. **When it is a second copy, both are handed to
/// [`ordering::reconcile`]**, which decides which is the later truth and names
/// the fields that disagree. `core/ordering.rs` already knows that a real id beats
/// an empty one; re-deriving that here would be a second answer to a question
/// with one right answer.
///
/// # The one message clone, and where it is
///
/// A copy of the held message is taken **only on the merge path**, because
/// `ordering::reconcile` takes ownership. The overwhelmingly common arrival — a
/// message this client does not hold — clones nothing at all, and
/// `AGENTS.md` §2.3's "no deep clones in hot paths" is why `begin_send` and
/// `insert_message` take the message by value.
fn ingest(state: &mut AppState, incoming: Message) -> ApplyOutcome {
    let client_msg_id = incoming.client_msg_id;
    let outcome = ingest_by_identity(state, incoming);

    // A row of ours that now holds a **real server id** has been accepted, and
    // every copy the server sends carries one -- `sh_nexus::network::mapping`
    // refuses a blank required id, so a `DomainEvent` with an empty `id` is
    // inexpressible. Which frame delivered it therefore does not matter, and the
    // resync echo of a pending send reaches the same state as its `message.ack`.
    //
    // **A `Failed` row is upgraded too** -- module docs, §2.2. Both checks are
    // here rather than in `acknowledge` because a resync echo of a *failed* send
    // is the same late-ACK case arriving through a different door.
    if matches!(
        state.delivery(&client_msg_id),
        Some(DeliveryState::Pending) | Some(DeliveryState::Failed)
    ) {
        if let Some(held_channel) = state.channel_holding(&client_msg_id).map(str::to_owned) {
            let reconciled_with_a_server_id = state
                .message(&held_channel, &client_msg_id)
                .is_some_and(|held| !held.id.is_empty());
            if reconciled_with_a_server_id {
                // **The outbox entry retires here too, and this door was not in
                // the original design.** The proptest in
                // `tests/state_actions.rs` found the gap: a *resync echo* of a
                // queued send — a `message.new` carrying a real server id —
                // upgrades the row to `Acked` through this arm, and the entry
                // was left behind. That is not a cosmetic inconsistency. A row
                // the client can see holds the **server's stored copy**, so a
                // queue entry pointing at it is a send the client *knows* is
                // stored, and every reconnect would re-drive it, forever,
                // against an entry that nothing else will ever retire.
                //
                // **It is still the server acknowledging the send**, which is
                // what the rule is about: the evidence is the server's own id
                // for the row, not a successful write. `PLAN.md` §7's first two
                // bullets are satisfied and its last one ("the server
                // deduplicates … so a flush that partially succeeded … does not
                // duplicate messages") is what makes the replay this avoids
                // having been harmless in the first place.
                state.dequeue_outbox(&client_msg_id);
                state.set_delivery(client_msg_id, held_channel, DeliveryState::Acked);
            }
        }
    }

    outcome
}

/// The identity half of [`ingest`]: a new row, a move, or a reconciliation.
fn ingest_by_identity(state: &mut AppState, incoming: Message) -> ApplyOutcome {
    let client_msg_id = incoming.client_msg_id;
    let channel_id = incoming.channel_id.clone();

    match state.message(&channel_id, &client_msg_id).cloned() {
        // The identity is held in the channel the message names: reconcile.
        Some(held) => merge_into_held(state, &channel_id, held, incoming),

        None => {
            // The identity is held in *another* channel, which means the server
            // disagrees with this client about where the message lives. It is the
            // authority, so the row moves rather than being duplicated -- two rows
            // for one identity is the reshuffle bug `core/ordering.rs` §1 is about,
            // and a `client_msg_id` is globally unique, so this is a server defect
            // rather than a client race.
            //
            // **The lookup is over every channel, not over the send map**, because
            // a foreign message delivered twice under one identity in two channels
            // is the same defect and must not produce two rows. The cost is one
            // hash probe per channel on the arrival path, and the path already
            // pays an O(n) `Vec::insert` within the channel.
            if let Some(held_channel) = state.channel_holding(&client_msg_id).map(str::to_owned) {
                if held_channel != channel_id {
                    let counts_as_unread = should_count_unread(state, &incoming);
                    state.remove_message(&held_channel, &client_msg_id);
                    state.insert_message(incoming);
                    if counts_as_unread {
                        state.count_unread(&channel_id, client_msg_id);
                    }
                    state.retarget_outgoing(&client_msg_id, &channel_id);
                    return ApplyOutcome::Merged {
                        fields: [DifferingField::ChannelId].into_iter().collect(),
                    };
                }
            }

            // A new message. The unread rule is applied here and nowhere else.
            let counts_as_unread = should_count_unread(state, &incoming);
            if state.insert_message(incoming).is_none() {
                return ApplyOutcome::Ignored(IgnoreReason::AlreadyHeld { client_msg_id });
            }
            if counts_as_unread {
                state.count_unread(&channel_id, client_msg_id);
            }
            ApplyOutcome::Applied
        }
    }
}

/// Reconciles a message this client already holds against a later copy.
///
/// **The shared path for the ACK handoff, an edit, and the resync echo of a
/// pending send** — module docs, §§2.2 and 3.
fn merge_into_held(
    state: &mut AppState,
    held_channel: &str,
    held: Message,
    incoming: Message,
) -> ApplyOutcome {
    let client_msg_id = incoming.client_msg_id;
    let reconciled = ordering::reconcile([held, incoming]);

    let (incoming_is_later, fields) = match reconciled.outcomes().last() {
        Some(IngestOutcome::Conflicting {
            this_copy_was_retained,
            fields,
        }) => (*this_copy_was_retained, fields.clone()),
        // An exact repeat, and the arm that cannot be taken: both copies carry the
        // same `client_msg_id`, so the second can never be `Added`. Both mean
        // "keep the copy we already hold", which is what `core/ordering.rs` says
        // an exact repeat means.
        Some(IngestOutcome::Added) | Some(IngestOutcome::Collapsed) | None => {
            (false, DifferingFields::new())
        }
    };

    if !incoming_is_later {
        return ApplyOutcome::Ignored(IgnoreReason::AlreadyHeld { client_msg_id });
    }

    // `reconciled` always retains exactly one message -- the batch had one
    // identity -- and the absence arm is a value rather than a panic, per
    // `AGENTS.md` §2.1.
    let Some(mut retained) = reconciled.messages().first().cloned() else {
        return ApplyOutcome::Ignored(IgnoreReason::AlreadyHeld { client_msg_id });
    };

    // `sh_nexus::network::mapping` promises that `state/` drops a reaction group
    // whose `user_ids` is empty -- what the server sends when the last user
    // removed a reaction and this client has not seen the increment. This is the
    // only path in the application that replaces a whole `Message`, so it is the
    // only place that promise can be kept.
    retained
        .reactions
        .retain(|group| !group.user_ids.is_empty());

    // A content change invalidates the cached Markdown. Driving this from the
    // *report* rather than from "a merge happened" is what makes it correct: an
    // ACK that changes only the `id` must not throw away a parse that is still
    // good, and an edit must never be rendered from yesterday's tree.
    if fields.contains(&DifferingField::Content) {
        state.forget_rendered(&client_msg_id);
    }

    // Remove from where the row is and insert where it belongs, rather than
    // trusting the two to be the same channel -- a single-channel replacement
    // would leave a duplicate behind if the server moved it.
    //
    // **The unread state travels with the row.** `remove_message` retires the
    // count, because that is what makes the count's upper bound structural, so
    // the merge has to put it back: a message that was unread is still unread
    // after being reconciled against a later copy, and a copy that moves channel
    // keeps its unread state in the channel it moved to.
    let retained_channel = retained.channel_id.clone();
    let was_counted = state.stop_counting(&client_msg_id);
    state.remove_message(held_channel, &client_msg_id);
    state.insert_message(retained);
    if was_counted {
        state.count_unread(&retained_channel, client_msg_id);
    }
    state.retarget_outgoing(&client_msg_id, &retained_channel);

    ApplyOutcome::Merged { fields }
}

/// Applies a `message.ack`.
///
/// **Reconciling by `client_msg_id`, never by position and never by content** —
/// and the reason is worth stating, because a position-based implementation is
/// the obvious one and it is wrong: the pending row need not be the last message
/// in the channel (a resync may have appended a page behind it), two sends can
/// be in flight at once, and two clients can send the same text in the same
/// millisecond. Only the `client_msg_id` identifies the send.
///
/// A failed row is **upgraded**, not refused: module docs, §2.2.
fn acknowledge(state: &mut AppState, client_msg_id: Uuid, stored: Message) -> ApplyOutcome {
    // **One of the two doors out of the outbox, and it is a door rather than a
    // side effect further down** so that both branches below get it. The entry
    // leaves because the server holds the row, which is the only evidence that has
    // ever justified dropping a queued send -- module docs, section 7.
    //
    // **Before the reconcile, not after.** `ingest` can be refused (a replay of a
    // row the client already holds answers `AlreadyHeld`), and a refusal means the
    // server's copy is still the later truth, so retiring the entry on the same
    // evidence either way is right: the client has the row and the server has
    // acknowledged the identity.
    state.dequeue_outbox(&client_msg_id);

    // **"Held" means held *anywhere*, and the check is the verified one.** The
    // first version asked only whether the send map had an entry and whether the
    // row was in the channel the ACK named, and a foreign message already held in
    // a *different* channel then fell through to the adoption path — which
    // inserted a second row for one `client_msg_id`. The proptest in
    // `tests/state_actions.rs` found it; `AppState::channel_holding` verifies its
    // own answer, which is what makes this the same question the rest of the layer
    // asks.
    if state.channel_holding(&client_msg_id).is_some() {
        return ingest(state, stored);
    }

    // Module docs, section 4: the server holds a message this client does not,
    // and the ACK carries it in full. Adopting costs one insertion and is the
    // difference between a caught-up client and one that silently lost a message
    // the user sent.
    let channel_id = stored.channel_id.clone();
    let counts_as_unread = should_count_unread(state, &stored);
    if state.insert_message(stored).is_none() {
        return ApplyOutcome::Ignored(IgnoreReason::NotHeld { client_msg_id });
    }
    if counts_as_unread {
        state.count_unread(&channel_id, client_msg_id);
    }
    // Only this client's own sends are tracked, so a row adopted for somebody
    // else -- a server defect -- does not enter the delivery map.
    if state.self_owns(&channel_id, &client_msg_id) {
        state.set_delivery(client_msg_id, channel_id, DeliveryState::Acked);
    }
    ApplyOutcome::Adopted { client_msg_id }
}

/// Applies a `message.error`: the rollback, which is a transition.
///
/// **The row is kept** — module docs, §§2.1 and 2.2. The reason is recorded
/// beside it so the UI can show it, and the unread count does not move because
/// the message is this client's own and was therefore never counted.
fn fail_send(
    state: &mut AppState,
    client_msg_id: Uuid,
    code: String,
    detail: String,
) -> ApplyOutcome {
    let Some(channel_id) = state.outgoing_channel(&client_msg_id).map(str::to_owned) else {
        return ApplyOutcome::Ignored(IgnoreReason::NotHeld { client_msg_id });
    };
    // **The other of the two doors out of the outbox.** `PLAN.md` §7: *"A send
    // that fails terminally transitions to `DeliveryState::Failed` and stays
    // visible for retry."* Every `message.error` on this protocol is terminal --
    // there is no retryable code and no backoff hint, and `retry_send` is the
    // user's gesture for the next attempt -- so this event *is* the terminal one.
    //
    // **After the held check and only on the applied path**, so a `message.error`
    // for a send this client does not hold retires nothing: there is nothing to
    // retire, and taking the row's queue slot on the strength of a frame naming an
    // identity the client never sent would be a way to lose a queued send.
    state.dequeue_outbox(&client_msg_id);
    state.record_failure(client_msg_id, SendFailure::new(code, detail));
    state.set_delivery(client_msg_id, channel_id, DeliveryState::Failed);
    ApplyOutcome::Applied
}

/// Whether a newly-held message should increment its channel's unread count.
///
/// **The unread rule, in one place.** `state/app_state.rs` module docs §5 states
/// the rule and argues it; this is the only function that applies it, so its
/// three conditions cannot drift apart between the paths that hold a message.
fn should_count_unread(state: &AppState, incoming: &Message) -> bool {
    // (3) our own message, from any device: the user read it by writing it.
    if incoming.user_id == state.self_user_id() {
        return false;
    }
    // (2) arrived while the channel is on screen: the user was looking at it.
    if state.selected() == Some(incoming.channel_id.as_str()) {
        return false;
    }
    // (1) is handled by the caller, which only asks about a newly-held message.
    true
}

/// Merges one reaction increment into a held message.
///
/// **A set insert, so replaying the same increment changes nothing** — the
/// property `core/models/events.rs` claims for `ReactionUpdated` and which a
/// resync after a reconnect needs, because reactions the client already has come
/// back a second time.
///
/// The message is found by its **server** id, because that is all the event
/// carries. A message this client has not accepted has no server id and so
/// cannot be named by the server either: there is no pending-message case here,
/// and inventing one would be a reaction on a message the server has never seen.
fn apply_reaction(
    state: &mut AppState,
    message_id: &str,
    emoji: String,
    user_id: String,
) -> ApplyOutcome {
    let unknown = || {
        ApplyOutcome::Ignored(IgnoreReason::NoSuchMessage {
            message_id: message_id.to_owned(),
        })
    };

    let Some((channel_id, client_msg_id)) = state
        .locate(message_id)
        .map(|(channel_id, client_msg_id)| (channel_id.to_owned(), *client_msg_id))
    else {
        return unknown();
    };
    let Some(message) = state.message_mut(&channel_id, &client_msg_id) else {
        return unknown();
    };

    match message
        .reactions
        .iter_mut()
        .find(|group| group.emoji == emoji)
    {
        Some(group) => {
            if !group.user_ids.contains(&user_id) {
                group.user_ids.push(user_id);
            }
        }
        None => message.reactions.push(Reaction {
            emoji,
            user_ids: vec![user_id],
        }),
    }
    ApplyOutcome::Applied
}

/// Applies one typing transition, within both bounds `AGENTS.md` §7.1 requires.
fn apply_typing(
    state: &mut AppState,
    channel_id: &str,
    user_id: &str,
    active: bool,
) -> ApplyOutcome {
    if !active {
        let Some(typing) = state.typing_mut(channel_id) else {
            return ApplyOutcome::Ignored(IgnoreReason::NobodyTyping {
                channel_id: channel_id.to_owned(),
            });
        };
        typing.remove(user_id);
        state.drop_typing_if_empty(channel_id);
        return ApplyOutcome::Applied;
    }

    // The channel-count bound is checked before the set bound and only for a
    // *new* channel, so an existing entry can never be displaced by it.
    if state.typing_slot_is_new(channel_id) && state.typing_channel_count() >= MAX_TYPING_CHANNELS {
        return ApplyOutcome::Ignored(IgnoreReason::TypingChannelLimit {
            channel_id: channel_id.to_owned(),
        });
    }
    if state
        .typing_mut(channel_id)
        .as_deref()
        .is_some_and(|typing| typing.contains(user_id))
    {
        // Already shown. A resync replays transitions the client already applied,
        // and a full set refusing a duplicate would put a refusal in the bridge's
        // log for an event that changed nothing.
        return ApplyOutcome::Applied;
    }
    if state
        .typing_mut(channel_id)
        .as_deref()
        .is_some_and(TypingUsers::is_full)
    {
        return ApplyOutcome::Ignored(IgnoreReason::TypingSetFull {
            channel_id: channel_id.to_owned(),
        });
    }
    state.typing_slot(channel_id).add(user_id.to_owned());
    ApplyOutcome::Applied
}
