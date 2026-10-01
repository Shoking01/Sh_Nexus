//! The application shell, headless: `src/app.rs`, its pump, and its palette.
//!
//! `PLAN.md` §4's shell is four obligations — root component, global state,
//! theme provider, key handling — and this file is the evidence for all four,
//! driven through the production path only. There is no mock and no test-only
//! seam: `bridge::install`, real `DomainEvent`s through a real `EventSender`,
//! the shell's own `cx.spawn` pump, a real window. The standard
//! `tests/bridge.rs` holds itself to, for the same reason — a test that wires
//! the layers together differently from the application proves something about
//! the test.
//!
//! # What is checked, and what kind of evidence each claim rests on
//!
//! | Claim | Test | Kind |
//! |---|---|---|
//! | The shell renders, and the view it composes is the production `MessageList` | [`the_shell_renders_the_production_message_list`] | runtime, real window, real bounds |
//! | The retained sender accepts a delivery instead of refusing it | [`the_shell_keeps_the_sender_alive_so_delivery_is_not_refused`] | runtime |
//! | The pump is on the shell's schedule, not the frame loop's | [`nothing_is_applied_before_the_first_tick`] | runtime, negative |
//! | One tick applies **every** queued event, and the rows are on screen | [`one_tick_applies_every_queued_event_and_puts_the_rows_on_screen`] | runtime, parameterised |
//! | The applied palette is the active built-in theme, all the way to the rows | [`the_shell_applies_the_active_theme_to_the_rows`] | runtime + parsed document |
//! | The shell's one key reaches the list | [`escape_returns_the_list_to_the_newest_message`] | runtime, simulated keystroke |
//!
//! # Why the pump is driven by the clock rather than by `run_until_parked`
//!
//! **A pump that re-arms its own timer cannot be tested by parking**, and knowing
//! that before writing the test is worth stating: on the pinned `rev e683fd7`
//! `TestScheduler::run` is `while step() {}` with no clock advancement, so
//! `run_until_parked` returns with the pump's timer still in the future — but
//! `advance_clock` walks the clock forward to the next expiry and polls the woken
//! task (`crates/scheduler/src/test_scheduler.rs`, `advance_clock`). So a single
//! `advance_clock(DRAIN_INTERVAL)` fires the pump **once** and the next pump tick
//! is one interval further out. That is what makes the real schedule testable at
//! all, rather than a hand-called helper that happens to share its body.
//!
//! # Test placement
//!
//! Cargo auto-discovers `tests/*.rs` only. A file at `tests/ui/app_shell.rs` would
//! never be compiled and this suite would look green while containing nothing
//! (`PLAN.md` §4).

use chrono::{DateTime, TimeZone, Utc};
use gpui::{px, Entity, Focusable, TestAppContext, VisualTestContext};
use rstest::rstest;
use sh_nexus::app::{self, Shell, DRAIN_INTERVAL, STARTUP_CHANNEL};
use sh_nexus::core::models::events::DomainEvent;
use sh_nexus::core::models::message::Message;
use sh_nexus::core::theme::BuiltIn;
use sh_nexus::state::bridge::{self, Delivery, EventSender};
use sh_nexus::ui::Colors;
use sh_nexus::UNSIGNED_IN_USER;
use smallvec::SmallVec;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A colleague, so an arriving message is somebody else's.
///
/// **Deliberately not [`UNSIGNED_IN_USER`]**, the identity the shell installs
/// with. A message authored by the client's own placeholder would set
/// `is_self` on the row and would exercise the self-bubble branch, which is
/// `message_row.rs`'s business and not this file's question.
const THEM: &str = "u_them";

/// How many messages the tail-following test needs before "scrolled away from
/// the bottom" means anything.
///
/// `add_window_view` maximizes the window, and `gpui::List` re-engages following
/// on its own when the scroll position *is* the bottom
/// (`gpui/src/elements/list.rs:1211-1218`), so a pause with nothing scrolled is
/// a pause the next layout is entitled to undo. `tests/ui_message_list.rs` uses
/// the same count for the same reason.
const ENOUGH_TO_OVERFLOW: u128 = 40;

