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
//! directory and fails if any of them names `serde`, `gpui`, `tokio`,
//! `sh_nexus_wire`, `reqwest`, `rusqlite`, `std::fs` or `std::io`, after
//! stripping comments.
//!
//! # What is in scope for work unit 1A
//!
//! Only [`models`]. `markdown`, `cache`, `ordering` and `theme` are `AGENTS.md`
//! §3.1 modules that later Phase 1 work units implement, and they are not
//! declared here rather than being declared empty: a module that exists and
//! does nothing is worse than a module that does not exist, because it reads as
//! finished work and `cargo` will not tell anyone it is hollow.

pub mod models;
