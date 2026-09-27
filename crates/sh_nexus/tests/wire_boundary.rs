//! The wire/domain boundary: conversions in both directions, and the validation
//! that `AGENTS.md` §2.1 requires.
//!
//! `PLAN.md` §5 puts the validation here, and this file is where it is proved:
//!
//! > A malformed wire payload fails the conversion and never reaches `state/`.
//!
//! `AGENTS.md` §4.2's row for `models/` -- "Serde round-trips for every wire
//! format; malformed payload rejection" -- is discharged in two places: the
//! *format* half in `crates/sh_nexus_wire/tests/wire_frames.rs`, and the
//! *rejection* half here. Both are needed. A round trip proves the encoding is
//! faithful; it says nothing about whether a payload that is not the encoding is
//! refused.
//!
//! # Test placement
//!
//! Flat in `crates/sh_nexus/tests/`, so Cargo discovers it. `PLAN.md` §4 records
//! what happens to a test written at `tests/integration/*.rs`: it is silently
//! never compiled, and the suite looks green while containing nothing.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sh_nexus::core::models::channel::Channel;
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::{Attachment, Message, Reaction};
use sh_nexus::core::models::user::{User, UserStatus};
use sh_nexus::errors::{Result, ShNexusError};
use sh_nexus::network::mapping::{
    parse_client_msg_id, reduce_wire_error, MAX_ATTACHMENT_BYTES, TIMESTAMP_CEILING_UNIX_SECS,
    TIMESTAMP_FLOOR_UNIX_SECS,
};
use sh_nexus_wire::dto::{
    WireAttachment, WireChannel, WireMessage, WireReaction, WireUser, WireUserStatus,
};
use sh_nexus_wire::frame::{ServerEnvelope, ServerFrame};
use sh_nexus_wire::WireError;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CLIENT_MSG_ID: &str = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";

/// A `DateTime<Utc>` inside the plausible window, built from a literal so the
/// tests do not depend on chrono's default features or on a clock.
fn stamp() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-27T12:00:00Z")
        .expect("the fixture timestamp is valid RFC-3339")
        .with_timezone(&Utc)
}

/// A `DateTime<Utc>` before the plausible floor: 1999-12-31T23:59:59Z.
fn before_floor() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("1999-12-31T23:59:59Z")
        .expect("the fixture timestamp is valid RFC-3339")
        .with_timezone(&Utc)
}

/// A `DateTime<Utc>` after the plausible ceiling: 2100-01-01T00:00:01Z.
fn after_ceiling() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2100-01-01T00:00:01Z")
        .expect("the fixture timestamp is valid RFC-3339")
        .with_timezone(&Utc)
}

/// A valid message, before validation. Every negative test mutates one field of
/// this, so a test that fails says which field broke.
fn wire_message() -> WireMessage {
    WireMessage {
        id: "m_1".to_owned(),
        client_msg_id: CLIENT_MSG_ID.to_owned(),
        channel_id: "c_1".to_owned(),
        user_id: "u_1".to_owned(),
        content: "hello team".to_owned(),
        timestamp: stamp(),
        edited_at: None,
        reactions: Vec::new(),
        thread_id: None,
        attachments: Vec::new(),
    }
}

fn wire_user() -> WireUser {
    WireUser {
        id: "u_1".to_owned(),
        username: "ada".to_owned(),
        display_name: "Ada Lovelace".to_owned(),
        avatar_url: None,
        status: WireUserStatus::Online,
    }
}

fn wire_channel() -> WireChannel {
    WireChannel {
        id: "c_1".to_owned(),
        name: "general".to_owned(),
        description: Some("everything".to_owned()),
        is_private: false,
        members: vec!["u_1".to_owned(), "u_2".to_owned()],
        last_message_at: Some(stamp()),
    }
}

// ---------------------------------------------------------------------------
// Happy path: wire -> domain
// ---------------------------------------------------------------------------

/// A valid message converts, and every field is carried across unchanged.
#[test]
fn a_valid_message_converts_to_the_domain() {
    let mut source = wire_message();
    source.edited_at = Some(stamp());
    source.thread_id = Some("m_0".to_owned());
    source.reactions = vec![WireReaction {
        emoji: "\u{1f44d}".to_owned(),
        user_ids: vec!["u_1".to_owned()],
    }];
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: 1024,
    }];

    let expected_id = uuid::Uuid::parse_str(CLIENT_MSG_ID).expect("the fixture id is a UUID");
    let message = Message::try_from(source).expect("a valid message converts");

    assert_eq!(message.id, "m_1");
    assert_eq!(message.client_msg_id, expected_id);
    assert_eq!(message.channel_id, "c_1");
    assert_eq!(message.user_id, "u_1");
    assert_eq!(message.content, "hello team");
    assert_eq!(message.timestamp, stamp());
    assert_eq!(message.edited_at, Some(stamp()));
    assert_eq!(message.thread_id.as_deref(), Some("m_0"));
    assert_eq!(
        message.reactions.as_slice(),
        &[Reaction {
            emoji: "\u{1f44d}".to_owned(),
            user_ids: vec!["u_1".to_owned()],
        }]
    );
    assert_eq!(
        message.attachments.as_slice(),
        &[Attachment {
            id: "a_1".to_owned(),
            filename: "shot.png".to_owned(),
            url: "https://example.invalid/shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size: 1024,
        }]
    );
}

