//! The single seam between `network/`'s plain events and GPUI's main thread.
//!
//! `PLAN.md` §4 names this file, and §3.2 and §7.3 are the two rules that make
//! it necessary: §3.2 forbids `network/` from importing `gpui`, and §7.3 forbids
//! blocking `cx.update_global` from a non-UI thread. Something has to carry an
//! event from a socket to the main thread, and the module that produces the
//! event may not call the function that delivers it. `PLAN.md` resolves that by
//! naming an owner. This file is it.
//!
//! # 1. What the compiler guarantees, and what the invariant guarantees
//!
//! These are two different guarantees with two different failure modes, so the
//! distinction is stated precisely rather than as "the single-thread rule is
//! enforced".
//!
//! | | Holds because | Fails as |
//! |---|---|---|
//! | A worker thread cannot call `cx.update_global` | `Context<'a, T>` holds `&'a mut App` (`gpui/src/app/context.rs:22`), and `App` holds `Weak<AppCell>` (`gpui/src/app.rs:748`), `Rc<dyn Platform>` (`gpui/src/app.rs:749`) and `Rc<ActionRegistry>` (`gpui/src/app.rs:752`), so `App` and therefore `Context` are `!Send` | a compile error — asserted in `tests/bridge.rs` both ways, and demonstrated as a `compile_fail` doctest on [`install`] |
//! | `AppState` is only ever read and written on the main thread | **nothing in the type system.** See `state/app_state.rs` module docs §3 | no *type* detects it. Two scanners make every shape that *owns* the state a build failure, and the remaining shape is a closure outliving `try_read` |
//!
//! **So the second row is still a rule, and this file cannot promote it to a
//! type.** What this file does is remove every *other* way to reach the state:
//! the state is reachable only through a `gpui::Global`, a global is reachable
//! only through `&App`, and `&App` is only obtainable from a main-thread
//! context — and the global that holds it is `!Sync` ([`AppStateGlobal`] owns a
//! `Receiver`), so no other thread can hold a reference to the state even if one
//! wanted to. **What is new in 1E-2 is that the gap is now one crossing, not
//! many:** before this file existed every caller of `actions.rs` had to be
//! trusted with the invariant, and there were going to be a dozen of them.
//!
//! **The residual gap, stated exactly rather than rounded off.** `AppState` is
//! `Send + Sync` and its constructor is `pub`, so a `db/`, `network/` or
//! `platform/` module that built its *own* state and applied events to it off the
//! main thread would still compile — nothing in the type system objects.
//!
//! **Both halves of that are now build failures, and neither is a type.** Two
//! scanners in `crates/sh_nexus/tests/bridge.rs` close it:
//! `only_the_bridge_constructs_an_application_state` fails on a second
//! *constructor*, and `no_module_outside_state_names_the_state_type` fails on
//! *any* mention of the type outside `src/state/` — which forecloses holding one
//! as a field or borrowing it mutably in the same rule. The second scanner's doc
//! records why the obvious alternative, a crate-private constructor, would have
//! been wrong.
//!
//! **What that still does not make true is that the confinement is a type
//! property.** It is not, and the remaining shape is narrow: `try_read` lends an
//! `&AppState` to a closure, and that borrow is main-thread only because
//! `&App` is. A closure that outlived the call is the next thing to rule out, and
//! it is not what these guards rule out.
//!
//! # 2. Why the event travels as `Send` and the state does not
//!
//! The boundary carries an **owned, self-contained value** — one
//! [`DomainEvent`] — and never a reference to anything. The producer is handed
//! an [`EventSender`]: `Send + Sync + Clone`, holding nothing but a bounded
//! channel, with no method that returns a reference to anything this crate
//! owns. The consumer drains the channel on the main thread inside
//! `cx.update_global` and hands each event to [`actions::apply_event`].
//!
//! **A `DomainEvent` is the right thing to move because it is complete on its
//! own.** By the time one exists, `sh_nexus::network::mapping` has validated it
//! (`AGENTS.md` §2.1), so delivering it needs no lock, no borrow and no context.
//!
//! **The state is the exact opposite, and the unread rule is the proof.** Whether
//! a message increments its channel's unread count depends on *which channel was
//! selected at the moment it was applied* (`state/app_state.rs` module docs §5,
//! condition 2). That is a fact about the main thread at one instant and it is
//! in no event. An implementation that applied events to a *copy* of the state
//! on a worker thread would have to decide each unread count against a
//! `selected` that may already be stale, then merge the copy back — a
//! lost-update race with no compiler protection and no way to reproduce a
//! failure. The same argument covers the rendered-segment cache, whose recency
//! order is a statement about what the user is looking at *right now*.
//!
//! So the rule this module enforces by construction is: **values cross, state
//! stays.** An event *describes* something that happened; applying it is a
//! decision, and decisions belong to `actions.rs` on the main thread.
//!
//! # 3. Why there is no `Mutex`, and what replaces it
//!
//! A lock would be worse than nothing here, and the reason is structural rather
//! than stylistic:
//!
//! - **It would make a second thread *possible*,** and the invariant is "one
//!   owner, one thread". A `Mutex<AppState>` is not a cheaper way to hold that
//!   invariant; it is standing permission to stop holding it.
//! - **It would sit in front of every state mutation,** on the path
//!   `AGENTS.md` §6.2's 8ms scroll-frame budget measures.
//! - **It would need a poisoning policy,** which `AGENTS.md` §2.1 forbids a
//!   module like this from having.
//!
//! **What replaces it is a channel, and the distinction is the whole point.** A
//! `Mutex` is a *shared handle to mutable state*: two threads hold one object
//! and may both enter. A bounded channel is a *queue of owned values*: the state
//! sits inside a struct only the main thread can reach, and what the other
//! thread holds is a slot to put finished descriptions into. **The `AppState` in
//! this file is never behind anything a worker thread owns.**
//!
//! Stated with full honesty, because this is the sort of claim that is easy to
//! get wrong in the documentation: `std::sync::mpsc` protects its *queue* with an
//! internal mutex. That mutex synchronises producer against consumer and never
//! involves `AppState`; the state is read and written only inside
//! `cx.update_global`, which takes no lock at all. `AGENTS.md` §2.3's "no
//! blocking in the frame loop" and 1C-2b's "no internal synchronisation" are
//! both statements about the state, and neither is affected.
//!
//! **The queue is bounded, and a full inbox is a reported refusal rather than
//! unbounded growth or a silent drop.** `AGENTS.md` §7.1 forbids unbounded
//! in-memory state, and an unbounded `mpsc::channel` would have broken that in
//! the one place a remote peer can push faster than this client can render, so
//! the bridge uses `sync_channel` and `try_send`. **The cost of the bound,
//! stated rather than assumed:** a legitimate burst larger than
//! [`MAX_PENDING_EVENTS`] loses the events past the bound, each reported to its
//! producer as [`DeliveryRefusal::InboxFull`] *with the event handed back*. A
//! client that hits it has a rendering problem, and the refusals are how that
//! becomes visible instead of becoming a quietly stale sidebar.
//!
//! # 4. The rendered-segment cache, and the one probe this file will not add
//!
//! [`AppState::rendered`] is the named exception in 1E-1's
//! `state_exposes_no_public_mutation_surface`: it takes `&mut self` because **a
//! cache hit is what protects an entry**, and `core/cache.rs` §6 is explicit that
//! a probe which does not promote is a *different operation*.
//!
//! [`try_rendered`] therefore calls [`AppState::rendered`] and adds no `&self`
//! variant. A read-only accessor here would have to be a membership test plus a
//! second lookup, and the two would disagree about what counts as a hit — the
//! cache would report misses the reader never caused and evict entries the
//! reader earned. **The honest version costs a `cx.update_global` per read**,
//! which leases the global out and back. `AGENTS.md` §2.3's concern is a lock on
//! the frame path, and there is no lock on this one.
//!
//! # 5. The surface, and why it is short
//!
//! | Door | For |
//! |---|---|
//! | [`install`] | once, at startup, before a window opens |
//! | [`EventSender::deliver`] | a worker thread, with a value |
//! | [`drain`] | the caller that owns the schedule |
//! | [`try_apply_event`] | one event, applied through [`actions::apply_event`] |
//! | [`try_select_channel`], [`try_begin_send`] | the two user gestures `AGENTS.md` §8.1's flows need |
//! | [`try_render_and_cache`], [`try_rendered`] | the parse on render, and the read that promotes it |
//! | [`try_read`] | any read of the state |
//!
//! **There is no general `with_state_mut`, and its absence is the point.**
//! `AGENTS.md` §3.2 asks for every mutation to be auditable, and 1E-1 achieved
//! that by making the mutators `pub(crate)`. One public `&mut AppState` would
//! hand the entire mutator surface back to every module in the crate and undo
//! it. **The number of doors is the audit trail**, and each door below is a
//! decision `actions.rs` already makes; none of them makes a new one.
//!
//! **What is deliberately missing, and why:** [`actions::set_channels`] has no
//! door, because no `DomainEvent` carries a channel list and `network/`'s REST
//! client is Phase 4. Adding a door nothing calls is dead code with a doc
//! comment, which is the shape of work that reads as finished and is not.
//!
//! # 6. What this file does not decide
//!
//! - **When to drain.** [`drain`] is synchronous and the caller owns the
//!   schedule. A foreground task in a later work unit will call it; which loop
//!   calls it decides what a burst costs, and that is not this file's call.
//! - **What to do with a refusal.** Every entry point reports what happened and
//!   changes nothing else. Rendering a refusal, reconnecting, dropping a
//!   message: a view's decision and a worker's decision, not a seam's.
//! - **Whether the connection is up.** `AppState` records it and [`actions`]
//!   owns every transition.

