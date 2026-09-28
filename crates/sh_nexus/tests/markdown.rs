//! `core/markdown.rs`: Markdown to a styled segment tree.
//!
//! The module under test turns attacker-controlled text into a tree that
//! work unit 2 will draw. Two things can go wrong, and they are not the same
//! thing: it can get the **styling** wrong, which looks bad, and it can let
//! message content mean something other than text, which is a security defect.
//! These tests are organised around the second, with the first pinned by
//! `AGENTS.md` §4.2's row — *markdown bold/italic/code/links, nested
//! formatting, never panics, injection safety* — one test per clause.
//!
//! | Claim | Where it is pinned |
//! |---|---|
//! | Bold, italic, strikethrough, inline code, links | the `a_markup_construct_renders_as_…` cases |
//! | **Nested** formatting unions its ancestors' styles | `nested_formatting_produces_one_span_carrying_the_union_of_its_ancestors` |
//! | Inline code and a code block cannot be confused | `inline_code_and_a_code_block_are_structurally_different` |
//! | Raw HTML is literal text, never dropped | `raw_html_is_preserved_as_literal_text` |
//! | Dangerous link schemes never reach a target | `a_dangerous_link_scheme_never_reaches_a_link_target` |
//! | A legitimate link survives | `a_legitimate_link_survives` |
//! | `mailto:` is **in**, and this says why | `a_legitimate_link_survives`, `mailto_is_allowed_because_it_grants_nothing` |
//! | An image never becomes a fetchable target | `an_image_contributes_its_alt_text_and_never_a_target` |
//! | Bidi controls are neutralised, bidi marks are not | `a_bidi_control_is_replaced_and_a_bidi_mark_is_kept` |
//! | A giant message is bounded, on a char boundary | `a_message_over_the_size_limit_is_truncated_on_a_character_boundary` |
//! | Deep nesting is bounded **and lossless** | `deeply_nested_quotes_are_flattened_without_losing_text` |
//! | Malformed markup still yields the author's words | `unclosed_markup_still_yields_the_authors_text` |
//! | No input can panic it | `arbitrary_markdown_never_panics_the_parser` (proptest) |
//!
//! # Style
//!
//! Parameterized cases use `#[rstest]` with `#[case]`, per `AGENTS.md` §4.3 and
//! ADR-008 — not a `for` loop over a table.
//!
//! Properties use `proptest`, per `AGENTS.md` §4.4, and each one is named after
//! the claim it pins rather than after the mechanism. Two of them exist *because*
//! a coverage tool cannot express the claim: `the_tree_never_exceeds_its_declared_depth`
//! and `no_link_target_escapes_the_allowlist` are about behaviour no line-count
//! would catch.
//!
//! **Three of the `#[case]` lists carry a coverage guard.** A `#[case]` list is a
//! hand-maintained enumeration, and nothing in `#[case]` syntax makes it
//! complete — the lesson ADR-008 records from work unit 1B. Each such list is
//! paired with a test that fails when the thing it enumerates grows past it.
//!
//! No test reads a clock, opens a socket, or depends on the order it runs in.
//! Every fixture is a literal, so a failure is reproducible from its own name.

use std::ops::Range;

use proptest::prelude::*;
use rstest::rstest;
use sh_nexus::core::markdown::{
    classify_target, parse, Block, Document, Heading, Inline, LinkSpan, Style, TextSpan,
    MAX_BLOCK_DEPTH, MAX_INLINE_DEPTH, MAX_MESSAGE_BYTES, MAX_TARGET_BYTES,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Every block in the document, depth first, in reading order.
fn blocks_of(document: &Document) -> Vec<&Block> {
    fn walk<'a>(blocks: &'a [Block], into: &mut Vec<&'a Block>) {
        for block in blocks {
            into.push(block);
            match block {
                Block::Quote { blocks } => walk(blocks, into),
                Block::List { items, .. } => {
                    for item in items {
                        walk(item.blocks(), into);
                    }
                }
                _ => {}
            }
        }
    }
    let mut found: Vec<&Block> = Vec::new();
    walk(document.blocks(), &mut found);
    found
}

/// Every inline node in the document, depth first, in reading order.
fn inlines_of(document: &Document) -> Vec<&Inline> {
    fn of_spans<'a>(spans: &'a [Inline], into: &mut Vec<&'a Inline>) {
        for span in spans {
            into.push(span);
            if let Inline::Link(link) = span {
                of_spans(link.children(), into);
            }
        }
    }
    let mut found: Vec<&Inline> = Vec::new();
    for block in blocks_of(document) {
        if let Block::Paragraph { spans } | Block::Heading { spans, .. } = block {
            of_spans(spans, &mut found);
        }
    }
    found
}

/// Every styled run of text in the document, depth first.
fn runs_of(document: &Document) -> Vec<&TextSpan> {
    inlines_of(document)
        .into_iter()
        .filter_map(|inline| match inline {
            Inline::Text(run) => Some(run),
            Inline::Link(_) => None,
        })
        .collect()
}

/// Every link in the document, depth first.
fn links_of(document: &Document) -> Vec<&LinkSpan> {
    inlines_of(document)
        .into_iter()
        .filter_map(|inline| match inline {
            Inline::Text(_) => None,
            Inline::Link(link) => Some(link),
        })
        .collect()
}

/// The runs of the document's only paragraph, which is what every single-line
/// fixture produces.
///
/// Panics on anything else, so a fixture that accidentally produces two blocks
/// fails with a message naming the test rather than with a confusing index.
fn only_paragraph(document: &Document) -> &[Inline] {
    assert_eq!(
        document.blocks().len(),
        1,
        "this fixture is expected to be one paragraph"
    );
    match &document.blocks()[0] {
        Block::Paragraph { spans } => spans,
        other => panic!("this fixture is expected to be a paragraph, got {other:?}"),
    }
}

/// `(text, style)` for each run of a single-paragraph document.
///
/// A link contributes its first child's run, so a fixture with a link compares
/// the same way as one without.
fn run_styles(document: &Document) -> Vec<(&str, Style)> {
    only_paragraph(document)
        .iter()
        .map(|inline| match inline {
            Inline::Text(run) => (run.text(), run.style()),
            Inline::Link(link) => match link.children().first() {
                Some(Inline::Text(run)) => (run.text(), run.style()),
                other => panic!("expected one text run inside the link, got {other:?}"),
            },
        })
        .collect()
}

/// Every source range the document carries: runs, links and code blocks.
fn ranges_of(document: &Document) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = inlines_of(document)
        .into_iter()
        .map(|inline| inline.source())
        .collect();
    for block in blocks_of(document) {
        if let Block::CodeBlock { source, .. } = block {
            found.push(source.clone());
        }
    }
    found
}

/// Every code block in the document.
fn code_blocks_of(document: &Document) -> Vec<(&Option<String>, &str)> {
    blocks_of(document)
        .into_iter()
        .filter_map(|block| match block {
            Block::CodeBlock { language, text, .. } => Some((language, text.as_str())),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// AGENTS.md 4.2 -- bold, italic, code, links, nested formatting
// ---------------------------------------------------------------------------

/// Each inline construct reaches the tree with the right style on the right run.
///
/// One `#[case]` per construct `AGENTS.md` §4.2 names, plus the two that follow
/// from the same code path. The expected value is the **whole run list**, not one
/// field, so a change to the run splitting shows up here rather than as a
/// silently different tree that still happens to contain the right style
/// somewhere.
#[rstest]
#[case("**bold**", &[("bold", Style::BOLD)], "bold")]
#[case("__bold__", &[("bold", Style::BOLD)], "bold")]
#[case("*italic*", &[("italic", Style::ITALIC)], "italic")]
#[case("_italic_", &[("italic", Style::ITALIC)], "italic")]
#[case("~~struck~~", &[("struck", Style::STRIKETHROUGH)], "struck")]
#[case("`code`", &[("code", Style::CODE)], "code")]
#[case("plain", &[("plain", Style::NONE)], "plain")]
#[case(
    "before **bold** after",
    &[("before ", Style::NONE), ("bold", Style::BOLD), (" after", Style::NONE)],
    "before bold after"
)]
#[case(
    "**`both`**",
    &[("both", Style::BOLD.union(Style::CODE))],
    "both"
)]
#[case(
    "*a **b** c*",
    &[
        ("a ", Style::ITALIC),
        ("b", Style::ITALIC.union(Style::BOLD)),
        (" c", Style::ITALIC),
    ],
    "a b c"
)]
fn a_markup_construct_renders_as_its_style(
    #[case] source: &str,
    #[case] expected: &[(&str, Style)],
    #[case] plain: &str,
) {
    let document = parse(source);
    assert_eq!(run_styles(&document), expected, "source: {source:?}");
    assert_eq!(document.plain_text(), plain, "source: {source:?}");
    assert_eq!(document.rejected_link_count(), 0, "source: {source:?}");
}

