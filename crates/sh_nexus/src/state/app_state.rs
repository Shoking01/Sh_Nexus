//! The application's state, and the data it is made of.
//!
//! This is `AGENTS.md` §3.1's `state/app_state.rs` — *"Channels, messages,
//! presence, current selection"* — plus the two things `core/models/mod.rs` and
//! `core/models/message.rs` both promised would be found **here** and nowhere
//! else: [`DeliveryState`] and the unread counts.
//!
//! # 1. This file is the data; `state/actions.rs` is the policy
//!
//! `AGENTS.md` §3.2 requires that *"all mutations go through `actions.rs` so they
//! are auditable and testable"*, and this is how that is made structural rather
//! than aspirational:
//!
//! | | `state/app_state.rs` (here) | `state/actions.rs` |
//! |---|---|---|
//! | Holds | the data, and `pub(crate)` **primitives** that move it | every **decision** about when and how |
//! | Public surface | read accessors only | the free functions the UI and `state/bridge.rs` call |
//! | Imports | `core/` and `std` | `core/` and this module |
//!
//! **A mutation primitive is `pub(crate)`, so it is unreachable from outside the
//! crate** — the compiler is the guard, not a review. The crate's own test suite
//! can see `pub` items and therefore cannot see a primitive; that is the
//! property, not a gap in it.
//! `state_exposes_no_public_mutation_surface` in
//! `crates/sh_nexus/tests/state_actions.rs` scans this file and fails if a
//! `pub fn` grows a `&mut self` **other than [`rendered`](AppState::rendered)**,
//! whose one exception is named in the test with its reason. The split cannot rot
//! silently.
//!
//! # 2. Purity, and why this file has no `gpui` in it
//!
//! **No `gpui`, no `tokio`, no `std::fs`/`std::io`/`std::net`, no clock, no
//! environment, no interior mutability.** Every method that changes this type
//! takes `&mut self`; every method that only reads takes `&self`.
//!
//! The rule is not a comment — `state_app_state_and_actions_are_pure` and
//! `state_app_state_and_actions_hold_no_interior_mutability` in
//! `crates/sh_nexus/tests/state_actions.rs` scan these **two files by name** and
//! fail the build otherwise.
//!
//! **The scan names two files rather than the whole directory on purpose.**
//! Work unit 1E-2 adds `state/bridge.rs`, and `PLAN.md` §4 makes that file the
//! single owner of `cx.update_global` — so it will import `gpui` legitimately.
//! A directory-wide scan would have to be relaxed one work unit later, and a
//! boundary that is relaxed is a boundary nobody reads.
//!
//! # 3. The main-thread invariant, stated and guarded
//!
//! > **INVARIANT (single-thread confinement).** An [`AppState`] is read and
//! > mutated on exactly one thread — the thread that owns the GPUI application
//! > context — and no other thread ever holds a reference to it.
//!
//! **The invariant is real, and nothing in the type system enforces it.**
//! [`AppState`] is `Send + Sync`, so a whole copy can be moved to a worker
//! thread, shared behind a lock, or handed to a `tokio` task without a single
//! compiler objection. That *is* the hazard this layer has, and it is why
//! `the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee`
//! in the test suite **demonstrates the move instead of denying it**.
//!
//! **The basis of the invariant is ownership, not a type.** `PLAN.md` §4 makes
//! `state/bridge.rs` the only module permitted to call `cx.update_global` or
//! `cx.update`, and `AGENTS.md` §7.3 requires both to happen on the main
//! thread. `network/` emits plain [`crate::core::models::events::DomainEvent`] values
//! (`crate::core::models::events::DomainEvent`) and never imports `gpui`. So
//! every path from a socket to this state goes through the main thread by
//! construction of the layers above it.
//!
//! **One pillar of that argument is stronger than a convention, and it is worth
//! stating precisely because it was not stated before.** GPUI's `Context<'a, T>`
//! holds `&'a mut App`, and `App` holds `Rc<dyn Platform>` and
//! `Rc<ActionRegistry>`, so `Context` is `!Send`. A worker thread therefore
//! **cannot** call `cx.update_global` or `cx.update` at all — `AGENTS.md` §7.3's
//! prohibition is compiler-enforced on the GPUI path, not merely documented.
//! What that does **not** do is stop this *value* from travelling: a `db/`,
//! `network/` or `platform/` module holding its own `AppState` would compile and
//! run. The gap between "the context cannot cross a thread" and "the state
//! cannot cross a thread" is the invariant above, and it is a rule — so it is
//! written down and guarded rather than assumed.
//!
//! # 4. The empty `id` before the ACK
//!
//! > **CONVENTION.** A [`Message`] this client holds whose [`id`](Message::id)
//! > is **empty** is one the server has not accepted. Its
//! > [`client_msg_id`](Message::client_msg_id) is present from the instant the
//! > user pressed Enter and is its only identity.
//!
//! **Why the id is empty rather than invented.** A server id means *"the server
//! has accepted this and stored it under this name"*, and nothing has happened
//! yet. A local placeholder — `"local-1"`, a UUID, a counter — would be
//! indistinguishable from a real one at every call site, and the first thing
//! that breaks is not a bug report but a *missing message*: the client would
//! show two rows for one send, or address a message the server has never heard
//! of. `core/ordering.rs` §2 names this exact failure as reason enough to key
//! identity on `client_msg_id`.
//!
//! **Why `client_msg_id` is the one that can dedupe.** It is minted by this
//! client before the send exists on the wire, it survives the optimistic row,
//! the ACK, a re-send after a reconnect and a resync unchanged (`AGENTS.md`
//! §7.4), and the server can therefore recognise a replay. A server id cannot
//! do any of that, because it does not exist until the last step. It is also why
//! the rendered-segment cache is keyed by it: a server id could not hold a
//! pending message's segments, and the pending message is the one the user is
//! looking at while they wait.
//!
//! **Two consequences a reader should not have to derive.**
//!
//! - **Nothing may treat the empty id as an id.** It is not unique — two
//!   unacknowledged sends share it — and it is not addressable. It is a
//!   *state*, and [`AppState::pending_sends`] is how a caller asks about it.
//! - **An acknowledged message always has one.** The test
//!   `an_unacknowledged_message_holds_no_server_id_and_an_acknowledged_one_always_does`
//!   is that sentence as an assertion.
//!
//! # 5. The unread rule
//!
//! §4.2 requires unread counts and does not say what counts as unread, so the
//! rule is stated here and pinned by the test suite. **It is a product decision
//! and not a detail**, which is why the reasoning is longer than the rule.
//!
//! > **RULE.** A message increments its channel's unread count **iff**
//! >
//! > 1. it is **newly held** — not a duplicate, not a merge of one already held;
//! > 2. its channel is **not the selected channel** at the moment it is applied;
//! > 3. its author is **not this session's user** — `self_user_id`.
//!
//! **And it is cleared to zero by [`crate::state::actions::select_channel`]**,
//! which is what `AGENTS.md` §8.1's Channel Switch Flow means by *"unread badge
//! clears"*.
//!
//! **Why (2) is evaluated at arrival and not "was it ever read".** The counter
//! answers a question about *now* — "is there anything I have not looked at?" —
//! and a message that arrived while the channel was on screen was looked at. The
//! alternative (mark everything before the last selection as unread when the user
//! leaves) is a different product, it needs a read watermark rather than a
//! counter, and it produces the badge nobody wants: a notification for a
//! conversation the user just finished reading.
//!
//! **Why (3) excludes this user's own messages, from any device.** A badge
//! counting your own message is a lie about your attention: you read it, because
//! you wrote it. It is also the behaviour of both products this is modelled on,
//! and it is what makes the counter *small*: a user who talks in a channel they
//! are not reading does not accumulate a badge of their own words. **Excluding
//! by `user_id` and not by `client_msg_id` is deliberate** — a message of yours
//! sent from your phone while your desktop is open is still a message you have
//! read, and matching on the local id would count it.
//!
//! **Why the count is derived from a *set* of counted identities and not stored
//! as a counter.** Both are implementable, and the proptest in
//! `tests/state_actions.rs` chose between them: the first version of this layer
//! kept a per-channel `u32`, and the model property found the hole immediately —
//! a message the server moves from one channel to another left the old channel's
//! count behind, so the count exceeded the messages actually held there. A
//! counter's bound is maintained by every path that can change a count, and the
//! path that forgets is invisible. **A set of `(channel, client_msg_id)` pairs
//! cannot drift**: every element names a held message, and removing a message
//! removes its element, so `unread <= messages held` is a property of the
//! representation rather than of anybody's care.
//!
//! **What the set costs, stated rather than assumed.** Reading a count is O(k) in
//! the number of *unread* messages across the workspace — not the number of
//! messages held, which is the n `AGENTS.md` §2.3's frame-loop rule is about.
//! A client that is behind in a hundred channels has k in the low hundreds, and
//! [`unread_counts`](AppState::unread_counts) is a single pass for all of them,
//! which is what a sidebar should call. The write is O(log k) and happens once
//! per arriving message.
//!
//! **And the property test over it is the one the task asked for by name**, so
//! it is worth saying what it can and cannot show: it replays arbitrary
//! operation sequences and checks after *every* step that the count is neither
//! negative nor greater than the channel's held messages. That is a *measured*
//! bound, and the set makes it structural as well.
//!
//! **What is deliberately *not* in the rule: window focus.** Whether a message
//! that arrives in the *selected* channel, while the window is in the
//! background, should be unread is a genuine product fork — the two products
//! this is modelled on disagree — and it is **left to the project owner rather
//! than decided here**, because answering it needs a fact this layer may not
//! have: focus is a GPUI value, and §2 forbids importing `gpui`. The rule above
//! is the channel-scoped default, it is the conservative one (it never badges
//! something the user was looking at), and §5's test suite pins it as the
//! default so that changing it is a visible decision.
//!
//! # 6. What is *not* here, and why
//!
//! - **No `anyhow`, no `Result`, no error type.** An action that cannot apply
//!   what it was asked to apply is not a failure of the program, it is a **policy
//!   outcome** — and `core/cache.rs`'s `InsertOutcome` is the precedent for
//!   saying so with a type instead of an error. `state/actions.rs` returns
//!   `ApplyOutcome` for exactly this reason, and a refusal is *reported* rather
//!   than dropped, so `AGENTS.md` §3.2's "auditable" has something to audit.
//! - **No clock, and no id generator.** The optimistic send needs *when* the
//!   user pressed Enter and *which* UUID to use, and this layer is not allowed
//!   to read a clock (`Utc::now`) or to generate a random UUID (the `uuid`
//!   crate is configured without its `rng` feature precisely because no
//!   generator existed yet). Both are therefore **parameters** of
//!   `state::actions::begin_send`, supplied by the caller that owns the clock
//!   and the generator. **The cost is stated:** a caller that supplies a wrong
//!   clock writes a wrong optimistic timestamp, and it is corrected by the ACK —
//!   which is exactly why `Message::timestamp` is documented as the server's
//!   acceptance time and not the time the user pressed Enter.
//! - **No `network/`, no `db/`, no `platform/`.** `AGENTS.md` §3.2 puts this
//!   layer above `network/` and `core/`; `errors.rs` is a sibling, and §3.3's
//!   direction of travel is module error → `ShNexusError`, never the reverse.
//! - **No *durable* outbox.** There is a bounded in-memory one —
//!   [`MAX_OUTBOX_ENTRIES`] and [`AppState::outbox_len`] — because
//!   `PLAN.md` §7's guarantee is about a *disconnection*, and a queue that
//!   forgets everything when the process exits does not survive one. **What is
//!   still `db/`'s in Phase 3 is the persistence:** an outbox that outlives a
//!   restart needs a schema, a migration and a reconciliation against what the
//!   server already holds, none of which this layer may have (no I/O, no
//!   filesystem). **The honest statement of the limit is that a queued send is
//!   lost if the client exits before the connection returns** — which is a
//!   smaller hole than the one this queue closed and a different one, and
//!   calling it a solved offline model would not be true.
//! - **No backfill for what [`MAX_MESSAGES_PER_CHANNEL`] evicts, so eviction
//!   loses it.** That is the one cost of the bound, and it is stated here rather
//!   than left for somebody to discover: a channel past 10 000 rows has no
//!   record that the older ones existed, and nothing in this layer can fetch them
//!   back. `PLAN.md`'s *"load history on startup, paged with cursors"* is Phase 3
//!   and `db/`'s, and until it lands there is no "scroll up to load more" either
//!   — so a reader who scrolls past the head of the window is at the head of the
//!   window, full stop. **The alternative was not an unbounded `Vec`:** §7.1
//!   names message history first, and the budget row this constant comes from
//!   (`§6.2`, *"RAM with 10k cached messages < 200 MB"*) is only a budget if a
//!   client holds 10 000 and not 10 001. What §1 ranks second is *"no lost
//!   messages"*, and the honest reading of the two together is that a message
//!   older than the window is history the user has not asked for in this session
//!   — not a message in flight, which is why eviction never takes one.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::core::cache::LruCache;
use crate::core::markdown::Document;
use crate::core::models::channel::Channel;
use crate::core::models::events::ConnectionState;
use crate::core::models::message::Message;
use crate::core::models::user::UserStatus;
use crate::core::ordering::{self, DifferingField, SyncExpectation};

