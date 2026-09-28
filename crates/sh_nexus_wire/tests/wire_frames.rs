//! Round-trips, exact JSON shapes, and malformed-payload rejection for every
//! wire format.
//!
//! `AGENTS.md` §4.2 requires "Serde round-trips for every wire format;
//! malformed payload rejection" for `models/`, and the requirement is discharged
//! here rather than in the client's domain models, because the wire formats
//! live in this crate. ADR-004 §3 sets the crate's floor at ≥80% and its
//! decision 4 explains why: §4.2's mandate is *about* this crate.
//!
//! # Why exact JSON strings, not just round-trips
//!
//! A round-trip test proves `decode(encode(x)) == x`. It passes just as happily
//! if the encoding is `{"t":"message_send","v":1}` as it does if it is
//! `{"v":1,"type":"message.send"}` -- and the first one is a protocol no other
//! implementation of `PLAN.md` §6 speaks. A DTO whose JSON does not match the
//! documented protocol is worse than no DTO, because it looks like a working
//! protocol: the client and server would share the *same* wrong protocol and
//! every test would still be green.
//!
//! That is not hypothetical. An earlier draft of this crate used
//! `#[serde(rename_all = "snake_case")]` and emitted `"type":"typing_update"`.
//! Every round-trip passed. The shape assertions below, and the doctest on
//! `ClientFrame`, are what caught it.
//!
//! So each frame's expected bytes are written out literally here, copied from
//! `PLAN.md` §6. When the encoding and the plan disagree, one of the two tests
//! here fails.
//!
//! # Test placement
//!
//! This file is `tests/wire_frames.rs`, flat, so Cargo discovers it. See
//! `tests/support/mod.rs` for the `tests/integration/` trap this avoids.

mod support;

use std::collections::HashSet;

use rstest::rstest;
use sh_nexus_wire::dto::{WireAttachment, WireMessage, WireReaction, WireUser, WireUserStatus};
use sh_nexus_wire::frame::{
    ClientEnvelope, ClientFrame, Direction, FrameKind, ServerEnvelope, ServerFrame,
};
use sh_nexus_wire::{WireError, PROTOCOL_VERSION};
use support::{
    every_client_frame, every_server_frame, json_of, minimal_message, sample_channel,
    sample_message, sample_user, stamp, SAMPLE_CLIENT_MSG_ID, SAMPLE_TIMESTAMP_RFC3339,
};

// ---------------------------------------------------------------------------
// 1. The encoded shape matches PLAN.md section 6, frame by frame.
//
// Note on the emoji in the expected JSON below: it is written as the literal
// multi-byte character, and that is load-bearing twice over.
//
// `PLAN.md` section 6 writes `reaction.add`'s emoji as a real character, and
// `serde_json` does not escape non-ASCII on output, so a fixture using ASCII
// would not have caught a change that started emitting `\uXXXX` -- which the
// server would read as a different reaction key.
//
// And it is written literally rather than as `\u{1f44d}` because a Rust **raw**
// string does not process escapes: `r#""emoji":"\u{1f44d}""#` contains the seven
// characters `\u{1f44d}`, not the emoji. That mistake produces an expected value
// which is wrong in a way that reads exactly like a serialization bug, so it is
// called out here rather than left to be rediscovered.
// ---------------------------------------------------------------------------

/// Every client frame encodes to exactly the JSON `PLAN.md` §6 documents.
///
/// One case per frame rather than a loop over a `match`, so a failure names the
/// frame that broke instead of printing an index into a list.
#[test]
fn client_frames_encode_to_the_documented_json() {
    let cases: [(&str, ClientFrame, &str); 5] = [
        (
            "message.send",
            ClientFrame::MessageSend {
                channel_id: "c_1".to_owned(),
                content: "hello team".to_owned(),
            },
            r#"{"v":1,"client_msg_id":"cid-1","type":"message.send","channel_id":"c_1","content":"hello team"}"#,
        ),
        (
            "reaction.add",
            ClientFrame::ReactionAdd {
                message_id: "m_1".to_owned(),
                emoji: "\u{1f44d}".to_owned(),
            },
            r#"{"v":1,"client_msg_id":"cid-1","type":"reaction.add","message_id":"m_1","emoji":"👍"}"#,
        ),
        (
            "typing.start",
            ClientFrame::TypingStart {
                channel_id: "c_1".to_owned(),
            },
            r#"{"v":1,"client_msg_id":"cid-1","type":"typing.start","channel_id":"c_1"}"#,
        ),
        (
            "typing.stop",
            ClientFrame::TypingStop {
                channel_id: "c_1".to_owned(),
            },
            r#"{"v":1,"client_msg_id":"cid-1","type":"typing.stop","channel_id":"c_1"}"#,
        ),
        (
            "resync",
            ClientFrame::Resync {
                channel_id: "c_1".to_owned(),
                after: stamp(),
            },
            r#"{"v":1,"client_msg_id":"cid-1","type":"resync","channel_id":"c_1","after":"2026-09-27T12:00:00Z"}"#,
        ),
    ];

    for (name, frame, expected) in cases {
        let envelope = ClientEnvelope::new("cid-1", frame);
        assert_eq!(
            envelope.encode().expect("a client frame encodes"),
            expected,
            "`{name}` does not match the JSON in PLAN.md section 6"
        );
    }
}