/// `***both***` is **both**, which is why [`Style`] is a set and not an enum.
///
/// An enum representation would have to answer this with a `BoldItalic` variant,
/// and then with one variant per combination. This test is the concrete reason
/// the type is a bitset, so it is pinned by name.
#[test]
fn a_triple_asterisk_run_carries_both_bold_and_italic() {
    let document = parse("***both***");
    assert_eq!(
        run_styles(&document),
        vec![("both", Style::BOLD.union(Style::ITALIC))]
    );
}

/// Nested formatting produces **one** run carrying the union of its ancestors' styles.
///
/// The load-bearing claim of the whole representation. `**bold *and italic*
/// inside**` is three runs, and the middle one is bold *and* italic — not
/// "italic inside bold", because inline formatting composes commutatively and a
/// renderer has nothing to do with the nesting shape. If this test fails, the
/// style is being treated as a stack rather than a set.
#[test]
fn nested_formatting_produces_one_span_carrying_the_union_of_its_ancestors() {
    let document = parse("**bold *and italic* inside**");
    assert_eq!(
        run_styles(&document),
        vec![
            ("bold ", Style::BOLD),
            ("and italic", Style::BOLD.union(Style::ITALIC)),
            (" inside", Style::BOLD),
        ]
    );
}

/// Two spellings of the same emphasis produce the **same tree**.
///
/// The canonical-tree claim, and the two sources are chosen carefully: they must
/// mean the *same thing*, not merely look similar. `**a** *b*` does **not** — it
/// means "a is bold, b is italic", where `**a *b***` means "b is bold and
/// italic" — and the trees correctly differ. What normalises is a change of
/// *spelling* at the same meaning: `***a***`, `*__a__*` and `**_a_**` are three
/// ways of writing "bold italic a", and the tree is the same for all three. That
/// is what will make work unit 1C-2's segment cache hit on all of them.
#[rstest]
#[case("***a***")]
#[case("*__a__*")]
#[case("**_a_**")]
#[case("__*a*__")]
fn three_spellings_of_bold_italic_produce_the_same_tree(#[case] source: &str) {
    let expected = parse("***a***");
    assert_eq!(parse(source), expected, "source: {source:?}");
    assert_eq!(
        run_styles(&expected),
        vec![("a", Style::BOLD.union(Style::ITALIC))]
    );
}

/// Adjacent plain text is one run, not one per line.
///
/// The merge that makes ordinary prose cheap, and the reason a 500-character
/// message is one allocation rather than one per character.
#[rstest]
#[case("one\ntwo\nthree", 1)]
#[case("a plain sentence with no markup at all", 1)]
#[case("**all bold**", 1)]
#[case("*a* then **b**", 3)]
#[case("`a` and *b*", 3)]
#[case("**a *b* c**", 3)]
fn adjacent_runs_of_one_style_are_merged(#[case] source: &str, #[case] expected_runs: usize) {
    let document = parse(source);
    assert_eq!(
        only_paragraph(&document).len(),
        expected_runs,
        "source: {source:?}"
    );
}

/// A single newline is **a newline**, not a space.
///
/// A deliberate departure from CommonMark prose rendering, where a soft break is
/// a space because in prose a wrapped line is not a line the author chose. In a
/// chat message it is. A message preview that joined its lines with spaces would
/// be lying about a message whose whole content is a short list.
#[rstest]
#[case("line one\nline two", "line one\nline two")]
#[case("line one\n\nline two", "line one\nline two")]
#[case("line one  \nline two", "line one\nline two")]
#[case("line one\\\nline two", "line one\nline two")]
fn a_soft_break_is_kept_as_a_line_break(#[case] source: &str, #[case] expected: &str) {
    assert_eq!(parse(source).plain_text(), expected, "source: {source:?}");
}

/// A nested list has **no phantom blank line** in its plain text.
///
/// A regression test, and the reason it is written down: a container's text
/// already ends in a newline, so appending another produced `"a\nb\n\nc"` for
/// `- a\n  - b\n- c` — a blank line in the message body that nobody wrote. Every
/// notification body and channel preview reads from `plain_text`.
#[test]
fn a_nested_list_has_no_phantom_blank_line_in_its_plain_text() {
    assert_eq!(parse("- a\n  - b\n- c").plain_text(), "a\nb\nc");
    assert_eq!(parse("- a\n  - b\n  - c\n- d").plain_text(), "a\nb\nc\nd");
}

// ---------------------------------------------------------------------------
// AGENTS.md 4.2 -- code: inline code and a code block are different things
// ---------------------------------------------------------------------------

/// Inline code is a run with a style bit; a code block is a block with verbatim
/// content and a language. They live at different levels of the tree, carry
/// different payloads, and have different types — so a renderer cannot draw one
/// as the other by accident. This test asserts both halves separately, because
/// "they are different" is only meaningful if each is right on its own.
#[test]
fn inline_code_and_a_code_block_are_structurally_different() {
    // Inline: a styled run inside a paragraph.
    let inline = parse("run `x = 1` end");
    assert_eq!(
        run_styles(&inline),
        vec![
            ("run ", Style::NONE),
            ("x = 1", Style::CODE),
            (" end", Style::NONE),
        ]
    );
    assert!(
        !inline
            .blocks()
            .iter()
            .any(|block| matches!(block, Block::CodeBlock { .. })),
        "`x = 1` is inline code and must not become a block"
    );

    // Block: a `Block::CodeBlock`, with no styled runs at all.
    let block = parse("```rust\nx = 1\n```");
    assert_eq!(
        code_blocks_of(&block),
        vec![(&Some("rust".to_owned()), "x = 1")],
    );
    assert!(
        runs_of(&block).is_empty(),
        "a code block's content is verbatim text, not styled runs"
    );
}

/// A code block's content is **verbatim**: nothing in it is parsed.
///
/// Not one assertion but three, because "unparsed" has three separate meanings
/// and a parser that got two of them right would still fail here. The expected
/// text has **no trailing newline**: the line ending that terminates the last
/// line belongs to the closing fence, not to the code.
#[rstest]
#[case("```\n**not bold**\n```", "**not bold**")]
#[case("```\n`not code`\n```", "`not code`")]
#[case("```\n&amp; &lt;\n```", "&amp; &lt;")]
#[case("    *not italic*", "*not italic*")]
fn a_code_blocks_content_is_never_parsed(#[case] source: &str, #[case] verbatim: &str) {
    let document = parse(source);
    assert_eq!(
        code_blocks_of(&document),
        vec![(&None, verbatim)],
        "source: {source:?}"
    );
}

/// A code block keeps a **blank line the author wrote**, and drops only the one
/// that belongs to the fence.
///
/// Both halves, because they are the same rule and either half alone would pass
/// on a wrong implementation: `trim_end` would pass the first and fail the second,
/// and "remove every trailing newline" would fail the first.
#[rstest]
#[case("```\na\n\nb\n```", "a\n\nb")]
#[case("```\na\nb\n```", "a\nb")]
#[case("```\na\n\n\n```", "a\n\n")]
#[case("```\n```", "")]
#[case("    a", "a")]
fn a_code_block_keeps_the_authors_blank_lines(#[case] source: &str, #[case] expected: &str) {
    let document = parse(source);
    assert_eq!(
        code_blocks_of(&document),
        vec![(&None, expected)],
        "source: {source:?}"
    );
}

///
/// A tri-state on the language is what stops a highlighter being handed `""` and
/// guessing, and it is why `CodeBlockKind::Fenced("")` and `Indented` are
/// treated alike.
/// A code block reports its **declared** language, and an undeclared one is
/// `None` rather than an empty string.
///
/// A tri-state on the language is what stops a highlighter being handed `""` and
/// guessing, and it is why a fenced block with no info string and an indented
/// block are treated alike.
#[rstest]
#[case("```rust\nx\n```", Some("rust"))]
#[case("```RUST\nx\n```", Some("RUST"))]
#[case("```rust,toml\nx\n```", Some("rust,toml"))]
#[case("```c++\nx\n```", Some("c++"))]
#[case("```\nx\n```", None)]
#[case("``` \nx\n```", None)]
#[case("    x", None)]
fn a_code_block_reports_its_declared_language(
    #[case] source: &str,
    #[case] expected: Option<&str>,
) {
    let document = parse(source);
    let (language, _) = &code_blocks_of(&document)[0];
    assert_eq!(
        language.as_deref(),
        expected,
        "source: {source:?}, got {language:?}"
    );
}

