//! One message, drawn.
//!
//! `docs/ARCHITECTURE.md` ADR-006's step 3: *"One `Entity<MessageRow>` per
//! `client_msg_id`, held in a map bounded per §7.1, so a row's state survives
//! scroll-out. This is §7.3's recycle requirement and it is the only part of the
//! list that is built rather than wired."*
//!
//! # Why a row is an entity and the cache holds entities
//!
//! `gpui::List`'s contract is that elements outside the viewport may not change
//! height, and it keeps its layout state intrusively so the caller can
//! coordinate with it. An element built fresh in the render closure satisfies
//! neither half of that: nothing survives the frame, so a row that scrolls out
//! and back loses whatever it had, and every visit pays for the whole tree
//! again. A row that is an `Entity` is cached by GPUI like any other view, so an
//! unchanged row is not re-rendered at all and a changed one is re-rendered
//! because it was told to be.
//!
//! **The cache is bounded by count, not by bytes, and the distinction is
//! stated because §7.1 asks for a bound rather than a unit.** A row holds an
//! `Arc<Document>`, and the documents are shared and themselves bounded by
//! `core/cache.rs`'s segment budget; what is unbounded without this bound is the
//! number of live entities. [`MAX_RETAINED_ROWS`] is that bound, and the
//! eviction order is first-in-first-out because rows arrive in message order —
//! the oldest row is the one furthest from the viewport, which is the one whose
//! state is least likely to be missed.
//!
//! # What a row is *not* allowed to hold
//!
//! It holds a [`RowSpec`] — owned display data — and never a reference into the
//! application state. `AGENTS.md` §3.2 gives `ui/` no ownership of the state,
//! and `tests/bridge.rs` fails the build on a view that names the state's type.
//! A by-value spec is also what makes a row testable without a bridge at all:
//! `RowSpec` is constructible from nothing but plain data.
//!
//! **A second field arrived in work unit 3D, and it is a handle to the *view*,
//! not to the state.** [`MessageRow::owner`] is a
//! `WeakEntity<MessageList>`: the list that owns this row, so a control the row
//! draws has somewhere to dispatch to. It holds no application state, names no
//! state type, and would survive every check above unchanged. The invariant it
//! does not break is the one that was actually about safety.
//!
//! **A third arrived in work unit 3E, and it is a `bool` on the spec rather than
//! a field at all.** [`RowSpec::is_selected`] says whether the list's keyboard
//! cursor is on this row, which makes the row *show* a selection while adding no
//! state to it: a row rebuilt from a spec cannot disagree with the list about
//! which row is current, because the list is the only thing that knows. This is
//! the same reason the owner handle is weak and the same reason nothing here
//! reaches `cx.update_global` — the row reads a snapshot and draws it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::{div, prelude::*, AnyElement, App, Context, Entity, Hsla, Render, WeakEntity, Window};
use uuid::Uuid;

use crate::core::markdown::Document;
use crate::state::DeliveryState;
use crate::ui::views::message_list::MessageList;
use crate::ui::{markdown, Colors};

/// The debug selector the retryable badge records its bounds under.
///
/// A *static* selector, for the reason the row's own is static: `debug_bounds`
/// takes `&'static str`, so a per-message key could never be queried. The headless
/// test drives a single failed send, and one failed send is one badge.
pub const RETRY_BADGE_SELECTOR: &str = "retry-badge";

/// The element id the retryable badge carries.
///
/// **Load-bearing rather than decorative.** `.on_click` lives on
/// `StatefulInteractiveElement`, so an element only reaches it through `.id(..)`
/// — a plain `div()` cannot be clickable at all. And the id is unique per row
/// without saying so: GPUI builds a global element id from the whole ancestor id
/// stack, and every entity-rooted element pushes `ElementId::View(entity_id)`
/// first (`gpui/src/element.rs::prepare_element_id`), so this constant in row A
/// and this constant in row B are different elements.
pub const RETRY_BADGE_ID: &str = "delivery-retry";

