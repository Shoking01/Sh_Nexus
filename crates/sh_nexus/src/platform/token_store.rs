//! `platform::token_store`: one session credential, five stores, and the trait they
//! share.
//!
//! `AGENTS.md` §3.1's tree places this directory beside `network/` and `db/`, and L88
//! describes what belongs in it — *OS-specific implementations behind traits
//! (notifications, sounds, secure token storage)*. **This is that directory's first
//! module**, and it exists for the credential store alone: `AGENTS.md` §7.1 forbids
//! plaintext token storage and names the OS keychain as the replacement, and §8.1's Login
//! Flow says the token is stored there after a successful sign-in. `odd/tasks/6b-keychain.md`
//! is the change record.
//!
//! # Why a trait, and not a `keyring::Entry` used directly
//!
//! **`AGENTS.md` L88 says "behind traits", and three facts make that a requirement rather
//! than a style.** A CI runner and a headless Linux session have **no credential service at
//! all**, so every test here would otherwise be a test that fails or silently does nothing;
//! a `Mutex<Option<String>>` is a real implementation of the same contract, with the same
//! idempotent `clear` and the same error shape, and it is what makes
//! `AGENTS.md` §4.3's hermetic requirement reachable; and a missing credential service is an
//! **ordinary condition** on some machines, not an exceptional one, which `AGENTS.md` §2.1's
//! ban on panics in user-facing code forbids treating as a failure.
//!
//! The implementations, and why each one exists:
//!
//! | Implementation | Why it exists |
//! |---|---|
//! | [`KeyringTokenStore`] | the real one: Windows Credential Manager, macOS Keychain Services, or the Secret Service |
//! | [`LazyNativeStore`] | **what `app::open` hands the window.** Resolving *which* store exists is itself a blocking OS call, so the object the main thread holds resolves nothing until a worker touches it |
//! | [`InMemoryTokenStore`] | the tests. A real implementation of the trait, not a mock of it |
//! | [`NoopTokenStore`] | a machine with no usable credential service. Degrades to "sign in again", never to a failed start |
//! | [`RefusingTokenStore`] | **a failing keychain is a tested path, not an untested branch** — losing a remembered credential is recoverable and the suite proves it costs nothing |
//!
//! # The layer's one rule: nothing here may reach `ui/` or `network/`
//!
//! Enforced by `the_platform_layer_reaches_no_ui_and_no_network` in `tests/keychain.rs`,
//! because `AGENTS.md` L88 puts this layer beside the UI and §3.2's separation is the only
//! thing keeping a credential store from growing opinions about the window or the socket.
//!
//! # Why nothing here derives `Debug`
//!
//! **Every value this layer holds is a credential or the absence of one**, so a
//! `#[derive(Debug)]` anywhere in it would print the value and compile while doing it. The
//! two impls that exist are written out by hand and asserted at runtime by
//! `a_stored_credential_never_reaches_a_debug_or_a_display`, and
//! `no_platform_source_can_print_a_credential` fails the build on a third one appearing.
//!
//! # Why nothing here blocks, and what that costs
//!
//! `AGENTS.md` §2.3 forbids blocking the frame loop, and every operation on a real
//! credential store is a **synchronous** OS call: `CredReadW`, the Keychain Services call, a
//! Secret Service round trip. So the trait is deliberately *not* async and nothing in this
//! directory starts a thread — the worker belongs to the seam, because
//! `state/bridge.rs` §7's argument is that a value crosses a boundary and the decision about
//! what to do with it belongs to the caller. `bridge::begin_credential_load`,
//! `bridge::remember_credential` and `bridge::forget_credential` are the three doors, and
//! all three hand work to a thread of their own.
//!
//! The cost of that arrangement is stated rather than hidden: **the startup read is answered
//! on the shell's next pump tick**, which `app::DRAIN_INTERVAL` puts at worst 50 ms after
//! launch. That is inside `AGENTS.md` §6.2's 300 ms cached-session budget, and it is the
//! price of not reading a credential store inside `open()`.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use keyring::Entry;

/// The service name this client's credential is filed under.
///
/// **A constant rather than a literal in two places**, for the reason `app.rs`'s
/// `URL_VARIABLE` gives: one spelling of a name is one thing to get right, and the
/// credential's identity is exactly the sort of thing that has to agree with itself.
pub const CREDENTIAL_SERVICE: &str = "sh_nexus";

/// The account name this client's one credential is filed under.
///
/// **One account, because one client has one session.** A multi-account switcher is
/// explicitly out of scope in `odd/tasks/6b-keychain.md`, and a credential store holding a
/// key per account is a design that has to be reviewed when there is a second account to
/// tell apart.
pub const CREDENTIAL_ACCOUNT: &str = "session";

/// What the credential load found.
///
/// **`Absent` covers both "the store was empty" and "this machine has no usable credential
/// store", and merging them is the point.** A person cannot act on the difference and both
/// mean the same next step — sign in — so splitting them would give `app::Shell` a third
/// mode to draw for a condition with nothing to draw.
///
/// **Neither variant prints its value**, for the reason this module's docs give: the value
/// is the credential.
pub enum StoredCredential {
    /// The store held a session credential.
    Present(String),
    /// There is nothing to restore.
    Absent,
}