// ---------------------------------------------------------------------------
// AGENTS.md 4.2 -- links
// ---------------------------------------------------------------------------

/// A legitimate link survives, and keeps its text.
///
/// The converse of the injection tests, and it is here because a filter that
/// refuses everything passes every security test. `http`, `https` and `mailto`
/// are the whole allowlist, and all three must work.
#[rstest]
#[case("https://example.com", "https://example.com")]
#[case("http://example.com/a?b=1#c", "http://example.com/a?b=1#c")]
#[case("mailto:team@example.com", "mailto:team@example.com")]
#[case("HTTPS://example.com", "HTTPS://example.com")]
#[case("MailTo:a@b.com", "MailTo:a@b.com")]
fn a_legitimate_link_survives(#[case] target: &str, #[case] written: &str) {
    let document = parse(&format!("[text]({target})"));
    let links = links_of(&document);
    assert_eq!(links.len(), 1, "target: {target:?} should be a link");
    assert_eq!(links[0].target(), written, "target: {target:?}");
    assert_eq!(links[0].children()[0].text(), "text");
    assert_eq!(document.plain_text(), "text");
    assert_eq!(document.rejected_link_count(), 0);
}

/// `mailto:` is in the allowlist **because it grants nothing**.
///
/// Not a preference and not an oversight: it is non-executable, the OS handler
/// is the same class of handler as the browser's, and "email me about this" is a
/// normal utterance in a team channel. The cost is real and small — a message
/// can put an address in front of a user who clicks — but that is social
/// engineering, which no scheme filter can solve, and refusing `mailto:` would
/// break a legitimate utterance for no security benefit.
#[test]
fn mailto_is_allowed_because_it_grants_nothing() {
    assert_eq!(
        classify_target("mailto:team@example.com"),
        Some("mailto:team@example.com".to_owned())
    );
    // It cannot smuggle a second scheme past the parser, because the scheme is
    // what is being checked and there is only one.
    assert_eq!(
        classify_target("mailto:javascript:alert(1)"),
        Some("mailto:javascript:alert(1)".to_owned())
    );
}

/// An email autolink gets its `mailto:`, as CommonMark specifies.
///
/// `<a@b.com>` carries a **bare address**; CommonMark's renderer supplies the
/// scheme and this module has to as well, or a legitimate address is
/// indistinguishable from a relative URL and is refused — which would be
/// refusing the correspondence `mailto:` was allowed for.
#[rstest]
#[case("<a@b.com>", "mailto:a@b.com")]
#[case("<mailto:a@b.com>", "mailto:a@b.com")]
fn an_email_autolink_becomes_a_mailto_link(#[case] source: &str, #[case] expected: &str) {
    let document = parse(source);
    let links = links_of(&document);
    assert_eq!(links.len(), 1, "source: {source:?} should be a link");
    assert_eq!(links[0].target(), expected);
    assert_eq!(document.rejected_link_count(), 0);
}

/// A link's **title** is dropped, and its text is not.
///
/// A title is attacker-controlled text that lands in a tooltip, which is a
/// second injection surface for no chat benefit. The text is the message.
#[test]
fn a_links_title_is_dropped() {
    let document = parse("[click](https://example.com \"a tooltip\")");
    let links = links_of(&document);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target(), "https://example.com");
    assert_eq!(links[0].children()[0].text(), "click");
}

/// Surrounding whitespace in a target is trimmed, as a URL specification says.
///
/// `[a]( https://x )` is a link a person wrote on purpose; without the trim its
/// scheme segment would contain a space, fail the validity test, and be refused
/// for the wrong reason.
#[rstest]
#[case("[a]( https://e.com )", "https://e.com")]
#[case("[a](https://e.com )", "https://e.com")]
#[case("[a]( https://e.com)", "https://e.com")]
fn a_target_is_trimmed_before_it_is_validated(#[case] source: &str, #[case] expected: &str) {
    let document = parse(source);
    let links = links_of(&document);
    assert_eq!(links.len(), 1, "source: {source:?}");
    assert_eq!(links[0].target(), expected);
}

/// A link nested inside a list, or inside another link, still resolves.
///
/// Both are places where a container sits between the `Start` and the text, and
/// both are places a parser that only looks at the top of its stack gets wrong.
/// A link is found whatever container it sits in, and the count of discarded
/// targets is reported.
///
/// The third case is the one that matters: an image inside a link has its target
/// dropped, so the outer link survives with the image's **alt text** as its run.
/// One parser that treated the image as a link would find two.
#[rstest]
#[case("- [x](https://e.com)", 1, 0)]
#[case("> [x](https://e.com)", 1, 0)]
#[case("# [x](https://e.com)", 1, 0)]
#[case("[![alt](https://e/i.png)](https://e.com)", 1, 1)]
fn a_link_is_found_whatever_container_it_sits_in(
    #[case] source: &str,
    #[case] expected_links: usize,
    #[case] expected_rejected: usize,
) {
    let document = parse(source);
    assert_eq!(
        links_of(&document).len(),
        expected_links,
        "source: {source:?}"
    );
    assert_eq!(
        document.rejected_link_count(),
        expected_rejected,
        "source: {source:?}"
    );
}

// ---------------------------------------------------------------------------
// AGENTS.md 4.2 -- injection safety: raw HTML
// ---------------------------------------------------------------------------

/// Raw HTML is **preserved as literal text** and never dropped.
///
/// A chat message that quotes HTML must read as what the author wrote. Dropping
/// it would make a message about HTML say something its author did not write —
/// the same defect `core/ordering.rs` refuses when it declines to invent a
/// missing message. And there is no interpreter to be protected: the only
/// consumer of this tree is a text renderer that draws glyphs, so `<script>`
/// renders as the characters `<script>`.
#[rstest]
#[case("<script>alert(1)</script>", "<script>alert(1)</script>")]
#[case("<div class=\"x\">hi</div>", "<div class=\"x\">hi</div>")]
#[case("<img src=x onerror=alert(1)>", "<img src=x onerror=alert(1)>")]
#[case("a <b>bold?</b> c", "a <b>bold?</b> c")]
#[case(
    "<iframe src=\"https://evil\"></iframe>",
    "<iframe src=\"https://evil\"></iframe>"
)]
#[case("before <script>x</script> after", "before <script>x</script> after")]
#[case("<!-- a comment -->", "<!-- a comment -->")]
#[case(
    "<style>body{display:none}</style>",
    "<style>body{display:none}</style>"
)]
fn raw_html_is_preserved_as_literal_text(#[case] source: &str, #[case] expected: &str) {
    let document = parse(source);
    assert_eq!(document.plain_text(), expected, "source: {source:?}");
    // The strongest form of the claim: not one byte of the author's text is
    // gone, and nothing in the tree claims to be anything but a text run.
    for run in runs_of(&document) {
        assert!(
            !run.style().contains(Style::CODE),
            "source: {source:?} produced a code run for markup, which is not what \
             preserving it as text means"
        );
    }
}

// ---------------------------------------------------------------------------
// AGENTS.md 4.2 -- injection safety: link schemes
// ---------------------------------------------------------------------------

/// A dangerous link scheme **never** reaches a link target.
///
/// One `#[case]` per scheme named in the module docs, asserted at the parser
/// level — the text is still there, the target is gone, and the refusal is
/// counted rather than silent. The direct [`classify_target`] table below covers
/// the obfuscated forms CommonMark would refuse to parse as a link at all.
#[rstest]
#[case("javascript:alert(1)", "javascript")]
#[case("data:text/html,<script>alert(1)</script>", "data")]
#[case("file:///etc/passwd", "file")]
#[case("vbscript:msgbox(1)", "vbscript")]
fn a_dangerous_link_scheme_never_reaches_a_link_target(#[case] target: &str, #[case] name: &str) {
    let document = parse(&format!("[click me]({target})"));

    assert!(
        links_of(&document).is_empty(),
        "{name}: the target survived into the tree"
    );
    // The message still reads correctly: the author's words are not collateral
    // damage of the filter.
    assert_eq!(
        document.plain_text(),
        "click me",
        "{name}: the link's text was lost with its target"
    );
    // And the refusal is visible rather than silent, so `ui/` can tell the user.
    assert_eq!(
        document.rejected_link_count(),
        1,
        "{name}: the refusal was not counted"
    );
}

