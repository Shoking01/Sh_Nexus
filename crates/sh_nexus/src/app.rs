//! The application shell: root component, global-state owner, theme provider,
//! key handling. `PLAN.md` §4's four items, and nothing else.
//!
//! # 1. The four obligations, and where each one is discharged
//!
//! | `PLAN.md` §4 names | Discharged by |
//! |---|---|
//! | root component | [`Shell`]'s `Render` impl |
//! | global state | [`open`], through [`bridge::install`] |
//! | theme provider | [`theme_colors`], handed to the list and the composer at construction |
//! | key handling | `Shell::on_key_down` for the `Escape` ladder, and `InputBar::on_key_down` for the field's two keys |
//!
//! **The list is short because every obligation that could have been written as
//! a fifth thing here has a named owner somewhere else**, and duplicating an
//! owner is how a boundary stops being one. `AGENTS.md` §3.2 gives this layer no
//! business logic, `ui/` owns every gesture, and `state/` owns every decision
//! about what the state means — so the shell's own logic is the schedule and the
//! palette, and nothing else.
//!
//! ## The global-state half is a call, not a context mutation
//!
//! `PLAN.md` §4 and `AGENTS.md` §3.2 name `state/bridge.rs` the single owner of
//! the context-mutating GPUI API, and
//! `bridge_is_the_only_file_that_calls_the_context_mutating_api` in
//! `crates/sh_nexus/tests/bridge.rs` fails the build on a second caller anywhere
//! under `src/` — this file included. So [`open`] installs the state by calling
//! the seam's own installer and never reaches for the API itself.
//!
//! # 2. Why the shell holds the producer handle, and the bug that fixed
//!
//! [`bridge::install`] returns an [`EventSender`], and **a dropped sender closes
//! the inbox**: the receiving end reports `Disconnected`, and every later
//! `deliver` answers [`bridge::DeliveryRefusal::BridgeDropped`]. Until this file
//! existed, `src/lib.rs::run` installed the state and discarded the handle
//! immediately, on the reasoning that `network/` has no producer yet. That
//! reasoning was wrong in a way that cost nothing until it would have cost
//! everything: the first work unit to hand this sender to a socket task would
//! have found a client that silently discards every message it receives, with
//! the refusal visible only to the producer.
//!
//! **So the handle is a field of [`Shell`], and that is the whole fix.** It lives
//! as long as the shell does, which is as long as the window does, which is as
//! long as the application does. `network/` will take a *clone* when it lands —
//! [`EventSender`] is `Clone` by design (`bridge.rs` §2) so several producers may
//! share one inbox — and the shell's own handle is what stops the last producer
//! from closing an inbox that still has a consumer.
//!
//! # 3. The drain schedule, and why it is a timer and not a read
//!
//! [`bridge::drain`] is synchronous, and that module's own docs, §6, assign the
//! schedule to the caller. **A shell that never drains renders a channel that
//! never updates**: events queue, the bound is enforced, and nothing the user can
//! see changes. So the schedule is this file's, and it is a repeating
//! [`DRAIN_INTERVAL`] timer.
//!
//! | The alternative | Why it is rejected here |
//! |---|---|
//! | drain from inside `Render` | a frame is not a scheduler, and mutating the state a frame is drawing is re-entrancy on the frame path (`AGENTS.md` §7.3) |
//! | a worker thread with a blocking `recv` | `Context` is `!Send` (`docs/ARCHITECTURE.md` ADR-009), so the state cannot be touched off the main thread at all — and `AGENTS.md` §2.3 forbids blocking the frame loop regardless |
//! | an async channel with an `AsyncApp` wakeup | the right end state, and not available: `bridge.rs` §3's channel is `std::sync::mpsc` with `try_recv`, which has no wakeup to bridge. Phase 4's socket task replaces the poll |
//!
//! **So the pump costs one timer wakeup per [`DRAIN_INTERVAL`] for the life of
//! the process, and that cost is stated rather than hidden.** A tick whose inbox
//! is empty does exactly one non-blocking `try_recv` that answers `Empty`.
//! `AGENTS.md` §6.2's idle-CPU row is `< 2%`, and twenty such polls a second sit
//! three orders of magnitude inside it.
//!
//! **The interval is half the end-to-end budget, and derived from it.**
//! `AGENTS.md` §6.2 puts end-to-end delivery at `< 100 ms`; half of that is the
//! event's wait for a tick and half is the network plus the frame, so
//! [`DRAIN_INTERVAL`] spends the first half and no more. A shorter interval buys
//! latency the budget never asked for and pays a wakeup for it.
//!
//! **The pump ends with the shell rather than with the process.** `cx.spawn`
//! hands the future a weak handle to the shell, and the loop returns when that
//! handle can no longer be upgraded. Holding a strong [`Entity`] instead would be
//! a cycle: the pump would keep itself — and the message list it repaints — alive
//! after the window that owned them closed, which `AGENTS.md` §7.1's ban on
//! unbounded in-memory growth exists to prevent.
//!
//! # 4. The theme, and why there is no hot reload here
//!
//! [`crate::core::theme`] parses and validates and nothing else — `AGENTS.md`
//! §3.2 forbids it calling GPUI — so *applying* a theme is this file's job by
//! construction rather than by choice. [`theme_colors`] resolves
//! [`ACTIVE_THEME`] into a [`Colors`], and the shell hands that to the list once,
//! at construction.
//!
//! **`AGENTS.md` §10.2 also asks for hot reload and for discovery under
//! `~/.config/sh_nexus/themes/`, and neither is here.** Both need
//! `platform/file_watch.rs` and the `notify` dependency, neither of which exists,
//! and `AGENTS.md` §7.2 makes a dependency a recorded decision rather than one to
//! smuggle in alongside a feature. A "reload" that can never fire because nothing
//! watches anything is exactly the dead code `ui/views/mod.rs` refuses to write.
//!
//! **The one affordance that *is* here is `MessageList::set_colors`, and it is
//! here rather than deferred for a reason:** the palette is applied once at
//! construction, so without a setter the theme-provider obligation would be
//! unfulfilled even in the shape that has no runtime change to serve — and a
//! setter that silently fails to reach the recycled rows would be a documented
//! lie about `AGENTS.md` §7.3's rule. `tests/app_shell.rs` asserts the palette
//! reaches the rows, not only the container.
//!
//! # 5. What this file deliberately does not contain
//!
//! `PLAN.md` §6 asks for *"App shell: sidebar + chat area + input bar"*, and the
//! input bar now exists — [`crate::ui::views::input_bar::InputBar`], composed in
//! [`Render`] and focused by [`open`]. **It is a separate view rather than a
//! section of this file, and the reason is the one `bridge.rs` §5 argues
//! generally: a second place to do something is the audit trail, and a composer
//! held here would be a second place to answer "what does Enter do".**
//!
//! The other two of §6's three are still not constructible today, verified
//! rather than assumed.
//!
//! | Missing | Why it is not here |
//! |---|---|
//! | the channel rail | `actions::set_channels` has zero callers because no `DomainEvent` carries a channel list (`bridge.rs` §5), so a rail would render zero channels, permanently |
//! | thread panel, search, editing | `docs/ARCHITECTURE.md` ADR-006 places them outside this work |
//! | a log line carrying the source chain of a failed open | `errors.rs` assigns this file one, and `main.rs` now has a subscriber to receive it — but a GPUI `anyhow::Error`'s chain is a foreign type and `errors.rs` rule 1 forbids the `From<anyhow::Error>` that would name it, so the reduction below is the half that can be honoured without a new error variant |
//!
//! **The composer is not listed among the missing things any more, because
//! `AGENTS.md` §3.2's rule is that a module that exists and does nothing reads
//! as finished work** — and the flip side is that a row claiming a feature is
//! deliberately withheld has to be removed when it is not. It was there in the
//! draft of this file that first composed the list, and the input-bar work unit
//! deleted it for exactly that reason.
//!
//! # 6. What the user sees about the connection, and where
//!
//! **One line, between the log and the composer, and it is a view rather than a
//! branch of this file** for the reason §5 gives: a second place to answer "what
//! does the connection look like" is the audit trail. It is
//! [`crate::ui::views::connection_banner`], and the one thing this file owns about
//! it is **whether there is one**: [`Self::transport`] says whether a connection was
//! ever attempted, which is a question about this shell and not about the state.
//! `ui/` is not permitted to reach for the shell's field, and the seam has no
//! opinion on whether a socket was started — so the decision has to be made here,
//! and the two must not be merged. See that module's docs for the other half.
//!
//! **The shell stays silent about it, which is the point.** `AGENTS.md` §7.1 bans
//! `println!` in production and §7.5 assigns levels to `tracing`; a client that
//! printed its connection state into a terminal would be a client reporting to
//! whoever launched it rather than to the person looking at it. The banner is the
//! user-facing surface, and `main.rs`'s subscriber is for the developer's terminal.
//!
//! # 7. The login mode, and why it is this file's decision
//!
//! **A window shows the form *instead of* the chat while there is no session, and
//! this file is the only one that can say so.** `ui/views/login.rs` owns what the
//! form looks like and §5's argument is why the form is not a third branch of
//! this file; but *"which of two things is this window"* has one owner, and
//! splitting it would be the audit trail §5 warns about. [`Self::login_form`] is
//! the answer, [`Render`] reads it, and `login::LoginView` is handed an
//! [`crate::ui::Colors`] it never asks this file about.
//!
//! **Why the chat is not merely hidden behind the form.** `tests/login.rs`
//! asserts that `debug_bounds("message-list")` is `None` while the form is up — a
//! window showing a chat a person has no claim on is a window telling them they
//! are already signed in. That is a claim about the *element tree*, so it is made
//! by an early return rather than by a conditional child.
//!
//! ## The startup matrix has six cases, and the fifth one is this feature
//!
//! [`Startup`] is that matrix as a value, and it exists because the decision used
//! to be a `match` inside [`open`]'s window closure where no test could reach it.
//! `startup_from_env` is the same decision, moved somewhere a test can call it.
//!
//! | `SH_NEXUS_URL` | `SH_NEXUS_TOKEN` | Result |
//! |---|---|---|
//! | absent | absent | [`Startup::Local`] — the offline shell, unchanged |
//! | present | absent | [`Startup::Resume`] — the keychain has not answered yet |
//! | present | present | [`Startup::Connected`] — the shell, unchanged |
//! | absent | present | [`MissingSetting::Url`] |
//!
//! **The rule it encodes: a login screen appears when a server is configured and
//! you have no session.** No server configured is not a failure to log in to; it is a
//! different mode that already works and that the offline suites depend on.
//! **The other four cases are byte-for-byte what they were**, which is why
//! `ConnectionSettings::from_env` still exists with its stricter four-case
//! reading for the callers that have no form to offer.
//!
//! ## The startup phase, and why there is one
//!
//! **The keychain made the matrix a phase rather than a decision.** Whether a launch
//! shows a chat or a form now depends on a credential store that has not been read,
//! and reading one is a blocking operating-system call — `CredReadW`, the Keychain
//! Services call, a Secret Service round trip — which §2.3 forbids on the main thread.
//! So [`Startup::Resume`] is a value that says *ask*, [`resume_with`] is the same
//! decision with the answer supplied, and [`Shell::resuming`] is the window drawing
//! neither a chat nor a form while it waits.
//!
//! **The waiting is a poll on the shell's existing pump, not a second mechanism.**
//! [`Self::apply_resume`] is the first and ungated thing [`Self::apply_inbox`] does,
//! for exactly the reason [`Self::apply_login`] is: the answer arrives on its own
//! channel with the event inbox silent, so gating it behind `applied() == 0` would
//! leave a window stuck in the startup phase until the socket happened to say
//! something.
//!
//! **The cost is stated rather than hidden: the worst case is one
//! [`DRAIN_INTERVAL`] after launch**, because that is the pump's period. §6.2's
//! cached-session budget is 300 ms and this spends at most 50 ms of it.
//!
//! ## A restored session that the server refuses must not lock the user out
//!
//! **The trap this design exists to avoid.** If a launch restores a session and the
//! server refuses it, an app that treats "I have a session" as final shows a chat
//! that can never connect and **never offers the login form** — the user is stuck
//! with no way back except deleting the credential by hand. So
//! [`Self::recover_from_a_rejected_restored_session`] stops the socket, drops the
//! session, clears the store and opens the form, in one step.
//!
//! **Only a *restored* session takes that path**, and [`Self::restored`] is what
//! makes the difference checkable rather than a guess. A session from the
//! environment or from a sign-in the user just typed is not cleared on refusal: the
//! first is an operator's configuration and the second is a credential they can see,
//! and deleting either without being asked would be a destructive action with no
//! confirmation. That asymmetry is the cost of not recovering on every refusal, and
//! it is stated here rather than discovered.
//!
//! ## The poll is first, ungated, and the only thing that can end an attempt
//!
//! [`Self::apply_login`] runs **before** the drain and **before** the flush, and
//! it is not behind the `report.applied() == 0` early return. That early return is
//! an optimisation about *frames* — twenty quiet ticks a second should cost no
//! repaints — and a login answer is not a frame event: it arrives on its own
//! channel with the event inbox silent, which is exactly the state
//! `a_login_completes_with_the_event_inbox_completely_silent` drives. Gating the
//! poll on applied events would have made a successful sign-in wait for the socket
//! to say something.
//!
//! ## One attempt, one socket, and the guard that makes it true
//!
//! [`Self::enter_login`] refuses while a form exists or a transport is held, and
//! [`Self::accept_login`] drops the form and starts the socket in one step. So
//! there is no state in which a form can be re-entered and a second socket
//! started, and the answer that ends it is taken by the poll rather than by the
//! view — [`bridge::try_take_login_outcome`] is the only caller, which is what
//! makes `bridge::login_in_flight` deterministic rather than a race against the
//! pump.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, App, AsyncApp, Bounds, Context, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Render, Window, WindowBounds, WindowHandle, WindowOptions,
};

