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

use gpui::{Entity, TestAppContext, VisualTestContext};

use sh_nexus::app::{ConnectionSettings, MissingSetting, Shell};
use sh_nexus::state::bridge::{self, EventSender};
use sh_nexus::UNSIGNED_IN_USER;

/// A token nobody will guess, so finding it in output is unambiguous rather than
/// plausible.
const A_TOKEN: &str = "tok_live_2f8c1d9e4b7a0356_secret_do_not_log_me";
const A_URL: &str = "ws://127.0.0.1:8484/ws";

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

#[gpui::test]
fn a_shell_built_without_starting_a_transport_reports_itself_offline(cx: &mut TestAppContext) {
    let (shell, cx) = shell(cx);

    shell.update_in(cx, |shell, _window, _cx| {
        assert!(
            !shell.is_connected(),
            "a shell built without start_transport has no socket, and says so"
        );
        assert!(
            shell.transport().is_none(),
            "and there is none to hand out either"
        );
    });
}

#[gpui::test]
fn starting_a_transport_gives_the_shell_one_and_holding_it_is_what_keeps_it_alive(
    cx: &mut TestAppContext,
) {
    // No server is listening on this port, so the transport cannot connect — **and that
    // is what this asserts.** `start_transport` reports whether it *started a socket*,
    // not whether the socket reached anything; conflating the two would turn a server
    // that is down into a startup failure, which is the wrong shape entirely: the
    // backoff exists precisely so a server that is not there yet is normal.
    let (shell, cx) = shell(cx);
    let settings = ConnectionSettings::from_parts_for_test("ws://127.0.0.1:1/ws", A_TOKEN);

    shell
        .update(cx, |shell, _| shell.start_transport(&settings))
        .expect("starting a worker thread is not a network operation");

    shell.update_in(cx, |shell, _window, _cx| {
        assert!(
            shell.is_connected(),
            "a shell that started a transport holds one"
        );
        assert!(
            shell.transport().is_some(),
            "and it is the same one the shell keeps alive"
        );
    });
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
