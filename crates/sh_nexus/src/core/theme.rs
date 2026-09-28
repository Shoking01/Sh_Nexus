//! Theme parsing, schema validation, and the three built-in themes.
//!
//! This is the module `AGENTS.md` §3.1 names as *"Theme parsing, validation,
//! application"* and `PLAN.md` §4 draws as `theme.rs   # parse, validate, apply
//! (parsing only — no file I/O)`.
//!
//! # 1. The boundary, and what is deliberately not here
//!
//! [`parse`] takes bytes and returns a [`Theme`] or a [`ThemeError`]. It never
//! asks where those bytes came from. `PLAN.md` §4 makes that explicit --
//! *"Theme file watching therefore lives in `platform/file_watch.rs`, and
//! `core/theme.rs` only parses and validates bytes it is handed"* -- and the
//! reason is the same reason the rest of this layer is pure: `AGENTS.md` §4.1
//! sets a ≥90% coverage floor for `core/`, and a layer that opens a file cannot
//! be tested hermetically at that rate without a filesystem in every test.
//!
//! So three things §10.2 asks for are **not** here, and each has a named home:
//!
//! | `AGENTS.md` §10.2 clause | Where it lives |
//! |---|---|
//! | parse and validate a theme file's bytes | **here** |
//! | discovery under `~/.config/sh_nexus/themes/` | `platform/file_watch.rs` |
//! | hot reload on change | `platform/file_watch.rs` (the `notify` watcher) |
//! | invalid themes fall back with an in-app message | [`load_or_default`], **here** |
//! | at least 3 built-in themes in the binary | [`BuiltIn`], **here** |
//!
//! "Apply" -- handing resolved values to the renderer -- is [`Theme`]'s job and
//! not this module's, because applying means calling GPUI, and §3.2 forbids that
//! here. What this module provides instead is [`Color::components`], so
//! `ui/theme/` never has to re-parse a hex string.
//!
//! # 2. No derives: why the parser is hand-written
//!
//! [`Theme`] and every type below it **derive nothing**. There is no
//! `#[derive(Deserialize)]`, and `core/theme.rs` does not name `serde` at all --
//! only `serde_json`, whose *parser* is admitted to `core/` by
//! `crates/sh_nexus/tests/layer_boundary.rs` and by nothing else.
//!
//! The reason is `PLAN.md` §5: serialization belongs to `sh_nexus_wire`, and
//! the boundary is asserted by `core_models_derives_no_serde_traits`, which
//! fails the build if `core/models` so much as mentions `Serialize`. A theme
//! file is not the domain model, but a derive on a `core/` type is the same
//! failure one layer out: the JSON shape and the Rust shape become one type, so
//! a key can never be added, renamed or reordered without changing the struct
//! the renderer reads. Here they are separate, and the key list in
//! [`COLOR_KEYS`] is the honest description of the wire format.
//!
//! [`parse`] therefore does what the derives would have done, one field at a
//! time, with the type of every field checked by hand. That is more code, and
//! the trade is deliberate: **the error message is the product.** §10.2
//! requires an invalid theme to produce an in-app message, and a derive's
//! message for a bad theme is `missing field 'accent'` -- which does not say
//! *where*, does not distinguish a typo from a type error, and cannot name the
//! value that was wrong. Every message from this module names the exact JSON
//! path (`colors.accent`, `typography.sizes.body`) and the exact reason.
//!
//! # 3. The strictness policy, and what it costs
//!
//! Three rules, each of which rejects something a lenient parser would accept:
//!
//! 1. **Unknown keys are an error**, not a shrug. `"colour": "#fff"` in place of
//!    `"accent"` is the single most likely mistake in a hand-edited theme, and
//!    the lenient outcome -- ignore it, use the default accent, look wrong with
//!    no explanation -- is precisely the failure §10.2's error message exists to
//!    prevent.
//! 2. **Colours are exactly `#rrggbb`.** Not `#rgb`, not `#rrggbbaa`, not
//!    `red`. Each of those three is rejected on purpose: expanding `#abc`
//!    silently reinterprets bytes, dropping an alpha channel silently loses
//!    information the schema has no slot for, and a colour name needs a
//!    name-to-components table the schema does not have. Lower case and upper
//!    case hex digits are both accepted, because a text editor's colour picker
//!    produces either, and [`Color`]'s canonical form is lower case.
//! 3. **`version` must be exactly [`THEME_FORMAT_VERSION`].** §7.4 requires an
//!    unknown major version to be rejected explicitly rather than interpreted,
//!    and a build that accepted `version: 2` would be reading fields whose
//!    meaning the author had already changed.
//!
//! **What rule 1 costs, stated rather than hidden:** this schema has no forward
//! compatibility. A theme carrying a key this build has never heard of is
//! rejected outright, not partially applied -- so a future release that adds a
//! key breaks every older client that opens the new theme. That is a real cost
//! and it is accepted for one reason: the alternative failure has **no symptom
//! at all**. A partially applied theme looks like a rendering bug, whereas a
//! rejected one produces a message naming the key, and §10.2's recovery is one
//! keystroke from a message that says what is wrong. An error the user can read
//! beats a theme that renders wrong.
//!
//! # 4. The numeric limits, and why each bound is the bound
//!
//! Every number in the schema is an integer, and each family has a documented
//! range. The policy underneath all three is: **reject what the renderer cannot
//! recover from, and do not enforce taste.** A validator that rejected
//! `spacing.xs = 2` because it disliked the rhythm would be a validator nobody
//! could satisfy.
//!
//! | Field | Range | Why that bound and not another |
//! |---|---|---|
//! | `spacing.*` | [`MIN_SPACING`]–[`MAX_SPACING`] | **0 is legal**, because a deliberate flush layout is a real design decision and a validator that forbade it would forbid a legitimate theme. The ceiling is a screen: a spacing larger than a display is a typo, not a scale. |
//! | `radii.*` | [`MIN_RADIUS`]–[`MAX_RADIUS`] | **0 is legal and meaningful** -- square corners -- and the high-contrast built-in uses it. A *negative* radius is the one value here that is not a matter of taste: it describes no corner, and it is the case a `u32` cannot hold, so it is rejected by the type as well as by the range. The ceiling is arbitrary but bounded. |
//! | `typography.sizes.*` | [`MIN_FONT_SIZE`]–[`MAX_FONT_SIZE`] | **0 is rejected and this is a design decision, not a convention.** A zero font size has zero extent: the text cannot be measured, cannot be hit-tested and cannot be read, and unlike a zero spacing it is never anybody's design. 1 is the floor, not 12: the schema expresses no legibility policy, and inventing one here would reject themes a user can read perfectly well. |
//!
//! [`MAX_THEME_BYTES`] bounds the document itself, and [`MAX_LABEL_CHARS`]
//! bounds `name`, `author` and `typography.family`. Both are §7.1's *"no
//! unbounded growth of in-memory state"*: a theme file is user-authored, so its
//! size is not a property this layer can assume, and a hot-reloadable file that
//! can be 60KB is a file that can be 60GB.
//!
//! # 5. Why the error is not `ShNexusError`
//!
//! [`ThemeError`] is this module's own type, and that is a constraint rather
//! than a preference. `errors.rs` states it in its own module documentation:
//! *"A `core/` function that needs to report 'this is not a valid message' does
//! not return a `ShNexusError`; it returns a `bool`, an `Option`, or a narrower
//! error its own module defines -- §3.3's 'each module may define narrower error
//! types that convert into `ShNexusError`' is the sanctioned direction of travel,
//! not the reverse."* `core_reaches_only_its_own_modules` in
//! `crates/sh_nexus/tests/layer_boundary.rs` enforces it, and it is a test this
//! project does not relax.
//!
//! The conversion still happens, in the direction §3.3 prescribes:
//! `errors.rs` carries `impl From<ThemeError> for ShNexusError`, which lands in
//! `ShNexusError::Theme` -- the variant `errors_display.rs` already pins. So a
//! caller outside `core/` sees exactly the one-line error type §3.3 specifies,
//! and this module stays a leaf.
//!
//! # 6. The fallback, and why it is a `Result` rather than an `unwrap`
//!
//! §10.2 requires an invalid theme to fall back to the default. [`load_or_default`]
//! does that, and it returns the rejection alongside the theme so the caller can
//! show it -- a fallback that silently swallowed the reason would satisfy §10.2
//! and defeat its other half.
//!
//! It returns `Result<ThemeLoad, ThemeError>` for a reason that is worth being
//! explicit about. Its `Err` arm is **unreachable in any binary this repository
//! builds**: the default theme is an `include_str!` constant, and
//! `every_built_in_theme_parses_and_validates` in `tests/theme.rs` fails the
//! build if any of the three stops parsing. The arm exists anyway, and the
//! alternatives were both worse:
//!
//! - `unwrap()` on the default's parse is banned by §2.1, and it is precisely
//!   the wrong answer in the one case that can cause it -- a build-time fixture
//!   corrupted by a bad merge should degrade, not crash the process at startup.
//! - A hand-written "emergency" theme would duplicate the default's data and
//!   introduce a second place to update when a colour changes, for a case that
//!   cannot happen.
//!
//! **So the dead arm is the honest shape, and it is guarded by a test rather
//! than by a comment.** That also makes it one of the few uncovered regions in
//! the file, and `docs/COVERAGE.md` records it as such rather than hiding it.
//!
//! # 7. Sentinels: absent is not a value
//!
//! `core/cache.rs` §13's thread-safety contract rests on a budget typed
//! `Option<u64>` rather than a sentinel, because `Some(0)` and `None` are
//! different things and a `0` sentinel would have made them the same. The same
//! reasoning runs through this module, in three places, and it is the reason
//! several APIs here return `u32` or `&str` where a "default" would have been
//! easier:
//!
//! - **A missing key is an error, not a default colour.** There is no
//!   "substitute `accent` with the previous theme's accent" path, because a
//!   half-applied theme is indistinguishable from a broken one. [`Palette`]
//!   therefore has thirteen [`Color`] fields rather than a map with a fallback.
//! - **A lookup returns the value, not an `Option`.** [`Color`] is a newtype
//!   over three `u8`, and `#000000` is a legitimate colour -- the high-contrast
//!   built-in's `background` is exactly that. A sentinel-based "black means
//!   missing" convention would have made the darkest theme in the set
//!   indistinguishable from an absent key.
//! - **`version` is a number, not a flag.** Zero is not "unversioned, assume
//!   one"; it is a version this build does not speak, and
//!   [`ThemeError::UnsupportedVersion`] says so.
//!
//! # 8. Round-tripping, and what `to_json` is for
//!
//! [`Theme::to_json`] emits the canonical form: no whitespace, keys in schema
//! order, colours in lower-case hex. It exists for two reasons, both
//! test-related and neither decorative:
//!
//! 1. **It is the property test's input.** A proptest that generates a legal
//!    [`Theme`] and requires [`parse`] to accept its serialisation is what
//!    proves the bounds in §4 are *reachable* rather than merely documented --
//!    `MAX_FONT_SIZE` is a real number only if a theme may actually use it.
//! 2. **It is a schema-drift detector.** `parse(built_in)` round-tripped through
//!    `to_json` and back must be identical, so a field added to one and not the
//!    other fails a test.
//!
//! It is written by hand rather than derived, for §2's reason: a derived
//! serializer would emit whatever the struct happened to be, which is the
//! opposite of a canonical form.
//!
//! # 9. A known limit of the schema, recorded rather than papered over
//!
//! `AGENTS.md` §10.1 has **no slot for a border, a border colour, or an alpha
//! channel**, and a theme therefore cannot express a 1px outline. The
//! consequence is real and it lands on the high-contrast built-in: with a pure
//! black `background`, a message bubble has to be separated from the page by
//! fill alone, and WCAG 1.4.11's 3:1 non-text contrast is not reachable for the
//! lightest bubble fill that still reads as a surface. The values in
//! `themes/high-contrast.json` are the best separation available without a
//! border slot, and **text-on-surface contrast exceeds 8:1 everywhere in it**,
//! which is the half of the requirement that a fill-only palette can meet.
//! `docs/API.md` records this as a schema defect to resolve by amending §10.1
//! through the process `docs/ARCHITECTURE.md`'s Appendix describes -- not by
//! quietly adding a key here, which would be a version-1 schema change made
//! without the version bump rule 3 of §3 exists to require.