use crate::core::models::events::ConnectionState;
use crate::core::theme::BuiltIn;
use crate::errors::ShNexusError;
use crate::network::ws::{TransportConfig, TransportError, WsTransport};
use crate::platform::{LazyNativeStore, NoopTokenStore, TokenStore};
use crate::state::bridge::{self, EventSender, LoginOutcome};
use crate::ui::views::connection_banner;
use crate::ui::views::input_bar::InputBar;
use crate::ui::views::login::LoginView;
use crate::ui::views::message_list::MessageList;
use crate::ui::Colors;
use crate::UNSIGNED_IN_USER;

/// Width of the shell's window, in logical pixels.
///
/// **The same viewport ADR-006 step 6 measured, deliberately.**
/// `benches/frame_time.rs` sizes its own window to this and says why: rows per
/// frame — and therefore any frame-time figure — scale with viewport height, so a
/// differently-sized window would make a re-measurement of `docs/BASELINES.md`'s
/// app-level row incomparable with the floor already recorded. The spike's
/// 480x320 is the number that bench's `WINDOW_HEIGHT` rejected in writing, and
/// repeating it here would repeat that mistake. **The equality is now checked by
/// the compiler** — a `const` assertion in the bench fails the build if these two
/// ever diverge — because a doc sentence is not a guarantee.
pub const WINDOW_WIDTH: f32 = 1024.0;

/// Height of the shell's window, in logical pixels. See [`WINDOW_WIDTH`].
pub const WINDOW_HEIGHT: f32 = 768.0;

/// How long the shell waits between two drains of the event inbox.
///
/// **Derived from `AGENTS.md` §6.2's end-to-end budget rather than picked.** That
/// row is `< 100 ms`; half of it is the arriving event's wait for the next tick
/// and half is the network plus the frame, so 50 ms spends the first half and no
/// more. See the module docs, §3, for what the poll costs and what it replaces.
pub const DRAIN_INTERVAL: Duration = Duration::from_millis(50);

/// The channel the shell shows when it opens.
///
/// **This was `c_startup`, a self-declared placeholder, and it was wrong the moment
/// a server existed.** The shell names this channel in every `message.send`, because
/// `MessageList::begin_send` fills `channel_id` from the channel the list was shown,
/// and the server refuses a send naming a channel it has not seeded
/// (`Store::accept_message` returns `UnknownChannel`). A placeholder id that no
/// server has ever heard of therefore fails at runtime, not at build time: PR #43
/// wired the socket and the first message typed into the window was rejected.
///
/// **The id is spelled here rather than imported, deliberately.** `sh_nexus` cannot
/// depend on `sh_nexus_server` -- that would put `axum` and `rusqlite` into the
/// client's dependency graph -- and the shared `sh_nexus_wire` crate is the wrong
/// home for it too: a protocol crate should describe frame shapes, not the name a
/// particular deployment gave its first channel. `tests/ws_transport.rs` reaches the
/// same conclusion for its own copy of this literal and says why. Two literals and a
/// test that fails when they disagree beats one constant that hides the coupling.
///
/// **What enforces the agreement, since nothing does by construction:**
/// `the_shell_s_channel_is_one_the_server_accepts` in `tests/ws_transport.rs` sends
/// through this very constant against a real server process and requires a
/// `message.ack`. A rename on either side turns that test red instead of turning
/// production red.
///
/// **What is still a placeholder, and stated rather than hidden:** *which* channel to
/// open. Nothing populates the channel list -- `actions::set_channels` has no door
/// until a `DomainEvent` carries one (`bridge.rs` §5) -- so this is the seeded
/// channel, not a chosen one, and `PLAN.md`'s channel rail arrives after Phase 4
/// supplies the data. The unread badge is over-reported while
/// [`crate::UNSIGNED_IN_USER`] is the author, which is the safe direction.
pub const STARTUP_CHANNEL: &str = "c_general";

/// The built-in theme the shell applies.
///
/// **A constant rather than a setting, and the reason is that §10.2's other half
/// cannot exist yet.** A user-chosen theme needs discovery and hot reload, both
/// of which need `platform/file_watch.rs` (see the module docs, §4), so the only
/// honest value here is the one compiled into the binary. It is public because
/// `tests/app_shell.rs` asserts the applied palette *is* this theme's, which is a
/// statement about the wiring rather than about a colour value.
pub const ACTIVE_THEME: BuiltIn = BuiltIn::Dark;

/// The key [`Shell::on_key_down`] acts on, as `gpui`'s keystroke parser spells it.
///
/// **Named for the key rather than for the effect, and that is a correction.** It
/// was `RETURN_TO_TAIL_KEY`, which was honest while the handler did one thing.
/// The handler now does two — the log returns to the newest message *and* the log
/// takes the keyboard — and a constant named after half of it is the kind of name
/// that makes the next reader look for a second handler that does not exist.
const ESCAPE_KEY: &str = "escape";

/// Resolves [`ACTIVE_THEME`] into the colours every element in the shell's tree
/// draws with.
///
/// **This is the "apply" half of `core/theme.rs`, and it lives here because
/// applying means calling GPUI** — `AGENTS.md` §3.2 forbids that in `core/`, and
/// `core/theme.rs`'s own module docs say so in the same words.
///
/// **The fallback arm exists so this cannot fail, not because it is expected.**
/// `BuiltIn::theme` returns a `Result` because a build-time fixture can be
/// corrupted by a bad merge, and `AGENTS.md` §2.1 forbids the `unwrap` that would
/// otherwise be the obvious response. Returning a `Result` instead would push the
/// decision onto every caller — including the frame path — and there is no caller
/// for which "no colours" is a useful answer. `tests/theme.rs`'s
/// `every_built_in_theme_parses_and_validates` is what keeps the document `Ok`,
/// so the arm is unreachable in a shipped binary and is the honest shape for a
/// path that must not be allowed to panic.
pub fn theme_colors() -> Colors {
    match ACTIVE_THEME.theme() {
        Ok(theme) => Colors::from_palette(theme.colors()),
        Err(_) => Colors::dark_fallback(),
    }
}

/// The environment variable naming the server's WebSocket endpoint.
///
/// **A constant rather than a literal in two places, and the reason is the module
/// docs' §7 argument rather than tidiness:** the URL is read by two functions —
/// [`ConnectionSettings::from_env`] and [`startup_from_env`] — and the login URL is
/// derived from the value it holds. One spelling of the variable's name is one
/// thing to get right.
const URL_VARIABLE: &str = "SH_NEXUS_URL";

/// The environment variable carrying the session token.
///
/// See [`URL_VARIABLE`] for why this is a constant.
const TOKEN_VARIABLE: &str = "SH_NEXUS_TOKEN";

/// The two variables, read once.
///
/// **One reader for both public readings, and that is the point.** There are two
/// functions that answer "what is configured" — [`ConnectionSettings::from_env`]
/// and [`startup_from_env`] — and they differ on exactly one arm. A second
/// `std::env::var` pair would be a second answer to that question and the shape of
/// the mistake `bridge.rs` §5's audit-trail argument is written about.
///
/// **`Option` rather than the `Result` because a missing variable is not an error
/// at this level**: which of the two is missing is what the two callers each answer,
/// and `Err(_)` for "not present" would put a `std::env::VarError` into a type
/// whose whole purpose is to say *which variable*.
fn configured() -> (Option<String>, Option<String>) {
    (
        std::env::var(URL_VARIABLE).ok(),
        std::env::var(TOKEN_VARIABLE).ok(),
    )
}

/// The environment the client reads its server from.
///
/// # The two variables, and what absent means
///
/// | Variable | Meaning |
/// |---|---|
/// | `SH_NEXUS_URL` | The server's WebSocket URL, e.g. `ws://127.0.0.1:8484/ws`. |
/// | `SH_NEXUS_TOKEN` | The session token, sent as `Authorization: Bearer`. |
///
/// **Both absent means no transport is started and the shell runs exactly as it did
/// before this existed** — no error, no banner, no connection state. A developer with
/// no server configured gets the offline shell, which is the point: a client that
/// refuses to start without a server is a client nobody can run for the first time.
///
/// **A URL with no token is two different things depending on who is asking.** A
/// caller with a window to put a form in has one: [`Startup::Login`], and the user
/// signs in. A caller that has nowhere to offer a form has only one: a URL alone
/// can end in an opaque 401 the user cannot act on, so [`from_env`] reports
/// [`MissingSetting::Token`] instead. `AGENTS.md` §7.5 says an actionable code
/// beats a silently dropped explanation, and refusing *before* a window opens says
/// which variable is missing, which is strictly more useful than the same message
/// arriving after the user has typed something.
///
/// # Why the environment and not a file
///
/// `AGENTS.md` §7.1 forbids plaintext token storage and names the OS keychain as where
/// a token belongs, and **`platform/` now exists and does that** — see
/// [`Self::use_credential_store`] and [`Self::remember`]. An environment variable is
/// still one of the two places a credential can arrive from, and it is the only one
/// that needs no operating-system call to read: a developer with a token in their
/// shell gets the shell with no keychain round trip and no startup phase at all.
/// **The token is never logged, never written to a file this project owns, and never
/// rendered into a `Debug` or `Display` output** — [`ConnectionSettings`]'s own
/// `Debug` impl is what enforces that, and a test asserts it.
///
/// **The keychain does not widen this type, and that is deliberate.** A credential read
/// out of the store is built by the *same* [`Self::new`] the environment path uses and
/// reaches the same two fields, so there is one representation of "connected" rather
/// than two that could disagree — `tests/keychain.rs` asserts it, beside `6A`'s own
/// assertion for the two ways the login milestone had.
///
/// This is also the server's own convention, which is why the names match:
/// `crates/sh_nexus_server/src/main.rs` reads `SH_NEXUS_BIND` and `SH_NEXUS_DB` the
/// same way. One project, one way of being configured.
///
/// # What this is not
///
/// It is not multi-account and not a settings screen. It is the wiring that makes the
/// transport reachable from the running application, which is what stopped it being a
/// library only tests could exercise.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionSettings {
    url: String,
    token: String,
}

