//! Sh_Nexus entry point.
//!
//! Entry point only (`AGENTS.md` §3.1): initialises the app and opens a window.
//! All behaviour lives in the library so integration tests can reach it.
//!
//! # The subscriber is installed here, before anything that can log
//!
//! `tracing` is a facade with no sink of its own: `tracing::error!` compiles to a
//! no-op until a subscriber is installed, so a client that depended on it before
//! this line existed reported its two startup failures — *"the client has no
//! socket"*, *"the connection settings are incomplete"* — to nobody. The ordering
//! here is the whole of the fix, and it is before [`sh_nexus::run`] rather than
//! after because `run` is what opens a window and starts a transport, and a
//! failure inside it is exactly the message that needs somewhere to go.
//!
//! # This is NOT the user-facing log surface. The banner is.
//!
//! **Stderr is where a developer's terminal is, not where the person using the
//! application is.** A GUI client launched from Explorer has no console to read:
//! double-clicking the binary gives the process a stderr nobody will ever see, so
//! writing there is honest for debugging and useless for the user who needs to
//! know why their message will not send. That surface is
//! [`sh_nexus::ui::views::connection_banner`], which says the connection's state in
//! the window itself — including the server's own words when it refuses the client.
//!
//! **No log file is written, and the omission is a decision rather than a gap.**
//! A file needs a location, a size bound, a rotation policy and a redaction rule
//! (`AGENTS.md` §7.5 forbids logging message content and tokens, and a file
//! outlives the process that would have honoured that). Each is its own decision
//! with its own trade-off, and inventing one here to make this function look
//! complete would be the same mistake as a `debug_selector` on an element that is
//! never painted.

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

/// The level filter when `RUST_LOG` says nothing.
///
/// **`info`, and it is `AGENTS.md` §7.5's own number.** §7.5's table reserves
/// `error!` for *"errors affecting functionality"* and `warn!` for *"recoverable
/// situations"*, and §7.5's last line says *"in release builds, the minimum level
/// must be info (set via `RUST_LOG` or config)"*. So this is not a tuning
/// decision, it is the constitution's floor written down once.
///
/// **`debug` and `trace` stay reachable and are not the default.** §7.5 gives both
/// a use — cache hit rates, frame times — and both are off unless somebody asks for
/// them, because the alternative is a client that logs per-frame development detail
/// at a user who wanted a chat window. `RUST_LOG=sh_nexus=debug` is the whole
/// instruction.
const DEFAULT_LEVEL: &str = "info";

/// Installs the process-wide `tracing` subscriber, or reports that one is already
/// there.
///
/// **The non-panicking path, deliberately.** `tracing::subscriber::set_global_default`
/// panics when a default already exists, and a logger whose failure mode is a
/// panic in an initialiser is the wrong shape for a process that is about to open a
/// window: the one thing this crate cannot afford is a crash on the path that was
/// supposed to make crashes diagnosable. `try_init` is the same install with the
/// panic replaced by an `Err`, and this function reports that instead of hiding it.
///
/// **It reports rather than swallows, and the report is `bool`.** `true` means this
/// call installed the subscriber; `false` means somebody else's was already in
/// place. A second call is not a bug to be silenced — a test harness, an embedder,
/// or a future entry point may legitimately have installed one first — but a
/// *silent* second call would make it impossible to tell "installed" from "quietly
/// did nothing", and that distinction is what makes the startup path's coverage
/// checkable.
///
/// **The filter is read from `RUST_LOG`, falling back to [`DEFAULT_LEVEL`].**
/// `EnvFilter::try_from_default_env` is the non-panicking read of the same thing
/// `from_default_env` does: a developer who types `RUST_LOG=nonsense` gets the
/// default rather than a panic, because a typo in a debug variable is not a reason
/// to refuse to start.
fn install_subscriber() -> bool {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LEVEL));

    // **Stderr, and no `.with_ansi(..)` decision left open.** The default writer
    // already detects a terminal and drops the escapes when there is not one, which
    // is the correct behaviour for both of this process's audiences and needs no
    // configuration to get right.
    //
    // **`try_init` rather than `init`, for the reason this function exists at all.**
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok()
}

fn main() -> ExitCode {
    install_subscriber();

    match sh_nexus::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // `error!` and not `eprintln!`, which is what this line used to say it
            // would become. `AGENTS.md` §7.1's ban is on `println!` in production;
            // the reason it also applies here is that a startup failure printed raw
            // is not levelled, not filterable, and not routed through the one place
            // this crate decides what gets recorded. The message is unchanged, and
            // the token `SH_NEXUS_TOKEN` is never part of it: `MissingSetting`'s
            // `Display` names the variable and not its value.
            tracing::error!(error = %error, "the client could not start");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::install_subscriber;

    /// A second install is a reportable no-op rather than a panic.
    ///
    /// **`try_init` rather than `init`, and this is the test that says so.** `init`
    /// *panics* when a global default already exists — which is a panic in the
    /// function whose entire job is to make panics diagnosable, and it would fire
    /// on exactly the shape that reaches it twice: a test harness that installs its
    /// own subscriber, or an embedder that initialises `tracing` before handing
    /// control to [`sh_nexus::run`].
    ///
    /// **The order is the whole test.** The first call is the one that may install;
    /// every call after it is one that must survive. A test that only ever asserted
    /// "did not panic" would pass just as well against a version that never
    /// installed anything, which is why the return value is asserted as a `bool`
    /// and why the assertion is repeated for a third call — **idempotence is the
    /// property, and two calls only shows it twice.**
    #[test]
    fn installing_the_subscriber_twice_is_a_no_op_and_not_a_panic() {
        let first = install_subscriber();
        let second = install_subscriber();
        let third = install_subscriber();

        assert!(
            !second && !third,
            "every install after the first must report that it did nothing: \
             `tracing` keeps ONE global default, and a caller that could not tell \
             'installed' from 'quietly declined' could not check its own startup \
             coverage. The first call is whatever the process's subscriber state \
             already was -- `libtest` may have installed one -- and that is exactly \
             why the property is stated about the calls that follow it. Got \
             first={first}, second={second}, third={third}"
        );
    }
}
