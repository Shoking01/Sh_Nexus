//! The application's state: what it knows, and the only ways it may change.
//!
//! `AGENTS.md` §3.1 places three files here, and this layer now has all three:
//!
//! | File | Role | Status |
//! |---|---|---|
//! | [`app_state`] | the data, plus the `pub(crate)` primitives that move it | work unit 1E-1 |
//! | [`actions`] | every decision about when and how to move it | work unit 1E-1 |
//! | [`bridge`] | the **only** module permitted to call `cx.update_global` / `cx.update` | work unit 1E-2 |
//!
//! The split is not cosmetic. [`app_state`] and [`actions`] are pure and hold no
//! `gpui`, which is what makes the ≥80% `state/` coverage floor of `AGENTS.md`
//! §4.1 measurable at all; [`bridge`] is the one file that imports `gpui`, and it
//! owns the `gpui::Global` the other two are reached through.
//!
//! # Why the seam is load-bearing, and what it does and does not prove
//!
//! `AGENTS.md` §7.3 forbids blocking `cx.update_global` from a non-UI thread and
//! §3.2 forbids `network/` from importing `gpui`. Something has to carry an event
//! from a socket to the main thread, and the module that produces it may not
//! call the function that delivers it. `PLAN.md` §4 resolves that by naming an
//! owner, and the resolution is *compilable* rather than conventional: GPUI's
//! `Context<'a, T>` borrows a `&'a mut App`, and `App` holds `Rc`s, so
//! `Context` is `!Send` and a worker thread **cannot** call `cx.update_global`
//! at all. That is a stronger guarantee than `AGENTS.md` had to ask for.
//!
//! **What it does not do is confine this layer's values.** [`AppState`] is
//! `Send + Sync`, so a copy could be moved to a worker thread, shared behind a
//! lock, or handed to a task without a compiler objection. The single-thread
//! invariant is therefore a **written rule** — see `app_state`'s module docs, §3
//! — and it is enforced by guards rather than by the type:
//! `the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee`
//! demonstrates the move is expressible, and
//! `state_app_state_and_actions_hold_no_interior_mutability` makes the cheap
//! mistake (reaching for a `Mutex` to make it convenient) a build failure.
//!
//! **The verdict on work unit 1C-2b's no-lock decision is recorded in
//! `app_state`'s module docs, §3, and it is more qualified than 1C-2b wrote:**
//! the decision holds, and one of its two stated pillars is wrong.
//!
//! **What 1E-2 changed, and what it did not.** Before the bridge existed, every
//! caller of [`actions`] was a place the invariant had to be trusted. It is now
//! one crossing instead of many: the state is reachable only through the
//! `gpui::Global` in [`bridge`], and that is reachable only from a main-thread
//! context, because the *value* that crosses the thread boundary is a
//! [`DomainEvent`] and the state never does. The global itself is `!Sync`, so no
//! second thread can hold a reference to the state even in principle.
//!
//! **The residual gap is narrowed, not closed, and the narrowing is a scanner
//! rather than a type.** `only_the_bridge_constructs_an_application_state` in
//! `crates/sh_nexus/tests/bridge.rs` fails the build when a module outside
//! `bridge.rs` constructs an [`AppState`] of its own — the cheap mistake, and the
//! one a Phase 3 or Phase 4 author would actually make. **What stays open is a
//! module that is *handed* a state by value**, which no guard in this repository
//! would see; closing that would mean making `AppState::new` crate-private, and
//! `app_state.rs` is not this work unit's file to change. `bridge.rs`'s module
//! docs, §1, says the same thing in the same words rather than claiming a
//! guarantee that does not exist.
//!
//! # What the layer may not reach
//!
//! | Not here | Where it is | Why |
//! |---|---|---|
//! | `gpui` | **only** `state/bridge.rs`, and `ui/` | §3.2. A pure `AppState` is what makes the `state/` coverage floor measurable at all |
//! | `tokio`, sockets, the filesystem | `network/`, `db/`, `platform/` | §3.2, and the same reason |
//! | a clock or a random `Uuid` | the caller of [`actions::begin_send`] | the two facts this layer cannot know, supplied rather than invented |
//! | `ShNexusError` | `errors.rs` | §3.3's direction of travel is module error → the global error, never the reverse |
//! | the outbox | `db/`, Phase 3 | `PLAN.md` §7 — persistence is not client state |
//!
//! The first row is now the one 1E-2's guards hold: `app_state.rs` and
//! `actions.rs` are named by file rather than by directory *precisely so that*
//! `bridge.rs` could import `gpui` legitimately, and
//! `bridge_is_the_only_file_that_calls_the_context_mutating_api` in
//! `crates/sh_nexus/tests/bridge.rs` holds the other half of that promise.

pub mod actions;
pub mod app_state;
pub mod bridge;

pub use crate::state::actions::{ApplyOutcome, IgnoreReason, SendOutcome};
pub use crate::state::app_state::{
    AppState, DeliveryState, DifferingFields, SendFailure, DEFAULT_SEGMENT_CACHE_BUDGET,
    DEFAULT_SEGMENT_CACHE_CAPACITY, MAX_TYPING_CHANNELS, MAX_TYPING_USERS_PER_CHANNEL,
};
