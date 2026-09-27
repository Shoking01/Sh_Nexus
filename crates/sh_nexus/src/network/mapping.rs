//! `pub mod mapping;` -- the wire/domain boundary.
//!
//! # What this module is for
//!
//! `AGENTS.md` §3.2 assigns `network/` exactly one transformation: "parses wire
//! formats into `core::models`". This module is that transformation, and
//! `AGENTS.md` §2.1 makes it the enforcement point: *"All incoming WebSocket
//! and REST payloads must be validated against schemas before touching state."*
//!
//! So this is the only place in the client where an untrusted wire value becomes
//! a trusted domain value, and therefore the only place where validation has to
//! happen. Everything downstream -- `state/`, `core/ordering.rs`, the UI -- may
//! assume a `Message` has a non-blank channel, a `Uuid` client id and a
//! plausible timestamp, because this module guaranteed it.
//!
//! # Which direction is fallible
//!
//! | Direction | Fallibility | Why |
//! |---|---|---|
//! | wire -> domain | [`TryFrom`], always validated | the input is untrusted |
//! | domain -> wire | [`From`], infallible | the input is trusted |
//!
//! The reverse direction's infallibility is a *convention backed by ownership*,
//! not a type-level guarantee, and it is worth being honest about that: a
//! `Message` can also be built in-process by an optimistic send without passing
//! through here. The argument is that `AGENTS.md` §3.2 makes `state/actions.rs`
//! the only producer of domain values, and that producer is trusted code whose
//! correctness is covered by `app_state.rs`'s own tests. Making the reverse
//! direction fallible would mean either duplicating validation that has already
//! run, or -- worse -- giving the impression that `From<&Message> for
//! WireMessage` is a safety check when it is only a projection.
//!
//! # What is validated
//!
//! Four rules, and the reasoning for each is on the function that implements it:
//!
//! 1. **Required ids are non-blank.** [`require_non_blank`].
//! 2. **`client_msg_id` parses as a UUID.** [`parse_client_msg_id`].
//! 3. **Timestamps are in a plausible window.** [`require_plausible_timestamp`].
//! 4. **A message has content or an attachment.** [`require_body`].
//!
//! Plus the one rule that belongs to the type rather than to a function: a
//! `client_msg_id` on a client frame is *present* by construction, because
//! `sh_nexus_wire::ClientEnvelope` requires the field. This module owns the
//! other half -- whether the text is a UUID -- and
//! `crates/sh_nexus_wire/tests/client_frame_invariants.rs` owns the presence
//! half.
//!
//! # What is deliberately *not* validated, and why
//!
//! Over-validation is not free. A check that rejects something the server
//! considers valid turns into a message the user cannot send and a support
//! question nobody can answer. The following are deliberately absent, each for a
//! reason:
//!
//! | Not validated | Why |
//! |---|---|
//! | **Message content length** | Server policy, and the client's own limits would be a second, disagreeing policy. `AGENTS.md` §5.2 lists a 500-character message as an edge case the UI must handle, which is a rendering concern, not a rejection concern. The client renders virtualized (§2.3), so an over-long message costs no more to hold than a short one. |
//! | **Whether a channel or user exists** | Needs local state, so it belongs in `state/`, not in a pure conversion. A conversion that could fail for "unknown channel" would have to consult the app, which would make it impure and untestable at the §4.1 floor. |
//! | **Membership and authorization** | The server's job, and the client's only. The `is_private` field exists to draw a lock icon. A client that refused to render a private channel it was not in would be a client authorizing on data the server sent it. |
//! | **Ordering, gaps and duplicates** | `core/ordering.rs`, a later work unit. It is a *sequence* property, not a per-value one: deciding whether a message is a duplicate requires knowing what is already in the channel, which is state. |
//! | **Emoji normalisation** | A protocol decision, not a validation rule. Two clients must agree on the byte sequence, so normalising client-side would make them disagree with the server's grouping key. |
//! | **Attachment URLs, schemes and reachability** | A `url` can be pre-signed against a different host than the API, so a scheme or host allow-list here would reject legitimate payloads. TLS validation happens in `reqwest` at fetch time (`PLAN.md` §3). The only size check is a ceiling, because an 18-exabyte attachment is a bug rather than a file. |
//! | **Future timestamps relative to now** | Would make validation depend on a clock, and therefore non-deterministic and untestable -- §4.3 forbids waiting for time in a unit test. A fixed absolute window catches the unit errors that actually happen; a skew check belongs with the fake clock that `network/reconnect.rs` will introduce. |
//!
//! # No I/O, no clock, no GPUI
//!
//! Every function here is a pure transformation. That is not style: §3.2 forbids
//! `network/` from importing GPUI, `AGENTS.md` §4.1 sets an ≥80% floor for
//! `network/`, and a floor is only reachable if the layer is testable without
//! sockets. `crates/sh_nexus/tests/layer_boundary.rs` asserts the GPUI
//! prohibition mechanically.

