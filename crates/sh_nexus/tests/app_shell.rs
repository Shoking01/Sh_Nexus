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
//! | **`Escape` chains composer → shell → log, and the cursor activates there** | [`escape_chains_from_the_composer_to_the_log_and_its_retry`] | runtime, simulated keystroke, full chain |
//! | **A second `Escape` on the log is a fixed point, not a focus loop** | [`a_second_escape_on_the_log_is_a_fixed_point_and_does_not_bounce`] | runtime, repeated keystroke |
//! | The composer is on screen, below the log, and is the production view | [`the_input_bar_is_on_screen_below_the_list`] | runtime, real bounds |
//! | The window opens with the field focused, not the shell root | [`the_window_opens_with_the_composer_focused`] | runtime |
//! | **Typing reaches the draft** | [`typing_reaches_the_draft_through_the_platform_input_handler`] | runtime, platform input path |
//! | Enter sends and clears, and no newline is left behind | [`enter_sends_the_draft_and_clears_it`] | runtime |
//! | A blank Enter neither sends nor destroys the draft | [`a_blank_enter_does_not_send_and_does_not_destroy_the_draft`] | runtime, negative |
//! | The optimistic row is up before any ACK, and stays `Pending` | [`the_optimistic_row_is_on_screen_before_any_ack`] | runtime |
//! | **The composer's frame actually reaches the transport's outbound queue** | [`begin_send_puts_the_frame_on_the_wire_when_a_transport_is_published`] | runtime, three-way contrast, no server |
//! | Escape hands focus to the shell and does **not** fire its gesture | [`escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail`] | runtime, negative |
//! | The draft is bounded, by a constant that already exists | [`the_draft_is_bounded_by_the_message_ceiling`] | runtime + parsed constant |
//! | **A healthy connection renders no banner element at all** | [`a_connected_client_renders_no_banner_at_all`] | runtime, negative |
//! | Each unhealthy state renders its own words and its own colour | [`each_unhealthy_connection_renders_its_own_words`] | runtime + the view's own answer |
//! | **No banner when no connection was ever attempted** | [`no_banner_is_rendered_when_no_connection_was_attempted`] | runtime, negative |
//!
//! # The composer tests are the ones that could pass for the wrong reason
//!
//! **Every test above the composer section could be written without a text field
//! and would still pass**, which is why the composer needs tests of its own here
//! rather than a line in the table above. Four of them are negative, and that is
//! deliberate: the failure modes `ui/views/input_bar.rs`'s module documents are
//! all *absences*.
//!
//! | The defect | How it presents | The test that catches it |
//! |---|---|---|
//! | a key handler that stops propagation on character keys | the field renders, keys arrive, **no text ever appears** | [`typing_reaches_the_draft_through_the_platform_input_handler`] |
//! | a key handler that lets `enter` propagate | the message sends **and then a newline lands in the cleared draft** | [`enter_sends_the_draft_and_clears_it`] asserts the draft is `""`, not `"\n"` |
//! | a `send` that clears unconditionally | a refused send **silently eats what the user typed** | [`a_blank_enter_does_not_send_and_does_not_destroy_the_draft`] |
//! | a composer's `escape` that fails to stop propagation | `escape` also yanks the log to the newest message | [`escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail`] |
//! | a composer's `escape` that drops focus instead of handing it over | `escape` works once, and then **the shell's own `escape` is dead forever** | the same test, second half |
//! | **the shell's `escape` that does not hand focus to the log** | the log renders, `up`/`down` work when the list is focused *by API*, and **no key in the client can reach it** — the cursor and its retry are pointer- and test-only | [`escape_chains_from_the_composer_to_the_log_and_its_retry`] |
//! | **an `escape` handler that hands focus back up the chain** | a focus loop: `escape` stops doing anything, and focus ends up where nobody intended, with no error anywhere | [`a_second_escape_on_the_log_is_a_fixed_point_and_does_not_bounce`] |
//!
//! **The first of those is the one that cannot be caught by reading the code.**
//! `Window::dispatch_keystroke` forwards a character to the focused input
//! handler only when the key propagated (`gpui/src/window.rs:5365`), so a
//! handler that stops propagation on `"h"` compiles, runs, and drops the
//! character. Only a test that types through the platform path and asks the view
//! what it received can tell the difference — which is why this file uses
//! `simulate_input` rather than calling the handler or the view's text method
//! directly.
//!
//! **The last row was found by a failing test and not by reading the framework,
//! which is why it is worth stating.** "Blur and stop" looked right, and
//! `Window`'s own code seems to agree: with no focused element it routes keys to
//! `DispatchTree::root_node_id()`. What neither the docs nor the code comment
//! says is that this node is **not** the root view's element — so a window with
//! nothing focused delivers every key to an empty listener list. The proof lives
//! in [`escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail`]:
//! blurring and stopping passes that test's first half and fails its second.
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
use gpui::{px, Entity, EntityInputHandler, Focusable, TestAppContext, VisualTestContext};
use rstest::rstest;
use sh_nexus::app::{self, ConnectionSettings, Shell, DRAIN_INTERVAL, STARTUP_CHANNEL};
use sh_nexus::core::markdown::MAX_MESSAGE_BYTES;
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::Message;
use sh_nexus::core::theme::BuiltIn;
use sh_nexus::state::bridge::{self, Delivery, EventSender};
use sh_nexus::state::{ApplyOutcome, DeliveryState};
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

/// A WebSocket endpoint with nothing listening on it.
///
/// **A dead port is the design, not a compromise.** `WsTransport::send_message`
/// hands a frame to an `mpsc` queue and returns; the worker thread writes the
/// socket only once it has connected to something. So an endpoint nothing is
/// serving still exercises everything the send-path test below asks — *did the
/// composer's gesture reach the transport* — and it does so without spawning
/// `sh_nexus_server`, without waiting for a handshake, and without a `sleep()`.
/// `tests/ws_transport.rs` follows a frame the rest of the way to a real server's
/// `message.ack`; this file is the half that runs on a machine with no server on
/// it at all, and the two are one sentence each rather than one test that needs
/// both.
const DEAD_ENDPOINT: &str = "ws://127.0.0.1:1/ws";

/// A token for a socket that will never answer.
///
/// **Fixture text rather than a credential, and it says so in its own body.**
/// `AGENTS.md` §7.5 names tokens as a thing that must never reach a log; this value
/// exists only because `ConnectionSettings::from_parts_for_test` takes two arguments,
/// and a string that looked like a real credential would be one that could be
/// mistaken for one in a failing `Debug` — the failure mode
/// `tests/shell_connection.rs`'s own token is written to avoid.
const NO_TOKEN: &str = "tok_fixture_never_answers_do_not_log_me";

/// The three bodies the send-path test types, one per send.
///
/// **Distinct so a test can tell the rows apart, and the reason it must do it by
/// body is worth recording.** `InputBar::send` mints every `client_msg_id` with
/// `Uuid::new_v4()` and stamps each row with `Utc::now()`, so a test can neither
/// name which row is which in advance nor rely on the order they landed in:
/// `core::ordering::compare` breaks a timestamp tie on `client_msg_id` (see its own
/// documentation), and three sends from one gesture can share a timestamp. The
/// composer is the only party that knows the pairing, and the body is what it
/// paired. None of these is ever printed — `AGENTS.md` §7.5 forbids message content
/// in a log and a panic message is a log.
const BODY_NO_SOCKET: &str = "typed with no socket";
const BODY_ON_THE_WIRE: &str = "typed with a socket";
const BODY_AT_A_STOPPED_SOCKET: &str = "typed at a stopped socket";

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

