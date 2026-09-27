//! Shared fixtures for the `sh_nexus_wire` test suites.
//!
//! # Why this file is `tests/support/mod.rs` and not `tests/integration/…`
//!
//! `PLAN.md` §4 records a defect from `PLAN.md` Rev 2: files at
//! `tests/integration/*.rs` are **silently never compiled or run** by Cargo,
//! because Cargo auto-discovers `tests/*.rs` and `tests/*/main.rs` and nothing
//! else. The result was a green build with all ten mandatory integration flows
//! from `AGENTS.md` §8.1 absent.
//!
//! This module is the *other* shape, and the difference matters. It lives in a
//! subdirectory with no `main.rs`, so Cargo does **not** discover it as a test
//! binary -- but every test file that needs it declares `mod support;`, and
//! `mod` declarations are compiled. So it is a shared module that is guaranteed
//! to be compiled, and it is deliberately *not* a test target. A fixture that
//! were itself a test target would run its (empty) body and look like coverage.
//!
//! # The fixtures
//!
//! One canonical instance of every wire type, with **fixed** identifiers and a
//! **fixed** timestamp. Two reasons, both of them about test honesty:
//!
//! - A fixed timestamp means the expected JSON in the shape assertions is a
//!   literal string rather than something computed with the current clock, so a
//!   serialization change shows up as a diff instead of as a moving target.
//! - A fixed `client_msg_id` means a missing field in the encoded JSON is
//!   visible as a missing literal, not as a field that happens to hold some
//!   other value.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;
use sh_nexus_wire::dto::{
    WireAttachment, WireChannel, WireMessage, WireReaction, WireUser, WireUserStatus,
};
use sh_nexus_wire::frame::{ClientFrame, ServerFrame};

/// The canonical `client_msg_id` used across the suites.
pub const SAMPLE_CLIENT_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";

/// The canonical timestamp, in the exact form `PLAN.md` §6 shows for `resync`.
///
/// `PLAN.md` §6 writes `"after": "2026-09-27T12:00:00Z"`, so the fixture is that
/// literal and the shape assertions compare against chrono's own rendering of
/// it. If chrono's canonical output ever changed, the shape test would fail --
/// which is correct: the protocol's byte-level format is a contract with the
/// server, not an implementation detail of this crate.
pub const SAMPLE_TIMESTAMP_RFC3339: &str = "2026-09-27T12:00:00Z";

/// The canonical timestamp as a `DateTime<Utc>`.
///
/// Built by parsing rather than by `DateTime::from_timestamp` so that the
/// fixture and the protocol's documented spelling cannot drift apart: the
/// constant above is the single source and this reads it.
pub fn stamp() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(SAMPLE_TIMESTAMP_RFC3339)
        .expect("SAMPLE_TIMESTAMP_RFC3339 is valid RFC-3339")
        .with_timezone(&Utc)
}

/// A message with every optional field populated.
///
/// Populating `edited_at` and `thread_id` matters: the default fixtures for the
/// common case leave them `null`, and a struct whose optional fields are only
/// ever tested as `null` is a struct whose optional-field handling is untested.
pub fn sample_message() -> WireMessage {
    WireMessage {
        id: "m_1".to_owned(),
        client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(),
        channel_id: "c_1".to_owned(),
        user_id: "u_1".to_owned(),
        content: "hello team".to_owned(),
        timestamp: stamp(),
        edited_at: Some(stamp()),
        reactions: vec![WireReaction {
            emoji: "\u{1f44d}".to_owned(),
            user_ids: vec!["u_1".to_owned(), "u_2".to_owned()],
        }],
        thread_id: Some("m_0".to_owned()),
        attachments: vec![WireAttachment {
            id: "a_1".to_owned(),
            filename: "shot.png".to_owned(),
            url: "https://example.invalid/shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size: 1024,
        }],
    }
}