/// Every server frame encodes to exactly the JSON `PLAN.md` §6 documents.
#[test]
fn server_frames_encode_to_the_documented_json() {
    let cases: [(&str, ServerFrame, &str); 7] = [
        (
            "message.ack",
            ServerFrame::MessageAck {
                client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(),
                message: sample_message(),
            },
            concat!(
                r#"{"v":1,"type":"message.ack","client_msg_id":"5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d","message":"#,
                r#"{"id":"m_1","client_msg_id":"5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d","#,
                r#""channel_id":"c_1","user_id":"u_1","content":"hello team","#,
                r#""timestamp":"2026-09-27T12:00:00Z","edited_at":"2026-09-27T12:00:00Z","#,
                r#""reactions":[{"emoji":"👍","user_ids":["u_1","u_2"]}],"#,
                r#""thread_id":"m_0","attachments":[{"id":"a_1","filename":"shot.png","#,
                r#""url":"https://example.invalid/shot.png","mime_type":"image/png","size":1024}]}}"#,
            ),
        ),
        (
            "message.new",
            ServerFrame::MessageNew {
                message: sample_message(),
            },
            concat!(
                r#"{"v":1,"type":"message.new","message":"#,
                r#"{"id":"m_1","client_msg_id":"5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d","#,
                r#""channel_id":"c_1","user_id":"u_1","content":"hello team","#,
                r#""timestamp":"2026-09-27T12:00:00Z","edited_at":"2026-09-27T12:00:00Z","#,
                r#""reactions":[{"emoji":"👍","user_ids":["u_1","u_2"]}],"#,
                r#""thread_id":"m_0","attachments":[{"id":"a_1","filename":"shot.png","#,
                r#""url":"https://example.invalid/shot.png","mime_type":"image/png","size":1024}]}}"#,
            ),
        ),
        (
            "message.error",
            ServerFrame::MessageError {
                client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(),
                code: "message_too_long".to_owned(),
                detail: "exceeds 4000 characters".to_owned(),
            },
            r#"{"v":1,"type":"message.error","client_msg_id":"5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d","code":"message_too_long","detail":"exceeds 4000 characters"}"#,
        ),
        (
            "reaction.update",
            ServerFrame::ReactionUpdate {
                message_id: "m_1".to_owned(),
                emoji: "\u{1f44d}".to_owned(),
                user_id: "u_1".to_owned(),
            },
            r#"{"v":1,"type":"reaction.update","message_id":"m_1","emoji":"👍","user_id":"u_1"}"#,
        ),
        (
            "typing.update",
            ServerFrame::TypingUpdate {
                user_id: "u_1".to_owned(),
                channel_id: "c_1".to_owned(),
                active: true,
            },
            r#"{"v":1,"type":"typing.update","user_id":"u_1","channel_id":"c_1","active":true}"#,
        ),
        (
            "presence.update",
            ServerFrame::PresenceUpdate {
                user_id: "u_1".to_owned(),
                status: WireUserStatus::Away,
            },
            r#"{"v":1,"type":"presence.update","user_id":"u_1","status":"away"}"#,
        ),
        (
            "error",
            ServerFrame::Error {
                code: "auth_expired".to_owned(),
                detail: "the session token has expired".to_owned(),
            },
            r#"{"v":1,"type":"error","code":"auth_expired","detail":"the session token has expired"}"#,
        ),
    ];

    for (name, frame, expected) in cases {
        let envelope = ServerEnvelope::new(frame);
        assert_eq!(
            envelope.encode().expect("a server frame encodes"),
            expected,
            "`{name}` does not match the JSON in PLAN.md section 6"
        );
    }
}

