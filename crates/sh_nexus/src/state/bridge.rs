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
//! | [`try_retry_send`] | the third user gesture: a failed send's badge. **It does not transmit** |
//! | [`try_send_message`] | one send, straight onto the wire |
//! | [`try_flush_outbox`] | every send `PLAN.md` §7 queued, in enqueue order |
//! | [`try_render_and_cache`], [`try_rendered`] | the parse on render, and the read that promotes it |
//! | [`try_read`] | any read of the state |
//! | [`begin_login`] | the one door a credential crosses — see §7 |
//! | [`try_take_login_outcome`], [`login_in_flight`] | the answer, and whether one is outstanding |
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
//! **`actions::discard_failed_send` has no door either, and that one is a
//! decision rather than a queue.** Its sibling above got a door because a badge
//! wants a retry; this one removes a row the user wrote, and a destructive action
//! reached by a stray click is worse than no affordance at all. A door here would
//! also be a door with no caller, which is what the paragraph above rejects — so
//! the absence is doubly deliberate, and `tests/layer_boundary.rs`
//! (`the_destructive_discard_has_no_ui_caller`) is what keeps it honest instead
//! of leaving it to the next reader's judgement.
//!
//! # 6. The outbox, and the one thing this seam must not do with it
//!
//! [`try_flush_outbox`] is the whole of `PLAN.md` §7's outbox on this side of the
//! boundary, and it exists because of a fact about the module above: `network/ws.rs`
//! consumes an item off its outbound queue and **does not put it back** when
//! `write_frame` fails. A transport queue is therefore backpressure between the
//! enqueue and the write, and a send that met a dying socket there was destroyed.
//!
//! **The rule this file follows, stated once: this seam may push frames and it may
//! report what happened, and it may never retire a queued send.** An entry leaves
//! the outbox when the server acknowledges the send or refuses it terminally, and
//! that is [`actions::acknowledge`] and [`actions::fail_send`] — decisions on the
//! other side of this line, made on evidence from the server rather than on
//! evidence about bytes leaving this process. **A door here that "clears" the
//! queue would be the exact defect this unit removes, wearing a return type.**
//!
//! # 7. The login door, and why this file is allowed one credential
//!
//! A login is an HTTP exchange, not a [`DomainEvent`], and it is the one place in
//! this crate where a **password** exists at all. It arrives here by way of
//! [`begin_login`], and three properties make it safe rather than policed:
//!
//! | Property | How it is structural |
//! |---|---|
//! | nothing stores it | the parameter is taken **by value** and moved into a worker closure, so there is no field below this layer to put it in |
//! | nothing prints it | [`LoginOutcome`]'s `Debug` and `Display` are written out by hand and no other type here carries a secret |
//! | nothing logs it | the worker names the outcome and the handle; `AGENTS.md` §7.5's rule is a source scan in `tests/login.rs` |
//!
//! **The answer travels on its own bounded channel rather than through the inbox,
//! and that is a decision rather than an oversight.** A login produces no
//! [`DomainEvent`], so making `drain` return it would mean a second return type on
//! a door that exists to return one thing — and it would make the answer's arrival
//! depend on whether the socket happened to be quiet. `tests/login.rs` drives a
//! completed login against a completely silent inbox to keep the two independent.
//!
//! **What this file still refuses to do with it: decide.** The seam reports what
//! the server said; `app::Shell` decides what the window shows and `ui/views/
//! login.rs` decides how it is drawn. That is §8 below, applied to the one door
//! that carries a secret.
//!
//! # 8. What this file does not decide
//!
//! - **When to drain, and when to flush.** [`drain`] is synchronous and the caller
//!   owns the schedule; `Shell::apply_inbox` is what calls both, and why it calls
//!   the flush on every tick rather than only on the transition to `Connected` is
//!   that method's own note.
//! - **What to do with a refusal.** Every entry point reports what happened and
//!   changes nothing else. Rendering a refusal, reconnecting, dropping a
//!   message: a view's decision and a worker's decision, not a seam's.
//! - **Whether the connection is up.** `AppState` records it and [`actions`]
//!   owns every transition.
//! - **Whether a login was good enough to open a window.** [`try_take_login_outcome`]
//!   hands the answer over; the shell builds the [`ConnectionSettings`] and the
//!   view draws the refusal.

