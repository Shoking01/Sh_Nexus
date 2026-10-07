//! A login screen, one HTTP POST, and the rule that a password never leaves the
//! window.
//!
//! # What this file is for
//!
//! `odd/tasks/6a-login.md` is the change record; the short version is that until
//! the login milestone a person could not use this application at all. The token
//! came from `SH_NEXUS_TOKEN`, so using the client meant calling the REST API by
//! hand and pasting the result into an environment variable. Everything built in
//! PRs #43-#49 — the socket, the outbox, the banner, the resync — was reachable
//! only that way.
//!
//! | Test | What it proves | Kind |
//! |---|---|---|
//! | [`a_correct_password_against_a_real_server_yields_a_token_the_socket_accepts`] | `AGENTS.md` §8.1's Login Flow, end to end, with the token opening a real handshake | real server process |
//! | [`a_correct_login_moves_the_window_from_the_form_to_the_chat`] | the mode switch, and that the token reaches the transport | real server process, real window |
//! | [`a_wrong_password_shows_the_servers_own_reason_and_keeps_no_token`] | §8.1's Auth Failure Flow, and acceptance criterion 3 | real server process |
//! | [`a_server_that_is_not_there_is_reported_rather_than_swallowed`] | an unreachable server is *reported*, not swallowed | real, absent port |
//! | [`a_refusal_reaches_the_window_as_the_servers_own_words`] | decision 3: the failure is shown, not only traced | real server process |
//! | [`an_endpoint_asking_for_tls_is_refused_rather_than_downgraded`] | the ADR-012 coupling, made loud | pure |
//! | [`the_websocket_endpoint_is_translated_into_an_http_login_url`] | the five shapes a configured URL can take | pure |
//! | [`a_success_body_without_a_token_is_not_reported_as_a_refused_credential`] | the one mapping a careless client gets wrong | pure |
//! | [`neither_variable_set_still_starts_the_offline_shell`] | acceptance criterion 4 — local mode is unchanged | runtime |
//! | [`the_connected_state_has_exactly_one_representation`] | one answer to "is this client configured" | runtime |
//! | [`the_password_reaches_no_state_or_core_source`] | acceptance criterion 5, enforced rather than asserted | source scan |
//! | [`the_one_sanctioned_login_door_takes_the_password_by_value`] | the exemption the scan relies on | source scan |
//! | [`no_production_module_prints_a_credential_in_its_debug_output`] | acceptance criterion 5, at runtime | runtime |
//! | [`no_production_source_logs_a_credential_or_prints_one`] | `AGENTS.md` §7.5, over the whole tree | source scan |
//! | [`typing_reaches_the_login_fields_through_the_platform_input_handler`] | the absence failure mode `input_bar.rs` documents | runtime, platform path |
//! | [`tab_moves_between_the_two_fields_and_enter_does_not_type_a_newline`] | the two claimed keys, and their asymmetry | runtime, platform path |
//! | [`a_blank_submit_is_refused_without_being_sent`] | the one refusal the server never got to make | runtime |
//! | [`the_login_screen_renders_its_fields_and_no_failure_line_until_one_exists`] | both halves of "the message is conditional" | runtime, real bounds |
//! | [`a_second_submit_while_one_is_outstanding_is_refused`] | one attempt, one session | runtime, negative |
//! | [`a_login_outcome_does_not_travel_through_the_event_inbox`] | the poll runs independently of the event drain | runtime, negative |
//! | [`the_login_view_asks_the_seam_and_names_no_network_module`] | acceptance criterion 6, re-asserted for the new view | source scan |
//!
//! # Why the server runs as a child process
//!
//! `tests/support/mod.rs` gives the argument in full: a `[dev-dependencies]` entry
//! naming `sh_nexus_server` would put `axum`, `rusqlite` and SQLite's C amalgamation
//! into `cargo tree -p sh_nexus`, which is the mistake ADR-002 exists to prevent.
//! Running the binary exercises the **shipped** server, which is a stronger claim
//! than a library harness would be.
//!
//! # Where the password is and is not, and why the tests below are shaped this way
//!
//! **No test here asserts against a password the server received**, and that is
//! deliberate: doing so would need the server to log it, and `AGENTS.md` §7.5
//! forbids that on the other side too. What is asserted instead is the property
//! that actually matters and *can* be checked from here: **no source file in
//! `state/` or `core/` names a password**, **no `Debug` impl in the crate prints
//! one**, and **no `tracing` call anywhere names one**. Those are the three ways
//! the rule fails in practice — a credential handed to the state layer, a credential
//! reaching a log through `{:?}`, and a credential reaching a log deliberately —
//! and all three are covered rather than left to a comment.
//!
//! **The typed password is asserted through the view's own accessor, not through a
//! painted frame.** That is not a shortcut: `LoginView` draws a bullet per
//! character and never the characters, because a password field that draws its
//! contents is a wrong password field.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use gpui::{px, Entity, Focusable, TestAppContext, VisualTestContext};
use rstest::rstest;
use sh_nexus::app::{self, ConnectionSettings, Shell, Startup, DRAIN_INTERVAL};
use sh_nexus::core::models::events::ConnectionState;
use sh_nexus::network::rest::{self, LoginError, LOGIN_PATH, MAX_RESPONSE_BYTES};
use sh_nexus::state::bridge::{self, LoginOutcome};
use sh_nexus::ui::views::login::{self as login_view, Field};
use sh_nexus::UNSIGNED_IN_USER;

use support::{ServerProcess, ADMIN_PASSWORD, ADMIN_USERNAME, READY_BUDGET, WS_PATH};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A password that is not [`ADMIN_PASSWORD`].
///
/// **A distinct value rather than a mangled one, so a failure message naming the
/// password that was sent says which half of the assertion lapsed** — the same
/// reasoning `sh_nexus_server`'s `WRONG_PASSWORD` gives. Over the twelve-character
/// minimum the server enforces, so the refusal is about the *value* and not about
/// the request's shape: a login refused for a short password would be answering a
/// different question than the one this file asks.
const WRONG_PASSWORD: &str = "a-different-passphrase";

/// Installs the application state and returns the producer handle.
fn installed(cx: &mut TestAppContext) -> bridge::EventSender {
    cx.update(|cx| bridge::install(cx, UNSIGNED_IN_USER))
        .expect("the first install must succeed")
}

/// Installs the state and builds the shell in a real headless window.
///
/// **The production order, for the reason `tests/app_shell.rs`'s fixture gives:**
/// `app::open` installs before a window exists, so a shell built against an absent
/// global would render an empty list and satisfy half the assertions here for the
/// wrong reason.
fn shell(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let sender = installed(cx);
    cx.add_window_view(move |_, cx| Shell::new(sender, cx))
}

/// Puts the shell into the login mode for `endpoint`.
fn login_shell<'a>(
    cx: &'a mut TestAppContext,
    endpoint: &str,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = shell(cx);
    assert!(
        shell.update_in(cx, |shell, _window, cx| shell.enter_login(endpoint, cx)),
        "the first `enter_login` on a fresh shell must be accepted"
    );
    cx.run_until_parked();
    (shell, cx)
}

/// Types a handle and a secret into the form, through its own setters.
fn fill(cx: &mut VisualTestContext, shell: &Entity<Shell>, username: &str, password: &str) {
    shell.update_in(cx, |shell, _window, cx| {
        let form = shell.login_form().expect("a shell in the login mode");
        form.update(cx, |form, cx| {
            form.set_username(username, cx);
            form.set_password(password, cx);
        });
    });
}

/// Lets the shell's drain pump run once.
///
/// **Through `advance_clock` and not `run_until_parked`, for the reason
/// `tests/app_shell.rs`'s `tick` gives:** the pinned `TestScheduler::run` is
/// `while step() {}` with no clock advancement, so parking returns with the pump's
/// timer still in the future and never fires it. `advance_clock` walks the clock to
/// the next expiry and polls the woken task, so one call runs the pump **once** and
/// the next tick lands one interval further out.
fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(DRAIN_INTERVAL);
    cx.run_until_parked();
}

