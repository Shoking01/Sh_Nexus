//! The login screen: two fields, one Enter, and the server's own words when it
//! says no.
//!
//! `AGENTS.md` §8.1's **Login Flow** and **Auth Failure Flow** are two of the ten
//! flows it makes mandatory, and until this file existed neither was writable: the
//! token came from `SH_NEXUS_TOKEN`, so using the application meant calling the
//! REST API by hand and pasting the result into an environment variable. This is
//! the view that removes that.
//!
//! # What this view knows, and what it deliberately does not
//!
//! **It holds the password, because a login form has to.** That is the whole of
//! the exception, and it is worth being precise about where the line is:
//!
//! | Holds the password | Does not |
//! |---|---|
//! | this view's [`LoginView::password`] buffer, cleared on a successful login | `state/`, `core/`, and every `Debug` impl |
//! | the request body, on a worker thread, for the length of one request | any disk, any log line, any panic message |
//!
//! **The password never crosses into the state layer, and the mechanism is
//! structural rather than a rule.** This view calls [`bridge::begin_login`], which
//! takes the password **by value** and moves it into a worker thread's closure;
//! there is no seam that returns a reference to it, and no field anywhere below
//! this layer that one could be stored in. What comes back is a
//! [`bridge::LoginOutcome`] — a token, a refusal, or a reason — and the token, not
//! the password, is what the shell keeps.
//!
//! # The text fields, and why this file implements `EntityInputHandler` itself
//!
//! **There is no text-input widget in `gpui` at the pinned rev**, and
//! [`crate::ui::views::input_bar`] verified that against the checkout rather than
//! taking it on report: no `elements/input.rs`, and `InputState` exists nowhere.
//! The precedent is the composer — implement [`EntityInputHandler`] over a
//! `String` — and this file follows it for the same reason it exists there: three
//! new dependencies for two single-line fields is not a trade `AGENTS.md` §7.1's
//! supply-chain rule would take.
//!
//! ## Two buffers, one focus handle, and why there is no second one
//!
//! **The obvious shape is two focused elements, and it is the wrong one here.** A
//! field only receives platform input while *its own* handle is the focused one
//! (`Window::handle_input` registers the handler under the handle it is given), so
//! two fields means two handles, two registrations, and a `Tab` handler that has
//! to know which one it is moving between. **One handle, two buffers and a
//! [`Field`] selector is the same behaviour with one registration**, and `Tab`
//! becomes a two-line match instead of a focus dance.
//!
//! **The three mechanisms this file inherits from the composer are load-bearing
//! and are quoted at `input_bar.rs`'s module docs rather than repeated:**
//!
//! | Rule | What breaks without it |
//! |---|---|
//! | character keys must **propagate** | a field that stops propagation is untypeable and reports no error anywhere |
//! | `enter` must **stop** propagation | `with_simulated_ime` synthesises a newline for `enter`, so a propagating submit also types one |
//! | registration happens **during paint** | `Window::handle_input` asserts the paint phase, so it belongs in a `canvas` closure |
//!
//! The third is why this view has a canvas of its own, and why the composer and
//! the login form are **siblings rather than one component**: each owns a handle
//! and a registration, and sharing them would mean one `Field` selector spanning a
//! chat composer and a login form — two unrelated forms wearing one component.
//!
//! # The password is drawn masked, and that is a product decision
//!
//! **A password field that draws its contents is a wrong password field**, so this
//! one draws a bullet per character and never the characters. **The testable
//! consequence is that no test can learn the password from a painted frame** — it
//! is [`LoginView::password`] that answers, which is the same division
//! `input_bar.rs::draft` makes and for the same reason: the platform's text path is
//! an *absence* failure mode, and only asking the view can catch it.
//!
//! # The server's words, and why this view has no login vocabulary of its own
//!
//! [`failure`] turns a [`bridge::LoginOutcome`] into the one line the window shows,
//! and for [`bridge::LoginOutcome::Refused`] that line is the **server's** `detail`
//! and `code` — the same decision `connection_banner.rs` makes for
//! `ConnectionState::Rejected`, and for the same reason: `AGENTS.md` §5.2 asks for
//! a "clear, actionable error", and only the server knows why it said no. A client
//! rendering its own "login failed" would be strictly less useful than one
//! rendering *"that username and password combination was not accepted by this
//! server"*, which is the only half of the sentence the user can act on.
//!
//! **What the view *does* own is the one refusal the server never got to make**: a
//! submit with a blank field. That request cannot succeed on any server, so it is
//! not sent — not to save a round trip, but because a window that says "invalid
//! credentials" to somebody who typed nothing is lying about a credential.