impl ConnectionSettings {
    /// Builds settings from a URL and a token.
    ///
    /// **A real constructor, and it is public because both ways of becoming
    /// connected have to build the same value.** The environment path gets here
    /// through [`Self::from_env`]; the login path gets here through
    /// [`Startup::Login`] and a server-issued session. **Two constructors would be
    /// two representations of "connected", and `bridge::install_transport`'s
    /// one-transport-one-publication-point argument only holds if there is one way
    /// to be connected** — `tests/login.rs::the_connected_state_has_exactly_one_
    /// representation` is what asserts it rather than leaving it to review.
    ///
    /// **It takes the token and does nothing with it**, which is worth noticing
    /// rather than fixing: this is where the only credential the client keeps comes
    /// into existence, and it is a two-field struct with a hand-written `Debug`
    /// rather than anything with storage.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
        }
    }

    /// Reads both variables, or explains precisely what is missing.
    ///
    /// `Ok(None)` is the **no-server case and is not an error**: the shell runs
    /// offline. `Err` is only ever "you asked for a connection and did not finish
    /// describing it".
    ///
    /// **This is the four-case reading, and it is deliberately still that.** A URL
    /// with no token is [`MissingSetting::Token`] here rather than a login screen,
    /// because this function has no window to put a form in — and a refusal that
    /// arrives before any UI exists is the one place the user can still read the
    /// explanation in a terminal. **The caller that *does* have a window uses
    /// [`startup_from_env`]**, whose fifth case is that same arm answered with a
    /// form instead of an error; see the type's docs. Both read through
    /// [`configured`], so the two spellings of "the server is at this URL" cannot
    /// drift.
    pub fn from_env() -> Result<Option<Self>, MissingSetting> {
        match configured() {
            (None, None) => Ok(None),
            (Some(url), Some(token)) => Ok(Some(Self::new(url, token))),
            (Some(_), None) => Err(MissingSetting::Token),
            (None, Some(_)) => Err(MissingSetting::Url),
        }
    }

    /// The server's WebSocket URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Builds settings from values the caller already has.
    ///
    /// **A thin delegation to [`Self::new`], and the whole of its reason is that
    /// the name is a lie the compiler cannot check.** It is `*_for_test` because
    /// [`Self::from_env`] is untestable for the populated case, and that
    /// untestability is worth recording: reaching the "both variables are set" arm
    /// means calling `std::env::set_var`, which is `unsafe` under Rust 2024 and
    /// mutates state every other test in the process reads. A test that sets an
    /// environment variable to prove something about a constructor is a test that
    /// breaks unrelated ones.
    ///
    /// **It is not a second representation, and that is what this line buys.** The
    /// name survives so no existing caller changes, while the body is the one
    /// constructor — so a caller reading `from_parts_for_test` sees one path into
    /// the value and not two that could drift.
    pub fn from_parts_for_test(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self::new(url, token)
    }

    /// Builds the transport configuration this describes.
    ///
    /// The token goes in here and nowhere else. It is passed to the transport, which
    /// drops it inside `handshake()` — that function is the only place in the
    /// codebase where a credential exists at all, which is what makes a leak
    /// something that would have to be written on purpose rather than arrived at by
    /// refactoring.
    pub fn transport_config(&self) -> TransportConfig {
        TransportConfig::new(&self.url).with_token(&self.token)
    }
}

/// Which half of the connection configuration is absent.
///
/// **A distinct type rather than a `String`,** because the message is the whole point:
/// an operator who set a URL and got "invalid configuration" has to guess, and one who
/// is told `SH_NEXUS_TOKEN` is missing can fix it without reading this source.
///
/// **Both variants are still reachable, and `Token` is reachable from exactly one
/// caller.** [`ConnectionSettings::from_env`] is the four-case reading and reports
/// it; [`startup_from_env`] answers the same arm with [`Startup::Login`] instead,
/// because it has a window to put a form in. **The asymmetry is the feature rather
/// than an oversight** — and it is why this type is not a "configuration is wrong"
/// flag with a login special case bolted on: which answer is right depends on
/// whether the user is looking at a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSetting {
    /// `SH_NEXUS_URL` is set but `SH_NEXUS_TOKEN` is not, and no form was offered.
    Token,
    /// `SH_NEXUS_TOKEN` is set but `SH_NEXUS_URL` is not.
    Url,
}

impl fmt::Display for MissingSetting {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token => formatter.write_str(
                "SH_NEXUS_URL is set but SH_NEXUS_TOKEN is not; the server requires a \
                 session token, so a URL alone can only be refused with an opaque 401",
            ),
            Self::Url => formatter.write_str(
                "SH_NEXUS_TOKEN is set but SH_NEXUS_URL is not; there is no server to \
                 send it to",
            ),
        }
    }
}

impl std::error::Error for MissingSetting {}

/// What this client should open, and why — the six-case matrix as a value.
///
/// **A value rather than a `match` inside [`open`], because a decision expressed as
/// a value can be asserted.** The window closure reads the environment and then
/// decides, and there was no way to ask "what did it decide" without opening a
/// window; `tests/login.rs` asks it directly for the arm that acceptance criterion
/// 4 is about.
///
/// **Four cases and one refusal, and the refusal is asymmetric on purpose.** A
/// token with no server has nothing to offer the user — there is nowhere to send it
/// — so it is [`MissingSetting::Url`]. A server with no session credential has a
/// form, so it is [`Self::Login`] — **but only once the store has answered**, which is
/// what [`Self::Resume`] is for. [`ConnectionSettings::from_env`] keeps the stricter
/// reading for callers that have no form; this is the reading [`open`] uses.
///
/// **`Self::Connected` holds the same [`ConnectionSettings`] the environment path
/// would have built**, which is what `tests/login.rs::the_connected_state_has_exactly_
/// one_representation` asserts. One type, one constructor, two ways of arriving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    /// Neither variable is set: the documented offline shell, and not an error.
    Local,
    /// A server is configured, there is no environment token, and the credential
    /// store has not answered yet.
    ///
    /// **A phase, not a view, and that is the whole reason it exists.** Whether the
    /// window shows a chat or a login form now depends on a store that has not been
    /// read, and reading it is a blocking operating-system call that `AGENTS.md` §2.3
    /// forbids on the main thread. So there is a window between "I read the
    /// environment" and "I know which view to show", and this is what it looks like
    /// as a value.
    Resume {
        /// The configured WebSocket endpoint. The login URL is derived from it inside
        /// `network/rest.rs`; the view is handed the endpoint and never asked to
        /// assemble a URL of its own.
        endpoint: String,
    },
    /// A server is configured and there is no session, so the window opens the form.
    Login {
        /// The configured WebSocket endpoint. See [`Self::Resume`].
        endpoint: String,
    },
    /// Both variables are set, a sign-in just succeeded, or a stored session was
    /// found: open the shell and start the socket.
    Connected(ConnectionSettings),
}

/// Reads the environment into the matrix [`Startup`] describes.
///
/// `Ok(Startup::Local)` is the **no-server case and is not an error**, and it is
/// the arm every offline suite depends on — `tests/login.rs` asserts it before it
/// asserts anything else, because a leaked `SH_NEXUS_URL` would make every other
/// assertion in that suite pass or fail for the wrong reason.
///
/// The only `Err` is [`MissingSetting::Url`]: a token with nowhere to go. See the
/// type's docs for why that one arm is not a login screen.
///
/// **A server with no token is [`Startup::Resume`] and not [`Startup::Login`]**, and
/// that is the one case this function had to stop answering on its own. It cannot:
/// the credential store is a blocking OS call, `AGENTS.md` §2.3 forbids that on the
/// frame path, and answering "the form" before asking the store would put a login
/// prompt on screen on every launch for a user who has never signed in on this
/// machine. [`resume_with`] is the same decision, with the store's answer supplied.
pub fn startup_from_env() -> Result<Startup, MissingSetting> {
    match configured() {
        (None, None) => Ok(Startup::Local),
        (Some(url), None) => Ok(Startup::Resume { endpoint: url }),
        (Some(url), Some(token)) => Ok(Startup::Connected(ConnectionSettings::new(url, token))),
        (None, Some(_)) => Err(MissingSetting::Url),
    }
}

/// The mode a resumed window opens in, given what the credential store held.
///
/// **A pure function of two strings, and that is the point.** The alternative — a
/// `match` inside [`Shell::apply_resume`] — would make "which view does a launch
/// show" a thing only a test with a window, a store and a server could answer.
/// `tests/keychain.rs` asserts both arms directly and then asserts that a real
/// window ends up where this function said it would.
///
/// **`None` covers "nothing stored" and "this machine has no usable store",** and
/// merging them is what keeps this a two-arm function: a person asked to sign in
/// again cannot do anything about which of the two it was.
pub fn resume_with(endpoint: &str, found: Option<&str>) -> Startup {
    match found {
        Some(credential) => Startup::Connected(ConnectionSettings::new(endpoint, credential)),
        None => Startup::Login {
            endpoint: endpoint.to_owned(),
        },
    }
}

/// Why [`Shell::start_transport`] did not start one.
///
/// **Two causes, and they are not the same kind of thing.** A [`MissingSetting`] is the
/// operator's configuration being incomplete and is fixed in a shell; a
/// [`TransportError`] is this machine failing to spawn a thread and is fixed by
/// restarting. Collapsing them into one string would lose exactly the distinction that
/// tells the user whether to edit an environment variable or reboot.
#[derive(Debug)]
pub enum StartError {
    /// Half the configuration was given.
    Missing(MissingSetting),
    /// The worker thread could not be started.
    Transport(TransportError),
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(missing) => write!(formatter, "{missing}"),
            Self::Transport(error) => write!(formatter, "could not start the socket: {error}"),
        }
    }
}

impl std::error::Error for StartError {}

impl From<MissingSetting> for StartError {
    fn from(missing: MissingSetting) -> Self {
        Self::Missing(missing)
    }
}

/// `#[derive(Debug)]` would print `token`, and this struct is reachable from any
/// `{:?}` on an error or a log line. `AGENTS.md` §7.5 forbids logging credentials,
/// so the implementation is written out instead of derived — the same reason
/// `network::ws::TransportConfig` does the same thing.
impl fmt::Debug for ConnectionSettings {
    /// Prints the URL and whether a token is present, never the token.
    ///
    /// The token's *presence* is printed rather than the field hidden entirely,
    /// because "a URL with no token is configured" is a diagnosable condition and one
    /// boolean is enough to see it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionSettings")
            .field("url", &self.url)
            .field("token_configured", &!self.token.is_empty())
            .finish()
    }
}

impl From<TransportError> for StartError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

