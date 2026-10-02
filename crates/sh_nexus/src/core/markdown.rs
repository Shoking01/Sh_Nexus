//! Message Markdown: a CommonMark source string in, a styled segment tree out.
//!
//! This is the module `AGENTS.md` §4.2 names as *"Markdown parsing/rendering to
//! a styled segment tree"*, and it is the boundary between "what the user
//! typed" and "what the renderer draws".
//!
//! # Why there are no GPUI types in here
//!
//! `AGENTS.md` §3.2 and `PLAN.md` §4 make `core/` the innermost layer: no
//! `gpui`, no `tokio`, no I/O, no filesystem. So this module does not draw
//! anything, and the next reader will look for a `Div` and not find one.
//!
//! **That is the feature, not the limitation.** A parser that returns styled
//! data is testable with no window, no GPU, and no `TestAppContext` — which is
//! what lets `core/` hold a ≥90% coverage floor (`AGENTS.md` §4.1). A parser
//! that returned GPUI elements would need a live `App` to assert anything at
//! all, and its coverage would be measured on the paths a headless harness
//! happens to reach. Work unit 2 (`ui/`) is what turns this tree into elements;
//! everything below is deliberately unaware that GPUI exists.
//!
//! # 1. The representation: a nested tree whose *inline* half is flat
//!
//! The tempting shape is a uniform recursive tree — one `Node` enum, every node
//! either text or a container. It is the shape `pulldown_cmark`'s own event
//! stream has, so it maps one-to-one and looks obvious. It is also **wrong for
//! half of the language**, and the reason is worth stating carefully because it
//! decides the whole type.
//!
//! **Block structure is genuinely nested and must stay nested.** A block quote
//! is visually indented. A list item is a bullet beside a block of text. A code
//! block is a background box. The nesting *is* the visual meaning, and a flat
//! run list cannot express it without re-deriving it, which is re-deriving a
//! structure the parser already knows.
//!
//! **Inline style is not nested, and flattening it is a normalisation, not a
//! loss.** Consider:
//!
//! ```text
//! **a *b* c**        versus        **a** *b* **c**
//! ```
//!
//! These are different source strings with **identical meaning** — every
//! character is bold, and `b` is additionally italic. A nested tree preserves
//! the two different shapes, so a renderer would have to compare them to draw
//! them the same way. A flat run list with a style set produces the *same three
//! runs* for both:
//!
//! | Run | Style |
//! |---|---|
//! | `a ` | bold |
//! | `b` | bold + italic |
//! | ` c` | bold |
//!
//! So inline formatting is modelled as **a flat `Vec<Inline>` of runs, each
//! carrying a [`Style`] set** — because emphasis composes commutatively,
//! associatively and idempotently, and a data structure that exposes those
//! properties is one a renderer can trust. Bold-inside-bold is a no-op, which is
//! exactly right, and it is free.
//!
//! **The cost, stated plainly.** A nested tree can express "this run is inside
//! that run" and a flat list cannot. That information is not lost, because
//! nothing in the renderer can use it: inline formatting has no containment
//! semantics to honour. The thing a flat list *cannot* do is survive a
//! formatting change mid-paragraph without splitting a run — and that is one
//! comparison, not a data structure. Meanwhile the flat half buys three things
//! the tree does not: no recursion in the hot path (`AGENTS.md` §2.3, no
//! blocking the frame loop), a `Style` that is a single `u8` the renderer can
//! test with `contains`, and adjacent same-style runs that merge, so ordinary
//! prose is one span rather than one per line.
//!
//! **Where the tree still nests, and why that is not a contradiction:** links.
//! A link is a span-level *container*, because it has a payload — a target —
//! that character styles do not have, and because a link is clicked as a unit.
//! So [`Inline`] is `Text(TextSpan)` or `Link(LinkSpan)`, and a `LinkSpan` holds
//! its own `Vec<Inline>` of styled runs. CommonMark forbids a link inside a
//! link, so that second level is where it ends: inline nesting is bounded by
//! [`MAX_INLINE_DEPTH`] and always is, structurally rather than by a cap this
//! module enforces.
//!
//! # 2. `Style` is a set, not an enum
//!
//! [`Style`] is a hand-rolled bitset over four flags — bold, italic, inline
//! code, strikethrough. An enum is the obvious alternative and it fails on
//! `***both***`, which the prompt's question reaches directly: pulldown emits
//! `Emphasis` then `Strong` around one run of text, so the honest answer is
//! **both at once**. An enum cannot say that without a `BoldItalic` variant, and
//! then it needs one variant per combination: `BoldItalicStrikethrough` and so
//! on. That is 2â¿ variants for n styles, and adding a fourth style is a breaking
//! change to a public enum.
//!
//! A bitset says "bold and italic" in one byte, composes with `union`, and gains
//! a style without touching existing code. The cost is that not every
//! combination is meaningful — nothing produces bold inline code from a
//! backtick, though ``**`x`**`` legitimately does — and a bitset cannot stop a
//! caller from constructing a nonsensical one. That is the right trade for a
//! value type that only the parser constructs.
//!
//! **The two constructs that must not collapse.** Inline code is
//! [`Style::CODE`] on a run: it is *styled text that was not parsed further*.
//! A code block is a [`Block::CodeBlock`] with verbatim content and an optional
//! language. They are different types, live at different levels of the tree,
//! and have different payloads, so a caller cannot confuse them by accident —
//! which is the point. `AGENTS.md` §4.2 asks for code, and there are two
//! distinct things to get right.
//!
//! # 3. Source positions: kept, and not what they look like
//!
//! Every byte of text in the tree carries a [`Range<usize>`] into the source it
//! came from, because a caller needs it and the alternative is worse.
//!
//! **The caller.** `PLAN.md` §8, Phase 5 lists *"Search messages"*, and search
//! highlights hits **inside** a message body. Search runs over
//! `Message::content` — the raw source — and produces byte offsets. Turning
//! those into "highlight this run" needs the mapping, and the only place it can
//! exist is here. Without ranges, the only route is to re-render the message
//! into a string and search *that*, which searches a string where markup has
//! already been removed, so an offset in one space is meaningless in the other.
//! The mapping has to be built at parse time or not at all.
//!
//! **The cost, and it is not small.** The range is **provenance, not a
//! verbatim slice.** pulldown normalises: `&amp;` becomes `&`, `\*` becomes
//! `*`, trailing spaces before a soft break are dropped, and adjacent same-style
//! runs are merged (see [`Document::plain_text`]). So `&source[range]` is *not*
//! generally equal to the run's text, and a caller must highlight **the run's
//! own text**, never a re-slice of the source. Anyone expecting a round-trip
//! slice will be wrong, and this paragraph is the only place that says so.
//!
//! # 4. What "the text is the message" is allowed to mean
//!
//! In a web client the injection risk is XSS. **There is no HTML renderer
//! anywhere in this project**, so the classic attack has no interpreter to
//! reach — the only consumer of this tree is a text renderer that draws glyphs.
//! Asking "can this be XSS" is asking about a component that does not exist.
//! So the real question is different, and it has five parts. Each is closed by a
//! rule in this module and a test named after it.
//!
//! **Raw HTML is preserved as literal text, never dropped and never
//! interpreted.** pulldown emits [`Event::Html`] and [`Event::InlineHtml`] for
//! `<script>alert(1)</script>` and for `<div>`. Dropping it would make a message
//! that quotes HTML read as something the author did not write — the same
//! defect `core/ordering.rs` refuses when it declines to invent a missing
//! message. Preserving it as literal text means the reader sees exactly the
//! characters the author typed, the tree contains no HTML *semantics* at all,
//! and a future consumer that somehow had them could not be misled by a span
//! that has already been through this decision. A block-level HTML construct
//! degrades to a paragraph of its own literal text.
//!
//! **Link targets are filtered by an allowlist, and the allowlist is
//! `http`, `https`, `mailto` — everything else is refused.** An allowlist
//! rather than a denylist is the whole argument: a denylist must enumerate every
//! dangerous scheme and a scheme nobody thought of is allowed, so the failure
//! mode is silent. The same reasoning is why `layer_boundary.rs` uses
//! `CORE_ALLOWED_CRATES`.
//!
//! **Images are not links, and a message's Markdown never causes a fetch.**
//! `![alt](url)` is a network request to a host the *message author* chose, with
//! this client's IP address attached, from inside a chat window. That is a
//! read receipt. Attachments belong in [`crate::core::models::message::Message::attachments`], which the
//! network boundary validates (`network/mapping.rs`); an image source has no
//! business in attacker-controlled Markdown. So an image contributes its **alt
//! text** and nothing else, and its target is counted as one this client will
//! not act on.
//!
//! **Unicode bidi controls are neutralised, and this is the one a web-focused
//! review misses.** A message can contain `U+202E RIGHT-TO-LEFT OVERRIDE`, and
//! `gnp<U+202E>exe` then *displays* as `exe.png` — a colleague reads a filename
//! that the author did not write. No XSS, no markup, no interpreter: the text is
//! simply **shown as something other than what it is**, which is precisely the
//! question. So the directional *formatting* controls (U+202A–U+202E,
//! U+2066–U+206F) and `U+FEFF` are replaced with `U+FFFD REPLACEMENT CHARACTER`,
//! which makes the tampering **visible** instead of invisible. The directional
//! *marks* (`U+200E`, `U+200F`, `U+061C`) and the zero-width joiners are **kept**,
//! because they are load-bearing in Arabic, Hebrew and Indic scripts and a chat
//! client that mangles those is worse than one that is slightly permissive. The
//! count is reported by [`Document::neutralized_characters`] rather than
//! swallowed.
//!
//! **Link titles are dropped.** `[a](url "title")` puts attacker-controlled text
//! into a tooltip, which is a second injection surface for no chat benefit.
//!
//! **The remaining path, named honestly.** Nothing in this tree is interpreted
//! as anything but text, and the only consumers this work unit knows about are
//! `ui/` (glyphs) and [`Document::plain_text`] (a `String`). One *future*
//! consumer does interpret it: `PLAN.md` §2's `syntect` syntax highlighting for
//! code blocks. That is a real risk to re-examine when it lands, and the reason
//! it is manageable is that highlighting is driven by a *declared* language
//! string and emits styled text, never anything executable.
//!
//! # 5. The two limits, and what happens at the boundary
//!
//! Both limits are **reported, not silent** — a truncated or depth-limited
//! message is a fact the user can be told, and a silently shortened message is
//! the client lying about what was sent.
//!
//! **Size: [`MAX_MESSAGE_BYTES`] of source.** `AGENTS.md` §5.2 lists a
//! 500-character message as a normal edge case, so the limit is set far above
//! any human message (64 KiB is roughly 1.5× Slack's 40,000-character cap in
//! UTF-8 bytes) and exists for one reason: a hostile peer sending a 5 MB
//! "message" would otherwise make the *UI thread* parse 5 MB of Markdown, which
//! is a denial-of-service surface with no error path. At the boundary the source
//! is cut **on a UTF-8 character boundary** — never mid-`char`, which would
//! panic — and [`Document::was_truncated`] is `true`.
//!
//! **Why truncate rather than reject the message.** Discarding a 5 MB message
//! would destroy the evidence the user might want to report, and would let a
//! peer erase a message by making it large. Truncating keeps what arrived,
//! bounds the work, and flags itself. The caller decides what to show.
//!
//! **Depth: [`MAX_BLOCK_DEPTH`] nested block containers.** Block quotes nest
//! without limit in CommonMark (`> > > > …`) and a renderer that recurses on
//! them can be walked off the stack. At the boundary the module **stops adding
//! containers but keeps every byte**: the deeper content continues to collect
//! into the deepest open container, so the nesting flattens and no text is lost.
//! [`Document::was_depth_limited`] reports it. Inline nesting is *not* capped
//! and does not need to be — see §1.
//!
//! **There is deliberately no third limit on span count.** It is already
//! bounded: 64 KiB of source cannot produce more than O(source) runs, and that
//! is tens of thousands of small allocations, which is under a millisecond of
//! work on a message that has already crossed a socket. A cap here would be a
//! knob with no measurement behind it.
//!
//! # 6. The properties, and what they pin
//!
//! `AGENTS.md` §4.4 mandates a property test and `PLAN.md` §10 names the one for
//! this module: *"markdown parsing never panics for arbitrary input"*. That is
//! implemented, literally, as
//! `arbitrary_markdown_never_panics_the_parser`. Four more earn their place
//! because they are the claims the rest of this file makes:
//!
//! | Property | The claim it pins |
//! |---|---|
//! | `arbitrary_markdown_never_panics_the_parser` | §4.4's mandate, verbatim |
//! | `parsing_is_deterministic` | same input, same tree — which is what makes work unit 1C-2's segment cache worth having |
//! | `the_tree_never_exceeds_its_declared_depth` | §5's two bounds, from the outside |
//! | `every_source_range_lies_inside_the_source` | §3's ranges are real, not fabricated |
//! | `no_link_target_escapes_the_allowlist` | §4's allowlist, over arbitrary input rather than a hand-written list of bad URLs |
//! | `inline_style_union_is_commutative_associative_and_idempotent` | §2's claim that the style algebra is the reason a renderer can trust it |
//!
//! **There is deliberately no "concatenated text equals the source with markup
//! removed" property.** It is the obvious one to reach for and it **does not
//! hold** — pulldown decodes entities (`&amp;` → `&`), resolves escapes, drops
//! the spaces before a soft break, and drops a link's title. Asserting it
//! would produce a test that fails on a correct parser. What holds instead is
//! tested exactly, on inputs chosen so the normalisation cannot fire: the
//! `a_markup_spelling_renders_to_exactly_the_text_the_reader_sees` table
//! asserts the plain text of each construct by hand. A universal claim was
//! replaced by a specific one that is true, which is the better trade.
//!
//! # What the parser library does and does not do here
//!
//! `pulldown-cmark` 0.13.4 does the CommonMark lexing and the nesting; the §7.2
//! audit is in the root `Cargo.toml`. The extension set is **deliberately
//! narrow**: [`Options::ENABLE_STRIKETHROUGH`] and [`Options::ENABLE_TASKLISTS`]
//! on, everything else off. `~~struck~~` and `- [ ] todo` are things people
//! write in chat. Tables, footnotes, definition lists, math, wikilinks,
//! metadata blocks and smart punctuation are not, and leaving them off is what
//! keeps [`Block`] a five-variant enum instead of a mirror of the parser's
//! twenty. A table in a chat bubble is a layout problem, not a parsing one, and
//! solving it here would be solving it for nobody.

