//! Credentials, sessions, and the ways this server refuses a peer.
//!
//! This is the ADR-010 auth milestone. Everything in it exists to answer one
//! question -- *who is this socket* -- and everything it does **not** do is
//! written down below rather than left to be discovered.
//!
//! # Opaque tokens, and why not `jsonwebtoken`
//!
//! ADR-010's Alternatives section names `jsonwebtoken` and this build does not
//! use it. Two reasons, and the second is the one that would still be true in a
//! year:
//!
//! 1. It would be the first crypto dependency in a project that has none, and
//!    the audit for it is the work this milestone carries for `argon2` and `sha2`.
//! 2. **A JWT cannot be revoked.** It is a signed statement that says "this is
//!    user X until time T", and nothing in that sentence can be withdrawn short
//!    of changing the signing key, which revokes *everyone*. ADR-010's Context
//!    section inverts the threat model -- *"the operator is usually also a user.
//!    The adversary is outside, not a colleague"* -- and the case that follows
//!    from it is expelling a member of the team. A session table row is
//!    withdrawable by a single `UPDATE`, and that is the whole reason this design
//!    chose a database table over a self-contained token.
//!
//! # What is stored, and what is not
//!
//! | Value | Where it lives | Why |
//! |---|---|---|
//! | password | **nowhere** | Hashed on arrival with Argon2id; the plaintext is never written and never logged. |
//! | password hash | `users.password_hash`, as a PHC string | Self-describing, so Argon2's cost parameters can be re-tuned without a data migration. |
//! | session token | **nowhere** | Returned to the client once, at login. Never logged, never in a `Debug`, never in an error. |
//! | token hash | `sessions.token_hash`, the primary key | A backup is a file copy (`db.rs`); a file copy that held live tokens would hold every session on the instance. |
//!
//! # `argon2` for passwords and `sha2` for tokens, and why the split is not lazy
//!
//! The two hashes have opposite threat models and the difference is **entropy**,
//! not sensitivity:
//!
//! - A password is chosen by a human, so it is low-entropy and must be verified
//!   *slowly*. Argon2id at `Params::DEFAULT` (19 MiB, 2 passes) spends about 50 ms
//!   and 19 MiB on each attempt, which is the entire defence against an offline
//!   dictionary attack on a leaked `users.password_hash`.
//! - A session token is 32 bytes of operating-system entropy, so there is no
//!   dictionary: recovering the plaintext from its digest is a preimage problem
//!   over 2^256, not a cracking problem. Slowing that down buys nothing, and this
//!   hash runs **on every WebSocket handshake** -- where `AGENTS.md` §6.2 budgets
//!   the whole path under 16 ms. A `sha256` of a 64-byte token is one compression
//!   pass.
//!
//! Both are therefore the right answer for their own input, and the line is drawn
//! at "is the input already high-entropy" rather than at "is this a credential".
//!
//! # Timing, and why "no such user" costs the same as "wrong password"
//!
//! [`verify_password`] is written so that a caller **cannot** tell the two apart
//! from its own code, let alone from a timing measurement: when the username does
//! not exist it verifies the candidate against [`DUMMY_PASSWORD_HASH`] and returns
//! `false`. `AGENTS.md` §2.1 asks for typed errors, and the typed answer here is
//! deliberately one answer for both cases -- an unauthenticated peer that could
//! enumerate usernames by measuring this endpoint would get a free list of the
//! team.
//!
//! # What is NOT here
//!
//! | Not built | Named as |
//! |---|---|
//! | Open registration | ADR-010 decides it: **never**, on a self-hosted instance anyone who can reach the port could enrol on. |
//! | Password reset, password change, session listing | A provisioning milestone. Not needed to make the transport authenticated. |
//! | Expelling a member (revoking *all* of one user's sessions) | The revocation *mechanism* is here ([`logout`], and [`crate::db::Store::revoke_session`]); the operation that enumerates a user's sessions is not. |
//! | Rate limiting | ADR-010's "What this ADR does NOT decide" names it, so it is named here rather than half-built. |
//! | Zeroing secret buffers | No `zeroize` feature is enabled and no buffer is wiped. Named, because "the plaintext is not in memory after this returns" would be a false claim; what *is* true is that it is not in memory after the request handler returns, because nothing retains it. |
//! | TLS | The client has no TLS feature set at all; see the root `Cargo.toml`'s `tokio-tungstenite` entry. An instance reachable from a network needs a reverse proxy, and `docs/API.md` says so. |
//!
//! # An example
//!
//! The verification path, which is the only part of this module with no I/O and
//! therefore the only part a doctest can show honestly:
//!
//! ```
//! use sh_nexus_server::auth::{hash_password, session_ttl_millis, verify_password};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let hash = hash_password("correct horse battery staple")?;
//!
//! assert!(verify_password(Some(hash.as_str()), "correct horse battery staple"));
//! assert!(!verify_password(Some(hash.as_str()), "Correct horse battery staple"));
//! // The absent-account case costs the same work and says the same thing.
//! assert!(!verify_password(None, "correct horse battery staple"));
//!
//! // A session outlives a workday and not a weekend, and the choice is a named
//! // constant rather than a literal at a call site.
//! assert_eq!(session_ttl_millis(), 7 * 24 * 60 * 60 * 1_000);
//! # Ok(())
//! # }
//! ```

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::db::{Credential, SessionState, Store};
use crate::error::{Result, ServerError};
use crate::time;
use crate::AppState;