/// The production message list, the one view this shell composes.
/// The root component: the palette's owner, the list's parent, the drain pump's
/// lifetime, and the window's key target.
///
/// **The list is held as an [`Entity`] and not built inline in [`Render`], and
/// three things in this file are impossible without that.** The drain pump has to
/// repaint it from a timer callback that owns no window; the key handler has to
/// reach it from a listener; and the palette has to be handed to it *before* the
/// first frame rather than during one. An element constructed inside the render
/// closure exists only for that closure, so all three would have to become
/// "whatever the next frame happens to build", which is the state a view must not
/// be in. ADR-006's step 3 reaches the same conclusion one level down, for the
/// same reason: a thing whose state must outlive a frame is an entity.
pub struct Shell {
    /// The production message list, the one view this shell composes.
    list: Entity<MessageList>,
    /// The composer, above nothing and below the list.
    ///
    /// **An `Entity` for the same reason [`Self::list`] is one** — see this
    /// struct's documentation — and for one more: the shell's render must hand
    /// the composer a stable identity across frames, or the platform's input
    /// handler, which is registered from a paint callback keyed on the *entity*,
    /// would be rebuilt against a different target on every frame.
    input: Entity<InputBar>,
    /// The producer handle `bridge::install` returned, held for the shell's life.
    ///
    /// See the module docs, §2: dropping it closes the inbox.
    sender: EventSender,
    /// The socket, when one was started.
    ///
    /// **An `Option` because both being absent is the documented offline case**, not a
    /// failure — see [`ConnectionSettings::from_env`]. `None` means this shell is the
    /// offline one and the user sees nothing connection-shaped, which is correct
    /// rather than a degraded state to apologise for.
    ///
    /// **Held, and not dropped, and that is the whole reason the field exists.** The
    /// transport owns the worker thread; dropping it would tear the socket down
    /// silently, with no event and no log, and the composer would simply stop
    /// delivering sends. Holding it for the shell's life is what makes the connection
    /// outlive a frame.
    transport: Option<WsTransport>,
    /// The session a sign-in issued, when one has.
    ///
    /// **An `Option` and its own field rather than something the transport can be
    /// asked for, because "who am I connected as" and "is there a socket" are
    /// different questions.** `Self::transport` answers the second and is honest
    /// about only the second; this one answers the first and is the *only* place
    /// the client keeps the credential [`ConnectionSettings`] carries — see the
    /// module docs, §7.
    session: Option<ConnectionSettings>,
    /// The login form, while there is no session.
    ///
    /// **`None` is the ordinary offline and connected case**, so this is not a mode
    /// flag with a boolean next to it: one `Option` says both "is there a form" and
    /// "is there no session", and the two cannot disagree.
    ///
    /// **An [`Entity`] for the reason [`Self::list`] is one**, plus one more: the
    /// form's platform input handler is registered from a paint callback keyed on
    /// the *entity*, so a form rebuilt per frame would have a different target on
    /// every frame and would accept nothing.
    login: Option<Entity<LoginView>>,
    /// The endpoint whose stored session this window is still waiting for.
    ///
    /// **`Some` is the startup phase, and it is a field rather than a mode flag because
    /// `AGENTS.md` §3.1's tree makes this file the only one that can say which of two
    /// things a window is.** It answers the question the login field also answers, and two
    /// answers to one question is `bridge.rs` §5's audit-trail hazard.
    ///
    /// **Holding the endpoint and not a bool is what makes the phase recoverable.** The
    /// worker that reads the store takes time, and every way this can end — the answer
    /// arriving, the worker refusing to start, a socket that cannot be opened — needs the
    /// endpoint to build the next thing.
    resuming: Option<String>,
    /// Whether the live session was read out of the credential store rather than typed or
    /// configured.
    ///
    /// **The one field in this struct whose value decides a destructive action**, and it
    /// exists so the decision is checkable rather than inferred. See the module docs, §7:
    /// only a restored session is cleared and replaced with the login form when the server
    /// refuses it, because the other two kinds are an operator's configuration or a
    /// credential the user can see.
    restored: bool,
    /// The store this window keeps its session credential in.
    ///
    /// **A [`NoopTokenStore`] until [`Self::use_credential_store`] replaces it**, and that
    /// default is the documented offline behaviour rather than a placeholder: a shell built
    /// without one keeps no credential and loses nothing, because there was never one to
    /// keep. It is also what keeps the six suites that call [`Self::new`] directly from
    /// touching a developer's real keychain.
    ///
    /// **An `Arc` because the seam hands it to a worker**, which is where every operating-
    /// system call happens — see `platform/mod.rs`'s docs for why this struct may not do it
    /// itself.
    credentials: Arc<dyn TokenStore>,
    /// The palette this shell's own elements draw with.
    ///
    /// **A copy rather than a borrow of the theme, and the same reason
    /// [`Colors`] exists at all** (`ui/mod.rs`): a frame reads these values, and a
    /// parsed theme document with strings in it is not what the frame path should
    /// be reaching back into.
    colors: Colors,
    /// The focus target for the window's keyboard.
    ///
    /// A root component that never takes focus is a window no key reaches, and
    /// `AGENTS.md` §5.2 requires the feature to work from the keyboard alone.
    ///
    /// **This is the shell's *middle* focus target, not the one the window opens
    /// on and not the one the keyboard ends on.** [`open`] focuses
    /// [`Self::composer_focus_handle`] instead, because a client that opens with a
    /// text field focused is a client the user can type into. This handle is the
    /// rung between: it is what [`InputBar`] hands focus to when `Escape` leaves
    /// the field, and [`Shell::on_key_down`] hands focus *on* to the log from it.
    /// **The three are a ladder and not a ring**, and
    /// [`Shell::on_key_down`] says why the ladder does not come back up.
    focus_handle: FocusHandle,
}

impl Shell {
    /// Builds the shell around a freshly installed application state.
    ///
    /// **`sender` is a parameter rather than something this function installs,
    /// and the ordering is the reason.** `bridge::install` needs a `&mut App`,
    /// which a `Context<Self>` derefs to — but a call made *here* would run inside
    /// `open_window`'s build callback, which is after a window exists, and
    /// `AGENTS.md` §7.3 and `bridge::install`'s own documentation both require
    /// the state to exist before any view can be built. [`open`] installs; this
    /// receives what it installed and keeps it.
    ///
    /// Three things happen once, here, rather than on a later event: the palette
    /// is resolved and handed to both views, the list is pointed at
    /// [`STARTUP_CHANNEL`], and the drain pump is armed. All three are one-time
    /// wiring, and a constructor is the one place a reader can be sure they are
    /// not conditional.
    pub fn new(sender: EventSender, cx: &mut Context<Self>) -> Self {
        let colors = theme_colors();

        // Taken before either view is built, and moved into `Self` at the end,
        // because the composer needs it: `Escape` in the field has to hand focus
        // *somewhere*, and a window left with nothing focused delivers every key
        // to a dispatch node with no listener on it. See `InputBar::fallback_focus`.
        let focus_handle = cx.focus_handle();

        let list = cx.new(MessageList::new);
        list.update(cx, |list, cx| {
            // Two calls, one order: the palette first, so the first frame the
            // user ever sees is already themed rather than briefly not.
            list.set_colors(colors, cx);
            list.show_channel(STARTUP_CHANNEL, cx);
        });

        // The composer is built from the finished list rather than the other way
        // round, and the order is the dependency: `InputBar` holds the list
        // because the send gesture is the list's own door
        // (`MessageList::begin_send`), so a composer built first would be
        // building a reference to a list that does not exist yet. The palette is
        // passed in rather than pushed afterwards, because it is applied once
        // here — see this file's module docs, §4.
        let input = cx.new(|cx| InputBar::new(list.clone(), focus_handle.clone(), colors, cx));

        let shell = Self {
            list,
            input,
            sender,
            colors,
            focus_handle,
            transport: None,
            session: None,
            login: None,
            resuming: None,
            restored: false,
            credentials: Arc::new(NoopTokenStore),
        };
        shell.start_drain_pump(cx);
        shell
    }

    /// Puts the store this window keeps its session credential in.
    ///
    /// **A setter rather than a constructor parameter, and the reason is a signature six
    /// test suites and `benches/frame_time.rs` depend on.** [`Self::new`] is called by
    /// `tests/app_shell.rs`, `tests/login.rs`, `tests/shell_connection.rs`,
    /// `tests/soak_cycle.rs`, `tests/soak_e2e.rs` and the bench, none of which is about
    /// the keychain, and widening its arity would have touched all seven for one feature.
    /// [`open`] calls this immediately after constructing the shell, so a production
    /// window never spends a moment on the default.
    pub fn use_credential_store(&mut self, store: Arc<dyn TokenStore>) {
        self.credentials = store;
    }

    /// The store this window is keeping its session credential in.
    ///
    /// **Exposed for the reason [`Self::list`] and [`Self::input`] are**: a test asserting
    /// that the production window holds the *real* store rather than the no-op one is what
    /// makes "the keychain is wired up" a checkable claim rather than prose. It returns a
    /// clone because the field is an `Arc` the seam shares with a worker.
    pub fn credential_store(&self) -> Arc<dyn TokenStore> {
        Arc::clone(&self.credentials)
    }

    /// Puts this window into the startup phase for `endpoint` and asks the seam to read
    /// the store off the main thread.
    ///
    /// **Refused when the window is already in the phase, already connected, or already
    /// showing a form**, for the reason [`Self::enter_login`] refuses: two startup phases
    /// would be two workers reading one slot, and the second answer would have nothing to
    /// apply to.
    ///
    /// **The worker starts before the flag is set, and the order is the contract.** A
    /// worker that could not start has to leave the window somewhere it can be acted on,
    /// and a window stuck in a phase nothing will ever leave is the one state with no way
    /// forward — so a refusal falls straight through to [`Self::enter_login`].
    ///
    /// **Non-blocking, and that is what makes it safe from [`open`].** The credential read
    /// is a synchronous OS call that `AGENTS.md` §2.3 forbids on the frame path; the seam
    /// owns the thread and this function only asks.
    pub fn begin_resume(&mut self, endpoint: &str, cx: &mut Context<Self>) -> bool {
        if self.resuming.is_some() || self.transport.is_some() || self.login.is_some() {
            return false;
        }
        self.resuming = Some(endpoint.to_owned());
        if !bridge::begin_credential_load(cx, Arc::clone(&self.credentials)) {
            tracing::error!(
                "this machine refused to read the credential store, and this window has no session"
            );
            return self.resume_failed(endpoint, cx);
        }
        // **`Entity::update` does not mark anything dirty**, and the frame already on
        // screen is a chat from before the launch decision — the same stale-frame problem
        // `enter_login` solves with its own `notify`.
        cx.notify();
        true
    }

    /// Whether this window is waiting for the credential store to answer.
    ///
    /// **`Render` reads it and `tests/keychain.rs` asserts it, and it is a separate
    /// accessor rather than a field read** for the reason [`Self::login_form`] gives: which
    /// of three things this window is has exactly one answer, and every reader asking the
    /// owner is what keeps it one question.
    pub fn is_resuming(&self) -> bool {
        self.resuming.is_some()
    }

    /// Whether the live session was read out of the credential store.
    ///
    /// **The flag the recovery path gates on, exposed so the distinction is checkable.**
    /// `a_rejected_stored_credential_is_cleared_and_the_login_form_appears` asserts it is
    /// true after a restore and false afterwards, which is what proves the recovery cleared
    /// the *restored* marker rather than merely the session.
    pub fn session_was_restored(&self) -> bool {
        self.restored
    }

    /// Gives up on the startup phase and shows the form instead.
    ///
    /// **The only way out of [`Self::resuming`] other than an answer**, and it exists so
    /// the field has no path that leaves a window stranded. `AGENTS.md` §2.1's rule about
    /// not panicking in user-facing code has an equally firm counterpart: a user-facing path
    /// with no way forward is the same defect wearing a different hat.
    fn resume_failed(&mut self, endpoint: &str, cx: &mut Context<Self>) -> bool {
        self.resuming = None;
        self.enter_login(endpoint, cx)
    }

