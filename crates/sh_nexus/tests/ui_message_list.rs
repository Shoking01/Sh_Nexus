//! The message list, headless: the harness, the rows, the seam.
//!
//! `docs/ARCHITECTURE.md` ADR-006's work order is six steps, and this file is
//! the tests for steps 2 through 5 — a `ListState` that renders headlessly, one
//! recycled `Entity<MessageRow>` per `client_msg_id`, a `core/markdown.rs` tree
//! turned into GPUI elements, and the state seam. Step 6 is a *measurement*
//! rather than a test, needs a real window, and is recorded as owed rather than
//! asserted here.
//!
//! # Why these tests go through the bridge rather than around it
//!
//! Every assertion below drives the **production path**: `bridge::install`, real
//! `DomainEvent`s delivered through the `EventSender`, `bridge::drain`, and a
//! real window whose view reads the state through `bridge::try_read`. There is
//! no mock and no test-only seam, which is the standard `tests/bridge.rs` holds
//! itself to, and for the same reason: a test that wires the layers together
//! differently from the application proves something about the test.
//!
//! # What the harness can and cannot prove on Windows
//!
//! Phase 0's finding 3 is the constraint: `current_headless_renderer()` returns
//! `Ok(None)` on Windows, so **no screenshot comparison is possible here**. What
//! *is* possible is layout, focus and state — which is what these tests assert:
//! real bounds from `ListState::bounds_for_item` and from `debug_bounds`, real
//! entity identity across a channel switch, real delivery state after an ACK.
//! `AGENTS.md` §7.3's `.text_color()` rule is asserted at the level it can be
//! asserted without pixels: every run `ui/markdown.rs` produces carries an
//! explicit colour, and the tests below read those colours back.
//!
//! # Test placement
//!
//! Cargo auto-discovers `tests/*.rs` only. A file at
//! `tests/ui/message_list.rs` would never be compiled and this suite would look
//! green while containing nothing (`PLAN.md` §4).

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use gpui::{
    div, list, prelude::*, px, Context, FontStyle, FontWeight, IntoElement, ListAlignment,
    ListState, Render, TestAppContext, TextStyle, VisualTestContext, Window,
};
use rstest::rstest;
use sh_nexus::core::markdown as core_markdown;
use sh_nexus::core::markdown::Block;
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::{Message, Reaction};
use sh_nexus::core::theme::BuiltIn;
use sh_nexus::state::bridge::{self, Delivery, EventSender};
use sh_nexus::state::{DeliveryState, MAX_MESSAGES_PER_CHANNEL};
use sh_nexus::ui::markdown as ui_markdown;
use sh_nexus::ui::views::message_list::MessageList;
use sh_nexus::ui::views::message_row::{RowCache, RowSpec, MAX_RETAINED_ROWS};
use sh_nexus::ui::Colors;
use smallvec::SmallVec;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The user this client is signed in as.
const ME: &str = "u_me";
/// A colleague, so an arriving message is somebody else's.
const THEM: &str = "u_them";
/// The channel the fixtures mostly use.
const CHANNEL: &str = "c_1";
/// A second channel, so a switch has somewhere to switch to.
const OTHER: &str = "c_2";

/// A timestamp the caller supplies, since this layer may not read a clock.
fn at(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_000_000 + second, 0)
        .single()
        .expect("the test timestamp is in range")
}

/// A deterministic identity, since no `Uuid` is generated in production yet.
fn cid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// A stored message as the server would deliver it.
fn stored_in(channel: &str, client_msg_id: u128, content: &str, second: i64) -> Message {
    Message {
        id: format!("m_{client_msg_id}"),
        client_msg_id: cid(client_msg_id),
        channel_id: channel.to_owned(),
        user_id: THEM.to_owned(),
        content: content.to_owned(),
        timestamp: at(second),
        edited_at: None,
        reactions: SmallVec::new(),
        thread_id: None,
        attachments: SmallVec::new(),
    }
}

/// A stored message in [`CHANNEL`].
fn stored(client_msg_id: u128, content: &str, second: i64) -> Message {
    stored_in(CHANNEL, client_msg_id, content, second)
}

/// A row spec with nothing but a body, for the cache tests.
fn spec_for(client_msg_id: u128, content: &str) -> RowSpec {
    RowSpec {
        client_msg_id: cid(client_msg_id),
        author: THEM.to_owned(),
        is_self: false,
        timestamp: at(1),
        document: Arc::new(core_markdown::parse(content)),
        delivery: None,
        failure_detail: None,
        reactions: Vec::new(),
    }
}