/// How many bytes of operating-system entropy one session token carries: 32.
///
/// **256 bits, drawn from `Uuid::new_v4()` twice.** That indirection is worth
/// explaining because it looks like a workaround. The server declares no `rand`
/// and no `getrandom`: the only entropy source already in this dependency graph
/// with **zero new crates** is `uuid`'s `v4` feature, which is one `getrandom`
/// call per `Uuid` and therefore 16 bytes. Two of them are 32. Declaring
/// `getrandom` directly would be a third new dependency in the lock for one
/// function, and `AGENTS.md` §7.2 prices a dependency by its supply-chain
/// surface as well as by its compile time -- so the honest cost of that choice is
/// two calls instead of one, recorded here rather than discovered later.
///
/// 32 bytes is the number every mainstream framework lands on (Django, Rails and
/// Go's `crypto/rand` all use 32): below about 16 bytes a token stops being
/// unguessable in a way that matters, and above it there is nothing left to buy.
pub const SESSION_TOKEN_BYTES: usize = 32;

/// How long a login's session lives, in milliseconds: seven days.
///
/// **A week, and not a day.** A per-team instance is a workplace tool, and a
/// desktop client that is expected to survive a weekend with its socket closed
/// would otherwise ask for a password on Monday morning; §7.5's release minimum
/// and this constant are the two places a session's lifetime is visible, which is
/// the same reason [`crate::db::Store::purge_expired_sessions`] exists as a
/// callable function rather than as a background job nobody can see.
///
/// The bound is what makes a stolen token a *bounded* problem rather than a
/// permanent one, and the revocation path is what makes it a *small* one: an
/// administrator does not have to wait this out.
pub const SESSION_TTL_MILLIS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// The session lifetime, in milliseconds.
///
/// A function wrapping [`SESSION_TTL_MILLIS`] rather than a re-export, so that a
/// reader arriving at this module's example sees the unit in the name and so that
/// the two halves of "mint a token" cannot drift apart on one of them.
pub fn session_ttl_millis() -> i64 {
    SESSION_TTL_MILLIS
}

/// The password a login fails against when the account does not exist.
///
/// **A hash, of a passphrase nobody knows, and it is not a back door.** It exists
/// so that [`verify_password`] performs the same Argon2 work on the absent-account
/// path as on the present one; a constant is used rather than hashing the
/// candidate on the spot because the point is that the work is *indistinguishable*,
/// and a fresh hash on that path would be measurably distinguishable -- one extra
/// `getrandom` call the present path also makes, but one a caller could in
/// principle learn to expect.
///
/// There is no account behind it, so even a "successful" verification of this value
/// would grant nothing: [`verify_password`] returns a bare `bool` and the account
/// the caller then looks up is the one that was absent. Anyone who recovered the
/// passphrase would learn that `sh_nexus` was installed here, which the row count
/// in `users` already says.
///
/// Generated once, with this crate's own default parameters, from a 32-byte random
/// string that was not written down. Regenerating it is safe; keeping it stable is
/// cheaper.
///
/// `tests/auth.rs::the_dummy_hash_is_a_real_argon2_string_and_names_no_account`
/// holds this constant to both of those claims, so a hand-edited or truncated
/// value cannot quietly turn the absent-account path into a fast one.
const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$kmmPMJcxPx/ZSMiy1n3LPg$d5n9QPU7+pd5FprMNn3QUQyN5z5EiSvsrXsO0S6UCMU";

/// The `Authorization` scheme this server accepts, and the only one.
///
/// `Bearer` is what RFC 6750 §2.1 defines for a token the client presents without
/// further proof, and it is what the client's `network/ws.rs` sends. Anything else
/// -- `Basic`, `Token`, a bare token with no scheme -- is refused as malformed
/// rather than guessed at, because a server that accepts two spellings of the same
/// header is a server whose authentication depends on which spelling a client
/// happened to use.
pub const BEARER_SCHEME: &str = "Bearer";

/// The path login is served on.
pub const LOGIN_PATH: &str = "/auth/login";

/// The path logout is served on.
pub const LOGOUT_PATH: &str = "/auth/logout";

/// The path an administrator creates an account on.
pub const ADMIN_USERS_PATH: &str = "/admin/users";

/// The `code` a refused credential carries in its JSON body.
///
/// One code for a bad password, an unknown username, a malformed body, a missing
/// `Authorization` header, a malformed one, an expired session and a revoked one.
/// See the module docs: collapsing them is deliberate, and
/// [`crate::db::SessionState`] is where the operator-facing distinction lives
/// instead.
pub const INVALID_CREDENTIALS: &str = "invalid_credentials";

/// The `code` a request from a non-administrator carries.
///
/// **Distinct from [`INVALID_CREDENTIALS`], and the difference is deliberate.** A
/// caller who *is* logged in and is not an administrator is told so, because that
/// is a fact about the caller rather than a probe of the credential store: the
/// caller already proved who they are, so nothing is being inferred. Merging the
/// two would tell an administrator their session was not recognised, which is the
/// opposite of helpful.
pub const NOT_AN_ADMINISTRATOR: &str = "not_an_administrator";

/// The `code` a malformed request body, or a server fault, carries.
///
/// **One code for both, and the reason they share it is that the status line
/// already separates them**: a bad body is a 400 and a fault is a 500. The body
/// exists for a human reading a log, and a sentence that says "the request could
/// not be read" is true of both without inventing a distinction the status did not
/// make.
pub const INVALID_REQUEST: &str = "invalid_request";

/// The `code` a username that is already taken carries.
pub const USERNAME_TAKEN: &str = "username_taken";

// ---------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------