/// Reactions and attachments land in the `SmallVec`s `AGENTS.md` §2.3 mandates,
/// with the inline capacity `PLAN.md` §5 specifies.
#[test]
fn reactions_and_attachments_land_in_smallvecs_with_the_specified_capacity() {
    let mut source = wire_message();

    // One reaction, one attachment: the case that must not touch the heap.
    source.reactions = vec![WireReaction {
        emoji: "\u{1f44d}".to_owned(),
        user_ids: vec!["u_1".to_owned()],
    }];
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: 1,
    }];
    let message = Message::try_from(source).expect("a valid message converts");
    assert_eq!(message.reactions.len(), 1);
    assert_eq!(message.attachments.len(), 1);
    assert!(
        !message.reactions.spilled(),
        "one reaction should fit the [Reaction; 2] inline buffer without a heap \
         allocation -- that is the whole point of AGENTS.md 2.3"
    );
    assert!(
        !message.attachments.spilled(),
        "one attachment should fit the [Attachment; 1] inline buffer"
    );

    // Three reactions: past the inline capacity, which is allowed, not an error.
    let mut source = wire_message();
    source.reactions = (0..3)
        .map(|index| WireReaction {
            emoji: format!("e{index}"),
            user_ids: vec!["u_1".to_owned()],
        })
        .collect();
    let message = Message::try_from(source).expect("three reactions is not an error");
    assert_eq!(message.reactions.len(), 3);
    assert!(
        message.reactions.spilled(),
        "three reactions should have spilled past the inline buffer"
    );
}

/// A valid user and channel convert, and `members` becomes an `Arc<[String]>`.
#[test]
fn a_valid_user_and_channel_convert_to_the_domain() {
    let user = User::try_from(wire_user()).expect("a valid user converts");
    assert_eq!(user.id, "u_1");
    assert_eq!(user.username, "ada");
    assert_eq!(user.display_name, "Ada Lovelace");
    assert_eq!(user.avatar_url, None);
    assert_eq!(user.status, UserStatus::Online);

    let channel = Channel::try_from(wire_channel()).expect("a valid channel converts");
    assert_eq!(channel.id, "c_1");
    assert_eq!(channel.name, "general");
    assert_eq!(channel.description.as_deref(), Some("everything"));
    assert!(!channel.is_private);
    assert_eq!(
        channel.members.as_ref(),
        &["u_1".to_owned(), "u_2".to_owned()]
    );
    assert_eq!(channel.last_message_at, Some(stamp()));

    // The Arc sharing AGENTS.md 2.3 asks for: cloning a Channel must not copy
    // the member list.
    let clone = channel.clone();
    assert!(
        std::sync::Arc::ptr_eq(&channel.members, &clone.members),
        "cloning a Channel should share its member list, not copy it"
    );
}

/// An empty member list is valid: a channel created seconds ago has no members
/// but the creator, and rejecting it would break channel creation on the client.
#[test]
fn a_channel_with_no_members_is_valid() {
    let mut source = wire_channel();
    source.members = Vec::new();
    source.last_message_at = None;

    let channel = Channel::try_from(source).expect("an empty member list is not an error");
    assert!(channel.members.is_empty());
    assert_eq!(channel.last_message_at, None);
}

// ---------------------------------------------------------------------------
// Validation: blank ids
// ---------------------------------------------------------------------------

