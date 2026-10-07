//! One HTTP POST: the login request `network::rest` makes, and every answer it
//! has to read correctly.
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
//! **This is the HTTP half of that milestone.** The work unit shipped as chained
//! PRs along the architecture's own seam — `network/` and `ui/` are separable, and
//! `tests/layer_boundary.rs` is what enforces it — so this file carries the tests
//! that need nothing but `network::rest`: the endpoint translation, the status and
//! body mappings, the size ceiling and the login path. The window, the view, the
//! real-server flows and the password-rule scans arrive in the second half, which
//! extends this same file rather than opening another one.
//!
//! | Test | What it proves | Kind |
//! |---|---|---|
//! | [`an_endpoint_asking_for_tls_is_refused_rather_than_downgraded`] | the ADR-012 coupling, made loud | pure |
//! | [`the_websocket_endpoint_is_translated_into_an_http_login_url`] | the five shapes a configured URL can take | pure |
//! | [`a_success_body_without_a_token_is_not_reported_as_a_refused_credential`] | the one mapping a careless client gets wrong | pure |
//! | [`a_refusal_body_is_read_as_the_servers_own_code_and_sentence`] | a refusal keeps the server's own code and sentence | pure |
//! | [`a_success_body_yields_a_session_and_an_absent_advisory_field_is_not_a_failure`] | advisory fields stay advisory | pure |
//! | [`a_body_past_the_ceiling_is_refused_rather_than_buffered`] | `AGENTS.md` §7.1's ban on unbounded state, applied to a peer | pure |
//! | [`the_login_path_this_client_posts_to_is_the_one_the_suite_uses`] | the one duplicated constant, watched | pure |
//!
//! # Why the server runs as a child process
//!
//! `tests/support/mod.rs` gives the argument in full: a `[dev-dependencies]` entry
//! naming `sh_nexus_server` would put `axum`, `rusqlite` and SQLite's C amalgamation
//! into `cargo tree -p sh_nexus`, which is the mistake ADR-002 exists to prevent.
//! Running the binary exercises the **shipped** server, which is a stronger claim
//! than a library harness would be.
//!
//! **This half starts no server**, and says so rather than carrying fixtures it
//! never calls: the only thing it takes from `mod support` is `support::LOGIN_PATH`,
//! the test-side copy of the server's own constant, which the last test below
//! exists to compare against `rest::LOGIN_PATH`.

mod support;

use rstest::rstest;
use sh_nexus::network::rest::{self, LoginError, LOGIN_PATH, MAX_RESPONSE_BYTES};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

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
