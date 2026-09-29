//! The bridge is a real seam, exercised against a real GPUI application.
//!
//! # Why these tests use a headless app and not a plain `#[test]`
//!
//! `PLAN.md` §10 makes the whole test strategy depend on `gpui/test-support`
//! being usable on the target platform, and `tests/spike_render.rs` is the
//! proof that it is. **A bridge tested with a plain `#[test]` would test nothing
//! that matters**, because everything load-bearing about this file is the
//! interaction with GPUI: that a global registers, that `cx.update_global` is
//! the only way to reach it, and that the state it holds is visible from inside
//! a rendered entity. All of that needs a `TestAppContext`.
//!
//! # What is checked here, and by what kind of evidence
//!
//! | Property | Test | Kind |
//! |---|---|---|
//! | The global registers and is recoverable with `cx.global::<T>()` | [`the_global_registers_and_is_recoverable_through_cx_global`] | runtime, real app |
//! | An event from a **real thread** mutates state a **real window** sees | [`an_event_delivered_from_a_worker_thread_mutates_state_in_a_real_window`] | runtime |
//! | `App`, `Context` and `AsyncApp` are `!Send` | [`a_main_thread_context_cannot_be_sent_to_another_thread`] | compile-time, both directions |
//! | The installed global is `Send` but **not** `Sync` | [`the_installed_global_can_be_moved_but_never_shared`] | compile-time, both directions |
//! | `network/` cannot reach the main-thread context | [`network_cannot_reach_the_main_thread_context`] | scanner |
//! | Only this file mutates a GPUI context | [`bridge_is_the_only_file_that_calls_the_context_mutating_api`] | scanner |
//! | Only this file constructs an `AppState` | [`only_the_bridge_constructs_an_application_state`] | scanner |
//! | The bridge exposes no `&mut AppState` | [`the_bridge_never_names_a_mutable_borrow_of_the_state`] | scanner |
//! | This file holds no interior mutability | [`bridge_holds_no_interior_mutability`] | scanner |
//! | The scanner itself can see a violation | [`the_bridge_scanner_sees_code_and_not_prose`] | scanner, on synthetic source |
//! | The inbox is bounded and a full one hands the event back | [`a_full_inbox_refuses_the_delivery_and_returns_the_event`] | runtime |
//! | A second install is refused and the first inbox survives | [`installing_twice_is_refused_and_orphans_nothing`] | runtime |
//! | `delivered == applied + refused` across a mixed burst | [`the_drain_report_accounts_for_every_event_it_took`] | runtime |
//! | Every door reports a missing install instead of panicking | [`every_door_reports_a_missing_install_instead_of_panicking`] | runtime |
//! | `try_rendered` promotes on a hit and is inert on a miss | [`a_rendered_probe_promotes_on_a_hit_and_is_inert_on_a_miss`] | runtime |
//! | `try_begin_send` takes the clock from its caller | [`a_send_through_the_bridge_uses_the_caller_s_timestamp`] | runtime |
//! | No message content in any line this module renders | [`a_refusal_renders_a_line_with_no_message_content`] | runtime |

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use gpui::{div, prelude::*, App, FocusHandle, Focusable, Render, TestAppContext, Window};
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::Message;
use sh_nexus::core::models::user::UserStatus;
use sh_nexus::state::actions::{ApplyOutcome, IgnoreReason};
use sh_nexus::state::app_state::AppState;
use sh_nexus::state::bridge::{
    self, AppStateGlobal, Delivery, DeliveryRefusal, DrainReport, EventSender, InstallError,
    Rendered, MAX_PENDING_EVENTS,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The user this client is signed in as.
const ME: &str = "u_me";
/// A colleague, so an arriving message is somebody else's.
const THEM: &str = "u_them";
/// The only channel the fixtures use.
const CHANNEL: &str = "c_1";

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

/// A stored message as the server would deliver it: a real id, a real
/// timestamp, and a body chosen to be recognisable if any log line ever carried
/// it.
fn stored(client_msg_id: u128, content: &str, second: i64) -> Message {
    Message {
        id: format!("m_{client_msg_id}"),
        client_msg_id: cid(client_msg_id),
        channel_id: CHANNEL.to_owned(),
        user_id: THEM.to_owned(),
        content: content.to_owned(),
        timestamp: at(second),
        edited_at: None,
        reactions: smallvec::SmallVec::new(),
        thread_id: None,
        attachments: smallvec::SmallVec::new(),
    }
}

// ---------------------------------------------------------------------------
// 1. The global
// ---------------------------------------------------------------------------

/// The state registers as a GPUI global and is recoverable with
/// `cx.global::<T>()`.
///
/// **The retrieval is the load-bearing half.** `App::global` panics when no
/// value of the type has been assigned (`gpui/src/app.rs:2129-2134`), so a test
/// that reaches the retrieval with a real `App` has proved both that `install`
/// wrote the global and that the type is reachable by the documented mechanism
/// rather than by a side channel.
#[gpui::test]
fn the_global_registers_and_is_recoverable_through_cx_global(cx: &mut TestAppContext) {
    cx.read(|app| {
        assert!(
            !bridge::is_installed(app),
            "a fresh app has no application state, and the door must say so"
        );
    });

    cx.update(|cx| {
        let outcome = bridge::install(cx, ME);
        assert!(
            outcome.is_ok(),
            "the first install must succeed, got {:?}",
            outcome.as_ref().err()
        );
    });

    // The documented retrieval path, on a real `App`.
    cx.read(|app| {
        assert!(app.has_global::<AppStateGlobal>());
        // Panics if the global was not assigned, which is the assertion.
        let _recovered: &AppStateGlobal = app.global::<AppStateGlobal>();
        assert!(bridge::is_installed(app));
    });
}

/// Installing twice is refused, and the first inbox is not orphaned.
///
/// **The property worth having is the second half.** A naive second
/// `set_global` would drop the first `Receiver`, which would make every
/// already-queued event unreachable and turn the first [`EventSender`] into a
/// silent black hole. The test queues an event, refuses the second install, and
/// then drains — so the refusal is proved to have changed nothing.
#[gpui::test]
fn installing_twice_is_refused_and_orphans_nothing(cx: &mut TestAppContext) {
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("the first install must succeed");

    assert_eq!(
        sender.deliver(DomainEvent::ConnectionStateChanged(
            ConnectionState::Connected
        )),
        Delivery::Queued,
        "the event should be in the first inbox"
    );

    cx.update(|cx| {
        assert_eq!(
            bridge::install(cx, "u_someone_else").err(),
            Some(InstallError::AlreadyInstalled),
            "a second install must be refused rather than replacing the state"
        );
    });

    let report = cx.update(bridge::drain).expect("the state is installed");
    assert_eq!(
        report.delivered(),
        1,
        "the event queued before the refused install must still be applied"
    );
    cx.read(|app| {
        assert!(
            bridge::try_read(app, AppState::can_send).unwrap_or(false),
            "the drained event took effect, so the first state is the live one"
        );
    });
}

/// Every door reports a missing install instead of panicking.
///
/// `AGENTS.md` §2.1 forbids a panic on a path a user can reach, and a
/// startup-ordering mistake is exactly that. **Every** entry point is exercised
/// here, because the failure this guards against is a new door added without the
/// guard clause — and a door that panics on the missing global is the shape that
/// bug takes.
#[gpui::test]
fn every_door_reports_a_missing_install_instead_of_panicking(cx: &mut TestAppContext) {
    cx.update(|cx| {
        assert!(
            !bridge::is_installed(cx),
            "nothing has been installed, which is the point"
        );
        assert_eq!(
            bridge::try_apply_event(
                cx,
                DomainEvent::ConnectionStateChanged(ConnectionState::Connected)
            ),
            None
        );
        assert_eq!(bridge::drain(cx), None);
        assert_eq!(bridge::try_select_channel(cx, CHANNEL), None);
        assert_eq!(
            bridge::try_begin_send(cx, CHANNEL, "hello", cid(1), at(0)),
            None
        );
        assert_eq!(bridge::try_render_and_cache(cx, cid(1), "body", 8), None);
        assert_eq!(bridge::try_rendered(cx, &cid(1)), Rendered::NotInstalled);
        assert_eq!(
            bridge::try_read(cx, |state| state.self_user_id().to_owned()),
            None
        );
    });
}

// ---------------------------------------------------------------------------
// 2. The event path, end to end
// ---------------------------------------------------------------------------

/// An event delivered from a **real** thread reaches the state, and a **real**
/// window's view sees it.
///
/// **This is the test the work unit exists to be able to write.** The spike
/// proved clicks and keystrokes reach an element tree; this proves the whole
/// production path — producer thread, bounded channel, `drain`, `actions.rs`,
/// global, rendered entity — with no mock anywhere in it.
///
/// [`StateView`] reads the state through [`bridge::try_read`] inside its own
/// `render`, so the assertion is that the **frame** shows the arrived message
/// and not merely that a variable did.
#[gpui::test]
fn an_event_delivered_from_a_worker_thread_mutates_state_in_a_real_window(cx: &mut TestAppContext) {
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");

    let (view, cx) = cx.add_window_view(|_, cx| StateView::new(cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.seen()),
        0,
        "the channel starts empty, and the first frame says so"
    );

    // A real OS thread, holding nothing but the sender. A clone stays here so the
    // inbox is still open afterwards -- a producer that has finished is a closed
    // producer, and `a_disconnected_inbox_is_reported_once` covers that case.
    let kept = sender.clone();
    let delivered = std::thread::spawn(move || {
        sender.deliver(DomainEvent::MessageReceived(stored(
            1,
            "hello from the other thread",
            10,
        )))
    })
    .join()
    .expect("the producer thread must not panic");
    assert_eq!(delivered, Delivery::Queued);

    // Nothing has been applied yet: delivery and application are different
    // steps, and the separation is the whole point of the seam.
    cx.read(|app| {
        assert_eq!(
            bridge::try_read(app, |state| state.message_count(CHANNEL)),
            Some(0),
            "a queued event must not change state before the main thread drains it"
        );
    });

    let report = cx
        .update(|_window, cx| bridge::drain(cx))
        .expect("the state is installed");
    assert_eq!(report.delivered(), 1);
    assert_eq!(report.applied(), 1);
    assert_eq!(report.refused(), 0);
    assert!(
        !report.is_disconnected(),
        "the kept clone is still alive, so the inbox is open"
    );
    drop(kept);

    // The frame now shows it. `run_until_parked` flushes the redraw the global
    // mutation asked for.
    view.update_in(cx, |_view, _window, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.seen()),
        1,
        "a real window's view must observe the message that arrived on another thread"
    );
}