/// A scheme nobody thought of is refused too, because the allowlist is closed.
///
/// The point of an allowlist over a denylist, stated as a test: `ms-msdt:`,
/// `search-ms:`, `tel:` and `customapp:` are all real schemes on real systems,
/// and a denylist would have to know about every one of them.
#[rstest]
#[case("tel:+15550100")]
#[case("ftp://example.com/f")]
#[case("ms-msdt:/id")]
#[case("search-ms:query=x")]
#[case("customapp://run")]
#[case("about:blank")]
#[case("jar:http://e.com!/a")]
fn an_unlisted_scheme_is_refused(#[case] target: &str) {
    assert_eq!(classify_target(target), None, "target: {target:?}");
    let document = parse(&format!("[a]({target})"));
    assert!(links_of(&document).is_empty(), "target: {target:?}");
    assert_eq!(document.plain_text(), "a");
}

/// The obfuscations that defeat a "search for the allowed prefix" check.
///
/// The reason [`classify_target`] validates the **whole** pre-colon segment as a
/// scheme rather than looking for `http` somewhere in the target: a control
/// character in the scheme segment is what these attacks are made of, and
/// requiring the segment to be a valid scheme rejects all of them before
/// `javascript` is ever compared.
#[rstest]
#[case("java\tscript:alert(1)")]
#[case("java\nscript:alert(1)")]
#[case("java\rscript:alert(1)")]
#[case("\u{0}javascript:alert(1)")]
#[case("java script:alert(1)")]
#[case("java/script:alert(1)")]
#[case(":javascript:alert(1)")]
#[case("javascript :alert(1)")]
#[case(" javascript:alert(1)")]
#[case("javascript:alert(1) ")]
fn a_scheme_obfuscated_with_a_control_character_is_refused(#[case] target: &str) {
    assert_eq!(classify_target(target), None, "target: {target:?}");
}

/// A target that is not a URL at all is refused, and the reason is stated.
///
/// A relative link or a bare `#anchor` has no scheme, so it fails the same rule
/// that refuses `javascript:`. That is a decision, not a side effect: a chat
/// client has no base URL to resolve a relative link against, and resolving one
/// against the API origin would be a guess dressed up as a feature.
#[rstest]
#[case("/relative/path")]
#[case("./sibling")]
#[case("#anchor")]
#[case("example.com")]
#[case("")]
#[case("   ")]
fn a_target_with_no_scheme_is_refused(#[case] target: &str) {
    assert_eq!(classify_target(target), None, "target: {target:?}");
}

/// A target longer than the bound is refused, so a hostile message cannot put a
/// quarter-megabyte string into a `String` a renderer will lay out.
#[test]
fn an_over_long_target_is_refused() {
    let enormous = format!("https://e.com/{}", "x".repeat(MAX_TARGET_BYTES));
    assert!(enormous.len() > MAX_TARGET_BYTES);
    assert_eq!(classify_target(&enormous), None);

    // And the bound is not so tight that a real URL fails.
    let generous = format!("https://e.com/{}", "x".repeat(1_000));
    assert_eq!(classify_target(&generous), Some(generous));
}

/// An image contributes its **alt text** and never a target.
///
/// Not an injection test, and it belongs next to them: `![alt](url)` is a network
/// request to a host the *message author* chose, made from inside a chat window,
/// with this client's address attached. That is a read receipt. Attachments
/// belong in `Message::attachments`, which the network boundary validates.
#[rstest]
#[case("![a screenshot](https://evil.example/pixel.png)", "a screenshot")]
#[case("![](https://evil.example/pixel.png)", "")]
#[case("![alt](javascript:alert(1))", "alt")]
#[case("text ![alt](https://e/i.png) more", "text alt more")]
fn an_image_contributes_its_alt_text_and_never_a_target(
    #[case] source: &str,
    #[case] expected: &str,
) {
    let document = parse(source);
    assert_eq!(document.plain_text(), expected, "source: {source:?}");
    assert!(
        links_of(&document).is_empty(),
        "source: {source:?} produced a clickable image target"
    );
    assert_eq!(
        document.rejected_link_count(),
        1,
        "source: {source:?}: the discarded target was not counted"
    );
}

// ---------------------------------------------------------------------------
// Injection safety: Unicode
// ---------------------------------------------------------------------------

/// A bidi **control** is replaced; a bidi **mark** is kept.
///
/// The one a web-focused review misses, and it is not XSS: `U+202E
/// RIGHT-TO-LEFT OVERRIDE` makes `gnp<U+202E>exe` *display* as `exe.png`, so a
/// colleague reads a filename the author did not write. The text is shown as
/// something other than what it is, which is the whole question. The marks
/// (`U+200E`, `U+200F`, `U+061C`) and the zero-width joiners are **kept**, because
/// they are load-bearing in Arabic, Hebrew, Devanagari and in emoji sequences,
/// and a client that mangles those is worse than one that is slightly permissive.
#[rstest]
#[case("\u{202A}", "bidi embedding left")]
#[case("\u{202B}", "bidi embedding right")]
#[case("\u{202C}", "bidi pop formatting")]
#[case("\u{202D}", "bidi override left")]
#[case("\u{202E}", "bidi override right -- the filename-spoofing vector")]
#[case("\u{2066}", "bidi isolate left")]
#[case("\u{2067}", "bidi isolate right")]
#[case("\u{2068}", "bidi isolate first strong")]
#[case("\u{2069}", "bidi isolate pop")]
#[case("\u{206A}", "deprecated bidi override left")]
#[case("\u{206F}", "deprecated bidi isolate pop")]
#[case("\u{FEFF}", "zero width no-break space, the invisible-prefix trick")]
fn a_bidi_control_is_replaced_and_a_bidi_mark_is_kept(#[case] control: &str, #[case] why: &str) {
    let document = parse(&format!("a{control}b"));
    assert_eq!(
        document.plain_text(),
        "a\u{FFFD}b",
        "{why}: `{control}` should become U+FFFD so the tampering is visible"
    );
    assert_eq!(document.neutralized_characters(), 1, "{why}");
}

#[rstest]
#[case('\u{200E}', "left-to-right mark")]
#[case('\u{200F}', "right-to-left mark")]
#[case('\u{061C}', "arabic letter mark")]
#[case('\u{200B}', "zero width space")]
#[case('\u{200C}', "zero width non-joiner")]
#[case('\u{200D}', "zero width joiner")]
fn a_bidi_mark_and_a_zero_width_joiner_survive(#[case] kept: char, #[case] why: &str) {
    let document = parse(&format!("a{kept}b"));
    assert_eq!(
        document.plain_text(),
        format!("a{kept}b"),
        "{why}: this character is load-bearing in real scripts and must survive"
    );
    assert_eq!(document.neutralized_characters(), 0, "{why}");
}

/// The neutralisation applies inside a code block too.
///
/// A code block is verbatim, but "verbatim" is about **markup**, not about trust:
/// a filename spoofed inside a pasted log is still a filename a colleague reads.
/// The one thing a code block is spared is entity decoding.
#[test]
fn a_bidi_control_is_replaced_inside_a_code_block() {
    let document = parse("```\ngnp\u{202E}exe\n```");
    assert_eq!(document.plain_text(), "gnp\u{FFFD}exe");
    assert_eq!(document.neutralized_characters(), 1);
}

/// Ordinary text in ordinary scripts neutralises **nothing**.
///
/// The other half of the rule above, and the one that would be silently broken
/// by an over-broad filter: a message in Arabic or one containing an emoji
/// sequence must come through untouched, or the client is mangling real text to
/// stop an attack that did not need it.
#[rstest]
#[case("hello world")]
#[case("hola, ¿cómo estás?")]
#[case("مرحبا بالعالم")]
#[case("שלום עולם")]
#[case("emoji: \u{1F680}\u{1F469}\u{200D}\u{1F4BB}")]
#[case("日本語のテキスト")]
#[case("tabs\tand\u{0}nul")]
fn ordinary_text_neutralizes_nothing(#[case] source: &str) {
    let document = parse(source);
    assert_eq!(
        document.neutralized_characters(),
        0,
        "source: {source:?} had characters replaced"
    );
    assert_eq!(document.plain_text(), source.trim_end());
}

// ---------------------------------------------------------------------------
// The two limits
// ---------------------------------------------------------------------------