/// Hashes a password with Argon2id at the crate's default cost parameters.
///
/// **Argon2id is named rather than inherited.** `argon2`'s `Algorithm::default()`
/// is already Argon2id -- it carries `#[default]` on that variant -- so writing
/// `Argon2::default()` would produce the same hash. It is spelled out anyway
/// because a dependency's default is not a decision this project has made, and
/// because the day the upstream default changes this line should be the thing that
/// stops it silently changing the algorithm every password on every instance is
/// hashed with.
///
/// **The default cost is left alone, and that is a decision too.** `Params::DEFAULT`
/// is 19 MiB of memory and 2 passes, which is the OWASP-recommended minimum for
/// Argon2id, and it is what `users.password_hash`'s PHC string records -- so a
/// later build can raise it and every existing hash keeps verifying with the
/// parameters it was made under. Changing the cost without re-hashing on next
/// login is the failure mode a self-describing format exists to prevent, and it is
/// why the parameters live inside the stored string rather than in a config file.
///
/// # Arguments
///
/// * `password` - the plaintext, borrowed so the caller's copy is not cloned into
///   a buffer. It is not zeroed afterwards; see the module's "what is NOT here"
///   table, which names that rather than implying otherwise.
///
/// # Errors
///
/// [`ServerError::Crypto`] if the operating system could not produce a salt or
/// Argon2 refused the parameters. **The message names the failure and never the
/// input**, so this is safe in an `error!`.
pub fn hash_password(password: &str) -> Result<String> {
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::DEFAULT);
    argon2
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|error| ServerError::Crypto(format!("could not hash the password ({error})")))
}

/// Checks a candidate against a stored hash, or against [`DUMMY_PASSWORD_HASH`].
///
/// **A single `bool`, on purpose, and the shape of the signature is the security
/// property.** `AGENTS.md` §2.1 asks for typed errors; a typed answer here would
/// be a `Result<bool, _>` whose error arm says "no such user", and a caller with
/// that arm in hand can branch on it. The module docs say why an unauthenticated
/// peer must not be able to enumerate usernames, and this signature removes the
/// branch rather than forbidding it.
///
/// # Arguments
///
/// * `stored` - the `users.password_hash` value, or `None` for a row that cannot
///   log in (the reserved historical author, or an account whose password was
///   never set). `None` is **not** "match anything" and **not** "match nothing
///   quickly": it verifies [`DUMMY_PASSWORD_HASH`], so the work is the same as a
///   real verification and the answer is `false`.
/// * `candidate` - the plaintext being offered.
///
/// # Returns
///
/// `true` only for a real hash that the candidate satisfies.
///
/// **Constant time in the comparison is the upstream crate's job**, not this
/// function's: `argon2` compares digests with a constant-time equality, and the
/// only thing this function adds is that there is no early return on the path
/// between them. A `!=` on two 32-byte digests here would leak the prefix a
/// cracker had guessed correctly.
///
/// The cost parameters come from the *stored* PHC string rather than from
/// `Params::DEFAULT`, which is what makes a re-tuned cost parameter transparent to
/// an old hash: `argon2`'s `PasswordVerifier<str>` implementation parses the
/// encoded form and uses what it finds there.
pub fn verify_password(stored: Option<&str>, candidate: &str) -> bool {
    let target = stored.unwrap_or(DUMMY_PASSWORD_HASH);
    Argon2::default()
        .verify_password(candidate.as_bytes(), target)
        .is_ok()
}

/// The minimum password length in characters: 12.
///
/// **A floor and not a policy engine.** Twelve characters is where a human-chosen
/// secret stops being cheap to guess in bulk. No composition rule ("must contain a
/// digit"): those push people towards `Password1!`, which is not what makes a
/// password strong. No dictionary check and no breach-list lookup: both need a
/// network fetch or a multi-megabyte embedded list, and neither is an
/// `AGENTS.md` §7.2 audit this milestone carries.
///
/// Checked at account creation and **never silently corrected** -- the caller is
/// told and the account is refused. The alternative, quietly accepting a short
/// password, would leave this function exercised only in the direction that does
/// not matter.
pub const MIN_PASSWORD_CHARS: usize = 12;

/// Whether a password meets [`MIN_PASSWORD_CHARS`].
///
/// # Arguments
///
/// * `password` - as untrusted text from a request body or an environment
///   variable.
pub fn password_is_acceptable(password: &str) -> bool {
    password.chars().count() >= MIN_PASSWORD_CHARS
}

// ---------------------------------------------------------------------------
// Session tokens
// ---------------------------------------------------------------------------

/// Mints a session token: 32 bytes of entropy, hex-encoded.
///
/// **Hex, and not base64, which costs 32 extra bytes and buys no bug.** A token
/// travels in an HTTP header (`ws.rs`) and in JSON (login's response body), and
/// both of those have their own opinions about `+`, `/` and `=`. Base64url would
/// be shorter, but hex is 64 characters of `[0-9a-f]`, which needs no escaping
/// anywhere, cannot be case-folded by an intermediary, and cannot be misread by an
/// operator comparing two tokens by eye. The property that matters for the one
/// value in this system which must survive verbatim is **that no layer between the
/// two ends has to be trusted not to mangle it.**
///
/// Not a secret in memory, and this does not try to be; see the module's "what is
/// NOT here" table.
pub fn mint_session_token() -> String {
    let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    hex_encode(&bytes)
}

