//! The composer: a text field that sends on Enter.
//!
//! # Why this file builds a text field instead of composing one
//!
//! **There is no text-input widget in `gpui` at the pinned `rev e683fd7`,** and
//! that was verified against the checkout rather than taken on report:
//!
//! - `crates/gpui/src/elements/` holds `anchored, animation, canvas,
//!   container_query, deferred, div, image_cache, img, list, mod, surface, svg,
//!   text, uniform_list` — **no `input.rs`**.
//! - `crates/gpui/src/input.rs` exists, but it is platform/IME plumbing:
//!   [`EntityInputHandler`] and `ElementInputHandler<V>`, not a widget.
//! - **`InputState` does not exist anywhere in the checkout.** Zed's own
//!   single-line field is `ui_input::InputField`, which depends on `ui`, `editor`
//!   and `component` — none of them in this project's graph, and `AGENTS.md` §7.2
//!   makes each one a recorded decision rather than something to smuggle in
//!   alongside a feature.
//!
//! So the composer **implements [`EntityInputHandler`] itself over its own
//! [`String`]**. That is the honest reading of "constructible today": the
//! *protocol* is public and eight methods wide, and the *widget* does not exist.
//! The alternative — three new dependencies for a single-line field — was
//! rejected for the reason `AGENTS.md` §7.1 states: every dependency is compile
//! time and supply-chain risk, and a chat composer is not worth three crates.
//!
//! # The three mechanisms this file has to get right, and what each costs
//!
//! All three are properties of `gpui` at this rev, and all three were read in
//! the framework's source rather than inferred from behaviour.
//!
//! ## 1. Registration happens during paint, so it happens in a `canvas`
//!
//! `Window::handle_input` is documented as *"should only be called as part of the
//! paint phase of element drawing"* and asserts it (`self.invalidator
//! .debug_assert_paint()`, `gpui/src/window.rs:5211`). It is the only door to
//! the platform's text input, and it lives on `Window` rather than on `Context`,
//! so it cannot be called from an event handler at all.
//!
//! **A `canvas` is the only element that runs a closure with the `Window` in
//! hand during paint**, and it is the idiom `gpui/src/input.rs:384-407` uses in
//! its own test for exactly this. So the field is registered from the canvas's
//! paint closure, with the element's own bounds — which is why the bounds
//! parameter exists at all: they are the rectangle the platform should use for
//! caret geometry.
//!
//! ## 2. Character keys must **propagate**, and this is the one silently broken thing
//!
//! `Window::dispatch_keystroke` (`gpui/src/window.rs:5365`) forwards `key_char`
//! to the focused [`InputHandler`](gpui::InputHandler) **only when the key
//! propagated**:
//!
//! ```text
//! let result = self.dispatch_event(PlatformInput::KeyDown(..), cx);
//! if !result.propagate { return true; }        // <- character dropped here
//! if let Some(input) = keystroke.key_char { .. input_handler.dispatch_input(input, ..) }
//! ```
//!
//! **A composer that stops propagation on character keys is untypeable**, and it
//! would look completely correct: the key handler runs, nothing panics, and no
//! text appears. Every other key in this file returns without touching
//! `cx.propagate_event` for exactly this reason, and
//! `tests/app_shell.rs`'s `typing_reaches_the_draft_through_the_platform_input_handler`
//! is the test that says so.
//!
//! ## 3. Enter must **stop** propagation — the opposite rule, for a reason nobody guesses
//!
//! `dispatch_keystroke` begins with `keystroke.with_simulated_ime()`, and
//! `with_simulated_ime` (`gpui/src/platform/keystroke.rs:242`) synthesises a
//! `key_char` for named keys:
//!
//! ```text
//! "space" => Some(" ".into()),
//! "tab"   => Some("\t".into()),
//! "enter" => Some("\n".into()),
//! ```
//!
//! **So a propagating Enter sends the message *and then* appends a newline to the
//! draft it just cleared.** One keypress, one sent message, and a composer
//! holding `"\n"` that looks empty and is not. `is_printable_key("enter")` is
//! `true` (the deny-list at `keystroke.rs:378` does not name it), which is
//! exactly why that table has an explicit `"enter"` arm.
//!
//! This is the inverse of rule 2, it applies to `space` and `tab` for the same
//! reason, and it is why this file's key handler matches exactly two keys and
//! treats every other key as *not its business*.
//!
//! # What this field is not
//!
//! **A single-line, append-at-the-end, no-selection field, and the eight
//! [`EntityInputHandler`] methods are written to that shape rather than to the
//! shape of a general text editor.** Each method that would need a selection or a
//! caret coordinate to answer *correctly* says so in its own documentation and
//! returns the conservative answer instead of a plausible wrong one. Concretely:
//!
//! | Not here | Why |
//! |---|---|
//! | a selection, a caret, arrow keys, Home/End | `PLAN.md` says "send on Enter"; a caret is a product decision this unit does not make |
//! | multi-line, `shift-enter`, `alt-enter` | the task's own Out list: *"newline insertion is not specified and is a product decision"* |
//! | an IME composition mark | `marked_text_range` answers `None`, so a composing IME is treated as plain insertion — see that method |
//! | click-to-position the caret | `character_index_for_point` answers `None`, so the platform does not move a caret that does not exist |
//! | history, draft persistence, retry/discard buttons | `db/` is Phase 3; `retry_send` has its own decision to make |
//!
//! **The draft text is a plain element child, not a text run, and that is a
//! consequence of §3 rather than a simplification.** The field renders what the
//! user typed; it does not render a cursor, and a cursor that does not blink in
//! step with a caret that does not exist would be a lie about where typing goes.
//!
//! # The one bound, and why it is [`MAX_MESSAGE_BYTES`]
//!
//! `AGENTS.md` §7.1 bans unbounded growth of in-memory state, and a draft grows
//! by however much a user pastes into it — so the ceiling is enforced at
//! [`InputBar::replace_text_in_range`], which is the single choke point for
//! *both* typing and the [`EntityInputHandler::paste`] default.
//!
//! **The number is [`crate::core::markdown::MAX_MESSAGE_BYTES`] and not a new
//! one**, because that constant answers the only question that matters here: it
//! is the largest body the render path will show *faithfully*, since
//! `core/markdown.rs` truncates past it. A draft longer than that is a message
//! the user cannot see all of, so accepting it would be letting someone send
//! something this client has no way to display. §2.1's rejection of unbounded
//! in-memory state is answered by a number that already exists and is already
//! documented, rather than by a second one that would have to be kept in step.