/// The burst sizes the drain test runs, and why these three.
///
/// **One event is the case a per-wakeup pump passes**, and the second two are the
/// cases it fails: two is more than a tick's worth of noise would suggest, and
/// sixteen is more than a screenful so a pump that drained "enough to fill the
/// viewport" would still be wrong. The sizes are a `const` rather than three
/// `#[rstest]` cases because this test needs a `TestAppContext`, and the harness
/// that provides one is `#[gpui::test]` -- the two attribute macros cannot be
/// stacked, and rebuilding the harness by hand inside a test is exactly the
/// "a test wired differently from the application proves something about the
/// test" this file's module docs rule out. `AGENTS.md` §4.3's `rstest` is used
/// below, where the property under test needs no window.
const BURST_SIZES: [u128; 3] = [1, 2, 16];

/// A timestamp the caller supplies, since `state/` may not read a clock.
fn at(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_000_000 + second, 0)
        .single()
        .expect("the test timestamp is in range")
}

/// A deterministic identity, since no `Uuid` is generated in production yet.
fn cid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// A stored message in [`STARTUP_CHANNEL`], as the server would deliver it.
fn stored(client_msg_id: u128, content: &str, second: i64) -> Message {
    Message {
        id: format!("m_{client_msg_id}"),
        client_msg_id: cid(client_msg_id),
        channel_id: STARTUP_CHANNEL.to_owned(),
        user_id: THEM.to_owned(),
        content: content.to_owned(),
        timestamp: at(second),
        edited_at: None,
        reactions: SmallVec::new(),
        thread_id: None,
        attachments: SmallVec::new(),
    }
}

/// Installs the application state and returns the producer handle.
///
/// **With the crate's real placeholder identity rather than a fixture's**, because
/// the identity is part of what is under test: the unread rule in
/// `state/app_state.rs` module docs §5 excludes this client's own messages, and a
/// fixture identity would quietly assert a different rule than the one
/// `app::open` installs under.
fn installed(cx: &mut TestAppContext) -> EventSender {
    cx.update(|cx| bridge::install(cx, UNSIGNED_IN_USER))
        .expect("the first install must succeed")
}

/// Installs the state and builds the shell in a real headless window.
///
/// **The two calls are the production order, and that is the point:**
/// `app::open` installs before a window exists, so a shell built against an
/// absent global would render an empty list and satisfy half the assertions in
/// this file for the wrong reason.
fn shell(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let sender = installed(cx);
    cx.add_window_view(move |_, cx| Shell::new(sender, cx))
}

/// Hands one event to the shell's producer handle and asserts it was accepted.
fn queued(cx: &VisualTestContext, shell: &Entity<Shell>, event: DomainEvent) {
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.sender().deliver(event)),
        Delivery::Queued,
        "the shell holds the producer handle for the life of the window, so the \
         inbox it belongs to is still open"
    );
}

/// Lets the shell's drain pump run once.
///
/// **Through `advance_clock` and not `run_until_parked`, and the reason is a
/// property of the pinned scheduler.** `TestScheduler::run` is `while step() {}`
/// with no clock advancement, so `run_until_parked` returns with the pump's timer
/// still in the future and never fires it; `advance_clock` walks the clock forward
/// to the next expiry and polls the woken task
/// (`crates/scheduler/src/test_scheduler.rs`, `advance_clock`). One call therefore
/// runs the pump **once**, and the next tick lands one interval further out — so
/// this is the shell's real schedule under test rather than a helper that shares
/// its body.
fn tick(cx: &VisualTestContext) {
    cx.executor().advance_clock(DRAIN_INTERVAL);
    cx.run_until_parked();
}

