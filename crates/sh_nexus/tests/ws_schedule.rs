//! The transport's schedule, tested with no clock and no socket.
//!
//! # Why these tests exist as a separate file from the socket ones
//!
//! `AGENTS.md` §4.3 forbids `sleep()` in a unit test, and the two things this
//! milestone most needs to be *sure* about — the backoff schedule and the
//! keepalive threshold — are both a function of an integer and a `Duration`. A
//! test that reached them through a real socket would have to wait 30 seconds to
//! observe one ping and a minute to observe an abandonment, and would be a slower
//! and weaker proof than calling the function.
//!
//! So the *policy* is tested here, exhaustively, in microseconds; and the socket
//! suite in `ws_transport.rs` tests that the policy is *wired up*. Both halves are
//! needed: a correct function nothing calls is not a reconnection schedule, and a
//! correct loop calling an unproven function is a schedule whose arithmetic nobody
//! has checked.
//!
//! # What is asserted and how
//!
//! | Property | How |
//! |---|---|
//! | The schedule is 1s → 2s → 4s … capped at 60s | exact equality per attempt |
//! | Jitter stays inside its documented window | the window is a function, so the test compares against it rather than re-deriving it |
//! | The cap is a ceiling, not a floor | every jittered value, at every attempt, is `<= BACKOFF_MAX` |
//! | Out-of-range jitter is clamped | values far outside are answered, not rejected |
//! | `draw_jitter` never leaves the range | many draws, all checked |
//! | The keepalive pings then abandons, at the documented instants | every second from 0 to 90 |
//! | Every frame carries a version and a parseable identity | encode, decode, re-parse |

use std::time::Duration;

use rstest::rstest;
use sh_nexus::core::models::events::ConnectionState;
use sh_nexus::network::mapping::TIMESTAMP_FLOOR_UNIX_SECS;
use sh_nexus::network::ws::{
    backoff_delay, backoff_window, classify_frame_error, decode_server_frame, draw_jitter,
    epoch_cursor, keepalive_action, nominal_backoff, protocol_version, FrameFailure,
    KeepaliveAction, TransportConfig, BACKOFF_INITIAL, BACKOFF_MAX, CONNECT_TIMEOUT,
    JITTER_RANGE_PERMILLE, KEEPALIVE_ABANDON_FACTOR, KEEPALIVE_INTERVAL, MAX_OUTBOUND_FRAMES,
};
use sh_nexus_wire::frame::{ClientEnvelope, ClientFrame};
use sh_nexus_wire::version::{PROTOCOL_VERSION, UNSUPPORTED_VERSION_CODE};
use uuid::Uuid;

/// The largest attempt this file asks about.
///
/// `u32::MAX` is asserted separately, in `the_schedule_is_flat_past_the_point…`;
/// this is the largest *ordinary* attempt, well past the point where the cap has
/// been reached seven times over.
const FAR_ENOUGH: u32 = 64;

/// The exact schedule `AGENTS.md` §4.2 names.
#[test]
fn the_schedule_doubles_from_one_second_and_is_capped_at_sixty() {
    let expected = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(8),
        Duration::from_secs(16),
        Duration::from_secs(32),
        Duration::from_secs(60),
        Duration::from_secs(60),
    ];
    for (index, want) in expected.iter().enumerate() {
        let attempt = index as u32 + 1;
        assert_eq!(
            nominal_backoff(attempt),
            *want,
            "attempt {attempt} should wait {want:?}"
        );
    }
    assert_eq!(BACKOFF_INITIAL, Duration::from_secs(1));
    assert_eq!(BACKOFF_MAX, Duration::from_secs(60));
}

/// Attempt zero is treated as the first retry rather than refused.
///
/// The loop that calls these functions holds a counter that starts at zero, and a
/// schedule whose first entry is an error would push a `Result` into a place with
/// no way to report one.
#[test]
fn a_zero_attempt_is_the_first_retry_rather_than_an_error() {
    assert_eq!(nominal_backoff(0), nominal_backoff(1));
    assert_eq!(backoff_delay(0, 0), BACKOFF_INITIAL);
}

