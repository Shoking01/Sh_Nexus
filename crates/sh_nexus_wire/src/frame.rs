//! Frames, the versioned envelope, and the codec.
//!
//! `PLAN.md` §6 is the specification this module implements. Read the two
//! together: where this module and the plan disagree, the plan is the bug and
//! the fix goes through the ADR path (`AGENTS.md` §9.2).
//!
//! # The envelope
//!
//! Every frame is `{ "v": <major>, ...payload }`. The version lives on an
//! *envelope*, not on each frame variant, which is what makes "every frame
//! carries a version" (`AGENTS.md` §7.4) structural rather than a convention
//! repeated five times and eventually forgotten on the sixth variant.
//!
//! ```jsonc
//! { "v": 1, "type": "message.send", "client_msg_id": "…", "channel_id": "…", "content": "…" }
//! ```
//!
//! The two envelopes are not symmetrical, and the asymmetry is deliberate:
//!
//! | | [`ClientEnvelope`] | [`ServerEnvelope`] |
//! |---|---|---|
//! | `v` | present | present |
//! | `client_msg_id` | **always** present, on the envelope | only on the frames that answer a send |
//!
//! `AGENTS.md` §7.4 requires a `client_msg_id` on *every* client frame, for
//! idempotent dedup across reconnects -- including the ephemeral ones. It says
//! nothing of the sort about server frames, and `PLAN.md` §6 shows
//! `presence.update` and `typing.update` carrying no such field, because
//! there is nothing to acknowledge and nothing to dedup. So on the client side
//! the id is a field of the envelope, outside the payload enum, and a new
//! client frame cannot forget to carry one. On the server side it stays a field
//! of the two frames that need it. `PLAN.md` Rev 2 asserted this rule and then
//! omitted the id from three of its own five client frames, which is the defect
//! ADR-004 was written to close; the type system now closes it.
//!
//! # Decoding order, and why it is fixed
//!
//! [`ClientEnvelope::decode`] performs four steps, in this order, and the order
//! is the design:
//!
//! 1. Parse `{ v, type }` and nothing else.
//! 2. [`negotiate`](crate::version::negotiate) the `v`.
//! 3. Resolve `type` to a [`FrameKind`] and check its direction.
//! 4. Only now deserialize the full envelope.
//!
//! The version is checked *before* the body is touched, because a body parsed
//! under an unnegotiated version has already been interpreted. A peer running a
//! version whose `message.send` has a different field layout would otherwise be
//! silently half-understood, and the resulting corruption would surface much
//! later as a message attributed to the wrong user.
//!
//! # Compatibility policy
//!
//! `v` is a bare integer (`PLAN.md` §6), so there is no minor number to
//! negotiate, and the policy has to be stated rather than encoded. Within one
//! major version:
//!
//! - **New optional fields may be added.** The wire types never use
//!   `deny_unknown_fields`, so a peer ignores fields it does not know.
//! - **New frame types may be added only if losing one costs nothing.**
//!   `message.*` and `reaction.update` are data-bearing, so adding one requires
//!   a major bump. `typing.*`, `presence.*` and `error` are advisory, so a peer
//!   that meets an unknown one drops it with a `warn!` and keeps the connection
//!   open ([`WireError::UnknownFrameType`]).
//! - **New variants of a closed enum require a major bump**, because
//!   [`WireUserStatus`] is rendered and a client cannot render what it does not
//!   have a case for.
//! - **New `error` codes are additive.** `code` is a `String`, not an enum,
//!   precisely so a client built before a code existed can still read the frame
//!   and show the raw code to the user, instead of discarding the server's
//!   explanation because it did not recognise the label.

use std::fmt;
use std::str;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::dto::{WireMessage, WireUserStatus};
use crate::error::WireError;
use crate::version::{self, UnsupportedVersion, UNSUPPORTED_VERSION_CODE};