use std::ops::Range;

use gpui::{
    canvas, div, prelude::*, px, App, Bounds, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, Focusable, Hsla, KeyDownEvent, Pixels, Point, Render, TextInputAction,
    TextInputConfiguration, UTF16Selection, Window,
};

use crate::state::bridge::{self, LoginOutcome};
use crate::ui::Colors;

/// The debug selector this view records its bounds under.
///
/// **A *static* selector, for the reason `connection_banner.rs`'s is:** `debug_bounds`
/// takes `&'static str`, so a selector keyed on which field is focused could never
/// be queried. There is at most one login view in a window, so the name has
/// nothing to disambiguate.
pub const SELECTOR: &str = "login-view";

/// The debug selector the username card records its bounds under.
///
/// **Named rather than indexed**, because a test that has to know an index is a test
/// whose failure says `field-0` when the reader needs "the password field".
/// Static, for the reason [`SELECTOR`] gives.
pub const USERNAME_SELECTOR: &str = "login-username";

/// The debug selector the password card records its bounds under. See
/// [`USERNAME_SELECTOR`].
pub const PASSWORD_SELECTOR: &str = "login-password";

/// The debug selector the failure line records its bounds under.
///
/// **A separate selector, and the reason is that the line is conditionally
/// present.** A view that always contributed a row would be indistinguishable from
/// one with a message in it — which is exactly the distinction `tests/login.rs`
/// has to make. See `connection_banner.rs` for the same argument at the same
/// granularity.
pub const FAILURE_SELECTOR: &str = "login-failure";

/// The heading the window shows above the fields.
const HEADING: &str = "Sign in to Sh_Nexus";

/// The label shown in a field that holds nothing.
const USERNAME_PLACEHOLDER: &str = "Username";

/// The label shown in the secret field while it holds nothing.
///
/// **The word is "Password", not "Password (sent in the clear)".** A form that
/// narrates its own transport teaches a user to distrust every one of them, and the
/// fact is recorded where it belongs — `network/rest.rs`'s module docs and
/// ADR-012's implementation notes — rather than shouted at somebody trying to
/// work.
const PASSWORD_PLACEHOLDER: &str = "Password";

/// The instruction that sits below the fields.
const SUBMIT_HINT: &str = "Press Enter to sign in. Tab moves between fields.";

/// The bullet a typed password character is drawn as.
///
/// **A bullet and not an asterisk**, because an asterisk is what a shell prints for
/// a masked prompt and a chat client that looks like a shell prompt is a client
/// somebody types a command into.
const MASK: char = '\u{2022}';

/// The key that submits, as `gpui`'s keystroke parser spells it.
///
/// **A constant for the reason `input_bar.rs` has one**, and the pair matters:
/// `enter` is claimed and `tab` is claimed, and every other key is deliberately
/// not. An arm that stopped propagation for a key it did not handle would swallow
/// the character before the platform ever saw it — a field that is untypeable and
/// reports nothing.
const SUBMIT_KEY: &str = "enter";

/// The key that moves between the two fields. See [`SUBMIT_KEY`].
const NEXT_FIELD_KEY: &str = "tab";

