//! Version negotiation: the accept path, the reject path, and the rejection
//! frame.
//!
//! `AGENTS.md` §7.4 requires every WebSocket payload to be versioned and an
//! **unknown major version to be rejected explicitly**. These tests are where
//! that requirement stops being a sentence in a document and becomes behaviour.
//!
//! The rejection path is the part worth writing carefully, because there are
//! three ways to get it wrong and all three compile:
//!
//! 1. **Check the version last.** Parse the body, then look at `v`. The damage
//!    is done: a body has been interpreted under a dialect this build does not
//!    speak. `version_is_checked_before_the_body_is_read` below pins the order.
//! 2. **Reject silently.** Close the socket with no explanation. Indistinguishable
//!    from a network fault, so the user reconnects forever against a server that
//!    will refuse identically. `rejection_frame_names_both_versions` pins the
//!    explanation.
//! 3. **Echo the peer's version in the rejection.** Structurally wrong, and
//!    wrong precisely for the audience that needs it: a client too old to speak
//!    the announced version is the client least able to parse a frame stamped
//!    with it. `rejection_frame_advertises_our_version_not_theirs` pins it.

use rstest::rstest;
use sh_nexus_wire::frame::{ClientEnvelope, ServerEnvelope, ServerFrame};
use sh_nexus_wire::version::{UnsupportedVersion, PROTOCOL_VERSION, SUPPORTED_MAJOR_VERSIONS};
use sh_nexus_wire::{negotiate, ProtocolVersion, WireError, UNSUPPORTED_VERSION_CODE};

/// Every version this build can speak is accepted, and nothing else is.
#[test]
fn negotiate_accepts_every_supported_version() {
    for version in SUPPORTED_MAJOR_VERSIONS {
        let accepted = negotiate(*version)
            .unwrap_or_else(|error| panic!("{version} is in the supported set: {error}"));
        assert_eq!(accepted.get(), *version);
    }
}

/// A version outside the supported set is rejected, with the peer's version and
/// this build's set both reported.
#[test]
fn negotiate_rejects_every_unsupported_version() {
    let rejected = [0, 2, 3, u16::from(u8::MAX), u16::MAX];
    for version in rejected {
        let error = negotiate(version)
            .err()
            .unwrap_or_else(|| panic!("{version} must not be accepted by this build"));
        assert_eq!(error.received, version);
        assert_eq!(error.supported, SUPPORTED_MAJOR_VERSIONS);
    }
}

/// A frame whose `v` this build does not speak is refused, and the refusal is
/// fatal -- the one error a caller must act on rather than log.
#[test]
fn a_frame_with_an_unsupported_version_is_refused_and_is_fatal() {
    let json = r#"{"v":2,"type":"error","code":"x","detail":"y"}"#;

    let error = ServerEnvelope::decode(json).expect_err("v2 must be refused");
    assert!(
        matches!(error, WireError::UnsupportedVersion(_)),
        "expected UnsupportedVersion, got {error:?}"
    );
    assert!(
        error.is_fatal(),
        "a version mismatch is not a transient fault"
    );

    // And the same for the client direction, which is the one that matters:
    // a client too old to understand a newer server is the case that has to be
    // detected rather than misread.
    let error = ClientEnvelope::decode(
        r#"{"v":2,"type":"typing.start","client_msg_id":"x","channel_id":"c"}"#,
    )
    .expect_err("v2 must be refused");
    assert!(matches!(error, WireError::UnsupportedVersion(_)));
}