/// Focuses the composer, which is exactly what `app::open` does on launch.
///
/// **The test harness builds the shell directly rather than through `open`, so
/// every test that needs a focused field asks for one here** — and the
/// `run_until_parked` is load-bearing rather than tidy: focusing marks the
/// window dirty, the frame that follows is the one whose paint registers the
/// platform input handler, and a keystroke dispatched before that frame is a
/// keystroke with no handler to receive it.
fn focus_composer(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
    shell.update_in(cx, |shell, window, cx| {
        shell.composer_focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();
}

/// What the user has typed, read from the view rather than from the element tree.
fn draft(cx: &VisualTestContext, shell: &Entity<Shell>) -> String {
    shell.read_with(cx, |shell, app| {
        shell
            .input()
            .read_with(app, |input, _| input.draft().to_owned())
    })
}

/// The `client_msg_id` of the one message this client has sent, read through the
/// seam.
///
/// **A helper rather than a fixture constant, and the reason is
/// [`InputBar::send`]:** the composer mints the id with `Uuid::new_v4()`, so no
/// test can know it in advance. [`first_sent`] exists for the same reason and
/// returns everything *except* the id, because a test that needs to reject the
/// send afterwards has to address it.
fn sent_id(cx: &VisualTestContext, shell: &Entity<Shell>) -> Option<Uuid> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| {
            state
                .messages(STARTUP_CHANNEL)
                .first()
                .map(|message| message.client_msg_id)
        })
        .flatten()
    })
}

/// Where the window's keyboard currently is, as a name a failure message can use.
///
/// **The three answers are the three rungs of the `Escape` ladder, and the
/// question is asked on the window rather than inferred from what a gesture did.**
/// `FocusHandle::is_focused` is `window.focus == Some(self.id)`
/// (`gpui/src/window.rs:606`), so this reads the same fact the framework's own
/// dispatch reads when it builds a key's path — which is what makes it the right
/// question to ask about focus rather than "did anything move".
///
/// **`composer` is checked before `list` and `shell` because only one handle can
/// hold the focus at a time**, so the order is only about which assertion message
/// a broken chain produces; `none` is the fourth answer and the one the chain must
/// never produce, because a window with nothing focused routes every key to
/// `DispatchTree::root_node_id()` — a node that is not this view's element.
fn focus_is_on(cx: &mut VisualTestContext, shell: &Entity<Shell>) -> &'static str {
    shell.update_in(cx, |shell, window, cx| {
        if shell.composer_focus_handle(cx).is_focused(window) {
            "composer"
        } else if shell
            .list()
            .read_with(cx, |list, app| list.focus_handle(app).is_focused(window))
        {
            "list"
        } else if shell.focus_handle(cx).is_focused(window) {
            "shell"
        } else {
            "none"
        }
    })
}

/// The first message in the startup channel, with this client's delivery state.
///
/// **Read through the seam, and through a closure that names no state type.**
/// `tests/bridge.rs` fails the build on a module outside `state/` that names the
/// state type, and a closure parameter is inferred rather than written, so this
/// is the shape that can read the state without naming it. Ordered rather than
/// keyed because the composer mints its `client_msg_id` with `Uuid::new_v4()`
/// and the test has no way to know which one it was — which is precisely the
/// property the seam's contract creates.
///
/// **Read off the shell rather than off the `App`, and that is a convenience
/// rather than an accident:** `Entity::read_with` hands the closure a `&App`,
/// which is exactly what `bridge::try_read` takes, so the test needs no `update`
/// and cannot be confused with the two-argument `TestAppContext::update` that
/// the rest of this file uses for draining.
fn first_sent(
    cx: &VisualTestContext,
    shell: &Entity<Shell>,
) -> Option<(String, Option<DeliveryState>, bool)> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| {
            let message = state.messages(STARTUP_CHANNEL).first()?;
            Some((
                message.content.clone(),
                state.delivery(&message.client_msg_id),
                message.user_id.as_str() == state.self_user_id(),
            ))
        })
        .flatten()
    })
}

/// Whether the application has a socket installed for sends.
///
/// **Asks the bridge, because that is where the socket lives and asking anywhere
/// else would test the wrong thing.** The first version of this helper read
/// `MessageList::has_transport`, which existed only because the feature first put a
/// `WsTransport` in the view — a design `tests/layer_boundary.rs` rejects, because
/// `AGENTS.md` §3.2 gives `network/` to the seam. With the socket behind
/// [`sh_nexus::state::bridge`], "can this client send?" has exactly one honest
/// answer and this asks it.
///
/// **A send that leaves here does not prove it arrived.** `bridge::try_send_message`
/// hands the frame to the worker thread's queue and returns; whether a server
/// acknowledges it is a different claim, and
/// `ws_transport::the_shell_s_channel_is_one_the_server_accepts` is the test that
/// makes that one against a real server.
fn bridge_has_transport(cx: &VisualTestContext) -> bool {
    cx.read(sh_nexus::state::bridge::has_transport)
}

/// How the send carrying `body` ended up: its delivery state and its failure code.
///
/// **Addressed by body rather than by identity, and that is forced rather than
/// convenient.** The composer mints each `client_msg_id` itself and stamps each row
/// with its own clock, so a test can neither name a row in advance nor order two
/// rows against each other — [`BODY_NO_SOCKET`]'s documentation gives both reasons,
/// and the second one is the one that bites: `ordering::compare` breaks a
/// timestamp tie on `client_msg_id`, so two sends in the same tick could come back
/// in either order. **The body is the only pairing the composer ever recorded.**
///
/// A closure parameter rather than a written type, so this reads the state without
/// naming it — see [`first_sent`]'s documentation for why that is the shape a file
/// outside `state/` can use.
fn sent_named(
    cx: &VisualTestContext,
    shell: &Entity<Shell>,
    body: &str,
) -> Option<(Option<DeliveryState>, Option<String>)> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| {
            state
                .messages(STARTUP_CHANNEL)
                .iter()
                .find(|message| message.content == body)
                .map(|message| {
                    (
                        state.delivery(&message.client_msg_id),
                        state
                            .failure(&message.client_msg_id)
                            .map(|failure| failure.code().to_owned()),
                    )
                })
        })
        .flatten()
    })
}

