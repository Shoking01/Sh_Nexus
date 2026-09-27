//! The error type of the wire crate.
//!
//! # Why this is not `ShNexusError`
//!
//! `AGENTS.md` §3.3 makes `ShNexusError` the project's global error enum, and
//! it looks like every fallible operation should return it. It cannot be used
//! here. `ShNexusError` lives in `sh_nexus`, the client; this crate is compiled
//! by the *server* as well (ADR-002), so using the client's error type would
//! mean the client and the server both depend on `sh_nexus` -- the back-edge
//! that would destroy the "protocol defined once" guarantee ADR-002 exists to
//! provide. See the note at the top of this crate's `Cargo.toml`.
//!
//! So [`WireError`] is defined the same way `AGENTS.md` §3.3 defines
//! `ShNexusError` -- `thiserror`, one `#[error]` message per variant -- and
//! `sh_nexus::network` reduces it into `ShNexusError::Protocol` at the
//! boundary. The reduction is lossless for diagnosis: every variant here maps to
//! a distinct message, and `AGENTS.md` §7.5 forbids logging payload *content*,
//! never an error kind.
//!
//! # What is and is not an error here
//!
//! Not every failure to understand a frame is an error the caller should treat
//! as fatal, and conflating the two is how a client ends up disconnecting every
//! time a server adds an advisory frame. The variants split along that line:
//!
//! | Variant | Caller's obligation |
//! |---|---|
//! | [`WireError::UnsupportedVersion`] | Reject explicitly: send the `error` frame, close the socket. `AGENTS.md` §7.4. |
//! | [`WireError::MalformedPayload`] | Drop the frame, `warn!`, keep the connection. It may be a bug, a proxy, or an attack. |
//! | [`WireError::InvalidUtf8`] | Drop the frame, `warn!`, keep the connection. Same reasoning. |
//! | [`WireError::UnknownFrameType`] | Drop the frame, `warn!`, keep the connection. Expected during a rolling deploy within one major version. |
//! | [`WireError::WrongDirection`] | Drop the frame, `warn!`, keep the connection. A peer is speaking the wrong protocol role. |
//!
//! Only [`WireError::UnsupportedVersion`] is unrecoverable, and it is the only
//! one a caller must act on structurally rather than by logging.
//!
//! **Why the other four are non-fatal even when they could indicate data loss.**
//! A frame this build cannot decode is a frame it cannot render, and inventing
//! a value for it would be worse than dropping it: a fabricated placeholder in a
//! chat transcript is a lie the user cannot detect, whereas a dropped frame is
//! visible as a gap. The protocol's own compatibility policy (crate root)
//! removes the risk instead of papering over it -- any change whose loss would
//! be data-bearing requires a major version bump, so within one major the only
//! frames a peer may meet and not understand are advisory ones, where a drop
//! costs nothing.

use thiserror::Error;

use crate::frame::{Direction, FrameKind};
use crate::version::UnsupportedVersion;