use std::fmt;

use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Schema constants
// ---------------------------------------------------------------------------

/// The path an error carries when the problem is the document itself.
///
/// Every other path in this module is a dotted JSON path (`colors.accent`). The
/// root has no key to be named by, so it is named by this instead of by the
/// empty string, which would render as `` `` is required `` in a message.
pub const DOCUMENT: &str = "<document>";

/// The only theme format version this build speaks.
///
/// Pinned rather than ranged: §7.4 requires an unknown major version to be
/// rejected explicitly, and a `version` field this module treats as
/// informational is a field that will eventually be read wrongly.
pub const THEME_FORMAT_VERSION: u32 = 1;

/// Longest a `name`, `author` or `typography.family` may be, in characters.
///
/// A theme label is rendered in one line of one control. 128 characters is
/// already longer than any such line, and §7.1's *"no unbounded growth of
/// in-memory state"* is the reason the bound exists rather than the layout.
pub const MAX_LABEL_CHARS: usize = 128;

/// Longest a theme document may be, in bytes.
///
/// A complete theme -- thirteen colours, eleven numbers, three short strings --
/// is under one kilobyte. This leaves roughly sixty times that for comments,
/// whitespace and multi-byte text, which is generous, and still bounds what a
/// hot-reloadable file can cost the allocator.
pub const MAX_THEME_BYTES: usize = 64 * 1024;