use std::fmt;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag};

/// The largest message body this module will parse, in UTF-8 bytes.
///
/// 64 KiB. Roughly 1.5× Slack's 40,000-character limit measured in UTF-8 bytes,
/// and about 130× the 500-character edge case `AGENTS.md` §5.2 calls normal. The
/// limit is not about being polite to well-behaved peers; it is that parsing is
/// pure CPU on the UI thread (`AGENTS.md` §2.3), so a 5 MB "message" is a
/// denial-of-service surface with no error path. See the module docs, §5, for
/// what happens at the boundary.
pub const MAX_MESSAGE_BYTES: usize = 65_536;

/// The deepest block nesting this module will build.
///
/// 16 nested block quotes, lists or list items. A chat message does not nest
/// this deep; CommonMark permits it without limit, and a renderer recursing on
/// an unbounded tree can be walked off the stack. At the boundary the nesting
/// **flattens and no text is lost** — see [`Document::was_depth_limited`].
pub const MAX_BLOCK_DEPTH: usize = 16;

/// The deepest inline nesting a parsed tree can have.
///
/// 2: a run, inside a link. Not a cap this module enforces but one the format
/// guarantees, because CommonMark forbids a link inside a link, so pulldown
/// never emits one. It is a named constant because the depth is **queryable**
/// ([`Document::inline_nesting_depth`]) and a claim nobody can inspect is a
/// claim nobody should rely on. Nothing breaks if it were ever exceeded: the
/// tree is recursive, so a deeper tree would simply render deeper — which is
/// also why there is no enforcement arm to write.
pub const MAX_INLINE_DEPTH: usize = 2;

/// The longest link target this module will keep, in UTF-8 bytes.
///
/// 2 KiB. A URL longer than this is not a URL a person will click, and
/// [`classify_target`] bounds it so a hostile message cannot put a
/// quarter-megabyte string into a `String` that `ui/` will render.
pub const MAX_TARGET_BYTES: usize = 2_048;

/// A set of inline character styles applied to one run of text.
///
/// A bitset rather than an enum, and the reason is the question of what
/// `***both***` means: **both**. pulldown emits `Emphasis` then `Strong` around
/// one run, so a style has to be able to hold two flags at once. An enum would
/// need one variant per combination and would grow a variant for every style
/// added. See the module docs, §2.
///
/// [`Event`] is linked in the docs only to name the crate's type; this module
/// does not re-export it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style(u8);

impl Style {
    /// No style at all: the run is plain body text.
    ///
    /// The [`Default`], and a real value rather than "the absence of a style",
    /// because a run always has one.
    pub const NONE: Style = Style(0);
    /// **Bold.** `**x**`, `__x__`, or `Tag::Strong` from the parser.
    pub const BOLD: Style = Style(1 << 0);
    /// *Italic.* `*x*`, `_x_`, or `Tag::Emphasis` from the parser.
    pub const ITALIC: Style = Style(1 << 1);
    /// Inline code. `` `x` `` — preformatted, and **not parsed further**.
    ///
    /// A style bit and not a container, because it has no payload beyond the
    /// flag: it composes with [`BOLD`](Self::BOLD) because ``**`x`**`` is
    /// legitimately bold code. It is **not** the same thing as
    /// [`Block::CodeBlock`], which is a block with verbatim content and a
    /// language; the two cannot be confused by a caller because one is a bit on
    /// a run and the other is a tree variant with its own fields.
    pub const CODE: Style = Style(1 << 2);
    /// ~~Strikethrough.~~ `~~x~~`. Requires [`Options::ENABLE_STRIKETHROUGH`].
    pub const STRIKETHROUGH: Style = Style(1 << 3);

