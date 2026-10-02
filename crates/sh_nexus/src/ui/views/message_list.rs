//! The virtualized message list: `gpui::List`, a row cache, and the state seam.
//!
//! `docs/ARCHITECTURE.md` ADR-006's steps 2, 3 and 5 are this file, and the
//! framework shapes all three.
//!
//! # Step 2 — the list, and why this is `List` rather than `UniformList`
//!
//! ADR-006 is Accepted and its premise is measured rather than assumed: at the
//! pinned `rev e683fd7` GPUI ships `elements/list.rs` for *"a large number of
//! differently sized elements"*, and `uniform_list.rs` points readers at it. A
//! chat message's height is a function of its content — `AGENTS.md` §10 and §4.2
//! require Markdown and code blocks — so a uniform estimator is not merely slow
//! here, it is wrong: a tall first row misplaces every row after it. **No
//! `UniformList` appears anywhere in this file, and that is a decision the ADR
//! made rather than an omission.**
//!
//! `ListState` is held on this view because that is what `gpui::List` requires:
//! the framework keeps its layout state intrusively *"so that your code can
//! coordinate directly with the list element's cached state"*. That is also why
//! there is no height cache here — the ADR is explicit that `List` holds the
//! layout state itself, and a second cache would be a second answer to the same
//! question.
//!
//! # Step 3 — recycling
//!
//! One `Entity<MessageRow>` per `client_msg_id`, in a bounded cache. See
//! [`super::message_row`] for why.
//!
//! # Step 5 — the state seam, and the one obligation it centralises
//!
//! Every read of the application state goes through `bridge::try_read`, every
//! channel switch through `bridge::try_select_channel`, and every send through
//! `bridge::try_begin_send` or `bridge::try_retry_send`. `AGENTS.md` §3.2 gives
//! this layer no access to the state's mutators and `tests/bridge.rs` fails the
//! build on a view that names the state's type, so the seam is the only door and
//! the named doors in `bridge.rs` are the audit trail.
//!
//! **A row's control dispatches *here*, and that is what keeps
//! [`super::message_row`] free of a `Context`.** The badge is built inside
//! `MessageRow::render`, so a listener written there can only reach the row; the
//! row therefore holds a `WeakEntity<MessageList>` — a handle to the view that
//! owns it, never a handle to the application state — and calls
//! [`MessageList::retry_failed_send`]. So every gesture a row can offer arrives
//! here, reads the state back the same way [`MessageList::begin_send`] does, and
//! asks for its own frame from here.
//!
//! **And every height-changing action routes its `remeasure_items` or `splice`
//! from here**, which is the part of ADR-006's plan that is easy to get subtly
//! wrong. `gpui::List` requires the caller to report a row whose height changed,
//! and the tempting place to do that is inside the render closure where the
//! change is detected. **That place is a panic rather than a nicety:**
//! `List::layout_items` is called with `&mut StateInner`
//! (`gpui/src/elements/list.rs:1027`), so a `remeasure_items` from inside
//! `render_item` would take a second `&mut` borrow of the same `RefCell`. So a
//! change found while rendering is *recorded* ([`MessageList::pending_remeasure`])
//! and *applied* by [`MessageList::flush_remeasures`], which runs at the top of
//! [`MessageList::render`] — before the list lays out for that frame, and
//! outside every borrow of its state.
//!
//! # Step 6 — the keyboard, and why the cursor lives on the list
//!
//! Work unit 3D shipped a retry affordance that was **pointer-only** and recorded
//! that as a defect; `AGENTS.md` §5.2 requires keyboard-only operation. This
//! section is the other half of that fix, and its placement in the module docs is
//! the point: the keyboard is not a feature of the list element, it is a fourth
//! thing the list owns alongside its `ListState`, its row cache and its seam.
//!
//! **The design rests on four properties of the pinned `gpui`, each read rather
//! than assumed**, and they are the reason the cursor is one `client_msg_id` on
//! this view rather than a `FocusHandle` per row:
//!
//! | Property | Where |
//! |---|---|
//! | a key's dispatch path runs **from the focused node upwards**, so a view that holds the focus hears its own `on_key_down` first | `window.rs`, `dispatch_key_event` |
//! | `tab_stops` is **rebuilt every painted frame**, so an unpainted element has no tab stop | `window.rs` (`tab_stops.clear()`), `div.rs` (insert during paint) |
//! | `focus_next` / `focus_prev` read the **already-rendered** frame | `window.rs`, `focus_next` |
//! | the dispatch walk **returns the instant a handler stops propagation** | `window.rs`, `dispatch_key_down_up_event` |
//!
//! **A row cannot host the handle, and the second row of that table is why.** A
//! handle's tab stop exists only on frames where its element is painted, so a row
//! scrolled out of the overdraw would take a window's focus with it; and
//! [`RowCache`] evicts at [`MAX_RETAINED_ROWS`], which would leave a window
//! focused on a released handle — a dangling focus, not a lost keystroke. **This
//! view is never evicted and is painted on every frame the window exists**, so a
//! `client_msg_id` here has neither failure mode. See [`MessageList::focus_handle`]
//! for how the keyboard reaches this handle, and for the one thing this unit does
//! *not* do.
//!
//! **Four keys, and the count is the listbox convention rather than a shortfall.**
//! `up`/`down` move the cursor and `enter`/`space` act on it. Every other key
//! propagates, and [`MessageList::on_key_down`] says why that asymmetry — a
//! propagating `enter` synthesises a `"\n"` — is the whole of `AGENTS.md` §5.2's
//! focus-order rule rather than a detail.
//!
//! **`escape` is deliberately among the propagating keys, and that is what makes
//! the chain terminate.** The log is *entered* by the shell handing focus here, so
//! an `escape` arriving after that has already done its job; claiming it here would
//! either swallow a key 3D established this handler must never swallow, or start a
//! cycle back up the ladder. Letting it propagate reaches
//! [`crate::app::Shell::on_key_down`], which re-asserts this handle — a no-op,
//! because [`Window::focus`] returns early when the handle already holds focus —
//! and re-snaps the log to the newest message.
//!
//! # What this file deliberately does not do
//!
//! No delegate trait (ADR-006: *"a trait over a single call site is indirection
//! with no second implementation to justify it"*), no `ListMeasuringBehavior::Measure`
//! opt-in until a measurement says the default is insufficient, and no search,
//! thread panel or editing UI — all Phase 2 items this ADR does not govern. The
//! `set_scroll_handler` hook Phase 3's paged history needs is not wired either,
//! because nothing calls it yet and a registered handler nobody triggers is
//! indistinguishable from no handler at all.
//!
//! # The `Escape` ladder, and the `Tab` gap it does not close
//!
//! **The log is reachable by key in the shipped client, and the route is `Escape`
//! twice.** `InputBar`'s `Escape` hands focus to the shell's root handle;
//! `Shell::on_key_down`'s hands it to [`MessageList::focus_handle`]. That is the
//! whole chain, it is tested end to end in `tests/app_shell.rs`, and it is **not
//! `Tab`** — a keyboard purist expects `Tab` to move between panes and
//! `Shift+Tab` to move back, and this client has neither.
//!
//! **The cost is a real gap rather than a stylistic note, and it is stated here
//! because the ladder is one-way.** Nothing in the crate binds a key to the
//! composer's handle *from here*, so a keyboard user who presses `Escape` twice
//! can reach the log, move the cursor and retry a send, and **cannot get back to
//! the field with a key**. That is not a regression — before the ladder the log
//! had no key route at all, and one `Escape` already stranded the keyboard on the
//! shell root — and it is not what a `Tab` binding would leave either, since
//! `Tab` reaches the log by traversal and returns by traversal. **`AGENTS.md` §5.2
//! is therefore not satisfied in full by what is here**, and no part of this file
//! says it is.
//!
//! | Missing | Why it is not here |
//! |---|---|
//! | a `Tab` → [`Window::focus_next`] binding | `focus_next` has **no caller anywhere in `gpui` at this rev** outside its own tests, and this crate has no keymap. Adding a traversal binding is a gesture and a product decision, not a detail of this unit. The decision not to reverse is 3B's: `app::open` keeps focus on the composer, because a client that opens on the log is a client that cannot be typed into. **The `Escape` ladder is this unit's own answer to reachability, and `tab_index(0)` is deliberately absent on the handle** — with no traversal, it would declare an intent nothing implements. |
//! | a key that returns from the log to the composer | It would make `Escape` a three-way toggle, so pressing it repeatedly walks the three views forever — the focus loop this change exists to avoid — and it would need a second owner of `escape`. The missing piece is the `Tab` binding above, not a fourth rung. |
//! | multi-select, `Shift`+arrows, `Home`/`End` | `AGENTS.md` §5.2 asks for a focus *order*, and a single cursor is what it describes. The rest is scope this unit does not have to invent, and a second cursor is a second thing every assertion above would have to be restated for. |