/// Longest malformed value echoed back inside an error message.
///
/// An error message is something a user reads and something a log records
/// (§7.5). A theme file is user-authored, so a 60KB string in the wrong place
/// must not become a 60KB line in either.
pub const MAX_ECHOED_CHARS: usize = 40;

/// Smallest accepted spacing, in logical pixels. **Zero is legal**: see §4.
pub const MIN_SPACING: u32 = 0;

/// Largest accepted spacing, in logical pixels.
///
/// One display is already far larger than any spacing a layout can use, so a
/// value above this is a typo rather than a scale.
pub const MAX_SPACING: u32 = 256;

/// Smallest accepted corner radius, in logical pixels. **Zero is legal** --
/// square corners -- and the high-contrast built-in uses it.
pub const MIN_RADIUS: u32 = 0;

/// Largest accepted corner radius, in logical pixels.
///
/// A radius larger than the element it rounds is drawn as a stadium shape by the
/// renderer, so beyond this the number is describing nothing.
pub const MAX_RADIUS: u32 = 512;

/// Smallest accepted font size, in logical pixels. **Zero is rejected**: a zero
/// extent is not a design, it is a value that cannot be rendered. See §4.
pub const MIN_FONT_SIZE: u32 = 1;

/// Largest accepted font size, in logical pixels.
///
/// Generous by a factor of twenty over any UI text, and bounded because the
/// theme file is not a trusted input.
pub const MAX_FONT_SIZE: u32 = 512;

/// The keys of `colors`, in schema order.
///
/// This is the whole unknown-key allow-list for `colors`, and
/// `every_colour_role_is_pinned_to_exactly_one_key` in `tests/theme.rs` is what
/// keeps it honest against [`Palette`].
pub const COLOR_KEYS: [&str; 13] = [
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
];

/// The keys of `spacing`, in schema order.
pub const SPACING_KEYS: [&str; 4] = ["xs", "sm", "md", "lg"];

/// The keys of `radii`, in schema order.
pub const RADIUS_KEYS: [&str; 3] = ["sm", "md", "lg"];

/// The keys of `typography.sizes`, in schema order.
pub const FONT_SIZE_KEYS: [&str; 4] = ["caption", "timestamp", "body", "title"];

/// The keys of the document root, in schema order.
pub const THEME_KEYS: [&str; 7] = [
    "name",
    "author",
    "version",
    "colors",
    "spacing",
    "radii",
    "typography",
];

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

/// One colour, as three 8-bit channels.
///
/// A newtype rather than a `String` so that **no unvalidated colour can exist**:
/// the only ways to obtain one are [`Color::from_hex`], which enforces
/// [`COLOR_HEX_FORMAT`], and [`Color::from_rgb`], which needs no validation
/// because every triple of `u8` is a colour.
///
/// # Example
///
/// ```
/// use sh_nexus::core::theme::Color;
///
/// // Both hex cases, canonicalised to lower.
/// let colour = Color::from_hex("#1E1E2E");
/// assert_eq!(colour.map(|c| c.to_string()).as_deref(), Some("#1e1e2e"));
///
/// // Every rejection this module promises, in one place.
/// for rejected in ["1e1e2e", "#fff", "#1e1e2e00", "red", "#gggggg", "#1e 1e2e", ""] {
///     assert!(Color::from_hex(rejected).is_none(), "{rejected} should be rejected");
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    red: u8,
    green: u8,
    blue: u8,
}

impl Color {
    /// The only accepted textual form, stated as documentation rather than as a
    /// regex nobody can see: a `#` and exactly six hexadecimal digits.
    pub const COLOR_HEX_FORMAT: &'static str = "#rrggbb";

    /// Parses [`Color::COLOR_HEX_FORMAT`], or returns `None`.
    ///
    /// Both cases of hex digit are accepted, because a text editor's colour
    /// picker produces either; [`Display`] emits the lower-case canonical form.
    ///
    /// The `is_ascii_hexdigit` pass before the radix parse is **not
    /// redundant**: `u8::from_str_radix` accepts a leading `+` or `-`, so
    /// `"#+f1e2e"` would otherwise parse. Rejecting every byte that is not a hex
    /// digit closes that, and rejects whitespace, `+`, `-` and any non-ASCII
    /// character in the same pass.
    pub fn from_hex(value: &str) -> Option<Self> {
        let digits = value.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }

        let mut channels = [0u8; 3];
        for (index, channel) in channels.iter_mut().enumerate() {
            let start = index * 2;
            // `get` rather than indexing: the length check above makes this
            // always `Some`, and a bounds check that cannot fail is still a
            // panic path this module may not have (AGENTS.md 2.1).
            let pair = digits.get(start..start + 2)?;
            *channel = u8::from_str_radix(pair, 16).ok()?;
        }

        Some(Self {
            red: channels[0],
            green: channels[1],
            blue: channels[2],
        })
    }

    /// Builds a colour from three channels.
    ///
    /// Public because `ui/theme/` derives shades from a theme's own colours, and
    /// validation-free because there is nothing to validate: `#000000` and
    /// `#ffffff` are both real colours, and neither is a sentinel.
    pub fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// The three channels, in order.
    ///
    /// The seam into `ui/theme/`: GPUI wants normalised floats, this module
    /// refuses to know that (§3.2), so the conversion happens where GPUI is
    /// already named.
    pub fn components(self) -> [u8; 3] {
        [self.red, self.green, self.blue]
    }

    /// The red channel.
    pub fn red(self) -> u8 {
        self.red
    }

    /// The green channel.
    pub fn green(self) -> u8 {
        self.green
    }

    /// The blue channel.
    pub fn blue(self) -> u8 {
        self.blue
    }
}