/// The most characters the username field will accept.
///
/// **`sh_nexus_server`'s own `MAX_USERNAME_CHARS`, duplicated rather than
/// approximated.** The server refuses a longer handle on any endpoint that creates
/// an account, so a client accepting one would accept a login this server will
/// never honour — `AGENTS.md` §5.2's unactionable error arriving through the front
/// door. **Counted in characters and not bytes for the same reason the server
/// counts that way**: a 64-character handle of non-ASCII text is more than 64
/// bytes, and a byte bound here would refuse a valid account.
/// `tests/support/mod.rs` duplicates the socket path for the same reason, and
/// `tests/login.rs` proves the two agree against a real server.
pub const MAX_USERNAME_CHARS: usize = 64;

/// The most bytes the password field will accept.
///
/// **Derived from `sh_nexus_server`'s `MAX_LOGIN_BODY_BYTES` (4 KiB), and derived
/// rather than invented.** That constant is the server's own ceiling on the whole
/// login body, so a password past it is refused with a 400 by *any* server this
/// client will ever talk to. **Bounding the field there refuses exactly what the
/// server would refuse and not one byte sooner** — a tighter client bound would be
/// refusing a working credential on the client's own authority.
///
/// **A bound at all, though, because `AGENTS.md` §7.1 bans unbounded in-memory
/// state** and this is a paste target. Somebody pasting a 40 MB file into a
/// password field should get a bounded field rather than 40 MB of main-thread heap.
pub const MAX_PASSWORD_BYTES: usize = 4 * 1024;

/// Which of the two fields typing goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The login handle.
    Username,
    /// The secret.
    Password,
}

impl Field {
    /// The other field, for `Tab`.
    ///
    /// **A two-variant `match` and not index arithmetic**, because `Tab` from the
    /// password field has to land on the *username* field, and a wrap-around over
    /// a list would make that behaviour an accident of declaration order rather
    /// than a decision.
    fn other(self) -> Self {
        match self {
            Self::Username => Self::Password,
            Self::Password => Self::Username,
        }
    }
}

/// What a failed login put on screen.
///
/// **A value rather than a rendered element, for the reason
/// `connection_banner.rs::Announcement` is one:** an `AnyElement` is opaque, so a
/// test holding one learns nothing about the text inside it. The line is built as
/// data and painted separately so that the text is assertable *and* the paint is.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// Exactly the string the window draws.
    pub text: String,
    /// The colour it is drawn in.
    pub color: Hsla,
}

/// What `outcome` means the user should read.
///
/// **`None` for success, and only success.** A login that worked has no news, and
/// the shell is about to replace this view with the chat anyway — so the honest
/// rendering is no line at all. Every failure is something the user cannot work
/// out alone.
///
/// **The colour split is why this is a function and not an `if` at the call
/// site.** `Refused` is `danger`: the server has answered and the answer is no,
/// and the same two strings will not help on a retry. `Failed` is `text_muted`:
/// nothing was decided, the server was not reached or not understood, and painting
/// that in the failure colour would report a rejected credential that never
/// happened.
///
/// **The refusal line is the server's `detail` with its `code` and status beside
/// it**, exactly as `connection_banner.rs` renders `ConnectionState::Rejected`:
/// `detail` is the sentence to act on and `code` is the handle to quote in a bug
/// report. The status is in the line too, because a 400 and a 500 look identical
/// from the `detail` alone and they are fixed by different people.
pub fn failure(outcome: &LoginOutcome, colors: Colors) -> Option<Failure> {
    let (text, color) = match outcome {
        LoginOutcome::LoggedIn { .. } => return None,
        LoginOutcome::Refused {
            status,
            code,
            detail,
        } => (format!("{detail} (HTTP {status}, {code})"), colors.danger),
        LoginOutcome::Failed { reason } => (reason.clone(), colors.text_muted),
    };
    Some(Failure { text, color })
}