/// The word the retryable badge carries.
///
/// **`retry`, and not "resend" or "send again", because it is the only word the
/// client can keep.** [`crate::state::bridge::try_retry_send`] moves the send
/// from `Failed` to `Pending` and stops — there is no socket on this side of the
/// seam, and the outbox that would transmit it is Phase 3. "resend" would name a
/// transport that does not exist.
const RETRY_LABEL: &str = "retry";

/// How many rows the cache keeps before evicting the oldest.
///
/// **Derived from the frame budget rather than picked.** A row exists so a
/// visible one has stable state; the viewport plus `gpui::List`'s overdraw is
/// the set that has to be stable at once, and at 512 rows a 600px window with
/// generous overdraw is two orders of magnitude inside the bound. `AGENTS.md`
/// §7.1's requirement is that there *is* a bound and that it is stated; the
/// number is documented so a later measurement can move it with evidence rather
/// than by feel.
pub const MAX_RETAINED_ROWS: usize = 512;

/// Everything one row displays, owned.
///
/// Produced by [`super::message_list::MessageList`] from the application state,
/// and deliberately free of any state type other than [`DeliveryState`] — which
/// `PLAN.md` §5 makes client state the UI is expected to name. A spec is
/// complete: rendering it needs no second lookup, which is what lets a row be
/// asserted on in a test that has no window.
#[derive(Debug, Clone)]
pub struct RowSpec {
    /// The message's client-generated identity, and the row's identity.
    ///
    /// `AGENTS.md` §7.4's `client_msg_id` is the one identifier a message has
    /// *before* the server has seen it, which is exactly what makes it the right
    /// key for a row that has to survive an optimistic send being reconciled: a
    /// row keyed on the server's `id` would be a different row after the ACK.
    pub client_msg_id: Uuid,
    /// The author's id, which is what the state holds. A display name arrives
    /// with `network/`'s user directory in Phase 4, and inventing one from an id
    /// here would be a guess this layer is not entitled to make.
    pub author: String,
    /// Whether the author is this client's own user, so the row can say so.
    pub is_self: bool,
    /// When the server accepted it. `AGENTS.md` §2.1: a `DateTime<Utc>`, never a
    /// string; the display string is built here, at render time.
    pub timestamp: DateTime<Utc>,
    /// The parsed body. An `Arc` because the cache owns the document and a row
    /// holds a lease on it, not a copy.
    pub document: Arc<Document>,
    /// `None` for a message this client did not send.
    pub delivery: Option<DeliveryState>,
    /// The server's explanation, when the send failed.
    ///
    /// A `String` rather than the state's `SendFailure`, so that this layer holds
    /// data and not a state type: `PLAN.md` §7 requires a failure to stay
    /// visible, and the visible part is the detail sentence.
    pub failure_detail: Option<String>,
    /// Reactions, as `(emoji, count)`. `PLAN.md` §5 has the domain holding the
    /// reacting users; a chip shows a count, so the count is what crosses.
    pub reactions: Vec<(String, usize)>,
    /// Whether this row is the one the list's keyboard cursor is on.
    ///
    /// **A flag in the spec rather than state in the row, and the reason is the
    /// same one the whole file's contract rests on.** The selection lives on
    /// [`MessageList`](super::message_list::MessageList) as a `client_msg_id`,
    /// because the list is never evicted and a row is; storing it here would make
    /// a row that is scrolled out and back disagree with the list about which
    /// row is current. It arrives the way `delivery` does — as data the frame
    /// draws — so the row's invariant still reads: no reference into the state,
    /// no state type named.
    ///
    /// **A row that is not selected must still *look* the same size as one that
    /// is**, because moving the cursor must not reflow the log. That is enforced
    /// by [`MessageRow::selection_rule_color`] being a colour swap on a
    /// constant-width border and never a width change.
    pub is_selected: bool,
}