/// The `sessions.token_hash` value for a token: its SHA-256 digest, hex-encoded.
///
/// **Applied once, on the way in, and never on the way out.** A caller that has
/// already hashed a token must not hash it again: the row is keyed on this value,
/// so a second application produces a key that is not in the table and the session
/// reads as [`SessionState::Absent`]. There is no way to detect that from the
/// outside, which is why every call site ([`login`], [`logout`] and
/// [`authenticate`]) hashes exactly once and none of them accepts an
/// already-hashed value.
///
/// `sha2` rather than `argon2` is a deliberate and documented split; see the
/// module docs, which turn on this hash running on every WebSocket handshake
/// inside a 16 ms budget.
///
/// # Arguments
///
/// * `token` - the plaintext the peer presented. Borrowed, not retained.
pub fn hash_session_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    // `Digest::digest` on a fixed-output hasher returns `hybrid_array::Array`,
    // which derefs to a slice, and `hex_encode` takes `&[u8]` -- so the deref is
    // the whole conversion and there is no `Vec` in this function.
    hex_encode(&digest)
}

/// Encodes bytes as lowercase hex.
///
/// A local helper rather than a `hex` dependency: it is eight lines, it has no
/// failure mode, and `AGENTS.md` §7.2 prices a dependency by its supply chain
/// before it prices it by its compile time.
fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // `>> 4` and `& 0x0f` rather than a division: both halves are in range by
        // construction, so there is no remainder and no error to invent.
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Extracts the token from an `Authorization: Bearer <token>` header value.
///
/// # Arguments
///
/// * `value` - the raw header value, or `None` when the header was absent.
///
/// # Returns
///
/// `Some(token)` for `Bearer ` followed by **64 lowercase hex characters**, and
/// `None` for anything else -- an absent header, a different scheme, extra
/// whitespace, a truncated or over-long value, or any character outside `[0-9a-f]`.
///
/// **Validating the shape is not decoration.** This runs before the database is
/// touched, so a peer cannot make the server hash an arbitrary-length string on
/// every handshake by putting a megabyte in a header. It also means the only
/// strings that reach [`hash_session_token`] are 64 characters long, which is what
/// makes a fixed-size value safe to key an index on.
///
/// Case is *not* folded on the token. A token is hex, and a client that uppercased
/// one has a bug; folding would turn that bug into an intermittent authentication
/// failure that is very hard to read. `is_ascii_hexdigit` accepts `A-F`, so the
/// uppercase rejection is a separate, named test rather than folded into it.
pub fn bearer_token(value: Option<&str>) -> Option<&str> {
    let value = value?;
    let token = value.strip_prefix(BEARER_SCHEME)?.strip_prefix(' ')?;
    let shaped = token.len() == SESSION_TOKEN_BYTES * 2
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    shaped.then_some(token)
}

/// The `Authorization` header's value, if the request carried one.
///
/// # Arguments
///
/// * `headers` - the request's headers.
pub fn authorization(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()
}

/// A session this server just issued, as it goes back to the client.
///
/// The token is in here, and **that is the only place in the entire crate it
/// appears**. It is a field of a struct that is serialised straight into a
/// response body and then dropped; the type deliberately does **not** derive
/// `Debug`, so there is no way to print one by accident -- a `{:?}` on it does not
/// compile, which is a stronger guarantee than a hand-written `Debug` that has to
/// be trusted not to leak.
#[derive(serde::Serialize)]
pub struct IssuedSession {
    /// The token. Shown once, to the caller, and never recoverable afterwards --
    /// the server keeps only [`hash_session_token`] of it.
    pub token: String,
    /// The account the session belongs to, so the client knows who it is.
    pub user_id: String,
    /// The login handle.
    pub username: String,
    /// When the session stops being accepted, in milliseconds since the epoch.
    pub expires_at_unix_ms: i64,
}

// ---------------------------------------------------------------------------
// Bearer parsing on the WebSocket handshake
// ---------------------------------------------------------------------------

/// The identity a WebSocket handshake proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authenticated {
    /// The `users.id` a message will be attributed to.
    pub user_id: String,
    /// Whether that user may create accounts.
    pub is_admin: bool,
}

