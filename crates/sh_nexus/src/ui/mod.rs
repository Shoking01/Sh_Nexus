//! Presentation only: this layer reads state, renders elements, and dispatches
//! gestures. It decides nothing.
//!
//! `AGENTS.md` §3.2 and `PLAN.md` §4 give `ui/` one job, and `docs/ARCHITECTURE.md`
//! ADR-006's work order makes the *guard* for that job the first thing this
//! layer gets. That ordering is the one work unit 1E-1 used and the reason is
//! mechanical rather than stylistic: a boundary written after the code is a
//! boundary that has to be retrofitted around whatever the code already does,
//! and the retrofit is where the exception gets made.
//!
//! # The layer rule, and where it lives
//!
//! `crates/sh_nexus/tests/layer_boundary.rs` enforces this, so the table below
//! is a description of a build failure rather than a convention.
//!
//! | `ui/` may name | why |
//! |---|---|
//! | `gpui` | this is where GPUI was confined (`PLAN.md` §4) |
//! | `crate::state::bridge` | the single seam (`PLAN.md` §4) |
//! | `crate::state::DeliveryState` | client state the UI displays, named as such by `PLAN.md` §5 |
//! | `crate::core::{markdown, models, theme}` | pure data and pure parsers, no I/O |
//! | `crate::ui` | its own modules |
//!
//! | `ui/` may not name | why |
//! |---|---|
//! | `crate::network`, `crate::db` | §3.2: the layers below are reached through the seam or not at all |
//! | `crate::core::cache` | the rendered-segment cache is reached through the bridge, which is what keeps its recency order a statement about the main thread |
//! | `crate::state::{app_state, actions}` | the named doors in `bridge.rs` are the audit trail (§3.2); importing the mutators would route around them |
//!
//! **`state::DeliveryState` is the one state type this layer names, and the
//! reason is that it is not a domain type.** `PLAN.md` §5 puts it in `state/`
//! precisely because it is a fact about *this* client's connection, and a row
//! that says "sending…" or "failed" has to name it to render. The same
//! reasoning does **not** extend to `AppState`: `tests/bridge.rs`'s
//! `no_module_outside_state_names_the_state_type` fails the build on that name
//! anywhere outside `state/`, and this layer reads the state through
//! `bridge::try_read` instead of holding one.
//!
//! # What is here
//!
//! [`markdown`] renders a parsed message's segment tree as GPUI elements, and
//! is the first consumer of `core/markdown.rs`. [`views`] holds the message
//! list and its rows. [`Colors`] is the palette adapter every element on this
//! layer draws its explicit colour from (`AGENTS.md` §7.3: GPUI does not
//! inherit a colour from a parent, so every text element sets one).
//!
//! **`Colors` carries the palette keys something on this layer actually
//! draws, and no others.** The theme has thirteen; `sidebar` and `accent_hover`
//! belong to views that do not exist yet, and `success` is the delivered state
//! that [`views::message_row`] deliberately leaves silent (`PLAN.md` §7 makes a
//! *failure* visible; a badge on every delivered message is noise). A palette
//! field nothing reads is a scaffold wearing a colour's name.

pub mod markdown;
pub mod views;

use gpui::{rgb, Hsla};

use crate::core::theme::{BuiltIn, Color, Palette};

/// The colours a rendered message is drawn with.
///
/// # Why a `Copy` struct rather than the theme itself
///
/// A row is drawn on every frame it is visible, and the theme is a parsed
/// document with strings in it. Reading thirteen fields out of the palette once,
/// when the view is built, means the frame path copies a few hundred bytes
/// instead of reaching back into a document. `Colors` is derived `Copy`, so a
/// row holds its own and nothing borrows.
///
/// # Why the fields are `Hsla`
///
/// GPUI's `text_color`, `bg` and `TextRun::color` all take or convert from
/// `Hsla`, so converting once here is what keeps the conversion out of the
/// element tree. `core/theme::Color` stays the project's validated format
/// (`#rrggbb`, `AGENTS.md` §10.1); this is a rendering-side projection of it and
/// is never persisted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    /// Behind the whole list.
    pub background: Hsla,
    /// Raised panels, and a code block's inline background.
    pub surface: Hsla,
    /// Body text.
    pub text: Hsla,
    /// Timestamps, counts, markers, a quote's rule.
    pub text_muted: Hsla,
    /// The interactive colour: links, and this client's own name.
    pub accent: Hsla,
    /// The failed-send state.
    pub danger: Hsla,
    /// Somebody else's name, and a mention.
    pub mention: Hsla,
    /// Behind a fenced code block.
    pub code_block_bg: Hsla,
    /// Behind this client's own messages.
    pub bubble_self: Hsla,
    /// Behind everyone else's.
    pub bubble_other: Hsla,
}