/// Past the seventh attempt the value is the cap, and stays there forever.
#[test]
fn the_schedule_is_flat_past_the_point_where_the_cap_bites() {
    for attempt in 7..=FAR_ENOUGH {
        assert_eq!(
            nominal_backoff(attempt),
            BACKOFF_MAX,
            "attempt {attempt} is past the cap and should be flat"
        );
    }
    // Values that would overflow a shift if the exponent were not bounded. This is
    // the assertion that the exponent is clamped, rather than merely large.
    for attempt in [u32::MAX, u32::MAX - 1, 1_000_000] {
        assert_eq!(nominal_backoff(attempt), BACKOFF_MAX);
    }
}

/// Every jittered delay, at every attempt, lies inside the documented window.
///
/// **Compared against [`backoff_window`] rather than against arithmetic written
/// here**, which is the whole reason that function exists: a test that re-derived
/// the bound would agree with the implementation by construction and could not
/// fail. The window is the promise, and this checks that the promise holds.
#[test]
fn jitter_stays_inside_the_window_it_documents_for_every_attempt() {
    for attempt in 0..=FAR_ENOUGH {
        let window = backoff_window(attempt);
        for jitter in -JITTER_RANGE_PERMILLE..=JITTER_RANGE_PERMILLE {
            let delay = backoff_delay(attempt, jitter);
            assert!(
                window.contains(&delay),
                "attempt {attempt} with {jitter} permille produced {delay:?}, outside {window:?}"
            );
        }
    }
}

/// The cap is a ceiling on the *jittered* value, not only on the nominal one.
///
/// This is what stops a client exceeding `AGENTS.md` §7.4's ceiling exactly when it
/// has reached the cap, which is when it is under the most pressure to retry
/// slowly.
#[test]
fn jitter_never_pushes_a_delay_past_the_cap() {
    for attempt in 0..=FAR_ENOUGH {
        for jitter in [-JITTER_RANGE_PERMILLE, 0, JITTER_RANGE_PERMILLE] {
            assert!(
                backoff_delay(attempt, jitter) <= BACKOFF_MAX,
                "attempt {attempt} with {jitter} permille exceeded the cap"
            );
        }
    }
}

/// Jitter may *shorten* a capped delay, and the window says so.
#[test]
fn jitter_shortens_a_capped_delay_without_leaving_the_window() {
    let window = backoff_window(FAR_ENOUGH);
    assert_eq!(*window.end(), BACKOFF_MAX, "the window's top is the cap");
    assert!(
        window.start() < &BACKOFF_MAX,
        "the window's bottom is below the cap, so jitter can shorten"
    );
    assert_eq!(
        backoff_delay(FAR_ENOUGH, -JITTER_RANGE_PERMILLE),
        *window.start()
    );
}

/// Out-of-range jitter is clamped into the range, not honoured.
///
/// A clamped delay is still a bounded delay. The alternative — a `Result` here —
/// would put an error path on a function whose only job is arithmetic, and the only
/// caller that could pass an out-of-range value is this module's own jitter draw,
/// which is written to stay in range.
#[rstest]
#[case(9_000, JITTER_RANGE_PERMILLE)]
#[case(-9_000, -JITTER_RANGE_PERMILLE)]
#[case(i32::MAX, JITTER_RANGE_PERMILLE)]
#[case(i32::MIN, -JITTER_RANGE_PERMILLE)]
fn out_of_range_jitter_is_clamped_rather_than_applied(#[case] given: i32, #[case] clamped: i32) {
    assert_eq!(backoff_delay(1, given), backoff_delay(1, clamped));
}

/// The window's endpoints are the ones the documentation prints, and a zero jitter
/// reproduces the nominal schedule exactly.
#[test]
fn the_window_matches_the_extremes_it_documents() {
    assert_eq!(
        *backoff_window(1).start(),
        Duration::from_millis(800),
        "a one-second nominal less 20%"
    );
    assert_eq!(*backoff_window(1).end(), Duration::from_millis(1_200));
    assert_eq!(backoff_delay(3, 0), nominal_backoff(3));
}

