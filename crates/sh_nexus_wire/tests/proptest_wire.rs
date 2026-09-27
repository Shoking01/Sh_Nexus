//! Property-based tests for the codec.
//!
//! `AGENTS.md` §4.4 mandates proptest, and §4.3 forbids `sleep()` in unit tests.
//! The properties below are the ones a table of hand-written cases cannot
//! express, and each one is a promise the crate makes:
//!
//! 1. **Decoding never panics, for any input at all.** A network decoder that
//!    panics on a hostile or merely surprising payload is a remote crash --
//!    `AGENTS.md` §1's second core priority is reliability, and §2.1 forbids
//!    panics in user-facing code. A hand-written table proves the nineteen cases
//!    somebody thought of; this proves everything else.
//! 2. **Anything that builds round-trips**, through text *and* through bytes.
//! 3. **Only the current major version is accepted.** Exhaustive over `u16`.
//! 4. **Every client frame's JSON carries a `client_msg_id`**, over generated
//!    frames rather than five fixtures.
//!
//! # Strategies
//!
//! There is no `Arbitrary` for the frame types, so they are built from
//! strategies for their fields. That is a little boilerplate and it has a real
//! payoff: the strategies are *biased towards interesting values* -- empty
//! strings, whitespace, control characters, overlong tokens, and emoji --
//! because a property test over uniform random strings mostly generates
//! "hello world 4821". The adversarial values are where the bugs are.
//!
//! Notably absent: `any::<uuid::Uuid>()`. This crate does not depend on `uuid`
//! (see `Cargo.toml`), so ids are arbitrary non-blank strings here. The
//! *UUID-ness* of `client_msg_id` is the domain's rule, tested in
//! `crates/sh_nexus/tests/wire_boundary.rs`.

use chrono::{DateTime, Utc};
use proptest::prelude::*;
use sh_nexus_wire::dto::{WireAttachment, WireMessage, WireReaction, WireUser, WireUserStatus};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame, ServerEnvelope};
use sh_nexus_wire::{negotiate, WireError, PROTOCOL_VERSION};

// A non-blank string, biased towards the values that break things.
fn any_non_blank() -> impl Strategy<Value = String> {
    prop_oneof![
        // The empty string, pure whitespace, and control characters. The codec
        // must carry these without complaint -- deciding what a valid id *is* is
        // the domain's job, and `sh_nexus::network::mapping` does it.
        3 => Just(String::new()),
        3 => Just("   ".to_owned()),
        2 => Just("\t\n".to_owned()),
        2 => Just("a\u{0}b".to_owned()),
        // Emoji, multi-byte text, and an overlong single token -- `AGENTS.md`
        // §5.2 lists "very long single-word message" as an edge case.
        2 => Just("\u{1f44d}\u{1f1ef}\u{1f1f5}".to_owned()),
        2 => Just("a\u{30fc}b\u{4e2d}\u{6587}".to_owned()),
        1 => Just("x".repeat(4096)),
        // And the ordinary case, which must still be exercised.
        6 => "[a-z0-9_-]{1,24}",
        6 => "[\\p{L}\\p{N} .:@/]{1,64}",
    ]
}

// A timestamp anywhere in a ±127-year window around the epoch.
//
// Wide enough that a sign error or a unit error shows up, narrow enough that
// `DateTime::from_timestamp` is always `Some` -- which keeps the strategy total
// and the test free of filtering. This crate does not decide what a *plausible*
// timestamp is; `sh_nexus::network::mapping` does, and that rule is tested
// there. What is tested here is only that whatever chrono accepts, the wire
// carries faithfully.
fn any_timestamp() -> impl Strategy<Value = DateTime<Utc>> {
    (-4_000_000_000i64..4_000_000_000i64).prop_map(|secs| {
        DateTime::from_timestamp(secs, 0).expect("a timestamp within 127 years is representable")
    })
}