use std::fmt;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::{App, BorrowAppContext, Global};
use thiserror::Error;
use uuid::Uuid;

use crate::core::markdown::Document;
use crate::core::models::events::DomainEvent;
use crate::network::rest::{self, LoginError, Session};
use crate::network::ws::WsTransport;
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
    cx.set_global(OutboundGlobal { transport: None });
    // The login seam's channel is created here rather than per attempt, so that
    // `begin_login` needs nothing but `&mut App` and a second window could not
    // arrive at a second channel. See §7.
    let (login_sender, login_inbox) = sync_channel(MAX_PENDING_LOGINS);
    cx.set_global(LoginGlobal {
        sender: login_sender,
        inbox: login_inbox,
        in_flight: false,
    });
    Ok(EventSender { sender })
}

/// The socket this application sends through, or `None`.
///
/// **Its own global rather than a field on [`AppStateGlobal`], and the reason is
/// that the state is domain data with a documented shape while a socket handle is
/// not domain data at all.** `AGENTS.md` §3.1 describes `state/` as channels,
/// presence, unread counts and cursors; an `mpsc::Sender` in the middle of that
/// would be a field no reader of `AppState` could interpret, and §7.1's ban on
/// unbounded in-memory state would be argued about by anyone who found it.
///
/// **This global is the blessed edge from `network/` into the application.**
/// `tests/layer_boundary.rs::ui_reaches_gpui_and_the_bridge_and_nothing_below_them`
/// forbids `ui/` from importing `network/` and names this module as the only way
/// through, which is what §3.2 means by *"network/ is reached through
/// state/bridge.rs or not at all"*. The first attempt at this feature held a
/// `WsTransport` in `MessageList` instead, and that test caught it — the UI layer
/// reached three layers down to build a frame it had no business naming.
#[derive(Default)]
pub struct OutboundGlobal {
    transport: Option<WsTransport>,
}

impl Global for OutboundGlobal {}

/// What happened to a send handed to [`try_send_message`].
///
/// **Named `EnqueueOutcome`, not `SendOutcome`, because this module already imports
/// `actions::SendOutcome`** for the other half of the gesture — whether the *state*
/// took the optimistic row. Two different answers to two different questions, and
/// giving them one name would invite a caller to match the wrong one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// The frame is on the transport's outbound queue.
    Queued,
    /// There is no socket, so nothing was enqueued.
    ///
    /// **Not a failure, and the distinction is the whole point.** `AppState`'s
    /// `can_send` already requires `ConnectionState::Connected`, so a user cannot
    /// reach this by typing.
    ///
    /// **And a send composed while disconnected never reaches here at all** — it
    /// is queued in `AppState` by [`actions::begin_send`] and driven by
    /// [`try_flush_outbox`] when the connection returns. `PLAN.md` §7's outbox
    /// exists so that this variant is about *this* door's socket rather than about
    /// every send.
    NoTransport,
    /// The socket refused the frame synchronously — closed, or its queue full.
    ///
    /// The reason is carried because it is shown to the user on the failed row's
    /// badge, and a badge reading "failed" with no cause is the thing `AGENTS.md`
    /// §5.2's "clear, actionable error" is written against.
    Refused(String),
}

/// Publishes the socket sends go through, or takes it away again.
///
/// **Called by [`crate::app::Shell::start_transport`] and nowhere else.** One
/// owner and one publication point: if a second path could install a transport,
/// two sockets could exist and a send would land on whichever one happened to be
/// installed last.
///
/// Passing `None` is what a stopped socket looks like, and it is **not** a
/// teardown — the worker thread belongs to the shell, and dropping a handle here
/// only stops new sends from being enqueued.
pub fn install_transport(cx: &mut App, transport: Option<WsTransport>) {
    if !is_installed(cx) {
        return;
    }
    cx.update_global::<OutboundGlobal, _>(|global, _| global.transport = transport);
}

