//! A message, in the domain.

use chrono::{DateTime, Utc};
use smallvec::SmallVec;
use uuid::Uuid;

/// A chat message.
///
/// The fields are `PLAN.md` §5's, unchanged. Two of them carry a rule worth
/// stating where a reader will hit it:
///
/// - [`Message::client_msg_id`] is a **`Uuid`**, where the wire DTO carries a
///   `String`. The wire is untrusted text; this is validated, typed data. See
///   `sh_nexus_wire::dto`'s module docs for why the split is there.
/// - [`Message::reactions`] and [`Message::attachments`] are `SmallVec`, not
///   `Vec`. `AGENTS.md` §2.3 names "reactions on a message" as the canonical
///   case: a typical message has zero, one or two reactions, and a message list
///   holds thousands of messages, so the inline buffer is the difference between
///   one and two heap allocations per message in the scroll path.
///
/// # What is not on this type
///
/// `DeliveryState` -- pending, acked, failed -- is **client state** and lives in
/// `state/app_state.rs`, not here. `PLAN.md` §5 says so explicitly, and §3.2 puts
/// all mutations in `state/actions.rs`. A `Message` in this crate means "a
/// message that exists"; whether *this* client has finished sending it is a fact
/// about this client's connection, and two clients holding the same message will
/// legitimately disagree. Putting the flag on the message would make the
/// domain type mean two different things depending on who is looking at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Server-assigned identifier, unique and never reused. What other clients
    /// address this message by, and the stable key for a rendered row.
    pub id: String,
    /// The client-generated id of the `message.send` that produced this.
    ///
    /// `AGENTS.md` §7.4 requires a client-generated UUID on every client frame
    /// so the server can dedupe a replayed send across a reconnect. This is that
    /// UUID, carried through optimistic send, ACK reconciliation and resync
    /// unchanged. It is **not** [`Message::id`]: that one is minted by the
    /// server on acceptance, and this one exists before the server has seen the
    /// message at all -- which is exactly why it is the one that can dedupe.
    ///
    /// A resync from the server also carries it, which is what lets the client
    /// recognise a message that was sent optimistically, re-sent after a
    /// reconnect, and delivered back by the resync as the *same* message.
    pub client_msg_id: Uuid,
    /// The channel this message belongs to.
    pub channel_id: String,
    /// The author. An `id`, not a `User`: a rendered list must not hold a copy
    /// of every author, and the author may be edited or have their presence
    /// change without this message changing.
    pub user_id: String,
    /// The message body, as authored.
    ///
    /// Markdown is **rendered, never executed** (`PLAN.md` §5). Parsing happens
    /// in `core/markdown.rs` in a later work unit; nothing here interprets the
    /// text, and in particular nothing here trusts it.
    pub content: String,
    /// When the server accepted the message.
    ///
    /// This is *not* the time the user hit Enter. A message queued in the outbox
    /// while offline is accepted long after it was typed, and every ordering
    /// guarantee is defined on this field -- so it is the one the message list
    /// sorts by, not the one the composer optimistically displays.
    pub timestamp: DateTime<Utc>,
    /// When the message was last edited, or `None` if it never was.
    ///
    /// An edit arrives as a *whole* new value of this struct carrying the
    /// original [`Message::id`] and [`Message::client_msg_id`], because
    /// `PLAN.md` §6 has no edit frame. `core/ordering.rs` -- a later work unit
    /// -- reconciles it; nothing in this crate does.
    pub edited_at: Option<DateTime<Utc>>,
    /// Reactions on this message, grouped by emoji.
    ///
    /// `SmallVec<[Reaction; 2]>` per §2.3: the buffer holds the first two inline,
    /// which covers the overwhelming majority of messages at zero allocation, and
    /// spills to the heap for the rest. A third reaction is not an error, just
    /// the point where the inline buffer runs out.
    ///
    /// Order is unspecified. Nothing may depend on it, because the server is
    /// free to send reactions in any order and the UI sorts by count.
    pub reactions: SmallVec<[Reaction; 2]>,
    /// The message this one replies to, or `None` for a top-level message.
    ///
    /// An `Option<String>` rather than a link to another `Message`: a thread
    /// reply's parent may not be loaded, and a `Message`-typed field would either
    /// force loading the parent or carry an `Option<Box<Message>>` and a
    /// cycle risk. The id resolves against the channel's message map in
    /// `state/`.
    pub thread_id: Option<String>,
    /// Files attached to this message.
    ///
    /// `SmallVec<[Attachment; 1]>`: most messages have none, some have one, and
    /// the buffer holds the first inline.
    pub attachments: SmallVec<[Attachment; 1]>,
}

/// One emoji's reactions, grouped by emoji.
///
/// The grouping is the renderer's requirement: a reaction chip shows an emoji
/// and a count, so the count has to be somewhere. Keeping the users per emoji
/// means the chip's tooltip list -- "Ada, Grace" -- needs no second lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    /// The emoji, as the exact string the server reported.
    ///
    /// Stored verbatim rather than normalised, because a reaction key has to be
    /// byte-identical across clients for two clients to agree that they are
    /// showing the same reaction. Normalising is a protocol decision -- it
    /// changes which keys exist -- and it belongs in a version bump, not in a
    /// string method.
    pub emoji: String,
    /// Who reacted, in no guaranteed order.
    pub user_ids: Vec<String>,
}

/// A file attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Server-assigned identifier, used to request a fresh download URL.
    pub id: String,
    /// Name as uploaded, for display. User-supplied text, and therefore text the
    /// UI must escape rather than render.
    pub filename: String,
    /// Where to fetch the file.
    ///
    /// Pre-signed and short-lived, so it must not be cached past the message's
    /// lifetime. Certificate validation is never disabled for this fetch
    /// (`PLAN.md` §3).
    pub url: String,
    /// IANA media type, e.g. `image/png`.
    ///
    /// **Advisory.** The client picks a preview renderer from it and falls back
    /// to a generic chip for anything it does not recognise, so an unrecognised
    /// type is not an error. A preview that trusts the declared type is a
    /// preview that renders an HTML file as an image.
    pub mime_type: String,
    /// Size in bytes, validated at the boundary against
    /// `sh_nexus::network::mapping::MAX_ATTACHMENT_BYTES`.
    pub size: u64,
}