impl fmt::Display for Color {
    /// The canonical form, which [`parse`] accepts back unchanged.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "#{:02x}{:02x}{:02x}",
            self.red, self.green, self.blue
        )
    }
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// A validated theme.
///
/// **The only ways to obtain one are [`parse`], [`BuiltIn::theme`] and
/// [`load_or_default`].** The fields are private for that reason: a `Theme` that
/// could be assembled field-by-field would be a `Theme` no validator ever saw.
/// The four sub-structs' fields *are* public, because reading a value out of a
/// `Theme` needs no validation and hiding it would only add accessors that
/// forward to nothing.
///
/// # Example
///
/// ```
/// use sh_nexus::core::theme::BuiltIn;
///
/// let dark = BuiltIn::Dark.theme();
/// assert!(dark.is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    name: String,
    author: String,
    version: u32,
    colors: Palette,
    spacing: Spacing,
    radii: Radii,
    typography: Typography,
}

impl Theme {
    /// The theme's display name, never blank and never longer than
    /// [`MAX_LABEL_CHARS`].
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Who wrote the theme. Same validation as [`Theme::name`].
    pub fn author(&self) -> &str {
        &self.author
    }

    /// The format version, always [`THEME_FORMAT_VERSION`].
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The thirteen colours.
    pub fn colors(&self) -> &Palette {
        &self.colors
    }

    /// The four spacing steps.
    pub fn spacing(&self) -> &Spacing {
        &self.spacing
    }

    /// The three corner radii.
    pub fn radii(&self) -> &Radii {
        &self.radii
    }

    /// The font family and the four size steps.
    pub fn typography(&self) -> &Typography {
        &self.typography
    }

    /// The theme as canonical JSON: no whitespace, keys in schema order, colours
    /// in lower-case hex.
    ///
    /// `parse(theme.to_json())` is `theme` for every theme this module can
    /// produce; `docs/COVERAGE.md` §5.6 records the mutation that checks it.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::theme::{parse, BuiltIn};
    ///
    /// let dark = BuiltIn::Dark.theme();
    /// assert!(dark.is_ok());
    /// let theme = match dark {
    ///     Ok(theme) => theme,
    ///     Err(error) => panic!("the dark built-in must validate: {error}"),
    /// };
    ///
    /// let json = theme.to_json();
    /// assert_eq!(parse(json.as_bytes()), Ok(theme));
    /// ```
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str("{\"name\":");
        push_json_string(&mut out, &self.name);
        out.push_str(",\"author\":");
        push_json_string(&mut out, &self.author);
        out.push_str(",\"version\":");
        out.push_str(&self.version.to_string());
        out.push_str(",\"colors\":{");
        for (index, (key, value)) in [
            ("background", self.colors.background),
            ("surface", self.colors.surface),
            ("sidebar", self.colors.sidebar),
            ("text", self.colors.text),
            ("text_muted", self.colors.text_muted),
            ("accent", self.colors.accent),
            ("accent_hover", self.colors.accent_hover),
            ("danger", self.colors.danger),
            ("success", self.colors.success),
            ("mention", self.colors.mention),
            ("code_block_bg", self.colors.code_block_bg),
            ("bubble_self", self.colors.bubble_self),
            ("bubble_other", self.colors.bubble_other),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                out.push(',');
            }
            push_json_string(&mut out, key);
            out.push(':');
            push_json_string(&mut out, &value.to_string());
        }
        out.push_str("},\"spacing\":{");
        for (index, (key, value)) in [
            ("xs", self.spacing.xs),
            ("sm", self.spacing.sm),
            ("md", self.spacing.md),
            ("lg", self.spacing.lg),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                out.push(',');
            }
            push_json_string(&mut out, key);
            out.push(':');
            out.push_str(&value.to_string());
        }
        out.push_str("},\"radii\":{");
        for (index, (key, value)) in [
            ("sm", self.radii.sm),
            ("md", self.radii.md),
            ("lg", self.radii.lg),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                out.push(',');
            }
            push_json_string(&mut out, key);
            out.push(':');
            out.push_str(&value.to_string());
        }
        out.push_str("},\"typography\":{\"family\":");
        push_json_string(&mut out, &self.typography.family);
        out.push_str(",\"sizes\":{");
        for (index, (key, value)) in [
            ("caption", self.typography.sizes.caption),
            ("timestamp", self.typography.sizes.timestamp),
            ("body", self.typography.sizes.body),
            ("title", self.typography.sizes.title),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                out.push(',');
            }
            push_json_string(&mut out, key);
            out.push(':');
            out.push_str(&value.to_string());
        }
        out.push_str("}}}");
        out
    }
}

/// The thirteen colours a theme must define.
///
/// Named fields rather than a map, and the reason is §7 of the module docs: a
/// map would need a "missing key means use this default" rule, and a
/// half-applied theme has no symptom. With fields, a missing key is a
/// [`ThemeError::MissingKey`] naming it, and adding a key is a compile error in
/// [`palette_from`] and in [`COLOR_KEYS`] together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The window background, behind everything.
    pub background: Color,
    /// Raised panels: the message list, cards, menus.
    pub surface: Color,
    /// The channel rail.
    pub sidebar: Color,
    /// Body text on `background` and `surface`.
    pub text: Color,
    /// Secondary text: timestamps, counts, placeholders.
    pub text_muted: Color,
    /// The interactive colour: focus rings, links, the send affordance.
    pub accent: Color,
    /// `accent` under the pointer. The schema has no derived colours, so a theme
    /// states this one explicitly.
    pub accent_hover: Color,
    /// Destructive actions, and the failed-send state.
    pub danger: Color,
    /// Confirmations, presence, and the delivered state.
    pub success: Color,
    /// A message that names the reader.
    pub mention: Color,
    /// Behind a fenced code block, which is legible on `surface`.
    pub code_block_bg: Color,
    /// Behind the reader's own messages.
    pub bubble_self: Color,
    /// Behind everyone else's messages.
    pub bubble_other: Color,
}

impl Palette {
    /// The colour for a key held as *data*, or `None` if the key is not one of
    /// the thirteen.
    ///
    /// **The validator never calls this, and that is the point of it existing.**
    /// Validation has to *fail* on a key it does not recognise, so it reads named
    /// fields through [`palette_from`] and a lookup returning `None` would be
    /// swallowed somewhere. This accessor is for the other direction: a settings
    /// screen, a diagnostic dump, or a caller that received a key from a
    /// configuration UI and needs to resolve it without a `match` over thirteen
    /// arms.
    pub fn get(&self, key: &str) -> Option<Color> {
        match key {
            "background" => Some(self.background),
            "surface" => Some(self.surface),
            "sidebar" => Some(self.sidebar),
            "text" => Some(self.text),
            "text_muted" => Some(self.text_muted),
            "accent" => Some(self.accent),
            "accent_hover" => Some(self.accent_hover),
            "danger" => Some(self.danger),
            "success" => Some(self.success),
            "mention" => Some(self.mention),
            "code_block_bg" => Some(self.code_block_bg),
            "bubble_self" => Some(self.bubble_self),
            "bubble_other" => Some(self.bubble_other),
            _ => None,
        }
    }
}

