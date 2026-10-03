//! Networking: protocol handling only.
//!
//! `AGENTS.md` §3.2: *"Protocol handling only. Parses wire formats into
//! `core::models`. Emits domain events; never touches GPUI state directly."*
//!
//! That last clause is a hard prohibition, and it is the reason
//! `state/bridge.rs` exists. §7.3 forbids blocking `cx.update_global` from a
//! non-UI thread while §3.2 forbids `network/` from importing GPUI, and the
//! obvious way to satisfy the first -- call `cx.update_global` from the
//! WebSocket task -- is precisely the one §3.2 prohibits. `PLAN.md` §4
//! resolves it with a named owner: `network/` emits plain
//! [`crate::core::models::DomainEvent`] values and `state/bridge.rs` is the only
//! module that calls `cx.update_global`, always on the main thread.
//!
//! `crates/sh_nexus/tests/layer_boundary.rs` asserts the GPUI prohibition by
//! scanning this directory's source, so the rule is a test failure rather than a
//! review opinion.
//!
//! # What is in scope today
//!
//! [`mapping`] is the wire/domain boundary and the validation that `AGENTS.md`
//! §2.1 requires to happen before a payload touches state. [`ws`] is the
//! WebSocket transport that produces those events, on a worker thread, and hands
//! them to `state/bridge.rs` — the only module in this crate permitted to reach
//! the main thread.
//!
//! `rest` and `auth` are `AGENTS.md` §3.1 modules that arrive with ADR-010's
//! next PR (`PLAN.md` §8), and they are **not** declared here rather than
//! declared empty: a module that exists and does nothing reads as finished work,
//! and `cargo` will not tell anyone it is hollow.
//!
//! **The transport does not live in a file called `websocket.rs`, and `ws.rs` is
//! the reason.** `AGENTS.md` §3.1's table names the module `websocket.rs`, and
//! the name was kept at the cost of a slight mismatch with that table rather than
//! the other way round, because `reconnect.rs` is a *separate* `AGENTS.md` §3.1
//! module and this file contains its backoff schedule. Filing both under
//! `websocket.rs` would have merged two entries of the constitution's own tree
//! into one file, and `reconnect.rs` deserves its own row in that table the day
//! `Auth` lands beside it.

pub mod mapping;
pub mod ws;
