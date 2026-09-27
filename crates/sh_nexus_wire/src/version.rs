//! Protocol versioning and version negotiation.
//!
//! # Why this module exists
//!
//! `AGENTS.md` §7.4 requires two things that are easy to state and easy to get
//! subtly wrong:
//!
//! 1. *Every* WebSocket frame carries a version.
//! 2. An **unknown major version is rejected explicitly** -- not ignored, not
//!    best-effort parsed, not accepted on a hope that the fields happen to
//!    line up.
//!
//! The second requirement is the one that is usually faked. A decoder that
//! parses the body and *then* looks at the version has already done the damage
//! it was supposed to prevent: it has interpreted a payload from a dialect it
//! does not understand, and whatever it decided is now indistinguishable from
//! a decision about a payload it did understand. So the version is checked
//! **first**, before the frame's `type` is resolved and before a single field
//! of the body is deserialized. [`negotiate`] is the only way to obtain a
//! [`ProtocolVersion`], and [`ProtocolVersion`] cannot be constructed any other
//! way -- so "the version was checked" is a property of the type, not of a code
//! review.
//!
//! # The version number
//!
//! `PLAN.md` §6 specifies `"v": 1` -- a bare JSON integer, not a dotted
//! `major.minor` string. That is what the protocol on the wire is, so that is
//! what is implemented here: **`v` is the protocol major version and nothing
//! else.** There is no minor field, because the documented format has no room
//! for one.
//!
//! The compatibility policy that follows from "a major version is a bare
//! integer" is stated in the crate root, because it governs the frame types in
//! `frame` as much as it governs this module. In short: evolution *within* a
//! major is additive-only, and additive changes that would lose data require a
//! major bump.
//!
//! # Multi-version builds
//!
//! [`SUPPORTED_MAJOR_VERSIONS`] is a slice rather than a single constant so
//! that a future build which can speak two majors -- to ease a rollout, or to
//! survive a client that lags a server deploy -- widens one list. No call site,
//! no error type, and no frame type has to change, and the rejection path
//! already reports the full set back to the peer.
//!
//! # Examples
//!
//! ```
//! use sh_nexus_wire::version::{negotiate, PROTOCOL_VERSION};
//!
//! // The version this build speaks is accepted.
//! assert_eq!(
//!     negotiate(PROTOCOL_VERSION).map(|version| version.get()),
//!     Ok(PROTOCOL_VERSION),
//! );
//!
//! // Anything else is an explicit, reportable rejection.
//! assert!(negotiate(PROTOCOL_VERSION + 1).is_err());
//! ```

use std::fmt;

use thiserror::Error;

/// The protocol major version this build speaks.
///
/// Bump this only together with a change to
/// [`SUPPORTED_MAJOR_VERSIONS`](self::SUPPORTED_MAJOR_VERSIONS) and a note in
/// `docs/API.md`; `AGENTS.md` §5.3 requires a WebSocket protocol change to be
/// documented there.
pub const PROTOCOL_VERSION: u16 = 1;

/// Every major version this build can speak, in ascending order.
///
/// A slice, not a constant, so a build that can speak more than one major
/// widens this list and nothing else. See the module docs.
pub const SUPPORTED_MAJOR_VERSIONS: &[u16] = &[PROTOCOL_VERSION];

/// The `code` carried by the `error` frame that rejects an incompatible peer.
///
/// Sent as an `error` frame with [`UNSUPPORTED_VERSION_CODE`] and a `detail`
/// naming both the peer's version and the set this build speaks, after which
/// the socket is closed. A client that cannot close the socket can still show
/// the detail, which is the whole point of sending it rather than just hanging
/// up.
pub const UNSUPPORTED_VERSION_CODE: &str = "unsupported_version";