/// Whether a send would reach a socket right now.
///
/// **Read-only, and it exists so a caller can ask the question without sending
/// anything to ask it.** The obvious alternative — calling [`try_send_message`] and
/// matching on the outcome — enqueues a real `message.send` as a side effect of a
/// question, which is a probe that costs a frame on the server and can fail for
/// reasons that have nothing to do with the presence of a socket.
pub fn has_transport(cx: &App) -> bool {
    cx.has_global::<OutboundGlobal>() && cx.global::<OutboundGlobal>().transport.is_some()
}

/// Puts one `message.send` on the wire.
///
/// **The door the composer uses, and the reason it lives here rather than in the
/// view is the layer boundary.** `MessageList::begin_send` has the channel, the
/// content and the `client_msg_id`, so it is the natural place to *decide* to send;
/// naming `WsTransport` to do it is what §3.2 forbids. A send is still the view's
/// decision — this function takes the arguments the view already has and applies
/// no policy of its own beyond reporting what happened.
///
/// Synchronous, and that is what keeps it safe to call from a key handler: it hands
/// the frame to the worker thread's queue and returns. No socket work happens here,
/// so no frame budget is spent and §2.3's "no blocking on the main thread" holds.
pub fn try_send_message(
    cx: &mut App,
    client_msg_id: Uuid,
    channel_id: &str,
    content: &str,
) -> EnqueueOutcome {
    let transport = cx.update_global::<OutboundGlobal, _>(|global, _| global.transport.clone());
    let Some(transport) = transport else {
        return EnqueueOutcome::NoTransport;
    };
    match transport.send_message(client_msg_id, channel_id, content) {
        Ok(()) => EnqueueOutcome::Queued,
        Err(error) => EnqueueOutcome::Refused(format!("the socket refused the send: {error}")),
    }
}

/// What one [`try_flush_outbox`] put on the wire, and what it could not.
///
/// **Three counters and no identities, and that is deliberate twice over.** The
/// counts are what an operator needs from a `warn!` — how much was driven, how
/// much the socket turned away, how much is still sitting in the queue — and
/// `AGENTS.md` §7.5 forbids logging **message content**, so this type carries no
/// `client_msg_id` and no text even though it could: the caller that wants the
/// identities asks [`try_read`] for them, and a reader that wanted them here
/// would be one refactor away from a `Debug` on a message body.
#[must_use = "a flush whose report goes unread cannot tell a refused frame from an empty queue"]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlushReport {
    driven: usize,
    refused: usize,
    held: usize,
}

impl FlushReport {
    /// How many frames reached the transport's outbound queue.
    ///
    /// **Not "how many the server has".** A frame in that queue has left this
    /// process and nothing more; the entry stays in `AppState`'s outbox until the
    /// server acknowledges the send, which is the entire design of
    /// [`actions::flush_outbox`].
    pub fn driven(&self) -> usize {
        self.driven
    }

    /// How many the transport refused synchronously — closed, or its own queue
    /// full.
    ///
    /// **These stay queued and are driven again on the next flush.** A refusal is
    /// a fact about *this* attempt, not a terminal answer, and rolling the rows
    /// back to `Failed` here would defeat the mechanism: the socket's outbound
    /// queue is backpressure, and this queue is the retry that survives it.
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// How many were left in the outbox because there is no socket to drive them
    /// into.
    ///
    /// **The offline shell's ordinary answer**, and worth distinguishing from
    /// "nothing was queued": a client with no `SH_NEXUS_URL` holds a transport
    /// that is `None` forever, so this is the number that says the queue is
    /// waiting rather than empty.
    pub fn held(&self) -> usize {
        self.held
    }

    /// Whether the flush drove nothing.
    pub fn is_empty(&self) -> bool {
        self.driven == 0 && self.refused == 0 && self.held == 0
    }
}

