//! Property-based tests for the wire/domain boundary.
//!
//! `AGENTS.md` §4.4 mandates proptest, and §4.3 forbids `sleep()` in unit tests.
//! Two properties here are the ones a table of hand-written cases cannot state,
//! and both are the promise `AGENTS.md` §2.1 makes:
//!
//! > All incoming WebSocket and REST payloads must be validated against schemas
//! > before touching state.
//!
//! 1. **No wire value at all reaches the domain unvalidated.** The hand-written
//!    cases in `wire_boundary.rs` cover the fields somebody thought of. This
//!    covers the ones nobody did: any combination of blank ids, bad UUIDs,
//!    absurd timestamps and implausible sizes, in any arrangement.
//! 2. **Any valid value round-trips** through the boundary and the codec, with
//!    the `Uuid` projection intact.
//!
//! # What is deliberately *not* a property
//!
//! There is no property saying "the boundary rejects bad input", because that is
//! false and asserting it would be a lie about the design. `wire_boundary.rs`
//! proves the *fields* it validates are rejected. What is asserted here is the
//! weaker, true, and much more useful claim: **whatever comes in, what comes out
//! is either a value that satisfies every documented rule, or an error.** A
//! boundary that silently accepted a blank channel id would satisfy a naive
//! "never panics" property perfectly; this one checks the *result*.

use chrono::{DateTime, Utc};
use proptest::prelude::*;
use sh_nexus::core::models::events::DomainEvent;
use sh_nexus::core::models::message::{Attachment, Message, Reaction};
use sh_nexus::core::models::user::{User, UserStatus};
use sh_nexus::errors::ShNexusError;
use sh_nexus::network::mapping::{
    MAX_ATTACHMENT_BYTES, TIMESTAMP_CEILING_UNIX_SECS, TIMESTAMP_FLOOR_UNIX_SECS,
};
use sh_nexus_wire::dto::{WireAttachment, WireMessage, WireReaction, WireUser, WireUserStatus};
use sh_nexus_wire::frame::{ServerEnvelope, ServerFrame};
use uuid::Uuid;

// A string that is either a usable id or blank, biased towards blank.
//
// The generator, not the assertion, is where the adversarial values go. A
// property test over uniform random text mostly generates "hello world 4821";
// the bugs live in `""`, `"   "`, and control characters.
fn any_id() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[a-z0-9_-]{1,16}",
        3 => Just(String::new()),
        3 => Just("   ".to_owned()),
        1 => Just("\u{0}\u{1}".to_owned()),
        1 => Just("\u{feff}".to_owned()),
    ]
}

// A `client_msg_id` that is either a canonical UUID or something that is not.
fn any_client_msg_id() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => any::<[u8; 16]>().prop_map(|bytes| {
            Uuid::from_bytes(bytes).hyphenated().to_string()
        }),
        1 => Just(String::new()),
        1 => Just("not-a-uuid".to_owned()),
        1 => any_id(),
    ]
}

// A timestamp either inside or outside the plausible window.
fn any_timestamp() -> impl Strategy<Value = DateTime<Utc>> {
    prop_oneof![
        6 => (TIMESTAMP_FLOOR_UNIX_SECS..=TIMESTAMP_CEILING_UNIX_SECS)
            .prop_map(|secs| DateTime::from_timestamp(secs, 0).expect("inside the window")),
        1 => (0..TIMESTAMP_FLOOR_UNIX_SECS)
            .prop_map(|secs| DateTime::from_timestamp(secs, 0).expect("representable")),
        1 => (TIMESTAMP_CEILING_UNIX_SECS..=253_402_300_800)
            .prop_map(|secs| DateTime::from_timestamp(secs, 0).expect("representable")),
    ]
}

// A content string, blank about a third of the time.
fn any_content() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => ".{0,40}",
        2 => Just(String::new()),
        2 => Just("   ".to_owned()),
    ]
}

// A reaction, with a blank emoji and blank user ids both in the generator.
fn any_wire_reaction() -> impl Strategy<Value = WireReaction> {
    (
        prop_oneof![3 => ".{1,8}", 1 => Just(String::new()), 1 => Just("  ".to_owned())],
        prop::collection::vec(any_id(), 0..3),
    )
        .prop_map(|(emoji, user_ids)| WireReaction { emoji, user_ids })
}

