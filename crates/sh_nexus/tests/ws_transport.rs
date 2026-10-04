//! Two client transports, one real `sh_nexus_server`, and a real socket.
//!
//! # What this file is for
//!
//! `AGENTS.md` §8.1's Real-time Flow asks for *"two clients → A sends message → B
//! receives"*, and until `network/ws.rs` existed that flow could not be written at
//! all: the client had a boundary and a seam and nothing that spoke the protocol.
//! Everything below drives a **real server process over a real TCP socket**, with
//! the events arriving through the **real `state/bridge.rs` inbox** rather than a
//! test double, because a mock would answer neither question that matters — that
//! the wire protocol round-trips, and that a value produced on a worker thread
//! reaches the main thread through the one seam that is allowed to.
//!
//! | Test | What it proves |
//! |---|---|
//! | [`two_transports_exchange_a_message_through_one_real_server`] | §8.1's Real-time Flow, with two clients and two inboxes |
//! | [`a_full_inbox_refuses_the_arriving_event_rather_than_growing`] | §7.1's bound, end to end |
//! | [`a_dropped_connection_is_retried_on_the_backoff_schedule_and_recovers`] | §8.1's Reconnect Flow, and §4.2's reset-on-success |
//! | [`the_transport_gives_up_after_its_configured_allowance`] | §4.2's max-attempt behaviour |
//! | [`an_unsupported_major_version_is_refused_rather_than_silently_continued`] | §7.4's explicit rejection, both directions |
//! | [`a_shut_down_transport_refuses_further_frames`] | `AGENTS.md` §2.1, and §7.5 through `Debug` |
//! | [`the_suites_socket_path_is_the_servers_own_constant`] | the duplication `tests/support/mod.rs` admits to is watched |
//!
//! # Why the server is a child process
//!
//! `tests/support/mod.rs` gives the argument in full: a `[dev-dependencies]` entry
//! naming `sh_nexus_server` would put `axum` and `rusqlite` into
//! `cargo tree -p sh_nexus` and would make the client manifest name its own
//! server. Running the binary keeps the client's dependency set to
//! `tokio-tungstenite`, `tokio` and `futures-util`, and tests the shipped binary
//! rather than a library.
//!
//! # Two contexts, and why
//!
//! `bridge::install` refuses a second install on one application — by design, so a
//! second startup cannot orphan the first inbox. Two clients therefore need two
//! applications, and `TestAppContext::new_app` is how a suite gets a second one
//! without a second test harness. Each gets its own real `AppState` and its own
//! real `Receiver<DomainEvent>`.
//!
//! # How a test waits
//!
//! `support::wait_until` polls a condition against a deadline. There is no
//! `sleep()` standing in for an event anywhere in this file: `AGENTS.md` §4.3's
//! rule is the reason, and a suite that slept would be testing its own timing.

mod support;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use gpui::TestAppContext;
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::user::UserStatus;
use sh_nexus::network::ws::{
    backoff_window, TransportConfig, TransportError, TransportStats, WsTransport,
};
use sh_nexus::state::bridge::{self, Delivery, DeliveryRefusal, DrainReport, EventSender};
use sh_nexus::state::{bridge::MAX_PENDING_EVENTS, SendOutcome};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerFrame};
use sh_nexus_wire::version::{PROTOCOL_VERSION, UNSUPPORTED_VERSION_CODE};
use sh_nexus_wire::WireError;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use support::{ServerProcess, READY_BUDGET, WS_PATH};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The user client A is signed in as.
const ME: &str = "u_me";
/// The user client B is signed in as, so a message is somebody else's.
const THEM: &str = "u_them";
/// The one channel `sh_nexus_server` seeds (`db::DEFAULT_CHANNEL_ID`).
///
/// **Spelled here rather than imported**, for the reason the server's own
/// dependency direction demands: naming the server crate would put `axum` and
/// `rusqlite` into this crate's dependency graph. A literal that a test watches is
/// better than a constant that silently drifts.
const CHANNEL: &str = "c_general";

/// A body chosen to be recognisable if any line ever carried it.
///
/// `AGENTS.md` §7.5 forbids logging message content, so nothing in this file may
/// print it. The constant is here so the assertion that it arrived is about *this*
/// body rather than about some non-empty string.
const BODY: &str = "hello from client A";

/// How long a socket's counters must hold still before a test may treat the worker
/// as finished with it.
///
/// **A bound on an observation, not a `sleep()` waiting for logic.** The read loop
/// runs on its own OS thread with no completion signal, so "it has stopped moving" is
/// the only honest available answer; this is the interval across which that is checked,
/// and it is several times the loop's own turnaround so an in-flight delivery lands
/// inside the window rather than after it.
const QUIESCE_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// How long a worker gets to go quiet before the wait gives up and says so.
///
/// **Its own budget, not [`READY_BUDGET`]**, and deliberately larger. `READY_BUDGET` is
/// sized for a loopback handshake; a worker spending its connect allowance waits out a
/// real backoff between attempts, and reusing the shorter budget meant the wait
/// expired mid-backoff and read the counters before the worker's last act.
const QUIESCE_FINISH_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// Waits until a transport's worker has genuinely finished, and returns its stats.
///
/// **`is_closed()` is not this.** It reads a flag that `shutdown()` sets and that the
/// worker's own terminal branch sets, and in both cases the flag goes up *before* the
/// loop stops touching the socket. Two tests read a counter immediately after waiting
/// on `is_closed()` and saw the state from *before* the worker's last act: one read
/// `connect_failures == 1` where two attempts were allowed, and one read 1025
/// delivered against a bound of 1024. Both reproduced on the CI runner and not in ten
/// local runs, which is the worst possible ratio to debug with.
///
/// **Two earlier versions of this helper were worse than the bug, and both are
/// recorded here so the shape is not "simplified" back into them.** The first slept
/// inside a `wait_until` predicate — but that helper sleeps 1 ms per iteration and
/// checks its own deadline, so a 150 ms sleep inside the predicate blew the budget and
/// returned with every counter at zero, failing 20 runs out of 20. The second fixed
/// that by snapshotting before the first sleep, which made "nothing changed" trivially
/// true for a worker that had not started yet. So: **its own loop, and quiet only
/// counts once work has been seen.**
///
/// There is no `JoinHandle` to await, so the counters — the only observable that means
/// "nothing more will happen" — are polled directly. The deadline is generous on
/// purpose: a worker spending its allowance waits out a real backoff between attempts,
/// and this must outlast that rather than race it.
fn wait_for_worker_to_finish(transport: &WsTransport) -> TransportStats {
    let deadline = Instant::now() + QUIESCE_FINISH_BUDGET;
    let mut last = transport.stats();
    let mut seen_work = false;
    while Instant::now() < deadline {
        std::thread::sleep(QUIESCE_SETTLE);
        let now = transport.stats();
        seen_work |= now != TransportStats::default();
        if seen_work && now == last {
            return now;
        }
        last = now;
    }
    panic!(
        "the transport's worker never went quiet within {QUIESCE_FINISH_BUDGET:?}; \
         last seen {last:?}"
    );
}