/// Which peer a frame travels between.
///
/// Used to reject a frame that arrives on the wrong socket. A client that
/// accepted a `message.send` from a server, or a `message.new` from a client,
/// would be acting on a frame the other side never meant to send it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// A frame the client sends to the server.
    Client,
    /// A frame the server sends to the client.
    Server,
}

impl fmt::Display for Direction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Client => "client",
            Self::Server => "server",
        })
    }
}

/// Every `type` value in the protocol, in both directions.
///
/// A closed enum rather than a bare `String`, so that `PLAN.md` §6's frame list
/// lives in exactly one place, so the direction of each type is a property of
/// the type system, and so a test can iterate all twelve and assert the codec's
/// behaviour on each.
///
/// [`FrameKind::CLIENT`] and [`FrameKind::SERVER`] hold the two lists, and
/// `tests/wire_frames.rs` asserts that every variant appears in exactly one of
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameKind {
    // ---- Client -> Server (PLAN.md section 6) ----
    /// `message.send` - submit a message for delivery.
    MessageSend,
    /// `reaction.add` - add this user's reaction to a message.
    ReactionAdd,
    /// `typing.start` - the user began typing in a channel.
    TypingStart,
    /// `typing.stop` - the user stopped typing, or switched channels.
    TypingStop,
    /// `resync` - request everything after a per-channel cursor.
    Resync,

    // ---- Server -> Client (PLAN.md section 6) ----
    /// `message.ack` - a `message.send` was accepted; carries the stored message.
    MessageAck,
    /// `message.new` - a message for delivery, from any author including this user.
    MessageNew,
    /// `message.error` - a `message.send` was rejected.
    MessageError,
    /// `reaction.update` - somebody reacted to a message.
    ReactionUpdate,
    /// `typing.update` - somebody started or stopped typing.
    TypingUpdate,
    /// `presence.update` - somebody came online, went away, or went offline.
    PresenceUpdate,
    /// `error` - a protocol-level error not attributable to one send.
    Error,
}

impl FrameKind {
    /// Every client-to-server frame type, in `PLAN.md` §6's declaration order.
    pub const CLIENT: [Self; 5] = [
        Self::MessageSend,
        Self::ReactionAdd,
        Self::TypingStart,
        Self::TypingStop,
        Self::Resync,
    ];

    /// Every server-to-client frame type, in `PLAN.md` §6's declaration order.
    pub const SERVER: [Self; 7] = [
        Self::MessageAck,
        Self::MessageNew,
        Self::MessageError,
        Self::ReactionUpdate,
        Self::TypingUpdate,
        Self::PresenceUpdate,
        Self::Error,
    ];

    /// The `type` string this kind serializes to and parses from.
    ///
    /// Exactly the spelling in `PLAN.md` §6, which is what the
    /// `#[serde(rename_all = "snake_case")]` on the frame enums produces. These
    /// constants and that attribute are two spellings of one fact, so
    /// `tests/wire_frames.rs` asserts the encoded JSON's `type` against this
    /// function for every variant -- if either side drifts, the test fails
    /// rather than the protocol.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MessageSend => "message.send",
            Self::ReactionAdd => "reaction.add",
            Self::TypingStart => "typing.start",
            Self::TypingStop => "typing.stop",
            Self::Resync => "resync",
            Self::MessageAck => "message.ack",
            Self::MessageNew => "message.new",
            Self::MessageError => "message.error",
            Self::ReactionUpdate => "reaction.update",
            Self::TypingUpdate => "typing.update",
            Self::PresenceUpdate => "presence.update",
            Self::Error => "error",
        }
    }

    /// The direction this kind travels.
    pub const fn direction(self) -> Direction {
        match self {
            Self::MessageSend
            | Self::ReactionAdd
            | Self::TypingStart
            | Self::TypingStop
            | Self::Resync => Direction::Client,
            Self::MessageAck
            | Self::MessageNew
            | Self::MessageError
            | Self::ReactionUpdate
            | Self::TypingUpdate
            | Self::PresenceUpdate
            | Self::Error => Direction::Server,
        }
    }

    /// Resolves a `type` string from a payload to a kind.
    ///
    /// Returns `None` for anything not in the protocol, which the caller turns
    /// into [`WireError::UnknownFrameType`]. Kept as `Option` rather than a
    /// `Result` with a `WireError` because the error needs the receiving
    /// direction, which is the caller's to supply.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::CLIENT
            .into_iter()
            .chain(Self::SERVER)
            .find(|kind| kind.as_str() == name)
    }

    /// Every kind in the protocol.
    pub const fn all() -> [Self; 12] {
        [
            Self::MessageSend,
            Self::ReactionAdd,
            Self::TypingStart,
            Self::TypingStop,
            Self::Resync,
            Self::MessageAck,
            Self::MessageNew,
            Self::MessageError,
            Self::ReactionUpdate,
            Self::TypingUpdate,
            Self::PresenceUpdate,
            Self::Error,
        ]
    }

    /// Checks that this kind travels in `expected`.
    ///
    /// # Errors
    ///
    /// [`WireError::WrongDirection`] if it does not.
    pub fn require_direction(self, expected: Direction) -> Result<(), WireError> {
        let received = self.direction();
        if received == expected {
            Ok(())
        } else {
            Err(WireError::WrongDirection {
                kind: self,
                kind_direction: received,
                expected_direction: expected,
            })
        }
    }
}