use std::ops::Range;

use chrono::Utc;
use gpui::{
    canvas, div, prelude::*, App, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, KeyDownEvent, Pixels, Point, Render, TextInputAction,
    TextInputConfiguration, UTF16Selection, Window,
};
use uuid::Uuid;

use crate::core::markdown::MAX_MESSAGE_BYTES;
use crate::ui::views::message_list::MessageList;
use crate::ui::Colors;

/// The key [`InputBar::on_key_down`] sends on, as `gpui`'s keystroke parser
/// spells it.
///
/// **A constant rather than an inline literal because the two keys in this
/// handler are a matched pair**: this one is claimed, and every other key is
/// deliberately not. Spelling the pair next to each other is what makes the
/// asymmetry in [`InputBar::on_key_down`] — propagate for characters, stop for
/// `enter` — readable instead of looking like an oversight.
const SEND_KEY: &str = "enter";

/// The key [`InputBar::on_key_down`] blurs on, as `gpui`'s keystroke parser
/// spells it. See [`SEND_KEY`].
const BLUR_KEY: &str = "escape";

/// Shown in the field while the draft is empty.
///
/// **An instruction rather than a channel name, and that is forced.** The shell
/// cannot ask which channel it is composing into — nothing populates the channel
/// list (`actions::set_channels` has no door until a `DomainEvent` carries one,
/// `bridge.rs` §5) — so a name here would be a guess this layer is not entitled
/// to make, exactly as `RowSpec::author` is an id rather than a display name.
const PLACEHOLDER: &str = "Type a message, then press Enter";

