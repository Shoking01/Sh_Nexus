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
//! | [`a_full_batch_is_followed_by_another_request_and_a_short_batch_stops_the_loop`] | the catch-up loop's whole policy, at both edges of the bound |
//! | [`the_cursor_advances_to_the_batch_maximum_and_a_duplicate_moves_it_too`] | the advancement rule that makes an inclusive server bound finite |
//! | [`a_catch_up_recovers_every_missed_message_with_no_gap_and_no_duplicate_row`] | §7.4's resume, across more than one batch |
//! | [`messages_sent_while_this_client_was_down_are_present_after_the_resync`] | §8.1's Reconnect Flow's outcome, after a real drop |
//! | [`the_transport_gives_up_after_its_configured_allowance`] | §4.2's max-attempt behaviour |
//! | [`an_unsupported_major_version_is_refused_rather_than_silently_continued`] | §7.4's explicit rejection, both directions |
//! | [`a_shut_down_transport_refuses_further_frames`] | `AGENTS.md` §2.1, and §7.5 through `Debug` |
//! | [`the_suites_socket_path_is_the_servers_own_constant`] | the duplication `tests/support/mod.rs` admits to is watched |
//! | [`the_shell_s_channel_is_one_the_server_accepts`] | the one duplicated constant that a behavioural test can watch, in place of a third copy of the literal |
//! | [`only_a_message_send_is_owed_when_its_write_fails`] | §7's outbox is owed durable content and **only** durable content; a registry guard over `FrameKind::CLIENT` |
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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use gpui::TestAppContext;
use sh_nexus::app::STARTUP_CHANNEL;
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::user::UserStatus;
use sh_nexus::network::ws::{
    advance_cursor, backoff_window, catch_up, is_owed_on_failed_write, CatchUp, TransportConfig,
    TransportError, TransportStats, WsTransport, RESYNC_BATCH_LIMIT,
};
use sh_nexus::state::bridge::{self, Delivery, DeliveryRefusal, DrainReport, EventSender};
use sh_nexus::state::{bridge::MAX_PENDING_EVENTS, SendOutcome};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, FrameKind, ServerFrame};
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

/// How many messages the catch-up fixture stores before the client under test
/// connects.
///
/// **More than one batch, and that is the whole point of the number.**
/// [`RESYNC_BATCH_LIMIT`] is 256; 300 is one full batch plus a short one, which is
/// the only pair of lengths that can tell "fetched everything" from "fetched the
/// first page and stopped". A client that ignored the follow-up would hold 256 rows
/// and pass a fixture that stored fewer.
const PLANTED_MESSAGES: usize = 300;

/// How many messages the reconnect fixture sends while the client is down.
const DURING_THE_GAP: usize = 4;

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
/// "recoverable, not fatal": the server drops a `typing.start` at `warn!` and keeps
/// the socket, so a client that treated an unanswered frame as a reason to disconnect
/// would be unable to use the very frames `AGENTS.md` §7.4 requires it to send.
///
/// **`typing.start` is the frame this names, and the choice is not incidental.**
/// It used to be `resync`, back when `sh_nexus_server/src/ws.rs` decoded it and
/// logged it without answering — which is precisely the gap this work unit closed. A
/// test whose subject had stopped existing would keep passing while describing a
/// server that no longer exists, so the frame moved to one the server still does not
/// implement and the property it protects is the same one.
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

        // A `typing.start` is exactly the frame `sh_nexus_server/src/ws.rs` decodes,
        // `warn!`s about and drops. The connection must survive it.
        let typing = ClientEnvelope::new(
            Uuid::new_v4().hyphenated().to_string(),
            ClientFrame::TypingStart {
                channel_id: CHANNEL.to_owned(),
            },
        );
        socket
            .send(Message::Text(typing.encode().expect("encodes").into()))
            .await
            .expect("the typing frame to reach the server");

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