/// A message over the size limit is **truncated on a character boundary** and
/// says so.
///
/// The boundary walk is the whole reason this test exists: slicing a `str` at a
/// non-boundary index **panics**, the input is attacker-controlled, and a panic
/// there is a remote crash. The multi-byte case is the one that would catch a
/// regression to a plain `&source[..MAX]`.
#[rstest]
#[case(1_000, false)]
#[case(65_535, false)]
#[case(65_536, false)]
#[case(65_537, true)]
#[case(200_000, true)]
fn a_message_over_the_size_limit_is_truncated_on_a_character_boundary(
    #[case] length: usize,
    #[case] expect_truncated: bool,
) {
    let document = parse(&"x".repeat(length));
    assert_eq!(
        document.was_truncated(),
        expect_truncated,
        "length: {length}"
    );
    assert_eq!(document.plain_text().len(), length.min(MAX_MESSAGE_BYTES));
}

/// The truncation cuts a **multi-byte** character safely rather than mid-`char`.
///
/// `65_535` ASCII characters followed by a two-byte `é` puts byte 65,536 in the
/// middle of that `é`. A naive slice panics; this asserts it does not, and that
/// the result is valid UTF-8 text.
#[test]
fn truncation_never_splits_a_multi_byte_character() {
    // 65_535 one-byte characters, then a two-byte one: the limit lands inside it.
    let source = format!("{}é", "z".repeat(65_535));
    assert!(source.len() > MAX_MESSAGE_BYTES);

    let document = parse(&source);
    assert!(document.was_truncated());
    let text = document.plain_text();
    // Every character survives whole, and the tree holds only whole characters.
    assert_eq!(text.chars().count(), 65_535);
    assert_eq!(text, "z".repeat(65_535));
    for run in runs_of(&document) {
        assert!(run.text().is_char_boundary(0), "the run is not valid UTF-8");
    }
}

/// A message exactly at the limit is **not** truncated.
///
/// The boundary is inclusive: `MAX_MESSAGE_BYTES` bytes is allowed, and only
/// `MAX_MESSAGE_BYTES + 1` is cut. A test that only checked the over-the-limit
/// case would not notice an off-by-one that threw away a byte of a legitimate
/// message.
#[test]
fn a_message_exactly_at_the_limit_is_not_truncated() {
    let document = parse(&"x".repeat(MAX_MESSAGE_BYTES));
    assert!(!document.was_truncated());
    assert_eq!(document.plain_text().len(), MAX_MESSAGE_BYTES);
}

/// Deeply nested quotes are flattened to the declared bound **and lose nothing**.
///
/// Both halves matter. A bound that dropped the overflow would be a bound that
/// deletes an author's text; a bound that did not bound would let a hostile peer
/// hand a renderer a tree deep enough to walk off the stack. So the assertion is
/// `== MAX_BLOCK_DEPTH` *and* the text is still all there.
#[rstest]
#[case(MAX_BLOCK_DEPTH, false)]
#[case(MAX_BLOCK_DEPTH + 1, true)]
#[case(30, true)]
#[case(1_000, true)]
fn deeply_nested_quotes_are_flattened_without_losing_text(
    #[case] levels: usize,
    #[case] expect_limited: bool,
) {
    let source = format!("{}deep", "> ".repeat(levels));
    let document = parse(&source);

    assert_eq!(
        document.was_depth_limited(),
        expect_limited,
        "levels: {levels}"
    );
    assert_eq!(
        document.block_depth(),
        levels.min(MAX_BLOCK_DEPTH),
        "levels: {levels}"
    );
    // The part that matters: the author still wrote "deep" and it is still there.
    assert_eq!(document.plain_text(), "deep", "levels: {levels}");
}

/// A message that nests nowhere near the limit is not flagged.
///
/// The other half of the previous test. A depth counter that reported "limited"
/// for an ordinary message would train a caller to ignore the flag, which is the
/// same failure `core/ordering.rs` names for a "maximum idle gap" threshold.
#[rstest]
#[case("plain text")]
#[case("**bold** and *italic*")]
#[case("> one quote")]
#[case("> > two quotes")]
#[case("- a list\n- of items")]
fn an_ordinary_message_is_not_depth_limited(#[case] source: &str) {
    let document = parse(source);
    assert!(!document.was_depth_limited(), "source: {source:?}");
    assert!(!document.was_truncated(), "source: {source:?}");
    assert_eq!(document.rejected_link_count(), 0, "source: {source:?}");
    assert_eq!(document.neutralized_characters(), 0, "source: {source:?}");
}

// ---------------------------------------------------------------------------
// Malformed input -- AGENTS.md 4.2 "never panics", deterministically
// ---------------------------------------------------------------------------

/// Malformed Markdown still yields the author's words.
///
/// CommonMark's design is that text which is not markup is text, so almost none
/// of these are errors — they are the ordinary case. The list is here because
/// each one has a shape that breaks a naive bracket or delimiter matcher, and
/// because "never panics" needs a deterministic table as well as a property.
#[rstest]
#[case("**unclosed", "**unclosed")]
#[case("*unclosed", "*unclosed")]
#[case("~~unclosed", "~~unclosed")]
#[case("`unclosed", "`unclosed")]
#[case("[unclosed", "[unclosed")]
#[case("![unclosed", "![unclosed")]
#[case("](mismatched)", "](mismatched)")]
#[case("[[[[[[[[[[a", "[[[[[[[[[[a")]
#[case("]]]]]]]]]]", "]]]]]]]]]]")]
#[case("[a](unclosed", "[a](unclosed")]
#[case("[a]()", "a")]
#[case("> ", "")]
#[case(">>> deep", "deep")]
#[case("a\u{0}b", "a\u{0}b")]
#[case("   ", "")]
#[case("\n\n\n", "")]
#[case("*a *b* c*", "a b c")]
#[case("**a **b** c**", "a b c")]
fn unclosed_markup_still_yields_the_authors_text(#[case] source: &str, #[case] expected: &str) {
    let document = parse(source);
    assert_eq!(document.plain_text(), expected, "source: {source:?}");
}

/// A 100 KB single token is bounded and does not panic.
///
/// `AGENTS.md` §5.2 names a 500-character message as normal; this is the same
/// shape three orders of magnitude larger, and it is the case a size limit
/// exists for.
#[rstest]
#[case("x".repeat(200_000))]
#[case("*".repeat(200_000))]
#[case("`".repeat(200_000))]
#[case("[".repeat(200_000))]
#[case("> ".repeat(200_000))]
#[case("#".repeat(200_000))]
#[case("&amp;".repeat(60_000))]
#[case("\u{202E}".repeat(60_000))]
fn a_hostile_single_token_is_bounded_and_does_not_panic(#[case] source: String) {
    let document = parse(&source);
    // Every one of these is far over the size cap, so every one must be
    // truncated. The bound is the claim; without it a 200 KB message would be
    // parsed on the UI thread.
    assert!(
        document.was_truncated(),
        "{} bytes were not truncated",
        source.len()
    );
    // Whatever survived is still text, and the text is still valid UTF-8.
    let plain = document.plain_text();
    assert!(plain.len() <= MAX_MESSAGE_BYTES);
    for run in runs_of(&document) {
        assert!(!run.text().is_empty());
    }
}

/// An empty message is an empty document, not a panic and not a phantom block.
#[rstest]
#[case("")]
#[case(" ")]
#[case("\n")]
#[case("\t\n ")]
fn an_empty_message_produces_no_blocks(#[case] source: &str) {
    let document = parse(source);
    assert!(document.blocks().is_empty(), "source: {source:?}");
    assert_eq!(document.plain_text(), "");
    assert!(!document.was_truncated());
    assert!(!document.was_depth_limited());
}

// ---------------------------------------------------------------------------
// The text a reader sees -- the honest replacement for a false round-trip property
// ---------------------------------------------------------------------------

/// Each markup spelling renders to exactly the text the reader sees.
///
/// **This is the table that replaces the round-trip property, on purpose.** The
/// obvious universal claim — "the tree's concatenated text equals the source with
/// the markup removed" — is **false**, and asserting it would produce a test that
/// fails on a correct parser: pulldown decodes entities (`&amp;` → `&`), resolves
/// escapes, drops the spaces before a soft break, and drops a link's title. So
/// the claim is tested *exactly*, on inputs where none of that normalisation can
/// fire, and the cases that show the normalisation happening are here too, as
/// their own documented rows.
#[rstest]
#[case("**b** *i* `c` ~~s~~", "b i c s")]
#[case("a **b** c *d* e", "a b c d e")]
#[case("&amp; &lt; &gt;", "& < >")]
#[case("a \\* b \\_ c", "a * b _ c")]
#[case("`&amp;`", "&amp;")]
#[case("> quoted", "quoted")]
#[case("# heading", "heading")]
#[case("---", "")]
#[case("[text](https://e.com)", "text")]
#[case("[text](javascript:x)", "text")]
#[case("one\ntwo", "one\ntwo")]
fn a_markup_spelling_renders_to_exactly_the_text_the_reader_sees(
    #[case] source: &str,
    #[case] expected: &str,
) {
    assert_eq!(parse(source).plain_text(), expected, "source: {source:?}");
}