// An attachment, with every field adversarial and the size spanning the limit.
fn any_wire_attachment() -> impl Strategy<Value = WireAttachment> {
    (
        any_id(),
        any_id(),
        any_id(),
        any_id(),
        prop_oneof![
            6 => 0u64..=MAX_ATTACHMENT_BYTES,
            2 => (MAX_ATTACHMENT_BYTES + 1)..=(u64::MAX / 2),
            1 => Just(u64::MAX),
        ],
    )
        .prop_map(|(id, filename, url, mime_type, size)| WireAttachment {
            id,
            filename,
            url,
            mime_type,
            size,
        })
}

// Any `WireMessage`, with every field independently adversarial.
fn any_wire_message() -> impl Strategy<Value = WireMessage> {
    let scalar = (any_id(), any_client_msg_id(), any_id(), any_id());
    let rest = (
        any_content(),
        any_timestamp(),
        proptest::option::of(any_timestamp()),
        proptest::collection::vec(any_wire_reaction(), 0..3),
        proptest::option::of(any_id()),
        proptest::collection::vec(any_wire_attachment(), 0..3),
    );

    (scalar, rest).prop_map(
        |(
            (id, client_msg_id, channel_id, user_id),
            (content, timestamp, edited_at, reactions, thread_id, attachments),
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

// Any `WireUser`.
fn any_wire_user() -> impl Strategy<Value = WireUser> {
    (
        any_id(),
        any_id(),
        any_id(),
        proptest::option::of(any_id()),
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

// **The central property.** Any wire message is either rejected, or produces a
// domain message that satisfies every rule the boundary documents.
//
// Stated as an implication rather than as "rejected iff invalid", because the
// boundary deliberately does not validate everything -- a blank `thread_id` is
// accepted, content length is unbounded, an attachment URL need not be a URL.
// Those are documented decisions, not oversights, and a property that asserted
// they were rejections would be asserting something false.
//
// What it *can* assert is the soundness direction: an `Ok` is never a lie. If
// the conversion succeeded, the result really does have non-blank ids, a
// `Uuid`, plausible timestamps, and a body.
proptest! {
    #[test]
    fn any_wire_message_either_fails_or_satisfies_every_documented_rule(
        wire in any_wire_message(),
    ) {
        match Message::try_from(wire.clone()) {
            Err(error) => {
                // A rejection is always a `Protocol` error: the boundary has no
                // other failure mode, and a second one would mean a caller
                // cannot tell "the payload was bad" from "we have a bug".
                prop_assert!(
                    matches!(error, ShNexusError::Protocol(_)),
                    "a rejection must be a Protocol error, got {error:?}"
                );
                // Whether the content is echoed is a *separate* property with a
                // separate generator -- see `a_rejection_never_echoes_the_message_content`.
                // It cannot be checked here: a substring test on arbitrary
                // content is meaningless, because `.{0,40}` will eventually
                // generate "id", which is a substring of "message.id" and of
                // every other field name in the boundary's vocabulary. A
                // property that fails on a coincidence teaches people to ignore
                // it.
            }
            Ok(message) => {
                // Rule 1: required ids are non-blank.
                prop_assert!(!message.id.trim().is_empty(), "message.id is blank");
                prop_assert!(
                    !message.channel_id.trim().is_empty(),
                    "message.channel_id is blank"
                );
                prop_assert!(!message.user_id.trim().is_empty(), "message.user_id is blank");

                // Rule 2: the client id is a real UUID, and it is the one that
                // was sent (modulo spelling -- see the accepted-spellings test).
                prop_assert_eq!(
                    Uuid::parse_str(wire.client_msg_id.trim()).map(|id| id == message.client_msg_id),
                    Ok(true),
                    "the stored client_msg_id must be the parsed incoming one"
                );

                // Rule 3: timestamps are inside the window.
                prop_assert!(message.timestamp.timestamp() >= TIMESTAMP_FLOOR_UNIX_SECS);
                prop_assert!(message.timestamp.timestamp() <= TIMESTAMP_CEILING_UNIX_SECS);
                if let Some(edited_at) = message.edited_at {
                    prop_assert!(edited_at.timestamp() >= TIMESTAMP_FLOOR_UNIX_SECS);
                    prop_assert!(edited_at.timestamp() <= TIMESTAMP_CEILING_UNIX_SECS);
                }

                // Rule 4: a body, or an attachment.
                prop_assert!(
                    !message.content.trim().is_empty() || !message.attachments.is_empty(),
                    "a message with neither body nor attachment was accepted"
                );

                // And the nested rules.
                for reaction in &message.reactions {
                    prop_assert!(!reaction.emoji.trim().is_empty(), "a blank emoji got in");
                    for user_id in &reaction.user_ids {
                        prop_assert!(!user_id.trim().is_empty(), "a blank reactor got in");
                    }
                }
                for attachment in &message.attachments {
                    prop_assert!(!attachment.id.trim().is_empty());
                    prop_assert!(!attachment.filename.trim().is_empty());
                    prop_assert!(!attachment.url.trim().is_empty());
                    prop_assert!(!attachment.mime_type.trim().is_empty());
                    prop_assert!(
                        attachment.size <= MAX_ATTACHMENT_BYTES,
                        "an over-limit attachment size got in"
                    );
                }

                // Content is carried verbatim: the boundary must not trim or
                // rewrite a user's text on the way in.
                prop_assert_eq!(message.content, wire.content);
                prop_assert_eq!(message.thread_id, wire.thread_id);
            }
        }
    }
}

// Any valid message survives both boundary directions and the codec.
proptest! {
    #[test]
    fn a_valid_message_round_trips_through_the_boundary_and_the_codec(
        id in "[a-z0-9]{1,12}",
        channel_id in "[a-z0-9]{1,12}",
        user_id in "[a-z0-9]{1,12}",
        client_bytes in any::<[u8; 16]>(),
        content in ".{1,40}",
        timestamp in any_timestamp(),
        thread_id in proptest::option::of("[a-z0-9]{1,12}"),
    ) {
        let wire = WireMessage {
            id: id.clone(),
            client_msg_id: Uuid::from_bytes(client_bytes).hyphenated().to_string(),
            channel_id: channel_id.clone(),
            user_id: user_id.clone(),
            content: content.clone(),
            timestamp,
            edited_at: None,
            reactions: Vec::new(),
            thread_id: thread_id.clone(),
            attachments: Vec::new(),
        };

        // Only run the round trip when the generated values are actually valid;
        // `timestamp` is drawn from a strategy that includes out-of-window
        // values on purpose, and this test is about fidelity, not validation.
        let Ok(domain) = Message::try_from(wire.clone()) else {
            return Ok(());
        };

        prop_assert_eq!(
            WireMessage::from(&domain),
            wire.clone(),
            "the projection is not the identity"
        );

        // And through the real codec, as a frame.
        let json = ServerEnvelope::new(ServerFrame::MessageNew {
            message: WireMessage::from(&domain),
        })
        .encode()
        .expect("encodes");
        let envelope = ServerEnvelope::decode(&json).expect("decodes");
        let ServerFrame::MessageNew { message } = envelope.frame else {
            return Err(TestCaseError::fail("expected a message.new frame"));
        };
        prop_assert_eq!(message, wire, "the codec changed the message");
    }
}

// Any valid user round-trips, and an accepted one always has non-blank ids.
proptest! {
    #[test]
    fn a_valid_user_round_trips_and_an_accepted_one_is_never_blank(
        wire in any_wire_user(),
    ) {
        match User::try_from(wire.clone()) {
            Err(error) => prop_assert!(matches!(error, ShNexusError::Protocol(_))),
            Ok(user) => {
                prop_assert!(!user.id.trim().is_empty());
                prop_assert!(!user.username.trim().is_empty());
                prop_assert!(!user.display_name.trim().is_empty());
                prop_assert_eq!(
                    WireUser::from(&user),
                    wire.clone(),
                    "the projection is not the identity"
                );
                prop_assert_eq!(WireUserStatus::from(user.status), wire.status);
            }
        }
    }
}

// Any `message.new` frame either fails or produces a fully valid
// [`DomainEvent::MessageReceived`].
//
// The end-to-end property, from a frame to an event. It is the one that would
// catch a future change that validated a bare `WireMessage` but forgot that
// frames reach the domain through a different conversion.
proptest! {
    #[test]
    fn any_message_new_frame_either_fails_or_yields_a_valid_event(wire in any_wire_message()) {
        let result = DomainEvent::try_from(ServerFrame::MessageNew { message: wire });

        match result {
            Err(error) => prop_assert!(matches!(error, ShNexusError::Protocol(_))),
            Ok(event) => {
                let DomainEvent::MessageReceived(message) = event else {
                    return Err(TestCaseError::fail("message.new must yield MessageReceived"));
                };
                prop_assert!(!message.id.trim().is_empty());
                prop_assert!(!message.channel_id.trim().is_empty());
                prop_assert!(!message.user_id.trim().is_empty());
                prop_assert!(message.timestamp.timestamp() >= TIMESTAMP_FLOOR_UNIX_SECS);
                prop_assert!(message.timestamp.timestamp() <= TIMESTAMP_CEILING_UNIX_SECS);
                prop_assert!(
                    !message.content.trim().is_empty() || !message.attachments.is_empty()
                );
            }
        }
    }
}

// Any JSON text that decodes to a `message.new` either fails or yields a valid
// event -- with the codec in the loop this time.
//
// The property above starts from a `ServerFrame`; this one starts from the
// bytes, so a defect in the codec's interaction with the boundary shows up here
// rather than only in a unit test of one of the two.
proptest! {
    #[test]
    fn any_json_either_fails_to_decode_or_yields_a_valid_event(json in ".{0,400}") {
        // Whatever happens, nothing panics and nothing invents a value.
        let decoded = ServerEnvelope::decode(&json);
        let Ok(envelope) = decoded else {
            return Ok(());
        };
        let ServerFrame::MessageNew { message } = envelope.frame else {
            return Ok(());
        };

        match Message::try_from(message) {
            Err(error) => prop_assert!(matches!(error, ShNexusError::Protocol(_))),
            Ok(domain) => {
                prop_assert!(!domain.id.trim().is_empty());
                prop_assert!(!domain.channel_id.trim().is_empty());
                prop_assert!(!domain.user_id.trim().is_empty());
                // And the nested invariants, since a frame can carry them.
                for reaction in &domain.reactions {
                    prop_assert!(!reaction.emoji.trim().is_empty());
                }
                for attachment in &domain.attachments {
                    prop_assert!(attachment.size <= MAX_ATTACHMENT_BYTES);
                }
            }
        }
    }
}

// `Reaction` and `Attachment` convert both ways without loss.
proptest! {
    #[test]
    fn nested_collections_round_trip(wire in any_wire_message()) {
        let Ok(domain) = Message::try_from(wire.clone()) else {
            return Ok(());
        };

        // Same length on both sides: the conversion is a 1:1 projection, and a
        // length mismatch would mean a reaction or attachment was dropped --
        // which for a chat client is data loss, not a rendering detail.
        prop_assert_eq!(domain.reactions.len(), wire.reactions.len());
        prop_assert_eq!(domain.attachments.len(), wire.attachments.len());

        for (reaction, wire_reaction) in domain.reactions.iter().zip(&wire.reactions) {
            // The wire reaction validated as part of the message, so it converts
            // again here; a failure would mean the rule is not idempotent.
            match Reaction::try_from(wire_reaction.clone()) {
                Ok(again) => prop_assert_eq!(reaction, &again),
                Err(error) => prop_assert!(false, "a validated reaction should convert again: {error}"),
            }
        }

        for (attachment, wire_attachment) in domain.attachments.iter().zip(&wire.attachments) {
            match Attachment::try_from(wire_attachment.clone()) {
                Ok(again) => prop_assert_eq!(attachment, &again),
                Err(error) => prop_assert!(false, "a validated attachment should convert again: {error}"),
            }
        }
    }
}

// `UserStatus` converts exhaustively and invertibly, for every value.
proptest! {
    #[test]
    fn user_status_round_trips(index in 0u8..3) {
        let domain = match index {
            0 => UserStatus::Online,
            1 => UserStatus::Away,
            _ => UserStatus::Offline,
        };
        let wire = WireUserStatus::from(domain);
        prop_assert_eq!(UserStatus::from(wire), domain);
    }
}

/// A marker that cannot occur in any of this crate's own error text.
///
/// The point of the sentinel: the no-echo rule can only be tested by substring,
/// and a substring test over arbitrary content is worthless because short content
/// collides with field names. A token that appears nowhere in the boundary's
/// vocabulary turns "did the value get echoed?" into a question with a real
/// answer.
const CONTENT_SENTINEL: &str = "zzq7x-content-must-never-be-echoed-zzq7x";

// Every rejection path, with the message content marked so an echo is
// detectable. `AGENTS.md` 7.5 forbids logging message content, and an error that
// can reach a log is subject to the same rule -- so this is a rule about the
// *text* of an error, and text is exactly what a test can see.
//
// The rejection reasons are varied on purpose: a blank id, a bad UUID, a bad
// timestamp, a blank reaction emoji and an over-limit attachment size all fail
// at different points in the conversion, and a rule honoured in one and not
// another would be a rule nobody wrote down.
proptest! {
    #[test]
    fn a_rejection_never_echoes_the_message_content(suffix in ".{0,60}", reason in 0u8..5) {
        let content = format!("{CONTENT_SENTINEL}{suffix}");

        let mut wire = WireMessage {
            id: "m_1".to_owned(),
            client_msg_id: Uuid::from_bytes([7u8; 16]).hyphenated().to_string(),
            channel_id: "c_1".to_owned(),
            user_id: "u_1".to_owned(),
            content: content.clone(),
            timestamp: DateTime::from_timestamp(1_790_616_000, 0).expect("representable"),
            edited_at: None,
            reactions: Vec::new(),
            thread_id: None,
            attachments: Vec::new(),
        };

        // Break exactly one thing, in five different ways.
        match reason {
            0 => wire.channel_id = String::new(),
            1 => wire.client_msg_id = "not-a-uuid".to_owned(),
            2 => wire.timestamp = DateTime::from_timestamp(100_000_000_000, 0)
                .expect("representable"),
            3 => wire.reactions = vec![WireReaction {
                emoji: String::new(),
                user_ids: vec!["u_1".to_owned()],
            }],
            _ => wire.attachments = vec![WireAttachment {
                id: "a_1".to_owned(),
                filename: "shot.png".to_owned(),
                url: "https://example.invalid/shot.png".to_owned(),
                mime_type: "image/png".to_owned(),
                size: u64::MAX,
            }],
        }

        let error = Message::try_from(wire)
            .err()
            .unwrap_or_else(|| panic!("reason {reason} should be rejected"));
        let rendered = error.to_string();

        prop_assert!(
            !rendered.contains(CONTENT_SENTINEL),
            "the error echoed the message content: {rendered}"
        );
        // And the rule it did report: an id may be quoted, content may not.
        prop_assert!(rendered.starts_with("protocol error: "));
    }
}

// A `client_msg_id` is the one attacker-controlled id, so its rejection must not
// echo it either. Pinned with a sentinel for the same reason as above, and
// asserted for a hostile id specifically: this is the value an attacker supplies.
proptest! {
    #[test]
    fn a_bad_client_msg_id_is_never_echoed(suffix in ".{0,60}") {
        let hostile = format!("zzq7x-hostile-id-{suffix}-zzq7x");
        let mut wire = WireMessage {
            id: "m_1".to_owned(),
            client_msg_id: hostile.clone(),
            channel_id: "c_1".to_owned(),
            user_id: "u_1".to_owned(),
            content: "hello".to_owned(),
            timestamp: DateTime::from_timestamp(1_790_616_000, 0).expect("representable"),
            edited_at: None,
            reactions: Vec::new(),
            thread_id: None,
            attachments: Vec::new(),
        };
        wire.channel_id = wire.channel_id.clone();

        let error = Message::try_from(wire).expect_err("a hostile id must be rejected");
        let rendered = error.to_string();

        prop_assert!(
            !rendered.contains("zzq7x-hostile-id-"),
            "the error echoed the rejected id: {rendered}"
        );
        prop_assert_eq!(
            rendered,
            "protocol error: message.client_msg_id must be a UUID"
        );
    }
}