impl fmt::Display for FrameKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A frame the client sends to the server.
///
/// The `client_msg_id` is **not** here. It is on [`ClientEnvelope`], so it
/// cannot be omitted by a variant -- see the module docs.
///
/// # Why every variant renames itself explicitly
///
/// `PLAN.md` §6 spells the frame types with a **dot**: `message.send`,
/// `typing.update`, `presence.update`. serde's `rename_all` cannot produce a
/// dot -- `snake_case` yields `message_send` and `typing_update` -- so every
/// variant carries an explicit `#[serde(rename = "...")]` instead. This is not
/// belt-and-braces: an early draft of this crate used `rename_all = "snake_case"`
/// and emitted `"type":"typing_update"`, which is a protocol that matches no
/// other implementation of `PLAN.md` §6. A DTO whose JSON does not match the
/// documented protocol is worse than no DTO, because it looks like a working
/// protocol.
///
/// With the rename explicit, [`FrameKind::as_str`] and the serde attribute are
/// adjacent lines, and `tests/wire_frames.rs` asserts they agree for all twelve
/// frame types -- so a future rename that changes one without the other fails a
/// test rather than shipping.
///
/// # Example
///
/// ```
/// use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame};
///
/// let envelope = ClientEnvelope::new(
///     "6f0b1f8e-0e2a-4a5b-9c3d-1f2e3a4b5c6d",
///     ClientFrame::MessageSend {
///         channel_id: "general".to_owned(),
///         content: "hello team".to_owned(),
///     },
/// );
/// let json = envelope.encode().unwrap_or_default();
/// assert!(json.contains(r#""v":1"#));
/// assert!(json.contains(r#""type":"message.send""#));
/// assert!(json.contains(r#""client_msg_id":"6f0b1f8e-0e2a-4a5b-9c3d-1f2e3a4b5c6d""#));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientFrame {
    /// `message.send` - submit a message.
    ///
    /// The `client_msg_id` that travels with this frame is what lets the server
    /// dedupe a replayed send after a reconnect (`PLAN.md` §7), and what lets
    /// the client reconcile the `message.ack` with the optimistic row it
    /// rendered.
    #[serde(rename = "message.send")]
    MessageSend {
        /// Target channel. Non-blank.
        channel_id: String,
        /// Message body, as authored.
        content: String,
    },
    /// `reaction.add` - add this user's reaction to a message.
    ///
    /// There is no `reaction.remove` in `PLAN.md` §6. Removing a reaction is a
    /// Phase 5 concern and needs its own frame so the removal is idempotent the
    /// same way `reaction.add` is; inventing one now would be a protocol
    /// decision taken before anything can send it.
    #[serde(rename = "reaction.add")]
    ReactionAdd {
        /// The message being reacted to. Non-blank.
        message_id: String,
        /// The emoji. Non-blank.
        emoji: String,
    },
    /// `typing.start` - the user began typing in a channel.
    #[serde(rename = "typing.start")]
    TypingStart {
        /// The channel being typed in. Non-blank.
        channel_id: String,
    },
    /// `typing.stop` - the user stopped typing, or left the channel.
    ///
    /// Needed and not optional, because a `typing.start` with no matching stop
    /// leaves a stuck indicator. `AGENTS.md` §8.1's Typing Indicator Flow
    /// requires indicators to clear; the server's inactivity timeout is the
    /// backstop, not the mechanism.
    #[serde(rename = "typing.stop")]
    TypingStop {
        /// The channel being typed in. Non-blank.
        channel_id: String,
    },
    /// `resync` - request everything after a per-channel cursor.
    ///
    /// **Scoped per channel, and that is the whole point.** A single global
    /// cursor cannot reconstruct per-channel history, because each channel has
    /// its own `last_message_at` and its own unread count. With one global
    /// cursor, a client that had read `general` up to 10:00 and `#design` up to
    /// 09:00 has no way to say so: one cursor is either too old (replaying
    /// messages the user has already seen into `general`) or too new (skipping
    /// messages in `#design`). Both are failures, and `AGENTS.md` §8.1's
    /// Reconnect Flow requires "no duplicates, no gaps" -- which is
    /// unsatisfiable without a per-channel cursor. `PLAN.md` Rev 2 had a global
    /// one and ADR-004 records it as a real defect; this is the fix, in the type
    /// rather than in a comment.
    #[serde(rename = "resync")]
    Resync {
        /// The channel to catch up. Non-blank.
        channel_id: String,
        /// Exclusive lower bound: the client already holds messages up to and
        /// including this instant, and wants everything strictly after it.
        ///
        /// Inclusive here, exclusive at the server, so the boundary is stated
        /// once instead of twice. A `>` at the server means the last message
        /// the client holds is never re-sent -- which is the half of "no
        /// duplicates" that the client's own dedup on `client_msg_id` cannot
        /// provide, since it has never seen a message it was not sent.
        after: DateTime<Utc>,
    },
}