/// Everything a login produced, read from the shell and the seam.
#[derive(Debug, PartialEq, Eq)]
struct LoginState {
    /// Whether the window is still showing the form.
    in_login_mode: bool,
    /// Whether the shell holds a session.
    has_session: bool,
    /// Whether a socket was published into the bridge.
    has_transport: bool,
    /// Whether the bridge says a send would reach the wire.
    can_send: bool,
    /// Whether an attempt is outstanding.
    in_flight: bool,
}

impl LoginState {
    /// One line, so a failure names the whole situation rather than one field.
    fn describe(&self) -> String {
        format!("{self:#?}")
    }
}

fn login_state(cx: &VisualTestContext, shell: &Entity<Shell>) -> LoginState {
    shell.read_with(cx, |shell, app| LoginState {
        in_login_mode: shell.login_form().is_some(),
        has_session: shell.session().is_some(),
        has_transport: bridge::has_transport(app),
        can_send: bridge::try_read(app, |state| state.can_send()).unwrap_or(false),
        in_flight: bridge::login_in_flight(app),
    })
}

/// What the connection state says, or `None` when the state is not installed.
fn connection(cx: &VisualTestContext, shell: &Entity<Shell>) -> Option<ConnectionState> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.connection().clone())
    })
}

/// Polls the shell's own drain until `ready` holds, or reports what it saw.
///
/// **A condition poll and not a sleep**, per `AGENTS.md` §4.3 and per
/// `support::wait_until`'s own argument: the login POST completes on a worker thread
/// and arrives at the inbox, and *that* is the condition. `advance_clock` is the
/// only way the shell's schedule runs at all, so the loop drives the real timer
/// rather than calling a private helper that shares its body.
fn pump_until(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    what: &str,
    mut ready: impl FnMut(&VisualTestContext, &Entity<Shell>) -> bool,
) {
    let deadline = std::time::Instant::now() + READY_BUDGET;
    loop {
        tick(cx);
        if ready(cx, shell) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out after {READY_BUDGET:?} waiting for {what}; the shell is {}",
                login_state(cx, shell).describe()
            );
        }
        std::thread::yield_now();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The form's failure line, if it is showing one.
fn form_failure(cx: &VisualTestContext, shell: &Entity<Shell>) -> Option<(String, gpui::Hsla)> {
    shell.read_with(cx, |shell, app| {
        // **Owned out of the read, and that is the only shape a borrow out of
        // `read_with` can have**: a reference into the view would not outlive the
        // lease GPUI takes out to serve the read.
        shell.login_form()?.read_with(app, |form, _| {
            form.failure().map(|f| (f.text.clone(), f.color))
        })
    })
}

/// What the user typed into the two fields, and which one is active.
fn typed(cx: &VisualTestContext, shell: &Entity<Shell>) -> (String, String, Field) {
    shell.read_with(cx, |shell, app| {
        let form = shell.login_form().expect("a shell in the login mode");
        form.read_with(app, |form, _| {
            (
                form.username().to_owned(),
                form.password().to_owned(),
                form.active_field(),
            )
        })
    })
}

/// Focuses the form, which is exactly what `app::open` does on launch.
///
/// **The `run_until_parked` is load-bearing rather than tidy:** focusing marks the
/// window dirty, and the frame that follows is the one whose paint registers the
/// platform input handler. A keystroke dispatched before that frame is a keystroke
/// with no handler to receive it.
fn focus_form(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
    shell.update_in(cx, |shell, window, cx| {
        if let Some(form) = shell.login_form() {
            form.read(cx).focus_handle(cx).focus(window, cx);
        }
    });
    cx.run_until_parked();
}

/// A decoded login answer, as a line a failure message can use.
///
/// **Not `{:?}`, and that is a property of the production type rather than a
/// limitation of this test.** `network::rest::Session` deliberately does **not**
/// implement `Debug` — it holds a token, and a `{:?}` that cannot compile is a
/// stronger guarantee than a hand-written one that has to be trusted — so
/// `format!("{outcome:?}")` does not compile for either half of the `Result`. This
/// helper names the account and the error instead, which is what a reader of a
/// failing assertion actually needs.
fn describe(outcome: &Result<rest::Session, LoginError>) -> String {
    match outcome {
        Ok(session) => format!("a session for {:?}", session.username),
        Err(error) => format!("{error:?}"),
    }
}

// ---------------------------------------------------------------------------
// The real-server flows
// ---------------------------------------------------------------------------

/// A correct password logs in, and the token is one the socket accepts.
///
/// **The whole of `AGENTS.md` §8.1's Login Flow, and the second half is the part
/// that matters.** A login that returns a token is not a login; it is a login if
/// the token opens a socket. So this drives the production path end to end: the
/// view submits, the seam posts, the server answers, the shell stores the session
/// and starts the transport, and then the client reports
/// [`ConnectionState::Connected`].
///
/// **A refused handshake cannot pass this.** ADR-010 made authentication mandatory
/// and the server answers an unauthenticated upgrade with a 401 and no socket, which
/// the transport maps to [`ConnectionState::Rejected`] — a state that is not
/// `Connected`, so the wait below would time out rather than pass. **That is the
/// strongest available claim about the token**, and it is checked on the real
/// server rather than on a token-shaped string.
#[gpui::test]
fn a_correct_password_against_a_real_server_yields_a_token_the_socket_accepts(
    cx: &mut TestAppContext,
) {
    let server = ServerProcess::start("login-ok");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);

    fill(cx, &shell, ADMIN_USERNAME, ADMIN_PASSWORD);
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");

    pump_until(cx, &shell, "the socket to connect", |cx, shell| {
        connection(cx, shell) == Some(ConnectionState::Connected)
    });

    let state = login_state(cx, &shell);
    assert!(
        !state.in_login_mode,
        "a session was issued, so the window must have left the login mode: {}",
        state.describe()
    );
    assert!(
        state.has_session,
        "the issued token must reach `ConnectionSettings`, which is the only place \
         this client keeps a credential: {}",
        state.describe()
    );
    assert!(
        state.can_send,
        "and the state must agree the connection can carry a send, because \
         `AGENTS.md` 7.3's composer's whole question is `can_send`: {}",
        state.describe()
    );
}

/// A correct login moves the window from the form to the chat.
///
/// **The mode switch, asserted through the shell's own accessors rather than
/// through a painted frame**, because "which of two things is this window" is a
/// question with one owner and reading the owner is the exact answer. The chat side
/// is checked by asking the shell for its session and the seam for its socket,
/// which are the two facts the chat mode consists of.
#[gpui::test]
fn a_correct_login_moves_the_window_from_the_form_to_the_chat(cx: &mut TestAppContext) {
    let server = ServerProcess::start("login-mode");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);

    let before = login_state(cx, &shell);
    assert!(
        before.in_login_mode,
        "a server with no session must open the form, not the chat: {}",
        before.describe()
    );
    assert!(
        !before.has_transport,
        "and there is nothing to connect to until there is a session: {}",
        before.describe()
    );
    assert!(
        cx.debug_bounds("message-list").is_none(),
        "**the chat log must not be painted behind the form** -- a window showing a \
         chat a person has no claim on is a window telling them they are already \
         signed in"
    );

    fill(cx, &shell, ADMIN_USERNAME, ADMIN_PASSWORD);
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");

    pump_until(cx, &shell, "the mode switch", |cx, shell| {
        !login_state(cx, shell).in_login_mode
    });

    let after = login_state(cx, &shell);
    assert!(after.has_session, "and a session: {}", after.describe());
    assert!(after.has_transport, "and a socket: {}", after.describe());

    // And the form is not merely hidden: the chat tree is on screen, which
    // `debug_bounds` proves was painted rather than merely built.
    assert!(
        cx.debug_bounds("message-list").is_some(),
        "the chat log must be painted once the form is gone"
    );
    assert!(
        cx.debug_bounds(login_view::SELECTOR).is_none(),
        "and the form must be gone from the tree, not merely empty"
    );
}