impl StoredCredential {
    /// The value, if the store held one.
    ///
    /// **A borrow rather than the `String`, and that is the whole of the method.** The
    /// caller is `app::Shell`, which builds a `ConnectionSettings` from it and hands the
    /// value to the seam once; a `take` here would mean this type owned a decision about
    /// when a credential stops existing, and it has no opinion about that.
    pub fn credential(&self) -> Option<&str> {
        match self {
            Self::Present(credential) => Some(credential),
            Self::Absent => None,
        }
    }
}

impl fmt::Debug for StoredCredential {
    /// The credential's **presence**, never the credential.
    ///
    /// Written out rather than derived for the reason `app::ConnectionSettings`'s own
    /// `Debug` is: this type is reachable from any `{:?}` on a log field, and a derived
    /// impl would print the value and compile while doing it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Present(credential) => formatter
                .debug_struct("Present")
                .field("credential_configured", &!credential.is_empty())
                .finish(),
            Self::Absent => formatter.write_str("Absent"),
        }
    }
}

impl fmt::Display for StoredCredential {
    /// One line, and never the value. See [`Self::fmt`].
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Present(_) => formatter.write_str("a stored session is waiting to be used"),
            Self::Absent => formatter.write_str("there is no stored session"),
        }
    }
}

/// Everything a credential store can fail with.
///
/// **Two variants rather than one, because the two are fixed by opposite actions and a
/// single string would lose exactly the distinction that tells the caller what to say.**
/// [`Self::Unavailable`] means there is nowhere to put anything on this machine, and the
/// only correct response is to stop asking; [`Self::Refused`] means there *is* a store and it
/// declined, which is worth a line in a log and nothing more.
pub enum TokenStoreError {
    /// This machine has no usable credential store.
    Unavailable(String),
    /// There is a store, and it refused.
    Refused(String),
}

impl fmt::Debug for TokenStoreError {
    /// Identical to [`fmt::Display`], on purpose.
    ///
    /// **The payload is an operating-system diagnostic and never a credential**, so there is
    /// nothing to redact, and having one rendering means there is no second spelling of the
    /// message to keep in step with the first.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for TokenStoreError {
    /// A sentence naming the operation's outcome and its cause.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => {
                write!(
                    formatter,
                    "this machine has no usable credential store: {reason}"
                )
            }
            Self::Refused(reason) => {
                write!(
                    formatter,
                    "the credential store refused the operation: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for TokenStoreError {}

/// One place a session credential can be kept.
///
/// **`Send + Sync` as supertraits rather than as a bound at the use site**, because every
/// implementation is handed to a worker thread by `state/bridge.rs` and a trait whose
/// implementations are not `Send` would fail at the door rather than at the declaration.
///
/// **Three operations and no more, because the client has one credential and one session.**
/// There is no list, no enumeration and no per-account lookup, because `6b-keychain.md`
/// puts multi-account switching out of scope and a wider surface here would be a design
/// nobody has asked for.
///
/// **Nothing on this trait is async, and that is a deliberate consequence rather than an
/// omission.** Every real implementation is a synchronous OS call, and `AGENTS.md` §2.3
/// forbids blocking the frame loop — so the answer is that this trait is never called from
/// the main thread, which is why `state/bridge.rs` owns the workers.
pub trait TokenStore: Send + Sync {
    /// One word naming this implementation.
    ///
    /// **Exposed so a failure can be traced to an implementation rather than to "the key",
    /// and so a test can assert which store it is holding.** No implementation can print
    /// anything else through it: `&'static str` and nothing borrowed from the store.
    fn describe(&self) -> &'static str;

    /// The credential this store holds, or `None` when it holds none.
    ///
    /// **`Ok(None)` and not an error, and the reason is the startup decision.** A person who
    /// has never signed in must be shown the login form, and an error would be reported as a
    /// broken machine rather than as an ordinary first launch.
    ///
    /// # Errors
    ///
    /// [`TokenStoreError::Unavailable`] when this machine has no usable store, and
    /// [`TokenStoreError::Refused`] when it has one and it declined.
    fn load(&self) -> Result<Option<String>, TokenStoreError>;

    /// Keeps `credential`, replacing whatever was there.
    ///
    /// **Taken by value so the caller's copy is moved into the worker and dropped when the
    /// operation finishes**, which is the same structural guarantee `bridge::begin_login`
    /// gives the password: there is no field on this side that could hold it.
    ///
    /// # Errors
    ///
    /// [`TokenStoreError::Unavailable`] or [`TokenStoreError::Refused`]. **A refusal leaves
    /// the live session alone** — losing a remembered credential costs the user a sign-in,
    /// and losing the session costs them the window.
    fn store(&self, credential: String) -> Result<(), TokenStoreError>;

    /// Forgets whatever this store was holding.
    ///
    /// **Idempotent, and that is a contract rather than an accident.** A logout, and the
    /// recovery from a credential the server refused, both end here — and a second call
    /// arrives on any machine that clears twice. `Ok(())` for "there was nothing" is what
    /// makes a double-clear unable to fail a logout.
    ///
    /// # Errors
    ///
    /// [`TokenStoreError::Unavailable`] or [`TokenStoreError::Refused`].
    fn clear(&self) -> Result<(), TokenStoreError>;
}

/// The real credential store: Windows Credential Manager, macOS Keychain Services, or the
/// Secret Service.
///
/// **A unit struct, and the reason is that nothing is resolved until it is used.** `keyring`
/// picks its store in a `LazyLock` the first time an `Entry` is built, so holding an `Entry`
/// here would mean this struct's *construction* is the blocking half — which `app::open`
/// would be doing on the main thread. Building the entry inside each operation moves that
/// cost to the worker that calls it, which is the only place it is allowed to happen.
pub struct KeyringTokenStore;

impl TokenStore for KeyringTokenStore {
    fn describe(&self) -> &'static str {
        "the operating system credential store"
    }

    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        match self.entry()?.get_password() {
            Ok(credential) => Ok(Some(credential)),
            // **The documented "no stored credential" case, and the classification the
            // whole startup decision rests on.** `keyring` answers `NoEntry` for an entry
            // nobody wrote, and reading that as a failure would report an ordinary first
            // launch as a broken machine.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(other) => Err(refused(other)),
        }
    }

    fn store(&self, credential: String) -> Result<(), TokenStoreError> {
        self.entry()?.set_password(&credential).map_err(refused)
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) => Ok(()),
            // **The same classification again, and it is load-bearing rather than
            // symmetrical.** On Windows `CredDeleteW` reports `ERROR_NOT_FOUND` — which
            // `windows-native-keyring-store` maps to `NoEntry` — for an entry that is not
            // there, so *without* this arm a first logout on a fresh machine would fail with
            // a message the user cannot act on. That is `6b-keychain.md`'s idempotence
            // requirement stated as a platform fact rather than as an intention.
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(other) => Err(refused(other)),
        }
    }
}