/// Which fields two copies of one message disagreed about.
///
/// The same inline buffer `core/ordering.rs` uses for its own report, spelled
/// out here because that module's alias is private. **A report and not a merge**
/// — see `core/ordering.rs` §3 for why a disagreement is never resolved by
/// silently picking one.
pub type DifferingFields = smallvec::SmallVec<[DifferingField; 4]>;

/// How many users one channel will remember as typing at once.
///
/// `AGENTS.md` §7.1 forbids unbounded growth of in-memory state, and
/// `core/models/events.rs` says of `TypingUpdated` that the set *"is client
/// state, not an event, so this carries one user's transition and `state/` owns
/// the set -- including bounding it, which §7.1 requires and which only the owner
/// can do."* This is the owner doing it.
///
/// **Thirty-two is a guess with a stated basis, not a measured number.** The
/// largest real channel has tens of members, so this is never the binding
/// constraint in practice; the point of the number is that *there is one*, and
/// that a client which never sees
/// `state::actions::IgnoreReason::TypingSetFull` is a client whose bound was
/// never tested. What is bounded is *this client's belief*, not the server's:
/// the server still reports every transition, and `TypingUpdated` is advisory by
/// `events.rs`'s own contract, so refusing the thirty-third is a missing
/// indicator and not a wrong one.
pub const MAX_TYPING_USERS_PER_CHANNEL: usize = 32;

/// How many channels may hold a non-empty typing set at once.
///
/// The companion to [`MAX_TYPING_USERS_PER_CHANNEL`], and it exists for the same
/// reason in the other direction: bounding each channel is not enough if the
/// number of channels is not bounded, and a user in a large workspace can
/// accumulate one stale entry per channel. **An existing entry is never
/// discarded by this rule** — only a *new* channel's first typist is refused —
/// so the bound costs a missing indicator on the sixty-fifth busy channel and
/// never a vanished one on a channel the user is already watching.
pub const MAX_TYPING_CHANNELS: usize = 64;

/// How many of this client's own sends the outbox will hold at once.
///
/// `AGENTS.md` §7.1 forbids unbounded growth of in-memory state and names
/// *"message history"* first; the outbox is the second queue of client-originated
/// messages, and this is its bound.
///
/// # 1024, and why that number
///
/// **The same figure as [`MAX_PENDING_EVENTS`](crate::state::bridge::MAX_PENDING_EVENTS),
/// which is precedent rather than invention.** That constant bounds the events
/// queued *for* the main thread; this one bounds the sends queued *by* the user.
/// Both are "client-originated items waiting for the network", both are drained
/// by the same reconnect, and a client with a thousand sends outstanding has a
/// rendering problem long before it has a memory one. **A queue whose bound
/// differs per queue is a bound somebody has to remember twice**, so the two
/// agree and a future change to one should change the other.
///
/// # At capacity the enqueue is refused, and that is the whole policy
///
/// **Not the oldest entry dropped, and not the newest.** Both are the failure
/// `PLAN.md` §7's *"failures are never silently dropped"* is written against:
/// dropping the oldest discards a message the user wrote and still believes is
/// queued, and dropping the newest makes their most recent words vanish at the
/// moment they are watching them disappear. **So the refusal is reported and the
/// row reads [`DeliveryState::Failed`] with a reason naming this bound** — the
/// same shape as any other terminal failure, with the user's retry as the way out.
///
/// **The cost, stated rather than assumed:** a client that queues 1024 sends
/// while offline sees its 1025th refused. That is a user typing into a client
/// with no connection for a long time, and the alternative is either a dropped
/// message or a bound with no number — neither of which `AGENTS.md` §7.1 permits.
/// [`MAX_MESSAGES_PER_CHANNEL`] already accepts the same trade for history, and
/// says why the over-cap case is preferable to a lost message.
pub const MAX_OUTBOX_ENTRIES: usize = 1_024;

/// How many messages one channel holds before its oldest row is evicted.
///
/// `AGENTS.md` §7.1 forbids *"unbounded growth of in-memory state"* and names
/// *"message history"* first. The two constants above are the precedent for
/// what this is: a bound with a stated basis, in the layer that owns the state
/// it bounds, beside the others rather than in a policy file.
///
/// **Ten thousand, and the number is the project's own.** `AGENTS.md` §6.2
/// budgets *"RAM with 10k cached messages < 200 MB"*, and `docs/BASELINES.md`
/// records the measurement that row is written about: 10 000 messages cost
/// **12,8 MB**, 6,4% of that budget. So §6.2's row *already assumes* a client
/// holds 10 000 messages, and this constant is what makes that assumption true
/// rather than aspirational.
///
/// **Per channel, not global.** §6.2's row is about cached history,
/// `MessageList` is per channel, and a global ceiling would let one busy channel
/// starve every other one — which is a worse failure than the unboundedness
/// this exists to close.
///
/// # What is evicted, and through which door
///
/// The **oldest** row, by the order `ChannelMessages` already maintains
/// (`core::ordering::compare`, ascending, so the head is the oldest), and
/// through `remove_message` rather than past it.
/// That is not a style preference: `remove_message` is the only thing that
/// retires `message_index` and the unread set's element, so an eviction that
/// reached into `ChannelMessages` directly would leave a counted message with
/// no row behind it — and the unread set's own property 1, *"it cannot exceed
/// the channel's held messages"*, is a property of *that* path rather than of
/// anybody's care.
///
/// # The two rows eviction may never take
///
/// **1. A send this client is still waiting on** — anything in `outgoing` as
/// [`DeliveryState::Pending`] or [`DeliveryState::Failed`], or in `failures`.
/// `state/actions.rs`'s `acknowledge` asks `channel_holding(..)` before
/// anything else, so an evicted send in flight answers `None`, falls through to
/// the adoption path, and is re-inserted as if this client had never seen it:
/// a silent reordering, a row that looks duplicated, and a message the user
/// watches disappear before the server answered it. A bound that costs the
/// user a message they can see is a worse bug than an unbounded `Vec`.
///
/// **2. The row this insert just added**, and everything at or after it.
/// Evicting the row that has just arrived is not merely unfair, it is
/// *incorrect*: the caller has not yet run the unread rule for it, so the
/// unread set would gain an element naming a message that is no longer held —
/// the one invariant this whole design is built to make structural. So
/// candidates are the rows **strictly older** than the arrival, which is
/// "evict the oldest" with a floor, and which is also why the scan is bounded
/// by the arrival's index rather than by the channel's length.
///
/// # The one case where the bound does not hold
///
/// **When every held row is a send in flight, eviction takes nothing and the
/// channel stays over the cap.** That is the deliberate choice: dropping a send
/// to satisfy a memory bound is the failure §7.1's *"failures are never
/// silently dropped"* is written to prevent, and an over-cap channel is a
/// bounded, self-correcting condition rather than a lost message.
///
/// **The over-cap is bounded by the number of unacknowledged sends, and that
/// bound is stated rather than assumed.** The task document for this unit
/// attributed it to [`MAX_PENDING_EVENTS`](crate::state::bridge::MAX_PENDING_EVENTS)
/// (1024), and **that is wrong**: 1024 bounds the events queued for the main
/// thread, while a `Pending` row is created by `begin_send` — a user gesture,
/// not an inbox delivery — and nothing in this layer bounds how many a user may
/// send before the server answers. The honest bound is therefore "however many
/// sends this client is waiting on", which is unbounded here and is a separate
/// §7.1 finding, not something this constant can fix. The condition resolves
/// itself as sends are acknowledged: each later arrival evicts one row per
/// arrival until the channel is back inside the cap.
///
/// `a_channel_whose_every_row_is_a_send_in_flight_is_allowed_over_the_cap` in
/// `crates/sh_nexus/tests/state_actions.rs` is that sentence as an assertion.
pub const MAX_MESSAGES_PER_CHANNEL: usize = 10_000;