/// A wrong password shows the server's own reason, and no token is kept.
///
/// **`AGENTS.md` §8.1's Auth Failure Flow and acceptance criterion 3, and the
/// assertion that matters most is the negative one**: the window must still be in
/// the login mode with no session. A client that showed an error *and* kept a
/// credential would be the failure the criterion is about.
///
/// **The text is the server's, not ours.** `sh_nexus_server`'s auth module answers
/// a bad credential with `invalid_credentials` and a sentence explaining it, and
/// this test asserts both halves are on screen — a client rendering its own "login
/// failed" would be strictly less useful and would fail here.
#[gpui::test]
fn a_wrong_password_shows_the_servers_own_reason_and_keeps_no_token(cx: &mut TestAppContext) {
    let server = ServerProcess::start("login-refused");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);

    fill(cx, &shell, ADMIN_USERNAME, WRONG_PASSWORD);
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");

    pump_until(cx, &shell, "the refusal", |cx, shell| {
        form_failure(cx, shell).is_some()
    });

    let state = login_state(cx, &shell);
    assert!(
        state.in_login_mode,
        "a refused login must leave the window on the form: {}",
        state.describe()
    );
    assert!(
        !state.has_session,
        "**no token may be kept from a refused login** -- this is acceptance \
         criterion 3, and a client that kept one would be connected as somebody it \
         did not authenticate: {}",
        state.describe()
    );
    assert!(
        !state.has_transport && !state.can_send,
        "and no socket, because nothing authenticated one: {}",
        state.describe()
    );

    let (text, _color) = form_failure(cx, &shell).expect("the refusal is on screen");
    assert!(
        text.contains("invalid_credentials"),
        "the server's refusal code must reach the user, because it is the handle \
         they can quote in a bug report; got {text:?}"
    );
    assert!(
        text.contains("401"),
        "and the status, because a 400 and a 401 look identical from the detail \
         alone and are fixed by different people; got {text:?}"
    );
    assert!(
        text.contains("not accepted"),
        "the server's own sentence, not a paraphrase of it; got {text:?}"
    );
}

/// A server that is not there is reported, and not swallowed.
///
/// **The negative half of the Login Flow, and the reason it is its own test
/// rather than a comment on the refusal case.** A wrong password and an absent
/// server are different *kinds* of thing to a user — one is an answer, the other is
/// silence — and a client that reported both as "login failed" would send the user
/// to retype a password that was never wrong. `AGENTS.md` §3.3 requires a network
/// failure to surface as a recoverable state and §5.2 asks for an actionable one.
///
/// **The port is chosen by binding and releasing it, which is `support::free_port`
/// and the honest way to name a port with nothing behind it.** It is a small race
/// against another process on the machine, and a lost race fails this test with a
/// *successful* login rather than with a wrong assertion.
#[gpui::test]
fn a_server_that_is_not_there_is_reported_rather_than_swallowed(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let endpoint = format!("ws://127.0.0.1:{port}{WS_PATH}");
    let (shell, cx) = login_shell(cx, &endpoint);

    fill(cx, &shell, ADMIN_USERNAME, ADMIN_PASSWORD);
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");

    pump_until(cx, &shell, "the unreachable report", |cx, shell| {
        form_failure(cx, shell).is_some()
    });

    let (text, _color) = form_failure(cx, &shell).expect("the failure is on screen");
    let lowered = text.to_lowercase();
    assert!(
        lowered.contains("refused") || lowered.contains("connect") || lowered.contains("reach"),
        "an unreachable server must say so in words a person can act on, and must \
         not claim the credential was rejected; got {text:?}"
    );

    let state = login_state(cx, &shell);
    assert!(
        state.in_login_mode,
        "and the window stays on the form: {}",
        state.describe()
    );
    assert!(
        !state.has_session && !state.has_transport,
        "an unreachable server yields neither a session nor a socket: {}",
        state.describe()
    );
}

/// A refusal reaches the window as the server's own words, in the failure colour.
///
/// **Read through the view's own rendering rather than through a painted frame,
/// and the reason is that the view's answer is data.** `login::failure` returns a
/// `Failure` precisely so a test can read it; asserting through a painted frame
/// would only prove that a string was drawn somewhere.
///
/// **The colour is asserted because it is a decision and not a default.**
/// `connection_banner.rs` draws `Rejected` in `danger` because that state is
/// terminal, and a refused login is terminal in exactly the same way — the same
/// two strings will not help on a retry. A muted line would under-report a decision
/// the server has already made.
#[gpui::test]
fn a_refusal_reaches_the_window_as_the_servers_own_words(cx: &mut TestAppContext) {
    let server = ServerProcess::start("login-words");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);

    fill(cx, &shell, ADMIN_USERNAME, WRONG_PASSWORD);
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");
    pump_until(cx, &shell, "the refusal", |cx, shell| {
        form_failure(cx, shell).is_some()
    });

    let (text, color) = form_failure(cx, &shell).expect("the refusal is on screen");
    let colors = sh_nexus::ui::Colors::dark();
    let rendered = login_view::failure(
        &LoginOutcome::Refused {
            status: 401,
            code: "invalid_credentials".to_owned(),
            detail: "that username and password combination was not accepted by this \
                     server"
                .to_owned(),
        },
        colors,
    )
    .expect("a refusal renders a line");

    assert_eq!(
        text, rendered.text,
        "what the window drew must be exactly what a refusal renders, so there is \
         one answer to \"what does a rejected login say\""
    );
    assert_eq!(
        color, rendered.color,
        "and one answer to which colour it is in"
    );
    assert_eq!(
        color, colors.danger,
        "which is the failure colour, not the muted one"
    );

    // The *other* outcome must not be drawn the same way: an unreachable server is
    // not a rejected credential and must not wear the failure colour.
    let unreachable = login_view::failure(
        &LoginOutcome::Failed {
            reason: "the connection was refused".to_owned(),
        },
        colors,
    )
    .expect("an unreachable server also renders a line");
    assert_eq!(
        unreachable.color, colors.text_muted,
        "nothing was decided, so nothing failed -- painting it in the failure colour \
         would report a rejected credential that never happened"
    );
    assert_ne!(
        unreachable.text, text,
        "and the two lines must differ, or a user cannot tell a wrong password from \
         a server that is not running"
    );
}

// ---------------------------------------------------------------------------
// The pure mappings
// ---------------------------------------------------------------------------

/// A `ws://` endpoint becomes an `http://` login URL, and the path is replaced.
///
/// **`rstest` rather than a loop, per `AGENTS.md` §4.3**, and each case is a
/// spelling an operator can actually type.
///
/// **The path replacement is the assertion worth reading twice.** The WebSocket
/// path is `/ws` and the login path is `/auth/login`; appending would produce
/// `/ws/auth/login`, which is a 404 that costs an afternoon. This test is what turns
/// that from a possible bug into a caught one.
#[rstest]
#[case("ws://127.0.0.1:8484/ws", "http://127.0.0.1:8484/auth/login")]
#[case("ws://localhost:8484/ws", "http://localhost:8484/auth/login")]
// A URL with no path at all is still a server, and the login URL is complete
// without inventing one.
#[case("ws://127.0.0.1:8484", "http://127.0.0.1:8484/auth/login")]
// A base path is *replaced*, not kept: the two paths are unrelated and a client
// that kept the first would post into the socket's namespace.
#[case(
    "ws://nexus.example:8484/socket",
    "http://nexus.example:8484/auth/login"
)]
// An operator who spelled the HTTP base directly gets the same answer, which is
// what makes a single configuration setting enough.
#[case("http://127.0.0.1:8484/anything", "http://127.0.0.1:8484/auth/login")]
fn the_websocket_endpoint_is_translated_into_an_http_login_url(
    #[case] given: &str,
    #[case] expected: &str,
) {
    assert_eq!(
        rest::http_login_endpoint(given).expect("a translatable endpoint"),
        expected
    );
}