/// Each DTO encodes to the field names and value spellings `PLAN.md` §5 names.
#[test]
fn dtos_encode_to_the_documented_field_names() {
    assert_eq!(
        json_of(&sample_user()),
        r#"{"id":"u_1","username":"ada","display_name":"Ada Lovelace","avatar_url":null,"status":"online"}"#
    );
    assert_eq!(
        json_of(&sample_channel()),
        concat!(
            r#"{"id":"c_1","name":"general","description":"everything and nothing","#,
            r#""is_private":false,"members":["u_1","u_2"],"#,
            r#""last_message_at":"2026-09-27T12:00:00Z"}"#,
        )
    );
    assert_eq!(
        json_of(&sample_message()),
        concat!(
            r#"{"id":"m_1","client_msg_id":"5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d","#,
            r#""channel_id":"c_1","user_id":"u_1","content":"hello team","#,
            r#""timestamp":"2026-09-27T12:00:00Z","edited_at":"2026-09-27T12:00:00Z","#,
            r#""reactions":[{"emoji":"👍","user_ids":["u_1","u_2"]}],"#,
            r#""thread_id":"m_0","attachments":[{"id":"a_1","filename":"shot.png","#,
            r#""url":"https://example.invalid/shot.png","mime_type":"image/png","size":1024}]}"#,
        )
    );
    assert_eq!(
        json_of(&WireReaction {
            emoji: "\u{1f44d}".to_owned(),
            user_ids: vec!["u_1".to_owned()],
        }),
        r#"{"emoji":"👍","user_ids":["u_1"]}"#
    );
    assert_eq!(
        json_of(&WireAttachment {
            id: "a_1".to_owned(),
            filename: "shot.png".to_owned(),
            url: "https://example.invalid/shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size: 1024,
        }),
        r#"{"id":"a_1","filename":"shot.png","url":"https://example.invalid/shot.png","mime_type":"image/png","size":1024}"#
    );
}

/// Every `WireUserStatus` variant serializes to the lowercase spelling in
/// `PLAN.md` §6, which shows `"status": "online"`.
#[test]
fn user_status_variants_serialize_lowercase() {
    let cases = [
        (WireUserStatus::Online, "\"online\""),
        (WireUserStatus::Away, "\"away\""),
        (WireUserStatus::Offline, "\"offline\""),
    ];
    for (status, expected) in cases {
        assert_eq!(json_of(&status), expected);
    }
}

/// Timestamps cross the wire as RFC-3339, in the exact form `PLAN.md` §6 shows.
///
/// `PLAN.md` §3 forbids hand-formatting them, so this asserts chrono's own
/// output rather than a hand-rolled format string: the guarantee being tested
/// is that the wire carries a parseable timestamp, not that this crate
/// assembled one.
#[test]
fn timestamps_serialize_as_rfc3339_never_as_raw_numbers() {
    let message = minimal_message();
    let json = json_of(&message);
    assert!(
        json.contains(&format!(r#""timestamp":"{SAMPLE_TIMESTAMP_RFC3339}""#)),
        "timestamp should serialize as RFC-3339, got {json}"
    );
    assert!(
        !json.contains(r#""timestamp":1"#),
        "a timestamp must never serialize as a raw number, got {json}"
    );
}

// ---------------------------------------------------------------------------
// 2. Round trips, in both directions, for every wire format.
// ---------------------------------------------------------------------------

/// Every DTO survives encode -> decode unchanged.
///
/// Tested in both the fully-populated and the minimal state, because a struct
/// whose optional fields are only ever populated is a struct whose `null`
/// handling is untested, and the two take different paths through serde.
#[test]
fn every_dto_round_trips_through_json() {
    assert_round_trips(&sample_message());
    assert_round_trips(&minimal_message());
    assert_round_trips(&sample_user());
    assert_round_trips(&sample_channel());
    assert_round_trips(&WireReaction {
        emoji: "\u{1f44d}".to_owned(),
        user_ids: vec!["u_1".to_owned(), "u_2".to_owned()],
    });
    assert_round_trips(&WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: u64::from(u32::MAX),
    });
}

/// Encodes a DTO, decodes it, and asserts the value *and* the bytes survived.
///
/// Checks re-encoding separately from the value, because they are different
/// failures: a value can survive while the encoding is unstable (two spellings
/// of the same value), and that instability is invisible to a `decode == x`
/// assertion and very visible to a server diffing two of its own frames.
fn assert_round_trips<T>(original: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = json_of(original);
    let decoded: T = serde_json::from_str(&encoded)
        .unwrap_or_else(|error| panic!("{encoded} should decode: {error}"));
    assert_eq!(
        &decoded, original,
        "{encoded} did not survive the round trip"
    );
    assert_eq!(
        json_of(&decoded),
        encoded,
        "re-encoding {encoded} was not byte-stable"
    );
}

/// Every client frame survives `encode` then `decode` through the real codec,
/// version check and all.
#[test]
fn every_client_frame_round_trips_through_the_codec() {
    for frame in every_client_frame() {
        let envelope = ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame);
        let json = envelope.encode().expect("a client frame encodes");
        let decoded = ClientEnvelope::decode(&json)
            .unwrap_or_else(|error| panic!("{json} should decode: {error}"));
        assert_eq!(decoded, envelope, "{json} did not survive the round trip");
    }
}

/// Every server frame survives `encode` then `decode` through the real codec.
#[test]
fn every_server_frame_round_trips_through_the_codec() {
    for frame in every_server_frame() {
        let envelope = ServerEnvelope::new(frame);
        let json = envelope.encode().expect("a server frame encodes");
        let decoded = ServerEnvelope::decode(&json)
            .unwrap_or_else(|error| panic!("{json} should decode: {error}"));
        assert_eq!(decoded, envelope, "{json} did not survive the round trip");
    }
}

/// Decoding from raw bytes agrees with decoding from text.
///
/// WebSocket text frames arrive as bytes, so `decode_bytes` is the entry point
/// the transport will actually use. If it disagreed with `decode`, the tests
/// above would be testing a path the client never takes.
#[test]
fn decoding_from_bytes_agrees_with_decoding_from_text() {
    for frame in every_client_frame() {
        let envelope = ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame);
        let json = envelope.encode().expect("a client frame encodes");
        let from_bytes =
            ClientEnvelope::decode_bytes(json.as_bytes()).expect("valid UTF-8 bytes decode");
        assert_eq!(from_bytes, envelope);
    }
    for frame in every_server_frame() {
        let envelope = ServerEnvelope::new(frame);
        let json = envelope.encode().expect("a server frame encodes");
        let from_bytes =
            ServerEnvelope::decode_bytes(json.as_bytes()).expect("valid UTF-8 bytes decode");
        assert_eq!(from_bytes, envelope);
    }
}