/// The message shown when a submit was refused before it left this machine.
///
/// **Separate from [`failure`] on purpose.** Everything [`failure`] renders came
/// from a server that made a decision; this did not, and it is the one message in
/// this view whose text is the client's own. It exists because the alternative is a
/// window telling somebody they typed the wrong password when they typed nothing.
fn incomplete(username: &str, password: &str, colors: Colors) -> Option<Failure> {
    if !username.trim().is_empty() && !password.is_empty() {
        return None;
    }
    let text = if username.trim().is_empty() {
        "Enter a username before pressing Enter."
    } else {
        "Enter a password before pressing Enter."
    };
    Some(Failure {
        text: text.to_owned(),
        color: colors.danger,
    })
}

/// The login screen.
///
/// **Constructed with the endpoint and nothing else, and that is the shape of the
/// configuration decision.** `SH_NEXUS_URL` names a WebSocket endpoint; the login
/// URL is derived from it inside the REST module. The view is handed the endpoint
/// and never asked to assemble a URL, because a view that could build one would be
/// a second place the two spellings of "where the server is" could disagree.
pub struct LoginView {
    /// The configured WebSocket endpoint, shown in the window so a misconfigured
    /// client says which server it is trying.
    endpoint: String,
    /// What the user has typed in [`Field::Username`].
    username: String,
    /// What the user has typed in [`Field::Password`].
    password: String,
    /// Which buffer typing goes into.
    active: Field,
    /// The last refusal, if any.
    ///
    /// **`Option`, not a log.** A retry replaces it, a successful login drops it
    /// with the view, and there is no history: `AGENTS.md` §7.1 bans unbounded
    /// in-memory state, and a list of past failures is unbounded by nature.
    failure: Option<Failure>,
    /// The palette every element draws with, for `app::theme_colors`'s reason: a
    /// frame copies a few hundred bytes rather than reaching into a parsed document.
    colors: Colors,
    /// The focus target, and the registration key for the platform's text input.
    ///
    /// **One handle for both fields**, for the reason the module docs give.
    focus_handle: FocusHandle,
}

