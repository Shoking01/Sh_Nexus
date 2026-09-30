//! A parsed message's segment tree, as GPUI elements.
//!
//! `docs/ARCHITECTURE.md` ADR-006's step 4: *"`core/markdown.rs` → GPUI
//! elements. The segment tree becomes a row's content, with an explicit
//! `.text_color()` on every text element"*. This module is that, and it is the
//! first consumer of a `core/` module that had 1,956 lines and no caller.
//!
//! # The split this module exists to make
//!
//! `core/markdown.rs` owns *what the message says*: blocks, styled runs, link
//! targets, and every bound on how much of it survives. This module owns *what
//! that looks like*: which GPUI element each block becomes and which colour
//! each style bit resolves to. **No parsing happens here.** Running the parser
//! twice — once for content and once for appearance — is how a renderer and a
//! model drift until a message shows something the model never contained.
//!
//! # Two decisions worth stating
//!
//! **A paragraph is one `StyledText`, not one element per run.** A run boundary
//! is a style boundary, not a layout boundary: laying each run out separately
//! would forbid a line break in the middle of a bolded phrase and give every
//! emphasis its own line when it did not fit. `StyledText` carries a `TextRun`
//! per style, which is what makes wrapping and a mixed-style line compatible.
//!
//! **No font family is set, anywhere.** `AGENTS.md` §10.1 puts
//! `typography.family` in the theme document, and `PLAN.md` §9 makes font
//! selection the theme system's job. Guessing a monospace family here —
//! `"monospace"` resolves on Linux and not on Windows — would make a fenced
//! block render in an arbitrary face on one platform and the intended one on
//! another. Until a theme carries a monospace family, a code block is
//! distinguished by its background and its colour, which are facts this layer
//! does own.

use gpui::{
    div, prelude::*, px, AnyElement, Div, FontStyle, FontWeight, IntoElement, StrikethroughStyle,
    StyledText, TextRun, TextStyle, UnderlineStyle, Window,
};

use crate::core::markdown::{Block, Document, Inline, Style};
use crate::ui::Colors;

/// Renders a whole message: its blocks, plus what had to be done to the source
/// to produce them.
///
/// The truncation notice is **rendered rather than logged**, because
/// `core/markdown.rs` says what it means for the caller: *"`true` means text is
/// missing. The caller should say so; a message silently shortened is the client
/// lying about what was sent."* A chat client that drops the end of a long
/// message and says nothing is worse than one that drops it and admits it.
pub fn document(document: &Document, colors: Colors, window: &Window) -> AnyElement {
    let mut root = blocks(document.blocks(), colors, window);

    if document.was_truncated() {
        root = root.child(
            div()
                // The selector for the same reason every block has one: this is
                // the one branch of `document()` that is not a block, so it is
                // the one branch a block-indexed test would never reach.
                .debug_selector(|| "truncation-notice".to_owned())
                .text_sm()
                // Explicit, per AGENTS.md 7.3, on the element that carries text.
                .text_color(colors.danger)
                .child("This message was longer than the client will render, and was cut."),
        );
    }

    root.into_any_element()
}

/// Renders a block sequence as a column of elements.
///
/// Public because it is what `document` and the recursive cases both call, and
/// because a test can assert on a block tree without building a whole document.
///
/// **Every block element carries a debug selector, and that is the reason
/// `block_element` is testable at all.** It returns an `AnyElement`, which is
/// opaque: a caller can hold it and learn nothing about it. A debug selector
/// makes it findable from a real window — `cx.debug_bounds(..)` in a headless
/// test, the inspector in a running app — and a `Bounds` is the only place a
/// *height* becomes observable. That matters more than it sounds: variable row
/// height is the entire reason `docs/ARCHITECTURE.md` ADR-006 chose `gpui::List`
/// over `gpui::UniformList`, so **the claim that a code block is taller than a
/// one-word paragraph is a claim the decision rests on**, and without a selector
/// it is untestable.
///
/// **It is a `debug_selector` and not an `id`,** which is a distinction this file
/// has to get right twice: `src/ui/views/message_row.rs` already records that
/// *"`.id()` alone records nothing"*. An `ElementId` is state identity, and one id
/// per block across N rows of a virtualized list is N copies of the same
/// identity — the wrong tool for a debug hook, and the wrong tool twice over.
pub fn blocks(blocks: &[Block], colors: Colors, window: &Window) -> Div {
    blocks_prefixed("block", blocks, colors, window)
}