impl Colors {
    /// Projects a validated theme palette onto the rendering colours.
    ///
    /// Total, and deliberately: every field of the theme's palette is required
    /// by its own validator (`core/theme.rs`), so there is no missing key for
    /// this to guess at and no `Option` for a caller to carry.
    pub fn from_palette(palette: &Palette) -> Self {
        Self {
            background: hsla(palette.background),
            surface: hsla(palette.surface),
            text: hsla(palette.text),
            text_muted: hsla(palette.text_muted),
            accent: hsla(palette.accent),
            danger: hsla(palette.danger),
            mention: hsla(palette.mention),
            code_block_bg: hsla(palette.code_block_bg),
            bubble_self: hsla(palette.bubble_self),
            bubble_other: hsla(palette.bubble_other),
        }
    }

    /// The built-in dark theme, with a literal fallback for a theme that fails
    /// to parse.
    ///
    /// **The fallback exists so this cannot fail, not because it is expected.**
    /// `BuiltIn::Dark.theme()` returns a `Result` because a build-time fixture
    /// can be corrupted by a bad merge, and `AGENTS.md` §2.1 forbids the
    /// `unwrap` that would otherwise be the obvious response. Returning
    /// `Result<Colors, _>` instead would push the decision onto every caller —
    /// including the frame path — and there is no caller for which "no colours"
    /// is a useful answer. `tests/theme.rs` is what keeps the built-in document
    /// `Ok`, so the fallback is unreachable in a shipped binary and is the
    /// honest shape for a path that cannot be allowed to panic.
    pub fn dark() -> Self {
        match BuiltIn::Dark.theme() {
            Ok(theme) => Self::from_palette(theme.colors()),
            Err(_) => Self::dark_fallback(),
        }
    }

    /// The built-in dark theme's values, spelled out.
    ///
    /// Every constant here is duplicated from `themes/dark.json`, and that
    /// duplication is deliberate: this function's whole job is to be the value
    /// that survives the theme *not* parsing, so it cannot be the parse's
    /// output.
    ///
    /// **Public so that the duplication is testable**, which is the only thing
    /// that makes a duplicated constant safe. `tests/ui_message_list.rs`'s
    /// `the_palette_matches_the_built_in_dark_theme_and_its_fallback` compares
    /// this against the parsed document, so the two cannot drift while the theme
    /// is valid — which is every shipped build, and therefore the only state in
    /// which drift could go unnoticed.
    pub fn dark_fallback() -> Self {
        Self {
            background: rgb(0x14161c).into(),
            surface: rgb(0x1e2129).into(),
            text: rgb(0xe4e7ee).into(),
            text_muted: rgb(0x949aab).into(),
            accent: rgb(0x5b8def).into(),
            danger: rgb(0xe5534b).into(),
            mention: rgb(0xd9b56b).into(),
            code_block_bg: rgb(0x0b0d12).into(),
            bubble_self: rgb(0x2a2f3c).into(),
            bubble_other: rgb(0x1e2129).into(),
        }
    }
}

impl Default for Colors {
    /// The built-in dark theme — the theme `core/theme.rs` falls back to as
    /// well, so an unconfigured client and an invalid theme agree.
    fn default() -> Self {
        Self::dark()
    }
}

/// A validated theme colour, as a rendering colour.
///
/// `core/theme::Color` is `#rrggbb`; `gpui::rgb` takes the same value packed
/// into a `u32`.
fn hsla(color: Color) -> Hsla {
    let packed =
        u32::from(color.red()) << 16 | u32::from(color.green()) << 8 | u32::from(color.blue());
    rgb(packed).into()
}
