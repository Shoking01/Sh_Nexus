//! The block-level rendering of a message, and the heights that come out of it.
//!
//! **`ui/markdown.rs` is where step 4 of ADR-006's plan is decided, and this
//! file is where that decision is checked.** ADR-006 chose `gpui::List` over
//! `gpui::UniformList` because chat rows are genuinely variable-height, and a
//! uniform estimator misplaces every row after the first tall one. That is a
//! claim about *rendered height*, and a rendered height is only observable from a
//! real window through a `Bounds` — which is why `block_element` now carries a
//! debug selector per block.
//!
//! **The file this completes had 100 unexecuted regions and 57% coverage**, and
//! the uncovered part was precisely the block dispatcher: heading, quote, list,
//! rule, code block, and the four-way list marker. Runs and colours were covered;
//! **the blocks that produce the variable heights were not.** That is the part
//! ADR-006's decision rests on, and it is why this file exists rather than a
//! coverage number.
//!
//! Each block is reached two ways on purpose. `list_marker` is pure and is called
//! directly, because asserting a bullet glyph out of a `Bounds` would not test
//! which arm ran. The rendered path is checked separately, and the height
//! assertion there is the one that matters: a code block and a one-word paragraph
//! occupy different vertical space, and if they did not, the reason this project
//! did not use a uniform estimator would be gone.

use gpui::{px, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window};
use rstest::rstest;
use sh_nexus::core::markdown::{self as core_markdown, Block, Document};
use sh_nexus::ui::markdown::{self as ui_markdown, blocks, list_marker};
use sh_nexus::ui::Colors;

/// The source every rendered test shares: one of every block kind, in the order
/// `block.{index}` names them.
const SOURCE: &str = "\
# A heading

A paragraph of ordinary prose.

> A quotation.

- bullet
- item

1. first
2. second

- [x] done
- [ ] not done

---

```rust
fn main() {}
```
";

/// A one-window view that renders one message's blocks and nothing else.
///
/// The smallest harness that can answer the only question this file asks. A
/// whole `MessageList` would also answer it, and would couple these tests to the
/// list's caching, tail-following and recycling — none of which is what is being
/// checked here.
struct BlocksHarness {
    document: Document,
}

impl BlocksHarness {
    fn new(source: &str) -> Self {
        Self {
            document: core_markdown::parse(source),
        }
    }
}

impl Render for BlocksHarness {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        blocks(self.document.blocks(), Colors::default(), window)
    }
}

/// Leaks a selector so `debug_bounds` can take it, which wants `&'static str`.
///
/// **The leak is the honest cost and it is bounded by the test run, not the
/// process.** `debug_bounds` is a debug hook that accepts a `&'static str` because
/// the elements carry static debug selectors, so a test that composes a selector
/// at runtime has no way to avoid this short of changing GPUI. The allocation
/// happens once per composed selector in a test binary that exits afterwards, so
/// the leak is bounded by the number of assertions — not by the number of
/// messages, and not by anything a user can reach. It is written as a named
/// function rather than a bare `Box::leak` so that the cost is stated in one
/// place instead of appearing as a leak in four.
fn selector(parts: &[&str], index: usize) -> &'static str {
    Box::leak(format!("{}.{index}", parts.join(".")).into_boxed_str())
}

/// Renders [`SOURCE`] in a real window and runs it to completion.
fn rendered(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (_view, cx) = cx.add_window_view(|_, _| BlocksHarness::new(SOURCE));
    cx.run_until_parked();
    cx
}

// ---------------------------------------------------------------------------
// The pure half: `list_marker`, whose whole behaviour is a four-way decision.
// ---------------------------------------------------------------------------