    /// Every style this module can produce, or'd together.
    ///
    /// Exists so a test can assert the *closedness* of the set — that no
    /// arbitrary input ever yields a style outside it — without a test having to
    /// re-list the flags and drift when one is added.
    pub const ALL: Style = Style(0b1111);

    /// The empty set. Identical to [`Style::NONE`]; named so a caller building a
    /// style incrementally says what it means.
    pub const fn empty() -> Style {
        Style::NONE
    }

    /// The set holding exactly the bits of `bits`.
    ///
    /// Bits outside [`Style::ALL`] are **kept**, not masked out. A bitset that
    /// silently dropped unknown bits would make `Style` disagree with
    /// [`bits`](Self::bits), and a future flag added to this type would be
    /// invisible to any code holding an older `Style`.
    pub const fn from_bits(bits: u8) -> Style {
        Style(bits)
    }

    /// The raw bits, for a caller that needs to store or forward them.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every flag in `other` is set in `self`.
    ///
    /// `self.contains(Style::NONE)` is `true` for every `self`, which is what
    /// makes `Style::NONE` the identity of [`union`](Self::union).
    pub const fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether `self` and `other` hold exactly the same flags.
    pub const fn is(self, other: Style) -> bool {
        self.0 == other.0
    }

    /// Whether no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many flags are set.
    ///
    /// Named rather than shadowing `Iterator::count` so a renderer reading
    /// `style.count()` does not wonder whether it is iterating.
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// Every flag set in either `self` or `other`.
    ///
    /// The operation inline styling is built from, and the reason [`Style`] is
    /// a set: `**a *b* c**` is `BOLD.union(ITALIC)` on the run that is both.
    pub const fn union(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }

    /// Only the flags set in both.
    pub const fn intersect(self, other: Style) -> Style {
        Style(self.0 & other.0)
    }

    /// `self` with the flags of `other` removed.
    pub const fn without(self, other: Style) -> Style {
        Style(self.0 & !other.0)
    }

    /// The flags set in exactly one of `self` and `other`.
    pub const fn symmetric_difference(self, other: Style) -> Style {
        Style(self.0 ^ other.0)
    }
}

impl fmt::Binary for Style {
    /// The raw bits in binary, so a `Debug` line and a `Binary` line cannot
    /// disagree about what a style is.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Binary::fmt(&self.0, formatter)
    }
}

/// A heading's level, as this crate's own type.
///
/// pulldown-cmark has a perfectly good `HeadingLevel`, and this module does not
/// expose it. The reason is the same one `core/models` keeps its distance from
/// serde (`PLAN.md` §5): **a domain type should not be a parser's type.** If
/// [`Block`] embedded `pulldown_cmark::HeadingLevel`, then a dependency bump
/// would be a breaking change to `core/`'s public API, and the boundary the
/// wire/domain split exists to create would be recreated at the parser boundary.
/// The conversion is six arms and is tested one per arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Heading {
    /// `#`
    H1,
    /// `##`
    H2,
    /// `###`
    H3,
    /// `####`
    H4,
    /// `#####`
    H5,
    /// `######`
    H6,
}

impl Heading {
    /// The level as a number, 1 through 6, for a caller sizing type.
    ///
    /// Total, and the reason [`Heading`] is an enum and not a `u8`: the number
    /// is derived here rather than stored, so there is no way to hold a `0` or a
    /// `7`.
    pub const fn level(self) -> u8 {
        match self {
            Self::H1 => 1,
            Self::H2 => 2,
            Self::H3 => 3,
            Self::H4 => 4,
            Self::H5 => 5,
            Self::H6 => 6,
        }
    }
}

impl From<HeadingLevel> for Heading {
    fn from(level: HeadingLevel) -> Self {
        match level {
            HeadingLevel::H1 => Self::H1,
            HeadingLevel::H2 => Self::H2,
            HeadingLevel::H3 => Self::H3,
            HeadingLevel::H4 => Self::H4,
            HeadingLevel::H5 => Self::H5,
            HeadingLevel::H6 => Self::H6,
        }
    }
}

/// One run of text with a single, uniform style.
///
/// A run is a **maximal** stretch of one style: `Builder::push_text` merges an
/// adjacent same-style run into its predecessor, so a paragraph of ordinary
/// prose is one `TextSpan` rather than one per line, and two different spellings
/// of the same emphasis produce the same tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSpan {
    style: Style,
    text: String,
    source: Range<usize>,
}

impl TextSpan {
    fn new(style: Style, text: String, source: Range<usize>) -> Self {
        Self {
            style,
            text,
            source,
        }
    }

    /// The style every character of this run shares.
    pub fn style(&self) -> Style {
        self.style
    }

    /// The text, exactly as the reader sees it.
    ///
    /// Already sanitised: bidi controls are gone (see the module docs, §4) and
    /// entities are already decoded by the parser. This string is the run, and
    /// `&source[source_range()]` is **not** necessarily equal to it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The byte range of the **source** this run came from.
    ///
    /// Provenance, not a verbatim slice: entities, escapes and trailing
    /// whitespace are normalised, and adjacent same-style runs are merged, so
    /// this range can cover source the run's text does not contain. Map *search
    /// hits* through it, and highlight the run's own
    /// [`text`](Self::text) — never a re-slice of the source.
    pub fn source(&self) -> Range<usize> {
        self.source.clone()
    }
}

/// A link: an allow-listed target and the styled runs it contains.
///
/// The one thing that nests inline, because it is the one thing with a payload.
/// See the module docs, §1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    target: String,
    children: Vec<Inline>,
    source: Range<usize>,
}

impl LinkSpan {
    fn new(target: String, children: Vec<Inline>, source: Range<usize>) -> Self {
        Self {
            target,
            children,
            source,
        }
    }

    /// The destination, or `None` if the target was refused.
    ///
    /// Always `Some` for a `LinkSpan` that exists: a refused target produces a
    /// plain [`TextSpan`] instead of a `LinkSpan`, so **a `LinkSpan` in the tree
    /// is itself proof the target passed the allowlist.** That is the property
    /// `no_link_target_escapes_the_allowlist` checks, and it is why the field is
    /// a `String` rather than an `Option<String>`.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The styled runs inside the link. Empty only if the link had no text.
    pub fn children(&self) -> &[Inline] {
        &self.children
    }

    /// The byte range of the source the whole link came from.
    pub fn source(&self) -> Range<usize> {
        self.source.clone()
    }
}

/// Inline content: a styled run, or a link containing styled runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// A run of text with one style. See [`TextSpan`].
    Text(TextSpan),
    /// A link with an allow-listed target. See [`LinkSpan`].
    Link(LinkSpan),
}

impl Inline {
    /// The byte range of the source this came from.
    pub fn source(&self) -> Range<usize> {
        match self {
            Self::Text(span) => span.source(),
            Self::Link(link) => link.source(),
        }
    }

    /// The text a reader sees, with no styling and no target.
    ///
    /// Recurses into a link's children, so a caller that wants display text
    /// without walking the tree gets all of it.
    pub fn text(&self) -> String {
        let mut collected = String::new();
        self.collect_text(&mut collected);
        collected
    }

    fn collect_text(&self, into: &mut String) {
        match self {
            Self::Text(span) => into.push_str(span.text()),
            Self::Link(link) => {
                for child in &link.children {
                    child.collect_text(into);
                }
            }
        }
    }

    /// Nesting depth, counting this node as 1.
    fn depth(&self) -> usize {
        match self {
            Self::Text(_) => 1,
            Self::Link(link) => 1 + link.children.iter().map(Self::depth).max().unwrap_or(0),
        }
    }
}

/// One item of a list, whether or not it is a task-list checkbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    checked: Option<bool>,
    blocks: Vec<Block>,
}

impl ListItem {
    fn new(checked: Option<bool>, blocks: Vec<Block>) -> Self {
        Self { checked, blocks }
    }

    /// `Some(true)` / `Some(false)` for a `- [x]` / `- [ ]` task item, and `None`
    /// for an ordinary one.
    ///
    /// A tri-state rather than a `bool`, because "not a task" and "an unchecked
    /// task" must not render the same, and collapsing them would need a
    /// convention about which is which.
    pub fn checked(&self) -> Option<bool> {
        self.checked
    }

    /// The item's blocks. A loose item has several; a tight one has one.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
}