/// A blank required id is rejected, and the error names the field.
///
/// Cases are `(mutation, expected field name)` pairs so a failure says which
/// rule broke rather than only which case index. A `fn` pointer rather than a
/// closure so the array is a plain const-evaluable literal.
#[test]
fn a_blank_required_id_is_rejected() {
    type Case = (fn(&mut WireMessage), &'static str);
    let cases: [Case; 3] = [
        (|m| m.id = String::new(), "message.id"),
        (|m| m.channel_id = String::new(), "message.channel_id"),
        (|m| m.user_id = String::new(), "message.user_id"),
    ];

    for (blank, expected_field) in cases {
        let mut source = wire_message();
        blank(&mut source);
        let error = Message::try_from(source).expect_err("a blank id must not reach the domain");
        assert!(
            error.to_string().contains(expected_field),
            "expected {expected_field} in {error}"
        );
    }
}

/// **Whitespace** is blank too.
///
/// A `"   "` id is worse than an empty one, not better: it passes a naive
/// `is_empty` check, then renders as an invisible author, and the bug it causes
/// is very hard to see.
#[test]
fn a_whitespace_only_id_is_rejected() {
    for blank in ["   ", "\t", "\n", " \t\r\n "] {
        let mut source = wire_message();
        source.channel_id = blank.to_owned();
        let error =
            Message::try_from(source).expect_err("a whitespace-only id must not reach the domain");
        assert!(
            error.to_string().contains("message.channel_id"),
            "expected message.channel_id in {error}"
        );
    }
}

/// A blank id anywhere in the structure is rejected: user, channel, channel
/// members, reactions, attachments, and the frame-level updates.
#[test]
fn a_blank_id_anywhere_in_the_structure_is_rejected() {
    let cases: [(&str, Result<()>, &str); 10] = [
        (
            "user.id",
            User::try_from(WireUser {
                id: String::new(),
                ..wire_user()
            })
            .map(|_| ()),
            "user.id",
        ),
        (
            "user.username",
            User::try_from(WireUser {
                username: "  ".to_owned(),
                ..wire_user()
            })
            .map(|_| ()),
            "user.username",
        ),
        (
            "user.display_name",
            User::try_from(WireUser {
                display_name: String::new(),
                ..wire_user()
            })
            .map(|_| ()),
            "user.display_name",
        ),
        (
            "channel.id",
            Channel::try_from(WireChannel {
                id: String::new(),
                ..wire_channel()
            })
            .map(|_| ()),
            "channel.id",
        ),
        (
            "channel.name",
            Channel::try_from(WireChannel {
                name: "\t".to_owned(),
                ..wire_channel()
            })
            .map(|_| ()),
            "channel.name",
        ),
        (
            "channel.members[]",
            Channel::try_from(WireChannel {
                members: vec!["u_1".to_owned(), " ".to_owned()],
                ..wire_channel()
            })
            .map(|_| ()),
            "channel.members[]",
        ),
        (
            "reaction.emoji",
            Message::try_from(WireMessage {
                reactions: vec![WireReaction {
                    emoji: String::new(),
                    user_ids: vec!["u_1".to_owned()],
                }],
                ..wire_message()
            })
            .map(|_| ()),
            "reaction.emoji",
        ),
        (
            "reaction.user_ids[]",
            Message::try_from(WireMessage {
                reactions: vec![WireReaction {
                    emoji: "\u{1f44d}".to_owned(),
                    user_ids: vec![String::new()],
                }],
                ..wire_message()
            })
            .map(|_| ()),
            "reaction.user_ids[]",
        ),
        (
            "attachment.url",
            Message::try_from(WireMessage {
                attachments: vec![WireAttachment {
                    id: "a_1".to_owned(),
                    filename: "f".to_owned(),
                    url: "  ".to_owned(),
                    mime_type: "image/png".to_owned(),
                    size: 1,
                }],
                ..wire_message()
            })
            .map(|_| ()),
            "attachment.url",
        ),
        (
            "channel.last_message_at (blank is not the rule; timestamp range is)",
            Channel::try_from(wire_channel()).map(|_| ()),
            "",
        ),
    ];

    for (name, result, expected_field) in cases {
        match result {
            Ok(()) => assert!(
                expected_field.is_empty(),
                "{name} was accepted but the case expects {expected_field} to be rejected"
            ),
            Err(error) => {
                if !expected_field.is_empty() {
                    assert!(
                        error.to_string().contains(expected_field),
                        "{name}: expected {expected_field} in {error}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Validation: the client_msg_id must be a UUID
// ---------------------------------------------------------------------------

/// A `client_msg_id` that is not a UUID is rejected.
///
/// This is the rule `AGENTS.md` §7.4 states as "UUID" and that the wire type
/// cannot enforce, because the wire type is a `String`. It is the domain's rule,
/// and this is where it is proved.
#[test]
fn a_client_msg_id_that_is_not_a_uuid_is_rejected() {
    for bad in [
        "",
        "   ",
        "not-a-uuid",
        "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5",   // too short
        "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5dd", // too long
        "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5g",  // not hex
        "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5-",  // trailing hyphen
        "5c8a1b2d3e4f4a5b8c9d0e1f2a3b4c",        // 31 hex digits
        "../../etc/passwd",
        "'; DROP TABLE messages; --",
        "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d\n{\"v\":1}",
    ] {
        let mut source = wire_message();
        source.client_msg_id = bad.to_owned();
        let error = Message::try_from(source)
            .expect_err("a non-UUID client_msg_id must not reach the domain");
        assert!(
            error
                .to_string()
                .contains("message.client_msg_id must be a UUID"),
            "for {bad:?}: {error}"
        );
    }
}

/// A `client_msg_id` in any of `uuid`'s accepted spellings is **accepted**, and
/// the domain stores the canonical one.
///
/// `Uuid::parse_str` accepts hyphenated, unhyphenated, `urn:uuid:`-prefixed and
/// braced forms, because they all denote the same 128 bits. That leniency is
/// correct and this test pins it deliberately: a server using a different UUID
/// formatter is still sending a UUID, and rejecting its message would drop a
/// legitimate delivery for a cosmetic difference.
///
/// The canonicalisation matters downstream. The domain holds a `Uuid`, not text,
/// so the spelling a peer used cannot survive into `state/` -- and the outbound
/// projection always writes the hyphenated form, which is what makes the round
/// trip in `every_dto_round_trips_through_the_boundary_in_both_directions`
/// stable.
#[test]
fn a_client_msg_id_in_any_accepted_uuid_spelling_is_accepted() {
    let canonical = "5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d";
    let accepted = [
        canonical.to_owned(),
        canonical.replace('-', ""),
        format!("urn:uuid:{canonical}"),
        format!("{{{canonical}}}"),
        format!("  {canonical}  "),
    ];

    for spelling in accepted {
        let mut source = wire_message();
        source.client_msg_id = spelling.clone();
        let message = Message::try_from(source)
            .unwrap_or_else(|error| panic!("{spelling:?} is a UUID: {error}"));
        assert_eq!(
            message.client_msg_id.hyphenated().to_string(),
            canonical,
            "the domain should store the canonical spelling of {spelling:?}"
        );
    }
}

/// The error for a bad `client_msg_id` does not echo the value back.
///
/// Unlike a blank-id error, which names the field and nothing else, this one is
/// built from the *only* attacker-controlled id in the system. A `client_msg_id`
/// arrives from a peer and is length-unbounded, so quoting it in an error that
/// can reach a log is exactly what `AGENTS.md` §7.5's "never log message
/// content" is about in spirit. The test pins the behaviour so a later
/// convenience edit cannot quietly reintroduce it.
#[test]
fn a_bad_client_msg_id_error_does_not_echo_the_value() {
    let hostile = "'; DROP TABLE messages; -- and then some more text";
    let mut source = wire_message();
    source.client_msg_id = hostile.to_owned();

    let error = Message::try_from(source).expect_err("must be rejected");
    assert_eq!(
        error.to_string(),
        "protocol error: message.client_msg_id must be a UUID"
    );
    assert!(
        !error.to_string().contains(hostile),
        "the rejected id must not be echoed into the error"
    );
}

/// A `client_msg_id` with surrounding whitespace is accepted, because the rule
/// parses the trimmed value.
///
/// Lenient on the way in and strict on the way out: a peer that adds a trailing
/// space is a cosmetic bug, and rejecting it would drop a legitimate message. The
/// domain stores a real `Uuid`, so the whitespace cannot survive into state
/// anyway.
#[test]
fn a_client_msg_id_with_surrounding_whitespace_is_accepted_after_trimming() {
    let mut source = wire_message();
    source.client_msg_id = format!("  {CLIENT_MSG_ID}\t");
    let message = Message::try_from(source).expect("a padded UUID is still a UUID");
    assert_eq!(
        message.client_msg_id,
        uuid::Uuid::parse_str(CLIENT_MSG_ID).expect("the fixture id is a UUID")
    );
}

/// The `client_msg_id` rule is reusable by the outbound path.
///
/// Phase 4's `state/actions.rs` generates a `client_msg_id` and
/// `network/websocket.rs` puts it on a frame. Both need the same rule, and both
/// should call this rather than each writing a parse inline. It is public and
/// tested for exactly that reason.
#[test]
fn parse_client_msg_id_is_the_reusable_rule() {
    let parsed = parse_client_msg_id("outgoing.client_msg_id", CLIENT_MSG_ID)
        .expect("the fixture id is a UUID");
    assert_eq!(parsed.hyphenated().to_string(), CLIENT_MSG_ID);

    let error = parse_client_msg_id("outgoing.client_msg_id", "nope")
        .expect_err("a non-UUID must be rejected");
    assert_eq!(
        error.to_string(),
        "protocol error: outgoing.client_msg_id must be a UUID"
    );
}

// ---------------------------------------------------------------------------
// Validation: timestamps
// ---------------------------------------------------------------------------

/// A timestamp outside the plausible window is rejected.
///
/// The realistic bug this catches is a unit error. Seconds sent where
/// milliseconds were expected land in 1970, below the floor; microseconds or
/// milliseconds sent where seconds were expected land tens of thousands of years
/// out, above the ceiling. Both directions are tested, because the ceiling and
/// the floor catch different mistakes.
#[test]
fn a_timestamp_outside_the_plausible_window_is_rejected() {
    let mut too_early = wire_message();
    too_early.timestamp = before_floor();
    let error = Message::try_from(too_early).expect_err("a 1999 timestamp must be rejected");
    assert!(
        error.to_string().contains("message.timestamp is outside"),
        "{error}"
    );

    let mut too_late = wire_message();
    too_late.timestamp = after_ceiling();
    let error = Message::try_from(too_late).expect_err("a 2101 timestamp must be rejected");
    assert!(
        error.to_string().contains("message.timestamp is outside"),
        "{error}"
    );
}

/// The window's **boundaries are inclusive**: the first and last plausible
/// instants are accepted.
#[test]
fn the_plausible_window_boundaries_are_inclusive() {
    let floor = DateTime::from_timestamp(TIMESTAMP_FLOOR_UNIX_SECS, 0)
        .expect("the floor is a representable timestamp");
    let ceiling = DateTime::from_timestamp(TIMESTAMP_CEILING_UNIX_SECS, 0)
        .expect("the ceiling is a representable timestamp");

    let mut source = wire_message();
    source.timestamp = floor;
    assert!(
        Message::try_from(source).is_ok(),
        "the floor itself is valid"
    );

    let mut source = wire_message();
    source.timestamp = ceiling;
    assert!(
        Message::try_from(source).is_ok(),
        "the ceiling itself is valid"
    );

    // One second outside each end is not.
    let mut source = wire_message();
    source.timestamp = before_floor();
    assert!(Message::try_from(source).is_err());
    let mut source = wire_message();
    source.timestamp = after_ceiling();
    assert!(Message::try_from(source).is_err());
}

/// A unit error in the "milliseconds or microseconds sent as seconds" direction
/// is caught by the ceiling.
///
/// `1_790_616_000` seconds is 2026-09-27. Sent as *milliseconds* it is a year
/// around 58,000, and sent as *microseconds* it is far beyond even chrono's
/// range. The value below is the milliseconds case, chosen because chrono can
/// represent it -- the test is about the boundary rejecting a representable but
/// absurd time, not about chrono refusing to parse one.
#[test]
fn a_millisecond_unit_error_is_caught_by_the_ceiling() {
    // 100_000_000_000 seconds is 5138-11-16, well past the 2100 ceiling and
    // comfortably inside what chrono can represent.
    let absurd = DateTime::from_timestamp(100_000_000_000, 0)
        .expect("chrono can represent this, which is the point");
    assert!(
        absurd.timestamp() > TIMESTAMP_CEILING_UNIX_SECS,
        "the fixture must actually be past the ceiling"
    );

    let mut source = wire_message();
    source.timestamp = absurd;
    let error = Message::try_from(source).expect_err("a year-5138 timestamp is a unit error");
    assert!(
        error.to_string().contains("message.timestamp is outside"),
        "{error}"
    );
}

/// `edited_at` is validated on the same terms as `timestamp`.
///
/// An edit with an impossible timestamp is as much a data-integrity problem as a
/// new message with one, and "only the primary timestamp is checked" is the kind
/// of half-rule that survives because nothing tests the second field.
#[test]
fn edited_at_is_validated_on_the_same_terms_as_timestamp() {
    let mut source = wire_message();
    source.edited_at = Some(after_ceiling());
    let error = Message::try_from(source).expect_err("a bad edited_at must be rejected");
    assert!(error.to_string().contains("message.edited_at"), "{error}");

    let mut source = wire_message();
    source.edited_at = Some(stamp());
    assert!(Message::try_from(source).is_ok());
}

/// A channel's `last_message_at` is validated too -- it is the resync cursor, so
/// a wrong value produces a wrong resync rather than a wrong-looking row.
#[test]
fn a_channels_last_message_at_is_validated() {
    let mut source = wire_channel();
    source.last_message_at = Some(before_floor());
    let error = Channel::try_from(source).expect_err("a bad cursor must be rejected");
    assert!(
        error.to_string().contains("channel.last_message_at"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Validation: the body
// ---------------------------------------------------------------------------

/// A message with neither content nor an attachment is rejected.
///
/// The only cross-field rule in the boundary. Individually every field is
/// well-formed; the *combination* is not a message.
#[test]
fn a_message_with_no_body_and_no_attachments_is_rejected() {
    for blank in ["", "   ", "\n\t"] {
        let mut source = wire_message();
        source.content = blank.to_owned();
        let error = Message::try_from(source).expect_err("an empty message must be rejected");
        assert!(
            error
                .to_string()
                .contains("message.content must not be blank when the message has no attachments"),
            "for {blank:?}: {error}"
        );
    }
}

/// A file-only message is valid: the attachment is the body.
///
/// The escape hatch that stops the rule above from rejecting a legitimate
/// message. A rule with no escape hatch is a rule that eventually rejects
/// something real.
#[test]
fn a_message_with_only_an_attachment_is_valid() {
    let mut source = wire_message();
    source.content = String::new();
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: 1,
    }];
    assert!(
        Message::try_from(source).is_ok(),
        "a file-only message is a message"
    );
}

/// An attachment's size is bounded, because a `u64` off the network can be 18
/// exabytes.
#[test]
fn an_implausible_attachment_size_is_rejected() {
    let mut source = wire_message();
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: MAX_ATTACHMENT_BYTES + 1,
    }];
    let error = Message::try_from(source).expect_err("an 18-exabyte file is a bug");
    assert!(error.to_string().contains("attachment.size"), "{error}");
    assert!(
        error
            .to_string()
            .contains(&(MAX_ATTACHMENT_BYTES + 1).to_string()),
        "the error should report the received size: {error}"
    );

    // Exactly at the limit is valid -- a ceiling that rejected the limit itself
    // would be off by one and would reject a legitimate file.
    let mut source = wire_message();
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: MAX_ATTACHMENT_BYTES,
    }];
    assert!(Message::try_from(source).is_ok());

    // And `u64::MAX` is caught by the same rule.
    let mut source = wire_message();
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: u64::MAX,
    }];
    assert!(Message::try_from(source).is_err());
}

// ---------------------------------------------------------------------------
// Deliberate non-validation
// ---------------------------------------------------------------------------

/// Over-validation is a failure mode, so the rules that are deliberately
/// **absent** are tested as positively as the rules that are present.
///
/// A test like this one earns its keep the first time somebody adds a "helpful"
/// check: it fails, and the conversation is about the check rather than about a
/// regression.
#[test]
fn the_deliberate_non_validations_hold() {
    // 1. Content length is not bounded. A 500,000-character message is the
    //    client accepting what the server sent; AGENTS.md 5.2 lists a
    //    500-character message as a UI edge case, not a rejection rule.
    let mut source = wire_message();
    source.content = "x".repeat(500_000);
    assert!(
        Message::try_from(source).is_ok(),
        "content length is server policy, not a client rejection rule"
    );

    // 2. `thread_id` is not validated as non-blank or as a UUID, because it is
    //    another message's id and its parent may simply not be loaded. A
    //    `Some("")` is nonsense but it is *state's* problem to resolve, not
    //    this conversion's.
    let mut source = wire_message();
    source.thread_id = Some(String::new());
    assert!(
        Message::try_from(source).is_ok(),
        "an unresolvable thread_id is state/'s question, not the boundary's"
    );

    // 3. A blank avatar URL is treated as "no avatar", not as a malformed URL.
    //    Inventing a placeholder would be a fabricated network request on every
    //    sidebar render.
    let user = User::try_from(WireUser {
        avatar_url: Some(String::new()),
        ..wire_user()
    })
    .expect("a blank avatar url is not an error");
    assert_eq!(user.avatar_url.as_deref(), Some(""));

    // 4. A non-UUID-looking attachment URL is accepted. Pre-signed URLs point at
    //    a different host than the API, so a host allow-list here would reject
    //    legitimate payloads; TLS validation belongs to the fetch.
    let mut source = wire_message();
    source.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "not a url at all".to_owned(),
        mime_type: "totally/unknown".to_owned(),
        size: 1,
    }];
    assert!(
        Message::try_from(source).is_ok(),
        "an unrecognised mime type and an odd url are the client's problem to \
         render, not the boundary's to reject"
    );

    // 5. An emoji is not normalised and not length-bounded: a reaction key must
    //    be byte-identical across clients, so client-side normalisation would
    //    make two clients disagree with the server's grouping key.
    let mut source = wire_message();
    source.reactions = vec![WireReaction {
        emoji: "x".repeat(10_000),
        user_ids: Vec::new(),
    }];
    assert!(Message::try_from(source).is_ok());
}