/// A `DomainEvent` is `Send`, and `EventSender` is `Send + Sync + Clone`.
///
/// **The positive half of the seam's `Send` story, asserted at compile time and
/// then exercised, because a `Send` claim nobody has moved is a claim nobody
/// has checked.** The negative half — that the context cannot be moved — is
/// [`a_main_thread_context_cannot_be_sent_to_another_thread`].
#[gpui::test]
fn the_producer_side_is_send_and_clone(cx: &mut TestAppContext) {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send::<DomainEvent>();
    assert_send_sync::<EventSender>();
    assert_send_sync::<Delivery>();
    assert_send_sync::<DeliveryRefusal>();
    assert_send_sync::<DrainReport>();

    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");
    // Clone, because a network module has more than one producer and giving
    // each its own channel would mean several inboxes racing one state.
    let second = sender.clone();
    assert_eq!(second.capacity(), sender.capacity());
    assert_eq!(second.capacity(), MAX_PENDING_EVENTS);

    // Both clones reach the one inbox, in order.
    assert_eq!(
        second.deliver(DomainEvent::ConnectionStateChanged(
            ConnectionState::Connected
        )),
        Delivery::Queued
    );
    assert_eq!(
        sender.deliver(DomainEvent::ConnectionStateChanged(
            ConnectionState::Disconnected
        )),
        Delivery::Queued
    );
    let report = cx.update(bridge::drain).expect("the state is installed");
    assert_eq!(report.delivered(), 2, "one inbox, two producers, no loss");
}

/// A full inbox refuses the delivery and hands the event back.
///
/// **`AGENTS.md` §7.1's "no unbounded growth" is only kept by a bound that is
/// actually enforced, and this is the test that says the bound binds.** The
/// event comes back inside the refusal: a refusal that consumed it would be a
/// silent drop wearing a return type, which is the failure `AGENTS.md` §7.5 and
/// `PLAN.md` §7 both name.
#[gpui::test]
fn a_full_inbox_refuses_the_delivery_and_returns_the_event(cx: &mut TestAppContext) {
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");
    assert_eq!(
        sender.capacity(),
        MAX_PENDING_EVENTS,
        "the sender must report the bound the bridge was built with"
    );

    for n in 0..MAX_PENDING_EVENTS {
        assert_eq!(
            sender.deliver(DomainEvent::ConnectionStateChanged(
                ConnectionState::Connecting
            )),
            Delivery::Queued,
            "event {n} of {MAX_PENDING_EVENTS} should fit"
        );
    }

    let overflow = stored(9_999, "the one that did not fit", 0);
    let outcome = sender.deliver(DomainEvent::MessageReceived(overflow.clone()));
    assert_eq!(
        outcome,
        Delivery::Refused(DeliveryRefusal::InboxFull {
            capacity: MAX_PENDING_EVENTS,
            event: Box::new(DomainEvent::MessageReceived(overflow)),
        }),
        "the delivery past the bound must be refused, with the event returned"
    );
    assert_eq!(
        outcome.to_string(),
        "refused: the event inbox is full at 1024",
        "the refusal names the bound and carries no message content"
    );

    // The refusal is reported, not stored: the inbox holds exactly the bound.
    let report = cx.update(bridge::drain).expect("the state is installed");
    assert_eq!(report.delivered(), MAX_PENDING_EVENTS);
}

