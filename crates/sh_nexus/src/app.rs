//! The application shell: root component, global-state owner, theme provider,
//! key handling. `PLAN.md` §4's four items, and nothing else.
//!
//! # 1. The four obligations, and where each one is discharged
//!
//! | `PLAN.md` §4 names | Discharged by |
//! |---|---|
//! | root component | [`Shell`]'s `Render` impl |
//! | global state | [`open`], through [`bridge::install`] |
//! | theme provider | [`theme_colors`], handed to the list and the composer at construction |
//! | key handling | [`Shell::on_key_down`] for the `Escape` ladder, and [`InputBar::on_key_down`] for the field's two keys |
//!
//! **The list is short because every obligation that could have been written as
//! a fifth thing here has a named owner somewhere else**, and duplicating an
//! owner is how a boundary stops being one. `AGENTS.md` §3.2 gives this layer no
//! business logic, `ui/` owns every gesture, and `state/` owns every decision
//! about what the state means — so the shell's own logic is the schedule and the
//! palette, and nothing else.
//!
//! ## The global-state half is a call, not a context mutation
//!
//! `PLAN.md` §4 and `AGENTS.md` §3.2 name `state/bridge.rs` the single owner of
//! the context-mutating GPUI API, and
//! `bridge_is_the_only_file_that_calls_the_context_mutating_api` in
//! `crates/sh_nexus/tests/bridge.rs` fails the build on a second caller anywhere
//! under `src/` — this file included. So [`open`] installs the state by calling
//! the seam's own installer and never reaches for the API itself.
//!
//! # 2. Why the shell holds the producer handle, and the bug that fixed
//!
//! [`bridge::install`] returns an [`EventSender`], and **a dropped sender closes
//! the inbox**: the receiving end reports `Disconnected`, and every later
//! `deliver` answers [`bridge::DeliveryRefusal::BridgeDropped`]. Until this file
//! existed, `src/lib.rs::run` installed the state and discarded the handle
//! immediately, on the reasoning that `network/` has no producer yet. That
//! reasoning was wrong in a way that cost nothing until it would have cost
//! everything: the first work unit to hand this sender to a socket task would
//! have found a client that silently discards every message it receives, with
//! the refusal visible only to the producer.
//!
//! **So the handle is a field of [`Shell`], and that is the whole fix.** It lives
//! as long as the shell does, which is as long as the window does, which is as
//! long as the application does. `network/` will take a *clone* when it lands —
//! [`EventSender`] is `Clone` by design (`bridge.rs` §2) so several producers may
//! share one inbox — and the shell's own handle is what stops the last producer
//! from closing an inbox that still has a consumer.
//!
//! # 3. The drain schedule, and why it is a timer and not a read
//!
//! [`bridge::drain`] is synchronous, and that module's own docs, §6, assign the
//! schedule to the caller. **A shell that never drains renders a channel that
//! never updates**: events queue, the bound is enforced, and nothing the user can
//! see changes. So the schedule is this file's, and it is a repeating
//! [`DRAIN_INTERVAL`] timer.
//!
//! | The alternative | Why it is rejected here |
//! |---|---|
//! | drain from inside `Render` | a frame is not a scheduler, and mutating the state a frame is drawing is re-entrancy on the frame path (`AGENTS.md` §7.3) |
//! | a worker thread with a blocking `recv` | `Context` is `!Send` (`docs/ARCHITECTURE.md` ADR-009), so the state cannot be touched off the main thread at all — and `AGENTS.md` §2.3 forbids blocking the frame loop regardless |
//! | an async channel with an `AsyncApp` wakeup | the right end state, and not available: `bridge.rs` §3's channel is `std::sync::mpsc` with `try_recv`, which has no wakeup to bridge. Phase 4's socket task replaces the poll |
//!
//! **So the pump costs one timer wakeup per [`DRAIN_INTERVAL`] for the life of
//! the process, and that cost is stated rather than hidden.** A tick whose inbox
//! is empty does exactly one non-blocking `try_recv` that answers `Empty`.
//! `AGENTS.md` §6.2's idle-CPU row is `< 2%`, and twenty such polls a second sit
//! three orders of magnitude inside it.
//!
//! **The interval is half the end-to-end budget, and derived from it.**
//! `AGENTS.md` §6.2 puts end-to-end delivery at `< 100 ms`; half of that is the
//! event's wait for a tick and half is the network plus the frame, so
//! [`DRAIN_INTERVAL`] spends the first half and no more. A shorter interval buys
//! latency the budget never asked for and pays a wakeup for it.
//!
//! **The pump ends with the shell rather than with the process.** `cx.spawn`
//! hands the future a weak handle to the shell, and the loop returns when that
//! handle can no longer be upgraded. Holding a strong [`Entity`] instead would be
//! a cycle: the pump would keep itself — and the message list it repaints — alive
//! after the window that owned them closed, which `AGENTS.md` §7.1's ban on
//! unbounded in-memory growth exists to prevent.
//!
//! # 4. The theme, and why there is no hot reload here
//!
//! [`crate::core::theme`] parses and validates and nothing else — `AGENTS.md`
//! §3.2 forbids it calling GPUI — so *applying* a theme is this file's job by
//! construction rather than by choice. [`theme_colors`] resolves
//! [`ACTIVE_THEME`] into a [`Colors`], and the shell hands that to the list once,
//! at construction.
//!
//! **`AGENTS.md` §10.2 also asks for hot reload and for discovery under
//! `~/.config/sh_nexus/themes/`, and neither is here.** Both need
//! `platform/file_watch.rs` and the `notify` dependency, neither of which exists,
//! and `AGENTS.md` §7.2 makes a dependency a recorded decision rather than one to
//! smuggle in alongside a feature. A "reload" that can never fire because nothing
//! watches anything is exactly the dead code `ui/views/mod.rs` refuses to write.
//!
//! **The one affordance that *is* here is `MessageList::set_colors`, and it is
//! here rather than deferred for a reason:** the palette is applied once at
//! construction, so without a setter the theme-provider obligation would be
//! unfulfilled even in the shape that has no runtime change to serve — and a
//! setter that silently fails to reach the recycled rows would be a documented
//! lie about `AGENTS.md` §7.3's rule. `tests/app_shell.rs` asserts the palette
//! reaches the rows, not only the container.
//!
//! # 5. What this file deliberately does not contain
//!
//! `PLAN.md` §6 asks for *"App shell: sidebar + chat area + input bar"*, and the
//! input bar now exists — [`views::input_bar::InputBar`], composed in
//! [`Render`] and focused by [`open`]. **It is a separate view rather than a
//! section of this file, and the reason is the one `bridge.rs` §5 argues
//! generally: a second place to do something is the audit trail, and a composer
//! held here would be a second place to answer "what does Enter do".**
//!
//! The other two of §6's three are still not constructible today, verified
//! rather than assumed.
//!
//! | Missing | Why it is not here |
//! |---|---|
//! | the channel rail | `actions::set_channels` has zero callers because no `DomainEvent` carries a channel list (`bridge.rs` §5), so a rail would render zero channels, permanently |
//! | thread panel, search, editing | `docs/ARCHITECTURE.md` ADR-006 places them outside this work |
//! | a `tracing::error!` for a failed open | `errors.rs` assigns this file a source-chain log, `tracing` is not in the workspace, and §7.2's approval process is not this unit's business |
//!
//! **The composer is not listed among the missing things any more, because
//! `AGENTS.md` §3.2's rule is that a module that exists and does nothing reads
//! as finished work** — and the flip side is that a row claiming a feature is
//! deliberately withheld has to be removed when it is not. It was there in the
//! draft of this file that first composed the list, and the input-bar work unit
//! deleted it for exactly that reason.
//!
//! **And the shell is silent, which is `AGENTS.md` §7.1's ban on `println!`
//! arriving as a consequence rather than as a preference.** There is no sanctioned
//! logger in this crate yet, so a failure here is reported by a returned error
//! and by `main.rs`'s `eprintln!`, and by nothing else.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, App, AsyncApp, Bounds, Context, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Render, Window, WindowBounds, WindowHandle, WindowOptions,
};

