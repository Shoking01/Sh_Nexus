//! `platform/`: the one layer allowed to ask the operating system for something.
//!
//! `AGENTS.md` §3.1's tree places this directory beside `network/` and `db/`, and L88
//! describes what belongs in it — *OS-specific implementations behind traits (notifications,
//! sounds, secure token storage)*. **This is that directory's first module**, and it exists
//! for the credential store alone: §7.1 forbids plaintext token storage and names the OS
//! keychain as the replacement, and §8.1's Login Flow says the token is stored there after a
//! successful sign-in. `odd/tasks/6b-keychain.md` is the change record.
//!
//! # What is here, and what each item is for
//!
//! | Item | For |
//! |---|---|
//! | [`token_store`] | the [`TokenStore`] trait and its five implementations |
//! | [`LazyNativeStore`] | **what `app::open` hands the window.** It decides which store this machine has *on a worker thread*, because deciding is itself a blocking operating-system call |
//! | [`KeyringTokenStore`] | the real one: Windows Credential Manager, macOS Keychain Services, or the Secret Service |
//! | [`InMemoryTokenStore`] | the tests, and a real implementation rather than a mock |
//! | [`NoopTokenStore`] | a machine with no usable credential service: degrade to "sign in again", never to a failed start |
//! | [`RefusingTokenStore`] | the failing-keychain path, so it is tested rather than assumed |
//! | [`StoredCredential`] | the load's answer, with no `Debug` that could print it |
//!
//! # Why a trait, and not a `keyring::Entry` used directly
//!
//! **`AGENTS.md` L88 says "behind traits", and three facts make that a requirement rather
//! than a style.** A CI runner and a headless Linux session have **no credential service at
//! all**, so every test would otherwise be a test that fails or silently does nothing; a
//! `Mutex<Option<String>>` is a real implementation of the same contract — same
//! synchronisation, same one-slot semantics, same idempotent `clear`, same error shape — and
//! it is what makes §4.3's hermetic requirement reachable; and a missing credential service
//! is an **ordinary condition** on some machines, which §2.1's ban on panics in user-facing
//! code forbids treating as a failure.
//!
//! # The layer's one rule: nothing here may reach `ui/` or `network/`
//!
//! Enforced by `the_platform_layer_reaches_no_ui_and_no_network` in `tests/keychain.rs`,
//! because `AGENTS.md` L88 puts this layer beside the UI and §3.2's separation is the only
//! thing keeping a credential store from growing opinions about the window or the socket.
//!
//! # Why nothing in here blocks, and what that costs
//!
//! §2.3 forbids blocking the frame loop, and every operation on a real credential store is a
//! **synchronous** operating-system call: `CredReadW`, the Keychain Services call, a Secret
//! Service round trip. So the trait is deliberately **not** async and nothing in this
//! directory starts a thread — the worker belongs to the seam, because
//! `state/bridge.rs` §7's argument is that a value crosses a boundary and the decision about
//! what to do with it belongs to the caller. `bridge::begin_credential_load`,
//! `bridge::remember_credential` and `bridge::forget_credential` are the three doors, and all
//! three hand work to a thread of their own before touching a store.
//!
//! **The cost of that arrangement is stated rather than hidden: the startup read is answered
//! on the shell's next pump tick**, which `app::DRAIN_INTERVAL` puts at worst 50 ms after
//! launch. That is well inside §6.2's 300 ms cached-session budget, and it is the price of
//! not reading a credential store inside `open()`.
//!
//! # Why nothing in here derives `Debug`
//!
//! **Every value this layer holds is a credential or the absence of one**, so a
//! `#[derive(Debug)]` anywhere in it would print the value and compile while doing it. The two
//! impls that exist are written out by hand and asserted at runtime by
//! `a_stored_credential_never_reaches_a_debug_or_a_display`, and
//! `no_platform_source_can_print_a_credential` fails the build on a third one appearing.

pub mod token_store;

pub use token_store::{
    InMemoryTokenStore, KeyringTokenStore, LazyNativeStore, NoopTokenStore, RefusingTokenStore,
    StoredCredential, TokenStore, TokenStoreError, CREDENTIAL_ACCOUNT, CREDENTIAL_SERVICE,
};