    /// Applies the credential answer, if one is waiting, and reports whether one was.
    ///
    /// **First and ungated in [`Self::apply_inbox`], and before the login poll.** The
    /// answer arrives on its own channel with the event inbox silent — the same state
    /// `a_login_completes_with_the_event_inbox_completely_silent` drives for 6A — so
    /// gating it would leave a window in the startup phase until the socket said
    /// something.
    ///
    /// **The decision about what to do with it is [`resume_with`], not this function.** That
    /// is what lets both of its arms be asserted without a window, and it is why this
    /// method has no branch of its own for "nothing was stored".
    ///
    /// **A socket that cannot be opened falls back to the form rather than keeping a
    /// session with no socket**, for the reason [`Self::accept_login`] gives: the credential
    /// is still valid and the user is still looking at a window, so the answer is a form
    /// they can use and not a chat that can never connect.
    fn apply_resume(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(answer) = bridge::try_take_credential_load(cx) else {
            return false;
        };
        let Some(endpoint) = self.resuming.take() else {
            // **Unreachable while this poll is the only caller**, and reported rather than
            // unwrapped: an answer with nothing to apply it to would mean a credential was
            // read and then dropped without a word.
            tracing::warn!("a stored session was read with nothing waiting for it");
            return false;
        };

        match resume_with(&endpoint, answer.credential()) {
            Startup::Connected(settings) => match self.start_transport(&settings, cx) {
                Ok(()) => {
                    self.session = Some(settings);
                    self.restored = true;
                    // **The handle, never the value** — `AGENTS.md` §7.5, and the per-line
                    // scan in `tests/login.rs` and `tests/keychain.rs` is what says so.
                    tracing::info!("a stored session was found and the socket is starting");
                }
                Err(error) => {
                    tracing::error!(error = %error, "this machine could not open the socket for a stored session");
                    self.resume_failed(&endpoint, cx);
                }
            },
            Startup::Login { endpoint } => {
                self.enter_login(&endpoint, cx);
            }
            // **Unreachable, and handled rather than matched with `unreachable!()`.**
            // `resume_with` has two arms and `AGENTS.md` §2.1 forbids a panic in a
            // user-facing path; a third arm here would be a decision to invent rather than
            // to check.
            Startup::Local | Startup::Resume { .. } => {
                tracing::error!("the stored-session answer produced no window at all");
            }
        }
        cx.notify();
        true
    }

    /// Starts the socket if this machine is configured for one, and reports what it did.
    ///
    /// **A separate method, and not a step inside [`Self::new`], because a constructor
    /// that reads the environment is only testable by luck.** `new` is called from
    /// tests that must not depend on the developer's shell, and a transport started
    /// inside it would try to reach whatever `SH_NUSUS_URL` happens to hold — or
    /// spawn a thread on every test that builds a shell. Reading the environment here,
    /// explicitly, keeps the constructor pure and makes this the single place where
    /// the decision is made.
    ///
    /// **Both variables absent is `Ok(None)` and not an error.** See
    /// [`ConnectionSettings::from_env`]; the offline shell is the documented default.
    ///
    /// The transport publishes through this shell's **own** [`EventSender`], so frames
    /// arrive at the same inbox the drain pump already empties. There is one channel
    /// into the state, not two, which is what keeps ADR-009's confinement intact: the
    /// worker touches no context type and the main thread still applies everything
    /// itself.
    ///
    /// # Errors
    ///
    /// Two, and neither is about the network:
    ///
    /// - [`MissingSetting`] — half the configuration was given. Reported before any
    ///   window opens rather than surfacing later as an opaque 401.
    /// - [`TransportError`] — the worker thread could not be started.
    ///
    /// The `cx` is taken because installing a transport is not a field write: the
    /// message list is a **child view**, and reaching it needs a context. It was
    /// added to this signature when the send path was wired, and it is the honest
    /// arity — a method that starts a socket and leaves the composer unable to reach
    /// it is the defect this parameter exists to prevent.
    pub fn start_transport(
        &mut self,
        settings: &ConnectionSettings,
        cx: &mut Context<Self>,
    ) -> Result<(), StartError> {
        let transport = WsTransport::start(settings.transport_config(), self.sender.clone())?;
        // **Published into the bridge, not held by a view.** This is the only thing
        // PR #43 got wrong about sending: the socket existed, `Shell::transport`
        // returned it, and `WsTransport::send_message` had zero production callers,
        // because nothing connected the composer to it. The bridge is where
        // `network/` is reachable from — `AGENTS.md` §3.2 and
        // `tests/layer_boundary.rs` both say so, and the first attempt at publishing
        // this into `MessageList` failed that test. One owner, one publication point:
        // if a second path could install a transport, two sockets could exist and a
        // send would land on whichever was installed last.
        bridge::install_transport(cx, Some(transport.clone()));
        self.transport = Some(transport);
        Ok(())
    }

    /// The socket this shell is holding, if any.
    ///
    /// Exposed for the same reason [`Self::list`] and [`Self::input`] are: a test
    /// asserting the transport is *actually running* is what makes "the client
    /// connects" a checkable claim rather than prose.
    ///
    /// **`Option<&WsTransport>` and never a `bool`, and that is the whole reason
    /// this accessor exists.** A `is_connected()` used to stand beside it and was
    /// removed rather than corrected: it returned `transport.is_some()`, which is
    /// *a worker thread was started*, while the name promised *the peer answered*.
    /// The two differ on the first frame and forever after — a shell pointed at a
    /// dead port reports "connected" for as long as the backoff keeps retrying —
    /// and PR #43's own test asserted that lie against `ws://127.0.0.1:1/ws`.
    ///
    /// **The truth already exists and has one owner:** `AppState::can_send()`, which
    /// answers the composer's question and is exposed through
    /// [`bridge::try_read`]. Two answers to one question is what `bridge.rs` §5
    /// calls the audit-trail hazard, so this accessor stays about the one thing the
    /// field honestly knows — and that is also exactly what
    /// [`connection_banner`] needs to decide whether to draw at all.
    pub fn transport(&self) -> Option<&WsTransport> {
        self.transport.as_ref()
    }

    /// Puts this window into the login mode, for `endpoint`.
    ///
    /// **Refused when the window is already in it or already connected, and the
    /// guard is the whole reason this returns a `bool`.** There is no state in
    /// which a second form replaces the first: two forms would be two sets of
    /// buffers and two focus handles, and a form entered while a socket is being
    /// started would let a user submit credentials into a window that is about to
    /// leave for the chat. **A caller that ignored the refusal would not get two
    /// forms — it would get one form and one stale entity nobody rendered.**
    ///
    /// **The endpoint is the configured WebSocket URL and is passed straight
    /// through.** The login URL is derived from it inside `network/rest.rs`, and a
    /// view that could build one would be a second place the two spellings of
    /// "where the server is" could disagree — so the view displays the endpoint
    /// and never assembles a URL.
    pub fn enter_login(&mut self, endpoint: &str, cx: &mut Context<Self>) -> bool {
        if self.login.is_some() || self.transport.is_some() {
            return false;
        }
        let endpoint = endpoint.to_owned();
        let colors = self.colors;
        self.login = Some(cx.new(|cx| LoginView::new(endpoint, colors, cx)));
        // **`Entity::update` does not mark anything dirty**, and the frame that is
        // already on screen is the chat. Without this line the window would keep
        // painting a log for a person who has no session, and
        // `tests/login.rs::a_correct_login_moves_the_window_from_the_form_to_the_chat`
        // would find `message-list` still occupying the window — a stale frame
        // rather than a wrong tree, which is the harder of the two to diagnose.
        cx.notify();
        true
    }

    /// Clears a restored session the server refused, and opens the form in its place.
    ///
    /// **This is the lockout trap's exit, and it exists because of what happens without
    /// it.** A launch that restores a credential and then meets a 401 has a chat that can
    /// never connect and no way to reach a login form, so the user's only recovery is
    /// deleting a credential by hand in a system dialog they have never heard of. The fix
    /// is four actions in one step and **the order is the contract**:
    ///
    /// 1. the socket is shut down and dropped, and unpublished from the seam;
    /// 2. the session is dropped, so nothing holds a credential the server has refused;
    /// 3. the store is told to forget it, off the main thread;
    /// 4. the form is opened for the endpoint that session was for.
    ///
    /// **Step 1 precedes step 4 because [`Self::enter_login`] refuses while a transport is
    /// held.** A form and a refused socket together is precisely the state this method
    /// exists to leave, and `bridge.rs`'s one-transport-one-publication-point rule means the
    /// drop has to be published as well as performed — a socket the seam still holds would
    /// accept a send into a socket nobody is watching.
    ///
    /// **Only a *restored* session takes this path, and [`Self::restored`] is the gate.** A
    /// session from `SH_NEXUS_TOKEN` is an operator's configuration and one the user just
    /// typed is a credential they can see; clearing either without being asked is a
    /// destructive action with no confirmation, which is the same reasoning
    /// `actions::discard_failed_send` gives for not wiring its affordance. The cost is
    /// stated: **a refused environment token still leaves the user at the banner**, and that
    /// is the pre-existing behaviour rather than a regression.
    ///
    /// **Gated on `report.applied() != 0` at the call site**, because a refusal always
    /// arrives as an applied [`ConnectionState`] event. Reading the state on every quiet
    /// tick would be a lease of the global twenty times a second for a condition that
    /// cannot have changed.
    fn recover_from_a_rejected_restored_session(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.restored || self.transport.is_none() {
            return false;
        }
        let rejected = bridge::try_read(cx, |state| {
            matches!(state.connection(), ConnectionState::Rejected { .. })
        })
        .unwrap_or(false);
        if !rejected {
            return false;
        }
        let Some(endpoint) = self
            .session
            .as_ref()
            .map(|session| session.url().to_owned())
        else {
            // **Unreachable while [`Self::restored`] implies a session** — the two are set
            // together in `apply_resume` — and reported rather than unwrapped, because the
            // alternative is a window that has decided to drop a credential it cannot name.
            tracing::warn!(
                "a restored session was refused and this window cannot say which server it was for"
            );
            return false;
        };

        // `WsTransport` has no `Drop` that stops its worker: the handle owns a clone of the
        // outbound sender, so the socket has to be told to stop. Publishing `None` as well
        // is the other half of the same step, per the seam's one-publication-point rule.
        if let Some(transport) = self.transport.take() {
            transport.shutdown();
        }
        bridge::install_transport(cx, None);
        self.session = None;
        self.restored = false;
        self.forget();

        // **The return value is the frame decision, not a summary.** `enter_login` cannot
        // be refused here — the socket is gone and no form was open — and returning its
        // answer keeps "did the window change" in one place rather than two.
        self.enter_login(&endpoint, cx)
    }
    ///
    /// **A private method rather than a `bridge::remember_credential` call at each site,
    /// because there are exactly two sites and they must not drift**: a sign-in that just
    /// succeeded, and nothing else. `AGENTS.md` §8.1's Login Flow says the token is stored
    /// in the keychain after a valid credential, and `odd/tasks/6b-keychain.md` decision 2
    /// records that this is specified behaviour rather than an open product question — a
    /// "remember me" toggle is deliberately out of scope.
    ///
    /// **The value is moved, not borrowed**, and the worker's copy dies with the operation.
    /// Nothing on this side keeps it, which is the same structural guarantee
    /// `bridge::begin_login` gives the password.
    ///
    /// **A store that refuses costs nothing here, and that asymmetry is the requirement.**
    /// Losing a remembered credential costs the user a sign-in next launch; losing the login
    /// screen costs them the window. The seam reports the refusal as a log line and the
    /// session is untouched.
    fn remember(&self, credential: String) {
        if !bridge::remember_credential(Arc::clone(&self.credentials), credential) {
            tracing::warn!("this machine refused to start the worker that would keep the session");
        }
    }

