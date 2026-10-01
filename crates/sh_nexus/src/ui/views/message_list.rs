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
//! # What this file deliberately does not do
//!
//! No delegate trait (ADR-006: *"a trait over a single call site is indirection
//! with no second implementation to justify it"*), no `ListMeasuringBehavior::Measure`
//! opt-in until a measurement says the default is insufficient, and no search,
//! thread panel or editing UI — all Phase 2 items this ADR does not govern. The
//! `set_scroll_handler` hook Phase 3's paged history needs is not wired either,
//! because nothing calls it yet and a registered handler nobody triggers is
//! indistinguishable from no handler at all.

use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::{
    div, list, prelude::*, px, AnyElement, Context, Empty, Entity, FollowMode, IntoElement,
    ListAlignment, ListSizingBehavior, ListState, Render, Window,
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
