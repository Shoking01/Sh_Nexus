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
//! # Why every suite here logs in first
//!
//! **Because the server refuses an unauthenticated handshake**, and that is the
//! auth milestone's whole point: `sh_nexus_server::ws::handler` answers a missing,
//! malformed, expired or revoked `Authorization: Bearer` with a 401 and no socket.
//!
//! [`ServerProcess::start_on`] therefore exports `SH_NEXUS_ADMIN_USERNAME` and
//! `SH_NEXUS_ADMIN_PASSWORD` to the child, so the instance has an administrator;
//! and [`ServerProcess::token`] performs a real `POST /auth/login` over a real
//! socket and hands back the token, which the suite puts in
//! [`TransportConfig::with_token`].
//!
//! **A real login rather than a token written into the database**, and the reason
//! is that this is the client's suite: it should exercise the same request the
//! product will make, against the same endpoint, with the same JSON. A test that
//! inserted a session row behind the server's back would be testing a server that
//! does not exist.
//!
//! # Why the port is probed and then handed to the server
//!
//! `SH_NEXUS_BIND` takes a full `host:port` and its default is
//! `127.0.0.1:8484` (`sh_nexus_server/src/main.rs`, `Settings::from_env`), so a
//! test cannot ask for port 0 and read the result back out — the port the
//! operator asked for is not the port the server got, which is exactly why that
//! binary logs `listener.local_addr()` rather than echoing the configuration.
//! So the harness binds an ephemeral port itself, drops the listener, and starts
//! the server on that number. **There is a race window between the two**, which is
//! why [`ServerProcess::start_on`] retries the readiness probe for
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

use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// How long a suite waits for something that should arrive on its own.
///
/// Generous on purpose: the alternative is a tight bound that turns a slow machine
/// into a flake, and a generous bound costs nothing once the thing has happened.
/// **Never used to wait *for* logic** — every wait below polls a condition, which
/// is `AGENTS.md` §4.3's distinction between a `sleep()` standing in for a
/// condition and a condition actually being polled.
pub const READY_BUDGET: Duration = Duration::from_secs(20);

/// The login handle exported to the child server.
///
/// A fixture value rather than something imported from the server crate, for the
/// reason the module docs give for the WebSocket path: naming `sh_nexus_server`
/// would put `axum` and `rusqlite` into this crate's dependency graph. The
/// administrator's handle is an *operator* decision (`SH_NEXUS_ADMIN_USERNAME`), so
/// a test that imported a constant would be asserting that this particular string
/// is the right one.
pub const ADMIN_USERNAME: &str = "root";

/// The password exported to the child server.
///
/// Over the twelve-character minimum in the server's `auth::MIN_PASSWORD_CHARS`, and
/// recognisable as a fixture: a log line or a panic that ever printed it would be
/// obviously wrong rather than plausibly a real credential.
pub const ADMIN_PASSWORD: &str = "fixture-admin-passphrase";