    /// Asks the store to forget whatever it was holding, off the main thread.
    ///
    /// **Idempotent end to end.** Two callers reach it on any ordinary run — a logout and
    /// the recovery from a refused stored session — and `platform::TokenStore::clear`
    /// answers `Ok(())` for "there was nothing", so neither can fail on the second.
    fn forget(&self) {
        if !bridge::forget_credential(Arc::clone(&self.credentials)) {
            tracing::warn!("this machine refused to start the worker that would clear the session");
        }
    }

    /// Applies what a finished attempt said, and reports whether the window left
    /// the login mode.
    ///
    /// **The shell owns the mode and the view owns the sentence, and the split is
    /// what keeps the layer boundary honest.** A successful login takes the whole
    /// window somewhere else, and a view cannot decide that about itself — so it
    /// answers "here is the line to draw" ([`crate::ui::views::login::LoginView`]
    /// `::show`) and this answers "here is what happens now".
    ///
    /// **A refusal keeps the form, and keeps no token.** `AGENTS.md`'s Auth Failure
    /// Flow is about a window that says why and holds nothing; storing the token
    /// from a refused attempt would be a credential in a `String` with no owner,
    /// and this function is the only place one could be stored.
    ///
    /// **The socket starts in the same step that drops the form**, so there is no
    /// window in which the client holds a session, has no form, and has not been
    /// told — which is what makes the "no second socket" claim in the module
    /// docs, §7, true rather than aspirational.
    ///
    /// **A transport that cannot start does not throw the session away and does not
    /// open the chat.** `start_transport` failing means this machine refused to
    /// spawn a thread; the credential is still valid and the user is still looking
    /// at a window, so the answer is rendered as a [`LoginOutcome::Failed`] and the
    /// form stays where it is.
    pub fn accept_login(&mut self, outcome: LoginOutcome, cx: &mut Context<Self>) -> bool {
        match outcome {
            LoginOutcome::LoggedIn { token, username } => {
                let Some(endpoint) = self
                    .login
                    .as_ref()
                    .map(|form| form.read(cx).endpoint().to_owned())
                else {
                    // **Unreachable while the poll is the only caller** — an attempt
                    // can only have been started by a form — and it is reported
                    // rather than handled, because the one honest answer to "a valid
                    // session arrived and there is nothing to apply it to" is to say
                    // so. Discarding the credential quietly would leave a user who
                    // signed in correctly staring at a chat that never connects.
                    tracing::warn!(
                        reason = "no form is open",
                        "a sign-in succeeded with nothing to apply it to"
                    );
                    return false;
                };
                // **One value, cloned once, and the reason is that the store needs it too.**
                // `ConnectionSettings` keeps it for the life of the session and
                // `TokenStore::store` takes it by value, so exactly one of the two
                // receives the original. Which one is arbitrary; *that it is one and not
                // two hand-rolled copies* is not, because a credential reassembled by hand
                // is a credential with no owner.
                let settings = ConnectionSettings::new(endpoint, token.clone());
                match self.start_transport(&settings, cx) {
                    Ok(()) => {
                        self.session = Some(settings);
                        self.login = None;
                        // **The handle, never the session.** `AGENTS.md` §7.5, and
                        // `tests/login.rs` scans every production source for a log
                        // line naming either credential.
                        tracing::info!(username = %username, "signed in; the socket is starting");
                        // **And only once the socket is up**, so the remembered credential
                        // is one this client has actually used. A session whose socket could
                        // not be opened is dropped above, and a token nobody has presented
                        // anywhere is not worth writing to disk.
                        self.remember(token);
                        true
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            "signed in, but this machine could not open the socket"
                        );
                        self.show_login_outcome(
                            LoginOutcome::Failed {
                                reason: error.to_string(),
                            },
                            cx,
                        );
                        false
                    }
                }
            }
            refusal => {
                self.show_login_outcome(refusal, cx);
                false
            }
        }
    }

    /// Shows `outcome` on the open form, or records that there is nothing to show
    /// it on.
    ///
    /// **`Option::None` on the form is not a failure and is not an error.** The
    /// window has already left the login mode, so there is no surface left to draw
    /// a line on — and the outcome is still worth a trace, because `AGENTS.md` §7.5
    /// says ids and outcomes, and a login the user never saw explained is exactly
    /// what a silent drop looks like from the other side.
    fn show_login_outcome(&self, outcome: LoginOutcome, cx: &mut Context<Self>) {
        match self.login.clone() {
            Some(form) => form.update(cx, |form, cx| form.show(outcome, cx)),
            None => tracing::info!(outcome = %outcome, "a sign-in attempt finished"),
        }
    }

    /// The login form, while this window is in the login mode.
    ///
    /// **Exposed for the same reason [`Self::list`] and [`Self::input`] are**, and
    /// for one more reason: "which of the two things is this window" is a question
    /// with exactly one answer, and [`Render`], [`Self::accept_login`] and
    /// `tests/login.rs` all asking the same accessor is what keeps it one question
    /// rather than three inspections of a field.
    pub fn login_form(&self) -> Option<&Entity<LoginView>> {
        self.login.as_ref()
    }

    /// The session this client is connected with, if it has one.
    ///
    /// **`None` in two quite different situations, and the difference is not this
    /// accessor's business**: the offline shell has no session because no server was
    /// configured, and a window sitting on the login form has no session because
    /// nobody has signed in yet. [`Self::login_form`] is what tells them apart.
    pub fn session(&self) -> Option<&ConnectionSettings> {
        self.session.as_ref()
    }

    /// Applies a finished attempt, if one is waiting, and reports whether one was.
    ///
    /// **First and ungated in [`Self::apply_inbox`], and the reason is in the module
    /// docs, §7**: a login answer arrives on its own channel with the event inbox
    /// silent, so putting this behind the `applied() == 0` early return would make a
    /// successful sign-in wait for the socket to say something.
    ///
    /// **The repaint is conditional on the mode changing rather than on the answer
    /// arriving.** A refusal repaints the form through the view's own `notify`,
    /// because the form is the thing that changed; a success changes what this
    /// shell renders, so only that case asks for a frame here.
    fn apply_login(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(outcome) = bridge::try_take_login_outcome(cx) else {
            return false;
        };
        let left_the_form = self.accept_login(outcome, cx);
        if left_the_form {
            cx.notify();
        }
        true
    }

    /// The production message list this shell composes.
    ///
    /// Exposed so a test can assert what the shell actually holds — that the view
    /// being rendered is `ui::views::message_list::MessageList` and not a stub
    /// that looks like it.
    pub fn list(&self) -> &Entity<MessageList> {
        &self.list
    }

    /// The composer this shell composes.
    ///
    /// Exposed for the same reason [`Self::list`] is: a test asserting that the
    /// input bar on screen is the production [`InputBar`] and not a stand-in is
    /// what makes the wiring claim checkable rather than prose.
    pub fn input(&self) -> &Entity<InputBar> {
        &self.input
    }

    /// The focus handle of the composer — the one this window opens with.
    ///
    /// **A dedicated accessor rather than letting [`open`] reach through
    /// `input().read(..)`, because "what does a new window focus" is a question
    /// with one answer and `bridge.rs` §5's audit-trail argument is about there
    /// being one place to read it.**
    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    /// The producer handle, still open.
    ///
    /// **`network/`'s future entry point, and the assertion that makes the module
    /// docs' §2 claim checkable:** a delivery through this handle must answer
    /// [`bridge::Delivery::Queued`], not
    /// [`bridge::DeliveryRefusal::BridgeDropped`].
    pub fn sender(&self) -> &EventSender {
        &self.sender
    }

    /// The palette this shell's elements draw with.
    pub fn colors(&self) -> Colors {
        self.colors
    }

    /// Arms the repeating task that applies queued events and repaints.
    ///
    /// **Called from [`Shell::new`], so no caller can forget it and no caller can
    /// start it twice.** A second pump would be a second drain of one inbox from
    /// two tasks, and `bridge::drain` is a `try_recv` loop — the second would
    /// simply find nothing, having raced the first for every event, which is a
    /// schedule nobody can reason about.
    ///
    /// **The timer is awaited on the executor rather than spun on, and it is
    /// never a thread.** `AGENTS.md` §2.3 forbids blocking the frame loop and
    /// `docs/ARCHITECTURE.md` ADR-009 establishes that `Context` is `!Send`, so a
    /// worker thread could not drain at all: `bridge::drain` needs a `&mut App`.
    fn start_drain_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |shell, cx: &mut AsyncApp| loop {
            cx.background_executor().timer(DRAIN_INTERVAL).await;
            // `WeakEntity::update` is the liveness check *and* the drain, in one
            // call: it answers `Err` once the shell is released, which is the
            // signal to stop, and it hands the work to the shell that owns it
            // rather than to a captured copy of its list.
            if shell.update(cx, Shell::apply_inbox).is_err() {
                return;
            }
        })
        .detach();
    }

    /// Applies everything the inbox holds, and repaints what read it.
    ///
    /// Returns how many events changed the state, which is the only thing the
    /// pump asks and the only thing a caller could act on.
    ///
    /// **The repaint is conditional on that number, and the condition is the
    /// difference between a chat client and a busy one.** `bridge::drain` never
    /// schedules a frame — its module docs, §6, leave scheduling to the caller —
    /// and `MessageList::sync` must not notify from inside its own render or the
    /// list would redraw forever. So the two have to be joined *here*, on the one
    /// path that knows a real change happened: a tick that applied nothing asks
    /// for no frame, and twenty quiet ticks a second cost no repaints.
    ///
    /// **Two views are notified, and the second one is why this is no longer a
    /// one-line repaint.** [`crate::ui::views::connection_banner`] is not an
    /// `Entity`: `Render` cannot return nothing, so a healthy connection would
    /// always contribute an element and the banner's whole design — *draw nothing
    /// at all when connected* — would be unobservable from a painted frame. Being a
    /// function of the shell's own frame instead makes the **shell** the view that
    /// has to be told, and that is this notification.
    ///
    /// **It is gated on `transport.is_some()` so the cost stays where the banner
    /// is.** An offline shell draws no banner whatever the state says, so
    /// repainting its root on every tick that changed a message would be a
    /// repaint that cannot change a pixel — the exact thing the paragraph below
    /// rejects. And when the shell *does* hold a transport, the repaint is a
    /// rebuild of the element tree with two cached child entities in it; neither
    /// the list nor the composer re-renders, because GPUI only re-renders an
    /// entity it was told to.
    ///
    /// **The shell's own tree otherwise draws no function of the state**, so
    /// without this line a changed `ConnectionState` would sit in the state while
    /// the last painted frame kept showing the previous word — and
    /// `tests/app_shell.rs`'s connection-visibility section is what says so rather
    /// than leaving it to be discovered.
    ///
    /// **`None` from the drain is reported as zero applied rather than treated as
    /// a failure**, and the pump keeps running. `None` means the state is not
    /// installed, which [`open`] makes impossible by installing before the window
    /// opens; the alternative is a pump that dies on a condition it cannot fix
    /// and cannot report, which is the silent stall the whole module exists to
    /// prevent.
    ///
    /// ## The flush, and why it is not only on the transition to `Connected`
    ///
    /// [`bridge::try_flush_outbox`] runs on **every** tick, before the early
    /// return. `PLAN.md` §7 asks for the outbox to be flushed *"on reconnect"*, and
    /// the transition is where a caller would naturally put that — **but a
    /// transition-only trigger strands one case this unit created.**
    /// [`crate::state::actions::retry_send`] now puts a retried send at the back
    /// of the queue, and the retry can happen while the connection is already up;
    /// with a trigger on the transition there is nothing to drive it until the
    /// *next* reconnect, which may be never. **So the trigger is "the queue is not
    /// empty and the connection can carry it", and the transition is one way to
    /// reach that condition rather than the only way.**
    ///
    /// **The cost is bounded and stated.** A tick with a non-empty queue costs one
    /// `try_send` per queued entry until the server answers, and
    /// [`DRAIN_INTERVAL`] sets the rate at twenty a second. That is the price of
    /// §7's third bullet: re-driving is safe *because* the server deduplicates and
    /// answers a duplicate, and every answer empties the queue. A tick that finds
    /// the queue empty does one `can_send` check and stops — `actions::flush_outbox`
    /// gates on it, so an offline shell copies nothing.
    ///
    /// **The report is logged rather than rendered, and that is the seam's
    /// contract.** A refused frame is a recoverable condition about this machine's
    /// own socket (`AGENTS.md` §7.5's "reconnect scheduled" family), and
    /// [`bridge::FlushReport`] carries counts only — ids and sizes, never content.
    /// Nothing about it changes a row, so it does not ask for a repaint either.
    ///
    /// ## The login poll is first, and outside every gate below
    ///
    /// **Before the drain, before the flush, and before the `applied() == 0` early
    /// return** — for the reason the module docs, §7, give: a login produces no
    /// [`bridge`](crate::state::bridge) event, so on a socket that is quiet the
    /// drain returns a report of zeroes and the early return would swallow the one
    /// answer that mattered. `tests/login.rs` drives a completed sign-in against a
    /// completely silent inbox precisely because that is the state in which the
    /// mistake is invisible.
    ///
    /// **The credential poll is ahead of it for the same reason, and by the same
    /// argument.** A window in its startup phase has no socket and therefore no events
    /// at all, so it is the state in which that mistake is not merely invisible but
    /// terminal: nothing else on this tick would ever end the phase.
    ///
    /// ## The refusal check is last, and only when something arrived
    ///
    /// [`Self::recover_from_a_rejected_restored_session`] reads the connection state, so
    /// it runs after the drain — the drain is what applied the refusal — and behind
    /// `applied() != 0`, which is the condition that says the state changed at all. The
    /// alternative, checking on every tick, would lease the global twenty times a second
    /// to re-ask a question no quiet tick can have changed the answer to.
    fn apply_inbox(&mut self, cx: &mut Context<Self>) -> usize {
        // **First of all, and ungated, for the reason §7's two polls give.** The
        // credential answer is what gets this window out of its startup phase, and it
        // arrives on its own channel while the event inbox is silent.
        self.apply_resume(cx);

        // **Unconditional, and the return value is deliberately not consulted
        // below.** A refusal keeps the form and repaints it through the view's own
        // notify; only a mode change needs a frame from here. Returning early on
        // `false` would mean a refusal arriving in the same tick as no events
        // skipped the *next* poll — which is where the answer that finally succeeds
        // would be waiting.
        self.apply_login(cx);

        let Some(report) = bridge::drain(cx) else {
            return 0;
        };

        match bridge::try_flush_outbox(cx) {
            Some(flush) if flush.refused() > 0 => {
                tracing::warn!(
                    driven = flush.driven(),
                    refused = flush.refused(),
                    held = flush.held(),
                    "the outbox could not be driven in full; the entries stay queued"
                );
            }
            Some(_) | None => {}
        }

        // **After the drain, because the drain is what applied the refusal, and only
        // when something actually arrived.** The check itself is one field read once
        // the window has a restored session; twenty leases of the global a second to
        // re-ask a question that cannot have changed is a cost with no claim behind it.
        if report.applied() != 0 && self.recover_from_a_rejected_restored_session(cx) {
            // **A frame, because this is a mode change and the shell renders the mode.**
            // The recovery returns the answer to *that* question rather than the
            // notification, so "did the window change" stays one decision.
            cx.notify();
        }

        if report.applied() == 0 {
            return 0;
        }
        self.list.update(cx, |_list, cx| cx.notify());
        if self.transport.is_some() {
            cx.notify();
        }
        report.applied()
    }

    /// Handles a key-down aimed at the window.
    ///
    /// **One key, and the count is the point rather than a shortfall.** The one
    /// gesture a chat log needs from the keyboard that a composer does not
    /// already own is the way back to the newest message: ADR-006's table names
    /// *"stick to the newest message"* as a first-class feature of `List`, and
    /// `MessageList::follow_tail` is the half of it that a reader who has
    /// scrolled away needs. `AGENTS.md` §5.2 lists `Escape` among the keys that
    /// must work from the keyboard alone.
    ///
    /// **`Escape` has two owners, and the split is by focus rather than by
    /// arbitration.** While the composer holds focus, `Escape` is the composer's:
    /// it hands focus back to [`Self::focus_handle`] and calls
    /// `cx.stop_propagation`, so this handler never runs. With this handle
    /// focused, `Escape` is this shell's. **The two keys are sequential
    /// gestures, not competing ones**, and the user gets both from one key.
    ///
    /// # The chain: composer → shell → log, and where it stops
    ///
    /// **This handler is the middle rung, and it is the rung that makes the log
    /// reachable by key at all.** `InputBar`'s `fallback_focus` is this shell's own
    /// root handle, so the first `Escape` lands here; the second hands focus to the
    /// list's handle, and from there `up`/`down` move the log's cursor and `enter`
    /// retries a failed send. Before this, the log was reachable by API and by test
    /// and by **no key in the shipped client** — the gap ADR-006's 3E notes record
    /// (`docs/ARCHITECTURE.md`).
    ///
    /// **The chain is a ladder, and it stops at the bottom rather than
    /// alternating.** A second `Escape` while the list is focused runs *this*
    /// handler again — [`MessageList::on_key_down`] deliberately does not claim
    /// `escape`, so the key travels up the dispatch path — and lands on exactly the
    /// same state: the log snaps to the newest message and the list holds focus,
    /// which it already did. **That is idempotent because of a measured property of
    /// the framework rather than a flag in this file:** [`Window::focus`] returns
    /// early when the handle it is given already holds the focus
    /// (`gpui/src/window.rs:2303`), so re-asserting it is free, cannot recurse, and
    /// no handler in this crate ever targets the shell from below. "Terminating"
    /// means exactly that: the result is a function of where focus already is, and
    /// nothing moves it back up.
    ///
    /// **The alternative was a cycle — the list handing `Escape` back to the
    /// composer — and it is rejected for one reason stated three times.** It would
    /// make `escape` a toggle, so a user pressing it repeatedly walks three views
    /// forever, which is the focus loop this chain exists to avoid. It would need a
    /// **second owner** of `escape`, against the rule 3D established and the
    /// paragraph below depends on: one key, one place that answers what it does.
    /// And it would make the log's own "back to the newest message" gesture
    /// unreachable from *inside* the log — which is exactly where a reader who has
    /// scrolled away most wants it.
    ///
    /// **The cost is stated rather than hidden: the ladder does not come back up, so
    /// a keyboard-only user who presses `Escape` twice cannot return to the composer
    /// with a key.** That is a real hole and it is **pre-existing rather than
    /// introduced here**: with one rung, a single `Escape` already stranded the
    /// keyboard on this shell root, and nothing in the crate has a key that returns
    /// focus to the composer. The missing piece is a `Tab` binding on
    /// [`Window::focus_next`], which is a gesture and a product decision (ADR-006,
    /// 3E) and is **not** taken here. What this change does is make the log
    /// reachable and its retry operable from the keyboard, which is what
    /// `AGENTS.md` §5.2 asks of the feature.
    ///
    /// **This is a real keyboard route and it is not `Tab`.** A keyboard purist
    /// expects `Tab` to move between panes and `Shift+Tab` to move back, and this
    /// client has neither; `Escape` meaning two different things depending on where
    /// focus is is the trade this change makes instead of the one a binding would
    /// have made. **`AGENTS.md` §5.2 is therefore *not* satisfied in full** — see
    /// ADR-006's 3E notes — and nothing in this file claims otherwise.
    ///
    /// **The composer has to *hand focus over* rather than simply drop it, and
    /// that is a measured property of the framework rather than a style
    /// preference.** With no element focused, `Window` routes keys to
    /// `DispatchTree::root_node_id()` (`gpui/src/window.rs:6258`) — and that node
    /// is **not** this view's element, so a window with nothing focused delivers
    /// every key to an empty listener list. A composer that blurred and stopped
    /// would consume `Escape` *and* strand this gesture, breaking `AGENTS.md`
    /// §5.2's keyboard-only requirement with the very key meant to satisfy it.
    /// `InputBar::fallback_focus` is the handle it is given for exactly this, and
    /// `tests/app_shell.rs` asserts both halves: this `Escape` works after the
    /// composer's, and the composer's does not fire it.
    ///
    /// **And this handler still adds no `is_focused` test of its own, which was
    /// the obvious design and the wrong one.** A guard here reading "give up if
    /// the composer has focus" is **unreachable in every state it tests**:
    /// `dispatch_key_down_up_event` walks the bubble path focused-node-first
    /// (`gpui/src/window.rs:6068`) and returns the moment `cx.propagate_event`
    /// is false, so the composer's `stop_propagation` means this function is *not
    /// called at all* while the composer has focus. A condition that can never be
    /// false is a branch nothing tests, and `AGENTS.md` §6.1's "any new warning
    /// fails the build" is the smaller half of why that is a cost. **The same
    /// argument covers the list, and it is why the hand-off below needs no guard
    /// either:** the list's own handler does not claim `escape`, so this function
    /// *is* called with the list focused, and doing the same thing in both cases
    /// is what makes the chain idempotent. The ownership is real, it is enforced
    /// by the framework's own ordering, and
    /// `tests/app_shell.rs::escape_hands_focus_to_the_shell_without_returning_the_list_to_the_tail`
    /// plus `escape_chains_from_the_composer_to_the_log_and_its_retry` is what
    /// proves it rather than restating it in a condition.
    ///
    /// **Every other key falls through**, and GPUI's own
    /// `DispatchPhase::Bubble` documentation is why: in the bubble phase
    /// *"keyboard event listeners are invoked from the focused element to the
    /// root of the element tree"*, so this handler runs **after** anything the
    /// focused child handled. That ordering is what lets the composer claim
    /// `Escape` and `Enter` for itself — a shell that swallowed every key would
    /// be a shell that cannot be typed into, and fixing that would mean editing
    /// this function. **It is also the ordering the hand-off relies on**: with the
    /// list focused, this handler runs *after* the list's, so the list gets first
    /// refusal on `escape` and can claim it in future without this function
    /// noticing.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != ESCAPE_KEY {
            return;
        }

        self.list.update(cx, |list, cx| list.follow_tail(cx));

        // The hand-off, and it is unconditional for the reason the section above
        // gives: this handler is reached with the shell root focused *or* with the
        // list focused, and both want the log to be where the keyboard is. A
        // `focus` call on the handle that already holds focus returns before
        // mutating anything (`gpui/src/window.rs:2303`), so the second `escape`
        // costs one comparison and changes nothing.
        //
        // The handle is read out of the list rather than held here, so there is
        // exactly one place in the crate that knows the log's focus target.
        let log = self.list.read(cx).focus_handle(cx);
        log.focus(window, cx);
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // **The startup phase short-circuits first, before the login mode, and the
        // order is chronological rather than arbitrary.** The phase is earlier: it is
        // what a window is in *instead of* either of the two things below, and a shell
        // cannot hold both `resuming` and a form. The claim is the negative one —
        // `tests/keychain.rs` asks the window whether `message-list` or the login
        // selector occupies any space, and a hidden-but-present surface would answer
        // "yes" to the wrong question. A window showing a chat before it knows whether
        // there is a session is telling the user they are signed in.
        if self.resuming.is_some() {
            return resuming_panel(self.colors).into_any_element();
        }

        // **The login mode short-circuits the whole tree, and an early return is
        // the only shape that makes the claim true.** A conditional child would
        // still *build* the chat, and `tests/login.rs` asks the window whether
        // `message-list` occupies any space — which a hidden-but-present log would
        // answer "yes" to. So there is no path from this function to a message list
        // while there is no session.
        //
        // **`AnyElement` in every arm**, and that is what makes the early returns
        // possible at all: `Render` returns one `impl IntoElement`, so a `Div` and an
        // `Entity<LoginView>` cannot both be returned from two branches without
        // erasing the type. The same reason `connection_banner` is a function
        // returning `Option<AnyElement>` rather than an entity.
        if let Some(form) = self.login.clone() {
            return form.into_any_element();
        }

        // **The banner is read before the tree is built, and that ordering is the
        // two-questions rule made mechanical.** `self.transport.is_some()` answers
        // *was a connection attempted* — the one thing that field honestly knows —
        // and the element itself is built inside the read of the real
        // `ConnectionState`. Both halves must be true to draw anything, and the
        // first is checked here rather than inside the view because the view cannot
        // see the shell's field at all: `ui/` is not allowed to reach for
        // `Shell::transport`, and the seam has no opinion on whether a socket was
        // ever started.
        //
        // **One read, and the state is borrowed rather than cloned.** The obvious
        // shape reads first and formats second — `try_read(.., |s|
        // s.connection().clone())` — and it clones a `ConnectionState` on **every
        // frame the shell repaints**, which for `Rejected` is two `String`
        // allocations to produce text nobody repaints. `AGENTS.md` §2.3 counts
        // allocations on the frame path, so the borrow and the formatting happen
        // inside one closure: the only string built per frame is the one the
        // element draws.
        //
        // **A client with no `SH_NEXUS_URL` therefore draws nothing here**, which
        // is the offline shell documented on `ConnectionSettings::from_env` rather
        // than a failure. `app::open`'s `Ok(None)` arm says the same in one line,
        // and the two are the same decision stated in two layers.
        let banner = if self.transport.is_some() {
            bridge::try_read(cx, |state| {
                connection_banner::banner(state.connection(), self.colors)
            })
            .flatten()
        } else {
            None
        };

        // Every element here is a container rather than a text element, and the
        // colours are still set explicitly. GPUI does not inherit text colour
        // from a parent (AGENTS.md 7.3), the spike established that satisfying
        // the rule unconditionally is cheaper than explaining which elements
        // inherit, and the container's own background is what shows between the
        // list's rows and at a resize. A view added inside this column inherits
        // nothing, and that is the point of setting it.
        div()
            .id("app-shell")
            .key_context("Shell")
            // Records this container's bounds for the headless test, for the same
            // reason the message list records its own.
            .debug_selector(|| "app-shell".to_owned())
            .flex()
            .flex_col()
            .size_full()
            .bg(self.colors.background)
            .text_color(self.colors.text)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.list.clone())
            // The connection line, between the log and the field, and the order is
            // the reason it is not anywhere else: it is what a user crosses on the
            // way to type, so a refusal to send has to be readable *before* they
            // type rather than after. `when_some` rather than an unconditional
            // `child` is what makes "renders nothing when connected" true of the
            // element tree and not merely of the banner's own return value.
            .when_some(banner, |column, banner| column.child(banner))
            // The composer, below the list and last in the column, and the order
            // is `PLAN.md` §6's: the log above, the field below, which is what a
            // chat client has looked like since before any of this was a
            // framework. It is not `flex_grow_1` — the list is — so the field
            // takes the height its content needs and the log takes the rest,
            // rather than the two sharing a row of an unbounded column.
            .child(self.input.clone())
            // The erasure the early return above needs, and only that.
            .into_any_element()
    }
}