/// An endpoint that asks for TLS is refused rather than downgraded to plain HTTP.
///
/// **This is the ADR-012 coupling made loud, and the refusal is the whole
/// design.** `hyper` is declared with no TLS feature, so a `wss://` or `https://`
/// endpoint cannot be spoken by this binary at all — and rewriting it to `http`
/// would send a password in the clear to a host that explicitly asked for
/// encryption, which is the worst outcome available rather than a safe one.
///
/// **A test rather than a comment because the day remote deployment lands this is
/// the line that breaks**, and a client that fails loudly at that moment is worth
/// several that fail silently.
///
/// **`rstest` with the expected phrase rather than a blanket assertion**, because
/// the two refusals here say different things and both are load-bearing: the TLS one
/// has to explain that *TLS* is the problem (an operator who cannot tell "this URL is
/// unusable" from "this URL needs encryption" has nothing to act on), and the
/// scheme one has to name the schemes this client speaks.
#[rstest]
#[case("wss://nexus.example/ws", "TLS")]
#[case("https://nexus.example/ws", "TLS")]
#[case("ftp://nexus.example/ws", "ws, wss, http or https")]
#[case("nexus.example/ws", "does not name a scheme")]
#[case("ws://", "names no host")]
#[case("", "does not name a scheme")]
fn an_endpoint_asking_for_tls_is_refused_rather_than_downgraded(
    #[case] given: &str,
    #[case] expected: &str,
) {
    let refused = rest::http_login_endpoint(given).expect_err("nothing here is translatable");
    assert!(
        matches!(refused, LoginError::Endpoint { .. }),
        "every unusable endpoint is an `Endpoint` refusal so the view can say so \
         plainly; got {refused:?}"
    );
    assert!(
        refused.to_string().contains(expected),
        "the refusal must explain itself -- \"this URL is not usable\" gives an \
         operator nothing to act on. Expected it to mention {expected:?}; got \
         {refused}"
    );
}

/// A `200` carrying no token is not reported as a refused credential.
///
/// **The one mapping a careless client gets wrong, and the reason it is its own
/// test rather than a branch assertion.** Every status this endpoint uses for a
/// refusal is a non-`2xx` — `401` for credentials, `400` for an unreadable body,
/// `500` for a storage fault — so a `200` with no `token` is not this server
/// refusing anything. It is **this client talking to something that is not this
/// API**, and reporting "your password was refused" for that is a false statement
/// about a credential.
///
/// A test that only checked the success path would never notice, and the difference
/// between the two messages is the difference between a user who fixes their
/// `SH_NEXUS_URL` and a user who retypes a password that was never wrong.
#[rstest]
#[case("{}", 200)]
#[case(r#"{"user_id":"u_1","username":"root"}"#, 200)]
#[case("not json at all", 200)]
#[case("[]", 201)]
#[case(r#"{"token": 42}"#, 200)]
fn a_success_body_without_a_token_is_not_reported_as_a_refused_credential(
    #[case] body: &str,
    #[case] status: u16,
) {
    let outcome = rest::decode_login_response(status, body.as_bytes());
    assert!(
        matches!(outcome, Err(LoginError::Unreadable { .. })),
        "a 2xx without a usable token means this is not the login endpoint, and \
         saying so is the only honest answer; got {}",
        describe(&outcome)
    );
}

/// A refusal body is read as the server's own code and sentence.
///
/// **And a body missing both still produces something a person can read**, which is
/// the case a `["code"]` mapping would render as an empty string — the one message
/// a user can act on least.
#[rstest]
#[case(
    r#"{"code":"invalid_credentials","detail":"that combination was not accepted"}"#,
    401,
    "invalid_credentials",
    "that combination was not accepted"
)]
#[case(
    r#"{"code":"invalid_request"}"#,
    400,
    "invalid_request",
    "the server refused the login with HTTP 400"
)]
#[case(r#"{}"#, 500, "unknown", "the server refused the login with HTTP 500")]
fn a_refusal_body_is_read_as_the_servers_own_code_and_sentence(
    #[case] body: &str,
    #[case] status: u16,
    #[case] code: &str,
    #[case] detail: &str,
) {
    match rest::decode_login_response(status, body.as_bytes()) {
        Err(LoginError::Refused {
            status: seen,
            code: seen_code,
            detail: seen_detail,
        }) => {
            assert_eq!(seen, status);
            assert_eq!(seen_code, code);
            assert_eq!(seen_detail, detail);
        }
        other => panic!(
            "a non-2xx status must produce a Refusal, got {}",
            describe(&other)
        ),
    }
}