impl LoginView {
    /// Builds an empty login screen for `endpoint`.
    ///
    /// **Starts with the username field active**, which is the order a person types
    /// in and the reason `Tab` exists rather than a Shift modifier.
    pub fn new(endpoint: impl Into<String>, colors: Colors, cx: &mut Context<Self>) -> Self {
        Self {
            endpoint: endpoint.into(),
            username: String::new(),
            password: String::new(),
            active: Field::Username,
            failure: None,
            colors,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The server this screen will try, for the window's own line and for a test.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// What the user has typed as a handle, for a test to assert on without a window.
    ///
    /// **Exists because the alternative could not catch the failure mode.** A
    /// character key that does not propagate reaches neither the painted frame nor
    /// the platform handler, so a test that walked the element tree would see a
    /// correct-looking form that silently accepts nothing. Asking the view is the
    /// only thing that distinguishes the two.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// What the user has typed as a secret, for a test to assert on.
    ///
    /// **This is the one accessor in the crate that returns a plaintext
    /// credential, and it exists for a test and nothing else.** It is a `&str`
    /// borrow rather than an owned copy, it is printed nowhere, and
    /// [`LoginOutcome`]'s hand-written `Debug` exists in part so that no `{:?}` of
    /// this view could route one into a log. `tests/login.rs` asserts both halves
    /// of that: what the field received, and that no `Debug` output carries it.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Which field typing is going into, for a test to assert that `Tab` moves it.
    pub fn active_field(&self) -> Field {
        self.active
    }

    /// The failure line currently on screen, if any.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Whether the screen is showing a failure right now.
    ///
    /// **A named question rather than `failure().is_some()` repeated at four call
    /// sites**, which is the shape `bridge.rs`'s `FlushReport::is_empty` exists for.
    pub fn has_failure(&self) -> bool {
        self.failure.is_some()
    }

    /// Fills the username field.
    ///
    /// **A seam the headless harness uses, and it is not named `*_for_test` because
    /// the harness is not the only thing that could want it**: a method that says
    /// `set_` says that without claiming to be test-only. It is **not** part of the
    /// gesture path — typing arrives through
    /// [`EntityInputHandler::replace_text_in_range`].
    pub fn set_username(&mut self, username: impl Into<String>, cx: &mut Context<Self>) {
        self.username = take_chars(&username.into(), MAX_USERNAME_CHARS);
        self.failure = None;
        cx.notify();
    }

    /// Fills the password field. See [`Self::set_username`] for why this exists.
    pub fn set_password(&mut self, password: impl Into<String>, cx: &mut Context<Self>) {
        self.password = truncate_at_boundary(&password.into(), MAX_PASSWORD_BYTES);
        self.failure = None;
        cx.notify();
    }

    /// Shows what a finished attempt said.
    ///
    /// **The shell calls this, because the shell owns the mode.** A successful login
    /// takes the whole window somewhere else, and a view cannot decide that about
    /// itself — so it answers "here is the line to draw" and the shell answers "here
    /// is what happens now". `bridge.rs` §7 says the same thing about every other
    /// door: this seam reports, it does not decide.
    ///
    /// **The password is dropped here, on success.** A session that was issued makes
    /// the secret worthless, and this is the last instruction the view has that
    /// could forget it — so "logged in" and "the secret is gone" are the same event
    /// rather than two things that can drift apart.
    pub fn show(&mut self, outcome: LoginOutcome, cx: &mut Context<Self>) {
        if matches!(outcome, LoginOutcome::LoggedIn { .. }) {
            self.password.clear();
        }
        self.failure = failure(&outcome, self.colors);
        cx.notify();
    }

    /// Handles the two keys this view owns, and leaves every other key alone.
    ///
    /// **Both claimed keys stop propagation, and for `tab` that is not optional
    /// either** — which a test found, and which is the reason it is written down
    /// here rather than left as a plausible asymmetry. `with_simulated_ime`
    /// synthesises a `key_char` for named keys — `"enter" => Some("\n")` *and*
    /// `"tab" => Some("\t")` (`gpui/src/platform/keystroke.rs:242`) — so a
    /// propagating `Tab` would move to the password field **and then type a tab
    /// character into it**. The password would become `"\tcorrect-horse"` and the
    /// login would be refused for a reason the user cannot see.
    /// `tests/login.rs::tab_moves_between_the_two_fields_and_enter_does_not_type_a_newline`
    /// is the test that pins both halves.
    ///
    /// **Losing the ability to type a literal tab costs nothing here**, and that is
    /// the honest test of the trade: a single-line login form has no use for one, and
    /// a password containing a tab is not a thing anybody has.
    ///
    /// **Everything else returns without touching propagation**, and the empty
    /// default arm is the load-bearing part — see [`SUBMIT_KEY`].
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            SUBMIT_KEY => {
                cx.stop_propagation();
                self.submit(cx);
            }
            NEXT_FIELD_KEY => {
                cx.stop_propagation();
                self.active = self.active.other();
                cx.notify();
            }
            _ => {}
        }
    }