/// **The four arms, and the arm is the test.** `core/markdown.rs` keeps
/// `Option<bool>` as a tri-state precisely so that "not a task" and "an unchecked
/// task" do not render the same, and this is the only place that tri-state
/// becomes visible. Collapsing `None` and `Some(false)` would make a task list
/// indistinguishable from a bullet list, which is the mistake the tri-state
/// exists to prevent.
#[rstest]
#[case(false, 1_u64, 0_usize, None, "•")]
#[case(true, 1, 0, None, "1.")]
#[case(true, 1, 4, None, "5.")]
#[case(true, 7, 0, None, "7.")]
#[case(true, 7, 2, None, "9.")]
#[case(false, 1, 0, Some(true), "[x]")]
#[case(false, 1, 0, Some(false), "[ ]")]
#[case(true, 7, 2, Some(true), "[x]")]
#[case(true, 7, 2, Some(false), "[ ]")]
fn a_list_item_shows_the_marker_its_tri_state_and_number_imply(
    #[case] ordered: bool,
    #[case] start: u64,
    #[case] index: usize,
    #[case] checked: Option<bool>,
    #[case] expected: &str,
) {
    assert_eq!(list_marker(ordered, start, index, checked), expected);
}

/// **The number is `start + index`, and the case that pins it is the one that
/// would be "simplified" away.** An ordered list that begins at 7 must read
/// 7, 8, 9; a list that ignores `start` reads 1, 2, 3 and looks plausible in a
/// test that only ever uses `start: 1`. And a task list's marker must win over the
/// number, or `- [x] item` renders as `3.`
#[test]
fn an_ordered_list_counts_from_its_declared_start() {
    let markers: Vec<String> = (0..4).map(|i| list_marker(true, 7, i, None)).collect();
    assert_eq!(
        markers,
        ["7.", "8.", "9.", "10."],
        "a list that declares it starts at 7 must read 7, 8, 9, 10 and not 1, 2, 3, 4"
    );
}

/// `u64::MAX` as a start would overflow a plain `+` and panic in debug.
///
/// The project bans panics in production paths (`AGENTS.md` §2.1), and this is
/// production code reached from message content, so the saturating add is
/// load-bearing rather than cautious: a crash triggered by what somebody typed in
/// a list marker is a crash the project does not have.
#[test]
fn an_absurd_list_start_saturates_rather_than_panicking() {
    let marker = list_marker(true, u64::MAX, 3, None);
    assert_eq!(
        marker, "18446744073709551615.",
        "saturating_add pins at u64::MAX and still renders a number, rather than \
         wrapping to a small one or panicking in debug"
    );
}

// ---------------------------------------------------------------------------
// The rendered half: every block kind reaches the window.
// ---------------------------------------------------------------------------

/// **Every block kind is laid out, in a real window, with real bounds.** This is
/// the assertion a coverage number could not make: `block_element` returns an
/// `AnyElement`, and before the debug selectors existed there was no way to ask
/// the window what any of these produced. `debug_bounds` asks the *window*, so a
/// `Some` here means the element was painted rather than merely built — the same
/// distinction `spike_render.rs` records, and a list whose blocks rendered as
/// nothing would satisfy a check that only looked at construction.
#[gpui::test]
fn every_block_kind_is_laid_out_with_bounds(cx: &mut TestAppContext) {
    let cx = rendered(cx);

    for selector in [
        "block.0", // heading
        "block.1", // paragraph
        "block.2", // quote
        "block.3", // unordered list
        "block.4", // ordered list
        "block.5", // task list
        "block.6", // rule
        "block.7", // code block
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{} must have been laid out", selector));
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "{selector} laid out at zero width or height: {:?}. A block that occupies \
             no space is a block the reader cannot see.",
            bounds.size,
        );
    }
}