/// A success body yields a session, and a missing advisory field is not a failure.
///
/// **`user_id` and `username` are advisory on this side, and demanding them would
/// be the failure mode worth naming**: the token is what the socket presents, so an
/// answer missing the account's id is a working credential with less information,
/// not a broken one.
#[test]
fn a_success_body_yields_a_session_and_an_absent_advisory_field_is_not_a_failure() {
    let full = rest::decode_login_response(
        200,
        br#"{"token":"t-1","user_id":"u_1","username":"root","expires_at_unix_ms":42}"#,
    )
    .expect("a complete answer");
    assert_eq!(full.token, "t-1");
    assert_eq!(full.user_id, "u_1");
    assert_eq!(full.username, "root");

    let sparse =
        rest::decode_login_response(200, br#"{"token":"t-2"}"#).expect("a token is enough");
    assert_eq!(sparse.token, "t-2");
    assert_eq!(
        sparse.user_id, "",
        "an absent advisory field is empty, not a failure -- refusing here would \
         refuse a working credential over a field nothing here uses"
    );
    assert_eq!(sparse.username, "");
}

/// A body past the ceiling is refused rather than buffered.
///
/// **`AGENTS.md` §7.1's ban on unbounded in-memory state, applied to a peer.** The
/// bound is this client's, because it is the client that would have to hold the
/// bytes, and it is far larger than the documented success body.
#[test]
fn a_body_past_the_ceiling_is_refused_rather_than_buffered() {
    let oversized = vec![b'x'; MAX_RESPONSE_BYTES + 1];
    match rest::decode_login_response(200, &oversized) {
        Err(LoginError::Unreadable { reason, .. }) => assert!(
            reason.contains("ceiling"),
            "the message must say the size was the problem, so the operator knows \
             the server is answering something other than this endpoint; got {reason:?}"
        ),
        other => panic!(
            "an oversized body must be refused, got {}",
            describe(&other)
        ),
    }
}

/// The login path this client posts to is the one the suite uses.
///
/// **The duplication `network/rest.rs`'s module docs admit to, watched rather than
/// hidden.** `rest::LOGIN_PATH` is a literal because the client cannot name the
/// server crate, and the two spellings agreeing is the only thing that keeps that
/// honest. `tests/support/mod.rs` carries the test-side copy of the same argument.
#[test]
fn the_login_path_this_client_posts_to_is_the_one_the_suite_uses() {
    assert_eq!(
        rest::LOGIN_PATH,
        support::LOGIN_PATH,
        "`rest::LOGIN_PATH` and `tests/support/mod.rs::LOGIN_PATH` are two copies of \
         the server's own `auth::LOGIN_PATH`. A rename on any side must turn this red."
    );
    assert!(
        LOGIN_PATH.starts_with('/') && LOGIN_PATH.len() > 1,
        "the login path must be an absolute path or every request is a 404: \
         {LOGIN_PATH:?}"
    );
}

// ---------------------------------------------------------------------------
// Local mode is unchanged
// ---------------------------------------------------------------------------

/// Neither variable set is still the offline shell.
///
/// **Acceptance criterion 4, and the reason it is a test rather than a claim:**
/// `ConnectionSettings::from_env` has four arms and this milestone added a fifth
/// *case* rather than a fifth arm, and the whole argument for doing it that way is
/// that *no existing test has to change*. This is the test that holds that promise.
///
/// **The environment cannot be set from a test**, which is the constraint
/// `ConnectionSettings::from_parts_for_test` documents at length: `set_var` is
/// `unsafe` under Rust 2024 and mutates state every other test in the process reads.
/// So this runs with neither variable set — asserted first, so a leaked one fails
/// here rather than making the assertion below pass for the wrong reason.
#[test]
fn neither_variable_set_still_starts_the_offline_shell() {
    assert!(
        std::env::var("SH_NEXUS_URL").is_err() && std::env::var("SH_NEXUS_TOKEN").is_err(),
        "this suite must run with neither variable set; a leaked one would make the \
         assertions below pass or fail for the wrong reason"
    );
    assert_eq!(
        ConnectionSettings::from_env().expect("absent configuration is not an error"),
        None,
        "**both variables absent is the documented offline shell and not an \
         error** -- a client that refuses to start without a server is a client \
         nobody can run for the first time"
    );
    assert_eq!(
        app::startup_from_env().expect("the offline arm is never an error"),
        Startup::Local,
        "and the login milestone's matrix must leave that answer untouched"
    );
}

/// The connected state has exactly one representation.
///
/// **The runtime half of the local-mode claim**, and the reason it exists is that
/// the two ways of becoming connected must not be able to disagree. The
/// environment path and the login path both build a [`ConnectionSettings`]; if
/// they built different things there would be two code paths for "connected" and
/// `bridge::install_transport`'s own warning — one transport, one publication point
/// — would only hold for one of them.
#[test]
fn the_connected_state_has_exactly_one_representation() {
    let built = ConnectionSettings::new("ws://a/ws", "token-a");
    let via_parts = ConnectionSettings::from_parts_for_test("ws://a/ws", "token-a");
    assert_eq!(
        built, via_parts,
        "the login path and the environment path must build the same value"
    );

    assert_eq!(
        Startup::Connected(built),
        app::Startup::Connected(via_parts),
        "and one `Startup` arm for it -- a second would be a second answer to \
         \"is this client configured\""
    );
}

/// A token is never rendered, by `Debug` or by `Display`.
///
/// **Acceptance criterion 5's runtime half, and the two hand-written impls are
/// the whole of what it checks.** `LoginOutcome` carries the token in one variant
/// and `ConnectionSettings` carries it in a field; both write their `Debug` out by
/// hand for `AGENTS.md` §7.5, and **a derived `Debug` on either would compile and
/// print the credential**. So this asserts the rendered output rather than the
/// presence of a manual impl — the behaviour, not the technique.
///
/// **`Display` is asserted too**, because it is what `tracing::info!(%outcome)` would
/// print and a hand-written `Display` is exactly as capable of a leak as a
/// hand-written `Debug`.
#[test]
fn no_production_module_prints_a_credential_in_its_debug_output() {
    let token = "a-token-that-must-never-be-printed-0123456789";

    let settings = ConnectionSettings::new("ws://127.0.0.1:8484/ws", token);
    let rendered = format!("{settings:?}");
    assert!(
        !rendered.contains(token),
        "`ConnectionSettings`'s `Debug` printed the token: {rendered}"
    );
    assert!(
        rendered.contains("token_configured"),
        "and it must still say *whether* a token is present -- \"a URL with no token \
         configured\" is diagnosable and one boolean is enough to see it; got \
         {rendered}"
    );

    let outcome = LoginOutcome::LoggedIn {
        token: token.to_owned(),
        username: "root".to_owned(),
    };
    for rendered in [format!("{outcome:?}"), outcome.to_string()] {
        assert!(
            !rendered.contains(token),
            "`LoginOutcome` printed the token: {rendered}"
        );
        assert!(
            rendered.contains("root"),
            "the username is not a secret and is exactly what an operator needs; \
             got {rendered}"
        );
    }

    // And no `LoginError` may carry one, in any variant -- which is what lets
    // `Debug` be *derived* on it rather than written out.
    for (status, code, detail) in [
        (
            401,
            "invalid_credentials",
            "that combination was not accepted",
        ),
        (400, "invalid_request", "the body was unreadable"),
        (500, "invalid_request", "the store failed"),
    ] {
        let error = LoginError::Refused {
            status,
            code: code.to_owned(),
            detail: detail.to_owned(),
        };
        let rendered = format!("{error:?} / {error}");
        assert!(
            !rendered.contains(token),
            "a refusal must never carry a credential, or a derived `Debug` would \
             leak one: {rendered}"
        );
    }
}

// ---------------------------------------------------------------------------
// The password rule, as source scans
// ---------------------------------------------------------------------------

/// Every `.rs` file under `crates/sh_nexus/src`.
fn source_files_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(source_files_under(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// `crates/sh_nexus/src`.
fn source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// A file's source with every comment removed.
///
/// **The same shape `tests/layer_boundary.rs` uses, with one addition, and the
/// addition is load-bearing.** A comment can *discuss* the rule without violating it,
/// and `network/rest.rs`'s and `login.rs`'s own documentation talk about passwords at
/// length; a naive substring scan would fail on its own documentation and the fix
/// would be to stop documenting the boundary.
///
/// **The addition is `//!`.** A module doc begins `/` `!` `/`, and a stripper that
/// only tests for `//` sees a lone `/`, decides it is division, and leaves the whole
/// prose behind — so `core/`'s module docs would be scanned as code. That is not a
/// hypothetical: it is a trap this scanner walked into while being written, and the
/// only reason it was caught is that
/// [`the_password_scanner_is_not_vacuous`] asserts prose is *not* a violation.
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
                // `//` and `//!` are both line comments. `AGENTS.md` 2.2 mandates
                // `///` on public items and this crate leads every module with a
                // `//!` header, so both spellings are load-bearing to the scanner.
                Some('/') | Some('!') => {
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

/// The comment-stripped source of one file, or a panic naming it.
fn stripped_source(path: &Path) -> String {
    without_comments(
        &fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display())),
    )
}

/// The words that mean "this source is holding a credential".
///
/// **Lowercase, and matched against a lowercased source**, so `Password`,
/// `CREDENTIAL` and `credential` are one rule rather than three spellings of it.
/// An allow-list that forgets a capitalisation is a rule that has a hole in it, and
/// `Credential` is exactly the shape a real mistake takes — `sh_nexus_server` has a
/// type by that name, and somebody porting it would not think twice.
const CREDENTIAL_WORDS: [&str; 2] = ["password", "credential"];

/// The first credential word in a comment-stripped source, if it names one.
///
/// **A question rather than an assertion, and the reason is
/// [`the_password_scanner_is_not_vacuous`].** A scanner that can only fail by
/// panicking has to be exercised with `catch_unwind`, and a caught panic still
/// prints -- so a suite demonstrating its own scanners would bury its real failures
/// in noise it had manufactured. Returning the token instead makes the demonstration
/// a comparison.
fn names_a_credential(stripped: &str) -> Option<&'static str> {
    let lowered = stripped.to_lowercase();
    CREDENTIAL_WORDS
        .into_iter()
        .find(|word| lowered.contains(word))
}

/// Asserts that a comment-stripped source names no credential.
///
/// **A function rather than an inline loop, and the reason is that
/// [`the_password_scanner_is_not_vacuous`] has to run the *same* check.** A rule
/// demonstrated on synthetic sources and a rule enforced on the tree are two
/// different things unless they share one implementation, and a scanner that only
/// ever sees real code proves nothing about whether it can still fail — the class of
/// bug `tests/layer_boundary.rs` records at length, where a scanner that fails
/// **open** keeps every test passing while checking nothing.
#[track_caller]
fn assert_no_credential(label: &str, stripped: &str) {
    if let Some(word) = names_a_credential(stripped) {
        panic!(
            "{label} names `{word}` after comment stripping. A password must never \
             reach state/ or core/: `AGENTS.md` 3.2 gives state/ no I/O and core/ no \
             knowledge of anything outside its own arguments, and the login view hands \
             the credential to `network/rest.rs` directly rather than through either. \
             The one sanctioned door is `state/bridge.rs::begin_login`, which takes it \
             by value and stores nothing."
        );
    }
}

/// The credential scan rejects what it claims to and admits what it must.
///
/// **A green tree proves only that nothing *currently* violates the rule.** The
/// samples are the shapes a real mistake takes: a struct field, a function
/// parameter, a type path in each of the capitalisations the rule has to catch, and a
/// mention on a line that also carries a comment. **Prose is asserted *not* to trip
/// it**, which is what lets `network/rest.rs` and `login.rs` document the rule at
/// length — and a scanner that flagged their own documentation would have been
/// "fixed" by deleting the documentation.
#[test]
fn the_password_scanner_is_not_vacuous() {
    for violation in [
        "pub struct AppState { password: String }",
        "pub fn remember(state: &mut AppState, password: &str) {}",
        "let secret: Password = read();",
        "let p = password.clone(); // a trailing comment about credentials",
        "use crate::core::models::Credential;",
        "struct CREDENTIAL(String);",
    ] {
        assert!(
            names_a_credential(&without_comments(violation)).is_some(),
            "`{violation}` reaches a word the credential rule must reject, but the \
             check found nothing -- the scan is vacuous and every test built on it is \
             checking nothing"
        );
    }

    for permitted in [
        // The rule discussed in prose, which must not be a violation.
        "//! The password never reaches this layer.\nuse std::fmt;\n",
        "/// This field is a message body, not a credential.\npub struct Message {}\n",
        // A word that merely contains a token as a substring.
        "pub fn compile(&self) -> Result<()> { Ok(()) }\n",
        // The state, which legitimately holds ids, sizes and cursors.
        "pub struct AppState { self_user_id: String, messages: HashMap<String, Vec<Message>> }",
    ] {
        assert_eq!(
            names_a_credential(&without_comments(permitted)),
            None,
            "`{permitted}` must be admitted: it is either prose about the rule or \
             code that has nothing to do with one"
        );
    }
}

/// The password never reaches a source file under `state/` or `core/`.
///
/// **Acceptance criterion 5, enforced rather than asserted, and the reason this
/// lives in a test is that the rule has no compiler answer.** A password is a
/// `String`; a `String` may be a message body, a channel id, a theme hex or a
/// credential, and nothing about the type distinguishes them. So the only way to
/// hold the rule is to check that the two layers which have no business knowing a
/// password never name one.
///
/// **`state/bridge.rs` is the one file that does — as a by-value parameter of one
/// door — and it is therefore excluded by name rather than by a special case in the
/// scan.** That exclusion is the point rather than a hole in it: the door is where
/// the credential is *supposed* to be, and it holds it for the length of one
/// request by construction. [`the_one_sanctioned_login_door_takes_the_password_by_value`]
/// is the test that holds the exemption honest. `state/app_state.rs` and
/// `state/actions.rs` are both inside the scan, which is the half that matters: the
/// state is what a credential would be *stored* in rather than passed through.
#[test]
fn the_password_reaches_no_state_or_core_source() {
    let root = source_dir();
    let mut scanned = 0usize;

    for layer in ["state", "core"] {
        for file in source_files_under(&root.join(layer)) {
            if file.file_name().is_some_and(|name| name == "bridge.rs") {
                continue;
            }
            assert_no_credential(&file.display().to_string(), &stripped_source(&file));
            scanned += 1;
        }
    }

    assert!(
        scanned > 5,
        "the scan found only {scanned} files, which means it is looking in the \
         wrong place -- it should cover every module under state/ and core/"
    );

    // And `AppState` itself, named directly: the strongest form of the rule, since
    // the state is where a credential would live rather than pass through.
    let app_state = stripped_source(&root.join("state").join("app_state.rs"));
    assert!(
        !app_state.contains("token") && !app_state.contains("password"),
        "AppState must hold neither a session token nor a password. The token lives \
         in `app::ConnectionSettings` and the password exists only inside one HTTP \
         request; a credential in the application state would be a field with no \
         `AGENTS.md` 3.1 entry describing it."
    );
}

/// The one sanctioned login door holds the password by value and stores nothing.
///
/// **The positive half of the previous test, and the reason it is a separate test
/// rather than an exemption.** A rule enforced by "the scanner skips one file" is
/// only as good as what that file does — so this asserts the shape the exemption
/// relies on: `begin_login` takes a `String`, and **no field named for a password
/// exists anywhere in `state/`**, which is what makes "moved and dropped" true
/// rather than merely intended.
#[test]
fn the_one_sanctioned_login_door_takes_the_password_by_value() {
    let stripped = stripped_source(&source_dir().join("state").join("bridge.rs"));

    assert!(
        stripped.contains("password: String"),
        "`begin_login` must take the password **by value** -- a `&str` would leave \
         the caller holding it for the length of the request and make the \"moved \
         and dropped\" claim untrue"
    );
    // **Counted rather than searched for, because `"password:"` is a substring of
    // `"password: String"`.** Every occurrence of the label has to be the by-value
    // parameter; a struct field or a struct literal would be one more than the
    // parameter and would outlive the request.
    let labelled = stripped.matches("password:").count();
    let by_value = stripped.matches("password: String").count();
    assert_eq!(
        labelled, by_value,
        "**every mention of a password in `state/` must be the by-value parameter** \
         -- {labelled} occurrences of the label and {by_value} of them by value. A \
         field would outlive the request and would be a credential in a struct."
    );
    assert!(
        stripped.contains("pub fn begin_login"),
        "and the door must exist by name, since the view calls it"
    );
}

/// No production source names a credential in a `tracing` field or prints one.
///
/// **The log half of `AGENTS.md` §7.5, and the reason it is a source scan is that
/// the alternative is a `Debug`-only check, which covers the accident and not the
/// deliberate act.** A `tracing::info!(password = %password)` would pass every
/// `Debug` test above and print a credential to every subscriber.
///
/// **Comments are stripped first**, so a module can *explain* the rule at length —
/// which `network/rest.rs`, `login.rs` and `bridge.rs` all do — without tripping it.
#[test]
fn no_production_source_logs_a_credential_or_prints_one() {
    for file in source_files_under(&source_dir()) {
        let stripped = stripped_source(&file);

        assert!(
            !stripped.contains("println!") && !stripped.contains("eprintln!"),
            "{} prints to a stream. `AGENTS.md` 7.1 bans `println!` in production and \
             7.5 requires `tracing`; a login form's own diagnostics belong in a \
             `tracing` call the window's subscriber decides about.",
            file.display()
        );

        for line in stripped.lines() {
            let trimmed = line.trim();
            let is_log = trimmed.contains("info!(")
                || trimmed.contains("warn!(")
                || trimmed.contains("error!(")
                || trimmed.contains("debug!(")
                || trimmed.contains("trace!(")
                || trimmed.contains("event =");
            if !is_log {
                continue;
            }
            for token in ["password", "token"] {
                assert!(
                    !trimmed.contains(token),
                    "{} logs `{token}`: {trimmed}\n`AGENTS.md` 7.5 says never log \
                     credentials -- ids, sizes and outcomes only. A session token in \
                     a log line is a credential in a file somebody will paste into a \
                     bug report.",
                    file.display()
                );
            }
        }
    }
}

/// The login view asks the seam and names no layer below it.
///
/// **Acceptance criterion 6, re-asserted here rather than left to
/// `tests/layer_boundary.rs` for one reason: the login view is the first thing in
/// `ui/` with an actual reason to want a network call.** The existing scanner proves
/// the rule holds; this test proves the rule still has teeth *and* that the new
/// view is the thing exercising it, which is the claim a future edit to
/// `ui/views/login.rs` could break.
#[test]
fn the_login_view_asks_the_seam_and_names_no_network_module() {
    let stripped = stripped_source(&source_dir().join("ui").join("views").join("login.rs"));

    assert!(
        stripped.contains("use crate::state::bridge"),
        "the view must reach the state layer the one permitted way"
    );
    assert!(
        stripped.contains("bridge::begin_login"),
        "and it must actually use that door rather than merely importing it -- an \
         unused import would leave the view unable to do anything at all"
    );
    for forbidden in [
        "crate::network",
        "crate::db",
        "crate::state::app_state",
        "crate::state::actions",
    ] {
        assert!(
            !stripped.contains(forbidden),
            "the login view names `{forbidden}`. `AGENTS.md` 3.2 and PLAN.md section \
             4 give ui/ the seam and the pure domain modules: network/ and db/ are \
             reached through state/bridge.rs or not at all."
        );
    }
}

/// The network layer reaches nothing above it.
///
/// **A structural check rather than a behavioural one, and the reason it earns its
/// place is that a cycle here would be a hang rather than a wrong answer.** The
/// login path is `login.rs` -> `bridge::begin_login` -> `network::rest::login`, and a
/// mistake that made the REST module reach back through the seam would compile
/// (both are `pub`) and would deadlock on the first attempt.
///
/// **`crate::state::bridge` is allowed and everything else above it is not**, and
/// the exemption is the design rather than a hole: `network/ws.rs` takes a
/// [`bridge::EventSender`] and reports `Delivery`, which is a *reported value* and
/// not a call back into the seam. The forbidden half is `crate::state::app_state`
/// and `crate::state::actions` — the layers a transport must not know exist —
/// plus `crate::ui` and `crate::app`.
#[test]
fn the_network_layer_reaches_no_state_module() {
    for file in source_files_under(&source_dir().join("network")) {
        let stripped = stripped_source(&file);
        for forbidden in [
            "crate::state::app_state",
            "crate::state::actions",
            "crate::ui",
            "crate::app",
        ] {
            assert!(
                !stripped.contains(forbidden),
                "{} names `{forbidden}`. `AGENTS.md` 3.2 makes network/ protocol \
                 handling only: it emits values and never reaches up. A cycle \
                 between the transport and the state layer would compile and would \
                 hang on the first message.",
                file.display()
            );
        }
        for forbidden in ["gpui", "cx.update_global", "cx.update"] {
            assert!(
                !stripped.contains(forbidden),
                "{} names `{forbidden}`. `network/` never touches GPUI state \
                 directly -- PLAN.md section 4 names state/bridge.rs as the only \
                 module allowed to call cx.update_global.",
                file.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The view's own behaviour
// ---------------------------------------------------------------------------

/// Typing reaches both fields through the platform input handler.
///
/// **The absence failure mode, and the reason it is written the way it is.**
/// `Window::dispatch_keystroke` forwards a character to the focused input handler
/// **only when the key propagated** (`gpui/src/window.rs:5365`), so a login form
/// that stopped propagation on character keys would compile, run, paint correctly
/// and silently accept nothing. Only a test that types through the platform path
/// and then asks the *view* can tell the difference.
///
/// **`cx.simulate_input`, not a direct call to the view's text method**, for
/// exactly that reason. The draft is a *summary* of what happened, and the input
/// method is the thing that could be broken.
#[gpui::test]
fn typing_reaches_the_login_fields_through_the_platform_input_handler(cx: &mut TestAppContext) {
    let server = ServerProcess::start("login-typing");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);
    focus_form(cx, &shell);

    cx.simulate_input("root");
    cx.run_until_parked();
    let (username, password, active) = typed(cx, &shell);
    assert_eq!(
        username, "root",
        "a character reaches the handle field only if the key propagated, the \
         paint-time handler was registered, and the platform forwarded `key_char`. \
         A form that stopped propagation would fail here with no error anywhere."
    );
    assert_eq!(password, "", "and only the field that was active");
    assert_eq!(active, Field::Username, "the form opens on the handle");

    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        typed(cx, &shell).2,
        Field::Password,
        "`tab` moves to the secret field, and `enter` alone does not"
    );

    cx.simulate_input("hunter2hunter2");
    cx.run_until_parked();
    let (username, password, _) = typed(cx, &shell);
    assert_eq!(
        username, "root",
        "the handle field is untouched by typing elsewhere"
    );
    assert_eq!(
        password, "hunter2hunter2",
        "the secret field took the characters"
    );
}

/// `Tab` moves between the fields, and `Enter` does not type a newline into one.
///
/// **Both claimed keys must stop propagation, and this is the test that forces
/// both.** `Window::dispatch_keystroke` begins with `keystroke.with_simulated_ime()`,
/// and that synthesises a `key_char` for named keys — `"enter" => Some("\n")` *and*
/// `"tab" => Some("\t")` (`gpui/src/platform/keystroke.rs:242`). A propagating
/// `Tab` would move to the secret field **and then type a tab into it**, and a
/// propagating `Enter` would submit and then type a newline into whatever is
/// focused.
///
/// **The `Enter` half is asserted on a submit that is refused before the password is
/// moved**, which is the only shape in which the buffer survives to be read. A
/// *successful* submit empties the secret on purpose — `LoginView::submit` moves it
/// into the request — so reading it afterwards would prove nothing about
/// propagation and would instead prove the move, which
/// [`a_submitted_password_leaves_the_form_rather_than_lingering_in_it`] asserts
/// directly.
#[gpui::test]
fn tab_moves_between_the_two_fields_and_enter_does_not_type_a_newline(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let endpoint = format!("ws://127.0.0.1:{port}{WS_PATH}");
    let (shell, cx) = login_shell(cx, &endpoint);
    focus_form(cx, &shell);

    cx.simulate_input("root");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("secret-passphrase");
    let (username, password, active) = typed(cx, &shell);
    assert_eq!(username, "root");
    assert_eq!(active, Field::Password);
    assert_eq!(
        password, "secret-passphrase",
        "**a tab must move the field and not also type a tab** -- \
         `with_simulated_ime` synthesises a tab character for the `tab` key, so a \
         propagating Tab would make the secret field start with one nobody typed"
    );

    // A submit that is refused *before* the secret is moved, with the secret field
    // focused so a synthesised newline would land in it.
    fill(cx, &shell, "", "secret-passphrase");
    focus_form(cx, &shell);
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (_, password, _) = typed(cx, &shell);
    assert_eq!(
        password, "secret-passphrase",
        "**an Enter must submit and not also type a newline** -- `with_simulated_ime` \
         synthesises a newline for `enter`, and a propagating key would leave one in \
         a password nobody typed"
    );
    assert!(
        form_failure(cx, &shell).is_some(),
        "and the refusal must be the blank-handle one, which is what kept the \
         secret from being moved"
    );
}

/// A submitted password leaves the form rather than lingering in it.
///
/// **The move, asserted on its own rather than as a side effect of the Enter test.**
/// `LoginView::submit` takes the secret with `std::mem::take` before it hands it to
/// the seam, so a window that failed to start a request does not sit holding a
/// credential it has no use for. **A `clone` there would pass every other test in
/// this file** — the login would still work, the password would still be sent, and
/// the buffer would simply keep a copy of a credential the request already
/// consumed.
///
/// **An absent server, so the form stays on screen** and the buffer is readable
/// afterwards. The assertion does not care which outcome came back.
#[gpui::test]
fn a_submitted_password_leaves_the_form_rather_than_lingering_in_it(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let endpoint = format!("ws://127.0.0.1:{port}{WS_PATH}");
    let (shell, cx) = login_shell(cx, &endpoint);

    fill(cx, &shell, "root", "secret-passphrase");
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let (username, password, _) = typed(cx, &shell);
    assert_eq!(
        password, "",
        "**the secret must be moved into the request, not copied into it** -- a \
         buffer still holding a credential after a submit is a credential in a \
         `String` with no owner"
    );
    assert_eq!(
        username, "root",
        "and only the secret: the handle is not a credential, it is the user's name, \
         and clearing it would make a retry after a typo impossible"
    );
}

/// A blank submit is refused in the window, without being sent.
///
/// **The one refusal the server never got to make, and the reason it exists at all
/// is that the alternative is a lie.** A submit with an empty handle cannot succeed
/// on any server, so sending it produces a 401 whose detail says the *credential*
/// was not accepted -- which is false, and `AGENTS.md` §5.2's actionable-error rule
/// is written against exactly that.
///
/// **The assertion that it was *not sent* is the one worth reading twice**: a form
/// that draws the right message and still posts the password would have fixed the
/// visible half of the defect.
#[gpui::test]
fn a_blank_submit_is_refused_without_being_sent(cx: &mut TestAppContext) {
    let server = ServerProcess::start("login-blank");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);
    focus_form(cx, &shell);

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let (text, _color) = form_failure(cx, &shell).expect("the refusal is on screen");
    assert!(
        text.contains("username"),
        "an empty handle must be named, so the user knows which field to fill; got {text:?}"
    );
    assert!(
        !text.contains("credential") && !text.contains("password"),
        "and it must not claim the credential was rejected, because nothing was \
         sent; got {text:?}"
    );

    let state = login_state(cx, &shell);
    assert!(
        state.in_login_mode && !state.has_session && !state.has_transport,
        "the window stays on the form and nothing is authenticated: {}",
        state.describe()
    );

    // Two real ticks, so a request that *had* been sent would have had time to
    // leave on loopback and leave its flag set.
    tick(cx);
    tick(cx);
    assert!(
        !login_state(cx, &shell).in_flight,
        "**nothing was sent, so no attempt is outstanding** -- and the flag is the \
         only place that could know, which is why the seam owns it rather than the \
         view"
    );
}

/// A second submit while one is outstanding is refused.
///
/// **One attempt, one session, and the reason it matters is a server-side fact
/// rather than a style one.** Every `POST /auth/login` mints a session on the
/// server, so two concurrent attempts mint two — and which one the socket then
/// presented would depend on which answer the drain happened to take first. A user
/// double-tapping `Enter` would get a credential the client then discarded, and a
/// session that lives on the server with nobody holding it.
///
/// **The two calls are made back to back inside one `cx.update`, and that is what
/// makes this deterministic rather than a race.** The in-flight flag is cleared only
/// by `bridge::try_take_login_outcome`, which only the shell's drain pump calls — so
/// between two calls in the same turn nothing can retire the first attempt, however
/// fast the server answers. A test that pressed `Enter` twice through the view and
/// then asserted on the flag would be asserting on Argon2id's timing.
#[gpui::test]
fn a_second_submit_while_one_is_outstanding_is_refused(cx: &mut TestAppContext) {
    let server = ServerProcess::start("login-twice");
    let endpoint = server.endpoint();
    let (_shell, cx) = login_shell(cx, &endpoint);

    // The seam, not the view: this is the door that has to refuse, because a view
    // that could be trusted to call it once would be a second place that decides.
    let (first, second) = cx.update(|_window, app| {
        let first = bridge::begin_login(app, &endpoint, ADMIN_USERNAME, ADMIN_PASSWORD.to_owned());
        let second = bridge::begin_login(app, &endpoint, ADMIN_USERNAME, ADMIN_PASSWORD.to_owned());
        (first, second)
    });

    assert!(first, "the first attempt must be accepted");
    assert!(
        !second,
        "**the seam must refuse a concurrent attempt** -- every login mints a \
         session on the server, and two of them would leave the socket presenting \
         whichever answer the drain happened to take first"
    );
}

/// The form paints its fields, and no failure line until one exists.
///
/// **Both halves, and the second is the one that could pass for the wrong reason.**
/// `debug_bounds` asks the window, so a non-zero size proves the elements were
/// *painted* rather than merely built. But a view that always contributed a row for
/// its message would paint one too, and "there is no line" would be unobservable --
/// which is why the message is conditionally added with `when_some` and has a
/// selector of its own.
///
/// **The post-refusal assertion is the one that matters**: a form whose error line
/// never appears is a form that swallows a failed login, and `AGENTS.md` §8.1's
/// Auth Failure Flow is exactly that flow.
#[gpui::test]
fn the_login_screen_renders_its_fields_and_no_failure_line_until_one_exists(
    cx: &mut TestAppContext,
) {
    let server = ServerProcess::start("login-paint");
    let endpoint = server.endpoint();
    let (shell, cx) = login_shell(cx, &endpoint);

    for selector in [
        login_view::SELECTOR,
        login_view::USERNAME_SELECTOR,
        login_view::PASSWORD_SELECTOR,
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} must have recorded its bounds"));
        assert!(
            bounds.size.height > px(0.) && bounds.size.width > px(0.),
            "{selector} must occupy real space, got {:?}",
            bounds.size
        );
    }
    assert!(
        cx.debug_bounds(login_view::FAILURE_SELECTOR).is_none(),
        "**no failure line may be painted before anything failed** -- a permanent \
         red strip on a form whose user has not typed yet is a client permanently \
         apologising for a choice they have not made"
    );

    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let bounds = cx
        .debug_bounds(login_view::FAILURE_SELECTOR)
        .expect("the refusal must be painted, not merely stored");
    assert!(
        bounds.size.height > px(0.),
        "a failure line drawn with no height is a failure nobody can read; got {:?}",
        bounds.size
    );
}

/// A login completes with the event inbox completely silent.
///
/// **A login is an HTTP exchange and produces no `DomainEvent` at all**, which is
/// the whole reason [`app::Shell`] polls the login outcome *outside* the event drain
/// rather than as something `bridge::drain` returns. This test proves the two are
/// independent in both directions:
///
/// - **The outcome arrives with nothing queued.** `EventSender::deliver` has zero
///   calls here, so the inbox is empty from the first tick onwards and the only
///   thing that can put a message on the screen is the login poll.
/// - **The outcome did not come through the inbox.** The drain reports zero
///   delivered on the far side of a completed login.
///
/// **Without it, a future edit that moved the login poll below `bridge::drain` would
/// pass every other test in this file** — every one of which submits against a socket
/// that is silent anyway, which is exactly the state where the mistake is invisible.
#[gpui::test]
fn a_login_completes_with_the_event_inbox_completely_silent(cx: &mut TestAppContext) {
    let port = support::free_port().expect("an ephemeral loopback port");
    let endpoint = format!("ws://127.0.0.1:{port}{WS_PATH}");
    let (shell, cx) = login_shell(cx, &endpoint);

    // Nothing is delivered on purpose, so the inbox is empty from the first tick.
    fill(cx, &shell, ADMIN_USERNAME, ADMIN_PASSWORD);
    focus_form(cx, &shell);
    cx.simulate_keystrokes("enter");

    pump_until(cx, &shell, "the login answer", |cx, shell| {
        form_failure(cx, shell).is_some()
    });

    let report = cx
        .update(|_window, app| bridge::drain(app))
        .expect("the state is installed");
    assert_eq!(
        report.delivered(),
        0,
        "**the login answer was not delivered through the event inbox** -- it arrived \
         with nothing else in it, so the poll is a different call from `drain`"
    );
    assert!(
        report.is_empty(),
        "and the inbox is simply quiet rather than closed: {report:?}"
    );
}
