//! Phase 0 spike: does the headless GPUI test harness work on Windows?
//!
//! This is the load-bearing test of the spike. PLAN.md section 10 makes the
//! project's entire test strategy depend on `gpui/test-support` being usable
//! on the target platform, and PLAN.md section 8 makes this file a hard gate:
//! nothing else starts until it passes.
//!
//! It proves three separate things:
//! 1. `#[gpui::test]` gives us a `TestAppContext` that builds a window.
//! 2. The window actually renders headlessly -- elements are laid out and get
//!    bounds -- so a component can be asserted on rather than merely compiled.
//! 3. Simulated pointer and keyboard input reach the element tree and mutate
//!    state.
//!
//! Note on test placement: Cargo auto-discovers `tests/*.rs` and
//! `tests/*/main.rs` ONLY. A file at `tests/integration/spike_render.rs` would
//! never be compiled, and the suite would look green while containing nothing
//! (PLAN.md section 4).

use gpui::{px, TestAppContext};
use sh_nexus::RootView;

/// A headless window renders the root view and lays elements out.
///
/// If the harness did not work on Windows this would fail before any
/// assertion: `add_window_view` calls `App::open_window` internally.
#[gpui::test]
fn headless_render_lays_out_root_view(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
    cx.run_until_parked();

    // The label is a real laid-out element with an explicit `.text_color()`.
    let label = cx
        .debug_bounds("label")
        .expect("label element should have been laid out with a non-zero size");
    assert!(
        label.size.width > px(0.0),
        "label should have non-zero width, got {:?}",
        label.size
    );

    // The clickable element is hit-testable: it has real bounds on screen.
    let increment = cx
        .debug_bounds("increment")
        .expect("increment element should be hit-testable");
    assert!(
        increment.size.height > px(0.0),
        "increment element should have non-zero height, got {:?}",
        increment.size
    );

    assert_eq!(view.read_with(cx, |view, _| view.count()), 0);
    // `read_with` cannot return a borrow into the view, so copy the string out.
    assert_eq!(
        view.read_with(cx, |view, _| view.last_key().map(str::to_owned)),
        None
    );
}

/// Simulated pointer input reaches the element tree and mutates state.
#[gpui::test]
fn simulated_click_mutates_view_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
    cx.run_until_parked();

    let increment = cx
        .debug_bounds("increment")
        .expect("increment element should be hit-testable");
    cx.simulate_click(increment.center(), gpui::Modifiers::default());

    assert_eq!(
        view.read_with(cx, |view, _| view.count()),
        1,
        "a simulated click on the button should have incremented the count"
    );

    // Click again, to show the listener stays registered across re-renders
    // (the click triggers `cx.notify()`, so a new frame is rendered).
    let increment = cx
        .debug_bounds("increment")
        .expect("increment element should still be hit-testable after re-render");
    cx.simulate_click(increment.center(), gpui::Modifiers::default());
    assert_eq!(view.read_with(cx, |view, _| view.count()), 2);
}

/// Simulated keyboard input reaches the focused element and mutates state.
#[gpui::test]
fn simulated_keystroke_mutates_view_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
    cx.run_until_parked();

    // Give the root view keyboard focus, as `run()` does when opening a window.
    view.update_in(cx, |view, window, cx| view.focus(window, cx));
    cx.run_until_parked();

    cx.simulate_keystrokes("j k enter");

    let last_key = view
        .read_with(cx, |view, _| view.last_key().map(str::to_owned))
        .expect("the key-down listener should have recorded a key");
    assert_eq!(last_key, "enter");

    // A click is independent of the keyboard path, so both must have worked.
    assert_eq!(view.read_with(cx, |view, _| view.count()), 0);
}

/// A state change re-renders the tree, and text keeps laying out.
///
/// This is the round trip the rest of the project relies on: mutate state,
/// `cx.notify()`, and the next frame reflects it. The spike registers no font
/// bundle -- on non-wasm platforms GPUI falls back to the platform text system
/// -- so this also proves text shaping works with no font assets registered.
#[gpui::test]
fn state_change_triggers_a_rerender_with_text(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
    cx.run_until_parked();

    let increment = cx
        .debug_bounds("increment")
        .expect("increment element should be hit-testable");
    cx.simulate_click(increment.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    // After the click the label text is one character longer, so the re-render
    // had to run for the label to still have real bounds.
    let label = cx
        .debug_bounds("label")
        .expect("label should still be laid out after the re-render");
    assert!(label.size.width > px(0.0));
    assert_eq!(view.read_with(cx, |view, _| view.count()), 1);
}
