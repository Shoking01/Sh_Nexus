//! The project's global error type.
//!
//! `AGENTS.md` §3.3 specifies this enum, and every fallible operation in the
//! client returns it. The `Result` alias at the bottom of this module is the
//! other half of that contract: `Result<T, ShNexusError>` is written once here
//! so that the §9.1 doc-comment template can be used verbatim.
//!
//! # Where this type is allowed to appear
//!
//! Anywhere in the client. It is deliberately *outside* `core/`: `core/` is pure
//! domain logic per §3.2, and an error enum that carries `serde_json::Error` is
//! about I/O, not about the domain. A `core/` function that needs to report
//! "this is not a valid message" does not return a `ShNexusError`; it returns a
//! `bool`, an `Option`, or a narrower error its own module defines -- §3.3's
//! "each module may define narrower error types that convert into
//! `ShNexusError`" is the sanctioned direction of travel, not the reverse.
//!
//! # `anyhow` and GPUI
//!
//! **GPUI returns `anyhow::Result`, and `anyhow` does not appear in this enum.**
//! `Application::run`, `App::open_window` and the window lifecycle functions all
//! return `anyhow::Error`, because GPUI is a UI framework that predates this
//! project's error policy and cannot be made to take a type parameter. That is a
//! fact about the dependency, not a licence to adopt it.
//!
//! The intended conversion is at the **bootstrap boundary only** -- in
//! `app.rs`, which is GPUI-facing and will land in Phase 2:
//!
//! ```text
//!   cx.open_window(options, build) -> anyhow::Result<WindowHandle<T>>
//!            |
//!            |  log the full source chain with tracing::error!
//!            v
//!   ShNexusError::Unknown(error.to_string())   // app.rs, and nowhere else
//! ```
//!
//! Three rules, and the reasons they are rules rather than preferences:
//!
//! 1. **No `From<anyhow::Error>` impl.** An automatic conversion is how a
//!    dependency's error type becomes your domain's error type: it makes
//!    `?` propagate `anyhow` through every layer, until `core/` -- which
//!    §3.2 declares free of I/O and platform code -- transitively depends on it.
//!    A deliberate, named conversion at one boundary cannot do that, because
//!    there is exactly one boundary and it is written by hand.
//! 2. **The conversion is not in this module.** This module is the domain's
//!    vocabulary. A GPUI-shaped hole in it would invite the next module to
//!    depend on it, and the hole would spread.
//! 3. **The source chain is logged, not stringified into the domain.** The
//!    `Unknown` variant carries one line of text because that is all a
//!    domain-level error can honestly carry. Anything a developer would need to
//!    debug it is a `tracing` concern, and `AGENTS.md` §7.5's "never log message
//!    content, tokens, or credentials" applies to that line as much as to
//!    anything else -- a window-creation error is never going to contain one, but
//!    the habit is what matters.
//!
//! The doctest on [`ShNexusError`] shows the shape of the conversion without
//! naming `anyhow`, so it compiles in this crate without depending on it.

use thiserror::Error;

/// Every way an operation in the client can fail.
///
/// The variant set is `AGENTS.md` §3.3's, verbatim, and the `Display` text of
/// each is what that section specifies. The payload of the three I/O variants is
/// where this implementation deviates, and it is called out on each.
///
/// # Example
///
/// The shape of the GPUI boundary conversion, without naming `anyhow`:
///
/// ```
/// use sh_nexus::errors::ShNexusError;
///
/// # fn open_window() -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
/// # let platform_error: Box<dyn std::error::Error + Send + Sync> =
/// #     Box::new(std::io::Error::other("window creation failed"));
/// // `app.rs` logs the full source chain, then reduces the GPUI error to the
/// // domain error type. `anyhow` is never named in a variant.
/// let domain_error = ShNexusError::Unknown(platform_error.to_string());
/// assert_eq!(
///     domain_error.to_string(),
///     "unknown error: window creation failed",
/// );
/// # let _ = open_window();
/// ```
#[derive(Debug, Error)]
pub enum ShNexusError {
    /// A transport-level failure talking to the server: a refused connection, a
    /// DNS failure, a TLS failure, a request that could not be sent.
    ///
    /// **The payload is a `String` in this revision, not a `#[from]`
    /// `reqwest::Error`.** `AGENTS.md` §3.3 shows the typed form, and the typed
    /// form is the right end state -- but it requires depending on `reqwest`,
    /// which is Phase 4 work (`PLAN.md` §8). Taking it in work unit 1A would mean
    /// pulling a TLS stack and a full HTTP client into a work unit whose entire
    /// content is four type files, against §7.2's criterion 5 and against the
    /// phase plan's own sequencing. The variant name, the payload's meaning and
    /// the `Display` text are already what §3.3 specifies, so the change to the
    /// typed form is a two-line diff when `reqwest` lands and no call site moves.
    ///
    /// Callers should expect this only from `network/`, and should treat it as
    /// **recoverable**: `AGENTS.md` §3.3 requires network failures to surface as
    /// a reconnecting banner or offline mode, never as a crash or a silent drop.
    #[error("network error: {0}")]
    Network(String),