use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::{
    div, list, prelude::*, px, AnyElement, App, Context, Empty, Entity, FocusHandle, Focusable,
    FollowMode, IntoElement, KeyDownEvent, ListAlignment, ListSizingBehavior, ListState, Render,
    Window,
};
use uuid::Uuid;

use crate::core::markdown::{parse as parse_markdown, Document};
use crate::state::bridge::{self, Rendered};
use crate::state::DeliveryState;
use crate::ui::views::message_row::{MessageRow, RowCache, RowSpec};
use crate::ui::Colors;

/// How much extra space above and below the viewport is rendered and measured.
///
/// `List`'s `overdraw` parameter, documented as *"how much extra space is
/// rendered above and below the visible area … can help ensure that the list
/// doesn't flicker or pop in when scrolling"*. One screenful is the smallest
/// value that removes the pop without measuring rows the user is nowhere near,
/// and it is in pixels because that is what the list's own API takes — a row
/// count would assume the height this whole design refuses to assume.
const OVERDRAW_PX: f32 = 512.0;

/// How far `up` moves the keyboard cursor, in rows. Negative because
/// [`MessageList::move_selection`] takes a signed step.
const STEP_BACK: isize = -1;

/// How far `down` moves the keyboard cursor, in rows.
const STEP_FORWARD: isize = 1;

/// The key [`MessageList::on_key_down`] moves the cursor towards older messages
/// on, as `gpui`'s keystroke parser spells it.
const MOVE_BACK_KEY: &str = "up";

/// The key [`MessageList::on_key_down`] moves the cursor towards newer messages
/// on. See [`MOVE_BACK_KEY`].
const MOVE_FORWARD_KEY: &str = "down";

/// The keys [`MessageList::on_key_down`] activates the selected row with.
///
/// **A pair, and the listbox convention is why both are here.** `enter` is what
/// a keyboard user presses for "act on this", and `space` is what the same
/// convention offers to anyone using an on-screen keyboard, where `enter` is
/// frequently absent or taken. `gpui`'s keystroke parser spells them exactly so
/// (`gpui/src/platform/keystroke.rs:249`), which is what makes the pair
/// writable as an `|` arm rather than two arms that could drift apart.
const ACTIVATE_KEYS: [&str; 2] = ["enter", "space"];

/// The chat log, virtualized.
pub struct MessageList {
    /// The list's own state. Held here because `gpui::List` requires it, and
    /// never rebuilt: `reset` and `splice` are how its count changes.
    list: ListState,
    /// One recycled row per message shown.
    rows: RowCache,
    /// The channel this view is showing, or `None` before anything is shown.
    channel: Option<String>,
    /// The palette every row draws with.
    colors: Colors,
    /// The item count last reconciled with the state, so a change can be routed
    /// as an insertion, a removal, or nothing at all.
    count: usize,
    /// The `client_msg_id` the shown channel's head row had when this list last
    /// reconciled, so a **head** eviction can be told from a tail change.
    ///
    /// `None` whenever there is no anchor worth protecting: nothing shown, no
    /// state, or a first frame. See [`MessageList::sync`], which is the method
    /// that reads this field and the reason it exists — `AGENTS.md` §7.1's
    /// history bound evicts the oldest row, and a list that ignored where the
    /// removal happened would move the content under a reader scrolled into
    /// history.
    head: Option<Uuid>,
    /// The `client_msg_id` that was under the scroll position when this list last
    /// reconciled — **the reader's anchor, remembered rather than re-read.**
    ///
    /// See [`MessageList::reanchor`] for why the *previous* frame's identity is
    /// the one that has to be kept: the state has already been applied by the
    /// time a frame runs, so the row now at the scroll top is the row that slid
    /// into it.
    anchor: Option<Uuid>,
    /// Rows whose content changed while rendering, waiting to be remeasured.
    ///
    /// See the module docs: a `remeasure_items` from inside the render closure
    /// would be a second borrow of the list's `RefCell`. This is drained by
    /// [`MessageList::flush_remeasures`] before the next layout.
    pending_remeasure: Vec<usize>,
    /// The `client_msg_id` of the row the keyboard cursor is on, or `None`.
    ///
    /// # An identity, not an index, and the eviction is why
    ///
    /// `AGENTS.md` §7.1's history bound evicts the channel's **oldest** row at
    /// the cap (work unit 3C), which shifts every index below it down by one. An
    /// index would therefore point at a *different message* one arrival later —
    /// a cursor that silently walks away from the row the user chose, and an
    /// `enter` that retries somebody else's send. A `client_msg_id` survives
    /// that, and it is already the key [`RowCache`] and [`RowSpec`] use, so this
    /// is the third place that identity appears rather than a new notion of it.
    ///
    /// **The index is derived on every keystroke and never stored.** A `down` is
    /// one O(1) `position_of` probe plus one slice index, which is not
    /// `AGENTS.md` §2.3's O(n) in the frame loop; holding an index as well as
    /// an id would buy nothing and give two fields that can disagree.
    ///
    /// **`None` is a real state, not an initialisation artefact**: it is what the
    /// list shows when nothing is selected, and it is what
    /// [`MessageList::activate_selection`] refuses on. See
    /// [`MessageList::drop_stale_selection`] for the two events that clear it.
    selected: Option<Uuid>,
    /// This list's own focus handle — **the keyboard's whole way in.**
    ///
    /// # One handle, on this view, and not one per row
    ///
    /// **A row cannot host a focus handle, and work unit 3D measured why.** Three
    /// independent facts, each read in the pinned `gpui`:
    ///
    /// - The dispatch path for a key runs *from the focused node upwards*
    ///   (`window.rs` builds it with `dispatch_path(focus_node_id_in_rendered_frame(window.focus))`),
    ///   so a view that holds the focus receives its own `on_key_down` as the
    ///   first node of its own path. One handle on the ancestor covers every row.
    /// - The tab-stop registry is rebuilt **every painted frame**: `window.rs`
    ///   clears `tab_stops` before each frame and `div.rs` inserts into it during
    ///   paint. A handle's tab stop exists only on frames where its element is
    ///   painted, so a row scrolled out of the overdraw loses it — and takes a
    ///   window's focus with it, because `focus_next` reads the *rendered* frame.
    /// - `RowCache::evict` drops rows at [`MAX_RETAINED_ROWS`], and a window left
    ///   focused on a released row's handle is a dangling focus rather than a
    ///   lost keystroke.
    ///
    /// A `client_msg_id` on the list has none of those failure modes: this view
    /// is never evicted, and it is painted on every frame the window exists.
    ///
    /// # How the keyboard gets here, and what that is not
    ///
    /// **The log is reachable by key, and the route is `Escape` twice — not
    /// `Tab`.** `Shell::on_key_down` is the middle rung: the composer's `Escape`
    /// hands focus to the shell's own root handle, and the shell's `Escape` hands
    /// it to this one. That is a real keyboard route, it is asserted end to end in
    /// `tests/app_shell.rs` (`escape_chains_from_the_composer_to_the_log_and_its_retry`),
    /// and it is **not** what a keyboard purist expects.
    ///
    /// **What is still absent is `Tab`.** `TabStopMap` marks a handle reachable by
    /// traversal only when something binds `Tab` to [`Window::focus_next`], and
    /// **nothing in this crate or in `gpui` at the pinned rev does** — `focus_next`
    /// has no caller outside `gpui`'s own tests, and this crate has no keymap. So
    /// the log is reachable by an explicit gesture and **not** by traversal, which
    /// is a real gap: a keyboard user cannot come *back* from the log, because
    /// nothing binds a key to the composer's handle either. That gap is
    /// pre-existing — before the `Escape` ladder the log had no key route at all —
    /// and closing it is a `Tab` binding, which is a gesture and a product
    /// decision this work unit records rather than picks (ADR-006, 3E).
    ///
    /// **`tab_index` is deliberately not set on this handle.** It would be inert:
    /// nothing calls [`Window::focus_next`], and the tab-stop registry is rebuilt
    /// every painted frame, so a `tab_index` on a handle no traversal reads is a
    /// declaration of an intent nothing implements — worse than the absence it
    /// would look like it fills.
    ///
    /// **The alternative was not "move focus on open".** `app::open` focuses the
    /// composer, and a client that opens on a log is a client that cannot be typed
    /// into; that decision is not this work unit's to reverse.
    focus_handle: FocusHandle,
}

