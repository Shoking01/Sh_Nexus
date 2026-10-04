//! What the connection is doing, said out loud — or nothing at all.
//!
//! `AGENTS.md` §3.3 requires a network failure to surface as a recoverable state
//! rather than as silence, and until this module existed nothing rendered one: the
//! socket fails in a worker thread, a `tracing::error!` goes to a subscriber, and
//! the composer simply refuses to send. **A user who cannot send could not tell
//! why**, which is the gap this closes.
//!
//! # Two questions, and they are not the same question
//!
//! | Question | Answered by | Why it cannot be the other one |
//! |---|---|---|
//! | **Is there a banner at all?** | `Shell::transport` — *a connection was attempted* | a client launched with no `SH_NEXUS_URL` is in **local mode by design**; [`ConnectionSettings::from_env`] treats both variables absent as offline rather than as a misconfiguration, so a banner there would be a permanent red strip apologising for a choice the user made |
//! | **What does it say?** | `AppState::connection` — the real [`ConnectionState`] | that is the only place the answer is maintained: `DomainEvent::ConnectionStateChanged` → `actions::apply_event` → `AppState::set_connection` |
//!
//! **Merging them is the specific mistake this module's shape exists to prevent.**
//! The obvious single rule — *"draw whatever the state says"* — makes the offline
//! shell permanently broken-looking, and the other obvious rule — *"draw a banner
//! whenever a transport was started"* — makes a healthy client permanently
//! decorated. `Shell::render` asks the first question and this module answers the
//! second, and each has exactly one owner.
//!
//! # Rendering nothing when connected is the design, not an omission
//!
//! | State | Drawn | Colour |
//! |---|---|---|
//! | `Connected` | **nothing at all** | — |
//! | `Disconnected` | `Disconnected` | `text_muted` |
//! | `Connecting` | `Connecting…` | `accent` |
//! | `Reconnecting { attempt }` | `Reconnecting (attempt N)…` | `accent` |
//! | `Rejected { code, detail }` | the server's `detail`, with `code` | `danger` |
//!
//! **The healthy state deserves no pixels.** `Connected` is the state a user spends
//! their day in; a client that permanently displays its own health is a client with
//! a permanent distraction, sitting in the strip between the log and the composer
//! where the eye goes to read. It also buys the strongest test available: *"no
//! banner while connected"* is one assertion that fails loudly the moment somebody
//! adds one.
//!
//! # Why this is a function and not an `Entity<MessageRow>`-shaped view
//!
//! **Because `Render` cannot return nothing.** `Render::render` returns
//! `impl IntoElement`; there is no `Option`, so a view that is composed into the
//! tree unconditionally *always* contributes an element — an empty one, at whatever
//! height its padding gives it — and "renders nothing when connected" becomes
//! unobservable from the painted frame. [`banner`] returns [`Option<AnyElement`]
//! instead, and `Shell::render` adds it with `when_some`, so a connected client
//! genuinely has no such element in the tree. `tests/app_shell.rs` asserts that
//! through `debug_bounds`, which asks the window and therefore only answers for
//! elements that were painted.
//!
//! The precedent is [`crate::ui::markdown`]: `document()` returns an `AnyElement`
//! and `blocks()` a `Div`, and the reason each is split out is that an element is
//! opaque to a caller — a selector is what makes it findable, and a returned value
//! is what makes its content assertable.
//!
//! **The cost, stated: this view is rebuilt by the shell's frame rather than
//! cached by GPUI.** That is the right trade for a strip that is either one line
//! tall or absent, and it costs the *shell* a repaint on ticks that changed the
//! state — see `Shell::apply_inbox`. An entity would have inverted that: the
//! banner could repaint itself without the shell, but it could never be absent.
//!
//! # `Rejected` speaks the server's words, and that is load-bearing
//!
//! `ConnectionState::Rejected`'s own documentation says its code and detail are
//! *"shown to the user, because 'the connection failed' with no reason is
//! unactionable"*, and `AGENTS.md` §5.2 asks for a *"clear, actionable error"*. So
//! the text here is the server's `detail` with the machine-readable `code` beside
//! it, **not** a string of this client's own — a generic `"the connection failed"`
//! would be precisely the failure the enum's payload exists to prevent, and it is
//! the one outcome that cannot be fixed by anything the user does.
//!
//! **Detail leads and the code follows in parentheses, which is the order that
//! makes the two do different jobs.** `detail` is the sentence the user acts on;
//! `code` is the handle they can quote in a bug report to someone who has never
//! read this file, which is why it is not dropped even though it is the less
//! readable half. `tests/app_shell.rs` asserts both are present, and that the text
//! is the *server's* text rather than a paraphrase.

use gpui::{div, prelude::*, AnyElement, Hsla};

use crate::core::models::events::ConnectionState;
use crate::ui::Colors;

/// The debug selector the banner records its bounds under.
///
/// **A *static* selector, for the reason `message_row.rs`'s badge selector is
/// static:** `debug_bounds` takes `&'static str`, so a selector keyed on the
/// connection state could never be queried, and a test that wanted to look up
/// "the reconnecting banner" would have to know its spelling to ask. There is at
/// most one banner in the window, so the name has nothing to disambiguate.
///
/// A `debug_selector` and not an `.id(..)`: `message_row.rs` records that
/// *".id() alone records nothing"*, and `ElementId` is state identity rather than a
/// debug hook.
pub const SELECTOR: &str = "connection-banner";

