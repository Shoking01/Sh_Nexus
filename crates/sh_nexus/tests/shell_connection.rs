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

use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use sh_nexus::app::{ConnectionSettings, MissingSetting, Shell};
use sh_nexus::core::models::ConnectionState;
use sh_nexus::state::bridge::{self, EventSender};
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
// Helpers
// ---------------------------------------------------------------------------

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