// ---------------------------------------------------------------------------
// 3. Malformed-payload rejection, for every wire format.
// ---------------------------------------------------------------------------

/// A DTO missing a required field is rejected, whichever field is missing.
///
/// `AGENTS.md` §2.1 requires all incoming payloads to be validated against
/// schemas. serde's required fields are the *first* line of that: a message with
/// no `channel_id` cannot be placed, and a message with no `id` cannot be
/// ordered or deduped, so neither may reach `state/`.
#[test]
fn a_dto_missing_any_required_field_is_rejected() {
    let cases: [(&str, &str); 6] = [
        (
            "id",
            r#"{"client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":[]}"#,
        ),
        (
            "client_msg_id",
            r#"{"id":"m","channel_id":"c","user_id":"u","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":[]}"#,
        ),
        (
            "channel_id",
            r#"{"id":"m","client_msg_id":"x","user_id":"u","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":[]}"#,
        ),
        (
            "user_id",
            r#"{"id":"m","client_msg_id":"x","channel_id":"c","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":[]}"#,
        ),
        (
            "content",
            r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":[]}"#,
        ),
        (
            "timestamp",
            r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","reactions":[],"attachments":[]}"#,
        ),
    ];

    for (field, json) in cases {
        assert!(
            serde_json::from_str::<WireMessage>(json).is_err(),
            "a message missing `{field}` must not decode"
        );
    }
}

/// A DTO field of the wrong JSON type is rejected.
#[test]
fn a_dto_field_of_the_wrong_type_is_rejected() {
    let cases: [(&str, &str); 4] = [
        (
            "id as a number",
            r#"{"id":7,"client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":[]}"#,
        ),
        (
            "reactions as an object",
            r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":{},"attachments":[]}"#,
        ),
        (
            "attachments as null",
            r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":"2026-09-27T12:00:00Z","reactions":[],"attachments":null}"#,
        ),
        (
            "size as a float",
            r#"{"id":"a","filename":"f","url":"u","mime_type":"m","size":1.5}"#,
        ),
    ];

    for (description, json) in cases {
        assert!(
            serde_json::from_str::<WireMessage>(json).is_err()
                || serde_json::from_str::<WireAttachment>(json).is_err(),
            "a DTO with {description} must not decode"
        );
    }
}

/// An unknown value for a closed enum is rejected.
///
/// `WireUserStatus` is closed on purpose -- the crate root states that adding a
/// variant is a major-version commitment precisely because a client cannot
/// render a presence it has no case for. This test is where that promise is
/// kept.
#[test]
fn an_unknown_enum_variant_is_rejected() {
    let cases = [
        r#"{"id":"u","username":"ada","display_name":"Ada","avatar_url":null,"status":"away_bathing"}"#,
        r#"{"v":1,"type":"presence.update","user_id":"u_1","status":"away_bathing"}"#,
        r#"{"v":1,"type":"presence.update","user_id":"u_1","status":"ONLINE"}"#,
    ];

    for json in cases {
        assert!(
            serde_json::from_str::<WireUser>(json).is_err()
                || serde_json::from_str::<ServerEnvelope>(json).is_err(),
            "an unknown enum variant must not decode: {json}"
        );
    }
}