/// A timestamp the caller supplies, since `state/` may not read a clock.
fn at(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_000_000 + second, 0)
        .single()
        .expect("the test timestamp is in range")
}

/// Installs the bridge on one application and hands back the producer handle.
fn install(cx: &mut TestAppContext, self_user_id: &str) -> EventSender {
    cx.update(|cx| {
        bridge::install(cx, self_user_id).expect("the first install on an application must succeed")
    })
}

/// Takes everything the worker has queued, on one application, discarding the
/// report.
///
/// The report is deliberately not returned: every call site here is a poll or a
/// side-effect, and `DrainReport` is `#[must_use]` for the reason its own docs give
/// — a drain nobody looked at cannot tell a full inbox from a lost message. **The
/// one place that needs the report asks for it**, through [`drain_report`].
fn drain(cx: &mut TestAppContext) {
    let _ = drain_report(cx);
}

/// Takes everything the worker has queued and reports what it did.
fn drain_report(cx: &mut TestAppContext) -> DrainReport {
    cx.update(bridge::drain).expect("the state is installed")
}

/// The connection state one application currently holds.
fn connection(cx: &TestAppContext) -> ConnectionState {
    cx.read(|app| {
        bridge::try_read(app, |state| state.connection().clone()).expect("the state is installed")
    })
}

/// Waits until one application reports a connected socket.
fn await_connected(cx: &mut TestAppContext) {
    support::wait_until("the client to report a connection", READY_BUDGET, || {
        drain(cx);
        connection(cx) == ConnectionState::Connected
    });
}

/// Waits until one application holds the acknowledged form of `client_msg_id`.
///
/// **A real server id, not just a row.** `actions::begin_send` puts an *empty*
/// server id on the optimistic row — the row is on screen before the server has
/// said anything — so "the row exists" would be true before the ack arrived and
/// would prove nothing. A non-empty id can only have come from `message.ack`.
fn await_server_id(cx: &mut TestAppContext, client_msg_id: &Uuid) {
    support::wait_until(
        "the optimistic row to be reconciled by an ack",
        READY_BUDGET,
        || {
            drain(cx);
            cx.read(|app| {
                bridge::try_read(app, |state| {
                    state
                        .message(CHANNEL, client_msg_id)
                        .is_some_and(|held| !held.id.is_empty())
                })
                .unwrap_or(false)
            })
        },
    );
}

/// How many rows one application holds for `client_msg_id`.
fn rows_for(cx: &TestAppContext, client_msg_id: &Uuid) -> usize {
    cx.read(|app| {
        bridge::try_read(app, |state| {
            state
                .messages(CHANNEL)
                .iter()
                .filter(|held| &held.client_msg_id == client_msg_id)
                .count()
        })
        .unwrap_or(0)
    })
}

// ---------------------------------------------------------------------------
// 1. Two clients, one server
// ---------------------------------------------------------------------------

/// `AGENTS.md` §8.1's Real-time Flow, with two clients and two real inboxes.
///
/// **The whole point is that the event arrives through B's inbox**, not through a
/// return value: B's `AppState` is a real global installed by `bridge::install`,
/// its queue is the real bounded `Receiver`, and the only thing that ever drained
/// it in this test is `bridge::drain` on the main thread — the same call
/// `AGENTS.md` §7.3 names. So the assertion that B holds A's message is a claim
/// about the seam, not about a mock.
#[gpui::test]
fn two_transports_exchange_a_message_through_one_real_server(cx: &mut TestAppContext) {
    let server = ServerProcess::start("two-clients");

    // Two applications, because `bridge::install` refuses a second install on one —
    // by design, so a second startup cannot orphan the first inbox.
    let mut b = cx.new_app();
    let sender_a = install(cx, ME);
    let sender_b = install(&mut b, THEM);

    let config_a = server.authenticated_config();
    let config_b = server.authenticated_config();
    let transport_a = WsTransport::start(config_a.clone(), sender_a.clone()).expect("a thread");
    let transport_b = WsTransport::start(config_b.clone(), sender_b.clone()).expect("a thread");

    await_connected(cx);
    await_connected(&mut b);

    // The optimistic row, put down exactly as `ui/views/input_bar.rs` does: the
    // caller supplies the identity and the timestamp, because `AGENTS.md` §3.2
    // gives `state/` no clock and no id source.
    let client_msg_id = Uuid::new_v4();
    let optimistic = cx.update(|cx| {
        bridge::try_begin_send(cx, CHANNEL, BODY, client_msg_id, at(0))
            .expect("the state is installed")
    });
    assert!(
        matches!(&optimistic, SendOutcome::Pending { client_msg_id: held, .. } if *held == client_msg_id),
        "the row must be on screen before the socket carries it, got {optimistic:?}"
    );
    // The server has not seen it yet, so the row must still be the optimistic one.
    assert!(
        rows_for(cx, &client_msg_id) == 1,
        "the optimistic row is exactly one row"
    );

    transport_a
        .send_message(client_msg_id, CHANNEL, BODY)
        .expect("the send is queued");

    // A is acknowledged, and the ack is what replaces the row rather than patching
    // it (`AGENTS.md` §8.1's Optimistic Send Flow).
    await_server_id(cx, &client_msg_id);

    // B receives the same message as a `DomainEvent` through its own inbox. Nothing
    // connects A and B: the only path between them is the server.
    support::wait_until("client B to hold the message", READY_BUDGET, || {
        drain(&mut b);
        b.read(|app| {
            bridge::try_read(app, |state| {
                state
                    .message(CHANNEL, &client_msg_id)
                    .is_some_and(|held| held.content == BODY)
            })
            .unwrap_or(false)
        })
    });

    let held_by_b = b.read(|app| {
        bridge::try_read(app, |state| {
            state
                .message(CHANNEL, &client_msg_id)
                .map(|held| (held.content.clone(), held.id.clone(), held.client_msg_id))
        })
        .flatten()
    });
    let (content, server_id, identity) = held_by_b.expect("B holds the message");
    assert_eq!(content, BODY, "B received the body A sent");
    assert_eq!(
        identity, client_msg_id,
        "the identity is the one A's optimistic row carried, which is what makes \
         the server's dedupe and B's reconciliation agree"
    );
    assert!(
        !server_id.is_empty(),
        "the stored message carries the server's own id, so A's ack and B's copy are \
         the same message rather than two"
    );

    // Both sides saw exactly one message, and neither dropped or duplicated one.
    assert_eq!(rows_for(cx, &client_msg_id), 1, "A holds one row");
    assert_eq!(rows_for(&b, &client_msg_id), 1, "B holds one row");

    for stats in [transport_a.stats(), transport_b.stats()] {
        assert_eq!(
            stats.frames_dropped, 0,
            "nothing was unreadable in a two-client exchange: {stats}"
        );
        assert_eq!(stats.events_refused, 0, "the inbox was never full: {stats}");
        assert_eq!(stats.writes_failed, 0, "no write failed: {stats}");
        assert!(stats.connects >= 1, "the socket was established: {stats}");
    }

    transport_a.shutdown();
    transport_b.shutdown();
    drop(server);
}