/// The jitter draw never leaves its range, and is not stuck at one value.
///
/// Not a randomness test. One asserting two draws differ would flake, and one
/// asserting a distribution would be testing `uuid`'s generator rather than this
/// mapping. What is asserted is the bound, which is the only property
/// [`backoff_delay`] relies on — plus that the mapping reaches all three regions,
/// because a draw that could only ever produce zero would satisfy the bound while
/// defeating the point of jittering.
#[test]
fn the_jitter_draw_always_lands_inside_the_documented_range() {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..512 {
        let drawn = draw_jitter();
        assert!(
            (-JITTER_RANGE_PERMILLE..=JITTER_RANGE_PERMILLE).contains(&drawn),
            "{drawn} is outside the range"
        );
        seen.insert(drawn);
    }
    assert!(
        seen.contains(&-JITTER_RANGE_PERMILLE) || seen.iter().any(|value| *value < 0),
        "the draw never shortened a delay, so jitter is not doing its job"
    );
    assert!(
        seen.iter().any(|value| *value > 0),
        "the draw never lengthened a delay"
    );
    // 512 draws into 401 buckets: about 283 distinct values are expected. A bound
    // of twenty is far below that and far above anything a mapping which returned,
    // say, only `0` and `-200` could reach — so this catches a mapping collapsed to
    // a couple of values without being a distribution test, which would be
    // `uuid`'s to pass, not this module's.
    assert!(
        seen.len() >= 20,
        "512 draws produced only {} distinct values",
        seen.len()
    );
}

/// The keepalive pings at one interval and abandons at two, at the documented
/// instants and nowhere else.
///
/// Walked one second at a time to 90, three times past the abandon point. A coarser
/// walk would miss an off-by-one at the boundary, and the boundary is the only thing
/// this function has.
#[test]
fn the_keepalive_pings_then_abandons_exactly_where_the_documentation_says() {
    assert_eq!(KEEPALIVE_INTERVAL, Duration::from_secs(30));
    assert_eq!(KEEPALIVE_ABANDON_FACTOR, 2);

    for second in 0..=90_u64 {
        let silent = Duration::from_secs(second);
        let expected = if second < 30 {
            KeepaliveAction::Idle
        } else if second < 60 {
            KeepaliveAction::Ping
        } else {
            KeepaliveAction::Abandon
        };
        assert_eq!(
            keepalive_action(silent, KEEPALIVE_INTERVAL),
            expected,
            "{silent:?} of silence should be {expected:?}"
        );
    }
}

/// The threshold scales with the interval, so a shortened interval moves both ends.
///
/// The property the production constants rely on and that a test of the constants
/// alone cannot establish: the abandon point is the interval times the factor, not
/// a second hard-coded number that happened to agree with 30 seconds.
#[rstest]
#[case(20_u64)]
#[case(250_u64)]
#[case(5_000_u64)]
fn the_abandon_point_is_the_interval_times_the_factor(#[case] millis: u64) {
    let interval = Duration::from_millis(millis);
    let abandon = interval * KEEPALIVE_ABANDON_FACTOR;

    assert_eq!(
        keepalive_action(interval - Duration::from_millis(1), interval),
        KeepaliveAction::Idle
    );
    assert_eq!(keepalive_action(interval, interval), KeepaliveAction::Ping);
    assert_eq!(
        keepalive_action(abandon - Duration::from_millis(1), interval),
        KeepaliveAction::Ping
    );
    assert_eq!(
        keepalive_action(abandon, interval),
        KeepaliveAction::Abandon
    );
    // Silence beyond the abandon point stays abandoned rather than oscillating.
    assert_eq!(
        keepalive_action(abandon * 100, interval),
        KeepaliveAction::Abandon
    );
}

/// The version every frame this transport writes carries is the one it advertises.
#[test]
fn every_written_frame_is_versioned_with_the_builds_own_major() {
    assert_eq!(protocol_version(), PROTOCOL_VERSION);
    assert_eq!(PROTOCOL_VERSION, 1, "PLAN.md section 6 fixes this at 1");
}

/// The `client_msg_id` the transport writes is a UUID the boundary can parse back.
///
/// `AGENTS.md` §7.4 requires a client-generated UUID on every frame, and the domain
/// half of that rule is `mapping::parse_client_msg_id`, which rejects anything that
/// does not parse. **Testing the round trip rather than the rendering is the
/// point:** a hyphenated `Uuid` and a `String` are the same thing only if the
/// renderer and the parser agree, and that agreement is a fact about code in two
/// crates.
#[test]
fn the_identity_written_on_a_frame_parses_back_as_a_uuid() {
    let id = Uuid::new_v4();
    let envelope = ClientEnvelope::new(
        id.hyphenated().to_string(),
        ClientFrame::TypingStop {
            channel_id: "general".to_owned(),
        },
    );
    let decoded = ClientEnvelope::decode(&envelope.encode().expect("these types encode"))
        .expect("the frame the transport writes decodes");
    assert_eq!(
        Uuid::parse_str(&decoded.client_msg_id).expect("the id is a UUID"),
        id,
        "the identity on the wire must be the identity the caller minted"
    );
}

