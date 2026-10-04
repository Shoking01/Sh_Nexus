//! The shell's connection wiring: what the environment says, and what the user gets.
//!
//! `AGENTS.md` §8.1's Real-time Flow needs two clients and a server, and until the
//! Shell started a transport there was no way to run it from the application at all —
//! only from `ws_transport.rs`, which builds the transport itself. **That gap is what
//! this file closes**: the wiring is real, so a socket can now belong to a window.
//!
//! # What is deliberately not here
//!
//! There is **no keychain** (`platform/` does not exist) and no login screen, so the
//! token arrives in an environment variable and lives only in memory. That is not a
//! shortcut around `AGENTS.md` §7.1's prohibition on plaintext token storage — an
//! environment variable is the one place a credential can be that is not a file this
//! project wrote. **The moment `platform/` lands this changes**, and the test below
//! pins the property that must survive the change: the token never reaches a `Debug`
//! output, which is the surface a log line or an error message would travel through.
//!
//! # Why nothing here touches the process environment
//!
//! **`std::env::set_var` appears in no test in this file.** It is `unsafe` under Rust
//! 2024, it mutates state that every other test in the process reads, and a test that
//! sets an environment variable to prove something about a constructor is a test that
//! breaks unrelated ones. `from_env` is therefore exercised only for the **absent**
//! case, where the process genuinely has neither variable, and the populated case is
//! reached through the constructor the shell itself uses.
//!
//! # And why the two shell tests read the state rather than a flag
//!
//! **`Shell::is_connected` used to stand here and has been removed.** It returned
//! `transport.is_some()` — *a worker thread was started* — while its name and its doc
//! comment promised *the peer answered*, and the test below asserted `true` against
//! `ws://127.0.0.1:1/ws`, a port nothing listens on. **The lie was written into the
//! suite**, which is the part worth recording: a wrong accessor is one defect, and a
//! test that pins the wrong answer is the one that keeps it.
//!
//! [`AppState::can_send`](sh_nexus::core::models::ConnectionState) is the one honest
//! answer, `bridge::try_read` already exposes it, and these tests ask it through the
//! seam. The accessor that survives is [`Shell::transport`], which is honest about the
//! one thing the field knows.
//!
//! # What section 4 adds, and why one of its tests needs a server
//!
//! Everything above runs without spawning a server process, which is why this file
//! does not pull in `support`. **The outbox tests are the exception, and the
//! exception is one test.** `PLAN.md` §7's second bullet is a claim about a real
//! socket completing a real handshake and answering, and a dead port cannot make
//! that claim — so `queued_sends_reach_the_server_when_the_connection_returns`
//! brings the fixture in and says why. Its two neighbours stay server-free on
//! purpose: the dead-port cases are the ones that assert the *negative* half of the
//! design (a frame that reached the socket is not an acknowledgement), and a live
//! server could not produce that condition at all.

use std::time::{Duration, Instant};

mod support;

use chrono::{TimeZone, Utc};
use gpui::{Entity, TestAppContext, VisualTestContext};

use sh_nexus::app::{ConnectionSettings, MissingSetting, Shell};
use sh_nexus::core::models::events::DomainEvent;
use sh_nexus::core::models::ConnectionState;
use sh_nexus::network::ws::{TransportConfig, WsTransport};
use sh_nexus::state::bridge::{self, EventSender, FlushReport};
use sh_nexus::state::SendOutcome;
use sh_nexus::UNSIGNED_IN_USER;

/// A token nobody will guess, so finding it in output is unambiguous rather than
/// plausible.
const A_TOKEN: &str = "tok_live_2f8c1d9e4b7a0356_secret_do_not_log_me";
const A_URL: &str = "ws://127.0.0.1:8484/ws";