impl RowSpec {
    /// Whether a row already showing `self` would have to be redrawn for `other`.
    ///
    /// **The document is compared by pointer, and that is the whole reason this
    /// method exists rather than a `PartialEq` derive.** `Document` is a deep
    /// tree and `Arc<Document>`'s derived equality walks it, on the frame path,
    /// for every visible row — to decide whether to redraw the very tree it just
    /// walked. Two specs that share an `Arc` are the same parse; two that do not
    /// are re-parsed even when the text is identical, and redrawing is the safe
    /// answer for that case.
    ///
    /// **`is_selected` is compared, and the remeasure that comes with it is
    /// conservative rather than necessary.** Leaving it out would be the cheaper
    /// choice and a strictly worse one: the row under a cursor that moved onto it
    /// would not be told to redraw, and an invisible selection is not a
    /// selection. Including it means a cursor move also routes the two affected
    /// rows through [`MessageRow::set`]'s `true`, which
    /// [`MessageList`](super::message_list::MessageList) turns into one
    /// coalesced `remeasure_items` over the visible range. **That remeasure
    /// changes nothing, and it is deliberately so:** the selection rule is a
    /// constant-width border whose *colour* changes
    /// ([`MessageRow::selection_rule_color`]), so the row's geometry is
    /// identical either way. The cost is one extra layout pass per cursor move,
    /// against the alternative of a selection nobody can see.
    pub fn differs_from(&self, other: &RowSpec) -> bool {
        self.client_msg_id != other.client_msg_id
            || self.author != other.author
            || self.is_self != other.is_self
            || self.timestamp != other.timestamp
            || self.delivery != other.delivery
            || self.failure_detail != other.failure_detail
            || self.reactions != other.reactions
            || self.is_selected != other.is_selected
            || !Arc::ptr_eq(&self.document, &other.document)
    }

    /// Whether this row's delivery badge offers a retry.
    ///
    /// **On the spec rather than inside the render, and that placement is what
    /// makes the rule checkable.** Whether a badge is a control is a fact about
    /// the data the frame draws, and `RowSpec` is the complete statement of that
    /// data — so a predicate living here can be asserted without a window. The
    /// failure this guards against is a badge that *looks* clickable on a send
    /// with nothing to retry (`Pending` has already asked, `Acked` is done), and
    /// a rule that could only be reached by painting is a rule a test asserts
    /// indirectly at best.
    ///
    /// [`DeliveryState::Failed`] and nothing else, which is also the only state
    /// the seam's retry door accepts — see
    /// [`try_retry_send`](crate::state::bridge::try_retry_send). So the set of rows
    /// that *look* retryable and the set that are retryable are the same set, by
    /// construction rather than by two literals happening to agree.
    pub fn offers_retry(&self) -> bool {
        matches!(self.delivery, Some(DeliveryState::Failed))
    }
}

/// One row of the message list.
///
/// An `Entity<MessageRow>` is what [`RowCache`] holds, and its `Render` is what
/// GPUI caches between frames.
pub struct MessageRow {
    /// The message this row shows. Replaced through [`MessageRow::set`].
    spec: RowSpec,
    /// The palette the row draws with. Copied in, because `Colors` is `Copy` and
    /// a row outlives any borrow of the view that built it.
    colors: Colors,
    /// The list this row belongs to, weakly.
    ///
    /// # Why this exists, when the row's invariant says it holds no state
    ///
    /// The design this implements said the badge's click listener could be built
    /// in `MessageList::render_row` and handed down, on the grounds that
    /// `render_row` already holds a `&mut MessageList`. **That is not where the
    /// badge is built.** `render_row` returns `row.into_any_element()`, and the
    /// badge is built inside [`MessageRow::render`], whose context is a
    /// `Context<MessageRow>` — a listener written there reaches the row and
    /// nothing above it. The alternative routes were a GPUI `Action` (dispatched
    /// on the *focused* node's dispatch path, and the focused node here is the
    /// composer, a sibling of this list) and a callback closure per row (an
    /// allocation per cached row, 512 of them). A weak handle to the owner is the
    /// third option, and it is the cheapest of the three.
    ///
    /// **What it is not.** It is a handle to the view that owns this row, not
    /// state about the message: two rows of the same message would hold the same
    /// value, and replacing the row's content cannot change it. So the invariant
    /// above — *no reference into the application state, and no state type named*
    /// — still reads exactly as it did before this field, and would still hold
    /// with this field deleted.
    ///
    /// **Weak, so ownership is not a cycle.** The list holds
    /// `Entity<MessageRow>` through [`RowCache`], so a strong handle back would
    /// be a cycle. `WeakEntity::update` answers `Err` once the list is released,
    /// which is the natural no-op for a click that arrives afterwards: a list that
    /// is gone cannot have a live row inside it. `app.rs`'s drain pump reads the
    /// same answer as the signal that the shell is gone.
    owner: WeakEntity<MessageList>,
}