/// The parts of a message the row needs, read in one pass.
///
/// A named struct rather than a tuple because it is seven fields and a tuple of
/// seven is a positional bug waiting to happen. Private: it is a local transfer
/// type between the state read and [`RowSpec`], not API.
struct RowMeta {
    /// The author's id.
    author: String,
    /// Whether the author is this client.
    is_self: bool,
    /// The server's acceptance time.
    timestamp: DateTime<Utc>,
    /// This client's delivery state, if it sent the message.
    delivery: Option<DeliveryState>,
    /// The server's explanation for a failure.
    failure_detail: Option<String>,
    /// Reactions as `(emoji, count)`.
    reactions: Vec<(String, usize)>,
}

impl MessageList {
    /// Builds an empty list, tail-following, showing no channel.
    ///
    /// `FollowMode::Tail` is set here rather than by a later call because it is
    /// the only correct starting state for a chat log: ADR-006's table names
    /// *"stick to the newest message"* as a first-class feature of `List`, and a
    /// client that opens scrolled to the middle of history is a bug the user has
    /// to fix by hand.
    ///
    /// The `cx` parameter is used for exactly one thing: handing the row cache a
    /// weak handle to this view, so a control a row draws has somewhere to
    /// dispatch to ([`super::message_row::MessageRow::owner`]). That is also why
    /// it is taken for symmetry with GPUI's view constructors rather than left off
    /// — a constructor with a different arity from every other view in the crate
    /// is more to explain than a used parameter.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Bottom, px(OVERDRAW_PX));
        list.set_follow_mode(FollowMode::Tail);