/// Types `body` into the focused composer and presses `Enter`.
///
/// **The user's gesture rather than a call to `MessageList::begin_send`,** for the
/// reason this file's module docs give for every keystroke it simulates: a test that
/// reaches past the field proves something about the view it reached past. Here the
/// point is sharper than usual — the send path has three links (typing reaches the
/// draft, `Enter` reaches the list, and the frame leaves the list) and this is the
/// only way to drive the last one through the first two.
///
/// **Focus is the caller's job, as it is everywhere else in this file**, because
/// `focus_composer`'s documentation explains why the paint frame matters and this
/// helper has no business repeating it.
fn type_and_send(cx: &mut VisualTestContext, body: &str) {
    cx.simulate_input(body);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
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
/// **This test focuses the shell root on purpose, and the reason changed when the
/// composer landed.** `app::open` now focuses the composer, because a chat
/// window that opens with its text field focused is one the user can type into.
/// That does not make this gesture unreachable — it makes it *second*: `Escape`
/// in the field hands focus to the shell's root handle, and the `Escape` after
/// that is this handler's. So the test states the state it needs rather than
/// inheriting whatever the window happened to focus: the two halves are
/// [`escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail`] and
/// this one, and between them they are the whole of `AGENTS.md` §5.2's `Escape`
/// requirement.
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
// 6. The composer
// ---------------------------------------------------------------------------

/// The composer is on screen, below the log, and is the production view.
///
/// **The vertical ordering is asserted rather than assumed, and it is the half
/// that a "does it render" test would miss.** A composer that laid out *above*
/// the log would satisfy every other assertion in this section — it would be on
/// screen, it would be focusable, and it would type — while being a chat client
/// upside down. `PLAN.md` §6 puts the field under the chat area, so the shell
/// puts it last in a `flex_col`.
#[gpui::test]
fn the_input_bar_is_on_screen_below_the_list(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    let list = cx
        .debug_bounds("message-list")
        .expect("the message list must record its bounds");
    let bar = cx
        .debug_bounds("input-bar")
        .expect("the composer must record its bounds");

    assert!(
        bar.size.height > px(0.),
        "the composer must be laid out, got {:?}",
        bar.size
    );
    assert!(
        bar.size.width > px(0.) && bar.size.width <= list.size.width,
        "the composer spans the log's width: it is a sibling in the shell's column, \
         so it cannot be wider than the log beside it -- got {bar:?} against \
         {list:?}"
    );
    assert!(
        bar.origin.y >= list.origin.y + list.size.height,
        "PLAN.md section 6 puts the input bar under the chat area: the field must \
         start at or below the end of the log, got bar {bar:?} and list {list:?}"
    );

    // And the view behind those bounds is the production composer, for the same
    // reason `the_shell_renders_the_production_message_list` checks the list: a
    // stand-in that looked like a text field would satisfy every bounds assertion
    // above and none of the behaviour in the tests below.
    let composed = shell.read_with(cx, |shell, app| {
        shell
            .input()
            .read_with(app, |input, _| input.draft().is_empty())
    });
    assert!(
        composed,
        "the view behind those bounds is the production InputBar, already built and \
         holding an empty draft"
    );
}

/// The window opens with the field focused, not the shell root.
///
/// **Asserted on the view's own answer, not on the window's.** `InputBar::
/// is_focused` reads the focus handle, which is the same handle
/// `Window::handle_input` is keyed on during paint — so this is asking the
/// question the typing path depends on, rather than the question "is anything
/// focused".
#[gpui::test]
fn the_window_opens_with_the_composer_focused(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);

    // What `app::open` does, done here because the harness builds the shell
    // directly; see `focus_composer`.
    focus_composer(cx, &shell);

    shell.update_in(cx, |shell, window, cx| {
        assert!(
            shell.input().read(cx).is_focused(window),
            "AGENTS.md 5.2 requires the feature to be reachable from the keyboard \
             alone, and a chat window that does not open with its field focused is \
             one the user has to click before they can type"
        );
    });
}

/// Typing reaches the draft, through the platform's input handler.
///
/// **This is the test that exists because the failure is invisible in the code.**
/// `gpui` has no text-input widget at this rev, so the field registers itself
/// with `Window::handle_input` from a canvas's *paint* callback; a keystroke
/// character then travels
///
/// ```text
/// simulate_input -> Window::dispatch_keystroke -> dispatch_event (the key tree)
///                -> if the key propagated: platform_window.take_input_handler()
///                -> ElementInputHandler::dispatch_input
///                -> InputBar::replace_text_in_range
/// ```
///
/// and **every arrow in that chain is a place the character can be lost** without
/// an error. The one this file's module docs name is propagation:
/// `dispatch_keystroke` returns before forwarding `key_char` when the key did not
/// propagate (`gpui/src/window.rs:5375`). A field whose handler stopped
/// propagation on `"h"` would render, handle the key, and receive nothing — and
/// asserting the element tree would not catch it, because the tree is built
/// whether or not the platform ever calls back.
///
/// **So the test types like a user and asks the view what it got.** `simulate_input`
/// builds one keystroke per character *with* `key_char` set, which is the only
/// representation that exercises the forwarding branch.
#[gpui::test]
fn typing_reaches_the_draft_through_the_platform_input_handler(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    focus_composer(cx, &shell);

    assert_eq!(draft(cx, &shell), "", "the field opens empty");

    cx.simulate_input("hello");
    cx.run_until_parked();

    assert_eq!(
        draft(cx, &shell),
        "hello",
        "a character reaches the draft only if the key propagated, the paint-time \
         handler was registered, and the platform forwarded `key_char`. A composer \
         that stopped propagation on character keys would fail here with no error \
         anywhere -- see this file's module docs"
    );

    // And a second batch appends rather than replacing, which is what pins the
    // shape of the field: the caret is the end, so insertion is at the end.
    cx.simulate_input(" there");
    assert_eq!(
        draft(cx, &shell),
        "hello there",
        "successive insertions append: this field has no caret, so the end is the \
         only insertion point (InputBar::selected_text_range returns None)"
    );
    assert_eq!(
        showing(cx, &shell),
        0,
        "typing is not sending: PLAN.md says the message goes on Enter, and a \
         composer that sent on every keystroke would be unusable"
    );
}

/// Enter sends the draft, clears it, and leaves nothing behind.
///
/// **The `""` assertion is the whole test, and it is not a restatement of "it
/// cleared".** `Window::dispatch_keystroke` begins with
/// `keystroke.with_simulated_ime()`, and that synthesises `key_char` for named
/// keys — including `"enter" => Some("\n")`
/// (`gpui/src/platform/keystroke.rs:242`). A `Enter` that propagated would
/// therefore send the message *and then* append a newline to the draft it had
/// just cleared: one keypress, one message sent, and a composer holding `"\n"`
/// that looks empty and is not, and that a second `Enter` would refuse to send
/// because it trims to nothing.
///
/// **So this is the test that forces the inverse propagation rule.** The same
/// handler must *let characters through* (see
/// [`typing_reaches_the_draft_through_the_platform_input_handler`]) and *stop
/// `enter`*, and the only way to know both are right is to assert the draft is
/// empty and not merely that something was sent.
#[gpui::test]
fn enter_sends_the_draft_and_clears_it(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    focus_composer(cx, &shell);

    cx.simulate_input("ship it");
    assert_eq!(draft(cx, &shell), "ship it");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert_eq!(
        showing(cx, &shell),
        1,
        "Enter must put the optimistic row on screen"
    );
    let (content, delivery, is_self) =
        first_sent(cx, &shell).expect("the state must hold the message the composer sent");
    assert_eq!(
        content, "ship it",
        "the body the user typed is the body that was sent, verbatim"
    );
    assert_eq!(
        delivery,
        Some(DeliveryState::Pending),
        "PLAN.md section 8.1's Optimistic Send Flow: the row is on screen as \
         `sending...` before the server has seen the message, and no ACK has been \
         delivered in this test -- only a `DomainEvent` can produce one"
    );
    assert!(
        is_self,
        "the state must attribute the message to this client, or the row would \
         render as somebody else's"
    );
    assert_eq!(
        draft(cx, &shell),
        "",
        "a sent draft is cleared, and it is cleared *entirely*: dispatch_keystroke \
         synthesises key_char \"\\n\" for enter (gpui/src/platform/keystroke.rs), so \
         a propagating Enter would leave a newline in the draft it just cleared. \
         An empty string, not \"\\n\" -- that is the assertion with teeth"
    );
}