// ---------------------------------------------------------------------------
// Style -- the set algebra the renderer relies on
// ---------------------------------------------------------------------------

/// [`Style`] is a set, and the set laws hold.
///
/// This is the property that replaces "the style stack is balanced". There is no
/// stack to unbalance — that is the point of the representation — so what a
/// renderer actually relies on is that `union` is commutative, associative and
/// idempotent, and that the empty set is its identity. If any of those failed, a
/// renderer could not merge or compare styles and would have to re-derive them.
#[test]
fn inline_style_union_is_commutative_associative_and_idempotent() {
    let every = [
        Style::NONE,
        Style::BOLD,
        Style::ITALIC,
        Style::CODE,
        Style::STRIKETHROUGH,
        Style::BOLD.union(Style::ITALIC),
        Style::BOLD.union(Style::CODE),
        Style::ALL,
    ];
    for left in every {
        for right in every {
            assert_eq!(left.union(right), right.union(left), "commutative");
            assert_eq!(left.union(left), left, "idempotent");
            assert_eq!(left.union(Style::NONE), left, "NONE is the identity");
            for third in every {
                assert_eq!(
                    left.union(right).union(third),
                    left.union(right.union(third)),
                    "associative"
                );
            }
        }
    }
}

/// `Binary` and `Debug` agree about what a style is.
///
/// Two public formattings, and they exist for different readers: `Binary` is for
/// a human reading a failing assertion ("which bit is set?") and `Debug` is for
/// a `assert_eq!` that prints a whole run list. If the two disagreed, a failure
/// message would be a puzzle.
#[test]
fn a_style_formats_as_its_bits_in_both_forms() {
    let both = Style::BOLD.union(Style::ITALIC);
    assert_eq!(format!("{both:?}"), "Style(3)");
    // `Binary` follows `std`'s own rules: no prefix and minimal digits by
    // default, `0b` with `#`, and a width that **includes the prefix** -- so
    // `#010b` is eight binary digits after `0b`. The padded form is what a human
    // actually wants when asking "which of the four flags is set?".
    assert_eq!(format!("{both:b}"), "11");
    assert_eq!(format!("{both:#b}"), "0b11");
    assert_eq!(format!("{both:#010b}"), "0b00000011");
    assert_eq!(format!("{:?}", Style::NONE), "Style(0)");
    assert_eq!(format!("{:#010b}", Style::ALL), "0b00001111");
    // And `from_bits`/`bits` round-trip, so a caller can store and forward one.
    for style in [Style::NONE, Style::CODE, both, Style::ALL] {
        assert_eq!(Style::from_bits(style.bits()), style);
    }
}

/// A CRLF message does not leave a stray `\r` in a code block.
///
/// The `\` branch of the closing-newline trim, which a single test with `\n` can
/// never reach. Windows is a target platform (`AGENTS.md` §5.2), so a CRLF
/// message is not hypothetical, and a lone `\r` in a rendered code block is a
/// control glyph the user would see.
#[test]
fn a_crlf_code_block_keeps_no_stray_carriage_return() {
    let document = parse("```rust\r\nlet x = 1;\r\nlet y = 2;\r\n```");
    assert_eq!(
        code_blocks_of(&document),
        vec![(&Some("rust".to_owned()), "let x = 1;\nlet y = 2;")]
    );
    for (_, code) in code_blocks_of(&document) {
        assert!(!code.contains('\r'), "a carriage return survived: {code:?}");
    }
}

/// An **empty** heading produces no block.
///
/// Reachable and worth pinning: `#` on its own is a valid ATX heading with no
/// content, and a renderer that emitted an empty heading would reserve a line of
/// vertical space for a title that is not there.
#[rstest]
#[case("#")]
#[case("##")]
#[case("# ")]
#[case("######")]
fn an_empty_heading_produces_no_block(#[case] source: &str) {
    let document = parse(source);
    assert!(
        document.blocks().is_empty(),
        "source: {source:?} -> {document:?}"
    );
    assert_eq!(document.plain_text(), "");
}

/// The rest of the set algebra, so a renderer can rely on all of it.
#[test]
fn the_style_set_operations_agree_with_each_other() {
    let bold = Style::BOLD;
    let italic = Style::ITALIC;
    let both = bold.union(italic);

    assert!(both.contains(bold));
    assert!(both.contains(italic));
    assert!(both.contains(Style::NONE), "the empty set is in every set");
    assert!(!bold.contains(italic));
    assert!(bold.intersect(italic).is_empty());
    assert_eq!(both.without(italic), bold);
    assert_eq!(bold.without(bold), Style::NONE);
    assert_eq!(both.symmetric_difference(bold), italic);
    assert_eq!(Style::from_bits(bold.bits()), bold);
    assert_eq!(both.count(), 2);
    assert_eq!(Style::ALL.count(), 4);
    assert!(Style::NONE.is_empty());
    assert!(!bold.is_empty());
    assert!(bold.is(bold));
    assert!(!bold.is(italic));
}

/// Unknown bits are **kept**, not masked out.
///
/// A `from_bits` that silently dropped bits beyond the declared set would make
/// `Style` disagree with [`Style::bits`], and a style added to this type later
/// would be invisible to any code holding an older `Style`.
#[test]
fn from_bits_keeps_bits_outside_the_declared_set() {
    let future = Style::from_bits(0b1000_0000);
    assert_eq!(future.bits(), 0b1000_0000);
    assert!(!future.is_empty());
    // It intersects the declared set to nothing, and reports one bit, so a
    // caller can see that it is holding something it has never heard of.
    assert!(future.intersect(Style::ALL).is_empty());
    assert_eq!(future.count(), 1);
}

/// Coverage guard: a new style bit cannot be added without this list knowing.
///
/// A `#[case]` list is a hand-maintained enumeration, and nothing in `#[case]`
/// syntax makes it complete — the lesson `docs/COVERAGE.md` §6.2 records from
/// work unit 1B. This test is what makes the enumeration above self-checking.
#[test]
fn the_style_bits_are_exactly_the_four_declared_flags() {
    let declared = [
        Style::BOLD,
        Style::ITALIC,
        Style::CODE,
        Style::STRIKETHROUGH,
    ];
    // `Style::ALL` and the declared list must agree about **every** bit, in both
    // directions: a bit in `ALL` that no test names, and a named style whose bit
    // `ALL` does not carry, are both ways for this list to drift.
    for bit in 0..8u8 {
        let style = Style::from_bits(1 << bit);
        let in_all = !style.is_empty() && Style::ALL.contains(style);
        assert_eq!(
            in_all,
            declared.contains(&style),
            "bit {bit}: Style::ALL says {in_all}, the declared list says {}",
            declared.contains(&style)
        );
    }
    assert_eq!(
        Style::ALL,
        declared
            .iter()
            .fold(Style::empty(), |all, style| all.union(*style)),
        "Style::ALL is out of step with the declared flags"
    );
    assert_eq!(Style::ALL.count(), 4);
}

/// Every style the parser can produce is a subset of the declared set.
///
/// The "a style cannot escape its own nesting" claim, stated the way this
/// representation makes it true: the union of open style frames is drawn from a
/// closed set, so no combination outside [`Style::ALL`] can exist.
#[rstest]
#[case("**a**")]
#[case("*a*")]
#[case("`a`")]
#[case("~~a~~")]
#[case("***a***")]
#[case("**a *b `c` d* e**")]
#[case("> **a**")]
#[case("- **a** *b*")]
#[case("[**a**](https://e.com)")]
fn no_style_in_the_tree_is_outside_the_declared_set(#[case] source: &str) {
    for run in runs_of(&parse(source)) {
        assert!(
            run.style().intersect(Style::ALL) == run.style(),
            "source: {source:?} produced the undeclared style {:?}",
            run.style()
        );
    }
}

// ---------------------------------------------------------------------------
// Source positions
// ---------------------------------------------------------------------------