/// Installs the application state and returns the producer handle.
///
/// The `App`-only form of `update`, because no window exists yet and
/// `bridge::install` is documented to run before one is opened (§7.3, and
/// `src/lib.rs`'s `run` does exactly this).
fn installed(cx: &mut TestAppContext) -> EventSender {
    cx.update(|cx| bridge::install(cx, ME))
        .expect("the first install must succeed")
}

/// Delivers one event and applies it on the main thread.
fn delivered(cx: &mut VisualTestContext, sender: &EventSender, event: DomainEvent) {
    assert_eq!(
        sender.deliver(event),
        Delivery::Queued,
        "the fixtures are far under the inbox bound"
    );
    let report = cx
        .update(|_window, cx| bridge::drain(cx))
        .expect("the state must be installed");
    assert_eq!(
        report.refused(),
        0,
        "a fixture event must not be refused: refusals are policy outcomes, and \
         one here would mean the test is asserting against a state it never built"
    );
}

/// A connected state, so a send's optimism does not depend on `AppState`'s
/// default.
fn connected(cx: &mut VisualTestContext, sender: &EventSender) {
    delivered(
        cx,
        sender,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
}

// ---------------------------------------------------------------------------
// Step 2 — a ListState and a first row, headless
// ---------------------------------------------------------------------------

/// A view holding nothing but a `ListState` and a stub row.
///
/// ADR-006's step 2 asks for exactly this, and for exactly this reason: *"This
/// unit exists to prove the harness renders a `List` headlessly before any
/// content logic is layered on it — the Phase 0 spike proved the harness for a
/// plain view, not for a virtualized one."* No state, no bridge, no rows of
/// ours: a failure here is the harness or the framework, never this project's
/// rendering.
struct StubList(ListState);

impl Render for StubList {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        list(self.0.clone(), |_index, _window, _cx| {
            div()
                .h(px(20.))
                .w_full()
                .debug_selector(|| "stub-row".to_owned())
                .into_any_element()
        })
        .w_full()
        .h_full()
    }
}

/// The harness renders a bare `gpui::List` headlessly and measures its rows.
///
/// Top-aligned deliberately. A bottom-aligned list in `FollowMode::Tail` anchors
/// its scroll position at the *end sentinel*, and `bounds_for_item` answers
/// `None` for every index below it — which is the finding
/// [`only_visible_rows_are_built_and_a_row_can_be_located_off_the_tail`] pins on
/// the real list. Asserting a row's geometry needs an anchor that names a row,
/// so this is the alignment where the harness claim is testable at all.
#[gpui::test]
fn the_harness_renders_a_bare_list_headlessly(cx: &mut TestAppContext) {
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    let (_view, cx) = cx.add_window_view(|_, _| StubList(state.clone()));
    cx.run_until_parked();

    assert_eq!(state.item_count(), 3);

    let first = state.bounds_for_item(0);
    assert!(
        first.is_some_and(|bounds| bounds.size.height == px(20.)),
        "the stub row is 20px tall and must be measured as such, got {first:?}"
    );
    assert!(
        cx.debug_bounds("stub-row")
            .is_some_and(|bounds| bounds.size.height > px(0.)),
        "the row's own element must have painted with a non-empty size"
    );
}

/// The harness renders a **virtualized** list headlessly, and the row has real
/// bounds.
///
/// **This is the one thing the Phase 0 spike did not prove.** The spike proved
/// the harness for a plain view; ADR-006's step 2 exists because a virtualized
/// one is a different question, and it is the question this whole design rests
/// on: if `gpui::List` cannot lay out headlessly, none of the row, markdown or
/// seam work is testable at all.
///
/// Two independent measurements are asserted, deliberately: the container has
/// real width and height, and a row of it does too. `debug_bounds` asks the
/// **window**, which proves the elements were painted rather than merely built —
/// and per the spike's finding 2 it only answers because the elements carry
/// `.debug_selector()`. A test asserting only one of the two would pass for a
/// list whose rows rendered as nothing.
#[gpui::test]
fn the_list_renders_headlessly_and_a_row_has_bounds(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        0,
        "a list with no channel shows nothing, and says so before any message arrives"
    );

    for (n, body) in [(1_u128, "first"), (2, "second"), (3, "third")] {
        delivered(
            cx,
            &sender,
            DomainEvent::MessageReceived(stored(n, body, n as i64)),
        );
    }

    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        3,
        "the list must hold one item per message the state holds"
    );

    let row = cx
        .debug_bounds("message-row")
        .expect("a row element must have recorded its bounds");
    assert!(
        row.size.height > px(0.),
        "a row must be laid out with a non-zero height, got {:?}",
        row.size
    );

    let container = cx
        .debug_bounds("message-list")
        .expect("the list container must have recorded its bounds");
    assert!(container.size.height > px(0.), "the list must occupy space");
}