/// A blank `Enter` neither sends nor destroys what the user typed.
///
/// **The destructive half is the point, and it is why the composer checks the
/// draft before minting anything.** `actions.rs` refuses a whitespace-only body
/// with `IgnoreReason::EmptyContent` — *"a message with no body and no attachment
/// is not a message"* — so the send is going to be refused regardless. A
/// composer that cleared the draft anyway would destroy what the user was in the
/// middle of typing, on a keypress that sent nothing.
///
/// **The composer restores rather than re-reads, which is why the assertion can
/// be exact.** `InputBar::send` takes the draft, hands it to
/// `MessageList::begin_send`, and puts it back when the answer is that no row is
/// on screen. So the refused path round-trips the user's bytes untouched, and
/// `assert_eq!` on the whole string is the strongest form of that claim.
#[gpui::test]
fn a_blank_enter_does_not_send_and_does_not_destroy_the_draft(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    focus_composer(cx, &shell);

    // Whitespace only, and not the empty string: an empty draft is the trivial
    // case, and `actions.rs` refuses on `content.trim().is_empty()`, so a body of
    // spaces is the one that would slip past a `is_empty()` check.
    cx.simulate_input("   ");
    assert_eq!(
        draft(cx, &shell),
        "   ",
        "spaces are characters like any other: they must reach the draft, or this \
         test would pass without ever putting a blank draft in front of Enter"
    );

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert_eq!(
        showing(cx, &shell),
        0,
        "a whitespace-only body is not a message (actions.rs: EmptyContent), so \
         Enter on one must not put a row on screen"
    );
    assert_eq!(
        draft(cx, &shell),
        "   ",
        "and the draft must survive the refusal byte for byte: a refused send that \
         cleared the field would destroy what the user was typing, which is the one \
         thing a composer must never do"
    );
    assert_eq!(
        first_sent(cx, &shell),
        None,
        "nothing at all reached the state"
    );
}

/// The optimistic row is on screen before any `ACK`, and is not delivered yet.
///
/// **`PLAN.md` §8.1's Optimistic Send Flow is a claim about *ordering*, and this
/// test is the only place that ordering can be observed**: it asserts that the
/// message is visible while the state still says `Pending`, and it does that
/// without delivering a single `DomainEvent`. An ACK is a network event and there
/// is no network here, so the absence is structural rather than asserted — which
/// is what makes "before any ACK" a fact about this test and not a hope.
#[gpui::test]
fn the_optimistic_row_is_on_screen_before_any_ack(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    focus_composer(cx, &shell);

    cx.simulate_input("optimistic");
    cx.simulate_keystrokes("enter");
    // No tick: the drain pump is irrelevant here, because a send goes straight
    // through the seam on the gesture rather than through the inbox.
    cx.run_until_parked();

    assert_eq!(
        showing(cx, &shell),
        1,
        "the row is on screen from the gesture itself -- it does not wait for a \
         tick, let alone for a server"
    );
    let (_, delivery, _) = first_sent(cx, &shell).expect("the message is held by the state");
    assert_eq!(
        delivery,
        Some(DeliveryState::Pending),
        "and the state still says `sending...`, which is what the row draws"
    );

    // The row is a real laid-out element, not a count: `item_count` is bookkeeping
    // the list updates itself, and a non-zero rectangle is a row that was painted.
    let row = cx
        .debug_bounds("message-row")
        .expect("the optimistic row must have recorded its bounds");
    assert!(
        row.size.height > px(0.),
        "the optimistic row must be laid out with a non-zero height, got {:?}",
        row.size
    );
}

/// `begin_send` puts the frame on the wire when a transport is published.
///
/// **The defect this guards had no failing test to write, which is exactly why it
/// needs one.** Before the send path was wired, `MessageList::begin_send` put a row
/// on screen and stopped: the socket lived in `Shell::transport`, nobody published
/// it anywhere a view could reach, and `WsTransport::send_message` had **zero
/// production callers**. Every assertion above that says "the message was sent" was
/// therefore only ever saying "a row appeared" — so a user could type into the
/// field, watch the row go up as `sending...`, and wait for ever for an ack that
/// could not come, with no error on screen and none in the state.
///
/// **So the assertion is that the frame leaves the view, and the observable is the
/// transport's own answer.** `MessageList::enqueue` cannot invent an outcome: it
/// either hands the frame to `WsTransport::send_message`, and that either queues it
/// or refuses it. **A refusal comes back as `MessageSendFailed` and
/// `actions::fail_send` moves the row to `Failed`**, which means a row in `Failed`
/// carrying this crate's own code is reachable *only* through the wiring, and a row
/// in `Pending` with no transport is reachable *only* without it. Both halves are
/// asserted below from the same gesture, with the transport as the only difference,
/// and that is what makes this evidence rather than two facts that happen to sit in
/// one test.
///
/// **Three sends, and the middle one is the honest one.** The first has no socket,
/// so nothing can be offered. The second has a socket that has not connected, and
/// its row is `Pending` for the ordinary reason — nothing has acknowledged it — so
/// it is asserted *together with* `frames_sent == 0`: the frame was accepted by the
/// outbound queue and has been written nowhere. The third goes to a socket this test
/// has stopped, which is the refusal the rollback exists for. **Stopping the socket
/// is what makes that refusal deterministic rather than lucky:**
/// `WsTransport::shutdown` sets the closed flag synchronously and
/// `WsTransport::queue` reads it before it touches the channel, so there is no
/// window in which the third send could be accepted.
///
/// **No server, and that is the point rather than a limitation.** `send_message` is
/// a hand-off to an `mpsc` queue; the worker writes the socket only after it has
/// connected to something. So the endpoint above has nothing on it and every
/// assertion still holds — deterministically, with no polling and no `sleep()`.
/// `the_shell_s_channel_is_one_the_server_accepts` in `tests/ws_transport.rs` is the
/// other half of the pair: it names the shell's channel at a real server process
/// and requires a `message.ack`.
///
/// **What this cannot show, stated rather than implied.** It cannot show that a
/// frame was written to a socket, and `frames_sent == 0` is asserted precisely so
/// that limit is on the record instead of discovered by a reader who assumed
/// otherwise: nothing ever connected, by design. Nor does it say anything about
/// delivery to another user, about reconnection, or about the retry gesture — a send
/// this test fails is a send the user can retry, which
/// [`escape_chains_from_the_composer_to_the_log_and_its_retry`] already drives
/// through the keyboard.
#[gpui::test]
fn begin_send_puts_the_frame_on_the_wire_when_a_transport_is_published(cx: &mut TestAppContext) {
    /// The namespace `MessageList::enqueue` gives this machine's own refusals.
    ///
    /// **Asserted rather than matched loosely, and the reason it is namespaced at all
    /// is the reason to pin it.** A send can fail because a server rejected it or
    /// because this machine's outbound queue would not take it, and whoever reads the
    /// badge has to be able to tell those apart. A test that asserted only `Failed`
    /// would pass whichever had happened — including if a server had answered.
    const ENQUEUE_FAILED_CODE: &str = "transport.enqueue_failed";

    let (shell, cx) = shell(cx);

    // --- 1. Offline: the row goes up, and there is nowhere to offer it. -----
    assert!(
        !bridge_has_transport(cx),
        "the shell is built offline and the list must say so before anything is \
         claimed about a send: `Shell::new` publishes no transport, so a `true` here \
         would mean something started one"
    );

    focus_composer(cx, &shell);
    type_and_send(cx, BODY_NO_SOCKET);

    assert_eq!(
        sent_named(cx, &shell, BODY_NO_SOCKET),
        Some((Some(DeliveryState::Pending), None)),
        "with no socket the frame has nowhere to go, so the row is `Pending` and \
         carries no reason — `MessageList::enqueue`'s documented `None` arm, and the \
         state every other send test in this file already asserted"
    );

    // --- 2. Published: the shell holds it, and so does the log. ------------
    let settings = ConnectionSettings::from_parts_for_test(DEAD_ENDPOINT, NO_TOKEN);
    shell
        .update(cx, |shell, cx| shell.start_transport(&settings, cx))
        .expect("starting a worker thread is not a network operation");

    assert!(
        shell.read_with(cx, |shell, _| shell.transport().is_some()),
        "`start_transport` must leave the shell holding the socket it started: that \
         field exists so the connection outlives any one frame, and dropping it \
         would tear the socket down with no event and no log"
    );
    assert!(
        bridge_has_transport(cx),
        "**and the log must be told, which is the half this whole test exists \
         for.** The shell holding a socket the composer cannot reach is precisely \
         the defect: `Shell::transport` answers `Some`, the user is looking at a \
         message log that looks alive, and nothing they type can leave the machine — \
         a state every other assertion in this file would pass"
    );

    focus_composer(cx, &shell);
    type_and_send(cx, BODY_ON_THE_WIRE);

    assert_eq!(
        sent_named(cx, &shell, BODY_ON_THE_WIRE),
        Some((Some(DeliveryState::Pending), None)),
        "the frame reached a transport that accepted it, and the row is `Pending` \
         for the ordinary reason: nothing has acknowledged it. Publishing a socket \
         must not disturb the send before it either — that one is still `Pending` \
         with no reason, and is asserted separately"
    );
    assert_eq!(
        sent_named(cx, &shell, BODY_NO_SOCKET),
        Some((Some(DeliveryState::Pending), None)),
        "the earlier send is untouched by the one after it, so the second send \
         reaching the transport did not reach back and change anything already on \
         screen"
    );

    // --- 3. Refused: a row that could not have existed before. -------------
    let transport = shell
        .read_with(cx, |shell, _| shell.transport().cloned())
        .expect("the socket was published two steps ago and nothing has replaced it");
    transport.shutdown();
    assert!(
        transport.is_closed(),
        "the closed flag is set synchronously, which is what makes the send below a \
         determinate outcome rather than a race with the worker thread"
    );

    focus_composer(cx, &shell);
    type_and_send(cx, BODY_AT_A_STOPPED_SOCKET);

    assert_eq!(
        sent_named(cx, &shell, BODY_AT_A_STOPPED_SOCKET),
        Some((
            Some(DeliveryState::Failed),
            Some(ENQUEUE_FAILED_CODE.to_owned())
        )),
        "**the row is `Failed`, and only the wiring can put it there.** A stopped \
         socket refuses the frame, `MessageList::enqueue` turns that refusal into a \
         `MessageSendFailed`, and `actions::fail_send` moves the row. Before the \
         send path was wired, `begin_send` never asked the transport anything, so \
         this row would have sat at `Pending` for ever with nothing able to move it — \
         which is the whole regression, visible as one enum value"
    );

    // The refusal is the one thing this test changes about a row, and it changed it
    // in place: three sends, three rows, no send having removed another's.
    assert_eq!(
        showing(cx, &shell),
        3,
        "three sends leave three rows, and `PLAN.md` §7 is explicit that a failed send \
         stays visible for retry rather than being dropped"
    );

    // And the honest limit of this file's reach, asserted rather than implied.
    let stats = transport.stats();
    assert_eq!(
        stats.frames_sent, 0,
        "no frame was written, because no socket was ever established: the endpoint \
         has nothing listening on it. `send_message` is a hand-off to the outbound \
         queue, which is why the rows above moved at all — {stats}"
    );
    assert_eq!(
        stats.connects, 0,
        "and this test never reached a server, which is deliberate — it is the half \
         of the pair that runs on a machine with none. `tests/ws_transport.rs` is \
         the half that needs one, and the two between them say the same sentence \
         about two different links — {stats}"
    );
}

