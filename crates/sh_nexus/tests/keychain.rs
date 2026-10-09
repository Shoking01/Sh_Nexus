//! The OS keychain: a session that survives closing the app, and a client that
//! survives the keychain not being there.
//!
//! # What this file is for
//!
//! `odd/tasks/6b-keychain.md` is the change record. Before it, a session lasted one
//! process: `odd/tasks/6a-login.md` put a login form in front of a person and left the
//! token in memory, and it said why — §7.1 of `AGENTS.md` forbids plaintext token
//! storage and names the OS keychain as where a credential belongs, and a form that
//! persisted its own session would be a plaintext token on disk.
//!
//! **Every test here is about the window.** What the credential store itself promises is
//! asserted beside the code that makes the promise — the `#[cfg(test)] mod tests` in
//! `src/platform/token_store.rs`, which holds the store seam's round-trip, its five
//! implementations, the `StoredCredential` redaction and the scan that keeps a credential
//! out of `state/` and `core/`. Those are unit tests of a module with no window in it, and
//! a test sitting here would pay for `gpui`, `sh_nexus::app` and `sh_nexus::state::bridge`
//! to reach assertions that mention none of them.
//!
//! | Test | What it proves | Kind |
//! |---|---|---|
//! | [`the_resumed_window_opens_in_one_of_two_modes_and_only_two`] | the startup decision is a pure function | pure |
//! | [`the_startup_phase_draws_neither_the_chat_nor_the_login_form`] | a third mode that shows neither | runtime, real bounds |
//! | [`an_empty_credential_store_opens_the_login_form`] | the startup phase resolves to a form | runtime |
//! | [`a_stored_credential_opens_the_chat_with_no_login_form`] | acceptance criterion 1 | real server process, real window |
//! | [`a_rejected_stored_credential_is_cleared_and_the_login_form_appears`] | acceptance criterion 2 — the lockout trap | real server process, real window |
//! | [`a_login_the_store_refuses_to_keep_still_leaves_a_working_session`] | "losing a remembered credential is recoverable" | real server process, real window |
//! | [`the_platform_layer_reaches_no_ui_and_no_network`] | acceptance criterion 5, enforced | source scan |
//! | [`the_platform_boundary_is_not_vacuous`] | that scan can still fail | source scan |
//! | [`no_platform_source_can_print_a_credential`] | acceptance criterion 4, at the source | source scan |
//! | [`the_credential_printers_are_not_vacuous`] | that scan can still fail | source scan |
//!
//! # The four scans of `src/platform/` are here rather than beside the store
//!
//! **A scan and its proof of non-vacuity must both sit outside the tree being scanned, and
//! that is the whole reason these four did not follow the store's tests into
//! `src/platform/token_store.rs`.** Each tree scan reads every `.rs` file under
//! `src/platform/` and fails one for naming a forbidden layer, or a construct that would
//! print a credential; each non-vacuity proof demonstrates those forbidden samples as
//! **string literals**, which a comment stripper does not remove. Inside that directory a
//! proof would be found by its own scan, in the very file the two of them are written in —
//! so the suite would fail on the fact that it can still detect a violation. Their
//! vocabulary stays with them for the same reason: half a scanner is no scanner.
//!
//! # Why there is no keychain in CI, and what replaces it
//!
//! **`AGENTS.md` §4.3 requires hermetic tests, and a CI runner has no credential
//! service.** So every flow here runs against [`InMemoryTokenStore`], which is a real
//! implementation of the trait rather than a mock of it — it has the same
//! synchronisation, the same idempotent `clear`, and the same error shape.
//!
//! **The one thing this suite deliberately does not do is write to a developer's real
//! credential store.** A round-trip would prove the write path against the real API,
//! and it would also mean `cargo test` transiently replaces the session of anyone who
//! happens to be signed in on that machine, leaving a stray entry behind if the run
//! dies mid-test. The write path is covered by the in-memory store instead, and the two
//! classifications that could plausibly be wrong against the real API — `NoEntry` meaning
//! "no stored credential", and `NoEntry` on delete meaning "already clear" — are
//! asserted directly in `src/platform/token_store.rs`, because both are read-only.
//!
//! # How the stale credential is produced without faking anything
//!
//! **A real session, taken from a real server and then revoked through that server's own
//! logout route.** `support::revoke` exists for exactly this. The alternative — writing a
//! bogus string into the store — would test that the client notices a token shaped like
//! nonsense rather than that it notices *the server refusing it*, which is the claim
//! decision 3 of `6b-keychain.md` is about.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Entity, Focusable, TestAppContext, VisualTestContext};
use sh_nexus::app::{self, Shell, Startup};
use sh_nexus::core::models::events::ConnectionState;
use sh_nexus::platform::{InMemoryTokenStore, RefusingTokenStore, TokenStore};
use sh_nexus::state::bridge;
use sh_nexus::ui::views::login as login_view;
use sh_nexus::UNSIGNED_IN_USER;