use crate::core::theme::BuiltIn;
use crate::errors::ShNexusError;
use crate::state::bridge::{self, EventSender};
use crate::ui::views::input_bar::InputBar;
use crate::ui::views::message_list::MessageList;
use crate::ui::Colors;
use crate::UNSIGNED_IN_USER;

/// Width of the shell's window, in logical pixels.
///
/// **The same viewport ADR-006 step 6 measured, deliberately.**
/// `benches/frame_time.rs` sizes its own window to this and says why: rows per
/// frame — and therefore any frame-time figure — scale with viewport height, so a
/// differently-sized window would make a re-measurement of `docs/BASELINES.md`'s
/// app-level row incomparable with the floor already recorded. The spike's
/// 480x320 is the number that bench's `WINDOW_HEIGHT` rejected in writing, and
/// repeating it here would repeat that mistake. **The equality is now checked by
/// the compiler** — a `const` assertion in the bench fails the build if these two
/// ever diverge — because a doc sentence is not a guarantee.
pub const WINDOW_WIDTH: f32 = 1024.0;

/// Height of the shell's window, in logical pixels. See [`WINDOW_WIDTH`].
pub const WINDOW_HEIGHT: f32 = 768.0;

/// How long the shell waits between two drains of the event inbox.
///
/// **Derived from `AGENTS.md` §6.2's end-to-end budget rather than picked.** That
/// row is `< 100 ms`; half of it is the arriving event's wait for the next tick
/// and half is the network plus the frame, so 50 ms spends the first half and no
/// more. See the module docs, §3, for what the poll costs and what it replaces.
pub const DRAIN_INTERVAL: Duration = Duration::from_millis(50);