/// A disconnected inbox is reported, so a drain loop knows to stop.
///
/// **The `is_disconnected` flag exists to stop a `loop { drain(cx) }` from
/// spinning on a permanently closed channel,** and a flag nothing checks is a
/// flag that will not be checked. This asserts it flips exactly when the last
/// producer is gone, and not before.
#[gpui::test]
fn a_disconnected_inbox_is_reported_once(cx: &mut TestAppContext) {
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");

    let first = cx.update(bridge::drain).expect("the state is installed");
    assert!(
        !first.is_disconnected(),
        "the sender is alive, so the inbox is not closed"
    );

    drop(sender);

    let second = cx.update(bridge::drain).expect("the state is installed");
    assert!(
        second.is_disconnected(),
        "with the last producer gone the inbox is empty and closed"
    );
    assert!(second.is_empty(), "and it holds nothing");

    // A third drain still reports it, so a polling loop sees a stable answer.
    let third = cx.update(bridge::drain).expect("the state is installed");
    assert!(third.is_disconnected() && third.is_empty());
}

/// A delivery to a closed inbox is refused as dropped, with the event back.
///
/// **Distinct from a full inbox on purpose.** A full inbox means "try later"; a
/// closed one means the application is gone, and a producer that retried
/// forever against it would spin. The refusal says which of the two it is.
#[gpui::test]
fn a_delivery_to_a_closed_inbox_is_refused_as_dropped(cx: &mut TestAppContext) {
    // A sender whose receiver is gone: install, take the sender, then remove the
    // global. `App::remove_global` is public in the pinned rev.
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");
    cx.update(|cx| {
        let _removed: AppStateGlobal = cx.remove_global();
    });

    let event = DomainEvent::MessageReceived(stored(4_242, "orphaned", 0));
    assert_eq!(
        sender.deliver(event.clone()),
        Delivery::Refused(DeliveryRefusal::BridgeDropped {
            event: Box::new(event)
        }),
        "a sender whose bridge is gone must be told so, with the event returned"
    );
    assert_eq!(
        sender
            .deliver(DomainEvent::ConnectionStateChanged(
                ConnectionState::Connected
            ))
            .to_string(),
        "refused: the application state is no longer reachable"
    );
}

/// The report's arithmetic holds across a burst that mixes outcomes.
///
/// **`delivered == applied + refused` is the invariant a caller reads the report
/// for, and an `ApplyOutcome` is a three-way split rather than a boolean.** The
/// burst below produces applied *and* refused outcomes on purpose: an arrival
/// that lands, a reaction for a message this client does not hold, and the same
/// arrival replayed. A report that lost one of them would still print plausible
/// numbers.
#[gpui::test]
fn the_drain_report_accounts_for_every_event_it_took(cx: &mut TestAppContext) {
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");

    let arrival = DomainEvent::MessageReceived(stored(1, "once", 10));
    for event in [
        arrival.clone(),
        // A reaction naming a message this client does not hold: refused, with
        // its reason, by `actions.rs`.
        DomainEvent::ReactionUpdated {
            message_id: "m_absent".to_owned(),
            emoji: "\u{1F44D}".to_owned(),
            user_id: THEM.to_owned(),
        },
        arrival,
    ] {
        assert_eq!(sender.deliver(event), Delivery::Queued);
    }

    let report = cx.update(bridge::drain).expect("the state is installed");
    assert_eq!(report.delivered(), 3, "every event taken must be counted");
    assert_eq!(
        report.applied(),
        1,
        "only the first arrival changed the state"
    );
    assert_eq!(
        report.refused(),
        2,
        "the reaction and the replay are refusals"
    );
    assert_eq!(
        report.delivered(),
        report.applied() + report.refused(),
        "the report's own invariant: nothing is unaccounted for"
    );
    assert!(!report.is_empty());
}

/// A quiet drain is immediate, and empty, and not an error.
///
/// `try_recv` does not block, so a drain of an empty inbox is `Some` with
/// zeroes rather than `None` and rather than a wait. **A drain that blocked
/// would put a socket read on the frame thread**, which is the specific mistake
/// `AGENTS.md` §2.3 and §7.3 both name.
#[gpui::test]
fn a_quiet_drain_is_empty_and_returns_immediately(cx: &mut TestAppContext) {
    // The sender is held for the whole test, so the inbox is open and this is a
    // quiet drain rather than a disconnect -- the two are different answers.
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");
    assert_eq!(sender.capacity(), MAX_PENDING_EVENTS);

    let report = cx.update(bridge::drain).expect("the state is installed");
    assert!(report.is_empty());
    assert_eq!(report.delivered(), 0);
    assert_eq!(report.applied(), 0);
    assert_eq!(report.refused(), 0);
    assert!(!report.is_disconnected(), "the sender is still alive");
    assert_eq!(report, DrainReport::default());
}

/// Every `DomainEvent` variant reaches the state through the one door.
///
/// **`actions::apply_event` is exhaustive over `DomainEvent` by construction**
/// (`actions.rs` module docs: adding a variant is a compile error there until it
/// is handled on purpose). This test's job is the half a compiler cannot: that
/// the bridge's door does not drop or rewrite any of them, and that the caller
/// sees the outcome for each — including the refusals.
#[gpui::test]
fn every_domain_event_variant_reaches_the_state_through_the_bridge(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _sender = bridge::install(cx, ME).expect("install must succeed");
    });

    let cases: Vec<(DomainEvent, ApplyOutcome)> = vec![
        (
            DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
            ApplyOutcome::Applied,
        ),
        (
            DomainEvent::MessageReceived(stored(1, "hello", 10)),
            ApplyOutcome::Applied,
        ),
        (
            DomainEvent::TypingUpdated {
                channel_id: CHANNEL.to_owned(),
                user_id: THEM.to_owned(),
                active: true,
            },
            ApplyOutcome::Applied,
        ),
        (
            DomainEvent::PresenceUpdated {
                user_id: THEM.to_owned(),
                status: UserStatus::Online,
            },
            ApplyOutcome::Applied,
        ),
        (
            // `after` is *after* the arrival above, so this is the newest cursor
            // the state has seen. `actions.rs` keeps the later of the two, and
            // an earlier request would be a no-op rather than a test.
            DomainEvent::ResyncRequested {
                channel_id: CHANNEL.to_owned(),
                after: at(20),
            },
            ApplyOutcome::Applied,
        ),
    ];

    for (event, expected) in cases {
        let outcome = cx.update(|cx| bridge::try_apply_event(cx, event));
        assert_eq!(
            outcome,
            Some(expected),
            "every variant must travel the door with its outcome intact"
        );
    }

    cx.read(|app| {
        let read = bridge::try_read(app, |state| {
            (
                state.message_count(CHANNEL),
                state.typing(CHANNEL).to_vec(),
                state.presence(THEM),
                state.resync_cursor(CHANNEL),
            )
        });
        let (messages, typing, presence, cursor) = read.expect("the state is installed");
        assert_eq!(messages, 1, "the arrival is held");
        assert_eq!(typing, vec![THEM.to_owned()], "the typing set is held");
        assert_eq!(presence, Some(UserStatus::Online), "presence is held");
        assert_eq!(cursor, Some(at(20)), "the resync cursor is held");
    });

    // The one that legitimately refuses, so the door is shown refusing too.
    let refused = cx.update(|cx| {
        bridge::try_apply_event(
            cx,
            DomainEvent::TypingUpdated {
                channel_id: "c_quiet".to_owned(),
                user_id: THEM.to_owned(),
                active: false,
            },
        )
    });
    assert_eq!(
        refused,
        Some(ApplyOutcome::Ignored(IgnoreReason::NobodyTyping {
            channel_id: "c_quiet".to_owned()
        })),
        "a stop for a quiet channel is reported rather than invented"
    );
}