use support::{revoke, ServerProcess, ADMIN_PASSWORD, ADMIN_USERNAME, READY_BUDGET};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// An endpoint that names nothing, for the tests that never reach the network.
///
/// **Not a real port and not a real host**, so a test that accidentally starts a
/// transport against it fails loudly rather than quietly connecting somewhere. Every test
/// that needs a server starts one.
const UNREACHABLE: &str = "ws://127.0.0.1:1/ws";

/// Installs the application state and returns the producer handle.
fn installed(cx: &mut TestAppContext) -> bridge::EventSender {
    cx.update(|cx| bridge::install(cx, UNSIGNED_IN_USER))
        .expect("the first install must succeed")
}

/// Builds the shell with a credential store already in it.
///
/// **The production order, then one extra call.** `app::open` installs the state before a
/// window exists and hands the store to the shell before it enters the startup phase, so a
/// fixture that did it in the other order would be testing a window no launch produces.
fn shell_with_store(
    cx: &mut TestAppContext,
    store: Arc<dyn TokenStore>,
) -> (Entity<Shell>, &mut VisualTestContext) {
    let sender = installed(cx);
    // `add_window_view` builds the root view by value, which is why the store is handed
    // over inside the closure rather than through `Entity::update` afterwards: this is the
    // same order `app::open` uses — construct, then configure, then render a frame.
    cx.add_window_view(move |_, cx| {
        let mut shell = Shell::new(sender, cx);
        shell.use_credential_store(store);
        shell
    })
}

/// Lets the shell's drain pump run once. See `tests/login.rs`'s fixture for why
/// `advance_clock` rather than `run_until_parked`.
fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(app::DRAIN_INTERVAL);
    cx.run_until_parked();
}

/// Polls the shell's own schedule until `ready` holds, or reports what it saw.
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
                "timed out after {READY_BUDGET:?} waiting for {what}; the window is \
                 resuming={}, form={}, session={}, socket={}",
                shell.read_with(cx, |shell, _| shell.is_resuming()),
                shell.read_with(cx, |shell, _| shell.login_form().is_some()),
                shell.read_with(cx, |shell, _| shell.session().is_some()),
                shell.read_with(cx, |shell, _| shell.transport().is_some()),
            );
        }
        std::thread::yield_now();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// What the connection state says, or `None` when the state is not installed.
fn connection(cx: &VisualTestContext, shell: &Entity<Shell>) -> Option<ConnectionState> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.connection().clone())
    })
}

/// Puts the window into the startup phase for `endpoint`.
fn begin_resume(cx: &mut VisualTestContext, shell: &Entity<Shell>, endpoint: &str) {
    assert!(
        shell.update_in(cx, |shell, _window, cx| shell.begin_resume(endpoint, cx)),
        "the first `begin_resume` on a fresh shell must be accepted"
    );
}

// ---------------------------------------------------------------------------
// The decision, as a value
// ---------------------------------------------------------------------------

/// The startup phase resolves to exactly two modes, and to the right one.
///
/// **The decision is a pure function so it can be asserted without a window, without a
/// server and without a store.** `AGENTS.md` §4.3 asks for independent tests, and this is
/// what independence looks like for a branch: the two arms are checked directly, and the
/// integration tests below check that the *window* ends up in the mode this function chose.
#[test]
fn the_resumed_window_opens_in_one_of_two_modes_and_only_two() {
    assert_eq!(
        app::resume_with("ws://127.0.0.1:8484/ws", Some("a-session-credential")),
        Startup::Connected(app::ConnectionSettings::new(
            "ws://127.0.0.1:8484/ws",
            "a-session-credential",
        )),
        "**a stored credential must produce the connected state, built by the one \
         constructor the environment path uses** -- two ways of building it would be two \
         answers to \"is this client configured\""
    );

    assert_eq!(
        app::resume_with("ws://127.0.0.1:8484/ws", None),
        Startup::Login {
            endpoint: "ws://127.0.0.1:8484/ws".to_owned(),
        },
        "and nothing stored must produce the form, which is the ordinary first launch"
    );
}