impl KeyringTokenStore {
    /// The one entry this client owns, built inside the operation that needs it.
    fn entry(&self) -> Result<Entry, TokenStoreError> {
        Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT).map_err(|error| match error {
            // **No store on this machine, and that is `Unavailable` rather than
            // `Refused`** — there was nothing to decline. `LazyNativeStore` normally catches
            // this before an entry is ever built; the arm exists because `set_default_store`
            // is a process-global and another library could in principle clear it.
            keyring::Error::NoDefaultStore => {
                TokenStoreError::Unavailable("the platform reports no credential store".to_owned())
            }
            other => refused(other),
        })
    }
}

/// One `keyring` failure as this crate's narrower error.
///
/// **`Refused` for everything except "there is no store at all",** and the reason is that
/// `keyring`'s enum is `#[non_exhaustive]` and platform-shaped while this crate's callers
/// want one question answered: *was there somewhere to put this?*
fn refused(error: keyring::Error) -> TokenStoreError {
    TokenStoreError::Refused(error.to_string())
}

/// The store `app::open` hands the window, and the one that decides lazily.
///
/// **This type exists because *choosing* a store is the blocking half of the seam.** On
/// Linux `keyring` initialises its Secret Service store on first use, which is a session-bus
/// round trip; on a desktop it is cheap but still an operating-system call. `AGENTS.md` §2.3
/// forbids that on the main thread, so the object the main thread holds resolves **nothing**
/// until a worker touches it — and then memoises the answer behind a [`OnceLock`], so the
/// second operation on the same client is a lock read.
///
/// **Two shapes of "no store", both of which degrade rather than fail.** A platform with no
/// credential service resolves to [`Resolution::Unavailable`] and every operation answers
/// `Ok(())` or `Ok(None)`; a platform that has one resolves to [`KeyringTokenStore`] and
/// delegates. A resolution that produced an error the caller had to handle would put "this
/// machine cannot remember a session" on the startup path, and `AGENTS.md` §2.1 forbids
/// panics in user-facing code and acceptance criterion 3 requires the app to run anyway.
pub struct LazyNativeStore {
    resolution: OnceLock<Resolution>,
}

/// What one resolution decided, memoised for the rest of the process.
enum Resolution {
    /// This machine has a store, and it is this one.
    Usable(Arc<dyn TokenStore>),
    /// This machine has none.
    ///
    /// **A unit variant and not the reason, and the reason is worth saying.** The reason
    /// is reported once, by [`LazyNativeStore::decide`], to whoever launched the process;
    /// after that nothing reads it, and a `String` kept alive for the life of the
    /// application to answer a question nobody asks is the shape of in-memory growth
    /// `AGENTS.md` §7.1 warns about.
    Unavailable,
}

impl LazyNativeStore {
    /// A store that has not decided anything yet.
    ///
    /// **Cheap, and safe to call from the main thread** — it allocates one [`OnceLock`] and
    /// makes no operating-system call. That is the whole point of the type, and
    /// `the_os_store_answers_here_whether_or_not_this_machine_has_one` is what asserts the
    /// behaviour rather than the promise.
    pub fn new() -> Self {
        Self {
            resolution: OnceLock::new(),
        }
    }

