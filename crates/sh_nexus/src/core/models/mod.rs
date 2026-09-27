//! The domain models: what the application *is*, as opposed to what crosses a
//! socket.
//!
//! # The boundary this module sits on
//!
//! `PLAN.md` §5 splits the data model in two, and this module is one half of the
//! split:
//!
//! | | `sh_nexus_wire::dto` | `core::models` (here) |
//! |---|---|---|
//! | Question answered | "what do these bytes mean?" | "what is true in the app?" |
//! | Knows about | serde, JSON, frame types, protocol versions | nothing but the domain |
//! | Trust level | untrusted input | trusted, validated data |
//! | `reactions` on a message | `Vec<Reaction>` | `SmallVec<[Reaction; 2]>` |
//! | `client_msg_id` | `String` | `Uuid` |
//!
//! The conversion between them is `sh_nexus::network::mapping`, and it is the
//! only place validation happens (`AGENTS.md` §2.1). The absence of `serde`
//! derives below is not an oversight or a simplification -- **it is the
//! boundary**. A type that can be both the domain model and the wire format
//! cannot be changed in one without changing the other, which is how a protocol
//! and an application model drift apart while the tests stay green.
//!
//! `crates/sh_nexus/tests/layer_boundary.rs` asserts the absence mechanically,
//! by scanning this directory's source. Documentation of a boundary that
//! nothing enforces is a comment; a boundary that a test watches is a boundary.
//!
//! # What is deliberately *not* here
//!
//! Two things a reader will look for in this module and find missing:
//!
//! - **`DeliveryState`** -- pending, acked, failed. Client state, not domain
//!   state. `AGENTS.md` §3.2 puts all mutations in `state/actions.rs` and
//!   `PLAN.md` §5 says so explicitly. It belongs in `state/app_state.rs` with
//!   the unread counts, and it lands there in a later Phase 1 work unit. It is
//!   not in `message.rs` because a `Message` in this crate means "a message that
//!   exists"; whether *this client* has finished sending it is a fact about the
//!   connection, and two clients holding the same message will disagree about it.
//! - **`unread_count`** -- likewise client state, and likewise a per-channel
//!   fact that a message does not own. It lives beside `DeliveryState` in
//!   `state/app_state.rs`.
//!
//! # What is deliberately here
//!
//! Nothing here performs I/O, reads a clock, or spawns a thread. `AGENTS.md`
//! §3.2 forbids `core/` from importing `gpui` or `tokio` and from touching the
//! filesystem, and that purity is what makes the module's ≥90% coverage floor
//! (§4.1) reachable rather than aspirational.

pub mod channel;
pub mod events;
pub mod message;
pub mod user;

pub use crate::core::models::channel::Channel;
pub use crate::core::models::events::{ConnectionState, DomainEvent};
pub use crate::core::models::message::{Attachment, Message, Reaction};
pub use crate::core::models::user::{User, UserStatus};