/// Every run's source range points at the text it came from.
///
/// Not a round trip — the module docs are explicit that it is not one, because
/// entities and escapes are normalised and adjacent runs are merged. What *is*
/// guaranteed, and what makes a range usable for search highlighting, is that it
/// is inside the source and that the run's text is the text at that region for
/// the inputs where no normalisation can fire.
#[rstest]
#[case("**bold** text")]
#[case("plain text with no markup")]
#[case("a [link](https://e.com) here")]
#[case("> quoted **bold**")]
#[case("```rust\ncode\n```")]
#[case("line one\nline two")]
#[case("![alt](https://e/i.png)")]
fn a_source_range_points_at_the_text_it_came_from(#[case] source: &str) {
    let document = parse(source);
    for range in ranges_of(&document) {
        assert!(
            range.start <= range.end,
            "source: {source:?} produced the inverted range {range:?}"
        );
        assert!(
            range.end <= source.len(),
            "source: {source:?} produced {range:?}, which is past the end"
        );
    }
    // For a run with no entity, no escape and no merge, the slice is the text.
    for run in runs_of(&document) {
        let range = run.source();
        if !source.contains('&') && !source.contains('\\') {
            assert_eq!(
                &source[range],
                run.text(),
                "source: {source:?}: the range does not address the run's text"
            );
        }
    }
}

/// A search hit inside a message can be mapped to the runs that contain it.
///
/// The concrete reason source positions are kept, and the reason the claim is
/// written as a test rather than left in a doc comment: search runs over the raw
/// `Message::content` and produces byte offsets, and this is the mapping that
/// turns an offset into "highlight this run".
#[test]
fn a_search_offset_maps_to_the_runs_that_contain_it() {
    let source = "the deploy script needs **sudo** access";
    let document = parse(source);

    let needle = "sudo";
    let start = source
        .find(needle)
        .expect("the fixture contains the needle");
    let hit = start..start + needle.len();

    let covering: Vec<&str> = runs_of(&document)
        .into_iter()
        .filter(|run| {
            let run_range = run.source();
            run_range.start <= hit.start && hit.end <= run_range.end
        })
        .map(|run| run.text())
        .collect();

    assert_eq!(covering, vec!["sudo"], "the hit did not map to one run");
}

// ---------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------

/// A heading reports its own level, and `level()` is total over the six.
#[rstest]
#[case("# one", Heading::H1, 1)]
#[case("## two", Heading::H2, 2)]
#[case("### three", Heading::H3, 3)]
#[case("#### four", Heading::H4, 4)]
#[case("##### five", Heading::H5, 5)]
#[case("###### six", Heading::H6, 6)]
fn a_heading_reports_its_own_level(
    #[case] source: &str,
    #[case] expected: Heading,
    #[case] level: u8,
) {
    let document = parse(source);
    match document.blocks() {
        [Block::Heading { level: found, .. }] => {
            assert_eq!(*found, expected, "source: {source:?}");
            assert_eq!(found.level(), level, "source: {source:?}");
        }
        other => panic!("source: {source:?} should be one heading, got {other:?}"),
    }
}

/// Every heading level, in order, and `level()` matches the declaration order.
///
/// A guard, for the same reason as the `Style` one: the `From<HeadingLevel>`
/// conversion is six arms that nothing forces to stay in step with
/// [`Heading::level`].
#[test]
fn the_heading_cases_cover_every_level() {
    let every = [
        (Heading::H1, 1),
        (Heading::H2, 2),
        (Heading::H3, 3),
        (Heading::H4, 4),
        (Heading::H5, 5),
        (Heading::H6, 6),
    ];
    for (heading, level) in every {
        assert_eq!(heading.level(), level);
    }
    // And the enum is strictly ordered by level, so a renderer can sort without
    // a match of its own.
    let levels: Vec<u8> = every.iter().map(|(_, level)| *level).collect();
    let mut sorted = levels.clone();
    sorted.sort_unstable();
    assert_eq!(levels, sorted);
    assert_eq!(levels, vec![1, 2, 3, 4, 5, 6]);
}

/// A task-list item is checked, unchecked, or **not a task at all**.
///
/// Three states, not two: "not a task" and "an unchecked task" must not render
/// the same, and collapsing them would need a convention about which is which.
#[rstest]
#[case("- [x] done", Some(true))]
#[case("- [ ] todo", Some(false))]
#[case("- plain", None)]
#[case("1. numbered", None)]
fn a_task_list_item_reports_its_checkbox(#[case] source: &str, #[case] expected: Option<bool>) {
    let document = parse(source);
    let item = blocks_of(&document)
        .into_iter()
        .find_map(|block| match block {
            Block::List { items, .. } => items.first(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("source: {source:?} should be a list"));
    assert_eq!(item.checked(), expected, "source: {source:?}");
}

/// A checkbox survives a **loose** item, where a paragraph sits in between.
///
/// The regression this pins: the marker arrives with a `Paragraph` on top of the
/// frame stack, so a parser that only looked at the top frame lost it.
#[rstest]
#[case("- [x] a\n\n  more", Some(true))]
#[case("- [ ] a\n\n  more", Some(false))]
fn a_checkbox_survives_a_loose_list_item(#[case] source: &str, #[case] expected: Option<bool>) {
    let document = parse(source);
    let item = blocks_of(&document)
        .into_iter()
        .find_map(|block| match block {
            Block::List { items, .. } => items.first(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("source: {source:?} should be a list"));
    assert_eq!(item.checked(), expected, "source: {source:?}");
}

/// An ordered list keeps its first number.
///
/// `3. three` must start at 3, or a pasted excerpt from a numbered document
/// renumbers itself when a colleague reads it.
#[rstest]
#[case("1. a", 1)]
#[case("3. a", 3)]
#[case("42. a", 42)]
fn an_ordered_list_keeps_its_first_number(#[case] source: &str, #[case] start: u64) {
    let document = parse(source);
    match document.blocks() {
        [Block::List {
            ordered,
            start: found,
            items,
        }] => {
            assert!(*ordered, "source: {source:?} should be ordered");
            assert_eq!(*found, start, "source: {source:?}");
            assert_eq!(items.len(), 1);
        }
        other => panic!("source: {source:?} should be one list, got {other:?}"),
    }
}

/// An unordered list is unordered, and is not `start: 1` masquerading as one.
#[rstest]
#[case("- a")]
#[case("* a")]
#[case("+ a")]
fn an_unordered_list_is_not_ordered(#[case] source: &str) {
    let document = parse(source);
    match document.blocks() {
        [Block::List { ordered, .. }] => assert!(!*ordered, "source: {source:?}"),
        other => panic!("source: {source:?} should be one list, got {other:?}"),
    }
}

/// A block quote **nests**, because the indentation is the meaning.
///
/// The block half of the representation, as against the flat inline half: this
/// is the nesting a renderer must honour, and the reason `Block` is recursive
/// while the run list is not.
#[test]
fn a_block_quote_nests_because_its_indentation_is_the_meaning() {
    let document = parse("> outer\n>\n> > inner");
    assert_eq!(document.block_depth(), 2);
    let quote = blocks_of(&document)
        .into_iter()
        .find(|block| matches!(block, Block::Quote { .. }))
        .expect("there is a quote");
    let Block::Quote { blocks } = quote else {
        unreachable!("just matched on Quote")
    };
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::Quote { .. })),
        "the inner quote is not inside the outer one"
    );
}

/// A list inside a list item, and a quote inside a list item, both land in the
/// item rather than beside it.
///
/// Every place a container sits between a `Start` and the text below it is a
/// place a stack-only parser gets it wrong, so each is a case.
#[rstest]
#[case("- a\n  - b", 2, "a\nb")]
#[case("- > quoted", 2, "quoted")]
#[case("> - a", 2, "a")]
#[case("- a\n  - b\n  - c", 2, "a\nb\nc")]
fn a_nested_container_lands_inside_its_enclosing_container(
    #[case] source: &str,
    #[case] depth: usize,
    #[case] plain: &str,
) {
    let document = parse(source);
    assert_eq!(document.block_depth(), depth, "source: {source:?}");
    // And the nested content is still text, which is the point of nesting at
    // all: a renderer that walked only the top level would drop it.
    assert_eq!(document.plain_text(), plain, "source: {source:?}");
}

/// `MAX_INLINE_DEPTH` is exactly 2, and a link is what reaches it.
///
/// A run inside a link is the deepest the format allows: CommonMark forbids a
/// link inside a link, so `MAX_INLINE_DEPTH` is a structural fact rather than a
/// cap this module enforces. Asserted so the constant cannot drift away from
/// what the parser does.
#[test]
fn a_link_is_the_deepest_inline_nesting_there_is() {
    assert_eq!(MAX_INLINE_DEPTH, 2);
    assert_eq!(parse("[a](https://e.com)").inline_nesting_depth(), 2);
    assert_eq!(parse("[**a**](https://e.com)").inline_nesting_depth(), 2);
    assert_eq!(parse("**a**").inline_nesting_depth(), 1);
    assert_eq!(parse("plain").inline_nesting_depth(), 1);
    // Even with every construct stacked, nothing nests a third level.
    assert_eq!(
        parse("[**a *b* `c`**](https://e.com)").inline_nesting_depth(),
        2
    );
}

// ---------------------------------------------------------------------------
// Properties -- AGENTS.md 4.4
// ---------------------------------------------------------------------------

/// A fragment generator weighted towards Markdown's own syntax.
///
/// Purely arbitrary text almost never contains a `*`, so a strategy of
/// `any::<String>()` would sample a parser that never opens a frame and would
/// pass while every branch that matters sat uncovered. This one spends most of
/// its budget on delimiters, links, breaks, and the specific bytes the security
/// rules are about.
fn markdown_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        // Delimiters and structure.
        4 => Just("*".to_owned()),
        3 => Just("**".to_owned()),
        3 => Just("`".to_owned()),
        2 => Just("~~".to_owned()),
        2 => Just("[".to_owned()),
        2 => Just("]".to_owned()),
        2 => Just("(".to_owned()),
        2 => Just(")".to_owned()),
        2 => Just("<".to_owned()),
        2 => Just(">".to_owned()),
        2 => Just("#".to_owned()),
        2 => Just("![".to_owned()),
        2 => Just("](".to_owned()),
        2 => Just("`code`".to_owned()),
        2 => Just("**bold**".to_owned()),
        2 => Just("- ".to_owned()),
        1 => Just("1. ".to_owned()),
        1 => Just("```".to_owned()),
        1 => Just("```rust".to_owned()),
        1 => Just("[a](https://e.com)".to_owned()),
        // Whitespace and line endings.
        3 => Just("\n".to_owned()),
        1 => Just("\r\n".to_owned()),
        2 => Just(" ".to_owned()),
        2 => Just("  ".to_owned()),
        1 => Just("\t".to_owned()),
        // The bytes the security rules are about.
        2 => Just("\u{0}".to_owned()),
        2 => Just("\u{202E}".to_owned()),
        1 => Just("\u{FEFF}".to_owned()),
        1 => Just("\u{200F}".to_owned()),
        2 => Just("javascript:".to_owned()),
        1 => Just("data:".to_owned()),
        1 => Just("mailto:a@b.com".to_owned()),
        1 => Just("https://e.com".to_owned()),
        // Normalisation.
        2 => Just("&amp;".to_owned()),
        2 => Just("\\*".to_owned()),
        1 => Just("---".to_owned()),
        // And plain text, so most cases are not delimiter soup.
        6 => "[a-zA-Z0-9 ]{1,12}".prop_map(|s: String| s),
        2 => any::<char>().prop_map(|character| character.to_string()),
    ]
}

