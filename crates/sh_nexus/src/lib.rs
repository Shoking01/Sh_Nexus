//! Sh_Nexus client library root.
//!
//! The binary target (`src/main.rs`) is a thin shim over [`run`]. Everything
//! reusable lives in this library so that integration tests under `tests/` can
//! construct the root view directly -- a binary-only crate cannot be imported
//! by a test, and the headless test in `tests/spike_render.rs` is the
//! load-bearing proof that `gpui_platform/test-support` works on Windows.
//!
//! Phase 1 moves the root view into `src/app.rs` per PLAN.md section 4; it is
//! kept here for the spike so the spike's footprint stays small.
//!
//! # The module tree (AGENTS.md section 3.1)
//!
//! `app.rs`, `ui/`, `db/` and `platform/` are not declared yet. They are later
//! work units, and they are absent rather than declared empty: a module that
//! exists and does nothing reads as finished work. `state/bridge.rs` arrived in
//! work unit 1E-2 and is the single module permitted to call
//! `cx.update_global`; [`run`] installs the global it owns, before any window
//! exists.
//!
//! [`core`] is pure domain logic with no side effects -- no `gpui`, no `tokio`,
//! no I/O (`AGENTS.md` section 3.2). [`network`] is protocol handling only: it
//! parses wire formats into [`core::models`] and emits
//! [`core::models::DomainEvent`] values, and never touches GPUI state
//! directly. [`state`] holds the application's data and the only decisions about
//! changing it. [`errors`] holds the project's global error type.

// `AGENTS.md` section 2.2 requires `///` doc comments on all public items and
// section 5.1 makes it a pre-commit gate. The lint is how the gate is enforced
// rather than remembered: an undocumented public item is a compile warning, so it
// cannot reach a commit unnoticed. Kept in step with the same lint in
// `sh_nexus_wire/src/lib.rs`.
#![warn(missing_docs)]

pub mod core;
pub mod errors;
pub mod network;
pub mod state;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle, Focusable, Render, Window,
    WindowBounds, WindowOptions,
};

/// Width of the root window, in logical pixels.
pub const WINDOW_WIDTH: f32 = 480.0;
/// Height of the root window, in logical pixels.
pub const WINDOW_HEIGHT: f32 = 320.0;

/// The spike's root view.
///
/// Deliberately trivial. Its only job is to prove that on Windows we can
/// register a root view, lay elements out, hit-test a pointer click onto an
/// element, and deliver a keyboard event to a focused element.
pub struct RootView {
    /// Incremented by the clickable button; asserted on by the spike test.
    count: u32,
    /// The key of the last key-down event received; asserted on by the spike
    /// test to prove keyboard simulation reaches the element tree.
    last_key: Option<String>,
    /// Focus target for the keyboard half of the test.
    focus_handle: FocusHandle,
}

impl RootView {
    /// Builds the root view and creates its focus handle.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            count: 0,
            last_key: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The current click count. Exposed so tests need not reach into private
    /// state, and so `clippy` sees the field as read.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The key of the last key-down event received, if any.
    pub fn last_key(&self) -> Option<&str> {
        self.last_key.as_deref()
    }

    /// Gives this view keyboard focus, so key events are dispatched to it.
    ///
    /// Exists for the headless test, which focuses through
    /// `Entity::update_in` so the keyboard path has a focus target. `run()`
    /// does the same thing inline via `Focusable::focus_handle`, because there
    /// the view is being constructed and is not yet bound to a variable.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // AGENTS.md 7.3: GPUI does not inherit text colour from a parent, so
        // `.text_color()` is set explicitly on EVERY element below, containers
        // included. The spike did not empirically test which elements inherit
        // and which do not -- it just satisfies the rule unconditionally, which
        // is what the rule asks for.
        div()
            .id("root")
            .key_context("RootView")
            .flex()
            .flex_col()
            .gap_3()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.last_key = Some(event.keystroke.key.clone());
                cx.notify();
            }))
            .child(
                div()
                    .id("label")
                    // Records this element's bounds under the key "label" so the
                    // headless test can assert on real layout. `.id()` does NOT do
                    // this -- `debug_selector` (from the `InteractiveElement`
                    // trait, in `gpui::prelude`) is a separate call, and it is a
                    // no-op unless `gpui/test-support` is enabled.
                    .debug_selector(|| "label".to_owned())
                    .text_lg()
                    // Explicit, per AGENTS.md 7.3.
                    .text_color(rgb(0xcdd6f4))
                    .child(format!("Sh_Nexus spike - count: {}", self.count)),
            )
            .child(
                div()
                    .id("increment")
                    .debug_selector(|| "increment".to_owned())
                    .px_4()
                    .py_2()
                    .bg(rgb(0x313244))
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(rgb(0xcdd6f4))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.count += 1;
                        cx.notify();
                    }))
                    .child("Increment"),
            )
            .child(
                div()
                    .id("last-key")
                    .debug_selector(|| "last-key".to_owned())
                    .text_sm()
                    .text_color(rgb(0x7f849c))
                    .child(format!(
                        "last key: {}",
                        self.last_key.as_deref().unwrap_or("<none>")
                    )),
            )
    }
}