/// **The height claim ADR-006 rests on, measured rather than argued.**
///
/// A code block and a one-word paragraph must not occupy the same vertical
/// space, or row height is uniform in practice and the reason this project did
/// not use a uniform estimator evaporates. This is the single most
/// decision-relevant assertion in the file, and it is the reason the debug
/// selectors were worth adding: **there is no way to make it without them.**
///
/// The comparison is between two blocks of the same document, so it is a
/// within-window comparison with one font and one width. The code block also
/// carries padding (`p_2`) and the paragraph does not, so a strictly-greater
/// relation is guaranteed by construction rather than by hope.
#[gpui::test]
fn a_code_block_occupies_more_height_than_a_paragraph(cx: &mut TestAppContext) {
    let cx = rendered(cx);

    let paragraph = cx
        .debug_bounds("block.1")
        .expect("the paragraph must be laid out");
    let code = cx
        .debug_bounds("block.7")
        .expect("the code block must be laid out");

    assert!(
        code.size.height > paragraph.size.height,
        "a fenced code block ({:?}) must be taller than a single paragraph ({:?}). \
         Row height is therefore variable, which is the fact ADR-006 is built on: a \
         uniform estimator would misplace every row after a code block.",
        code.size.height,
        paragraph.size.height,
    );
}

/// **A list has one addressable row per item, and nested content is addressable
/// under it.**
///
/// Without this, a `Block::List` whose recursion silently produced nothing would
/// still satisfy a check that the list itself has bounds — the container would be
/// laid out and empty. The item selectors are the only thing that sees inside.
#[gpui::test]
fn a_list_lays_out_one_addressable_row_per_item(cx: &mut TestAppContext) {
    let cx = rendered(cx);

    for (list, items) in [("block.3", 2_usize), ("block.4", 2), ("block.5", 2)] {
        for index in 0..items {
            let item_selector = selector(&[list, "item"], index);
            assert!(
                cx.debug_bounds(item_selector).is_some(),
                "{} must be laid out: a list whose items are invisible is a list that \
                 has lost its content",
                item_selector,
            );
        }
    }

    // And the content of a list item, one level deeper still.
    assert!(
        cx.debug_bounds(selector(&["block.3.item.0.block"], 0))
            .is_some(),
        "a paragraph inside a list item must be laid out under the item's own prefix, \
         or the nested content is in the tree but has no address"
    );
}

/// **A rule is a hairline, not a block of text and not a banner.**
///
/// **The bound is a fraction of a paragraph, and a fraction is what makes this
/// test able to fail.** A first version asserted only `rule < paragraph`, and
/// mutation M6 — replacing the rule's 1px with 24px — left it green, because a
/// 24px line is still shorter than a paragraph of body text. The assertion that
/// would have caught it is a *ratio*: a rule is a separator, so it must be a small
/// fraction of a line of text no matter how tall that line is. That is a claim
/// about what a rule *is*, and it fails when the rule is not one.
#[gpui::test]
fn a_horizontal_rule_is_a_hairline_rather_than_a_block_of_text(cx: &mut TestAppContext) {
    let cx = rendered(cx);

    let rule = cx
        .debug_bounds("block.6")
        .expect("the rule must be laid out");
    let paragraph = cx
        .debug_bounds("block.1")
        .expect("the paragraph must be laid out");

    assert!(
        rule.size.height * 4 < paragraph.size.height,
        "a rule ({:?}) should be a hairline: less than a quarter of a paragraph \
         ({:?}). A separator that is a sizeable fraction of a line of text is not a \
         separator, and a comparison that only said 'shorter than a paragraph' would \
         not notice the difference.",
        rule.size.height,
        paragraph.size.height,
    );
}

/// **A quote nests its own blocks, and they are addressable under it.**
///
/// `Block::Quote` recurses through `blocks_prefixed`, and that recursion is the
/// one path in this module where a prefix bug would produce an *empty* quote that
/// still has bounds. The selector is `block.2.item.0` because the quote arms its
/// children with the same `.item` prefix a list uses — asserted rather than
/// assumed, since the two share a prefix on purpose.
#[gpui::test]
fn a_quote_lays_out_its_own_nested_blocks(cx: &mut TestAppContext) {
    let cx = rendered(cx);

    assert!(
        cx.debug_bounds("block.2.item.0").is_some(),
        "the paragraph inside the quote must be laid out under the quote's prefix. A \
         quote that renders its own container and drops its content is the failure \
         this recursion is most likely to produce."
    );
}