/// Puts every queued send on the wire, in enqueue order.
///
/// **The one door the outbox has, and `PLAN.md` §7's flush.** The order of the
/// operations is the whole contract:
///
/// 1. [`actions::flush_outbox`] resolves the queued identities into frames, **in
///    enqueue order**, reading each body from the row the client holds.
/// 2. Each frame goes to the transport, **in that order** — one loop, no sorting,
///    no re-queueing.
/// 3. **Nothing is dequeued.** Not by a success, not by a refusal. The entry
///    leaves when `MessageAcked` or a terminal `MessageSendFailed` arrives, which
///    `state/actions.rs` owns.
///
/// **Step 3 is the defect this door exists to fix, and the reason is a fact about
/// `network/ws.rs`.** Its worker takes an item off the outbound queue, and if
/// `write_frame` fails the arm returns without putting the item back — so a frame
/// that reaches [`try_send_message`] and then meets a dying socket is *gone*. A
/// transport-level queue is not a queue that survives a disconnection, and
/// `MAX_OUTBOUND_FRAMES` is where that illusion ends. Driving the same frames
/// again on the next flush is safe because the server deduplicates on
/// `client_msg_id` and answers a duplicate with an ACK.
///
/// **Re-driving is a bounded cost, not an unbounded one.** The loop stops driving
/// an entry only when the server answers, and every drive is a `try_send` into a
/// 256-slot queue. A caller that wants the flush only on a *transition* may call
/// this when `ConnectionState` becomes `Connected`; the shell calls it on every
/// tick, because a retry enqueued while already connected would otherwise have no
/// trigger at all (see `Shell::apply_inbox`).
///
/// # Errors
///
/// `None` when the state is not installed, for the reason [`try_send_message`]
/// gives. **A flush with nothing queued, and a flush with no socket, are both
/// `Some`** — a [`FlushReport`] of zeroes or of `held`, never a failure.
pub fn try_flush_outbox(cx: &mut App) -> Option<FlushReport> {
    if !is_installed(cx) {
        return None;
    }

    // **The queue is read before the socket is named**, so a client with no
    // transport pays one map lookup and stops: `flush_outbox` yields nothing while
    // `can_send` is false, and `actions.rs` owns that decision rather than this
    // seam re-deriving it.
    let queued =
        cx.update_global::<AppStateGlobal, _>(|global, _| actions::flush_outbox(&global.state));
    if queued.is_empty() {
        return Some(FlushReport::default());
    }

    let transport = cx
        .try_global::<OutboundGlobal>()
        .and_then(|global| global.transport.clone());
    let Some(transport) = transport else {
        return Some(FlushReport {
            held: queued.len(),
            ..FlushReport::default()
        });
    };

    let mut report = FlushReport::default();
    for send in &queued {
        match transport.send_message(send.client_msg_id(), send.channel_id(), send.content()) {
            Ok(()) => report.driven += 1,
            // **Reported and nothing else.** The entry stays queued and the row
            // stays `Pending`; see `FlushReport::refused`.
            Err(_) => report.refused += 1,
        }
    }
    Some(report)
}

/// How many finished sign-in attempts may sit in this seam's inbox at once.
///
/// **One, and the number is the whole of the concurrency rule rather than a
/// tuning choice.** [`begin_login`] refuses a second attempt while one is
/// outstanding, so the queue can never hold two, and a larger capacity would be a
/// capacity nothing could ever use. `AGENTS.md` §7.1's bound on unbounded
/// in-memory state is therefore satisfied by the door rather than by a policy
/// about it — which is the only kind of bound that survives being forgotten.
pub const MAX_PENDING_LOGINS: usize = 1;