/// Only the rows near the viewport are built, and a row can be located once the
/// anchor leaves the tail.
///
/// **Two findings from the pinned `rev e683fd7`, both recorded because they are
/// behaviour a later work unit would otherwise have to rediscover.**
///
/// 1. **Virtualization is real and is asserted by count.** 40 messages produce
///    far fewer than 40 row entities, because `gpui::List` calls `render_item`
///    for the visible range plus the overdraw and for nothing else. This is
///    `AGENTS.md` §7.3's *"render only visible messages"* measured rather than
///    assumed.
/// 2. **`bounds_for_item` — and therefore ADR-006's *"jump to a message"* —
///    answers `None` while the list is anchored to the tail.** A tail-following
///    `ListAlignment::Bottom` list keeps `logical_scroll_top` at the *end
///    sentinel* (`item_ix == item_count`), and `bounds_for_item` returns `None`
///    for every index below it (`gpui/src/elements/list.rs:711-737`). So an
///    unread jump or a search jump must `pause_following_tail()` and move the
///    anchor first; it cannot ask where a row is while the list is at the
///    bottom. Asserting the `None` is the point: it is the precondition a Phase
///    3 or Phase 5 feature has to satisfy, and it is invisible from the API's
///    own documentation.
#[gpui::test]
fn only_visible_rows_are_built_and_a_row_can_be_located_off_the_tail(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    for n in 0..40_u128 {
        delivered(
            cx,
            &sender,
            DomainEvent::MessageReceived(stored(n, "a body long enough to be a row", n as i64)),
        );
    }
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    assert_eq!(view.read_with(cx, |list, _| list.item_count()), 40);
    let built = view.read_with(cx, |list, _| list.retained_rows());
    assert!(
        built < 40,
        "only the rows around the viewport may be built, got {built} rows for 40 messages"
    );

    // Finding 2: at the tail, the list cannot say where a row is.
    assert_eq!(
        view.read_with(cx, |list, _| list.list_state().bounds_for_item(0)),
        None,
        "a tail-anchored list reports no row bounds, which is what a jump must \
         pause following for"
    );

    // Move the anchor off the tail, and the same query answers.
    view.update_in(cx, |list, _window, cx| {
        list.pause_following_tail(cx);
        list.list_state().scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
    });
    cx.run_until_parked();

    let first = view.read_with(cx, |list, _| list.list_state().bounds_for_item(0));
    assert!(
        first.is_some_and(|bounds| bounds.size.height > px(0.)),
        "a row must have real bounds once the anchor is not the tail, got {first:?}"
    );
}