/// **A heading is *strictly* bigger than a paragraph, which is the whole of a
/// heading.**
///
/// **This assertion is strict, and the `>=` it replaced was worthless.** A first
/// version of this test compared with `>=` and therefore could not fail: the
/// heading and the paragraph wrappers carry the same padding, so the *only* thing
/// that makes the heading taller is the larger font in its own arm. Mutation M4
/// removed that `text_lg()` and the `>=` test stayed green, which is how a test
/// that asserts nothing gets mistaken for one that asserts the right thing. A
/// strict `>` is what makes the heading's own arm load-bearing, and it is
/// mutation-checked rather than trusted.
#[gpui::test]
fn a_heading_is_strictly_taller_than_a_paragraph(cx: &mut TestAppContext) {
    let cx = rendered(cx);

    let heading = cx
        .debug_bounds("block.0")
        .expect("the heading must be laid out");
    let paragraph = cx
        .debug_bounds("block.1")
        .expect("the paragraph must be laid out");

    assert!(
        heading.size.height > paragraph.size.height,
        "a heading ({:?}) must be strictly taller than a paragraph ({:?}). Both \
         wrappers carry the same padding, so the only thing that can make the \
         heading taller is the `text_lg()` in its own arm -- which is exactly what \
         removing that arm takes away.",
        heading.size.height,
        paragraph.size.height,
    );
}

// ---------------------------------------------------------------------------
// `document`, and the one branch that is not a block.
// ---------------------------------------------------------------------------

/// **A truncated message reports it, in the real `document()`.**
///
/// `core/markdown.rs` states the contract: *"`true` means text is missing. The
/// caller should say so; a message silently shortened is the client lying about
/// what was sent."* This is `ui::markdown::document` complying, called rather than
/// mirrored: an earlier version of this test rebuilt the notice inline, which
/// would have passed even if `document()` had stopped rendering it. A test that
/// reimplements the unit under test is a test of the reimplementation.
#[gpui::test]
fn a_truncated_message_renders_a_notice_rather_than_losing_its_tail(cx: &mut TestAppContext) {
    let over = "x".repeat(core_markdown::MAX_MESSAGE_BYTES + 64);
    let document = core_markdown::parse(&over);
    assert!(
        document.was_truncated(),
        "the fixture must actually exceed the limit, or this test asserts nothing"
    );

    let (_view, cx) = cx.add_window_view(|_, _| TruncationHarness { document });
    cx.run_until_parked();

    let notice = cx
        .debug_bounds("truncation-notice")
        .expect("document() must render a notice for a truncated message");
    assert!(
        notice.size.height > px(0.),
        "the notice must occupy space: a notice laid out at zero height is not shown"
    );
}

/// **An untruncated message must *not* render the notice.**
///
/// The pair is what makes the first test mean something. A `document()` that
/// rendered the notice unconditionally would satisfy
/// `a_truncated_message_renders_a_notice` on its own, and the user would see
/// "this message was cut" on every message in the channel.
#[gpui::test]
fn an_ordinary_message_renders_no_truncation_notice(cx: &mut TestAppContext) {
    let document = core_markdown::parse("nothing was cut here");
    assert!(
        !document.was_truncated(),
        "the fixture must not be truncated, or this test asserts nothing"
    );

    let (_view, cx) = cx.add_window_view(|_, _| TruncationHarness { document });
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("truncation-notice").is_none(),
        "an ordinary message must not claim it was truncated"
    );
    assert!(
        cx.debug_bounds("block.0").is_some(),
        "and it must still render its own blocks"
    );
}

/// Carries an already-parsed document, because the fixture is the interesting part.
struct TruncationHarness {
    document: Document,
}

impl Render for TruncationHarness {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        ui_markdown::document(&self.document, Colors::default(), window)
    }
}

