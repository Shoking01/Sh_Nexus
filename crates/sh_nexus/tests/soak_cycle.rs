//! The send/ACK cycle, in a real window, at the history bound.
//!
//! `benches/frame_time.rs --mode soak` is a thirty-minute measurement of whether
//! the client's own structures grow under sustained use, and it is a measurement
//! of *something* only if the cycle it runs is the shipped one. This file is that
//! cycle, run a handful of times, asserted.
//!
//! **The bench is thirty minutes of wall clock and cannot be a test.** So each
//! property the soak's numbers rest on is pinned here instead, in a headless
//! window, in seconds. A soak whose waits were satisfied by the state rather
//! than by the renderer would publish a clean slope of nothing; these are the
//! tests that make that failure impossible to ship without noticing.
//!
//! # What each test is for
//!
//! | Claim the soak's report makes | Test | Kind |
//! |---|---|---|
//! | The ACK reconciles onto the row the renderer already built — no second row for one identity | [`a_send_and_its_ack_reconcile_onto_the_row_the_renderer_drew`] | runtime, real window |
//! | **The retained row's spec is a renderer signal, not a state signal** | [`the_row_the_soak_waits_on_only_changes_when_a_frame_draws`] | runtime, **negative** |
//! | A send that pushes the channel over the bound is not the row eviction takes, and the cap holds | [`a_send_past_the_cap_keeps_its_own_row_and_holds_the_bound`] | runtime, full history |
//! | The cycle can be repeated without the channel growing | [`repeated_cycles_hold_the_bound_and_reconcile_in_place`] | runtime, repeated |
//!
//! # What is not pinned here, and where it is pinned instead
//!
//! **The delivery map's bound is a property of `state/`, not of the cycle, so its
//! tests live in `tests/state_actions.rs`.** That map grew by one entry per
//! acknowledged send with no retirement path — `clear_outgoing` was reachable
//! only from `discard_failed_send`, which applies only to a *failed* send — and
//! `evict_one_over_cap` now retires an `Acked` send's entry along with the row it
//! evicts. A characterisation test for the leak was deleted from this file rather
//! than inverted, on its own instruction: a test that fails when a bug is fixed
//! is a canary, and its failure message said to delete it and record the fix in
//! `docs/BASELINES.md`. Three tests now hold the fixed behaviour in
//! `tests/state_actions.rs`, where the cap and the send-in-flight guard are
//! already tested:
//!
//! - `an_acknowledged_sends_delivery_entry_is_retired_with_the_row_it_is_evicted_with`
//! - `a_failed_sends_delivery_entry_survives_eviction`
//! - `a_row_the_server_moves_between_channels_keeps_its_delivery_entry`
//!
//! **The two merge paths are why the fix could not live in `remove_message`,**
//! which this file's deletion is a standing reminder of: two of its four callers
//! remove a row only to re-insert the same `client_msg_id` in another channel.
//!
//! # The test that could pass for the wrong reason
//!
//! **`the_row_the_soak_waits_on_only_changes_when_a_frame_draws` is a negative
//! test, and that is the whole point of it.** The bench waits for the retained
//! row's spec to reach `Acked` precisely because only a frame can do that. If a
//! future change made the spec track the state directly, the soak's wait would
//! still pass — one step earlier — and every figure it published would be a
//! measurement of a client that never drew. Nothing else in the suite would
//! catch it, because the soak looks identical when it is not.
//!
//! # Test placement
//!
//! Cargo auto-discovers `tests/*.rs` only. A file at `tests/ui/soak_cycle.rs`
//! would never be compiled and this suite would look green while containing
//! nothing (`PLAN.md` §4).

use chrono::{DateTime, TimeZone, Utc};
use gpui::{Entity, TestAppContext, VisualTestContext};
use sh_nexus::app::{Shell, DRAIN_INTERVAL, STARTUP_CHANNEL};
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::Message;
use sh_nexus::state::bridge::{self, Delivery};
use sh_nexus::state::{DeliveryState, MAX_MESSAGES_PER_CHANNEL};
use sh_nexus::UNSIGNED_IN_USER;
use smallvec::SmallVec;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A colleague, so the seeded history is somebody else's.
const THEM: &str = "u_them";