/// `Escape` in the field hands focus to the shell, and does **not** fire its
/// gesture.
///
/// **Two owners for one key, split by focus, and this test is what makes the
/// split observable rather than asserted in a comment.** `Shell::on_key_down`
/// still returns the log to the newest message on `Escape` — that is the half
/// [`escape_returns_the_list_to_the_newest_message`] covers — but only once the
/// field has given the key up. While the composer holds focus, its handler moves
/// focus to the shell's root handle and calls `cx.stop_propagation`;
/// `Window`'s bubble pass walks the path focused-node-first and returns the
/// moment propagation stops (`gpui/src/window.rs:6068`), so the shell's handler
/// is never called.
///
/// **So the first `Escape` asserts `false`, and that is the assertion with
/// teeth.** The log is set up scrolled away from the tail *first*, which means a
/// leaked `Escape` would flip it to `true`. Without that setup both outcomes are
/// `false` and the test would pass whatever the composer did.
///
/// **The second `Escape` is here rather than left to the other test, because it
/// is the half that caught the real bug.** A composer that blurred and stopped
/// there would pass the first assertion and quietly break the shell's gesture
/// forever: with no element focused, `Window` routes keys to
/// `DispatchTree::root_node_id()` (`gpui/src/window.rs:6258`), and that node is
/// **not** the root view's element, so every key lands on an empty listener list.
/// `InputBar::fallback_focus` exists to prevent exactly that, and this is the
/// test that says it works.
#[gpui::test]
fn escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail(cx: &mut TestAppContext) {
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

    // Away from the bottom, so that a leaked Escape has something to change.
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
        "the reader has scrolled away, so the log's own Escape gesture has \
         something to do -- otherwise the assertion below could not fail"
    );

    focus_composer(cx, &shell);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    shell.update_in(cx, |shell, window, cx| {
        assert!(
            !shell.input().read(cx).is_focused(window),
            "AGENTS.md 5.2's focus order: Escape must leave the text field, not \
             leave the user typing into one they cannot see"
        );
    });
    assert_eq!(
        draft(cx, &shell),
        "",
        "Escape blurs; it does not send. The field was empty here on purpose so \
         that a send would show up as a row rather than as a change of contents"
    );
    assert_eq!(
        showing(cx, &shell),
        ENOUGH_TO_OVERFLOW as usize,
        "and nothing was sent: a blur is not a submit"
    );
    assert!(
        !following_tail(cx, &shell),
        "the shell's Escape gesture must NOT also have run. The composer's \
         stop_propagation is what prevents it, and this is the only assertion in \
         the file that can see a leak of that ownership"
    );

    // The second `Escape` is now the shell's. Proving both halves here rather
    // than only in the other test is what shows the split is by focus and not by
    // the key being claimed once and for all -- and it is the assertion that
    // fails if the composer's Escape leaves the window with nothing focused.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        following_tail(cx, &shell),
        "the composer's Escape must hand focus to the shell's root handle, not \
         merely drop it. With nothing focused, Window routes every key to \
         DispatchTree::root_node_id(), which is not the root view's element, so \
         the shell's own Escape would be stranded forever -- AGENTS.md 5.2 \
         keyboard-only operation broken by the key meant to provide it"
    );
    assert_eq!(
        focus_is_on(cx, &shell),
        "list",
        "and the shell's own `escape` is also the rung that hands the keyboard to \
         the log. Asserted here because this test already holds the shell root \
         focused, so it is free; walking the whole chain from the composer is \
         `escape_chains_from_the_composer_to_the_log_and_its_retry`'s job"
    );
}