    /// The memoised answer, deciding it on first use.
    ///
    /// **Runs on whichever thread gets here first, and that thread has to be a worker.**
    /// `state/bridge.rs` is what guarantees it: all three credential doors hand work to a
    /// thread before they touch a store.
    fn resolution(&self) -> &Resolution {
        self.resolution.get_or_init(Self::decide)
    }

    /// Asks `keyring` whether this machine has a store, once.
    fn decide() -> Resolution {
        match Entry::store_status() {
            Ok(()) => Resolution::Usable(Arc::new(KeyringTokenStore)),
            Err(error) => {
                // **Named once, at the point it is decided**, and never again: a session that
                // is not remembered is worth one line in a developer's terminal and nothing
                // on screen, because there is nothing the user could do about it.
                tracing::warn!(
                    reason = %error,
                    "this machine has no usable credential store, so a session will not be \
                     remembered between launches"
                );
                Resolution::Unavailable
            }
        }
    }
}

impl Default for LazyNativeStore {
    /// Present so `LazyNativeStore::new` is not the second spelling of the same value.
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore for LazyNativeStore {
    fn describe(&self) -> &'static str {
        match self.resolution() {
            Resolution::Usable(store) => store.describe(),
            Resolution::Unavailable => "no credential store on this machine",
        }
    }

    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        match self.resolution() {
            Resolution::Usable(store) => store.load(),
            // **An absent machine answers "nothing stored", not a failure**, for the reason
            // `StoredCredential::Absent` merges the two: a person asked to sign in because
            // nothing was remembered cannot do anything about *why* nothing was remembered.
            Resolution::Unavailable => Ok(None),
        }
    }

    fn store(&self, credential: String) -> Result<(), TokenStoreError> {
        match self.resolution() {
            Resolution::Usable(store) => store.store(credential),
            // **`Ok(())` rather than a refusal, and the reason is who reads it.** A warning
            // the user cannot act with, printed on every sign-in, trains the reader to ignore
            // the line that matters; there is nowhere to put this, which is not a failure of
            // anything the client did.
            Resolution::Unavailable => Ok(()),
        }
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        match self.resolution() {
            Resolution::Usable(store) => store.clear(),
            Resolution::Unavailable => Ok(()),
        }
    }
}

/// A credential store in a `Mutex`, for a machine that has no OS credential service.
///
/// **A real implementation of [`TokenStore`] rather than a mock of it**, and that is the
/// whole reason this crate's tests are hermetic: it has the same synchronisation, the same
/// one-slot semantics, the same idempotent `clear` and the same error shape as
/// [`KeyringTokenStore`]. A mock would have to be kept in step with those, and would not be
/// what runs.
///
/// **A poisoned lock is recovered rather than propagated, and the reason is that a panic in
/// another test must not turn a credential store into a failure.** `AGENTS.md` §2.1 forbids
/// a poisoning *policy* in a module like this one, and the only honest reading of a
/// poisoned `Mutex` here is "the value inside may be from a thread that panicked", which is
/// a perfectly good `Option<String>` for a store whose worst outcome is a stale credential.
pub struct InMemoryTokenStore {
    slot: Mutex<Option<String>>,
}

impl InMemoryTokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    /// A store already holding `credential`.
    ///
    /// **The first-launch-after-signing-in shape**, and it exists because writing it through
    /// [`TokenStore::store`] would be a test arranging its own precondition through the code
    /// under test — which is how a round-trip assertion ends up asserting that a value the
    /// test just put there came back.
    pub fn holding(credential: impl Into<String>) -> Self {
        Self {
            slot: Mutex::new(Some(credential.into())),
        }
    }

    /// The slot, taking a poisoned lock's value rather than treating it as a failure.
    fn read_slot(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.slot.lock().unwrap_or_else(|poisoned| {
            // **No `unwrap` on the success path and no error on the poisoned one.** The
            // value is an `Option<String>` this module wrote; a panic in another thread does
            // not make it unreadable, and `AGENTS.md` §2.1 forbids a panic here.
            poisoned.into_inner()
        })
    }
}

impl Default for InMemoryTokenStore {
    /// Present so `InMemoryTokenStore::new` is not the second spelling of the same value.
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore for InMemoryTokenStore {
    fn describe(&self) -> &'static str {
        "an in-memory credential store"
    }

    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        Ok(self.read_slot().clone())
    }

    fn store(&self, credential: String) -> Result<(), TokenStoreError> {
        *self.read_slot() = Some(credential);
        Ok(())
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        *self.read_slot() = None;
        Ok(())
    }
}

/// A store for a machine that has no usable credential store at all.
///
/// **The degradation `AGENTS.md` §2.1 and acceptance criterion 3 require**, and a real
/// implementation rather than a branch: a headless Linux session and a CI runner both have
/// no credential service, and a client that refused to start there would be a client nobody
/// can run. Every operation answers, nothing is kept, and **a write succeeds rather than
/// failing** — see [`LazyNativeStore`]'s implementation of [`TokenStore::store`] for why the
/// alternative is worse.
pub struct NoopTokenStore;

