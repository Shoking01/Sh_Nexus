//! `core/theme.rs`: parse, validate, fall back, and keep the schema honest.
//!
//! Work unit 1D. `AGENTS.md` §4.2 names four behaviours for this module --
//! *"JSON parsing; schema validation; color format validation; fallback
//! behavior"* -- and this suite is scoped to all four, plus the two properties
//! §4.4 demands of any parser.
//!
//! # What is asserted here, and why each is a claim rather than a case
//!
//! 1. **Every rejection names its JSON path.** Not "it errored" -- `colors.accent`,
//!    `typography.sizes.body`. §10.2 requires an in-app message, and a message
//!    that does not say *where* is not actionable for the person who hand-edited
//!    the file. This is the module's one hard rule about error text, so it is
//!    asserted on every parametrized case rather than once.
//! 2. **Unknown keys fail.** `"colour"` for `"accent"` is the most likely
//!    hand-editing mistake there is, and the lenient outcome -- ignore it, render
//!    wrong, say nothing -- is the exact failure the message exists to prevent.
//! 3. **The numeric bounds are reachable, not decorative.** A validator that
//!    rejects everything above `MAX_FONT_SIZE` is indistinguishable from one that
//!    rejects every font size, so every bound has a case proving a value *at* it
//!    is accepted, and §4's three zero-boundary decisions (0 spacing legal, 0
//!    radius legal, 0 font size refused) each have a case.
//! 4. **The fallback never loses the reason.** §10.2 asks for the fallback *and*
//!    the message; a fallback that swallowed the rejection would satisfy half of
//!    it and none of the point.
//! 5. **The built-in themes are valid.** One test per theme, so a corrupted
//!    fixture names the theme it broke rather than failing a generic assertion.
//!
//! # Style
//!
//! 1C-2a's: names that read as assertions per `AGENTS.md` §4.3, `#[rstest]` with
//! `#[case]` for the parametrized rejections (ADR-008), and proptest for the two
//! properties §4.4 requires of a parser: **no arbitrary input panics**, and **a
//! valid theme always round-trips**.
//!
//! # The proptest inputs are built here, not taken from `Theme::to_json`
//!
//! A property whose cases are produced by the code under test proves the code
//! agrees with itself, which is a real risk: `to_json` and `parse` could both be
//! wrong about a bound and the property would still pass. So the generator here
//! builds **JSON text directly**, and the property is stated in the direction that
//! catches it: *the generator decides the values, `parse` must return them.* The
//! serializer is covered by a separate property and by the round-trip against the
//! three built-in themes, where the expected value is a file a person wrote.

use proptest::prelude::*;
use rstest::rstest;
use sh_nexus::core::theme::{
    default_theme, load_or_default, parse, BuiltIn, Color, Theme, ThemeError, ThemeLoad,
    COLOR_KEYS, FONT_SIZE_KEYS, MAX_ECHOED_CHARS, MAX_FONT_SIZE, MAX_LABEL_CHARS, MAX_RADIUS,
    MAX_SPACING, MAX_THEME_BYTES, MIN_FONT_SIZE, MIN_RADIUS, MIN_SPACING, RADIUS_KEYS,
    SPACING_KEYS, THEME_FORMAT_VERSION, THEME_KEYS,
};
use sh_nexus::errors::ShNexusError;

/// `AGENTS.md` §8.2's valid theme fixture.
///
/// `include_str!` rather than a `fs::read` at run time: it proves the file is
/// committed *where §8.2 says it is*, and it keeps every case hermetic -- the
/// same reasoning `core/theme.rs` uses to embed the built-in themes.
const FIXTURE_VALID: &str = include_str!("fixtures/theme_valid.json");

/// `AGENTS.md` §8.2's invalid theme fixture.
///
/// **The defect is a typo, deliberately.** §8.2 asks for "one valid, one
/// invalid" and does not say which failure the invalid one should carry. This
/// one carries the failure the module's strictness policy exists for --
/// `"colour"` in place of `"accent"` -- because that is the mistake a user
/// actually makes and the one whose lenient handling is completely invisible.
/// Every *other* rejection is covered by the parametrized tables below.
const FIXTURE_INVALID: &str = include_str!("fixtures/theme_invalid.json");

// ---------------------------------------------------------------------------
// The fixture, as data
// ---------------------------------------------------------------------------

/// Every leaf of the fixture as `(path, json value)`, in schema order.
///
/// `path` is the dotted path the module's errors use, so a case names a field the
/// way the message does. The value is the *JSON text*, quotes included where the
/// field is a string, which is what lets one table drive all three kinds of
/// surgery below: replace it, remove it, or move it out of range.
///
/// `the_leaf_table_points_at_the_fixture_exactly_once` is what keeps this table
/// honest -- a needle that matched twice or not at all would produce a document
/// that tests something other than what the case claims.
const FIXTURE_LEAVES: [(&str, &str); 28] = [
    ("name", "\"Fixture Valid Theme\""),
    ("author", "\"Sh_Nexus test suite\""),
    ("version", "1"),
    ("colors.background", "\"#14161c\""),
    ("colors.surface", "\"#1e2129\""),
    ("colors.sidebar", "\"#101218\""),
    ("colors.text", "\"#e4e7ee\""),
    ("colors.text_muted", "\"#949aab\""),
    ("colors.accent", "\"#5b8def\""),
    ("colors.accent_hover", "\"#7aa4f5\""),
    ("colors.danger", "\"#e5534b\""),
    ("colors.success", "\"#57ab5a\""),
    ("colors.mention", "\"#d9b56b\""),
    ("colors.code_block_bg", "\"#0b0d12\""),
    ("colors.bubble_self", "\"#2a2f3c\""),
    ("colors.bubble_other", "\"#1e2129\""),
    ("spacing.xs", "4"),
    ("spacing.sm", "8"),
    ("spacing.md", "16"),
    ("spacing.lg", "24"),
    ("radii.sm", "4"),
    ("radii.md", "8"),
    ("radii.lg", "16"),
    ("typography.family", "\"Inter\""),
    ("typography.sizes.caption", "11"),
    ("typography.sizes.timestamp", "10"),
    ("typography.sizes.body", "14"),
    ("typography.sizes.title", "16"),
];

/// The documented range of each numeric family, as `(path prefix, min, max)`.
const NUMERIC_FAMILIES: [(&str, u32, u32); 3] = [
    ("spacing", MIN_SPACING, MAX_SPACING),
    ("radii", MIN_RADIUS, MAX_RADIUS),
    ("typography.sizes", MIN_FONT_SIZE, MAX_FONT_SIZE),
];

// ---------------------------------------------------------------------------
// Fixture surgery
//
// Text surgery on the committed fixture rather than a JSON builder, for a reason
// that matters: a builder would share this suite's idea of the schema with the
// code under test, and a shared assumption is exactly what a schema test must not
// have. The file on disk is the reference; these helpers change one thing in it.
// ---------------------------------------------------------------------------

/// The `"key": value` text a path occupies in the fixture.
fn needle_of(path: &str) -> String {
    let value = FIXTURE_LEAVES
        .iter()
        .find(|(candidate, _)| *candidate == path)
        .map(|(_, value)| *value)
        .unwrap_or_else(|| panic!("{path} is not a leaf of the fixture"));
    format!("\"{}\": {value}", key_of(path))
}

/// The fixture's colours as `(key, value)`, derived rather than restated.
///
/// Derived from [`FIXTURE_LEAVES`] on purpose: a second hand-written table would
/// be a second place to forget an entry, and the point of this file is that there
/// is one.
fn fixture_colors() -> Vec<(&'static str, String)> {
    FIXTURE_LEAVES
        .iter()
        .filter_map(|(path, value)| {
            path.strip_prefix("colors.")
                .map(|key| (key, (*value).trim_matches('"').to_owned()))
        })
        .collect()
}

/// The fixture, with `needle` replaced by `replacement`.
///
/// The count is asserted rather than assumed: a fixture edit that made a needle
/// ambiguous would otherwise produce a document that tests something other than
/// what the case names -- a green test for the wrong reason, which is the bug
/// class this project's other boundary tests exist to prevent.
fn document_with(needle: &str, replacement: &str) -> String {
    assert_eq!(
        FIXTURE_VALID.matches(needle).count(),
        1,
        "the fixture should contain {needle:?} exactly once"
    );
    FIXTURE_VALID.replace(needle, replacement)
}

/// The fixture with the leaf at `path` set to `value`.
fn with_value(path: &str, value: &str) -> String {
    let needle = needle_of(path);
    let key = key_of(path);
    document_with(&needle, &format!("\"{key}\": {value}"))
}

/// The fixture with the byte span `start..=end` removed, together with whichever
/// comma the removal orphaned.
///
/// **Both commas matter, and which one is orphaned depends on where the entry
/// sat.** JSON forbids a trailing comma, so removing the *last* entry of an object
/// strands the comma that preceded it, while removing a *middle* one strands the
/// comma that followed it. Deciding up front which case applies is a special case
/// that is right until the fixture is reformatted, so the repair happens here,
/// once, for both directions.
fn without_span(start: usize, end: usize) -> String {
    let after = &FIXTURE_VALID[end + 1..];
    let mut out = String::with_capacity(FIXTURE_VALID.len());

    if after.starts_with(',') {
        out.push_str(&FIXTURE_VALID[..start]);
        out.push_str(&FIXTURE_VALID[end + 2..]);
    } else {
        let comma = FIXTURE_VALID[..start].rfind(',').unwrap_or_else(|| {
            panic!("a span with no comma on either side cannot be removed cleanly")
        });
        out.push_str(&FIXTURE_VALID[..comma]);
        out.push_str(after);
    }
    out
}

/// The fixture with the leaf at `path` removed.
fn without_value(path: &str) -> String {
    let needle = needle_of(path);
    let start = FIXTURE_VALID
        .find(&needle)
        .unwrap_or_else(|| panic!("the fixture should contain {needle:?} exactly once"));
    without_span(start, start + needle.len() - 1)
}