/// How many messages the shell's list is showing.
fn showing(cx: &VisualTestContext, shell: &Entity<Shell>) -> usize {
    shell.read_with(cx, |shell, app| {
        shell.list().read_with(app, |list, _| list.item_count())
    })
}

/// Whether the shell's list is snapping to the newest message.
fn following_tail(cx: &VisualTestContext, shell: &Entity<Shell>) -> bool {
    shell.read_with(cx, |shell, app| {
        shell
            .list()
            .read_with(app, |list, _| list.is_following_tail())
    })
}

// ---------------------------------------------------------------------------
// 1. The root component
// ---------------------------------------------------------------------------

/// The shell lays out, and the view it composes is the production
/// [`MessageList`](sh_nexus::ui::views::message_list::MessageList).
///
/// **Both halves are asserted, and the second is the one that could pass for the
/// wrong reason.** `debug_bounds` asks the window, so a non-zero size proves the
/// elements were *painted* rather than merely built (Phase 0's finding 2: it only
/// answers because the elements carry `.debug_selector()`). But a shell that
/// composed some other view would also lay out and paint, so the assertion that
/// makes this the shell is that the entity it holds is a `MessageList` already
/// pointed at the startup channel — which is what `Shell::new` wires, and what a
/// stub standing in for the list would not be.
#[gpui::test]
fn the_shell_renders_the_production_message_list(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    let container = cx
        .debug_bounds("app-shell")
        .expect("the shell's root element must have recorded its bounds");
    assert!(
        container.size.height > px(0.),
        "the shell must occupy the window, got {:?}",
        container.size
    );

    let list = cx
        .debug_bounds("message-list")
        .expect("the message list the shell composes must be on screen");
    assert!(
        list.size.height > px(0.),
        "the list must occupy space inside the shell, got {:?}",
        list.size
    );
    assert!(
        list.size.width <= container.size.width,
        "the list is a child of the shell, so it cannot be wider than it: \
         list {list:?} inside container {container:?}"
    );

    let channel = shell.read_with(cx, |shell, app| {
        shell
            .list()
            .read_with(app, |list, _| list.channel().map(str::to_owned))
    });
    assert_eq!(
        channel,
        Some(STARTUP_CHANNEL.to_owned()),
        "the view behind those bounds is the production list, already pointed at \
         the channel the shell opens on"
    );
}

// ---------------------------------------------------------------------------
// 2. The retained producer handle
// ---------------------------------------------------------------------------

/// The shell keeps the sender `install` returned, so a delivery is queued rather
/// than refused as dropped.
///
/// **This is the defect the shell exists to fix, and the assertion is the exact
/// one it would fail before.** A dropped `EventSender` closes the inbox, and every
/// later `deliver` answers
/// [`DeliveryRefusal::BridgeDropped`](sh_nexus::state::bridge::DeliveryRefusal)
/// with the event handed back. `src/lib.rs` used to install and discard the handle
/// on the same line, so this test is the difference between a shell that will
/// accept Phase 4's socket events and one that refuses every one of them.
///
/// **The second half asserts the inbox is open from the inside**, which is a
/// different fact from the first: `deliver` reports what `try_send` saw, and a
/// `Disconnected` drain is the receiving end agreeing. Both are checked because
/// the bug is one-sided — the sender can be alive while the receiver is gone, and
/// only the drain's own flag says so.
#[gpui::test]
fn the_shell_keeps_the_sender_alive_so_delivery_is_not_refused(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    queued(
        cx,
        &shell,
        DomainEvent::MessageReceived(stored(1, "delivered to a live handle", 1)),
    );

    let report = cx
        .update(|_window, cx| bridge::drain(cx))
        .expect("the state is installed");
    assert!(
        !report.is_disconnected(),
        "the receiving end must still have a live producer: the shell holds one, \
         so the inbox is open rather than closed and empty"
    );
    assert_eq!(
        report.delivered(),
        1,
        "and the event was taken off the inbox"
    );
}

// ---------------------------------------------------------------------------
// 3. The drain schedule
// ---------------------------------------------------------------------------