/// Entries in the rendered-segment cache before it evicts by recency.
///
/// `AGENTS.md` §7.3 requires a virtualized message list — "render only visible
/// items" — so the working set is a screenful, not a history. A few hundred
/// covers a very tall screen plus a scroll's worth of overshoot, and
/// `core/cache.rs` §2's arithmetic puts a recency scan at that size on the order
/// of a microsecond against §6.2's 8ms scroll-frame budget.
pub const DEFAULT_SEGMENT_CACHE_CAPACITY: usize = 512;

/// Declared-cost ceiling for the rendered-segment cache, in bytes.
///
/// `core/cache.rs` §5 is explicit that a cache cannot measure memory and only
/// keeps the exact sum of what the caller declared, so this is a ceiling on a
/// **declared** figure: 8MiB of *rendered markdown*, which is not the same as
/// 8MiB of process memory and is not claimed to be. `AGENTS.md` §6.2's
/// 200MB-with-10k-messages row is an RSS figure no in-process accounting can
/// prove, and the honest response to that is to say so rather than to pretend a
/// number proves it.
pub const DEFAULT_SEGMENT_CACHE_BUDGET: u64 = 8 * 1024 * 1024;

/// How far this client has got with one of its own sends.
///
/// **Client state, not domain state** — `PLAN.md` §5 says so in as many words
/// and `core/models/message.rs` repeats it at length: a `Message` means "a
/// message that exists", and whether *this* client has finished sending it is a
/// fact about this client's connection. Two clients holding the same message
/// will legitimately disagree, and putting the flag on the message would make
/// one domain type mean two different things depending on who is looking at it.
///
/// **Not `Default`, and deliberately.** The first state a send is in is
/// [`Pending`](Self::Pending); a `Default` of `Acked` would make a forgotten
/// insertion look like a completed send, and a `Default` of `Failed` would make
/// it look like a bug the user has to dismiss. There is no honest neutral value,
/// so there is no `Default`.
///
/// # Example
///
/// ```
/// use sh_nexus::state::app_state::DeliveryState;
///
/// assert_ne!(DeliveryState::Pending, DeliveryState::Acked);
/// assert_ne!(DeliveryState::Pending, DeliveryState::Failed);
/// assert_eq!(DeliveryState::Acked, DeliveryState::Acked);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryState {
    /// Sent or queued, not yet acknowledged.
    ///
    /// **The state in which [`Message::id`] is empty** (module docs, §4). A
    /// pending row is the Optimistic Send Flow of `AGENTS.md` §8.1: the message
    /// is on screen before the server has seen it. **It stays on screen in this
    /// state across a disconnection**, and that is now backed by the outbox
    /// ([`MAX_OUTBOX_ENTRIES`]) rather than by optimism: a send composed while
    /// [`AppState::can_send`] was false is queued there and re-driven on
    /// reconnect. **The outbox's durability limit is stated in the module docs,
    /// §6** — a queue that does not survive a process exit.
    Pending,
    /// The server accepted it, and the client holds the stored message.
    ///
    /// Reached by `state::actions::apply_event`'s `MessageAcked` arm, and by the
    /// resync echo of a pending send. **A row in this state always holds a
    /// non-empty [`id`](Message::id)** — the invariant the test suite pins.
    Acked,
    /// The server rejected it, terminally.
    ///
    /// **The row is kept, not removed** — `PLAN.md` §7: *"A send that fails
    /// terminally transitions to `DeliveryState::Failed` and stays visible for
    /// retry. Failures are never silently dropped."* What a *later*
    /// `MessageAcked` for the same send does is decided in
    /// `state/actions.rs`'s module docs, and it is not "ignore it".
    Failed,
}

/// The reason the server gave for a [`DeliveryState::Failed`] send.
///
/// Held beside the message rather than inside it, for the same reason
/// `DeliveryState` is: a `Message` means "a message that exists", and why *this
/// client's* send of it was refused is a fact about this client's connection.
///
/// **The server's own words, and they are safe to show.** `AGENTS.md` §7.5
/// forbids logging *message content*; a machine code and a human explanation are
/// what `core/models/events.rs` says the user is shown, and an unrecognised code
/// is passed through rather than discarded precisely because an opaque code the
/// user can report beats a silently dropped explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendFailure {
    code: String,
    detail: String,
}

impl SendFailure {
    /// Records why a send was refused.
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }

    /// The machine-readable reason, as the server reported it.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The human-readable explanation, as the server reported it.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for SendFailure {
    /// One line: the machine code, and the explanation when there is one.
    ///
    /// **The code leads, and the detail is omitted rather than left dangling when
    /// it is empty.** A `message.error` may carry a code and no prose, and
    /// `"validation_error: "` is a worse line than `"validation_error"` — it
    /// reads as a sentence that lost its ending. Both halves are the *server's*
    /// words (or, for a client-side refusal such as the outbox being full, words
    /// this client chose), and `AGENTS.md` §7.5's ban is on **message content**:
    /// a code and an explanation are exactly what it says the user is shown.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            formatter.write_str(&self.code)
        } else {
            write!(formatter, "{}: {}", self.code, self.detail)
        }
    }
}

/// One channel's held messages and the index that makes them findable.
///
/// **The `Vec` is the data; the map is derived.** `messages` is the only place a
/// [`Message`] lives, and the index is rebuilt from it — which is what makes
/// "one row per `client_msg_id`" a structural property rather than a thing to
/// remember.
///
/// # The invariant
///
/// ```text
/// messages is ascending by core::ordering::compare
/// by_client_msg_id maps each message's client_msg_id to its index in `messages`
/// ```
///
/// **One row per `client_msg_id`, always.** `core/ordering.rs` §2 keys identity
/// on `client_msg_id` precisely because a message has no server id until the
/// server accepts it; two rows for one `client_msg_id` would be collapsed by the
/// next reconcile, so the duplicate would appear for one frame and then vanish,
/// which is the reshuffle bug §1 is about. The invariant is what makes
/// *"reconcile a resynced send against the optimistic row"* a lookup rather than
/// a search.
///
/// **The cost of the index, stated rather than assumed.** [`insert_at_order`] and
/// [`remove_at`] fix up positions from the insertion point onward, which is O(n)
/// in the channel's held messages — **the same order as the `Vec::insert` /
/// `Vec::remove` they accompany**, so the index does not change the complexity
/// class of the operation, only its constant. The alternative (no positional
/// index, a linear search per lookup) is also O(n) and pays it on *every*
/// arrival, including the overwhelmingly common non-duplicate one. For a bounded
/// window — `AGENTS.md` §7.3 virtualizes the list and Phase 3 pages the history
/// — the constant is a few hundred pointer-sized writes.
#[derive(Debug, Default)]
struct ChannelMessages {
    messages: Vec<Message>,
    by_client_msg_id: HashMap<Uuid, usize>,
}

impl ChannelMessages {
    /// Holds one more message, at the position [`ordering::compare`] gives it.
    ///
    /// The position is found with `partition_point` over the *held* messages, so
    /// the comparison this module uses is `core/ordering.rs`'s and not a second
    /// one: an implementation that sorted by timestamp alone would be a
    /// reimplementation, and the two could disagree about a same-millisecond tie
    /// in exactly the way §1's failure mode 3 describes.
    ///
    /// Returns the index the message now occupies, or `None` if the identity was
    /// already held — **which is a dedup decision, so it belongs here and not in
    /// the caller.** A caller that inserted anyway would hold two rows for one
    /// `client_msg_id`.
    fn insert_at_order(&mut self, message: Message) -> Option<usize> {
        let client_msg_id = message.client_msg_id;
        if self.by_client_msg_id.contains_key(&client_msg_id) {
            return None;
        }

        // Ascending: the held messages that are not *after* the incoming one are
        // exactly the prefix, which is what `partition_point` requires.
        let at = self
            .messages
            .partition_point(|held| ordering::compare(held, &message) != Ordering::Greater);

        self.messages.insert(at, message);
        for index in self.by_client_msg_id.values_mut() {
            if *index >= at {
                *index += 1;
            }
        }
        self.by_client_msg_id.insert(client_msg_id, at);
        Some(at)
    }

