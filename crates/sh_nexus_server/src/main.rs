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
//! Four environment variables. The first two were here before authentication
//! existed; the second pair are ADR-010's and are the only way an account comes
//! into existence without an authenticated administrator.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `SH_NEXUS_BIND` | `127.0.0.1:8484` | The socket address to bind. A full `host:port`, not a port alone, so there is never a question about which interface was meant. |
//! | `SH_NEXUS_DB` | `sh_nexus.db` | The SQLite file. Relative paths resolve against the process's working directory. |
//! | `SH_NEXUS_ADMIN_USERNAME` | — | The login handle for the instance's first administrator. Read only when the instance has no account that can log in. |
//! | `SH_NEXUS_ADMIN_PASSWORD` | — | That account's password. **Never logged, never echoed, and never required on a second start.** |
//!
//! # Bootstrapping, and what "only the first time" is measured against
//!
//! [`auth::bootstrap`] runs on **every** start and creates exactly one account the
//! first time it finds an instance with no account that can log in. It is
//! deliberately a no-op afterwards, and the check is
//! [`db::Store::account_count`] rather than a row count -- see that function for
//! why the difference is what makes this correct on a database migrated from
//! schema 1 as well as on a fresh file.
//!
//! **A partially-configured environment is a startup failure, not a warning.**
//! Setting one of the two variables without the other, or setting either to an
//! empty or too-short value, exits with [`ServerError::Configuration`] naming the
//! variable. The alternative -- starting with no administrator and logging a
//! `warn!` -- produces an instance nobody can log into and an operator who finds
//! out from a failed connection attempt rather than from the log line that would
//! have told them.
//!
//! **The password is not required once an account exists.** That is what makes the
//! second start safe: an operator can stop exporting it, and the instance keeps
//! running. `auth::bootstrap`'s own docs cover what happens if the username
//! collides with a row that cannot log in.
//!
//! # Why the bind default is still loopback
//!
//! The default is `127.0.0.1`, not `0.0.0.0`, and that is still deliberate under
//! ADR-010 even though this milestone has authentication: the threat model there is
//! that the adversary is *outside* the team, and the cheapest defence against a
//! peer who has to be on the network at all is not being on the same network. An
//! operator who wants the instance exposed says so explicitly with
//! `SH_NEXUS_BIND`, and `docs/API.md` says what has to be in front of it.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use sh_nexus_server::db::Store;
use sh_nexus_server::error::{Result, ServerError};
use sh_nexus_server::{auth, AppState, Hub};
use tokio::net::TcpListener;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

/// The default filter when `RUST_LOG` is unset.
///
/// `info`, because that is `AGENTS.md` §7.5's release minimum and a server that
/// needs more detail should say so in its environment rather than have it on by
/// default.
const DEFAULT_FILTER: &str = "info";

/// The bind address used when `SH_NEXUS_BIND` is unset. Loopback; see the module
/// docs for why that is the safe default.
const DEFAULT_BIND: &str = "127.0.0.1:8484";

/// The database file used when `SH_NEXUS_DB` is unset.
const DEFAULT_DATABASE: &str = "sh_nexus.db";

/// The name of the bind environment variable.
const BIND_VARIABLE: &str = "SH_NEXUS_BIND";

/// The name of the database environment variable.
const DATABASE_VARIABLE: &str = "SH_NEXUS_DB";

/// The name of the bootstrap administrator's login handle.
const ADMIN_USERNAME_VARIABLE: &str = "SH_NEXUS_ADMIN_USERNAME";

/// The name of the bootstrap administrator's password.
const ADMIN_PASSWORD_VARIABLE: &str = "SH_NEXUS_ADMIN_PASSWORD";

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

/// Opens the database, bootstraps the first administrator, starts the runtime, and
/// serves until stopped.
///
/// The database is opened **before** the runtime exists, on purpose. `Store` is
/// synchronous (`db.rs`'s module docs) and opening it is blocking I/O; doing it
/// here means no runtime thread is blocked, not even once, and there is no
/// `spawn_blocking` call whose failure mode would have to be reasoned about at
/// startup.
///
/// The bootstrap is here for the same reason, with one addition that matters:
/// **an argon2 hash is ~50 ms of deliberately slow work**, and running it on the
/// main thread before the runtime exists means a cold start pays it once, visibly,
/// instead of paying it inside a request that some other thread is waiting on.
fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    let store = Store::open(&settings.database)?;
    bootstrap_first_administrator(&store, &settings)?;
    let state = AppState::new(store, Hub::new());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(serve(settings.bind, settings.database, state))
}

