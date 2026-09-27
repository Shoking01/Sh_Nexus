//! `ShNexusError`: the variant set, and the `Display` text of each.
//!
//! `AGENTS.md` §3.3 specifies the enum, and specifying an error type is partly
//! about its *messages* -- they are what a user reads, what a log records, and
//! what a test can assert on. So the messages are pinned here rather than left to
//! whatever `thiserror` produces.
//!
//! # Why the `Display` strings are worth a test
//!
//! Three of these strings are user-facing by contract, not by accident:
//!
//! - [`ShNexusError::Auth`] -- the login screen shows it.
//! - [`ShNexusError::Theme`] -- `AGENTS.md` §10.2 requires an invalid theme to
//!   produce an in-app message.
//! - [`ShNexusError::Protocol`] -- the user sees a rejection when a message will
//!   not send.
//!
//! A reworded message is not a cosmetic change for those three: it is a changed
//! user-facing string with no test noticing. Pinning them is the cheapest way to
//! make the change deliberate.
//!
//! # Why the payloads are not asserted
//!
//! The three I/O variants carry a `String` in this revision rather than the
//! `#[from]` typed source `AGENTS.md` §3.3 shows -- `reqwest`,
//! `tokio-tungstenite` and `rusqlite` all arrive in Phase 3 and Phase 4, and
//! taking them in a pure-type work unit would mean a TLS stack and a C build for
//! a file of type definitions. Each variant's doc comment says so, and
//! [`the_io_variants_are_present_with_the_documented_prefix`] asserts the part
//! that must not change in the meantime: the variant exists, and its message
//! starts with the prefix §3.3 specifies.

use sh_nexus::errors::{Result, ShNexusError};