use std::fmt;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::{App, BorrowAppContext, Global};
use thiserror::Error;
use uuid::Uuid;

use crate::core::markdown::Document;
use crate::core::models::events::DomainEvent;
use crate::state::actions::{self, ApplyOutcome, SendOutcome};
use crate::state::app_state::AppState;

/// How many events may sit in the inbox before a producer is refused.
///
/// **A bound rather than a default, and the number is justified by the failure
/// it prevents.** `AGENTS.md` §7.1 forbids unbounded in-memory state, and an
/// unbounded channel would break that in the one place a remote peer can push
/// faster than this client can render. At `AGENTS.md` §6.2's sub-100ms
/// end-to-end target, 1024 queued events is a main thread roughly 100 seconds
/// behind, so **a full inbox means something is already wrong upstream** and the
/// honest response is to refuse the delivery and say so rather than to grow.
pub const MAX_PENDING_EVENTS: usize = 1024;

/// The application state, as GPUI sees it.
///
/// # Why a newtype rather than `impl Global for AppState`
///
/// Two reasons, and the second is load-bearing.
///
/// 1. `AppState` would acquire a GPUI association it has no use for, in the one
///    file 1E-1's guards scan for the absence of `gpui` — and those guards name
///    two files precisely so this one could import `gpui` legitimately. Handing
///    `AppState` a `Global` impl would mean relaxing them, and **a boundary
///    that is relaxed is a boundary nobody reads.**
/// 2. **The global needs somewhere to keep the inbox.** The receiving end is
///    `!Sync` and single-owner by design, so it belongs on the same heap object
///    as the state it feeds: one lease per event instead of two, and no window
///    in which one exists without the other.
///
/// # Why the state is never exposed mutably
///
/// There is deliberately **no `state_mut`**, and no `with_state_mut`. See the
/// module docs, §5. Reads go through [`try_read`], which lends `&AppState`;
/// writes go through the named doors.
pub struct AppStateGlobal {
    state: AppState,
    inbox: Receiver<DomainEvent>,
}