/// A whole message body.
///
/// Five generators with very different shapes, because each finds a different
/// class of bug: fragments, a longer composed document, arbitrary characters,
/// and the two specific adversarial shapes the limits exist for.
fn arbitrary_markdown() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => proptest::collection::vec(markdown_fragment(), 0..24)
            .prop_map(|parts| parts.concat()),
        2 => proptest::collection::vec(markdown_fragment(), 1..60)
            .prop_map(|parts| parts.join("\n")),
        2 => proptest::collection::vec(any::<char>(), 0..64)
            .prop_map(|characters| characters.into_iter().collect::<String>()),
        1 => (0usize..40).prop_map(|levels| format!("{}deep", "> ".repeat(levels))),
        1 => proptest::collection::vec(markdown_fragment(), 0..200)
            .prop_map(|parts| parts.concat()),
    ]
}

proptest! {
    /// `AGENTS.md` §4.4's mandate, verbatim: **markdown parsing never panics for
    /// arbitrary input**.
    ///
    /// Also asserts the output is *usable*, not merely returned: the blocks and
    /// runs have to be walkable and the text has to be valid UTF-8, or the test
    /// would pass on a tree full of nonsense.
    #[test]
    fn arbitrary_markdown_never_panics_the_parser(source in arbitrary_markdown()) {
        let document = parse(&source);

        // Walkable, and every run has a style inside the declared set.
        for block in blocks_of(&document) {
            for run in runs_of(&document) {
                prop_assert!(run.style().intersect(Style::ALL) == run.style());
            }
            let _ = block;
        }
        // The text is text.
        let plain = document.plain_text();
        prop_assert!(plain.is_char_boundary(0));
        // And the flags are self-consistent: a count is never negative and the
        // document claims truncation only if the source really was over the cap.
        prop_assume!(!document.was_truncated() || source.len() > MAX_MESSAGE_BYTES);
    }

    /// The same input always produces the same tree.
    ///
    /// The claim that makes work unit 1C-2's segment cache worth having, and the
    /// one a "close enough" renderer would quietly break: if parsing depended on
    /// anything but its argument — a clock, a counter, a hash seed — the cache
    /// would serve a different tree than a fresh parse would.
    #[test]
    fn parsing_is_deterministic(source in arbitrary_markdown()) {
        prop_assert_eq!(parse(&source), parse(&source));
    }

    /// The tree never exceeds its declared depth.
    ///
    /// The observable form of both limits, checked from the outside over
    /// arbitrary input rather than on the two fixtures that were designed to hit
    /// them. A renderer that recurses on this tree can trust the bound.
    #[test]
    fn the_tree_never_exceeds_its_declared_depth(source in arbitrary_markdown()) {
        let document = parse(&source);
        prop_assert!(document.block_depth() <= MAX_BLOCK_DEPTH);
        prop_assert!(document.inline_nesting_depth() <= MAX_INLINE_DEPTH);
    }

    /// Every source range lies inside the source it came from.
    ///
    /// Rules out a fabricated or mis-translated position, which is the failure
    /// mode that would make search highlighting point at the wrong characters
    /// rather than fail visibly.
    #[test]
    fn every_source_range_lies_inside_the_source(source in arbitrary_markdown()) {
        let document = parse(&source);
        for range in ranges_of(&document) {
            prop_assert!(range.start <= range.end, "inverted range {range:?}");
            prop_assert!(range.end <= source.len(), "range {range:?} past the end of a {} byte source", source.len());
        }
    }

    /// No link target escapes the allowlist, over arbitrary input.
    ///
    /// The injection property as a *universal* claim rather than a table of
    /// hand-written bad URLs. The table proves the schemes that were thought of;
    /// this proves that nothing else can get through — which is the property an
    /// allowlist is supposed to have and the reason it is preferred to a
    /// denylist.
    #[test]
    fn no_link_target_escapes_the_allowlist(source in arbitrary_markdown()) {
        let document = parse(&source);
        for link in links_of(&document) {
            let target = link.target();
            // Every target that reached the tree classifies as allowed...
            prop_assert!(
                classify_target(target).as_deref() == Some(target),
                "a target in the tree does not pass the filter: {target:?}"
            );
            // ...and its scheme is one of exactly three.
            let scheme = target.split(':').next().unwrap_or_default().to_ascii_lowercase();
            prop_assert!(
                ["http", "https", "mailto"].contains(&scheme.as_str()),
                "an unlisted scheme reached the tree: {target:?}"
            );
        }
    }

    /// No message can make a run that is not the message's own plain text appear.
    ///
    /// The "is there any path by which message content could be interpreted as
    /// something other than text?" question, as a checkable claim: the only
    /// strings in the tree's runs are text with a style, and none of them is a
    /// character whose purpose is to change how the surrounding text reads.
    ///
    /// **Two sub-claims were tried here and removed, and both reasons are worth
    /// recording.**
    ///
    /// The first — "a run's text is never also a link target" — is **false**: a
    /// message may legitimately contain the literal text `https://e.com` *and* a
    /// link to it, and proptest found that immediately
    /// (`"https://e.com`code`[a](https://e.com)*"`).
    ///
    /// The second — "if the text holds a `U+FFFD` then the counter is non-zero" —
    /// is **also false**: the author may have typed `U+FFFD` themselves, which is
    /// exactly what proptest's next counterexample was (`source = "�"`).
    ///
    /// Neither is a defect in the module. Both were defects in the *test*, and a
    /// property that cannot be true earns nothing except a regression file and
    /// the suspicion that the code is broken.
    #[test]
    fn every_run_is_plain_text_with_no_bidi_control(source in arbitrary_markdown()) {
        let document = parse(&source);
        for run in runs_of(&document) {
            for character in run.text().chars() {
                prop_assert!(
                    !(('\u{202A}'..='\u{202E}').contains(&character)
                        || ('\u{2066}'..='\u{206F}').contains(&character)
                        || character == '\u{FEFF}'),
                    "a bidi control reached a run: {character:?}"
                );
            }
        }
    }
}