/// Whether `stripped` declares `package` as a dependency.
///
/// **Matching the *declaration* rather than a substring, and this crate is what
/// proved the difference.** A bare `contains` scan of the manifest for `ring`
/// matches `keyring` — the cross-platform credential store, and the single crate
/// in this dependency set that most reduces plaintext token storage on disk. That
/// is the precise opposite of what the assertion below is for: the scan reported a
/// *credential store* as a *cryptographic one*.
///
/// The cost of the wrong answer is not a red build. It is a future maintainer
/// adding a legitimate dependency, seeing a ban they do not understand, and
/// working around a gate that was never actually about their crate. A gate that
/// cries wolf on its own dependency list stops being read.
///
/// So a dependency counts as declared when a line *begins* with the package name,
/// optionally quoted, followed by `=`. That is the shape Cargo gives a manifest
/// entry, and it cannot be produced by a longer name that merely contains a
/// forbidden token.
fn declares(stripped: &str, package: &str) -> bool {
    let bare = format!("{package} = ");
    let quoted = format!("\"{package}\" = ");
    stripped
        .lines()
        .map(|line| line.trim())
        .any(|line| line.starts_with(&bare) || line.starts_with(&quoted))
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
            !declares(&stripped, forbidden),
            "this crate must not depend on `{forbidden}`. The client is handed an \
             opaque session token and sends it; it never sees a password, so a \
             password-hashing dependency here would be a dependency with nothing to \
             do. ADR-010's token decision put hashing on the server side, and the \
             reason is revocation."
        );
    }

    // The scan is not vacuous. Tightening it to match declarations rather than
    // substrings could just as easily have tightened it into matching nothing, and
    // a crypto gate that detects no crypto dependencies is worse than no gate --
    // it is a green build carrying an unearned claim. This is the same discipline
    // `the_password_scanner_is_not_vacuous` applies to the credential scanner.
    for forbidden in ["argon2", "sha2", "jsonwebtoken", "ring"] {
        assert!(
            declares(&format!("\n{forbidden} = \"0.17.0\"\n"), forbidden),
            "the dependency scan must still catch a genuine `{forbidden}` \
             declaration; if this fails the gate above is matching nothing at all"
        );
        assert!(
            declares(
                &format!("\n\"{forbidden}\" = {{ version = \"0.17\" }}\n"),
                forbidden
            ),
            "the dependency scan must catch a quoted `{forbidden}` declaration too -- \
             Cargo allows both spellings and a gate that reads one of them reads a \
             convention, not a rule"
        );
    }

    // And the case that motivated the fix: a name which *contains* a forbidden
    // token is not a declaration of that token. `keyring` is in this manifest, and
    // it is the credential store ADR-014 put here on purpose -- so the gate above
    // passes while `keyring = ` sits three lines below it in the same file.
    assert!(
        declares(&stripped, "keyring"),
        "`keyring` is declared in this manifest; if it is not, ADR-014 has drifted \
         from the manifest"
    );
    assert!(
        stripped.contains("keyring = "),
        "the manifest text `keyring = ` is what made the substring scan misfire, so \
         its absence here means this test is no longer testing the thing it was \
         written to test"
    );

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