        Self {
            list,
            rows: RowCache::new(cx.weak_entity()),
            channel: None,
            colors: Colors::default(),
            count: 0,
            head: None,
            anchor: None,
            pending_remeasure: Vec::new(),
            selected: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The palette every row draws with.
    pub fn colors(&self) -> Colors {
        self.colors
    }

    /// Replaces the palette every row draws with.
    ///
    /// **The theme provider's only door into this view, and it exists because
    /// without one `AGENTS.md` §3.1's obligation is unfulfillable.** `colors` was
    /// hard-coded at construction and only readable, so a shell that resolved a
    /// theme had no way to hand it over — a theme provider that cannot reach the
    /// renderer is a provider that renders nothing.
    ///
    /// **This notifies, and that is the whole difference from [`MessageList::sync`].**
    /// `sync` runs at the top of every render and must not schedule a frame, or
    /// the view would mark itself dirty during its own frame and ask for another
    /// one forever. A theme change is the opposite: it is a caller-driven change
    /// to something the view draws, made between frames, and the caller is
    /// entitled to exactly one repaint for it. Two calls, two contracts, and the
    /// reason to say so is that merging them into a single "reconcile and repaint"
    /// method is the obvious refactor and it would reintroduce the redraw loop.
    ///
    /// **Every row is told, not only the container.** A row caches its own
    /// `Colors` (see [`super::message_row`]), so a palette that stopped at this
    /// view would repaint the list's background and leave every recycled row
    /// drawing the theme it was built under — which `AGENTS.md` §7.3's rule is
    /// about, and which no structural assertion would notice: a bubble in the old
    /// background is still a bubble.
    ///
    /// No remeasure is requested here, and `MessageList::render_row` will record
    /// the rows that changed on the next pass: a row whose palette changed is
    /// reported as changed by the cache, which routes to `remeasure_items` through
    /// the deferred path the module docs describe. Doing it eagerly from this
    /// method would be the `remeasure` from inside a layout that §"What this file
    /// deliberately does not do" is written against.
    pub fn set_colors(&mut self, colors: Colors, cx: &mut Context<Self>) {
        self.colors = colors;
        cx.notify();
    }

    /// The list's own state, for a caller that needs to scroll it.
    ///
    /// Handed out because scrolling to a message (`scroll_to_reveal_item`) and
    /// asking for the viewport (`is_scrolled_to_end`) are the caller's gestures,
    /// not this view's — ADR-006's table lists both as things `List` provides to
    /// *"the caller"*.
    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    /// How many messages this list is currently showing.
    ///
    /// Read from the list state rather than from [`MessageList::count`], so a
    /// caller sees what the list has rather than what this view last told it.
    pub fn item_count(&self) -> usize {
        self.list.item_count()
    }

    /// The channel being shown, if any.
    pub fn channel(&self) -> Option<&str> {
        self.channel.as_deref()
    }

    /// How many rows the cache is holding.
    pub fn retained_rows(&self) -> usize {
        self.rows.len()
    }

    /// The live row for a message, if the cache still holds it.
    ///
    /// The recycled-state question — *is this the same row as before?* — is only
    /// answerable by comparing `EntityId`s, so a test needs this to answer it.
    pub fn retained_row(&self, client_msg_id: &Uuid) -> Option<Entity<MessageRow>> {
        self.rows.get(client_msg_id)
    }

    /// The `client_msg_id` the keyboard cursor is on, or `None`.
    ///
    /// **Exposed as an identity rather than an index, and the test asks for the
    /// identity for the reason [`MessageList::selected`] documents:** an index
    /// would be a number this view happens to be holding, and a test asserting on
    /// one could not tell "the cursor is on the row I chose" from "the cursor is
    /// on whichever row now sits at that number" — which is precisely the defect
    /// a `client_msg_id` was chosen to prevent.
    pub fn selected(&self) -> Option<Uuid> {
        self.selected
    }

    /// Shows a channel: the selection gesture, then a recount.
    ///
    /// **Two things happen here and they are deliberately one call.** The
    /// gesture is `bridge::try_select_channel`, which is what clears the unread
    /// badge (`AGENTS.md` §8.1's Channel Switch Flow). The recount is
    /// `ListState::reset`, ADR-006's table row for a channel switch: the items
    /// are different items, so nothing about their measurements carries over.
    ///
    /// **The state's outcome is not interpreted here, and that is a boundary
    /// rather than an omission.** An unread badge belongs to the channel rail,
    /// which is a different view in a different work unit; a message list that
    /// started reporting refusals would be the second place in the crate that
    /// decides what a refusal means. The state records what it accepted, and
    /// `state.selected()` is where a caller reads it.
    ///
    /// **The view shows the channel it was told to show, whether or not the
    /// state has it in its channel list.** Those are different facts: the rail's
    /// list is what the user can pick from, and the messages are what this view
    /// renders. Asking the state which channel is selected instead would make a
    /// channel that arrived without a channel record — which is exactly what a
    /// test fixture and a `message.new` before the channel list does — impossible
    /// to display at all.
    ///
    /// Returns the item count, which is what the caller would ask for next.
    pub fn show_channel(&mut self, channel_id: impl Into<String>, cx: &mut Context<Self>) -> usize {
        let channel_id = channel_id.into();
        let _ = bridge::try_select_channel(cx, &channel_id);

        self.count = self.messages_in(&channel_id, cx);
        self.channel = Some(channel_id);
        // Rows for another channel are retained rather than dropped: the cache
        // is keyed by `client_msg_id`, which is global, so a row that comes back
        // is the same row (`docs/ARCHITECTURE.md` ADR-006, step 3).
        self.pending_remeasure.clear();
        // The head and the anchor are a different channel's now, so a comparison
        // against either would be evidence of nothing. `reset` puts the list at
        // the end sentinel and `List` re-anchors there itself; there is no
        // position to carry over.
        self.head = None;
        self.anchor = None;
        // **The cursor is cleared here and not carried across, and that is the
        // decision rather than an omission.** A `client_msg_id` is global, so the
        // row the cursor was on survives the switch as a *retained row* — which
        // means carrying the cursor forward would leave it pointing at a message
        // the list is not showing, and an `enter` would put a send from another
        // channel back in flight. `tests/ui_message_list.rs`
        // (`the_cursor_is_cleared_when_the_channel_changes`) asserts it, and
        // `the_cursor_survives_the_eviction_of_other_rows` asserts the half that
        // must *not* clear, because a rule that clears too eagerly is as wrong as
        // one that clears too late.
        self.selected = None;
        self.list.reset(self.count);
        // A gesture that changes what is on screen has to ask for a frame.
        // `reset` changes the list's state, not GPUI's: nothing else in this
        // call would schedule a redraw, so a switch that forgot this would leave
        // the previous channel on screen until something unrelated repainted.
        cx.notify();
        self.count
    }

    /// Reconciles the list with the state and reports the new item count.
    ///
    /// Called at the top of every [`MessageList::render`], so the list cannot
    /// drift from the state no matter who forgot to call it, and callable
    /// directly by an event loop that wants the reconciliation to happen before
    /// a frame is even scheduled.
    ///
    /// **An insertion is a `splice` and a channel switch is a `reset`, which is
    /// exactly the division ADR-006's table draws.** `splice` preserves the
    /// scroll position and is what a new message is; `reset` discards the
    /// measurements and is what *different messages* are.
    ///
    /// # The scroll anchor, which is not the count
    ///
    /// [`MAX_MESSAGES_PER_CHANNEL`](crate::state::MAX_MESSAGES_PER_CHANNEL) makes
    /// a full channel lose its **oldest** row, and every index below the head
    /// shifts down by one. Reconciling the *count* does nothing about that: at
    /// the cap, an arrival is one insertion and one eviction, so `count` is
    /// unchanged, the `Ordering::Equal` arm below runs, and a reader scrolled into
    /// history would watch the content move up one row for every message that
    /// arrives. That is the defect this section exists to prevent.
    ///
    /// **The anchor is an identity, and it is restored by identity.** Each frame
    /// the list reads the `client_msg_id` of the row at its own scroll top and
    /// remembers the head's; when the head's identity has changed — which is the
    /// eviction signal, and is a property of the *state* rather than of anything
    /// the reader did — it asks the state where the anchored row now sits and
    /// moves the scroll position to match. So the row the reader was looking at
    /// stays under the reader, and by how much it moved is the state's answer
    /// rather than a number inferred from a count.
    ///
    /// **The whole thing is skipped while the reader is following the tail.** The
    /// list is pinned to the newest message, there is no row to hold still, and
    /// `List` re-anchors itself at the end sentinel
    /// (`gpui/src/elements/list.rs:1211-1218`) — `scroll_to` would call
    /// `stop_following` and fight it. The head is still *recorded* on those
    /// frames, so a reader who scrolls up and is then evicted is re-anchored on
    /// the very next arrival rather than the one after.
    ///
    /// **The two costs are stated rather than assumed.** Each of the two reads is
    /// a `try_read` of a `Uuid` and an `Option<usize>`, so both are O(1) hash
    /// probes and neither is `AGENTS.md` §2.3's O(n) in the frame loop. And the
    /// *heights* of off-screen rows are not corrected: the rows shifted down by
    /// one, so a cached measurement now describes the message above it, and the
    /// scrollbar is fractionally wrong until those rows are scrolled into view
    /// and remeasured. The visible range is not affected —
    /// [`MessageList::render_row`] sees a different spec for every shifted index,
    /// records it, and [`MessageList::flush_remeasures`] remeasures that range in
    /// the same frame.
    ///
    /// **This never calls `cx.notify()`.** It runs at the top of every render,
    /// so notifying here would mark the view dirty during its own frame and ask
    /// for another one, forever — a redraw loop that also means
    /// `run_until_parked` never parks. Scheduling is the caller's, `bridge.rs`
    /// §6's decision about `drain` applied to the frame that follows it.
    ///
    /// **The keyboard cursor is reconciled here too, and here is the only place
    /// it can be.** A row leaves this list two ways — the channel changes
    /// ([`MessageList::show_channel`], which clears the cursor in the same breath)
    /// and `MAX_MESSAGES_PER_CHANNEL` evicts the oldest row — and eviction is
    /// applied by the *state*, on a drain, with no callback to a view. The list
    /// therefore learns about it the same way it learns about every other change
    /// to the state: by reading the state. See
    /// [`MessageList::drop_stale_selection`].
    pub fn sync(&mut self, cx: &mut Context<Self>) -> usize {
        self.flush_remeasures();

        let channel = self.channel.clone();
        let count = match channel.clone() {
            Some(channel) => self.messages_in(&channel, cx),
            None => 0,
        };

        match count.cmp(&self.count) {
            Ordering::Equal => {}
            Ordering::Greater => self.list.splice(self.count..self.count, count - self.count),
            Ordering::Less => self.list.splice(count..self.count, 0),
        }
        self.count = count;
        self.reanchor(&channel, cx);
        self.drop_stale_selection(channel.as_deref(), cx);
        count
    }

    /// Keeps the row the reader is looking at under the reader across a head
    /// eviction. See [`MessageList::sync`], which is where this is explained.
    ///
    /// **The anchor has to be remembered from the *previous* frame, because by
    /// the time this runs the eviction has already happened.** The state is
    /// applied on the main thread by `bridge::drain` before any frame is drawn,
    /// so reading "which row is at the scroll top" now returns the row that
    /// *slid into* that position — the very shift being corrected. The list
    /// therefore notes the identity under its scroll top on every frame and, on
    /// the frame the head changes, asks the state where last frame's identity
    /// ended up. Both halves are needed: the head identity is the *signal* that
    /// indices moved, and the remembered anchor is the *thing* to move.
    ///
    /// **One `try_read` when there is nothing to do and two when there is.** The
    /// head and the current anchor are two fields of one answer, so they are one
    /// lease of the global; the second lease is the position lookup, and it is
    /// only taken on a frame that actually evicted.
    fn reanchor(&mut self, channel: &Option<String>, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();

        // `(None, None)` is the "nothing shown, or no state" case and also what a
        // `show_channel` on an empty channel leaves behind. Neither is evidence
        // of an eviction, and both are recorded, so the next frame has something
        // real to compare against.
        let (head, at_top) = match channel.as_deref() {
            Some(channel) => bridge::try_read(cx, |state| {
                let held = state.messages(channel);
                (
                    held.first().map(|message| message.client_msg_id),
                    held.get(top.item_ix).map(|message| message.client_msg_id),
                )
            })
            .unwrap_or((None, None)),
            None => (None, None),
        };

        let evicted_head = head.is_some_and(|head| self.head.is_some_and(|last| last != head));
        if !self.list.is_following_tail() && evicted_head {
            // The remembered anchor, not `at_top`: if the row the reader was on is
            // itself the row that was evicted there is nothing to preserve, and
            // `position_of` answering `None` is the honest "leave them where they
            // are" — they are at the head of what is left.
            if let (Some(channel), Some(anchored)) = (channel.as_deref(), self.anchor) {
                let moved =
                    bridge::try_read(cx, |state| state.position_of(channel, &anchored)).flatten();
                if let Some(at) = moved.filter(|at| *at != top.item_ix) {
                    self.list.scroll_to(gpui::ListOffset {
                        item_ix: at,
                        offset_in_item: top.offset_in_item,
                    });
                }
            }
        }

        self.head = head;
        self.anchor = at_top;
    }

    /// Clears the keyboard cursor if the row it names is no longer in the list.
    ///
    /// # Why "no longer in the list" is the question, and not "did the count move"
    ///
    /// `MAX_MESSAGES_PER_CHANNEL` evicts the **head**, so a channel that is over
    /// the cap evicts on *every* arrival and its message count does not change at
    /// all. A rule keyed on the count would therefore clear the cursor constantly
    /// and a rule that never cleared it would let `enter` retry a send for a
    /// message the state has thrown away. **The rule is the only one that is
    /// right: the cursor names a row, and the cursor lives exactly as long as that
    /// row is held.**
    ///
    /// **Eviction of *other* rows leaves it alone, and that is the half worth
    /// stating.** A head eviction shifts every index below it down by one, so the
    /// cursor's index is stale the moment it happens — which is exactly why
    /// [`MessageList::selected`] holds a `client_msg_id`. `position_of` re-derives
    /// the index on the next keystroke and the user has not moved.
    ///
    /// **One `try_read` and one O(1) hash probe on the frames that have a cursor,
    /// and none at all on the frames that do not.** The early return on
    /// `self.selected == None` is the whole of that: with no cursor there is
    /// nothing to check and this is not on the frame path.
    ///
    /// **No `cx.notify()`, for the same reason [`MessageList::sync`] does not
    /// notify.** Whatever evicted the row already asked for a frame — an arrival
    /// goes through the drain, and the drain's caller schedules. The frame now
    /// running will draw the cursor's absence.
    fn drop_stale_selection(&mut self, channel: Option<&str>, cx: &mut Context<Self>) {
        let Some(selected) = self.selected else {
            return;
        };

        // No channel, or no state, means there is nothing the cursor could be
        // naming — and `AGENTS.md` 2.1 forbids the `unwrap` that would
        // distinguish "no state" from "no such row" by crashing.
        let held = match channel {
            Some(channel) => {
                bridge::try_read(cx, |state| state.position_of(channel, &selected).is_some())
                    .unwrap_or(false)
            }
            None => false,
        };

        if !held {
            self.selected = None;
        }
    }

    /// Stops the list snapping to the newest message.
    ///
    /// `pause_following_tail` rather than `FollowMode::Normal`, and the
    /// difference is the reason the framework has two: this keeps the list in
    /// `Tail` mode with following suspended, so it *resumes* on its own once the
    /// view returns to the bottom. `Normal` would be a one-way door — the user
    /// would have to re-enable it from a setting rather than by scrolling down.
    pub fn pause_following_tail(&mut self, cx: &mut Context<Self>) {
        self.list.pause_following_tail();
        cx.notify();
    }

    /// Re-engages tail-following and jumps to the newest message.
    pub fn follow_tail(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Whether the list is currently snapping to the newest message.
    pub fn is_following_tail(&self) -> bool {
        self.list.is_following_tail()
    }

    /// Handles this list's four keys and leaves every other key alone.
    ///
    /// **A view that holds the focus receives its own `on_key_down`, and that is
    /// what makes one handler enough for every row.** GPUI builds a key's
    /// dispatch path from the focused node *upwards*
    /// (`window.rs`: `focus_node_id_in_rendered_frame(window.focus)` then
    /// `dispatch_tree.dispatch_path(node_id)`, walked capture-first and then
    /// bubble-first), so the focused node is the first thing the walk reaches. A
    /// list-level handle therefore hears every key aimed at any row, which is the
    /// whole reason [`MessageList::selected`] is a `client_msg_id` rather than
    /// per-row state.
    ///
    /// | Key | What it does | Stops propagation |
    /// |---|---|---|
    /// | [`MOVE_BACK_KEY`] | cursor one row towards the head | yes |
    /// | [`MOVE_FORWARD_KEY`] | cursor one row towards the tail | yes |
    /// | [`ACTIVATE_KEYS`] | retries the selected row, if it is a failed send | **only if it did something** |
    /// | `escape` | nothing here — it propagates to [`crate::app::Shell::on_key_down`] | no |
    ///
    /// # Why `escape` is in that table and claims nothing
    ///
    /// **The chain that brings focus here is the shell's, and a key that arrives
    /// after it has arrived has nothing left to do.** The log is entered *by* the
    /// shell handing focus to [`MessageList::focus_handle`], so the `escape` that
    /// got the user here was consumed by the rung above. There are exactly two
    /// things this handler could do with the next one, and both are wrong:
    ///
    /// - **Swallow it.** 3D's rule, restated by the activation keys below: a
    ///   handler must not claim a key it does not own, because a swallowed
    ///   `escape` is a key the shell's own gesture can never see.
    /// - **Send it back up the ladder.** That is the cycle the module docs reject
    ///   — three views, `escape` a toggle, and a user pressing it repeatedly walks
    ///   them forever.
    ///
    /// **Propagating is the third option and it is the one that terminates.** The
    /// key reaches [`crate::app::Shell::on_key_down`], which re-snaps the log and
    /// re-asserts this handle; [`Window::focus`] returns early when the handle
    /// already holds the focus (`gpui/src/window.rs:2303`), so the second press
    /// changes nothing and no handler in the crate ever moves focus back up.
    ///
    /// # Why the arrows stop propagation and the activation keys do not always
    ///
    /// **The arrows own their keys unconditionally.** They are a list gesture and
    /// nothing above this list has an arrow binding, so propagating would only
    /// invite a future ancestor to answer them too. `up` and `down` synthesise no
    /// `key_char` (`with_simulated_ime` answers `None` for a non-printable key),
    /// so stopping here costs no character.
    ///
    /// **The activation keys stop propagation only when they changed something,
    /// and that asymmetry is the whole of `AGENTS.md` §5.2's focus-order rule.**
    /// `Window::dispatch_keystroke` begins with `keystroke.with_simulated_ime()`,
    /// and that synthesises `key_char` for `"enter"` and `"space"`
    /// (`gpui/src/platform/keystroke.rs:249-251`) — so a propagating `enter`
    /// leaves a `"\n"` in whatever text input is focused. Today nothing is: the
    /// composer is a *sibling* of this list and registers its input handler only
    /// while it holds focus, so with the list focused there is no handler to
    /// forward to. **That is exactly why the rule is written for the future
    /// rather than for today.** A row with nothing to do — no cursor, or a row
    /// whose send is not failed — is not an error and not a claim on the key, so
    /// it propagates; if focus ever reaches this list from the composer, a
    /// swallowed `enter` there would be a composer that cannot send.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();

        let step = match key {
            MOVE_BACK_KEY => Some(STEP_BACK),
            MOVE_FORWARD_KEY => Some(STEP_FORWARD),
            _ => None,
        };
        if let Some(step) = step {
            // The arrows are this list's gesture and nothing above it has an arrow
            // binding, so propagating would only invite a future ancestor to answer
            // them too. `up` and `down` synthesise no `key_char`
            // (`with_simulated_ime` answers `None` for a non-printable key), so
            // stopping here costs no character.
            cx.stop_propagation();
            self.move_selection(step, cx);
            return;
        }

        // `escape` is not in either match above, and that is deliberate rather than
        // an omission: it belongs to the rung above, and the table in this method's
        // documentation says why claiming it here would either swallow the shell's
        // gesture or start a cycle. It propagates, exactly as a character does.
        //
        // The activation keys claim the key only when they did something, and that
        // asymmetry is the whole of `AGENTS.md` §5.2's focus-order rule -- see the
        // section above. `&&` short-circuits, so a key that is neither an arrow nor
        // an activation key reaches neither this call nor `stop_propagation` and
        // propagates.
        if ACTIVATE_KEYS.contains(&key) && self.activate_selection(cx) {
            cx.stop_propagation();
        }
    }

    /// Moves the keyboard cursor `step` rows, and reveals it.
    ///
    /// **The first press lands on an end, and which end is chosen by the
    /// direction rather than by a rule of its own.** With no cursor, `down`
    /// selects the newest message and `up` selects the oldest — the two rows a
    /// user who has just arrived at the log and pressed an arrow is reaching for.
    /// Every press after that steps, clamped at both ends: a chat log's history is
    /// bounded and the ends are real, so `up` on the oldest row stays there
    /// rather than wrapping or refusing.
    ///
    /// **Two probes, both O(1), and neither is a frame-path cost.** The cursor's
    /// index is `AppState::position_of`, a hash lookup in the channel's index map
    /// rather than a scan of its messages; the destination's identity is one slice
    /// index. They happen on a keypress, not per row per frame, so
    /// `AGENTS.md` §2.3 is not in question — and the alternative, storing an index
    /// beside [`MessageList::selected`], would be a second field that goes stale
    /// on the very next head eviction.
    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let selected = self.selected;

        // One lease of the state for the whole question: where the cursor is, how
        // many rows there are, and what the row it lands on is called. A message
        // cannot arrive between them.
        let moved = bridge::try_read(cx, |state| {
            let held = state.messages(&channel);
            let count = held.len();
            if count == 0 {
                return None;
            }

            let at = selected.and_then(|id| state.position_of(&channel, &id));
            let next = match at {
                None if step < 0 => 0,
                None => count - 1,
                Some(at) => at.saturating_add_signed(step).min(count - 1),
            };

            Some(held.get(next).map(|message| message.client_msg_id))
        })
        .flatten()
        .flatten();

        if let Some(client_msg_id) = moved {
            self.select(client_msg_id, cx);
        }
    }