impl Global for AppStateGlobal {}

/// The producing half of the seam: a handle a worker thread may hold.
///
/// # What this type can and cannot do
///
/// It can deliver an owned [`DomainEvent`] and nothing else. It cannot read the
/// state, cannot mutate it, and cannot even *name* it — there is no method that
/// returns a reference to anything this crate owns. **That is the whole of the
/// confinement guarantee on the producing side, and it is structural rather
/// than conventional:** the type has no other operation.
///
/// `Send + Sync` is asserted at compile time in `tests/bridge.rs`, and a
/// delivery from a real `std::thread` is exercised there too, because a `Send`
/// claim nobody has moved is a claim nobody has checked.
#[derive(Clone)]
pub struct EventSender {
    sender: SyncSender<DomainEvent>,
}

impl EventSender {
    /// Hands one event to the main thread.
    ///
    /// **Never blocks, and never drops the event on the floor.**
    /// `AGENTS.md` §2.3 forbids blocking the frame loop, and this is the
    /// producer side: a `try_send` that blocked would put a network stall on
    /// whichever thread called it.
    ///
    /// # Errors
    ///
    /// None as such: a refused delivery is a reported value, not a failure of
    /// the program. See the module docs, §3, for why the refusal hands the
    /// event back rather than discarding it.
    pub fn deliver(&self, event: DomainEvent) -> Delivery {
        match self.sender.try_send(event) {
            Ok(()) => Delivery::Queued,
            Err(TrySendError::Full(event)) => Delivery::Refused(DeliveryRefusal::InboxFull {
                capacity: self.capacity(),
                event: Box::new(event),
            }),
            Err(TrySendError::Disconnected(event)) => {
                Delivery::Refused(DeliveryRefusal::BridgeDropped {
                    event: Box::new(event),
                })
            }
        }
    }