impl MessageRow {
    /// Builds a row for `spec`, owned by `owner`.
    ///
    /// **No `Context` is taken, and that is still true:** a row has no state of
    /// its own beyond its spec and the handle back to its owner, so there is
    /// nothing to register and nothing to focus. See [`MessageRow::owner`] for why
    /// the handle exists, and for why it is not the state the sentence above rules
    /// out.
    ///
    /// `owner` is fixed here and never updated, which is a consequence rather than
    /// an oversight: a row belongs to exactly one list — the cache that holds it
    /// — so there is no second owner to move it to.
    pub fn new(spec: RowSpec, colors: Colors, owner: WeakEntity<MessageList>) -> Self {
        Self {
            spec,
            colors,
            owner,
        }
    }

    /// What this row is currently showing.
    ///
    /// Exposed so a test can assert on what a frame would draw without walking
    /// the element tree, which is the same reason `state/app_state.rs` exposes
    /// accessors rather than fields.
    pub fn spec(&self) -> &RowSpec {
        &self.spec
    }

    /// Replaces the row's content, and reports whether anything changed.
    ///
    /// **The return value is load-bearing, and it is what makes the list
    /// correct rather than merely fast.** A changed row may have a new height,
    /// and `gpui::List` requires the caller to say so — the caller is
    /// `MessageList`, which turns `true` into a `remeasure_items` for this row's
    /// index. Returning `true` for an unchanged row would remeasure the list
    /// every frame; returning `false` for a changed one would leave every row
    /// below it positioned by a stale height.
    pub fn set(&mut self, spec: RowSpec, cx: &mut Context<Self>) -> bool {
        if !self.spec.differs_from(&spec) {
            return false;
        }
        self.spec = spec;
        cx.notify();
        true
    }

    /// Replaces the row's palette, and reports whether it changed.
    ///
    /// **A palette change is a change, and reporting it as one is what keeps a
    /// theme change from being half-applied.** A row holds its own `Colors` — it
    /// outlives the view that built it — so a caller that replaces the list's
    /// palette has to say so here or every row already on screen keeps drawing the
    /// theme it was created under.
    ///
    /// **A changed palette is reported as changed even when its text did not
    /// change**, because a different fill can be a different height and
    /// `gpui::List` is told about heights by this return value. The cost is one
    /// remeasure of the visible range on the frame after a theme change, coalesced
    /// by `MessageList::flush_remeasures` into a single range call; the saving is
    /// rows that stay where they were told to be.
    pub fn set_colors(&mut self, colors: Colors, cx: &mut Context<Self>) -> bool {
        if self.colors == colors {
            return false;
        }
        self.colors = colors;
        cx.notify();
        true
    }

    /// The palette this row draws with.
    ///
    /// Exposed so a test can assert that a theme change reached the rows rather
    /// than only the container they sit in.
    pub fn colors(&self) -> Colors {
        self.colors
    }

    /// Whether this row is the one the list's keyboard cursor is on.
    ///
    /// **A read of the spec rather than a field, and that is the invariant
    /// working.** The cursor is the *list's* state — an id, on the one view that
    /// is never evicted — and this row was handed a snapshot of whether it
    /// matches. Reading it back is how a test asks "did the cursor reach this
    /// row" without walking the element tree.
    pub fn is_selected(&self) -> bool {
        self.spec.is_selected
    }

