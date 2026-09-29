//! The application's state: what it knows, and the only ways it may change.
//!
//! `AGENTS.md` §3.1 places three files here and this work unit writes two of
//! them:
//!
//! | File | Role | Status |
//! |---|---|---|
//! | [`app_state`] | the data, plus the `pub(crate)` primitives that move it | work unit 1E-1 |
//! | [`actions`] | every decision about when and how to move it | work unit 1E-1 |
//! | `bridge.rs` | the **only** module permitted to call `cx.update_global` / `cx.update` | work unit 1E-2 |
//!
//! **The third file does not exist yet and that is deliberate, not an
//! oversight.** `PLAN.md` §4 makes `bridge.rs` the single seam between
//! `network/`'s plain domain events and the main thread, and it is the file that
//! will import `gpui`. Writing the two pure files first means the seam is the
//! *only* thing left to add, rather than a seam tangled through state that does
//! not exist yet.
//!
//! # Why the seam is load-bearing, and what it does and does not prove
//!
//! `AGENTS.md` §7.3 forbids blocking `cx.update_global` from a non-UI thread and
//! §3.2 forbids `network/` from importing `gpui`. Something has to carry an event
//! from a tokio task to the main thread, and the module that produces it may not
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
//! — and it is enforced here by two guards rather than by the type:
//! `the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee`
//! demonstrates the move is expressible, and
//! `state_app_state_and_actions_hold_no_interior_mutability` makes the cheap
//! mistake (reaching for a `Mutex` to make it convenient) a build failure.
//!
//! **The verdict on work unit 1C-2b's no-lock decision is recorded in
//! `app_state`'s module docs, §3, and it is more qualified than 1C-2b wrote:**
//! the decision holds, and one of its two stated pillars is wrong.
//!
//! # What the layer may not reach
//!
//! | Not here | Where it is | Why |
//! |---|---|---|
//! | `gpui` | `state/bridge.rs`, `ui/` | §3.2. A pure `AppState` is what makes the `state/` coverage floor measurable at all |
//! | `tokio`, sockets, the filesystem | `network/`, `db/`, `platform/` | §3.2, and the same reason |
//! | a clock or a random `Uuid` | the caller of [`actions::begin_send`] | the two facts this layer cannot know, supplied rather than invented |
//! | `ShNexusError` | `errors.rs` | §3.3's direction of travel is module error → the global error, never the reverse |
//! | the outbox | `db/`, Phase 3 | `PLAN.md` §7 — persistence is not client state |
//!
//! Each of those is checked by a test that names the two files this work unit
//! wrote, so the guard stays exact when `bridge.rs` arrives with its `gpui`.

pub mod actions;
pub mod app_state;

pub use crate::state::actions::{ApplyOutcome, IgnoreReason, SendOutcome};
pub use crate::state::app_state::{
    AppState, DeliveryState, DifferingFields, SendFailure, DEFAULT_SEGMENT_CACHE_BUDGET,
    DEFAULT_SEGMENT_CACHE_CAPACITY, MAX_TYPING_CHANNELS, MAX_TYPING_USERS_PER_CHANNEL,
};