// ---------------------------------------------------------------------------
// Round trips, both directions
// ---------------------------------------------------------------------------

/// Every DTO survives wire -> domain -> wire unchanged.
///
/// `AGENTS.md` §4.2 requires round trips in **both** directions across the
/// `TryFrom` boundary. This is that test, and the point of the *domain -> wire*
/// half is the part that is easy to get wrong: `client_msg_id` is a `Uuid` in the
/// domain and a `String` on the wire, so the projection has to render it
/// canonically or the second round trip drifts.
#[test]
fn every_dto_round_trips_through_the_boundary_in_both_directions() {
    let mut message = wire_message();
    message.edited_at = Some(stamp());
    message.thread_id = Some("m_0".to_owned());
    message.reactions = vec![WireReaction {
        emoji: "\u{1f44d}".to_owned(),
        user_ids: vec!["u_1".to_owned(), "u_2".to_owned()],
    }];
    message.attachments = vec![WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: 1024,
    }];

    let domain = Message::try_from(message.clone()).expect("converts");
    assert_eq!(
        WireMessage::from(&domain),
        message,
        "message -> domain -> wire should be the identity"
    );

    let user = User::try_from(wire_user()).expect("converts");
    assert_eq!(WireUser::from(&user), wire_user());

    let channel = Channel::try_from(wire_channel()).expect("converts");
    assert_eq!(WireChannel::from(&channel), wire_channel());

    let reaction = Reaction::try_from(WireReaction {
        emoji: "\u{1f44d}".to_owned(),
        user_ids: vec!["u_1".to_owned()],
    })
    .expect("converts");
    assert_eq!(
        WireReaction::from(&reaction),
        WireReaction {
            emoji: "\u{1f44d}".to_owned(),
            user_ids: vec!["u_1".to_owned()],
        }
    );

    let attachment = Attachment::try_from(WireAttachment {
        id: "a_1".to_owned(),
        filename: "shot.png".to_owned(),
        url: "https://example.invalid/shot.png".to_owned(),
        mime_type: "image/png".to_owned(),
        size: 1024,
    })
    .expect("converts");
    assert_eq!(
        WireAttachment::from(&attachment),
        WireAttachment {
            id: "a_1".to_owned(),
            filename: "shot.png".to_owned(),
            url: "https://example.invalid/shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size: 1024,
        }
    );
}