/// One block of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph: the runs, and nothing else.
    Paragraph {
        /// The paragraph's styled runs, flat and in reading order.
        spans: Vec<Inline>,
    },
    /// A heading. Legal in a chat message, and cheap to support.
    Heading {
        /// How deep, 1 through 6.
        level: Heading,
        /// The heading's styled runs.
        spans: Vec<Inline>,
    },
    /// A block quote. **Nests**, because the indentation is the meaning.
    Quote {
        /// The quoted blocks, themselves a full block sequence.
        blocks: Vec<Block>,
    },
    /// A list.
    List {
        /// `true` for `1.`, `false` for `-`.
        ordered: bool,
        /// The first number of an ordered list, so `3. …` starts at 3.
        start: u64,
        /// The items.
        items: Vec<ListItem>,
    },
    /// A thematic break: `---`, `***`, `___`.
    Rule,
    /// A code block: **verbatim, and never parsed for inline markup.**
    ///
    /// Structurally distinct from [`Style::CODE`], which is inline code. This
    /// one is a block, its content is raw text rather than a run list, and it
    /// carries a language. `AGENTS.md` §4.2's "code" test target needs both
    /// right, and they cannot be confused here because their types differ.
    CodeBlock {
        /// The declared language of a fenced block (```` ```rust ````), or
        /// `None` for a fenced block with no language and for an indented one.
        ///
        /// **A declaration, not a capability.** It is a string a peer chose, and
        /// it selects a highlighter (`PLAN.md` §2's `syntect`, Phase 5) — never
        /// anything executed. Truncated to a sane length by [`MAX_TARGET_BYTES`].
        language: Option<String>,
        /// The code, exactly as written. Entities are **not** decoded here,
        /// because nothing decodes anything in a code block.
        text: String,
        /// The byte range of the source the block came from.
        source: Range<usize>,
    },
}

impl Block {
    /// How many block **containers** enclose this block, counting itself if it is
    /// one.
    ///
    /// A leaf — a paragraph, a heading, a rule, a code block — is `0`, not `1`,
    /// and that is deliberate. It is what makes [`MAX_BLOCK_DEPTH`] mean one
    /// thing: a leaf cannot nest inside another leaf, so allowing one at the
    /// limit costs no recursion depth a caller has to reason about, and counting
    /// it would make the observable maximum one larger than the declared bound
    /// for no gain.
    fn container_depth(&self) -> usize {
        match self {
            Self::Paragraph { .. } | Self::Rule | Self::CodeBlock { .. } | Self::Heading { .. } => {
                0
            }
            Self::Quote { blocks } => 1 + deepest(blocks.iter().map(Self::container_depth)),
            Self::List { items, .. } => {
                1 + deepest(
                    items
                        .iter()
                        .map(|item| deepest(item.blocks().iter().map(Self::container_depth))),
                )
            }
        }
    }

    fn collect_text(&self, into: &mut String) {
        match self {
            Self::Paragraph { spans } | Self::Heading { spans, .. } => {
                for span in spans {
                    span.collect_text(into);
                }
            }
            Self::Quote { blocks } => {
                for block in blocks {
                    block.collect_text_into_line(into);
                }
            }
            Self::List { items, .. } => {
                for item in items {
                    for block in item.blocks() {
                        block.collect_text_into_line(into);
                    }
                }
            }
            Self::CodeBlock { text, .. } => into.push_str(text),
            // A thematic break has no text content. A message that is only
            // `---` genuinely has nothing to show, and `plain_text` says so
            // rather than inventing a placeholder for it.
            Self::Rule => {}
        }
    }

    /// Appends this block's text followed by a newline, so blocks are separated.
    ///
    /// The newline is **not** added if there already is one at the end, and that
    /// condition is load-bearing rather than a nicety. A container's
    /// [`collect_text`] already ends in a newline when its last child was a
    /// paragraph or a nested container, so unconditionally appending one turns
    /// `- a\n  - b\n- c` into `"a\nb\n\nc"` — a blank line that is in the
    /// message body and was never written. Previews, notification text and
    /// search snippets all read from here, so a phantom blank line in a chat
    /// client is a visible defect.
    fn collect_text_into_line(&self, into: &mut String) {
        self.collect_text(into);
        if !into.ends_with('\n') {
            into.push('\n');
        }
    }
}

/// The largest nesting depth among `depths`, or 0 for an empty iterator.
fn deepest(depths: impl Iterator<Item = usize>) -> usize {
    depths.max().unwrap_or(0)
}

/// A parsed message: blocks, and what had to be done to the source to get here.
///
/// Built only by [`parse`], and read-only afterwards. The three `was_*` /
/// counter accessors are not diagnostics — they are the module's contract about
/// being lossy, made inspectable so a caller can tell the user instead of
/// guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    blocks: Vec<Block>,
    truncated: bool,
    depth_limited: bool,
    rejected_links: usize,
    neutralized: usize,
}

impl Document {
    /// The message's blocks, in source order.
    ///
    /// Empty when the source held no blocks, which is what an empty message, a
    /// message of only whitespace, and a message that is only a thematic break
    /// all produce.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Whether the source was longer than [`MAX_MESSAGE_BYTES`] and was cut.
    ///
    /// `true` means text is **missing**. The caller should say so; a message
    /// silently shortened is the client lying about what was sent.
    pub fn was_truncated(&self) -> bool {
        self.truncated
    }

    /// Whether block nesting reached [`MAX_BLOCK_DEPTH`] and was flattened.
    ///
    /// `false` for every realistic message. `true` means no text was lost, only
    /// nesting depth — see the module docs, §5.
    pub fn was_depth_limited(&self) -> bool {
        self.depth_limited
    }

    /// How many link targets the tree does **not** offer as a clickable target.
    ///
    /// Two causes, both deliberate, and neither stores the offending target:
    ///
    /// 1. a link whose scheme is not `http`, `https` or `mailto`
    ///    ([`classify_target`]);
    /// 2. an image, whose target is discarded because a message's Markdown must
    ///    never cause a network fetch (module docs, §4).
    ///
    /// Non-zero is a signal, not an error: the text is still all there.
    pub fn rejected_link_count(&self) -> usize {
        self.rejected_links
    }

    /// How many Unicode characters were replaced with `U+FFFD`.
    ///
    /// Bidi formatting controls and `U+FEFF`, per module docs §4. Non-zero
    /// means the message contained a character whose whole purpose is to make
    /// text display as something other than what it is. Zero for every message
    /// a human wrote in a Latin or CJK script.
    pub fn neutralized_characters(&self) -> usize {
        self.neutralized
    }

    /// The whole message as one plain string, blocks separated by newlines.
    ///
    /// The text a reader sees: styles and link targets dropped, code included,
    /// line breaks kept. For a notification body, a channel preview, a search
    /// snippet, or a "message is too long to send" check.
    ///
    /// Trailing whitespace is trimmed, so a trailing soft break or rule does not
    /// leave a newline on the end. **Not** a round trip: this is the visible
    /// text, so link titles and markup are gone by construction.
    pub fn plain_text(&self) -> String {
        let mut collected = String::new();
        for block in &self.blocks {
            block.collect_text_into_line(&mut collected);
        }
        collected.trim_end().to_owned()
    }

    /// The deepest block **container** nesting, counting a top-level block quote
    /// as 1 and a plain message as 0.
    ///
    /// The observable form of [`MAX_BLOCK_DEPTH`]: the bound is part of the
    /// contract, so a caller — and a property test — can measure it instead of
    /// taking the module's word for it. A leaf block inside the deepest container
    /// does not raise it, which is why the figure for 30 nested `>` quotes is
    /// [`MAX_BLOCK_DEPTH`] and not one more.
    pub fn block_depth(&self) -> usize {
        deepest(self.blocks.iter().map(Block::container_depth))
    }

    /// The deepest inline nesting anywhere in the document.
    ///
    /// The observable form of [`MAX_INLINE_DEPTH`], and always 1 or 2 for a
    /// CommonMark source because a link cannot contain a link.
    pub fn inline_nesting_depth(&self) -> usize {
        fn of_blocks(blocks: &[Block]) -> usize {
            let mut depth = 0;
            for block in blocks {
                let spans: &Vec<Inline> = match block {
                    Block::Paragraph { spans } | Block::Heading { spans, .. } => spans,
                    Block::Quote { blocks } => {
                        depth = depth.max(of_blocks(blocks));
                        continue;
                    }
                    Block::List { items, .. } => {
                        for item in items {
                            depth = depth.max(of_blocks(item.blocks()));
                        }
                        continue;
                    }
                    Block::Rule | Block::CodeBlock { .. } => continue,
                };
                for span in spans {
                    depth = depth.max(span.depth());
                }
            }
            depth
        }
        of_blocks(&self.blocks)
    }
}