/// The four spacing steps, in the order a layout escalates through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spacing {
    /// Inside a control: icon padding, chip padding.
    pub xs: u32,
    /// Between a control's own parts.
    pub sm: u32,
    /// Between sibling elements: message rows, list items.
    pub md: u32,
    /// Between groups: the input bar and the message list.
    pub lg: u32,
}

/// The three corner radii.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Radii {
    /// Small controls: badges, chips.
    pub sm: u32,
    /// Cards, the input bar, menus.
    pub md: u32,
    /// Panels and dialogs.
    pub lg: u32,
}

/// The font family and the four size steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typography {
    /// A family name, passed to the platform's font resolver. The schema cannot
    /// validate a name, because whether it resolves is the renderer's business
    /// and a theme that names an absent family still renders.
    pub family: String,
    /// The four size steps.
    pub sizes: FontSizes,
}

/// The four font sizes, in the order a layout escalates through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontSizes {
    /// Secondary labels: channel topics, "edited".
    pub caption: u32,
    /// The timestamp on a message row. The smallest step in the schema.
    pub timestamp: u32,
    /// Message text. The size the whole layout is designed around.
    pub body: u32,
    /// Channel headers, dialog titles.
    pub title: u32,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can be wrong with a theme document.
///
/// **Every message names a JSON path.** `AGENTS.md` §10.2 requires an invalid
/// theme to produce an in-app message, and a message that does not say *which*
/// field is not actionable for the user this feature exists for: themes are
/// shared as single hand-editable JSON files, so the reader of the message is
/// the person who wrote the file, not a developer with a debugger.
///
/// A narrower type than `ShNexusError` on purpose -- see §5 of the module docs.
/// `errors.rs` converts it, and the conversion lands in
/// `ShNexusError::Theme`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// The bytes are not JSON at all.
    ///
    /// The path is [`DOCUMENT`], and the payload is `serde_json`'s own message,
    /// which carries a line and column -- the locator a parse failure needs, and
    /// the reason this variant has no path field.
    MalformedJson {
        /// `serde_json`'s message, verbatim.
        detail: String,
    },

    /// A value is not the JSON type the schema requires.
    WrongType {
        /// Dotted path, e.g. `typography.sizes.body`.
        path: String,
        /// What the schema requires, in a form that reads inside a sentence.
        expected: &'static str,
        /// What was found, likewise.
        found: &'static str,
    },

    /// A required key is absent.
    MissingKey {
        /// The full path of the key that should have been there.
        path: String,
    },

    /// A key this build does not know.
    ///
    /// The known keys are carried, not just named, because the useful half of
    /// this message is the list the user has to choose from.
    UnknownKey {
        /// Dotted path of the offending key, e.g. `colors.colour`.
        path: String,
        /// The keys that would have been accepted at that level.
        known: &'static [&'static str],
    },

    /// A string is empty or whitespace only.
    BlankText {
        /// Dotted path.
        path: String,
    },

    /// A string is longer than [`MAX_LABEL_CHARS`].
    TextTooLong {
        /// Dotted path.
        path: String,
        /// The limit that was exceeded.
        limit: usize,
    },

    /// A value is not [`Color::COLOR_HEX_FORMAT`].
    ///
    /// **The offending value is echoed**, truncated to [`MAX_ECHOED_CHARS`].
    /// §7.5 forbids logging *message content*; a theme file is not a message,
    /// and echoing the three characters that were wrong is the difference
    /// between a message a user can act on and one that only tells them
    /// something is wrong somewhere.
    InvalidColor {
        /// Dotted path, e.g. `colors.accent`.
        path: String,
        /// The value, truncated.
        found: String,
    },

    /// An integer outside the range its field documents.
    OutOfRange {
        /// Dotted path.
        path: String,
        /// The value found, reported as `i128` so a `u64` that overflows `i32` is
        /// not itself misreported as out of range.
        value: i128,
        /// Inclusive lower bound.
        min: i128,
        /// Inclusive upper bound.
        max: i128,
    },

    /// `version` is not [`THEME_FORMAT_VERSION`].
    ///
    /// Separate from [`ThemeError::OutOfRange`] because §7.4's rule is about
    /// *version negotiation*, not about a number being large, and a user who
    /// writes `"version": 2` needs to be told it is a version problem.
    UnsupportedVersion {
        /// The version found.
        found: i128,
    },

    /// The document is larger than [`MAX_THEME_BYTES`].
    ///
    /// Checked before the parse rather than after, because the point of the
    /// bound is to avoid allocating for a 60GB file, and a parse that succeeds
    /// on 60GB has already failed at the thing the bound exists to prevent.
    TooLarge {
        /// The document's length in bytes.
        bytes: usize,
        /// The limit.
        limit: usize,
    },
}

impl fmt::Display for ThemeError {
    /// The message `AGENTS.md` §10.2 puts in the app.
    ///
    /// Every arm names a path, which is the module's one hard rule about error
    /// text and the reason the test suite asserts on the paths.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedJson { detail } => {
                write!(formatter, "the document is not valid JSON: {detail}")
            }
            Self::WrongType {
                path,
                expected,
                found,
            } => write!(formatter, "`{path}` must be {expected}, found {found}"),
            Self::MissingKey { path } => write!(formatter, "`{path}` is required"),
            Self::UnknownKey { path, known } => write!(
                formatter,
                "`{path}` is not a key in this schema; expected one of: {}",
                known.join(", ")
            ),
            Self::BlankText { path } => {
                write!(formatter, "`{path}` is empty or whitespace only")
            }
            Self::TextTooLong { path, limit } => {
                write!(formatter, "`{path}` is longer than {limit} characters")
            }
            Self::InvalidColor { path, found } => write!(
                formatter,
                "`{path}` must be {} (six hex digits after `#`), found `{found}`",
                Color::COLOR_HEX_FORMAT
            ),
            Self::OutOfRange {
                path,
                value,
                min,
                max,
            } => write!(
                formatter,
                "`{path}` is {value}, outside the accepted range {min}..={max}"
            ),
            Self::UnsupportedVersion { found } => write!(
                formatter,
                "`version` is {found}; this build speaks theme format version {THEME_FORMAT_VERSION}"
            ),
            Self::TooLarge { bytes, limit } => write!(
                formatter,
                "the document is {bytes} bytes, over the {limit} byte limit"
            ),
        }
    }
}

impl std::error::Error for ThemeError {}

// ---------------------------------------------------------------------------
// Built-in themes
// ---------------------------------------------------------------------------