/// A protocol major version that [`negotiate`] has accepted.
///
/// Constructible only by [`negotiate`] and [`ProtocolVersion::CURRENT`]. The
/// private field is the enforcement mechanism: there is no path by which a
/// caller obtains a `ProtocolVersion` for a version this build does not speak,
/// so no decoder downstream can accidentally skip the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolVersion(u16);

impl ProtocolVersion {
    /// The major version this build speaks.
    ///
    /// Provided so a caller that needs to stamp a frame -- every frame carries
    /// `v` -- cannot reach for [`PROTOCOL_VERSION`] and drift from what
    /// [`negotiate`] accepts.
    pub const CURRENT: Self = Self(PROTOCOL_VERSION);

    /// The major version, as it appears in a frame's `v` field.
    ///
    /// # Returns
    ///
    /// The value to write into `v`. Always a member of
    /// [`SUPPORTED_MAJOR_VERSIONS`].
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A version that was not accepted, with everything needed to explain the
/// rejection to the peer and to the user.
///
/// The `supported` set is carried rather than looked up at display time so that
/// a `detail` built from this value is an accurate record of the decision, even
/// if a wider set is added later.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "unsupported protocol version {received}; this build speaks major version(s) {supported:?}"
)]
pub struct UnsupportedVersion {
    /// The major version the peer announced in its frame's `v` field.
    pub received: u16,
    /// The major versions this build accepts, from
    /// [`SUPPORTED_MAJOR_VERSIONS`].
    pub supported: &'static [u16],
}

impl UnsupportedVersion {
    /// The `detail` string for the `error` frame that rejects this peer.
    ///
    /// # Returns
    ///
    /// A sentence naming the peer's announced version and the set this build
    /// speaks. It deliberately does not suggest that reconnecting will help:
    /// a version mismatch is not transient, and telling the user to retry
    /// would send them into a reconnect loop against a server that will reject
    /// them identically every time.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus_wire::version::{negotiate, PROTOCOL_VERSION, UnsupportedVersion};
    ///
    /// let rejection = negotiate(PROTOCOL_VERSION + 7)
    ///     .expect_err("a major version this build does not speak must be rejected");
    /// assert_eq!(rejection.received, PROTOCOL_VERSION + 7);
    /// assert!(rejection.detail().contains("8"));
    /// ```
    pub fn detail(&self) -> String {
        format!(
            "peer announced protocol major version {}; this client speaks {:?}. \
             The two versions are incompatible: update the client or the server. \
             Reconnecting will not resolve this.",
            self.received, self.supported
        )
    }
}

/// Checks a peer's announced major version against what this build speaks.
///
/// This is the negotiation. It is the *only* gate between a raw frame and the
/// deserialization of that frame's body.
///
/// # Arguments
///
/// * `received` - the value of the frame's `v` field.
///
/// # Returns
///
/// The accepted [`ProtocolVersion`], which is the only way to obtain one.
///
/// # Errors
///
/// [`UnsupportedVersion`] if `received` is not in
/// [`SUPPORTED_MAJOR_VERSIONS`]. Callers must respond with an `error` frame
/// carrying [`UNSUPPORTED_VERSION_CODE`] and close the socket; `AGENTS.md` §7.4
/// says "reject explicitly", and a silent close is indistinguishable from a
/// network fault, so the user would retry forever against a server that will
/// refuse identically.
///
/// # Example
///
/// ```
/// use sh_nexus_wire::version::{negotiate, PROTOCOL_VERSION};
///
/// assert!(negotiate(PROTOCOL_VERSION).is_ok());
/// assert!(negotiate(0).is_err());
/// assert!(negotiate(u16::MAX).is_err());
/// ```
pub fn negotiate(received: u16) -> Result<ProtocolVersion, UnsupportedVersion> {
    if SUPPORTED_MAJOR_VERSIONS.contains(&received) {
        Ok(ProtocolVersion(received))
    } else {
        Err(UnsupportedVersion {
            received,
            supported: SUPPORTED_MAJOR_VERSIONS,
        })
    }
}