/// The fixture with the whole `"key": { ... }` object removed.
///
/// Brace-counted rather than cut at a fixed offset, so no case depends on how the
/// fixture happens to be indented: reformatting the fixture must not require
/// rewriting a dozen needles.
fn without_object(key: &str) -> String {
    let open = format!("\"{key}\": {{");
    let start = FIXTURE_VALID
        .find(&open)
        .unwrap_or_else(|| panic!("the fixture should contain a `{key}` object to remove"));

    // `i32` rather than `usize` so a miscount surfaces as a failed assertion
    // instead of a debug-build underflow at a point that says nothing useful.
    let mut depth = 0i32;
    let mut end = FIXTURE_VALID.len();
    for (offset, character) in FIXTURE_VALID[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert_eq!(depth, 0, "unbalanced braces while removing `{key}`");
    without_span(start, end)
}

/// The fixture with `key` added inside the object opened by `needle`.
fn with_extra_key(needle: &str, key: &str, value: &str) -> String {
    assert_eq!(
        FIXTURE_VALID.matches(needle).count(),
        1,
        "the fixture should contain {needle:?} exactly once"
    );
    FIXTURE_VALID.replace(needle, &format!("{needle}\"{key}\": {value}, "))
}

/// The last segment of a dotted path.
fn key_of(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// The rejection a document produces.
///
/// Panicking on acceptance is deliberate: every caller is a case asserting a
/// rejection, so an accepted document is a broken expectation and the panic says
/// which one.
fn rejection_of(document: &str) -> ThemeError {
    match parse(document.as_bytes()) {
        Ok(theme) => panic!("expected a rejection, got a valid theme: {}", theme.name()),
        Err(error) => error,
    }
}

/// The `Display` text of the rejection a document produces.
fn rejection_text(document: &str) -> String {
    rejection_of(document).to_string()
}

/// A theme the document describes, panicking with the rejection if it does not.
fn accepted(document: &str) -> Theme {
    match parse(document.as_bytes()) {
        Ok(theme) => theme,
        Err(error) => panic!("expected acceptance, got: {error}"),
    }
}

/// A built-in theme, panicking if it does not validate.
fn built_in(built_in: BuiltIn) -> Theme {
    built_in
        .theme()
        .unwrap_or_else(|error| panic!("the {} built-in must validate: {error}", built_in.id()))
}

/// The documented range for a numeric field's path.
fn bounds_of(path: &str) -> (u32, u32) {
    for (prefix, min, max) in NUMERIC_FAMILIES {
        if path.starts_with(prefix) {
            return (min, max);
        }
    }
    panic!("{path} is not a numeric field of the theme schema")
}

/// The number a parsed theme holds at `path`.
fn number_in(theme: &Theme, path: &str) -> u32 {
    match path {
        "spacing.xs" => theme.spacing().xs,
        "spacing.sm" => theme.spacing().sm,
        "spacing.md" => theme.spacing().md,
        "spacing.lg" => theme.spacing().lg,
        "radii.sm" => theme.radii().sm,
        "radii.md" => theme.radii().md,
        "radii.lg" => theme.radii().lg,
        "typography.sizes.caption" => theme.typography().sizes.caption,
        "typography.sizes.timestamp" => theme.typography().sizes.timestamp,
        "typography.sizes.body" => theme.typography().sizes.body,
        "typography.sizes.title" => theme.typography().sizes.title,
        other => panic!("{other} is not a numeric field"),
    }
}

// ---------------------------------------------------------------------------
// 0. The fixture table is itself correct
// ---------------------------------------------------------------------------

/// Every leaf's needle appears in the fixture exactly once.
///
/// Without this, twenty-eight cases could be building documents from a needle
/// that matched the wrong pair of bytes and still passing. It is the cheapest
/// insurance in the file and it is the one that makes the rest of the file mean
/// what it says.
#[test]
fn the_leaf_table_points_at_the_fixture_exactly_once() {
    for (path, _) in FIXTURE_LEAVES {
        let needle = needle_of(path);
        assert_eq!(
            FIXTURE_VALID.matches(&needle).count(),
            1,
            "{path} -> {needle:?} should occur exactly once in the fixture"
        );
    }
    assert_eq!(FIXTURE_LEAVES.len(), 28, "the theme schema has 28 leaves");
}

// ---------------------------------------------------------------------------
// 1. The valid path
// ---------------------------------------------------------------------------

/// A well-formed document is accepted with every one of its values intact.
#[test]
fn a_complete_document_is_accepted_with_every_value_intact() {
    let theme = accepted(FIXTURE_VALID);
    assert_eq!(theme.name(), "Fixture Valid Theme");
    assert_eq!(theme.author(), "Sh_Nexus test suite");
    assert_eq!(theme.version(), THEME_FORMAT_VERSION);
    assert_eq!(theme.colors().background.to_string(), "#14161c");
    assert_eq!(theme.colors().accent.to_string(), "#5b8def");
    assert_eq!(theme.colors().text_muted.to_string(), "#949aab");
    assert_eq!(theme.colors().code_block_bg.to_string(), "#0b0d12");
    assert_eq!(theme.spacing().xs, 4);
    assert_eq!(theme.spacing().lg, 24);
    assert_eq!(theme.radii().sm, 4);
    assert_eq!(theme.radii().lg, 16);
    assert_eq!(theme.typography().family, "Inter");
    assert_eq!(theme.typography().sizes.caption, 11);
    assert_eq!(theme.typography().sizes.title, 16);
}

/// The valid fixture is a claim in its own right, not only an input.
///
/// Separate from the case above on purpose: that one asserts *the parser*, this
/// one asserts *the file is committed and currently valid*. A fixture that drifts
/// into invalidity has to fail by name rather than as a confusing assertion
/// inside a larger test.
#[test]
fn the_committed_valid_fixture_is_valid() {
    let outcome = parse(FIXTURE_VALID.as_bytes());
    assert!(
        outcome.is_ok(),
        "crates/sh_nexus/tests/fixtures/theme_valid.json is AGENTS.md 8.2's valid \
         theme fixture and must parse. It was rejected: {outcome:?}"
    );
}

/// The invalid fixture is rejected, and the message says which key and why.
#[test]
fn the_committed_invalid_fixture_is_rejected_with_a_readable_reason() {
    match parse(FIXTURE_INVALID.as_bytes()) {
        Ok(theme) => panic!(
            "theme_invalid.json must be rejected; it parsed as {}",
            theme.name()
        ),
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("colors.colour"),
                "the message must name the offending key, got: {text}"
            );
            assert!(
                text.contains("accent"),
                "the message must list the keys that would have been accepted, \
                 which is the half the user acts on. Got: {text}"
            );
        }
    }
}

/// Whitespace and line breaks are not part of the schema.
///
/// A theme is a file a person edits, and a file a person edits has blank lines
/// and its keys in whatever order they typed them. Rejecting either would make
/// the format hostile for no gain.
#[test]
fn whitespace_and_line_breaks_do_not_change_a_theme() {
    let base = accepted(FIXTURE_VALID);
    let reformatted = FIXTURE_VALID
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n        ");

    assert_eq!(
        accepted(&reformatted),
        base,
        "reformatting a document must not change the theme it describes"
    );
}

/// A single-line document is as good as a pretty one.
#[test]
fn a_single_line_document_is_accepted() {
    let minified = FIXTURE_VALID.replace('\n', "");
    assert_eq!(accepted(&minified), accepted(FIXTURE_VALID));
}