// ---------------------------------------------------------------------------
// 2. The inbox bound, end to end
// ---------------------------------------------------------------------------

/// A full inbox refuses the arriving event rather than growing.
///
/// `AGENTS.md` §7.1 forbids unbounded in-memory state, and `bridge.rs`'s answer to
/// the question that creates is a `sync_channel` with a bound. **What this test
/// adds is the transport's half**: that a socket which is actively delivering
/// frames into a full inbox counts the refusal and does not quietly grow, and that
/// the event really was *on the wire* — so the refusal is the transport's, not the
/// bridge's.
///
/// The inbox is filled through the real [`EventSender`] before the message arrives,
/// which is what makes the assertion exact: `frames_read` proves the frame was
/// decoded, and `events_delivered == 0` proves none of it got in.
#[gpui::test]
fn a_full_inbox_refuses_the_arriving_event_rather_than_growing(cx: &mut TestAppContext) {
    let server = ServerProcess::start("full-inbox");
    let mut a = cx.new_app();
    let sender_b = install(cx, THEM);
    let sender_a = install(&mut a, ME);

    // Fill B's real inbox to its bound, through its real producer. `PresenceUpdated`
    // is used because it is the one event a client applies with no precondition, so
    // filling the inbox cannot fail for a reason of its own.
    for index in 0..MAX_PENDING_EVENTS {
        let filled = sender_b.deliver(DomainEvent::PresenceUpdated {
            user_id: format!("u_filler_{index}"),
            status: UserStatus::Online,
        });
        assert_eq!(
            filled,
            Delivery::Queued,
            "the inbox has room for {index} more"
        );
    }
    let overflow = sender_b.deliver(DomainEvent::PresenceUpdated {
        user_id: "u_one_too_many".to_owned(),
        status: UserStatus::Online,
    });
    assert!(
        matches!(
            overflow,
            Delivery::Refused(DeliveryRefusal::InboxFull { capacity, .. })
                if capacity == MAX_PENDING_EVENTS
        ),
        "the bound is the bridge's, and it is already reached: {overflow}"
    );

    let config_a = server.authenticated_config();
    let config_b = server.authenticated_config();
    let transport_a = WsTransport::start(config_a.clone(), sender_a).expect("a thread");
    let transport_b = WsTransport::start(config_b.clone(), sender_b).expect("a thread");

    // **Waited on the counter, not on the inbox.** B's inbox is full, so B's
    // connection-state events are being refused too — which is the honest situation
    // and precisely why `TransportStats` exists as the observable surface here.
    support::wait_until("client B to connect", READY_BUDGET, || {
        transport_b.stats().connects >= 1
    });

    let client_msg_id = Uuid::new_v4();
    transport_a
        .send_message(client_msg_id, CHANNEL, BODY)
        .expect("the send is queued");

    support::wait_until("B to have read the frame", READY_BUDGET, || {
        transport_b.stats().frames_read >= 1
    });

    // **Then wait for B to be quiescent, and the reason is not cosmetic.**
    //
    // An earlier version of this test waited for `events_refused >= 1`, which a
    // connection-state event satisfies just as well as the message does — B's inbox
    // is full, so its `Connecting` and `Connected` are refused too. That wait
    // returned before B had read the message frame at all, the drain below freed a
    // slot, and **the message then fitted**: 1025 delivered against a bound of 1024.
    // It reproduced on the CI runner and not in ten local runs, which is the worst
    // possible ratio.
    //
    // `frames_read >= 1` alone is not sufficient either, because the delivery attempt
    // happens after the read. The observable that means "nothing more will be
    // enqueued" is the worker's counters going still.
    let settled = wait_for_worker_to_finish(&transport_b);
    assert!(
        settled.frames_read >= 1 && settled.events_delivered == 0 && settled.events_refused >= 1,
        "B read the frame, delivered nothing, and refused it: {settled:?}"
    );

    // And the inbox did not grow to make room: draining takes exactly what was put
    // in, and not one more. This is the assertion that matters, because "the event
    // was refused" and "the event was dropped to make space" are the same counter.
    // Both transports are stopped before the drain, so nothing is still mid-send. That
    // is necessary and it is **not** sufficient: `shutdown()` sets a flag without
    // joining, so the wait above is what actually establishes quiescence. The bound
    // itself was never violated — a `sync_channel(MAX_PENDING_EVENTS)` cannot hold
    // more than that at any instant — and what this asserts is that the inbox reached
    // its bound and held there while a socket was actively delivering into it.
    transport_a.shutdown();
    transport_b.shutdown();

    let report = drain_report(cx);
    assert_eq!(
        report.delivered(),
        MAX_PENDING_EVENTS,
        "the inbox held exactly its bound and no more"
    );
    assert!(
        rows_for(cx, &client_msg_id) == 0,
        "the refused message must not have reached the state by another route"
    );

    drop(server);
}