/// One of the three themes compiled into the binary.
///
/// `AGENTS.md` §10.2 requires at least dark, light and high-contrast. They live
/// in `crates/sh_nexus/themes/` rather than the workspace's `assets/themes/`
/// because [`BuiltIn::source`] embeds them with `include_str!`, which resolves
/// at **compile time** relative to this source file -- so a crate whose
/// `include_str!` reaches outside its own directory cannot be packaged, and this
/// one is published (`publish.workspace = true`). It is a compile-time
/// constant, not a runtime read: `core/` still opens no file, and
/// `core_names_no_forbidden_dependency` still passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltIn {
    /// The default. Dark, and the one every other path falls back to.
    Dark,
    /// Light.
    Light,
    /// Pure black on pure white, for maximum legibility.
    HighContrast,
}

impl BuiltIn {
    /// Every built-in theme, in the order a settings list would show them.
    pub const ALL: [BuiltIn; 3] = [BuiltIn::Dark, BuiltIn::Light, BuiltIn::HighContrast];

    /// The identifier a persisted preference stores. **Stable**: it is written
    /// to disk, so renaming one silently discards a user's choice.
    pub fn id(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::HighContrast => "high-contrast",
        }
    }

    /// The name a theme picker shows. The same string the embedded document
    /// carries in its `name`, asserted equal by
    /// `every_built_in_theme_carries_its_own_id_and_name` in `tests/theme.rs`.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Dark => "Sh_Nexus Dark",
            Self::Light => "Sh_Nexus Light",
            Self::HighContrast => "Sh_Nexus High Contrast",
        }
    }

    /// The theme's JSON, embedded at compile time.
    pub fn source(self) -> &'static str {
        match self {
            Self::Dark => include_str!("../../themes/dark.json"),
            Self::Light => include_str!("../../themes/light.json"),
            Self::HighContrast => include_str!("../../themes/high-contrast.json"),
        }
    }

    /// Parses and validates the embedded document.
    ///
    /// `Result` rather than `Theme` because a build-time fixture can be
    /// corrupted by a bad merge, and returning an error says so while
    /// `unwrap`ing would crash the process at startup (§6 of the module docs).
    /// `every_built_in_theme_parses_and_validates` in `tests/theme.rs` is what
    /// keeps all three of these `Ok` for a shipped binary.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::theme::BuiltIn;
    ///
    /// for built_in in BuiltIn::ALL {
    ///     let theme = built_in.theme();
    ///     assert!(theme.is_ok(), "{} did not validate: {theme:?}", built_in.id());
    ///     assert_eq!(theme.as_ref().ok().map(|t| t.version()), Some(1));
    /// }
    /// ```
    pub fn theme(self) -> Result<Theme, ThemeError> {
        parse(self.source().as_bytes())
    }
}

/// The built-in theme every invalid document falls back to.
///
/// See §6 of the module docs for why this returns a `Result`.
pub fn default_theme() -> Result<Theme, ThemeError> {
    BuiltIn::Dark.theme()
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// The two possible outcomes of [`load_or_default`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeLoad {
    /// The bytes were a valid theme.
    Loaded(Theme),
    /// The bytes were not. The theme is the default and the rejection is the
    /// reason, because `AGENTS.md` §10.2 requires **both** the fallback and an
    /// in-app message.
    Fallback {
        /// The default theme. A client with the wrong colours is a working chat
        /// client; a client with no colours is not.
        theme: Theme,
        /// Why the bytes were rejected, with the exact JSON path.
        rejection: ThemeError,
    },
}

impl ThemeLoad {
    /// The theme to use. **Always one**: a `ThemeLoad` never exists without a
    /// theme, which is the property that makes the fallback safe to call from
    /// a hot-reload path.
    pub fn theme(&self) -> &Theme {
        match self {
            Self::Loaded(theme) => theme,
            Self::Fallback { theme, .. } => theme,
        }
    }

    /// Why the bytes were rejected, or `None` if they were not.
    pub fn rejection(&self) -> Option<&ThemeError> {
        match self {
            Self::Loaded(_) => None,
            Self::Fallback { rejection, .. } => Some(rejection),
        }
    }

    /// The line to show the user, or `None` when nothing was rejected.
    ///
    /// §10.2 asks for an error message in the app; this is where it comes from,
    /// and it is deliberately the same text the developer's log gets.
    pub fn message(&self) -> Option<String> {
        self.rejection().map(ToString::to_string)
    }
}

/// Parses and validates a theme document.
///
/// # Errors
///
/// Returns [`ThemeError`] for anything the schema does not accept. **Every
/// message names the exact JSON path** of the offending value.
///
/// # Example
///
/// ```
/// use sh_nexus::core::theme::parse;
///
/// // `br##"..."##` rather than `br#"..."#`: the document itself contains `"#`,
/// // which is the sequence that closes a one-hash raw string. Getting this
/// // wrong fails as a Rust syntax error inside a JSON document, which is a
/// // confusing way to learn the rule.
/// let theme = parse(br##"{"name":"T","author":"A","version":1,
///     "colors":{"background":"#14161c","surface":"#1e2129","sidebar":"#101218",
///       "text":"#e4e7ee","text_muted":"#949aab","accent":"#5b8def",
///       "accent_hover":"#7aa4f5","danger":"#e5534b","success":"#57ab5a",
///       "mention":"#d9b56b","code_block_bg":"#0b0d12","bubble_self":"#2a2f3c",
///       "bubble_other":"#1e2129"},
///     "spacing":{"xs":4,"sm":8,"md":16,"lg":24},
///     "radii":{"sm":4,"md":8,"lg":16},
///     "typography":{"family":"Inter",
///       "sizes":{"caption":11,"timestamp":10,"body":14,"title":16}}}"##);
/// assert!(theme.is_ok(), "{theme:?}");
/// assert_eq!(
///     theme.as_ref().ok().map(|t| t.colors().accent.to_string()).as_deref(),
///     Some("#5b8def")
/// );
/// ```
pub fn parse(bytes: &[u8]) -> Result<Theme, ThemeError> {
    if bytes.len() > MAX_THEME_BYTES {
        return Err(ThemeError::TooLarge {
            bytes: bytes.len(),
            limit: MAX_THEME_BYTES,
        });
    }

    let root: Value = serde_json::from_slice(bytes).map_err(|error| ThemeError::MalformedJson {
        detail: error.to_string(),
    })?;
    theme_from(&root)
}