impl ClientFrame {
    /// This frame's `type` value.
    pub fn kind(&self) -> FrameKind {
        match self {
            Self::MessageSend { .. } => FrameKind::MessageSend,
            Self::ReactionAdd { .. } => FrameKind::ReactionAdd,
            Self::TypingStart { .. } => FrameKind::TypingStart,
            Self::TypingStop { .. } => FrameKind::TypingStop,
            Self::Resync { .. } => FrameKind::Resync,
        }
    }
}

/// A frame the server sends to the client.
///
/// # Example
///
/// ```
/// use sh_nexus_wire::frame::ServerFrame;
///
/// let json = serde_json::to_string(&ServerFrame::TypingUpdate {
///     user_id: "u_7".to_owned(),
///     channel_id: "general".to_owned(),
///     active: true,
/// }).unwrap_or_default();
/// assert_eq!(json, r#"{"type":"typing.update","user_id":"u_7","channel_id":"general","active":true}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerFrame {
    /// `message.ack` - a `message.send` was accepted.
    ///
    /// Carries the *stored* message, not the sent one, because the server is
    /// the authority on `id` and `timestamp`. The client reconciles its
    /// optimistic row against this; `AGENTS.md` §8.1's Optimistic Send Flow
    /// depends on the ack being the point where the message becomes real.
    #[serde(rename = "message.ack")]
    MessageAck {
        /// The `client_msg_id` of the `message.send` being acknowledged.
        client_msg_id: String,
        /// The message as stored.
        message: WireMessage,
    },
    /// `message.new` - a message for delivery, from any author.
    ///
    /// Includes the receiving user's own messages, echoed. An edit and a thread
    /// reply both arrive here too, as a whole `message`: `PLAN.md` §6 has no
    /// separate edit frame, so there is no other path by which the client could
    /// learn about one.
    #[serde(rename = "message.new")]
    MessageNew {
        /// The message.
        message: WireMessage,
    },
    /// `message.error` - a `message.send` was rejected.
    ///
    /// Terminal for that send: the client moves it to `DeliveryState::Failed`
    /// and keeps it visible for retry. `PLAN.md` §7: failures are never
    /// silently dropped.
    #[serde(rename = "message.error")]
    MessageError {
        /// The `client_msg_id` of the `message.send` being rejected.
        client_msg_id: String,
        /// Machine-readable reason.
        code: String,
        /// Human-readable detail. Shown to the user.
        detail: String,
    },
    /// `reaction.update` - somebody reacted to a message.
    #[serde(rename = "reaction.update")]
    ReactionUpdate {
        /// The message reacted to. Non-blank.
        message_id: String,
        /// The emoji. Non-blank.
        emoji: String,
        /// Who reacted. Non-blank.
        user_id: String,
    },
    /// `typing.update` - somebody started or stopped typing.
    #[serde(rename = "typing.update")]
    TypingUpdate {
        /// Who is typing. Non-blank.
        user_id: String,
        /// Where. Non-blank.
        channel_id: String,
        /// `true` on start, `false` on stop.
        active: bool,
    },
    /// `presence.update` - somebody's presence changed.
    #[serde(rename = "presence.update")]
    PresenceUpdate {
        /// Whose presence changed. Non-blank.
        user_id: String,
        /// The new presence.
        status: WireUserStatus,
    },
    /// `error` - a protocol-level error not attributable to one send.
    ///
    /// Also the vehicle for a version rejection (see
    /// [`ServerEnvelope::version_rejection`]), and for failures that are the
    /// connection's rather than a message's -- an expired token, a revoked
    /// session, a server shutting down.
    ///
    /// `code` is a `String` and not an enum on purpose. An enum would be tidier
    /// to match on, and it would mean a client built before a code existed
    /// cannot deserialize the frame that explains what went wrong -- so the
    /// server's explanation would be discarded exactly when the user needs it.
    /// Codes are open within a major version; the one this build *generates* is
    /// [`UNSUPPORTED_VERSION_CODE`], and the client's boundary maps the codes it
    /// recognises and passes the rest through as an opaque code.
    #[serde(rename = "error")]
    Error {
        /// Machine-readable reason.
        code: String,
        /// Human-readable detail. Shown to the user.
        detail: String,
    },
}