/// Parses a message body into a styled segment tree.
///
/// Pure: no I/O, no clock, no thread, no `gpui` (`AGENTS.md` §3.2,
/// `PLAN.md` §4). A function of its argument and nothing else, which is what
/// lets the properties in `tests/markdown.rs` be universal claims rather than
/// observations of one run.
///
/// # Arguments
///
/// * `source` - the message body, as the user typed it. Not required to be
///   valid UTF-8-bounded, valid Markdown, or even valid Markdown *syntax*;
///   anything a peer can put in a frame is accepted.
///
/// # Returns
///
/// A [`Document`]. Three of its accessors report work this function had to do:
/// [`was_truncated`](Document::was_truncated),
/// [`was_depth_limited`](Document::was_depth_limited) and
/// [`rejected_link_count`](Document::rejected_link_count). None of them is an
/// error, and a document with all three set is still a document.
///
/// # Errors
///
/// **None, and deliberately so.** Parsing a string cannot fail. Truncation is
/// not failure, a refused link is not failure, and malformed Markdown is
/// CommonMark's *normal* case — it is text that happens not to be markup. A
/// `Result` here would be an error type with no reachable variant, which
/// `core/ordering.rs` already argues against in its own
/// [`Errors`](https://docs.rs/sh_nexus) section and which is the same argument:
/// this module cannot open a socket, read a file, or read a clock, so it has no
/// error condition to invent.
///
/// # Example
///
/// ```
/// use sh_nexus::core::markdown::{parse, Block, Inline, Style};
///
/// let document = parse("**deploy** is *ready*: see `main.rs`");
///
/// let Block::Paragraph { spans } = &document.blocks()[0] else {
///     panic!("a single line is one paragraph");
/// };
///
/// // The runs are **flat and maximal**: each is a stretch of one style, and the
/// // unstyled text between the marked-up words is a run like any other. That is
/// // what lets a renderer walk them without recursion, and it is why this looks
/// // up a run by its text rather than by index.
/// let style_of = |wanted: &str| {
///     spans
///         .iter()
///         .find_map(|span| match span {
///             Inline::Text(run) if run.text() == wanted => Some(run.style()),
///             _ => None,
///         })
///         .expect("the fixture contains this run")
/// };
///
/// assert!(style_of("deploy").contains(Style::BOLD));
/// assert!(style_of("ready").contains(Style::ITALIC));
/// assert!(style_of("main.rs").contains(Style::CODE));
///
/// // And the text a reader sees is the source with the markup removed.
/// assert_eq!(document.plain_text(), "deploy is ready: see main.rs");
/// ```
pub fn parse(source: &str) -> Document {
    let (source, truncated) = truncate_to_char_boundary(source);

    let mut options = Options::empty();
    // The only two extensions enabled, and the reason is that they are things
    // people write in chat. Everything else in CommonMark-plus-GFM is a
    // document feature, not a message feature; see the module docs.
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut builder = Builder::new();
    // `into_offset_iter` is the reason `TextSpan::source` exists at all: the
    // plain iterator carries no positions, and positions are what turn a search
    // hit into a highlight. See the module docs, section 3.
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        builder.handle(event, range);
    }
    builder.finish(truncated)
}

/// Cuts `source` to [`MAX_MESSAGE_BYTES`] on a character boundary.
///
/// The boundary walk is the whole point and the reason this is not a one-liner:
/// slicing a `str` at a non-boundary index **panics**, and the input is
/// attacker-controlled, so a panic here is a remote crash (`AGENTS.md` §2.1).
/// Walking back is at most three steps, because a UTF-8 character is at most
/// four bytes, so this is O(1) rather than a scan.
fn truncate_to_char_boundary(source: &str) -> (&str, bool) {
    if source.len() <= MAX_MESSAGE_BYTES {
        return (source, false);
    }
    let mut end = MAX_MESSAGE_BYTES;
    while end > 0 && !source.is_char_boundary(end) {
        end -= 1;
    }
    (&source[..end], true)
}

/// Removes the line ending that belongs to the closing fence, if there is one.
///
/// **Exactly one**, and that is the point. The parser hands over the block's raw
/// body, and the newline that terminates the last line is part of the *fencing*,
/// not of the code — CommonMark says so, and so does every HTML renderer. Keeping
/// it would put a blank line at the end of every code block, which a renderer
/// would draw as an empty line the author never wrote. Removing *all* of them
/// would be just as wrong: a blank line between two statements is content, and a
/// code block that ends in one is common.
///
/// `\r\n` is handled as a unit, so a CRLF message does not leave a stray `\r`
/// that a renderer would draw as a control glyph.
fn without_closing_newline(text: String) -> String {
    let mut trimmed = text;
    if trimmed.ends_with('\n') {
        trimmed.pop();
        if trimmed.ends_with('\r') {
            trimmed.pop();
        }
    }
    trimmed
}

/// The character substituted for every neutralised Unicode control.
///
/// `U+FFFD REPLACEMENT CHARACTER`. Chosen over deletion because deletion is
/// silent: `gnp<0x202E>exe` and `gnpexe` would become indistinguishable, and the
/// reader would have no idea the message had been tampered with. A visible
/// diamond is the correct outcome — the user learns something is wrong with the
/// text they are looking at.
const REPLACEMENT: char = '\u{FFFD}';

/// Whether `character` must never reach a renderer as itself.
///
/// Two classes, and the split between them is the decision:
///
/// - **Directional *formatting*** — `U+202A`–`U+202E` (LRE/RLE/PDF/LRO/RLO) and
///   `U+2066`–`U+206F` (LRI/RLI/FSI/PDI plus the deprecated forms). These
///   *reorder* a run. `U+202E RLO` is the classic: a filename can be displayed
///   with its extension moved to the front, so a message can display as
///   something the author did not write. No XSS, no markup, no interpreter —
///   just text shown as other text, which is the actual question.
/// - **`U+FEFF` ZERO WIDTH NO-BREAK SPACE** — invisible, and the oldest trick
///   there is for hiding the first character of a line.
///
/// **Not** in this set, and deliberately: `U+200E`/`U+200F`/`U+061C` (the
/// directional *marks*) and `U+200B`/`U+200C`/`U+200D` (zero-width space,
/// non-joiner, joiner). Those are load-bearing in Arabic, Hebrew, Devanagari
/// and in emoji sequences, and mangling them would break real text to stop an
/// attack that does not need them. Marks insert a hint; formatting reorders a
/// run. Only the second is a weapon.
fn must_be_neutralized(character: char) -> bool {
    matches!(character,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}')
}

/// Copies `text`, replacing every character from [`must_be_neutralized`].
///
/// The fast path matters: ordinary text has none of them, and a chat client
/// copies every character of every message, so the common case must not pay for
/// a per-character branch it does not need. A full scan is still required to
/// *know* the fast path applies, but a `str` scan is memchr-fast and this
/// returns the original text untouched.
fn neutralize(text: &str) -> (String, usize) {
    if !text.chars().any(must_be_neutralized) {
        return (text.to_owned(), 0);
    }
    let mut replaced = 0;
    let owned: String = text
        .chars()
        .map(|character| {
            if must_be_neutralized(character) {
                replaced += 1;
                REPLACEMENT
            } else {
                character
            }
        })
        .collect();
    (owned, replaced)
}

/// What a link target is allowed to be.
///
/// An **allowlist**, and the allowlist is the design: a denylist has to name
/// every dangerous scheme, and a scheme nobody thought of is then allowed —
/// which is the failure mode a security rule must not have. `file:` and `data:`
/// and `customapp:` and whatever a future OS registers all have the same problem,
/// and the allowlist gives them all the same answer: no.
///
/// - `http` / `https` — the point of a link in a chat message.
/// - `mailto` — **deliberately in.** It is non-executable, the OS handler is the
///   same class of handler as the browser's, "email me about this" is a normal
///   utterance in a team channel, and it grants an attacker nothing: a
///   recipient address and a compose window are not an execution, an
///   exfiltration or a privilege primitive. The cost is real and small — a
///   message can put an address in front of a user who clicks — but that is
///   social engineering, which no scheme filter can solve, and refusing
///   `mailto:` would break a legitimate utterance to no security benefit.
const ALLOWED_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Decides whether a link target may be offered to the user as a clickable link.
///
/// Returns the target to keep, or `None` to refuse it. On refusal the caller
/// keeps the link's **text** and drops the target, so the message still reads
/// correctly — see the module docs, §4.
///
/// # Arguments
///
/// * `raw` - the target exactly as the parser produced it, untrusted.
///
/// # Errors
///
/// None. Refusal is a return value, not an error, because a message containing
/// a `javascript:` link is a message, not a failure.
///
/// # Example
///
/// ```
/// use sh_nexus::core::markdown::classify_target;
///
/// assert_eq!(classify_target("https://example.com/a"), Some("https://example.com/a".to_owned()));
/// assert_eq!(classify_target("mailto:team@example.com"), Some("mailto:team@example.com".to_owned()));
///
/// // Refused: executable, local-filesystem, and unknown schemes.
/// assert_eq!(classify_target("javascript:alert(1)"), None);
/// assert_eq!(classify_target("file:///etc/passwd"), None);
/// ```
pub fn classify_target(raw: &str) -> Option<String> {
    // 1. Trim what a URL specification says to trim, so `[a]( https://x )` is a
    //    link rather than a target with a leading space that then fails the
    //    scheme test for the wrong reason.
    let trimmed = raw.trim_matches(|c: char| c.is_ascii_whitespace());

    // 2. Bound the length. A hostile message should not be able to put a
    //    megabyte into a String that a renderer will lay out.
    if trimmed.is_empty() || trimmed.len() > MAX_TARGET_BYTES {
        return None;
    }

    // 3. No control characters anywhere. Any remaining one is an attempt to
    //    split a scheme token from the parser's point of view, and none of them
    //    belongs in a URL a person clicks.
    if trimmed.chars().any(char::is_control) {
        return None;
    }

    // 4. The scheme is everything before the first colon, and it must be a
    //    *valid scheme*: ASCII alphanumerics, `+`, `-`, `.`, at least one
    //    character.
    //
    //    Requiring validity rather than searching for an allowed prefix is what
    //    defeats the obfuscations: `java\tscript:`, `java\nscript:` and
    //    `\u{0}javascript:` all put a non-scheme character in the pre-colon
    //    segment, so they are rejected here before `javascript` is ever
    //    compared. A pattern that looked for "http" anywhere in the target
    //    would pass all three.
    let Some((scheme, _rest)) = trimmed.split_once(':') else {
        // No colon at all: a relative link or a bare `#anchor`. Refused, and
        // not as a security measure -- a chat client has no base URL to resolve
        // a relative link against, and resolving one against the API origin
        // would be a guess dressed up as a feature.
        return None;
    };
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return None;
    }

    // 5. The allowlist, compared ASCII-case-insensitively because schemes are
    //    case-insensitive per RFC 3986, and `JAVASCRIPT:` must not slip past a
    //    lowercase comparison.
    let lower = scheme.to_ascii_lowercase();
    if !ALLOWED_SCHEMES.iter().any(|allowed| *allowed == lower) {
        return None;
    }

    // 6. The *original* case is returned, not the lowercased scheme: this is what
    //    the user clicks and what the OS handler is given, and rewriting a peer's
    //    URL is not this module's business.
    Some(trimmed.to_owned())
}