/// A reader scrolled into history keeps the row they are looking at when the
/// channel evicts its head.
///
/// **This is the UI consequence of `MAX_MESSAGES_PER_CHANNEL`, and the count is
/// not what it is about.** At the cap an arrival is one insertion and one
/// eviction, so the item count is *unchanged* — a test that asserted the count
/// would pass on a list that moved the reader's content up one row for every
/// message that arrived. So the assertion is on **which row is under the reader**:
///
/// 1. fill past the cap, so the head really is being evicted;
/// 2. scroll into history, off the tail, and record the `client_msg_id` the
///    reader is on by reading it back out of the list's own scroll offset;
/// 3. deliver one more message, which evicts the head;
/// 4. assert the anchored row is *still* the row at the list's scroll position —
///    by identity, and by asking the state where that identity now sits.
///
/// **Step 2's read-back is what makes step 4 mean anything.** Asserting that
/// `logical_scroll_top.item_ix` still equals some number would pass for a list
/// that held the *index* still while the content under it changed — which is the
/// exact defect. The identity is the thing that must not move.
///
/// **The evicted row is asserted to be gone too**, because a list that "kept the
/// reader steady" by refusing to reconcile would also pass the identity check.
#[gpui::test]
fn a_reader_scrolled_into_history_keeps_their_row_when_the_head_is_evicted(
    cx: &mut TestAppContext,
) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));
    connected(cx, &sender);

    // One past the cap, so the last arrival evicts.
    let total = MAX_MESSAGES_PER_CHANNEL + 1;
    for n in 0..total as u128 {
        delivered(
            cx,
            &sender,
            DomainEvent::MessageReceived(stored(n, "a body long enough to be a row", n as i64)),
        );
    }
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        MAX_MESSAGES_PER_CHANNEL,
        "the cap must have evicted on the way up, so the count is the cap and \
         not the number delivered"
    );

    // Into history: pause following, and move the anchor well away from both
    // ends so a one-row shift in either direction is unambiguous.
    let anchor_ix = 5_000;
    view.update_in(cx, |list, _window, cx| {
        list.pause_following_tail(cx);
        list.list_state().scroll_to(gpui::ListOffset {
            item_ix: anchor_ix,
            offset_in_item: px(0.),
        });
    });
    cx.run_until_parked();
    assert!(
        !view.read_with(cx, |list, _| list.is_following_tail()),
        "the reader must be off the tail for this to be a scroll-anchor question"
    );

    // Read the anchored identity back out of the list's own offset, and
    // confirm the state agrees about where it is. One `read_with`, so the list
    // and the state are read at the same instant and the pair cannot disagree
    // because the world moved between them.
    let (top_before, anchored_id) = view.read_with(cx, |list, app| {
        let top = list.list_state().logical_scroll_top();
        let held = bridge::try_read(app, |state| {
            state
                .messages(CHANNEL)
                .get(top.item_ix)
                .map(|message| message.client_msg_id)
        })
        .flatten();
        (top, held)
    });
    let anchored_id = anchored_id.expect("the anchored row must be a held message");
    assert_eq!(
        cx.read(
            |app| bridge::try_read(app, |state| { state.position_of(CHANNEL, &anchored_id) })
                .flatten()
        ),
        Some(top_before.item_ix),
        "the fixture must be coherent before the assertion means anything: the \
         identity at the list's scroll offset is the one the state places there"
    );

    // One more arrival: over the cap again, so the head is evicted.
    delivered(
        cx,
        &sender,
        DomainEvent::MessageReceived(stored(total as u128, "one more than the cap", total as i64)),
    );
    view.update_in(cx, |_list, _window, cx| cx.notify());
    cx.run_until_parked();

    let (top_after, at_top_after) = view.read_with(cx, |list, app| {
        let top = list.list_state().logical_scroll_top();
        let held = bridge::try_read(app, |state| {
            state
                .messages(CHANNEL)
                .get(top.item_ix)
                .map(|message| message.client_msg_id)
        })
        .flatten();
        (top, held)
    });

    assert_eq!(
        at_top_after,
        Some(anchored_id),
        "the row under the reader must be the row they were reading. Evicting \
         the head shifts every index below it by one, so holding the *index* \
         would leave the reader looking at the next message down"
    );
    assert_eq!(
        top_after.item_ix,
        top_before.item_ix - 1,
        "and the offset itself moved by exactly the one row the eviction took"
    );
    assert_eq!(
        top_after.offset_in_item, top_before.offset_in_item,
        "with the pixel offset inside the row untouched, so the reader's place \
         in the message they were reading is the same place"
    );
    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        MAX_MESSAGES_PER_CHANNEL,
        "and the count is reconciled, or the identity check would pass for a \
         list that simply stopped counting"
    );
    // An identity rather than a `&Message`: `bridge::try_read` hands out a borrow
    // of the state, and a borrow cannot outlive the `read` that produced it.
    assert_eq!(
        cx.read(|app| {
            bridge::try_read(app, |state| {
                state
                    .message(CHANNEL, &cid(0))
                    .map(|message| message.client_msg_id)
            })
            .flatten()
        }),
        None,
        "while the row that was actually evicted is gone"
    );
}

/// A message applied to the state reaches the list on the next frame, with
/// nobody calling `sync`.
///
/// **The property is that the list cannot drift from the state.** The seam
/// method is public so an event loop *can* reconcile eagerly, and `render` calls
/// it anyway — so a caller that forgets is not a client showing stale history.
/// The test reproduces exactly that: the message is applied to the state, the
/// view is merely asked to redraw, and nothing calls `sync` by hand.
#[gpui::test]
fn a_message_applied_to_the_state_reaches_the_list_on_the_next_frame(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |list, _| list.item_count()), 0);

    delivered(
        cx,
        &sender,
        DomainEvent::MessageReceived(stored(1, "arrived while idle", 1)),
    );

    // No `sync` call anywhere: just ask for a frame.
    view.update_in(cx, |_list, _window, cx| cx.notify());
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        1,
        "the render pass must reconcile the list with the state on its own"
    );
}