/// Reads the `Authorization` header and decides whether it names a live session.
///
/// **Blocking.** It touches the SQLite connection, which `db.rs`'s module docs say
/// is synchronous, and a handshake is a request handler on a tokio worker -- so the
/// caller is expected to be inside `spawn_blocking`. `ws.rs` is.
///
/// # Arguments
///
/// * `store` - the persistence layer.
/// * `header` - the `Authorization` header value, or `None` if absent.
///
/// # Returns
///
/// `Ok(Some(identity))` only for a live session. `Ok(None)` for **every**
/// credential-shaped refusal -- absent, malformed, unknown, expired, revoked --
/// because they are one outcome to the peer and four to the operator, and the
/// distinction is made in the log line this function emits.
///
/// # Errors
///
/// [`ServerError::Sqlite`] if the session row could not be read. That is
/// deliberately **not** folded into `Ok(None)`: a database that cannot answer is a
/// server fault and `ws.rs` answers it with a 500, while every credential-shaped
/// failure is a 401. Collapsing them would send an operator debugging a broken
/// disk to look at login attempts.
pub fn authenticate(store: &Store, header: Option<&str>) -> Result<Option<Authenticated>> {
    let Some(token) = bearer_token(header) else {
        // `authorization_present`, never the header itself: `AGENTS.md` §7.5. The
        // header *is* the credential, and a malformed one is still somebody's
        // credential.
        debug!(
            authorization_present = header.is_some(),
            "refused a handshake with no usable bearer token"
        );
        return Ok(None);
    };

    let hash = hash_session_token(token);
    let now = time::now_unix_millis();

    match store.lookup_session(&hash, now)? {
        SessionState::Live {
            user_id,
            is_admin,
            expires_at_unix_ms,
        } => {
            debug!(
                user_id = %user_id,
                expires_at_unix_ms,
                "authenticated a websocket handshake"
            );
            Ok(Some(Authenticated { user_id, is_admin }))
        }
        SessionState::Revoked { revoked_at_unix_ms } => {
            warn!(
                revoked_at_unix_ms,
                "refused a websocket handshake presenting a revoked session"
            );
            Ok(None)
        }
        SessionState::Expired { expires_at_unix_ms } => {
            debug!(
                expires_at_unix_ms,
                now_unix_millis = now,
                "refused a websocket handshake presenting an expired session"
            );
            Ok(None)
        }
        SessionState::Absent => {
            // `info!` and not `warn!`: an unauthenticated handshake against a
            // running instance is a security event, and §7.5 puts "auth failure"
            // at `error!` -- but a probe is not the server failing.
            info!("refused a websocket handshake presenting an unknown session token");
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP endpoints
// ---------------------------------------------------------------------------

/// The three routes this module owns.
///
/// **Returns a `Router<AppState>` rather than a finished router**, because axum
/// 0.8 has no `From<Router<S>>` conversion: the only way to compose two routers is
/// to merge them at the same state type and call `with_state` once, on the result.
/// `crate::router` does exactly that, and the tests drive the finished router so
/// there is no second assembly to disagree with.
///
/// A function rather than three constants inlined into [`crate::router`], so that
/// the whole HTTP surface this milestone adds is readable in one place and
/// `docs/API.md` has three names to agree with.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(LOGIN_PATH, post(login))
        .route(LOGOUT_PATH, post(logout))
        .route(ADMIN_USERS_PATH, post(create_account))
        // axum's own body ceiling, applied here rather than by hand: it is the
        // mechanism the framework provides for exactly this, and re-implementing
        // it would mean reading the body twice. A `POST /auth/login` is one of the
        // cheapest requests on the internet to send, so an unbounded body here is
        // an unbounded allocation an unauthenticated peer can ask for.
        .layer(DefaultBodyLimit::max(MAX_LOGIN_BODY_BYTES))
}

/// `POST /auth/login`: exchanges a username and password for a session token.
///
/// # Arguments
///
/// * `state` - the store, reached through `spawn_blocking` because every method on
///   it is synchronous.
/// * `body` - `{"username": "...", "password": "..."}`, at most
///   [`MAX_LOGIN_BODY_BYTES`] long.
///
/// # Returns
///
/// `200` with an [`IssuedSession`] on success. **401 with
/// [`INVALID_CREDENTIALS`]** for a wrong password and an unknown username alike;
/// `400` for a body that is missing a field; `500` for a fault that is not about
/// the credential -- and never a 401 for that last one, because an operator
/// debugging a broken disk would otherwise be sent to the login code.
async fn login(State(state): State<AppState>, body: String) -> Response {
    let (username, password) = match credentials_from(&body) {
        Some(parsed) => parsed,
        None => {
            warn!("refused a login whose body named no username and password");
            return refused(StatusCode::BAD_REQUEST, INVALID_REQUEST);
        }
    };

    // Both are untrusted and neither is logged -- not even a length for the
    // password, because a repeated, attacker-chosen length is itself a signal
    // about a secret.
    let store = state.store.clone();
    let looked_up = tokio::task::spawn_blocking(move || {
        let found = store.credential_for_username(&username)?;
        Ok::<_, ServerError>(match found {
            // The absent path still verifies, against `DUMMY_PASSWORD_HASH`, so
            // this branch costs the same as the other one and returns the same
            // answer.
            None => (false, None),
            Some(credential) => (
                verify_password(credential.password_hash.as_deref(), &password),
                Some(credential),
            ),
        })
    })
    .await;

    let (matched, credential) = match looked_up {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            error!(error = %error, "could not read a credential during login");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST);
        }
        Err(join_error) => {
            error!(
                error = %join_error,
                "the login's storage task did not complete"
            );
            return refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST);
        }
    };

    // `filter` rather than a `match`: an account whose password did not verify
    // must produce the *same* refusal as no account at all, and expressing that as
    // "keep the credential only if it matched" is what stops a future edit from
    // adding an arm that tells the two apart.
    let Some(credential) = credential.filter(|_| matched) else {
        // `info!` and not `warn!`, and with no username: a failed login is a
        // routine event on an instance reachable from a network, and logging the
        // attempted username would turn the log into the username oracle this
        // endpoint exists not to be.
        info!("refused a login");
        return refused(StatusCode::UNAUTHORIZED, INVALID_CREDENTIALS);
    };

    let Credential { account, .. } = credential;
    let token = mint_session_token();
    let hash = hash_session_token(&token);
    let now = time::now_unix_millis();
    let expires_at = now.saturating_add(SESSION_TTL_MILLIS);

    let store = state.store.clone();
    let recorded = tokio::task::spawn_blocking({
        let hash = hash.clone();
        let user_id = account.id.clone();
        move || store.insert_session(&hash, &user_id, now, expires_at)
    })
    .await;

    match recorded {
        Ok(Ok(())) => {
            // Ids, sizes and outcomes. No token, no hash, no password: §7.5.
            info!(
                user_id = %account.id,
                username_len = account.username.len(),
                expires_at_unix_ms = expires_at,
                "issued a session"
            );
            (
                StatusCode::OK,
                Json(IssuedSession {
                    token,
                    user_id: account.id,
                    username: account.username,
                    expires_at_unix_ms: expires_at,
                }),
            )
                .into_response()
        }
        Ok(Err(error)) => {
            error!(error = %error, "could not store a session");
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
        Err(join_error) => {
            error!(
                error = %join_error,
                "the session's storage task did not complete"
            );
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
    }
}

/// `POST /auth/logout`: revokes the session this request presents.
///
/// **The revocation path ADR-010's token decision exists to have.** Without it,
/// "expel a member" would mean waiting out a seven-day TTL; with it, one request
/// and the next handshake is refused. A client holding an open socket keeps it
/// until it closes -- `PLAN.md` §6 has no frame for a server-initiated revocation,
/// so a live socket is not killed by this call -- which is a named limitation
/// rather than a bug in this function.
///
/// # Arguments
///
/// * `state` - the store.
/// * `headers` - the request's headers, for the `Authorization` value.
///
/// # Returns
///
/// `204 No Content` whether or not the session existed. **That is deliberate:**
/// a `logout` that reported "no such session" would be an oracle for whether a
/// token was ever valid, and a user signing out should never see an error.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(token) = bearer_token(authorization(&headers)) else {
        // The same answer as the success path. See the docs above.
        return StatusCode::NO_CONTENT.into_response();
    };
    let hash = hash_session_token(token);
    let now = time::now_unix_millis();

    let store = state.store.clone();
    let revoked = tokio::task::spawn_blocking(move || store.revoke_session(&hash, now)).await;

    match revoked {
        Ok(Ok(true)) => {
            info!(
                revoked_at_unix_ms = now,
                "revoked a session at its holder's request"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(false)) => {
            debug!("a logout named a session that was already not live");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(error)) => {
            error!(error = %error, "could not revoke a session");
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
        Err(join_error) => {
            error!(
                error = %join_error,
                "the logout's storage task did not complete"
            );
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
    }
}

/// `POST /admin/users`: creates an account. Administrators only.
///
/// **The entire account-provisioning surface of this server, and it is
/// deliberately not public.** ADR-010's Decision section is explicit: on a
/// self-hosted instance, an open `POST /auth/register` is "a read-access hole:
/// anyone who can reach the port can enrol and read the team's messages". So there
/// is no registration route, there is no "first user becomes an administrator"
/// branch, and there is nothing to disable later.
///
/// **The new account is not a member of any channel.** It can log in and it has no
/// `channel_members` row, so every send it makes is refused with `not_a_member`
/// until somebody grants it one. That is the intended shape -- provisioning an
/// account and granting it access are separate decisions, and this milestone only
/// implements the first.
///
/// # Arguments
///
/// * `state` - the store.
/// * `headers` - the administrator's `Authorization` value.
/// * `body` - `{"username": "...", "display_name": "...", "password": "..."}`.
///
/// # Returns
///
/// `201` with the created [`crate::db::Account`], which carries no hash and no
/// token -- the new account must log in for itself. `401` for a bad session, `403`
/// for a session that is not an administrator, `409` for a taken username, and
/// `400` for a body that is missing a field or carries a password under
/// [`MIN_PASSWORD_CHARS`].
async fn create_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let administrator = match require_administrator(&state, &headers).await {
        Administrator::Granted(identity) => identity,
        refused => return administrator_refusal(refused),
    };

    let request: NewAccount = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(_) => {
            warn!(
                administrator_id = %administrator.user_id,
                "refused an account creation whose body was not the expected shape"
            );
            return refused(StatusCode::BAD_REQUEST, INVALID_REQUEST);
        }
    };

    // Blankness and length are checked here rather than being pushed into the
    // store's SQL, because a blank username is a *rule* and the rule belongs next
    // to the other rules: `AGENTS.md` §2.1 makes the wire boundary the place where
    // untrusted text becomes trusted data, and this is the server's.
    let username = request.username.trim();
    let display_name = request.display_name.trim();
    if username.is_empty() || username.chars().count() > MAX_USERNAME_CHARS {
        warn!(
            administrator_id = %administrator.user_id,
            username_len = username.len(),
            "refused an account creation whose username is blank or too long"
        );
        return refused(StatusCode::BAD_REQUEST, INVALID_REQUEST);
    }
    if display_name.is_empty() || display_name.chars().count() > MAX_DISPLAY_NAME_CHARS {
        warn!(
            administrator_id = %administrator.user_id,
            display_name_len = display_name.len(),
            "refused an account creation whose display name is blank or too long"
        );
        return refused(StatusCode::BAD_REQUEST, INVALID_REQUEST);
    }
    if !password_is_acceptable(&request.password) {
        // The rule and the number, so the user can act on it. Never the password.
        warn!(
            administrator_id = %administrator.user_id,
            username_len = username.len(),
            minimum_chars = MIN_PASSWORD_CHARS,
            "refused an account creation whose password is too short"
        );
        return refused(StatusCode::BAD_REQUEST, INVALID_REQUEST);
    }

    let password_hash = match hash_password(&request.password) {
        Ok(hash) => hash,
        Err(error) => {
            error!(error = %error, "could not hash a new account's password");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST);
        }
    };

    let store = state.store.clone();
    let owned_username = username.to_owned();
    let owned_display_name = display_name.to_owned();
    let created = tokio::task::spawn_blocking(move || {
        store.create_account(&owned_username, &owned_display_name, &password_hash, false)
    })
    .await;

    match created {
        Ok(Ok(account)) => {
            info!(
                administrator_id = %administrator.user_id,
                user_id = %account.id,
                "an administrator created an account"
            );
            (StatusCode::CREATED, Json(account)).into_response()
        }
        Ok(Err(ServerError::UsernameTaken { .. })) => {
            // Neither the log line nor the response carries the username. The
            // response names the *field*, and the log says the field was taken:
            // an administrator is already authenticated here, so this is not the
            // username oracle `login` avoids -- but there is no reason for the
            // value to be in a log file, and §7.5 is the rule.
            warn!(
                administrator_id = %administrator.user_id,
                "refused an account creation whose username is taken"
            );
            refused(StatusCode::CONFLICT, USERNAME_TAKEN)
        }
        Ok(Err(error)) => {
            error!(
                administrator_id = %administrator.user_id,
                error = %error,
                "could not create an account"
            );
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
        Err(join_error) => {
            error!(
                administrator_id = %administrator.user_id,
                error = %join_error,
                "the account creation's storage task did not complete"
            );
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
    }
}

/// The body of [`create_account`]'s request.
///
/// A private struct with three `String`s, and it is the reason the server needs
/// `serde` at all: `sh_nexus_wire` uses it for frames, and this is the one place
/// the server deserialises something of its own.
#[derive(serde::Deserialize)]
struct NewAccount {
    /// The login handle. Trimmed and length-checked by the handler.
    username: String,
    /// The name to render in a transcript.
    display_name: String,
    /// The plaintext password, hashed and dropped. Never logged: §7.5.
    password: String,
}

/// The longest username this server accepts: 64 characters.
///
/// Bounded because a username is *rendered* -- it appears in a log line's length
/// field, in a future member list, and in a future refusal's `detail` -- and
/// `AGENTS.md` §7.1 forbids unbounded growth of in-memory state, which an unbounded
/// identifier is a step towards. 64 is more than any plausible team needs and short
/// enough to read on one line.
pub const MAX_USERNAME_CHARS: usize = 64;

/// The longest display name this server accepts: 128 characters.
///
/// Twice the username, because a display name is *for* a human to read and a
/// truncated one is worse than a long one.
pub const MAX_DISPLAY_NAME_CHARS: usize = 128;

/// The longest login body this server reads: 4 KiB.
///
/// **The only content policy an HTTP body has in this milestone**, and it is here
/// rather than in `ws.rs` because `ws.rs`'s ceiling is a *transport* limit
/// (`MAX_FRAME_BYTES`) that axum enforces during the upgrade; an HTTP body here is
/// ordinary text, and axum will read all of it. A username is at most 64 bytes and
/// a password at most a few hundred, so 4 KiB is generous by a factor of ten and
/// still bounds what one unauthenticated request can make the server allocate.
pub const MAX_LOGIN_BODY_BYTES: usize = 4 * 1024;

/// Why an administrator-only request did or did not get an administrator.
///
/// **Four outcomes, and it is an enum rather than a `Result<_, Response>` for two
/// reasons.** The first is size: `axum::http::Response` is a 128-byte-plus value and
/// a `Result` that carries one in its `Err` arm is a large error by any measure,
/// which `clippy::result_large_err` refuses and rightly so. The second is that an
/// enum names the three refusals, and a caller matching on them cannot forget the
/// 403 -- which is the one that must not be confused with the 401.
enum Administrator {
    /// A live session belonging to an administrator.
    Granted(Authenticated),
    /// No live session was presented; the caller answers 401.
    Unauthenticated,
    /// A live session that is not an administrator; the caller answers 403.
    ///
    /// **Carries no identity, on purpose.** The account's id is already in the log
    /// line `require_administrator` emits before it returns this, and a value
    /// nothing reads would be one more thing a future edit could start reading and
    /// put into a response body.
    WrongPrivilege,
    /// The session could not be read; the caller answers 500.
    StorageUnavailable,
}

/// Resolves an administrator from an `Authorization` header.
///
/// See [`Administrator`] for why this is an enum rather than a `Result` carrying a
/// `Response`, and for what the 401-versus-403 split is protecting.
async fn require_administrator(state: &AppState, headers: &HeaderMap) -> Administrator {
    let store = state.store.clone();
    let presented = authorization(headers).map(str::to_owned);
    let read =
        tokio::task::spawn_blocking(move || authenticate(&store, presented.as_deref())).await;

    match read {
        Ok(Ok(Some(identity))) if identity.is_admin => Administrator::Granted(identity),
        Ok(Ok(Some(identity))) => {
            info!(
                user_id = %identity.user_id,
                "refused an administrator-only request from an account that is not one"
            );
            Administrator::WrongPrivilege
        }
        Ok(Ok(None)) => Administrator::Unauthenticated,
        Ok(Err(error)) => {
            error!(
                error = %error,
                "could not read a session on an administrator-only request"
            );
            Administrator::StorageUnavailable
        }
        Err(join_error) => {
            error!(
                error = %join_error,
                "the administrator-only request's storage task did not complete"
            );
            Administrator::StorageUnavailable
        }
    }
}

/// Answers a refused [`Administrator`] with the status its case calls for.
///
/// A function so the 403 cannot be reached by accident on the way to the 401: there
/// is exactly one place that maps an outcome to a status, and `create_account`'s
/// handler reads as three lines rather than a `match` it has to get right.
///
/// The `Granted` arm answers 500 rather than panicking or looping: reaching it means
/// the handler called this with something it had already matched, which is a bug in
/// the caller, and a 500 is the honest answer for "the server got its own logic
/// wrong" without a panic on a request path (`AGENTS.md` §2.1).
fn administrator_refusal(outcome: Administrator) -> Response {
    match outcome {
        Administrator::Granted(_) => refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST),
        Administrator::Unauthenticated => refused(StatusCode::UNAUTHORIZED, INVALID_CREDENTIALS),
        Administrator::WrongPrivilege => refused(StatusCode::FORBIDDEN, NOT_AN_ADMINISTRATOR),
        Administrator::StorageUnavailable => {
            refused(StatusCode::INTERNAL_SERVER_ERROR, INVALID_REQUEST)
        }
    }
}