/// Creates the instance's first administrator, if it has none.
///
/// # Arguments
///
/// * `store` - the already-opened store.
/// * `settings` - the process configuration, which is where the two
///   `SH_NEXUS_ADMIN_*` values came from.
///
/// # Returns
///
/// `Ok(())` whether or not one was created, and the difference is a log line.
///
/// # Errors
///
/// [`ServerError::Configuration`] naming the variable at fault when the pair is
/// incomplete, empty, or too short -- see the module docs for why that is a
/// startup failure rather than a warning. [`ServerError::UsernameTaken`] and
/// [`ServerError::Crypto`] are propagated as they are: both mean an operator has to
/// look at the file, and neither is something this process can sensibly work
/// around.
fn bootstrap_first_administrator(store: &Store, settings: &Settings) -> Result<()> {
    let username = settings.admin_username.as_deref();
    let password = settings.admin_password.as_deref();

    match (username, password) {
        (None, None) => {
            // The only case that is not an error: an instance that already has an
            // account, started without exporting the bootstrap pair. Logged at
            // `debug!` because it is the ordinary second start, and at `info!`
            // would tell an operator to look for an administrator problem they do
            // not have.
            debug!("no bootstrap credentials are exported; not creating an administrator")
        }
        (Some(_), None) => {
            return Err(ServerError::Configuration(format!(
                "{ADMIN_USERNAME_VARIABLE} is set but {ADMIN_PASSWORD_VARIABLE} is not; \
                 unset both to run an instance that already has an account, or set \
                 both to create the first administrator"
            )))
        }
        (None, Some(_)) => {
            return Err(ServerError::Configuration(format!(
                "{ADMIN_PASSWORD_VARIABLE} is set but {ADMIN_USERNAME_VARIABLE} is not; \
                 the two are read as a pair"
            )))
        }
        (Some(found), Some(secret)) => {
            let found = found.trim();
            if found.is_empty() {
                return Err(ServerError::Configuration(format!(
                    "{ADMIN_USERNAME_VARIABLE} is set but blank; unset it to run an \
                     instance that already has an account"
                )));
            }
            if found.chars().count() > sh_nexus_server::auth::MAX_USERNAME_CHARS {
                return Err(ServerError::Configuration(format!(
                    "{ADMIN_USERNAME_VARIABLE} is longer than the {} characters this \
                     server accepts",
                    sh_nexus_server::auth::MAX_USERNAME_CHARS
                )));
            }
            if !auth::password_is_acceptable(secret) {
                // The rule and the number; never the value. An operator reading
                // this line learns exactly what to fix and nothing about the
                // secret they chose.
                return Err(ServerError::Configuration(format!(
                    "{ADMIN_PASSWORD_VARIABLE} is shorter than the {} characters this \
                     server requires",
                    auth::MIN_PASSWORD_CHARS
                )));
            }
            auth::bootstrap(store, found, secret)?;
        }
    }

    Ok(())
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
    /// The bootstrap administrator's handle, if exported.
    admin_username: Option<String>,
    /// The bootstrap administrator's password, if exported.
    ///
    /// A `String` and not a `Secret`-shaped wrapper because this crate declares no
    /// `zeroize` and pretending otherwise would be the kind of security theatre the
    /// project does not do: what is true is that it is read once, hashed once,
    /// dropped when `Settings` goes out of scope at the end of `run`, and never
    /// logged. A wrapper type would change none of that and would need an audit.
    admin_password: Option<String>,
}

impl Settings {
    /// Reads all four variables, applying the documented defaults.
    ///
    /// # Errors
    ///
    /// [`ServerError::Configuration`] naming the variable at fault. The
    /// alternative -- falling back to the default and logging -- would leave an
    /// operator who meant `:9000` listening on 8484 and wondering why nobody can
    /// connect, which is the failure mode `AGENTS.md` §7.3's explicit rejections
    /// exist to prevent.
    ///
    /// **The two `SH_NEXUS_ADMIN_*` variables are read but not validated here.**
    /// Whether a set-but-incomplete pair is an error depends on whether the
    /// instance already has an account, which only the database knows, so the
    /// pairing rule lives in [`bootstrap_first_administrator`] where the store is
    /// available. That split is deliberate: `Settings` is "what the operator said"
    /// and the bootstrap is "what the instance needs".
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

        Ok(Self {
            bind,
            database,
            admin_username: optional_variable(ADMIN_USERNAME_VARIABLE)?,
            admin_password: optional_variable(ADMIN_PASSWORD_VARIABLE)?,
        })
    }
}

/// Reads one optional variable, treating an empty value as absent.
///
/// **Empty is absent, and the reason is operator ergonomics rather than
/// leniency.** `SH_NEXUS_ADMIN_PASSWORD=` in a shell profile or a systemd unit is
/// how a secret most often gets "removed" without anybody meaning to remove it, and
/// treating it as a set-but-empty credential would make the process exit with a
/// confusing message about a value the operator believes they unset.
///
/// The `NotUnicode` arm is reported rather than ignored: a variable that cannot be
/// read is a broken environment, and silently acting as though it were unset is the
/// class of bug `AGENTS.md` §2.1's explicit rejections exist to prevent.
fn optional_variable(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(ServerError::Configuration(format!(
            "{name} could not be read: {error}"
        ))),
    }
}