/// The list follows the tail, pauses without leaving tail mode, and resumes.
///
/// `pause_following_tail` rather than `FollowMode::Normal` is the distinction
/// worth pinning: `Normal` is a one-way door — nothing re-engages it when the
/// reader scrolls back down — while a paused tail resumes on its own. A test
/// that asserted only "following is off" would pass for both.
#[gpui::test]
fn tail_following_starts_engaged_and_pauses_without_leaving_tail_mode(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    // Enough messages that the content is taller than the viewport, which is
    // what makes "scrolled away from the bottom" mean anything. `List`
    // re-engages following on its own when the scroll position *is* the bottom
    // (`gpui/src/elements/list.rs:1211-1218`), so a pause with nothing to scroll
    // is a pause the next layout is entitled to undo.
    for n in 0..40_u128 {
        delivered(
            cx,
            &sender,
            DomainEvent::MessageReceived(stored(n, "a body long enough to be a row", n as i64)),
        );
    }
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    assert!(
        view.read_with(cx, |list, _| list.is_following_tail()),
        "a chat log opens on the newest message"
    );

    // Pause, and move the anchor off the tail. Pausing alone leaves the position
    // *at* the bottom, and `List` re-engages following when the position is the
    // bottom (`gpui/src/elements/list.rs:1211-1218`) — so a pause with nothing
    // scrolled is a pause the next layout is entitled to undo.
    view.update_in(cx, |list, _window, cx| {
        list.pause_following_tail(cx);
        list.list_state().scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
    });
    cx.run_until_parked();
    assert!(
        !view.read_with(cx, |list, _| list.is_following_tail()),
        "pausing must stop the snap to the newest message while the reader is away \
         from the bottom"
    );

    view.update_in(cx, |list, _window, cx| list.follow_tail(cx));
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |list, _| list.is_following_tail()),
        "the list must be able to resume following without being rebuilt"
    );
}

// ---------------------------------------------------------------------------
// Step 3 — row recycling
// ---------------------------------------------------------------------------

/// A row survives a channel switch, because it is keyed by `client_msg_id` and
/// the cache is not per-channel.
///
/// **Entity identity is the assertion, and it is the only assertion that can
/// distinguish recycling from rebuilding.** A rebuilt row produced the same
/// pixels last frame; what it did not do is keep anything it had. Asserting on
/// `retained_rows()` alone would pass for a cache that kept a row it never
/// handed back.
#[gpui::test]
fn a_row_is_the_same_row_after_switching_away_and_back(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    delivered(
        cx,
        &sender,
        DomainEvent::MessageReceived(stored(1, "the row that must survive", 1)),
    );
    delivered(
        cx,
        &sender,
        DomainEvent::MessageReceived(stored_in(OTHER, 2, "another channel's row", 2)),
    );

    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    let row = view
        .read_with(cx, |list, _| list.retained_row(&cid(1)))
        .expect("the visible row must be in the cache");
    let row_id = row.entity_id();

    // Away, and back.
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(OTHER, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        1,
        "a channel switch must show the other channel's messages and only those"
    );

    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    let same_row = view
        .read_with(cx, |list, _| list.retained_row(&cid(1)))
        .expect("the row must still be cached after a switch away and back");
    assert_eq!(
        same_row.entity_id(),
        row_id,
        "the row must be the same entity, not a new one with the same content"
    );
}

/// The cache is bounded, and evicts the oldest row first.
///
/// **`AGENTS.md` §7.1 forbids unbounded in-memory growth, and a row cache is
/// exactly the growth that is invisible until it is a leak.** The rows are
/// created directly rather than through a window, because the rows a viewport
/// renders are a screenful and proving a 512-row bound needs 520 of them — which
/// is why the bound is a constant a test reads instead of a number a test
/// guesses.
#[gpui::test]
fn the_row_cache_evicts_the_oldest_row_once_the_bound_is_reached(cx: &mut TestAppContext) {
    let mut cache = RowCache::new();
    let colors = Colors::default();
    let total = MAX_RETAINED_ROWS + 8;
    let ids: Vec<Uuid> = (0..total).map(|n| cid(n as u128 + 1)).collect();

    cx.update(|cx| {
        for id in &ids {
            cache.row(spec_for(id.as_u128(), "bounded"), colors, cx);
        }
    });

    assert_eq!(
        cache.len(),
        MAX_RETAINED_ROWS,
        "the cache must hold exactly the bound, not one more"
    );
    assert!(
        cache.get(&ids[0]).is_none(),
        "the oldest row must be the one evicted"
    );
    assert!(
        cache.get(&ids[total - 1]).is_some(),
        "the newest row must survive its own insertion"
    );
}

// ---------------------------------------------------------------------------
// Step 4 — the segment tree as GPUI elements
// ---------------------------------------------------------------------------

/// Whether a run carries bold weight.
fn is_bold(run: &gpui::TextRun) -> bool {
    run.font.weight == FontWeight::BOLD
}

