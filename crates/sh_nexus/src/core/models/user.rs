//! A user account, in the domain.

/// A person who can send and receive messages.
///
/// A plain data type with public fields, as `PLAN.md` §5 specifies. There are
/// no setters and no validation here, and that is a considered position rather
/// than an omission: the invariant that a `User`'s `id` is non-blank is
/// enforced at the boundary that untrusted data crosses
/// (`sh_nexus::network::mapping`), and the type that enters the domain from
/// anywhere else is produced by `state/actions.rs`, which §3.2 makes the sole
/// owner of mutations. A constructor here would be a second place to enforce
/// the same rule, and two places to enforce one rule is zero places the moment
/// somebody adds a struct literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Server-assigned identifier. Unique, and never reused.
    pub id: String,
    /// Login handle. Lowercased and unique per workspace by the server.
    pub username: String,
    /// Human-facing name. **Not** unique -- two people may share a display name,
    /// so a message must always be attributed by `id` and rendered with
    /// `display_name`, never the other way round.
    pub display_name: String,
    /// Where to fetch the avatar, or `None` for "no avatar set".
    ///
    /// `None` is the common case and is not an error, so it is not defaulted to
    /// a placeholder URL: a fabricated avatar URL would be a fabricated network
    /// request on every render of the sidebar.
    pub avatar_url: Option<String>,
    /// The user's current presence.
    pub status: UserStatus,
}

/// How present a user is.
///
/// Three states, matching the three the server reports. There is deliberately
/// no "recently active" or "busy": every added state is a state every client
/// must be able to render, and `sh_nexus_wire::dto::WireUserStatus` is a
/// **closed** enum precisely so that adding one is a visible protocol change
/// rather than a value that appears in the UI as a gap. See the compatibility
/// policy in that crate's root docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserStatus {
    /// Connected and recently active.
    Online,
    /// Connected but idle past the server's away threshold.
    Away,
    /// Not connected. The presence the user has when the app is closed, so it is
    /// the state a stale entry settles into rather than an error.
    Offline,
}