/// The shell's startup channel is one the server actually accepts.
///
/// **This is a behavioural assertion where a string comparison would have been
/// cheaper, and the reason it is not one is a dependency direction.** `sh_nexus`
/// cannot name `sh_nexus_server`'s seeded channel: a `[dev-dependencies]` entry
/// would put `axum` and `rusqlite` — SQLite's C amalgamation included — into the
/// client's dependency graph, which is the mistake ADR-002 exists to prevent and
/// the argument `tests/support/mod.rs` gives in full. Moving the name into the
/// shared `sh_nexus_wire` crate would be worse than useless, because that crate
/// describes frame shapes and the name one deployment gave its first channel is
/// not a frame shape.
///
/// **So both sides hold a literal — `sh_nexus::app::STARTUP_CHANNEL` and [`CHANNEL`]
/// above — and `assert_eq!` between them would be theatre.** A crate that cannot
/// see the server has no way to learn that two literals mean the same thing; it can
/// only report that they are the same string today, which is a fact about two files
/// rather than about a client and a server agreeing. What is actually wanted is the
/// sentence *"the server accepts a message naming this channel"*, and the only way
/// to learn it is to name the channel at a server that is running and insist on an
/// answer. A rename on either side then turns this test red instead of turning the
/// first message a user typed red — which is precisely how it went wrong once, when
/// the constant was a placeholder no deployment had ever seeded.
///
/// **The ack is the assertion, and not merely the absence of an error.** An
/// unknown channel is refused rather than ignored, so a test that watched only for
/// errors would still pass on a server that rejected the message and said so
/// clearly. Requiring a `message.ack` for that identity means the server *stored*
/// the message, which is the only outcome in which this constant names a channel
/// rather than a label.
///
/// **A failure here can mean two things, and both are the same bug.** `await_server_id`
/// reads [`CHANNEL`], not the imported constant, so a red says either that the
/// shell's constant has drifted from the server or that this file's own copy has.
/// That is deliberate rather than incidental: a suite copy nobody compares against
/// anything is a copy that rots in silence, which is exactly what
/// [`the_suites_socket_path_is_the_servers_own_constant`] exists to prevent for the
/// path.
///
/// **What this does not cover, stated rather than implied.** It proves the id is one
/// the server has seeded. It does not prove that id is the only channel a
/// deployment will ever have, and nothing here constrains the server's own seed
/// list. It does not prove the shell *opens* that channel — `STARTUP_CHANNEL` is
/// still the seeded channel rather than a chosen one, because no `DomainEvent`
/// carries a channel list and `state/bridge.rs` §5 assigns that gap its own entry;
/// the constant's own documentation says which half of it is still a placeholder.
/// And it does not exercise the composer: the frame is written by hand through
/// [`WsTransport::send_message`] with the constant as the channel, which is what
/// makes the claim about the *value*. That the shell's composer routes through this
/// same value is
/// `begin_send_puts_the_frame_on_the_wire_when_a_transport_is_published` in
/// `tests/app_shell.rs`, and the two are deliberately one sentence each: this one
/// that the server accepts the name, that one that the client sends it.
#[gpui::test]
fn the_shell_s_channel_is_one_the_server_accepts(cx: &mut TestAppContext) {
    let server = ServerProcess::start("startup-channel");
    let token = server.token();
    let sender = install(cx, ME);

    let transport = WsTransport::start(
        TransportConfig::new(server.endpoint()).with_token(token),
        sender,
    )
    .expect("a thread");
    await_connected(cx);

    // The optimistic row and then the frame, in the order
    // `MessageList::begin_send` puts them, and against the production constant
    // rather than this file's own copy: the row is on screen before the server has
    // seen anything (`AGENTS.md` §8.1), and the channel is filled in from the very
    // constant `Shell::new` showed the list. `AGENTS.md` §3.2 gives `state/` no
    // clock, so the timestamp is the caller's, exactly as `InputBar::send` supplies.
    let client_msg_id = Uuid::new_v4();
    cx.update(|cx| {
        bridge::try_begin_send(cx, STARTUP_CHANNEL, BODY, client_msg_id, at(0))
            .expect("the state is installed")
    });
    transport
        .send_message(client_msg_id, STARTUP_CHANNEL, BODY)
        .expect("the send is queued");

    // The server stored it under this identity. Nothing else in this file can say
    // that: every other assertion here is about a frame this client wrote.
    await_server_id(cx, &client_msg_id);

    let rows = cx.read(|app| {
        bridge::try_read(app, |state| {
            state
                .messages(STARTUP_CHANNEL)
                .iter()
                .filter(|held| held.client_msg_id == client_msg_id)
                .count()
        })
        .unwrap_or(0)
    });
    assert_eq!(
        rows, 1,
        "the acknowledged send is one row, in the channel the send named: the ack \
         reconciled the optimistic row rather than arriving as somebody else's second \
         copy"
    );

    let stats = transport.stats();
    assert_eq!(
        stats.frames_dropped, 0,
        "the server answered in frames this build speaks, so the ack above was read \
         rather than discarded: {stats}"
    );
    assert_eq!(
        stats.events_refused, 0,
        "and nothing was refused on the way in, or the row could not have been \
         reconciled: {stats}"
    );

    transport.shutdown();
    drop(server);
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

// ---------------------------------------------------------------------------
// 8. Which frame is owed when its write fails
// ---------------------------------------------------------------------------

/// One representative of every client frame kind.
///
/// **A function rather than five literals in the test, because it is the *whole*
/// enumeration.** `FrameKind::CLIENT` is the protocol's own closed list, and the
/// test below walks it and asks [`is_owed_on_failed_write`] about each entry — so a
/// sixth client frame kind is a compile error here and a decision somebody has to
/// make, rather than a variant that quietly joins the "not owed" side because
/// nobody re-read the filter.
fn one_of_every(kind: FrameKind) -> ClientFrame {
    match kind {
        FrameKind::MessageSend => ClientFrame::MessageSend {
            channel_id: CHANNEL.to_owned(),
            content: BODY.to_owned(),
        },
        FrameKind::ReactionAdd => ClientFrame::ReactionAdd {
            message_id: "m_1".to_owned(),
            emoji: "\u{1F44D}".to_owned(),
        },
        FrameKind::TypingStart => ClientFrame::TypingStart {
            channel_id: CHANNEL.to_owned(),
        },
        FrameKind::TypingStop => ClientFrame::TypingStop {
            channel_id: CHANNEL.to_owned(),
        },
        FrameKind::Resync => ClientFrame::Resync {
            channel_id: CHANNEL.to_owned(),
            after: sh_nexus::network::ws::epoch_cursor(),
        },
        // Server-to-server kinds are not constructible as a client frame, and
        // naming them makes that a compile error rather than a silent skip.
        other => panic!("{other:?} is not a client frame kind"),
    }
}

/// **Exactly `message.send` is owed a re-drive when its write fails — and every
/// other client frame is not.**
///
/// **The asymmetry is the design, so it is asserted one frame at a time rather
/// than as "the set is `{MessageSend}`".** A `typing.start` that failed to write
/// is *not* owed: the indicator is advisory, the server's inactivity timeout is the
/// backstop, and re-sending a stale `true` shows a typist who stopped typing — a
/// *worse* failure than the one being repaired. A `reaction.add` is not owed
/// either: it is an increment the user can see did not stick, and re-driving it
/// silently would undo a later deliberate removal. A `resync` is not owed because
/// the cursor is remembered and replayed on the next connect regardless, so a
/// failed one has already been scheduled.
///
/// **What is owed is the one frame carrying durable, user-authored content** —
/// the only frame whose loss is a lost message rather than a lost hint.
///
/// **This is the whole of the filter, and it is reachable because it is a `pub
/// fn`.** A decision the worker makes on a socket is otherwise unobservable from
/// outside the socket, which is the reason every other pure decision in `ws.rs`
/// (`keepalive_action`, `classify_frame_error`, `is_unauthorized`, `backoff_delay`)
/// is shaped the same way.
#[test]
fn only_a_message_send_is_owed_when_its_write_fails() {
    let owed: Vec<FrameKind> = FrameKind::CLIENT
        .iter()
        .copied()
        .filter(|kind| is_owed_on_failed_write(&one_of_every(*kind)))
        .collect();

    assert_eq!(
        owed,
        vec![FrameKind::MessageSend],
        "the owed set is the frames whose loss is a lost message. `typing.start`, \
         `typing.stop` and `reaction.add` are hints the peer is better off without, \
         and `resync` is replayed from the remembered cursor on the next connect."
    );

    // **Named individually as well, so a failure says which frame changed its mind**
    // rather than printing two vectors and leaving the reader to diff them.
    for kind in [
        FrameKind::TypingStart,
        FrameKind::TypingStop,
        FrameKind::ReactionAdd,
    ] {
        assert!(
            !is_owed_on_failed_write(&one_of_every(kind)),
            "{kind:?} must not be owed: re-sending a stale ephemeral frame is worse \
             than losing it, which is why only `message.send` is"
        );
    }
    assert!(
        !is_owed_on_failed_write(&one_of_every(FrameKind::Resync)),
        "`resync` must not be owed: `WsTransport::request_resync` remembers the \
         cursor and replays it on every connect, so a failed one is already scheduled"
    );
}

// ---------------------------------------------------------------------------
// 9. The catch-up loop
// ---------------------------------------------------------------------------

/// A full batch is followed by another request, and a short batch stops the loop.
///
/// **The whole of "ask again while a batch comes back full", asserted at both edges
/// of the bound and one past it.** The rule the client runs on is
/// [`catch_up`], and it is the only thing standing between an inclusive server bound
/// and an unbounded loop: the server's `after` is inclusive, so the boundary
/// millisecond comes back every time, and the loop terminates because a *short*
/// batch produces no further request.
///
/// **The three cases and what each one rules out:**
///
/// | `received` | Answer | The bug it catches |
/// |---|---|---|
/// | 1 | `Pending` | a client that asks again after every message, forever |
/// | `RESYNC_BATCH_LIMIT - 1` | `Pending` | an off-by-one that stops one message early |
/// | `RESYNC_BATCH_LIMIT` | `AskAgain` | a client that truncates at one batch and silently loses the rest |
/// | `RESYNC_BATCH_LIMIT + 9` | `AskAgain` | `==` instead of `>=`, which stalls on a counter that ran past the bound |
#[test]
fn a_full_batch_is_followed_by_another_request_and_a_short_batch_stops_the_loop() {
    assert_eq!(
        catch_up(1, RESYNC_BATCH_LIMIT),
        CatchUp::Pending,
        "one message is a short batch, so the client is caught up and asks nothing \
         more. A client that re-asked here would loop on the inclusive boundary row \
         the server keeps sending back."
    );
    assert_eq!(
        catch_up(RESYNC_BATCH_LIMIT - 1, RESYNC_BATCH_LIMIT),
        CatchUp::Pending,
        "a batch one short of the bound is still short: there may be nothing after it, \
         and the client cannot see the batch end, so it stops"
    );
    assert_eq!(
        catch_up(RESYNC_BATCH_LIMIT, RESYNC_BATCH_LIMIT),
        CatchUp::AskAgain,
        "a batch that came back full is the server's own statement that more rows \
         exist, and stopping here would lose them"
    );
    assert_eq!(
        catch_up(RESYNC_BATCH_LIMIT + 9, RESYNC_BATCH_LIMIT),
        CatchUp::AskAgain,
        "the rule is `>=`, not `==`: a counter that ran past the bound must ask \
         again rather than leave messages on the table"
    );
}

/// The cursor advances to the batch maximum, and a duplicate moves it too.
///
/// **This is the rule that makes an inclusive server bound safe, and it is the
/// opposite of the intuitive version.** The server returns the boundary millisecond
/// on every request, so the newest row of every full batch is usually one the client
/// already holds. A cursor that advanced only on *newly accepted* rows would
/// therefore never move past that boundary and would re-read the same millisecond
/// forever — the client-side half of the same lost-message problem the inclusive
/// bound fixes on the server, pointed the other way.
///
/// The second half is equally load-bearing: **the cursor must never move backwards**,
/// or a late frame carrying an older `accepted_at` would drag it behind and the next
/// catch-up would re-read everything after it.
#[test]
fn the_cursor_advances_to_the_batch_maximum_and_a_duplicate_moves_it_too() {
    let mut cursors: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();

    assert_eq!(
        advance_cursor(&mut cursors, CHANNEL, at(10)),
        at(10),
        "the first row of a channel defines its cursor"
    );
    assert_eq!(
        advance_cursor(&mut cursors, CHANNEL, at(10)),
        at(10),
        "the replayed boundary row -- the same instant the client already holds -- \
         moves nothing, because the cursor is already there"
    );
    assert_eq!(
        advance_cursor(&mut cursors, CHANNEL, at(30)),
        at(30),
        "the newest row of a batch does move it, which is what \"advance to the batch \
         maximum\" means"
    );
    assert_eq!(
        advance_cursor(&mut cursors, CHANNEL, at(4)),
        at(30),
        "and an out-of-order older row must not rewind it: the next catch-up from a \
         rewound cursor would re-read everything after it"
    );
    assert_eq!(
        advance_cursor(&mut cursors, "c_another", at(7)),
        at(7),
        "channels are independent, which is the whole reason the cursor is per channel"
    );
    assert_eq!(
        cursors.get(CHANNEL),
        Some(&at(30)),
        "and the map is the client's record of what it holds, one entry per channel"
    );
}

/// A catch-up recovers every message the client missed, with no gap and no duplicate.
///
/// **The end-to-end proof of `AGENTS.md` §7.4's resume rule and §8.1's Reconnect
/// Flow's "no duplicates, no gaps", against a real server and a real socket.** More
/// than one batch on purpose: a client that fetched only the first
/// [`RESYNC_BATCH_LIMIT`] rows would pass a smaller fixture and lose messages in
/// production, and that is precisely the bug a bound can hide.
///
/// The second connection is a raw `tokio-tungstenite` socket rather than another
/// [`WsTransport`], so it can put 300 messages on the wire **before** the transport
/// under test has connected — which is what makes "what this client missed" a fact
/// rather than a race. Each send is acknowledged before the next goes out, so all
/// 300 are stored when the transport starts.
#[gpui::test]
fn a_catch_up_recovers_every_missed_message_with_no_gap_and_no_duplicate_row(
    cx: &mut TestAppContext,
) {
    let server = ServerProcess::start("catch-up");
    let token = server.token();

    // A second client fills the channel while the one under test is not connected.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    runtime.block_on(async {
        use futures_util::{SinkExt, StreamExt};
        let request = server.handshake_request(Some(&token));
        let (mut other, _) = connect_async(request).await.expect("a websocket upgrade");
        for index in 0..PLANTED_MESSAGES {
            let envelope = ClientEnvelope::new(
                Uuid::new_v4().hyphenated().to_string(),
                ClientFrame::MessageSend {
                    channel_id: CHANNEL.to_owned(),
                    content: format!("missed message {index}"),
                },
            );
            other
                .send(Message::Text(envelope.encode().expect("encodes").into()))
                .await
                .expect("the send to reach the server");
            // Read the ack before the next send, so the server has committed this row
            // before the loop moves on and no write is left buffered against a
            // receive window nobody is draining.
            loop {
                match other.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let decoded = sh_nexus::network::ws::decode_server_frame(&text)
                            .expect("the ack is a frame this build speaks");
                        if matches!(decoded, ServerFrame::MessageAck { .. }) {
                            break;
                        }
                    }
                    Some(Ok(_)) => continue,
                    other => panic!("expected an ack, got {other:?}"),
                }
            }
        }
    });

    let sender = install(cx, ME);
    let transport = WsTransport::start(server.authenticated_config(), sender).expect("a thread");
    await_connected(cx);

    // The ask, from the beginning of time, exactly as `epoch_cursor` documents.
    transport
        .request_resync(CHANNEL, sh_nexus::network::ws::epoch_cursor())
        .expect("the cursor is queued");

    support::wait_until("every missed message to arrive", READY_BUDGET, || {
        drain(cx);
        cx.read(|app| {
            bridge::try_read(app, |state| {
                state.message_count(CHANNEL) >= PLANTED_MESSAGES
            })
            .unwrap_or(false)
        })
    });

    // **No duplicate row, and this is the half that is easy to get wrong.** The
    // inclusive bound means the last row of the first batch comes back a second time
    // at the head of the second; `core/ordering` collapses it by `client_msg_id`, so
    // what must be true is that the count did not grow past the number stored.
    let rows =
        cx.read(|app| bridge::try_read(app, |state| state.message_count(CHANNEL)).unwrap_or(0));
    assert_eq!(
        rows, PLANTED_MESSAGES,
        "every missed message is present exactly once. An inclusive server bound \
         overlaps by one row per batch, and the client's dedup on client_msg_id is \
         what turns that overlap into a comparison rather than a second row."
    );

    // **The order, which is what "no gap" is really about.** The rows are the ones
    // the server stored, in the order it accepted them: `ORDER BY
    // accepted_at_unix_ms, id` is a total order precisely because `mint_message_id`
    // packs the clock into the id's high bits.
    let ordered = cx.read(|app| {
        bridge::try_read(app, |state| {
            state
                .messages(CHANNEL)
                .iter()
                .map(|held| held.content.clone())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
    });
    let expected: Vec<String> = (0..PLANTED_MESSAGES)
        .map(|index| format!("missed message {index}"))
        .collect();
    assert_eq!(
        ordered, expected,
        "the catch-up arrives in acceptance order, so a transcript rebuilt from it \
         reads the way it was written. A gap would show here as a missing entry and a \
         mis-ordering as a permuted one."
    );

    // **And the loop stopped.** One ask plus one follow-up is the whole of a
    // two-batch catch-up; a third request would mean the second batch came back full,
    // which it did not, or that the cursor failed to advance and the first batch was
    // being fetched again.
    let stats = transport.stats();
    assert_eq!(
        stats.resyncs_sent, 2,
        "exactly two requests: the ask, plus one because the first batch came back \
         full. A short second batch is caught up, and this is the assertion that says \
         so -- it is what a client looping on the inclusive boundary row would break. \
         {stats}"
    );
    assert_eq!(
        stats.events_refused, 0,
        "the inbox never filled: a refused delivery is one this client does not hold, \
         and the cursor deliberately does not move past it. {stats}"
    );

    transport.shutdown();
    drop(server);
}

/// Messages sent while this client was down are present after the resync, once each.
///
/// **§8.1's Reconnect Flow, end to end, and the assertion is deliberately about the
/// outcome rather than about the route the messages took.** A reconnect, a second
/// client sending during the gap, and then a client that holds every message exactly
/// once. Whether a given message arrived live or through the catch-up depends on how
/// the reconnect and the send interleave, and asserting the route would make this a
/// test of scheduling; asserting the outcome is what the flow is for.
///
/// **The mechanism is asserted separately and unambiguously** — by
/// [`a_catch_up_recovers_every_missed_message_with_no_gap_and_no_duplicate_row`],
/// where the transport is not connected while the messages are written, and by the
/// `resyncs_sent` counter climbing across this test's reconnect. Between them the two
/// cover "the resync goes out on a reconnect" and "everything the client missed
/// arrives once".
#[gpui::test]
fn messages_sent_while_this_client_was_down_are_present_after_the_resync(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let url = format!("ws://127.0.0.1:{port}{WS_PATH}");
    let server = ServerProcess::start_on(port, "resync-after-reconnect");
    let token = server.token();

    let sender = install(cx, ME);
    let transport = WsTransport::start(TransportConfig::new(url).with_token(token.clone()), sender)
        .expect("a thread");
    await_connected(cx);

    // One message, acknowledged, so this client holds the channel and therefore has a
    // cursor for it. **The first connect asks for nothing**, because a client that
    // has held nothing has no cursor and `epoch_cursor` is the caller's choice rather
    // than a default this layer applies — which is why the assertions below are about
    // the *second* connect.
    let own = Uuid::new_v4();
    cx.update(|cx| {
        bridge::try_begin_send(cx, CHANNEL, BODY, own, at(0)).expect("the state is installed");
    });
    transport
        .send_message(own, CHANNEL, BODY)
        .expect("the send is queued");
    await_server_id(cx, &own);
    assert_eq!(
        transport.stats().resyncs_sent,
        0,
        "the first connect had no cursor to resume from, so it asked for nothing"
    );
    // **Drop the socket from the other end.** 70 KiB is above the server's 64 KiB
    // ceiling, so it terminates the connection without a close frame — the same
    // provocation, and the same reason, as the reconnect suite above.
    let oversized = "x".repeat(70 * 1024);
    transport
        .send_message(Uuid::new_v4(), CHANNEL, &oversized)
        .expect("the send is queued");

    // While this client is reconnecting, somebody else talks in the channel.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    runtime.block_on(async {
        use futures_util::{SinkExt, StreamExt};
        let request = server.handshake_request(Some(&token));
        let (mut other, _) = connect_async(request).await.expect("a websocket upgrade");
        for index in 0..DURING_THE_GAP {
            let envelope = ClientEnvelope::new(
                Uuid::new_v4().hyphenated().to_string(),
                ClientFrame::MessageSend {
                    channel_id: CHANNEL.to_owned(),
                    content: format!("during the gap {index}"),
                },
            );
            other
                .send(Message::Text(envelope.encode().expect("encodes").into()))
                .await
                .expect("the gap send to reach the server");
            loop {
                match other.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let decoded = sh_nexus::network::ws::decode_server_frame(&text)
                            .expect("the ack is a frame this build speaks");
                        if matches!(decoded, ServerFrame::MessageAck { .. }) {
                            break;
                        }
                    }
                    Some(Ok(_)) => continue,
                    other => panic!("expected an ack, got {other:?}"),
                }
            }
        }
    });

    let total = 1 + DURING_THE_GAP;
    support::wait_until(
        "the socket to be replaced and the gap filled",
        READY_BUDGET,
        || {
            drain(cx);
            transport.stats().connects >= 2 && {
                cx.read(|app| {
                    bridge::try_read(app, |state| state.message_count(CHANNEL) >= total)
                        .unwrap_or(false)
                })
            }
        },
    );

    // **Exactly one request, and it went out because of the reconnect.** The cursor
    // this client holds names its own acknowledged message, so the catch-up returns
    // that message again and nothing newer — a short batch against a bound of 256, so
    // the loop stops. A second request would mean the first batch came back full, or
    // that the cursor had failed to advance and the batch was being re-fetched.
    let stats = transport.stats();
    assert_eq!(
        stats.resyncs_sent, 1,
        "one reconnect, one remembered cursor, one request: that is §7.4's \"resume \
         from the last known last_message_at\" rather than \"rely on live delivery \
         across a gap\". Asking more than once here would mean the batch looped. \
         {stats}"
    );

    // **Exactly once each, and that is the whole of "no duplicates".** A reconnect
    // that replayed a channel the client had already caught up with would put the
    // same message in the transcript twice; the inclusive bound guarantees one
    // overlapping row per batch, and this is where the overlap has to disappear.
    let contents = cx.read(|app| {
        bridge::try_read(app, |state| {
            state
                .messages(CHANNEL)
                .iter()
                .map(|held| held.content.clone())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
    });
    let mut unique = contents.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        contents.len(),
        unique.len(),
        "no message appears twice across the reconnect: {contents:?}"
    );
    assert_eq!(
        contents.len(),
        total,
        "and no message is missing: this client holds its own message plus the {DURING_THE_GAP} sent while it was down"
    );
    for index in 0..DURING_THE_GAP {
        let expected = format!("during the gap {index}");
        assert!(
            contents.contains(&expected),
            "the message sent while this client was down is present: {expected}"
        );
    }

    transport.shutdown();
    drop(server);
}