    /// Drops the row at `at`, keeping the index consistent.
    ///
    /// Takes the row back by value rather than reading it, so the caller can
    /// retire its server id without a second borrow.
    fn remove_at(&mut self, at: usize) -> Option<Message> {
        // Checked rather than indexed: `Vec::remove` panics on an out-of-range
        // index, and `AGENTS.md` §2.1 forbids a panic on a path with nothing to
        // recover it. The bound holds by the index's own invariant, so this is
        // defence rather than a live branch.
        if at >= self.messages.len() {
            return None;
        }
        let removed = self.messages.remove(at);
        self.by_client_msg_id.remove(&removed.client_msg_id);
        for index in self.by_client_msg_id.values_mut() {
            if *index > at {
                *index -= 1;
            }
        }
        Some(removed)
    }

    /// Where the message with this `client_msg_id` sits, if it is held.
    fn position_of(&self, client_msg_id: &Uuid) -> Option<usize> {
        self.by_client_msg_id.get(client_msg_id).copied()
    }
}

/// The users this client believes are typing in one channel.
///
/// A `Vec` rather than a `HashSet`, and the reason is the *order*: a typing
/// indicator is a set of names or avatars, and the order transitions arrive in
/// is the order a reader expects to see them stabilise in. A `HashSet` would give
/// an order that changes between runs, which for a five-element list of user ids
/// is a visible flicker nobody asked for. Membership is still O(n), and n is
/// bounded by [`MAX_TYPING_USERS_PER_CHANNEL`], so nothing is lost.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TypingUsers(Vec<String>);

impl TypingUsers {
    /// Whether this channel is already at [`MAX_TYPING_USERS_PER_CHANNEL`].
    pub(crate) fn is_full(&self) -> bool {
        self.0.len() >= MAX_TYPING_USERS_PER_CHANNEL
    }

    /// Whether this user is already shown as typing here.
    pub(crate) fn contains(&self, user_id: &str) -> bool {
        self.0.iter().any(|held| held == user_id)
    }

    /// Whether nobody is typing, so the entry can be retired.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Records a typist, or does nothing if they are already recorded.
    ///
    /// **Idempotent, and that is the point.** A resync can replay a
    /// `TypingUpdated` the client already applied, so a set *insert* rather than
    /// an append is what stops a duplicate indicator.
    pub(crate) fn add(&mut self, user_id: String) {
        if !self.0.contains(&user_id) {
            self.0.push(user_id);
        }
    }

    /// Forgets a typist.
    pub(crate) fn remove(&mut self, user_id: &str) {
        self.0.retain(|held| held != user_id);
    }
}

/// Everything the application knows, and the only thing that owns it.
///
/// # What it holds
///
/// | | |
/// |---|---|
/// | Who | [`self_user_id`](Self::self_user_id) — the account this client is, and the identity the unread rule of module-docs §5 is stated against |
/// | Channels | the channel list, plus the sidebar's cached ordering of it |
/// | Messages | per channel, ascending, one row per `client_msg_id`, bounded by [`MAX_MESSAGES_PER_CHANNEL`] |
/// | Delivery | this client's own sends and how far each got ([`DeliveryState`]) |
/// | Unread | per channel, per the rule in module-docs §5 |
/// | Selection | which channel the chat view is showing |
/// | Presence | who is online, away or offline |
/// | Typing | who is typing where, bounded |
/// | Connection | the connection's state, and each channel's resync cursor |
/// | Rendered segments | the parsed Markdown of recent messages, through `core/cache.rs` |
///
/// # The invariant
///
/// > **INVARIANT (single-thread confinement).** See module docs, §3. An
/// > `AppState` is touched on the thread that owns the GPUI application context
/// > and nowhere else. **Nothing in the type system enforces this** — the type is
/// > `Send + Sync` and a copy can be moved to any thread — which is why the
/// > invariant is written down and why `PLAN.md` §4's single-seam rule is
/// > load-bearing rather than decorative.
///
/// # The segment cache, and why it is in *this* type
///
/// The rendered-Markdown cache is here because **`AppState` is its real owner** —
/// a cache reached only from a global is owned by the global, and work unit
/// 1C-2b's no-lock decision for `core/cache.rs` is only true of a cache reached
/// the way this one is.
/// `the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee` in
/// the test suite is what makes the question concrete: this whole type, cache
/// included, can be moved to another thread without a lock, which is exactly why
/// the confinement invariant is a rule and not a proof.
///
/// # Errors
///
/// None. Every fallible thing in this layer is a **policy outcome** rather than
/// an error — see `state::actions::ApplyOutcome` and `core/cache.rs`'s
/// `InsertOutcome` — so there is no error type here to define and nothing to
/// convert into `ShNexusError`.
///
/// # Example
///
/// ```
/// use sh_nexus::state::app_state::AppState;
///
/// let mut state = AppState::new("u_me");
/// assert_eq!(state.self_user_id(), "u_me");
/// assert_eq!(state.selected(), None);
/// assert_eq!(state.unread("c_1"), 0);
/// assert!(state.messages("c_1").is_empty());
/// assert!(!state.can_send(), "nothing is connected yet");
///
/// // Nothing is unread in a channel this client has not joined, and asking is
/// // not an error -- it is a count of zero.
/// assert_eq!(state.message_count("c_never_seen"), 0);
/// # let _ = &mut state;
/// ```
pub struct AppState {
    self_user_id: String,
    channels: HashMap<String, Arc<Channel>>,
    sidebar: Vec<Arc<Channel>>,
    messages: HashMap<String, ChannelMessages>,
    /// Server message id to `(channel_id, client_msg_id)`, for every held message
    /// that has one.
    ///
    /// **The only reason a lookup by server id exists.** A
    /// `ReactionUpdated` names a *server* id, because that is all the server has,
    /// and a linear scan of every channel's messages to find it would put a
    /// channel-count-sized walk on the reaction path. It holds no positions, so
    /// it needs no fix-up when a message moves.
    message_index: HashMap<String, (String, Uuid)>,
    outgoing: HashMap<Uuid, (String, DeliveryState)>,
    failures: HashMap<Uuid, SendFailure>,
    /// Every `(channel_id, client_msg_id)` currently counted as unread.
    ///
    /// **A set, not a counter, and that is the whole design of the unread
    /// feature.** The count a caller reads is *derived* by counting this set, and
    /// three properties follow structurally rather than by discipline:
    ///
    /// 1. **It cannot exceed the channel's held messages.** Every element names a
    ///    held message, and [`remove_message`](Self::remove_message) removes its
    ///    element with the row. A stored counter incremented once per arrival
    ///    would need the removal path to decrement it, and a path that forgets is
    ///    exactly the bug the proptest in `tests/state_actions.rs` hunts for --
    ///    it found the first version of this, where a message the server moved
    ///    between channels left its old channel's count behind. **The history
    ///    bound is what makes the first clause a live constraint rather than a
    ///    tautology:** [`evict_one_over_cap`](Self::evict_one_over_cap) evicts
    ///    through [`remove_message`](Self::remove_message) for exactly this
    ///    reason, and the count of an evicted message goes with its row.
    /// 2. **It cannot go negative**, because there is nothing to subtract.
    /// 3. **Moving a message is exact.** Relocating a row stops counting it where
    ///    it was and re-evaluates the rule where it now is, with no need to
    ///    reconstruct whether the old channel was on screen at the time it
    ///    arrived -- a question this state has no honest answer to.
    ///
    /// **The cost is on the read, and it is O(k) where k is the number of unread
    /// messages in the whole workspace** -- not the number of messages held. A
    /// busy client has tens, so this is tens of pointer comparisons. The sidebar
    /// should use [`unread_counts`](Self::unread_counts), which is one pass for
    /// every channel, rather than calling [`unread`](Self::unread) per row, which
    /// would be O(rows x k) per frame. `AGENTS.md` §2.3's "no O(n) in the frame
    /// loop" is about n being the history, and here it is not.
    counted: BTreeSet<(String, Uuid)>,
    /// This client's own sends waiting to be transmitted, in enqueue order.
    ///
    /// **`VecDeque<Uuid>` and nothing else, and the shape is the design.** An entry
    /// is an *identity*, not a copy of the message: the content is read back from
    /// the held row at flush time, so a queued send is stored exactly once and an
    /// edit to the row between enqueue and flush is what gets transmitted. **The
    /// alternative — holding `(Uuid, String, String)` — is a second copy of every
    /// queued body**, and `AGENTS.md` §2.3's "no deep clones in hot paths" plus
    /// §7.1's bound on memory both push the other way.
    ///
    /// **A `VecDeque` rather than a `HashMap` or a `BTreeMap` because the order is
    /// the contract.** `PLAN.md` §7 says the reconnect *"flushes the outbox in
    /// enqueue order"*, and a set or a sorted map would give an order chosen by a
    /// hash rather than by the user's keystrokes. The back is where a retry goes,
    /// for the reason `state/actions.rs` documents on `retry_send`.
    ///
    /// **The invariant, and it is what makes holding ids safe:** every entry
    /// names a held row whose [`delivery`](Self::delivery) is
    /// [`DeliveryState::Pending`] or [`DeliveryState::Failed`].
    /// [`oldest_candidate`](Self::oldest_candidate) skips exactly those rows when
    /// it evicts, so **the queue cannot outlive its own content** — an entry's
    /// message is never evicted from under it. The two paths that could break
    /// that invariant (a discard, and a removal) both retire the entry in the
    /// same call, so the property is structural rather than maintained.
    ///
    /// **Bounded by [`MAX_OUTBOX_ENTRIES`], and the bound refuses rather than
    /// drops.** See that constant for why neither alternative is available.
    outbox: VecDeque<Uuid>,
    selected: Option<String>,
    presence: HashMap<String, UserStatus>,
    typing: HashMap<String, TypingUsers>,
    resync_cursors: HashMap<String, DateTime<Utc>>,
    connection: ConnectionState,
    segments: LruCache<Uuid, Arc<Document>>,
}

