//! # `sh_nexus_wire` -- the Sh_Nexus wire protocol
//!
//! One crate, two compilers, one protocol. The client (`sh_nexus`) and the
//! server (`sh_nexus_server`, ADR-002) both link this, so a protocol change
//! that breaks one side does not compile on the other. That is not a
//! convention anyone has to remember; it is the build.
//!
//! ## The dependency direction, stated once
//!
//! ```text
//!     sh_nexus  ─────┐
//!                    ├──> sh_nexus_wire
//!     sh_nexus_server ┘
//! ```
//!
//! This crate depends on nothing project-specific. Not on `sh_nexus`, not on
//! `sh_nexus_server`, not on `gpui`, not on `tokio`, not on `reqwest`, not on
//! `rusqlite`. A back-edge to any of them would make the protocol depend on one
//! of its two implementations, and "defined once" -- the property ADR-002 exists
//! to guarantee -- would quietly become "defined once, except where it matters".
//! `tests/dependency_direction.rs` asserts the rule against `Cargo.toml`, so a
//! back-edge is a failing test rather than a review opinion.
//!
//! For the same reason every function here is a pure transformation over bytes
//! or values: no I/O, no clock, no thread, no GPU. That is what makes the ≥80%
//! coverage floor (ADR-004) reachable, and what makes it safe to compile into
//! both ends of a socket.
//!
//! ## What lives here, and what deliberately does not
//!
//! | Module | Contents |
//! |---|---|
//! | [`version`] | `v`, the supported-version set, and [`version::negotiate`] -- the one gate between bytes and meaning. |
//! | [`frame`] | The twelve frame types, the two envelopes, and the codec. |
//! | [`dto`] | The wire DTOs: what a message, channel, user, reaction or attachment looks like on the wire. |
//! | [`error`] | [`WireError`], and which failures a caller must act on versus log. |
//!
//! What does **not** live here is as deliberate. No domain type: `Message` in
//! this crate means "the JSON shape", and `core::models::Message` means "the
//! thing the UI renders", and they are different types on purpose (`PLAN.md`
//! §5). No validation rules about blank ids or timestamp ranges: those belong
//! to the boundary that turns this crate's untrusted text into trusted data,
//! `sh_nexus::network::mapping`. No connection state, no reconnection, no
//! transport. This crate describes a protocol; it does not speak it.
//!
//! ## The compatibility policy
//!
//! `PLAN.md` §6 defines the version field as a bare integer (`"v": 1`), not a
//! dotted `major.minor`. So there is no minor number to negotiate, and the rules
//! for what may change without a major bump have to be written down rather than
//! encoded in a type. They are:
//!
//! - **New optional fields are always allowed.** No wire type uses
//!   `deny_unknown_fields`, so a peer ignores what it does not know. This is the
//!   only reason additive change is safe at all, and it is why the types say
//!   nothing about extra fields: a `deny_unknown_fields` added to one struct
//!   would turn a routine rollout into a hard failure for every older client.
//! - **New frame types are allowed only when losing one costs nothing.** A
//!   dropped `message.*` is a lost message, so a new one requires a major bump.
//!   A dropped `typing.*`, `presence.*` or `error` frame costs nothing, so a
//!   peer may meet an unknown one and drop it with a `warn!` while keeping the
//!   connection open -- see [`WireError::UnknownFrameType`], and
//!   [`WireError::is_fatal`] for the one case that is *not* a drop.
//! - **New variants of a closed enum require a major bump.** `WireUserStatus` is
//!   rendered in the UI, and a client has no way to render a value it has no
//!   case for. Failing loudly beats inventing a placeholder the user cannot
//!   distinguish from the truth.
//! - **New `error` codes are allowed.** `code` is a `String`, not an enum,
//!   precisely so a client too old to know a code can still read the frame that
//!   explains the failure. An enum would make the server's explanation
//!   undeliverable to the client that most needs it.
//!
//! ## Reading order for a new contributor
//!
//! 1. `PLAN.md` §6 -- the specification, including what each revision got wrong.
//! 2. [`frame`]'s module docs -- the envelope, the decode order, and why the
//!    client envelope is not symmetrical with the server's.
//! 3. [`version`]'s module docs -- what "reject explicitly" has to mean to be
//!    worth anything.
//!
//! # Examples
//!
//! ```
//! use sh_nexus_wire::{ClientEnvelope, ClientFrame, ServerEnvelope, PROTOCOL_VERSION};
//!
//! // Send.
//! let outgoing = ClientEnvelope::new(
//!     "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d",
//!     ClientFrame::MessageSend { channel_id: "general".into(), content: "hi".into() },
//! );
//! let json = outgoing.encode().unwrap_or_default();
//!
//! // Receive, on the other side of the same crate. A client frame read as a
//! // server frame is a role violation, and is rejected before its body is read.
//! let misread = ServerEnvelope::decode(&json)
//!     .expect_err("a client frame read as a server frame is a role violation");
//! assert!(matches!(misread, sh_nexus_wire::WireError::WrongDirection { .. }));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dto;
pub mod error;
pub mod frame;
pub mod version;

pub use crate::dto::{
    WireAttachment, WireChannel, WireMessage, WireReaction, WireUser, WireUserStatus,
};
pub use crate::error::WireError;
pub use crate::frame::{
    ClientEnvelope, ClientFrame, Direction, FrameKind, ServerEnvelope, ServerFrame,
};
pub use crate::version::{
    negotiate, ProtocolVersion, UnsupportedVersion, PROTOCOL_VERSION, SUPPORTED_MAJOR_VERSIONS,
    UNSUPPORTED_VERSION_CODE,
};