    /// How many events this sender was created with room for.
    ///
    /// Always [`MAX_PENDING_EVENTS`]. Exposed so a refusal's `capacity` field
    /// can be checked against the value the client was built with, rather than
    /// against a number a reader has to remember.
    pub fn capacity(&self) -> usize {
        MAX_PENDING_EVENTS
    }
}

/// What [`EventSender::deliver`] did.
///
/// **One word wide in practice, and that is a decision rather than an accident** —
/// see [`DeliveryRefusal`] for why the refused variant boxes its event.
#[must_use = "a delivery nobody looked at is a message that vanished without saying so"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The event is in the inbox and will be applied on the main thread.
    Queued,
    /// The event was not queued, and this is why.
    Refused(DeliveryRefusal),
}

impl fmt::Display for Delivery {
    /// A single line naming the outcome and never the event's contents.
    ///
    /// `AGENTS.md` §7.5: no message content, ever. A refusal holds a whole
    /// `DomainEvent`, and it is deliberately **not** printed — ids and reasons
    /// are what a developer needs to reproduce a refusal, and a `Debug` on a
    /// message body is one refactor away from a log line carrying the user's
    /// text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queued => write!(formatter, "queued for the main thread"),
            Self::Refused(refusal) => write!(formatter, "refused: {refusal}"),
        }
    }
}

/// Why a delivery was refused.
///
/// **Every variant hands the event back.** `AGENTS.md` §7.5 and `PLAN.md` §7
/// forbid a silent drop, and a refusal that consumed the event would be a silent
/// drop wearing a return type.
///
/// **The event is boxed, and the reason is the shape of the common path.** A
/// `DomainEvent` is over 400 bytes, so an unboxed variant would make every
/// [`Delivery`] over 400 bytes — and `deliver` returns one by value for *every*
/// arriving event, where the overwhelmingly common answer is `Queued` and
/// carries nothing. Boxing moves the cost to the exceptional path and keeps
/// `Delivery` one word wide. The cost of the box is one allocation on a refusal,
/// which is the path that already means something has gone wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryRefusal {
    /// The inbox is at [`MAX_PENDING_EVENTS`].
    InboxFull {
        /// The bound that was reached.
        capacity: usize,
        /// The event, undelivered.
        event: Box<DomainEvent>,
    },
    /// The application is gone, or the bridge was never installed.
    ///
    /// **Not something the producer can act on,** and the honest reading is that
    /// the app is shutting down. The event is still returned, so a caller that
    /// can flush it elsewhere may.
    BridgeDropped {
        /// The event, undelivered.
        event: Box<DomainEvent>,
    },
}