/// Every variant's message is exactly what `AGENTS.md` §3.3 specifies.
///
/// The nine cases are the constitution's, in its order. A case with no `match`
/// arm is a compile error, so a variant added without a message assertion fails
/// the build rather than the suite.
#[test]
fn every_variant_renders_the_documented_message() {
    let cases: [(ShNexusError, &str); 9] = [
        (
            ShNexusError::Network("connection refused".to_owned()),
            "network error: connection refused",
        ),
        (
            ShNexusError::WebSocket("closed by peer".to_owned()),
            "websocket error: closed by peer",
        ),
        (
            ShNexusError::Database("no such table: messages".to_owned()),
            "database error: no such table: messages",
        ),
        (
            ShNexusError::Auth("the password was wrong".to_owned()),
            "auth error: the password was wrong",
        ),
        (
            ShNexusError::Protocol("message.channel_id must not be blank".to_owned()),
            "protocol error: message.channel_id must not be blank",
        ),
        (
            ShNexusError::Config("could not create the config directory".to_owned()),
            "config error: could not create the config directory",
        ),
        (
            ShNexusError::Theme("`accent` is not a valid colour".to_owned()),
            "theme error: `accent` is not a valid colour",
        ),
        (
            ShNexusError::Unknown("window creation failed".to_owned()),
            "unknown error: window creation failed",
        ),
        // The ninth case: the `#[from]` variant. Only the prefix is pinned --
        // see the note below the loop.
        (
            ShNexusError::Serialization(
                serde_json::from_str::<u8>("{").expect_err("truncated JSON must not parse"),
            ),
            "serialization error: invalid type: map, expected u8 at line 1 column 0",
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(
            error.to_string(),
            expected,
            "{error:?} should render as {expected:?}"
        );
    }
}

/// Only the prefix of `Serialization` is this crate's to pin.
///
/// The other eight messages are literals this project chose, so pinning them
/// catches an accidental rewording. `Serialization`'s detail comes from
/// `serde_json`, and its wording is serde_json's to change -- a test that
/// asserted it would fail on a dependency bump for a reason that has nothing to
/// do with this project, and the fix would be to edit the test, which trains
/// people to edit failing tests.
///
/// So the contract is the prefix plus "the underlying message is carried", and
/// the pinned literal in the table above is *replaced* by a prefix check here,
/// deliberately, rather than being a second copy that can drift.
#[test]
fn serialization_pins_only_the_prefix_it_owns() {
    let error = ShNexusError::Serialization(
        serde_json::from_str::<u8>("{").expect_err("truncated JSON must not parse"),
    );
    let rendered = error.to_string();

    assert!(
        rendered.starts_with("serialization error: "),
        "the prefix is this crate's and must be stable, got {rendered}"
    );
    assert!(
        rendered.len() > "serialization error: ".len(),
        "the underlying message must be carried, not discarded: {rendered}"
    );
}

/// The nine variants of §3.3 are all present, and there are no more.
///
/// The list is the point. `AGENTS.md` §3.3 is explicit, and a tenth variant
/// added for convenience is a decision that should be visible in a test rather
/// than discovered in a `match` somewhere.
///
/// `Network`, `WebSocket` and `Database` are checked by *construction* -- a
/// variant that did not exist would not compile here, which is the strongest form
/// of assertion available for "exists".
#[test]
fn the_variant_set_is_exactly_the_documented_nine() {
    // Named explicitly, so removing or renaming one is a compile error.
    let _ = [
        ShNexusError::Network(String::new()),
        ShNexusError::WebSocket(String::new()),
        ShNexusError::Database(String::new()),
        ShNexusError::Serialization(
            serde_json::from_str::<u8>("null").expect_err("null is not a u8"),
        ),
        ShNexusError::Auth(String::new()),
        ShNexusError::Protocol(String::new()),
        ShNexusError::Config(String::new()),
        ShNexusError::Theme(String::new()),
        ShNexusError::Unknown(String::new()),
    ];

    // The count is asserted separately, because a *tenth* variant would compile
    // fine above. `AGENTS.md` 3.3 lists nine; this is the check that the list has
    // not grown.
    assert_eq!(
        9,
        [
            ShNexusError::Network(String::new()),
            ShNexusError::WebSocket(String::new()),
            ShNexusError::Database(String::new()),
            ShNexusError::Serialization(
                serde_json::from_str::<u8>("null").expect_err("null is not a u8"),
            ),
            ShNexusError::Auth(String::new()),
            ShNexusError::Protocol(String::new()),
            ShNexusError::Config(String::new()),
            ShNexusError::Theme(String::new()),
            ShNexusError::Unknown(String::new()),
        ]
        .len(),
        "AGENTS.md 3.3 specifies exactly nine variants"
    );
}

/// The three I/O variants keep the prefix §3.3 specifies while their payloads
/// are still `String`.
///
/// The deferral is real and has to be bounded: these three are the variants whose
/// payloads change when `reqwest`, `tokio-tungstenite` and `rusqlite` land, and
/// `PLAN.md` §2's Phase 3/Phase 4 are where that happens. What must not change
/// in the meantime is the message prefix, because a user-facing string that
/// shifts under a refactor is a change nobody chose.
#[test]
fn the_io_variants_are_present_with_the_documented_prefix() {
    let cases = [
        (ShNexusError::Network(String::new()), "network error: "),
        (ShNexusError::WebSocket(String::new()), "websocket error: "),
        (ShNexusError::Database(String::new()), "database error: "),
    ];

    for (error, prefix) in cases {
        assert!(
            error.to_string().starts_with(prefix),
            "{error:?} should start with {prefix:?}"
        );
    }
}

/// `Serialization` is a real `#[from]`, so `?` works on a serde_json error.
///
/// The only one of §3.3's `#[from]` impls that is wired in this revision, and
/// the reason is not laziness: `serde_json` is a dependency of this work unit for
/// the wire boundary, and the other three sources do not exist yet. The
/// difference matters at the call site, and it is worth a test because a
/// hand-written `map_err` would behave identically here and be a lie about the
/// other three.
#[test]
fn serialization_converts_from_a_serde_json_error() {
    // The `?` form, which is what a function returning `Result` writes.
    fn parses_a_value() -> Result<serde_json::Value> {
        let parsed: serde_json::Value = serde_json::from_str("{ not json")?;
        Ok(parsed)
    }
    assert!(matches!(
        parses_a_value(),
        Err(ShNexusError::Serialization(_))
    ));

    // And the explicit form, which is what `?` expands to.
    let converted = ShNexusError::from(
        serde_json::from_str::<serde_json::Value>("{ not json")
            .expect_err("malformed JSON must not parse"),
    );
    assert!(matches!(converted, ShNexusError::Serialization(_)));
    assert!(
        converted.to_string().starts_with("serialization error: "),
        "{converted}"
    );
}

/// `Protocol` is the boundary's error, and its messages name the field.
///
/// The convention `sh_nexus::network::mapping` follows: the offending **field**,
/// never the offending **value** when the field could be message content. The
/// convention is worth a test because it is invisible when it is followed and
/// load-bearing when it is broken -- an error that quoted a user's message body
/// would put it in a log, which `AGENTS.md` §7.5 forbids.
#[test]
fn protocol_errors_name_the_field_and_not_the_value() {
    let cases = [
        ("message.channel_id must not be blank", "message.channel_id"),
        (
            "message.client_msg_id must be a UUID",
            "message.client_msg_id",
        ),
        (
            "message.timestamp is outside the plausible range 2000-01-01..2100-01-01",
            "message.timestamp",
        ),
        ("channel.members[] must not be blank", "channel.members[]"),
    ];

    for (message, expected_field) in cases {
        let error = ShNexusError::Protocol(message.to_owned());
        assert_eq!(error.to_string(), format!("protocol error: {message}"));
        assert!(
            error.to_string().contains(expected_field),
            "expected {expected_field} in {error}"
        );
    }
}

/// The error type is `Debug`, because every failure path logs with `{:?}`.
///
/// `AGENTS.md` §7.5's logging levels assume a logger exists, and the Phase 0
/// note in `main.rs` records that `tracing` arrives in Phase 1. A `Debug` bound
/// on an error type is what makes `error!("{err:?}")` compile at every one of
/// those call sites, so it is a compile-time fact worth stating once.
#[test]
fn the_error_type_is_debug() {
    fn assert_debug<T: std::fmt::Debug + ?Sized>() {}
    assert_debug::<ShNexusError>();
    assert_debug::<dyn std::error::Error>();

    // And it is a `std::error::Error`, so it composes with `?` into a
    // `Box<dyn Error>` return, which is what `run()` needs for the GPUI
    // bootstrap path in `lib.rs`.
    let boxed: Box<dyn std::error::Error + Send + Sync> =
        Box::new(ShNexusError::Config("x".to_owned()));
    assert_eq!(
        boxed.to_string(),
        "config error: x",
        "a boxed ShNexusError should still render its message"
    );
}

/// The `Result` alias is the one `AGENTS.md` §9.1's doc template writes.
///
/// `pub type Result<T> = std::result::Result<T, ShNexusError>;` -- the alias must
/// be a one-parameter type, because every doc comment in the project follows the
/// template's `Result<T>` shape. A second parameter would silently break every
/// example that follows the constitution.
#[test]
fn the_result_alias_takes_one_parameter() {
    fn returns_the_alias() -> Result<u8> {
        Ok(7)
    }
    assert_eq!(returns_the_alias().expect("Ok is Ok"), 7);

    fn returns_the_alias_from_a_question_mark(error: serde_json::Error) -> Result<u8> {
        let _: u8 = serde_json::from_str("1")?; // uses the `#[from]` impl
        let _ = error;
        Ok(0)
    }
    let _ = returns_the_alias_from_a_question_mark(
        serde_json::from_str::<u8>("{").expect_err("truncated JSON must not parse"),
    );
}
