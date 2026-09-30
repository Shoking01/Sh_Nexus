//! The views this layer renders.
//!
//! `AGENTS.md` §3.1 lists sidebar, chat view, thread panel and input bar here.
//! Two of them exist so far — the message list and a row of it — and the rest
//! are **absent rather than declared empty**, for the reason `src/lib.rs` gives
//! about `ui/` itself: a module that exists and does nothing reads as finished
//! work.
//!
//! # What the split is between list and row
//!
//! `docs/ARCHITECTURE.md` ADR-006's steps 2 and 3 divide the work along the line
//! the framework itself draws. The list owns the *scrolling* state — the
//! `ListState`, the item count, and where the viewport is — because that is what
//! `gpui::List` requires the caller to hold. A row owns *its own message* and
//! nothing else, and is an `Entity` because the list asks for one: a row's state
//! has to survive being scrolled out of view and back, which a rebuilt element
//! cannot do.

pub mod message_list;
pub mod message_row;