/// A timestamp that is not a parseable RFC-3339 string is rejected.
///
/// A bare integer is rejected too, and that is worth stating: a server that sent
/// milliseconds where RFC-3339 was specified is a *bug we want to see*, not a
/// value to guess at. `PLAN.md` §3 forbids hand-formatting timestamps precisely
/// so that a unit mistake is a decode failure rather than a message ordered
/// 55,000 years from the others.
#[test]
fn an_unparseable_timestamp_is_rejected() {
    let cases = [
        r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":"not-a-date","reactions":[],"attachments":[]}"#,
        r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":1790616000,"reactions":[],"attachments":[]}"#,
        r#"{"id":"m","client_msg_id":"x","channel_id":"c","user_id":"u","content":"hi","timestamp":"2026-09-27 12:00:00","reactions":[],"attachments":[]}"#,
    ];

    for json in cases {
        assert!(
            serde_json::from_str::<WireMessage>(json).is_err(),
            "an unparseable timestamp must not decode: {json}"
        );
    }
}

/// A frame that is not JSON at all is rejected, and reported as malformed
/// rather than as a protocol violation.
#[test]
fn malformed_json_is_rejected_as_a_malformed_payload() {
    for json in ["", "{", "not json", r#"{"v":1,"type":}"#, "\u{0}\u{1}"] {
        let error = ServerEnvelope::decode(json).expect_err("malformed JSON must not decode");
        assert!(
            matches!(error, WireError::MalformedPayload(_)),
            "{json:?} produced {error:?}, expected a malformed payload"
        );
        assert!(
            !error.is_fatal(),
            "one malformed frame must not take the connection down"
        );
    }
}