/// Parses a theme, falling back to the default if the bytes are not valid.
///
/// Never returns a `ThemeLoad` without a theme, and never discards the reason:
/// §10.2 requires the fallback *and* the message, and a fallback that swallowed
/// the reason would satisfy half of it.
///
/// # Errors
///
/// Returns [`ThemeError`] **only** when the built-in default itself fails to
/// parse, which `every_built_in_theme_parses_and_validates` in `tests/theme.rs`
/// makes impossible for a shipped binary. The reason this is a `Result` rather
/// than an `unwrap` is §6 of the module docs.
///
/// # Example
///
/// ```
/// use sh_nexus::core::theme::load_or_default;
///
/// let broken = load_or_default(b"{ not json");
/// assert!(broken.as_ref().is_ok());
///
/// let loaded = broken.ok();
/// assert!(loaded.as_ref().and_then(|l| l.rejection()).is_some());
/// ```
pub fn load_or_default(bytes: &[u8]) -> Result<ThemeLoad, ThemeError> {
    match parse(bytes) {
        Ok(theme) => Ok(ThemeLoad::Loaded(theme)),
        Err(rejection) => match default_theme() {
            Ok(theme) => Ok(ThemeLoad::Fallback { theme, rejection }),
            // Unreachable in a build whose fixtures are valid, and reported
            // rather than panicked: a corrupted build-time constant should
            // surface as an error a developer reads at startup, not as a crash
            // on a path with nothing to recover it (AGENTS.md 2.1, 7.1). The
            // default's own failure is returned rather than the user's
            // rejection, because the user's rejection is recoverable by editing
            // a file and this one is a build defect.
            Err(failure) => Err(failure),
        },
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// The dotted path of `key` inside `parent`.
///
/// [`DOCUMENT`] is the root and contributes no prefix, so a top-level key's path
/// is `name` rather than `<document>.name` -- a message reads better without
/// the ceremony.
fn child(parent: &str, key: &str) -> String {
    if parent == DOCUMENT {
        key.to_owned()
    } else {
        format!("{parent}.{key}")
    }
}

/// A JSON value's type, phrased to read inside `expected X, found Y`.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "an integer",
        // A float, which includes `1.0` as well as `1.5`. Both are rejected for
        // the same reason -- the schema's numbers are integers, and a size
        // written `14.0` is a theme author who will write `14.5` next.
        Value::Number(_) => "a number that is not an integer",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The document itself, as an object.
fn theme_from(root: &Value) -> Result<Theme, ThemeError> {
    let object = as_object(root, DOCUMENT)?;
    reject_unknown_keys(object, DOCUMENT, &THEME_KEYS)?;

    Ok(Theme {
        name: text_member(object, DOCUMENT, "name")?,
        author: text_member(object, DOCUMENT, "author")?,
        version: version_of(member(object, DOCUMENT, "version")?)?,
        colors: palette_from(member(object, DOCUMENT, "colors")?)?,
        spacing: spacing_from(member(object, DOCUMENT, "spacing")?)?,
        radii: radii_from(member(object, DOCUMENT, "radii")?)?,
        typography: typography_from(member(object, DOCUMENT, "typography")?)?,
    })
}

/// `object[key]`, or the rejection that names it.
fn member<'a>(
    object: &'a Map<String, Value>,
    parent: &str,
    key: &str,
) -> Result<&'a Value, ThemeError> {
    object.get(key).ok_or_else(|| ThemeError::MissingKey {
        path: child(parent, key),
    })
}

/// `value` as an object, or the rejection that names `path`.
fn as_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, ThemeError> {
    value.as_object().ok_or_else(|| ThemeError::WrongType {
        path: path.to_owned(),
        expected: "an object",
        found: type_name(value),
    })
}

/// Rejects any key outside `known`.
///
/// **The unknown keys are sorted before one is chosen, and in *this* workspace
/// that is not a precaution -- it is the only thing making the behaviour
/// deterministic.** `serde_json::Map` is a `BTreeMap` by default and an
/// `IndexMap` under the `preserve_order` feature, and `cargo tree -e features -i
/// serde_json` shows that feature **is** enabled here, by `gpui` and
/// `http_client` (Zed's git pin). Feature unification means the *client's* map
/// iterates in document order, so a document listing `zeta` before `alpha` would
/// otherwise report `zeta` -- and the message a user sees would depend on which
/// text editor they used.
///
/// That is the general hazard in one sentence: **a crate this project does not
/// depend on directly has already changed the iteration order of a type it does
/// parse.** One small allocation on the error path buys back a property of the
/// schema rather than of the dependency graph.
///
/// Checked **before** the required keys, so a typo is reported as a typo: fixing
/// `colour` to `accent` often fixes the `MissingKey` that follows it, and
/// reporting the missing key first would send the user looking in the wrong
/// place.
fn reject_unknown_keys(
    object: &Map<String, Value>,
    parent: &str,
    known: &'static [&'static str],
) -> Result<(), ThemeError> {
    let mut unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !known.contains(key))
        .collect();
    unknown.sort_unstable();

    match unknown.first() {
        Some(key) => Err(ThemeError::UnknownKey {
            path: child(parent, key),
            known,
        }),
        None => Ok(()),
    }
}

/// A string field: a string, not blank, and within [`MAX_LABEL_CHARS`].
fn text(value: &Value, path: &str) -> Result<String, ThemeError> {
    let raw = value.as_str().ok_or_else(|| ThemeError::WrongType {
        path: path.to_owned(),
        expected: "a string",
        found: type_name(value),
    })?;

    if raw.trim().is_empty() {
        return Err(ThemeError::BlankText {
            path: path.to_owned(),
        });
    }
    if raw.chars().count() > MAX_LABEL_CHARS {
        return Err(ThemeError::TextTooLong {
            path: path.to_owned(),
            limit: MAX_LABEL_CHARS,
        });
    }
    Ok(raw.to_owned())
}

/// `object[parent.key]` as a validated label.
fn text_member(object: &Map<String, Value>, parent: &str, key: &str) -> Result<String, ThemeError> {
    let path = child(parent, key);
    text(member(object, parent, key)?, &path)
}

/// `value` as an integer, whatever width it arrived at.
///
/// `i128` rather than `i64` so that a `u64` too large for `i64` is reported with
/// its true value by [`ThemeError::OutOfRange`] instead of being called
/// "not an integer", which it is.
fn integer(value: &Value, path: &str) -> Result<i128, ThemeError> {
    if let Some(signed) = value.as_i64() {
        return Ok(i128::from(signed));
    }
    if let Some(unsigned) = value.as_u64() {
        return Ok(i128::from(unsigned));
    }
    Err(ThemeError::WrongType {
        path: path.to_owned(),
        expected: "an integer",
        found: type_name(value),
    })
}