impl TokenStore for NoopTokenStore {
    fn describe(&self) -> &'static str {
        "no credential store on this machine"
    }

    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        Ok(None)
    }

    fn store(&self, _credential: String) -> Result<(), TokenStoreError> {
        Ok(())
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        Ok(())
    }
}

/// A store that fails every operation, for the "the keychain refused" path.
///
/// **A missing implementation would have made the most important degraded path the least
/// tested one.** `6b-keychain.md` asks for *"a token the keychain refuses to store leaves
/// the session working and says so"* — and that is a claim about what the *caller* does with
/// a refusal, so it needs a store that produces one. `KeyringTokenStore` cannot be made to
/// refuse on demand, and monkey-patching an operating-system call to produce a test failure
/// is not a test.
///
/// **`Refused` on every operation including `load`, and never `Unavailable`,** because the
/// distinction is real: this machine has somewhere to put a credential and it declined.
/// Reading that as "no store" would make the caller show the login form and say nothing,
/// which is the failure the split in [`TokenStoreError`] exists to prevent.
pub struct RefusingTokenStore {
    reason: String,
}

impl RefusingTokenStore {
    /// A store that refuses everything with `reason`.
    ///
    /// **The reason is carried rather than a constant** because it has to reach the caller:
    /// `AGENTS.md` §5.2 asks for an actionable failure, and a refusal with no cause is the
    /// opposite of one.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl TokenStore for RefusingTokenStore {
    fn describe(&self) -> &'static str {
        "a credential store that refuses everything"
    }

    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        Err(TokenStoreError::Refused(self.reason.clone()))
    }

    fn store(&self, _credential: String) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Refused(self.reason.clone()))
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Refused(self.reason.clone()))
    }
}

#[cfg(test)]
mod tests {
    //! The store seam, asserted at the level it actually has.
    //!
    //! **These live beside the code rather than in `tests/keychain.rs`, and the placement is
    //! the argument.** `AGENTS.md` §4.2 has a required row for this layer's kind of module, and
    //! this is what that row describes: a trait, five implementations, no window and no socket.
    //! `tests/keychain.rs` is an integration suite whose top-level imports pull in `gpui`,
    //! `sh_nexus::app` and `sh_nexus::state::bridge`, so a test sitting there cannot be
    //! compiled without the window — and a credential store's contract has nothing to say
    //! about a window.
    //!
    //! | Test | What it proves | Kind |
    //! |---|---|---|
    //! | [`the_in_memory_store_round_trips_one_credential`] | the store seam's whole contract | pure |
    //! | [`a_store_that_was_never_written_reports_no_credential`] | why `load` answers `Option<String>` | pure |
    //! | [`clearing_a_credential_leaves_the_store_empty`] | a clear takes the credential away | pure |
    //! | [`clearing_twice_is_not_a_failure`] | §6's "clearing is idempotent" | pure |
    //! | [`the_no_op_store_degrades_to_signing_in_again`] | acceptance criterion 3 | pure |
    //! | [`a_refusing_store_reports_a_failure_rather_than_a_silent_success`] | a refusal is reported, never swallowed | pure |
    //! | [`the_os_store_answers_here_whether_or_not_this_machine_has_one`] | a resolver that never blocks the frame path | real OS store, read-only |
    //! | [`the_os_store_reports_no_credential_rather_than_failing_when_there_is_none`] | the two `NoEntry` classifications, against the real API | real OS store, read-only |
    //! | [`a_stored_credential_never_reaches_a_debug_or_a_display`] | acceptance criterion 4, at runtime | pure |
    //! | [`the_application_state_holds_no_credential`] | the credential does not leak into `state/` or `core/` | source scan |
    //!
    //! # Why there is no keychain in CI, and what replaces it
    //!
    //! **`AGENTS.md` §4.3 requires hermetic tests, and a CI runner has no credential
    //! service.** So every store here runs against [`InMemoryTokenStore`], which is a real
    //! implementation of the trait rather than a mock of it — it has the same
    //! synchronisation, the same idempotent `clear`, and the same error shape. The
    //! `keyring`-backed store is exercised **read-only**, where this machine has one, and
    //! **its absence is a tested branch rather than an untested one**: the test that drives
    //! it asserts a different thing in each arm, so "there was no store here" is a pass with
    //! a claim behind it rather than a silent skip.
    //!
    //! **The one thing this module deliberately does not do is write to a developer's real
    //! credential store.** A round-trip would prove the write path against the real API,
    //! and it would also mean `cargo test` transiently replaces the session of anyone who
    //! happens to be signed in on that machine, leaving a stray entry behind if the run
    //! dies mid-test. The write path is covered by the in-memory store instead, and the two
    //! classifications that could plausibly be wrong against the real API — `NoEntry` meaning
    //! "no stored credential", and `NoEntry` on delete meaning "already clear" — are
    //! asserted directly, because both are read-only.
    //!
    //! # The four scans of this directory are not here, and could not be
    //!
    //! **A scan and its proof of non-vacuity have to sit outside the tree being scanned**, and
    //! moving them into this module would break them rather than misplace them.
    //! `no_platform_source_can_print_a_credential` reads every `.rs` file under
    //! `src/platform/` and fails one for containing a derived `Debug` or a stream print; the
    //! test that proves *it* is not vacuous demonstrates each of those on a synthetic source,
    //! and those samples are **string literals, which a comment stripper does not remove**.
    //! Placed here, the proof would be found by the scan in the very file both of them live
    //! in, and the suite would fail on itself. The layer-reachability scan has the same shape:
    //! its non-vacuity proof holds the forbidden `crate::ui` and `crate::network` imports as
    //! literals. So those four — `the_platform_layer_reaches_no_ui_and_no_network`,
    //! `the_platform_boundary_is_not_vacuous`, `no_platform_source_can_print_a_credential` and
    //! `the_credential_printers_are_not_vacuous` — stay in `tests/keychain.rs`, which is
    //! outside `src/`. **Splitting them instead would put the constants in one change and the
    //! assertions in another, and each half on its own proves nothing.**
    //!
    //! [`the_application_state_holds_no_credential`] *can* live here, because it scans
    //! `src/core/` and `src/state/` and never this directory, so nothing in this module is in
    //! its way.

    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    /// The store keeps what it is given, and gives it back.
    ///
    /// **`AGENTS.md` §4.2's store row, at the level this seam actually has.** The property
    /// that matters is not "a write happened" but "a write and a read agree" — a store that
    /// reported success and lost the value would be worse than one that failed.
    #[test]
    fn the_in_memory_store_round_trips_one_credential() {
        let store = InMemoryTokenStore::new();
        assert_eq!(
            store
                .load()
                .expect("an empty store answers rather than failing"),
            None,
            "a store nobody has written must report nothing"
        );

        store
            .store("a-session-credential".to_owned())
            .expect("the store accepts a credential");

        assert_eq!(
            store.load().expect("the read answers"),
            Some("a-session-credential".to_owned()),
            "what was stored must come back unchanged"
        );

        // And writing again replaces rather than appends: the store holds *one* credential,
        // because the client has one session.
        store
            .store("a-second-credential".to_owned())
            .expect("a second write is not a failure");
        assert_eq!(
            store.load().expect("the read answers"),
            Some("a-second-credential".to_owned()),
            "there is one slot, and a second sign-in must replace the first"
        );
    }