/// How long the file below waits for the worker thread's first publication.
///
/// **Five seconds against a socket that is refused immediately, and the number is a
/// ceiling rather than a wait.** The condition being polled is *the worker has
/// published a connection attempt*, which it does as the first thing its loop does
/// (`network/ws.rs`, `Session::run` emits before it connects); the poll returns the
/// instant that holds and only spends the budget when the worker never publishes at
/// all. The 5 s of `AGENTS.md` §7.4's connect timeout is the right order of magnitude
/// for "a machine so loaded the thread did not get scheduled", and nothing here waits
/// for it on the happy path.
const PUBLISH_BUDGET: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 1. Reading the environment
// ---------------------------------------------------------------------------

#[test]
fn both_variables_absent_is_the_offline_shell_and_not_an_error() {
    // This is the developer's first run with no server configured, and it is the
    // property that makes the client usable at all: a client that refuses to start
    // without a server is a client nobody can run the first time. `cargo test` runs in
    // an environment where neither variable is set, which is why this is a plain
    // `#[test]` rather than one needing a GPUI context.
    assert_eq!(
        ConnectionSettings::from_env().expect("absent configuration is not an error"),
        None,
        "no URL and no token is the documented offline case, not a failure"
    );
}

#[test]
fn a_missing_setting_names_the_variable_it_is_missing() {
    // The whole point of the type over a `String`. An operator who sees the message
    // can fix it without reading this source, which `AGENTS.md` §7.5 asks for: an
    // actionable code beats a silently dropped explanation.
    let rendered = MissingSetting::Token.to_string();
    assert!(
        rendered.contains("SH_NEXUS_TOKEN"),
        "the message must name the variable, and it says {rendered:?}"
    );

    // Each variant names the variable that is *missing* first, which is the one the
    // operator has to go and set. The message may also mention the other — saying
    // "SH_NEXUS_TOKEN is set but SH_NEXUS_URL is not" is more use than naming only the
    // gap — so what is asserted is the leading name, not the absence of the other.
    let other = MissingSetting::Url.to_string();
    assert!(
        other.starts_with("SH_NEXUS_TOKEN is set but SH_NEXUS_URL is not"),
        "the message leads with the set variable and names the missing one: {other:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. The credential never escapes
// ---------------------------------------------------------------------------

#[test]
fn the_rendered_debug_of_the_settings_omits_the_token() {
    // `#[derive(Debug)]` was the first version of this struct, and it printed the
    // token. This test is what makes the omission a property rather than an accident
    // a future edit can reintroduce.
    let settings = ConnectionSettings::from_parts_for_test(A_URL, A_TOKEN);
    let rendered = format!("{settings:?}");

    assert!(
        !rendered.contains(A_TOKEN),
        "the rendered Debug leaked the token: {rendered}"
    );
    assert!(
        !rendered.contains("tok_live"),
        "no part of the token may appear, not even a prefix: {rendered}"
    );
}

#[test]
fn the_token_presence_is_still_reported_because_it_is_diagnosable() {
    // Hiding the token is not the same as hiding that there is one: "a URL is
    // configured with no token" is exactly the condition an operator needs to see, and
    // one boolean is enough to see it.
    let settings = ConnectionSettings::from_parts_for_test(A_URL, A_TOKEN);
    let rendered = format!("{settings:?}");

    assert!(
        rendered.contains("token_configured"),
        "the presence is what makes a misconfiguration visible: {rendered}"
    );
    assert!(
        rendered.contains(A_URL),
        "the URL is not a secret and is what identifies which server: {rendered}"
    );
}

#[test]
fn the_transport_config_is_another_debug_surface_the_token_cannot_reach() {
    // `TransportConfig` has its own hand-written `Debug`, so the credential is
    // already protected one hop away. This asserts the seam holds, because the two
    // structs are the only two places a token exists in the client.
    let settings = ConnectionSettings::from_parts_for_test(A_URL, A_TOKEN);
    let rendered = format!("{:?}", settings.transport_config());

    assert!(
        !rendered.contains(A_TOKEN),
        "the token reached the transport config's Debug: {rendered}"
    );
}

#[test]
fn an_empty_token_is_reported_as_unconfigured_rather_than_present() {
    // The `Debug` prints a boolean, and "has a token" must not be true for the empty
    // string — a shell configured with `SH_NEXUS_TOKEN=` would otherwise look
    // configured while presenting a blank credential to the server.
    let settings = ConnectionSettings::from_parts_for_test(A_URL, "");
    let rendered = format!("{settings:?}");

    assert!(
        rendered.contains("token_configured: false"),
        "an empty token is not a configured token: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// 3. The shell, offline and connected
// ---------------------------------------------------------------------------

/// An offline shell holds no socket, **and the state says the same thing**.
///
/// **Two questions, and the accessor change made them separable.** `Shell::transport`
/// answers *was a connection attempted*; the state's `connection` answers *what
/// happened*. The removed `is_connected` answered only the first and was named after
/// the second, so this test now reads both and can fail if they ever disagree — which
/// is the failure the removed accessor existed to hide.
#[gpui::test]
fn a_shell_built_without_starting_a_transport_reports_itself_offline(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    cx.run_until_parked();

    shell.update_in(cx, |shell, _window, _cx| {
        assert!(
            shell.transport().is_none(),
            "a shell built without start_transport holds no socket, and the field \
             says so -- `Shell::new` never reads the environment, which is what keeps \
             the constructor testable"
        );
    });

    // **Through the seam, because that is the only way a file outside `state/` may
    // read the state** -- `tests/bridge.rs` fails the build on naming the type. The
    // closure's parameter is inferred, so this names no state type either.
    let answered = shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| (state.can_send(), state.connection().clone()))
    });

    assert_eq!(
        answered,
        Some((false, ConnectionState::Disconnected)),
        "the offline shell's own answer is 'not connected': `AppState::new` starts \
         at `Disconnected` and `can_send` is false there. Read through the seam \
         rather than through a field of the shell, because the seam is where the \
         truth is maintained"
    );
}

/// A started transport publishes an **attempt**, and never a connection it did not
/// make.
///
/// **The test that used to assert the bug.** It ran
/// `assert!(shell.is_connected())` immediately after pointing a transport at
/// `ws://127.0.0.1:1/ws` — a port nothing listens on — so it asserted "connected"
/// for a client that had not connected to anything. The accessor was removed rather
/// than corrected, and this is the assertion that replaced it: **the same gesture,
/// asked about the same dead port, must now report that nothing was connected.**
///
/// **Why a single read would be a race, and this is the whole reason for the poll.**
/// The answer is produced by a *different thread*. `WsTransport::start` spawns a
/// worker and returns immediately; that worker builds a runtime and then, as the first
/// statement of its loop, publishes either `Connecting` or `Reconnecting { attempt }`
/// (`network/ws.rs`, `Session::run`). So at the instant `start_transport` returns the
/// state is still `Disconnected` **or** it is already an attempt, depending on
/// scheduling, and the test cannot know which. `AGENTS.md` §4.3 forbids standing in
/// for that with a `sleep()`: the honest shape is a bounded poll on the *condition*,
/// which returns the instant the condition holds and reports honestly when it never
/// does. `tests/support/mod.rs::wait_until` is that helper; it lives in a module that
/// pulls in the server-binary fixture, so the poll is written out here rather than
/// pulling `support` into a file that spawns no server.
///
/// **The assertion is two-sided on purpose.** "Not `Connected`" alone would be
/// satisfied by a client that published nothing at all, which is the failure mode of a
/// transport whose worker died before its first event — so the test also requires that
/// *something* was published, and that it is an attempt. Together they say "the socket
/// was tried and the try did not succeed", which is what a dead port is.
#[gpui::test]
fn a_started_transport_publishes_an_attempt_and_never_a_connection(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);
    let settings = ConnectionSettings::from_parts_for_test("ws://127.0.0.1:1/ws", A_TOKEN);

    shell
        .update(cx, |shell, cx| shell.start_transport(&settings, cx))
        .expect("starting a worker thread is not a network operation");

    shell.update_in(cx, |shell, _window, _cx| {
        assert!(
            shell.transport().is_some(),
            "a shell that started a transport holds one, and holding it for the \
             shell's life is what keeps the socket from being torn down silently"
        );
    });

    let published = poll_for_a_published_attempt(cx);

    assert!(
        !matches!(published, ConnectionState::Connected),
        "nothing is listening on this port, so `Connected` is a lie about the \
         network. `AppState::can_send` gates the composer's send on this value, so \
         asserting it wrongly here would assert that a client with no server may \
         send -- which is the exact defect the removed `is_connected` hid"
    );
    assert!(
        !matches!(published, ConnectionState::Disconnected),
        "the worker must have published something: it emits `Connecting` or \
         `Reconnecting {{ attempt }}` as the first statement of its loop, before it \
         reaches the socket. `Disconnected` here would mean the drain path from \
         `network/` to the state is broken, which no amount of `transport().is_some()` \
         would have detected"
    );
    assert!(
        matches!(
            published,
            ConnectionState::Connecting | ConnectionState::Reconnecting { .. }
        ),
        "and what it published must be an attempt, not a refusal: a dead port is a \
         connect failure, which `network/reconnect.rs` backs off from rather than \
         treating as terminal. Got {published:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. The outbox, at the seam
// ---------------------------------------------------------------------------

/// A timestamp a caller supplies, since `state/` may not read a clock.
///
/// The same helper `tests/state_actions.rs` and `tests/ws_transport.rs` each carry
/// their own copy of, for the reason every test file in this project has its own
/// scanner: a test binary cannot import another test binary's helpers.
fn at(second: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_789_000_000 + second, 0)
        .single()
        .expect("a fixed base plus a small offset is in range")
}

/// The one channel `sh_nexus_server` seeds, and the channel the offline client
/// composes into.
const CHANNEL: &str = "c_general";

/// **A frame reaching the socket is not an acknowledgement, and this is the test
/// that says so through the real seam.**
///
/// The defect `PLAN.md` §7's outbox exists to remove is in `network/ws.rs`: its
/// worker takes an item off the outbound queue and **returns without putting it
/// back** when `write_frame` fails. So a transport queue is backpressure between
/// the enqueue and the write, and any design that retires a send when a write
/// succeeds retires it on evidence that the bytes left the process.
///
/// **A dead port is what makes this observable without a server.** Nothing
/// listens on `127.0.0.1:1`, so no `message.ack` can arrive — and yet
/// `WsTransport::send_message` still succeeds, because it is a `try_send` into the
/// worker's outbound queue and the worker has not yet discovered the port is
/// closed. **That is precisely the shape of the bug**: a successful push, and a
/// send nobody is waiting on any more. So if the outbox emptied here, the design
/// would be retiring sends on a write; it must not, and must drive all of them
/// again on the next flush.
#[gpui::test]
fn a_frame_that_reached_the_socket_is_not_an_acknowledgement(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let transport = WsTransport::start(
        TransportConfig::new("ws://127.0.0.1:1/ws").with_token(A_TOKEN),
        sender,
    )
    .expect("starting a worker thread is not a network operation");
    cx.update(|cx| bridge::install_transport(cx, Some(transport)));

    // Three sends, composed while the connection is `Disconnected`.
    let ids: Vec<uuid::Uuid> = (0..3u128)
        .map(|index| {
            let identity = uuid::Uuid::from_u128(index + 1);
            let outcome = cx.update(|cx| {
                bridge::try_begin_send(cx, CHANNEL, "written offline", identity, at(index as i64))
                    .expect("the state is installed")
            });
            assert!(
                matches!(outcome, SendOutcome::Pending { offline: true, .. }),
                "send {index} was composed offline and must say so, got {outcome:?}"
            );
            identity
        })
        .collect();
    assert_eq!(outbox_len(cx), 3, "all three are queued");

    // **A transport, and a connection the state has not been told about.** The
    // state says it cannot carry a send, so the flush must decline — and must
    // decline *without touching the queue*.
    let held_nothing = cx
        .update(bridge::try_flush_outbox)
        .expect("the state is installed");
    assert_eq!(held_nothing.driven(), 0, "nothing could be driven");
    assert_eq!(outbox_len(cx), 3, "and the queue is untouched");

    // The server the port never had, arrives.
    cx.update(|cx| {
        bridge::try_apply_event(
            cx,
            DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
        )
    });

    let first = cx
        .update(bridge::try_flush_outbox)
        .expect("the state is installed");
    assert_eq!(
        first.driven(),
        3,
        "every queued send was put on the wire -- this is PLAN.md section 7's flush"
    );
    assert_eq!(first.refused(), 0, "the outbound queue was not full");
    assert_eq!(
        outbox_len(cx),
        3,
        "and NOT ONE of them left the outbox: a frame in the transport's queue is \
         not the server storing the row, and no ack can arrive on a dead port"
    );

    // **And again.** This is the assertion that makes the previous one mean
    // something: a queue that emptied on the first drive would return nothing here,
    // and the send would be gone for good — which is the whole defect.
    let second = cx
        .update(bridge::try_flush_outbox)
        .expect("the state is installed");
    assert_eq!(
        second.driven(),
        3,
        "the same three are driven again: re-driving is what makes a frame lost to \
         a dying socket recoverable, and the server's dedup on client_msg_id is \
         what makes it safe (PLAN.md section 7, third bullet)"
    );
    assert_eq!(outbox_len(cx), 3);

    // **Only the server's two answers retire anything.** Nothing can arrive on a
    // dead port, so the queue is fed two events directly and the second one is the
    // only thing that changes it.
    cx.update(|cx| {
        bridge::try_apply_event(
            cx,
            DomainEvent::MessageSendFailed {
                client_msg_id: ids[1],
                code: "server.refused".to_owned(),
                detail: String::new(),
            },
        )
    });
    assert_eq!(
        outbox_len(cx),
        2,
        "a terminal failure retired exactly one entry"
    );

    // **The order, at the seam, is the order `actions::flush_outbox` produced.**
    // `tests/state_actions.rs::a_reconnect_drives_every_queued_send_in_enqueue_order`
    // is where the ordering claim is decided and checked; what is asserted here is
    // that the seam drives them all and retires none, because the seam's loop is a
    // `for` over the resolved list and there is no second place order could change.
    assert!(
        cx.update(bridge::try_flush_outbox)
            .expect("the state is installed")
            .driven()
            == 2,
        "the remaining two are driven, and the refused one is not re-driven"
    );
}

/// **A flush with no socket reports what is waiting, and changes nothing.**
///
/// The offline shell — no `SH_NEXUS_URL`, so `OutboundGlobal.transport` is `None`
/// forever — is the ordinary case for this arm, and it is the one where a flush
/// that silently reported "nothing to do" would be indistinguishable from an empty
/// queue. `AGENTS.md` §7.1 forbids a bound whose breaches are invisible, and a
/// queue that is waiting rather than empty is the fact an operator needs.
#[gpui::test]
fn a_flush_without_a_socket_reports_what_is_waiting(cx: &mut TestAppContext) {
    let sender = installed(cx);
    let _transport = WsTransport::start(
        TransportConfig::new("ws://127.0.0.1:1/ws").with_token(A_TOKEN),
        sender,
    )
    .expect("a worker thread");

    let identity = uuid::Uuid::from_u128(1);
    cx.update(|cx| {
        let outcome = bridge::try_begin_send(cx, CHANNEL, "written offline", identity, at(0))
            .expect("the state is installed");
        assert!(matches!(
            outcome,
            SendOutcome::Pending { offline: true, .. }
        ));
    });

    // **No transport installed at all**, which is what an offline shell is.
    assert!(
        !cx.read(bridge::has_transport),
        "this application has a worker but never published it, so the seam has no \
         socket -- the documented offline shell"
    );

    // The connection is up as far as the state is concerned, so the flush is not
    // declined by the gate; it is the missing socket that stops it.
    cx.update(|cx| {
        bridge::try_apply_event(
            cx,
            DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
        )
    });
    let report = cx
        .update(bridge::try_flush_outbox)
        .expect("the state is installed");

    assert_eq!(report.driven(), 0, "nothing was driven");
    assert_eq!(
        report.refused(),
        0,
        "and nothing was refused -- there was no socket"
    );
    assert_eq!(
        report.held(),
        1,
        "and the report says the entry is WAITING, which is not the same answer as \
         an empty queue"
    );
    assert!(
        !report.is_empty(),
        "so a caller can tell this from nothing to do"
    );
    assert_eq!(
        outbox_len(cx),
        1,
        "and the entry is still queued for the next socket"
    );
}

/// **Reconnecting really does deliver the queued sends, end to end, in order.**
///
/// **This is the one test in the file that needs a server, and it pulls in
/// `support` for exactly that** — the module the rest of this file avoids, because
/// `support` carries the server-binary fixture and every other test here spawns no
/// server. `PLAN.md` §7's second bullet is a claim about a real socket answering a
/// real handshake, and a mock transport cannot make it.
///
/// **The sequence is the reconnect, and it is built the only way that can hold a
/// valid credential across the gap:** a server is started on a chosen port to mint
/// a token, then *stopped*; the transport is started against that now-empty port;
/// three sends are composed while the client is failing to connect; the server
/// comes back on the same port against the same database, so the token the
/// transport already holds is still the right one. That is
/// `tests/ws_transport.rs::a_dropped_connection_is_retried_on_the_backoff_schedule_and_recovers`
/// verbatim, and the reason it is worth copying rather than inventing.
///
/// **The order is asserted through the client's own state, and the assertion is
/// weak on purpose.** The server stamps each stored row with its own acceptance
/// time, so three sends flushed in enqueue order must come back with
/// non-decreasing timestamps *in enqueue order*. Equal timestamps satisfy it — a
/// fast loopback can land three accepts inside one clock tick — so this cannot
/// prove the server preserved the order. **What it does catch is a flush that
/// drove the queue in the wrong order**, because a reversed flush produces a
/// reversed sequence here and nothing else would. The strong form of the claim
/// needs the server's transcript over REST, which this crate has no client for, so
/// the order is asserted where it is decided instead — see
/// `tests/state_actions.rs::a_reconnect_drives_every_queued_send_in_enqueue_order`.
#[gpui::test]
fn queued_sends_reach_the_server_when_the_connection_returns(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    // The path is bound to a local first: `format!` would read `support::WS_PATH`
    // as a field name rather than a value, and `WS_PATH` is duplicated from the
    // server's own constant on purpose -- see that constant's documentation.
    let socket_path = support::WS_PATH;
    let url = format!("ws://127.0.0.1:{port}{socket_path}");

    // Mint a credential, then take the server away. A token is bound to one
    // database file, and a database is only readable through a server.
    let mut server = support::ServerProcess::start_on(port, "outbox-reconnect");
    let token = server.token();
    server.stop();

    let sender = installed(cx);
    let transport = WsTransport::start(TransportConfig::new(url).with_token(token), sender)
        .expect("starting a worker thread is not a network operation");
    cx.update(|cx| bridge::install_transport(cx, Some(transport.clone())));

    // **Wait for a failed attempt, so the state has left `Disconnected`** and the
    // sends below are composed by a client that genuinely cannot reach a server.
    // Polling the condition rather than sleeping is `AGENTS.md` §4.3's distinction.
    support::wait_until("the first connect to fail", support::READY_BUDGET, || {
        cx.update(|cx| {
            let _ = bridge::drain(cx);
        });
        transport.stats().connect_failures >= 1
    });
    assert!(
        !cx.read(|app| bridge::try_read(app, |state| state.can_send()).unwrap_or(false)),
        "the client cannot send while nothing is listening, so these three sends \
         take the offline path -- which is the path the outbox exists for"
    );

    let ids: Vec<uuid::Uuid> = (0..3u128)
        .map(|index| {
            let identity = uuid::Uuid::from_u128(index + 1);
            cx.update(|cx| {
                let outcome = bridge::try_begin_send(
                    cx,
                    CHANNEL,
                    &format!("queued while offline {index}"),
                    identity,
                    at(index as i64),
                )
                .expect("the state is installed");
                assert!(
                    matches!(outcome, SendOutcome::Pending { offline: true, .. }),
                    "send {index} must be queued, got {outcome:?}"
                );
            });
            identity
        })
        .collect();
    assert_eq!(outbox_len(cx), 3, "three sends, three entries");

    // The server returns, on the same port and the same database.
    server.restart();
    poll_for("the client to report a connection", || {
        cx.update(|cx| {
            let _ = bridge::drain(cx);
        });
        cx.read(|app| bridge::try_read(app, |state| state.connection().clone()))
            == Some(ConnectionState::Connected)
    });

    // **The flush, and then the server's answers.** Every entry is driven, and the
    // queue only empties as the acknowledgements land — which is the design: the
    // exit is the server's, not the writer's.
    let report: FlushReport = cx
        .update(bridge::try_flush_outbox)
        .expect("the state is installed");
    assert_eq!(report.driven(), 3, "all three were put on the wire");

    // **The poll waits for the server's evidence, not for our own bookkeeping.**
    // `outbox_len == 0` would be the wrong condition: it is satisfied both by the
    // acknowledgements landing and by anything that retired the entries without a
    // server round trip — and an implementation doing the latter would satisfy it
    // instantly and let this test read the rows before the ack had arrived. The
    // rows carrying the **server's own ids** is the condition only the server can
    // bring about.
    poll_for("all three sends to be acknowledged", || {
        cx.update(|cx| {
            let _ = bridge::drain(cx);
        });
        cx.read(|app| {
            bridge::try_read(app, |state| {
                ids.iter().all(|identity| {
                    state
                        .message(CHANNEL, identity)
                        .is_some_and(|held| !held.id.is_empty())
                })
            })
            .unwrap_or(false)
        })
    });

    assert_eq!(
        outbox_len(cx),
        0,
        "and the queue is empty because the server answered every entry, not \
         because anything retired them locally"
    );

    let stamps: Vec<chrono::DateTime<Utc>> = cx.read(|app| {
        bridge::try_read(app, |state| {
            ids.iter()
                .filter_map(|identity| state.message(CHANNEL, identity).map(|held| held.timestamp))
                .collect()
        })
        .expect("the state is installed")
    });
    assert_eq!(
        stamps.len(),
        3,
        "every queued send is held as an acknowledged row with the server's own \
         timestamp -- none was lost, and none was duplicated"
    );
    assert!(
        stamps.windows(2).all(|pair| pair[0] <= pair[1]),
        "the three acceptance times come back in enqueue order, which a flush that \
         drove the queue backwards would invert: {stamps:?}"
    );

    // And each row holds a real server id, so the Optimistic Send Flow completed
    // rather than the row merely sitting there looking finished.
    let acked = cx.read(|app| {
        bridge::try_read(app, |state| {
            ids.iter()
                .filter(|identity| {
                    state
                        .message(CHANNEL, identity)
                        .is_some_and(|held| !held.id.is_empty())
                })
                .count()
        })
        .expect("the state is installed")
    });
    assert_eq!(
        acked, 3,
        "all three were reconciled against the server's rows"
    );

    transport.shutdown();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// How many sends the outbox is holding, read through the seam.
fn outbox_len(cx: &TestAppContext) -> usize {
    cx.read(|app| {
        bridge::try_read(app, |state| state.outbox_len()).expect("the state is installed")
    })
}

/// Polls `condition` on the main thread until it holds or the budget runs out.
///
/// **The same bounded poll `poll_for_a_published_attempt` below uses**, written
/// separately because that one is about a connection attempt and this one is about
/// the outbox. `AGENTS.md` §4.3 forbids a `sleep()` standing in for logic that
/// should be waited on; every wait in this file is a condition poll that returns
/// the instant its condition holds.
///
/// **`bridge::drain` on every pass, not once at the end**, and that is the
/// load-bearing part: the state only changes when something applies an event, and
/// `bridge::drain` is the only thing that applies one.
fn poll_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PUBLISH_BUDGET;
    loop {
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "`{what}` did not happen within {PUBLISH_BUDGET:?}"
        );
        std::thread::yield_now();
    }
}

