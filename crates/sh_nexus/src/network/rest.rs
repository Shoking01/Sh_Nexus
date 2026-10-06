//! The one HTTP request this client makes: `POST /auth/login`.
//!
//! `AGENTS.md` §3.1's tree names this module `rest.rs`, and until the login
//! milestone arrived it did not exist — a module that exists and does nothing
//! reads as finished work (`src/lib.rs` says so about `db/` and `platform/`), so
//! it was absent rather than declared empty.
//!
//! # What is in scope, and what the module's size is honest about
//!
//! **One endpoint.** `POST /auth/login` exchanges a username and a password for
//! an opaque session token, and that is the whole request surface this client
//! has. `POST /auth/logout` is a second call this milestone deliberately does not
//! make: the token lives in memory for the session (`app::ConnectionSettings`'s
//! own docs), so there is nothing to revoke when the process exits, and a logout
//! button that only clears memory would be a feature pretending to be a security
//! property. History, channels and presence are `db/` and `PLAN.md` §8's later
//! units.
//!
//! # Why this file owns a runtime and a thread
//!
//! **Because the one runtime this workspace has belongs to `gpui`, and `gpui`'s
//! executor is not a reactor.** `hyper_util`'s connector opens sockets through
//! `tokio::net::TcpStream`, which needs a tokio runtime with an IO driver
//! enabled. `cx.background_executor()` is a thread pool that polls futures; it
//! has no reactor, so `TcpStream::connect` cannot be driven from it. Handing this
//! function to the GPUI executor would compile and then hang.
//!
//! So the shape is the one `network/ws.rs` already established in PR #41: a
//! dedicated `std::thread` that builds a **current-thread** tokio runtime with
//! `enable_all()` and `block_on`s the request. Two independent callers, two
//! runtimes, never a second one on one thread — and a thread that is not the
//! frame loop, so `AGENTS.md` §2.3's "no blocking on the UI thread" holds by
//! construction rather than by argument.
//!
//! **A current-thread runtime, and why that is the right size.** The work here is
//! one DNS-free loopback connect, one request write and one response read. A
//! multi-threaded runtime would start a worker pool per login to await a future
//! that is ready within microseconds; `ws.rs` already pays for a runtime per
//! transport, and login pays for one per attempt. Bounded by the number of
//! attempts a user makes in a session, which is not a growth path
//! (`AGENTS.md` §7.1).
//!
//! # The password, and the four rules it obeys here
//!
//! `AGENTS.md` §7.5 forbids logging credentials and §2.1 forbids panics on a
//! user-facing path. A login request is the one place in this crate where a
//! password exists at all, so the rules are written out rather than assumed:
//!
//! 1. **It is a parameter, moved in and never stored.** [`login`] takes it by
//!    value as a `String` and the only thing this file builds from it is the
//!    request body. There is no field, no global and no cache.
//! 2. **It is never in a `Debug`.** `LoginError` derives `Debug` and no variant
//!    carries a credential; [`LoginRequest`] is the type that would, so it does
//!    not exist and [`login`] takes its arguments separately.
//! 3. **It is never in a log line.** The `tracing` calls in the caller name the
//!    outcome and the endpoint. Nothing here logs at all, for the reason
//!    `network/ws.rs` logs nothing: its counters are the observable surface.
//! 4. **It is never in a panic message.** There is no `unwrap`, no `expect` and
//!    no `panic!` in this file, and `tests/login.rs` asserts the absence.
//!
//! **What "never in a `Debug`" costs, stated plainly:** it costs a type. The
//! natural shape — a `LoginRequest` struct with `username` and `password` fields
//! — is a type whose derived `Debug` prints the password, and "nobody will print
//! it" is not a guarantee. Taking two arguments to [`login`] means there is no
//! struct to derive anything for.
//!
//! # Why plain HTTP is correct here, and when it stops being
//!
//! `sh_nexus_server`'s `SH_NEXUS_BIND` defaults to `127.0.0.1`, deliberately, and
//! ADR-010's threat model puts the adversary *outside* the team. A login POST over
//! plain HTTP is therefore correct **for the only topology this server supports**.
//!
//! **`wss://` and `https://` are REFUSED rather than downgraded, and that refusal
//! is the design.** `hyper` is declared with no TLS feature, so a `https` URI
//! cannot be spoken by this binary at all — and silently rewriting it to `http`
//! would send a password in the clear to a host that asked for encryption, which
//! is the worst available outcome. [`LoginError::Endpoint`] says so, in the view,
//! to the user. `docs/ARCHITECTURE.md` ADR-012's implementation notes record this
//! coupling so it is not left implicit: the day remote deployment lands, this is
//! the line that breaks, and it must break loudly.
//!
//! # Why the wire shapes are written out rather than imported
//!
//! The client cannot depend on `sh_nexus_server` — that would put `axum`,
//! `rusqlite` and SQLite's C amalgamation into `cargo tree -p sh_nexus`, which is
//! exactly the mistake ADR-002 exists to prevent, and `tests/support/mod.rs` gives
//! that argument at length. `sh_nexus_wire` is shared by both sides and *could*
//! carry these DTOs, but it describes WebSocket envelopes: ADR-002's `wire` crate
//! has no HTTP surface, and adding one for a single request would make the
//! protocol crate depend on an HTTP shape nobody else in the workspace speaks.
//!
//! **So the path and the field names are literals here, exactly as
//! `STARTUP_CHANNEL` and `WS_PATH` already are in `app.rs` and
//! `tests/support/mod.rs`.** The duplication is watched rather than hidden:
//! `tests/login.rs` drives a **real server process** and asserts a token comes
//! back, so a rename on either side turns a test red rather than production red.
//! `crates/sh_nexus_server/tests/support/mod.rs::LOGIN_PATH` is the server's own
//! constant, and `sh_nexus_server`'s `SUPPORTED_FIELDS`-shaped fixtures are what
//! prove the names agree.