/// The composer: a text field, and the send gesture.
///
/// **It holds the message list, and the reason is that [`MessageList::begin_send`]
/// is the only send door there is.** That method fills `channel_id` from the
/// list's own channel and reports whether the optimistic row reached the screen;
/// a composer that held a channel id instead would be a second answer to "which
/// channel is this", and one that could disagree with the list it is drawn
/// above. Holding an `Entity` rather than a callback is the same reason
/// [`super::message_list::MessageList`] is an `Entity` in
/// [`crate::app::Shell`]: this view's send must be able to run from a key
/// handler, and a closure stored in the composer would be indirection with one
/// call site.
pub struct InputBar {
    /// What the user has typed so far, as UTF-8.
    ///
    /// A `String` rather than a text buffer because `InputState` does not exist
    /// at this `rev` — see the module docs. It is the *only* copy: the field has
    /// no undo stack, no selection, and no second representation, which is what
    /// makes [`InputBar::replace_text_in_range`] the single choke point where
    /// every byte enters and where [`MAX_MESSAGE_BYTES`] is enforced.
    draft: String,
    /// The list whose channel this composes into, and whose `begin_send` runs
    /// the send.
    list: Entity<MessageList>,
    /// The palette every element in this view draws with.
    ///
    /// Taken at construction rather than pushed later, for the reason
    /// [`crate::app::module docs`](crate::app)'s §4 gives: the palette is applied
    /// once, at construction, and a runtime theme change does not exist to serve.
    /// A `set_colors` nobody calls would be a second way to do the one thing
    /// `app.rs` already does in one.
    colors: Colors,
    /// The focus target for the field.
    ///
    /// **`track_focus` on this view's own element, and the handle is what
    /// [`Window::handle_input`] is keyed on.** A focus handle no element tracks
    /// can be focused but will not be the element the platform's dispatch tree
    /// routes to, and `dispatch_keystroke` reads the focused handle's handler —
    /// so an untracked handle is a field that can be focused and cannot be typed
    /// into, which is the same silent failure as §2.
    focus_handle: FocusHandle,
    /// Where [`BLUR_KEY`] sends focus, so that the window is never left with
    /// nothing focused.
    ///
    /// **This field exists because of a measured dead end, and the measurement is
    /// the reason it is here rather than being a nicety.** When no element is
    /// focused, `Window` routes keys to `DispatchTree::root_node_id()`
    /// (`gpui/src/window.rs:6258`), and *that node is not the root view's
    /// element* — it is a node below the frame's own root, so a window with
    /// nothing focused delivers every key to a listener list that is empty. A
    /// composer whose `Escape` called [`Window::blur`] and stopped there would
    /// therefore consume the key **and** make the shell's own `Escape` gesture
    /// unreachable, which is `AGENTS.md` §5.2's keyboard-only requirement broken
    /// by the very key meant to satisfy it.
    ///
    /// `tests/app_shell.rs` pins both halves of that: the composer's `Escape` is
    /// followed by a working shell `Escape`, and the same `Escape` with nothing
    /// focused would do nothing at all.
    fallback_focus: FocusHandle,
}