/// An inline frame open at the current point in the event stream.
///
/// A separate stack from [`Frame`], and it is what makes [`Style`] a union
/// rather than a stack: a style frame *contributes* and does not nest, so
/// [`Builder::current_style`] ors the open ones together and the result is
/// independent of the order they were opened in.
enum InlineFrame {
    /// Contributes a style to the runs inside it.
    Style(Style),
    /// Contributes nothing.
    ///
    /// For a construct whose payload is discarded but whose children are still
    /// text — an image, whose target is dropped (module docs, §4) and whose alt
    /// text is kept. The frame exists so the construct's `End` event has
    /// something to pop; without it, the `End` would pop a block frame and
    /// desynchronise the stack.
    Plain,
    /// An open link, collecting the runs inside it.
    Link {
        /// The allow-listed target.
        target: String,
        /// The runs inside, in reading order.
        children: Vec<Inline>,
        /// The source range of the whole link.
        source: Range<usize>,
    },
}

/// A block frame open at the current point in the event stream.
///
/// There is deliberately **no `Root` variant**: the document's own block list
/// lives on [`Builder::root`]. A root frame would have to be skipped in every
/// place that looks for the innermost block-hosting container, and skipping it is
/// exactly the sort of special case that a later edit forgets.
enum Frame {
    /// An open block quote.
    Quote {
        /// Blocks collected so far.
        blocks: Vec<Block>,
    },
    /// An open list.
    List {
        /// `true` for an ordered list.
        ordered: bool,
        /// First number of an ordered list.
        start: u64,
        /// Items collected so far.
        items: Vec<ListItem>,
    },
    /// An open list item.
    Item {
        /// Task-list state, if any.
        checked: Option<bool>,
        /// Blocks collected so far.
        blocks: Vec<Block>,
    },
    /// An open paragraph, collecting runs.
    Paragraph {
        /// Runs collected so far.
        spans: Vec<Inline>,
    },
    /// An open heading, collecting runs.
    Heading {
        /// Its level.
        level: Heading,
        /// Runs collected so far.
        spans: Vec<Inline>,
    },
    /// An open code block, collecting raw text.
    Code {
        /// The declared language, if fenced with one.
        language: Option<String>,
        /// The verbatim text collected so far.
        text: String,
        /// The source range, widened as text arrives.
        source: Range<usize>,
    },
    /// A block whose content is rendered as literal inline text.
    ///
    /// Two producers, and the second is the reason this is a single variant
    /// rather than an `Html` variant plus a `Table` variant: an HTML block, and
    /// **any block construct this module does not enable**. The second is
    /// unreachable today — see the module docs on `Options` — and it is here so
    /// that turning on, say, `ENABLE_TABLES` degrades a table to a paragraph of
    /// its cell text instead of dropping the author's words.
    Literal {
        /// Runs collected so far.
        spans: Vec<Inline>,
    },
}

impl Frame {
    /// This frame's block list, if it is a container that holds blocks.
    fn as_block_host(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Self::Quote { blocks } | Self::Item { blocks, .. } => Some(blocks),
            // A `List` holds items; a leaf holds runs or text. Neither takes a
            // bare block, which is why `Builder::push_block` searches *down* the
            // stack rather than looking only at the top.
            Self::List { .. }
            | Self::Paragraph { .. }
            | Self::Heading { .. }
            | Self::Code { .. }
            | Self::Literal { .. } => None,
        }
    }

    /// This frame's item list, if it is a list.
    fn as_list_host(&mut self) -> Option<&mut Vec<ListItem>> {
        if let Self::List { items, .. } = self {
            Some(items)
        } else {
            None
        }
    }

    /// Whether this frame is a list. Read-only, for the search down the stack.
    fn is_list(&self) -> bool {
        matches!(self, Self::List { .. })
    }

    /// This frame's checkbox slot, if it is a list item.
    fn as_task_host(&mut self) -> Option<&mut Option<bool>> {
        if let Self::Item { checked, .. } = self {
            Some(checked)
        } else {
            None
        }
    }
}

/// What the parser is building.
struct Builder {
    /// The document's own blocks. Not a [`Frame`], see above.
    root: Vec<Block>,
    /// Open block frames, outermost first. Length is the container nesting depth.
    frames: Vec<Frame>,
    /// Runs collected with no owning frame yet.
    ///
    /// **This is the load-bearing field of the whole builder, and its absence is
    /// the bug a first draft of this module had.** A tight list item emits
    /// `Start(Item) … Text … End(Item)` with no paragraph at all, so a builder
    /// that opens a paragraph to hold that text has one more frame open than
    /// there are `End` events to close it — and every later `End` pops the wrong
    /// frame, nesting each list item inside the previous one. Buffering the runs
    /// until a frame actually closes keeps the frame count equal to the event
    /// count, so `Builder::close` can pop exactly one frame per `End` with no
    /// matching to do and no way to desynchronise.
    ///
    /// It is also what makes refusing a frame at [`MAX_BLOCK_DEPTH`]
    /// lossless: the text that would have gone into the refused frame stays
    /// buffered and is handed to the frame that does close.
    pending: Vec<Inline>,
    /// Open inline frames, innermost last.
    inline: Vec<InlineFrame>,
    /// Link targets refused or discarded so far.
    rejected_links: usize,
    /// Unicode characters replaced so far.
    neutralized: usize,
    /// Whether [`MAX_BLOCK_DEPTH`] was reached.
    depth_limited: bool,
}

impl Builder {
    fn new() -> Self {
        Self {
            root: Vec::new(),
            frames: Vec::new(),
            pending: Vec::new(),
            inline: Vec::new(),
            rejected_links: 0,
            neutralized: 0,
            depth_limited: false,
        }
    }