/// Everything that can go wrong turning bytes into a frame.
///
/// No `Clone`, `PartialEq` or `Eq`: [`WireError::MalformedPayload`] wraps a
/// [`serde_json::Error`], which implements none of them. That is a real
/// constraint rather than an oversight -- the consequence is that a test
/// asserting *which* failure occurred must match on the variant
/// (`assert!(matches!(...))`) instead of comparing whole errors, which is what
/// the tests here do. Matching is the better assertion anyway: it states the
/// expectation, whereas `==` would also have to state the whole error message
/// and would break on an unrelated wording change.
#[derive(Debug, Error)]
pub enum WireError {
    /// The frame's version was not accepted by
    /// [`negotiate`](crate::version::negotiate).
    ///
    /// The one unrecoverable variant: see the module docs for the caller's
    /// obligations and for why every other variant here is a drop.
    #[error("{0}")]
    UnsupportedVersion(#[from] UnsupportedVersion),

    /// The frame is not well-formed JSON, or a field is missing or has the
    /// wrong shape.
    ///
    /// Note that an *unknown value for a closed enum* also arrives here rather
    /// than as its own variant: `WireUserStatus` is a closed enum, so serde
    /// rejects `"away_bathing"` as a malformed value. That is deliberate. An
    /// unknown status is not something the client could render if it wanted to,
    /// and a closed enum makes that impossible to forget at the type level.
    #[error("malformed frame payload: {0}")]
    MalformedPayload(#[from] serde_json::Error),

    /// The payload is not valid UTF-8.
    ///
    /// Kept separate from [`WireError::MalformedPayload`] because the two have
    /// different causes and different fixes. Invalid UTF-8 is what a
    /// non-UTF-8 WebSocket *binary* frame looks like, and WebSocket text frames
    /// are required to be UTF-8 -- so receiving this means the peer sent a
    /// binary frame where a text frame was required, or something between the
    /// two ends is transcoding. A JSON error here would say "expected value",
    /// which sends a reader looking in the wrong place entirely.
    ///
    /// The wrapped text is the [`std::str::Utf8Error`] `Display`, which names
    /// the invalid sequence's length and its byte index and never echoes the
    /// bytes -- `AGENTS.md` §7.5 forbids logging payload content.
    #[error("frame payload is not valid UTF-8: {0}")]
    InvalidUtf8(String),

    /// The frame's `type` is not a frame type in this protocol.
    ///
    /// `expected_direction` is the direction the decoder was reading, and is
    /// carried in the error because "unknown `type`" and "known `type`, wrong
    /// direction" are different bugs: the first is a version skew the
    /// compatibility policy does not cover, the second is two peers wired to
    /// each other with the roles swapped.
    #[error("unknown {expected_direction} frame type {name:?}")]
    UnknownFrameType {
        /// The `type` value as it appeared in the payload.
        name: String,
        /// The direction the receiving end was decoding.
        expected_direction: Direction,
    },

    /// The frame's `type` is a real frame type, but of the other direction.
    ///
    /// A `message.send` arriving from a server, or a `message.new` arriving
    /// from a client, is a protocol role violation. It is rejected here rather
    /// than deserialized, because a client that accepts `message.new` from a
    /// client is a client that will insert a message into its own history
    /// without the server having accepted it.
    #[error("frame type {kind} is a {kind_direction} frame, but a {expected_direction} frame was expected")]
    WrongDirection {
        /// The `type` that was found.
        kind: FrameKind,
        /// The direction the `type` actually belongs to.
        kind_direction: Direction,
        /// The direction the receiving end was decoding.
        expected_direction: Direction,
    },
}

impl WireError {
    /// Whether this error means the connection cannot continue.
    ///
    /// The single question a reconnect loop needs to ask, and the reason the
    /// variant split above exists. `AGENTS.md` §8.1's Reconnect Flow requires
    /// the client to back off and resume; backing off is only correct for a
    /// recoverable error, and retrying a version rejection forever is a
    /// reconnect loop against a server that will refuse identically every time.
    ///
    /// # Returns
    ///
    /// `true` only for [`WireError::UnsupportedVersion`]. Every other variant
    /// is a single bad frame, not a broken connection.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus_wire::version::{negotiate, PROTOCOL_VERSION};
    /// use sh_nexus_wire::WireError;
    ///
    /// let rejected = negotiate(PROTOCOL_VERSION + 1)
    ///     .expect_err("a future major version must be rejected");
    /// let error = WireError::from(rejected);
    /// assert!(error.is_fatal());
    ///
    /// let malformed = WireError::MalformedPayload(
    ///     serde_json::from_str::<u8>("{").expect_err("truncated JSON must not parse"),
    /// );
    /// assert!(!malformed.is_fatal());
    /// ```
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::UnsupportedVersion(_))
    }
}