    /// The fill behind this row's text: its own bubble, not the other's.
    ///
    /// Named rather than inlined because two things need the same answer and
    /// disagreeing about it would be a bug nobody could see: the bubble itself,
    /// and the *unselected* end of the selection rule below.
    fn bubble_color(&self) -> Hsla {
        if self.spec.is_self {
            self.colors.bubble_self
        } else {
            self.colors.bubble_other
        }
    }

    /// The colour of this row's left-hand selection rule.
    ///
    /// **[`Colors::accent`] when the row is selected and the row's own bubble
    /// colour when it is not, and both halves of that are load-bearing.**
    ///
    /// *`accent`, not `danger`.* The rule answers "where is the cursor", and
    /// `danger` is the failed-send state (`ui/mod.rs`) — a rule in `danger`
    /// would tell a keyboard user that the row they are standing on has failed,
    /// which on most rows is a lie. `accent` is the palette's *interactive*
    /// colour, so the rule reads as the affordance it is, the same word the
    /// retry badge is drawn in.
    ///
    /// *The bubble colour, not a second palette entry.* Unselected, the rule is
    /// invisible because it is painted in the fill it sits on. That is why
    /// `Colors` does not grow a `selection_idle` key: an opaque rule in some
    /// third colour would be a permanent stripe on every row of the log, and
    /// `AGENTS.md` §7.3's theme test pins every colour to full opacity, so
    /// "absent" cannot be expressed as a colour — it is expressed as the colour
    /// it is hiding against.
    ///
    /// **Width never changes, and that is why this is a colour swap.** A rule
    /// that appeared or vanished would change the row's height, and moving the
    /// cursor would reflow every row below it — the same reason the retry badge
    /// hovers on `bg` only. See [`RowSpec::differs_from`], which carries the
    /// remeasure this conservative height-neutral change costs anyway.
    pub fn selection_rule_color(&self) -> Hsla {
        if self.spec.is_selected {
            self.colors.accent
        } else {
            self.bubble_color()
        }
    }