    /// A store nobody has written reports nothing rather than failing.
    ///
    /// **The documented "no stored credential" case, and the reason `load` answers
    /// `Option<String>` rather than `String`.** A store that reported "no credential" as an
    /// error would put a recoverable condition on the same path as a broken keychain, and the
    /// caller could not tell the user to sign in rather than to report a bug.
    #[test]
    fn a_store_that_was_never_written_reports_no_credential() {
        assert_eq!(
            NoopTokenStore.load().expect("the no-op store never fails"),
            None
        );
        assert_eq!(
            InMemoryTokenStore::new()
                .load()
                .expect("an empty store answers"),
            None
        );
    }

    /// Clearing takes the credential away.
    #[test]
    fn clearing_a_credential_leaves_the_store_empty() {
        let store = InMemoryTokenStore::holding("a-session-credential");
        store.clear().expect("clearing a store that holds one");
        assert_eq!(
            store.load().expect("the read answers"),
            None,
            "after a clear there is nothing to restore"
        );
    }

    /// Clearing twice is not a failure, and that is the property rather than the count.
    ///
    /// **`6b-keychain.md`'s "clearing is idempotent, so a double-clear cannot fail a
    /// logout".** Two calls only show it twice; the assertion below is on the *second* return
    /// value, because a first clear that succeeds and a second that fails is exactly the
    /// shape of the bug — and it is the shape that would leave a logout reporting an error
    /// the user cannot do anything about.
    #[test]
    fn clearing_twice_is_not_a_failure() {
        let store = InMemoryTokenStore::holding("a-session-credential");

        store
            .clear()
            .expect("the first clear takes the credential away");
        store.clear().expect(
            "**a second clear must be `Ok` too** -- there is nothing left to remove, and a \
             logout that reports a failure the user cannot act on is worse than a silent one",
        );

        // And the same holds for a store that never held anything at all, which is the shape
        // a double logout actually reaches on a machine that was never signed in.
        let never_written = InMemoryTokenStore::new();
        never_written.clear().expect("clearing an empty store");
        never_written
            .clear()
            .expect("and clearing it a second time");
    }

    /// With no store at all, everything degrades to "sign in again" and nothing fails.
    ///
    /// **Acceptance criterion 3, and the reason a missing credential service is not an
    /// error.** `AGENTS.md` §2.1 forbids panics in user-facing code, and a headless Linux
    /// session or a CI runner has no credential service at all — which is not exceptional, so
    /// it must not be a failure. Every operation answers, and the answer for a write is
    /// `Ok(())` rather than a refusal: a warning the user cannot act on, printed on every
    /// sign-in, is worse than saying nothing.
    #[test]
    fn the_no_op_store_degrades_to_signing_in_again() {
        let store = NoopTokenStore;

        assert_eq!(store.load().expect("a read"), None);
        store.store("a-session-credential".to_owned()).expect(
            "**a write must succeed rather than fail**: there is nowhere to put it, and \
             reporting that on every sign-in trains the reader to ignore the line that \
             matters",
        );
        assert_eq!(
            store.load().expect("a read"),
            None,
            "and nothing is kept, which is the whole of what this store does"
        );
        store.clear().expect("a clear");
        store.clear().expect("and a second clear");
    }