    /// Feeds one parser event through.
    fn handle(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.open(tag, range),
            // The end event's *value* is deliberately unused, and that is a
            // design decision rather than an oversight. pulldown documents its
            // stream as a preorder traversal with "Start and end events
            // guaranteed to be balanced", so the top of the frame stack is
            // always the thing that just ended, and the frame's own variant
            // already says what it is. Matching the two against each other would
            // add a comparison that can only ever agree, plus an arm for the case
            // where it did not — an arm no test could reach and no input could
            // cause. [`Builder::pending`] is what makes that sound: it guarantees
            // one frame is pushed per `Start` and popped per `End`.
            Event::End(_) => self.close(),
            Event::Text(text) => {
                let (owned, count) = neutralize(&text);
                self.neutralized += count;
                self.push_text(owned, range);
            }
            // Inline code. NOT a frame and NOT a container: it is one run of
            // preformatted text, so it becomes a run carrying `Style::CODE` and
            // its content is never parsed. That is the distinction from
            // `Frame::Code` the module docs insist on, and it lives here rather
            // than in a doc comment because the two cases sit a few lines apart in
            // the parser's output and only one of them is in this arm.
            //
            // `CODE` is passed as an *extra* style, unioned with whatever is
            // already open: `**`code`**` is legitimately bold code, and a code
            // span inside emphasis has to keep the emphasis.
            Event::Code(text) => {
                let (owned, count) = neutralize(&text);
                self.neutralized += count;
                self.push_text_with(owned, range, Style::CODE);
            }
            // Raw HTML, both block and inline. Preserved as literal text: the
            // module docs, section 4, argue why dropping it is worse and
            // interpreting it is not available.
            Event::Html(text) | Event::InlineHtml(text) => {
                let (owned, count) = neutralize(&text);
                self.neutralized += count;
                self.push_text(owned, range);
            }
            // A single newline, for BOTH kinds of break. CommonMark renders a
            // soft break as a space, because in prose a wrapped line is not a
            // line the author chose. In a chat message it is: people write
            // multi-line messages and mean every line. So a soft break is a
            // newline here, which is a deliberate departure from prose rendering
            // and the reason `plain_text` of a two-line message keeps its shape.
            //
            // A hard break is the same newline, because the distinction between
            // "wrapped" and "meant" has no visual meaning once both are one.
            Event::SoftBreak | Event::HardBreak => self.push_text("\n".to_owned(), range),
            // Thematic break. Not a Start/End pair, so it is emitted directly.
            Event::Rule => self.push_block(Block::Rule),
            Event::TaskListMarker(checked) => {
                // Reaches the enclosing item, which is **not always the frame
                // directly below**: a *loose* task-list item has a paragraph
                // inside it, so the marker arrives with a `Paragraph` on top of
                // the stack. Searching down for the nearest item rather than
                // looking only at the top is what makes `- [x] a\n\n  more`
                // report a checkbox.
                let host = self
                    .frames
                    .iter_mut()
                    .rev()
                    .find_map(|frame| frame.as_task_host());
                if let Some(slot) = host {
                    *slot = Some(checked);
                }
            }
            // Constructs this module does not enable: inline and display math, and
            // footnote references. One arm because the treatment is the same —
            // there is nowhere to put them and no chat surface for any of them.
            // Unreachable with the `Options` set in [`parse`]; see the module
            // docs. Their *text* is not lost, because the parser emits the body
            // of a footnote definition as ordinary `Text` events and those do
            // reach a paragraph.
            Event::InlineMath(_) | Event::DisplayMath(_) | Event::FootnoteReference(_) => {}
        }
    }

    /// Reacts to a `Start` event.
    fn open(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            // --- inline ---
            Tag::Emphasis => self.inline.push(InlineFrame::Style(Style::ITALIC)),
            Tag::Strong => self.inline.push(InlineFrame::Style(Style::BOLD)),
            Tag::Strikethrough => self.inline.push(InlineFrame::Style(Style::STRIKETHROUGH)),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                // An email autolink — `<a@b.com>` — carries a **bare address**:
                // CommonMark's renderer supplies the `mailto:`, and this module has
                // to as well. Without it a legitimate address looks exactly like a
                // relative URL and is refused, which would be refusing the
                // correspondence `mailto:` was allowed for.
                let target = if matches!(link_type, LinkType::Email) {
                    classify_target(&format!("mailto:{dest_url}"))
                } else {
                    classify_target(&dest_url)
                };
                match target {
                    Some(target) => self.inline.push(InlineFrame::Link {
                        target,
                        children: Vec::new(),
                        source: range.clone(),
                    }),
                    // Refused. A `Plain` frame keeps the link's text in the tree as
                    // ordinary runs, so the message still reads correctly, and the
                    // refusal is counted rather than silent.
                    None => {
                        self.rejected_links += 1;
                        self.inline.push(InlineFrame::Plain);
                    }
                }
            }
            // An image is deliberately NOT treated as a link. Its only possible
            // use is a network fetch to a host the message author chose, from
            // inside a chat window, carrying this client's address — a read
            // receipt. Attachments are `Message::attachments`, validated at the
            // network boundary. So the alt text is kept and the target is counted
            // as one this client will not act on.
            Tag::Image { .. } => {
                self.rejected_links += 1;
                self.inline.push(InlineFrame::Plain);
            }

            // --- block ---
            Tag::Paragraph => self.open_leaf(Frame::Paragraph { spans: Vec::new() }),
            Tag::Heading { level, .. } => self.open_leaf(Frame::Heading {
                level: level.into(),
                spans: Vec::new(),
            }),
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(name) if !name.is_empty() => {
                        // A declared language, bounded. It selects a highlighter in
                        // Phase 5 and is never executed, but an unbounded string
                        // from a peer should not reach a renderer either.
                        Some(
                            name.chars()
                                .take(MAX_TARGET_BYTES)
                                .filter(|character| !character.is_control())
                                .collect(),
                        )
                    }
                    CodeBlockKind::Fenced(_) | CodeBlockKind::Indented => None,
                };
                self.open_leaf(Frame::Code {
                    language,
                    text: String::new(),
                    source: range.clone(),
                });
            }
            Tag::BlockQuote(_) => self.open_container(Frame::Quote { blocks: Vec::new() }),
            Tag::List(Some(first)) => self.open_container(Frame::List {
                ordered: true,
                start: first,
                items: Vec::new(),
            }),
            Tag::List(None) => self.open_container(Frame::List {
                ordered: false,
                start: 1,
                items: Vec::new(),
            }),
            Tag::Item => {
                // An item outside a list cannot happen. If it somehow did, the item
                // is still wrapped in a list rather than discarded, because losing
                // a block is worse than synthesising a container.
                let already_in_list = self.frames.iter().any(Frame::is_list);
                if !already_in_list {
                    self.open_container(Frame::List {
                        ordered: false,
                        start: 1,
                        items: Vec::new(),
                    });
                }
                self.open_container(Frame::Item {
                    checked: None,
                    blocks: Vec::new(),
                });
            }
            // An HTML block, and — see `Frame::Literal` — any block construct this
            // module does not enable. Both become a paragraph of their own literal
            // text, so nothing an author wrote is ever dropped.
            Tag::HtmlBlock
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_) => self.open_leaf(Frame::Literal { spans: Vec::new() }),
        }
    }

    /// Pushes a block **container**, honouring the depth cap.
    ///
    /// Containers are the only thing that nests, so they are the only thing the
    /// cap applies to. Past the cap the frame is not pushed: the runs that would
    /// have gone into it stay in `pending` and are handed to the frame that does
    /// close, so the nesting flattens and **no text is lost**.
    ///
    /// **No open leaf is closed first**, and that is deliberate rather than an
    /// oversight. A first draft of this builder did close one, and it was
    /// unreachable code: pulldown emits the `End` event for a block before the
    /// `Start` of the next one, so a leaf is never open when a container starts.
    /// Leaving it in would have been defence against a case the parser has
    /// already excluded, at the cost of six lines that no test can execute.
    fn open_container(&mut self, frame: Frame) {
        self.flush_pending();
        if self.frames.len() >= MAX_BLOCK_DEPTH {
            self.depth_limited = true;
            return;
        }
        self.frames.push(frame);
    }

    /// Pushes a block **leaf**, with no depth cap.
    ///
    /// A leaf cannot nest inside another leaf — a leaf is only ever closed by a
    /// matching `End` — and it contributes **0** to [`Block::container_depth`], so
    /// allowing one at the limit costs no recursion depth a caller has to reason
    /// about. Capping leaves as well would make a message with exactly
    /// [`MAX_BLOCK_DEPTH`] quotes report
    /// [`was_depth_limited`](Document::was_depth_limited), which is a false report
    /// about a message that is inside the bound.
    fn open_leaf(&mut self, frame: Frame) {
        self.flush_pending();
        self.frames.push(frame);
    }

    /// Closes the innermost open frame — one frame, per `End` event.
    fn close(&mut self) {
        if !self.inline.is_empty() {
            self.close_inline();
            return;
        }
        // Buffered runs belong to the block that is ending here, not to whatever
        // comes after it. Taking them *before* the pop is what makes a tight list
        // item's text land inside that item.
        let loose = std::mem::take(&mut self.pending);
        let Some(frame) = self.frames.pop() else {
            return;
        };
        self.emit_frame(frame, loose);
    }

    /// Closes the innermost inline frame and materialises it.
    fn close_inline(&mut self) {
        let Some(frame) = self.inline.pop() else {
            return;
        };
        let InlineFrame::Link {
            target,
            children,
            source,
        } = frame
        else {
            // `Style` and `Plain` frames carry no payload; their effect was
            // already applied to the runs collected inside them.
            return;
        };
        self.push_inline(Inline::Link(LinkSpan::new(target, children, source)));
    }

    /// Turns buffered runs into a paragraph, in the right order.
    ///
    /// Called before a new frame is pushed. If runs were buffered and a
    /// span-collecting leaf were then opened, the leaf would take the *next* run
    /// and the buffered ones would be emitted afterwards — the message's text in
    /// the wrong order, which is worse than losing it.
    ///
    /// There is deliberately no "fill the open leaf instead" branch. A leaf is
    /// only ever open when a run went straight into it, and a run only stays
    /// buffered when no leaf was open, so the two states are exclusive and that
    /// branch could never be taken.
    fn flush_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let loose = std::mem::take(&mut self.pending);
        self.push_block(Block::Paragraph { spans: loose });
    }

    /// Turns a closed frame into a [`Block`] and adds it to the enclosing
    /// container, handing it the runs buffered while it was open.
    fn emit_frame(&mut self, frame: Frame, loose: Vec<Inline>) {
        match frame {
            Frame::Quote { mut blocks } => {
                blocks.append_loose(loose);
                self.push_block(Block::Quote { blocks });
            }
            Frame::List {
                ordered,
                start,
                mut items,
            } => {
                if !loose.is_empty() {
                    // Runs buffered directly inside a list, with no item. They
                    // become an item of their own rather than being dropped.
                    items.push(ListItem::new(None, vec![Block::Paragraph { spans: loose }]));
                }
                // A list with no items draws nothing and would contribute a
                // phantom newline to `plain_text`, for the same reason an empty
                // paragraph is dropped.
                if items.is_empty() {
                    return;
                }
                self.push_block(Block::List {
                    ordered,
                    start,
                    items,
                });
            }
            Frame::Item {
                checked,
                mut blocks,
            } => {
                blocks.append_loose(loose);
                let item = ListItem::new(checked, blocks);
                // The item belongs to the nearest enclosing list, which is not
                // necessarily the frame directly below: a list nested in a list
                // item puts another list between them.
                let host = self.frames.iter_mut().rev().find_map(Frame::as_list_host);
                if let Some(items) = host {
                    items.push(item);
                    return;
                }
                // No list anywhere above. `open` synthesises one for an item that
                // has none, so this is unreachable; kept so the item survives
                // rather than vanishing if that invariant is ever broken.
                self.push_block(Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![item],
                });
            }
            Frame::Paragraph { mut spans } | Frame::Literal { mut spans } => {
                // A paragraph with no runs draws nothing and contributes a stray
                // newline to `plain_text`. Markdown has no "intentionally empty
                // paragraph", and the parser does emit the empty frame for a few
                // list shapes, so it is dropped here rather than shown.
                if spans.is_empty() {
                    return;
                }
                spans.extend(loose);
                self.push_block(Block::Paragraph { spans });
            }
            Frame::Heading { level, mut spans } => {
                spans.extend(loose);
                if spans.is_empty() {
                    return;
                }
                self.push_block(Block::Heading { level, spans });
            }
            Frame::Code {
                language,
                mut text,
                mut source,
            } => {
                // A code block's body arrives as `Text` events and is written
                // straight into `text` by `push_text_with`, so `loose` is empty.
                // It is joined in anyway: the content is the author's code, and
                // dropping any of it would be the worst outcome in this module.
                for node in &loose {
                    text.push_str(&node.text());
                    source.end = source.end.max(node.source().end);
                }
                self.push_block(Block::CodeBlock {
                    language,
                    text: without_closing_newline(text),
                    source,
                });
            }
        }
    }

    /// Adds a block to the innermost container that can hold one.
    ///
    /// Searches the whole stack rather than only its top, because the top is
    /// often a leaf or a list — and a block that went into neither would be lost.
    fn push_block(&mut self, block: Block) {
        let host = self
            .frames
            .iter_mut()
            .rev()
            .find_map(Frame::as_block_host)
            .unwrap_or(&mut self.root);
        host.push(block);
    }

    /// The style in force at this point: the union of every open style frame.
    ///
    /// A **union**, not a stack, and that is the design decision from the module
    /// docs, §2, made executable: `**a *b* c**` puts `b` inside `BOLD` and
    /// `ITALIC`, and both apply.
    fn current_style(&self) -> Style {
        self.inline.iter().fold(Style::empty(), |style, frame| {
            if let InlineFrame::Style(flag) = frame {
                style.union(*flag)
            } else {
                style
            }
        })
    }

    /// Appends a run of text with the current style.
    fn push_text(&mut self, text: String, source: Range<usize>) {
        self.push_text_with(text, source, Style::NONE);
    }

    /// Appends a run, carrying the current style **plus** `extra`.
    ///
    /// `extra` is a style to *add*, not to use instead. Inline code is the case
    /// that makes this necessary: `Event::Code` is a single event rather than a
    /// `Start`/`End` pair, so the code span never appears on the open-frame stack
    /// and cannot contribute its own flag there. Unioning here means
    /// ``**`code`**`` is bold code, which is what the author wrote, and
    /// `*`code`*` is italic code.
    ///
    /// A same-styled predecessor is merged, which is why ordinary prose is one
    /// span rather than one per line and why two spellings of the same emphasis
    /// produce the same tree. The merge also makes the tree canonical, which is
    /// what work unit 1C-2's segment cache needs to be worth having.
    fn push_text_with(&mut self, text: String, source: Range<usize>, extra: Style) {
        // An empty run would be a span that renders nothing, carries a range, and
        // costs an allocation.
        if text.is_empty() {
            return;
        }
        let style = self.current_style().union(extra);
        // 1. An open code block takes raw text, not runs, and its content is
        //    never parsed. This is the half of the inline-code/code-block
        //    distinction that lives in the event handling.
        if let Some(Frame::Code {
            text: body,
            source: extent,
            ..
        }) = self.frames.last_mut()
        {
            body.push_str(&text);
            extent.end = extent.end.max(source.end);
            return;
        }
        // 2. An open link collects the runs inside it. **Searched down the
        //    stack, not looked up at the top**: `[**a**](x)` has a `Strong` style
        //    frame above the link, so a top-of-stack check would put the link's own
        //    text beside it instead of inside it, and produce a link with no
        //    content.
        if let Some(children) = self.open_link() {
            children.push_merged(TextSpan::new(style, text, source));
            return;
        }
        // 3. An open span-collecting leaf takes the run.
        let open_leaf = matches!(
            self.frames.last(),
            Some(Frame::Paragraph { .. })
                | Some(Frame::Heading { .. })
                | Some(Frame::Literal { .. })
        );
        if open_leaf {
            if let Some(
                Frame::Paragraph { spans }
                | Frame::Heading { spans, .. }
                | Frame::Literal { spans },
            ) = self.frames.last_mut()
            {
                spans.push_merged(TextSpan::new(style, text, source));
            }
            return;
        }
        // 4. Nothing owns it yet. A tight list item arrives exactly like this, and
        //    buffering is what keeps the frame count equal to the `End` count.
        self.pending.push_merged(TextSpan::new(style, text, source));
    }

    /// Appends an already-built inline node — a finished link — to wherever the
    /// runs around it are going.
    /// The innermost open link's children, if a link is open at all.
    ///
    /// Searches the whole inline stack rather than only its top, because a style
    /// frame can sit above the link: `[**a**](x)` has `Strong` open when the run
    /// arrives, and the run belongs **inside** the link. A top-of-stack check puts
    /// the link's own text beside it and produces a link with no content.
    fn open_link(&mut self) -> Option<&mut Vec<Inline>> {
        self.inline.iter_mut().rev().find_map(|frame| match frame {
            InlineFrame::Link { children, .. } => Some(children),
            InlineFrame::Style(_) | InlineFrame::Plain => None,
        })
    }

    /// Appends an already-built inline node — a finished link — to wherever the
    /// runs around it are going.
    fn push_inline(&mut self, inline: Inline) {
        if let Some(children) = self.open_link() {
            children.push(inline);
            return;
        }
        let open_leaf = matches!(
            self.frames.last(),
            Some(Frame::Paragraph { .. })
                | Some(Frame::Heading { .. })
                | Some(Frame::Literal { .. })
        );
        if open_leaf {
            if let Some(
                Frame::Paragraph { spans }
                | Frame::Heading { spans, .. }
                | Frame::Literal { spans },
            ) = self.frames.last_mut()
            {
                spans.push(inline);
            }
            return;
        }
        self.pending.push(inline);
    }

    /// Finishes parsing and returns the document.
    fn finish(mut self, truncated: bool) -> Document {
        // Close whatever is still open. A well-formed source leaves nothing
        // open; an unclosed `**` or an unterminated `>` leaves frames, and
        // closing them is what keeps a malformed message's text.
        while !self.inline.is_empty() {
            self.close_inline();
        }
        while !self.frames.is_empty() {
            self.close();
        }
        self.flush_pending();
        Document {
            blocks: std::mem::take(&mut self.root),
            truncated,
            depth_limited: self.depth_limited,
            rejected_links: self.rejected_links,
            neutralized: self.neutralized,
        }
    }
}