    /// Puts the keyboard cursor on `client_msg_id`, and brings it into view.
    ///
    /// **One frame for one gesture, asked here rather than by the caller.** The
    /// cursor is drawn by rows the list renders, and a cursor that moved without a
    /// repaint would be a cursor that does not move — the same argument
    /// [`MessageList::retry_failed_send`] makes for the badge.
    ///
    /// **The scroll follows the cursor, and it is `scroll_to` rather than
    /// `scroll_to_reveal_item`, and the difference is load-bearing.** A cursor
    /// that moves off screen is not a cursor, so the list has to scroll — but a
    /// chat log opens tail-following, and `List`'s own "make this item visible"
    /// does **not** call `stop_following`. Its `logical_scroll_top` would be
    /// re-anchored to the end sentinel on the next layout
    /// (`gpui/src/elements/list.rs:1211-1218`) and the cursor the user just moved
    /// would be undone by the frame that drew it. `ListState::scroll_to` *does*
    /// suspend following whenever the target is below the end, which is the
    /// behaviour this gesture wants: moving the cursor up through history and
    /// having it stay where it was put.
    ///
    /// **It only scrolls when the row is actually off screen**, and that is what
    /// keeps `down` from yanking the log on every press. `List` answers
    /// `item_is_above_viewport` / `item_is_below_viewport` from real bounds, and
    /// `None` — not enough layout yet — is treated as "off screen", because a row
    /// whose position is unknown cannot honestly be called visible. Note that a
    /// tail-following list answers `above == true` for *every* row below the end
    /// sentinel, so the first cursor move out of a fresh log suspends following
    /// exactly once, which is the intent.
    ///
    /// **A cursor on the newest row leaves following *on*, and that is the
    /// framework agreeing with us rather than overriding us.** `List` re-engages
    /// following on any layout where `is_scrolled_to_end()` is true, and the newest
    /// row is the end — so placing the cursor there and re-engaging is the same
    /// scroll position. The interesting case, where the two rules disagree, is a
    /// row in history, and that is where `stop_following` above earns its keep.
    /// `tests/ui_message_list.rs`
    /// (`the_cursor_is_brought_into_view_and_takes_the_scroll_with_it`) asserts on
    /// the *oldest* row for exactly that reason.
    fn select(&mut self, client_msg_id: Uuid, cx: &mut Context<Self>) {
        let revealed = bridge::try_read(cx, |state| {
            let channel = self.channel.as_deref()?;
            let at = state.position_of(channel, &client_msg_id)?;
            let off_screen = self.list.item_is_above_viewport(at) != Some(false)
                || self.list.item_is_below_viewport(at) != Some(false);
            Some((at, off_screen))
        })
        .flatten();

        if let Some((at, true)) = revealed {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: at,
                offset_in_item: px(0.),
            });
        }

        self.selected = Some(client_msg_id);
        cx.notify();
    }

    /// Retries the selected row, and reports whether this gesture moved the send.
    ///
    /// **The same door the click opens.** [`MessageList::retry_failed_send`] is
    /// called here, not a second time in some keyboard-flavoured shape, so there
    /// is exactly one definition of "put a failed send back in flight" and the
    /// two activations cannot disagree about what it does.
    ///
    /// **And it still does not transmit.** `retry_send` moves
    /// [`DeliveryState::Failed`] to [`DeliveryState::Pending`] and stops, because
    /// there is no socket on this side of the seam; the outbox that would carry a
    /// `Pending` send is `PLAN.md` §7's Phase 3 work. A user who presses `enter`
    /// on a failed row therefore watches the badge change from `failed: …` to
    /// `sending…` and it stays there — exactly as a click does, and for exactly
    /// the same reason. **The keyboard path does not fix that and must not imply
    /// otherwise**, which is why this method has no keyboard-shaped prose about
    /// resending anything.
    ///
    /// **`false` for everything a retry must not do**, and the list cannot tell
    /// those cases apart: no cursor, no state, no such row, and — the one that
    /// matters — a send that is no longer failed because the server's ACK landed
    /// between the frame that drew the cursor and the keypress. The row is already
    /// right in that case, and `false` is what keeps a propagating `enter` from
    /// being swallowed by a gesture that did nothing.
    fn activate_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(client_msg_id) = self.selected else {
            return false;
        };
        self.retry_failed_send(client_msg_id, cx)
    }

    /// Sends a message as this client, optimistically.
    ///
    /// The clock and the identity are parameters for the reason `bridge.rs`
    /// gives: `AGENTS.md` §3.2 gives `state/` no clock, and a UI that invented a
    /// timestamp would write a second, invisible ordering.
    ///
    /// **Returns whether the row is now on screen, not whether the socket
    /// accepted it.** The seam answers in its own vocabulary — an outcome enum
    /// this layer has no business rendering — and the question a list actually
    /// has is "is there a row". Asking the state that question directly is both
    /// simpler and more honest than pattern-matching a refusal taxonomy to
    /// arrive at the same boolean.
    pub fn begin_send(
        &mut self,
        content: &str,
        client_msg_id: Uuid,
        at: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(channel) = self.channel.clone() else {
            return false;
        };

        bridge::try_begin_send(cx, &channel, content, client_msg_id, at);
        let on_screen = bridge::try_read(cx, |state| {
            state.message(&channel, &client_msg_id).is_some()
        })
        .unwrap_or(false);

        if on_screen {
            self.sync(cx);
            // Sending is a gesture, and the optimistic row must be on screen
            // before the server has seen it — `AGENTS.md` §8.1's whole point.
            cx.notify();
        }
        on_screen
    }

    /// Puts a failed send back in flight, on the badge's behalf.
    ///
    /// **This is the whole gesture. It does not transmit anything, and the
    /// difference is the honest limit of work unit 3D.**
    /// [`bridge::try_retry_send`] moves the send from
    /// [`DeliveryState::Failed`] to [`DeliveryState::Pending`] and stops: there is
    /// no socket on this side of the seam, and the outbox that would put a
    /// `Pending` send on a wire is `PLAN.md` §7's Phase 3 work. **A user who
    /// clicks retry therefore watches the badge change from `failed: …` to
    /// `sending…`, and it stays there** until an outbox exists to move it. That is
    /// what the label says and what this method does; neither claims more.
    ///
    /// **The gesture asks for a frame here rather than in the listener.** The badge
    /// is drawn by [`super::message_row`], which cannot schedule anything: a row
    /// holds no `Context` and has nothing to notify. And the frame is needed — a
    /// gesture that changed a row's badge and asked for nothing would leave the old
    /// badge on screen until something unrelated repainted.
    ///
    /// **A pointer click would mask a missing `notify`, and that is why this method
    /// is tested by calling it directly.** Pressing a stateful element updates that
    /// element's hover and pressed state, which dirties the window on its own, so a
    /// click-driven test cannot tell "the gesture asked for a frame" from "the
    /// framework happened to repaint". `tests/ui_message_list.rs`
    /// (`the_retry_gesture_schedules_its_own_frame`) closes that gap by driving the
    /// gesture the way a non-pointer activation would have to.
    ///
    /// # Returns whether *this* gesture moved the send, and why that is the question
    ///
    /// **Not "does the badge read `sending…`", and the difference is a real bug.**
    /// Asking the second question answers `true` for a send that was *already*
    /// `Pending` — so a second click on a badge the first click has already
    /// retired would report success, ask for a frame, and redraw a list whose
    /// contents had not changed. Asking the first question cannot: it is a
    /// comparison, and a refused retry changes nothing to compare.
    ///
    /// **The comparison, and why there are two reads.** The seam answers in its own
    /// vocabulary — an outcome enum this layer may not name
    /// (`crate::state::actions` is on the forbidden-token list in
    /// `tests/layer_boundary.rs`) and has no business rendering. So the state is
    /// read before and after, and the answer is the difference. That costs two extra
    /// `try_global` probes on a gesture a user performs once per failed send, which
    /// is not a cost worth optimising away in exchange for a `ui/` file naming a
    /// state type.
    ///
    /// **`false` covers every case where the badge must not change:** no bridge
    /// installed, nothing held under that id, and — the one that matters — **a send
    /// that is no longer failed because the server's ACK landed between the frame
    /// that painted the badge and the click that hit it.** The state reports that as
    /// a refusal for exactly this reason, and the row is already correct.
    ///
    /// **The `if retried` guard also keeps a refusal from repainting, and that part
    /// is documented rather than tested.** A refusal changes nothing, so the frame it
    /// would produce is identical to the one already on screen; the harness exposes
    /// no frame counter, so a test could not tell the two apart and would pass for
    /// the wrong reason. `tests/ui_message_list.rs`
    /// (`the_retry_gesture_schedules_its_own_frame`) asserts the half that *is*
    /// observable — that a successful gesture schedules its own frame — and its
    /// documentation names this half as untested so the coverage gap is on the
    /// record.
    pub fn retry_failed_send(&mut self, client_msg_id: Uuid, cx: &mut Context<Self>) -> bool {
        // `None` for "no bridge installed" and `None` for "no delivery recorded"
        // are the same value here on purpose: a list cannot tell them apart, and
        // neither can it act differently on either.
        let before = bridge::try_read(cx, |state| state.delivery(&client_msg_id)).unwrap_or(None);

        // The door's own answer is deliberately discarded. `Option` is
        // `#[must_use]` and so is the outcome inside it, and the comparison below
        // is both more honest and more useful than matching on a refusal taxonomy.
        let _ = bridge::try_retry_send(cx, client_msg_id);

        let after = bridge::try_read(cx, |state| state.delivery(&client_msg_id)).unwrap_or(None);
        let retried = after == Some(DeliveryState::Pending) && before != after;

        if retried {
            // One frame for one gesture. `sync` is *not* called and does not need
            // to be: a retry changes no message and no count, so there is nothing
            // to splice or reset — only the row's badge, which `RowSpec::differs_from`
            // catches on the next render because it compares `delivery` and
            // `failure_detail`. That comparison then reports `changed`, so the
            // height change is remeasured through the deferred path the module docs
            // describe.
            //
            // **This notify is load-bearing even though a pointer click refreshes
            // the window on its own.** A mousedown on a stateful element updates
            // that element's hover and pressed state, which dirties the window
            // whether or not anything asked it to — so the badge would appear to
            // repaint either way, and a reader could not tell this line from
            // decoration. The gesture that cannot rely on that is any future
            // non-pointer activation, and the test that pins the contract calls
            // this method directly rather than through the pointer.
            cx.notify();
        }
        retried
    }

    /// How many messages the state holds for a channel.
    ///
    /// `0` for every failure — no bridge, or a channel nothing has arrived for —
    /// because "no messages" and "no state" are the same thing to a list, and
    /// `AGENTS.md` §2.1 forbids the `unwrap` that would distinguish them by
    /// crashing.
    fn messages_in(&self, channel: &str, cx: &mut Context<Self>) -> usize {
        bridge::try_read(cx, |state| state.messages(channel).len()).unwrap_or(0)
    }

    /// Applies every deferred height change in one call.
    ///
    /// A single range from the lowest to the highest pending index rather than
    /// one call per index: the pending set is the rows currently visible, so the
    /// range is a screenful, and `remeasure_items` is documented to preserve the
    /// scroll position (*"does not… blow away `logical_scroll_top`"*). One call
    /// also avoids re-anchoring the scroll position several times in a frame.
    fn flush_remeasures(&mut self) {
        if self.pending_remeasure.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_remeasure);

        let Some(first) = pending.iter().min().copied() else {
            return;
        };
        let Some(last) = pending.iter().max().copied() else {
            return;
        };
        if self.count == 0 {
            return;
        }
        self.list.remeasure_items(first..last + 1);
    }

    /// The element for one item, called by `gpui::List` for visible rows.
    ///
    /// **Never calls `remeasure_items`, and must not be changed to.** See the
    /// module docs: this closure runs inside `list.rs`'s own `&mut` borrow of its
    /// state, so a `remeasure_items` here is a `RefCell` double borrow rather
    /// than a slow frame.
    fn render_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(spec) = self.row_spec(index, cx) else {
            return Empty.into_any_element();
        };

        let (row, changed) = self.rows.row(spec, self.colors, cx);
        if changed {
            self.pending_remeasure.push(index);
        }
        row.into_any_element()
    }

    /// What one row should show, read from the state.
    fn row_spec(&self, index: usize, cx: &mut Context<Self>) -> Option<RowSpec> {
        let channel = self.channel.clone()?;

        // One lease of the state for everything the row needs, so the message
        // cannot change under the four reads it would otherwise take.
        let (client_msg_id, meta) = bridge::try_read(cx, |state| {
            let message = state.messages(&channel).get(index)?;
            let is_self = state.self_user_id() == message.user_id.as_str();
            Some((
                message.client_msg_id,
                RowMeta {
                    author: message.user_id.clone(),
                    is_self,
                    timestamp: message.timestamp,
                    delivery: state.delivery(&message.client_msg_id),
                    failure_detail: state
                        .failure(&message.client_msg_id)
                        .map(|failure| failure.detail().to_owned()),
                    reactions: message
                        .reactions
                        .iter()
                        .map(|reaction| (reaction.emoji.clone(), reaction.user_ids.len()))
                        .collect(),
                },
            ))
        })??;

        let document = self.document_for(client_msg_id, index, cx)?;

        Some(RowSpec {
            client_msg_id,
            author: meta.author,
            is_self: meta.is_self,
            timestamp: meta.timestamp,
            document,
            delivery: meta.delivery,
            failure_detail: meta.failure_detail,
            reactions: meta.reactions,
            // One comparison, and it is the identity the cursor already holds.
            // A `bool` rather than the cursor itself, so the row receives the
            // answer and not the question — the row's invariant says it holds no
            // state about the message, and this keeps that true while letting the
            // row *show* where the cursor is.
            is_selected: self.selected == Some(client_msg_id),
        })
    }

    /// The parsed body of the message at `index`, parsed on first sight and
    /// cached afterwards.
    ///
    /// **Parsed on render rather than on arrival, and the bridge says why:** *"a
    /// channel's whole history would be parsed by an eager caller, and a bounded
    /// cache would then evict what the user is about to scroll to."* So the miss
    /// path here is the normal first frame for a message, and every frame after
    /// it is a hit — `try_rendered` is the promoting probe (`bridge.rs` §4), so
    /// reading a row is what protects its document from eviction.
    ///
    /// The source text is read **only on a miss**. Reading it every frame to
    /// compare against a cached copy would be the deep clone of message history
    /// `AGENTS.md` §2.3 forbids, on the one path where it is pure waste.
    fn document_for(
        &self,
        client_msg_id: Uuid,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Document>> {
        let channel = self.channel.clone()?;

        match bridge::try_rendered(cx, &client_msg_id) {
            Rendered::Hit(document) => Some(document),
            Rendered::Miss => {
                let source = bridge::try_read(cx, |state| {
                    state
                        .messages(&channel)
                        .get(index)
                        .map(|message| message.content.clone())
                })??;

                bridge::try_render_and_cache(cx, client_msg_id, &source, source.len() as u64);

                match bridge::try_rendered(cx, &client_msg_id) {
                    Rendered::Hit(document) => Some(document),
                    // The cache refused to hold it — `core/cache.rs`'s ceiling,
                    // which is a budget rather than a limit on what may be sent.
                    // Parsing it here for this frame is that module's stated
                    // cost, and the alternative is a message that never draws.
                    Rendered::Miss | Rendered::NotInstalled => {
                        Some(Arc::new(parse_markdown(&source)))
                    }
                }
            }
            // No state, therefore no messages, therefore nothing to parse: the
            // caller's `row_spec` returns `None` on the same condition.
            Rendered::NotInstalled => None,
        }
    }
}