/// The word shown while no socket is up and none is being attempted.
///
/// **Plain and unpunctuated, because `Rejected` is what carries punctuation in this
/// view.** A banner reading `Disconnected.` or `Disconnected — retrying` would be
/// claiming something this client cannot know: whether anything will retry is
/// `network/reconnect.rs`'s decision, and this layer does not import it
/// (`tests/layer_boundary.rs` forbids `ui/` from naming `crate::network`).
const DISCONNECTED: &str = "Disconnected";

/// The word shown while a first attempt is in flight.
///
/// **`Connecting…` and not `Reconnecting…`, and the difference is a payload the
/// enum carries.** `ConnectionState::Connecting` is documented as *"the first
/// attempt after a disconnect; subsequent ones are `Reconnecting`, so the UI can
/// distinguish 'connecting' from 'retrying' and say so"* — so collapsing the two
/// would throw away the distinction the state machine exists to make.
const CONNECTING: &str = "Connecting\u{2026}";

/// The word shown while an attempt after a failure is in flight, before the
/// attempt number.
const RECONNECTING: &str = "Reconnecting";

/// What a frame would say about the connection, and in which colour.
///
/// **A value rather than a rendered element, and the reason is that an
/// `AnyElement` is opaque.** A test can hold one and learn nothing about the text
/// inside it; `debug_bounds` proves an element was *painted* but not what it said.
/// This is the split [`crate::ui::markdown::runs`] makes for the same reason —
/// return the text and its styling as data so the arithmetic is assertable, and
/// paint the element separately so the paint is assertable too.
/// `tests/app_shell.rs` asserts both halves of every state.
///
/// `color` rather than the palette key it came from: the rule being honoured is
/// `AGENTS.md` §7.3's, that a text element sets its own colour explicitly, so what
/// a test needs to check is the colour that element draws with.
#[derive(Clone, PartialEq)]
pub struct Announcement {
    /// Exactly the string the banner draws. No prefix, no elision, no redaction.
    pub text: String,
    /// The colour that text is drawn in.
    pub color: Hsla,
}

/// What `state` means to the user, or `None` when it means nothing.
///
/// **`None` is `Connected` and only `Connected`.** A healthy connection is the one
/// state with no news, so the honest rendering of it is no element; every other
/// variant carries something the user cannot work out alone — including
/// `Disconnected`, which is the state a client with no server configured is in and
/// which is *still* worth saying, because [`crate::app::Shell`] has already
/// established that a transport was started (see the module docs on the two
/// questions).
///
/// **The colour is chosen per state rather than derived from severity, and the
/// `danger`/`accent` split is the load-bearing half.** `Rejected` is `danger`
/// because the enum documents it as terminal — retrying cannot succeed — so a
/// red banner is the accurate reading. `Connecting` and `Reconnecting` are
/// `accent`, which is this palette's *interactive* colour and the same one
/// `message_row.rs` draws its retry affordance in: progress is not failure, and a
/// retry banner painted `danger` would report a failure that has not happened.
/// `Disconnected` is `text_muted`, which is the quietest colour the palette has and
/// the right one for a state that is about to become `Reconnecting`.
pub fn announcement(state: &ConnectionState, colors: Colors) -> Option<Announcement> {
    let (text, color) = match state {
        // The one state with no news. See the module docs on why this is a design.
        ConnectionState::Connected => return None,

        ConnectionState::Disconnected => (DISCONNECTED.to_owned(), colors.text_muted),
        ConnectionState::Connecting => (CONNECTING.to_owned(), colors.accent),

        // The attempt number is carried by the enum *for this view* — "so the UI
        // can show it, and so the backoff's max-attempt rule is observable rather
        // than invisible" — so a banner that dropped it would leave the backoff
        // invisible again. It counts from 1, and a client sitting on attempt 4 is a
        // different situation from a client on its first try.
        ConnectionState::Reconnecting { attempt } => (
            format!("{RECONNECTING} (attempt {attempt})\u{2026}"),
            colors.accent,
        ),

        // The server's words, not ours. See the module docs on `Rejected`.
        ConnectionState::Rejected { code, detail } => (format!("{detail} ({code})"), colors.danger),
    };

    Some(Announcement { text, color })
}

/// The banner element for `state`, or `None` when there is nothing to say.
///
/// **Total over every state and free of any precondition, because presence is not
/// this function's business.** `Shell::render` asks *whether* to show a banner from
/// `Shell::transport`, which knows whether a connection was attempted; this function
/// answers only *what to write*. Folding the two together is the mistake the module
/// docs name, and it would show up as an offline client permanently apologising for
/// a configuration the user chose.
///
/// The element is a single flex row with explicit padding rather than a bare `div`,
/// because it sits between the log and the composer: without vertical padding the
/// strip would touch both, and a rejection the user has to read carefully deserves
/// room. **The background is not set**, deliberately — this strip is an annotation
/// on the chat area rather than a panel, and `Colors::surface` would make it read as
/// a third region in the column. Every text colour is explicit per `AGENTS.md` §7.3,
/// on the container and on the child, because GPUI inherits nothing.
pub fn banner(state: &ConnectionState, colors: Colors) -> Option<AnyElement> {
    let announced = announcement(state, colors)?;

    Some(
        div()
            .debug_selector(|| SELECTOR.to_owned())
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .py_2()
            .text_sm()
            // Explicit on the container as well as on the text below, for the same
            // reason `input_bar.rs` sets it twice: AGENTS.md 7.3 is unconditional
            // and satisfying it everywhere is cheaper than explaining which
            // elements would have inherited it.
            .text_color(announced.color)
            .child(div().text_color(announced.color).child(announced.text))
            .into_any_element(),
    )
}