impl fmt::Display for DeliveryRefusal {
    /// A single line naming the reason, with no message content.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InboxFull { capacity, .. } => {
                write!(formatter, "the event inbox is full at {capacity}")
            }
            Self::BridgeDropped { .. } => {
                write!(formatter, "the application state is no longer reachable")
            }
        }
    }
}

/// [`install`] was asked for a state that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InstallError {
    /// The state is already installed.
    ///
    /// **The message names the consequence, not just the condition,** because
    /// the consequence is what a reader needs: a second install orphans the
    /// first inbox, and the events already queued in it go with it.
    #[error(
        "the application state is already installed; installing again would drop the \
         events already queued in the first inbox"
    )]
    AlreadyInstalled,
}

/// Registers the application state and returns the handle a worker may keep.
///
/// Called once, on the main thread, before any window is opened — which is what
/// [`crate::run`] does. **A second call is refused rather than allowed to replace
/// the global,** because replacing it would drop the first inbox: the queued
/// events would be lost and the first [`EventSender`] would start reporting
/// [`DeliveryRefusal::BridgeDropped`] without a word. A refusal reported at
/// startup is cheap to notice.
///
/// # Errors
///
/// [`InstallError::AlreadyInstalled`], and nothing else. This function allocates
/// one bounded channel and one state, neither of which can fail.
///
/// The error deliberately does **not** convert into `ShNexusError`.
/// `AGENTS.md` §3.3's direction of travel is a module error becoming the global
/// error, and a startup-ordering mistake in this crate's own entry point is not
/// a condition any remote peer can cause — putting it in the type every network
/// failure uses would dilute that type.
///
/// # A worker thread cannot do this
///
/// ```compile_fail
/// use gpui::App;
///
/// fn steal_the_context(cx: &mut App) -> std::thread::JoinHandle<()> {
///     std::thread::spawn(move || {
///         let _ = sh_nexus::state::bridge::drain(cx);
///     })
/// }
/// ```
///
/// The closure must be `Send`, `App` is not, and the build stops. `rustc` emits
/// one error per member it cannot send, and three of them name exactly the
/// members this module's table above points at:
///
/// ```text
/// error[E0277]: `std::rc::Weak<gpui::AppCell>` cannot be sent between threads safely
/// error[E0277]: `Rc<(dyn Platform + 'static)>` cannot be sent between threads safely
/// error[E0277]: `Rc<gpui::action::ActionRegistry>` cannot be sent between threads safely
/// ```
///
/// **The three are quoted and not the count** on purpose. On the pinned rev the
/// same build reports around fifty of these, one per `!Send` member reachable
/// from `App`; the count is a property of the pinned revision and the toolchain,
/// and the three named above are a property of this crate's reasoning.
///
/// **This is the one guarantee in this module that is a compile error rather than
/// a rule**, so it is asserted in both directions in `tests/bridge.rs` and
/// cannot be lost silently when GPUI is re-pinned.
///
/// The same shape *without* the thread is fine, and that second block is not
/// decoration: a `compile_fail` doctest passes for **any** compile error, so a
/// typo in the first block would make it pass for the wrong reason. The control
/// proves the only difference is the thread.
///
/// ```no_run
/// use gpui::App;
///
/// fn stay_on_the_main_thread(cx: &mut App) {
///     let _ = sh_nexus::state::bridge::drain(cx);
/// }
/// ```
pub fn install(cx: &mut App, self_user_id: impl Into<String>) -> Result<EventSender, InstallError> {
    if is_installed(cx) {
        return Err(InstallError::AlreadyInstalled);
    }
    let (sender, inbox) = sync_channel(MAX_PENDING_EVENTS);
    cx.set_global(AppStateGlobal {
        state: AppState::new(self_user_id),
        inbox,
    });
    Ok(EventSender { sender })
}

