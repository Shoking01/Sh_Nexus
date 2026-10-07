//! Sh_Nexus client library root.
//!
//! The binary target (`src/main.rs`) is a thin shim over [`run`]. Everything
//! reusable lives in this library so that integration tests under `tests/` can
//! construct the root view directly -- a binary-only crate cannot be imported
//! by a test, and the headless test in `tests/spike_render.rs` is the
//! load-bearing proof that `gpui_platform/test-support` works on Windows.
//!
//! # The module tree (AGENTS.md section 3.1)
//!
//! [`app`] is the root component and arrived with work unit 3A; [`run`] opens it.
//! `db/` is not declared yet. It is a later work unit, and it is absent rather
//! than declared empty: a module that exists and does nothing reads as finished
//! work. [`platform`] arrived with the keychain milestone and is the project's
//! first OS-facing layer, behind the [`platform::TokenStore`] trait `AGENTS.md`
//! L88 describes. `state/bridge.rs` arrived in work unit 1E-2
//! and is the single module permitted to mutate a GPUI context, which is why
//! [`app::open`] installs the global it owns and this file names no such call.
//! [`ui`] arrived in work unit 2A with the message list, and it is the layer
//! GPUI is confined to (`PLAN.md` section 4) -- [`app`] being the one place
//! outside that layer that composes it.
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

pub mod app;
pub mod core;
pub mod errors;
pub mod network;
pub mod platform;
pub mod state;
pub mod ui;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{div, prelude::*, rgb, App, Context, FocusHandle, Focusable, Render, Window};

/// The spike's root view.
///
/// **Deliberately trivial, and deliberately no longer what [`run`] opens.** Its
/// only job is to prove that on Windows we can register a root view, lay elements
/// out, hit-test a pointer click onto an element, and deliver a keyboard event to
/// a focused element. The application itself is [`app::Shell`], and
/// [`app::open`] is what a launch callback calls.
///
/// **It is retained for `tests/spike_render.rs` and must not be deleted.** That
/// test is the load-bearing proof that `gpui_platform/test-support` works on
/// Windows, and this view is the subject it renders: a shell that composes a
/// virtualized `gpui::List` is a *much* stronger harness claim, so if the list
/// ever fails to lay out headlessly the spike would no longer notice, and
/// `PLAN.md` section 8 makes the spike a hard gate.
///
/// **The window options that used to accompany it went with [`run`].** They
/// described the spike's 480x320 window, which nothing opens any more; the
/// shell's geometry lives in [`app::WINDOW_WIDTH`] and [`app::WINDOW_HEIGHT`],
/// beside the view it describes.
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
    /// `Entity::update_in` so the keyboard path has a focus target. Nothing in
    /// `src/` calls this any more -- [`app::open`] focuses the shell inline via
    /// `Focusable::focus_handle`, because there the view is being constructed and
    /// is not yet bound to a variable -- so the method exists for the test alone.
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
///
/// **The one production caller of this placeholder is [`app::open`].** It stays
/// here rather than moving beside the shell because `benches/frame_time.rs`
/// installs its own state with it, and a constant that two independent pieces of
/// startup code reach for belongs at the crate root rather than in the view that
/// happens to be first today.
pub const UNSIGNED_IN_USER: &str = "u_unsigned_in";

/// Starts the GPUI application and opens the shell's single window.
///
/// **This function opens a window and nothing else.** Installing the application
/// state, keeping the producer handle alive, resolving the theme, arming the drain
/// schedule and handling keys are [`app`]'s obligations (`AGENTS.md` §3.1), and
/// [`app::open`] is the one call that does them in the order they require: the
/// state is registered *before* a window exists, so no view is ever built against
/// a global that is not there yet.
///
/// **The failure is carried out of the launch callback rather than returned from
/// it.** The callback `Application::run` is given is `FnOnce(&mut App)` and cannot
/// return a value, so the error travels through an `Rc<RefCell<_>>` and is
/// surfaced after the event loop returns — the same shape `benches/frame_time.rs`
/// uses, and for the same reason.
///
/// # Errors
///
/// [`app::open`]'s error: the application state could not be installed, or the
/// window could not be created (window creation, D3D11 device creation, or
/// text-system initialisation). It arrives here already reduced to
/// [`errors::ShNexusError`] at the one boundary `errors.rs` permits. No
/// `unwrap`/`expect` is used on this path (AGENTS.md 2.1).
pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // The launch callback passed to `Application::run` is `FnOnce(&mut App)` and
    // must be `'static`, so it cannot capture a `&mut`.
    let failure: Rc<RefCell<Option<Box<dyn std::error::Error + Send + Sync>>>> =
        Rc::new(RefCell::new(None));
    let failure_in_callback = Rc::clone(&failure);

    gpui_platform::application().run(move |cx: &mut App| {
        // Both failure modes are the same shape here: a client with no state and
        // a client with no window have nothing worth showing, so `app::open`
        // refuses to open one and the reason is reported rather than unwrapped
        // (AGENTS.md 2.1).
        match app::open(cx) {
            Ok(_) => cx.activate(true),
            Err(error) => *failure_in_callback.borrow_mut() = Some(Box::new(error)),
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