    /// The element for the author-and-time line.
    fn header(&self) -> AnyElement {
        let colors = self.colors;
        let author_color = if self.spec.is_self {
            colors.accent
        } else {
            colors.mention
        };

        div()
            .flex()
            .flex_row()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    // Explicit, per AGENTS.md 7.3.
                    .text_color(author_color)
                    .child(self.spec.author.clone()),
            )
            .child(
                div()
                    .text_sm()
                    // Explicit, per AGENTS.md 7.3.
                    .text_color(colors.text_muted)
                    // `AGENTS.md` §2.1 forbids a stored formatted timestamp; this
                    // one exists for the duration of the frame.
                    .child(self.spec.timestamp.format("%H:%M").to_string()),
            )
            .into_any_element()
    }

    /// The element for the delivery badge, if this client sent the message.
    ///
    /// `PLAN.md` §7 requires a failed send to stay visible, and §8.1's
    /// Optimistic Send Flow requires the pending state to be on screen before
    /// the server has seen the message. Both are here, and `Acked` is the one
    /// state that draws nothing: a delivered message is the normal case, and a
    /// badge for it would be noise on every row.
    ///
    /// # A failed badge is a control, and the label is the honest one
    ///
    /// `PLAN.md` §7 says a terminally-failed send *"stays visible for retry"*.
    /// Staying visible was already true; this is the half that was missing, and it
    /// is the whole of work unit 3D. So the failed badge carries an `on_click` that
    /// dispatches to the list, and the badge reads `failed: <the server's words>`
    /// followed by **`retry`** in [`Colors::accent`] — the interactive colour, which
    /// is what tells the reader which word is the affordance.
    ///
    /// **What the word does not promise.** The click moves the send to
    /// [`DeliveryState::Pending`] and stops; there is no socket on this side of the
    /// seam, so the badge immediately afterwards reads `sending…` and stays there
    /// until the outbox exists to move it. `retry` is the only label the client can
    /// keep — `resend` names a transport that does not exist — and the gap between
    /// the label and the delivery is stated here and on
    /// [`crate::state::bridge::try_retry_send`] rather than left for a user to
    /// discover.
    ///
    /// **Cursor and hover are not decoration on a control drawn in `danger`.**
    /// Without them the badge reads as static text that happens to be clickable,
    /// which is a worse failure than no affordance: the user tries it, gets
    /// nothing, and concludes the feature is broken. `bg` is the only property
    /// that changes on hover, so pointing at the badge cannot re-measure the row
    /// under the pointer — and [`MAX_RETAINED_ROWS`] rows are measured precisely
    /// once each.
    ///
    /// # What this control deliberately does *not* advertise
    ///
    /// **There is no `Role::Button` here, and the omission is the point.** A role
    /// is a claim about how assistive technology may operate the element, and this
    /// element has exactly one activation route *on itself*: a pointer click, or
    /// the list's cursor arriving here and pressing Enter
    /// ([`MessageList::retry_failed_send`](super::message_list::MessageList::retry_failed_send)).
    /// It has no focus handle of its own to `track_focus` and no `on_a11y_action`
    /// handler (the route an AT uses to synthesise a click), so `Role::Button`
    /// would still announce a button whose only *direct* activation is a pointer.
    /// `accesskit`'s `Action::Click` handler cannot be exercised by
    /// this crate's headless harness either — `Window::a11y.is_active()` is false
    /// there — so shipping it would be shipping an untested activation path.
    ///
    /// # The keyboard omission recorded by work unit 3D, and what closed it
    ///
    /// **3D left a defect on the record here, and 3E closed it — the entry below is
    /// kept as the reasoning that chose the mechanism, because that reasoning is
    /// why this file still holds no focus handle.** `AGENTS.md` §5.2 requires
    /// keyboard-only operation and a click is not that.
    ///
    /// 3D's argument was that a per-row fix is not available, and each of its four
    /// legs still holds:
    ///
    /// - A keyboard-operable *element* needs a [`gpui::FocusHandle`] plus
    ///   `track_focus`, and a `FocusHandle` is *state on the row* — the exact
    ///   thing this file's constructor exists to avoid.
    /// - A focus handle would have to survive `RowCache::evict`, and eviction
    ///   happens at [`MAX_RETAINED_ROWS`]; a window left focused on a released
    ///   row's handle is a dangling focus, not a lost keystroke.
    /// - GPUI rebuilds the tab-stop map every painted frame from the handles that
    ///   were actually painted (`window.rs` clears `tab_stops` before each frame
    ///   and `div.rs` inserts into it during paint), so a row scrolled out of the
    ///   overdraw loses its tab stop and takes the focus with it.
    /// - `on_key_down` on a row fires only for the focused node's dispatch path,
    ///   so it needs the focus model before it needs the handler.
    ///
    /// **So the two options 3D named were a per-row handle with eviction-aware
    /// focus management, or a list-level selection model. 3E took the second**,
    /// and this is the file that shows what that costs: the row holds a
    /// [`RowSpec::is_selected`] flag and draws it, and every question about *where
    /// the cursor is* — the keys, the tab order, the focus handle, the eviction —
    /// belongs to [`MessageList`](super::message_list::MessageList). The row stays
    /// a function of its spec.
    fn delivery_badge(&self) -> Option<AnyElement> {
        let colors = self.colors;
        let (label, color) = match self.spec.delivery? {
            DeliveryState::Acked => return None,
            DeliveryState::Pending => ("sending…".to_owned(), colors.text_muted),
            DeliveryState::Failed => match &self.spec.failure_detail {
                Some(detail) => (format!("failed: {detail}"), colors.danger),
                None => ("failed".to_owned(), colors.danger),
            },
        };

        // Only a failed send offers anything, and the decision is the spec's so a
        // test can assert it without painting a row.
        if !self.spec.offers_retry() {
            return Some(
                div()
                    .text_sm()
                    // Explicit, per AGENTS.md 7.3.
                    .text_color(color)
                    .child(label)
                    .into_any_element(),
            );
        }

        let owner = self.owner.clone();
        let client_msg_id = self.spec.client_msg_id;

        Some(
            div()
                .id(RETRY_BADGE_ID)
                // Records this element's bounds for the headless test, for the
                // same reason the row records its own: `.id()` alone records
                // nothing.
                .debug_selector(|| RETRY_BADGE_SELECTOR.to_owned())
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_1()
                .rounded_sm()
                // Explicit, per AGENTS.md 7.3. GPUI inherits nothing, so this
                // sets the badge's own default and both children below set theirs
                // again rather than relying on it.
                .text_color(color)
                .cursor_pointer()
                // `bg` only, so hovering cannot change the badge's size — see the
                // note above on remeasurement.
                .hover(|style| style.bg(colors.surface))
                .on_click(move |_event, _window, cx| {
                    // `Err` is a released list, and there is nothing to retry
                    // against: it cannot be holding the row that painted this
                    // badge. Discarding the answer is the whole of the correct
                    // behaviour — see `MessageRow::owner`.
                    //
                    // `retry_failed_send` reports whether the row now reads
                    // `sending…`, and the list asks for its own frame from there.
                    // This listener adds nothing, because a frame requested by the
                    // list is the same frame the row's next render belongs to.
                    let _ = owner.update(cx, |list, cx| {
                        list.retry_failed_send(client_msg_id, cx);
                    });
                })
                .child(div().text_color(color).child(label))
                .child(div().text_color(colors.accent).child(RETRY_LABEL))
                .into_any_element(),
        )
    }

    /// The element for the reaction chips, if there are any.
    fn reactions(&self) -> Option<AnyElement> {
        if self.spec.reactions.is_empty() {
            return None;
        }
        let colors = self.colors;
        let mut root = div().flex().flex_row().gap_1();

        for (emoji, count) in &self.spec.reactions {
            root = root.child(
                div()
                    .px_2()
                    .rounded_sm()
                    .bg(colors.surface)
                    // Explicit, per AGENTS.md 7.3.
                    .text_color(colors.text)
                    .child(format!("{emoji} {count}")),
            );
        }

        Some(root.into_any_element())
    }
}

