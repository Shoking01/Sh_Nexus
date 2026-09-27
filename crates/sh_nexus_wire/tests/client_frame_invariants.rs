//! The `client_msg_id` invariant: **every** client frame carries one.
//!
//! `AGENTS.md` §7.4 requires it --
//!
//! > Every WS message must have a client-generated `client_msg_id` (UUID) for
//! > idempotent dedup across reconnects.
//!
//! This is not a hypothetical requirement. `PLAN.md` Rev 2 asserted exactly
//! this rule and then omitted the field from **three of its own five** client
//! frames; ADR-004 records it as one of two defects that produce a green build
//! over a void, because the plan that documents the rule and breaks it teaches
//! the next reader to break it too.
//!
//! # The two halves of the guarantee
//!
//! | Half | Enforced by | Tested by |
//! |---|---|---|
//! | **Presence** -- a client frame cannot be built or decoded without a `client_msg_id` | [`ClientEnvelope`] holds the field, outside the payload enum | the tests below, plus a compile-time guard |
//! | **Validity** -- the id parses as a `Uuid` | the client's domain boundary, `sh_nexus::network::mapping::parse_client_msg_id` | `crates/sh_nexus/tests/wire_boundary.rs` |
//!
//! The split is deliberate and is the reason this crate carries the id as a
//! `String`. This crate never trusts its input, so it enforces structure; the
//! domain, which is trusted, enforces meaning. Putting `Uuid` on the DTO would
//! collapse both into one serde error and leave the domain nothing to assert.
//!
//! # Why the compile-time guard
//!
//! A list of the five frames is a convention. `exhaustive_client_kind` is a
//! `match` with no wildcard arm, so adding a sixth `ClientFrame` variant -- a
//! `reaction.remove`, say -- is a **compile error** until it is accounted for
//! here. That is the difference between "every frame is tested" and "every
//! frame I remembered to test is tested".

mod support;

use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, FrameKind};
use sh_nexus_wire::WireError;
use support::{every_client_frame, SAMPLE_CLIENT_MSG_ID};

/// Every client frame's JSON carries a `client_msg_id` at the top level.
///
/// The assertion is on the *decoded JSON object*, not on a substring, so it
/// cannot be satisfied by a `client_msg_id` nested somewhere inside the payload.
#[test]
fn every_client_frame_carries_a_client_msg_id() {
    for frame in every_client_frame() {
        let envelope = ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame);
        let json = envelope.encode().expect("a client frame encodes");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("the encoded frame is valid JSON");

        assert_eq!(
            parsed
                .get("client_msg_id")
                .and_then(serde_json::Value::as_str),
            Some(SAMPLE_CLIENT_MSG_ID),
            "{json} does not carry the client_msg_id at the top level"
        );
    }
}

/// The id survives a round trip unchanged, for every frame.
#[test]
fn the_client_msg_id_survives_a_round_trip_for_every_frame() {
    for frame in every_client_frame() {
        let envelope = ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame);
        let json = envelope.encode().expect("a client frame encodes");
        let decoded = ClientEnvelope::decode(&json).expect("the encoded frame decodes");
        assert_eq!(decoded.client_msg_id, SAMPLE_CLIENT_MSG_ID);
    }
}

/// The id is *required*: a frame without one is refused, for every frame type.
///
/// This is the runtime half of a type-level guarantee. The compiler already
/// makes it impossible to *construct* a `ClientEnvelope` without an id; this
/// makes it impossible to *receive* one without an id, which the compiler cannot
// see because the JSON comes off a socket.
#[test]
fn a_client_frame_without_a_client_msg_id_is_refused_for_every_frame_type() {
    for frame in every_client_frame() {
        // Captured before the move, so the failure message can name the frame.
        let kind = frame.kind();
        let mut value = serde_json::to_value(ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame))
            .expect("the envelope serializes to a JSON value");
        value
            .as_object_mut()
            .expect("an envelope is a JSON object")
            .remove("client_msg_id");

        let json = value.to_string();
        let error = ClientEnvelope::decode(&json)
            .err()
            .unwrap_or_else(|| panic!("{kind} decoded without a client_msg_id"));
        assert!(
            matches!(error, WireError::MalformedPayload(_)),
            "{kind} should be refused as malformed, got {error:?}"
        );
    }
}

/// The frame-type registry has no client frame without a corresponding
/// `FrameKind`, and the fixture list covers the registry exactly.
///
/// The list length check is redundant with the exhaustive `match` below -- and is
/// kept anyway, because a mismatch is the one failure that would make the
/// per-frame assertions above vacuous. If the fixture list drifted short of the
/// enum, the tests above would still pass while testing three of five frames.
#[test]
fn the_fixture_list_covers_every_client_frame_type() {
    assert_eq!(every_client_frame().len(), FrameKind::CLIENT.len());

    for kind in FrameKind::CLIENT {
        assert!(
            every_client_frame()
                .iter()
                .any(|frame| frame.kind() == kind),
            "{kind} has no fixture, so its client_msg_id invariant is untested"
        );
    }
}

/// **Compile-time guard.** Adding a `ClientFrame` variant breaks this build.
///
/// The `match` has no wildcard arm, so the compiler rejects any new variant
/// until it is handled. That is what makes "every client frame carries a
/// `client_msg_id`" a property of the build rather than of whoever remembered
/// to update a list.
#[test]
fn client_frame_kind_is_total() {
    for frame in every_client_frame() {
        let kind = exhaustive_client_kind(&frame);
        assert_eq!(
            kind,
            frame.kind(),
            "the fixture's kind should match its variant"
        );
    }
}

/// Exhaustive over `ClientFrame`. See [`client_frame_kind_is_total`].
fn exhaustive_client_kind(frame: &ClientFrame) -> FrameKind {
    match frame {
        ClientFrame::MessageSend { .. } => FrameKind::MessageSend,
        ClientFrame::ReactionAdd { .. } => FrameKind::ReactionAdd,
        ClientFrame::TypingStart { .. } => FrameKind::TypingStart,
        ClientFrame::TypingStop { .. } => FrameKind::TypingStop,
        ClientFrame::Resync { .. } => FrameKind::Resync,
    }
}

/// An explicit envelope stamps the protocol version and the id together, so a
/// caller cannot forget either.
#[test]
fn the_envelope_constructor_stamps_version_and_id_together() {
    let envelope = ClientEnvelope::new(
        "an-id",
        ClientFrame::TypingStart {
            channel_id: "c".into(),
        },
    );
    assert_eq!(envelope.v, sh_nexus_wire::PROTOCOL_VERSION);
    assert_eq!(envelope.client_msg_id, "an-id");
}