impl Focusable for MessageList {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MessageList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Before the frame's layout, and outside every borrow of the list's
        // state — which is the only place a remeasure may happen. See the module
        // docs.
        self.sync(cx);

        div()
            .id("message-list")
            // Records this container's bounds for the headless test, for the
            // same reason a row records its own.
            .debug_selector(|| "message-list".to_owned())
            .flex()
            .flex_col()
            .size_full()
            // Explicit, per AGENTS.md 7.3.
            .bg(self.colors.background)
            .text_color(self.colors.text)
            // **On this container's own element, and it has to be here rather
            // than anywhere in the tree.** `track_focus` is what registers the
            // handle against *this* node in the rendered frame's dispatch tree,
            // and the dispatch path of a keypress starts at the focused node —
            // so a handle this element does not track is a handle that can be
            // focused and from which no listener will ever run. That is the same
            // silent failure `InputBar::focus_handle` documents, and it is the
            // reason a listbox handle is a container's rather than a leaf's.
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                list(
                    self.list.clone(),
                    cx.processor(
                        |this: &mut MessageList,
                         index: usize,
                         _window: &mut Window,
                         cx: &mut Context<MessageList>| {
                            this.render_row(index, cx)
                        },
                    ),
                )
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .w_full()
                .flex_grow_1(),
            )
    }
}