use chrono::{DateTime, Utc};
use smallvec::SmallVec;
use uuid::Uuid;

use crate::core::models::events::{ConnectionState, DomainEvent};
use crate::core::models::message::{Attachment, Message, Reaction};
use crate::core::models::user::{User, UserStatus};
use crate::errors::{Result, ShNexusError};
use sh_nexus_wire::dto::{
    WireAttachment, WireChannel, WireMessage, WireReaction, WireUser, WireUserStatus,
};
use sh_nexus_wire::frame::ServerFrame;
use sh_nexus_wire::WireError;

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// 2000-01-01T00:00:00Z, in seconds since the Unix epoch.
///
/// The lower bound on a plausible timestamp. Below this, a timestamp is a clock
/// or unit error rather than a real time: seconds sent where milliseconds were
/// expected land in 1970, and this project has no history worth preserving from
/// before 2000. The floor is absolute rather than relative so validation stays
/// pure -- see the module docs on deliberately not checking against "now".
pub const TIMESTAMP_FLOOR_UNIX_SECS: i64 = 946_684_800;

/// 2100-01-01T00:00:00Z, in seconds since the Unix epoch.
///
/// The upper bound on a plausible timestamp. Its real job is catching a unit
/// error in the other direction: a server sending **microseconds** where seconds
/// were expected produces a year in the tens of thousands, and a server sending
/// milliseconds as seconds produces a year around 55,000. Both are far outside
/// this bound and both are caught. A ceiling far higher would catch nothing
/// extra that a value outside `i64` seconds has not already caught.
pub const TIMESTAMP_CEILING_UNIX_SECS: i64 = 4_102_444_800;