/// A timestamp the caller supplies, since `state/` may not read a clock.
fn at(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_000_000 + second, 0)
        .single()
        .expect("the test timestamp is in range")
}

/// A deterministic identity for a fixture the test has to name later.
fn cid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// A stored message in [`STARTUP_CHANNEL`], as the server would deliver it.
fn stored(client_msg_id: u128, content: &str, at: DateTime<Utc>) -> Message {
    Message {
        id: format!("m_{client_msg_id}"),
        client_msg_id: cid(client_msg_id),
        channel_id: STARTUP_CHANNEL.to_owned(),
        user_id: THEM.to_owned(),
        content: content.to_owned(),
        timestamp: at,
        edited_at: None,
        reactions: SmallVec::new(),
        thread_id: None,
        attachments: SmallVec::new(),
    }
}

/// A stored message **this client** authored, which is what an ACK reconciles
/// onto an optimistic row.
///
/// [`stored`] is somebody else's, so a self-send's ACK needs its own fixture or
/// the reconciled row would change author as well as delivery state, and the
/// delivery bookkeeping would skip it.
fn stored_by_me(client_msg_id: u128, content: &str, at: DateTime<Utc>) -> Message {
    Message {
        user_id: UNSIGNED_IN_USER.to_owned(),
        ..stored(client_msg_id, content, at)
    }
}

/// Installs the state and builds the shell in a real headless window.
///
/// **The production order, and it is the point:** `app::open` installs before a
/// window exists, so a shell built against an absent global would render an
/// empty list and satisfy half the assertions here for the wrong reason.
fn shell(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let sender = cx
        .update(|cx| bridge::install(cx, UNSIGNED_IN_USER))
        .expect("the first install must succeed");
    cx.add_window_view(move |_, cx| Shell::new(sender, cx))
}

/// The producer handle the shell retains, reached through its own accessor.
///
/// **Through `Shell::sender` rather than a handle the test kept**, because the
/// soak delivers its ACKs through the sender the shell holds and the test should
/// be the same door; a handle the fixture captured would prove a second thing
/// about a second handle.
fn queued(cx: &VisualTestContext, shell: &Entity<Shell>, event: DomainEvent) {
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.sender().deliver(event)),
        Delivery::Queued,
        "the shell holds the producer handle for the life of the window, so the inbox \
         it belongs to is still open"
    );
}

/// Lets the shell's drain pump run once.
///
/// **Through `advance_clock` and not `run_until_parked`.** `TestScheduler::run`
/// is `while step() {}` with no clock advancement, so parking returns with the
/// pump's timer still in the future and never fires it; `advance_clock` walks the
/// clock to the next expiry and polls the woken task
/// (`crates/scheduler/src/test_scheduler.rs`). One call runs the pump **once**,
/// and the next tick lands one interval further out — so this is the shell's
/// real schedule under test rather than a helper that shares its body.
fn tick(cx: &VisualTestContext) {
    cx.executor().advance_clock(DRAIN_INTERVAL);
    cx.run_until_parked();
}

/// A connected shell, so a send is a network send rather than an outbox one.
///
/// `AppState::can_send` is false in every other state, and a send made while
/// disconnected is the outbox's row — a path `PLAN.md` §7 gives to `db/` in
/// Phase 3 and which this build does not have.
fn connected(cx: &VisualTestContext, shell: &Entity<Shell>) {
    queued(
        cx,
        shell,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    tick(cx);
}

/// How many messages the shell's channel holds, through the seam.
fn held(cx: &VisualTestContext, shell: &Entity<Shell>) -> usize {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.message_count(STARTUP_CHANNEL)).unwrap_or(0)
    })
}

