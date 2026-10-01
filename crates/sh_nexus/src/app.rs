//! The application shell: root component, global-state owner, theme provider,
//! key handling. `PLAN.md` §4's four items, and nothing else.
//!
//! # 1. The four obligations, and where each one is discharged
//!
//! | `PLAN.md` §4 names | Discharged by |
//! |---|---|
//! | root component | [`Shell`]'s `Render` impl |
//! | global state | [`open`], through [`bridge::install`] |
//! | theme provider | [`theme_colors`], handed to the list through `MessageList::set_colors` |
//! | key handling | [`Shell::on_key_down`] |
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
//! `PLAN.md` §6 asks for *"App shell: sidebar + chat area + input bar"*, and **two
//! of those three are not constructible today**, verified rather than assumed.
//!
//! | Missing | Why it is not here |
//! |---|---|
//! | the channel rail | `actions::set_channels` has zero callers because no `DomainEvent` carries a channel list (`bridge.rs` §5), so a rail would render zero channels, permanently |
//! | the input bar | constructible today through `bridge::try_begin_send`, and it is its own view with its own tests; folding it in here would double the unit and the review slice |
//! | thread panel, search, editing | `docs/ARCHITECTURE.md` ADR-006 places them outside this work |
//! | a `tracing::error!` for a failed open | `errors.rs` assigns this file a source-chain log, `tracing` is not in the workspace, and §7.2's approval process is not this unit's business |
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
use crate::ui::views::message_list::MessageList;
use crate::ui::Colors;
use crate::UNSIGNED_IN_USER;

/// Width of the shell's window, in logical pixels.
///
/// **The same viewport ADR-006 step 6 measured, deliberately.**
/// `benches/frame_time.rs` sizes its own window to this and says why: rows per
/// frame — and therefore any frame-time figure — scale with viewport height, so a
/// differently-sized window would make the pending re-measurement of
/// `docs/BASELINES.md`'s app-level row incomparable with the floor already
/// recorded. The spike's 480x320 is the number `frame_time.rs:255` rejected in
/// writing, and repeating it here would repeat that mistake.
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
const RETURN_TO_TAIL_KEY: &str = "escape";

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
    /// is resolved and handed to the list, the list is pointed at
    /// [`STARTUP_CHANNEL`], and the drain pump is armed. All three are one-time
    /// wiring, and a constructor is the one place a reader can be sure they are
    /// not conditional.
    pub fn new(sender: EventSender, cx: &mut Context<Self>) -> Self {
        let colors = theme_colors();

        let list = cx.new(MessageList::new);
        list.update(cx, |list, cx| {
            // Two calls, one order: the palette first, so the first frame the
            // user ever sees is already themed rather than briefly not.
            list.set_colors(colors, cx);
            list.show_channel(STARTUP_CHANNEL, cx);
        });

        let shell = Self {
            list,
            sender,
            colors,
            focus_handle: cx.focus_handle(),
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
    /// **One key, and the count is the point rather than a shortfall.** There is
    /// no input bar in this shell (see the module docs, §5), so there is no text
    /// field whose keys need consuming, and the one gesture a chat log needs from
    /// the keyboard is the way back to the newest message: ADR-006's table names
    /// *"stick to the newest message"* as a first-class feature of `List`, and
    /// `MessageList::follow_tail` is the half of it that a reader who has scrolled
    /// away needs. `AGENTS.md` §5.2 lists `Escape` among the keys that must work
    /// from the keyboard alone.
    ///
    /// **Not intercepted, and that matters.** Every other key falls through, and
    /// GPUI's own `DispatchPhase::Bubble` documentation is why: in the bubble
    /// phase *"keyboard event listeners are invoked from the focused element to the
    /// root of the element tree"*, so this handler runs **after** anything the
    /// focused child handled and a child that stops propagation is not overridden.
    /// That ordering is what lets the input-bar work unit claim `Escape` for
    /// itself — a shell that swallowed every key would be a shell that cannot be
    /// typed into, and fixing that would mean editing this function.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != RETURN_TO_TAIL_KEY {
            return;
        }
        self.list.update(cx, |list, cx| list.follow_tail(cx));
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
        // Focus the root on open, mirroring how a real client focuses whatever
        // will receive typing. With no input bar yet that is the shell, and
        // `AGENTS.md` 5.2 requires the feature to be reachable from the keyboard
        // alone -- a window nothing focuses is a window whose keys go nowhere.
        shell.read(cx).focus_handle(cx).focus(window, cx);
        shell
    });

    opened.map_err(|error| ShNexusError::Unknown(error.to_string()))
}