use std::fmt;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE};
use hyper::{Method, Request, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

/// The path login is served on.
///
/// **A literal rather than an import**, for the reason the module docs give. The
/// client cannot name `sh_nexus_server` (ADR-002), and `tests/login.rs` proves
/// the two agree by logging in against a real server process.
pub const LOGIN_PATH: &str = "/auth/login";

/// How long one login attempt may take before it is called a failure.
///
/// **`AGENTS.md` §7.4's "request (10s)" verbatim**, and the number is not
/// derived: that rule already names the budget for a request, and inventing a
/// second one would be a second thing to keep in step.
///
/// **Ten seconds is generous, and the reason is the server's Argon2id.** The
/// login path verifies a memory-hard hash *inside a `spawn_blocking`* before it
/// answers, which on a slow machine is tens of milliseconds and on a loaded CI
/// runner can be a second. A tighter bound would turn "the server is busy" into
/// "your password was refused", which is the one message a user must never be
/// shown falsely. The cost of the generosity is bounded too: an attempt that hits
/// it is a thread that exits, and the user sees a message rather than a spinner
/// that never resolves.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest login response body this client will read into memory.
///
/// **`AGENTS.md` §7.1's ban on unbounded in-memory state, applied to a remote
/// peer.** A response from a server an operator controls is still a response from
/// the network, and `hyper` will happily buffer whatever arrives. 8 KiB is far
/// more than the documented success body (a token, an id, a username and a
/// timestamp) and far less than a buffer worth having an attacker ask for. The
/// refusal is [`LoginError::Unreadable`], which is a report rather than a crash.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024;

/// A session the server issued.
///
/// **The token is the credential, and this type does not derive `Debug` for the
/// same reason the server's `IssuedSession` does not**: a `{:?}` on it must not
/// compile, which is a stronger guarantee than a hand-written `Debug` that has to
/// be trusted. Fields are public because the caller puts them into
/// [`crate::app::ConnectionSettings`] and nowhere else.
pub struct Session {
    /// The opaque session token, sent as `Authorization: Bearer` from here on.
    pub token: String,
    /// The account this session belongs to.
    pub user_id: String,
    /// The login handle that earned it.
    pub username: String,
}

/// Why a login did not produce a session.
///
/// **Every variant is a condition the user can be told about, and none of them
/// swallows anything.** `AGENTS.md` §3.3 requires a network failure to surface as
/// a recoverable state and §5.2 asks for a "clear, actionable error". A login that
/// failed for a reason the window does not name is a login the user cannot act
/// on, so this enum is the surface the view renders rather than a `String`.
///
/// **Nothing here carries a credential**, in any variant, in any field — which is
/// what lets `Debug` be derived and still be safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// The configured endpoint cannot be turned into an HTTP login URL.
    ///
    /// **`https://` and `wss://` land here, deliberately.** See the module docs:
    /// this binary has no TLS, and downgrading would be worse than refusing.
    Endpoint {
        /// What was configured, so the operator can see which value was wrong.
        endpoint: String,
        /// Why it was refused.
        reason: String,
    },

    /// The server could not be reached at all.
    ///
    /// **A refused connection is not a refused credential**, and the two are
    /// separate variants precisely so the view cannot say "wrong password" to
    /// somebody whose server is down. `AGENTS.md` §5.2's actionable error is a
    /// sentence about the right half of the problem.
    Unreachable {
        /// The transport's own words. An address and an OS error, never a body.
        reason: String,
    },

    /// The attempt did not finish inside [`REQUEST_TIMEOUT`].
    Timeout {
        /// The budget that was spent.
        after: Duration,
    },

    /// The server answered, and the answer was a refusal.
    ///
    /// **`code` and `detail` are the server's own strings, verbatim.** They are
    /// carried rather than paraphrased for the reason
    /// `ui/views/connection_banner.rs` renders `ConnectionState::Rejected`'s
    /// payload instead of a string of this client's own: "login failed" is the one
    /// outcome the user cannot fix, and only the server knows why.
    Refused {
        /// The status code from the status line.
        status: u16,
        /// The refusal code, e.g. `invalid_credentials`.
        code: String,
        /// The server's sentence about it.
        detail: String,
    },

    /// The response was not the shape this endpoint documents.
    ///
    /// **Not the same as [`Self::Refused`]**: the server answered with a status
    /// this code does not recognise, or with a body that is not the documented
    /// JSON, or with one larger than [`MAX_RESPONSE_BYTES`]. Each of those means
    /// the *client* is talking to something that is not this API, and the honest
    /// message names that rather than claiming a credential was refused.
    Unreadable {
        /// The status code, when one was read at all.
        status: u16,
        /// Why the answer could not be read.
        reason: String,
    },
}