/// The channel the shell shows when it opens.
///
/// **A placeholder, and the same shape of placeholder as
/// [`crate::UNSIGNED_IN_USER`]: a name that says what it is rather than one that
/// looks real.** The shell cannot ask which channel to open, because nothing
/// populates the channel list — `actions::set_channels` has no door until a
/// `DomainEvent` carries one (`bridge.rs` §5) — and `PLAN.md`'s channel rail, the
/// view that would choose, arrives after Phase 4 supplies the data. Naming one is
/// what lets the list render at all, because `MessageList::show_channel` shows
/// the channel it is *told* to show whether or not the state has a record of it,
/// and a list with no channel shows nothing by construction.
///
/// **The cost of the placeholder, stated rather than hidden:** a message delivered
/// for any other channel id is invisible, and the unread badge for this one is
/// over-reported while [`crate::UNSIGNED_IN_USER`] is the author. Both are the
/// safe direction — content nobody sees is missed, a badge nobody can trust is
/// one the user stops reading — and both disappear with the channel rail.
pub const STARTUP_CHANNEL: &str = "c_startup";

/// The built-in theme the shell applies.
///
/// **A constant rather than a setting, and the reason is that §10.2's other half
/// cannot exist yet.** A user-chosen theme needs discovery and hot reload, both
/// of which need `platform/file_watch.rs` (see the module docs, §4), so the only
/// honest value here is the one compiled into the binary. It is public because
/// `tests/app_shell.rs` asserts the applied palette *is* this theme's, which is a
/// statement about the wiring rather than about a colour value.
pub const ACTIVE_THEME: BuiltIn = BuiltIn::Dark;

/// The key [`Shell::on_key_down`] acts on, as `gpui`'s keystroke parser spells it.
///
/// **Named for the key rather than for the effect, and that is a correction.** It
/// was `RETURN_TO_TAIL_KEY`, which was honest while the handler did one thing.
/// The handler now does two — the log returns to the newest message *and* the log
/// takes the keyboard — and a constant named after half of it is the kind of name
/// that makes the next reader look for a second handler that does not exist.
const ESCAPE_KEY: &str = "escape";

/// Resolves [`ACTIVE_THEME`] into the colours every element in the shell's tree
/// draws with.
///
/// **This is the "apply" half of `core/theme.rs`, and it lives here because
/// applying means calling GPUI** — `AGENTS.md` §3.2 forbids that in `core/`, and
/// `core/theme.rs`'s own module docs say so in the same words.
///
/// **The fallback arm exists so this cannot fail, not because it is expected.**
/// `BuiltIn::theme` returns a `Result` because a build-time fixture can be
/// corrupted by a bad merge, and `AGENTS.md` §2.1 forbids the `unwrap` that would
/// otherwise be the obvious response. Returning a `Result` instead would push the
/// decision onto every caller — including the frame path — and there is no caller
/// for which "no colours" is a useful answer. `tests/theme.rs`'s
/// `every_built_in_theme_parses_and_validates` is what keeps the document `Ok`,
/// so the arm is unreachable in a shipped binary and is the honest shape for a
/// path that must not be allowed to panic.
pub fn theme_colors() -> Colors {
    match ACTIVE_THEME.theme() {
        Ok(theme) => Colors::from_palette(theme.colors()),
        Err(_) => Colors::dark_fallback(),
    }
}

