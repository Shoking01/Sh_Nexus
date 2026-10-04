//! The views this layer renders.
//!
//! `AGENTS.md` §3.1 lists sidebar, chat view, thread panel and input bar here.
//! Three of them exist so far — the message list, a row of it, and a line that says
//! what the connection is doing — and the rest are **absent rather than declared
//! empty**, for the reason `src/lib.rs` gives about `ui/` itself: a module that
//! exists and does nothing reads as finished work.
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
//!
//! # And where the composer sits relative to both
//!
//! [`input_bar`] is a *sibling* of the list rather than a third stage of it, and
//! the reason is the direction of the two dependencies. The list and the row
//! point downwards: a row is a *projection* of a message the state holds, and
//! `MessageList` is a container that *asks* for rows. The composer points the
//! other way — it holds the list and *calls into it*, through
//! `MessageList::begin_send` — so folding it into `message_list.rs` would make
//! that file the owner of both a container and the thing that drives it, which
//! is the two-way coupling ADR-006's step order exists to avoid.
//!
//! **The composer builds its own text field rather than composing one, and that
//! is a finding about the framework rather than a preference**: `gpui` at
//! `rev e683fd7` has no text-input element and no `InputState`. Its module docs
//! record the verified evidence and the dependency that was rejected instead.
//!
//! # And where the banner sits relative to both
//!
//! [`connection_banner`] is a *sibling* of both rather than part of either, and the
//! reason is that it is the only thing in this module that reads the connection's
//! state rather than a channel's. It sits between the log and the composer because
//! that is the strip a user's eye crosses on its way to the field, and a client that
//! cannot send has to be able to learn why *before* they type.
//!
//! **It is the only module here that renders nothing for the common case**, which
//! is why it is a function returning `Option<AnyElement>` rather than an
//! `Entity` — `Render` cannot return nothing, and a permanently-on health indicator
//! would be a permanent distraction. Its own module docs carry that reasoning in
//! full; the short form is that presence (`Shell::transport`) and content
//! (`ConnectionState`) are two different questions with two different owners.

pub mod connection_banner;
pub mod input_bar;
pub mod message_list;
pub mod message_row;
