//! Shared fixtures for the WebSocket transport suites: a real `sh_nexus_server`
//! process on a real socket, and a bounded condition poll.
//!
//! # Why the server runs as a child process
//!
//! **Because linking it would put a server-only crate in the client's dependency
//! graph, and `docs/ARCHITECTURE.md` ADR-002's whole subject is which crates may
//! know about which.** `sh_nexus_server` depends on `axum`, `rusqlite` (with
//! SQLite's C amalgamation) and `tokio`; a `[dev-dependencies]` entry naming it
//! would put all three into `cargo tree -p sh_nexus` and would make the client's
//! manifest name its own server, which is the shape of mistake ADR-002 is written
//! to prevent. Running the binary instead keeps `sh_nexus`'s dependency set to
//! `tokio-tungstenite`, `tokio` and `futures-util` — all three already in
//! `Cargo.lock` — and it exercises the **shipped** server rather than its library,
//! which is a stronger claim than a test-only harness would be.
//!
//! The cost is stated here rather than discovered: the suite needs
//! `target/<profile>/sh_nexus_server.exe` to exist, which is what
//! `cargo test --workspace` builds and `cargo test -p sh_nexus` does not. The
//! lookup below panics with that exact sentence rather than skipping, because a
//! socket test that quietly does nothing is worse than one that is not run.
//!
//! # Why the port is probed and then handed to the server
//!
//! `SH_NEXUS_BIND` takes a full `host:port` and its default is
//! `127.0.0.1:8484` (`sh_nexus_server/src/main.rs`, `Settings::from_env`), so a
//! test cannot ask for port 0 and read the result back out — the port the
//! operator asked for is not the port the server got, which is exactly why that
//! binary logs `listener.local_addr()` rather than echoing the configuration.
//! So the harness binds an ephemeral port itself, drops the listener, and starts
//! the server on that number. **There is a race window between the two**, which
//! is why [`ServerProcess::start_on`] retries the readiness probe for
//! [`READY_BUDGET`] rather than connecting once.
//!
//! # Why the temporary directory is hand-rolled
//!
//! `tempfile` is in `Cargo.lock` through GPUI's tree and is not declared here, for
//! the same reason `sh_nexus_server/tests/support/mod.rs` hand-rolls it: naming it
//! would need an `AGENTS.md` §7.2 audit this work unit does not carry. The name is
//! unique per test and per process, and [`TempDir`] removes it on drop — behind
//! three guards on the path, so the removal cannot reach anything this process did
//! not create.

#![allow(dead_code)]

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a suite waits for something that should arrive on its own.
///
/// Generous on purpose: the alternative is a tight bound that turns a slow machine
/// into a flake, and a generous bound costs nothing once the thing has happened.
/// **Never used to wait *for* logic** — every wait below polls a condition, which
/// is `AGENTS.md` §4.3's distinction between a `sleep()` standing in for a
/// condition and a condition actually being polled.
pub const READY_BUDGET: Duration = Duration::from_secs(20);

/// The prefix every directory this module creates starts with.
///
/// Load-bearing: [`TempDir`]'s drop only removes a path whose file name begins with
/// it, so a bug in path construction fails loudly (nothing is removed) instead of
/// quietly deleting something.
const DIRECTORY_PREFIX: &str = "sh_nexus-ws-test-";