/// The root component: the palette's owner, the list's parent, the drain pump's
/// lifetime, and the window's key target.
///
/// **The list is held as an [`Entity`] and not built inline in [`Render`], and
/// three things in this file are impossible without that.** The drain pump has to
/// repaint it from a timer callback that owns no window; the key handler has to
/// reach it from a listener; and the palette has to be handed to it *before* the
/// first frame rather than during one. An element constructed inside the render
/// closure exists only for that closure, so all three would have to become
/// "whatever the next frame happens to build", which is the state a view must not
/// be in. ADR-006's step 3 reaches the same conclusion one level down, for the
/// same reason: a thing whose state must outlive a frame is an entity.
pub struct Shell {
    /// The production message list, the one view this shell composes.
    list: Entity<MessageList>,
    /// The composer, above nothing and below the list.
    ///
    /// **An `Entity` for the same reason [`Self::list`] is one** — see this
    /// struct's documentation — and for one more: the shell's render must hand
    /// the composer a stable identity across frames, or the platform's input
    /// handler, which is registered from a paint callback keyed on the *entity*,
    /// would be rebuilt against a different target on every frame.
    input: Entity<InputBar>,
    /// The producer handle `bridge::install` returned, held for the shell's life.
    ///
    /// See the module docs, §2: dropping it closes the inbox.
    sender: EventSender,
    /// The palette this shell's own elements draw with.
    ///
    /// **A copy rather than a borrow of the theme, and the same reason
    /// [`Colors`] exists at all** (`ui/mod.rs`): a frame reads these values, and a
    /// parsed theme document with strings in it is not what the frame path should
    /// be reaching back into.
    colors: Colors,
    /// The focus target for the window's keyboard.
    ///
    /// A root component that never takes focus is a window no key reaches, and
    /// `AGENTS.md` §5.2 requires the feature to work from the keyboard alone.
    ///
    /// **This is the shell's *middle* focus target, not the one the window opens
    /// on and not the one the keyboard ends on.** [`open`] focuses
    /// [`Self::composer_focus_handle`] instead, because a client that opens with a
    /// text field focused is a client the user can type into. This handle is the
    /// rung between: it is what [`InputBar`] hands focus to when `Escape` leaves
    /// the field, and [`Shell::on_key_down`] hands focus *on* to the log from it.
    /// **The three are a ladder and not a ring**, and
    /// [`Shell::on_key_down`] says why the ladder does not come back up.
    focus_handle: FocusHandle,
}

impl Shell {
    /// Builds the shell around a freshly installed application state.
    ///
    /// **`sender` is a parameter rather than something this function installs,
    /// and the ordering is the reason.** `bridge::install` needs a `&mut App`,
    /// which a `Context<Self>` derefs to — but a call made *here* would run inside
    /// `open_window`'s build callback, which is after a window exists, and
    /// `AGENTS.md` §7.3 and `bridge::install`'s own documentation both require
    /// the state to exist before any view can be built. [`open`] installs; this
    /// receives what it installed and keeps it.
    ///
    /// Three things happen once, here, rather than on a later event: the palette
    /// is resolved and handed to both views, the list is pointed at
    /// [`STARTUP_CHANNEL`], and the drain pump is armed. All three are one-time
    /// wiring, and a constructor is the one place a reader can be sure they are
    /// not conditional.
    pub fn new(sender: EventSender, cx: &mut Context<Self>) -> Self {
        let colors = theme_colors();

        // Taken before either view is built, and moved into `Self` at the end,
        // because the composer needs it: `Escape` in the field has to hand focus
        // *somewhere*, and a window left with nothing focused delivers every key
        // to a dispatch node with no listener on it. See `InputBar::fallback_focus`.
        let focus_handle = cx.focus_handle();

        let list = cx.new(MessageList::new);
        list.update(cx, |list, cx| {
            // Two calls, one order: the palette first, so the first frame the
            // user ever sees is already themed rather than briefly not.
            list.set_colors(colors, cx);
            list.show_channel(STARTUP_CHANNEL, cx);
        });

        // The composer is built from the finished list rather than the other way
        // round, and the order is the dependency: `InputBar` holds the list
        // because the send gesture is the list's own door
        // (`MessageList::begin_send`), so a composer built first would be
        // building a reference to a list that does not exist yet. The palette is
        // passed in rather than pushed afterwards, because it is applied once
        // here — see this file's module docs, §4.
        let input = cx.new(|cx| InputBar::new(list.clone(), focus_handle.clone(), colors, cx));

        let shell = Self {
            list,
            input,
            sender,
            colors,
            focus_handle,
        };
        shell.start_drain_pump(cx);
        shell
    }