/// The element the startup phase paints.
///
/// **A free function rather than a fourth branch written out in [`Shell::render`], for
/// the reason the module docs, §5, give: a second place to answer "what does the window
/// show in the startup phase" is the audit trail.** [`RESUME_SELECTOR`] is what makes the
/// claim checkable from a painted frame rather than from a field.
///
/// **It says one line and offers nothing, and both halves are the requirement.** A person
/// looking at it has a window that is about to decide between a chat and a form; anything
/// actionable in the meantime would be a control whose answer the phase has not reached
/// yet. `AGENTS.md` §7.3's explicit text colour is set on both containers for the reason
/// the spike established: satisfying the rule unconditionally is cheaper than explaining
/// which elements inherit.
fn resuming_panel(colors: Colors) -> impl IntoElement {
    div()
        .id("app-resume")
        .key_context("StartupResume")
        .debug_selector(|| RESUME_SELECTOR.to_owned())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .bg(colors.background)
        .text_color(colors.text)
        .child(
            div()
                .text_sm()
                .text_color(colors.text_muted)
                .child("Restoring your session"),
        )
}

/// The selector [`resuming_panel`] records its bounds under.
///
/// **Public so the test can name it and so the literal is written once**, which is
/// `tests/app_shell.rs`'s arrangement for every other selector in this shell.
pub const RESUME_SELECTOR: &str = "startup-resume";