/// A read is a borrow of the live state and not a snapshot.
///
/// `bridge::try_read` hands out a **closure scope**, not an escaping reference,
/// and this is what that buys: the value read inside the closure is a borrow of
/// the live state, so a mutation afterwards is visible to a later read, and
/// there is no way to hold a `&AppState` across one.
#[gpui::test]
fn a_read_is_a_borrow_of_the_live_state_and_not_a_snapshot(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _sender = bridge::install(cx, ME).expect("install must succeed");
    });

    let before = cx.read(|app| {
        bridge::try_read(app, |state| state.message_count(CHANNEL)).expect("installed")
    });
    assert_eq!(before, 0);

    cx.update(|cx| {
        let _ = bridge::try_apply_event(cx, DomainEvent::MessageReceived(stored(1, "x", 0)));
    });

    let after = cx.read(|app| {
        bridge::try_read(app, |state| state.message_count(CHANNEL)).expect("installed")
    });
    assert_eq!(
        after, 1,
        "the read observes the mutation, so it is a live borrow"
    );
}

// ---------------------------------------------------------------------------
// 3. The two user gestures
// ---------------------------------------------------------------------------

/// A send through the bridge carries the caller's timestamp, not a fresh one.
///
/// **This is the assertion behind the module docs' "the clock is a
/// parameter".** If the bridge read a clock, the optimistic row would carry a
/// time nobody chose, and `state/app_state.rs` module docs §4 is explicit that a
/// client-local estimate is an estimate the ACK replaces — an invented timestamp
/// would be a second, invisible ordering. The test pins the supplied value.
#[gpui::test]
fn a_send_through_the_bridge_uses_the_caller_s_timestamp(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _sender = bridge::install(cx, ME).expect("install must succeed");
    });

    let chosen = at(4_242);
    let outcome = cx.update(|cx| bridge::try_begin_send(cx, CHANNEL, "hello team", cid(7), chosen));
    let outcome = outcome.expect("the state is installed");

    assert!(
        outcome.is_pending(),
        "the row is on screen immediately, per the Optimistic Send Flow"
    );
    assert_eq!(
        outcome.to_string(),
        "queued offline as 00000000-0000-0000-0000-000000000007",
        "with the connection down the outbox owns it, and the row says so"
    );

    cx.read(|app| {
        let installed = bridge::try_read(app, |state| {
            state
                .message(CHANNEL, &cid(7))
                .map(|row| (row.timestamp, row.id.clone(), row.user_id.clone()))
        })
        .expect("the state is installed");
        let Some((timestamp, server_id, author)) = installed else {
            panic!("the optimistic row is on screen before the server has seen it");
        };
        assert_eq!(
            timestamp, chosen,
            "the optimistic row must carry the caller's timestamp verbatim"
        );
        assert_eq!(server_id, "", "and no server id, because none exists yet");
        assert_eq!(author, ME, "the author is the signed-in user");
    });
}

/// A refusal through the bridge keeps its reason, and the reason names no
/// content.
///
/// Two things are asserted at once. **The refusal is a value the caller can
/// see** — a channel that is not loaded is reported, not swallowed, which is
/// `actions.rs` module docs §1. And **the reason is a sentence about ids**, so
/// a `tracing` line carrying it cannot carry the user's text (`AGENTS.md` §7.5).
#[gpui::test]
fn a_refusal_through_the_bridge_keeps_its_reason_and_no_content(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _sender = bridge::install(cx, ME).expect("install must succeed");
    });

    let outcome = cx.update(|cx| bridge::try_select_channel(cx, "c_not_loaded"));
    assert_eq!(
        outcome,
        Some(ApplyOutcome::Ignored(IgnoreReason::UnknownChannel {
            channel_id: "c_not_loaded".to_owned()
        })),
        "selecting a channel the client has not loaded is refused, not accepted"
    );
    assert_eq!(
        outcome.map(|outcome| outcome.to_string()),
        Some("ignored: the channel c_not_loaded is not loaded".to_owned()),
        "the rendered line names the id and nothing else"
    );
}

/// A send the preconditions reject creates nothing, and says which rule refused.
///
/// The refusals of [`sh_nexus::state::actions::begin_send`] all travel through
/// one door, so this checks the bridge adds no refusal of its own and loses
/// none of theirs.
#[gpui::test]
fn a_send_the_bridge_refuses_creates_nothing(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _sender = bridge::install(cx, ME).expect("install must succeed");
    });

    for (channel_id, body, expected) in [
        ("  ", "body", "not sent: the channel id is blank"),
        (CHANNEL, "   ", "not sent: the message body is empty"),
    ] {
        let outcome = cx.update(|cx| bridge::try_begin_send(cx, channel_id, body, cid(1), at(0)));
        let outcome = outcome.expect("the state is installed");
        assert!(
            !outcome.is_pending(),
            "a refused send creates nothing: {expected}"
        );
        assert_eq!(outcome.to_string(), expected);
    }

    cx.read(|app| {
        assert_eq!(
            bridge::try_read(app, |state| state.pending_sends().len()),
            Some(0),
            "and the state holds no send at all"
        );
    });
}

// ---------------------------------------------------------------------------
// 4. The rendered-segment cache, and the probe this file will not add
// ---------------------------------------------------------------------------