/// A message survives a full trip through the **codec** and the boundary, with
/// the `Uuid` projection intact.
///
/// The two-step version of the test above, and the one that would catch a
/// `Uuid` rendering that survives a direct struct comparison but not a JSON
/// round trip.
#[test]
fn a_message_survives_the_wire_codec_and_the_boundary() {
    let source = wire_message();
    let domain = Message::try_from(source.clone()).expect("converts");

    let json = sh_nexus_wire::ServerEnvelope::new(ServerFrame::MessageNew {
        message: WireMessage::from(&domain),
    })
    .encode()
    .expect("encodes");

    let envelope = ServerEnvelope::decode(&json).expect("decodes");
    let ServerFrame::MessageNew { message } = envelope.frame else {
        panic!("expected a message.new frame");
    };
    assert_eq!(message, source);
    assert_eq!(Message::try_from(message).expect("converts"), domain);
}

/// `UserStatus` and `WireUserStatus` convert both ways, exhaustively.
///
/// A `match` with no wildcard in the *test* as well as in the impl, so a variant
/// added to either enum without the other is a compile error rather than a
/// presence the UI cannot render.
#[test]
fn user_status_converts_both_ways_exhaustively() {
    for (wire, domain) in [
        (WireUserStatus::Online, UserStatus::Online),
        (WireUserStatus::Away, UserStatus::Away),
        (WireUserStatus::Offline, UserStatus::Offline),
    ] {
        assert_eq!(UserStatus::from(wire), domain);
        assert_eq!(WireUserStatus::from(domain), wire);
    }
}