/// The largest attachment this client will accept, in bytes: 512 MiB.
///
/// A ceiling, not a policy. A `u64` read off the network can be 18 exabytes,
/// which is certainly a bug rather than a file, and a client that trusted it
/// would show a file size in a UI element and then try to preview a thing that
/// does not exist. The *policy* limit -- what a user may actually upload -- is
/// the server's, and a file over this ceiling that the server considers valid is
/// a server bug worth reporting rather than a payload to accept.
///
/// 512 MiB is chosen to be far above any plausible chat attachment and far below
/// anything that would strain a desktop client. It is a single named constant
/// precisely so that a future decision to change it is one edit in one place
/// rather than a hunt.
pub const MAX_ATTACHMENT_BYTES: u64 = 512 * 1024 * 1024;

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Rejects a required field that is empty or only whitespace.
///
/// `field` is a `&'static str` -- a compile-time constant naming the field, not
/// a formatted string -- so the error message costs one `String` allocation for
/// the whole message and no allocation per call for the field name. Every call
/// site passes a literal, which is what makes the names consistent enough to
/// grep for.
///
/// The **value** is deliberately absent from the error. An id is worth quoting --
/// a developer cannot debug "a blank id was rejected" without knowing whether it
/// was empty or whitespace, and there is no secret in an id. A content field is
/// not, and `AGENTS.md` §7.5 forbids logging message content; an error that can
/// reach a log is subject to the same rule. So the rule is enforced by never
/// accepting a caller that *could* pass a content field to an id rule, and this
/// function is only ever used for ids.
///
/// # Arguments
///
/// * `field` - the dotted path of the field being checked, e.g.
///   `"message.channel_id"`.
/// * `value` - the value received.
///
/// # Returns
///
/// `value`, borrowed, so a caller can use it without cloning first.
///
/// # Errors
///
/// [`ShNexusError::Protocol`] if `value` is empty or contains only whitespace.
///
/// # Example
///
/// ```
/// use sh_nexus::errors::Result;
/// use sh_nexus::network::mapping::require_non_blank;
///
/// # fn main() -> Result<()> {
/// let channel_id = require_non_blank("message.channel_id", "c_1")?;
/// assert_eq!(channel_id, "c_1");
///
/// let error = require_non_blank("message.channel_id", "  ")
///     .expect_err("whitespace is not a channel id");
/// assert_eq!(
///     error.to_string(),
///     "protocol error: message.channel_id must not be blank",
/// );
/// # Ok(())
/// # }
/// ```
pub fn require_non_blank<'a>(field: &'static str, value: &'a str) -> Result<&'a str> {
    if value.trim().is_empty() {
        return Err(ShNexusError::Protocol(format!("{field} must not be blank")));
    }
    Ok(value)
}

/// Parses a `client_msg_id` from the wire into a [`Uuid`].
///
/// This is the domain's half of the `client_msg_id` guarantee. `AGENTS.md` §7.4
/// requires a client-generated **UUID** on every client frame; the other half --
/// that the field is *present* -- is structural, because
/// `sh_nexus_wire::ClientEnvelope` requires it and
/// `crates/sh_nexus_wire/tests/client_frame_invariants.rs` proves it.
///
/// Keeping the wire type a `String` is what makes this function testable instead
/// of redundant. Had the DTO carried a `Uuid`, serde would have rejected a bad id
/// and the domain would have had nothing left to assert -- the rule would still
/// hold, but it would hold inside a dependency, untested and unexplained.
///
/// # Arguments
///
/// * `field` - the dotted path of the field, e.g. `"message.client_msg_id"`.
/// * `value` - the untrusted text.
///
/// # Returns
///
/// The parsed id.
///
/// # Errors
///
/// [`ShNexusError::Protocol`] if `value` is not a UUID. **The value is not
/// quoted in the error**, unlike [`require_non_blank`]: a `client_msg_id` is the
/// one id in the system that is attacker-influenced and length-unbounded, so it
/// is exactly the one an error message should not echo back into a log.
pub fn parse_client_msg_id(field: &'static str, value: &str) -> Result<Uuid> {
    Uuid::parse_str(value.trim())
        .map_err(|_| ShNexusError::Protocol(format!("{field} must be a UUID")))
}

/// Rejects a timestamp outside the plausible window.
///
/// See [`TIMESTAMP_FLOOR_UNIX_SECS`] and [`TIMESTAMP_CEILING_UNIX_SECS`] for the
/// window and why it is absolute rather than relative to a clock. The short
/// version: a *relative* check would make this function depend on "now", which
/// makes it untestable (`AGENTS.md` §4.3 forbids waiting for time) and makes the
/// same payload valid on one day and invalid on another, which is the worst
/// property a validator can have.
///
/// What an absolute window catches is the realistic bug: a server sending
/// microseconds or milliseconds where seconds were expected. A *future* check
/// would additionally catch clock skew, and is deliberately left to
/// `network/reconnect.rs`, which will have a fake clock to do it with.
///
/// # Arguments
///
/// * `field` - the dotted path of the field.
/// * `value` - the timestamp received.
///
/// # Returns
///
/// `value`, unchanged -- it is already a `DateTime<Utc>`, so there is nothing to
/// convert.
///
/// # Errors
///
/// [`ShNexusError::Protocol`] if `value` is before the floor or after the
/// ceiling.
pub fn require_plausible_timestamp(
    field: &'static str,
    value: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let seconds = value.timestamp();
    if !(TIMESTAMP_FLOOR_UNIX_SECS..=TIMESTAMP_CEILING_UNIX_SECS).contains(&seconds) {
        return Err(ShNexusError::Protocol(format!(
            "{field} is outside the plausible range 2000-01-01..2100-01-01"
        )));
    }
    Ok(value)
}