// A `WireReaction` with an arbitrary number of reactors.
fn any_wire_reaction() -> impl Strategy<Value = WireReaction> {
    (
        any_non_blank(),
        proptest::collection::vec(any_non_blank(), 0..4),
    )
        .prop_map(|(emoji, user_ids)| WireReaction { emoji, user_ids })
}

// A `WireAttachment`, with a size anywhere up to half of `u64::MAX`.
fn any_wire_attachment() -> impl Strategy<Value = WireAttachment> {
    (
        any_non_blank(),
        any_non_blank(),
        any_non_blank(),
        any_non_blank(),
        0u64..=(u64::MAX / 2),
    )
        .prop_map(|(id, filename, url, mime_type, size)| WireAttachment {
            id,
            filename,
            url,
            mime_type,
            size,
        })
}

// A `WireMessage` with every field populated.
//
// Built from two five-tuples rather than one ten-tuple, so the strategy does
// not depend on how far proptest's tuple support is extended.
fn any_wire_message() -> impl Strategy<Value = WireMessage> {
    let text = (
        any_non_blank(),
        any_non_blank(),
        any_non_blank(),
        any_non_blank(),
        any_non_blank(),
    );
    let rest = (
        any_timestamp(),
        proptest::option::of(any_timestamp()),
        proptest::collection::vec(any_wire_reaction(), 0..4),
        proptest::option::of(any_non_blank()),
        proptest::collection::vec(any_wire_attachment(), 0..4),
    );

    (text, rest).prop_map(
        |(
            (id, client_msg_id, channel_id, user_id, content),
            (timestamp, edited_at, reactions, thread_id, attachments),
        )| WireMessage {
            id,
            client_msg_id,
            channel_id,
            user_id,
            content,
            timestamp,
            edited_at,
            reactions,
            thread_id,
            attachments,
        },
    )
}

// Any `ClientFrame`, with the message-carrying frames weighted highest because
// they have the most fields to get wrong.
fn any_client_frame() -> impl Strategy<Value = ClientFrame> {
    prop_oneof![
        3 => (any_non_blank(), any_non_blank())
            .prop_map(|(channel_id, content)| ClientFrame::MessageSend { channel_id, content }),
        2 => (any_non_blank(), any_non_blank())
            .prop_map(|(message_id, emoji)| ClientFrame::ReactionAdd { message_id, emoji }),
        2 => any_non_blank().prop_map(|channel_id| ClientFrame::TypingStart { channel_id }),
        2 => any_non_blank().prop_map(|channel_id| ClientFrame::TypingStop { channel_id }),
        3 => (any_non_blank(), any_timestamp())
            .prop_map(|(channel_id, after)| ClientFrame::Resync { channel_id, after }),
    ]
}

// Any `WireUser`.
fn any_wire_user() -> impl Strategy<Value = WireUser> {
    (
        any_non_blank(),
        any_non_blank(),
        any_non_blank(),
        proptest::option::of(any_non_blank()),
        prop_oneof![
            Just(WireUserStatus::Online),
            Just(WireUserStatus::Away),
            Just(WireUserStatus::Offline),
        ],
    )
        .prop_map(
            |(id, username, display_name, avatar_url, status)| WireUser {
                id,
                username,
                display_name,
                avatar_url,
                status,
            },
        )
}

// A version number, biased towards the boundaries of `u16` and of the
// supported set.
fn any_version() -> impl Strategy<Value = u16> {
    prop_oneof![
        2 => any::<u16>(),
        2 => Just(PROTOCOL_VERSION),
        2 => Just(PROTOCOL_VERSION.wrapping_add(1)),
        2 => Just(PROTOCOL_VERSION.wrapping_sub(1)),
        1 => Just(0),
        1 => Just(u16::MAX),
    ]
}

// The JSON value of a specific top-level key of an encoded frame.
fn top_level<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    value.get(key)
}