// ---------------------------------------------------------------------------
// Frame -> domain event
// ---------------------------------------------------------------------------

/// Every server frame converts to the domain event it should.
///
/// One case per `PLAN.md` §6 server frame. A frame with no case here is a frame
/// `network/websocket.rs` cannot turn into an event, which means a feature that
/// silently does nothing.
#[test]
fn every_server_frame_converts_to_its_domain_event() {
    let message = wire_message();
    let expected_id = uuid::Uuid::parse_str(CLIENT_MSG_ID).expect("the fixture id is a UUID");

    let cases: [(ServerFrame, DomainEvent); 7] = [
        (
            ServerFrame::MessageAck {
                client_msg_id: CLIENT_MSG_ID.to_owned(),
                message: message.clone(),
            },
            DomainEvent::MessageAcked {
                client_msg_id: expected_id,
                message: Message::try_from(message.clone()).expect("converts"),
            },
        ),
        (
            ServerFrame::MessageNew {
                message: message.clone(),
            },
            DomainEvent::MessageReceived(Message::try_from(message.clone()).expect("converts")),
        ),
        (
            ServerFrame::MessageError {
                client_msg_id: CLIENT_MSG_ID.to_owned(),
                code: "message_too_long".to_owned(),
                detail: "exceeds 4000 characters".to_owned(),
            },
            DomainEvent::MessageSendFailed {
                client_msg_id: expected_id,
                code: "message_too_long".to_owned(),
                detail: "exceeds 4000 characters".to_owned(),
            },
        ),
        (
            ServerFrame::ReactionUpdate {
                message_id: "m_1".to_owned(),
                emoji: "\u{1f44d}".to_owned(),
                user_id: "u_1".to_owned(),
            },
            DomainEvent::ReactionUpdated {
                message_id: "m_1".to_owned(),
                emoji: "\u{1f44d}".to_owned(),
                user_id: "u_1".to_owned(),
            },
        ),
        (
            ServerFrame::TypingUpdate {
                user_id: "u_1".to_owned(),
                channel_id: "c_1".to_owned(),
                active: true,
            },
            DomainEvent::TypingUpdated {
                channel_id: "c_1".to_owned(),
                user_id: "u_1".to_owned(),
                active: true,
            },
        ),
        (
            ServerFrame::PresenceUpdate {
                user_id: "u_1".to_owned(),
                status: WireUserStatus::Away,
            },
            DomainEvent::PresenceUpdated {
                user_id: "u_1".to_owned(),
                status: UserStatus::Away,
            },
        ),
        (
            ServerFrame::Error {
                code: "unsupported_version".to_owned(),
                detail: "peer announced major version 2".to_owned(),
            },
            DomainEvent::ConnectionStateChanged(ConnectionState::Rejected {
                code: "unsupported_version".to_owned(),
                detail: "peer announced major version 2".to_owned(),
            }),
        ),
    ];

    for (frame, expected) in cases {
        let actual = DomainEvent::try_from(frame.clone())
            .unwrap_or_else(|error| panic!("{frame:?} should convert: {error}"));
        assert_eq!(actual, expected, "{frame:?} converted to the wrong event");
    }
}

/// A frame's message payload is validated exactly as a standalone message is.
///
/// The two conversions are separate on purpose. If a `message.new` took a
/// different path from a `WireMessage`, then "no malformed payload reaches
/// state" would be a property of whichever path the author happened to remember,
/// and the malformed frame would be the one nobody tested.
#[test]
fn a_malformed_message_inside_a_frame_is_rejected_by_the_same_rules() {
    let mut malformed = wire_message();
    malformed.channel_id = String::new();

    for frame in [
        ServerFrame::MessageNew {
            message: malformed.clone(),
        },
        ServerFrame::MessageAck {
            client_msg_id: CLIENT_MSG_ID.to_owned(),
            message: malformed.clone(),
        },
    ] {
        let error = DomainEvent::try_from(frame.clone())
            .expect_err("a malformed message must not reach the domain");
        assert!(
            error.to_string().contains("message.channel_id"),
            "{frame:?}: {error}"
        );
    }
}