/// `try_rendered` promotes on a hit and does nothing else on a miss.
///
/// **`core/cache.rs` §6 is the contract, and this is the assertion measured on
/// the counters the cache already reports.** A hit increments `hits` and returns
/// the document; a miss increments `misses` and touches nothing else. **A `&App`
/// variant of this door would have to be a membership test plus a second
/// lookup**, and the two would disagree about what counts as a hit — which is
/// why the assertions below are on the counters and not only on the return
/// value.
#[gpui::test]
fn a_rendered_probe_promotes_on_a_hit_and_is_inert_on_a_miss(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _sender = bridge::install(cx, ME).expect("install must succeed");
    });

    // Nothing cached yet: a miss, and the miss counter moved.
    let miss = cx.update(|cx| bridge::try_rendered(cx, &cid(1)));
    assert_eq!(miss, Rendered::Miss, "an unparsed message is a miss");
    assert!(!miss.is_hit());
    assert_eq!(miss.document(), None, "and a miss yields no document");
    assert_eq!(miss.to_string(), "miss", "one word, and no content");

    // Cache one document through the bridge's writing door.
    assert_eq!(
        cx.update(|cx| bridge::try_render_and_cache(cx, cid(2), "**bold** and `code`", 64)),
        Some(ApplyOutcome::Applied)
    );

    let before =
        cx.read(|app| bridge::try_read(app, AppState::segment_cache_stats).expect("installed"));
    assert_eq!(before.0, 1, "one document is resident");
    assert_eq!(before.3, 1, "the earlier miss is still counted");

    let hit = cx.update(|cx| bridge::try_rendered(cx, &cid(2)));
    assert!(hit.is_hit(), "the cached message must hit");
    assert!(hit.document().is_some(), "a hit yields the document");
    assert_eq!(
        hit.to_string(),
        "hit",
        "the rendered line is one word, and carries no document content"
    );
    let after_hit =
        cx.read(|app| bridge::try_read(app, AppState::segment_cache_stats).expect("installed"));
    assert_eq!(
        after_hit,
        (before.0, before.1, before.2 + 1, before.3, before.4),
        "a hit increments `hits` and changes nothing else"
    );

    // A miss changes nothing but the miss counter.
    let again = cx.update(|cx| bridge::try_rendered(cx, &cid(404)));
    assert_eq!(again, Rendered::Miss);
    let after_miss =
        cx.read(|app| bridge::try_read(app, AppState::segment_cache_stats).expect("installed"));
    assert_eq!(after_miss.0, after_hit.0, "a miss evicts nothing");
    assert_eq!(after_miss.4, after_hit.4, "and evicts nothing");
    assert_eq!(after_miss.2, after_hit.2, "and is not a hit");
    assert_eq!(after_miss.3, after_hit.3 + 1, "and counts exactly one miss");
}

/// `Rendered::NotInstalled` is distinct from a miss, and `document()` says so.
///
/// Three named cases rather than an `Option<Option<_>>`, and the third exists so
/// "the bridge is not installed" cannot be confused with "this message has never
/// been parsed" — a distinction a UI would otherwise have to invent.
#[test]
fn a_not_installed_probe_is_not_a_miss() {
    assert_ne!(Rendered::NotInstalled, Rendered::Miss);
    assert_eq!(Rendered::NotInstalled.document(), None);
    assert!(!Rendered::NotInstalled.is_hit());
    assert_eq!(Rendered::NotInstalled.to_string(), "not installed");
}

// ---------------------------------------------------------------------------
// 5. A view that reads the state, so "observable from a real context" is literal
// ---------------------------------------------------------------------------

/// A root view whose rendered frame reports the number of messages in `c_1`.
///
/// **Deliberately minimal, and it exists to make one claim literal:** that a
/// frame can reach the application state through this seam. Its `render` calls
/// [`bridge::try_read`] with the `Context<Self>` it is handed, which is the exact
/// call a real chat view would make, and it records what the frame saw so the
/// test can read the frame's output.
struct StateView {
    focus_handle: FocusHandle,
    seen: usize,
}

impl StateView {
    fn new(cx: &mut gpui::Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            seen: 0,
        }
    }

    fn seen(&self) -> usize {
        self.seen
    }
}

impl Focusable for StateView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StateView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // The seam's read door, called with the context a frame is handed.
        // `Context<StateView>` derefs to `App`, which is what `try_read` takes.
        self.seen = bridge::try_read(cx, |state| state.message_count(CHANNEL)).unwrap_or(0);

        div()
            .id("root")
            .key_context("StateView")
            .size_full()
            .text_color(gpui::rgb(0xcdd6f4))
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .id("count")
                    .debug_selector(|| "count".to_owned())
                    .text_color(gpui::rgb(0xcdd6f4))
                    .child(format!("messages: {}", self.seen)),
            )
    }
}

/// The view is a real renderable entity, and the seam works with no window.
///
/// **Two halves, and the second is the one that would otherwise be discovered by
/// `main`.** `#[gpui::test]` with `add_window_view` gives a
/// `VisualTestContext`; a real app's `run()` gives a bare `&mut App` in its
/// launch callback. Both must work, because the first is how a view reads the
/// state and the second is how `crate::run` installs it.
#[gpui::test]
fn the_seam_works_with_and_without_a_window(cx: &mut TestAppContext) {
    // No window, no view: the doors a launch callback uses.
    let sender = cx
        .update(|cx| bridge::install(cx, ME))
        .expect("install must succeed");
    assert_eq!(
        sender.deliver(DomainEvent::MessageReceived(stored(1, "no window", 0))),
        Delivery::Queued
    );
    let report = cx.update(bridge::drain).expect("the state is installed");
    assert_eq!(report.applied(), 1);

    // A window, a view, and a frame that reads the state.
    let (view, cx) = cx.add_window_view(|_, cx| StateView::new(cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("count")
            .is_some_and(|bounds| bounds.size.width > gpui::px(0.0)),
        "the view's child is laid out, so text shaped with no font bundle"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.seen()),
        1,
        "the first frame of this window sees the message drained above"
    );
}

// ---------------------------------------------------------------------------
// 6. The compile-time half: the context cannot be sent
// ---------------------------------------------------------------------------

/// A zero-sized carrier, so `T` is nameable without being constructed.
struct SendProbeWrapper<T>(core::marker::PhantomData<T>);

/// Resolves to `false`, unless an inherent method with a `Send` bound shadows
/// it.
///
/// **A trait with a blanket impl plus an inherent method that requires the bound
/// it is asserting.** Rust prefers the inherent method when its `where` clause
/// holds and falls back to the trait's default when it does not, which makes a
/// *negative* `Send` assertion expressible with no dependency and no macro. The
/// positive control in the test that uses this is what stops it from passing for
/// the wrong reason — a probe that always answered `false` would satisfy every
/// negative assertion in this file.
///
/// `Sync` is probed the same way and for the same reason. `gpui::Global` is a
/// bare `'static` marker trait (`gpui/src/global.rs:22`), so nothing in GPUI
/// demands a global be `Sync` and the assertion has to be made here.
trait SendProbe {
    /// `false`, unless shadowed by the inherent method below.
    fn is_send() -> bool {
        false
    }
    /// `false`, unless shadowed by the inherent method below.
    fn is_sync() -> bool {
        false
    }
}

impl<T> SendProbe for SendProbeWrapper<T> {}

impl<T: Send> SendProbeWrapper<T> {
    /// Shadows [`SendProbe::is_send`] exactly when `T: Send`.
    fn is_send() -> bool {
        true
    }
}

impl<T: Sync> SendProbeWrapper<T> {
    /// Shadows [`SendProbe::is_sync`] exactly when `T: Sync`.
    fn is_sync() -> bool {
        true
    }
}