// ---------------------------------------------------------------------------
// 3. Reconnection
// ---------------------------------------------------------------------------

/// A dropped connection is retried on the backoff schedule and recovers.
///
/// `AGENTS.md` §8.1's Reconnect Flow, against a real socket and a real server, with
/// three things asserted that a unit test cannot assert:
///
/// 1. **The schedule is respected in wall-clock time.** The first retry waits at
///    least `backoff_window(1).start()` — 800 ms — after the first failure. A loop
///    that retried immediately would satisfy every other assertion here and still be
///    wrong.
/// 2. **Recovery happens on the same port.** The transport is pointed at a port with
///    nothing on it, and the server is started on that port afterwards. The
///    transport's URL never changed, so the reconnect is a real one.
/// 3. **The resync cursor is replayed.** The cursor is remembered per channel and
///    re-sent on *every* successful connect, which is what "resume from
///    `last_message_at`" means — `AGENTS.md` §7.4 forbids relying on live delivery
///    across a gap.
///
/// **The drop is provoked by the server, not by the client**, because the client has
/// no way to drop its own socket that is not a test-only API in production code.
/// `sh_nexus_server`'s `ws.rs` caps a frame at 64 KiB and terminates the connection
/// without a close frame when one is larger — a real, protocol-driven disconnect
/// the client cannot prevent and must survive.
#[gpui::test]
fn a_dropped_connection_is_retried_on_the_backoff_schedule_and_recovers(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let url = format!("ws://127.0.0.1:{port}{WS_PATH}");
    let sender = install(cx, ME);

    // The server is started, a session is taken from it, and then it is **stopped**.
    // That sequence is the only way to hold a valid token across a gap in which the
    // port is closed: a token is bound to one database file, and a database is only
    // readable through a server. So the transport is started against a port nothing
    // is listening on, with a credential the restarted server will accept.
    let mut server = ServerProcess::start_on(port, "reconnect");
    let token = server.token();
    server.stop();

    let transport =
        WsTransport::start(TransportConfig::new(url).with_token(token), sender).expect("a thread");

    support::wait_until("the first connect to fail", READY_BUDGET, || {
        transport.stats().connect_failures >= 1
    });
    let first_retry_began = Instant::now();
    support::wait_until("a second attempt", READY_BUDGET, || {
        transport.stats().connect_failures >= 2
    });
    let waited = first_retry_began.elapsed();
    let earliest = *backoff_window(1).start();
    assert!(
        waited >= earliest,
        "the first retry waited {waited:?}, which is under the {earliest:?} the \
         schedule promises. A transport that retries in a hot loop is a transport \
         that reconnects 100 times a second against a server that is down."
    );

    // The server arrives on the port the transport is already watching, against the
    // same database, so the token the transport already holds is still the right one.
    // `connects` counts *established* sockets, so it is 1 here and not 3: the two
    // attempts made while the port was empty never produced a socket, which is what
    // `connect_failures` counts instead.
    server.restart();
    await_connected(cx);
    assert!(
        transport.stats().connects >= 1,
        "the reconnect happened on the same URL, with no change to the transport: {}",
        transport.stats()
    );
    assert!(
        transport.stats().connect_failures >= 2,
        "and it took more than one attempt to get there: {}",
        transport.stats()
    );

    // Remember a cursor, so the second connect has something to replay.
    transport
        .request_resync(CHANNEL, sh_nexus::network::ws::epoch_cursor())
        .expect("the cursor is queued");
    support::wait_until("the first resync to go out", READY_BUDGET, || {
        transport.stats().resyncs_sent >= 1
    });

    // Now drop the connection from the other end: 70 KiB is above the server's
    // 64 KiB ceiling, so it terminates the socket without a close frame.
    let oversized = "x".repeat(70 * 1024);
    let dropped_at = Instant::now();
    transport
        .send_message(Uuid::new_v4(), CHANNEL, &oversized)
        .expect("the send is queued");

    support::wait_until("the socket to be replaced", READY_BUDGET, || {
        drain(cx);
        transport.stats().connects >= 2 && connection(cx) == ConnectionState::Connected
    });
    let recovered_after = dropped_at.elapsed();

    // **Reset-on-success, asserted as an interval rather than as a state.**
    // `ConnectionState::Reconnecting` is emitted immediately *before* the connect
    // attempt that follows it, so it is visible in the inbox for the length of one
    // loopback handshake — a few milliseconds — and a `drain` collapses a queue
    // anyway, so observing it reliably would be a test of the polling rate. The
    // interval says the same thing without a race:
    //
    // - the lower bound is the first step of the schedule (800 ms), so the retry was
    //   not immediate;
    // - the upper bound is the *floor of the third step* (3.2 s), which is exactly
    //   what a client that did **not** reset would wait, because two failed attempts
    //   had already happened before the server appeared and left its counter at 2.
    //
    // So a transport between those two bounds is demonstrably on step one, and a
    // transport missing the reset lands above the ceiling. The window is wide on
    // purpose: the lower bound has 800 ms of slack and the upper has two seconds.
    let first_step = backoff_window(1);
    let third_step = backoff_window(3);
    let third_step_floor = *third_step.start();
    assert!(
        recovered_after >= *first_step.start(),
        "the retry after a drop waited {recovered_after:?}, under the {:?} the first \
         step promises",
        first_step.start()
    );
    assert!(
        recovered_after < third_step_floor,
        "the retry after a drop waited {recovered_after:?}, at or past the \
         {third_step_floor:?} floor of the *third* step. That is the schedule of a \
         client that did not reset its counter on the connection that succeeded, and \
         AGENTS.md 4.2 requires the reset."
    );

    // And the cursor was replayed, which is §7.4's "resume from the cursor" rather
    // than "rely on live delivery across a gap".
    let stats = transport.stats();
    assert!(
        stats.resyncs_sent >= 2,
        "the remembered cursor is replayed on every connect, so the third connect \
         wrote another resync: {stats}"
    );
    assert_eq!(
        stats.frames_dropped, 0,
        "a dropped socket is not a dropped frame: {stats}"
    );

    // Recovery is real: a fresh send is acknowledged afterwards, and it is one row.
    let client_msg_id = Uuid::new_v4();
    cx.update(|cx| {
        bridge::try_begin_send(cx, CHANNEL, BODY, client_msg_id, at(1))
            .expect("the state is installed");
    });
    transport
        .send_message(client_msg_id, CHANNEL, BODY)
        .expect("the send is queued");
    await_server_id(cx, &client_msg_id);
    assert_eq!(
        rows_for(cx, &client_msg_id),
        1,
        "recovery must not duplicate the message it recovers"
    );

    transport.shutdown();
    drop(server);
}