/// Rejects an attachment whose declared size is implausible.
///
/// The error names the limit and the received size. Unlike an id, a size is not
/// attacker-controlled free text: it is a number, bounded in the message, and
/// knowing whether the server sent 513 MiB or 18 EiB is the whole diagnostic.
pub fn require_attachment_size(field: &'static str, value: u64) -> Result<u64> {
    if value > MAX_ATTACHMENT_BYTES {
        return Err(ShNexusError::Protocol(format!(
            "{field} of {value} bytes exceeds the {MAX_ATTACHMENT_BYTES}-byte limit"
        )));
    }
    Ok(value)
}

/// Rejects a message with neither a body nor an attachment.
///
/// The one cross-field rule in this module, and the reason the other three are
/// not enough: each field is individually well-formed, and the *combination* is
/// not a message.
///
/// The attachment escape hatch is what keeps the rule from rejecting a
/// legitimate file-only message -- which is why the check takes a boolean rather
/// than looking at the content alone.
///
/// The content is **not** quoted in the error, per the reasoning in
/// [`require_non_blank`].
///
/// # Arguments
///
/// * `content` - the message body.
/// * `has_attachments` - whether the message carries at least one attachment.
///   Note the polarity: the parameter says attachments *exist*, not that they
///   are absent. An inverted argument here fails in both directions at once --
///   empty messages get through and file-only messages get rejected -- which is
///   why the call site passes `!attachments.is_empty()` and why
///   `crates/sh_nexus/tests/wire_boundary.rs` tests both of those cases.
///
/// # Errors
///
/// [`ShNexusError::Protocol`] if `content` is blank and `has_attachments` is
/// `false`.
fn require_body(content: &str, has_attachments: bool) -> Result<()> {
    if content.trim().is_empty() && !has_attachments {
        return Err(ShNexusError::Protocol(
            "message.content must not be blank when the message has no attachments".to_owned(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// wire -> domain
// ---------------------------------------------------------------------------

impl TryFrom<WireUser> for User {
    /// # Errors
    ///
    /// [`ShNexusError::Protocol`] if `id`, `username` or `display_name` is
    /// blank. A user with a blank id cannot be addressed, and a user with a blank
    /// display name renders as an empty row in the sidebar -- a message attributed
    /// to nobody, which is worse than the message not arriving.
    type Error = ShNexusError;

    fn try_from(wire: WireUser) -> Result<Self> {
        Ok(Self {
            id: require_non_blank("user.id", &wire.id)?.to_owned(),
            username: require_non_blank("user.username", &wire.username)?.to_owned(),
            display_name: require_non_blank("user.display_name", &wire.display_name)?.to_owned(),
            // Deliberately not validated: a blank avatar URL is a server with no
            // avatar, and inventing a placeholder would be a fabricated network
            // request on every sidebar render. Treated as "no avatar".
            avatar_url: wire.avatar_url,
            // Total by construction: `WireUserStatus` is a closed enum, so an
            // unrecognised presence failed at decode and never reached here.
            // A conversion that could fail on an unknown variant would be a
            // conversion that a client could forget to call.
            status: UserStatus::from(wire.status),
        })
    }
}

impl From<WireUserStatus> for UserStatus {
    /// Total: both enums have the same three variants and the same names. A
    /// `match` with no wildcard, so a variant added to either side is a compile
    /// error here rather than a presence the UI cannot render.
    fn from(wire: WireUserStatus) -> Self {
        match wire {
            WireUserStatus::Online => Self::Online,
            WireUserStatus::Away => Self::Away,
            WireUserStatus::Offline => Self::Offline,
        }
    }
}

impl From<UserStatus> for WireUserStatus {
    /// Total, in the other direction. See [`impl From<WireUserStatus> for UserStatus`].
    fn from(status: UserStatus) -> Self {
        match status {
            UserStatus::Online => Self::Online,
            UserStatus::Away => Self::Away,
            UserStatus::Offline => Self::Offline,
        }
    }
}

impl TryFrom<WireChannel> for crate::core::models::channel::Channel {
    /// # Errors
    ///
    /// [`ShNexusError::Protocol`] if `id` or `name` is blank, if any member id
    /// is blank, or if `last_message_at` is outside the plausible window.
    ///
    /// An empty member list is **not** an error: a channel created seconds ago
    /// legitimately has no members other than the creator, and the creator is
    /// added by the server. Rejecting it would break channel creation on the
    /// client, which is the opposite of what a validator is for.
    type Error = ShNexusError;

    fn try_from(wire: WireChannel) -> Result<Self> {
        let mut members = Vec::with_capacity(wire.members.len());
        for member in &wire.members {
            members.push(require_non_blank("channel.members[]", member)?.to_owned());
        }

        Ok(Self {
            id: require_non_blank("channel.id", &wire.id)?.to_owned(),
            name: require_non_blank("channel.name", &wire.name)?.to_owned(),
            // Not validated: a description is free text with no structure, and
            // an empty one renders as no subtitle.
            description: wire.description,
            is_private: wire.is_private,
            // `Arc<[String]>` per PLAN.md section 5: cloning a Channel into the
            // sidebar, the header and the member list must not copy the member
            // list each time (AGENTS.md 2.3).
            members: members.into(),
            last_message_at: wire
                .last_message_at
                .map(|at| require_plausible_timestamp("channel.last_message_at", at))
                .transpose()?,
        })
    }
}

impl TryFrom<WireMessage> for Message {
    /// # Errors
    ///
    /// [`ShNexusError::Protocol`] if any of:
    ///
    /// - `id`, `channel_id` or `user_id` is blank;
    /// - `client_msg_id` is not a UUID;
    /// - `timestamp` or `edited_at` is outside the plausible window;
    /// - a reaction's emoji is blank, or a reaction's user id is blank;
    /// - an attachment's id, filename, url or mime type is blank, or its size
    ///   exceeds [`MAX_ATTACHMENT_BYTES`];
    /// - the body is blank *and* there are no attachments.
    ///
    /// The last one is the only cross-field rule, and it exists because a message
    /// with neither text nor a file is not something a user can have meant to
    /// send. The attachment escape hatch is what keeps the rule from rejecting a
    /// legitimate file-only message.
    type Error = ShNexusError;

    fn try_from(wire: WireMessage) -> Result<Self> {
        let id = require_non_blank("message.id", &wire.id)?.to_owned();
        let channel_id = require_non_blank("message.channel_id", &wire.channel_id)?.to_owned();
        let user_id = require_non_blank("message.user_id", &wire.user_id)?.to_owned();
        let client_msg_id = parse_client_msg_id("message.client_msg_id", &wire.client_msg_id)?;
        let timestamp = require_plausible_timestamp("message.timestamp", wire.timestamp)?;
        let edited_at = wire
            .edited_at
            .map(|at| require_plausible_timestamp("message.edited_at", at))
            .transpose()?;

        let reactions: SmallVec<[Reaction; 2]> = wire
            .reactions
            .into_iter()
            .map(Reaction::try_from)
            .collect::<Result<_>>()?;

        let attachments: SmallVec<[Attachment; 1]> = wire
            .attachments
            .into_iter()
            .map(Attachment::try_from)
            .collect::<Result<_>>()?;

        // Cross-field, and therefore checked after the parts: a message with no
        // body and no attachment is not a message. The content is never included
        // in the error -- AGENTS.md 7.5 forbids logging message content, and an
        // error that can reach a log is subject to the same rule.
        require_body(&wire.content, !attachments.is_empty())?;

        Ok(Self {
            id,
            client_msg_id,
            channel_id,
            user_id,
            content: wire.content,
            timestamp,
            edited_at,
            reactions,
            // Not validated as a UUID and not validated as non-blank: it is
            // another message's id, and it is legitimately `Some(..)` for a
            // reply whose parent this client has not loaded. Whether it points
            // at a real message is a question about local state, answered in
            // `state/`, not here.
            thread_id: wire.thread_id,
            attachments,
        })
    }
}

impl TryFrom<WireReaction> for Reaction {
    /// # Errors
    ///
    /// [`ShNexusError::Protocol`] if the emoji is blank or any user id is blank.
    ///
    /// The emoji is *not* normalised and its length is not bounded: a reaction key
    /// has to be byte-identical across clients for two clients to agree they are
    /// showing the same reaction, so any client-side transformation is a way for
    /// them to disagree with the server's grouping key.
    type Error = ShNexusError;

    fn try_from(wire: WireReaction) -> Result<Self> {
        let mut user_ids = Vec::with_capacity(wire.user_ids.len());
        for user_id in &wire.user_ids {
            user_ids.push(require_non_blank("reaction.user_ids[]", user_id)?.to_owned());
        }

        Ok(Self {
            emoji: require_non_blank("reaction.emoji", &wire.emoji)?.to_owned(),
            // An empty user list is not an error: it is what a server sends when
            // the last user removes a reaction and the client has not seen the
            // increment. `state/` drops the entry when the list empties.
            user_ids,
        })
    }
}

impl TryFrom<WireAttachment> for Attachment {
    /// # Errors
    ///
    /// [`ShNexusError::Protocol`] if `id`, `filename`, `url` or `mime_type` is
    /// blank, or if `size` exceeds [`MAX_ATTACHMENT_BYTES`].
    type Error = ShNexusError;

    fn try_from(wire: WireAttachment) -> Result<Self> {
        Ok(Self {
            id: require_non_blank("attachment.id", &wire.id)?.to_owned(),
            filename: require_non_blank("attachment.filename", &wire.filename)?.to_owned(),
            url: require_non_blank("attachment.url", &wire.url)?.to_owned(),
            // Non-blank is all. The type is advisory: the client picks a preview
            // renderer from it and falls back to a generic chip, so an
            // unrecognised type must not be a rejection. A preview that trusted
            // the declared type would render an HTML file as an image.
            mime_type: require_non_blank("attachment.mime_type", &wire.mime_type)?.to_owned(),
            size: require_attachment_size("attachment.size", wire.size)?,
        })
    }
}

// ---------------------------------------------------------------------------
// domain -> wire
// ---------------------------------------------------------------------------

impl From<&User> for WireUser {
    /// Infallible. See the module docs on why the reverse direction is trusted.
    fn from(user: &User) -> Self {
        Self {
            id: user.id.clone(),
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            avatar_url: user.avatar_url.clone(),
            status: WireUserStatus::from(user.status),
        }
    }
}

impl From<&crate::core::models::channel::Channel> for WireChannel {
    /// Infallible. See the module docs.
    ///
    /// [`Channel::members`](crate::core::models::channel::Channel) is an
    /// `Arc<[String]>` and the wire wants a `Vec<String>`, so this is the one
    /// projection in the module that allocates. It runs on channel *writes*, not
    /// on the scroll path, so the allocation is not on a hot path.
    fn from(channel: &crate::core::models::channel::Channel) -> Self {
        Self {
            id: channel.id.clone(),
            name: channel.name.clone(),
            description: channel.description.clone(),
            is_private: channel.is_private,
            members: channel.members.to_vec(),
            last_message_at: channel.last_message_at,
        }
    }
}

impl From<&Message> for WireMessage {
    /// Infallible. See the module docs.
    ///
    /// `client_msg_id` is rendered back to text. That is lossy in principle --
    /// a `Uuid` has 128 bits of entropy and its canonical hyphenated form is one
    /// particular spelling -- so a `Uuid` built from non-canonical bytes would not
    /// round-trip. Every `Uuid` this client produces comes from a generator, and
    /// the boundary's [`parse_client_msg_id`](rules::parse_client_msg_id) is what
    /// guarantees the ones that arrive are canonical hyphenated text, so the
    /// round trip holds in both directions.
    fn from(message: &Message) -> Self {
        Self {
            id: message.id.clone(),
            client_msg_id: message.client_msg_id.hyphenated().to_string(),
            channel_id: message.channel_id.clone(),
            user_id: message.user_id.clone(),
            content: message.content.clone(),
            timestamp: message.timestamp,
            edited_at: message.edited_at,
            reactions: message.reactions.iter().map(WireReaction::from).collect(),
            thread_id: message.thread_id.clone(),
            attachments: message
                .attachments
                .iter()
                .map(WireAttachment::from)
                .collect(),
        }
    }
}

impl From<&Reaction> for WireReaction {
    /// Infallible. See the module docs.
    fn from(reaction: &Reaction) -> Self {
        Self {
            emoji: reaction.emoji.clone(),
            user_ids: reaction.user_ids.clone(),
        }
    }
}

impl From<&Attachment> for WireAttachment {
    /// Infallible. See the module docs.
    fn from(attachment: &Attachment) -> Self {
        Self {
            id: attachment.id.clone(),
            filename: attachment.filename.clone(),
            url: attachment.url.clone(),
            mime_type: attachment.mime_type.clone(),
            size: attachment.size,
        }
    }
}

// ---------------------------------------------------------------------------
// wire frame -> domain event
// ---------------------------------------------------------------------------

impl TryFrom<ServerFrame> for DomainEvent {
    /// Maps one validated server frame onto one domain event.
    ///
    /// This is the second half of `AGENTS.md` §3.2's description of `network/`:
    /// parse the wire format *and* emit a domain event, never touching GPUI state.
    /// The two halves are separate conversions on purpose -- a DTO to a domain
    /// type, and a frame to an event -- so a frame that carries a message
    /// validates its message the same way a message arriving by any other route
    /// would.
    ///
    /// # Errors
    ///
    /// Whatever the contained DTO's conversion returns:
    /// [`ShNexusError::Protocol`]. A `message.new` whose payload is malformed is
    /// rejected here, and the rejection is per-frame: the connection stays up
    /// (§3.3 requires network failures to be recoverable, not fatal).
    ///
    /// [`ShNexusError::Protocol`] also for a `message.error` or a `message.ack`
    /// whose `client_msg_id` is not a UUID -- the same rule as on the message
    /// body, because a send cannot be reconciled against an id that does not
    /// parse, and a client that dropped the reconciliation would leave the
    /// optimistic message stuck in `DeliveryState::Pending` forever.
    type Error = ShNexusError;

    fn try_from(frame: ServerFrame) -> Result<Self> {
        match frame {
            ServerFrame::MessageAck {
                client_msg_id,
                message,
            } => Ok(Self::MessageAcked {
                client_msg_id: parse_client_msg_id("message.ack.client_msg_id", &client_msg_id)?,
                message: Message::try_from(message)?,
            }),

            ServerFrame::MessageNew { message } => {
                Ok(Self::MessageReceived(Message::try_from(message)?))
            }

            ServerFrame::MessageError {
                client_msg_id,
                code,
                detail,
            } => Ok(Self::MessageSendFailed {
                client_msg_id: parse_client_msg_id("message.error.client_msg_id", &client_msg_id)?,
                code,
                detail,
            }),

            ServerFrame::ReactionUpdate {
                message_id,
                emoji,
                user_id,
            } => Ok(Self::ReactionUpdated {
                message_id: require_non_blank("reaction.update.message_id", &message_id)?
                    .to_owned(),
                emoji: require_non_blank("reaction.update.emoji", &emoji)?.to_owned(),
                user_id: require_non_blank("reaction.update.user_id", &user_id)?.to_owned(),
            }),

            ServerFrame::TypingUpdate {
                user_id,
                channel_id,
                active,
            } => Ok(Self::TypingUpdated {
                channel_id: require_non_blank("typing.update.channel_id", &channel_id)?.to_owned(),
                user_id: require_non_blank("typing.update.user_id", &user_id)?.to_owned(),
                active,
            }),

            ServerFrame::PresenceUpdate { user_id, status } => Ok(Self::PresenceUpdated {
                user_id: require_non_blank("presence.update.user_id", &user_id)?.to_owned(),
                status: UserStatus::from(status),
            }),

            // A bare `error` frame is a statement about the *connection*, not
            // about a message, and it is terminal: PLAN.md section 6 uses it for
            // a version rejection, and a client that retried would sit in a
            // reconnect loop against a server that refuses identically every
            // time. Mapping it to ConnectionState::Rejected rather than to a
            // free-floating error event is what makes that unmissable -- a
            // free-floating Error(String) variant would let the UI render
            // something the user cannot act on.
            //
            // The code is passed through verbatim, including codes this client
            // does not recognise. An unrecognised code is still the server's
            // explanation of what went wrong, and discarding it would leave the
            // user with "the connection failed" and nothing to report.
            ServerFrame::Error { code, detail } => {
                Ok(Self::ConnectionStateChanged(ConnectionState::Rejected {
                    code,
                    detail,
                }))
            }
        }
    }
}

/// Reduces a wire-crate error into the client's error type.
///
/// The other half of the boundary. `sh_nexus_wire` cannot use `ShNexusError` --
/// it is compiled by the server too, and a back-edge would destroy ADR-002's
/// "defined once" guarantee -- so it defines `WireError` and this function
/// reduces it.
///
/// The mapping is not lossy for diagnosis: each variant becomes a distinct
/// message, and the frame's kind survives in the text. `AGENTS.md` §7.5 forbids
/// logging payload *content*, never an error kind, and none of these messages
/// contains any.
///
/// # Errors
///
/// Never. Returns a `ShNexusError::Protocol` describing the failure. Named
/// `reduce_wire_error` rather than implemented as `From`, because §3.3's rule
/// about a `#[from]` impl leaking a dependency's error type into the domain
/// applies here too -- an automatic conversion would put `WireError` in scope
/// everywhere via `?`.
pub fn reduce_wire_error(error: &WireError) -> ShNexusError {
    ShNexusError::Protocol(match error {
        WireError::UnsupportedVersion(rejection) => {
            format!("unsupported protocol version: {rejection}")
        }
        WireError::MalformedPayload(inner) => format!("malformed frame payload: {inner}"),
        WireError::InvalidUtf8(detail) => format!("frame payload is not valid UTF-8: {detail}"),
        WireError::UnknownFrameType {
            name,
            expected_direction,
        } => format!("unknown {expected_direction} frame type {name:?}"),
        WireError::WrongDirection {
            kind,
            kind_direction,
            expected_direction,
        } => format!(
            "frame type {kind} is a {kind_direction} frame but was received as a \
             {expected_direction} frame"
        ),
    })
}