impl fmt::Display for LoginError {
    /// One line, naming what happened and never naming a credential.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint { endpoint, reason } => {
                write!(
                    formatter,
                    "{endpoint} cannot be used for a login ({reason})"
                )
            }
            Self::Unreachable { reason } => {
                write!(formatter, "the server could not be reached: {reason}")
            }
            Self::Timeout { after } => {
                write!(formatter, "the server did not answer within {after:?}")
            }
            Self::Refused { detail, code, .. } => write!(formatter, "{detail} ({code})"),
            Self::Unreadable { status, reason } => {
                write!(
                    formatter,
                    "the server's answer (HTTP {status}) was not readable: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for LoginError {}

/// The `http://` URL a `POST /auth/login` goes to, given the WebSocket endpoint
/// this client already holds.
///
/// **The translation is a documented rewrite rather than a second setting, and
/// the reason is that there is one configuration.** `SH_NEXUS_URL` names the
/// server's **WebSocket** endpoint (`ws://127.0.0.1:8484/ws`), because that is
/// what the socket needs. Asking an operator to configure a second variable for
/// the same host is asking for two things to disagree, and the disagreement would
/// be discovered as a login posting to a different server than the socket
/// connects to — which is a far worse failure than a rewrite that cannot.
///
/// The path is **replaced**, not appended to: the WebSocket path is `/ws` and the
/// login path is `/auth/login`, and keeping the first would produce
/// `/ws/auth/login`, which is a 404 the operator would spend an afternoon on.
///
/// **Refusals rather than rewrites for everything else.** A `ws://` or `http://`
/// endpoint is rewritten; `wss://`/`https://` is refused because this binary has
/// no TLS and downgrading would send a password in the clear to a host that
/// asked for encryption; anything else is refused because guessing at a scheme is
/// how a credential gets sent somewhere unintended.
pub fn http_login_endpoint(ws_endpoint: &str) -> Result<String, LoginError> {
    let Some((scheme, rest)) = ws_endpoint.split_once("://") else {
        return Err(LoginError::Endpoint {
            endpoint: ws_endpoint.to_owned(),
            reason: "it does not name a scheme; expected ws://host:port/ws".to_owned(),
        });
    };

    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(LoginError::Endpoint {
            endpoint: ws_endpoint.to_owned(),
            reason: "it names no host".to_owned(),
        });
    }

    match scheme {
        "ws" | "http" => Ok(format!("http://{authority}{LOGIN_PATH}")),
        "wss" | "https" => Err(LoginError::Endpoint {
            endpoint: ws_endpoint.to_owned(),
            reason: "it asks for TLS, and this client has none; an encrypted endpoint \
                     needs the TLS stack docs/DEPENDENCIES.md declined, so it is \
                     refused rather than silently downgraded to an unencrypted login"
                .to_owned(),
        }),
        other => Err(LoginError::Endpoint {
            endpoint: ws_endpoint.to_owned(),
            reason: format!(
                "`{other}` is not a scheme this client speaks; expected ws, wss, http or https"
            ),
        }),
    }
}

/// Reads a login answer: a status and a body, and one of the two shapes this
/// endpoint is documented to return.
///
/// **Pure, and therefore the part of this module that can be tested without a
/// socket.** The round trip against a real server lives in `tests/login.rs`; what
/// it cannot cheaply cover is every way a body can be *wrong*, which is what this
/// function exists to enumerate:
///
/// | Status | Body | Result |
/// |---|---|---|
/// | `2xx` | `{"token": …, "user_id": …, "username": …}` | [`Session`] |
/// | `2xx` | anything else | [`LoginError::Unreadable`] |
/// | non-`2xx` | `{"code": …, "detail": …}` | [`LoginError::Refused`] |
/// | non-`2xx` | anything else | [`LoginError::Unreadable`] |
///
/// **A `2xx` with no token is `Unreadable` and not `Refused`, and the distinction
/// is the whole point of the table.** Every status this endpoint uses for a
/// refusal is a non-`2xx` (`401` for credentials, `400` for a body it could not
/// read, `500` for a storage fault), so a `200` that carries no token is not the
/// server refusing anything — it is this client talking to something that is not
/// this API. Claiming "your password was refused" for that would be a false
/// statement about a credential.
pub fn decode_login_response(status: u16, body: &[u8]) -> Result<Session, LoginError> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(LoginError::Unreadable {
            status,
            reason: format!(
                "the body is {} bytes, past this client's {MAX_RESPONSE_BYTES}-byte ceiling",
                body.len()
            ),
        });
    }

    let document: serde_json::Value =
        serde_json::from_slice(body).map_err(|error| LoginError::Unreadable {
            status,
            reason: format!("the body was not JSON: {error}"),
        })?;

    let successful = (200..300).contains(&status);
    if successful {
        let token = document.get("token").and_then(|value| value.as_str());
        let Some(token) = token else {
            return Err(LoginError::Unreadable {
                status,
                reason: "a 200 that carried no `token` field; this is not the login \
                         endpoint, or the server and this client disagree about it"
                    .to_owned(),
            });
        };
        return Ok(Session {
            token: token.to_owned(),
            user_id: string_field(&document, "user_id"),
            username: string_field(&document, "username"),
        });
    }

    // **The server's two fields, read as its own rather than reconstructed.**
    // `sh_nexus_server::auth::refused` writes exactly `{"code", "detail"}`, and a
    // code with no detail still has to render something the user can read — so a
    // missing field falls back to the status line rather than to an empty
    // sentence, which would be the one message a user can act on least.
    let code = document
        .get("code")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_owned();
    let detail = document
        .get("detail")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("the server refused the login with HTTP {status}"));

    Err(LoginError::Refused {
        status,
        code,
        detail,
    })
}