/// [`blocks`] with a selector prefix, so a nested block does not collide with
/// its parent or with a sibling that has the same index.
fn blocks_prefixed(prefix: &str, blocks: &[Block], colors: Colors, window: &Window) -> Div {
    let base = window.text_style();
    let mut root = div()
        .flex()
        .flex_col()
        .gap_1()
        // Explicit, per AGENTS.md §7.3: a container that carries text sets its
        // own colour rather than relying on a parent's.
        .text_color(colors.text);

    for (index, block) in blocks.iter().enumerate() {
        let selector = format!("{prefix}.{index}");
        root = root.child(block_element(block, &selector, colors, &base, window));
    }
    root
}

/// The elements one styled run of text is drawn with, in order.
///
/// **The return is `(text, runs)` rather than a `StyledText`, and that is what
/// makes step 4 testable without a window.** `StyledText::new(..).with_runs(..)`
/// panics on a run list that does not exactly cover the text
/// (`gpui/src/elements/text.rs:526-537`, in release builds too), so the property
/// worth asserting is the arithmetic: every byte of the text is in exactly one
/// run, and the styles are where the parser said they were. A caller that got a
/// `StyledText` back could only find that out by rendering it.
///
/// `base` is the caller's text style — `Window::text_style()` in a real frame,
/// `TextStyle::default()` in a test — so this function decides only what the
/// *message* adds to it.
pub fn runs(spans: &[Inline], colors: Colors, base: &TextStyle) -> (String, Vec<TextRun>) {
    let mut out = Runs::default();
    push_inlines(spans, Style::NONE, false, colors, base, &mut out);
    (out.text, out.runs)
}

/// The accumulator [`push_inlines`] writes into.
#[derive(Default)]
struct Runs {
    /// The paragraph's text, exactly as it will be laid out.
    text: String,
    /// One run per distinct style, covering `text` byte for byte.
    runs: Vec<TextRun>,
}

/// Appends one styled run, converting `core/markdown`'s style bits into GPUI's
/// text style.
///
/// **The length is `text.len()`, in bytes, and that is not an approximation.**
/// `TextRun::len` is documented as *"A number of utf8 bytes"* and
/// `StyledText::with_runs` slices the accumulated string by it, so a
/// character-count would silently mis-style every message containing a
/// multi-byte character — the exact class of bug that makes a non-Latin script
/// look broken and a Latin one look fine.
fn push_run(
    text: &str,
    style: Style,
    is_link: bool,
    colors: Colors,
    base: &TextStyle,
    out: &mut Runs,
) {
    if text.is_empty() {
        return;
    }

    let mut text_style = base.clone();
    // Links are the one inline construct that is not a `Style` bit: a link is
    // somewhere to go, not a typeface, so `core/markdown.rs` models it as a
    // variant that nests rather than a flag that combines.
    text_style.color = if is_link { colors.accent } else { colors.text };

    if style.contains(Style::BOLD) {
        text_style.font_weight = FontWeight::BOLD;
    }
    if style.contains(Style::ITALIC) {
        text_style.font_style = FontStyle::Italic;
    }
    if style.contains(Style::CODE) {
        text_style.background_color = Some(colors.code_block_bg);
    }
    if style.contains(Style::STRIKETHROUGH) {
        text_style.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(colors.text_muted),
        });
    }
    if is_link {
        text_style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(colors.accent),
            wavy: false,
        });
    }

    out.text.push_str(text);
    out.runs.push(text_style.to_run(text.len()));
}

/// Walks an inline sequence, flattening it into one text and one run per style.
///
/// `inherited` is the union of the styles already in force, which is how
/// `**bold with *italic* inside**` produces a bold run, a bold-italic run and a
/// bold run rather than three unrelated ones. `Style::union` is the operation
/// `core/markdown.rs` provides for exactly this, and it is additive: a nested
/// style never clears an enclosing one.
fn push_inlines(
    spans: &[Inline],
    inherited: Style,
    is_link: bool,
    colors: Colors,
    base: &TextStyle,
    out: &mut Runs,
) {
    for span in spans {
        match span {
            Inline::Text(text) => {
                push_run(
                    text.text(),
                    inherited.union(text.style()),
                    is_link,
                    colors,
                    base,
                    out,
                );
            }
            Inline::Link(link) => {
                // A `LinkSpan` in the tree is proof the target passed the
                // allowlist (`core/markdown.rs`), so this layer renders it as a
                // link without re-deciding. It does **not** make it clickable:
                // opening a URL is `platform/`'s business and no work unit has
                // wired it, so the run is styled and inert rather than styled and
                // silently doing nothing on click.
                push_inlines(link.children(), inherited, true, colors, base, out);
            }
        }
    }
}