impl Render for MessageRow {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors;

        div()
            // Records this element's bounds under a selector the headless test
            // can look up. A *static* selector, deliberately: `debug_bounds`
            // takes `&'static str`, so a per-message key could never be queried.
            // The spike's finding 2 is why this call has to be here at all —
            // `.id()` alone records nothing.
            .debug_selector(|| "message-row".to_owned())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(self.bubble_color())
            // A selection rule, always present and always this wide.
            //
            // **The width is unconditional and only the colour varies, and that
            // pairing is the whole reason this is safe.** `border_l_2` on every
            // row costs two pixels on every row, once; a rule whose width came
            // and went with the cursor would move every row below it on each
            // arrow key, in a virtualized list whose layout is measured exactly
            // once per row. The colour answer is
            // [`MessageRow::selection_rule_color`], called here rather than
            // inlined so the test that asserts it asserts the value this frame
            // paints rather than a copy of the rule.
            .border_l_2()
            .border_color(self.selection_rule_color())
            // Explicit, per AGENTS.md 7.3.
            .text_color(colors.text)
            .child(self.header())
            .child(markdown::document(&self.spec.document, colors, window))
            .when_some(self.reactions(), |row, reactions| row.child(reactions))
            .when_some(self.delivery_badge(), |row, badge| row.child(badge))
    }
}

/// One `Entity<MessageRow>` per `client_msg_id`, bounded and recycled.
///
/// See the module docs for why this is a map of entities rather than a map of
/// specs. The cache is owned by
/// [`MessageList`](super::message_list::MessageList), which is the only caller,
/// and is public so that the recycling rule can be tested without a window.
pub struct RowCache {
    /// The live rows, by the identity that survives an ACK.
    rows: HashMap<Uuid, Entity<MessageRow>>,
    /// Insertion order, which is also the eviction order. A `VecDeque` because
    /// eviction is `pop_front` and insertion is `push_back`, both O(1).
    order: VecDeque<Uuid>,
    /// The list these rows belong to, handed to every row the cache builds.
    ///
    /// **The cache is what builds rows, so the cache is what tells them who their
    /// owner is.** Keeping it here rather than threading it through
    /// [`RowCache::row`] as a parameter means the recycling rules stay testable
    /// with a cache and nothing else, and it is the reason a caller has to *name*
    /// an owner at construction instead of getting an inert cache by default —
    /// see the removed `Default` impl below.
    owner: WeakEntity<MessageList>,
}