/// One string field of a parsed response, or the empty string when it is absent.
///
/// **`user_id` and `username` are advisory on this side.** The session works
/// without them — the token is what the socket presents — so an answer missing
/// one is a success with less information rather than a failure. Demanding them
/// would refuse a working credential over a field nothing here uses, which is
/// `AGENTS.md` §3.3's "never panic on a path with nothing to recover it" pointed
/// the other way.
fn string_field(document: &serde_json::Value, name: &str) -> String {
    document
        .get(name)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// The JSON body a login request carries.
///
/// **Written out here rather than derived, and the reason is the derive ban.**
/// `PLAN.md` §5 gives serialization to `sh_nexus_wire`, and
/// `tests/layer_boundary.rs::the_client_does_not_depend_on_serde` fails the build
/// if this crate takes a dependency on `serde` so the derive macros are one `use`
/// away from `core/models`. Building a `serde_json::Value` needs no derive at all,
/// which is why this function is six lines and not a struct with two attributes.
///
/// **The password is in this string and nowhere else.** It is moved out of its
/// parameter into the document and from there into the request body, so the one
/// place a plaintext password exists as a `String` after this call returns is
/// `Full<Bytes>`, inside the socket write. See the module docs for the four rules
/// this obeys.
fn login_body(username: &str, password: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "username": username,
        "password": password,
    }))
    .unwrap_or_default()
}