/// `App`, `Context<'_, T>` and `AsyncApp` are `!Send`, and `u32` is `Send`.
///
/// **This is the one property of the single-thread rule that is a compile error
/// rather than a convention, and the module docs make it the pillar of the whole
/// design.** Asserted in both directions:
///
/// - The **negatives** say the main-thread context cannot be moved to another
///   thread, so `AGENTS.md` §7.3's "no blocking `cx.update_global` from non-UI
///   threads" is enforced by the compiler on the GPUI path.
/// - The **positive control** says the probe is not simply answering `false` to
///   everything. Without it, a probe that failed open would make the three
///   negative assertions vacuous.
///
/// The reason is structural and is stated in the module docs: `App` holds
/// `Weak<AppCell>` (`gpui/src/app.rs:748`), `Rc<dyn Platform>`
/// (`gpui/src/app.rs:749`) and `Rc<ActionRegistry>` (`gpui/src/app.rs:752`),
/// and `Context<'a, T>` holds `&'a mut App` (`gpui/src/app/context.rs:22`).
///
/// **And the mirror of it, which is a gotcha worth recording.** The
/// `compile_fail` doctest on `bridge::install` cannot use `App::update`, because
/// in the pinned rev `App::update` is `pub(crate)` (`gpui/src/app.rs:1161`) and
/// the doctest is an external crate — so the first version of that doctest
/// failed with *"method `update` is private"* and would have passed as a
/// `compile_fail` for entirely the wrong reason. It calls a public bridge door
/// instead, and `rustc` then names the three `Rc`s above. **A `compile_fail`
/// doctest is a test that passes for any compile error**, which is why the
/// matching `no_run` control exists and why this test carries its own positive
/// control.
#[test]
fn a_main_thread_context_cannot_be_sent_to_another_thread() {
    // The positive control. If this ever fails, every negative assertion below
    // is meaningless.
    assert!(
        SendProbeWrapper::<u32>::is_send(),
        "the probe must answer `true` for a type that is `Send`"
    );
    assert!(
        SendProbeWrapper::<DomainEvent>::is_send(),
        "an event crosses the thread boundary, and that is the point"
    );

    // The three negatives.
    assert!(
        !SendProbeWrapper::<App>::is_send(),
        "`App` holds `Rc<dyn Platform>` and `Rc<ActionRegistry>`, so it is !Send"
    );
    assert!(
        !SendProbeWrapper::<gpui::Context<'static, StateView>>::is_send(),
        "`Context` holds `&mut App`, so it is !Send too"
    );
    assert!(
        !SendProbeWrapper::<gpui::AsyncApp>::is_send(),
        "`AsyncApp` holds a `Weak<AppCell>`, so it cannot be moved to a worker either"
    );

    // And the state itself remains movable, which is the hazard 1E-1 documented
    // and this module does not claim to have removed.
    fn assert_send<T: Send>() {}
    assert_send::<AppState>();
}

/// The installed global is `Send` but **not** `Sync`, and the `!Sync` half is the
/// mechanical form of the module docs' §3 argument.
///
/// **The confinement here is ownership, and this is the assertion that says so.**
/// `AppStateGlobal` owns a `Receiver<DomainEvent>`, and `Receiver` is `!Sync`, so
/// there is no `&AppStateGlobal` any thread other than the owner's could ever
/// hold. A `Mutex` would have bought the opposite property — a shared handle
/// several threads may enter — which is precisely what the module docs §3 argue
/// against.
///
/// **The positive `Send` half is stated rather than assumed, because the two
/// together are the whole claim.** `Send` says the value may be *moved* (a thread
/// may take it away outright — which is what `remove_global` makes possible, and
/// `a_delivery_to_a_closed_inbox_is_refused_as_dropped` exercises); `!Sync` says
/// it may never be *shared*. A container that was `!Send` too would make this
/// trivially true and the seam pointless, since `EventSender` is what crosses.
#[test]
fn the_installed_global_can_be_moved_but_never_shared() {
    // Positive controls for the `Sync` probe, which is what makes the negative
    // assertion below worth anything.
    assert!(
        SendProbeWrapper::<u32>::is_sync(),
        "the Sync probe must answer `true` for a type that is `Sync`"
    );
    assert!(
        SendProbeWrapper::<Arc<u8>>::is_sync(),
        "and for one that is `Sync` through a field"
    );

    fn assert_send<T: Send>() {}
    assert_send::<AppStateGlobal>();

    assert!(
        !SendProbeWrapper::<AppStateGlobal>::is_sync(),
        "`AppStateGlobal` holds a `Receiver<DomainEvent>`, which is !Sync. That is \
         what makes the confinement an ownership fact rather than a convention: no \
         thread but the owner's can hold a reference to the state, so there is \
         nothing to lock and nothing a second thread could be handed."
    );
}

// ---------------------------------------------------------------------------
// 7. The scanners
// ---------------------------------------------------------------------------

/// The client's `src/` directory.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `directory`, recursively.
fn rust_files_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files_under(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// A file's source with every comment removed.
///
/// **Stripping first is load-bearing, not a nicety.** Every file in this crate
/// *discusses* this boundary at length — `state/mod.rs` names
/// `cx.update_global`, `network/mod.rs` names it, and `AppStateGlobal`'s own
/// documentation names the guards — so a scan that counted prose would fail on
/// the project's documentation of the rule.
fn without_comments(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    let mut in_block_comment = false;

    while let Some(character) = characters.next() {
        if in_block_comment {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                in_block_comment = false;
            }
            continue;
        }
        if character == '/' {
            match characters.peek() {
                Some('/') => {
                    for next in characters.by_ref() {
                        if next == '\n' {
                            stripped.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    characters.next();
                    in_block_comment = true;
                }
                _ => stripped.push(character),
            }
            continue;
        }
        stripped.push(character);
    }
    stripped
}

/// The context-mutating calls that exactly one file in this crate may make.
///
/// **A deny-list of calls, deliberately, and not an allow-list of crates.** The
/// question is not which crates `bridge.rs` may import — that is a different and
/// weaker question — it is *which file may mutate a GPUI context*. A crate
/// allow-list would be satisfied by a `cx.update_global` in `ui/`, which is
/// exactly the mistake this catches.
///
/// `cx.global_mut` is on the list because it hands out `&mut G` and is the
/// mutating read. `cx.global` is deliberately **not**: reading a global is what
/// a view does, and `state/bridge.rs` exists to be read from. The list is
/// prefixed with `cx.` because a fully qualified call (`App::set_global(..)`)
/// would be invisible to it, and the two remaining crates that name GPUI
/// contexts — this one and the file below — are checked by name elsewhere in
/// this suite.
const CONTEXT_MUTATING_TOKENS: [&str; 3] = ["cx.update_global", "cx.set_global", "cx.global_mut"];

/// `state/bridge.rs` is the only file in the crate that mutates a GPUI context.
///
/// **This is the guard that makes `PLAN.md` §4's "the only module permitted to
/// call `cx.update_global`" a build failure rather than a review opinion.** It
/// covers the whole `src/` tree, so it catches a second seam appearing in
/// `ui/`, `db/`, `platform/` or `network/` — the four places one would appear.
///
/// **No file is exempt, and that includes the crate root.** `PLAN.md` §4 names
/// one owner and does not carve out `lib.rs`, so an exemption here would be the
/// guard being wider than the rule it enforces — and a boundary that is relaxed
/// is a boundary nobody reads. The crate root calls [`bridge::install`], which is
/// an ordinary public function and names no context-mutating token, so the
/// entry point needs no pass: the assertion at the end of this test is what keeps
/// it that way. `state/mod.rs` and the two pure state files are checked like
/// everything else — 1E-1's guards name them precisely so this file could import
/// `gpui` legitimately, and this is the guard that takes over the other half of
/// that promise.
#[test]
fn bridge_is_the_only_file_that_calls_the_context_mutating_api() {
    let bridge = src_dir().join("state").join("bridge.rs");
    assert!(
        bridge.is_file(),
        "{} should exist: PLAN.md section 4 makes src/state/bridge.rs the single seam",
        bridge.display()
    );

    let mut owners: Vec<PathBuf> = Vec::new();

    for file in rust_files_under(&src_dir()) {
        let stripped = without_comments(
            &fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display())),
        );

        for token in CONTEXT_MUTATING_TOKENS {
            if !stripped.contains(token) {
                continue;
            }
            assert!(
                file == bridge,
                "{} calls `{token}`. PLAN.md section 4 names state/bridge.rs as the \
                 single owner of cx.update_global, and AGENTS.md 3.2 says every event \
                 the network delivers reaches the state through that one file. A second \
                 caller is a second seam, and a second seam is how two threads end up \
                 touching the state. There is no exempt file: an entry point that needs \
                 to install state calls bridge::install, which is why src/lib.rs is \
                 scanned like every other.",
                file.display()
            );
            if !owners.contains(&file) {
                owners.push(file.clone());
            }
        }
    }

    assert_eq!(
        owners,
        vec![bridge],
        "the seam must be owned by exactly one file, and it must be state/bridge.rs"
    );
}