/// Installs the state and builds the shell in a real headless window.
///
/// **The two calls are the production order**, and that is the point: `app::open`
/// installs before a window exists, so a shell built against an absent global would
/// render an empty list and satisfy assertions for the wrong reason.
fn shell(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let sender = installed(cx);
    cx.add_window_view(move |_, cx| Shell::new(sender, cx))
}

fn installed(cx: &mut TestAppContext) -> EventSender {
    cx.update(|cx| bridge::install(cx, UNSIGNED_IN_USER))
        .expect("the first install succeeds")
}

/// Drains the inbox until the worker publishes something other than `Disconnected`,
/// or the budget runs out.
///
/// **The poll is on the condition, never on a duration, and that is `AGENTS.md` §4.3's
/// distinction.** A `sleep()` standing in for logic that should be waited on is what
/// that rule forbids; a bounded condition poll is the sanctioned replacement, because
/// it returns the instant the condition holds and names what it was waiting for when
/// it never does. `tests/support/mod.rs::wait_until` is that helper verbatim and is
/// not used here only because `support` also carries the server-binary fixture, which
/// would make this file's `cargo test -p sh_nexus` run fail for a reason that has
/// nothing to do with the connection.
///
/// **`bridge::drain` is called on every pass rather than once at the end**, and that
/// is the load-bearing part: the state only changes when something applies an event,
/// and `bridge::drain` is the only thing that applies one. A loop that only read the
/// state would poll a value nothing was writing and time out against correct code.
///
/// **`yield_now` and not a sleep, so the worker thread gets the core.** The window is
/// closed on every pass, so this cannot livelock against the process; the yield is
/// what makes the loop a *fair* poll rather than a spin that starves the very thread
/// it is waiting for.
fn poll_for_a_published_attempt(cx: &mut VisualTestContext) -> ConnectionState {
    let deadline = Instant::now() + PUBLISH_BUDGET;
    loop {
        let state = cx.update(|_window, app| {
            bridge::drain(app);
            bridge::try_read(app, |state| state.connection().clone())
        });

        match state {
            Some(current) if current != ConnectionState::Disconnected => return current,
            // `None` means the global is gone, which `installed` above rules out by
            // construction -- so this is a state no fixture can reach, and it is
            // reported rather than retried.
            None => panic!(
                "the application state was installed a moment ago and must still be: \
                 `bridge::drain` and `bridge::try_read` both read the same global"
            ),
            Some(ConnectionState::Disconnected) => {}
            // Unreachable: the guarded arm above returns for every variant that is
            // not `Disconnected`. Listed so a new `ConnectionState` variant cannot
            // make this loop silently fall through -- which is the failure mode of
            // leaving it to the compiler's exhaustiveness check, since a guard does
            // not count towards it.
            Some(_) => {}
        }

        assert!(
            Instant::now() < deadline,
            "the transport's worker published no connection state within {PUBLISH_BUDGET:?}. \
             `WsTransport::start` returns as soon as the thread is spawned, so the \
             event is produced on another thread; if it never arrives, the path from \
             `network/` through `EventSender` into `actions::apply_event` is broken, \
             and no assertion about `Shell::transport` would have shown it"
        );
        std::thread::yield_now();
    }
}