impl InputBar {
    /// Builds an empty composer for `list`, drawing with `colors`.
    ///
    /// `fallback_focus` is where `BLUR_KEY` sends focus — see
    /// `InputBar::fallback_focus` for why the field cannot simply blur and
    /// stop. It is the shell's own handle rather than something this view
    /// invents, because the shell is what owns the gesture focus has to be handed
    /// back to, and a second handle nobody focuses would be the dead end again.
    ///
    /// **The field starts unfocused**: focus is the caller's decision, because
    /// [`crate::app::open`] has a `Window` and this constructor does not, and a
    /// constructor that could not focus would push the question onto every caller
    /// — which is the property [`MessageList::new`]'s own documentation is
    /// written against.
    pub fn new(
        list: Entity<MessageList>,
        fallback_focus: FocusHandle,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            draft: String::new(),
            list,
            colors,
            focus_handle: cx.focus_handle(),
            fallback_focus,
        }
    }

    /// What the user has typed, for a test to assert on without a window.
    ///
    /// **Read access is the reason a test can prove the typing path, and the
    /// proof has to exist**: the failure mode in the module docs, §2, is a
    /// composer that renders and handles keys and silently receives no text, and
    /// a test that walked the element tree instead of asking the view would not
    /// see it — the element tree is *built* whether or not the platform ever
    /// calls back into the field.
    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Whether this field currently holds focus.
    ///
    /// The question [`Shell`](crate::app::Shell) needs in order to decide
    /// whether `Escape` is its own, and exposed because a caller that guessed
    /// instead of asking would be the second source of truth about focus.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// The draft's byte length, for the [`MAX_MESSAGE_BYTES`] bound.
    ///
    /// A method rather than a comparison at the call site so the bound is
    /// enforced in **bytes**, matching [`MAX_MESSAGE_BYTES`]'s own unit. Counting
    /// characters would admit a 256 KiB draft of four-byte emoji, and the
    /// constant is a byte ceiling for a byte reason — pure CPU on the UI thread,
    /// `AGENTS.md` §2.3.
    fn draft_bytes(&self) -> usize {
        self.draft.len()
    }

    /// Sends the draft, and clears it if the send was accepted.
    ///
    /// **The blank check comes first, before anything is minted, and that
    /// ordering is the behaviour rather than an optimisation.** `actions.rs`
    /// rejects a whitespace-only body with `EmptyContent` — *"a message with no
    /// body and no attachment is not a message"* — so the state would refuse it
    /// anyway. Checking here is what lets a blank `Enter` leave the draft
    /// **untouched**: a refused send that cleared the field would destroy what
    /// the user was in the middle of typing, which is the one thing a composer
    /// must never do. Nothing is minted before the check, so a blank `Enter`
    /// costs no `Uuid` and reads no clock.
    ///
    /// **A cleared draft on `offline: true` is correct, and it needs no second
    /// signal here.** [`MessageList::begin_send`] reports whether a *row* is on
    /// screen, and an offline send puts one there — `PLAN.md` §7 gives the outbox
    /// ownership and the row renders `sending…`. So "the row exists" already
    /// means "the draft has been consumed, by the socket or by the outbox", and
    /// the distinction this layer must not make is the one
    /// [`MessageList::begin_send`]'s own documentation declines to make.
    ///
    /// **A refused send puts the draft back, and the document did not cover
    /// this case.** `begin_send` answers `false` when the list has no channel, no
    /// state is installed, or the state refused (`BlankChannelId`,
    /// `AlreadyPending`, `AlreadyHeld`). In every one of those cases **nothing
    /// consumed the text**, and the same rule that governs a blank `Enter`
    /// governs this: the user's typing outlives a send that did not happen. The
    /// alternative — clearing unconditionally — would make a double-tap of
    /// `Enter` lose a message whenever the first was refused, which is the
    /// silent-drop shape `AGENTS.md` §7.5 forbids.
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.draft.trim().is_empty() {
            return;
        }

        // Both are the caller's to read, per `bridge.rs`: `state/` has no clock
        // (`AGENTS.md` §3.2), and a UI that invented a timestamp would write a
        // second, invisible ordering. The id is a `v4` for the same reason
        // `AGENTS.md` §7.4 requires it — a client-generated identity that exists
        // before the server has seen the message, which is what makes the
        // optimistic row and the ACK the same row.
        let client_msg_id = Uuid::new_v4();
        let at = Utc::now();

        let content = std::mem::take(&mut self.draft);
        let on_screen = self.list.update(cx, |list, cx| {
            list.begin_send(&content, client_msg_id, at, cx)
        });

        if on_screen {
            // The row is up, so the draft was consumed. `begin_send` notifies the
            // list itself; this view's own text changed, so it owes a frame too,
            // or the field would keep drawing the message that was just sent.
            cx.notify();
        } else {
            self.draft = content;
        }
    }

    /// Handles the two keys this field owns, and leaves every other key alone.
    ///
    /// **The two keys are claimed; everything else is deliberately not, and the
    /// default arm's empty body is the load-bearing part.** A character key that
    /// fell into an arm that stopped propagation would be dropped before the
    /// platform ever saw it — the module docs, §2, with the mechanism quoted. An
    /// arm that returned `true` for keys it did not handle would have the same
    /// effect for the same reason. So the correct amount of code here is *none*.
    ///
    /// **Both claimed keys stop propagation, and for `enter` that is not
    /// optional** — see the module docs, §3, where `with_simulated_ime` would
    /// otherwise append a newline to the draft this call just cleared.
    ///
    /// **`escape` hands focus to the shell rather than sending, and the second
    /// half is the load-bearing one.** A text field that sent on `Escape` would
    /// be a field where the most-typed key in a chat client is a footgun, so
    /// `Escape` blurs and nothing else.
    ///
    /// **Moving focus *is* the blur, and it needs no second call to
    /// [`Window::blur`].** `Window::focus` clears pending keystrokes, replaces the
    /// window's focus and asks for a frame (`gpui/src/window.rs:2302`), and
    /// `Window::handle_input` registers the platform handler only while
    /// `focus_handle.is_focused(window)` — so the moment this handle stops being
    /// the focused one, the field's text input is deregistered. That is the whole
    /// of "the field is no longer receiving input".
    ///
    /// **And focus is handed to [`InputBar::fallback_focus`] rather than dropped,
    /// because dropping it is a dead end** — that field's documentation has the
    /// measurement. The consequence is the gesture `AGENTS.md` §5.2 asks for:
    /// `Escape` leaves the field, and the `Escape` after that is the shell's.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            SEND_KEY => {
                cx.stop_propagation();
                self.send(cx);
            }
            BLUR_KEY => {
                cx.stop_propagation();
                self.fallback_focus.focus(window, cx);
            }
            // Propagating is not a default, it is the only behaviour that lets a
            // character reach `replace_text_in_range`. See the module docs, §2.
            _ => {}
        }
    }
}