    /// The production message list this shell composes.
    ///
    /// Exposed so a test can assert what the shell actually holds — that the view
    /// being rendered is `ui::views::message_list::MessageList` and not a stub
    /// that looks like it.
    pub fn list(&self) -> &Entity<MessageList> {
        &self.list
    }

    /// The composer this shell composes.
    ///
    /// Exposed for the same reason [`Self::list`] is: a test asserting that the
    /// input bar on screen is the production [`InputBar`] and not a stand-in is
    /// what makes the wiring claim checkable rather than prose.
    pub fn input(&self) -> &Entity<InputBar> {
        &self.input
    }

    /// The focus handle of the composer — the one this window opens with.
    ///
    /// **A dedicated accessor rather than letting [`open`] reach through
    /// `input().read(..)`, because "what does a new window focus" is a question
    /// with one answer and `bridge.rs` §5's audit-trail argument is about there
    /// being one place to read it.**
    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    /// The producer handle, still open.
    ///
    /// **`network/`'s future entry point, and the assertion that makes the module
    /// docs' §2 claim checkable:** a delivery through this handle must answer
    /// [`bridge::Delivery::Queued`], not
    /// [`bridge::DeliveryRefusal::BridgeDropped`].
    pub fn sender(&self) -> &EventSender {
        &self.sender
    }

    /// The palette this shell's elements draw with.
    pub fn colors(&self) -> Colors {
        self.colors
    }