/// What one finished sign-in attempt said.
///
/// **Three cases, and none of them means "something went wrong".**
/// `AGENTS.md` §5.2 asks for an actionable error and a catch-all is the opposite
/// of one. A refusal the server made is a *decision* and a transport failure is
/// the *absence* of one, and the two are fixed by opposite actions — retype a
/// credential, or start the server — so they are separate variants and
/// `ui/views/login.rs` draws them in different colours for exactly that reason.
///
/// **No variant carries a credential, in any field, and that is a property of the
/// type rather than of its users.** It is what lets the two `fmt` impls below be
/// written out by hand with no redaction logic in them at all: there is nothing to
/// redact. [`Session`](crate::network::rest::Session) is the type that *does* hold
/// a token, and it deliberately does not implement [`Debug`] at all, so the only
/// way this value exists is after the token has been separated from the rest of
/// the answer.
#[derive(Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// The server issued a session. The token is the credential; the handle is
    /// not, and is what an operator needs to see.
    LoggedIn {
        /// The opaque session token. Reaches
        /// [`app::ConnectionSettings`](crate::app::ConnectionSettings) and nowhere
        /// else, and is printed by neither impl below.
        token: String,
        /// The account the session belongs to.
        username: String,
    },
    /// The server answered, and the answer was no. All three fields are its own
    /// words, verbatim, for the reason `network/rest.rs` carries them rather than
    /// paraphrasing them.
    Refused {
        /// The status code from the status line.
        status: u16,
        /// The refusal code, e.g. `invalid_credentials`.
        code: String,
        /// The server's sentence about it.
        detail: String,
    },
    /// Nothing was decided: the server was not reached, was not understood, or did
    /// not answer inside its budget.
    Failed {
        /// A sentence a person can act on, and never a credential.
        reason: String,
    },
}

impl fmt::Debug for LoginOutcome {
    /// The token's **presence**, never the token, and for exactly the reason
    /// [`crate::app::ConnectionSettings`]'s own `Debug` is written out by hand:
    /// this type is reachable from any `{:?}` on a log field or an error, so a
    /// derived impl would print a credential and compile while doing it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoggedIn { token, username } => formatter
                .debug_struct("LoggedIn")
                .field("token_configured", &!token.is_empty())
                .field("username", username)
                .finish(),
            Self::Refused {
                status,
                code,
                detail,
            } => formatter
                .debug_struct("Refused")
                .field("status", status)
                .field("code", code)
                .field("detail", detail)
                .finish(),
            Self::Failed { reason } => formatter
                .debug_struct("Failed")
                .field("reason", reason)
                .finish(),
        }
    }
}

impl fmt::Display for LoginOutcome {
    /// One line, and never a credential.
    ///
    /// **`Display` is written out for the same reason `Debug` is**, and it is the
    /// one that matters in practice: `tracing::info!(%outcome)` prints *this*, and
    /// a hand-written `Display` is exactly as capable of a leak as a hand-written
    /// `Debug`. `tests/login.rs` asserts both.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoggedIn { username, .. } => write!(formatter, "signed in as {username}"),
            Self::Refused { detail, code, .. } => {
                write!(
                    formatter,
                    "the server refused the sign-in: {detail} ({code})"
                )
            }
            Self::Failed { reason } => write!(formatter, "{reason}"),
        }
    }
}

/// The login seam's own global: one outstanding attempt, and the slot its answer
/// arrives in.
///
/// **A separate global for the reason [`OutboundGlobal`] is one**, and here the
/// reason is narrower and stronger: a login answer is not domain data.
/// `AGENTS.md` §3.1 describes `state/` as channels, presence, unread counts and
/// cursors, and an attempt in flight is a field no reader of [`AppState`] could
/// interpret — §3.2's "every mutation is auditable" is worth nothing if the field
/// being mutated means nothing.
///
/// **And it is deliberately *not* a field on [`AppState`]**, because that is the
/// shape a credential would take if one were ever stored: the token belongs to
/// [`app::ConnectionSettings`](crate::app::ConnectionSettings) and the password
/// exists only inside one HTTP request. `tests/login.rs` asserts that `AppState`
/// names neither.
///
/// **Nothing secret is stored here.** The worker is handed the credential by
/// value and drops it when the request finishes; what comes back is one of the
/// three [`LoginOutcome`]s above, and two of the three carry no secret at all.
pub struct LoginGlobal {
    /// The producing half, cloned per attempt. `Sync` by construction, which is
    /// what lets a worker thread hold it while the state itself stays `!Sync`.
    sender: SyncSender<LoginOutcome>,
    /// The consuming half, on the main thread. `Receiver` is `!Sync`, so this
    /// global cannot be shared with a thread that is not this one — the same
    /// ownership argument [`AppStateGlobal`]'s inbox makes.
    inbox: Receiver<LoginOutcome>,
    /// Whether an attempt is outstanding: started, and not yet answered on screen.
    in_flight: bool,
}