/// How many messages the shell's list is showing.
fn showing(cx: &VisualTestContext, shell: &Entity<Shell>) -> usize {
    shell.read_with(cx, |shell, app| {
        shell.list().read_with(app, |list, _| list.item_count())
    })
}

/// What the row the renderer built for `client_msg_id` is drawing.
///
/// **The soak's wait condition, read the way the soak reads it** — through the
/// row cache, which only `render_row` fills. `None` and `Some(None)` are
/// deliberately not distinguished: the soak only ever asks "has this reached
/// `Pending`/`Acked`", and a test that distinguished them would be asserting a
/// distinction the measurement does not make.
fn drawn_delivery(
    cx: &VisualTestContext,
    shell: &Entity<Shell>,
    client_msg_id: &Uuid,
) -> Option<DeliveryState> {
    shell.read_with(cx, |shell, app| {
        shell.list().read_with(app, |list, app| {
            list.retained_row(client_msg_id)
                .and_then(|row| row.read_with(app, |row, _| row.spec().delivery))
        })
    })
}

/// What the state says about one of this client's sends, through the seam.
///
/// **`try_read` returns `Option<Option<..>>` and both layers matter here.** The
/// outer is "the state is not installed" and the inner is the answer; a helper
/// that flattened them would report "no" for a state that was never there, and
/// a test asserting on that would pass for the wrong reason. So the seam's own
/// `Rendered`/`Delivery` distinction survives into every assertion below.
fn delivery_in_state(
    cx: &VisualTestContext,
    shell: &Entity<Shell>,
    id: &Uuid,
) -> Option<DeliveryState> {
    let answer = shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.delivery(id))
    });
    answer.expect("the state must be installed: the fixture installs it")
}

/// Whether the channel still holds one message, through the seam.
///
/// **`is_some` inside the closure rather than a borrowed `&Message` out of it**,
/// for the reason `try_read`'s own documentation gives: its `R` cannot borrow
/// from the state, so a read must return owned data. A boolean is owned data.
fn holds_message(cx: &VisualTestContext, shell: &Entity<Shell>, id: &Uuid) -> bool {
    let answer = shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.message(STARTUP_CHANNEL, id).is_some())
    });
    answer.expect("the state must be installed: the fixture installs it")
}
/// One optimistic send, through the production door, and a frame to draw it.
///
/// **Exactly the first two steps of the soak's cycle**, including the wait: the
/// row must be on screen *and* drawn as `Pending` before the ACK is delivered,
/// because an optimistic row nobody ever rendered is not an optimistic row.
fn begin_send(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    client_msg_id: Uuid,
    content: &str,
    at: DateTime<Utc>,
) {
    let on_screen = shell.update_in(cx, |shell, _window, cx| {
        shell.list().update(cx, |list, list_cx| {
            list.begin_send(content, client_msg_id, at, list_cx)
        })
    });
    assert!(
        on_screen,
        "the optimistic send must put a row on screen; a refusal here means the \
         identity was already held, which is the failure the soak treats as fatal"
    );
    cx.run_until_parked();
    assert_eq!(
        drawn_delivery(cx, shell, &client_msg_id),
        Some(DeliveryState::Pending),
        "and the row must be DRAWN pending before any ACK, which is what \
         AGENTS.md 8.1's Optimistic Send Flow means"
    );
}

/// The server's answer, delivered and drained and drawn.
fn acked(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    client_msg_id: Uuid,
    content: &str,
    at: DateTime<Utc>,
) {
    queued(
        cx,
        shell,
        DomainEvent::MessageAcked {
            client_msg_id,
            message: stored_by_me(client_msg_id.as_u128(), content, at),
        },
    );
    tick(cx);
    shell.update_in(cx, |shell, _window, cx| {
        shell.list().update(cx, |_list, list_cx| list_cx.notify());
    });
    cx.run_until_parked();
    assert_eq!(
        drawn_delivery(cx, shell, &client_msg_id),
        Some(DeliveryState::Acked),
        "the reconciliation must be DRAWN, not merely applied: the soak waits for \
         exactly this, and a state read would have passed a step earlier"
    );
}