    /// Asks the seam to log in, if there is anything to log in with.
    ///
    /// **The view does not call the network and does not hold the answer**, and that
    /// is the whole of its role: `PLAN.md` §4 gives `ui/` the seam and the pure
    /// domain modules, so the one request this form makes is a call on
    /// [`bridge::begin_login`]. The result arrives later, as a
    /// [`LoginOutcome`] the shell takes with
    /// [`bridge::try_take_login_outcome`] — **which is what makes this method
    /// non-blocking, and therefore safe to call from a key handler**
    /// (`AGENTS.md` §2.3).
    ///
    /// **A blank field is refused here and drawn immediately**, for the reason
    /// [`incomplete`] gives.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if let Some(refusal) = incomplete(&self.username, &self.password, self.colors) {
            self.failure = Some(refusal);
            cx.notify();
            return;
        }

        self.failure = None;
        // The password is **moved**, not cloned. This is the line where the view
        // stops holding it: whatever `begin_login` refuses or fails to do, the
        // buffer here is emptied either way, so a window that failed to start a
        // request does not sit holding a secret it has no use for.
        let password = std::mem::take(&mut self.password);
        let started = bridge::begin_login(cx, &self.endpoint, &self.username, password);
        if !started {
            self.failure = Some(Failure {
                text: "A sign-in attempt is already running, or this window could not start \
                       one. Try again in a moment."
                    .to_owned(),
                color: self.colors.text_muted,
            });
        }
        cx.notify();
    }

    /// The buffer typing currently goes into.
    fn active_buffer(&self) -> &str {
        match self.active {
            Field::Username => &self.username,
            Field::Password => &self.password,
        }
    }

    /// The username card's line: the typed handle, or the placeholder.
    fn username_line(&self) -> (String, Hsla) {
        if self.username.is_empty() {
            (USERNAME_PLACEHOLDER.to_owned(), self.colors.text_muted)
        } else {
            (self.username.clone(), self.colors.text)
        }
    }

    /// The secret card's line: one bullet per character, never the characters.
    ///
    /// **The empty case shows the placeholder**, because a field showing nothing at
    /// all is indistinguishable from one that swallowed the input — and this is the
    /// second field on a form whose first one does show text.
    fn masked(&self) -> String {
        if self.password.is_empty() {
            PASSWORD_PLACEHOLDER.to_owned()
        } else {
            std::iter::repeat_n(MASK, self.password.chars().count()).collect::<String>()
        }
    }
}

impl Focusable for LoginView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The byte offset of a UTF-16 offset within `text`.
///
/// **The composer's own helper, duplicated rather than shared, and the reason is
/// that sharing it would be a boundary crossing for two dozen lines.** It converts
/// a platform offset into a byte index, and a byte index in the middle of a `char`
/// would make the caller's slice **panic** — so it answers `None` at a surrogate
/// boundary and the caller drops the edit. A `ui/` helper reached from two views
/// is a module neither of them owns; when a third field appears, a
/// `ui/text_field.rs` becomes worth its own file and this moves there.
fn utf16_to_byte(text: &str, utf16_offset: usize) -> Option<usize> {
    let mut seen = 0;
    for (byte, character) in text.char_indices() {
        if seen == utf16_offset {
            return Some(byte);
        }
        seen += character.len_utf16();
    }
    (seen == utf16_offset).then_some(text.len())
}

/// The byte range of `text` covering a UTF-16 range. See [`utf16_to_byte`].
fn utf16_range_to_bytes(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    Some(utf16_to_byte(text, range.start)?..utf16_to_byte(text, range.end)?)
}

/// The longest prefix of `text` that fits in `room` bytes without splitting a
/// character. See [`utf16_to_byte`] for why that matters more than it looks.
fn truncate_at_boundary(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_owned();
    }
    let mut end = room;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The first `limit` characters of `text`.
///
/// **Characters and not bytes, for the reason [`MAX_USERNAME_CHARS`] gives**: a
/// byte bound would refuse a valid non-ASCII handle.
fn take_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