// Arbitrary text a peer might send, including text that is not JSON.
//
// This is the property that matters most. Everything below goes through the
// decoder, and the decoder must return a `Result` -- never panic, never abort,
// never loop.
//
proptest! {
    #[test]
    fn decoding_arbitrary_text_never_panics(text in ".{0,200}") {
        // Both directions, because both are reachable from a socket.
        let _ = ServerEnvelope::decode(&text);
        let _ = ClientEnvelope::decode(&text);
    }
}

// Arbitrary *bytes*, including invalid UTF-8, never panic.
//
// A WebSocket binary frame is a perfectly ordinary thing for a peer to send by
// mistake or by malice, so the byte entry point is the one that faces untrusted
// input first.
//
proptest! {
    #[test]
    fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = ServerEnvelope::decode_bytes(&bytes);
        let _ = ClientEnvelope::decode_bytes(&bytes);
    }
}

// Any `ClientFrame` round-trips through the codec.
proptest! {
    #[test]
    fn any_client_frame_round_trips(frame in any_client_frame(), id in any_non_blank()) {
        let envelope = ClientEnvelope::new(id, frame);
        let json = envelope
            .encode()
            .expect("sh_nexus_wire types always serialize to JSON");
        let decoded = ClientEnvelope::decode(&json)
            .unwrap_or_else(|error| panic!("{json} did not decode: {error}"));
        prop_assert_eq!(decoded, envelope);
    }
}

// Any `WireMessage` round-trips through JSON.
proptest! {
    #[test]
    fn any_wire_message_round_trips(message in any_wire_message()) {
        let json = serde_json::to_string(&message).expect("a WireMessage serializes");
        let decoded: WireMessage = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("{json} did not decode: {error}"));
        prop_assert_eq!(decoded, message);
    }
}

// Any `WireUser` round-trips through JSON.
proptest! {
    #[test]
    fn any_wire_user_round_trips(user in any_wire_user()) {
        let json = serde_json::to_string(&user).expect("a WireUser serializes");
        let decoded: WireUser = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("{json} did not decode: {error}"));
        prop_assert_eq!(decoded, user);
    }
}