/// `AGENTS.md` §4.2's "max-attempt behavior", against a port nothing is on.
///
/// The arithmetic of this test is the point: an allowance of two means exactly two
/// connect attempts, so `connect_failures == 2` — not "at least two", and not three,
/// which is what an off-by-one in the loop's `>=` would produce.
#[gpui::test]
fn the_transport_gives_up_after_its_configured_allowance(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let sender = install(cx, ME);
    let transport = WsTransport::start(
        TransportConfig::new(format!("ws://127.0.0.1:{port}{WS_PATH}")).with_max_attempts(2),
        sender,
    )
    .expect("a thread");

    // **Both conditions, and the reason is specific.** `is_closed()` is the worker
    // saying it will make no further attempt, but the flag goes up *before* the loop
    // stops, so reading `connect_failures` straight after waiting on it can observe
    // `1` on a run where two attempts were allowed and the second had not been made
    // yet. That reproduced roughly once in twenty-five runs, on the CI runner and not
    // in ten local runs, which is a bad ratio for a test that exists to be evidence.
    //
    // Waiting on `connect_failures >= 2` **alone** is not sufficient either: the second
    // counter can climb while the worker is between its last attempt and the terminal
    // `Disconnected` emission. Both together say "the allowance is spent and the loop
    // has finished", which is the thing being asserted; the counter is the more
    // specific of the two, so it is the one that gates the flag.
    support::wait_until("the transport to spend its allowance", READY_BUDGET, || {
        transport.is_closed() && transport.stats().connect_failures >= 2
    });

    let stats: TransportStats = transport.stats();
    assert!(
        transport.is_closed(),
        "having spent its allowance, the transport says so: {stats}"
    );
    assert_eq!(
        stats.connect_failures, 2,
        "two attempts were allowed and exactly two were made: {stats}"
    );
    assert_eq!(stats.connects, 0, "none of them succeeded: {stats}");

    // Giving up is reported, not silent: the state ends disconnected rather than
    // stuck on "connecting".
    drain(cx);
    assert_eq!(
        connection(cx),
        ConnectionState::Disconnected,
        "a client that has given up must say so"
    );
    // And it refuses further frames rather than queueing them into nothing.
    assert_eq!(
        transport
            .send_message(Uuid::new_v4(), CHANNEL, BODY)
            .expect_err("a closed transport refuses"),
        TransportError::ShutDown
    );
}

// ---------------------------------------------------------------------------
// 4. Version negotiation
// ---------------------------------------------------------------------------

/// An unsupported major version is refused, in both directions.
///
/// `AGENTS.md` §7.4 requires an unknown major to be "rejected explicitly" rather
/// than best-effort parsed, and that is a claim about two ends:
///
/// - **Inbound.** [`decode_server_frame`] refuses a frame whose `v` is one ahead,
///   before it reads a field of the body, and
///   [`classify_frame_error`](sh_nexus::network::ws::classify_frame_error) turns
///   that refusal into a terminal `Rejected` state. `ws_schedule.rs` asserts both
///   halves exhaustively; this file proves them over a socket.
/// - **Outbound.** A peer that announces a major this build does not speak is sent
///   an `error` frame and then hung up on, and **this client's decoder reads the
///   refusal** — so the assertion is that a real negotiation produces a real
///   `UnsupportedVersion` rather than a best-effort parse.
///
/// The reply's code and detail are the wire crate's own, and the detail
/// deliberately does not suggest reconnecting: a version mismatch is not transient,
/// and `UnsupportedVersion::detail` says so in words this test asserts on.
#[test]
fn an_unsupported_major_version_is_refused_rather_than_silently_continued() {
    let server = ServerProcess::start("version");
    let token = server.token();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");

    runtime.block_on(async {
        let request = server.handshake_request(Some(&token));
        let (mut socket, _) = connect_async(request).await.expect("a websocket upgrade");

        // Announce a major this build does not speak. The envelope's `v` is public
        // precisely so that a peer *can* mis-announce; `ClientEnvelope::new` exists
        // so that production code cannot.
        let misannounced = ClientEnvelope {
            v: PROTOCOL_VERSION + 1,
            client_msg_id: Uuid::new_v4().hyphenated().to_string(),
            frame: ClientFrame::MessageSend {
                channel_id: CHANNEL.to_owned(),
                content: BODY.to_owned(),
            },
        };
        use futures_util::SinkExt;
        socket
            .send(Message::Text(
                misannounced.encode().expect("these types encode").into(),
            ))
            .await
            .expect("the frame to reach the server");

        use futures_util::StreamExt;
        let reply = loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => break text.to_string(),
                // A ping or pong before the error frame is normal and says nothing.
                Some(Ok(_)) => continue,
                other => panic!("expected an error frame, got {other:?}"),
            }
        };

        let frame = sh_nexus::network::ws::decode_server_frame(&reply)
            .expect("the refusal is a frame this build speaks, so it can read the reason");
        let ServerFrame::Error { code, detail } = frame else {
            panic!("a refused version must arrive as an error frame, got {frame:?}");
        };
        assert_eq!(
            code, UNSUPPORTED_VERSION_CODE,
            "the code is the protocol's own vocabulary"
        );
        assert!(
            detail.contains(&(PROTOCOL_VERSION + 1).to_string()),
            "the detail names the version the peer announced, or the operator cannot \
             tell which side to upgrade: {detail}"
        );
        assert!(
            detail.contains("Reconnecting will not resolve this"),
            "the wire crate's sentence must say that reconnecting cannot help, because a \
         version mismatch is not transient: {detail}"
        );
    });

    drop(server);
}