/// A frame whose `client_msg_id` is not a UUID is rejected.
///
/// `AGENTS.md` §8.1's Optimistic Send Flow needs the ack to find the optimistic
/// row. An ack whose id does not parse cannot be reconciled, and a client that
/// dropped the reconciliation would leave that message in
/// `DeliveryState::Pending` forever -- a message the user believes was sent and
/// that never will be.
#[test]
fn a_frame_with_a_non_uuid_client_msg_id_is_rejected() {
    for frame in [
        ServerFrame::MessageAck {
            client_msg_id: "nope".to_owned(),
            message: wire_message(),
        },
        ServerFrame::MessageError {
            client_msg_id: String::new(),
            code: "x".to_owned(),
            detail: "y".to_owned(),
        },
    ] {
        let error = DomainEvent::try_from(frame.clone())
            .expect_err("a non-UUID client_msg_id must be rejected");
        assert!(
            error.to_string().contains("must be a UUID"),
            "{frame:?}: {error}"
        );
    }
}

/// A blank id inside an advisory frame is rejected too.
///
/// `typing.update` and `reaction.update` are advisory, and `AGENTS.md` §7.5 says
/// an advisory frame may be dropped with a `warn!`. But "dropped" is the
/// *transport's* response to a frame it cannot parse; once a frame has parsed
/// and reached the boundary, a blank id is a semantic rejection, and letting it
/// through would put a blank `channel_id` into the typing set -- where it would
/// never match a channel and would sit there until the inactivity timeout.
#[test]
fn a_blank_id_inside_an_advisory_frame_is_rejected() {
    let cases = [
        (
            ServerFrame::TypingUpdate {
                user_id: "u_1".to_owned(),
                channel_id: String::new(),
                active: true,
            },
            "typing.update.channel_id",
        ),
        (
            ServerFrame::TypingUpdate {
                user_id: "  ".to_owned(),
                channel_id: "c_1".to_owned(),
                active: false,
            },
            "typing.update.user_id",
        ),
        (
            ServerFrame::ReactionUpdate {
                message_id: String::new(),
                emoji: "\u{1f44d}".to_owned(),
                user_id: "u_1".to_owned(),
            },
            "reaction.update.message_id",
        ),
        (
            ServerFrame::PresenceUpdate {
                user_id: String::new(),
                status: WireUserStatus::Online,
            },
            "presence.update.user_id",
        ),
    ];

    for (frame, expected_field) in cases {
        let error = DomainEvent::try_from(frame.clone()).expect_err("a blank id must be rejected");
        assert!(
            error.to_string().contains(expected_field),
            "{frame:?}: expected {expected_field} in {error}"
        );
    }
}

/// An unrecognised error code is passed through, not discarded.
///
/// The compatibility policy in the wire crate's root is that `error` codes are
/// open within a major version, precisely so a client can show a code it does
/// not recognise. This is the client half of that policy: a code from a newer
/// server becomes a `ConnectionState::Rejected` with the code intact, which the
/// user can report. An enum would have made this frame undecodable and the
/// server's explanation undeliverable.
#[test]
fn an_unrecognised_error_code_is_surfaced_rather_than_discarded() {
    let event = DomainEvent::try_from(ServerFrame::Error {
        code: "quota_exceeded_workspace".to_owned(),
        detail: "your workspace has used its 2026 quota".to_owned(),
    })
    .expect("an unknown code is not a rejection");

    let DomainEvent::ConnectionStateChanged(ConnectionState::Rejected { code, detail }) = event
    else {
        panic!("an error frame is a connection rejection");
    };
    assert_eq!(code, "quota_exceeded_workspace");
    assert!(detail.contains("quota"));
}

/// A full JSON string goes all the way to a domain event.
///
/// The end-to-end path a real frame takes: bytes on the socket, codec, boundary,
/// event. Every earlier test starts one step in; this one starts at the wire, so
/// a defect in any of the three steps shows up here rather than only in a unit
/// test of one of them.
#[test]
fn a_json_string_on_the_wire_becomes_a_domain_event() {
    let json = concat!(
        r#"{"v":1,"type":"message.new","message":"#,
        r#"{"id":"m_9","client_msg_id":"5c8a1b2d-3e4f-4a5b-8c9d-0e1f2a3b4c5d","#,
        r#""channel_id":"c_9","user_id":"u_9","content":"over the wire","#,
        r#""timestamp":"2026-09-27T12:00:00Z","edited_at":null,"reactions":[],"#,
        r#""thread_id":null,"attachments":[]}}"#,
    );

    let envelope = ServerEnvelope::decode(json).expect("the frame decodes");
    let event = DomainEvent::try_from(envelope.frame).expect("the frame converts");

    let DomainEvent::MessageReceived(message) = event else {
        panic!("expected a message.received event");
    };
    assert_eq!(message.id, "m_9");
    assert_eq!(message.channel_id, "c_9");
    assert_eq!(message.content, "over the wire");
    assert_eq!(message.timestamp, stamp());
}

// ---------------------------------------------------------------------------
// Wire errors -> the client's error type
// ---------------------------------------------------------------------------