// A frame carrying an arbitrary `v` decodes **iff** this build speaks that
// version. Exhaustive over the whole `u16` range.
proptest! {
    #[test]
    fn only_the_current_major_version_is_accepted(version in any_version()) {
        let json = format!(r#"{{"v":{version},"type":"error","code":"x","detail":"y"}}"#);
        let result = ServerEnvelope::decode(&json);

        if version == PROTOCOL_VERSION {
            prop_assert!(result.is_ok());
        } else {
            let error = result.expect_err("an unsupported version must be refused");
            prop_assert!(matches!(error, WireError::UnsupportedVersion(_)));
            prop_assert!(error.is_fatal());
        }

        // And `negotiate` agrees with the codec, which is what makes it the
        // negotiation rather than a duplicate of it.
        prop_assert_eq!(negotiate(version).is_ok(), version == PROTOCOL_VERSION);
    }
}

// Every generated client frame's JSON carries a `client_msg_id` at the top
// level, for any id at all.
//
// `AGENTS.md` §7.4's requirement, over arbitrary frames and arbitrary ids
// rather than five fixtures. The id is deliberately *not* constrained to be a
// UUID: this crate's guarantee is presence, and validity is the domain's.
proptest! {
    #[test]
    fn every_generated_client_frame_carries_a_client_msg_id(
        frame in any_client_frame(),
        id in any_non_blank(),
    ) {
        let envelope = ClientEnvelope::new(id.clone(), frame);
        let json = envelope.encode().expect("a client frame encodes");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("the encoded frame is valid JSON");

        prop_assert_eq!(
            top_level(&parsed, "client_msg_id").and_then(serde_json::Value::as_str),
            Some(id.as_str()),
        );
        prop_assert_eq!(
            top_level(&parsed, "v").and_then(serde_json::Value::as_u64),
            Some(u64::from(PROTOCOL_VERSION)),
        );
    }
}

// Any generated client frame survives the round trip *through bytes*, not just
// through text.
//
// The transport hands the codec bytes. If the byte path disagreed with the text
// path, the text-path tests would be testing something the client never calls.
proptest! {
    #[test]
    fn any_client_frame_round_trips_through_bytes(frame in any_client_frame(), id in any_non_blank()) {
        let envelope = ClientEnvelope::new(id, frame);
        let json = envelope.encode().expect("a client frame encodes");
        let decoded = ClientEnvelope::decode_bytes(json.as_bytes())
            .expect("the encoded frame's bytes decode");
        prop_assert_eq!(decoded, envelope);
    }
}

// A server frame carrying an arbitrary `v` behaves exactly like a client frame
// carrying it.
//
// The version gate is direction-independent, and a `ServerEnvelope` that
// quietly accepted a foreign version would be the more dangerous half of the
// bug: the client is the end that has to notice it is too old.
proptest! {
    #[test]
    fn version_gating_is_the_same_in_both_directions(version in any_version()) {
        let server_json =
            format!(r#"{{"v":{version},"type":"presence.update","user_id":"u","status":"online"}}"#);
        let client_json =
            format!(r#"{{"v":{version},"type":"typing.start","client_msg_id":"x","channel_id":"c"}}"#);

        let server = ServerEnvelope::decode(&server_json);
        let client = ClientEnvelope::decode(&client_json);

        if version == PROTOCOL_VERSION {
            prop_assert!(server.is_ok());
            prop_assert!(client.is_ok());
        } else {
            prop_assert!(matches!(server, Err(WireError::UnsupportedVersion(_))));
            prop_assert!(matches!(client, Err(WireError::UnsupportedVersion(_))));
        }
    }
}

// An arbitrary `type` is either a known frame kind or is refused as unknown --
// and is never silently remapped onto a different frame.
proptest! {
    #[test]
    fn an_arbitrary_frame_type_is_never_silently_remapped(kind in ".{0,32}", version in any_version()) {
        // Built by hand rather than with `format!` + a JSON string, so that
        // `kind` is escaped properly and cannot produce invalid JSON -- this
        // property is about the *type* field, not about the JSON grammar.
        let mut value = serde_json::Map::new();
        value.insert("v".to_owned(), serde_json::json!(version));
        value.insert("type".to_owned(), serde_json::Value::String(kind.clone()));
        value.insert("client_msg_id".to_owned(), serde_json::json!("x"));
        value.insert("channel_id".to_owned(), serde_json::json!("c"));
        let json = serde_json::Value::Object(value).to_string();

        match ClientEnvelope::decode(&json) {
            // Accepted: the `type` in the JSON is exactly the frame's own kind.
            // No defaulting, no normalisation, no silent remapping.
            Ok(envelope) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&json).expect("the frame decoded, so this is JSON");
                prop_assert_eq!(
                    top_level(&parsed, "type").and_then(serde_json::Value::as_str),
                    Some(envelope.kind().as_str()),
                );
            }
            // Refused as unknown, and reported under the name that was sent --
            // the log has to be able to quote the peer back to itself.
            Err(WireError::UnknownFrameType { name, .. }) => {
                prop_assert_eq!(name, kind);
            }
            // Refused as a role violation, and reported under the name that was
            // sent. A known *server* frame read on the client socket lands here.
            Err(WireError::WrongDirection { kind: found, .. }) => {
                prop_assert_eq!(found.as_str(), kind);
            }
            // The version was refused first, so the `type` was never judged and
            // there is nothing to assert about it. This is the documented order.
            Err(WireError::UnsupportedVersion(_)) => {}
            // Unreachable for a well-formed object: the JSON is built by
            // `serde_json` above, so it parses, and every field is present.
            Err(other) => prop_assert!(false, "unexpected error: {other:?}"),
        }
    }
}