impl RowCache {
    /// An empty cache whose rows belong to `owner`.
    ///
    /// **There is no `Default` impl, and work unit 3D removed one.** `Default`
    /// would have to invent an owner, and the only handle it can invent is
    /// `WeakEntity::new_invalid()` — a cache whose rows draw working controls that
    /// dispatch nowhere. That is the worst of both worlds: it passes every
    /// structural check and behaves like a bug. Naming the owner here makes the
    /// dependency explicit, and a test that only wants to exercise eviction can
    /// say so with `WeakEntity::new_invalid()` in full view of what it is doing.
    pub fn new(owner: WeakEntity<MessageList>) -> Self {
        Self {
            rows: HashMap::new(),
            order: VecDeque::new(),
            owner,
        }
    }

    /// How many rows are held. Never more than [`MAX_RETAINED_ROWS`].
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the cache holds no rows.
    ///
    /// Required by `clippy::len_without_is_empty`, and true exactly when
    /// [`RowCache::len`] is zero.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row for `client_msg_id`, if it is still held.
    pub fn get(&self, client_msg_id: &Uuid) -> Option<Entity<MessageRow>> {
        self.rows.get(client_msg_id).cloned()
    }

    /// The row for `spec`, created if absent and updated if its content changed.
    ///
    /// Returns the row and whether it changed, which is what
    /// [`MessageRow::set`] reports and what the list turns into a remeasure.
    /// A freshly created row counts as changed: it has never been measured.
    ///
    /// **`colors` is pushed into an existing row as well as read for a new one,
    /// and that is not a detail.** A row keeps its own palette, so a caller that
    /// replaced the list's palette without this would repaint the container and
    /// leave every recycled row on the previous theme — the failure `AGENTS.md`
    /// §7.3's rule is about and the one no structural assertion can see. The two
    /// change reports are folded with `||` because the list's answer to both is the
    /// same one: remeasure this row.
    pub fn row(
        &mut self,
        spec: RowSpec,
        colors: Colors,
        cx: &mut App,
    ) -> (Entity<MessageRow>, bool) {
        let key = spec.client_msg_id;

        if let Some(existing) = self.rows.get(&key) {
            let changed = existing.update(cx, |row, cx| {
                // Both updates must run, so the fold is a bitwise or rather than
                // `||`: they are independent writes, and short-circuiting would
                // silently drop the second one whenever the first reported a
                // change -- which is exactly the frame a theme switch happens on.
                let recoloured = row.set_colors(colors, cx);
                let restated = row.set(spec, cx);
                recoloured | restated
            });
            return (existing.clone(), changed);
        }

        let row = cx.new(|_| MessageRow::new(spec, colors, self.owner.clone()));
        self.order.push_back(key);
        self.rows.insert(key, row.clone());
        self.evict();
        (row, true)
    }

    /// Drops the oldest rows until the bound holds again.
    ///
    /// **The row being inserted is safe by construction:** it was pushed to the
    /// back, and this pops from the front. The bound is therefore a real upper
    /// bound rather than "one more than the bound while a row is being built".
    fn evict(&mut self) {
        while self.rows.len() > MAX_RETAINED_ROWS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.rows.remove(&oldest);
                }
                // Unreachable while `order` and `rows` are kept in step, and a
                // `break` rather than an `expect` per AGENTS.md 2.1: a cache that
                // stops evicting is bounded-wrong, while a panic here is a crash
                // in the frame path with nothing to recover it.
                None => break,
            }
        }
    }
}