/// Whether a run is italic.
fn is_italic(run: &gpui::TextRun) -> bool {
    run.font.style == FontStyle::Italic
}

/// Whether a run has a background, which is how inline code is drawn.
fn is_code(run: &gpui::TextRun) -> bool {
    run.background_color.is_some()
}

/// Whether a run is struck through.
fn is_struck(run: &gpui::TextRun) -> bool {
    run.strikethrough.is_some()
}

/// Each `core/markdown.rs` style bit becomes the GPUI text style it means.
///
/// **The runs are read back rather than rendered, and that is the only way to
/// assert this without pixels.** Phase 0's finding 3 rules out screenshot
/// comparison on Windows, so a test that wanted to *see* bold would have nothing
/// to look at; a test that reads the runs sees the weight, the slant, the
/// background and the strikethrough directly.
#[rstest]
#[case("**bold**", "bold", is_bold)]
#[case("*italic*", "italic", is_italic)]
#[case("`code`", "code", is_code)]
#[case("~~gone~~", "gone", is_struck)]
fn a_style_bit_becomes_the_gpui_text_style_it_means(
    #[case] source: &str,
    #[case] expected_text: &str,
    #[case] carries_the_style: fn(&gpui::TextRun) -> bool,
) {
    let document = core_markdown::parse(source);
    let Some(Block::Paragraph { spans }) = document.blocks().first() else {
        panic!(
            "`{source}` must parse to one paragraph, got {:?}",
            document.blocks()
        );
    };

    let (text, runs) = ui_markdown::runs(spans, Colors::default(), &TextStyle::default());

    assert_eq!(text, expected_text, "the run text must be the visible text");
    assert!(
        runs.iter().any(carries_the_style),
        "`{source}` must carry its style into a run, got {runs:?}"
    );
}

/// The runs cover the text exactly, byte for byte.
///
/// **This is the property `StyledText::with_runs` panics on.** GPUI slices the
/// accumulated text by each run's `len` and asserts the remainder is empty
/// (`gpui/src/elements/text.rs:526-537`), and the assert fires in release builds
/// too. So a renderer that loses a byte to a nested style, or counts characters
/// where the framework counts bytes, is a **crash** rather than a cosmetic bug —
/// and it is clean on Latin script, which is exactly the class of defect that
/// survives every hand-written case.
///
/// Multi-byte content is therefore the point of these cases, not decoration.
#[rstest]
#[case("plain")]
#[case("**bold** and *italic* and `code`")]
#[case("ünïcödé **and bold** — with an em dash")]
#[case("日本語のテキスト with **mixed** styles")]
#[case("[docs](https://example.com) and ~~struck~~")]
#[case("a `code` span and a [link](https://example.com/a/b) side by side")]
fn the_runs_cover_the_text_exactly(#[case] source: &str) {
    let document = core_markdown::parse(source);
    let mut checked = 0;

    for block in document.blocks() {
        if let Block::Paragraph { spans } | Block::Heading { spans, .. } = block {
            let (text, runs) = ui_markdown::runs(spans, Colors::default(), &TextStyle::default());
            let covered: usize = runs.iter().map(|run| run.len).sum();
            assert_eq!(
                covered,
                text.len(),
                "the runs must cover `{text}` exactly, in bytes: {runs:?}"
            );
            checked += 1;
        }
    }

    assert!(
        checked > 0,
        "`{source}` produced no inline content to check"
    );
}

/// Every run carries an explicit colour, and no run inherits one.
///
/// `AGENTS.md` §7.3: *"GPUI does not inherit color from parents. Always set
/// `.text_color()` on text elements."* For inline content the equivalent is a
/// `TextRun` whose `color` is set, and the failure mode of getting it wrong is
/// text that renders black on a dark background — which no structural assertion
/// would notice and which no screenshot test can catch on Windows.
#[test]
fn every_run_carries_an_explicit_colour() {
    let colors = Colors::default();
    let document = core_markdown::parse("plain **bold** [docs](https://example.com)");
    let Some(Block::Paragraph { spans }) = document.blocks().first() else {
        panic!("the source must parse to one paragraph");
    };

    let (_, runs) = ui_markdown::runs(spans, colors, &TextStyle::default());

    assert!(
        runs.iter()
            .all(|run| run.color == colors.text || run.color == colors.accent),
        "every run must carry one of this layer's explicit colours, got {runs:?}"
    );
    assert!(
        runs.iter().any(|run| run.color == colors.accent),
        "the link's run must take the accent colour"
    );
    assert!(
        runs.iter().any(|run| run.color == colors.text),
        "body text must take the text colour"
    );
    assert!(
        runs.iter().any(|run| run.underline.is_some()),
        "a link must be underlined as well as coloured, so the signal is not colour alone"
    );
}