/// A frame without an identity does not decode, so the transport cannot omit one.
///
/// Structural rather than conventional: `ClientEnvelope` requires the field, which is
/// why `AGENTS.md` §7.4's rule is enforced by the wire crate's type rather than by
/// a reviewer's eye.
#[test]
fn a_frame_without_an_identity_does_not_decode() {
    let json = format!(r#"{{"v":{PROTOCOL_VERSION},"type":"typing.stop","channel_id":"general"}}"#);
    assert!(
        ClientEnvelope::decode(&json).is_err(),
        "an envelope with no client_msg_id must fail to decode, or the rule is a \
         convention rather than a type"
    );
}

/// The default configuration is the constitution's numbers, and the builders move
/// only what they name.
#[test]
fn the_default_configuration_is_the_documented_one() {
    let config = TransportConfig::new("ws://127.0.0.1:8484/ws");
    assert_eq!(config.connect_timeout(), CONNECT_TIMEOUT);
    assert_eq!(config.connect_timeout(), Duration::from_secs(5));
    assert_eq!(config.keepalive_interval(), KEEPALIVE_INTERVAL);
    assert_eq!(config.outbound_capacity(), MAX_OUTBOUND_FRAMES);
    assert_eq!(config.max_attempts(), None);

    let shortened = config
        .clone()
        .with_connect_timeout(Duration::from_millis(50))
        .with_keepalive_interval(Duration::from_millis(10))
        .with_max_attempts(3)
        .with_outbound_capacity(4);
    assert_eq!(shortened.connect_timeout(), Duration::from_millis(50));
    assert_eq!(shortened.keepalive_interval(), Duration::from_millis(10));
    assert_eq!(shortened.max_attempts(), Some(3));
    assert_eq!(shortened.outbound_capacity(), 4);
    // And the original is untouched, which is what makes a builder a builder rather
    // than a setter.
    assert_eq!(config.max_attempts(), None);
    assert_eq!(config.url(), "ws://127.0.0.1:8484/ws");
}

/// The cursor for "nothing yet" is the boundary's own floor.
#[test]
fn the_empty_cursor_is_the_instant_the_boundary_stops_accepting() {
    assert_eq!(epoch_cursor().timestamp(), TIMESTAMP_FLOOR_UNIX_SECS);
}

/// An unknown major is terminal, with the protocol's own code and its own sentence.
#[test]
fn an_unknown_major_is_refused_terminally() {
    let future = format!(
        r#"{{"v":{},"type":"typing.update","user_id":"u_1","channel_id":"c_1","active":true}}"#,
        PROTOCOL_VERSION + 1,
    );
    let error = decode_server_frame(&future).expect_err("an unknown major is refused");
    let FrameFailure::Rejected(state) = classify_frame_error(&error) else {
        panic!("an unknown major must be terminal");
    };
    let ConnectionState::Rejected { code, detail } = &state else {
        panic!("a terminal refusal must be a rejected connection state");
    };
    assert_eq!(code, UNSUPPORTED_VERSION_CODE);
    assert!(
        detail.contains("Reconnecting will not resolve this"),
        "the wire crate's peer-facing sentence must say reconnecting cannot help, \
         because a version mismatch is not transient: {detail}"
    );
}

/// Anything else that cannot be read is one bad frame, not an incompatible peer.
///
/// `AGENTS.md` §3.3 requires a network failure to be a recoverable state rather
/// than the end of the connection, so the default has to be "keep going".
#[rstest]
#[case::unknown_type(r#"{"v":1,"type":"something.new"}"#)]
#[case::wrong_direction(r#"{"v":1,"type":"message.send","channel_id":"c_1","content":"hi","client_msg_id":"6f0b1f8e-0e2a-4a5b-9c3d-1f2e3a4b5c6d"}"#)]
#[case::not_json("not json at all")]
#[case::empty("")]
fn an_unreadable_frame_that_is_not_a_version_problem_keeps_the_connection(#[case] text: &str) {
    let error = decode_server_frame(text).expect_err("this frame cannot be read");
    assert_eq!(
        classify_frame_error(&error),
        FrameFailure::Transient,
        "{text:?} is one bad frame, not an incompatible peer"
    );
}