/// Bytes that are not valid UTF-8 are reported as such, not as a JSON error.
///
/// A WebSocket text frame is required to be UTF-8, so this is what a peer
/// sending a *binary* frame where a text frame was required looks like. The
/// distinction matters: "expected value" sends a reader looking at the JSON
/// grammar, and the bug is one layer down.
#[test]
fn invalid_utf8_bytes_are_reported_as_invalid_utf8() {
    // 0xFF is never valid in UTF-8, in any position.
    let bytes = br#"{"v":1,"type":"error","code":"x","detail":""#.to_vec();
    let mut payload = bytes;
    payload.push(0xFF);
    payload.extend_from_slice(br#""}"#);

    let error = ServerEnvelope::decode_bytes(&payload).expect_err("invalid UTF-8 must not decode");
    assert!(
        matches!(error, WireError::InvalidUtf8(_)),
        "expected InvalidUtf8, got {error:?}"
    );
    assert!(
        !error.is_fatal(),
        "one bad frame must not take the connection down"
    );

    let client_error = ClientEnvelope::decode_bytes(&payload)
        .expect_err("invalid UTF-8 must not decode in either direction");
    assert!(matches!(client_error, WireError::InvalidUtf8(_)));
}

/// A frame with an unrecognised `type` is rejected, and is not fatal.
#[test]
fn an_unknown_frame_type_is_rejected() {
    let cases = [
        r#"{"v":1,"type":"message.delete","client_msg_id":"x"}"#,
        r#"{"v":1,"type":"message_send","client_msg_id":"x"}"#,
        r#"{"v":1,"type":"","client_msg_id":"x"}"#,
    ];

    for json in cases {
        let error = ServerEnvelope::decode(json).expect_err("an unknown type must not decode");
        assert!(
            matches!(error, WireError::UnknownFrameType { .. }),
            "{json} produced {error:?}"
        );
        assert!(
            !error.is_fatal(),
            "an unknown frame type is expected during a rolling deploy"
        );
    }
}

/// A frame whose `type` belongs to the other direction is rejected.
///
/// A client that accepted a `message.send` from a server, or a `message.new`
/// from a client, would be acting on a frame the peer never meant to send it.
#[test]
fn a_frame_read_in_the_wrong_direction_is_rejected() {
    let client_only = r#"{"v":1,"type":"typing.start","client_msg_id":"x","channel_id":"c"}"#;
    let error = ServerEnvelope::decode(client_only)
        .expect_err("a client frame must not be accepted as a server frame");
    assert!(
        matches!(error, WireError::WrongDirection { .. }),
        "expected WrongDirection, got {error:?}"
    );

    let server_only = r#"{"v":1,"type":"presence.update","user_id":"u","status":"online"}"#;
    let error = ClientEnvelope::decode(server_only)
        .expect_err("a server frame must not be accepted as a client frame");
    assert!(matches!(error, WireError::WrongDirection { .. }));
}

/// A frame with no version, or a non-integer version, is rejected.
///
/// `v` being absent is not "assume the current version" -- that is precisely the
/// silent-acceptance failure `AGENTS.md` §7.4 forbids, and the difference
/// matters most for the client that is *too old*, which is the one that cannot
/// tell that anything is wrong.
#[test]
fn a_frame_without_a_usable_version_is_rejected() {
    let cases = [
        r#"{"type":"error","code":"x","detail":"y"}"#,
        r#"{"v":null,"type":"error","code":"x","detail":"y"}"#,
        r#"{"v":"1","type":"error","code":"x","detail":"y"}"#,
        r#"{"v":1.5,"type":"error","code":"x","detail":"y"}"#,
        r#"{"v":-1,"type":"error","code":"x","detail":"y"}"#,
    ];

    for json in cases {
        assert!(
            ServerEnvelope::decode(json).is_err(),
            "a frame without a usable `v` must not decode: {json}"
        );
        assert!(
            ClientEnvelope::decode(json).is_err(),
            "a frame without a usable `v` must not decode: {json}"
        );
    }
}

/// A frame missing its `client_msg_id` is rejected.
///
/// `AGENTS.md` §7.4 requires the id on *every* client frame. Because the field
/// lives on the envelope, "every" is a property of the type; this test is the
/// runtime proof that the property holds.
#[test]
fn a_client_frame_without_a_client_msg_id_is_rejected() {
    let cases = [
        r#"{"v":1,"type":"message.send","channel_id":"c","content":"hi"}"#,
        r#"{"v":1,"type":"typing.start","channel_id":"c"}"#,
        r#"{"v":1,"type":"resync","channel_id":"c","after":"2026-09-27T12:00:00Z"}"#,
    ];

    for json in cases {
        assert!(
            ClientEnvelope::decode(json).is_err(),
            "a client frame with no client_msg_id must not decode: {json}"
        );
    }
}

/// A frame with an unknown field is **accepted**, and the field is ignored.
///
/// This is the forward-compatibility half of the crate's compatibility policy,
/// and it is a test rather than a comment because it is easy to break by
/// accident: adding `deny_unknown_fields` to any wire struct would turn a
/// routine rolling deploy into a hard decode failure for every older client. The
/// failure would be invisible until a deploy, which is the worst time to find
/// it.
#[test]
fn unknown_fields_are_ignored_for_forward_compatibility() {
    let json = r#"{"v":1,"type":"typing.update","user_id":"u_1","channel_id":"c_1","active":true,"added_in_a_later_minor":{"nested":[1,2,3]}}"#;
    let decoded = ServerEnvelope::decode(json).expect("an unknown field must not break decoding");
    assert_eq!(
        decoded,
        ServerEnvelope::new(ServerFrame::TypingUpdate {
            user_id: "u_1".to_owned(),
            channel_id: "c_1".to_owned(),
            active: true,
        }),
        "the unknown field should have been ignored, and the known ones kept"
    );
}

// ---------------------------------------------------------------------------
// 4. `resync` is scoped per channel, not globally.
// ---------------------------------------------------------------------------

/// `resync` carries a `channel_id` alongside its cursor.
///
/// The type makes a global resync inexpressible: there is no constructor and no
/// JSON shape for one. `PLAN.md` §6 documents why a single global cursor cannot
/// reconstruct per-channel history -- each channel has its own `last_message_at`
/// and its own unread count, so one cursor is either too old (duplicates in the
/// channel the user had read) or too new (gaps in the channel they had not).
/// `AGENTS.md` §8.1's Reconnect Flow requires no duplicates *and* no gaps, which
/// is unsatisfiable without this.
#[test]
fn resync_carries_a_per_channel_cursor() {
    let early = ClientFrame::Resync {
        channel_id: "general".to_owned(),
        after: stamp(),
    };
    let late = ClientFrame::Resync {
        channel_id: "general".to_owned(),
        after: chrono::DateTime::parse_from_rfc3339("2026-09-27T10:00:00Z")
            .expect("valid RFC-3339")
            .with_timezone(&chrono::Utc),
    };

    // Two channels at different cursors are two different frames, which is the
    // whole point: a global cursor cannot express this pair at all.
    let other_channel = ClientFrame::Resync {
        channel_id: "design".to_owned(),
        after: stamp(),
    };
    assert_ne!(
        early, late,
        "two cursors in one channel must be distinguishable"
    );
    assert_ne!(
        early, other_channel,
        "the same cursor in two channels must be distinguishable"
    );

    let json = ClientEnvelope::new("cid-1", early)
        .encode()
        .expect("resync encodes");
    assert!(
        json.contains(r#""type":"resync""#)
            && json.contains(r#""channel_id":"general""#)
            && json.contains(r#""after":"2026-09-27T12:00:00Z""#),
        "resync must carry both the channel and the cursor, got {json}"
    );
}

// ---------------------------------------------------------------------------
// 5. The frame-type registry is complete and internally consistent.
// ---------------------------------------------------------------------------

/// Every frame kind's `as_str` equals the `type` the encoder actually emits.
///
/// `FrameKind::as_str` and the per-variant `#[serde(rename)]` are two spellings
/// of the same fact. This asserts they agree for all twelve, so a rename that
/// changes one without the other is a test failure rather than a protocol that
/// resolves unknown types in the wrong direction.
#[test]
fn frame_type_names_agree_between_frame_kind_and_the_encoder() {
    for frame in every_client_frame() {
        let expected = frame.kind().as_str().to_owned();
        let envelope = ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame);
        let json = envelope.encode().expect("a client frame encodes");
        assert!(
            json.contains(&format!(r#""type":"{expected}""#)),
            "expected {expected:?} in {json}"
        );
    }
    for frame in every_server_frame() {
        let expected = frame.kind().as_str().to_owned();
        let envelope = ServerEnvelope::new(frame);
        let json = envelope.encode().expect("a server frame encodes");
        assert!(
            json.contains(&format!(r#""type":"{expected}""#)),
            "expected {expected:?} in {json}"
        );
    }
}

/// `FrameKind::all` is exactly the union of the two direction lists.
///
/// Guards two failure modes at once: a kind added to neither list (so nothing
/// ever resolves it) and a kind added to both (so the direction check becomes
/// ambiguous). Both would leave the protocol's frame registry quietly wrong.
#[test]
fn frame_kind_lists_partition_the_protocol() {
    let all = FrameKind::all();
    assert_eq!(
        all.len(),
        FrameKind::CLIENT.len() + FrameKind::SERVER.len(),
        "FrameKind::all() must be exactly the client and server lists"
    );
    for kind in all {
        assert_eq!(
            FrameKind::from_name(kind.as_str()),
            Some(kind),
            "{} does not resolve from its own name",
            kind.as_str()
        );
    }
    for kind in FrameKind::CLIENT {
        assert_eq!(
            kind.direction(),
            Direction::Client,
            "{kind} is a client frame"
        );
        assert!(matches!(kind.require_direction(Direction::Client), Ok(())));
        assert!(matches!(
            kind.require_direction(Direction::Server),
            Err(WireError::WrongDirection { .. })
        ));
    }
    for kind in FrameKind::SERVER {
        assert_eq!(
            kind.direction(),
            Direction::Server,
            "{kind} is a server frame"
        );
    }
}

/// An unresolvable name returns `None` rather than a default kind.
///
/// A lookup that returned, say, `FrameKind::Error` for an unrecognised name
/// would turn a typo in a peer into a bogus `error` frame -- the protocol
/// reporting a problem it invented.
#[test]
fn an_unrecognised_frame_name_resolves_to_nothing() {
    for name in [
        "message.delete",
        "message_send",
        "MESSAGE.SEND",
        " message.send",
        "message.send ",
        "",
        "error ",
    ] {
        assert_eq!(
            FrameKind::from_name(name),
            None,
            "{name:?} must not resolve to a frame kind"
        );
    }
}

/// Every `ServerFrame` variant reports its own kind.
///
/// **This `match` has no wildcard arm on purpose.** Adding a `ServerFrame`
/// variant without adding it here is a compile error, which is the only way to
/// make "every frame type is tested" a property of the build rather than a
/// review habit. `support::every_server_frame` carries the runtime half.
#[test]
fn server_frame_kind_is_total() {
    for frame in every_server_frame() {
        let kind = exhaustive_server_kind(&frame);
        assert_eq!(kind, frame.kind());
    }
}

/// Exhaustive over `ServerFrame`; see [`server_frame_kind_is_total`].
fn exhaustive_server_kind(frame: &ServerFrame) -> FrameKind {
    match frame {
        ServerFrame::MessageAck { .. } => FrameKind::MessageAck,
        ServerFrame::MessageNew { .. } => FrameKind::MessageNew,
        ServerFrame::MessageError { .. } => FrameKind::MessageError,
        ServerFrame::ReactionUpdate { .. } => FrameKind::ReactionUpdate,
        ServerFrame::TypingUpdate { .. } => FrameKind::TypingUpdate,
        ServerFrame::PresenceUpdate { .. } => FrameKind::PresenceUpdate,
        ServerFrame::Error { .. } => FrameKind::Error,
    }
}

/// Every envelope is stamped with this build's protocol version by `new`.
#[test]
fn a_new_envelope_carries_the_current_protocol_version() {
    for frame in every_client_frame() {
        assert_eq!(
            ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame).v,
            PROTOCOL_VERSION
        );
    }
    for frame in every_server_frame() {
        assert_eq!(ServerEnvelope::new(frame).v, PROTOCOL_VERSION);
    }
}

// ---------------------------------------------------------------------------
// 6. An envelope's `kind()` reaches through to its frame.
//
// One case per frame rather than a loop over a case table, per `AGENTS.md` 4.3
// and ADR-008: a failure names the frame that broke instead of printing an index
// into a list, and the frames that still work still report as passing.
// ---------------------------------------------------------------------------

/// `ClientEnvelope::kind()` reports the kind of the frame it carries.
///
/// A delegating accessor, and the whole point of the test is that it delegates
/// **to the frame it actually holds** -- not to a constant, and not to the wrong
/// field. It is what a server-side dispatcher calls to route an incoming frame
/// without matching on the payload, so an accessor that answered wrongly for one
/// variant would route that frame into the wrong handler.
#[rstest]
#[case(ClientFrame::MessageSend { channel_id: "c_1".to_owned(), content: "hello team".to_owned() }, FrameKind::MessageSend)]
#[case(ClientFrame::ReactionAdd { message_id: "m_1".to_owned(), emoji: "\u{1f44d}".to_owned() }, FrameKind::ReactionAdd)]
#[case(ClientFrame::TypingStart { channel_id: "c_1".to_owned() }, FrameKind::TypingStart)]
#[case(ClientFrame::TypingStop { channel_id: "c_1".to_owned() }, FrameKind::TypingStop)]
#[case(ClientFrame::Resync { channel_id: "c_1".to_owned(), after: stamp() }, FrameKind::Resync)]
fn a_client_envelope_reports_its_frames_kind(
    #[case] frame: ClientFrame,
    #[case] expected: FrameKind,
) {
    let envelope = ClientEnvelope::new(SAMPLE_CLIENT_MSG_ID, frame);

    assert_eq!(envelope.kind(), expected);
}

/// `ServerEnvelope::kind()` reports the kind of the frame it carries.
///
/// Not symmetrical with the client one for a reason worth keeping in view: a
/// `ServerEnvelope` carries no blanket `client_msg_id` (`PLAN.md` section 6 gives
/// one only to the two frames that answer a send), so the accessor is the *only*
/// uniform way to ask a server frame what it is. That makes "it delegates
/// correctly, for all seven" the property that matters, not one variant.
#[rstest]
#[case(ServerFrame::MessageAck { client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(), message: sample_message() }, FrameKind::MessageAck)]
#[case(ServerFrame::MessageNew { message: minimal_message() }, FrameKind::MessageNew)]
#[case(ServerFrame::MessageError { client_msg_id: SAMPLE_CLIENT_MSG_ID.to_owned(), code: "message_too_long".to_owned(), detail: "exceeds 4000 characters".to_owned() }, FrameKind::MessageError)]
#[case(ServerFrame::ReactionUpdate { message_id: "m_1".to_owned(), emoji: "\u{1f44d}".to_owned(), user_id: "u_1".to_owned() }, FrameKind::ReactionUpdate)]
#[case(ServerFrame::TypingUpdate { user_id: "u_1".to_owned(), channel_id: "c_1".to_owned(), active: true }, FrameKind::TypingUpdate)]
#[case(ServerFrame::PresenceUpdate { user_id: "u_1".to_owned(), status: WireUserStatus::Away }, FrameKind::PresenceUpdate)]
#[case(ServerFrame::Error { code: "auth_expired".to_owned(), detail: "the session token has expired".to_owned() }, FrameKind::Error)]
fn a_server_envelope_reports_its_frames_kind(
    #[case] frame: ServerFrame,
    #[case] expected: FrameKind,
) {
    let envelope = ServerEnvelope::new(frame);

    assert_eq!(envelope.kind(), expected);
}

/// The two case lists above cover the frame registry exactly.
///
/// The runtime half of the compile-time guard `server_frame_kind_is_total`
/// provides for the `match`: if a variant is added to `ClientFrame` or
/// `ServerFrame` and not to a `#[case]` here, this fails and says which list is
/// short. Twelve cases, one per protocol frame, asserted against the counts the
/// registry itself reports -- so the count cannot drift away from the list.
#[test]
fn the_envelope_kind_cases_cover_every_frame_type() {
    let client_cases: HashSet<FrameKind> = HashSet::from([
        FrameKind::MessageSend,
        FrameKind::ReactionAdd,
        FrameKind::TypingStart,
        FrameKind::TypingStop,
        FrameKind::Resync,
    ]);
    let server_cases: HashSet<FrameKind> = HashSet::from([
        FrameKind::MessageAck,
        FrameKind::MessageNew,
        FrameKind::MessageError,
        FrameKind::ReactionUpdate,
        FrameKind::TypingUpdate,
        FrameKind::PresenceUpdate,
        FrameKind::Error,
    ]);

    assert_eq!(client_cases.len(), FrameKind::CLIENT.len());
    assert_eq!(server_cases.len(), FrameKind::SERVER.len());
    assert!(
        FrameKind::CLIENT
            .iter()
            .all(|kind| client_cases.contains(kind)),
        "a client frame has no `kind()` case, so its delegating accessor is untested"
    );
    assert!(
        FrameKind::SERVER
            .iter()
            .all(|kind| server_cases.contains(kind)),
        "a server frame has no `kind()` case, so its delegating accessor is untested"
    );
}