    /// Arms the repeating task that applies queued events and repaints.
    ///
    /// **Called from [`Shell::new`], so no caller can forget it and no caller can
    /// start it twice.** A second pump would be a second drain of one inbox from
    /// two tasks, and `bridge::drain` is a `try_recv` loop — the second would
    /// simply find nothing, having raced the first for every event, which is a
    /// schedule nobody can reason about.
    ///
    /// **The timer is awaited on the executor rather than spun on, and it is
    /// never a thread.** `AGENTS.md` §2.3 forbids blocking the frame loop and
    /// `docs/ARCHITECTURE.md` ADR-009 establishes that `Context` is `!Send`, so a
    /// worker thread could not drain at all: `bridge::drain` needs a `&mut App`.
    fn start_drain_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |shell, cx: &mut AsyncApp| loop {
            cx.background_executor().timer(DRAIN_INTERVAL).await;
            // `WeakEntity::update` is the liveness check *and* the drain, in one
            // call: it answers `Err` once the shell is released, which is the
            // signal to stop, and it hands the work to the shell that owns it
            // rather than to a captured copy of its list.
            if shell.update(cx, Shell::apply_inbox).is_err() {
                return;
            }
        })
        .detach();
    }

    /// Applies everything the inbox holds, and repaints what read it.
    ///
    /// Returns how many events changed the state, which is the only thing the
    /// pump asks and the only thing a caller could act on.
    ///
    /// **The repaint is conditional on that number, and the condition is the
    /// difference between a chat client and a busy one.** `bridge::drain` never
    /// schedules a frame — its module docs, §6, leave scheduling to the caller —
    /// and `MessageList::sync` must not notify from inside its own render or the
    /// list would redraw forever. So the two have to be joined *here*, on the one
    /// path that knows a real change happened: a tick that applied nothing asks
    /// for no frame, and twenty quiet ticks a second cost no repaints.
    ///
    /// **Only the list is repainted, and that is not an oversight.** The shell's
    /// own tree draws the palette and the child, neither of which is a function
    /// of the state, so asking it for a frame would be a repaint that changes no
    /// pixel. A view added later reads the state too and joins this list; the
    /// alternative — repainting the root unconditionally — is the frame cost this
    /// client cannot afford to pay for a quiet inbox.
    ///
    /// **`None` from the drain is reported as zero applied rather than treated as
    /// a failure**, and the pump keeps running. `None` means the state is not
    /// installed, which [`open`] makes impossible by installing before the window
    /// opens; the alternative is a pump that dies on a condition it cannot fix
    /// and cannot report, which is the silent stall the whole module exists to
    /// prevent.
    fn apply_inbox(&mut self, cx: &mut Context<Self>) -> usize {
        let Some(report) = bridge::drain(cx) else {
            return 0;
        };
        if report.applied() == 0 {
            return 0;
        }
        self.list.update(cx, |_list, cx| cx.notify());
        report.applied()
    }

    /// Handles a key-down aimed at the window.
    ///
    /// **One key, and the count is the point rather than a shortfall.** The one
    /// gesture a chat log needs from the keyboard that a composer does not
    /// already own is the way back to the newest message: ADR-006's table names
    /// *"stick to the newest message"* as a first-class feature of `List`, and
    /// `MessageList::follow_tail` is the half of it that a reader who has
    /// scrolled away needs. `AGENTS.md` §5.2 lists `Escape` among the keys that
    /// must work from the keyboard alone.
    ///
    /// **`Escape` has two owners, and the split is by focus rather than by
    /// arbitration.** While the composer holds focus, `Escape` is the composer's:
    /// it hands focus back to [`Self::focus_handle`] and calls
    /// `cx.stop_propagation`, so this handler never runs. With this handle
    /// focused, `Escape` is this shell's. **The two keys are sequential
    /// gestures, not competing ones**, and the user gets both from one key.
    ///
    /// # The chain: composer → shell → log, and where it stops
    ///
    /// **This handler is the middle rung, and it is the rung that makes the log
    /// reachable by key at all.** `InputBar`'s `fallback_focus` is this shell's own
    /// root handle, so the first `Escape` lands here; the second hands focus to the
    /// list's handle, and from there `up`/`down` move the log's cursor and `enter`
    /// retries a failed send. Before this, the log was reachable by API and by test
    /// and by **no key in the shipped client** — the gap ADR-006's 3E notes record
    /// (`docs/ARCHITECTURE.md`).
    ///
    /// **The chain is a ladder, and it stops at the bottom rather than
    /// alternating.** A second `Escape` while the list is focused runs *this*
    /// handler again — [`MessageList::on_key_down`] deliberately does not claim
    /// `escape`, so the key travels up the dispatch path — and lands on exactly the
    /// same state: the log snaps to the newest message and the list holds focus,
    /// which it already did. **That is idempotent because of a measured property of
    /// the framework rather than a flag in this file:** [`Window::focus`] returns
    /// early when the handle it is given already holds the focus
    /// (`gpui/src/window.rs:2303`), so re-asserting it is free, cannot recurse, and
    /// no handler in this crate ever targets the shell from below. "Terminating"
    /// means exactly that: the result is a function of where focus already is, and
    /// nothing moves it back up.
    ///
    /// **The alternative was a cycle — the list handing `Escape` back to the
    /// composer — and it is rejected for one reason stated three times.** It would
    /// make `escape` a toggle, so a user pressing it repeatedly walks three views
    /// forever, which is the focus loop this chain exists to avoid. It would need a
    /// **second owner** of `escape`, against the rule 3D established and the
    /// paragraph below depends on: one key, one place that answers what it does.
    /// And it would make the log's own "back to the newest message" gesture
    /// unreachable from *inside* the log — which is exactly where a reader who has
    /// scrolled away most wants it.
    ///
    /// **The cost is stated rather than hidden: the ladder does not come back up, so
    /// a keyboard-only user who presses `Escape` twice cannot return to the composer
    /// with a key.** That is a real hole and it is **pre-existing rather than
    /// introduced here**: with one rung, a single `Escape` already stranded the
    /// keyboard on this shell root, and nothing in the crate has a key that returns
    /// focus to the composer. The missing piece is a `Tab` binding on
    /// [`Window::focus_next`], which is a gesture and a product decision (ADR-006,
    /// 3E) and is **not** taken here. What this change does is make the log
    /// reachable and its retry operable from the keyboard, which is what
    /// `AGENTS.md` §5.2 asks of the feature.
    ///
    /// **This is a real keyboard route and it is not `Tab`.** A keyboard purist
    /// expects `Tab` to move between panes and `Shift+Tab` to move back, and this
    /// client has neither; `Escape` meaning two different things depending on where
    /// focus is is the trade this change makes instead of the one a binding would
    /// have made. **`AGENTS.md` §5.2 is therefore *not* satisfied in full** — see
    /// ADR-006's 3E notes — and nothing in this file claims otherwise.
    ///
    /// **The composer has to *hand focus over* rather than simply drop it, and
    /// that is a measured property of the framework rather than a style
    /// preference.** With no element focused, `Window` routes keys to
    /// `DispatchTree::root_node_id()` (`gpui/src/window.rs:6258`) — and that node
    /// is **not** this view's element, so a window with nothing focused delivers
    /// every key to an empty listener list. A composer that blurred and stopped
    /// would consume `Escape` *and* strand this gesture, breaking `AGENTS.md`
    /// §5.2's keyboard-only requirement with the very key meant to satisfy it.
    /// `InputBar::fallback_focus` is the handle it is given for exactly this, and
    /// `tests/app_shell.rs` asserts both halves: this `Escape` works after the
    /// composer's, and the composer's does not fire it.
    ///
    /// **And this handler still adds no `is_focused` test of its own, which was
    /// the obvious design and the wrong one.** A guard here reading "give up if
    /// the composer has focus" is **unreachable in every state it tests**:
    /// `dispatch_key_down_up_event` walks the bubble path focused-node-first
    /// (`gpui/src/window.rs:6068`) and returns the moment `cx.propagate_event`
    /// is false, so the composer's `stop_propagation` means this function is *not
    /// called at all* while the composer has focus. A condition that can never be
    /// false is a branch nothing tests, and `AGENTS.md` §6.1's "any new warning
    /// fails the build" is the smaller half of why that is a cost. **The same
    /// argument covers the list, and it is why the hand-off below needs no guard
    /// either:** the list's own handler does not claim `escape`, so this function
    /// *is* called with the list focused, and doing the same thing in both cases
    /// is what makes the chain idempotent. The ownership is real, it is enforced
    /// by the framework's own ordering, and
    /// `tests/app_shell.rs::escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail`
    /// plus `escape_chains_from_the_composer_to_the_log_and_its_retry` is what
    /// proves it rather than restating it in a condition.
    ///
    /// **Every other key falls through**, and GPUI's own
    /// `DispatchPhase::Bubble` documentation is why: in the bubble phase
    /// *"keyboard event listeners are invoked from the focused element to the
    /// root of the element tree"*, so this handler runs **after** anything the
    /// focused child handled. That ordering is what lets the composer claim
    /// `Escape` and `Enter` for itself — a shell that swallowed every key would
    /// be a shell that cannot be typed into, and fixing that would mean editing
    /// this function. **It is also the ordering the hand-off relies on**: with the
    /// list focused, this handler runs *after* the list's, so the list gets first
    /// refusal on `escape` and can claim it in future without this function
    /// noticing.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != ESCAPE_KEY {
            return;
        }

        self.list.update(cx, |list, cx| list.follow_tail(cx));

        // The hand-off, and it is unconditional for the reason the section above
        // gives: this handler is reached with the shell root focused *or* with the
        // list focused, and both want the log to be where the keyboard is. A
        // `focus` call on the handle that already holds focus returns before
        // mutating anything (`gpui/src/window.rs:2303`), so the second `escape`
        // costs one comparison and changes nothing.
        //
        // The handle is read out of the list rather than held here, so there is
        // exactly one place in the crate that knows the log's focus target.
        let log = self.list.read(cx).focus_handle(cx);
        log.focus(window, cx);
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every element here is a container rather than a text element, and the
        // colours are still set explicitly. GPUI does not inherit text colour
        // from a parent (AGENTS.md 7.3), the spike established that satisfying
        // the rule unconditionally is cheaper than explaining which elements
        // inherit, and the container's own background is what shows between the
        // list's rows and at a resize. A view added inside this column inherits
        // nothing, and that is the point of setting it.
        div()
            .id("app-shell")
            .key_context("Shell")
            // Records this container's bounds for the headless test, for the same
            // reason the message list records its own.
            .debug_selector(|| "app-shell".to_owned())
            .flex()
            .flex_col()
            .size_full()
            .bg(self.colors.background)
            .text_color(self.colors.text)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.list.clone())
            // The composer, below the list and last in the column, and the order
            // is `PLAN.md` §6's: the log above, the field below, which is what a
            // chat client has looked like since before any of this was a
            // framework. It is not `flex_grow_1` — the list is — so the field
            // takes the height its content needs and the log takes the rest,
            // rather than the two sharing a row of an unbounded column.
            .child(self.input.clone())
    }
}