/// Key order is not part of the schema either, and this proves it for a real
/// reordering rather than for whitespace alone.
///
/// Only the `colors` object is reversed -- it is the largest, so a mistake in it
/// cannot hide behind the other two -- and the resulting theme must be identical.
#[test]
fn reordering_the_colours_does_not_change_a_theme() {
    let base = accepted(FIXTURE_VALID);
    let entries = fixture_colors();
    assert_eq!(entries.len(), COLOR_KEYS.len());

    // The whole `colors` body, in reverse, as one line.
    let colours = entries
        .iter()
        .rev()
        .map(|(key, value)| format!("\"{key}\": \"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");

    // The span to replace runs from the first colour to the last, in the
    // fixture's own order rather than the reversed one.
    let first_needle = format!("\"{}\": \"{}\"", entries[0].0, entries[0].1);
    let last = entries.last().cloned().unwrap_or(("", String::new()));
    let last_needle = format!("\"{}\": \"{}\"", last.0, last.1);
    let start = FIXTURE_VALID
        .find(&first_needle)
        .unwrap_or_else(|| panic!("the fixture should begin its colours with {first_needle}"));
    let end = FIXTURE_VALID
        .find(&last_needle)
        .unwrap_or_else(|| panic!("the fixture should end its colours with {last_needle}"))
        + last_needle.len();

    let mut document = String::with_capacity(FIXTURE_VALID.len());
    document.push_str(&FIXTURE_VALID[..start]);
    document.push_str(&colours);
    document.push_str(&FIXTURE_VALID[end..]);

    assert_eq!(
        accepted(&document),
        base,
        "the order of the colour keys must not matter"
    );
}

// ---------------------------------------------------------------------------
// 2. Colour format
// ---------------------------------------------------------------------------

/// A colour that is not exactly `#rrggbb` is rejected, naming its path.
///
/// **Every row is a decision, not an accident** -- the module's section 3 gives
/// the reason for each group. The two that matter most are `shorthand` and
/// `with_alpha`: both are forms people write *expecting them to work*, and both
/// are refused rather than silently reinterpreted, because a silent
/// reinterpretation of a user's bytes is how a theme ends up a colour nobody
/// chose. `signed` and `minus_signed` are the subtler pair --
/// `u8::from_str_radix` accepts a leading sign, so `"#+f1e2e"` would parse as
/// `0x0f` if the digits were not checked before the radix parse.
#[rstest]
#[case::no_hash("1e1e2e", "colors.background")]
#[case::shorthand("#fff", "colors.background")]
#[case::shorthand_four("#fff0", "colors.background")]
#[case::with_alpha("#1e1e2eff", "colors.background")]
#[case::named("red", "colors.background")]
#[case::one_digit_too_many("#14161c0", "colors.background")]
#[case::not_hex("#gggggg", "colors.background")]
#[case::signed("#+14161c", "colors.background")]
#[case::minus_signed("#-14161c", "colors.background")]
#[case::space_inside("#141 61c", "colors.background")]
#[case::trailing_space("#14161c ", "colors.background")]
#[case::empty("", "colors.background")]
#[case::hash_only("#", "colors.background")]
#[case::accent("not-a-colour", "colors.accent")]
#[case::code_block_bg("nope", "colors.code_block_bg")]
#[case::bubble_self("0x14161c", "colors.bubble_self")]
fn a_malformed_colour_is_rejected_at_the_exact_path(#[case] value: &str, #[case] path: &str) {
    let text = rejection_text(&with_value(path, &format!("\"{value}\"")));
    assert!(
        text.contains(path),
        "the message must name `{path}`, got: {text}"
    );
}

/// Both hex cases are accepted and canonicalised to lower case.
///
/// A text editor's colour picker produces either, and refusing the upper case
/// would reject a theme that is perfectly readable to the person who wrote it.
#[rstest]
#[case::lower("#14161c")]
#[case::upper("#14161C")]
#[case::upper_digits("#14161c")]
#[case::upper_nibbles("#ABCDEF")]
#[case::pure_black("#000000")]
#[case::pure_white("#ffffff")]
fn either_hex_case_is_accepted_and_canonicalised_to_lower(#[case] input: &str) {
    let theme = accepted(&with_value("colors.background", &format!("\"{input}\"")));
    assert_eq!(theme.colors().background.to_string(), input.to_lowercase());
}

/// The whole `#rrggbb` alphabet round-trips, digit by digit.
///
/// proptest covers this statistically; this case covers it **exhaustively** for
/// the sixteen hex digits, which is cheap and catches a nibble misread on a path
/// the generator happened to skip.
#[test]
fn every_hex_digit_survives_a_round_trip() {
    for value in 0..16u8 {
        let digit = char::from_digit(u32::from(value), 16).unwrap_or('0');
        let colour = format!("#{digit}{digit}{digit}{digit}{digit}{digit}");
        assert_eq!(
            Color::from_hex(&colour).map(|parsed| parsed.to_string()),
            Some(colour.clone()),
            "{colour} should round-trip exactly"
        );
    }
}

/// An over-long malformed value is truncated in the message, not echoed whole.
///
/// `AGENTS.md` §7.5's rule is about message *content*, and a theme file is not
/// one -- but an error message also reaches a log, and a 60KB line in one is a
/// problem the limit exists to prevent.
#[test]
fn an_over_long_malformed_colour_is_truncated_in_the_message() {
    let noise = "z".repeat(MAX_ECHOED_CHARS * 4);
    match rejection_of(&with_value("colors.background", &format!("\"{noise}\""))) {
        ThemeError::InvalidColor { path, found } => {
            assert_eq!(path, "colors.background");
            assert_eq!(
                found.chars().count(),
                MAX_ECHOED_CHARS + 3,
                "the echoed value should be the first {MAX_ECHOED_CHARS} characters \
                 plus a visible ellipsis, got {found:?}"
            );
            assert!(found.ends_with("..."), "the truncation must be visible");
        }
        other => panic!("expected an InvalidColor, got {other:?}"),
    }
}

/// A malformed colour short enough to echo is echoed **whole**.
///
/// The other half of the truncation case: a limit that truncated everything would
/// satisfy the test above and make the message useless for a two-character typo.
#[test]
fn a_short_malformed_colour_is_echoed_whole() {
    match rejection_of(&with_value("colors.background", "\"#xyz\"")) {
        ThemeError::InvalidColor { found, .. } => assert_eq!(found, "#xyz"),
        other => panic!("expected an InvalidColor, got {other:?}"),
    }
}

/// A colour field of the wrong JSON type is a type error, not a format error.
///
/// The distinction is worth keeping: "must be a string, found an integer" tells
/// the user to add quotes, and "must be #rrggbb" tells them to fix a value that is
/// already the right shape.
#[rstest]
#[case::integer("42")]
#[case::boolean("true")]
#[case::null("null")]
#[case::array("[]")]
#[case::object("{}")]
fn a_colour_field_of_the_wrong_type_is_a_type_error(#[case] value: &str) {
    match rejection_of(&with_value("colors.background", value)) {
        ThemeError::WrongType {
            path,
            expected,
            found,
        } => {
            assert_eq!(path, "colors.background");
            assert_eq!(expected, "a string");
            assert!(!found.is_empty(), "the found-type should be named");
        }
        other => panic!("expected a WrongType, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. The numeric bounds
// ---------------------------------------------------------------------------

/// Every numeric field accepts a value at each end of its documented range.
///
/// **This is what makes the bounds real rather than decorative.** A validator
/// whose `MAX_FONT_SIZE` were unreachable would be indistinguishable, from the
/// outside, from one with no maximum at all -- and `MAX_FONT_SIZE` is documented
/// as a number a theme may use.
#[rstest]
#[case::spacing_xs("spacing.xs")]
#[case::spacing_sm("spacing.sm")]
#[case::spacing_md("spacing.md")]
#[case::spacing_lg("spacing.lg")]
#[case::radii_sm("radii.sm")]
#[case::radii_md("radii.md")]
#[case::radii_lg("radii.lg")]
#[case::caption("typography.sizes.caption")]
#[case::timestamp("typography.sizes.timestamp")]
#[case::body("typography.sizes.body")]
#[case::title("typography.sizes.title")]
fn a_value_at_either_bound_of_every_numeric_field_is_accepted(#[case] path: &str) {
    let (min, max) = bounds_of(path);

    for (label, value) in [("minimum", min), ("maximum", max)] {
        let theme = accepted(&with_value(path, &value.to_string()));
        assert_eq!(
            number_in(&theme, path),
            value,
            "{path} at its {label} ({value}) should be accepted and preserved"
        );
    }
}

/// A value one step outside a bound is rejected, naming the field and the range.
#[rstest]
#[case::spacing_xs("spacing.xs")]
#[case::spacing_sm("spacing.sm")]
#[case::spacing_md("spacing.md")]
#[case::spacing_lg("spacing.lg")]
#[case::radii_sm("radii.sm")]
#[case::radii_md("radii.md")]
#[case::radii_lg("radii.lg")]
#[case::caption("typography.sizes.caption")]
#[case::timestamp("typography.sizes.timestamp")]
#[case::body("typography.sizes.body")]
#[case::title("typography.sizes.title")]
fn a_value_one_step_outside_a_bound_is_rejected_naming_field_and_range(#[case] path: &str) {
    let (min, max) = bounds_of(path);

    for value in [i64::from(min) - 1, i64::from(max) + 1] {
        let text = rejection_text(&with_value(path, &value.to_string()));
        assert!(
            text.contains(path),
            "the message must name `{path}`, got: {text}"
        );
        assert!(
            text.contains(&min.to_string()) && text.contains(&max.to_string()),
            "the message must state the accepted range {min}..={max}, got: {text}"
        );
    }
}

/// Zero is a legal spacing and a legal radius, and an illegal font size.
///
/// **This is §4's one place where the three decisions could have been one rule and
/// were deliberately not**, so it is worth a case that says so in both directions.
/// A zero spacing is a flush layout somebody chose; a zero radius is square
/// corners; a zero font size is text that cannot be measured, hit-tested or read.
#[test]
fn zero_is_legal_for_spacing_and_radius_and_illegal_for_a_font_size() {
    assert_eq!(
        MIN_SPACING, 0,
        "spacing's lower bound is a documented decision"
    );
    assert_eq!(
        MIN_RADIUS, 0,
        "radius's lower bound is a documented decision"
    );
    assert_eq!(
        MIN_FONT_SIZE, 1,
        "a font size of zero is refused, which is why the bound is not 0"
    );

    for path in ["spacing.xs", "spacing.lg", "radii.sm", "radii.lg"] {
        assert_eq!(
            number_in(&accepted(&with_value(path, "0")), path),
            0,
            "{path} should accept zero"
        );
    }

    for path in [
        "typography.sizes.caption",
        "typography.sizes.timestamp",
        "typography.sizes.body",
        "typography.sizes.title",
    ] {
        let text = rejection_text(&with_value(path, "0"));
        assert!(
            text.contains(path),
            "a zero font size at `{path}` must be rejected by name, got: {text}"
        );
    }
}

/// A negative number is rejected for every numeric family, by name and by range.
///
/// Even where the lower bound is 0 a negative value is a different mistake from
/// an out-of-range one, and it deserves the same name-and-range message rather
/// than a complaint about something that *is* a number.
#[rstest]
#[case::spacing_xs("spacing.xs")]
#[case::radii_sm("radii.sm")]
#[case::body("typography.sizes.body")]
fn a_negative_number_is_rejected_naming_the_field_and_its_range(#[case] path: &str) {
    let (min, _) = bounds_of(path);
    let text = rejection_text(&with_value(path, "-1"));
    assert!(text.contains(path), "must name `{path}`, got: {text}");
    assert!(
        text.contains(&min.to_string()),
        "must state the lower bound {min}, got: {text}"
    );
}

/// A number beyond `u32` is reported with its true value, not as "not an integer".
///
/// `serde_json` keeps a `u64` that overflows `i64` as a `u64`, so a theme saying
/// `"xs": 18446744073709551615` is a legal JSON integer outside a legal range. The
/// message has to report **that** value: reporting it as "not an integer" would
/// send the user hunting for a syntax problem they do not have.
#[test]
fn a_number_beyond_u32_is_reported_with_its_true_value() {
    let text = rejection_text(&with_value("spacing.xs", "18446744073709551615"));
    assert!(
        text.contains("18446744073709551615"),
        "the value must be reported exactly, got: {text}"
    );
    assert!(
        text.contains("spacing.xs"),
        "must name the field, got: {text}"
    );
}

/// A fractional number is rejected, including one that is numerically integral.
///
/// `14.0` is worth rejecting on purpose: it is not a different value, but it is
/// evidence that the author is writing numbers loosely, and `14.5` comes next.
#[rstest]
#[case::truly_fractional("1.5")]
#[case::integral_float("4.0")]
#[case::exponent("1e2")]
fn a_number_that_is_not_an_integer_is_rejected(#[case] value: &str) {
    let text = rejection_text(&with_value("spacing.md", value));
    assert!(
        text.contains("spacing.md") && text.contains("integer"),
        "the message must say the field wants an integer, got: {text}"
    );
}

/// A number written as a string is a type error, not a range error.
#[test]
fn a_numeric_field_holding_a_string_is_a_type_error() {
    match rejection_of(&with_value("spacing.md", "\"16\"")) {
        ThemeError::WrongType {
            path,
            expected,
            found,
        } => {
            assert_eq!(path, "spacing.md");
            assert_eq!(expected, "an integer");
            assert_eq!(found, "a string");
        }
        other => panic!("expected a WrongType, got {other:?}"),
    }
}

/// A zero font size is rejected at **each** of the four size fields.
///
/// Separate from the test above, and per-field rather than looped, for a reason
/// the mutation table records rather than guesses at: the loop form left §4's one
/// asymmetric bound -- *a zero font size is refused where a zero spacing and a
/// zero radius are not* -- with a single catcher. A loop that breaks on the first
/// failing field is one test, and one test is a net a refactor can thin further
/// without anybody noticing.
#[rstest]
#[case::caption("typography.sizes.caption")]
#[case::timestamp("typography.sizes.timestamp")]
#[case::body("typography.sizes.body")]
#[case::title("typography.sizes.title")]
fn a_zero_font_size_is_rejected_at_every_size_field(#[case] path: &str) {
    let text = rejection_text(&with_value(path, "0"));
    assert!(
        text.contains(path) && text.contains('0'),
        "a zero font size at `{path}` must be rejected by name, got: {text}"
    );
}

/// An over-long label is refused at **each** of the three label fields.
///
/// `MAX_LABEL_CHARS` is an `AGENTS.md` §7.1 *"no unbounded growth of in-memory
/// state"* bound on a hot-reloadable file, and the mutation table caught exactly
/// one test when it was asserted for `name` alone. The other two fields are the
/// same rule reached by a different path through `text_member`, so they are worth
/// their own cases.
#[rstest]
#[case::name("name")]
#[case::author("author")]
#[case::family("typography.family")]
fn an_over_long_label_is_refused_at_every_label_field(#[case] path: &str) {
    let over = "n".repeat(MAX_LABEL_CHARS + 1);
    let text = rejection_text(&with_value(path, &format!("\"{over}\"")));
    assert!(
        text.contains(path) && text.contains(&MAX_LABEL_CHARS.to_string()),
        "an over-long `{path}` must name the field and the limit, got: {text}"
    );
}

/// A label exactly at the limit is accepted at each of the three label fields.
///
/// The other half of the case above, and the one that proves the limit is a
/// reachable boundary rather than a number nothing can satisfy.
#[rstest]
#[case::name("name")]
#[case::author("author")]
#[case::family("typography.family")]
fn a_label_at_the_limit_is_accepted_at_every_label_field(#[case] path: &str) {
    let at_limit = "n".repeat(MAX_LABEL_CHARS);
    let theme = accepted(&with_value(path, &format!("\"{at_limit}\"")));
    let read_back = match path {
        "name" => theme.name().to_owned(),
        "author" => theme.author().to_owned(),
        "typography.family" => theme.typography().family.clone(),
        other => panic!("{other} is not a label field"),
    };
    assert_eq!(read_back.chars().count(), MAX_LABEL_CHARS);
}

/// The limit is the number the documentation says it is.
#[test]
fn the_label_limit_is_the_documented_number() {
    assert_eq!(MAX_LABEL_CHARS, 128);
}

/// The documented ranges of §4, asserted as numbers rather than trusted as prose.
///
/// A doc comment that says `0..=256` while the code says `0..=255` is a wrong
/// answer to a question a theme author will ask, and this is the only thing that
/// notices.
#[test]
fn the_documented_numeric_bounds_are_the_bounds_in_force() {
    assert_eq!((MIN_SPACING, MAX_SPACING), (0, 256));
    assert_eq!((MIN_RADIUS, MAX_RADIUS), (0, 512));
    assert_eq!((MIN_FONT_SIZE, MAX_FONT_SIZE), (1, 512));
    assert_eq!(NUMERIC_FAMILIES[0], ("spacing", 0, 256));
    assert_eq!(NUMERIC_FAMILIES[1], ("radii", 0, 512));
    assert_eq!(NUMERIC_FAMILIES[2], ("typography.sizes", 1, 512));
}

// ---------------------------------------------------------------------------
// 4. Keys: missing and unknown
// ---------------------------------------------------------------------------

/// Every colour key lands in its own `Palette` field, under its own path.
///
/// **This is the schema test**, and it is what makes "removing a key is a visible
/// failure" true in all three places at once:
///
/// - remove a key from `COLOR_KEYS` and the explicit list below fails, *and*
///   `palette_from`'s struct literal stops compiling;
/// - rename one and the same two things fail;
/// - **swap two** -- give `sidebar` the value belonging to `surface` -- and this
///   case fails while everything still compiles, because each field receives a
///   distinct colour and the assertion notices the two fields exchanging places.
///
/// A distinct colour per key is what makes that last case detectable. Reusing the
/// fixture's own values would make a swap invisible, which is why the substitution
/// is `#0000XX` derived from the index rather than a value from the file.
#[test]
fn every_colour_key_reaches_its_own_palette_field_under_its_own_path() {
    assert_eq!(
        COLOR_KEYS,
        [
            "background",
            "surface",
            "sidebar",
            "text",
            "text_muted",
            "accent",
            "accent_hover",
            "danger",
            "success",
            "mention",
            "code_block_bg",
            "bubble_self",
            "bubble_other",
        ],
        "the colour schema is thirteen keys, in this order"
    );

    for (index, key) in COLOR_KEYS.iter().enumerate() {
        // `#0000XX`: distinct per index, and always a legal colour.
        let distinct = format!("#0000{:02x}", index as u8);
        let path = format!("colors.{key}");
        let theme = accepted(&with_value(&path, &format!("\"{distinct}\"")));

        assert_eq!(
            theme.colors().get(key).map(|colour| colour.to_string()),
            Some(distinct.clone()),
            "{key} should reach Palette's {key} field"
        );
        assert_eq!(
            theme.colors().get(key_of(&path)).map(|c| c.to_string()),
            Some(distinct),
            "and `get` should resolve it from its own key"
        );

        let text = rejection_text(&with_value(&path, "\"nope\""));
        assert!(
            text.contains(&path),
            "a bad {key} must be reported at {path}, got: {text}"
        );
    }
}

/// The other three key lists are pinned to the schema, explicitly.
#[test]
fn the_spacing_radius_and_font_size_schemas_are_pinned() {
    assert_eq!(SPACING_KEYS, ["xs", "sm", "md", "lg"]);
    assert_eq!(RADIUS_KEYS, ["sm", "md", "lg"]);
    assert_eq!(FONT_SIZE_KEYS, ["caption", "timestamp", "body", "title"]);
    assert_eq!(
        THEME_KEYS,
        [
            "name",
            "author",
            "version",
            "colors",
            "spacing",
            "radii",
            "typography"
        ]
    );
}

/// `Palette::get` resolves exactly the thirteen keys and nothing else.
///
/// The other half of the case above: a `get` that accepted `"colour"` would be a
/// silent-reintroduction of the very leniency `reject_unknown_keys` refuses.
#[test]
fn palette_get_resolves_exactly_the_schema_and_nothing_else() {
    let palette = *accepted(FIXTURE_VALID).colors();
    for key in COLOR_KEYS {
        assert!(
            palette.get(key).is_some(),
            "{key} is in the schema and should resolve"
        );
    }
    for key in ["colour", "bg", "Colors", "", "text-muted", "accent_hover "] {
        assert!(
            palette.get(key).is_none(),
            "{key:?} is not a schema key and must not resolve"
        );
    }
}

/// Every numeric key lands in its own field, under its own path.
///
/// A loop rather than a case table because the same assertion runs for all
/// eleven, and a failure names the path that broke -- which is the whole value of
/// running it at all.
#[test]
fn every_numeric_key_reaches_its_own_field_under_its_own_path() {
    let numeric: Vec<&str> = FIXTURE_LEAVES
        .iter()
        .map(|(path, _)| *path)
        .filter(|path| {
            path.starts_with("spacing.")
                || path.starts_with("radii.")
                || path.starts_with("typography.sizes.")
        })
        .collect();
    assert_eq!(numeric.len(), 11, "the schema has eleven numeric fields");

    for path in numeric {
        let text = rejection_text(&with_value(path, "-1"));
        assert!(
            text.contains(path),
            "a bad {path} must be reported at {path}, got: {text}"
        );
        assert_eq!(
            number_in(&accepted(&with_value(path, "7")), path),
            7,
            "{path} should reach its own field"
        );
    }
}

/// A required top-level **leaf** that is absent is reported by its path.
///
/// Separate from the object case below because the two need different surgery:
/// `name` is a string, `colors` is an object, and a helper that handled both
/// would be a helper with a branch in it that one of them never exercises.
#[rstest]
#[case::name("name")]
#[case::author("author")]
#[case::version("version")]
fn a_missing_top_level_leaf_is_reported_by_its_path(#[case] key: &str) {
    let text = rejection_text(&without_value(key));
    assert!(
        text.contains(&format!("`{key}` is required")),
        "a missing `{key}` must be reported as required, got: {text}"
    );
}

/// A required top-level **object** that is absent is reported by its path.
///
/// All four, because `member()` is the single helper every one of them goes
/// through -- and a level that is not exercised is a level whose removal is never
/// proven to leave a document the parser can still read.
#[rstest]
#[case::colors("colors")]
#[case::spacing("spacing")]
#[case::radii("radii")]
#[case::typography("typography")]
fn a_missing_top_level_object_is_reported_by_its_path(#[case] key: &str) {
    let text = rejection_text(&without_object(key));
    assert!(
        text.contains(&format!("`{key}` is required")),
        "a missing `{key}` must be reported as required, got: {text}"
    );
}

/// A missing leaf is reported with the full dotted path.
///
/// The `typography.sizes` rows are the ones that matter: they are the only paths
/// three segments deep, so they are the only place a dropped prefix would show.
#[rstest]
#[case::background("colors.background")]
#[case::accent("colors.accent")]
#[case::bubble_other("colors.bubble_other")]
#[case::spacing_xs("spacing.xs")]
#[case::spacing_lg("spacing.lg")]
#[case::radii_sm("radii.sm")]
#[case::family("typography.family")]
#[case::sizes("typography.sizes")]
#[case::caption("typography.sizes.caption")]
#[case::timestamp("typography.sizes.timestamp")]
#[case::body("typography.sizes.body")]
#[case::title("typography.sizes.title")]
fn a_missing_leaf_is_reported_with_the_full_dotted_path(#[case] path: &str) {
    let document = if path == "typography.sizes" {
        without_object("sizes")
    } else {
        without_value(path)
    };
    let text = rejection_text(&document);
    assert!(
        text.contains(&format!("`{path}` is required")),
        "a missing `{path}` must be reported as required, got: {text}"
    );
}

/// An unknown top-level key is rejected, and the alternatives are listed.
///
/// **A top-level key's path is the bare key, with no prefix.** That is
/// `child(DOCUMENT, key)` rather than `` `<document>.key` ``, and it is
/// deliberate: `<document>` names the *thing itself* when the whole document is
/// the wrong type, and prefixing every top-level key with it would make the two
/// read as the same location when they are not.
#[test]
fn an_unknown_top_level_key_is_rejected_with_the_alternatives() {
    let document = FIXTURE_VALID.replacen('{', "{\"colour\": 1,", 1);
    let text = rejection_text(&document);
    assert!(
        text.contains("`colour` is not a key in this schema"),
        "a root-level unknown key must be reported by its bare name, got: {text}"
    );
    assert!(
        text.contains("name") && text.contains("typography"),
        "the message must list the keys that would have been accepted. Got: {text}"
    );
}

/// An unknown key inside a nested object is rejected at the level it was found.
///
/// One case per level rather than one for all of them, because the path prefix is
/// built by `child()` and each prefix is a different string. `colors.m` and
/// `radii.xl` are the interesting rows: the first is a *near miss* on a real key
/// and the second is a key from another level entirely, and both must be refused
/// rather than quietly ignored.
#[rstest]
#[case::colors_typo("\"colors\": {", "colour", "\"#fff\"", "colors.colour")]
#[case::colors_near_miss("\"colors\": {", "m", "\"#fff\"", "colors.m")]
#[case::spacing_typo("\"spacing\": {", "m", "16", "spacing.m")]
#[case::radii_new_scale("\"radii\": {", "xl", "16", "radii.xl")]
#[case::typography_extra("\"typography\": {", "weight", "400", "typography.weight")]
#[case::sizes_heading("\"sizes\": {", "h1", "16", "typography.sizes.h1")]
fn an_unknown_key_is_rejected_at_the_level_it_was_found(
    #[case] needle: &str,
    #[case] key: &str,
    #[case] value: &str,
    #[case] path: &str,
) {
    let text = rejection_text(&with_extra_key(needle, key, value));
    assert!(
        text.contains(&format!("`{path}` is not a key in this schema")),
        "the message must name `{path}` as unknown, got: {text}"
    );
}

/// A typo'd colour key is rejected rather than ignored -- the mistake §3 is about.
#[test]
fn a_typo_instead_of_a_colour_key_is_rejected_rather_than_ignored() {
    let text = rejection_text(&with_extra_key("\"colors\": {", "colour", "\"#5b8def\""));
    assert!(text.contains("colors.colour"), "got: {text}");

    // The alternatives must be listed, because "you misspelled it" is only half
    // an answer; the other half is "here is what you could have written".
    for expected in ["accent", "accent_hover", "background"] {
        assert!(
            text.contains(expected),
            "the message must offer `{expected}` as an alternative, got: {text}"
        );
    }
}

/// Which unknown key is reported does not depend on the document's key order.
///
/// **This is a live hazard, not a hypothetical one.** `serde_json::Map` is a
/// `BTreeMap` by default and an `IndexMap` under `preserve_order`, and
/// `cargo tree -e features -i serde_json` shows that feature is enabled in this
/// workspace by `gpui` and `http_client` (Zed's git pin). Feature unification
/// makes the *client's* map iterate in document order, so without the sort inside
/// `reject_unknown_keys` the key named in the message would depend on the order
/// the user happened to type them in. The sort is what makes it a property of the
/// schema.
#[test]
fn the_reported_unknown_key_does_not_depend_on_document_order() {
    let text = rejection_text(&with_extra_key("\"colors\": {", "zeta", "1"));
    assert!(
        text.contains("colors.zeta"),
        "the one unknown key must be reported, got: {text}"
    );

    // Two unknown keys, the alphabetically later one written first in the
    // document -- which is the case that fails if iteration order leaks through.
    let forward = with_extra_key("\"colors\": {", "zeta", "1");
    let forward = with_extra_key_at(&forward, "\"colors\": {", "alpha", "1");
    let reversed = with_extra_key("\"colors\": {", "alpha", "1");
    let reversed = with_extra_key_at(&reversed, "\"colors\": {", "zeta", "1");

    let forward_text = rejection_text(&forward);
    let reversed_text = rejection_text(&reversed);
    assert!(
        forward_text.contains("colors.alpha"),
        "the alphabetically first unknown key must be the one reported whatever \
         the document's order. Got: {forward_text}"
    );
    assert_eq!(
        forward_text, reversed_text,
        "the message must not depend on the order the keys were typed in"
    );
}

/// `with_extra_key` against an already-modified document.
///
/// Needed because `with_extra_key` reads the fixture, and the case above has to
/// add a *second* key to the same object.
fn with_extra_key_at(document: &str, needle: &str, key: &str, value: &str) -> String {
    document.replace(needle, &format!("{needle}\"{key}\": {value}, "))
}

// ---------------------------------------------------------------------------
// 5. Version, labels, and the document as a whole
// ---------------------------------------------------------------------------

/// `version` must be exactly the one this build speaks.
#[test]
fn version_one_is_accepted_and_is_the_only_supported_version() {
    assert_eq!(THEME_FORMAT_VERSION, 1);
    assert_eq!(accepted(FIXTURE_VALID).version(), 1);
}

/// Any other version is refused by name, with the value reported.
///
/// §7.4 requires an unknown major version to be rejected explicitly rather than
/// interpreted, so the message has to say what was found and what is supported.
#[rstest]
#[case::zero("0")]
#[case::two("2")]
#[case::large("99")]
#[case::negative("-1")]
fn a_version_this_build_does_not_speak_is_refused_by_name(#[case] version: &str) {
    match rejection_of(&with_value("version", version)) {
        ThemeError::UnsupportedVersion { found } => assert_eq!(
            found.to_string(),
            version,
            "the message must report the version that was written"
        ),
        other => panic!("expected an UnsupportedVersion, got {other:?}"),
    }
}

/// A `version` of the wrong type is a type error naming `version`.
///
/// The message deliberately does **not** also name the supported version: at this
/// point the value is not a number at all, so "must be an integer" is the whole
/// of what is wrong with it, and the version negotiation belongs to the message
/// that has an actual version to negotiate about.
#[rstest]
#[case::string("\"1\"")]
#[case::integral_float("1.0")]
#[case::fractional("1.5")]
#[case::null("null")]
#[case::boolean("true")]
fn a_version_of_the_wrong_type_is_a_type_error(#[case] version: &str) {
    let text = rejection_text(&with_value("version", version));
    assert!(
        text.contains("`version` must be an integer"),
        "the message must name `version` and what it wants, got: {text}"
    );
}

/// A blank label is refused, and whitespace counts as blank.
///
/// A theme picker that renders an empty row for an unnamed theme is a UI with a
/// hole in it, and `"   "` is exactly as unusable as `""`.
#[rstest]
#[case::name_empty("name", "\"\"")]
#[case::name_spaces("name", "\"   \"")]
#[case::name_newline("name", "\"\\n\"")]
#[case::author_empty("author", "\"\"")]
#[case::author_tabs("author", "\"\\t\\t\"")]
#[case::family_spaces("typography.family", "\"  \"")]
fn a_blank_label_is_refused_wherever_it_appears(#[case] path: &str, #[case] value: &str) {
    let text = rejection_text(&with_value(path, value));
    assert!(
        text.contains(path) && text.contains("empty or whitespace only"),
        "a blank `{path}` must be reported as blank, got: {text}"
    );
}

/// A label exactly at the limit is accepted; one character more is refused.
#[test]
fn a_label_at_the_character_limit_is_accepted_and_one_more_is_refused() {
    let at_limit = "n".repeat(MAX_LABEL_CHARS);
    let theme = accepted(&with_value("name", &format!("\"{at_limit}\"")));
    assert_eq!(theme.name().chars().count(), MAX_LABEL_CHARS);

    let over = "n".repeat(MAX_LABEL_CHARS + 1);
    let text = rejection_text(&with_value("name", &format!("\"{over}\"")));
    assert!(
        text.contains("name") && text.contains(&MAX_LABEL_CHARS.to_string()),
        "the message must name the field and the limit, got: {text}"
    );
}

/// The limit counts characters, not bytes.
///
/// A non-ASCII label is legal input, and a byte count would refuse 43 two-byte
/// characters as "too long" when they are nowhere near the documented limit.
#[test]
fn the_label_limit_counts_characters_rather_than_bytes() {
    let label = "ñ".repeat(MAX_LABEL_CHARS);
    assert!(
        label.len() > MAX_LABEL_CHARS,
        "the label must be longer in bytes than the limit is in characters, \
         otherwise this case proves nothing"
    );
    let theme = accepted(&with_value("name", &format!("\"{label}\"")));
    assert_eq!(theme.name().chars().count(), MAX_LABEL_CHARS);
}

/// The document itself must be an object.
#[rstest]
#[case::array("[]")]
#[case::string("\"a theme\"")]
#[case::number("42")]
#[case::boolean("true")]
#[case::null("null")]
fn a_document_that_is_not_an_object_is_rejected_by_the_document_path(#[case] document: &str) {
    let text = rejection_text(document);
    assert!(
        text.contains("`<document>` must be an object"),
        "the message must name the document and the type it wanted, got: {text}"
    );
}

/// Bytes that are not JSON are rejected as a parse failure, with a locator.
///
/// **This is the one rejection whose message carries no JSON path**, and the
/// reason is recorded rather than left as a gap: a syntax error has no path, it
/// has a line and a column, and `serde_json`'s message carries both.
#[rstest]
#[case::empty("")]
#[case::open_brace("{")]
#[case::trailing_comma("{\"name\": \"a\",}")]
#[case::unquoted_key("{name: \"a\"}")]
#[case::single_quotes("{'name': 'a'}")]
#[case::text("not json at all")]
#[case::truncated(FIXTURE_VALID.split_at(60).0)]
fn bytes_that_are_not_json_are_rejected_as_a_parse_failure(#[case] document: &str) {
    match rejection_of(document) {
        ThemeError::MalformedJson { detail } => {
            assert!(!detail.is_empty(), "the parse error should say something")
        }
        other => panic!("expected a MalformedJson, got {other:?}"),
    }
}

/// A label field of the wrong JSON type is a type error, at every label field.
///
/// **This case exists because a coverage report said so.** `--show-missing-lines`
/// named lines 1246 and 1248-1249 -- the `WrongType` arm inside `text()` -- as
/// the only unexecuted path in the label handling, and the reason is a gap rather
/// than an unreachable arm: a colour field holding a number was tested, and a
/// *label* field holding one was not. `text()` is reached by three fields through
/// a different helper than the colour one, so the same rule needed its own case.
#[rstest]
#[case::name("name")]
#[case::author("author")]
#[case::family("typography.family")]
fn a_label_field_of_the_wrong_type_is_a_type_error(#[case] path: &str) {
    for value in ["42", "true", "null", "[]", "{}"] {
        match rejection_of(&with_value(path, value)) {
            ThemeError::WrongType {
                path: reported,
                expected,
                found,
            } => {
                assert_eq!(reported, path, "the message must name `{path}`");
                assert_eq!(expected, "a string");
                assert!(!found.is_empty(), "the found-type should be named");
            }
            other => panic!("expected a WrongType for {path} = {value}, got {other:?}"),
        }
    }
}

/// The size rejection's message says what was too big and what the limit is.
///
/// **Also a coverage finding.** The `TooLarge` variant was *constructed* by a test
/// but its `Display` was never called, because that test matched on the enum
/// instead of reading the message. `AGENTS.md` §10.2 asks for an in-app message,
/// and a variant whose message is never rendered is a message nobody has read.
#[test]
fn the_size_rejection_names_the_size_and_the_limit() {
    let oversized = "x".repeat(MAX_THEME_BYTES + 1);
    let text = rejection_text(&oversized);
    assert!(
        text.contains(&(MAX_THEME_BYTES + 1).to_string())
            && text.contains(&MAX_THEME_BYTES.to_string()),
        "the message must state what the document measured and what the limit is, \
         got: {text}"
    );
    assert!(
        text.contains("bytes"),
        "and it should say what the numbers are bytes, got: {text}"
    );
}

/// A document over the size limit is refused before it is parsed.
///
/// Checked first on purpose: the point of the bound is to avoid doing work on a
/// 60GB file, and a parse that *succeeded* would have already failed at the thing
/// the bound exists to prevent.
#[test]
fn an_oversized_document_is_refused_before_it_is_parsed() {
    let oversized = "x".repeat(MAX_THEME_BYTES + 1);
    match rejection_of(&oversized) {
        ThemeError::TooLarge { bytes, limit } => {
            assert_eq!(bytes, MAX_THEME_BYTES + 1);
            assert_eq!(limit, MAX_THEME_BYTES);
        }
        other => panic!("expected a TooLarge, got {other:?}"),
    }
}

/// A document exactly at the size limit is not refused for its size.
///
/// Without this the bound could be off by one in the direction that matters least
/// and nothing would notice.
#[test]
fn a_document_at_the_size_limit_is_not_refused_for_its_size() {
    let at_limit = "x".repeat(MAX_THEME_BYTES);
    let outcome = parse(at_limit.as_bytes());
    assert!(
        !matches!(outcome, Err(ThemeError::TooLarge { .. })),
        "a document of exactly {MAX_THEME_BYTES} bytes is at the limit, not over it"
    );
}

// ---------------------------------------------------------------------------
// 6. Colour as a value
// ---------------------------------------------------------------------------

/// `Color` is a newtype, and its accessors agree with the hex it was built from.
#[test]
fn a_colour_exposes_the_components_it_was_built_from() {
    let colour = Color::from_hex("#14161c").unwrap_or(Color::from_rgb(0, 0, 0));
    assert_eq!(colour.components(), [0x14, 0x16, 0x1c]);
    assert_eq!(colour.red(), 0x14);
    assert_eq!(colour.green(), 0x16);
    assert_eq!(colour.blue(), 0x1c);
    assert_eq!(colour, Color::from_rgb(0x14, 0x16, 0x1c));
}

/// `from_hex` refuses every malformed form and admits no others.
#[test]
fn from_hex_admits_exactly_the_documented_format() {
    assert_eq!(Color::from_hex("#000000"), Some(Color::from_rgb(0, 0, 0)));
    for rejected in [
        "",
        "#",
        "#0",
        "#00000",
        "#0000000",
        "000000",
        "# 00000",
        "#00000 ",
        "#-00000",
        "#+00000",
        "#0000g0",
        "##000000",
        "0x000000",
        "rgb(0,0,0)",
    ] {
        assert!(
            Color::from_hex(rejected).is_none(),
            "{rejected:?} is not #rrggbb and must be refused"
        );
    }
}

/// Pure black is a colour rather than a sentinel.
///
/// §7 of the module docs: `#000000` is a real colour, which is why a `Palette`
/// field is a `Color` and not an `Option<Color>` with a black default -- the
/// sentinel would have made the darkest theme in the set indistinguishable from an
/// absent key. The next case is what stops this being theoretical.
#[test]
fn pure_black_is_a_colour_rather_than_a_sentinel() {
    let black = Color::from_rgb(0, 0, 0);
    assert_eq!(black.to_string(), "#000000");
    assert_eq!(Color::from_hex("#000000"), Some(black));
    assert_ne!(black, Color::from_rgb(255, 255, 255));
}

/// The high-contrast built-in really does use pure black.
#[test]
fn the_high_contrast_built_in_uses_pure_black() {
    let theme = built_in(BuiltIn::HighContrast);
    assert_eq!(theme.colors().background.to_string(), "#000000");
    assert_eq!(theme.colors().text.to_string(), "#ffffff");
}

// ---------------------------------------------------------------------------
// 7. The built-in themes
// ---------------------------------------------------------------------------

/// The dark built-in parses, validates, and says what it is.
#[test]
fn the_dark_built_in_parses_and_validates() {
    let theme = built_in(BuiltIn::Dark);
    assert_eq!(theme.name(), "Sh_Nexus Dark");
    assert_eq!(theme.author(), "Sh_Nexus");
    assert_eq!(theme.version(), THEME_FORMAT_VERSION);
    assert_eq!(theme.typography().family, "Inter");
}

/// The light built-in parses, validates, and is actually lighter than the dark one.
///
/// The comparison is the assertion that matters: a "light" theme that is not
/// lighter is a broken fixture, and nothing about *parsing* would notice.
#[test]
fn the_light_built_in_parses_and_validates_and_is_lighter() {
    let theme = built_in(BuiltIn::Light);
    assert_eq!(theme.name(), "Sh_Nexus Light");
    assert_eq!(theme.author(), "Sh_Nexus");
    assert_eq!(theme.version(), THEME_FORMAT_VERSION);

    let dark = built_in(BuiltIn::Dark);
    assert!(
        theme.colors().background.red() > dark.colors().background.red(),
        "the light theme's background ({:?}) must be lighter than the dark one's \
         ({:?})",
        theme.colors().background,
        dark.colors().background
    );
}

/// The high-contrast built-in parses, validates, and uses square corners.
///
/// Zero radii are what exercise `MIN_RADIUS == 0`, so if this ever fails the
/// decision changed and §4's reasoning has to be re-read rather than patched.
#[test]
fn the_high_contrast_built_in_parses_and_validates_with_square_corners() {
    let theme = built_in(BuiltIn::HighContrast);
    assert_eq!(theme.name(), "Sh_Nexus High Contrast");
    assert_eq!(theme.author(), "Sh_Nexus");
    assert_eq!(theme.version(), THEME_FORMAT_VERSION);
    assert_eq!(theme.radii().sm, 0);
    assert_eq!(theme.radii().md, 0);
    assert_eq!(theme.radii().lg, 0);
}

/// `AGENTS.md` §10.2's "at least 3" is met, with the three it names.
#[test]
fn there_are_at_least_three_distinct_built_in_themes() {
    assert_eq!(
        BuiltIn::ALL.len(),
        3,
        "AGENTS.md 10.2 names dark, light and high-contrast"
    );
    assert_eq!(BuiltIn::ALL[0].id(), "dark");
    assert_eq!(BuiltIn::ALL[1].id(), "light");
    assert_eq!(BuiltIn::ALL[2].id(), "high-contrast");

    let mut sources: Vec<&str> = BuiltIn::ALL.iter().map(|b| b.source()).collect();
    let before = sources.len();
    sources.sort_unstable();
    sources.dedup();
    assert_eq!(
        sources.len(),
        before,
        "each built-in must be a different file"
    );
}

/// Every built-in carries the id and the name the code says it does.
///
/// `BuiltIn::id` is written to a persisted preference, so a rename there silently
/// discards a user's choice. That is why this is a test and not a comment.
#[test]
fn every_built_in_carries_its_own_id_and_name() {
    for id in BuiltIn::ALL {
        let theme = built_in(id);
        assert_eq!(
            theme.name(),
            id.display_name(),
            "{}'s display_name and its document's own name must agree",
            id.id()
        );
        assert!(!id.id().is_empty(), "{} must have an id", id.id());
    }
}

/// The default the fallback uses is the dark theme.
#[test]
fn the_default_theme_is_the_dark_built_in() {
    let fallback = default_theme().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(fallback, built_in(BuiltIn::Dark));
}

/// `AGENTS.md` §10.2 requires the built-ins to be in the binary.
///
/// `include_str!` is a compile-time constant, so this cannot prove the bytes
/// reached the binary. What it *can* prove -- and what it does -- is that the
/// embedded copy and the file on disk have not drifted, which is the failure that
/// actually happens.
#[test]
fn the_built_in_sources_are_the_files_on_disk() {
    for id in BuiltIn::ALL {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("themes")
            .join(format!("{}.json", id.id()));
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()));
        assert_eq!(
            id.source(),
            on_disk,
            "{} and the file on disk have drifted",
            id.id()
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Round-tripping
// ---------------------------------------------------------------------------

/// `to_json` emits something `parse` accepts back, unchanged.
///
/// Checked against the three built-ins, where the expected value is a theme file
/// a person wrote rather than one the serializer produced -- the only version of
/// this test that is not the code agreeing with itself.
#[test]
fn a_theme_round_trips_through_its_own_canonical_json() {
    for id in BuiltIn::ALL {
        let original = built_in(id);
        let json = original.to_json();
        match parse(json.as_bytes()) {
            Ok(round_tripped) => {
                assert_eq!(round_tripped, original, "{} did not round-trip", id.id())
            }
            Err(error) => panic!("{} emitted unparseable JSON: {error}", id.id()),
        }
    }
}

/// The canonical form is what the module says it is: compact, ordered, lower-case.
#[test]
fn the_canonical_json_is_compact_and_in_schema_order() {
    let json = accepted(FIXTURE_VALID).to_json();
    assert!(
        !json.contains('\n') && !json.contains('\t'),
        "the canonical form must be compact, got: {json}"
    );
    assert!(json.starts_with("{\"name\":\""), "must start with the name");

    let positions: Vec<usize> = [
        "\"name\"",
        "\"author\"",
        "\"version\"",
        "\"colors\"",
        "\"spacing\"",
        "\"radii\"",
        "\"typography\"",
    ]
    .iter()
    .map(|key| {
        json.find(key)
            .unwrap_or_else(|| panic!("{key} missing from {json}"))
    })
    .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "top-level keys must be in schema order, got: {json}"
    );

    // And the colours inside them, which is the half a reader of the file sees.
    let colour_positions: Vec<usize> = COLOR_KEYS
        .iter()
        .map(|key| {
            json.find(&format!("\"{key}\":"))
                .unwrap_or_else(|| panic!("{key} missing"))
        })
        .collect();
    assert!(
        colour_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "colour keys must be in schema order too, got: {json}"
    );
}

/// `to_json` is byte-stable, so a diff between two serialisations means a change.
#[test]
fn the_canonical_json_is_byte_stable() {
    let theme = accepted(FIXTURE_VALID);
    assert_eq!(theme.to_json(), theme.to_json());
    assert_eq!(
        accepted(FIXTURE_VALID).to_json(),
        theme.to_json(),
        "two parses of the same document must serialise identically"
    );
}

/// A label containing JSON metacharacters survives the round trip.
///
/// The escaping in `to_json` is hand-written, so it is the part most likely to be
/// wrong in a way nothing else notices: a theme *name* with a quote in it is not
/// a name anybody writes, which is exactly why it needs a case. The second
/// argument is the JSON text, the first the label it must decode to.
#[rstest]
#[case::quote("a \"quoted\" name", "a \\\"quoted\\\" name")]
#[case::backslash("back\\slash", "back\\\\slash")]
#[case::newline("line\nbreak", "line\\nbreak")]
#[case::carriage_return("line\rreturn", "line\\rreturn")]
#[case::tab("a\ttab", "a\\ttab")]
#[case::control_bell("bell\u{7}", "bell\\u0007")]
#[case::literal_escape_sequence("literal \\n sequence", "literal \\\\n sequence")]
fn a_label_containing_json_metacharacters_round_trips(
    #[case] expected: &str,
    #[case] as_json: &str,
) {
    let theme = accepted(&with_value("name", &format!("\"{as_json}\"")));
    assert_eq!(theme.name(), expected, "the label must survive verbatim");
    assert_eq!(
        parse(theme.to_json().as_bytes()),
        Ok(theme),
        "and it must survive the canonical form too"
    );
}

// ---------------------------------------------------------------------------
// 9. The fallback
// ---------------------------------------------------------------------------

/// A valid document is loaded as itself, with no rejection to report.
#[test]
fn a_valid_document_is_loaded_as_itself() {
    let loaded = load_or_default(FIXTURE_VALID.as_bytes())
        .unwrap_or_else(|error| panic!("a valid document cannot fail: {error}"));
    assert!(
        matches!(loaded, ThemeLoad::Loaded(_)),
        "a valid document must not be a fallback, got {loaded:?}"
    );
    assert_eq!(loaded.theme(), &accepted(FIXTURE_VALID));
    assert!(loaded.rejection().is_none());
    assert!(loaded.message().is_none());
}

/// An invalid document yields the default theme *and* the reason -- the whole of
/// `AGENTS.md` §10.2.
///
/// Asserted as two things because §10.2 is two requirements: the app must not
/// break, and it must say why. A fallback that kept the theme and dropped the
/// error would pass the first half and fail the second, which is the bug this
/// case exists for.
#[test]
fn an_invalid_document_yields_the_default_and_the_reason() {
    let loaded = load_or_default(FIXTURE_INVALID.as_bytes())
        .unwrap_or_else(|error| panic!("a fallback must always produce a theme: {error}"));

    match &loaded {
        ThemeLoad::Fallback { theme, rejection } => {
            assert!(
                rejection.to_string().contains("colors.colour"),
                "the reason must be kept, and must be the real one, got: {rejection}"
            );
            assert_eq!(theme, &default_theme().unwrap_or(theme.clone()));
        }
        ThemeLoad::Loaded(theme) => {
            panic!("an invalid document must not be loaded as itself: {theme:?}")
        }
    }
    assert_eq!(
        loaded.message().map(|m| m.contains("colors.colour")),
        Some(true)
    );
}

/// The fallback reports the **real** reason, for every kind of rejection.
///
/// `AGENTS.md` §10.2 is two requirements -- fall back, *and* say why -- and the
/// second is the one a fallback can quietly break: a fallback that substitutes a
/// generic message renders correctly and tells the user nothing. The mutation
/// table caught that with a single test, so each kind of rejection gets its own
/// case here, asserting that the message names the *actual* offending field.
#[rstest]
#[case::unknown_key(&with_extra_key("\"colors\": {", "colour", "\"#5b8def\""), "colors.colour")]
#[case::malformed_colour(&with_value("colors.accent", "\"nope\""), "colors.accent")]
#[case::missing_key(&without_value("colors.text_muted"), "colors.text_muted")]
#[case::out_of_range(&with_value("spacing.lg", "9999"), "spacing.lg")]
#[case::blank_label(&with_value("name", "\"  \""), "name")]
#[case::bad_version(&with_value("version", "7"), "version")]
#[case::not_an_object("[]", "<document>")]
#[case::not_json("{{{", "not valid JSON")]
fn the_fallback_reports_the_real_reason_not_a_generic_one(
    #[case] document: &str,
    #[case] expected_in_message: &str,
) {
    let loaded = load_or_default(document.as_bytes())
        .unwrap_or_else(|error| panic!("a fallback must always produce a theme: {error}"));

    let message = loaded
        .message()
        .unwrap_or_else(|| panic!("an invalid document must produce a message: {document}"));
    assert!(
        message.contains(expected_in_message),
        "the fallback message must name {expected_in_message:?}, got: {message}"
    );
    // And the theme it fell back to is the default, not a partly-applied version
    // of the rejected document.
    assert_eq!(
        loaded.theme(),
        &default_theme().unwrap_or(loaded.theme().clone())
    );
}

/// A `ThemeLoad` always has a usable theme, whatever the bytes were.
///
/// This is the property that makes the fallback safe to call from a hot-reload
/// path, and it is the reason `ThemeLoad` is a type rather than a
/// `(Theme, Option<ThemeError>)` pair a caller could assemble wrongly.
///
/// Note what is *not* asserted: that the theme is the default. For a valid
/// document it is the document's own theme, and
/// `an_invalid_document_yields_the_default_and_the_reason` is where the
/// "invalid means default" claim lives.
#[rstest]
#[case::valid(FIXTURE_VALID)]
#[case::invalid(FIXTURE_INVALID)]
#[case::not_json("{{{")]
#[case::empty("")]
#[case::array("[]")]
#[case::oversized("xxxx")]
fn a_load_always_produces_a_usable_theme(#[case] document: &str) {
    let loaded = load_or_default(document.as_bytes())
        .unwrap_or_else(|error| panic!("{document:?} produced no theme: {error}"));
    let theme = loaded.theme();

    assert_eq!(theme.version(), THEME_FORMAT_VERSION);
    assert!(!theme.name().is_empty(), "a loaded theme must be named");
    assert_eq!(
        theme.colors().accent.to_string().len(),
        7,
        "and must carry a resolved colour, not an empty one"
    );
}

/// The rejection reaches the project-wide error type as `Theme`, with §3.3's text.
///
/// This is the other half of §5 of the module docs: `core/` cannot name
/// `ShNexusError`, so `errors.rs` carries the conversion, and this is the test
/// that the conversion exists and produces the specified `Display`.
#[test]
fn a_theme_rejection_converts_into_the_project_error_type() {
    let rejection = rejection_of(FIXTURE_INVALID);
    let expected = rejection.to_string();
    let converted: ShNexusError = rejection.into();

    assert_eq!(converted.to_string(), format!("theme error: {expected}"));
    assert!(
        matches!(&converted, ShNexusError::Theme(_)),
        "a theme rejection must land in the Theme variant, got {converted:?}"
    );

    // And it is a real error a caller can box, which is the point of the type.
    let boxed: Box<dyn std::error::Error> = Box::new(converted);
    assert!(boxed.to_string().starts_with("theme error: "));
}

// ---------------------------------------------------------------------------
// 10. Properties
// ---------------------------------------------------------------------------

/// A JSON colour, generated rather than filtered.
fn arbitrary_color() -> impl Strategy<Value = String> {
    (0u8..=255, 0u8..=255, 0u8..=255)
        .prop_map(|(red, green, blue)| format!("#{red:02x}{green:02x}{blue:02x}"))
}

/// An integer anywhere in a numeric field's accepted range, biased to the bounds.
///
/// The bias matters: uniform sampling over `0..=256` would hit an exact boundary
/// roughly once in 250 cases, and the boundary is where an off-by-one in a
/// comparison operator lives.
fn arbitrary_number(min: u32, max: u32) -> impl Strategy<Value = u32> {
    prop_oneof![Just(min), Just(max), min..=max]
}

/// A non-blank label, including the characters `to_json` has to escape.
///
/// The generated alternative is anchored on a leading letter rather than being
/// `"{1,24}"`, and that is load-bearing rather than cosmetic: the schema refuses
/// a whitespace-only label, so a generator that could produce `"   "` would spend
/// its budget generating documents the parser is *right* to reject -- and the
/// property would fail for the correct reason with a misleading message. This was
/// found that way, which is worth recording.
fn arbitrary_label() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Inter".to_owned()),
        Just("a \"quoted\" name".to_owned()),
        Just("back\\slash".to_owned()),
        Just("new\nline".to_owned()),
        Just("tab\there".to_owned()),
        // Control characters that are NOT `\n`, `\r` or `\t`. Those three have
        // their own arms in `to_json`, so a generator that produced only those
        // would never reach the generic `< 0x20` arm -- and mutation M9 (removing
        // that arm) was caught by exactly one test until these were added.
        Just("bell\u{7}here".to_owned()),
        Just("unit\u{1}separator".to_owned()),
        "[a-zA-Z][a-zA-Z ]{0,23}",
    ]
}

/// The four spacing steps, in schema order.
fn arbitrary_spacing() -> impl Strategy<Value = [u32; 4]> {
    (
        arbitrary_number(MIN_SPACING, MAX_SPACING),
        arbitrary_number(MIN_SPACING, MAX_SPACING),
        arbitrary_number(MIN_SPACING, MAX_SPACING),
        arbitrary_number(MIN_SPACING, MAX_SPACING),
    )
        .prop_map(|(xs, sm, md, lg)| [xs, sm, md, lg])
}

/// The three corner radii, in schema order.
fn arbitrary_radii() -> impl Strategy<Value = [u32; 3]> {
    (
        arbitrary_number(MIN_RADIUS, MAX_RADIUS),
        arbitrary_number(MIN_RADIUS, MAX_RADIUS),
        arbitrary_number(MIN_RADIUS, MAX_RADIUS),
    )
        .prop_map(|(sm, md, lg)| [sm, md, lg])
}

/// The four font sizes, in schema order.
fn arbitrary_sizes() -> impl Strategy<Value = [u32; 4]> {
    (
        arbitrary_number(MIN_FONT_SIZE, MAX_FONT_SIZE),
        arbitrary_number(MIN_FONT_SIZE, MAX_FONT_SIZE),
        arbitrary_number(MIN_FONT_SIZE, MAX_FONT_SIZE),
        arbitrary_number(MIN_FONT_SIZE, MAX_FONT_SIZE),
    )
        .prop_map(|(caption, timestamp, body, title)| [caption, timestamp, body, title])
}

/// What a generated document encodes, so a property can check it came back.
///
/// A struct rather than a tuple so the assertions read `expected.accent` instead
/// of `.17`, which is the difference between a failure message a reader can act on
/// and one they have to count.
#[derive(Debug, Clone)]
struct Encoded {
    name: String,
    author: String,
    family: String,
    colors: Vec<(&'static str, String)>,
    spacing: [u32; 4],
    radii: [u32; 3],
    sizes: [u32; 4],
}

/// JSON-escapes a label so it can be embedded in a generated document.
///
/// **Hand-written, and deliberately not `Theme::to_json`'s escaper.** The whole
/// point of the properties in this file is that the generator and the code under
/// test are independent; sharing one escaper would let a bug in it cancel itself
/// out, which is the failure mode §4.4's properties are supposed to exclude.
///
/// It also encodes differently on purpose: this one writes every control
/// character as `\\u00XX`, while `to_json` writes the short forms `\\n` and `\\t`.
/// Both are valid JSON, so a round trip through both proves the parser accepts an
/// encoding the serializer never emits -- which no hand-written case would do.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out
}

/// A complete valid document, built as JSON text independently of `to_json`.
fn arbitrary_valid_document() -> impl Strategy<Value = (String, Encoded)> {
    (
        arbitrary_label(),
        arbitrary_label(),
        proptest::collection::vec(arbitrary_color(), 13),
        arbitrary_spacing(),
        arbitrary_radii(),
        arbitrary_label(),
        arbitrary_sizes(),
    )
        .prop_map(|(name, author, colors, spacing, radii, family, sizes)| {
            let pairs: Vec<(&'static str, String)> =
                COLOR_KEYS.iter().copied().zip(colors).collect();

            // Built by `push_str` rather than one `format!` with fifteen
            // positional placeholders: the brace escaping in that string is
            // counted by eye, and it was wrong twice while this was written. The
            // document *is* the fixture's shape, so writing it out is clearer
            // than compressing it.
            let mut document = String::with_capacity(1024);
            document.push_str("{\"name\":\"");
            document.push_str(&json_escape(&name));
            document.push_str("\",\"author\":\"");
            document.push_str(&json_escape(&author));
            document.push_str("\",\"version\":1,\"colors\":{");
            for (index, (key, value)) in pairs.iter().enumerate() {
                if index > 0 {
                    document.push(',');
                }
                document.push('"');
                document.push_str(key);
                document.push_str("\":\"");
                document.push_str(value);
                document.push('"');
            }
            document.push_str("},\"spacing\":{");
            for (index, (key, value)) in SPACING_KEYS.iter().zip(spacing).enumerate() {
                if index > 0 {
                    document.push(',');
                }
                document.push_str(&format!("\"{key}\":{value}"));
            }
            document.push_str("},\"radii\":{");
            for (index, (key, value)) in RADIUS_KEYS.iter().zip(radii).enumerate() {
                if index > 0 {
                    document.push(',');
                }
                document.push_str(&format!("\"{key}\":{value}"));
            }
            document.push_str("},\"typography\":{\"family\":\"");
            document.push_str(&json_escape(&family));
            document.push_str("\",\"sizes\":{");
            for (index, (key, value)) in FONT_SIZE_KEYS.iter().zip(sizes).enumerate() {
                if index > 0 {
                    document.push(',');
                }
                document.push_str(&format!("\"{key}\":{value}"));
            }
            document.push_str("}}}");

            (
                document,
                Encoded {
                    name,
                    author,
                    family,
                    colors: pairs,
                    spacing,
                    radii,
                    sizes,
                },
            )
        })
}

/// Bytes the parser has no business accepting: arbitrary noise, both fixtures, and
/// the valid fixture with one byte corrupted.
///
/// The third is the interesting one. Random bytes almost all die at the JSON
/// grammar, so a "never panics" property over them would exercise roughly one line
/// of the parser. Corrupting a byte inside a *valid* document keeps it valid JSON
/// in most cases and reaches the field checks, which is where a panic would
/// actually live.
fn arbitrary_input() -> impl Strategy<Value = Vec<u8>> {
    let mutated = (0usize..FIXTURE_VALID.len(), any::<u8>()).prop_map(|(index, byte)| {
        let mut bytes = FIXTURE_VALID.as_bytes().to_vec();
        if let Some(slot) = bytes.get_mut(index) {
            *slot = byte;
        }
        bytes
    });

    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..512),
        mutated,
        Just(b"{}".to_vec()),
        Just(FIXTURE_VALID.as_bytes().to_vec()),
        Just(FIXTURE_INVALID.as_bytes().to_vec()),
    ]
}

// `AGENTS.md` §4.4: parsing never panics for arbitrary input.
//
// The property is stated over arbitrary bytes rather than over "small mutations
// of a valid document", because the failure it guards is a crash in a chat client
// parsing a file somebody downloaded. `load_or_default` is in the same property
// because the fallback is the path that runs on untrusted input, and the
// accompanying assertion is real rather than decorative: a load that failed would
// mean the app has no theme at all, which §10.2 forbids.
//
// A `//` comment rather than a `///` one because `proptest!` is a macro
// invocation, and a doc comment in front of one attaches to nothing -- which
// `unused doc comment` says out loud.
proptest! {
    #[test]
    fn arbitrary_input_never_panics(input in arbitrary_input()) {
        let _ = parse(&input);
        let loaded = load_or_default(&input);
        prop_assert!(
            loaded.is_ok(),
            "load_or_default must always yield a theme, got {:?}",
            loaded.err()
        );
        if let Ok(loaded) = loaded {
            prop_assert_eq!(loaded.theme().version(), THEME_FORMAT_VERSION);
        }
    }
}

// A valid theme always parses, and every value survives.
//
// The property §4.4 asks for, stated in the direction that catches a shared
// assumption: **the generator decides the values and `parse` must return them.** A
// property that fed `to_json`'s output back into `parse` would pass even if both
// were wrong about a bound.
proptest! {
    #[test]
    fn a_generated_valid_theme_always_parses_to_what_it_encoded(
        (document, expected) in arbitrary_valid_document()
    ) {
        let outcome = parse(document.as_bytes());
        prop_assert!(
            outcome.is_ok(),
            "a generated valid document was rejected: {}\n{}",
            outcome.as_ref().err().map(ToString::to_string).unwrap_or_default(),
            document
        );
        if let Ok(theme) = outcome {
            prop_assert_eq!(theme.name(), &expected.name[..]);
            prop_assert_eq!(theme.author(), &expected.author[..]);
            prop_assert_eq!(theme.typography().family.as_str(), expected.family.as_str());

            for (key, wanted) in &expected.colors {
                prop_assert!(
                    theme.colors().get(key).map(|c| c.to_string()).as_ref() == Some(wanted),
                    "colors.{} did not survive the parse: wanted {}, got {:?}",
                    key,
                    wanted,
                    theme.colors().get(key).map(|c| c.to_string())
                );
            }
            prop_assert_eq!(theme.spacing().xs, expected.spacing[0]);
            prop_assert_eq!(theme.spacing().sm, expected.spacing[1]);
            prop_assert_eq!(theme.spacing().md, expected.spacing[2]);
            prop_assert_eq!(theme.spacing().lg, expected.spacing[3]);
            prop_assert_eq!(theme.radii().sm, expected.radii[0]);
            prop_assert_eq!(theme.radii().md, expected.radii[1]);
            prop_assert_eq!(theme.radii().lg, expected.radii[2]);
            prop_assert_eq!(theme.typography().sizes.caption, expected.sizes[0]);
            prop_assert_eq!(theme.typography().sizes.timestamp, expected.sizes[1]);
            prop_assert_eq!(theme.typography().sizes.body, expected.sizes[2]);
            prop_assert_eq!(theme.typography().sizes.title, expected.sizes[3]);
        }
    }
}

// A generated valid theme also survives the canonical form.
//
// The second half of the round-trip, and the one that covers `to_json` on inputs
// no hand-written case would produce: 0 spacing, 512 radius, 1pt body text, and
// labels carrying quotes, backslashes and newlines.
proptest! {
    #[test]
    fn a_generated_valid_theme_survives_the_canonical_json(
        (document, _) in arbitrary_valid_document()
    ) {
        if let Ok(theme) = parse(document.as_bytes()) {
            let json = theme.to_json();
            let round_tripped = parse(json.as_bytes());
            prop_assert!(
                round_tripped == Ok(theme),
                "the canonical form did not round-trip:\n{}",
                json
            );
        }
    }
}