/// No module outside the bridge builds a second application state.
///
/// **This is the residual gap of the single-thread invariant, made detectable,
/// and it is the one property the compiler will never give us.** `AppState` is
/// `Send + Sync` (1E-1's `the_state_is_send_and_sync_and_that_is_a_hazard_rather_`
/// `than_a_guarantee` documents why that is a hazard), and its constructor is
/// `pub`, so a
/// `network/`, `db/` or `platform/` module that built its own state and applied
/// events to it off the main thread would compile, run, and be invisible to every
/// other guard in this suite. **This scanner is the only thing standing between
/// that mistake and a silent second copy of the state** — not a type, not a
/// review, not `AGENTS.md` §7.3.
///
/// `state/app_state.rs` is excluded because it defines the type: the scan is for
/// *callers*, and `pub struct AppState {` would otherwise match a structural
/// token for a definition rather than a construction.
///
/// **What it deliberately does not claim:** this does not make the state
/// unreachable — a module that *received* one by value would still compile, and
/// nothing here would see it. What it removes is the cheap mistake, which is the
/// mistake that actually happens: a new module that decides it needs "its own"
/// state and constructs one.
#[test]
fn only_the_bridge_constructs_an_application_state() {
    const CONSTRUCTORS: [&str; 2] = ["AppState::new(", "AppState::with_segment_cache_bounds("];

    let bridge = src_dir().join("state").join("bridge.rs");
    let definition = src_dir().join("state").join("app_state.rs");
    let mut constructors: Vec<(PathBuf, &str)> = Vec::new();

    for file in rust_files_under(&src_dir()) {
        if file == definition {
            continue;
        }
        let stripped = without_comments(
            &fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display())),
        );
        for token in CONSTRUCTORS {
            if stripped.contains(token) {
                constructors.push((file.clone(), token));
            }
        }
    }

    assert_eq!(
        constructors,
        vec![(bridge, "AppState::new(")],
        "only state/bridge.rs may construct an AppState. A second one is a second copy \
         of the application state, on a thread this project cannot see: AGENTS.md 7.3 \
         forbids reaching the context off the main thread, and the single-thread \
         invariant in state/app_state.rs module docs section 3 assumes there is one \
         state to be on it. Route the event through bridge::EventSender instead."
    );
}

/// `network/` cannot reach the main-thread context, by name.
///
/// **A second, narrower guard than the one above, and it is not redundant.** The
/// first says "only one file mutates a context anywhere in `src/`"; this one says
/// "`network/` names no GPUI context *type* at all", which is the rule in
/// `AGENTS.md` §3.2 and `PLAN.md` §4 stated from the other side.
///
/// The difference matters because `network/` is the one layer a Phase 4 author
/// will be editing when a socket task starts running, and a *read* of a context
/// there — a `cx.spawn` closure, a `Context<T>` parameter — would pass the first
/// guard while breaking this one.
#[test]
fn network_cannot_reach_the_main_thread_context() {
    let network_dir = src_dir().join("network");
    let files = rust_files_under(&network_dir);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        network_dir.display()
    );

    for file in &files {
        let stripped = without_comments(
            &fs::read_to_string(file)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display())),
        );
        for token in [
            "gpui",
            "cx.update_global",
            "cx.update",
            "cx.set_global",
            "cx.global",
            "Context<",
            "App",
            "Window",
            "Entity",
        ] {
            assert!(
                !stripped.contains(token),
                "{} mentions `{token}`. AGENTS.md 3.2: network/ never touches GPUI \
                 state directly. PLAN.md section 4 names state/bridge.rs as the only \
                 module allowed to reach the main-thread context, and this test is that \
                 rule stated from network/'s side: it is about the context TYPE, not \
                 only about the mutating methods, because a Phase 4 socket task that \
                 takes a `Context<T>` has already crossed the line even if it never \
                 calls update_global.",
                file.display()
            );
        }
    }
}

/// The bridge never names a mutable borrow of the state.
///
/// **The mechanical half of the module docs' §5 argument, and the guard that
/// keeps the audit trail short.** 1E-1 made `AppState`'s mutators `pub(crate)`,
/// so a module could not call them without a `&mut AppState` in hand — and this
/// file is the only thing in the crate that can produce one. `&mut AppState`
/// appearing even once, in any signature, would hand the whole mutator surface
/// to every module in the crate and undo `AGENTS.md` §3.2's "all mutations go
/// through `actions.rs` so they are auditable and testable".
#[test]
fn the_bridge_never_names_a_mutable_borrow_of_the_state() {
    let bridge = src_dir().join("state").join("bridge.rs");
    let stripped = without_comments(
        &fs::read_to_string(&bridge)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", bridge.display())),
    );

    assert!(
        !stripped.contains("&mut AppState"),
        "state/bridge.rs names `&mut AppState`. There is deliberately no \
         `state_mut` and no `with_state_mut`: AppState's mutators are pub(crate), so \
         a public &mut AppState would hand the whole mutator surface back to every \
         module in the crate and undo AGENTS.md 3.2. The state is read through \
         `try_read` and written through the named doors, which is what makes the \
         list of doors the audit trail."
    );
    assert!(
        stripped.contains("try_read"),
        "the read door must exist, or the assertion above has nothing to contrast with"
    );
}

