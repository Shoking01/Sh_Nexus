//! The wire DTOs -- exactly what crosses the socket, and nothing else.
//!
//! # What a DTO is for
//!
//! These types are the protocol's *syntax*. They are not the application's
//! model, they are not what the UI renders, and they carry no invariant the
//! server has not already promised. `PLAN.md` §5 splits the two on purpose:
//! `sh_nexus_wire` owns "what crosses the socket", `core/models` owns "what the
//! app is", and `sh_nexus::network::mapping` is the one place the first becomes
//! the second.
//!
//! That split is what lets the client and the server share a type system
//! (ADR-002) without sharing an opinion about product. It is also what makes
//! validation possible at all: a DTO is untrusted text, a domain type is
//! trusted data, and a conversion between them has somewhere to put the checks
//! that keep untrusted text out of trusted data.
//!
//! # Why these are `Vec` and not `SmallVec`
//!
//! `PLAN.md` §5 types a message's reactions as `SmallVec<[Reaction; 2]>`. The
//! domain keeps that, because a rendered message's layout genuinely wants the
//! first two reactions inline. The DTO does not: the wire crate is compiled by
//! the server, which stores messages in rows and has no interest in a chat
//! bubble's memory layout, and baking an inline-capacity optimisation into a
//! serialization struct would make the DTO's memory cost depend on a rendering
//! decision made in a different crate. `Vec` is the honest shape for a value
//! whose job is to be encoded and decoded, and the conversion in
//! `network/mapping.rs` is where the domain's opinion gets applied.
//!
//! # Why `client_msg_id` is a `String` and not a `Uuid`
//!
//! `AGENTS.md` §7.4 requires a client-generated UUID on every client frame. It
//! does not require the *wire type* to be `Uuid`, and it should not be. This
//! crate never trusts its input -- that is the premise of the whole module --
//! so it carries the id as the text it actually is, and the rule that it
//! parses as a UUID is enforced where trusted data is created, in
//! `sh_nexus::network::mapping::parse_client_msg_id`.
//!
//! The alternative -- `client_msg_id: Uuid` on the DTO -- would push the
//! rejection into serde, where it becomes indistinguishable from a JSON syntax
//! error and leaves nothing for the domain to assert. See the `client_msg_id`
//! note in that module for the consequence: this crate's invariant is
//! *presence* (a `client_msg_id` that is absent fails to decode, because the
//! field is required), while *validity* is the domain's.
//!
//! # Timestamps
//!
//! Every timestamp is `DateTime<Utc>`, serialized by chrono's own serde impl as
//! RFC-3339 -- never a hand-formatted string, and never a raw integer. See
//! `PLAN.md` §3 for why. Note what this means for the *decoder*: chrono's impl
//! accepts several RFC-3339 spellings, so a payload written as
//! `2026-09-27T12:00:00+00:00` and one written as `2026-09-27T12:00:00Z` are the
//! same instant on the wire but different bytes. That is RFC-3339 working as
//! specified; the round-trip tests pin the canonical `Z` form this project
//! emits.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A user account as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireUser {
    /// Server-assigned user id. Non-blank; validated at the boundary.
    pub id: String,
    /// Login handle, unique and lowercased by the server. Non-blank.
    pub username: String,
    /// Human-facing name. Non-blank, and need not be unique.
    pub display_name: String,
    /// Absolute URL of the avatar, if the user has set one. Optional because
    /// "no avatar" is the default state, not an error.
    pub avatar_url: Option<String>,
    /// Presence, carried inline so a user list arrives in one frame.
    pub status: WireUserStatus,
}

/// A user's presence, as it appears on the wire.
///
/// **Closed on purpose.** `PLAN.md` §6 specifies the values as lowercase JSON
/// strings (`"online"`), so this is a serde enum rather than a bare `String`.
///
/// A closed enum here is a compatibility commitment, and it is the reason a new
/// presence value requires a major version bump. That is the correct
/// trade: a client that meets an unknown status cannot render it, so the honest
/// behaviour is to fail rather than to display a fabricated one, and the only
/// way to guarantee that fails everywhere is to make the type closed. The cost
/// is that a future sixth status is a protocol change. The benefit is that a
/// user never sees a presence indicator driven by a value the client invented.
///
/// # Example
///
/// ```
/// use sh_nexus_wire::dto::WireUserStatus;
///
/// // `PLAN.md` section 6 specifies lowercase JSON.
/// let json = serde_json::to_string(&WireUserStatus::Away).unwrap_or_default();
/// assert_eq!(json, "\"away\"");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireUserStatus {
    /// Connected and active.
    Online,
    /// Connected but idle past the server's away threshold.
    Away,
    /// Not connected.
    Offline,
}