impl Global for LoginGlobal {}

/// Starts one sign-in attempt on a thread of its own, and reports whether it
/// started.
///
/// **The one door in this crate that a credential crosses, and it takes the secret
/// by value for a structural reason rather than a conventional one.** The
/// parameter is moved into the worker's closure and dropped when the request
/// finishes, so there is no field on this side that could hold it, no borrow that
/// outlives the call, and no `Debug` on any type below that could print it.
/// `tests/login.rs` asserts all three by scanning this file's source, and it is
/// why this file is the single exemption in that scan.
///
/// **A second attempt while one is outstanding is refused, and the reason is a
/// fact about the server rather than a preference.** Every `POST /auth/login`
/// mints a session, so two concurrent attempts mint two, and which one the socket
/// then presented would depend on which answer this pump happened to take first.
/// [`login_in_flight`] is cleared only by [`try_take_login_outcome`], and only
/// when an answer actually arrived — so two calls inside one turn cannot retire
/// the first, and the flag cannot be cleared by a poll that found nothing.
///
/// **Non-blocking, and that is what makes it safe from a key handler.**
/// `AGENTS.md` §2.3 forbids blocking the frame loop; this hands the work to a
/// thread and returns.
///
/// # Errors
///
/// None as such: a refusal to start is a `false`, and it is one of three things —
/// the state is not installed, an attempt is already outstanding, or this machine
/// refused to spawn the thread. `AGENTS.md` §5.2 wants an actionable failure and
/// each of those is actionable by a different party, so the view says which in
/// its own words rather than this returning an enum nobody reads.
pub fn begin_login(cx: &mut App, ws_endpoint: &str, username: &str, password: String) -> bool {
    if !is_installed(cx) {
        return false;
    }
    let Some(attempt) = cx
        .try_global::<LoginGlobal>()
        .map(|global| global.sender.clone())
    else {
        return false;
    };

    // **The flag is claimed before the thread exists**, so a second caller in the
    // same turn cannot get past it, and it is handed back below if the thread
    // could not be started at all.
    let claimed = cx.update_global::<LoginGlobal, _>(|global, _| {
        if global.in_flight {
            return false;
        }
        global.in_flight = true;
        true
    });
    if !claimed {
        return false;
    }

    let endpoint = ws_endpoint.to_owned();
    let username = username.to_owned();
    match std::thread::Builder::new()
        .name("sh_nexus-login".to_owned())
        .spawn(move || {
            let outcome = interpret(rest::login(&endpoint, &username, password));
            tracing::info!(outcome = %outcome, "a sign-in attempt finished");
            // **The slot cannot be full and the channel cannot be closed** while
            // this global lives: there is exactly one attempt at a time and
            // exactly one slot for its answer, and the seam holds the sending half
            // for the process's life. So this cannot silently drop an answer the
            // way §3's inbox can — and if it ever did, the flag would stay set and
            // the window would say an attempt is running rather than nothing.
            let _ = attempt.try_send(outcome);
        }) {
        Ok(_) => true,
        Err(error) => {
            cx.update_global::<LoginGlobal, _>(|global, _| global.in_flight = false);
            tracing::warn!(reason = %error, "this machine refused to start the sign-in worker");
            false
        }
    }
}

/// Takes the answer to a finished attempt, if one is waiting.
///
/// **A separate call from [`drain`], deliberately.** A login produces no
/// [`DomainEvent``, so routing it through the inbox would mean a second answer on
/// a door whose whole shape is "one kind of thing", and would make a completed
/// sign-in depend on whether the socket happened to be quiet.
/// `tests/login.rs::a_login_completes_with_the_event_inbox_completely_silent`
/// drives a completed login against an inbox with nothing in it, in both
/// directions.
///
/// **The flag is cleared only here and only on an answer.** A poll that found
/// nothing leaves an outstanding attempt outstanding, which is what makes
/// [`begin_login`]'s refusal deterministic rather than a race against the pump.
pub fn try_take_login_outcome(cx: &mut App) -> Option<LoginOutcome> {
    if !is_installed(cx) {
        return None;
    }
    cx.update_global::<LoginGlobal, _>(|global, _| match global.inbox.try_recv() {
        Ok(outcome) => {
            global.in_flight = false;
            Some(outcome)
        }
        // Nothing has finished. `try_recv` does not block, so this is the normal
        // end of every poll rather than a wait.
        Err(_) => None,
    })
}