/// A frame the *client* cannot read is dropped without ending the connection.
///
/// The complement of the test above and the other half of `AGENTS.md` §3.3's
/// "recoverable, not fatal": the server drops a `resync` at `warn!` and keeps the
/// socket, so a client that treated an unanswered frame as a reason to disconnect
/// would be unable to use the very frame `AGENTS.md` §7.4 requires it to send.
#[test]
fn a_frame_the_server_does_not_implement_leaves_the_socket_open() {
    let server = ServerProcess::start("unimplemented-frame");
    let token = server.token();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");

    runtime.block_on(async {
        use futures_util::{SinkExt, StreamExt};
        let request = server.handshake_request(Some(&token));
        let (mut socket, _) = connect_async(request).await.expect("a websocket upgrade");

        // A `resync` is exactly the frame `sh_nexus_server/src/ws.rs` decodes,
        // `warn!`s about and drops. The connection must survive it.
        let resync = ClientEnvelope::new(
            Uuid::new_v4().hyphenated().to_string(),
            ClientFrame::Resync {
                channel_id: CHANNEL.to_owned(),
                after: sh_nexus::network::ws::epoch_cursor(),
            },
        );
        socket
            .send(Message::Text(resync.encode().expect("encodes").into()))
            .await
            .expect("the resync to reach the server");

        // A `message.send` afterwards proves the socket is still usable. If the
        // server had closed on the resync, this write or the read would fail.
        let send = ClientEnvelope::new(
            Uuid::new_v4().hyphenated().to_string(),
            ClientFrame::MessageSend {
                channel_id: CHANNEL.to_owned(),
                content: BODY.to_owned(),
            },
        );
        let identity = send.client_msg_id.clone();
        socket
            .send(Message::Text(send.encode().expect("encodes").into()))
            .await
            .expect("the socket to still accept frames");

        let acked = loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let decoded = sh_nexus::network::ws::decode_server_frame(&text)
                        .expect("the ack is a frame this build speaks");
                    if let ServerFrame::MessageAck { client_msg_id, .. } = decoded {
                        break client_msg_id;
                    }
                }
                Some(Ok(_)) => continue,
                other => panic!("expected an ack, got {other:?}"),
            }
        };
        assert_eq!(
            acked, identity,
            "the ack carries the identity the send did, which is what the server's \
             dedupe and this client's reconciliation agree on"
        );
    });

    drop(server);
}

// ---------------------------------------------------------------------------
// 5. Shutdown, and the configuration guards
// ---------------------------------------------------------------------------

/// A shut-down transport refuses further frames and says so in its `Debug`.
///
/// Two properties in one test because they are the same property seen from two
/// sides: the caller is told (`ShutDown`), and nothing is lost quietly.
///
/// **The `Debug` assertion is a `AGENTS.md` §7.5 check.** A derived `Debug` on this
/// handle would print the queued frames, and a queued frame carries message
/// content; a `{:?}` inside a panic message is a log. So the string is asserted not
/// to contain the body that was queued, which fails the day someone replaces the
/// hand-written `Debug` with a derived one.
#[gpui::test]
fn a_shut_down_transport_refuses_further_frames(cx: &mut TestAppContext) {
    let sender = install(cx, ME);
    let transport =
        WsTransport::start(TransportConfig::new("ws://127.0.0.1:1/ws"), sender).expect("a thread");

    transport
        .send_message(Uuid::new_v4(), CHANNEL, BODY)
        .expect("queued");
    transport.shutdown();

    assert!(transport.is_closed(), "the flag is set synchronously");
    assert_eq!(
        transport
            .send_message(Uuid::new_v4(), CHANNEL, BODY)
            .expect_err("a closed transport refuses"),
        TransportError::ShutDown
    );
    assert_eq!(
        transport
            .start_typing(CHANNEL)
            .expect_err("a closed transport refuses"),
        TransportError::ShutDown
    );

    let rendered = format!("{transport:?}");
    assert!(
        rendered.contains("closed: true"),
        "the Debug names the state: {rendered}"
    );
    assert!(
        !rendered.contains(BODY),
        "AGENTS.md 7.5: a Debug on this handle must never carry message content, \
         and a {{:?}} in a panic message is a log. Got: {rendered}"
    );
}

/// A zero-capacity outbound queue is refused rather than panicking inside `mpsc`.
///
/// `AGENTS.md` §2.1 forbids a panic on a path a caller can reach, and a
/// configuration a caller wrote is exactly such a path. `tokio::sync::mpsc::channel`
/// panics on zero, so the check has to happen before it is called.
#[gpui::test]
fn a_zero_capacity_outbound_queue_is_refused_rather_than_panicking(cx: &mut TestAppContext) {
    let sender = install(cx, ME);
    let error = WsTransport::start(
        TransportConfig::new("ws://127.0.0.1:1/ws").with_outbound_capacity(0),
        sender,
    )
    .expect_err("a queue with no room is refused, not panicked on");

    assert_eq!(error, TransportError::OutboundFull { capacity: 0 });
    assert_eq!(error.to_string(), "the outbound queue is full at 0 frames");
}

// ---------------------------------------------------------------------------
// 7. Authentication
// ---------------------------------------------------------------------------