impl Focusable for InputBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The byte offset of a UTF-16 offset within `text`.
///
/// **`None` when the offset lands inside a surrogate pair, and that is the
/// interesting answer.** An offset the platform computed against a different
/// revision of the text can land anywhere, and returning a byte index in the
/// middle of a `char` would make the caller's `&text[a..b]` **panic** — the one
/// way a `String`-backed field turns a stale platform callback into a crash.
/// `AGENTS.md` §2.1's ban on `unwrap`/`expect` is about *this crate's* choices;
/// this is about not handing a caller the means to panic in theirs.
fn utf16_to_byte(text: &str, utf16_offset: usize) -> Option<usize> {
    let mut seen = 0;
    for (byte, character) in text.char_indices() {
        if seen == utf16_offset {
            return Some(byte);
        }
        seen += character.len_utf16();
    }
    // Reached only by the offset one past the end, which is a legitimate
    // insertion point at the end of the text and is the `None`-returning loop
    // above's one case that is in range rather than out of it.
    (seen == utf16_offset).then_some(text.len())
}

/// The byte range of `text` covering a UTF-16 range.
fn utf16_range_to_bytes(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    Some(utf16_to_byte(text, range.start)?..utf16_to_byte(text, range.end)?)
}

impl EntityInputHandler for InputBar {
    /// The text covering a UTF-16 range, for the platform to read.
    ///
    /// **Implemented, because it is two lines and it is what makes a real IME
    /// candidate window work**: the platform calls this to read what it is about
    /// to replace, and answering `None` here is not a conservative simplification
    /// but a "the platform cannot find out what the user typed". `adjusted_range`
    /// is left alone because there is no composition to adjust.
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = utf16_range_to_bytes(&self.draft, range)?;
        Some(self.draft[bytes].to_owned())
    }

    /// Always `None`: this field has no selection.
    ///
    /// **The caret is wherever the last insertion put it — the end — and
    /// reporting that as a `UTF16Selection` would be reporting a highlight the
    /// user cannot move or dismiss.** `None` is the framework's own "this field
    /// does not model a selection", and `gpui/src/input.rs:420`'s reference view
    /// answers it the same way.
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    /// Always `None`: nothing is ever marked.
    ///
    /// This field does not model an IME composition — see
    /// [`InputBar::replace_and_mark_text_in_range`] — so there is no marked
    /// range to report, and a `None` here is the truth rather than a gap.
    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    /// Nothing to unmark, for the same reason [`InputBar::marked_text_range`]
    /// answers `None`.
    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Inserts or replaces text. **This is the typing path.**
    ///
    /// `dispatch_keystroke` reaches this method with `range: None` for every
    /// character key that propagated, and
    /// [`EntityInputHandler::paste`]'s default reaches it with `None` too, so
    /// **`None` is the overwhelmingly common case and it means "append".** A
    /// `Some(range)` is the platform replacing text it measured itself, which
    /// this field only ever sees from a paste over a selection it does not have.
    ///
    /// **A range that does not resolve is dropped rather than clamped.** Clamping
    /// would silently rewrite the wrong span of the user's message; dropping
    /// leaves the draft exactly as it was, which is the answer a field with no
    /// selection can defend.
    ///
    /// **The [`MAX_MESSAGE_BYTES`] bound is here rather than at the send, and
    /// that placement is the whole point.** Typing and pasting arrive through
    /// this one method, so a bound here is a bound on everything; a bound at the
    /// send would let a paste of a 40 MB file sit in memory until the user
    /// pressed `Enter`, which is `AGENTS.md` §7.1's unbounded growth with a
    /// keystroke between it and the user. An over-long insertion is **truncated
    /// at a character boundary** and the rest dropped, which is
    /// `core/markdown.rs`'s own boundary behaviour for the same reason.
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let room = MAX_MESSAGE_BYTES.saturating_sub(self.draft_bytes());
        if room == 0 {
            return;
        }
        let accepted = truncate_at_boundary(text, room);
        if accepted.is_empty() {
            return;
        }

        match range {
            None => self.draft.push_str(accepted),
            Some(range) => {
                if let Some(bytes) = utf16_range_to_bytes(&self.draft, range) {
                    self.draft.replace_range(bytes, accepted);
                }
            }
        }
        cx.notify();
    }

    /// Inserts text and reports no composition.
    ///
    /// **This field does not model an IME composition, and the consequence is
    /// stated rather than hidden:** because [`InputBar::marked_text_range`] is
    /// `None`, an IME that composes several keystrokes into one character has
    /// each intermediate update applied as an ordinary insertion, so a composing
    /// IME will appear to insert each intermediate state. Modelling composition
    /// means holding the marked range and rewriting it on every update, which is
    /// the editor machinery this work unit rejected as a dependency. On a
    /// keyboard with no IME the method is never called at all.
    ///
    /// The `new_selected_range` is ignored for the same reason
    /// [`InputBar::selected_text_range`] returns `None`.
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, new_text, window, cx);
    }

    /// Always `None`: this field does not know where its text is on screen.
    ///
    /// The field draws its draft as a plain element child, so it has no
    /// measurement of any character to answer with. **A rectangle for the
    /// candidate window is worse than none**: `None` lets the platform place its
    /// popup by its own rules, and a rectangle would put it somewhere this view
    /// never claimed.
    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    /// Always `None`: clicking in the field does not move a caret.
    ///
    /// There is no caret to move — see [`InputBar::selected_text_range`] — and
    /// the task's Out list keeps click-to-position out. `None` is the framework's
    /// "no opinion", so a click changes nothing rather than moving an insertion
    /// point the user cannot see.
    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    /// The draft's length in UTF-16 units, which is the platform's unit.
    ///
    /// **Counted rather than approximated, and counted in the right unit.** The
    /// platform measures text in UTF-16 code units, so a non-BMP character — an
    /// emoji, say — counts as two; reporting `self.draft.len()`, which is UTF-8
    /// bytes, would make every such draft look longer to the platform than it is.
    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.draft.encode_utf16().count())
    }

    /// Advertises a send key on a software keyboard.
    ///
    /// **`TextInputAction::Send` because this field's Enter sends, and the
    /// platform's own documentation is what makes this a presentation choice
    /// rather than a behaviour**: it says the variant *"affects only how the key
    /// is presented (icon or label); pressing it is still delivered as ordinary
    /// input"*. So nothing here is load-bearing — the key is routed to
    /// `InputBar::on_key_down` either way — and the reason to set it is that
    /// the alternative leaves a touch user with a key labelled for inserting a
    /// line break in a field that cannot hold one.
    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        TextInputConfiguration {
            input_action: TextInputAction::Send,
            ..Default::default()
        }
    }
}