/// Whether a sign-in attempt is outstanding right now.
///
/// **Read-only, and it exists so a caller can ask without attempting anything to
/// ask it.** A view that wanted to know would have to start one to find out, which
/// is a request on the server and a failure for reasons that have nothing to do
/// with whether one is already running.
///
/// **False when the state is not installed**, so the answer is never a panic on a
/// startup-ordering mistake (`AGENTS.md` §2.1) and never a second question with a
/// different shape.
pub fn login_in_flight(cx: &App) -> bool {
    cx.try_global::<LoginGlobal>()
        .is_some_and(|global| global.in_flight)
}

/// One [`LoginError`] as the three outcomes a window knows how to draw.
///
/// **The refusal is the server's own three fields, and the split is the point.**
/// A `Refused` is a decision the server made and a `Failed` is the absence of one,
/// so the view can draw the first in the failure colour and the second in the
/// muted one — and a client that reported a dead server as a rejected credential
/// would send the user to retype a password that was never wrong.
///
/// **Every other variant becomes `Failed`, and its `Display` is already a sentence
/// a person can act on** — "the server could not be reached: …", "the server did
/// not answer within 10s". `AGENTS.md` §3.3 requires a network failure to surface
/// as a recoverable state rather than a crash, and a sentence in the window is
/// that.
fn interpret(outcome: Result<Session, LoginError>) -> LoginOutcome {
    match outcome {
        Ok(session) => LoginOutcome::LoggedIn {
            token: session.token,
            username: session.username,
        },
        Err(LoginError::Refused {
            status,
            code,
            detail,
        }) => LoginOutcome::Refused {
            status,
            code,
            detail,
        },
        Err(other) => LoginOutcome::Failed {
            reason: other.to_string(),
        },
    }
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

/// Puts a failed send back in flight, on the main thread.
///
/// A named door rather than a general one, for the reason in the module docs, §5:
/// **the surface of ways to change this state is the audit trail.** This one is
/// the gesture behind the failed-send badge, so its only caller is
/// `MessageList::retry_failed_send`, and a second caller would have to justify
/// itself here.
///
/// # This door does not transmit anything
///
/// [`actions::retry_send`] moves a send from
/// [`DeliveryState::Failed`](crate::state::DeliveryState::Failed) to `Pending`
/// **and puts it at the back of `AppState`'s outbox**, then stops. It does not
/// push a frame: the frame is [`try_flush_outbox`]'s work, and the next flush
/// picks the entry up because it is queued.
///
/// **So a user who clicks retry now watches the badge change from `failed: …` to
/// `sending…`, and this time it stays only as long as the server takes to
/// answer.** A caller that reported this door's success as "sent" would still be
/// wrong — the transmission is [`try_flush_outbox`]'s, and its report is what
/// says a frame reached the socket.
///
/// # Errors
///
/// `None` when the state is not installed. A send this client does not hold is an
/// [`ApplyOutcome`] refusal per `actions.rs` module docs §1. So is a send that is
/// no longer
/// [`Failed`](crate::state::DeliveryState::Failed) — the server's ACK may land
/// between the frame that painted the badge and the click that hits it — and that
/// refusal is the correct answer rather than an error: there is nothing left to
/// retry, and the row is already right.
pub fn try_retry_send(cx: &mut App, client_msg_id: Uuid) -> Option<ApplyOutcome> {
    if !is_installed(cx) {
        return None;
    }
    Some(cx.update_global::<AppStateGlobal, _>(|global, _| {
        actions::retry_send(&mut global.state, client_msg_id)
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