/// Nothing is applied until the shell's pump ticks.
///
/// **The negative half of the pump's contract, and the half a test that only
/// checks "the message arrived" cannot distinguish.** If the drain happened on the
/// frame loop — or synchronously inside `deliver` — the message would already be
/// on screen here, and a shell that drained on no schedule at all would pass the
/// same assertion. So the message must be *queued*, visible to nobody, until the
/// clock reaches one interval.
#[gpui::test]
fn nothing_is_applied_before_the_first_tick(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    queued(
        cx,
        &shell,
        DomainEvent::MessageReceived(stored(1, "waiting for the pump", 1)),
    );
    cx.run_until_parked();

    assert_eq!(
        showing(cx, &shell),
        0,
        "a queued event is applied by the pump, not by the frame loop: delivery \
         and application are separate steps and the separation is the seam's whole \
         point"
    );

    tick(cx);

    assert_eq!(
        showing(cx, &shell),
        1,
        "one interval later the pump has drained it, and the list reconciled itself \
         without anybody calling `sync`"
    );
}

/// One tick applies every event that was queued, and the rows reach the screen.
///
/// **A drain takes everything on the inbox, not one event per tick**, so this is
/// asserted across growing bursts rather than once: a pump that drained a single
/// event per wakeup would show `1` after the first step and fall behind on every
/// one after it, and `AGENTS.md` §7.5's "never a silent drop" is exactly that
/// shape. **The bursts accumulate**, so the assertion is that a tick takes
/// everything queued since the previous one — which is the property — rather than
/// "the total happens to be right", which a pump that only ever applied the last
/// event would also satisfy.
///
/// **One shell and one install for the whole test**, because a global is
/// registered once per application and `bridge::install` refuses the second call
/// with `InstallError::AlreadyInstalled` — which is correct, and is exactly why
/// installing per iteration is not available as a way to get a fresh window.
///
/// **The row bounds are the load-bearing half.** `item_count` is bookkeeping the
/// list updates itself; a non-zero `message-row` rectangle is a row that was laid
/// out and painted, which on Windows without a headless renderer
/// (`current_headless_renderer()` answers `None`) is the strongest evidence
/// available that the event reached something the user could see.
#[gpui::test]
fn one_tick_applies_every_queued_event_and_puts_the_rows_on_screen(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    let mut delivered = 0_u128;
    for burst in BURST_SIZES {
        for n in 0..burst {
            let id = delivered + n;
            queued(
                cx,
                &shell,
                DomainEvent::MessageReceived(stored(
                    id,
                    "a body long enough to be a row",
                    id as i64,
                )),
            );
        }
        delivered += burst;

        assert_eq!(
            showing(cx, &shell),
            (delivered - burst) as usize,
            "nothing is applied until the pump ticks, even with {burst} events \
             queued ahead of it"
        );

        tick(cx);

        assert_eq!(
            showing(cx, &shell),
            delivered as usize,
            "one drain must take every event it found: the inbox is a queue of \
             owned values, and applying one of them per wakeup would fall {} \
             events behind after a burst of {burst}",
            burst - 1
        );

        let row = cx
            .debug_bounds("message-row")
            .expect("a row element must have recorded its bounds");
        assert!(
            row.size.height > px(0.),
            "with {delivered} messages applied, at least one row must have been \
             laid out with a non-zero height, got {:?}",
            row.size
        );
    }
}

// ---------------------------------------------------------------------------
// 4. The theme provider
// ---------------------------------------------------------------------------