/// A minimal message: no edits, no reactions, no thread, no attachments.
///
/// The counterpart to [`sample_message`]. Every DTO is tested in both states,
/// because "the field was absent" and "the field was `null`" take different
/// paths through serde and it is exactly the absent case that a struct full of
/// fixtures never reaches.
pub fn minimal_message() -> WireMessage {
    WireMessage {
        id: "m_2".to_owned(),
        client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(),
        channel_id: "c_1".to_owned(),
        user_id: "u_1".to_owned(),
        content: "hi".to_owned(),
        timestamp: stamp(),
        edited_at: None,
        reactions: Vec::new(),
        thread_id: None,
        attachments: Vec::new(),
    }
}

/// A user with no avatar set.
pub fn sample_user() -> WireUser {
    WireUser {
        id: "u_1".to_owned(),
        username: "ada".to_owned(),
        display_name: "Ada Lovelace".to_owned(),
        avatar_url: None,
        status: WireUserStatus::Online,
    }
}

/// A public channel with a member list and a resume cursor.
pub fn sample_channel() -> WireChannel {
    WireChannel {
        id: "c_1".to_owned(),
        name: "general".to_owned(),
        description: Some("everything and nothing".to_owned()),
        is_private: false,
        members: vec!["u_1".to_owned(), "u_2".to_owned()],
        last_message_at: Some(stamp()),
    }
}

/// One instance of every [`ClientFrame`] variant, in `PLAN.md` §6's order.
///
/// Paired with the exhaustive `match` in
/// `tests/client_frame_invariants.rs::client_frame_kind_is_total`, so adding a
/// variant without adding it here is a **compile error**, not a silently
/// untested frame.
pub fn every_client_frame() -> Vec<ClientFrame> {
    vec![
        ClientFrame::MessageSend {
            channel_id: "c_1".to_owned(),
            content: "hello team".to_owned(),
        },
        ClientFrame::ReactionAdd {
            message_id: "m_1".to_owned(),
            emoji: "\u{1f44d}".to_owned(),
        },
        ClientFrame::TypingStart {
            channel_id: "c_1".to_owned(),
        },
        ClientFrame::TypingStop {
            channel_id: "c_1".to_owned(),
        },
        ClientFrame::Resync {
            channel_id: "c_1".to_owned(),
            after: stamp(),
        },
    ]
}

/// One instance of every [`ServerFrame`] variant, in `PLAN.md` §6's order.
///
/// Paired with the exhaustive `match` in `tests/wire_frames.rs`, so a new
/// variant without a fixture here is a compile error.
pub fn every_server_frame() -> Vec<ServerFrame> {
    vec![
        ServerFrame::MessageAck {
            client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(),
            message: sample_message(),
        },
        ServerFrame::MessageNew {
            message: sample_message(),
        },
        ServerFrame::MessageError {
            client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(),
            code: "message_too_long".to_owned(),
            detail: "exceeds 4000 characters".to_owned(),
        },
        ServerFrame::ReactionUpdate {
            message_id: "m_1".to_owned(),
            emoji: "\u{1f44d}".to_owned(),
            user_id: "u_1".to_owned(),
        },
        ServerFrame::TypingUpdate {
            user_id: "u_1".to_owned(),
            channel_id: "c_1".to_owned(),
            active: true,
        },
        ServerFrame::PresenceUpdate {
            user_id: "u_1".to_owned(),
            status: WireUserStatus::Away,
        },
        ServerFrame::Error {
            code: "auth_expired".to_owned(),
            detail: "the session token has expired".to_owned(),
        },
    ]
}

/// Serializes a value to compact JSON for a shape assertion.
///
/// Returns `String` rather than `Result` because it is only ever called on
/// values this crate's own types can hold, where `serde_json` cannot fail.
/// `expect` is permitted in tests by `AGENTS.md` §2.1, and naming the assertion
/// helper makes a failure point at the caller.
pub fn json_of<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("sh_nexus_wire types always serialize to JSON")
}