/// A centred, windowed [`WINDOW_WIDTH`] x [`WINDOW_HEIGHT`] chat window.
///
/// Private because the surface is the point: `bridge.rs` §5 argues that the number
/// of ways to do something is the audit trail, and a second way to build these
/// options would be a second answer to "how big is this window".
fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    }
}

/// Installs the application state and opens the shell in one window.
///
/// **This is the one place the global is installed, and it is here because
/// `AGENTS.md` §3.1 makes this file the global-state owner.** The install happens
/// before [`App::open_window`] is called, not inside its build callback, because
/// a view is built during that callback and a view that reads the state through
/// `bridge::try_read` answers `None` — a client with no state — if the global does
/// not exist yet. [`crate::run`] calls this and then activates the application.
///
/// # Errors
///
/// [`ShNexusError::Unknown`] for both failure modes, and that is the reduction
/// `errors.rs`'s own module documentation prescribes: it names this file as the
/// bootstrap boundary where a GPUI error becomes the project's error type, and
/// [`ShNexusError::Unknown`] as "the landing place for the GPUI boundary
/// conversion".
///
/// - **The window could not be opened.** GPUI returns `anyhow::Result`, and
///   `errors.rs` rule 1 forbids a `From<anyhow::Error>` impl existing anywhere, so
///   the conversion is written here by hand and `anyhow` is never named. The
///   source chain is *not* logged: `errors.rs` assigns that to `tracing::error!`,
///   and `tracing` is not in the workspace, so the reduction is the half of that
///   plan this crate can honour today.
/// - **The state could not be installed.** A refusal here means a global of this
///   type already exists, which no remote peer can cause and this function cannot
///   cause twice — the launch callback runs once per process. It is `Unknown`
///   rather than one of the eight specific variants because none of them is the
///   truth: it is not a network failure, not a rejected credential, and not a
///   user-authored configuration, and `errors.rs` documents that landing in
///   `Unknown` is the visible signal that a variant is missing.
pub fn open(cx: &mut App) -> crate::errors::Result<WindowHandle<Shell>> {
    let sender = bridge::install(cx, UNSIGNED_IN_USER)
        .map_err(|error| ShNexusError::Unknown(error.to_string()))?;

    let options = window_options(cx);
    let opened = cx.open_window(options, move |window, cx| {
        let shell = cx.new(|cx| Shell::new(sender, cx));
        // Focus the composer on open, mirroring how a real client focuses
        // whatever will receive typing. That is now the composer and not the
        // root: a chat window that opens with a text field focused is one the
        // user can type into, and `AGENTS.md` 5.2 requires the feature to be
        // reachable from the keyboard alone -- a window nothing focuses is a
        // window whose keys go nowhere.
        //
        // The root is still focusable and still takes `Escape` (see
        // `Shell::on_key_down`), so nothing is lost by the root not being what
        // the window starts on.
        shell.read(cx).composer_focus_handle(cx).focus(window, cx);
        shell
    });

    opened.map_err(|error| ShNexusError::Unknown(error.to_string()))
}