/// The longest prefix of `text` that fits in `room` bytes without splitting a
/// character.
///
/// `core/markdown.rs` does this to cap a parsed message, and the two must agree:
/// a draft cut mid-character would not compile into a `String` at all, so this
/// is not a nicety but the difference between a bounded field and a panic on the
/// frame path.
fn truncate_at_boundary(text: &str, room: usize) -> &str {
    if text.len() <= room {
        return text;
    }
    let mut end = room;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

impl Render for InputBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Two clones per frame, and both are what the paint-time registration
        // needs: the focus handle to key `handle_input` on, and the entity to
        // hand the platform as the handler's target. `Context::entity` is an
        // `Arc` clone, so this is not a deep copy of anything.
        let focus_handle = self.focus_handle.clone();
        let view = cx.entity();

        // The placeholder is a second text element rather than a style of the
        // first, because `AGENTS.md` §7.3 says GPUI does not inherit a text
        // colour: two elements, two explicit colours, and no way for the frame
        // to draw the placeholder in the wrong one.
        let (label, label_color) = if self.draft.is_empty() {
            (PLACEHOLDER.to_owned(), self.colors.text_muted)
        } else {
            (self.draft.clone(), self.colors.text)
        };

        div()
            .id("input-bar")
            .key_context("InputBar")
            // Records this container's bounds for the headless test, for the same
            // reason the shell and the list record theirs.
            .debug_selector(|| "input-bar".to_owned())
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .py_2()
            .bg(self.colors.surface)
            // Explicit, per AGENTS.md 7.3, on the container as well as on the text.
            .text_color(self.colors.text)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                // `size_full` so the canvas reports the field's own bounds: those
                // are what the platform is told the text area is, and a
                // zero-height canvas would report a zero-height field.
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus_handle,
                            ElementInputHandler::new(bounds, view),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .flex_grow_1()
                    .text_sm()
                    // Explicit, per AGENTS.md 7.3.
                    .text_color(label_color)
                    .child(label),
            )
    }
}