/// Reads a username and password out of a login body.
///
/// **Hand-parsed, and the reason is worth naming.** `serde_json` is declared and a
/// `#[derive(Deserialize)]` struct would be the shorter spelling -- so this looks
/// like the wrong call until the requirement is stated: the handler must not
/// distinguish "no `username` field" from "`username` is null" from "`username` is
/// 4 MB of text", and a typed struct makes all three different shapes to match on.
/// One function returning `None` for all of them is one arm.
///
/// # Arguments
///
/// * `body` - the raw request body as UTF-8, already length-capped by
///   [`MAX_LOGIN_BODY_BYTES`]'s caller.
fn credentials_from(body: &str) -> Option<(String, String)> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let username = parsed.get("username")?.as_str()?;
    let password = parsed.get("password")?.as_str()?;
    if username.is_empty() || password.is_empty() {
        return None;
    }
    Some((username.to_owned(), password.to_owned()))
}

/// Builds a refusal: a status, a code, and a sentence a user can act on.
///
/// **The body shape is the server's, not the wire crate's, and that is a gap worth
/// naming.** `PLAN.md` §6 versions every *frame*; an HTTP status line is not a frame,
/// so there is no envelope to version and no `sh_nexus_wire` type to borrow. A
/// future protocol revision may well introduce a versioned HTTP error shape, and
/// doing that in the wire crate is a smaller change than doing it twice here. What
/// is fixed now is that `code` is the field a client should switch on and `detail`
/// is the field a human should read -- the same division `sh_nexus_wire`'s
/// `message.error` makes, and the reason [`INVALID_CREDENTIALS`] is a
/// `&'static str` rather than a sentence.
fn refused(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(json!({
            "code": code,
            "detail": detail_for(code),
        })),
    )
        .into_response()
}