/// The whole chain, by key: composer → shell → log → cursor → retry.
///
/// **This is the test that makes the log reachable by key a claim rather than a
/// wiring diagram, and every arrow in it is a link that can break alone.** A
/// chain that renders correctly and drops the focus one rung up satisfies every
/// other test in this file: the composer still blurs, the log still snaps to the
/// tail, and `tests/ui_message_list.rs` still proves the cursor and the retry work
/// when the list is focused *by API*. So the assertions here are ordered along the
/// chain and each one names the link it covers:
///
/// ```text
/// composer --escape--> shell --escape--> list --down--> cursor --enter--> Pending
/// ```
///
/// **The first `escape` is asserted to have reached the shell and not the log,
/// because "the log is reachable" and "the log skipped a rung" are the same test
/// passing for different reasons.** If the shell handed focus straight to the list
/// the chain would still work end to end and this assertion would fail — which is
/// the point: the shell root is a real stop, it owns the log's own
/// *return-to-newest* gesture, and a chain that routed around it would have made
/// that gesture unreachable from the shell.
///
/// **The fixture is a failed send, and it is built through the production door.**
/// `MessageSendFailed` is a real [`DomainEvent`] delivered through the shell's own
/// sender and applied by the shell's own pump, so the row is failed the way a
/// server's rejection fails it — and `enter` at the end is the same key the
/// listbox convention uses, dispatched through [`gpui::VisualTestContext::simulate_keystrokes`]
/// so a handler that is not registered on the focused element cannot pass.
///
/// **`down` is the right key and not `up` for this fixture.** With no cursor,
/// `down` selects the newest row and `up` the oldest; the failed send is the only
/// row, so either would do — but `down` is the one a user reaches for after
/// arriving at a log that is tail-following, and choosing it means the test walks
/// the gesture a person would walk.
#[gpui::test]
fn escape_chains_from_the_composer_to_the_log_and_its_retry(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);

    // The optimistic row, through the composer's own send path.
    focus_composer(cx, &shell);
    cx.simulate_input("ship it");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let send_id = sent_id(cx, &shell).expect("the composer must have minted a send");
    assert_eq!(
        first_sent(cx, &shell).map(|(_, delivery, _)| delivery),
        Some(Some(DeliveryState::Pending)),
        "the fixture must be a real optimistic send before anything can fail it"
    );

    // And the server's rejection, through the shell's own inbox and pump.
    queued(
        cx,
        &shell,
        DomainEvent::MessageSendFailed {
            client_msg_id: send_id,
            code: "forbidden".to_owned(),
            detail: "you may not post here".to_owned(),
        },
    );
    tick(cx);
    assert_eq!(
        first_sent(cx, &shell).map(|(_, delivery, _)| delivery),
        Some(Some(DeliveryState::Failed)),
        "the fixture must be a FAILED send before a retry means anything: a \
         Pending send has nothing to retry and `enter` on it is a no-op"
    );

    // --- Link 1: composer -> shell. ---------------------------------------
    assert_eq!(
        focus_is_on(cx, &shell),
        "composer",
        "the window opens on the composer, and this test's first `escape` is the \
         one that leaves it"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        focus_is_on(cx, &shell),
        "shell",
        "the first `escape` must land on the shell's own root handle, not skip \
         straight to the log. That handle is a real stop: it is what owns the \
         log's return-to-newest gesture, and a chain that routed around it would \
         make that gesture unreachable"
    );

    // --- Link 2: shell -> list. ------------------------------------------
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        focus_is_on(cx, &shell),
        "list",
        "the shell's `escape` must hand focus to the log. This is the link the \
         whole work unit exists for: without it the cursor is reachable by API \
         and by test and by no key in the shipped client, and AGENTS.md 5.2's \
         keyboard-only requirement is unmet for the retry"
    );

    // --- Links 3 and 4: cursor, then activation. -------------------------
    assert!(
        following_tail(cx, &shell),
        "and the log must be at its newest message before the cursor moves, \
         because the shell's gesture is a real part of what this chain buys"
    );
    cx.simulate_keystrokes("down");
    assert_eq!(
        shell.read_with(cx, |shell, app| shell
            .list()
            .read_with(app, |list, _| list.selected())),
        Some(send_id),
        "`down` with no cursor selects the newest row, and the fixture's only row \
         is the failed send. A cursor that did not move here would mean the list's \
         handler is not running on the focused node, and the `enter` below would \
         then be pressing nothing"
    );

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        first_sent(cx, &shell).map(|(_, delivery, _)| delivery),
        Some(Some(DeliveryState::Pending)),
        "`enter` on the selected failed row must put it back in flight, through \
         the chain and not through a direct call. Nothing above this client \
         transmits it -- `retry_send` does not send and the outbox is Phase 3 -- \
         so `Pending` is the honest end state"
    );
    assert_eq!(
        showing(cx, &shell),
        1,
        "and the same single row, in place: a retry reached by keyboard that \
         added or removed a row would be a different message"
    );
}

/// A second `Escape` on the log is a fixed point, not a bounce.
///
/// **The chain has to terminate, and "it does not crash" is not the claim.** The
/// failure this test exists for is a *focus loop*: a keydown handler that hands
/// focus somewhere whose handler hands it back, so one keypress moves focus
/// forever. A loop is not a hang — `Window::focus` bumps a focus generation and
/// marks the window dirty, and the dispatch walk has already returned by then — so
/// it would show up as a client whose `Escape` does nothing and whose focus is
/// somewhere nobody intended, with no error anywhere.
///
/// **So the assertion is on where focus is after each further press, and the
/// presses are three rather than one.** One extra press distinguishes "the second
/// `escape` reached the shell" from "the second `escape` bounced to the composer";
/// three distinguishes a fixed point from a two-cycle, which is the shape a
/// half-built chain takes. The composer is also asserted to be *unreachable* after
/// the loop, because the alternative design — the log handing `escape` back up to
/// the field — is precisely the cycle `Shell::on_key_down` rejects.
///
/// **`following_tail` is re-asserted rather than assumed idempotent.** The shell's
/// gesture re-runs on every press, and `follow_tail` is `set_follow_mode(Tail)`
/// plus `scroll_to_end`; if either were not idempotent the log would drift, and a
/// test that only checked focus would call that a pass.
#[gpui::test]
fn a_second_escape_on_the_log_is_a_fixed_point_and_does_not_bounce(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    focus_composer(cx, &shell);

    // Onto the log, by the chain, so the state under test is the one a user is in.
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();
    assert_eq!(
        focus_is_on(cx, &shell),
        "list",
        "the fixture must start on the log, or the presses below assert nothing"
    );

    for press in 1..=3 {
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();

        assert_eq!(
            focus_is_on(cx, &shell),
            "list",
            "press {press}: `escape` on the log must be a fixed point. Focus \
             bouncing between the shell and the log would leave it somewhere \
             nobody intended with no error anywhere -- and bouncing on to the \
             composer would make `escape` a three-way toggle, which is the cycle \
             `Shell::on_key_down` rejects"
        );
        assert!(
            following_tail(cx, &shell),
            "press {press}: and the log's own gesture must still hold. It re-runs \
             on every press, so a `follow_tail` that was not idempotent would drift \
             the scroll while a focus-only assertion called it a pass"
        );
    }

    // And the ladder is not a ring: the composer is not reachable from here.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        showing(cx, &shell),
        0,
        "the log's `enter` on an empty cursor must not have sent anything, and in \
         particular must not have reached the composer. Nothing binds a key from \
         the log back to the field, and that one-way ladder is the gap \
         `Shell::on_key_down` states rather than hides"
    );
    assert_eq!(
        focus_is_on(cx, &shell),
        "list",
        "focus must still be on the log after a key it did not claim: a handler \
         that moved focus as a side effect of ignoring a key would be the same \
         class of defect as one that swallowed it"
    );
}