impl ServerFrame {
    /// This frame's `type` value.
    pub fn kind(&self) -> FrameKind {
        match self {
            Self::MessageAck { .. } => FrameKind::MessageAck,
            Self::MessageNew { .. } => FrameKind::MessageNew,
            Self::MessageError { .. } => FrameKind::MessageError,
            Self::ReactionUpdate { .. } => FrameKind::ReactionUpdate,
            Self::TypingUpdate { .. } => FrameKind::TypingUpdate,
            Self::PresenceUpdate { .. } => FrameKind::PresenceUpdate,
            Self::Error { .. } => FrameKind::Error,
        }
    }
}

/// A versioned client-to-server frame.
///
/// Carries `v` and `client_msg_id` outside the payload, so both `AGENTS.md`
/// §7.4's requirements are properties of the type rather than of each variant.
///
/// # Example
///
/// ```
/// use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame};
///
/// let envelope = ClientEnvelope::new("id-1", ClientFrame::TypingStop { channel_id: "general".into() });
/// assert_eq!(envelope.v, 1);
/// assert_eq!(envelope.client_msg_id, "id-1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEnvelope {
    /// Protocol major version. Every frame carries it (`AGENTS.md` §7.4).
    ///
    /// Public so the server, and tests, can construct a deliberately
    /// mismatched frame; [`ClientEnvelope::new`] stamps
    /// [`PROTOCOL_VERSION`](crate::version::PROTOCOL_VERSION) so ordinary
    /// callers cannot get it wrong by accident.
    pub v: u16,
    /// The client-generated id for this frame, as untrusted text.
    ///
    /// **Required on every client frame**, which is why it is here and not in
    /// [`ClientFrame`]. A frame without one fails to decode.
    ///
    /// Whether it *parses* as a UUID is the domain's rule, not this crate's --
    /// see the `dto` module docs. This crate guarantees presence; the client
    /// guarantees validity, in `network/mapping::parse_client_msg_id`.
    pub client_msg_id: String,
    /// The frame itself.
    #[serde(flatten)]
    pub frame: ClientFrame,
}