/// Whether the application state is installed.
///
/// The non-panicking companion to reading the global, and the reason this module
/// has no `unwrap` and no panic on a startup-ordering mistake (`AGENTS.md` §2.1):
/// a caller that asks before [`install`] has run gets `None` from every entry
/// point here rather than a crash.
pub fn is_installed(cx: &App) -> bool {
    cx.has_global::<AppStateGlobal>()
}

/// Applies one event, on the main thread, and reports what it did.
///
/// **The only door a network event comes through**, and the signature says the
/// whole contract: it needs a `&mut App`, which is a main-thread value (module
/// docs, §1), it applies the event through [`actions::apply_event`], and it
/// returns the [`ApplyOutcome`] rather than discarding it — a refusal the caller
/// cannot see is a dropped message with extra steps.
///
/// **`None` means the state is not installed,** which is a startup-ordering
/// mistake reported rather than panicked on, per `AGENTS.md` §2.1. The event is
/// consumed by this call in that case, so a caller that cares about it should
/// ask [`is_installed`] first.
///
/// # Errors
///
/// None. `actions.rs` module docs §1 is the reason: a refusal is a policy
/// outcome, not a failure of the program.
pub fn try_apply_event(cx: &mut App, event: DomainEvent) -> Option<ApplyOutcome> {
    if !is_installed(cx) {
        return None;
    }
    Some(cx.update_global::<AppStateGlobal, _>(|global, _| {
        actions::apply_event(&mut global.state, event)
    }))
}

/// Applies every queued event, on the main thread, and reports what happened.
///
/// # Why draining is the caller's decision
///
/// Nothing here schedules anything. The caller's loop is a frame, a foreground
/// task, or a socket read — and which one it is decides what a burst costs, which
/// is a decision this file is not entitled to make. What this function does
/// guarantee is the part that is mechanical: **it applies every event it took,
/// in the order it took them, and the report's arithmetic accounts for all of
/// them.**
///
/// # Errors
///
/// `None` when the state is not installed, for the reason
/// [`try_apply_event`] gives. A drain of a quiet inbox is `Some` with a
/// [`DrainReport`] of zeroes, and returns immediately.
pub fn drain(cx: &mut App) -> Option<DrainReport> {
    if !is_installed(cx) {
        return None;
    }
    Some(cx.update_global::<AppStateGlobal, _>(|global, _| {
        let mut report = DrainReport::default();
        loop {
            match global.inbox.try_recv() {
                Ok(event) => report.record(actions::apply_event(&mut global.state, event)),
                // Nothing queued. `try_recv` does not block, so this is the
                // normal end of every drain rather than a wait.
                Err(TryRecvError::Empty) => break,
                // Queued *and* closed: everything sent has been taken and no
                // producer remains. Both facts are reported.
                Err(TryRecvError::Disconnected) => {
                    report.disconnected = true;
                    break;
                }
            }
        }
        report
    }))
}

/// What one [`drain`] did.
///
/// **The arithmetic is an invariant, not a convenience:** `delivered` is always
/// `applied + refused`, and a test asserts that across a burst. A report that
/// could add up wrong would be worse than no report, because it would be
/// believed.
#[must_use = "a drain whose report goes unread cannot tell a full inbox from a lost message"]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrainReport {
    delivered: usize,
    applied: usize,
    refused: usize,
    disconnected: bool,
}

impl DrainReport {
    /// How many events were taken off the inbox and applied.
    pub fn delivered(&self) -> usize {
        self.delivered
    }

    /// How many of them changed the state as intended.
    ///
    /// Counts [`ApplyOutcome::Applied`], [`ApplyOutcome::Adopted`] and
    /// [`ApplyOutcome::Merged`] together: all three are state this client did
    /// not have before, which is what a caller asking "did anything arrive"
    /// means.
    pub fn applied(&self) -> usize {
        self.applied
    }