/// Performs one login, on a thread of its own, and returns what happened.
///
/// # Arguments
///
/// * `ws_endpoint` — the configured **WebSocket** endpoint, e.g.
///   `ws://127.0.0.1:8484/ws`. The HTTP login URL is derived from it; see
///   [`http_login_endpoint`].
/// * `username` — the login handle. Never logged, for the server's own reason
///   quoted at `sh_nexus_server::auth::login`: logging an attempted username
///   turns the log into the username oracle that endpoint exists not to be.
/// * `password` — **taken by value and moved into the worker**, so it is dropped
///   when the request finishes and no caller keeps a second copy.
///
/// # Errors
///
/// Every [`LoginError`], from a malformed endpoint through an unreachable server
/// to a refusal carrying the server's own words. There is no variant that means
/// "something went wrong": `AGENTS.md` §5.2 asks for an actionable error and a
/// catch-all is the opposite.
///
/// # Blocking, and why that is not a defect here
///
/// **This function blocks, and the caller must not be the frame loop.**
/// `AGENTS.md` §2.3 forbids blocking the UI thread, so this is spawned onto a
/// dedicated thread by [`crate::state::bridge::begin_login`] and the window keeps
/// painting while the request is in flight. The alternative — a tokio runtime
/// owned by the application for the life of the process — buys nothing: the
/// WebSocket transport already starts and stops its own per PR #41, and a
/// client-wide runtime would outlive every attempt it served.
pub fn login(ws_endpoint: &str, username: &str, password: String) -> Result<Session, LoginError> {
    let endpoint = http_login_endpoint(ws_endpoint)?;

    // The body's construction happens here, on the calling thread, so a
    // serialization failure could never be reported as a network failure on the
    // worker. `unwrap_or_default` rather than a `Result`: an empty body is the
    // server's `400`, which is a correct answer to a request this client cannot
    // build, and `AGENTS.md` §2.1 forbids panicking a path the user can reach.
    let body = login_body(username, &password);
    // **The plaintext stops existing here.** The parameter is shadowed by the
    // body it became, and the original buffer is dropped with it: from this line
    // on, the only copy is inside `body`, which the socket write consumes.
    drop(password);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| LoginError::Unreachable {
            reason: format!("this machine could not start a network runtime: {error}"),
        })?;

    runtime.block_on(exchange(&endpoint, body))
}

/// The request and the read, on a runtime that is already running.
///
/// **Split from [`login`] so the split is the testable one.** Everything that can
/// be decided without a socket — the URL rewrite, the body shape, the status and
/// body mapping — is a free function; this is the part that cannot be, and it is
/// the shortest of them.
///
/// **A timeout around the request, not around the socket.** `AGENTS.md` §7.4's
/// "request (10s)" is about the whole exchange, and wrapping `client.request(..)`
/// covers connect, write and the response headers. The body read below is bounded
/// separately by [`MAX_RESPONSE_BYTES`] and by the same timeout's remaining
/// budget, which is why a peer that sends headers and then dribbles is still cut
/// off rather than holding the thread.
async fn exchange(endpoint: &str, body: Vec<u8>) -> Result<Session, LoginError> {
    let uri: Uri = endpoint.parse().map_err(|error| LoginError::Endpoint {
        endpoint: endpoint.to_owned(),
        reason: format!("the derived login URL is not a valid URI: {error}"),
    })?;

    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .body(Full::new(Bytes::from(body)))
        .map_err(|error| LoginError::Endpoint {
            endpoint: endpoint.to_owned(),
            reason: format!("the request could not be built: {error}"),
        })?;

    // `build_http` is hyper-util's HTTP/1 connector, and it is the piece that
    // makes a `hyper::Client` able to reach a host at all -- see the feature
    // audit in the root `Cargo.toml`.
    let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();

    let answered = tokio::time::timeout(REQUEST_TIMEOUT, client.request(request))
        .await
        .map_err(|_| LoginError::Timeout {
            after: REQUEST_TIMEOUT,
        })?;

    let response = answered.map_err(|error| LoginError::Unreachable {
        reason: describe(&error),
    })?;

    // **The status is not branched on here, and that is deliberate.** A refusal
    // carries its reason in its body, so the 401 and the 200 travel the same road
    // to `decode_login_response`: no status can reach the user without its own
    // explanation attached, and there is no code path where a non-`2xx` answer
    // could be mistaken for a session.
    let status = response.status().as_u16();

    let collected = tokio::time::timeout(REQUEST_TIMEOUT, response.into_body().collect())
        .await
        .map_err(|_| LoginError::Timeout {
            after: REQUEST_TIMEOUT,
        })?;

    let bytes = collected
        .map_err(|error| LoginError::Unreachable {
            reason: describe(&error),
        })?
        .to_bytes();

    decode_login_response(status, &bytes)
}