/// `state/bridge.rs` holds no interior mutability.
///
/// **The bridge is the one file in `state/` that legitimately needs a
/// cross-thread channel, so the question "does this layer have a lock" arrives
/// here rather than in 1E-1's two-file guard — and the answer has to be a
/// checked one.** The channel this file uses is `std::sync::mpsc`, whose
/// `SyncSender` and `Receiver` are *not* spelled with any of these tokens; what
/// is forbidden is a lock, a cell, or an `UnsafeCell` written by hand.
///
/// **Why it matters here specifically:** a `Mutex` in this file would be a
/// `Mutex` *around the state*, and a lock around the state makes a second thread
/// possible — which is the direction in which 1E-1's invariant becomes
/// unenforceable. [`a_main_thread_context_cannot_be_sent_to_another_thread`] is
/// the positive half; this is the negative one.
#[test]
fn bridge_holds_no_interior_mutability() {
    let bridge = src_dir().join("state").join("bridge.rs");
    let stripped = without_comments(
        &fs::read_to_string(&bridge)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", bridge.display())),
    );

    for token in [
        "Mutex",
        "RwLock",
        "RefCell",
        "Cell<",
        "UnsafeCell",
        "OnceCell",
        "LazyLock",
        "thread_local!",
    ] {
        assert!(
            !stripped.contains(token),
            "state/bridge.rs contains `{token}`. The bridge exists to carry owned \
             values across a thread boundary while the state stays on the main \
             thread; a lock or a cell here would put shared mutable state back on \
             the producing side, which is precisely the hazard state/app_state.rs \
             module docs section 3 documents. The bounded std::sync::mpsc channel is \
             the sanctioned mechanism and it names none of these tokens."
        );
    }
}

/// The bridge's own imports are the ones its documentation claims.
///
/// A cheap structural check with a real purpose: **the bridge is the layer
/// between the network and the main thread, so a new dependency here is a
/// decision about that boundary rather than a convenience.** An unlisted crate
/// is one nobody made on purpose.
#[test]
fn the_bridge_imports_only_what_it_declares() {
    const ALLOWED: [&str; 7] = [
        "std",
        "core",
        "crate",
        "chrono",
        "gpui",
        "thiserror",
        "uuid",
    ];
    let bridge = src_dir().join("state").join("bridge.rs");
    let stripped = without_comments(
        &fs::read_to_string(&bridge)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", bridge.display())),
    );

    for line in stripped.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed
            .strip_prefix("pub use ")
            .or_else(|| trimmed.strip_prefix("use "))
        else {
            continue;
        };
        let segment = rest
            .split("::")
            .next()
            .unwrap_or_default()
            .trim_start_matches("::")
            .split("::")
            .next()
            .unwrap_or_default();
        assert!(
            ALLOWED.contains(&segment),
            "{} imports `{segment}`, which is not on the bridge's allow-list: \
             {ALLOWED:?}",
            bridge.display()
        );
    }

    for forbidden in ["tokio", "reqwest", "rusqlite", "sh_nexus_wire"] {
        assert!(
            !stripped.contains(forbidden),
            "state/bridge.rs names `{forbidden}`. The bridge carries values between a \
             producer and the main thread; it does not speak a protocol, persist \
             anything, or open a socket. Those are network/ and db/, and a bridge that \
             also did them could not be exercised against a real headless app."
        );
    }
}

/// The bridge scanner sees code and not prose.
///
/// **A scanner that fails open is worse than no scanner, because every boundary
/// test then passes for the wrong reason** — the same class of bug
/// `layer_boundary.rs` records for its own comment stripper, and the reason that
/// file carries a test for the stripper. This asserts, on synthetic source, that
/// a violation sharing a line with a comment is still found, and that prose
/// *about* the boundary does not trip it.
#[test]
fn the_bridge_scanner_sees_code_and_not_prose() {
    let source = concat!(
        "//! A module doc naming cx.update_global in prose.\n",
        "/// An item doc naming cx.set_global in prose.\n",
        "fn f(cx: &mut App) { cx.update_global(|_, _| ()); }\n",
        "fn g(cx: &mut App) { cx.set_global(1); } // a trailing comment about cx.global_mut\n",
        "/* a block comment naming cx.global_mut */\n",
    );
    let stripped = without_comments(source);

    assert!(
        !stripped.contains("in prose"),
        "prose about the boundary must be stripped, or the crate's own documentation \
         of the rule would fail the rule"
    );
    for token in ["cx.update_global", "cx.set_global"] {
        assert!(
            stripped.contains(token),
            "`{token}` survives comment stripping: a trailing comment must not hide \
             the call it shares a line with, got {:?}",
            stripped.trim()
        );
    }
    assert!(
        !stripped.contains("cx.global_mut"),
        "a comment's contents go, whether trailing or in a block"
    );
}

// ---------------------------------------------------------------------------
// 8. The rendered lines, and the absence of message content
// ---------------------------------------------------------------------------

/// Every line this module renders names ids and reasons, and no message body.
///
/// **`AGENTS.md` §7.5 is "never log message content", and a refusal type that
/// carries a whole `DomainEvent` is exactly the kind of thing that leaks one
/// through a `Debug`.** The body below is chosen to be greppable, and the test
/// asserts it appears in none of the rendered lines.
///
/// The cost of the rule is stated rather than hidden: a developer debugging a
/// dropped message gets the id and the reason, not the text, and has to go to
/// the message row itself. That is the trade `AGENTS.md` §7.5 makes, and it is
/// the reason `DeliveryRefusal` keeps the event *out* of its `Display` even
/// though it holds one.
#[test]
fn a_refusal_renders_a_line_with_no_message_content() {
    const SECRET: &str = "the quick brown fox";

    assert_eq!(
        Delivery::Queued.to_string(),
        "queued for the main thread",
        "the success line is fixed and carries nothing at all"
    );

    for refusal in [
        DeliveryRefusal::InboxFull {
            capacity: 7,
            event: Box::new(DomainEvent::MessageReceived(stored(1, SECRET, 0))),
        },
        DeliveryRefusal::BridgeDropped {
            event: Box::new(DomainEvent::MessageReceived(stored(1, SECRET, 0))),
        },
    ] {
        let line = Delivery::Refused(refusal.clone()).to_string();
        assert!(
            line.starts_with("refused: "),
            "every refusal line names the reason, got {line}"
        );
        assert!(
            !line.contains(SECRET),
            "a refusal line must carry no message body, got {line}"
        );
        assert!(
            !line.contains(CHANNEL) && !line.contains("m_1"),
            "and no ids beyond the ones the reason is about, got {line}"
        );
    }

    // The two fixed strings, so a wording change is a test failure rather than a
    // diff nobody reads.
    assert_eq!(
        Delivery::Refused(DeliveryRefusal::InboxFull {
            capacity: 7,
            event: Box::new(DomainEvent::ConnectionStateChanged(
                ConnectionState::Connected
            )),
        })
        .to_string(),
        "refused: the event inbox is full at 7"
    );
    assert_eq!(
        Delivery::Refused(DeliveryRefusal::BridgeDropped {
            event: Box::new(DomainEvent::ConnectionStateChanged(
                ConnectionState::Connected
            )),
        })
        .to_string(),
        "refused: the application state is no longer reachable"
    );
    assert_eq!(
        InstallError::AlreadyInstalled.to_string(),
        "the application state is already installed; installing again would drop the \
         events already queued in the first inbox",
        "the install refusal names the consequence, not only the condition"
    );
}