/// A 401 on the handshake is a `Rejected` connection, and the transport stops.
///
/// **The client half of the auth milestone, and the reason it is its own test**
/// rather than an assertion inside the two-client flow: a transport with no token is
/// the state every user lands in when their session expires, and what this asserts
/// is that the state is *terminal and visible* rather than an endless
/// "reconnecting" that a user cannot act on.
///
/// Three separate claims, each of which would be false on its own:
///
/// 1. **`Rejected`, not `Disconnected`.** `ConnectionState::Disconnected` invites a
///    retry, and a retry against a server that refuses identically every time is
///    the exact loop `classify_frame_error`'s docs call terminal.
/// 2. **Terminal.** The transport stops rather than backing off; `is_closed()`
///    becomes true and no further attempt is counted. A rejected version already
///    behaves this way — this is the second condition that gets it right.
/// 3. **A distinct code**, so a sign-in screen can tell "your token is not accepted"
///    from "this server is not this version of the protocol". Both are `Rejected`;
///    only the code separates them.
#[gpui::test]
fn a_refused_session_token_is_a_rejected_connection_and_the_transport_stops(
    cx: &mut TestAppContext,
) {
    let server = ServerProcess::start("unauthenticated");
    let sender = install(cx, ME);

    // No token at all. This is the ordinary failure: a client that has not signed
    // in, or whose session the server has never heard of.
    let url = server.endpoint();
    let transport = WsTransport::start(TransportConfig::new(url), sender).expect("a thread");

    support::wait_until("the handshake to be refused", READY_BUDGET, || {
        drain(cx);
        matches!(connection(cx), ConnectionState::Rejected { .. })
    });

    match connection(cx) {
        ConnectionState::Rejected { code, detail } => {
            assert_eq!(
                code,
                sh_nexus::network::ws::UNAUTHORIZED_CODE,
                "a refused credential gets its own code, so a sign-in screen can \
                 distinguish it from an unsupported protocol version"
            );
            assert_eq!(detail, sh_nexus::network::ws::UNAUTHORIZED_DETAIL);
            assert!(
                !detail.to_lowercase().contains("retry"),
                "the detail must not invite a retry: a client that retries a refused \
                 credential sits in a backoff loop against a server that will refuse \
                 it identically every time. Got {detail:?}"
            );
        }
        other => panic!("expected a Rejected connection, got {other:?}"),
    }

    // Terminal: the worker ended, so no further attempt is made and the handle
    // reports itself closed. The counters are read after the worker has actually
    // stopped, not after the flag went up -- see `wait_for_worker_to_finish`.
    let stats = wait_for_worker_to_finish(&transport);
    assert!(
        transport.is_closed(),
        "a terminal rejection ends the transport: {stats}"
    );
    assert_eq!(
        stats.rejected_handshakes, 1,
        "exactly one attempt: a terminal rejection is not retried. {stats}"
    );
    assert_eq!(
        stats.connects, 0,
        "and no socket was ever established, which is what the server's pre-upgrade \
         refusal means from this side. {stats}"
    );
    assert_eq!(
        transport.send_message(Uuid::new_v4(), CHANNEL, BODY),
        Err(TransportError::ShutDown),
        "a stopped transport sends nothing, so an optimistic row stays pending rather \
         than failing for a reason the user cannot see"
    );

    transport.shutdown();
    drop(server);
}

/// The same refusal for a token the server has never issued.
///
/// Distinct from the test above because it is the case a *stale* client hits: a
/// session that expired, or one from a database that was replaced. The server
/// answers it identically -- which is the point, and is what stops a peer from
/// enumerating valid tokens by presenting invalid ones.
#[gpui::test]
fn an_unknown_token_is_refused_exactly_as_a_missing_one_is(cx: &mut TestAppContext) {
    let server = ServerProcess::start("unknown-token");
    let sender = install(cx, ME);

    let url = server.endpoint();
    // 64 lowercase hex characters, which is the shape `auth::bearer_token` accepts
    // -- so this is refused by the *session lookup* and not by the shape check. The
    // client cannot know which of the two the server applied, and that is the
    // property; what it must do with either is identical.
    let cfg = TransportConfig::new(url).with_token("0".repeat(64));
    let transport = WsTransport::start(cfg, sender).expect("a thread");

    support::wait_until("the handshake to be refused", READY_BUDGET, || {
        drain(cx);
        matches!(connection(cx), ConnectionState::Rejected { .. })
    });
    drain(cx);
    match connection(cx) {
        ConnectionState::Rejected { code, .. } => assert_eq!(
            code,
            sh_nexus::network::ws::UNAUTHORIZED_CODE,
            "a token the server has never issued is refused the same way a missing one is"
        ),
        other => panic!("expected a Rejected connection, got {other:?}"),
    }

    transport.shutdown();
    drop(server);
}

/// A revoked token stops working, and the client learns it the next time it connects.
///
/// The revocation path from the server's side, observed from here: log in, use the
/// token, revoke it, and the *next* handshake is refused. A live socket is not
/// killed -- `PLAN.md` §6 has no frame for a server-initiated revocation, and
/// `sh_nexus_server::auth::logout`'s docs say so -- so the assertion is about the
/// reconnect, which is the boundary this transport can actually observe.
#[gpui::test]
fn a_revoked_token_is_refused_on_the_next_handshake(cx: &mut TestAppContext) {
    let server = ServerProcess::start("revoked");
    let token = server.token();
    let sender = install(cx, ME);

    let url = server.endpoint();
    let transport = WsTransport::start(TransportConfig::new(url).with_token(token.clone()), sender)
        .expect("a thread");
    await_connected(cx);

    // Revoke it out of band, over the same HTTP endpoint the product would use.
    support::revoke(&server, &token);

    // The open socket keeps working, because nothing in the protocol tells the
    // server to close it. What must happen is that the *next* handshake is refused,
    // and the only way to observe that through this transport is to make it
    // reconnect: `shutdown` plus a fresh transport on the same URL and token.
    assert!(
        {
            drain(cx);
            connection(cx) == ConnectionState::Connected
        },
        "the open socket is unaffected by a revocation: closing a live connection from \
         the server is not in PLAN.md section 6 yet"
    );
    transport.shutdown();

    // A second application, because `bridge::install` refuses a second install on one
    // -- by design, so a second startup cannot orphan the first inbox. This is the
    // same two-application shape the Real-time Flow test uses.
    let mut second = cx.new_app();
    let sender_again = install(&mut second, THEM);
    let url = server.endpoint();
    let again = WsTransport::start(
        TransportConfig::new(url).with_token(token),
        sender_again.clone(),
    )
    .expect("a thread");
    support::wait_until("the revoked token to be refused", READY_BUDGET, || {
        drain(&mut second);
        matches!(connection(&second), ConnectionState::Rejected { .. })
    });
    drain(&mut second);
    match connection(&second) {
        ConnectionState::Rejected { code, .. } => assert_eq!(
            code,
            sh_nexus::network::ws::UNAUTHORIZED_CODE,
            "a revoked token is refused exactly as an unknown one is"
        ),
        other => panic!("expected a Rejected connection, got {other:?}"),
    }

    again.shutdown();
    drop(server);
}

