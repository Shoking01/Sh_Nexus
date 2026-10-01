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

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::{div, prelude::*, AnyElement, App, Context, Entity, Render, Window};
use uuid::Uuid;

use crate::core::markdown::Document;
use crate::state::DeliveryState;
use crate::ui::{markdown, Colors};

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
    pub fn differs_from(&self, other: &RowSpec) -> bool {
        self.client_msg_id != other.client_msg_id
            || self.author != other.author
            || self.is_self != other.is_self
            || self.timestamp != other.timestamp
            || self.delivery != other.delivery
            || self.failure_detail != other.failure_detail
            || self.reactions != other.reactions
            || !Arc::ptr_eq(&self.document, &other.document)
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
}

impl MessageRow {
    /// Builds a row for `spec`.
    ///
    /// No `Context` is taken: a row has no state of its own beyond its spec, so
    /// there is nothing to register and nothing to focus.
    pub fn new(spec: RowSpec, colors: Colors) -> Self {
        Self { spec, colors }
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

        Some(
            div()
                .text_sm()
                // Explicit, per AGENTS.md 7.3.
                .text_color(color)
                .child(label)
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
        let is_self = self.spec.is_self;

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
            .bg(if is_self {
                colors.bubble_self
            } else {
                colors.bubble_other
            })
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
}

impl RowCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self {
            rows: HashMap::new(),
            order: VecDeque::new(),
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

        let row = cx.new(|_| MessageRow::new(spec, colors));
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

impl Default for RowCache {
    /// An empty cache, so a caller that needs no configuration can take it.
    fn default() -> Self {
        Self::new()
    }
}