    /// A WebSocket failure: the socket closed, the handshake failed, or a frame
    /// could not be written.
    ///
    /// **A `String` payload for the same reason as [`ShNexusError::Network`].**
    /// `AGENTS.md` §3.3's `#[from] tokio_tungstenite::tungstenite::Error` needs
    /// `tokio-tungstenite`, which arrives in Phase 4.
    ///
    /// This is *not* the error for a frame that failed to parse -- that is
    /// [`ShNexusError::Protocol`]. This one means the connection itself is
    /// broken, and the distinction is what decides whether the client
    /// reconnects or drops a single message.
    #[error("websocket error: {0}")]
    WebSocket(String),

    /// A database failure: a query failed, a migration could not be applied, or
    /// the file is not a database.
    ///
    /// **A `String` payload for the same reason as [`ShNexusError::Network`].**
    /// `AGENTS.md` §3.3's `#[from] rusqlite::Error` needs `rusqlite`, which
    /// arrives in Phase 3, and `rusqlite/bundled` additionally compiles SQLite's
    /// C amalgamation -- a C toolchain requirement that has no business
    /// appearing in a pure-Rust type-foundation work unit.
    ///
    /// Recoverable only in the sense that the app must not panic: a corrupt
    /// local cache is a serious condition (`AGENTS.md` §7.5: `error!`) and the
    /// honest response is to report it, not to keep writing to it.
    #[error("database error: {0}")]
    Database(String),

    /// A serialization failure. A payload did not parse, or a value could not
    /// be encoded.
    ///
    /// This is the one I/O variant with its **real** `#[from]` source already in
    /// place, because `serde_json` is a dependency of this work unit for the wire
    /// boundary. Whether a given deserialization failure should surface as this
    /// or as [`ShNexusError::Protocol`] is a question about *where* it happened:
    /// this variant means "the bytes were not what we asked for", while
    /// `Protocol` means "the bytes were fine and the *content* was not
    /// acceptable". A malformed frame is a `Protocol` error, because §2.1's rule
    /// is about validating a payload's content, not about its grammar.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// An authentication failure: the credentials were rejected, the token has
    /// expired, or the session was revoked.
    ///
    /// The message is written for a user, not for a log: this is the text the
    /// login screen shows. Per §7.5 it must never contain the token itself, so a
    /// caller that puts a token in here has a bug that this type cannot catch.
    ///
    /// Not a `#[from]` variant because JWT handling does not exist yet
    /// (`PLAN.md` §2, Phase 4); there is no `jsonwebtoken::Error` to convert
    /// from, and inventing one now would be a variant nothing can construct.
    #[error("auth error: {0}")]
    Auth(String),

    /// A payload was structurally valid but semantically unacceptable.
    ///
    /// **This is the wire boundary's error**, and it is the busiest variant in
    /// the enum. `AGENTS.md` §2.1 requires every incoming WebSocket and REST
    /// payload to be validated against a schema before it touches state; a
    /// rejection from that validation is this variant, and it is what stops a
    /// malformed payload reaching `state/`.
    ///
    /// It covers, in `sh_nexus::network::mapping`: a blank or whitespace-only
    /// required id, a `client_msg_id` that is not a UUID, a timestamp outside the
    /// plausible window, a frame type arriving on the wrong socket, and a frame
    /// whose `v` this build does not speak. It also covers an `error` code the
    /// client does not recognise, which is passed through verbatim rather than
    /// discarded -- an opaque code the user can report beats a silently dropped
    /// explanation.
    ///
    /// The message names the offending **field**, never the offending **value**
    /// when the field is message content. `AGENTS.md` §7.5 forbids logging
    /// message content, and an error that can reach a log is subject to the same
    /// rule. Ids and enum-like values are included, because a developer cannot
    /// debug a rejection that does not say what was rejected.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A configuration failure: a theme file is missing, the config directory
    /// cannot be created, or a setting has an unusable value.
    ///
    /// Separate from [`ShNexusError::Theme`] because a *theme* is a
    /// user-authored, hot-reloadable, shareable artifact, and its failures have
    /// their own user-facing behaviour: `AGENTS.md` §10.2 requires an invalid
    /// theme to fall back to the default **with a message in the app**, not to
    /// fail. A config failure is usually not user-fixable at runtime and is
    /// usually a bug.
    #[error("config error: {0}")]
    Config(String),

    /// A theme failure: the JSON does not parse, a required colour key is
    /// missing, or a colour is not a valid format.
    ///
    /// Always recoverable by falling back to the default theme
    /// (`AGENTS.md` §10.2). Never panic, never leave the UI unstyled -- a chat
    /// client with the wrong colours is a working chat client, and one with no
    /// colours is not.
    #[error("theme error: {0}")]
    Theme(String),

    /// A failure this enum has no better home for.
    ///
    /// Also the landing place for the GPUI boundary conversion described in the
    /// module docs. Its `Display` prefix is "unknown error" because that is what
    /// `AGENTS.md` §3.3 specifies, and the practical consequence is that a
    /// variant which should have been one of the eight above is visible in a log
    /// as a smell.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// The result type used throughout the client.
///
/// `AGENTS.md` §9.1's doc-comment template writes `Result<T>`, and this is it.
pub type Result<T> = std::result::Result<T, ShNexusError>;