// ---------------------------------------------------------------------------
// The startup phase, in a real window
// ---------------------------------------------------------------------------

/// The startup phase is a third mode that shows neither the chat nor the form.
///
/// **The decision *which* view to show depends on a store that has not answered yet, so
/// there is a phase before there is a view.** The claim worth making is the negative one: a
/// window that painted the chat would be telling a person they are already signed in, and
/// one that painted the form would ask them to sign in while it is reading the store out
/// from under them. Both are asked of the *element tree* rather than of a field, which is
/// why `debug_bounds` is the assertion and not a private accessor.
#[gpui::test]
fn the_startup_phase_draws_neither_the_chat_nor_the_login_form(cx: &mut TestAppContext) {
    let store = Arc::new(InMemoryTokenStore::new());
    let (shell, cx) = shell_with_store(cx, store);

    // **No tick.** The pump only runs when the clock is advanced, so the window is still in
    // the phase it was put in — which is the state under test, and reaching it reliably is
    // the reason the assertion does not poll.
    begin_resume(cx, &shell, UNREACHABLE);
    cx.run_until_parked();

    assert!(
        shell.read_with(cx, |shell, _| shell.is_resuming()),
        "the window must still be in the startup phase before the store has answered"
    );
    assert!(
        cx.debug_bounds("message-list").is_none(),
        "**the chat log must not be painted during the startup phase** -- the window has no \
         session yet and showing one would be a claim it cannot back"
    );
    assert!(
        cx.debug_bounds(login_view::SELECTOR).is_none(),
        "and neither must the login form, which would ask for a credential the client may \
         not need"
    );
}

/// An empty store resolves the startup phase into the login form.
///
/// **The ordinary first launch, and the arm every test above depends on being reachable.**
/// The endpoint names nothing, so this also proves the resolution does not need a server:
/// deciding to show a form is a local decision.
#[gpui::test]
fn an_empty_credential_store_opens_the_login_form(cx: &mut TestAppContext) {
    let (shell, cx) = shell_with_store(cx, Arc::new(InMemoryTokenStore::new()));

    begin_resume(cx, &shell, UNREACHABLE);
    pump_until(cx, &shell, "the login form", |cx, shell| {
        shell.read_with(cx, |shell, _| shell.login_form().is_some())
    });

    assert!(
        !shell.read_with(cx, |shell, _| shell.is_resuming()),
        "and the window must have left the startup phase"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.session().is_none()),
        "with no session: nothing was stored, so there is nothing to be connected with"
    );
    assert!(
        cx.debug_bounds("message-list").is_none(),
        "and the chat must still not be painted behind the form"
    );
}

/// A stored credential opens the chat with no login form at all.
///
/// **Acceptance criterion 1, end to end: sign in once, reopen, no prompt.** The credential
/// is a real session minted by a real `POST /auth/login` against a real server process,
/// and the assertion that matters is the strongest one available — the socket reaches
/// `Connected`, so the restored credential was accepted by the peer and not merely carried
/// into a `ConnectionSettings`. A shell that showed the chat over a token-shaped string
/// would fail here.
#[gpui::test]
fn a_stored_credential_opens_the_chat_with_no_login_form(cx: &mut TestAppContext) {
    let server = ServerProcess::start("keychain-restore");
    let endpoint = server.endpoint();
    let stored = server.token();

    let store = Arc::new(InMemoryTokenStore::holding(stored));
    let (shell, cx) = shell_with_store(cx, store);

    begin_resume(cx, &shell, &endpoint);
    pump_until(
        cx,
        &shell,
        "the restored session to open a socket",
        |cx, shell| connection(cx, shell) == Some(ConnectionState::Connected),
    );

    assert!(
        shell.read_with(cx, |shell, _| shell.login_form().is_none()),
        "**a remembered session must not put the login form on screen** -- that is the \
         whole difference between a login screen and a desktop client"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.session().is_some()),
        "and the window must hold the session the store gave it"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.session_was_restored()),
        "flagged as restored, because that is what tells the shell to clear it if the \
         server refuses it"
    );
    assert!(
        cx.debug_bounds("message-list").is_some(),
        "and the chat must actually be painted, not merely built"
    );
}