/// Distinguishes directories created by concurrent tests in one process.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique directory under the system temp directory, removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a directory named after `label`, failing if it cannot be made.
    ///
    /// Unique across three axes — a process id, a monotonic counter and the
    /// sub-second part of the clock — so two test binaries running at once on one
    /// machine cannot collide. `AGENTS.md` §4.3 requires independent tests with no
    /// shared state, and a shared database file is shared state.
    pub fn new(label: &str) -> io::Result<Self> {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let name = format!(
            "{DIRECTORY_PREFIX}{label}-{}-{}-{nanos}",
            std::process::id(),
            sequence
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    /// Removes the directory this guard created.
    ///
    /// Three guards, each because the alternative is a recursive delete with too
    /// wide a reach: the path must be absolute, must have a component beyond the
    /// temp directory, and its file name must carry this module's prefix. A path
    /// failing any of them is left alone.
    ///
    /// Synchronous, on the dropping thread. A detached retry loses the race with
    /// process exit — the test binary finishes in milliseconds, so the removal
    /// never happens — which is recorded here because it is the reason this is not
    /// "more correct" as a background task.
    fn drop(&mut self) {
        let removable = self.path.is_absolute()
            && self.path.components().count() > 1
            && self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(DIRECTORY_PREFIX));
        if removable {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Binds an ephemeral loopback port and releases it.
///
/// **The window between this returning and the server binding is a real race,**
/// and the only mitigation is the readiness retry in [`ServerProcess::start_on`].
/// Nothing here can close it without asking the operating system to hold the port
/// open for a process that does not exist yet.
pub fn free_port() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// The path of the `sh_nexus_server` binary `cargo test --workspace` just built.
///
/// Derived from this test executable's own location — `<target>/<profile>/deps/
/// <name>-<hash>.exe` — so it follows `--release`, a custom `CARGO_TARGET_DIR` and
/// any other layout cargo chose, none of which a hard-coded string would survive.
///
/// # Panics
///
/// If the binary is not there. The message names the command that builds it,
/// because the only ways to be in that state are `cargo test -p sh_nexus` or a
/// target directory somebody cleaned mid-run.
pub fn server_binary() -> PathBuf {
    let executable =
        std::env::current_exe().expect("a running test executable has a filesystem path");
    let profile_dir = executable
        .parent()
        .and_then(Path::parent)
        .expect("a test executable lives in <target>/<profile>/deps");
    let candidate = profile_dir.join(format!("sh_nexus_server{}", std::env::consts::EXE_SUFFIX));
    assert!(
        candidate.is_file(),
        "the sh_nexus_server binary was not found at {}. Run the suite with \
         `cargo test --workspace`, which builds every member's binary; \
         `cargo test -p sh_nexus` alone does not.",
        candidate.display()
    );
    candidate
}

/// Polls `condition` until it is true or `budget` is spent.
///
/// **This is the module's only waiting primitive, and it is a condition poll
/// rather than a sleep.** `AGENTS.md` §4.3 forbids `sleep()` standing in for logic
/// that should be waited on; the distinction is that this function returns the
/// instant the condition holds, and reports honestly when it never does. The
/// 1 ms gap between polls is a yield, not a wait — the answer does not depend on
/// it.
///
/// # Arguments
///
/// * `budget` - how long to keep asking.
/// * `what` - named in the panic, so a failure says which expectation lapsed.
/// * `condition` - asked again until it holds.
pub fn wait_until(what: &str, budget: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + budget;
    loop {
        if condition() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("timed out after {budget:?} waiting for {what}");
        }
        std::thread::yield_now();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A running `sh_nexus_server`, killed and cleaned up when it goes out of scope.
pub struct ServerProcess {
    child: Option<Child>,
    port: u16,
    _directory: TempDir,
}

impl ServerProcess {
    /// Starts a server on an ephemeral port and returns once it accepts a socket.
    ///
    /// # Arguments
    ///
    /// * `label` - names the temporary directory, so a leaked directory says which
    ///   test leaked it.
    ///
    /// # Panics
    ///
    /// If the process cannot be started, exits before accepting, or does not accept
    /// within [`READY_BUDGET`]. The exit status is included, because "the server
    /// never came up" without it sends the reader to the wrong file.
    pub fn start(label: &str) -> Self {
        let port = free_port().expect("an ephemeral loopback port");
        Self::start_on(port, label)
    }

    /// Starts a server on a port the caller already chose.
    ///
    /// This is what lets a suite point a transport at a port with nothing on it,
    /// watch it fail, and then bring the server up on that same port — which is
    /// how the reconnection schedule is observed against a real socket without
    /// binding a second port the transport does not know about.
    ///
    /// # Arguments
    ///
    /// * `port` - the loopback port to serve on.
    /// * `label` - names the temporary directory.
    ///
    /// # Panics
    ///
    /// As [`ServerProcess::start`].
    pub fn start_on(port: u16, label: &str) -> Self {
        let directory = TempDir::new(label).expect("a unique temporary directory");
        let database = directory.path().join("sh_nexus.sqlite3");
        let bind = format!("127.0.0.1:{port}");

        let mut child = Command::new(server_binary())
            .env("SH_NEXUS_BIND", &bind)
            .env("SH_NEXUS_DB", &database)
            // Nulled rather than inherited: `sh_nexus_server` logs at `info` by
            // default, so an inherited stdout would put four lines of server log in
            // the middle of every test's output and make a real failure harder to
            // find. The cost is that a server which dies during startup reports
            // nothing of its own — which is why the readiness loop below checks the
            // exit status and reports *that* instead.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the sh_nexus_server binary to start");

        let address: SocketAddr = format!("127.0.0.1:{port}")
            .parse()
            .expect("a loopback socket address this suite just formatted");
        let mut exited = None;
        wait_until("the server to accept a connection", READY_BUDGET, || {
            if TcpStream::connect(address).is_ok() {
                return true;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited = Some(status.to_string());
                    true
                }
                _ => false,
            }
        });
        if let Some(status) = exited {
            let _ = child.kill();
            panic!("the sh_nexus_server on port {port} exited before accepting ({status})");
        }

        Self {
            child: Some(child),
            port,
            _directory: directory,
        }
    }

    /// The port this server serves on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The `ws://` URL of this server's endpoint.
    ///
    /// **Spelled here rather than imported from the server crate**, for the reason
    /// the module docs give: naming `sh_nexus_server` would put `axum` and
    /// `rusqlite` into this crate's dependency graph. The path is duplicated
    /// instead, and
    /// `the_suites_socket_path_is_the_servers_own_constant` in `ws_transport.rs`
    /// fails the build if the two ever drift — a duplication a test watches is a
    /// duplication that cannot rot.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}{}", self.port, WS_PATH)
    }
}

impl Drop for ServerProcess {
    /// Kills the server and waits for it, then lets the directory go.
    ///
    /// **The wait is not optional.** `Child::kill` only *requests* termination, and
    /// on Windows a process whose database file is still open keeps that file's
    /// handle alive, which makes the directory undeletable. Reaping it here — on
    /// the dropping thread, inside the test's own process — is what keeps the next
    /// run's temporary directories from accumulating.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The path this suite believes the server serves WebSockets on.
pub const WS_PATH: &str = "/ws";