/// The palette is derived from the built-in theme, and its literal fallback has
/// not drifted from `themes/dark.json`.
///
/// **A duplicated constant is only safe if something compares it to its
/// source.** `Colors::dark_fallback` exists so that `Colors::dark()` cannot fail
/// (see its documentation), which means the same colours are written in two
/// places; this is the test that fails when they disagree, rather than the two
/// drifting until a broken theme is the only way to notice.
#[test]
fn the_palette_matches_the_built_in_dark_theme_and_its_fallback() {
    let theme = BuiltIn::Dark
        .theme()
        .expect("the built-in dark theme must validate; tests/theme.rs asserts this too");

    assert_eq!(
        Colors::default(),
        Colors::from_palette(theme.colors()),
        "the default palette must be the built-in dark theme"
    );
    assert_eq!(
        Colors::dark_fallback(),
        Colors::from_palette(theme.colors()),
        "the literal fallback in ui/mod.rs has drifted from themes/dark.json"
    );
}

// ---------------------------------------------------------------------------
// Step 5 — the state seam
// ---------------------------------------------------------------------------

/// An optimistic send appears immediately, in `Pending`, and becomes `Acked` in
/// place when the server answers.
///
/// **`AGENTS.md` §8.1's Optimistic Send Flow, end to end through the seam.** The
/// row is asserted to be *pending* while the server has not answered — the half
/// that makes the flow optimistic — and then asserted to be `Acked` **in the same
/// row**, which is the half that makes the reconciliation an update rather than a
/// second message appearing.
#[gpui::test]
fn an_optimistic_send_is_pending_then_acked_in_place(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    connected(cx, &sender);
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    let send_id = cid(7);
    let sent = view.update_in(cx, |list, _window, cx| {
        list.begin_send("hello team", send_id, at(1), cx)
    });
    cx.run_until_parked();
    assert!(sent, "the send must put a row on screen");
    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        1,
        "the optimistic row must be in the list before any ACK"
    );

    let delivery = view.read_with(cx, |list, app| {
        list.retained_row(&send_id)
            .map(|row| row.read_with(app, |row, _| row.spec().delivery))
    });
    assert_eq!(
        delivery,
        Some(Some(DeliveryState::Pending)),
        "the row must be pending until the server answers"
    );

    delivered(
        cx,
        &sender,
        DomainEvent::MessageAcked {
            client_msg_id: send_id,
            message: stored(7, "hello team", 1),
        },
    );
    // The frame is asked for explicitly, which is what an event loop does after
    // `bridge::drain` — `sync` reconciles the count, and neither it nor the drain
    // schedules a redraw (`MessageList::sync`'s documentation).
    view.update_in(cx, |list, _window, cx| {
        list.sync(cx);
        cx.notify();
    });
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |list, _| list.item_count()),
        1,
        "the ACK must update the row, not add a second one for one identity"
    );

    let delivery = view.read_with(cx, |list, app| {
        list.retained_row(&send_id)
            .map(|row| row.read_with(app, |row, _| row.spec().delivery))
    });
    assert_eq!(
        delivery,
        Some(Some(DeliveryState::Acked)),
        "the ACK must be reconciled onto the row that was already there"
    );
}

/// The row carries the author, the time and the message's own markdown.
///
/// A row is asserted through its spec rather than through the element tree: the
/// spec is what the frame draws, and reading it back is the check Phase 0's
/// finding 3 permits.
#[gpui::test]
fn a_row_carries_its_message_s_markdown_and_metadata(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    delivered(
        cx,
        &sender,
        DomainEvent::MessageReceived(stored(1, "deploy **is** ready", 42)),
    );
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    let spec = view
        .read_with(cx, |list, app| {
            list.retained_row(&cid(1))
                .map(|row| row.read_with(app, |row, _| row.spec().clone()))
        })
        .expect("the visible row must be in the cache");

    assert_eq!(spec.client_msg_id, cid(1));
    assert_eq!(spec.author, THEM);
    assert!(!spec.is_self, "the fixture's author is somebody else");
    assert_eq!(spec.timestamp, at(42));
    assert_eq!(
        spec.document.plain_text(),
        "deploy is ready",
        "the row must hold the parsed body, not the raw source"
    );
}