/// One block, as an element.
///
/// `selector` is threaded down from [`blocks_prefixed`] so every rendered block
/// is addressable; see that function's docs for why an opaque `AnyElement` is
/// not a testable return, and why the address is a debug selector rather than an
/// `ElementId`.
fn block_element(
    block: &Block,
    selector: &str,
    colors: Colors,
    base: &TextStyle,
    window: &Window,
) -> AnyElement {
    // One allocation per block, not per arm: a closure would borrow `selector`
    // while the arms below also need it, and the prefix is the only thing that
    // has to be spelled more than once.
    let own = || selector.to_owned();
    let list_prefix = format!("{selector}.item");
    match block {
        Block::Paragraph { spans } => div()
            .debug_selector(own)
            .child(paragraph(spans, false, colors, base))
            .into_any_element(),
        Block::Heading { spans, .. } => div()
            .debug_selector(own)
            .child(paragraph(spans, true, colors, base))
            .into_any_element(),
        Block::Quote { blocks: quoted } => div()
            .debug_selector(own)
            .pl_3()
            .border_l_2()
            .border_color(colors.text_muted)
            .child(blocks_prefixed(&list_prefix, quoted, colors, window))
            .into_any_element(),
        Block::List {
            ordered,
            start,
            items,
        } => {
            let mut root = div()
                .debug_selector(own)
                .flex()
                .flex_col()
                .gap_1()
                .text_color(colors.text);
            for (index, item) in items.iter().enumerate() {
                // Cloned per item rather than moved: the quote arm above needs the
                // same prefix, and a `format!` per item is one small allocation
                // on a path that already builds an element tree.
                let item_prefix = format!("{list_prefix}.{index}");
                root = root.child(
                    div()
                        .debug_selector({
                            let p = item_prefix.clone();
                            move || p.clone()
                        })
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_start()
                        .child(div().text_color(colors.text_muted).child(list_marker(
                            *ordered,
                            *start,
                            index,
                            item.checked(),
                        )))
                        .child(blocks_prefixed(
                            &format!("{item_prefix}.block"),
                            item.blocks(),
                            colors,
                            window,
                        )),
                );
            }
            root.into_any_element()
        }
        Block::Rule => div()
            .debug_selector(own)
            .h(px(1.))
            .w_full()
            .bg(colors.text_muted)
            .into_any_element(),
        Block::CodeBlock { text, .. } => div()
            .debug_selector(own)
            .p_2()
            .rounded_sm()
            .bg(colors.code_block_bg)
            // Explicit, per AGENTS.md 7.3. A code block's text is never parsed
            // for inline markup (`core/markdown.rs`), so `text` goes in whole.
            .text_color(colors.text)
            .child(text.clone())
            .into_any_element(),
    }
}

/// A paragraph or heading: one `StyledText` made of the runs the spans produce.
fn paragraph(spans: &[Inline], heading: bool, colors: Colors, base: &TextStyle) -> AnyElement {
    let (text, mut runs) = runs(spans, colors, base);

    if heading {
        // A heading is bold as a *block*, so the weight is applied to every run
        // rather than to one inline style. `runs` has already resolved each
        // run's weight, and a heading's is at least BOLD.
        for run in &mut runs {
            run.font.weight = FontWeight::BOLD;
        }
    }

    div()
        .flex()
        .flex_col()
        // Explicit, per AGENTS.md 7.3.
        .text_color(colors.text)
        .when(heading, |paragraph| paragraph.text_lg())
        .child(StyledText::new(text).with_runs(runs))
        .into_any_element()
}

/// The bullet, number or checkbox a list item starts with.
///
/// Task lists are `Some(true)` / `Some(false)` and an ordinary item is `None` —
/// the tri-state `core/markdown.rs` keeps so that "not a task" and an
/// "unchecked task" do not render the same.
///
/// **Public for the same reason [`blocks`] and [`runs`] are**, and by the same
/// rule: it is a pure function whose whole behaviour is a four-way decision, and
/// an external test can call it directly. Asserting it through a rendered
/// element would mean reading a bullet glyph out of a `Bounds`, which tests
/// nothing about which arm ran. The numbers are the part that has to be right:
/// `start + index` is what makes an ordered list that begins at 7 read 7, 8, 9,
/// and a `u64` overflow here would be a silent `0.` rather than a panic.
pub fn list_marker(ordered: bool, start: u64, index: usize, checked: Option<bool>) -> String {
    match checked {
        Some(true) => "[x]".to_owned(),
        Some(false) => "[ ]".to_owned(),
        None if ordered => format!("{}.", start.saturating_add(index as u64)),
        None => "•".to_owned(),
    }
}