/// A centred, windowed [`WINDOW_WIDTH`] x [`WINDOW_HEIGHT`] chat window.
///
/// Private because the surface is the point: `bridge.rs` §5 argues that the number
/// of ways to do something is the audit trail, and a second way to build these
/// options would be a second answer to "how big is this window".
fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    }
}

/// Installs the application state and opens the shell in one window.
///
/// **This is the one place the global is installed, and it is here because
/// `AGENTS.md` §3.1 makes this file the global-state owner.** The install happens
/// before [`App::open_window`] is called, not inside its build callback, because
/// a view is built during that callback and a view that reads the state through
/// `bridge::try_read` answers `None` — a client with no state — if the global does
/// not exist yet. [`crate::run`] calls this and then activates the application.
///
/// # Errors
///
/// [`ShNexusError::Unknown`] for both failure modes, and that is the reduction
/// `errors.rs`'s own module documentation prescribes: it names this file as the
/// bootstrap boundary where a GPUI error becomes the project's error type, and
/// [`ShNexusError::Unknown`] as "the landing place for the GPUI boundary
/// conversion".
///
/// - **The window could not be opened.** GPUI returns `anyhow::Result`, and
///   `errors.rs` rule 1 forbids a `From<anyhow::Error>` impl existing anywhere, so
///   the conversion is written here by hand and `anyhow` is never named. The
///   source chain is *not* logged: `errors.rs` assigns that to `tracing::error!`,
///   and naming an `anyhow::Error`'s chain would need a variant this crate does not
///   have — so the reduction is the half of that plan this crate can honour today.
/// - **The state could not be installed.** A refusal here means a global of this
///   type already exists, which no remote peer can cause and this function cannot
///   cause twice — the launch callback runs once per process. It is `Unknown`
///   rather than one of the eight specific variants because none of them is the
///   truth: it is not a network failure, not a rejected credential, and not a
///   user-authored configuration, and `errors.rs` documents that landing in
///   `Unknown` is the visible signal that a variant is missing.
pub fn open(cx: &mut App) -> crate::errors::Result<WindowHandle<Shell>> {
    let sender = bridge::install(cx, UNSIGNED_IN_USER)
        .map_err(|error| ShNexusError::Unknown(error.to_string()))?;

    let options = window_options(cx);
    let opened = cx.open_window(options, move |window, cx| {
        let shell = cx.new(|cx| Shell::new(sender, cx));

        // **The store is handed over before the startup decision, and constructing it
        // costs nothing.** `platform::LazyNativeStore` resolves which credential store this
        // machine has on a *worker*, so this line is the only thing on the main thread
        // that touches `platform/` at all — see that type's docs for why choosing a store
        // is itself a blocking OS call and therefore cannot happen here.
        shell.update(cx, |shell, _cx| {
            shell.use_credential_store(Arc::new(LazyNativeStore::new()))
        });

        // **Read and start the transport here, inside the window's own closure,**
        // because the Shell must exist before it can be given a socket: the
        // transport publishes through the shell's `EventSender`, and that handle
        // comes from the shell. Reading the environment any earlier would make the
        // decision invisible from the window that the user is looking at.
        //
        // **The matrix is read once, as a value, and the sixth case is a window
        // rather than an error.** `AGENTS.md` §7.5 says an actionable code the user
        // can report beats a silently dropped explanation — and a login form is
        // the actionable version of "you set a URL and have no session". Every
        // other arm is byte-for-byte what it was; see `startup_from_env`.
        match startup_from_env() {
            // Both variables absent: the documented offline shell. Not an error, and
            // not a banner — there is no connection to describe.
            Ok(Startup::Local) => {}
            // A server, no environment token, and a store that has not answered. The
            // window opens the startup phase, and `Shell::apply_resume` decides between
            // the chat and the form when the answer arrives.
            Ok(Startup::Resume { endpoint }) => {
                if !shell.update(cx, |shell, cx| shell.begin_resume(&endpoint, cx)) {
                    tracing::error!(
                        "the startup phase could not be entered, and this window has no session"
                    );
                }
            }
            // A server and no session, with the store already asked and answered.
            // `startup_from_env` cannot produce this arm — see its docs — but the match
            // is exhaustive rather than `unreachable!()`, which `AGENTS.md` 2.1 forbids.
            Ok(Startup::Login { endpoint }) => {
                if !shell.update(cx, |shell, cx| shell.enter_login(&endpoint, cx)) {
                    tracing::error!(
                        "the login form could not be built, and this window has no session"
                    );
                }
            }
            Ok(Startup::Connected(settings)) => {
                if let Err(error) =
                    shell.update(cx, |shell, cx| shell.start_transport(&settings, cx))
                {
                    tracing::error!(error = %error, "the client has no socket");
                }
            }
            // A token with nowhere to go. There is no form that could help: the
            // server is the thing that is missing.
            Err(missing) => {
                tracing::error!(error = %missing, "the connection settings are incomplete");
            }
        }
        // Focus whatever will receive typing, mirroring how a real client focuses
        // the field it opens on. That is the login form when there is one, the
        // composer when there is a chat, and the shell's own root during the startup
        // phase: a window that opens with no focused text field is one whose keys go
        // nowhere, and `AGENTS.md` 5.2 requires the feature to be reachable from the
        // keyboard alone.
        //
        // The root is still focusable and still takes `Escape` (see
        // `Shell::on_key_down`), so nothing is lost by the root not being what
        // the window starts on.
        let focus = launch_focus_target(&shell, cx);
        focus.focus(window, cx);
        shell
    });

    opened.map_err(|error| ShNexusError::Unknown(error.to_string()))
}

/// The handle a freshly opened window puts the keyboard on.
///
/// **Three answers, and which one applies is chronological rather than a preference:**
/// the form's field when there is a form, the composer's when there is a chat, and this
/// shell's own root during the startup phase.
///
/// **The root is the right answer for the startup phase and the only one available.** The
/// phase draws one line of text and no input, so there is no field to focus; focusing the
/// composer would focus an entity that is not in the element tree, which GPUI can satisfy
/// and the keyboard cannot use. The root handle *is* in the tree, it holds `Escape`, and
/// `AGENTS.md` §5.2's requirement — that the window's keys reach something — is satisfied
/// by it for the at-most-50 ms the phase lasts.
///
/// **A free function rather than a closure in [`open`], because three nested borrows of
/// `cx` inside that closure is a shape a reader has to reconstruct.** One named place that
/// answers the question is also what `bridge.rs` §5's audit-trail argument asks for.
fn launch_focus_target(shell: &Entity<Shell>, cx: &App) -> FocusHandle {
    let borrowed = shell.read(cx);
    if let Some(form) = borrowed.login_form() {
        return form.read(cx).focus_handle(cx);
    }
    if borrowed.is_resuming() {
        return borrowed.focus_handle(cx);
    }
    borrowed.composer_focus_handle(cx)
}