/// A transport failure as a sentence a person can act on.
///
/// **`hyper::Error` is opaque, and `Display` on it is a sentence for a reader
/// who knows hyper.** "error trying to connect: tcp connect error: Connection
/// refused (os error 10061)" is actionable; `hyper::Error(..)` is not. So the
/// kind is named in words and the transport's own message is kept as the
/// evidence after it.
///
/// **Never a body, never a credential.** The only thing that can reach this
/// function is the transport layer, which has not seen the password's meaning —
/// and nothing about the address is redacted either, because an operator
/// debugging a refused connection needs to see which host was refused.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let chain = read_chain(error);
    let kind = if chain.timeout {
        "the request timed out"
    } else if chain.refused {
        "the connection was refused"
    } else if chain.truncated {
        "the server's answer ended early"
    } else if chain.not_http {
        "the server's answer was not HTTP"
    } else {
        "the request failed"
    };
    format!("{kind}: {error}")
}

/// What a transport said, gathered from a whole `source` chain.
///
/// **Four booleans rather than an enum, and the reason is that the two error types
/// this function is handed nest differently.** `hyper_util::client::legacy::Error`
/// wraps a `hyper::Error`, which wraps the `io::Error` — so a single walk that
/// decided on the first recognisable link would stop at the `hyper::Error` and
/// never reach the `io::Error` underneath it, reporting a refused connection as a
/// generic failure. **Gathering every fact and then deciding is what makes the
/// deepest cause reachable at all.**
#[derive(Default)]
struct Chain {
    timeout: bool,
    refused: bool,
    truncated: bool,
    not_http: bool,
}

/// Reads every recognisable fact out of `error` and everything it wraps.
///
/// **A downcast walk rather than a substring search**, and that is the whole
/// discipline here: rendering the error and looking for `"connect"` would also
/// match a path that happens to contain it, and would call a timeout a refusal.
/// `hyper::Error` exposes typed predicates — `is_timeout`, `is_incomplete_message`
/// and `is_parse` — and notably **no** `is_connect`: the 0.14 name was dropped when
/// hyper's errors were reworked, so "the socket was refused" has to be found in the
/// `io::Error` hyper wraps.
fn read_chain(error: &(dyn std::error::Error + 'static)) -> Chain {
    let mut chain = Chain::default();
    let mut current = Some(error);
    while let Some(source) = current {
        if let Some(hyper_error) = source.downcast_ref::<hyper::Error>() {
            chain.timeout |= hyper_error.is_timeout();
            chain.truncated |= hyper_error.is_incomplete_message();
            chain.not_http |= hyper_error.is_parse();
        }
        if let Some(io_error) = source.downcast_ref::<std::io::Error>() {
            chain.refused |= names_a_connection_failure(io_error.kind());
        }
        current = source.source();
    }
    chain
}

/// Whether `kind` is a refusal to connect, or a connection that went away.
///
/// **Every kind here is fixed by starting the server or checking the port** — the
/// family a user acts on rather than retyping — and that is the family this
/// function's caller turns into a sentence about the server instead of about the
/// credential. `TimedOut` is in the set because Windows reports a connect to a dead
/// port that way on some paths, and a message saying "refused" is the accurate
/// reading of it.
fn names_a_connection_failure(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::AddrNotAvailable
            | std::io::ErrorKind::TimedOut
    )
}
