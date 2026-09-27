//! Sh_Nexus entry point.
//!
//! Entry point only (AGENTS.md 3.1): initialises the app and opens a window.
//! All behaviour lives in the library so integration tests can reach it.
//!
//! `eprintln!` is used solely for the fatal-startup-failure path. AGENTS.md
//! 7.1 bans `println!` in production; the project-wide `tracing` logger
//! arrives in Phase 1 (PLAN.md section 2), which replaces this with `error!`.

use std::process::ExitCode;

fn main() -> ExitCode {
    match sh_nexus::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sh_nexus: failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}