/// The path login is served on.
///
/// Spelled here rather than imported, for the same reason [`WS_PATH`] is: a test
/// suite that hard-codes a path it depends on fails loudly when the server moves it,
/// which is the trade `WS_PATH`'s own docs describe.
pub const LOGIN_PATH: &str = "/auth/login";

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
    /// The SQLite file the child was told to open.
    ///
    /// Stored rather than recomputed so [`ServerProcess::restart`] can hand the
    /// same path to a new process: a session token is bound to one database file,
    /// and a test that stops and restarts the server has to keep the same file or
    /// the token it holds stops meaning anything.
    database: PathBuf,
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
            // The auth milestone's bootstrap pair. **Exported to every child**, so
            // every suite gets an administrator and therefore a usable token, and
            // so `main.rs`'s "read both or neither" rule has both present.
            .env("SH_NEXUS_ADMIN_USERNAME", ADMIN_USERNAME)
            .env("SH_NEXUS_ADMIN_PASSWORD", ADMIN_PASSWORD)
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
            database,
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

    /// The database file this server owns.
    pub fn database(&self) -> &Path {
        self.database.as_path()
    }

    /// Stops the child process but keeps its port, its database and its directory.
    ///
    /// **Exists for one shape of test: one that must observe a transport failing
    /// before the server is there, and then observe that same transport succeed.**
    /// A session token is only obtainable from a *running* server, and a token is
    /// bound to one database file -- so the only way to hold a valid token across a
    /// gap is to take it from the running server, stop the server, and start it
    /// again against the same file.
    ///
    /// Idempotent, and safe to call on a server that has already stopped. Dropping
    /// still works afterwards, so a suite that panics between `stop` and `restart`
    /// leaves nothing behind.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Starts the child process again, on the same port and against the same file.
    ///
    /// # Panics
    ///
    /// If the process cannot be started, or exits before accepting. The exit status
    /// is in the panic message, for the reason [`ServerProcess::start_on`] gives.
    pub fn restart(&mut self) {
        assert!(
            self.child.is_none(),
            "restart() after a running server would leave two children on one port"
        );
        let bind = format!("127.0.0.1:{}", self.port);
        let mut child = Command::new(server_binary())
            .env("SH_NEXUS_BIND", &bind)
            .env("SH_NEXUS_DB", self.database())
            // Exported again even though the database already has an administrator:
            // `main.rs` reads both or neither, and exporting one of the pair on a
            // restart would be a startup failure this test would have to explain.
            .env("SH_NEXUS_ADMIN_USERNAME", ADMIN_USERNAME)
            .env("SH_NEXUS_ADMIN_PASSWORD", ADMIN_PASSWORD)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the sh_nexus_server binary to start again");

        let address: SocketAddr = format!("127.0.0.1:{}", self.port)
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
            panic!("the restarted sh_nexus_server exited before accepting ({status})");
        }
        self.child = Some(child);
    }

    /// A live session token for the bootstrapped administrator.
    ///
    /// **A real `POST /auth/login` over a real socket**, blocking, because these
    /// suites are `#[gpui::test]` and have no runtime of their own before the
    /// transport starts its own worker. `std::net::TcpStream` is the right tool
    /// here precisely because it needs none.
    ///
    /// # Panics
    ///
    /// If the request cannot be sent, the server answers anything other than 200,
    /// or the response carries no token. The response body is included in the panic
    /// because the most likely cause by far is a fixture password that does not
    /// match, and "expected 200, got 401 with `invalid_credentials`" says that;
    /// "the login failed" would not.
    pub fn token(&self) -> String {
        let address = format!("127.0.0.1:{}", self.port);
        let mut stream = TcpStream::connect(&address)
            .unwrap_or_else(|error| panic!("could not reach the server on {address}: {error}"));

        let body = serde_json::json!({
            "username": ADMIN_USERNAME,
            "password": ADMIN_PASSWORD,
        })
        .to_string();
        let request = format!(
            "POST {LOGIN_PATH} HTTP/1.1\r\n\
             Host: {address}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {body}",
            body.len()
        );

        use std::io::{Read, Write};
        stream
            .write_all(request.as_bytes())
            .expect("the login request to be written");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("the login response to be read");
        drop(stream);

        assert!(
            response.starts_with("HTTP/1.1 200"),
            "the fixture's own login must succeed; the server said {:?}",
            response.lines().next().unwrap_or_default()
        );
        let (head, body) = response
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("the login response had no body: {response:?}"));
        let parsed: serde_json::Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("the login body was not JSON ({error}): {body:?}"));
        parsed["token"]
            .as_str()
            .unwrap_or_else(|| panic!("the login response carried no token: {head:?}"))
            .to_owned()
    }

    /// A [`TransportConfig`] pointed at this server, carrying a live token.
    ///
    /// The two halves are one call because a suite that forgot the token would fail
    /// with a 401 and a message about authentication, which sends the reader to the
    /// wrong file; there is no reason for a test to be able to forget it by accident.
    pub fn authenticated_config(&self) -> sh_nexus::network::ws::TransportConfig {
        sh_nexus::network::ws::TransportConfig::new(self.url()).with_token(self.token())
    }

    /// The URL this server's WebSocket endpoint is on.
    ///
    /// For the suites that open their own socket rather than going through a
    /// [`sh_nexus::network::ws::WsTransport`], so that a rejected handshake can be
    /// observed instead of mapped onto a connection state.
    pub fn endpoint(&self) -> String {
        self.url()
    }

    /// A handshake request carrying `token`, for `tokio_tungstenite::connect_async`.
    ///
    /// # Panics
    ///
    /// If the endpoint does not parse or the token cannot be a header value. Both
    /// are fixture faults: the endpoint is one this fixture built.
    pub fn handshake_request(
        &self,
        token: Option<&str>,
    ) -> tokio_tungstenite::tungstenite::http::Request<()> {
        let mut request = self
            .url()
            .parse::<tokio_tungstenite::tungstenite::http::Uri>()
            .expect("this fixture's own endpoint parses as a URI")
            .into_client_request()
            .expect("this fixture's own endpoint builds a handshake request");
        if let Some(token) = token {
            let mut value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&format!(
                "Bearer {token}"
            ))
            .expect("a fixture token is a usable header value");
            value.set_sensitive(true);
            request.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
                value,
            );
        }
        request
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

/// Revokes `token` through the server's own logout endpoint.
///
/// Blocking for the same reason [`ServerProcess::token`] is: these suites are
/// `#[gpui::test]` and have no runtime before the transport starts its own worker.
///
/// # Panics
///
/// If the request cannot be sent or the server answers anything other than 204.
/// A revocation that silently failed would make the test that follows it pass for
/// the wrong reason -- or fail, confusingly, at the handshake.
pub fn revoke(server: &ServerProcess, token: &str) {
    let address = format!("127.0.0.1:{}", server.port());
    let mut stream = TcpStream::connect(&address)
        .unwrap_or_else(|error| panic!("could not reach the server on {address}: {error}"));
    let request = format!(
        "POST {LOGOUT_PATH} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n"
    );

    use std::io::{Read, Write};
    stream
        .write_all(request.as_bytes())
        .expect("the logout request to be written");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("the logout response to be read");
    drop(stream);

    assert!(
        response.starts_with("HTTP/1.1 204"),
        "a logout must be answered with 204; the server said {:?}",
        response.lines().next().unwrap_or_default()
    );
}

/// The path logout is served on.
///
/// Spelled here rather than imported, for the reason [`LOGIN_PATH`] is.
pub const LOGOUT_PATH: &str = "/auth/logout";

/// The path this suite believes the server serves WebSockets on.
pub const WS_PATH: &str = "/ws";