/// A [`STARTUP_CHANNEL`] filled to [`MAX_MESSAGES_PER_CHANNEL`], in batches.
///
/// **Batched rather than one event at a time, and the batch is the inbox's own
/// bound.** `bridge::MAX_PENDING_EVENTS` is what a legitimate burst may be, and
/// the pump drains a whole batch on one tick, so filling 10 000 rows costs ten
/// ticks rather than ten thousand. The alternative — `ui_message_list.rs`'s
/// `filled_to_the_cap`, one drain per arrival — is the right shape for a test
/// that needs every arrival reconciled individually, and the wrong shape for one
/// that only needs a full channel.
///
/// **The identities are [`cid`]'s `0..CAP` and the timestamps are `at(0..CAP)`,
/// and both ranges are stated here because a caller has to place its own fixture
/// after them.** The soak places every send *after* the whole history
/// (`at(TOTAL_MESSAGES + iteration)`) and this file's callers do the same; a
/// fixture that reused one of these identities, or one of these timestamps, would
/// silently reconcile against a row it did not mean to.
fn filled_to_the_cap(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
    let mut seeded = 0usize;
    while seeded < MAX_MESSAGES_PER_CHANNEL {
        let batch_end = (seeded + bridge::MAX_PENDING_EVENTS).min(MAX_MESSAGES_PER_CHANNEL);
        for n in seeded..batch_end {
            queued(
                cx,
                shell,
                DomainEvent::MessageReceived(stored(
                    n as u128,
                    "history a row long enough to be one",
                    at(n as i64),
                )),
            );
        }
        tick(cx);
        seeded = batch_end;
    }
    assert_eq!(
        held(cx, shell),
        MAX_MESSAGES_PER_CHANNEL,
        "the fixture must be exactly at the cap: one more arrival evicts, and not one \
         fewer"
    );
}

/// A timestamp strictly after everything [`filled_to_the_cap`] put in the channel.
///
/// **A named helper because getting this wrong is silent, and it was.** The first
/// version of the cap tests passed `at(0)` — the *oldest* second in the fixture —
/// so `insert_at_order` appended nothing: the send landed at the head of 10 000
/// rows, the list was following the tail, and the row was never rendered. The
/// optimistic row was in the state and invisible, which is the one state the
/// Optimistic Send Flow is *about* and the one a test must never assert against.
/// A timestamp after the history is what makes "the newest row" true.
fn after_the_history(cycle: u128) -> DateTime<Utc> {
    at(MAX_MESSAGES_PER_CHANNEL as i64 + cycle as i64)
}

// ---------------------------------------------------------------------------
// 1. The cycle reconciles in place
// ---------------------------------------------------------------------------

/// A send and its ACK reconcile onto the row the renderer already built.
///
/// **`AGENTS.md` §8.1's Optimistic Send Flow, end to end, through the shell.** The
/// count is asserted after the ACK as well as before it, because "the row becomes
/// `Acked`" and "there is still only one row for one identity" are different
/// claims, and a reconciliation that appended rather than merged would satisfy
/// the first and fail the second. This is the cycle `--mode soak` runs, once.
#[gpui::test]
fn a_send_and_its_ack_reconcile_onto_the_row_the_renderer_drew(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    connected(cx, &shell);

    let send = cid(7);
    begin_send(cx, &shell, send, "hello team", at(1));
    assert_eq!(
        showing(cx, &shell),
        1,
        "the optimistic row is on screen from the gesture, before the server has seen \
         anything"
    );

    acked(cx, &shell, send, "hello team", at(1));

    assert_eq!(
        showing(cx, &shell),
        1,
        "the ACK must update the row, not add a second one for one identity -- the \
         failure is silent, and a channel that quietly doubles its rows is exactly the \
         shape a RAM slope would hide"
    );
    assert_eq!(
        held(cx, &shell),
        1,
        "and the state holds one row for it, which is the claim the count above is a \
         proxy for"
    );
}