/// The palette the shell applies is the active built-in theme, all the way to the
/// rows.
///
/// **Three links in one assertion, because each could be broken alone and a
/// broken link is invisible:** the resolved [`Colors`] must be the parsed palette of
/// [`BuiltIn::Dark`] (so `core/theme.rs`'s parser, not a hand-written literal, is
/// what the client draws with), the shell must hold it, and a *recycled row* must
/// hold it too.
///
/// **The row is the half that needed a new method to make possible.** A row caches
/// its own `Colors`, so a palette that stopped at the list would repaint the
/// container and leave every row on the theme it was built under — which
/// `AGENTS.md` §7.3's rule is about, and which no container assertion would
/// notice. `RowCache::row` pushes the palette into the rows it hands back, and this
/// is the test that says so.
#[gpui::test]
fn the_shell_applies_the_active_theme_to_the_rows(cx: &mut TestAppContext) {
    let theme = BuiltIn::Dark
        .theme()
        .expect("the built-in dark theme must validate; tests/theme.rs asserts this too");
    let expected = Colors::from_palette(theme.colors());

    assert_eq!(
        app::theme_colors(),
        expected,
        "the shell's theme provider resolves the active built-in theme's own \
         parsed palette"
    );
    assert_eq!(
        app::ACTIVE_THEME,
        BuiltIn::Dark,
        "and the active theme is the one AGENTS.md 10.2 requires compiled in"
    );

    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    assert_eq!(
        shell.read_with(cx, |shell, _| shell.colors()),
        expected,
        "the shell's own elements draw with the resolved palette"
    );
    assert_eq!(
        shell.read_with(cx, |shell, app| shell
            .list()
            .read_with(app, |list, _| list.colors())),
        expected,
        "and the list it composes draws with the same one, which is what \
         `MessageList::set_colors` exists to make possible"
    );

    // A row first, so there is a row whose palette could have gone stale.
    queued(
        cx,
        &shell,
        DomainEvent::MessageReceived(stored(1, "a row that must be themed", 1)),
    );
    tick(cx);
    assert_eq!(
        showing(cx, &shell),
        1,
        "the row the theme assertion needs must exist"
    );

    let row_palette = shell.read_with(cx, |shell, app| {
        shell.list().read_with(app, |list, _| {
            list.retained_row(&cid(1))
                .map(|row| row.read_with(app, |row, _| row.colors()))
        })
    });
    assert_eq!(
        row_palette,
        Some(expected),
        "the recycled row must hold the resolved palette too, not the one it was \
         constructed with: AGENTS.md 7.3 is about the text colour a row draws with"
    );
}