/// Appends a run to a run list, merging it into the last one when the style
/// matches.
///
/// The merge is what makes the tree *canonical*: `**a** *b*` and `**a *b***`
/// differ in the source and do not differ in the tree, so a segment cache keyed
/// on the tree (work unit 1C-2) hits on both. It also keeps ordinary prose at
/// one span rather than one per line, which is the difference between one
/// allocation and one per newline in a message that is mostly text.
trait PushMerged {
    /// Appends `run`, merging into the last element if it is a same-styled run.
    fn push_merged(&mut self, run: TextSpan);
}

impl PushMerged for Vec<Inline> {
    fn push_merged(&mut self, run: TextSpan) {
        if let Some(Inline::Text(previous)) = self.last_mut() {
            if previous.style == run.style {
                previous.text.push_str(&run.text);
                // Widened, not replaced: the two runs came from different parser
                // events and may not be adjacent in the source, so the merged
                // range can cover source the text does not contain. `source` is
                // provenance, and this is the honest provenance of a merge.
                previous.source.end = previous.source.end.max(run.source.end);
                return;
            }
        }
        self.push(Inline::Text(run));
    }
}

/// Appends buffered runs to a container's block list as one paragraph.
trait AppendLoose {
    /// Adds `runs` as a paragraph, unless there are none.
    fn append_loose(&mut self, runs: Vec<Inline>);
}

impl AppendLoose for Vec<Block> {
    fn append_loose(&mut self, runs: Vec<Inline>) {
        if runs.is_empty() {
            return;
        }
        self.push(Block::Paragraph { spans: runs });
    }
}