/// **The empty-span guard in `push_run` is dead code, and this is the test that
/// says so rather than leaving it uncovered.**
///
/// `push_run` returns early on an empty span, and coverage reports that line as
/// unexecuted. There are two honest ways to read that: a branch nobody can reach,
/// or a branch no fixture has found. **This test is what distinguishes them** — it
/// parses inputs chosen to produce empty spans and asserts the parser emits none,
/// which makes the guard unreachable by construction rather than untested by
/// accident. That is the same treatment 1C-2b gave its saturation arms and
/// 1E-1 gave the `TooLarge` `Display`.
///
/// If this test ever fails, the guard becomes reachable and *should* be covered
/// rather than removed: an empty run reaching `TextRun` would shift every
/// subsequent run's byte offset, because `StyledText::with_runs` slices the
/// accumulated string by each run's length.
#[rstest]
#[case("")]
#[case("**")]
#[case("` `")]
#[case("****")]
#[case("> ")]
#[case("- \n- ")]
#[case("[a](b)")]
#[case("```\n```")]
fn no_input_produces_an_empty_text_span(#[case] source: &str) {
    let document = core_markdown::parse(source);
    let mut empty = 0_usize;
    let mut total = 0_usize;
    // Walked structurally, the same way the renderer walks it, rather than
    // through a visitor: the assertion is about the tree `parse` returned.
    fn count(inlines: &[core_markdown::Inline], empty: &mut usize, total: &mut usize) {
        for inline in inlines {
            match inline {
                core_markdown::Inline::Text(text) => {
                    *total += 1;
                    if text.text().is_empty() {
                        *empty += 1;
                    }
                }
                core_markdown::Inline::Link(link) => count(link.children(), empty, total),
            }
        }
    }
    fn walk(blocks: &[Block], empty: &mut usize, total: &mut usize) {
        for block in blocks {
            match block {
                Block::Paragraph { spans } | Block::Heading { spans, .. } => {
                    count(spans, empty, total)
                }
                Block::Quote { blocks } => walk(blocks, empty, total),
                Block::List { items, .. } => {
                    for item in items {
                        walk(item.blocks(), empty, total);
                    }
                }
                Block::CodeBlock { .. } | Block::Rule => {}
            }
        }
    }
    walk(document.blocks(), &mut empty, &mut total);

    assert_eq!(
        empty, 0,
        "the parser produced an empty text span for {source:?}, so push_run's empty \
         guard is reachable and must be covered rather than called unreachable"
    );
}

// ---------------------------------------------------------------------------
// The parser is the thing being trusted, so it is pinned once.
// ---------------------------------------------------------------------------

/// **`SOURCE` parses to the block sequence the selectors assume.**
///
/// Every test above names `block.{index}` by hand, and every one of them would
/// pass *vacuously* if the parser ever emitted blocks in a different order or a
/// different count — a missing `debug_bounds` on a name that never existed looks
/// exactly like a missing block. This test is what makes the naming a fact rather
/// than a habit.
#[test]
fn the_fixture_produces_one_block_per_selector_the_other_tests_name() {
    let document = core_markdown::parse(SOURCE);
    let kinds: Vec<&'static str> = document
        .blocks()
        .iter()
        .map(|block| match block {
            Block::Heading { .. } => "heading",
            Block::Paragraph { .. } => "paragraph",
            Block::Quote { .. } => "quote",
            Block::List { .. } => "list",
            Block::Rule => "rule",
            Block::CodeBlock { .. } => "code_block",
        })
        .collect();

    assert_eq!(
        kinds,
        [
            "heading",
            "paragraph",
            "quote",
            "list",
            "list",
            "list",
            "rule",
            "code_block"
        ],
        "the selectors in these tests are hand-written, so this test is what makes \
         them a fact: a parser that reorders or drops a block would otherwise make \
         every bounds assertion in this file pass for the wrong reason"
    );
}