/// The draft is bounded by a constant the project already had.
///
/// **`AGENTS.md` §7.1 bans unbounded growth of in-memory state, and a draft grows
/// by however much a user pastes into it.** The bound is enforced where every
/// byte enters, so it is asserted on the entry point itself —
/// `EntityInputHandler::replace_text_in_range` — rather than through simulated
/// keystrokes. That is not a shortcut: it is the *same public method* typing and
/// [`EntityInputHandler::paste`] both arrive at, and simulating 131 072
/// keystrokes to reach a 128 KB buffer is a test that measures the harness.
///
/// **The constant is `core::markdown::MAX_MESSAGE_BYTES` and not a number chosen
/// for this view**, and that is the claim worth making checkable: it is the
/// largest body the render path will show faithfully, since the parser truncates
/// past it. A draft longer than that is a message this client cannot display, so
/// accepting it would be letting someone send something they have not seen.
#[gpui::test]
fn the_draft_is_bounded_by_the_message_ceiling(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    focus_composer(cx, &shell);

    // Well past the ceiling, and a whole-character payload: the truncation walks
    // back to a character boundary, and a payload that needed walking back would
    // be testing that rather than the bound.
    let overlong = "x".repeat(MAX_MESSAGE_BYTES * 2);
    let bar = shell.read_with(cx, |shell, _| shell.input().clone());
    bar.update_in(cx, |bar, window, cx| {
        bar.replace_text_in_range(None, &overlong, window, cx);
    });
    cx.run_until_parked();

    let held = draft(cx, &shell);
    assert_eq!(
        held.len(),
        MAX_MESSAGE_BYTES,
        "AGENTS.md 7.1 asks for a bound and for it to be stated; the draft stops at \
         the same ceiling the parser stops at, so the user cannot compose a body \
         this client would silently truncate on the way to the screen"
    );
    assert!(
        held.chars().all(|character| character == 'x'),
        "and the accepted prefix is intact text: the cut is on a character boundary"
    );

    // And the bound holds on the *second* insertion too, which is the case a
    // check against the incoming text alone would miss.
    bar.update_in(cx, |bar, window, cx| {
        bar.replace_text_in_range(None, "yyyy", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        draft(cx, &shell).len(),
        MAX_MESSAGE_BYTES,
        "a full draft accepts nothing further: the bound is on the draft, not on \
         any single insertion"
    );
}

// ---------------------------------------------------------------------------
// 6. What the shell's constants are, asserted rather than trusted
// ---------------------------------------------------------------------------

/// The window is chat-sized.
///
/// **A claim that would otherwise be prose, with a failure mode that is silent.** A
/// window at the spike's 480x320 would render four rows, so every frame-time figure
/// taken against it would be about a different program — the mistake
/// `benches/frame_time.rs`'s `WINDOW_HEIGHT` documentation records in writing.
///
/// **This test used to also assert that `STARTUP_CHANNEL` was a named placeholder,
/// and that assertion is deliberately gone rather than updated.** Its premise died
/// with the server: `STARTUP_CHANNEL` is now the channel the server seeds, because a
/// placeholder id that no server has heard of is rejected at runtime. What replaced it
/// is not a third copy of the literal here but
/// `the_shell_s_channel_is_one_the_server_accepts` in `tests/ws_transport.rs`, which
/// sends through the constant against a real server process and requires a
/// `message.ack`. String equality in a crate that cannot see the server proves the two
/// sides agree *today* only by coincidence; the behavioural test proves they agree.
/// Duplicating the literal a third time would have read like coverage while
/// guaranteeing nothing.
///
/// **The size check is a compile-time assertion, and that is clippy being right
/// rather than clippy being in the way.** Both operands are `const`, so a runtime
/// `assert!` here proves nothing a compilation would not — and
/// `clippy::assertions_on_constants` says so. `const { }` keeps the check and
/// moves it to the place it belongs. (`assert!` in const context takes a literal
/// message and formats nothing, which is why the reason for the number is in the
/// constant's own documentation in `app.rs` and not repeated here.)
#[test]
fn the_shell_window_is_chat_sized() {
    const _: () = assert!(
        app::WINDOW_WIDTH >= 1024.0 && app::WINDOW_HEIGHT >= 768.0,
        "the shell's window must be chat-sized: rows per frame scale with viewport \
         height, so a spike-sized window publishes a number that says nothing \
         about a chat client"
    );
}

// ---------------------------------------------------------------------------
// 7. Connection visibility
// ---------------------------------------------------------------------------

/// The banner's own answers about a state, read through the view rather than
/// rebuilt by the test.
///
/// **`use`d rather than spelled out per call so the test asserts the production
/// decision.** A test that rebuilt the `match` to check the `match` is a test of
/// the reimplementation — the mistake
/// `ui_markdown_blocks.rs::a_truncated_message_renders_a_notice_rather_than_losing_its_tail`
/// documents at length, where an earlier version of it inlined the notice and would
/// have passed even if `document()` had stopped rendering one.
use sh_nexus::ui::views::connection_banner::announcement;

/// The selector the banner records its bounds under.
///
/// **Imported from the view rather than spelled as a literal, for the reason
/// `message_row.rs` gives about static selectors:** a test that hard-coded the
/// string would keep passing if the view renamed it, and would then be asserting
/// about an element nobody draws.
const BANNER: &str = sh_nexus::ui::views::connection_banner::SELECTOR;

/// Starts the dead-endpoint socket the banner's *presence* rule needs.
///
/// **A transport is started, and the state is never left to the worker, because
/// those are two different fixtures.** Presence is answered by
/// `Shell::transport` — *a connection was attempted* — and content is answered by
/// the state's `ConnectionState`. This helper buys only the first: without a
/// transport the shell draws nothing connection-shaped at all, which is the
/// documented offline shell rather than a failure ([`DEAD_ENDPOINT`]'s
/// neighbourhood and `ConnectionSettings::from_env`). Content is then stated
/// explicitly by [`connection_is`], so no assertion in this section depends on
/// what a worker thread happened to publish or when.
fn with_a_transport(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
    let settings = ConnectionSettings::from_parts_for_test(DEAD_ENDPOINT, NO_TOKEN);
    shell
        .update(cx, |shell, cx| shell.start_transport(&settings, cx))
        .expect("starting a worker thread is not a network operation");

    assert!(
        shell.read_with(cx, |shell, _| shell.transport().is_some()),
        "the fixture must have started a socket: without one the shell draws no \
         banner whatever the state says, and every test below would pass for the \
         wrong reason"
    );
}

/// States `state` on the seam and asks for the frame that shows it.
///
/// **Through `bridge::try_apply_event` and not the inbox, and the reason is that
/// the fixture must not race a worker thread.** `WsTransport`'s worker publishes
/// `Connecting`, then `Reconnecting { attempt }` for every retry, into the very
/// same inbox the shell's pump drains — so a test that queued its own event and
/// ticked could have the worker's event applied after it and assert against a
/// state it never set. This door applies the event synchronously through
/// [`actions::apply_event`](sh_nexus::state::actions::apply_event), which is the
/// same production seam `MessageList::enqueue` uses when it turns a refused
/// outbound frame into a failed send.
///
/// **The `cx.notify()` afterwards is load-bearing rather than tidy.** No seam door
/// schedules a frame — `bridge::drain`'s own module docs, §6, assign that to the
/// caller — so a state changed here would sit in the state while the last painted
/// frame still showed the previous word. The notify is what makes the assertion
/// below about something a user could see rather than about a value in a struct.
/// `ui_message_list.rs`'s `a_click_that_arrives_after_the_ack_does_nothing` asks for
/// its frame the same way.
fn connection_is(cx: &mut VisualTestContext, shell: &Entity<Shell>, state: ConnectionState) {
    shell.update_in(cx, |_shell, _window, cx| {
        assert_eq!(
            bridge::try_apply_event(cx, DomainEvent::ConnectionStateChanged(state)),
            Some(ApplyOutcome::Applied),
            "a connection state is not something the state may refuse to hear: \
             `actions::apply_event` applies it unconditionally, and a fixture that \
             was quietly dropped would make every assertion below vacuous"
        );
        cx.notify();
    });
    cx.run_until_parked();
}

/// A healthy client draws no banner element at all.
///
/// **The negative assertion is the design, and this is the only test that can
/// enforce it.** `ConnectionState::Connected` is the state a user is in almost all
/// the time, so a banner for it would be a permanent distraction occupying the gap
/// between the log and the composer. `tests/ui_markdown_blocks.rs`'s pair
/// (`a_truncated_message_renders_a_notice_rather_than_losing_its_tail` /
/// `an_ordinary_message_renders_no_truncation_notice`) is the same shape: the
/// positive half alone cannot tell a selective notice from a constant one.
#[gpui::test]
fn a_connected_client_renders_no_banner_at_all(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    with_a_transport(cx, &shell);

    connection_is(cx, &shell, ConnectionState::Connected);

    assert!(
        cx.debug_bounds(BANNER).is_none(),
        "a connected client must paint no `{BANNER}` element whatsoever — not an \
         empty one, not a zero-height one. A banner for the healthy state is a \
         permanent distraction, and it is the state a user spends their day in"
    );
}

/// Each unhealthy state renders its own words and its own colour.
///
/// **One loop over all four rather than four tests, and the reason is that the
/// claim is comparative.** "The banner says something when it is not connected" is
/// satisfied by one hard-coded string, and so is "each state names itself" if only
/// one of them is ever checked. The four are asserted against each other in one
/// place so that a `match` with a single arm for everything but `Rejected` cannot
/// pass.
///
/// **The words come from [`ConnectionBanner::announcement`], and the element from
/// the painted frame, and both are asserted.** Reading only the view would pass
/// with the view wired into nothing; reading only the bounds would pass with any
/// text at all. `ui_message_list.rs` asserts the palette the same two ways — the
/// row's own accessor for the value, `debug_bounds` for the paint.
///
/// **The colour is half the claim.** `AGENTS.md` §7.3 is a rule about the colour a
/// text element draws with, and a banner that said `Reconnecting (attempt 4)…` in
/// `danger` would report a failure that has not happened: retrying is progress,
/// and `accent` is the palette's interactive colour — the same one the retry badge
/// uses ([`MessageRow`](sh_nexus::ui::views::message_row::MessageRow)'s
/// `RETRY_LABEL`). Only `Rejected` is `danger`, and `Rejected` is the only state
/// the enum documents as terminal.
#[gpui::test]
fn each_unhealthy_connection_renders_its_own_words(cx: &mut TestAppContext) {
    /// The four states that are not healthy, with the words and the colour each
    /// must draw.
    ///
    /// `Rejected`'s `detail` is a sentence no test in this project can predict,
    /// so it is written here as the kind of thing a *server* says and asserted
    /// against the state this section is about.
    const THE_SERVER_SAID: &str = "this client speaks protocol 2, and this server speaks 1";
    const THE_SERVERS_CODE: &str = "version.unsupported";

    let colors = app::theme_colors();
    let cases = [
        (
            ConnectionState::Disconnected,
            "Disconnected".to_owned(),
            colors.text_muted,
        ),
        (
            ConnectionState::Connecting,
            "Connecting\u{2026}".to_owned(),
            colors.accent,
        ),
        (
            ConnectionState::Reconnecting { attempt: 4 },
            "Reconnecting (attempt 4)\u{2026}".to_owned(),
            colors.accent,
        ),
        (
            ConnectionState::Rejected {
                code: THE_SERVERS_CODE.to_owned(),
                detail: THE_SERVER_SAID.to_owned(),
            },
            format!("{THE_SERVER_SAID} ({THE_SERVERS_CODE})"),
            colors.danger,
        ),
    ];

    let (shell, cx) = shell(cx);
    with_a_transport(cx, &shell);

    for (state, expected, expected_color) in cases {
        connection_is(cx, &shell, state.clone());

        // The paint first, because that is what a user would see; the view's own
        // answer second, because that is how this section knows *what* it is
        // painting. Asserted in this order so a failure names the missing half.
        let painted = cx
            .debug_bounds(BANNER)
            .unwrap_or_else(|| panic!("{state:?} must paint a `{BANNER}` element"));
        assert!(
            painted.size.height > px(0.),
            "{state:?}: the banner must be laid out, not merely built, got {painted:?}"
        );

        let announced = announcement(&state, colors)
            .unwrap_or_else(|| panic!("{state:?} is not connected, so it must say so"));

        assert_eq!(
            announced.text, expected,
            "each state must name itself: a banner that read the same word for \
             {state:?} would leave a user unable to tell waiting from failing"
        );
        assert_eq!(
            announced.color, expected_color,
            "{state:?} must draw in the palette's colour for what it means, and \
             AGENTS.md 7.3 is a rule about the colour a text element draws with"
        );
    }

    // And the last of the four is the one that must speak the server's words
    // rather than a string of its own. Asserted after the loop rather than inside
    // it because it is a claim about *whose* text, not about which text: it is the
    // difference between a user who can act on the banner and one who is told
    // "the connection failed" with nothing to do about it, which is the failure
    // `ConnectionState::Rejected`'s own documentation calls unactionable and
    // `AGENTS.md` §5.2's "clear, actionable error" is written against.
    let rejected = ConnectionState::Rejected {
        code: THE_SERVERS_CODE.to_owned(),
        detail: THE_SERVER_SAID.to_owned(),
    };
    let text = announcement(&rejected, colors)
        .expect("a rejection is not connected, so it must say so")
        .text;
    assert!(
        text.contains(THE_SERVER_SAID),
        "the banner must carry the server's own detail, got {text:?}"
    );
    assert!(
        text.contains(THE_SERVERS_CODE),
        "and the machine-readable code beside it, so a user can quote something \
         actionable rather than a paraphrase, got {text:?}"
    );
}

/// A shell that never attempted a connection draws no banner, even though the
/// state would have something to say.
///
/// **This is the test that keeps presence and content from being merged.** The
/// state is `Disconnected` here and the transport is `None`, and
/// [`ConnectionBanner::announcement`] has a word for exactly that state — so the
/// only thing stopping a banner is `Shell::transport`. A client launched with no
/// `SH_NEXUS_URL` is in local mode **by design**
/// ([`ConnectionSettings::from_env`](sh_nexus::app::ConnectionSettings::from_env)
/// treats both variables absent as offline, not as a misconfiguration), and a red
/// failure banner on that window forever would be noise about a choice the user
/// made. `app::open`'s `Ok(None)` arm says *"not an error, and not a banner — there
/// is no connection to describe"*.
///
/// **So the two halves are asserted in the order that makes the failure legible:**
/// first that the content exists, then that nothing is painted. Without the first,
/// this would pass for any shell that drew no banner at all.
#[gpui::test]
fn no_banner_is_rendered_when_no_connection_was_attempted(cx: &mut TestAppContext) {
    let colors = app::theme_colors();
    let (shell, cx) = shell(cx);

    assert!(
        shell.read_with(cx, |shell, _| shell.transport().is_none()),
        "this fixture must never start a transport: `Shell::new` publishes none, and \
         a shell that had one would draw a banner whatever the state said"
    );

    let announced = announcement(&ConnectionState::Disconnected, colors).expect(
        "`Disconnected` is a state worth reporting — the fixture is what the \
                 state alone would have drawn",
    );
    assert_eq!(
        announced.text, "Disconnected",
        "the content side of the rule is live here, which is what makes the missing \
         element below a decision rather than an accident"
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds(BANNER).is_none(),
        "a client with no configured server is offline by design and must draw no \
         failure banner: there is no connection to describe, and a permanent red \
         strip would be an apology for a choice the user made"
    );
}