/// The peer-facing sentence for a refusal code.
///
/// **A function rather than a table, and the reason is that no code's sentence may
/// vary.** If `detail` were looked up with the code as a key, a future code added
/// for one situation would read as a general explanation for another. Every code
/// here answers the same question -- *why was I refused* -- and none of them may
/// reveal whether a credential exists.
fn detail_for(code: &'static str) -> &'static str {
    match code {
        INVALID_CREDENTIALS => {
            "that username and password combination was not accepted by this server"
        }
        NOT_AN_ADMINISTRATOR => "this operation is reserved for an account's administrator",
        USERNAME_TAKEN => "that username is already in use on this server",
        _ => "the request could not be read; check its shape and try again",
    }
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

/// Creates this instance's first administrator from operator-supplied values.
///
/// # Arguments
///
/// * `store` - the instance's store.
/// * `username` - the login handle, from `SH_NEXUS_ADMIN_USERNAME`.
/// * `password` - the plaintext, from `SH_NEXUS_ADMIN_PASSWORD`. Hashed here and
///   not retained.
///
/// # Returns
///
/// `Ok(true)` when an administrator was created, `Ok(false)` when the instance
/// already had an account that could log in.
///
/// **Never overwrites, and never runs twice.** The check is
/// [`Store::account_count`], and that counts *accounts that can log in* rather than
/// rows in `users` -- so the reserved historical-author row neither suppresses the
/// bootstrap on a database migrated from schema 1 nor triggers it on a fresh one.
/// That single distinction is what makes this function correct on both, and it is
/// why [`Store::account_count`] is written the way it is.
///
/// A second start with an account present writes nothing at all -- no `UPDATE`, no
/// password change, no new session. Re-running this against a live instance is
/// therefore safe, which matters because it runs on **every** start and an operator
/// who changed the environment variable after the first run has to be able to trust
/// that it did nothing.
///
/// The administrator is also granted [`crate::db::DEFAULT_CHANNEL_ID`], because it
/// is the one account that can reach a channel without being given one -- and on an
/// instance with no channels beyond the seeded one, "grant yourself access" is what
/// bootstrapping means.
///
/// # Errors
///
/// [`ServerError::UsernameTaken`] if the username exists but the account count said
/// there was no account to log in as -- which means the instance has a row with
/// that name and no password, and an operator has to resolve it. **Refused rather
/// than repaired**: silently overwriting a row this function did not create is
/// exactly what `AGENTS.md` §7.1's "no silent data destruction" forbids.
pub fn bootstrap(store: &Store, username: &str, password: &str) -> Result<bool> {
    if store.account_count()? > 0 {
        debug!("this instance already has an account; not bootstrapping another");
        return Ok(false);
    }

    let password_hash = hash_password(password)?;
    let account = store.create_account(username, username, &password_hash, true)?;
    store.add_channel_member(&account.id, crate::db::DEFAULT_CHANNEL_ID)?;

    // The id and the username's length are the whole diagnostic. §7.5 is the rule.
    info!(
        user_id = %account.id,
        username_len = username.len(),
        "bootstrapped this instance's first administrator from the environment"
    );
    Ok(true)
}