    /// How many were refused. The reason is in each [`ApplyOutcome`], and
    /// `actions.rs`'s `IgnoreReason` is what keeps those reasons free of message
    /// content.
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// Whether every producer is gone, so the inbox is empty and closed.
    ///
    /// **The signal to stop polling.** A drain loop that ignores this spins on
    /// a permanently closed channel.
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    /// Whether the drain applied nothing.
    pub fn is_empty(&self) -> bool {
        self.delivered == 0
    }

    /// Folds one applied event into the report.
    fn record(&mut self, outcome: ApplyOutcome) {
        self.delivered += 1;
        match outcome {
            ApplyOutcome::Ignored(_) => self.refused += 1,
            ApplyOutcome::Applied | ApplyOutcome::Adopted { .. } | ApplyOutcome::Merged { .. } => {
                self.applied += 1;
            }
        }
    }
}

/// Selects a channel, on the main thread.
///
/// A named door rather than a general one, for the reason in the module docs, §5:
/// **the surface of ways to change this state is the audit trail.** This one
/// clears the unread count, which is `AGENTS.md` §8.1's Channel Switch Flow, and
/// the decision about which messages count is in `actions.rs`.
///
/// # Errors
///
/// `None` when the state is not installed. A channel that is not loaded, or a
/// blank id, is an [`ApplyOutcome`] refusal per `actions.rs` module docs §1.
pub fn try_select_channel(cx: &mut App, channel_id: &str) -> Option<ApplyOutcome> {
    if !is_installed(cx) {
        return None;
    }
    Some(cx.update_global::<AppStateGlobal, _>(|global, _| {
        actions::select_channel(&mut global.state, channel_id)
    }))
}

/// Puts this client's own message on screen, on the main thread.
///
/// The clock and the identity are **parameters rather than something this file
/// reads** (`actions.rs` module docs, "Why the clock and the identity are
/// parameters"). This is the one place that has to be said twice, because the
/// caller of a bridge is exactly the kind of caller that would otherwise assume
/// the bridge supplies a timestamp. It cannot: `AGENTS.md` §3.2 gives `state/` no
/// clock, and inventing one would write a second, invisible ordering
/// (`state/app_state.rs` module docs §4). **The optimistic row carries an empty
/// server id until the ACK**, and a caller that wants a real one waits for the
/// server.
///
/// # Errors
///
/// `None` when the state is not installed. A blank channel id, an empty body or
/// an already-outstanding identity are [`actions::SendOutcome`] refusals, and
/// `offline: true` in a [`SendOutcome::Pending`] means the outbox owns it.
pub fn try_begin_send(
    cx: &mut App,
    channel_id: &str,
    content: &str,
    client_msg_id: Uuid,
    at: DateTime<Utc>,
) -> Option<SendOutcome> {
    if !is_installed(cx) {
        return None;
    }
    Some(cx.update_global::<AppStateGlobal, _>(|global, _| {
        actions::begin_send(&mut global.state, channel_id, content, client_msg_id, at)
    }))
}

/// Parses a message's Markdown and caches it, on the main thread.
///
/// **The writing half of [`try_rendered`], and it exists because the read door
/// would otherwise be unreachable:** the only way a document enters the cache is
/// [`actions::render_and_cache`], so without this door [`try_rendered`] could
/// never return a hit and would be an untested `Miss`.
///
/// **The parse belongs here rather than in `ui/`** for the reason
/// `actions.rs` gives: `ui/` is presentation and `AGENTS.md` §3.2 gives it no
/// business logic, and the cache is keyed by a `client_msg_id` only this layer
/// holds. The declared cost is the caller's because `core/cache.rs` §5 makes the
/// caller the only party that knows what a rendered message weighs.
///
/// **The bridge does not parse on arrival,** and that is a decision worth
/// stating: a channel's whole history would be parsed by an eager caller, and a
/// bounded cache (§[`MAX_PENDING_EVENTS`]'s sibling, the segment budget) would
/// then evict what the user is about to scroll to. Parse on render.
///
/// # Errors
///
/// `None` when the state is not installed. A document larger than the whole
/// segment budget is an [`actions::IgnoreReason::RenderedDocumentTooLarge`]
/// refusal, and the message is unaffected: it is re-parsed on every read, which
/// is `core/cache.rs` §12's stated cost of the ceiling.
pub fn try_render_and_cache(
    cx: &mut App,
    client_msg_id: Uuid,
    source: &str,
    declared_cost: u64,
) -> Option<ApplyOutcome> {
    if !is_installed(cx) {
        return None;
    }
    Some(cx.update_global::<AppStateGlobal, _>(|global, _| {
        actions::render_and_cache(&mut global.state, client_msg_id, source, declared_cost)
    }))
}