/// Every `WireError` variant reduces to a `Protocol` error that keeps its kind.
///
/// `AGENTS.md` §3.3 requires one global error type; `sh_nexus_wire` cannot use it,
/// so this reduction is the seam. Each variant must stay distinguishable, because
/// they mean different things to a reconnect loop: a version rejection is
/// terminal and a malformed frame is not.
#[test]
fn every_wire_error_reduces_to_a_distinguishable_protocol_error() {
    let cases = [
        (
            WireError::UnsupportedVersion(
                sh_nexus_wire::negotiate(2).expect_err("v2 is not supported by this build"),
            ),
            vec!["unsupported protocol version", "2"],
        ),
        (
            WireError::MalformedPayload(
                serde_json::from_str::<u8>("{").expect_err("truncated JSON must not parse"),
            ),
            vec!["malformed frame payload"],
        ),
        (
            WireError::InvalidUtf8("invalid utf-8 sequence".to_owned()),
            vec!["not valid UTF-8", "invalid utf-8 sequence"],
        ),
        (
            WireError::UnknownFrameType {
                name: "message.delete".to_owned(),
                expected_direction: sh_nexus_wire::Direction::Server,
            },
            vec!["unknown server frame type", "message.delete"],
        ),
        (
            WireError::WrongDirection {
                kind: sh_nexus_wire::FrameKind::MessageSend,
                kind_direction: sh_nexus_wire::Direction::Client,
                expected_direction: sh_nexus_wire::Direction::Server,
            },
            vec!["message.send", "client frame", "server frame"],
        ),
    ];

    for (wire_error, expected_fragments) in cases {
        let reduced = reduce_wire_error(&wire_error);
        let text = reduced.to_string();
        assert!(
            text.starts_with("protocol error: "),
            "every wire error reduces to a protocol error, got {text}"
        );
        for fragment in expected_fragments {
            assert!(
                text.contains(fragment),
                "expected {fragment:?} in {text} (from {wire_error:?})"
            );
        }
    }
}

/// A version rejection stays recognisable as fatal after the reduction.
///
/// The reduction turns a typed error into a `String`, which is exactly the kind
/// of step that loses the one bit that matters. `network/reconnect.rs` will ask
/// "is this fatal?" and the answer has to survive, so the marker is pinned here.
#[test]
fn a_version_rejection_stays_recognisable_after_the_reduction() {
    let rejection = WireError::UnsupportedVersion(
        sh_nexus_wire::negotiate(9).expect_err("v9 is not supported"),
    );
    assert!(rejection.is_fatal(), "the wire crate says it is fatal");

    let reduced = reduce_wire_error(&rejection);
    assert!(
        matches!(reduced, ShNexusError::Protocol(ref text) if text.starts_with("unsupported protocol version")),
        "the fatal marker must survive the reduction, got {reduced:?}"
    );
}

/// The reduction is a named function, not a `#[from]` impl, on purpose.
///
/// A test cannot assert the *absence* of a trait impl, so this documents the
/// decision where the decision is made. `AGENTS.md` §3.3's rule about a `#[from]`
/// leaking a dependency's error type into the domain is the reason: an automatic
/// conversion would put `WireError` in scope through `?` in every module,
/// including `core/`, which §3.2 declares free of protocol concerns.
#[test]
fn the_reduction_is_reachable_without_a_from_impl() {
    // A caller that wants the conversion writes it out. If a `From` impl were
    // added later, this still compiles -- which is the point: the named
    // function is the documented path, and adding `From` would be a change
    // someone could make silently.
    let error = ServerEnvelope::decode(r#"{"v":99,"type":"error","code":"x","detail":"y"}"#)
        .expect_err("v99 is not supported");
    let reduced: ShNexusError = reduce_wire_error(&error);
    assert!(matches!(reduced, ShNexusError::Protocol(_)));
}

// ---------------------------------------------------------------------------
// Types the boundary produces
// ---------------------------------------------------------------------------

/// `DeliveryState` and `unread_count` are client state and are **not** on the
/// domain types.
///
/// `PLAN.md` §5 is explicit: they live in `state/app_state.rs`, because §3.2
/// puts mutations in `state/actions.rs`. This test cannot assert the absence of a
/// field, so what it does is assert the positive property: two independently
/// constructed `Message`s from the same wire message are equal, which they could
/// not be if a per-client delivery flag were part of the type.
#[test]
fn a_message_carries_no_per_client_delivery_state() {
    let a = Message::try_from(wire_message()).expect("converts");
    let b = Message::try_from(wire_message()).expect("converts");
    assert_eq!(a, b, "the same wire message is the same domain message");

    // And a Channel carries no unread count, for the same reason: two clients
    // showing the same channel have different unread counts, and both are right.
    let channel = Channel::try_from(wire_channel()).expect("converts");
    let members: Arc<[String]> = channel.members.clone();
    assert_eq!(
        members.len(),
        2,
        "members are the channel's, and are shared"
    );
}

/// Every rejection is a `Protocol` error, and never a panic.
///
/// `AGENTS.md` §2.1 forbids panics in user-facing code, and a boundary that
/// panics on a malformed payload is a remotely-triggerable crash. The
/// rejections are the return values; a panic would have failed this test before
/// it could run at all, which is the honest way to say "this is a crash, not an
/// error".
#[test]
fn rejections_are_values_and_never_panics() {
    let hostile: Vec<WireMessage> = vec![
        WireMessage {
            id: String::new(),
            client_msg_id: String::new(),
            channel_id: String::new(),
            user_id: String::new(),
            content: String::new(),
            timestamp: DateTime::from_timestamp(i64::MIN, 0).unwrap_or_default(),
            edited_at: None,
            reactions: Vec::new(),
            thread_id: None,
            attachments: Vec::new(),
        },
        WireMessage {
            id: "\u{0}\u{1}\u{2}".to_owned(),
            client_msg_id: "\u{0}".to_owned(),
            channel_id: "\u{feff}".to_owned(),
            user_id: "\u{202e}".to_owned(),
            content: "\u{0}".to_owned(),
            timestamp: DateTime::from_timestamp(i64::MAX, 0).unwrap_or_default(),
            edited_at: None,
            reactions: Vec::new(),
            thread_id: None,
            attachments: Vec::new(),
        },
    ];

    for message in hostile {
        // The only assertion that matters: this returns rather than unwinding.
        // If it panicked the test binary would abort, and the harness would say
        // so -- which is more informative than any assertion here could be.
        let result = Message::try_from(message);
        assert!(result.is_err(), "a hostile message must be rejected");
        assert!(matches!(result, Err(ShNexusError::Protocol(_))));
    }
}