/// A versioned server-to-client frame.
///
/// Not symmetrical with [`ClientEnvelope`]: server frames carry `v` but not a
/// blanket `client_msg_id`, because `PLAN.md` §6 gives one only to the two
/// frames that answer a send. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEnvelope {
    /// Protocol major version. Every frame carries it (`AGENTS.md` §7.4).
    pub v: u16,
    /// The frame itself.
    #[serde(flatten)]
    pub frame: ServerFrame,
}

impl ClientEnvelope {
    /// Builds an envelope stamped with this build's protocol version.
    ///
    /// # Arguments
    ///
    /// * `client_msg_id` - the client-generated id. See the field's docs.
    /// * `frame` - the frame to send.
    pub fn new(client_msg_id: impl Into<String>, frame: ClientFrame) -> Self {
        Self {
            v: version::PROTOCOL_VERSION,
            client_msg_id: client_msg_id.into(),
            frame,
        }
    }

    /// This envelope's frame `type` value.
    pub fn kind(&self) -> FrameKind {
        self.frame.kind()
    }

    /// Encodes to the JSON text that goes into a WebSocket text frame.
    ///
    /// # Returns
    ///
    /// The encoded frame.
    ///
    /// # Errors
    ///
    /// [`WireError::MalformedPayload`] if serialization fails. It cannot fail
    /// for any value these types can hold -- there are no map keys that are not
    /// strings and no floats, which are the only two things `serde_json` rejects
    /// structurally -- but it is returned rather than unwrapped because
    /// `AGENTS.md` §2.1 forbids a panic in a production path, and an
    /// `expect("cannot fail")` in a network send path is exactly the kind of
    /// claim that stops being true when a field is added in Phase 4.
    pub fn encode(&self) -> Result<String, WireError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Decodes one client-to-server frame.
    ///
    /// # Arguments
    ///
    /// * `json` - the text of a WebSocket text frame.
    ///
    /// # Returns
    ///
    /// The decoded envelope.
    ///
    /// # Errors
    ///
    /// [`WireError`], in the order the checks are applied -- see the module docs
    /// for why that order matters and what the caller owes each variant.
    pub fn decode(json: &str) -> Result<Self, WireError> {
        validate_frame_header(json, Direction::Client)?;
        Ok(serde_json::from_str(json)?)
    }

    /// Decodes one client-to-server frame from raw bytes.
    ///
    /// WebSocket text frames arrive as bytes, and a peer may send a *binary*
    /// frame where a text frame is required. Rather than letting that surface as
    /// a confusing JSON error, it is reported as
    /// [`WireError::InvalidUtf8`] -- see that variant for why the two have
    /// different causes and different fixes.
    ///
    /// # Errors
    ///
    /// [`WireError::InvalidUtf8`] if `bytes` is not UTF-8, otherwise as
    /// [`ClientEnvelope::decode`].
    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        Self::decode(
            str::from_utf8(bytes).map_err(|error| WireError::InvalidUtf8(error.to_string()))?,
        )
    }
}

impl ServerEnvelope {
    /// Builds an envelope stamped with this build's protocol version.
    ///
    /// # Arguments
    ///
    /// * `frame` - the frame to send.
    pub fn new(frame: ServerFrame) -> Self {
        Self {
            v: version::PROTOCOL_VERSION,
            frame,
        }
    }