impl AppState {
    /// A client signed in as `self_user_id`, with nothing loaded yet.
    ///
    /// The segment cache gets [`DEFAULT_SEGMENT_CACHE_CAPACITY`] and
    /// [`DEFAULT_SEGMENT_CACHE_BUDGET`]; a caller that needs other bounds uses
    /// [`with_segment_cache_bounds`](Self::with_segment_cache_bounds) before
    /// anything has been cached.
    ///
    /// **Not `Default`,** for the same reason [`DeliveryState`] is not: an
    /// `AppState` with no self user id cannot answer the unread rule, and a
    /// `Default` would have to invent one.
    pub fn new(self_user_id: impl Into<String>) -> Self {
        Self {
            self_user_id: self_user_id.into(),
            channels: HashMap::new(),
            sidebar: Vec::new(),
            messages: HashMap::new(),
            message_index: HashMap::new(),
            outgoing: HashMap::new(),
            failures: HashMap::new(),
            counted: BTreeSet::new(),
            outbox: VecDeque::new(),
            selected: None,
            presence: HashMap::new(),
            typing: HashMap::new(),
            resync_cursors: HashMap::new(),
            connection: ConnectionState::Disconnected,
            segments: LruCache::with_budget(
                DEFAULT_SEGMENT_CACHE_CAPACITY,
                DEFAULT_SEGMENT_CACHE_BUDGET,
            ),
        }
    }

    /// The same client, with the segment cache bounded differently.
    ///
    /// A builder rather than a `new` parameter because the defaults are right for
    /// production and the tests need small, exhaustively-enumerable bounds — and
    /// a `new(self, capacity, budget)` would make every caller pass numbers it has
    /// no opinion about, which is how a default becomes a de-facto constant
    /// nobody chose.
    pub fn with_segment_cache_bounds(mut self, capacity: usize, budget: Option<u64>) -> Self {
        self.segments = match budget {
            Some(budget) => LruCache::with_budget(capacity, budget),
            None => LruCache::new(capacity),
        };
        self
    }

    // -----------------------------------------------------------------------
    // Read accessors
    // -----------------------------------------------------------------------

    /// The account this client is signed in as.
    pub fn self_user_id(&self) -> &str {
        &self.self_user_id
    }

    /// The connection's state.
    pub fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    /// Whether a send would be transmitted rather than only rendered.
    ///
    /// [`ConnectionState::Connected`] is the only state in which sending is
    /// meaningful (`core/models/events.rs` says so of the variant), so this is
    /// the composer's one question and it is answered by the state rather than by
    /// a `match` the UI would have to repeat.
    pub fn can_send(&self) -> bool {
        matches!(self.connection, ConnectionState::Connected)
    }

    /// The channel the chat view is showing, if any.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// One channel, if it is loaded.
    pub fn channel(&self, channel_id: &str) -> Option<&Channel> {
        self.channels.get(channel_id).map(Arc::as_ref)
    }

    /// Whether a channel is in the loaded list.
    ///
    /// Separate from [`channel`](Self::channel) because a *message* can be held
    /// for a channel that is not in the list — see
    /// `set_channels` — and a caller that needs "can the
    /// user select this" needs the distinction.
    pub fn has_channel(&self, channel_id: &str) -> bool {
        self.channels.contains_key(channel_id)
    }

    /// The sidebar's cached channels, ordered by name then id.
    ///
    /// **Sorted by name and by id, and cached rather than re-sorted per frame**,
    /// which is what `core/models/channel.rs` is describing when it says the
    /// sidebar iterates *"a cached `Vec<Arc<Channel>>` in `AppState`"*. Name
    /// alone is not a total order — two channels may share a display name — so
    /// the id is the tiebreaker and the order is deterministic across runs,
    /// which is what makes two clients' sidebars comparable.
    pub fn sidebar(&self) -> &[Arc<Channel>] {
        &self.sidebar
    }

    /// A channel's messages, **ascending by [`ordering::compare`]**.
    ///
    /// Empty for a channel this client holds nothing for. The slice is
    /// contiguous and is never re-sorted on read.
    pub fn messages(&self, channel_id: &str) -> &[Message] {
        match self.messages.get(channel_id) {
            Some(held) => &held.messages,
            None => &[],
        }
    }

    /// How many messages a channel has, held.
    pub fn message_count(&self, channel_id: &str) -> usize {
        self.messages
            .get(channel_id)
            .map_or(0, |held| held.messages.len())
    }

    /// One message by its `client_msg_id`.
    ///
    /// **Keyed by `client_msg_id` and not by [`id`](Message::id)** because a
    /// pending message has no server id (module docs, §4), and a lookup that
    /// could not find a message the user is looking at would be useless.
    pub fn message(&self, channel_id: &str, client_msg_id: &Uuid) -> Option<&Message> {
        let at = self.messages.get(channel_id)?.position_of(client_msg_id)?;
        self.messages.get(channel_id)?.messages.get(at)
    }

    /// Which channel and `client_msg_id` a **server** message id refers to.
    pub fn locate(&self, server_message_id: &str) -> Option<(&str, &Uuid)> {
        self.message_index
            .get(server_message_id)
            .map(|(channel_id, client_msg_id)| (channel_id.as_str(), client_msg_id))
    }

    /// How far one of this client's own sends got, if it is held.
    pub fn delivery(&self, client_msg_id: &Uuid) -> Option<DeliveryState> {
        self.outgoing.get(client_msg_id).map(|(_, state)| *state)
    }

    /// Why a failed send failed, if it did.
    pub fn failure(&self, client_msg_id: &Uuid) -> Option<&SendFailure> {
        self.failures.get(client_msg_id)
    }

    /// Every send of this client's that has not been acknowledged, as
    /// `(channel_id, client_msg_id)`, sorted.
    ///
    /// **The empty-`id` convention as a query** (module docs, §4). A caller that
    /// renders pending rows, or labels the outbox "N unsent", asks here rather
    /// than scanning the message list and testing `id.is_empty()` itself — which
    /// is the shape that would let a *foreign* message with an empty id be
    /// mistaken for one of ours.
    pub fn pending_sends(&self) -> Vec<(&str, Uuid)> {
        let mut pending: Vec<(&str, Uuid)> = self
            .messages
            .iter()
            .flat_map(|(channel_id, held)| {
                held.messages
                    .iter()
                    .map(move |message| (channel_id.as_str(), message.client_msg_id))
            })
            .filter(|(_, client_msg_id)| {
                self.delivery(client_msg_id) == Some(DeliveryState::Pending)
            })
            .collect();
        pending.sort_unstable();
        pending
    }

    /// How many of this client's own sends the outbox is holding.
    ///
    /// **The number a caller wants before deciding whether a send can be
    /// queued**, and it is exposed rather than hidden behind a `bool` so a test
    /// can assert the *count* — a bound that is only observable as a refusal is
    /// a bound nobody can check has been reached.
    pub fn outbox_len(&self) -> usize {
        self.outbox.len()
    }

    /// Whether the outbox cannot take another send.
    ///
    /// A separate accessor rather than a caller's own comparison against
    /// [`MAX_OUTBOX_ENTRIES`], for the reason [`MAX_TYPING_CHANNELS`]'s two
    /// constants give: a bound the client re-derives is a bound that can be
    /// re-derived wrongly, and the failure mode is a queue that grew past the
    /// number the module documents.
    pub fn outbox_is_full(&self) -> bool {
        self.outbox.len() >= MAX_OUTBOX_ENTRIES
    }

    /// The queued identities, **in enqueue order** — oldest first.
    ///
    /// **`Vec<Uuid>` rather than a borrow, and that is the seam's constraint.**
    /// [`bridge::try_read`](crate::state::bridge::try_read) lends the state to a
    /// closure whose return type cannot borrow from it, so a read that wants to
    /// hand the queue to a caller has to copy the identities. The copy is 16 bytes
    /// per entry and bounded by [`MAX_OUTBOX_ENTRIES`], which is the same trade
    /// [`pending_sends`](Self::pending_sends) already makes for a larger
    /// collection.
    ///
    /// **The order is the contract, not an incidental consequence of the
    /// representation.** `PLAN.md` §7 requires the reconnect to flush in enqueue
    /// order, and this is the accessor that makes that order observable from
    /// outside the layer.
    pub fn outbox_ids(&self) -> Vec<Uuid> {
        self.outbox.iter().copied().collect()
    }

    /// Whether this identity is queued.
    ///
    /// Exposed because the outbox's one invariant — an entry names a held row
    /// that is still `Pending` or `Failed` — is a cross-structure claim, and a
    /// test that cannot ask the question cannot check it.
    pub fn outbox_holds(&self, client_msg_id: &Uuid) -> bool {
        self.outbox.contains(client_msg_id)
    }