/// A channel as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireChannel {
    /// Server-assigned channel id. Non-blank; validated at the boundary.
    pub id: String,
    /// Display name, unique per workspace. Non-blank.
    pub name: String,
    /// Optional topic/purpose text. Absent is normal, not an error.
    pub description: Option<String>,
    /// Whether membership is restricted. Drives the UI's lock affordance; the
    /// server enforces it, and the client never acts on this field for an
    /// authorization decision.
    pub is_private: bool,
    /// Member user ids, in no guaranteed order.
    ///
    /// Order is not specified because nothing may depend on it: the sidebar
    /// sorts by display name, and the member count is order-independent. A
    /// protocol that pinned an order would be making a promise about the
    /// server's query plan.
    pub members: Vec<String>,
    /// Timestamp of the most recent message, or `None` for a channel that has
    /// never had one.
    ///
    /// **This is the per-channel resume cursor.** `AGENTS.md` §7.4 requires
    /// reconnection to resume from it, and `PLAN.md` §7 is explicit that the
    /// resync is scoped per channel. See the `resync` comment on
    /// [`ClientFrame`](crate::frame::ClientFrame::Resync) for why a single
    /// global cursor cannot work.
    pub last_message_at: Option<DateTime<Utc>>,
}

/// A message as it appears on the wire.
///
/// `PLAN.md` §5 lists the fields; nothing is added or renamed. The two that
/// carry a rule are documented in place: [`WireMessage::client_msg_id`], which
/// the domain parses as a UUID, and [`WireMessage::thread_id`], whose presence
/// is what makes a message a thread reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMessage {
    /// Server-assigned message id. Non-blank; validated at the boundary. Note
    /// this is *not* the `client_msg_id`: it is minted by the server on
    /// acceptance and is what other clients address the message by.
    pub id: String,
    /// Echo of the client-generated id from the originating `message.send`, as
    /// untrusted text. The domain parses this as a `Uuid`; see the module docs
    /// for why the DTO does not.
    pub client_msg_id: String,
    /// Channel the message belongs to. Non-blank.
    pub channel_id: String,
    /// Author's user id. Non-blank.
    pub user_id: String,
    /// Message body, as authored. The client renders it as markdown and never
    /// executes anything in it (`PLAN.md` §5).
    pub content: String,
    /// When the server accepted the message. This is *not* the authoring time
    /// the client optimistically renders: a message queued in the outbox while
    /// offline can be accepted long after the user typed it, and the ordering
    /// guarantees are defined on this field.
    pub timestamp: DateTime<Utc>,
    /// When the message was last edited, or `None` if never.
    pub edited_at: Option<DateTime<Utc>>,
    /// Reactions on the message, in no guaranteed order.
    pub reactions: Vec<WireReaction>,
    /// Parent message id when this is a thread reply, or `None` for a
    /// top-level message.
    ///
    /// An edit arrives as a *whole* `message.new` with this field unchanged, so
    /// the UI learns about edits from the same path as new messages. There is
    /// no separate edit frame, and `PLAN.md` §6 has none.
    pub thread_id: Option<String>,
    /// Files attached to the message.
    pub attachments: Vec<WireAttachment>,
}

/// One emoji's reactions, grouped by emoji, as it appears on the wire.
///
/// Grouped rather than a flat list of `(user, emoji)` pairs because the UI
/// renders one chip per emoji with a count, and a flat list would make the
/// renderer aggregate on every frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireReaction {
    /// The emoji, as a short string of Unicode scalar values. Non-blank;
    /// validated at the boundary.
    ///
    /// A `String` rather than a grapheme cluster type because a reaction key
    /// must be byte-identical across clients to aggregate correctly, and no
    /// crate in this project's dependency set provides a canonical
    /// normalisation. Fixing a normalisation policy is a protocol decision and
    /// belongs in a version bump, not in a type.
    pub emoji: String,
    /// User ids who reacted with this emoji. Order is not specified; the UI
    /// shows a count and, on hover, a list it sorts itself.
    pub user_ids: Vec<String>,
}

/// A file attached to a message, as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireAttachment {
    /// Server-assigned attachment id, used to request a download URL. Non-blank.
    pub id: String,
    /// Name as uploaded, for display. Non-blank.
    pub filename: String,
    /// Absolute download URL. Non-blank. The client fetches it through
    /// `reqwest` with certificate validation on (`PLAN.md` §3).
    pub url: String,
    /// IANA media type, e.g. `image/png`. Non-blank. Advisory: the client picks
    /// a preview renderer from it and falls back to a generic chip when it
    /// does not recognise the type, so an unknown value is not an error.
    pub mime_type: String,
    /// Size in bytes. The client's boundary rejects values above
    /// `sh_nexus::network::mapping::MAX_ATTACHMENT_BYTES`; this crate only notes
    /// that a `u64` read off the network can be up to 18 exabytes, which is
    /// certainly a bug rather than a file.
    pub size: u64,
}
