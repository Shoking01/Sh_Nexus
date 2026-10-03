//! The binary: configuration, logging, bind, serve.
//!
//! `AGENTS.md` §3.1 says `main.rs` is an entry point only. It is 4 functions here
//! and no business logic: everything it does is process-shaped, and every decision
//! about messages lives in [`crate::message`].
//!
//! # Logging
//!
//! `tracing` only, never `println!` (`AGENTS.md` §7.1), through
//! `tracing_subscriber` with `RUST_LOG` as the filter and **`info` as the
//! default** (`AGENTS.md` §7.5's release minimum). The startup line names the
//! interface actually bound, the port the OS assigned, the database file and the
//! schema version -- which together answer every question a self-hosted operator
//! has in the first second after starting the process, including "did it bind
//! loopback or all interfaces", the question ADR-010 records as *not decided* and
//! therefore the one most worth making visible in a log rather than a convention.
//!
//! No message content appears in any line this crate emits. See
//! [`crate::message`]'s module docs for why `client_msg_id` and `channel_id` are
//! logged by length rather than by value: `AGENTS.md` §7.5 forbids content, and a
//! peer-supplied id is unbounded text that ends up in the same log.
//!
//! # Configuration
//!
//! Two environment variables, both optional:
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `SH_NEXUS_BIND` | `127.0.0.1:8484` | The socket address to bind. A full `host:port`, not a port alone, so there is never a question about which interface was meant. |
//! | `SH_NEXUS_DB` | `sh_nexus.db` | The SQLite file. Relative paths resolve against the process's working directory. |
//!
//! The bind default is loopback, not `0.0.0.0`, and that is deliberate under
//! ADR-010: this milestone has **no authentication**, so an instance reachable
//! from the network would be a readable one. An operator who wants it exposed has
//! to say so explicitly.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use sh_nexus_server::db::Store;
use sh_nexus_server::error::{Result, ServerError};
use sh_nexus_server::{AppState, Hub};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// The default filter when `RUST_LOG` is unset.
///
/// `info`, because that is `AGENTS.md` §7.5's release minimum and a server that
/// needs more detail should say so in its environment rather than have it on by
/// default.
const DEFAULT_FILTER: &str = "info";

/// The bind address used when `SH_NEXUS_BIND` is unset. Loopback; see the module
/// docs for why that is the safe default while there is no authentication.
const DEFAULT_BIND: &str = "127.0.0.1:8484";

/// The database file used when `SH_NEXUS_DB` is unset.
const DEFAULT_DATABASE: &str = "sh_nexus.db";

/// The name of the bind environment variable.
const BIND_VARIABLE: &str = "SH_NEXUS_BIND";

/// The name of the database environment variable.
const DATABASE_VARIABLE: &str = "SH_NEXUS_DB";

/// Entry point.
///
/// Reports through the exit code as well as the log, because `AGENTS.md` §7.5's
/// levels are for a human reading a log and a supervisor reading an exit code is a
/// different reader with no log in front of it.
fn main() -> ExitCode {
    init_tracing();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(%error, "sh_nexus_server stopped with an error");
            ExitCode::FAILURE
        }
    }
}

/// Installs the `tracing` subscriber.
///
/// `RUST_LOG` wins when it parses and the default applies when it does not. A
/// malformed filter is not a startup failure: refusing to serve because someone
/// mistyped a log level would be a worse outcome than serving at `info`.
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER)),
        )
        .with_target(false)
        .init();
}

/// Opens the database, starts the runtime, and serves until stopped.
///
/// The database is opened **before** the runtime exists, on purpose. `Store` is
/// synchronous (`db.rs`'s module docs) and opening it is blocking I/O; doing it
/// here means no runtime thread is blocked, not even once, and there is no
/// `spawn_blocking` call whose failure mode would have to be reasoned about at
/// startup.
fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    let store = Store::open(&settings.database)?;
    let state = AppState::new(store, Hub::new());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(serve(settings.bind, settings.database, state))
}

/// Binds the listener and serves until a shutdown signal arrives.
///
/// The address is read from the bound listener rather than echoed from the
/// configuration, because they differ: with port 0 -- which is how the integration
/// tests bind -- the port the operator asked for is not the port the server got,
/// and a log line reporting the requested port would be a lie that only shows up
/// as a connection refused.
async fn serve(bind: SocketAddr, database: PathBuf, state: AppState) -> Result<()> {
    let listener = TcpListener::bind(bind).await?;
    let bound = listener.local_addr()?;

    info!(
        interface = %bound.ip(),
        port = bound.port(),
        requested = %bind,
        database = %database.display(),
        schema_version = sh_nexus_server::SCHEMA_VERSION,
        websocket_path = sh_nexus_server::WS_PATH,
        "sh_nexus_server listening"
    );

    axum::serve(listener, sh_nexus_server::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("sh_nexus_server stopped");
    Ok(())
}

/// Resolves when the process is asked to stop.
///
/// A failure to install the Ctrl-C handler is logged and then **ignored**: the
/// process has no way to stop cleanly, and exiting instead would be worse than
/// serving on with no signal handling.
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(
            %error,
            "could not install the Ctrl-C handler; this process will not stop on Ctrl-C"
        );
    }
}

/// The process configuration, read once from the environment.
struct Settings {
    /// The socket address to bind.
    bind: SocketAddr,
    /// The SQLite file to open.
    database: PathBuf,
}

impl Settings {
    /// Reads both variables, applying the documented defaults.
    ///
    /// # Errors
    ///
    /// [`ServerError::Configuration`] naming the variable at fault. The
    /// alternative -- falling back to the default and logging -- would leave an
    /// operator who meant `:9000` listening on 8484 and wondering why nobody can
    /// connect, which is the failure mode `AGENTS.md` §7.3's explicit rejections
    /// exist to prevent.
    fn from_env() -> Result<Self> {
        let bind = match std::env::var(BIND_VARIABLE) {
            Ok(value) => value.parse::<SocketAddr>().map_err(|error| {
                ServerError::Configuration(format!(
                    "{BIND_VARIABLE} is {value:?}, which is not a host:port socket \
                     address ({error}); for example 127.0.0.1:8484"
                ))
            })?,
            Err(std::env::VarError::NotPresent) => {
                DEFAULT_BIND.parse::<SocketAddr>().map_err(|error| {
                    ServerError::Configuration(format!(
                        "the built-in default bind {DEFAULT_BIND:?} is not a socket \
                         address ({error}), which is a bug in this build"
                    ))
                })?
            }
            Err(error) => {
                return Err(ServerError::Configuration(format!(
                    "{BIND_VARIABLE} could not be read: {error}"
                )))
            }
        };

        let database = match std::env::var(DATABASE_VARIABLE) {
            Ok(value) if value.trim().is_empty() => {
                return Err(ServerError::Configuration(format!(
                    "{DATABASE_VARIABLE} is set but empty; unset it to use {DEFAULT_DATABASE:?}"
                )))
            }
            Ok(value) => PathBuf::from(value),
            Err(std::env::VarError::NotPresent) => PathBuf::from(DEFAULT_DATABASE),
            Err(error) => {
                return Err(ServerError::Configuration(format!(
                    "{DATABASE_VARIABLE} could not be read: {error}"
                )))
            }
        };

        Ok(Self { bind, database })
    }
}