    /// How many unread messages a channel has, per the rule in module docs, §5.
    ///
    /// **Zero for a channel this client holds no count for**, and asking is not
    /// an error: a sidebar renders a row for every channel, and most of them
    /// have nothing unread most of the time.
    pub fn unread(&self, channel_id: &str) -> u32 {
        let count = self
            .counted
            .iter()
            .filter(|(held_channel, _)| held_channel == channel_id)
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Every channel's unread count, paired with its id, for a whole-sidebar
    /// update in one pass.
    ///
    /// **The API the sidebar should call**, for the reason [`unread`](Self::unread)
    /// documents: one O(k log c) fold instead of O(rows x k) individual lookups on
    /// the frame path.
    ///
    /// Sorted by channel id so two clients enumerate the same order and a test can
    /// compare against a model without sorting.
    pub fn unread_counts(&self) -> Vec<(&str, u32)> {
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for (channel_id, _) in &self.counted {
            *counts.entry(channel_id.as_str()).or_insert(0) += 1;
        }
        counts.into_iter().collect()
    }

    /// One user's presence, if it is known.
    pub fn presence(&self, user_id: &str) -> Option<UserStatus> {
        self.presence.get(user_id).copied()
    }

    /// Who is typing in a channel, in the order their transitions arrived.
    ///
    /// Empty for a channel nobody is typing in, and **the entry is removed**
    /// rather than left empty — `AGENTS.md` §7.1's bound is on the growth of
    /// state, and an entry nobody can read is growth with no reader.
    pub fn typing(&self, channel_id: &str) -> &[String] {
        match self.typing.get(channel_id) {
            Some(typing) => &typing.0,
            None => &[],
        }
    }

    /// How many channels currently hold a typing set.
    pub fn typing_channel_count(&self) -> usize {
        self.typing.len()
    }

    /// The cursor a resync of this channel should resume from.
    ///
    /// `core/models/events.rs` on `ResyncRequested` says the cursor *"comes from
    /// local state"*, which is why that event is a request and not a frame: this
    /// is the method that supplies it.
    ///
    /// **The newest message this client holds wins over the recorded cursor**,
    /// because a message the client already has must not be asked for again. The
    /// recorded cursor is `channel.last_message_at` as `network/` saw it, and a
    /// message that arrived live after that — or an optimistic row whose local
    /// clock is ahead — is newer than it.
    pub fn resync_cursor(&self, channel_id: &str) -> Option<DateTime<Utc>> {
        let recorded = self.resync_cursors.get(channel_id).copied();
        let newest = self
            .messages
            .get(channel_id)
            .and_then(|held| held.messages.last())
            .map(|message| message.timestamp);
        match (recorded, newest) {
            (Some(recorded), Some(newest)) => Some(recorded.max(newest)),
            (recorded, None) => recorded,
            (None, newest) => newest,
        }
    }

    /// The gap-detection expectation for a channel, as `core/ordering.rs` takes
    /// it.
    ///
    /// **The watermark is `None` and that is the protocol's answer, not a gap in
    /// this layer.** `PLAN.md` §6 does not define a watermark field, so
    /// `core/ordering.rs` §4 states that *on the protocol as it stands today
    /// every resync is* `SyncStatus::Unverifiable` *— and that is the correct
    /// answer, not a missing feature.* Inventing a watermark here would make gap
    /// detection assert continuity it cannot see, which is precisely what that
    /// module exists to prevent.
    pub fn sync_expectation(&self, channel_id: &str) -> SyncExpectation {
        SyncExpectation {
            cursor: self.resync_cursor(channel_id),
            watermark: None,
        }
    }

    // -----------------------------------------------------------------------
    // The rendered-segment cache
    // -----------------------------------------------------------------------

    /// The parsed Markdown of a message, if it is cached.
    ///
    /// **Takes `&mut self`, and it is the one public method on this type that
    /// does.** A cache *hit* is what protects an entry, so `core/cache.rs` §6 is
    /// explicit that a probe which does not promote is a different operation: a
    /// `&self` version would have to be `contains_key` plus a second lookup, and
    /// the two would disagree about what counts as a hit. It changes the cache
    /// and nothing else — it is not a mutation of the application's state, and
    /// that distinction is the single exception
    /// `state_exposes_no_public_mutation_surface` allows.
    ///
    /// **A miss changes the miss counter and nothing else** — no promotion, no
    /// eviction, no allocation. The handle is an `Arc`, so a hit costs one atomic
    /// increment rather than a copy of the tree; `core/cache.rs` §5 is explicit
    /// that the `Arc` clone belongs to the caller.
    pub fn rendered(&mut self, client_msg_id: &Uuid) -> Option<Arc<Document>> {
        self.segments.get(client_msg_id).map(Arc::clone)
    }

    /// Caches a message's parsed Markdown, at a cost the caller declares.
    ///
    /// **The cost is the caller's to declare, and the cache keeps the exact sum
    /// of what it was told** — `core/cache.rs` §5's contract, which this method
    /// exists to honour rather than to compute around. The value type there is
    /// unconstrained precisely so a caller can decide what a thing weighs;
    /// deciding it here would be a guess about rendering that this layer is not
    /// allowed to make (`ui/` owns rendering, and `AGENTS.md` §3.2 gives it no
    /// business logic).
    pub(crate) fn cache_rendered(
        &mut self,
        client_msg_id: Uuid,
        document: Arc<Document>,
        declared_cost: u64,
    ) -> bool {
        !self
            .segments
            .insert_with_outcome(client_msg_id, document, declared_cost)
            .refused
    }

    /// Drops a message's cached segments.
    ///
    /// Called on the two paths where cached Markdown can be **stale or dead**:
    /// a merge whose differing fields include
    /// [`DifferingField::Content`], and the discard of a failed send. Missing
    /// the first is a user-visible bug — a client showing yesterday's text for
    /// an edited message — so the invalidation is driven by the *report* rather
    /// than by the merge happening.
    pub(crate) fn forget_rendered(&mut self, client_msg_id: &Uuid) -> Option<Arc<Document>> {
        self.segments.remove(client_msg_id)
    }

    /// Cache diagnostics: resident entries, declared total cost, hits, misses and
    /// evictions.
    ///
    /// Returned as a plain tuple rather than as borrowed handles so a caller
    /// cannot hold one across a mutation, and so a `tracing` line can carry the
    /// numbers without a `Debug` on the cache itself — `core/cache.rs` §11
    /// refuses to derive `Debug` precisely so that a cache of rendered message
    /// content is not one import away from a log line.
    pub fn segment_cache_stats(&self) -> (usize, u64, u64, u64, u64) {
        (
            self.segments.len(),
            self.segments.total_cost(),
            self.segments.hits(),
            self.segments.misses(),
            self.segments.evictions(),
        )
    }

    // -----------------------------------------------------------------------
    // `pub(crate)` primitives. `state/actions.rs` holds every decision about
    // when and how these are called; this file holds none.
    // -----------------------------------------------------------------------

    pub(crate) fn set_connection(&mut self, state: ConnectionState) {
        self.connection = state;
    }

    pub(crate) fn set_selected(&mut self, channel_id: Option<String>) {
        self.selected = channel_id;
    }

    /// Records the channel list, and **keeps every message**.
    ///
    /// A channel disappearing from the list does not delete its history here, and
    /// that is a decision rather than an oversight: this is the shape of
    /// `GET /channels`, and a channel missing from one response may be a
    /// permissions change, a partial response, or a channel the user was removed
    /// from and will be removed from. **Deleting history on that evidence loses
    /// messages, and `AGENTS.md` §1 ranks "no lost messages" second.** History's
    /// lifecycle belongs to `db/` in Phase 3, which is the layer that can tell a
    /// real deletion from a missing row.
    pub(crate) fn set_channels(&mut self, channels: Vec<Arc<Channel>>) {
        self.channels.clear();
        self.sidebar = channels;
        for channel in &self.sidebar {
            self.channels
                .insert(channel.id.clone(), Arc::clone(channel));
        }
        self.sidebar.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    /// Clears a channel's unread count, and does nothing for a channel that has
    /// none.
    ///
    /// A `retain` rather than a counter write, and **only a `retain`** — a
    /// per-channel counter would have to be created for every channel the user
    /// ever selects, which is unbounded growth of the kind `AGENTS.md` §7.1
    /// forbids, for no reader: a count of zero is already what an absent entry
    /// reports.
    pub(crate) fn clear_unread(&mut self, channel_id: &str) {
        self.counted.retain(|(held, _)| held != channel_id);
    }

    /// Records a message as unread, or does nothing if it already is.
    pub(crate) fn count_unread(&mut self, channel_id: &str, client_msg_id: Uuid) {
        self.counted.insert((channel_id.to_owned(), client_msg_id));
    }

    /// Stops counting a message, wherever it was counted, and reports whether it
    /// was.
    ///
    /// **Keyed by identity rather than by channel**, because the two callers
    /// disagree about which channel they mean: a removal knows the row's channel,
    /// and a merge or a relocation only knows where the message is going.
    ///
    /// The returned flag is what lets a merge keep a message's unread state: the
    /// merge path has to remove the row before re-inserting it (the ordering key
    /// may have changed), and that removal retires the count. **The proptest in
    /// `tests/state_actions.rs` found that leak** — the first version of
    /// `merge_into_held` dropped the count, so any message that was reconciled
    /// against a later copy stopped being unread.
    pub(crate) fn stop_counting(&mut self, client_msg_id: &Uuid) -> bool {
        let before = self.counted.len();
        self.counted.retain(|(_, counted)| counted != client_msg_id);
        self.counted.len() != before
    }

    /// The one mutable door onto a [`Message`] in the whole application.
    ///
    /// **`pub(crate)`, and used once**: a reaction merge in `state/actions.rs`.
    /// Every other write to a held message replaces the whole row through
    /// [`remove_message`](Self::remove_message) and
    /// [`insert_message`](Self::insert_message), which are the only two paths
    /// that also re-sort and re-index — so a field that changes the ordering key
    /// cannot be edited behind the index's back.
    pub(crate) fn message_mut(
        &mut self,
        channel_id: &str,
        client_msg_id: &Uuid,
    ) -> Option<&mut Message> {
        let at = self.messages.get(channel_id)?.position_of(client_msg_id)?;
        self.messages.get_mut(channel_id)?.messages.get_mut(at)
    }

    /// Which channel holds the message with this `client_msg_id`, if any.
    ///
    /// **The inverse of [`locate`](Self::locate)**, and the pair between them is
    /// what makes "one row per `client_msg_id` across the whole application"
    /// checkable from outside: a `client_msg_id` is globally unique, so the same
    /// identity in two channels is a defect of this layer rather than of the
    /// protocol.
    ///
    /// The send map is consulted first, because an ACK's row is almost always one
    /// of this client's sends; the scan is the fallback and costs one hash probe
    /// per channel, on the ACK and the send path only.
    ///
    /// **The send map's answer is verified against the row it names**, and that
    /// check is not redundant. The send map records which channel a *send* was
    /// made in, and a message the server later moves is retargeted on the move --
    /// so between the two there is a window in which the map names a channel that
    /// holds nothing. Reporting that would make this method's answer a statement
    /// about a `HashMap` rather than about where the message is, and the pair
    /// with [`locate`](Self::locate) is what makes the one-row-per-identity check
    /// possible. It cost one hash probe and the proptest in
    /// `tests/state_actions.rs` earned it.
    pub fn channel_holding(&self, client_msg_id: &Uuid) -> Option<&str> {
        if let Some(channel_id) = self.outgoing_channel(client_msg_id) {
            let named = self
                .messages
                .get(channel_id)
                .is_some_and(|held| held.position_of(client_msg_id).is_some());
            if named {
                return Some(channel_id);
            }
        }
        self.messages
            .iter()
            .find(|(_, held)| held.position_of(client_msg_id).is_some())
            .map(|(channel_id, _)| channel_id.as_str())
    }

    /// Whether the held message was written by this client.
    pub(crate) fn self_owns(&self, channel_id: &str, client_msg_id: &Uuid) -> bool {
        self.message(channel_id, client_msg_id)
            .is_some_and(|message| message.user_id == self.self_user_id)
    }

    /// The one place a message becomes newly held.
    ///
    /// Returns the index the message occupies **once this call has settled**,
    /// or `None` if its `client_msg_id` was already held in that channel — **a
    /// dedup decision, made here so that no caller can hold two rows for one
    /// identity.**
    ///
    /// Only the two index entries are copied out of the message before the move
    /// into the channel: its `client_msg_id` is `Copy`, and its server id is
    /// empty for a message the server has not accepted. **The body is never
    /// cloned** — `AGENTS.md` §2.3's "no deep clones in hot paths" is why this
    /// takes the message by value rather than by reference.
    ///
    /// # The cap is enforced here, and that placement is a decision
    ///
    /// [`MAX_MESSAGES_PER_CHANNEL`] is applied by this function rather than by
    /// its callers, and the reason is the same one [`insert_at_order`] gives for
    /// refusing a duplicate identity: **a bound maintained by every path that
    /// can breach it is maintained by whichever path somebody forgot.** There
    /// are three call sites here today and a fourth would be one more thing to
    /// remember, and a caller that inserted without evicting would not fail —
    /// it would simply leave the channel uncapped, which is the violation
    /// `AGENTS.md` §7.1 names first and the one this layer's whole set/index
    /// design exists to make impossible.
    ///
    /// **The index this returns is therefore the post-eviction one**, and a
    /// caller that uses it for anything other than a `Some`/`None` check would
    /// be reading a position this function may have just moved. No caller does:
    /// `begin_send`, `ingest_by_identity` and `acknowledge` each use it only as
    /// a refusal signal.
    pub(crate) fn insert_message(&mut self, incoming: Message) -> Option<usize> {
        let server_id = incoming.id.clone();
        let client_msg_id = incoming.client_msg_id;
        let channel_id = incoming.channel_id.clone();

        let at = self
            .messages
            .entry(channel_id.clone())
            .or_default()
            .insert_at_order(incoming)?;
        if !server_id.is_empty() {
            self.message_index
                .insert(server_id, (channel_id.clone(), client_msg_id));
        }

        // **After the insert, and bounded by the arrival's own index.** Two
        // rules, both load-bearing, both documented on the constant:
        //
        // - *after*, so the row being added can never be the row evicted — the
        //   caller has not yet run the unread rule for it, and evicting it here
        //   would leave the unread set naming a message that is no longer held.
        // - *before `at`*, so a channel whose every older row is a send in
        //   flight falls into the documented over-cap case instead of dropping
        //   the message that just arrived.
        //
        // The returned index is adjusted for the shift, which is why the
        // `Some`/`None` shape of the signature is unchanged: one insertion adds
        // one row, so at most one eviction restores the invariant, and a `while`
        // here would be the shape that hides an off-by-one.
        let settled = match self.evict_one_over_cap(&channel_id, at) {
            Some(removed_at) if removed_at < at => at - 1,
            _ => at,
        };
        Some(settled)
    }

    /// Evicts one row from `channel_id` if it is over
    /// [`MAX_MESSAGES_PER_CHANNEL`], and reports the index it was at.
    ///
    /// **Candidates are the rows strictly older than `before`.** `before` is the
    /// index the just-inserted row occupies, and the floor is what keeps the
    /// arrival itself safe — see the constant's documentation for why that is a
    /// correctness requirement and not a preference.
    ///
    /// **A send in flight is never a candidate**, and that is the trap this
    /// whole mechanism exists to avoid: `state/actions.rs`'s `acknowledge` asks
    /// `channel_holding(..)` before anything else, so an evicted pending send
    /// would answer `None` and be re-inserted down the adoption path as if this
    /// client had never sent it.
    ///
    /// **Through [`remove_message`](Self::remove_message), never past it.** That
    /// method is the only thing that retires `message_index` and the unread
    /// set's element, and the unread set's bound is structural *because* it
    /// travels with the row. Returns `None` for a channel within the cap and for
    /// the documented all-sends case alike, which are the same answer: nothing
    /// was retired.
    ///
    /// The returned row is dropped, not stored and not cloned: `AGENTS.md` §2.3's
    /// "no deep clones in hot paths" is why `remove_message` takes the row back
    /// by value — a caller that wanted it back gets it by value, and this caller
    /// wants it gone.
    ///
    /// **And the eviction retires the send's delivery entry, which is the only
    /// place in the application that does.** Before it did,
    /// [`clear_outgoing`](Self::clear_outgoing) was reachable only from
    /// [`discard_failed_send`](crate::state::actions::discard_failed_send), which
    /// applies only to a *failed* send — so an acknowledged send's entry was
    /// inserted once and never removed, the map grew by one entry per send for the
    /// life of the process, and `AGENTS.md` §7.1's bound on the growth of
    /// in-memory state did not hold. A thirty-minute `--mode soak` measured it
    /// still holding sends from its first minute; `docs/BASELINES.md` §"The
    /// 30-minute time base" records that run and, since this line, that the figure
    /// predates the fix.
    ///
    /// **Here and not inside [`remove_message`](Self::remove_message), because
    /// that method has three other callers and two of them are *moves*.** The merge
    /// paths in `state/actions.rs` — `merge_into_held`, and the channel-mismatch arm
    /// of `ingest_by_identity` — remove a row and re-insert the *same*
    /// `client_msg_id` into another channel, then call
    /// [`retarget_outgoing`](Self::retarget_outgoing). A retirement inside
    /// `remove_message` would therefore drop the delivery entry of a send that is
    /// still on screen in its new channel, which is a worse bug than the leak it
    /// fixed: the row would survive and its bookkeeping would not. So the
    /// retirement is placed here, after the removal has actually succeeded and on
    /// the one path where the row is genuinely gone.
    ///
    /// **Only an `Acked` send is retired, and deliberately.** A `Pending` or
    /// `Failed` row cannot reach this point at all —
    /// [`has_outstanding_send`](Self::has_outstanding_send) is what excluded it from
    /// candidacy — and the guard is written anyway so the two facts are not
    /// silently coupled: if a future change ever made such a row a candidate, this
    /// must not take the user's retry away with it. Those sends keep their entry,
    /// and `discard_failed_send` remains their retirement path.
    ///
    /// **What this buys is a structural bound rather than a number.** An entry
    /// either names a row that is held or a send this client is genuinely waiting
    /// on, so the map can never outnumber held rows plus in-flight sends. There is
    /// no cap to choose and no threshold to tune, and the bound moves when the
    /// client's own work does rather than with the length of the session.
    pub(crate) fn evict_one_over_cap(&mut self, channel_id: &str, before: usize) -> Option<usize> {
        if self.message_count(channel_id) <= MAX_MESSAGES_PER_CHANNEL {
            return None;
        }
        let at = self.oldest_candidate(channel_id, before)?;
        // The identity is copied out before the second borrow, so the removal is
        // a plain `&mut self` call rather than a re-entrant one.
        let client_msg_id = self.messages.get(channel_id)?.messages[at].client_msg_id;
        // Read before the removal, because the removal is what makes the answer
        // unreachable: `delivery` is the map this call is about to retire from.
        let delivery_of_evicted = self.delivery(&client_msg_id);
        self.remove_message(channel_id, &client_msg_id)?;
        if delivery_of_evicted == Some(DeliveryState::Acked) {
            // The same door `discard_failed_send` uses, because it already retires
            // from *both* maps and this call must not grow a second removal to
            // disagree with it. An `Acked` send has no `failures` entry —
            // `set_delivery` removed it when the state left `Failed` — so this is
            // consistent rather than merely harmless.
            self.clear_outgoing(&client_msg_id);
        }
        Some(at)
    }

    /// Where the oldest row this channel may lose sits, or `None` if there is
    /// none.
    ///
    /// **The scan walks the head forward and stops at `before`**, which is the
    /// index the just-inserted row occupies. The `Vec` is ascending by
    /// [`ordering::compare`], so the first candidate found is by construction the
    /// oldest thing eviction is allowed to take — *"evict the oldest"* is
    /// therefore not a rule this function has to implement, it is a property of
    /// the order [`ChannelMessages`] already maintains.
    ///
    /// **The `before` floor is the arrival protection.** The row that has just
    /// arrived, and anything newer, are outside the scan: the caller has not yet
    /// run the unread rule for the arrival, so retiring it here would leave the
    /// unread set naming a message that is no longer held. See
    /// [`MAX_MESSAGES_PER_CHANNEL`].
    ///
    /// **The cost, stated rather than assumed.** A channel within its bound never
    /// gets here (the caller checks the count first), and a full channel's head is
    /// the first element, so the common case is one step. The walk is only as
    /// long as the run of sends in flight at the head — bounded by what this
    /// client is waiting on, never by the history.
    fn oldest_candidate(&self, channel_id: &str, before: usize) -> Option<usize> {
        self.messages
            .get(channel_id)?
            .messages
            .iter()
            .take(before)
            .position(|message| !self.has_outstanding_send(&message.client_msg_id))
    }

    /// Where a held message sits in its channel, if it is held there.
    ///
    /// **A public read accessor because a view has to be able to say "that row,
    /// specifically" after the indices under it have moved.** `MessageList` uses
    /// it to keep the row a reader is looking at under the reader when
    /// [`MAX_MESSAGES_PER_CHANNEL`] evicts the head of the channel — the count
    /// tells it how many rows there are, and this tells it which one is the one
    /// that was under the reader before. It is one hash probe, and it is the same
    /// answer [`message`](Self::message) gives about a row, asked about a
    /// position instead of a message.
    pub fn position_of(&self, channel_id: &str, client_msg_id: &Uuid) -> Option<usize> {
        self.messages.get(channel_id)?.position_of(client_msg_id)
    }

    /// Whether this client is still waiting on the server about this message.
    ///
    /// **Both maps, and `failures` as well as `outgoing`, because either one
    /// alone answers the question wrongly.** A `Failed` row is in `outgoing` as
    /// [`DeliveryState::Failed`] and in `failures`; a `Pending` row is in
    /// `outgoing` as [`DeliveryState::Pending`] and in neither. `AGENTS.md` §7.5's
    /// *"failures are never silently dropped"* makes the failed case the one that
    /// must not be evicted, and it is the case a check on `Pending` alone would
    /// wave through.
    ///
    /// An `Acked` row is **not** outstanding: the server has spoken, so the
    /// message is ordinary history and an ordinary eviction candidate.
    fn has_outstanding_send(&self, client_msg_id: &Uuid) -> bool {
        if self.failures.contains_key(client_msg_id) {
            return true;
        }
        matches!(
            self.outgoing.get(client_msg_id),
            Some((_, DeliveryState::Pending | DeliveryState::Failed))
        )
    }

    /// Removes a held message and retires its index entries.
    ///
    /// Takes back by value so the caller can reuse the row if it is *moving* it
    /// rather than discarding it, which is what a channel-mismatched ACK needs.
    pub(crate) fn remove_message(
        &mut self,
        channel_id: &str,
        client_msg_id: &Uuid,
    ) -> Option<Message> {
        let at = self.messages.get(channel_id)?.position_of(client_msg_id)?;
        let removed = self.messages.get_mut(channel_id)?.remove_at(at)?;
        if !removed.id.is_empty() {
            self.message_index.remove(&removed.id);
        }
        // The unread set's element goes with the row, which is what makes the
        // count's upper bound structural rather than maintained. See the field's
        // documentation. The return value is discarded here on purpose: a
        // *removal* has nothing to restore the count for, and a merge that needs
        // it takes it before calling this.
        let _ = self.stop_counting(client_msg_id);
        Some(removed)
    }

    pub(crate) fn set_delivery(
        &mut self,
        client_msg_id: Uuid,
        channel_id: String,
        state: DeliveryState,
    ) {
        if state != DeliveryState::Failed {
            // The reason belonged to the failure and goes when the failure does,
            // so an upgraded send cannot leave a stale error for the UI to show
            // next to a message that was accepted.
            self.failures.remove(&client_msg_id);
        }
        self.outgoing.insert(client_msg_id, (channel_id, state));
    }

    /// Retires a send entirely — used by [`discard_failed_send`](crate::state::actions::discard_failed_send),
    /// which is the only path that removes a row.
    pub(crate) fn clear_outgoing(&mut self, client_msg_id: &Uuid) {
        self.failures.remove(client_msg_id);
        self.outgoing.remove(client_msg_id);
    }

    /// Puts one of this client's sends at the **back** of the outbox, or refuses.
    ///
    /// **The back, always, and the reason is §7's ordering.** `PLAN.md` §7 says
    /// the reconnect flushes *"in enqueue order"*, and a retry is a new attempt at
    /// an old message: putting it at the front would let one repeatedly-retried
    /// message starve everything queued behind it, and would reorder this
    /// client's history against every other client's. The decision that a retry
    /// goes to the back rather than the front is
    /// [`actions::retry_send`](crate::state::actions::retry_send)'s; this is the
    /// primitive it uses, and it has no opinion.
    ///
    /// **Refused at [`MAX_OUTBOX_ENTRIES`], and the refusal is a value rather
    /// than a growth.** `VecDeque::push_back` would accept the entry and leave the
    /// queue one over its own documented bound, which is how a bound stops being
    /// one. `AGENTS.md` §7.1 forbids the unbounded version and §7 forbids the
    /// silent one, so the caller has to be told — and
    /// [`actions::begin_send`](crate::state::actions::begin_send) turns this
    /// refusal into a `Failed` row with a reason naming the bound.
    ///
    /// **Idempotent for an identity already queued**, because a duplicate entry
    /// would make one message flush twice and would break the invariant the field
    /// documents. The `contains` is a linear walk of a queue bounded at 1024, on
    /// the send path and not the frame path.
    pub(crate) fn enqueue_outbox(&mut self, client_msg_id: Uuid) -> bool {
        if self.outbox.contains(&client_msg_id) || self.outbox_is_full() {
            return false;
        }
        self.outbox.push_back(client_msg_id);
        true
    }

    /// Takes one identity out of the outbox, wherever it sits, and reports whether
    /// it was there.
    ///
    /// **By identity rather than by position, because the three callers are three
    /// different positions.** An ACK and a terminal failure usually name the head
    /// or something near it, while a discard can name anything — and `VecDeque`
    /// removal by index would make each caller responsible for knowing which.
    ///
    /// **This is the only door out of the queue, and that is the whole design.**
    /// An entry leaves when the server acknowledges the send or refuses it
    /// terminally, and *nowhere else*: a frame that reached the transport's
    /// outbound queue is still queued, because a successful write means the bytes
    /// left this process and not that the server stored the row. `state/actions.rs`
    /// owns which of those two events has happened; this method only performs the
    /// removal.
    pub(crate) fn dequeue_outbox(&mut self, client_msg_id: &Uuid) -> bool {
        let before = self.outbox.len();
        self.outbox.retain(|queued| queued != client_msg_id);
        self.outbox.len() != before
    }

    pub(crate) fn record_failure(&mut self, client_msg_id: Uuid, failure: SendFailure) {
        self.failures.insert(client_msg_id, failure);
    }

    pub(crate) fn outgoing_channel(&self, client_msg_id: &Uuid) -> Option<&str> {
        self.outgoing
            .get(client_msg_id)
            .map(|(channel_id, _)| channel_id.as_str())
    }

    /// Points a tracked send at a different channel, keeping its state.
    ///
    /// **Only a move can need this**, and only when the server disagrees with
    /// this client about which channel a message is in. A no-op for a message
    /// that is not one of this client's sends, so the merge path can call it
    /// unconditionally instead of branching on ownership.
    pub(crate) fn retarget_outgoing(&mut self, client_msg_id: &Uuid, channel_id: &str) {
        if let Some((held_channel, _)) = self.outgoing.get_mut(client_msg_id) {
            *held_channel = channel_id.to_owned();
        }
    }

    pub(crate) fn has_outgoing(&self, client_msg_id: &Uuid) -> bool {
        self.outgoing.contains_key(client_msg_id)
    }

    pub(crate) fn set_presence(&mut self, user_id: String, status: UserStatus) {
        self.presence.insert(user_id, status);
    }

    /// The typing set for a channel, if one exists.
    pub(crate) fn typing_mut(&mut self, channel_id: &str) -> Option<&mut TypingUsers> {
        self.typing.get_mut(channel_id)
    }

    /// Whether the channel has no typing entry yet.
    pub(crate) fn typing_slot_is_new(&self, channel_id: &str) -> bool {
        !self.typing.contains_key(channel_id)
    }

    pub(crate) fn typing_slot(&mut self, channel_id: &str) -> &mut TypingUsers {
        self.typing.entry(channel_id.to_owned()).or_default()
    }

    pub(crate) fn drop_typing_if_empty(&mut self, channel_id: &str) {
        if self
            .typing
            .get(channel_id)
            .is_some_and(TypingUsers::is_empty)
        {
            self.typing.remove(channel_id);
        }
    }

    pub(crate) fn record_resync_cursor(&mut self, channel_id: &str, after: DateTime<Utc>) {
        let slot = self
            .resync_cursors
            .entry(channel_id.to_owned())
            .or_insert(after);
        if *slot < after {
            *slot = after;
        }
    }
}
