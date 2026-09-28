//! Pure domain logic.
//!
//! `AGENTS.md` §3.2: *"Pure, testable logic with no side effects. Must not
//! import `gpui`, `tokio`, or any UI/platform code."* And `PLAN.md` §4 adds the
//! filesystem: no file reads either, which is why theme *discovery* lives in
//! `platform/file_watch.rs` and `core/theme.rs` only ever parses bytes it is
//! handed.
//!
//! Those are not style rules; they are what makes the module's coverage floor
//! reachable. `AGENTS.md` §4.1 requires ≥90% for `core/`, and a layer that
//! opens a socket or reads a file cannot be tested hermetically at that rate --
//! so the floor is either met by keeping the layer pure, or waived. This
//! project keeps it pure.
//!
//! The rule is enforced, not merely stated:
//! `crates/sh_nexus/tests/layer_boundary.rs` scans every file under this
//! directory and fails if any of them names `serde` (the derive crate; its
//! parser, `serde_json`, is admitted -- see the test's own documentation),
//! `gpui`, `tokio`, `sh_nexus_wire`, `reqwest`, `rusqlite`, `std::fs` or
//! `std::io`, after stripping comments.
//!
//! # What is in scope
//!
//! [`models`], [`ordering`], [`markdown`], [`cache`] and [`theme`]. Every one
//! of them is a logic module, which is what §2.3's *"never render 10,000
//! DOM-equivalent elements"* analogue for this layer is: `core/` holds no I/O at
//! all, so the coverage floor is reachable by measurement rather than waived.
//!
//! [`markdown`] is the first `core/` module to take a *dependency*, and the
//! allow-list in `crates/sh_nexus/tests/layer_boundary.rs` was widened to admit
//! it in the same commit. That is the point of an allow-list: the crate was
//! rejected until somebody added it to the list and wrote down why. [`cache`]
//! needed no widening: it is `std`'s `HashMap` and nothing else. [`theme`]
//! needed the one widening that is a *split* rather than an addition -- it may
//! name `serde_json` and must not name `serde` -- because it parses a theme
//! document with a JSON parser and derives nothing.
//!
//! # The `serde` split, in one paragraph
//!
//! Until work unit 1D the boundary test carried a single forbidden token,
//! `"serde"`, and it was a **substring** check. That made `serde_json`
//! unreachable from `core/` by any spelling, including a fully qualified
//! `serde_json::from_slice` -- the allow-list and the token scan disagreed with
//! each other, which is the shape of a boundary nobody has checked. 1D splits
//! it: every occurrence of the literal `serde` in `core/` must now be part of
//! the single permitted name `serde_json`, and the test asserts both halves
//! with synthetic sources rather than relying on the tree to be green.

pub mod cache;
pub mod markdown;
pub mod models;
pub mod ordering;
pub mod theme;