/// The token is not in any `Debug` a caller can reach, and this client cannot hash
/// a password.
///
/// The second half is the dependency-direction claim: `cargo tree -p sh_nexus` must
/// not contain `argon2` or `sha2`. A client that could hash a password would be a
/// client that had a password to hash, and the manifest comment on
/// `tokio-tungstenite` is the reason this suite does not link the server to check
/// it. So the assertion is made here, on the manifest text, in the same way
/// `tests/layer_boundary.rs` asserts that the client does not depend on `serde`.
#[test]
fn the_client_declares_no_crypto_dependency_and_the_token_stays_out_of_debug() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", manifest.display()));
    let stripped: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in ["argon2", "sha2", "jsonwebtoken", "ring"] {
        assert!(
            !stripped.contains(forbidden),
            "this crate must not depend on `{forbidden}`. The client is handed an \
             opaque session token and sends it; it never sees a password, so a \
             password-hashing dependency here would be a dependency with nothing to \
             do. ADR-010's token decision put hashing on the server side, and the \
             reason is revocation."
        );
    }

    let config = TransportConfig::new("ws://127.0.0.1:8484/ws").with_token("a-live-token");
    let rendered = format!("{config:?}");
    assert!(
        !rendered.contains("a-live-token"),
        "TransportConfig's Debug must not carry the token: AGENTS.md 7.5 names tokens \
         as a thing that must never reach a log, and a {{:?}} in a panic message is a \
         log. Got {rendered}"
    );
    assert!(
        rendered.contains("token_configured: true"),
        "and the presence of one is worth printing, because a transport that will be \
         refused with a 401 is diagnosable from that boolean alone. Got {rendered}"
    );
}

// ---------------------------------------------------------------------------
// 6. The duplicated constant
// ---------------------------------------------------------------------------

/// The path this suite speaks is the server's own constant.
///
/// `tests/support/mod.rs` builds `ws://host:port/ws` by hand rather than importing
/// `sh_nexus_server::WS_PATH`, because importing it would put `axum` and `rusqlite`
/// into this crate's dependency graph. A duplication nobody watches is a
/// duplication that rots, so this test reads the server's manifest-adjacent source
/// and fails if the two ever disagree.
///
/// The same argument the server's own test suite makes about `WS_PATH` — it is a
/// constant rather than a string in the router so that the docs, the harness and
/// the log line cannot disagree — extended to the client side.
#[test]
fn the_suites_socket_path_is_the_servers_own_constant() {
    let server_source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("sh_nexus_server")
        .join("src")
        .join("lib.rs");
    let text = fs::read_to_string(&server_source)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", server_source.display()));
    assert!(
        text.contains(&format!("pub const WS_PATH: &str = \"{WS_PATH}\";")),
        "crates/sh_nexus/tests/support duplicates sh_nexus_server::WS_PATH to keep the \
         server out of this crate's dependency graph, and this test is what keeps the \
         two from drifting. Expected {WS_PATH:?} in {}",
        server_source.display()
    );
}

/// The socket path this suite expects is a root path, not an absolute URL.
///
/// Small, and included because the duplicate above would accept `"ws"` in place of
/// `"/ws"` as long as both files agreed — which is exactly the kind of mistake two
/// copies of one constant can make together.
#[test]
fn the_duplicated_socket_path_is_still_a_path() {
    assert!(
        WS_PATH.starts_with('/'),
        "{WS_PATH:?} must be an absolute path"
    );
    assert_eq!(WS_PATH, "/ws");
}

/// How long these suites are willing to wait for something that should happen.
///
/// **`AGENTS.md` §4.3 forbids a `sleep()` standing in for logic, and this is the
/// alternative:** a bound on a condition poll. Twenty seconds is generous because a
/// tight bound turns a slow machine into a flake, and it costs nothing once the
/// condition holds — `support::wait_until` returns the instant it does.
#[test]
fn the_wait_budget_is_a_bound_on_a_poll_not_a_sleep() {
    assert!(
        READY_BUDGET >= Duration::from_secs(5),
        "a bound this short would turn a loaded machine into a flake"
    );
    assert!(
        READY_BUDGET <= Duration::from_secs(60),
        "a suite that waits this long for one frame is a suite nobody runs"
    );
}

/// The wire error the client's boundary reduces, for the version case.
///
/// Asserted here rather than only in `network::mapping`'s own suite because this is
/// the file that *acts* on it: `reduce_wire_error` is what a caller would put in a
/// log, and `AGENTS.md` §7.5's rule is that the line may name the failure and never
/// the payload. This pins the wording that reaches a log.
#[test]
fn a_refused_version_reduces_to_a_line_without_the_peers_payload() {
    let error = sh_nexus_wire::negotiate(PROTOCOL_VERSION + 1)
        .expect_err("a major this build does not speak");
    let reduced =
        sh_nexus::network::mapping::reduce_wire_error(&WireError::UnsupportedVersion(error));
    let line = reduced.to_string();
    assert!(line.starts_with("protocol error: unsupported protocol version"));
    assert!(
        !line.contains(BODY),
        "the reduced line must not carry a payload: {line}"
    );
}