/// `value` as an integer inside `min..=max`.
///
/// One rejection path, deliberately: a value outside `u32` and a value inside
/// `u32` but outside the range are **the same mistake from the user's point of
/// view** -- the number is not a legal value for this field -- and giving them
/// one arm means there is no unreachable fallback in a function whose whole job
/// is to be total.
fn bounded(value: &Value, path: &str, min: u32, max: u32) -> Result<u32, ThemeError> {
    let found = integer(value, path)?;
    let accepted = u32::try_from(found)
        .ok()
        .filter(|number| *number >= min && *number <= max);
    accepted.ok_or_else(|| ThemeError::OutOfRange {
        path: path.to_owned(),
        value: found,
        min: i128::from(min),
        max: i128::from(max),
    })
}

/// `object[parent.key]` as an integer inside `min..=max`.
///
/// Every caller writes the range at the point of use, so auditing "is
/// `0..=256` right for `spacing.xs`?" is a read of one line rather than a
/// lookup.
fn number_member(
    object: &Map<String, Value>,
    parent: &str,
    key: &str,
    min: u32,
    max: u32,
) -> Result<u32, ThemeError> {
    let path = child(parent, key);
    bounded(member(object, parent, key)?, &path, min, max)
}

/// `value` as a [`Color`], or a rejection naming `path` and the bad value.
fn color(value: &Value, path: &str) -> Result<Color, ThemeError> {
    let raw = value.as_str().ok_or_else(|| ThemeError::WrongType {
        path: path.to_owned(),
        expected: "a string",
        found: type_name(value),
    })?;

    Color::from_hex(raw).ok_or_else(|| ThemeError::InvalidColor {
        path: path.to_owned(),
        found: truncate(raw),
    })
}

/// `object[parent.key]` as a [`Color`].
fn color_member(object: &Map<String, Value>, parent: &str, key: &str) -> Result<Color, ThemeError> {
    let path = child(parent, key);
    color(member(object, parent, key)?, &path)
}

/// The first [`MAX_ECHOED_CHARS`] of `value`, with an ellipsis if it was longer.
fn truncate(value: &str) -> String {
    if value.chars().count() <= MAX_ECHOED_CHARS {
        return value.to_owned();
    }
    let head: String = value.chars().take(MAX_ECHOED_CHARS).collect();
    format!("{head}...")
}

/// `value` as the pinned format version.
fn version_of(value: &Value) -> Result<u32, ThemeError> {
    let found = integer(value, "version")?;
    match u32::try_from(found) {
        Ok(version) if version == THEME_FORMAT_VERSION => Ok(version),
        _ => Err(ThemeError::UnsupportedVersion { found }),
    }
}

/// The `colors` object.
///
/// **This function is the schema.** Every one of the thirteen lines is a
/// compile-time statement that [`Palette`] has a field for
/// [`COLOR_KEYS`]'s key, so removing a key from either side is a build failure
/// rather than a silently ignored colour. That is why the key is written once
/// per line and not twice.
fn palette_from(value: &Value) -> Result<Palette, ThemeError> {
    let object = as_object(value, "colors")?;
    reject_unknown_keys(object, "colors", &COLOR_KEYS)?;

    Ok(Palette {
        background: color_member(object, "colors", "background")?,
        surface: color_member(object, "colors", "surface")?,
        sidebar: color_member(object, "colors", "sidebar")?,
        text: color_member(object, "colors", "text")?,
        text_muted: color_member(object, "colors", "text_muted")?,
        accent: color_member(object, "colors", "accent")?,
        accent_hover: color_member(object, "colors", "accent_hover")?,
        danger: color_member(object, "colors", "danger")?,
        success: color_member(object, "colors", "success")?,
        mention: color_member(object, "colors", "mention")?,
        code_block_bg: color_member(object, "colors", "code_block_bg")?,
        bubble_self: color_member(object, "colors", "bubble_self")?,
        bubble_other: color_member(object, "colors", "bubble_other")?,
    })
}

/// The `spacing` object.
fn spacing_from(value: &Value) -> Result<Spacing, ThemeError> {
    let object = as_object(value, "spacing")?;
    reject_unknown_keys(object, "spacing", &SPACING_KEYS)?;

    Ok(Spacing {
        xs: number_member(object, "spacing", "xs", MIN_SPACING, MAX_SPACING)?,
        sm: number_member(object, "spacing", "sm", MIN_SPACING, MAX_SPACING)?,
        md: number_member(object, "spacing", "md", MIN_SPACING, MAX_SPACING)?,
        lg: number_member(object, "spacing", "lg", MIN_SPACING, MAX_SPACING)?,
    })
}

/// The `radii` object.
fn radii_from(value: &Value) -> Result<Radii, ThemeError> {
    let object = as_object(value, "radii")?;
    reject_unknown_keys(object, "radii", &RADIUS_KEYS)?;

    Ok(Radii {
        sm: number_member(object, "radii", "sm", MIN_RADIUS, MAX_RADIUS)?,
        md: number_member(object, "radii", "md", MIN_RADIUS, MAX_RADIUS)?,
        lg: number_member(object, "radii", "lg", MIN_RADIUS, MAX_RADIUS)?,
    })
}

/// The `typography` object and its nested `sizes`.
fn typography_from(value: &Value) -> Result<Typography, ThemeError> {
    let object = as_object(value, "typography")?;
    reject_unknown_keys(object, "typography", &["family", "sizes"])?;

    let family = text_member(object, "typography", "family")?;
    let sizes = as_object(member(object, "typography", "sizes")?, "typography.sizes")?;
    reject_unknown_keys(sizes, "typography.sizes", &FONT_SIZE_KEYS)?;

    Ok(Typography {
        family,
        sizes: FontSizes {
            caption: number_member(
                sizes,
                "typography.sizes",
                "caption",
                MIN_FONT_SIZE,
                MAX_FONT_SIZE,
            )?,
            timestamp: number_member(
                sizes,
                "typography.sizes",
                "timestamp",
                MIN_FONT_SIZE,
                MAX_FONT_SIZE,
            )?,
            body: number_member(
                sizes,
                "typography.sizes",
                "body",
                MIN_FONT_SIZE,
                MAX_FONT_SIZE,
            )?,
            title: number_member(
                sizes,
                "typography.sizes",
                "title",
                MIN_FONT_SIZE,
                MAX_FONT_SIZE,
            )?,
        },
    })
}

/// Appends `value` to `out` as a JSON string literal.
///
/// Hand-written because §2's reason applies here too: a derived serializer emits
/// whatever the struct is, and this emits a canonical form. Control characters
/// are escaped rather than passed through, which is what makes a theme name
/// containing a newline round-trip instead of producing invalid JSON -- and
/// whether a name may contain one is deliberately not this module's business,
/// because [`Theme::to_json`] and [`parse`] agree either way.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}