/// The `self_user_id` this client runs with before authentication exists.
///
/// **A named placeholder rather than a plausible-looking id, and that is the
/// whole point of the name.** `AppState`'s unread rule excludes messages whose
/// author is `self_user_id` (`state/app_state.rs` module docs §5, condition 3),
/// so a real-looking value would make the client quietly treat *somebody's*
/// messages as its own. This string is greppable, appears in no fixture, and is
/// replaced by the signed-in user when `network/auth.rs` arrives in Phase 4.
///
/// **The cost of the placeholder, stated rather than hidden:** until then
/// nothing this client shows can be recognised as its own, so the unread rule
/// over-counts. That is the safe direction -- a badge that over-reports is a
/// badge the user dismisses, and an under-report is a message nobody reads.
pub const UNSIGNED_IN_USER: &str = "u_unsigned_in";

/// The window options the spike opens with: a centred, windowed 480x320.
fn spike_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    }
}

/// Starts the GPUI application and opens the single root window.
///
/// **The application state is installed here, and this is the one place in the
/// binary that calls [`state::bridge::install`].** `PLAN.md` §4 makes
/// `state/bridge.rs` the single owner of `cx.update_global`; registering the
/// global it owns is the first half of that, and doing it inside the launch
/// callback means the state exists before any window — and therefore before any
/// view — can be built. The `EventSender` it returns is dropped here on purpose:
/// `network/`'s WebSocket client is Phase 4, and the sender's only producer is
/// a socket task that does not exist yet. **When that task lands, it is handed
/// this sender, and the handle has to be *kept* rather than dropped** — a
/// dropped sender closes the inbox and every later delivery is reported as
/// [`state::bridge::DeliveryRefusal::BridgeDropped`].
///
/// # Errors
///
/// Returns the platform error from `App::open_window` (window creation, D3D11
/// device creation, or text-system initialisation). `Application::run` blocks
/// until the platform event loop exits and its launch callback cannot return a
/// value, so the failure is carried out through an `Rc<RefCell<_>>` and
/// surfaced after the loop returns. No `unwrap`/`expect` is used on this path
/// (AGENTS.md 2.1).
pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // The launch callback passed to `Application::run` is `FnOnce(&mut App)` and
    // must be `'static`, so it cannot capture a `&mut`.
    let failure: Rc<RefCell<Option<Box<dyn std::error::Error + Send + Sync>>>> =
        Rc::new(RefCell::new(None));
    let failure_in_callback = Rc::clone(&failure);

    gpui_platform::application().run(move |cx: &mut App| {
        // A second install is impossible here -- the launch callback runs once
        // per process -- but the refusal is surfaced rather than unwrapped, per
        // AGENTS.md 2.1, and a client with no application state has nothing worth
        // showing, so the window is not opened at all.
        if let Err(error) = state::bridge::install(cx, UNSIGNED_IN_USER) {
            *failure_in_callback.borrow_mut() = Some(Box::new(error));
            return;
        }

        let options = spike_window_options(cx);
        let opened = cx.open_window(options, |window, cx| {
            let view = cx.new(RootView::new);
            // Focus the root so `simulate_keystrokes` has somewhere to deliver
            // to, mirroring how a real app focuses its input bar on open.
            view.focus_handle(cx).focus(window, cx);
            view
        });

        match opened {
            Ok(_) => cx.activate(true),
            Err(error) => *failure_in_callback.borrow_mut() = Some(error.into()),
        }
    });

    // Bound to a local first so the `RefMut` temporary is dropped before
    // `failure` itself goes out of scope at the end of the block.
    let startup_error = failure.borrow_mut().take();
    match startup_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