/// Reads a message's parsed Markdown, on the main thread, through the promoting
/// probe.
///
/// **See the module docs, §4 before changing this.** A hit promotes the entry,
/// which is why the signature needs `&mut App`; a `&App` version would have to
/// be a membership test plus a second lookup, and the two would disagree about
/// what counts as a hit.
///
/// # Errors
///
/// [`Rendered::NotInstalled`] when the state is not installed. A cache refusal
/// is not this function's business: the *write* side reports it as
/// [`actions::IgnoreReason::RenderedDocumentTooLarge`], and a refused document
/// is re-parsed on every read, which is `core/cache.rs` §12's stated cost.
pub fn try_rendered(cx: &mut App, client_msg_id: &Uuid) -> Rendered {
    if !is_installed(cx) {
        return Rendered::NotInstalled;
    }
    cx.update_global::<AppStateGlobal, _>(|global, _| match global.state.rendered(client_msg_id) {
        Some(document) => Rendered::Hit(document),
        None => Rendered::Miss,
    })
}

impl fmt::Display for Rendered {
    /// One word, and no document content.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => write!(formatter, "not installed"),
            Self::Hit(_) => write!(formatter, "hit"),
            Self::Miss => write!(formatter, "miss"),
        }
    }
}

/// The outcome of a [`try_rendered`] probe.
///
/// **Three named cases rather than an `Option<Option<_>>`,** and the third exists
/// so "the bridge is not installed" cannot be confused with "this message has
/// never been parsed" — a distinction a UI would otherwise have to invent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendered {
    /// The state is not installed.
    NotInstalled,
    /// A hit. The entry was promoted, and the `Arc` is the caller's to keep.
    Hit(Arc<Document>),
    /// A miss. The miss counter moved and nothing else did — `core/cache.rs` §6.
    Miss,
}

impl Rendered {
    /// The cached document, if this probe was a hit.
    pub fn document(&self) -> Option<Arc<Document>> {
        match self {
            Self::Hit(document) => Some(Arc::clone(document)),
            Self::Miss | Self::NotInstalled => None,
        }
    }

    /// Whether this probe was a hit.
    pub fn is_hit(&self) -> bool {
        matches!(self, Self::Hit(_))
    }
}

/// Reads the state, on the main thread, through a shared borrow.
///
/// **The read side of the surface, and the reason it is a closure rather than a
/// returned `&AppState`:** a borrow cannot outlive the lease GPUI takes out to
/// serve it, so this hands the caller a scope rather than an escape hatch. A
/// caller that could hold `&AppState` across a mutation could observe a state
/// that never existed.
///
/// **A second consequence, free and not designed: `R` cannot borrow from the
/// state.** `R` is a plain type parameter rather than one tied to the closure's
/// argument, so a read must return owned data — `message_count(..)` rather than
/// a `&Message`. That is the right constraint for a frame, which wants numbers
/// and copies, and it is checked by the compiler rather than by review.
///
/// # Errors
///
/// `None` when the state is not installed.
pub fn try_read<R>(cx: &App, read: impl FnOnce(&AppState) -> R) -> Option<R> {
    cx.try_global::<AppStateGlobal>()
        .map(|global| read(&global.state))
}