/// A channel the state holds no messages for shows nothing, and shows it without
/// a panic.
///
/// The empty path is worth its own test because it is the one the state *will*
/// take in production while `network/` is still Phase 4: a channel the rail
/// offers, with no history behind it yet.
#[gpui::test]
fn a_channel_with_no_messages_shows_nothing(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));
    cx.run_until_parked();

    view.update_in(cx, |list, _window, cx| {
        let count = list.show_channel(CHANNEL, cx);
        assert_eq!(count, 0, "an empty channel has no rows");
    });
    cx.run_until_parked();

    assert_eq!(view.read_with(cx, |list, _| list.item_count()), 0);
    // `read_with` cannot return a borrow into the view, so copy the id out.
    assert_eq!(
        view.read_with(cx, |list, _| list.channel().map(str::to_owned)),
        Some(CHANNEL.to_owned()),
        "the view shows the channel it was told to show, even when it is empty"
    );
}

/// A row spec that has not changed is not redrawn, and one that has is.
///
/// **This is the property the remeasure obligation is built on.**
/// `MessageRow::set` reporting `false` for an unchanged spec is what keeps a
/// steady-state frame from re-rendering every visible row; reporting `true` when
/// the content changed is what makes the list remeasure it. Both halves are
/// asserted, including the one that is easy to get wrong: a different
/// `Arc<Document>` for the same text must count as a change, because the row is
/// holding a different parse.
#[gpui::test]
fn a_row_is_redrawn_only_when_its_spec_changed(cx: &mut TestAppContext) {
    let mut cache = RowCache::new();
    let colors = Colors::default();

    // The *same* spec, not a second one built from the same text: a spec is
    // rebuilt every frame in the real path, and the document it carries is an
    // `Arc` the state's cache handed out, so the second lookup is a pointer
    // comparison. Two separately parsed documents with identical text are
    // correctly a change, and `spec_for` builds one each call.
    let first = spec_for(1, "hello");
    let (row, changed) = cx.update(|cx| cache.row(first.clone(), colors, cx));
    assert!(changed, "a newly built row has never been measured");

    let (same, changed) = cx.update(|cx| cache.row(first, colors, cx));
    assert!(!changed, "an unchanged spec must not mark the row dirty");
    assert_eq!(
        same.entity_id(),
        row.entity_id(),
        "and it must be the same row"
    );

    let (_, changed) = cx.update(|cx| cache.row(spec_for(1, "hello, again"), colors, cx));
    assert!(changed, "a changed body must mark the row dirty");

    let mut edited = spec_for(1, "hello, again");
    edited.delivery = Some(DeliveryState::Pending);
    let (_, changed) = cx.update(|cx| cache.row(edited, colors, cx));
    assert!(
        changed,
        "a changed delivery state must mark the row dirty: the badge is part of the row"
    );
}

/// A reaction on a message reaches the row as an emoji and a count.
#[gpui::test]
fn a_reaction_reaches_the_row_as_a_chip(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));

    let mut message = stored(1, "react to me", 1);
    let mut reactions: SmallVec<[Reaction; 2]> = SmallVec::new();
    reactions.push(Reaction {
        emoji: "🎉".to_owned(),
        user_ids: vec![ME.to_owned(), THEM.to_owned()],
    });
    message.reactions = reactions;

    delivered(cx, &sender, DomainEvent::MessageReceived(message));
    view.update_in(cx, |list, _window, cx| {
        list.show_channel(CHANNEL, cx);
    });
    cx.run_until_parked();

    let reactions = view
        .read_with(cx, |list, app| {
            list.retained_row(&cid(1))
                .map(|row| row.read_with(app, |row, _| row.spec().reactions.clone()))
        })
        .expect("the visible row must be in the cache");

    assert_eq!(
        reactions,
        vec![("🎉".to_owned(), 2)],
        "a chip shows the emoji and how many people reacted"
    );
}

/// A view with no channel and no state does not panic.
///
/// The `try_read` doors all answer `None` when the bridge was never installed,
/// and `AGENTS.md` §2.1 forbids the `unwrap` that would turn that into a crash.
/// The app cannot reach this state — `src/lib.rs` installs before opening a
/// window — which is exactly why it is worth pinning: the failure mode of
/// getting it wrong is a startup panic nobody can reproduce.
#[gpui::test]
fn a_list_without_an_installed_bridge_renders_empty(cx: &mut TestAppContext) {
    assert!(
        cx.read(|app| !bridge::is_installed(app)),
        "this test deliberately does not install the state"
    );

    let (view, cx) = cx.add_window_view(|_, cx| MessageList::new(cx));
    cx.run_until_parked();

    assert_eq!(view.read_with(cx, |list, _| list.item_count()), 0);
    assert_eq!(
        view.read_with(cx, |list, _| list.retained_rows()),
        0,
        "nothing to draw means nothing cached"
    );
}