// ---------------------------------------------------------------------------
// 2. The soak's wait is a renderer wait, and that is a negative claim
// ---------------------------------------------------------------------------

/// The retained row's spec does not change until a frame draws.
///
/// **The negative test the whole soak rests on, and the reason it exists in this
/// form rather than as part of the test above.** `--mode soak` waits for the
/// retained row's spec to reach `Acked`, on the ground that only a frame can
/// change it. If that stopped being true the bench would keep working and start
/// lying: the wait would be satisfied by the drain alone, and a run whose window
/// had stopped drawing would publish a clean slope of nothing.
///
/// So the ACK is delivered and **drained** — the state is genuinely `Acked` —
/// and the row is then read with no frame in between. It still says `Pending`.
/// One `notify` and one frame is all it takes to change it.
///
/// **The drain is called directly rather than left to the pump, and the reason
/// is the harness rather than the claim.** `advance_clock` runs the test
/// scheduler `while step() {}`, and a step that wakes the drain pump also runs
/// the frame its `notify` asked for — so going through the pump cannot hold the
/// two apart, and the first version of this test asserted something the harness
/// makes impossible rather than something the client is. Draining on demand is
/// what `tests/bridge.rs` and `tests/ui_message_list.rs` already do; what it must
/// not become is a *second* drainer in the bench, which is the schedule
/// `app.rs`'s own module docs rule out. A test may drain deliberately; a client
/// may not have two.
#[gpui::test]
fn the_row_the_soak_waits_on_only_changes_when_a_frame_draws(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    connected(cx, &shell);

    let send = cid(11);
    begin_send(cx, &shell, send, "held until it is drawn", at(1));

    queued(
        cx,
        &shell,
        DomainEvent::MessageAcked {
            client_msg_id: send,
            message: stored_by_me(send.as_u128(), "held until it is drawn", at(1)),
        },
    );
    // The drain itself, with no notify and no frame after it: `bridge::drain`'s
    // own documentation says it schedules no redraw, and that is the whole basis
    // of the assertion below.
    let drained = cx.update(|_window, cx| bridge::drain(cx));
    let report = drained.expect("the state is installed: the fixture installed it");
    assert_eq!(
        (report.applied(), report.refused()),
        (1, 0),
        "the ACK must be applied for this test to mean anything; a refusal would \
         leave the row Pending for the wrong reason"
    );
    assert_eq!(
        delivery_in_state(cx, &shell, &send),
        Some(DeliveryState::Acked),
        "the STATE is Acked now, before any frame has run"
    );

    // Still no `notify`, still no `run_until_parked`.
    assert_eq!(
        drawn_delivery(cx, &shell, &send),
        Some(DeliveryState::Pending),
        "but the ROW still draws Pending, which is the property the soak's wait \
         depends on. If this fails, the bench is waiting on the state and every figure \
         it publishes describes a client that never drew a frame"
    );

    // And one frame is all it takes.
    shell.update_in(cx, |shell, _window, cx| {
        shell.list().update(cx, |_list, list_cx| list_cx.notify());
    });
    cx.run_until_parked();
    assert_eq!(
        drawn_delivery(cx, &shell, &send),
        Some(DeliveryState::Acked),
        "one frame after the drain, the row draws what the state already said -- and \
         nothing before that frame did"
    );
}

// ---------------------------------------------------------------------------
// 3. The history bound, exercised the way the soak exercises it
// ---------------------------------------------------------------------------

