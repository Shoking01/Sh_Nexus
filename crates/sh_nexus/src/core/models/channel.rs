//! A channel, in the domain.

use std::sync::Arc;

use chrono::{DateTime, Utc};

/// A conversation space: public, private, or direct.
///
/// The fields are `PLAN.md` §5's, unchanged. Two carry a rule.
///
/// # What is not on this type
///
/// `unread_count` is **client state** and lives in `state/app_state.rs`, not
/// here. `PLAN.md` §5 says so in as many words, and §3.2 is why: the unread
/// count is a fact about *this user's* reading of *this* channel, so it differs
/// per user and per device, while the channel itself does not. Two clients
/// showing the same channel will have different unread counts, and both are
/// correct. A field on `Channel` could not represent that without embedding a
/// client identity in a domain type, and the day somebody reads `channel.
/// unread_count` without thinking about which client they are, they have a bug.
///
/// The same argument applies to `last_message_at` *only partially*, and the
/// difference is worth being precise about: `last_message_at` is a property of
/// the channel and is the same for everyone, so it belongs here. It is also the
/// per-channel resume cursor for `AGENTS.md` §7.4's reconnection, which is why
/// the wire DTO documents it as such.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// Server-assigned identifier. Unique and never reused.
    pub id: String,
    /// Display name, unique per workspace. This is what the sidebar renders.
    pub name: String,
    /// Optional topic or purpose. `None` is normal and is rendered as no
    /// subtitle -- not as an empty string, which would render a blank line.
    pub description: Option<String>,
    /// Whether membership is restricted to an explicit list.
    ///
    /// Drives the UI's lock affordance and nothing else. The server enforces it;
    /// a client that used this field to make an authorization decision would be
    /// a client that authorizes on a value it received over the wire from the
    /// party it was supposed to be checking against.
    pub is_private: bool,
    /// The members' user ids.
    ///
    /// `Arc<[String]>` rather than `Vec<String>` per `PLAN.md` §5 and
    /// `AGENTS.md` §2.3: a channel is cloned into every place that needs it --
    /// the sidebar, the header, the member list, the reaction tooltips -- and a
    /// `Vec` makes each of those clones a deep copy of the member list. With an
    /// `Arc`, cloning a `Channel` is a pointer copy and allocates nothing.
    ///
    /// The sidebar iterates a *cached* `Vec<Arc<Channel>>` in `AppState`; it
    /// does not re-read or re-clone the member list per frame. That is the whole
    /// reason for the type, and it is why changing it back to `Vec` would be a
    /// performance regression in the scroll path rather than a style regression.
    ///
    /// Unsized, so it cannot be mutated in place through the `Arc` -- a
    /// membership change replaces the channel rather than editing it, which
    /// means a rendered row can never observe a half-updated member list.
    pub members: Arc<[String]>,
    /// When the most recent message arrived, or `None` for a channel that has
    /// never had one.
    ///
    /// The same value for every client, unlike the unread count -- so it is a
    /// property of the channel, and it is here.
    pub last_message_at: Option<DateTime<Utc>>,
}