/// Every built-in theme resolves to a palette the renderer can draw with, so
/// changing [`app::ACTIVE_THEME`] is a one-line change rather than a risk.
///
/// **Parameterised over `BuiltIn::ALL` because only one of the three is active
/// today and the other two are the ones nobody has looked at.** §10.2 requires all
/// three to be compiled in, and `AGENTS.md` §7.3's rule is about the colour a
/// row actually draws with — so the property asserted here is that every field the
/// projection fills is fully opaque, which `#rrggbb` guarantees and which a
/// translucent text colour would turn into invisible text with no other symptom.
///
/// **Windowless on purpose.** This is a property of the projection
/// [`app::theme_colors`] is written in terms of; opening a window to inspect a
/// [`Colors`] would buy nothing, and the shell's own wiring is asserted above.
#[rstest]
#[case(BuiltIn::Dark)]
#[case(BuiltIn::Light)]
#[case(BuiltIn::HighContrast)]
fn the_active_theme_can_be_any_built_in_theme_without_a_translucent_colour(
    #[case] built_in: BuiltIn,
) {
    assert!(
        BuiltIn::ALL.contains(&built_in),
        "AGENTS.md 10.2 compiles exactly three themes in and the parameterisation \
         must not grow a case the shell cannot resolve"
    );

    let theme = built_in
        .theme()
        .expect("tests/theme.rs's every_built_in_theme_parses_and_validates pins this");
    let colors = Colors::from_palette(theme.colors());

    for (role, channel) in [
        ("background", colors.background),
        ("surface", colors.surface),
        ("text", colors.text),
        ("text_muted", colors.text_muted),
        ("accent", colors.accent),
        ("danger", colors.danger),
        ("mention", colors.mention),
        ("code_block_bg", colors.code_block_bg),
        ("bubble_self", colors.bubble_self),
        ("bubble_other", colors.bubble_other),
    ] {
        assert_eq!(
            channel.a,
            1.0,
            "{} in {} resolved to a translucent colour, and the theme schema has \
             no alpha slot -- a text colour with alpha is invisible text",
            role,
            built_in.id()
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Key handling
// ---------------------------------------------------------------------------

/// `Escape` returns the list to the newest message, and only `Escape` does.
///
/// **The gesture has to be set up before it can be asserted, and the setup is the
/// subtle part.** `MessageList::pause_following_tail` suspends following *without*
/// leaving `Tail` mode, and `gpui::List` re-engages it on its own when the scroll
/// position is the bottom — so a pause with nothing scrolled away is a pause the
/// next layout is entitled to undo, and a test that only paused would assert
/// nothing. The scroll to the top is what makes "away from the bottom" real.
///
/// **The shell must be focused for the key to arrive at all**, which is why
/// `app::open` focuses the root on open: `AGENTS.md` §5.2 requires the feature to
/// work from the keyboard alone, and a window nothing focuses is a window whose
/// keys go nowhere.
#[gpui::test]
fn escape_returns_the_list_to_the_newest_message(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);

    for n in 0..ENOUGH_TO_OVERFLOW {
        queued(
            cx,
            &shell,
            DomainEvent::MessageReceived(stored(n, "a body long enough to be a row", n as i64)),
        );
    }
    tick(cx);
    cx.run_until_parked();
    assert!(
        following_tail(cx, &shell),
        "a chat log opens on the newest message"
    );

    shell.update_in(cx, |shell, _window, cx| {
        shell.list().update(cx, |list, cx| {
            list.pause_following_tail(cx);
            list.list_state().scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
        });
    });
    cx.run_until_parked();
    assert!(
        !following_tail(cx, &shell),
        "the reader has scrolled away, so following the tail must be off -- \
         otherwise the key below would assert nothing"
    );

    shell.update_in(cx, |shell, window, cx| {
        shell.focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert!(
        following_tail(cx, &shell),
        "the shell's one key must reach the list and resume tail-following"
    );
}

// ---------------------------------------------------------------------------
// 6. What the shell's constants are, asserted rather than trusted
// ---------------------------------------------------------------------------

/// The window is chat-sized, and the startup channel is a named placeholder.
///
/// **Two claims that would otherwise be prose, and both have a failure mode that
/// is silent.** A window at the spike's 480x320 would render four rows, so every
/// frame-time figure taken against it would be about a different program — the
/// mistake `benches/frame_time.rs:255` records in writing. And a startup channel id
/// that looked like a real id would let a fixture quietly address the channel the
/// application opens, at which point the placeholder stops being findable.
///
/// **The size check is a compile-time assertion, and that is clippy being right
/// rather than clippy being in the way.** Both operands are `const`, so a runtime
/// `assert!` here proves nothing a compilation would not — and
/// `clippy::assertions_on_constants` says so. `const { }` keeps the check and
/// moves it to the place it belongs. (`assert!` in const context takes a literal
/// message and formats nothing, which is why the reason for the number is in the
/// constant's own documentation in `app.rs` and not repeated here.)
#[test]
fn the_shell_window_is_chat_sized_and_the_startup_channel_is_a_named_placeholder() {
    const _: () = assert!(
        app::WINDOW_WIDTH >= 1024.0 && app::WINDOW_HEIGHT >= 768.0,
        "the shell's window must be chat-sized: rows per frame scale with viewport \
         height, so a spike-sized window publishes a number that says nothing \
         about a chat client"
    );

    assert_eq!(
        STARTUP_CHANNEL, "c_startup",
        "the startup channel is a placeholder and must say so in its name, so that \
         a fixture cannot address it by accident and so that `grep` finds every \
         place that has to be replaced when a real channel list arrives"
    );
}