/// The version is checked **before** the body is deserialized.
///
/// Both payloads below are internally inconsistent: an unsupported `v` *and* a
/// body that cannot decode. If the version were checked second, both would come
/// back as `MalformedPayload` and the connection would carry on with a frame
/// this build cannot interpret. Returning `UnsupportedVersion` for both is the
/// observable proof that the version gate comes first.
#[test]
fn version_is_checked_before_the_body_is_read() {
    // Valid body, unsupported version.
    let error = ServerEnvelope::decode(r#"{"v":9999,"type":"message.new","message":{}}"#)
        .expect_err("an unsupported version must be refused");
    assert!(
        matches!(error, WireError::UnsupportedVersion(_)),
        "expected UnsupportedVersion, got {error:?}"
    );

    // Invalid body *and* unsupported version: the version still wins, because it
    // is the coarser boundary and the body cannot be judged without one.
    let error = ServerEnvelope::decode(r#"{"v":9999,"type":"error","code":42}"#)
        .expect_err("an unsupported version must be refused");
    assert!(
        matches!(error, WireError::UnsupportedVersion(_)),
        "the version must be judged before the body, got {error:?}"
    );

    // And with a *supported* version the same broken body is a body error,
    // which is what makes the previous two assertions meaningful.
    let error = ServerEnvelope::decode(r#"{"v":1,"type":"error","code":42}"#)
        .expect_err("a broken body must be refused");
    assert!(
        matches!(error, WireError::MalformedPayload(_)),
        "with a negotiated version, the body error should surface, got {error:?}"
    );
}

/// An unknown `type` is judged **after** the version, for the same reason.
///
/// A frame with an unknown type *and* an unsupported version is a version
/// problem, not a type problem: telling the peer its frame type is unrecognised
/// would be advice about a protocol the peer may not even be speaking.
#[test]
fn the_frame_type_is_judged_after_the_version() {
    let error = ServerEnvelope::decode(r#"{"v":42,"type":"no.such.frame"}"#)
        .expect_err("an unsupported version must be refused");
    assert!(
        matches!(error, WireError::UnsupportedVersion(_)),
        "expected UnsupportedVersion, got {error:?}"
    );
}

/// The rejection frame is an `error` frame carrying the documented code.
#[test]
fn rejection_frame_is_an_error_frame_with_the_documented_code() {
    let reason = negotiate(2).expect_err("v2 is not supported by this build");
    let envelope = ServerEnvelope::version_rejection(&reason);

    assert_eq!(envelope.v, PROTOCOL_VERSION);
    assert!(matches!(envelope.frame, ServerFrame::Error { .. }));

    let json = envelope.encode().expect("the rejection frame encodes");
    // The first segment is raw because it is all quotes and braces; the rest are
    // ordinary literals because a raw string cannot end with a bare `"` without
    // the terminator swallowing it, which is exactly the bug this assertion was
    // written to catch.
    assert_eq!(
        json,
        concat!(
            r#"{"v":1,"type":"error","code":"unsupported_version","detail":"#,
            "\"peer announced protocol major version 2; this client speaks [1]. ",
            "The two versions are incompatible: update the client or the server. ",
            "Reconnecting will not resolve this.\"}",
        )
    );
}

/// The rejection names both the peer's version and the set this build speaks.
#[test]
fn rejection_frame_names_both_versions() {
    for announced in [0, 2, 77, u16::MAX] {
        let reason = negotiate(announced).expect_err("not supported");
        let detail = reason.detail();
        assert!(
            detail.contains(&announced.to_string()),
            "the detail must name the peer's version {announced}: {detail}"
        );
        assert!(
            detail.contains(&format!("{SUPPORTED_MAJOR_VERSIONS:?}")),
            "the detail must name the supported set: {detail}"
        );
    }
}

/// The rejection advertises **this build's** version, never the peer's.
///
/// A rejection stamped with the rejected version is addressed to a peer that has
/// already been shown not to understand that version. The `v` field is the one
/// thing a stale client can always parse, so it has to carry the version the
/// *speaker* speaks.
#[test]
fn rejection_frame_advertises_our_version_not_theirs() {
    let reason = negotiate(7).expect_err("v7 is not supported by this build");
    let envelope = ServerEnvelope::version_rejection(&reason);

    assert_eq!(
        envelope.v, PROTOCOL_VERSION,
        "the rejection must be framed in a version this build speaks"
    );
    assert_ne!(envelope.v, reason.received);

    // The peer's version appears in the detail as text, where a stale client can
    // still read it.
    let ServerFrame::Error { detail, .. } = &envelope.frame else {
        panic!("the rejection must be an error frame");
    };
    assert!(
        detail.contains('7'),
        "the detail should mention the peer: {detail}"
    );
}

/// A rejected peer can be told why, and the reason survives the codec.
///
/// The rejection frame is only useful if the very client that could not be
/// understood can still parse it -- so the frame is decoded here with the
/// ordinary server-direction decoder, as an older peer would.
#[test]
fn the_rejection_frame_round_trips_through_the_decoder() {
    let reason = negotiate(PROTOCOL_VERSION + 1).expect_err("not supported");
    let json = ServerEnvelope::version_rejection(&reason)
        .encode()
        .expect("encodes");
    let decoded = ServerEnvelope::decode(&json).expect("the rejection must be decodable");

    let ServerFrame::Error { code, detail } = decoded.frame else {
        panic!("the rejection must be an error frame");
    };
    assert_eq!(code, UNSUPPORTED_VERSION_CODE);
    assert_eq!(detail, reason.detail());
}

/// The code constant is the one `PLAN.md` §6 documents.
#[test]
fn the_rejection_code_matches_the_documented_spelling() {
    assert_eq!(UNSUPPORTED_VERSION_CODE, "unsupported_version");
}

/// The supported set is exactly what a multi-version build would widen.
///
/// `SUPPORTED_MAJOR_VERSIONS` is a slice precisely so a build that can speak two
/// majors changes one list. Asserting its current shape keeps that the only
/// change needed -- if this build ever hardcodes a single version in a signature
/// or an `if`, this is the test that says so.
#[test]
fn the_supported_version_set_is_the_single_documented_constant() {
    assert_eq!(SUPPORTED_MAJOR_VERSIONS, [PROTOCOL_VERSION]);
    assert_eq!(PROTOCOL_VERSION, 1, "PLAN.md section 6 documents v: 1");
}

/// An [`UnsupportedVersion`] is a value, not just a message.
///
/// The client reduces it into `ShNexusError`, and a reduced error that has lost
/// the peer's version is a bug report nobody can act on. So the fields have to
/// survive, and `Display` has to read as a sentence.
#[test]
fn unsupported_version_reports_both_versions_in_its_display() {
    let error = UnsupportedVersion {
        received: 5,
        supported: SUPPORTED_MAJOR_VERSIONS,
    };
    let text = error.to_string();
    assert!(text.contains('5'), "{text}");
    assert!(text.contains("unsupported protocol version"), "{text}");
    assert_eq!(error.received, 5);
    assert_eq!(error.supported, SUPPORTED_MAJOR_VERSIONS);
}

/// A [`ProtocolVersion`] displays as the **bare major number**.
///
/// `PLAN.md` §6 defines `v` as a bare JSON integer -- not `"1"`, not `"v1"`, not
/// a dotted `1.0` -- and this impl is the text form of that same field. It is
/// what goes into a log line or a `detail` string, so the two questions worth
/// asking are whether it is the bare integer, and whether it agrees with
/// [`ProtocolVersion::get`]. A `Display` that rendered `"v1"` would be a
/// display bug that no decoder would ever catch.
///
/// Parameterized per `AGENTS.md` §4.3 and ADR-008, one case per major this
/// build speaks, with [`the_display_cases_cover_every_supported_version`] as the
/// guard that makes a new major a test failure rather than a silent gap.
#[rstest]
#[case(PROTOCOL_VERSION)]
fn a_protocol_version_displays_as_its_bare_major_number(#[case] major: u16) {
    let version = negotiate(major).unwrap_or_else(|error| {
        panic!("{major} is in SUPPORTED_MAJOR_VERSIONS, so it must negotiate: {error}")
    });

    assert_eq!(
        version.to_string(),
        major.to_string(),
        "the rendering must be the bare integer `PLAN.md` section 6 puts in `v`"
    );
    assert_eq!(
        version.to_string(),
        version.get().to_string(),
        "`Display` and `get` are two spellings of one fact and must agree"
    );
}

/// The `Display` case list above covers [`SUPPORTED_MAJOR_VERSIONS`] exactly.
///
/// Redundant with the `assert_eq!` in
/// `the_supported_version_set_is_the_single_documented_constant`, and kept
/// because it is the check that fails when a **second** major is added: the set
/// widens, the guard notices, and the `#[case]` has to be added. Without it, a
/// multi-version build would render one of its majors untested and nothing would
/// say so.
#[test]
fn the_display_cases_cover_every_supported_version() {
    assert_eq!(SUPPORTED_MAJOR_VERSIONS, [PROTOCOL_VERSION]);
    assert_eq!(
        ProtocolVersion::CURRENT.to_string(),
        PROTOCOL_VERSION.to_string()
    );
}