/// A stored credential the server refuses is cleared, and the login form appears.
///
/// **Acceptance criterion 2 and decision 3 of `6b-keychain.md`, and it exists because of a
/// trap rather than a feature.** An app that treats "I have a session" as final shows a
/// chat that can never connect and **never offers the login form** — the user is stuck with
/// no way back except deleting the credential by hand. So a rejected restored credential is
/// cleared and the form takes its place.
///
/// **The credential is stale because the server revoked it, not because a test wrote a bad
/// string.** `support::revoke` posts to the server's own logout route, so what the client
/// meets is the same 401 a user's expired session would produce.
#[gpui::test]
fn a_rejected_stored_credential_is_cleared_and_the_login_form_appears(cx: &mut TestAppContext) {
    let server = ServerProcess::start("keychain-stale");
    let endpoint = server.endpoint();
    let stale = server.token();
    revoke(&server, &stale);

    let store = Arc::new(InMemoryTokenStore::holding(stale));
    let (shell, cx) = shell_with_store(cx, store.clone());

    begin_resume(cx, &shell, &endpoint);
    pump_until(
        cx,
        &shell,
        "the login form to replace the refused session",
        |cx, shell| shell.read_with(cx, |shell, _| shell.login_form().is_some()),
    );

    let shown = shell.read_with(cx, |shell, cx| {
        shell
            .login_form()
            .map(|form| form.read_with(cx, |form, _| form.endpoint().to_owned()))
    });
    assert_eq!(
        shown.as_deref(),
        Some(endpoint.as_str()),
        "the form must be the one for this server, or the user would be signing in \
         somewhere else"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.session().is_none()),
        "**and the refused session must be gone** -- keeping it would leave the client \
         holding a credential it has been told is worthless"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.transport().is_none()),
        "and so must the socket: the form and a refused socket together is the state this \
         path exists to leave"
    );
    assert!(
        !shell.read_with(cx, |shell, _| shell.session_was_restored()),
        "and the window must not still consider itself restored, or it would clear the \
         credential of the *next* sign-in as well"
    );

    // The store is the other half of the claim, and it is reached from the test's own thread
    // rather than the window's: the clear runs on a worker, so it is polled and not waited
    // on.
    support::wait_until(
        "the refused credential to leave the store",
        READY_BUDGET,
        || matches!(store.load(), Ok(None)),
    );
}