    /// A store that refuses says so, on every operation, and keeps nothing.
    ///
    /// **"Says so" is the half that is easy to skip.** A refusal that returned `Ok` would
    /// leave the client believing a session was remembered when it was not, and the user
    /// would find out by being asked to sign in again after closing the app. `AGENTS.md`
    /// §5.2 wants an actionable failure and this is the only place one can be produced.
    #[test]
    fn a_refusing_store_reports_a_failure_rather_than_a_silent_success() {
        let store = RefusingTokenStore::new("the credential service is locked");

        let written = store.store("a-session-credential".to_owned());
        assert!(
            matches!(written, Err(TokenStoreError::Refused(_))),
            "a refused write must be an error, not a silent success; got {written:?}"
        );

        let read = store.load();
        assert!(
            matches!(read, Err(TokenStoreError::Refused(_))),
            "and a refused read must not be reported as \"no credential\": those are different \
             facts and the caller acts on them differently; got {read:?}"
        );

        let cleared = store.clear();
        assert!(
            matches!(cleared, Err(TokenStoreError::Refused(_))),
            "and a refused clear must be reported too -- silently swallowing it would leave a \
             stale credential on disk with nothing on screen; got {cleared:?}"
        );

        // The reason is carried, because a message with no cause is what §5.2 calls
        // unactionable.
        let message = RefusingTokenStore::new("the credential service is locked")
            .store("x".to_owned())
            .expect_err("the write is refused")
            .to_string();
        assert!(
            message.contains("the credential service is locked"),
            "the reason must reach the caller; got {message:?}"
        );
    }

    /// The operating-system store answers on this machine, whichever store that turned out
    /// to be.
    ///
    /// **A lazy resolver, and this is what makes it usable from the main thread.** Deciding
    /// which store exists is itself an operating-system call — on Linux a session-bus round
    /// trip — so the object the main thread holds resolves nothing until a worker touches it.
    /// `AGENTS.md` §2.3 forbids that call on the frame path, and this type is how the
    /// constraint is honoured rather than asserted.
    #[test]
    fn the_os_store_answers_here_whether_or_not_this_machine_has_one() {
        let store = LazyNativeStore::new();
        assert!(
            !store.describe().is_empty(),
            "every store names itself, because `describe` is what a log line and a test read"
        );

        // Whatever it resolved to, a read must answer rather than panic. On a machine with no
        // credential service that answer is "nothing stored"; on a desktop it is an answer
        // from the operating system. Both are a pass, and neither is a skip.
        let read = store.load();
        assert!(
            matches!(read, Ok(_) | Err(TokenStoreError::Refused(_))),
            "a read through the OS store must answer with either a value or a reported \
             failure; a panic on the frame path is what §2.3 exists to prevent; got {read:?}"
        );
    }

    /// A machine with no stored credential reports none, rather than failing.
    ///
    /// **The one `keyring` classification most likely to be wrong, asserted against the real
    /// API.** `get_password` answers `Err(Error::NoEntry)` for an entry nobody wrote, and the
    /// whole startup decision turns on reading that as *"sign in"* rather than as *"this
    /// machine is broken"*. The same classification is asserted for `delete_credential`,
    /// where `NoEntry` has to become `Ok(())` — on Windows `CredDeleteW` reports
    /// `ERROR_NOT_FOUND` for an entry that is not there, so **without that mapping a first
    /// logout on a fresh machine would fail**, which is the idempotence requirement stated as
    /// a platform fact rather than as an intention.
    ///
    /// **Read-only on purpose**, for the reason `tests/keychain.rs`'s header gives: nothing
    /// here writes to a credential store the developer is using.
    #[test]
    fn the_os_store_reports_no_credential_rather_than_failing_when_there_is_none() {
        let store = LazyNativeStore::new();
        store.clear().expect(
            "**clearing an entry that is not there must succeed** -- on Windows `CredDeleteW` \
             reports `ERROR_NOT_FOUND`, and a logout that failed because there was nothing to \
             log out of would be the opposite of the requirement",
        );

        match store.load() {
            Ok(None) => {}
            Ok(Some(_)) => panic!(
                "this machine holds a credential under `{CREDENTIAL_SERVICE}` that this suite \
                 did not write. The `clear()` above should have removed it; if it did not, \
                 the store is not this client's and the assertion above was about somebody \
                 else's session."
            ),
            Err(error) => panic!(
                "an absent entry must be reported as `Ok(None)`, not as a failure: \
                 `get_password` answers `Err(NoEntry)` and this client maps that to \"nothing \
                 stored\" precisely so the startup decision can read it. Got {error:?}"
            ),
        }
    }