/// A send that pushes the channel over the bound keeps its own row, and the bound
/// still holds.
///
/// **This is work unit 3C's trap, met through the path the soak uses.** 3C
/// documented that eviction skips a row this client has an outstanding send for,
/// because `actions::acknowledge` asks `channel_holding(..)` first and an
/// evicted pending send would be re-inserted by the adoption path as a silent
/// reordering. 3C proved it against the state; this proves the *cycle* the
/// thirty-minute run performs twenty thousand times does not trip it.
///
/// **The full history is paid for in full, because there is no cheaper door.** A
/// view cannot be made to drop a row it is still being shown, so a fixture that
/// mocked the eviction would prove nothing about the thing being claimed.
#[gpui::test]
fn a_send_past_the_cap_keeps_its_own_row_and_holds_the_bound(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    connected(cx, &shell);
    filled_to_the_cap(cx, &shell);

    // Out of the identity range the seeded history uses, so the ACK reconciles
    // against the optimistic row instead of against a message the server also
    // knows about.
    const CLEAR_OF_THE_HISTORY: u128 = 1_000_000;
    let send = cid(CLEAR_OF_THE_HISTORY);
    begin_send(cx, &shell, send, "one over the bound", after_the_history(0));

    assert_eq!(
        held(cx, &shell),
        MAX_MESSAGES_PER_CHANNEL,
        "the insert is one over the cap and the eviction is what brings it back, so the \
         held count is read after the insert has already been balanced. A count of \
         cap+1 here would mean nothing evicted"
    );
    assert!(
        holds_message(cx, &shell, &send),
        "and the send survives its own overflow: it is the newest row, which eviction \
         never reaches, and a client that dropped it would show the user their message \
         vanishing before the server answered"
    );
    assert!(
        !holds_message(cx, &shell, &cid(0)),
        "while the oldest seeded row is the one that went: the cap is a real bound, not \
         a suspension"
    );

    acked(cx, &shell, send, "one over the bound", after_the_history(0));
    assert_eq!(
        held(cx, &shell),
        MAX_MESSAGES_PER_CHANNEL,
        "and the reconciliation does not evict a second time: it removes one row and \
         re-inserts one, so the count is unchanged"
    );
}

/// The cycle can be repeated, and the channel does not grow.
///
/// **The property the soak's slope column would be read against, at a size where
/// a test can finish.** Each cycle is a fresh identity, an optimistic send, and
/// the ACK that resolves it — the same four steps `--mode soak` runs, at the
/// same cap. The assertion is on the count and on the *identity*: `CYCLES`
/// cycles on a channel of `MAX_MESSAGES_PER_CHANNEL` rows must leave the channel
/// at the cap and every one of the `CYCLES` identities acknowledged, which is
/// what "the cycle does real work and the work does not accumulate" means.
///
/// **One test with a loop rather than an `#[rstest]` over two cycle counts, and
/// the reason is the harness.** `#[gpui::test]` and `#[rstest]` cannot be
/// stacked — one provides the `TestAppContext` and the other owns the attribute
/// — and a full 10 000-row channel per case is not something to build twice. A
/// loop inside the one window also *is* the experiment: the later cycles run
/// against a channel whose head has already moved, which is the state the soak's
/// twenty thousandth cycle is in and which a fresh fixture per case would not
/// reach.
#[gpui::test]
fn repeated_cycles_hold_the_bound_and_reconcile_in_place(cx: &mut TestAppContext) {
    /// How many cycles to run. Small enough that the test is seconds, large
    /// enough that the channel's head has demonstrably moved past the fixture.
    const CYCLES: u128 = 24;

    let (shell, cx) = shell(cx);
    connected(cx, &shell);
    filled_to_the_cap(cx, &shell);

    /// Out of the identity range the seeded history uses, so each ACK
    /// reconciles against its own optimistic row.
    const CLEAR_OF_THE_HISTORY: u128 = 1_000_000;
    for n in 0..CYCLES {
        let send = cid(CLEAR_OF_THE_HISTORY + n);
        let when = after_the_history(n);
        begin_send(cx, &shell, send, "one cycle of the soak", when);
        acked(cx, &shell, send, "one cycle of the soak", when);

        assert_eq!(
            drawn_delivery(cx, &shell, &send),
            Some(DeliveryState::Acked),
            "cycle {n}: each identity must resolve on its own row"
        );
        assert_eq!(
            held(cx, &shell),
            MAX_MESSAGES_PER_CHANNEL,
            "cycle {n}: the bound holds through every cycle, which is the claim the \
             soak's held-history row makes over thirty minutes"
        );
    }
}