/// A store that refuses to keep the credential leaves a working session behind.
///
/// **The asymmetry the specification asks for: losing a remembered credential is
/// recoverable, losing the login screen is not.** The sign-in is driven through the real
/// path — form, seam, HTTP, real server — with a store that fails every operation, so the
/// assertion is that nothing about the *session* changed: the window left the form, the
/// socket connected, and the state says a send would reach the wire.
#[gpui::test]
fn a_login_the_store_refuses_to_keep_still_leaves_a_working_session(cx: &mut TestAppContext) {
    let server = ServerProcess::start("keychain-refuses");
    let endpoint = server.endpoint();
    let store = Arc::new(RefusingTokenStore::new("the credential service is locked"));
    let (shell, cx) = shell_with_store(cx, store);

    assert!(
        shell.update_in(cx, |shell, _window, cx| shell.enter_login(&endpoint, cx)),
        "a fresh shell accepts the login mode"
    );
    shell.update_in(cx, |shell, window, cx| {
        let form = shell.login_form().expect("a shell in the login mode");
        form.update(cx, |form, cx| {
            form.set_username(ADMIN_USERNAME, cx);
            form.set_password(ADMIN_PASSWORD, cx);
        });
        form.read(cx).focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");

    pump_until(cx, &shell, "the socket to connect", |cx, shell| {
        connection(cx, shell) == Some(ConnectionState::Connected)
    });

    assert!(
        shell.read_with(cx, |shell, _| shell.session().is_some()),
        "**the session must survive a store that could not keep it** -- the credential is \
         still valid and the user is still looking at a window"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.login_form().is_none()),
        "and the window must have left the login mode: a refusal is not a reason to make \
         somebody sign in twice"
    );
    assert_eq!(
        shell.read_with(cx, |_shell, app| bridge::try_read(app, |state| state
            .can_send())),
        Some(true),
        "and the state must agree the connection can carry a send, because §7.3's \
         composer's whole question is `can_send`"
    );
    assert!(
        cx.debug_bounds("message-list").is_some(),
        "with the chat painted"
    );
}

// ---------------------------------------------------------------------------
// The scanners
// ---------------------------------------------------------------------------

/// Every `.rs` file under a directory, recursively.
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

/// The client's `src/` directory.
fn source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src/platform/`.
fn platform_files() -> Vec<PathBuf> {
    let directory = source_dir().join("platform");
    let files = source_files_under(&directory);
    assert!(
        !files.is_empty(),
        "no Rust files found under {} -- the scans below are looking in the wrong place",
        directory.display()
    );
    files
}

/// A file's source with every comment removed, `//!` included.
///
/// **The `//!` handling is load-bearing**, for the reason `tests/login.rs`'s copy of this
/// function records: a stripper that tests only for `//` sees the lone `/` of a module doc,
/// decides it is division, and leaves the prose behind — so `platform/`'s own documentation
/// would be scanned as code.
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

/// Paths that must never appear anywhere under `platform/`, in any spelling.
///
/// **A token scan rather than an import allow-list, and the reason is that this layer has
/// exactly one forbidden direction and three spellings of it.** `platform/` may reach
/// `core`, `errors`, `state` and itself; it may not reach `ui` or `network`. A deny-list of
/// four tokens also catches a fully qualified path written inline, which no import list
/// would show.
const PLATFORM_FORBIDDEN_PATHS: [&str; 4] = [
    "crate::ui",
    "crate::network",
    "sh_nexus::ui",
    "sh_nexus::network",
];

/// Constructs that must never appear under `platform/`, and why each one is a leak.
///
/// **A derived `Debug` is the one that matters, and it is why the rule is *derives none*
/// rather than *has none*.** Every value this layer holds is a credential or the absence of
/// one, so `#[derive(Debug)]` anywhere in it would compile and print the value — while a
/// hand-written `Debug` is allowed, and the two in this layer are asserted by
/// `a_stored_credential_never_reaches_a_debug_or_a_display` in
/// `src/platform/token_store.rs`.
const PLATFORM_FORBIDDEN_CONSTRUCTS: [&str; 3] = ["derive(Debug", "println!(", "eprintln!("];

/// Whether one trimmed line names a credential inside a log call, and which word.
fn credential_in_log_line(trimmed: &str) -> Option<&'static str> {
    let is_log = trimmed.contains("info!(")
        || trimmed.contains("warn!(")
        || trimmed.contains("error!(")
        || trimmed.contains("debug!(")
        || trimmed.contains("trace!(")
        || trimmed.contains("event =");
    if !is_log {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    ["token", "password"]
        .into_iter()
        .find(|word| lowered.contains(word))
}

/// `platform/` reaches neither `ui/` nor `network/`.
///
/// **Acceptance criterion 5, enforced rather than documented.** `AGENTS.md` L88 puts
/// `platform/` beside the UI layer, and §3.2's separation is what stops a credential store
/// from growing opinions about the window or about the socket. The direction that would be
/// genuinely hard to debug is the second one: a `platform/` that reached `network/` could
/// decide to refresh a credential over the wire, and `state/bridge.rs` is the only module
/// allowed to do that.
#[test]
fn the_platform_layer_reaches_no_ui_and_no_network() {
    for file in platform_files() {
        let stripped = stripped_source(&file);
        for forbidden in PLATFORM_FORBIDDEN_PATHS {
            assert!(
                !stripped.contains(forbidden),
                "{} names `{forbidden}`. AGENTS.md L88 puts platform/ beside ui/, and \
                 §3.2's separation is what stops a credential store from growing opinions \
                 about the window or the socket. Forbidden here: {PLATFORM_FORBIDDEN_PATHS:?}",
                file.display()
            );
        }
    }
}

/// That boundary can still catch a violation, and admits what it must.
///
/// **A green tree proves only that nothing *currently* violates the rule.** The rejected
/// samples are the imports a future author would actually write — including the fully
/// qualified one no import list would show — and the admitted ones are the imports this
/// layer legitimately has, so a tightening that broke one of them fails here rather than in
/// a build nobody expected.
#[test]
fn the_platform_boundary_is_not_vacuous() {
    for violation in [
        "use crate::ui::Colors;",
        "use crate::network::rest::Session;",
        "use sh_nexus::ui::views::login::LoginView;",
        "let colors = crate::ui::Colors::dark();",
    ] {
        let stripped = without_comments(violation);
        assert!(
            PLATFORM_FORBIDDEN_PATHS
                .iter()
                .any(|forbidden| stripped.contains(forbidden)),
            "`{violation}` reaches a layer platform/ must not touch, but the check found \
             nothing -- the scan is vacuous and every test built on it checks nothing"
        );
    }

    for permitted in [
        "use std::sync::Arc;\n",
        "use keyring::Entry;\n",
        "use crate::platform::TokenStore;\n",
        "use crate::core::models::events::ConnectionState;\n",
    ] {
        let stripped = without_comments(permitted);
        assert!(
            !PLATFORM_FORBIDDEN_PATHS
                .iter()
                .any(|forbidden| stripped.contains(forbidden)),
            "`{permitted}` is a legitimate platform/ import and must be admitted"
        );
    }
}

/// No source under `platform/` can print a credential, by derivation or by log line.
///
/// **Acceptance criterion 4 at the source.** The log half is `AGENTS.md` §7.5 applied to the
/// new module, and it is checked here as well as by `tests/login.rs` because that suite
/// scans the tree it grew up with; this one scans the layer that arrived afterwards.
#[test]
fn no_platform_source_can_print_a_credential() {
    for file in platform_files() {
        let stripped = stripped_source(&file);

        for construct in PLATFORM_FORBIDDEN_CONSTRUCTS {
            assert!(
                !stripped.contains(construct),
                "{} contains `{construct}`. Every value this layer holds is a credential or \
                 the absence of one, so a derived `Debug` is a leak that compiles, and \
                 §7.1 bans printing to a stream in production. Write the `Debug` out by \
                 hand and assert what it prints.",
                file.display()
            );
        }

        for line in stripped.lines() {
            let trimmed = line.trim();
            if let Some(word) = credential_in_log_line(trimmed) {
                panic!(
                    "{} logs `{word}`: {trimmed}\nAGENTS.md 7.5 says never log credentials \
                     -- ids, sizes and outcomes only. Name the *operation* and the \
                     *outcome*, never the value.",
                    file.display()
                );
            }
        }
    }
}

/// That scan can still fail, and prose about the rule is admitted.
///
/// **Same reasoning as [`the_platform_boundary_is_not_vacuous`].** Each construct is
/// demonstrated on a synthetic source through the *same* predicate the tree check uses, and
/// prose that discusses the rule is asserted not to trip it — which is what lets
/// `platform/` document this at length instead of the fix being "delete the
/// documentation".
#[test]
fn the_credential_printers_are_not_vacuous() {
    for violation in [
        "#[derive(Debug)]\npub struct Slot { inner: Mutex<Option<String>> }",
        "#[derive(Debug, Clone)]\npub enum Found { Present(String) }",
        "println!(\"{credential}\");",
        "eprintln!(\"the credential is gone\");",
    ] {
        let stripped = without_comments(violation);
        let found = PLATFORM_FORBIDDEN_CONSTRUCTS
            .iter()
            .find(|construct| stripped.contains(**construct));
        assert!(
            found.is_some(),
            "`{violation}` reaches a construct the rule rejects, but the check found \
             nothing -- the scan fails open, which is the one failure mode a source scan \
             cannot be caught by"
        );
    }

    for permitted in [
        "impl fmt::Debug for StoredCredential { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(\"Absent\") } }",
        "/// This type derives no `Debug`, and the reason is that it holds one.",
    ] {
        let stripped = without_comments(permitted);
        assert!(
            !PLATFORM_FORBIDDEN_CONSTRUCTS
                .iter()
                .any(|construct| stripped.contains(*construct)),
            "`{permitted}` must be admitted: it is either a hand-written impl or prose \
             about the rule, and a scanner that flagged the module's own documentation \
             would have been \"fixed\" by deleting it"
        );
    }

    assert_eq!(
        credential_in_log_line(
            "tracing::warn!(reason = %error, \"the credential could not \
                                 be read\");"
        ),
        None,
        "a log line naming the reason is admitted -- the reason is an operating-system \
         diagnostic, not a secret"
    );
    for violation in [
        "tracing::warn!(token = %credential, \"stored\");",
        "tracing::info!(password = %password, \"typed\");",
    ] {
        assert!(
            credential_in_log_line(violation).is_some(),
            "`{violation}` names a credential in a log field and must be rejected"
        );
    }
}