impl EntityInputHandler for LoginView {
    /// The text covering a UTF-16 range, for the platform to read.
    ///
    /// **Implemented because it is one line and it is what makes a real IME
    /// candidate window work.** The platform calls this to find out what it is about
    /// to replace; answering `None` says "the platform cannot find out what the user
    /// typed".
    ///
    /// **The password is returned here, and that is unavoidable.** The platform owns
    /// the string the user typed into this field — it holds it for the IME and for
    /// clipboard handling — and this is the only callback that hands text back.
    /// Refusing would be refusing to be a text field.
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let active = self.active_buffer();
        let bytes = utf16_range_to_bytes(active, range)?;
        Some(active[bytes].to_owned())
    }

    /// Always `None`: neither field has a selection.
    ///
    /// **The caret is wherever the last insertion put it — the end — so reporting
    /// one would be reporting a highlight the user cannot move or dismiss.**
    /// `gpui/src/input.rs:420`'s reference view answers the same way.
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
    /// **This view does not model an IME composition**, so a composing IME is treated
    /// as plain insertion — see
    /// [`EntityInputHandler::replace_and_mark_text_in_range`], which says the same
    /// thing about the consequence.
    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    /// Nothing to unmark. See [`EntityInputHandler::marked_text_range`].
    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Inserts or replaces text. **This is the typing path.**
    ///
    /// `dispatch_keystroke` reaches this with `range: None` for every character key
    /// that propagated, and [`EntityInputHandler::paste`]'s default reaches it with
    /// `None` too, so **`None` is the common case and it means "append"**.
    ///
    /// **The bounds are enforced here rather than at the submit**, and that
    /// placement is the point: typing and pasting both arrive through this one
    /// method, so a bound here bounds everything. A bound at the submit would let a
    /// pasted megabyte sit on the main thread until the user pressed `Enter`, which
    /// is `AGENTS.md` §7.1's unbounded growth with a keystroke between it and the
    /// user.
    ///
    /// **The secret's bound is in bytes and the handle's in characters**, because
    /// the two upstream limits are in different units — see [`MAX_PASSWORD_BYTES`]
    /// and [`MAX_USERNAME_CHARS`].
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.active {
            Field::Username => {
                let room = MAX_USERNAME_CHARS.saturating_sub(self.username.chars().count());
                let accepted: String = text.chars().take(room).collect();
                if accepted.is_empty() {
                    return;
                }
                match range {
                    None => self.username.push_str(&accepted),
                    Some(range) => {
                        if let Some(bytes) = utf16_range_to_bytes(&self.username, range) {
                            self.username.replace_range(bytes, &accepted);
                        }
                    }
                }
            }
            Field::Password => {
                let room = MAX_PASSWORD_BYTES.saturating_sub(self.password.len());
                let accepted = truncate_at_boundary(text, room);
                if accepted.is_empty() {
                    return;
                }
                match range {
                    None => self.password.push_str(&accepted),
                    Some(range) => {
                        if let Some(bytes) = utf16_range_to_bytes(&self.password, range) {
                            self.password.replace_range(bytes, &accepted);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Inserts text and reports no composition.
    ///
    /// **This view does not model an IME composition, and the consequence is stated
    /// rather than hidden:** because [`EntityInputHandler::marked_text_range`] is
    /// `None`, an IME composing several keystrokes into one character has each
    /// intermediate update applied as an ordinary insertion, so a composing IME
    /// will appear to insert each intermediate state. Modelling composition means
    /// holding the marked range, which is the editor machinery the module docs
    /// rejected as a dependency.
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

    /// Always `None`: this view does not know where its text is on screen.
    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    /// Always `None`: clicking does not move a caret.
    ///
    /// **There is no caret to move**, and clicking a field is not a gesture this view
    /// implements: [`SUBMIT_KEY`] and [`NEXT_FIELD_KEY`] are its two keys and `Tab`
    /// is its field-to-field gesture. `None` is the framework's "no opinion", so a
    /// click changes nothing rather than moving an insertion point the user cannot
    /// see.
    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    /// The active buffer's length in UTF-16 units, which is the platform's unit.
    ///
    /// **Counted, and counted in the right unit.** The platform measures text in
    /// UTF-16 code units, so a non-BMP character counts as two; reporting
    /// `str::len()`, which is UTF-8 bytes, would make every such buffer look longer
    /// to the platform than it is.
    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.active_buffer().encode_utf16().count())
    }

    /// Advertises a send key on a software keyboard.
    ///
    /// **`TextInputAction::Send`, and the platform's own documentation is what makes
    /// this a presentation choice rather than a behaviour**: the variant *"affects
    /// only how the key is presented; pressing it is still delivered as ordinary
    /// input"*. Nothing here is load-bearing — the key routes to
    /// [`LoginView::on_key_down`] either way — and the reason to set it is that the
    /// alternative labels a software keyboard's action key "insert newline" on a
    /// field that cannot hold one.
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

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Two clones per frame, and both are what the paint-time registration
        // needs: the handle `handle_input` is keyed on, and the entity handed to the
        // platform as the handler's target. `Context::entity` is an `Arc` clone, so
        // this is not a deep copy of anything.
        let focus_handle = self.focus_handle.clone();
        let view = cx.entity();

        // Read **before** the tree is built, for the reason `Shell::render` gives:
        // an `AnyElement` is opaque and a borrow cannot outlive the frame, so both
        // the label and the mask are materialised here rather than inside a
        // closure the frame borrows from. Three short copies per frame of a form
        // that is on screen for seconds at a time is not a cost worth designing
        // around.
        let (username_line, username_color) = self.username_line();
        let masked = self.masked();

        div()
            .id("login-view")
            .key_context("LoginView")
            .debug_selector(|| SELECTOR.to_owned())
            .flex()
            .flex_col()
            .size_full()
            .bg(self.colors.background)
            // Explicit on the container as well as on the text below, per
            // `AGENTS.md` 7.3: GPUI inherits nothing, and a form whose labels
            // depend on a parent's colour is a form with one unreadable label.
            .text_color(self.colors.text)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                // The form itself: a centred column, because a login reads top to
                // bottom and a row of two fields side by side is a filter bar.
                div()
                    .flex()
                    .flex_col()
                    .items_stretch()
                    .justify_center()
                    .w_full()
                    .max_w(px(420.0))
                    .gap_3()
                    .px_6()
                    .py_6()
                    .mx_auto()
                    .text_color(self.colors.text)
                    // The registration canvas, first, and `absolute` so it takes
                    // no part in the layout: it exists only to run one closure
                    // during paint, and a canvas that occupied space would be a
                    // zero-content row between the heading and the fields.
                    .child(
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
                        .absolute()
                        .size_full(),
                    )
                    .child(div().text_lg().text_color(self.colors.text).child(HEADING))
                    // The endpoint, in the muted colour and above the fields: a
                    // person whose `SH_NEXUS_URL` is wrong needs to read it before
                    // they can decide what to type.
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.colors.text_muted)
                            .child(self.endpoint.clone()),
                    )
                    .child(field(
                        USERNAME_SELECTOR,
                        "login-username-field",
                        username_line,
                        username_color,
                        USERNAME_PLACEHOLDER,
                        self.colors,
                    ))
                    .child(field(
                        PASSWORD_SELECTOR,
                        "login-password-field",
                        masked,
                        self.colors.text,
                        PASSWORD_PLACEHOLDER,
                        self.colors,
                    ))
                    // The failure line, conditionally. `Render` cannot return
                    // nothing, so `when_some` is what makes "no line when nothing
                    // failed" true of the element tree rather than of a value, and
                    // `debug_bounds` is what makes it observable from a painted
                    // frame.
                    .when_some(self.failure.clone(), |column, refusal| {
                        column.child(
                            div()
                                .id("login-failure")
                                .debug_selector(|| FAILURE_SELECTOR.to_owned())
                                .flex()
                                .flex_row()
                                .items_center()
                                .text_sm()
                                .text_color(refusal.color)
                                .child(div().text_color(refusal.color).child(refusal.text)),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.colors.text_muted)
                            .child(SUBMIT_HINT),
                    ),
            )
    }
}

/// One labelled card: the value line and the placeholder under it.
///
/// **A function rather than two inline trees, and the reason is that a login form's
/// two cards are the same element with different content** — and the placeholder
/// being *under* the value rather than in it is a deliberate shape, not a repeated
/// `placeholder` attribute: this framework has no placeholder element, so a field
/// that is empty has to say what belongs in it some other way.
fn field(
    selector: &'static str,
    id: &'static str,
    value: String,
    value_color: Hsla,
    placeholder: &'static str,
    colors: Colors,
) -> impl IntoElement {
    div()
        .id(id)
        .debug_selector(|| selector.to_owned())
        .flex()
        .flex_col()
        .w_full()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(colors.surface)
        .px_3()
        .py_2()
        .text_color(colors.text)
        .child(div().text_sm().text_color(value_color).child(value))
        .child(
            div()
                .text_sm()
                .text_color(colors.text_muted)
                .child(placeholder),
        )
}