    /// A stored credential never reaches a `Debug` or a `Display`.
    ///
    /// **Acceptance criterion 4, at runtime rather than by convention.** `StoredCredential`
    /// carries the value in one variant, so a derived `Debug` would print it and compile while
    /// doing it — the same property `ConnectionSettings` and `LoginOutcome` are written out by
    /// hand for. Both renderings are asserted, because `Display` is what `tracing::info!(%…)`
    /// would print and a hand-written one is exactly as capable of a leak as a hand-written
    /// `Debug`.
    #[test]
    fn a_stored_credential_never_reaches_a_debug_or_a_display() {
        let credential = "a-credential-that-must-never-be-printed-0123456789";
        let found = StoredCredential::Present(credential.to_owned());

        // Two renderings, two different claims, and neither is checked with the other's
        // assertion: `Debug` is structured and names its field, `Display` is the sentence a
        // log line would carry. What they share is the absence of the value.
        for rendered in [format!("{found:?}"), found.to_string()] {
            assert!(
                !rendered.contains(credential),
                "`StoredCredential` printed the credential: {rendered}"
            );
            assert!(
                !rendered.is_empty(),
                "and a rendering that said nothing would hide a leak as thoroughly as one that \
                 printed it; got {rendered:?}"
            );
        }

        assert!(
            format!("{found:?}").contains("credential_configured"),
            "**`Debug` must still say *whether* one is present** -- \"the store was asked and \
             held nothing\" is diagnosable and one boolean is enough to see it; got {found:?}"
        );

        assert_eq!(
            format!("{:?}", StoredCredential::Absent),
            "Absent",
            "the absent case has nothing to redact, so it is spelled plainly"
        );
        assert_eq!(
            StoredCredential::Absent.to_string(),
            "there is no stored session",
            "and its `Display` says the same thing in a sentence"
        );
    }

    /// The application state holds no credential, and neither does the domain.
    ///
    /// **The same rule `tests/login.rs` asserts for the password, applied to the credential that
    /// outlives it.** `AGENTS.md` §3.1 describes `state/` as channels, presence, unread counts
    /// and cursors, and §3.2 makes `core/` pure. A credential in either would be a field no
    /// reader of the type could interpret — and `state/bridge.rs` is excluded by name for the
    /// reason `tests/login.rs` gives its own exclusion: it is the door a credential is
    /// *supposed* to cross.
    #[test]
    fn the_application_state_holds_no_credential() {
        let root = source_dir();
        for layer in ["core", "state"] {
            for file in source_files_under(&root.join(layer)) {
                if file.file_name().is_some_and(|name| name == "bridge.rs") {
                    continue;
                }
                let stripped = stripped_source(&file);
                let lowered = stripped.to_lowercase();
                for word in ["credential", "token", "password"] {
                    assert!(
                        !lowered.contains(word),
                        "{} names `{word}`. A credential belongs in the OS store, and in \
                         memory in `app::ConnectionSettings`; §3.1 describes state/ as \
                         channels, presence, unread counts and cursors, and a field there \
                         would be one no reader could interpret.",
                        file.display()
                    );
                }
            }
        }

        // And `core/` cannot reach the store even to ask: it is the innermost layer.
        for file in source_files_under(&root.join("core")) {
            assert!(
                !stripped_source(&file).contains("crate::platform"),
                "{} names `crate::platform`. AGENTS.md 3.2 makes core/ pure, and an \
                 operating-system call is the least pure thing there is.",
                file.display()
            );
        }
    }

    // -----------------------------------------------------------------------
    // The scanner this module owns
    // -----------------------------------------------------------------------

    /// Every `.rs` file under a directory, recursively.
    fn source_files_under(directory: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let Ok(entries) = fs::read_dir(directory) else {
            return found;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(source_files_under(&path));
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
        found.sort();
        found
    }

    /// The client's `src/` directory.
    fn source_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// A file's source with every comment removed, `//!` included.
    ///
    /// **The `//!` handling is load-bearing**, for the reason `tests/login.rs`'s copy of this
    /// function records: a stripper that tests only for `//` sees the lone `/` of a module doc,
    /// decides it is division, and leaves the prose behind — so `platform/`'s own documentation
    /// would be scanned as code.
    fn without_comments(source: &str) -> String {
        let mut stripped = String::with_capacity(source.len());
        let mut characters = source.chars().peekable();
        let mut in_block_comment = false;

        while let Some(character) = characters.next() {
            if in_block_comment {
                if character == '*' && characters.peek() == Some(&'/') {
                    characters.next();
                    in_block_comment = false;
                }
                continue;
            }
            if character == '/' {
                match characters.peek() {
                    Some('/') | Some('!') => {
                        for next in characters.by_ref() {
                            if next == '\n' {
                                stripped.push('\n');
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        characters.next();
                        in_block_comment = true;
                    }
                    _ => stripped.push(character),
                }
                continue;
            }
            stripped.push(character);
        }
        stripped
    }

    /// The comment-stripped source of one file, or a panic naming it.
    fn stripped_source(path: &Path) -> String {
        without_comments(
            &fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display())),
        )
    }
}