    /// The `error` frame that rejects an incompatible peer.
    ///
    /// The frame to send, followed by closing the socket, when
    /// [`negotiate`](crate::version::negotiate) has refused a peer's version.
    /// `PLAN.md` §6: *"the server responds with an `error` frame and closes."*
    ///
    /// **The rejection advertises this build's version, never the peer's.**
    /// The peer's announced version is in `detail`, as text. Sending `"v": 2` to
    /// a v2 client to say "I do not speak v2" would frame the response in a
    /// dialect that cannot be decoded by the recipient of a version error --
    /// which is precisely the population most likely to be too old to read a
    /// structured field. The one field a stale client can always act on is the
    /// one it can parse with the version it already has.
    ///
    /// # Arguments
    ///
    /// * `reason` - the rejection from
    ///   [`negotiate`](crate::version::negotiate), which carries both the
    ///   version received and the set supported.
    pub fn version_rejection(reason: &UnsupportedVersion) -> Self {
        Self::new(ServerFrame::Error {
            code: UNSUPPORTED_VERSION_CODE.to_owned(),
            detail: reason.detail(),
        })
    }

    /// This envelope's frame `type` value.
    pub fn kind(&self) -> FrameKind {
        self.frame.kind()
    }

    /// Encodes to the JSON text that goes into a WebSocket text frame.
    ///
    /// # Errors
    ///
    /// As [`ClientEnvelope::encode`].
    pub fn encode(&self) -> Result<String, WireError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Decodes one server-to-client frame.
    ///
    /// # Errors
    ///
    /// [`WireError`], with the checks applied in the order the module docs
    /// specify.
    pub fn decode(json: &str) -> Result<Self, WireError> {
        validate_frame_header(json, Direction::Server)?;
        Ok(serde_json::from_str(json)?)
    }

    /// Decodes one server-to-client frame from raw bytes.
    ///
    /// # Errors
    ///
    /// [`WireError::InvalidUtf8`] if `bytes` is not UTF-8, otherwise as
    /// [`ServerEnvelope::decode`].
    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        Self::decode(
            str::from_utf8(bytes).map_err(|error| WireError::InvalidUtf8(error.to_string()))?,
        )
    }
}

/// The part of a frame the codec must read before it may read anything else.
///
/// `type` is a `String` rather than a [`FrameKind`] on purpose: an unrecognised
/// `type` has to become [`WireError::UnknownFrameType`], not a serde "unknown
/// variant" error, because the two mean different things to whoever reads the
/// log. `v` is a `u16` because `PLAN.md` §6 defines it as a bare integer; a
/// float or a string there is a malformed frame, not a version.
#[derive(Debug, Deserialize)]
struct RawHeader {
    v: u16,
    #[serde(rename = "type")]
    kind: String,
}

/// Runs steps 1 to 3 of the decode order: parse the header, negotiate the
/// version, resolve the frame kind and its direction.
///
/// Shared by both directions so the two cannot drift, which is the same
/// argument `sh_nexus_wire` as a whole crate exists to make. Returns nothing
/// because the resolved kind is not needed by the caller -- the header's `type`
/// and the payload's `type` are the same field of the same JSON document, so
/// resolving the kind twice would produce the same answer and comparing them
/// would be a check that cannot fail. What *can* fail is the version, the
/// unknown `type`, and the direction, and those are what this validates.
fn validate_frame_header(json: &str, expected_direction: Direction) -> Result<(), WireError> {
    // Step 1: the header only. Every other field in `json` is ignored here.
    let header: RawHeader = serde_json::from_str(json)?;

    // Step 2: the version, before the body is interpreted at all.
    version::negotiate(header.v)?;

    // Step 3: the frame type, and whether it travels the way we are reading.
    let kind = FrameKind::from_name(&header.kind).ok_or_else(|| WireError::UnknownFrameType {
        name: header.kind.clone(),
        expected_direction,
    })?;
    kind.require_direction(expected_direction)?;

    Ok(())
}
