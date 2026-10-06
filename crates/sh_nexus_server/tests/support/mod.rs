//! Shared fixtures for the `sh_nexus_server` test suites: a real server on a real
//! socket, a real WebSocket client, and a real database file per test.
//!
//! # Why the tests drive a real socket
//!
//! `AGENTS.md` §8.1's Real-time Flow asks for "two clients, a send crossing the
//! wire". A mock cannot answer whether the server does that, and ADR-010 makes
//! the point sharper: *"the first milestone is a real server, not a mock"*. So
//! every suite here binds a listener on port 0, reads the port the OS assigned,
//! and talks to it over TCP.
//!
//! # Why every client here carries a token
//!
//! [`TestServer::start`] bootstraps an administrator and mints a session for it
//! before it starts listening, and [`TestServer::connect`] presents that session's
//! token on the handshake. That is the whole point of the auth milestone: **an
//! upgrade without a live session is answered with a 401 and no socket**, so a
//! suite that wanted to test anything else would have to opt out explicitly with
//! [`TestServer::connect_anonymous`].
//!
//! The suites that need a refusal use [`attempt_upgrade`], which returns the HTTP
//! answer instead of a socket -- that is how `tests/auth.rs` proves the 401
//! happens *before* the upgrade rather than as a close after one.
//!
//! # Why there is a hand-written WebSocket client
//!
//! Because `crates/sh_nexus_server/Cargo.toml` cannot gain a dependency for it.
//! The manifest declares `axum`, and axum is a **server**-side WebSocket
//! implementation: `axum::extract::ws::WebSocket` has inherent `recv`/`send`, and
//! no client at all. `tokio-tungstenite` *is* in `Cargo.lock` -- axum's `ws`
//! feature depends on it -- but a transitive dependency is not a dependency, and
//! naming it here would mean adding it to `[dev-dependencies]`, which needs an
//! `AGENTS.md` §7.2 audit this milestone does not have.
//!
//! So the ~180 lines of RFC 6455 below are the cost of that audit not having
//! happened yet, and they are the honest place to notice it: **`tests/support/` is
//! where a WebSocket client belongs the day someone audits one.** What is
//! implemented is exactly what these suites use-- an HTTP/1.1 upgrade with an
//! optional `Authorization` header, masked text frames in both directions, close
//! and ping/pong -- and what is not implemented (permessage-deflate,
//! fragmentation beyond continuation frames, client-side keepalive) is refused
//! loudly rather than tolerated.
//!
//! Three deliberate simplifications, all recorded because they are spec deviations
//! and a reader should not have to find them:
//!
//! - The `Sec-WebSocket-Key` is the RFC 6455 §1.3 example nonce, constant. The
//!   server signs whatever it receives and this client does not verify the
//!   signature, so a varying key would exercise nothing. A real client must vary
//!   it.
//! - The masking key varies per frame (RFC 6455 §5.3 requires a fresh one) but is
//!   derived from a counter rather than a random source, because
//!   `rand`/`getrandom` are not declared here either. Against a server that does
//!   not enforce unpredictability -- tungstenite does not -- this is
//!   indistinguishable from random.
//! - **The `Authorization` header is whatever the caller passes**, verbatim. These
//!   are *test* credentials against a *test* instance, and the production code path
//!   that matters -- `auth::bearer_token`'s shape validation -- is exercised by
//!   passing it deliberately wrong strings, which is only possible because the
//!   fixture does not sanitise.
//!
//! # Why the temporary directory is hand-rolled too
//!
//! `tempfile` is in `Cargo.lock` (through GPUI's tree) and is **not** declared in
//! this crate's `[dev-dependencies]`, so the same §7.2 argument applies. The
//! directory name is unique per test and per process, and [`TempDir`] removes it
//! on drop -- with three guards on the path, so the removal cannot reach anything
//! this process did not create.

#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sh_nexus_server::auth;
use sh_nexus_server::db::Store;
use sh_nexus_server::hub::Hub;
use sh_nexus_server::time;
use sh_nexus_server::AppState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The tokio socket type, named once because this file also has a *blocking*
/// `std::time::Duration` and a `std::io` vocabulary around it.
type Socket = TcpStream;

/// How long a suite waits for a frame it expects to arrive.
///
/// Generous, because the alternative -- a tight bound -- turns a slow CI machine
/// into a flake, and a generous bound costs nothing when the frame is already
/// there. Absences are asserted with [`SILENCE_BUDGET`] instead, which is short
/// on purpose.
pub const FRAME_BUDGET: Duration = Duration::from_secs(10);

/// How long a suite waits before concluding that nothing is coming.
///
/// Short, and only ever used to prove an *absence*: that a replay produced no
/// second broadcast, or that a sender did not receive its own echo. It is not a
/// substitute for a real assertion -- every one of those tests makes its positive
/// assertions first and uses this only for the negative half -- but a test that
/// asserted nothing at all would prove nothing either.
pub const SILENCE_BUDGET: Duration = Duration::from_millis(400);

/// The login handle [`TestServer::start`] bootstraps.
///
/// A fixture value and not a constant in `auth.rs`, for the same reason the WebSocket
/// path is a literal here: the production crate's admin name is an *operator*
/// decision (`SH_NEXUS_ADMIN_USERNAME`), and a test that imported it would be
/// asserting that this particular string is the right one.
pub const ADMIN_USERNAME: &str = "root";

/// The password [`TestServer::start`] bootstraps.
///
/// Over the twelve-character minimum in `auth::MIN_PASSWORD_CHARS` and
/// recognisable as a fixture on sight, so a log line or a panic that ever printed it
/// would be obviously wrong rather than plausibly a real credential.
pub const ADMIN_PASSWORD: &str = "fixture-admin-passphrase";

/// The password every [`TestServer::create_account`] fixture account gets.
///
/// **A constant rather than `format!("{username}-passphrase")`, and the reason is
/// CodeQL rather than taste.** Deriving it from the username meant every caller's
/// username literal flowed through `format!` into [`auth::hash_password`], which
/// CodeQL read as a hard-coded credential and reported at `critical` security
/// severity. It was always a false positive — `username` is a name, not a secret —
/// but it fired on every fixture account in the suite, so a finding that says
/// nothing was burying the ones that might.
///
/// **Nothing logs in with this, so changing it costs nothing.** An account that
/// needs a *known* password is created through `create_account_over_http` or
/// hashed inline, which is what the authentication suites do; this one exists for
/// suites about the socket, which authenticate with
/// [`TestServer::session_for`] and never present a password at all. The value was
/// changed anyway, so the reason for the constant is visible rather than
/// historical.
pub const FIXTURE_PASSWORD: &str = "fixture-account-passphrase";

/// A password for an account that [`TestServer::start`] does not create.
///
/// For the login-failure tests. Distinguishable from [`ADMIN_PASSWORD`] so a
/// failure message naming the one that was sent says which half of the assertion
/// lapsed.
pub const WRONG_PASSWORD: &str = "a-different-passphrase";

/// The path the server is asked to serve, from the crate's own constant.
const WS_PATH: &str = sh_nexus_server::WS_PATH;

/// The prefix every directory this module creates starts with.
///
/// Load-bearing: [`TempDir`]'s drop only removes a path whose file name begins
/// with it, so a bug in path construction fails loudly (nothing is removed)
/// instead of quietly deleting something.
const DIRECTORY_PREFIX: &str = "sh_nexus_server-test-";

/// Distinguishes directories created by concurrent tests in one process.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique directory under the system temp directory, removed on drop.
///
/// `tempfile::TempDir` would be the ordinary answer; see the module docs for why
/// it is hand-rolled.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a directory named after `label`, failing if it cannot be made.
    ///
    /// Unique across three axes -- a process id, a monotonic counter and the
    /// sub-second part of the clock -- so two concurrently running test binaries
    /// on one machine cannot collide. `AGENTS.md` §4.3 requires independent tests
    /// with no shared state, and a shared database file is shared state.
    pub fn new(label: &str) -> io::Result<Self> {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let name = format!(
            "{DIRECTORY_PREFIX}{label}-{}-{}-{nanos}",
            process::id(),
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
    /// Three guards, and each one exists because the alternative is a recursive
    /// delete with too wide a reach: the path must be absolute, must have a parent
    /// component beyond the temp directory, and its file name must carry this
    /// module's prefix. A path that fails any of them is left alone.
    ///
    /// **Synchronous, and that is a correction.** The first version handed the
    /// removal to a detached thread and retried, which is wrong in a way that only
    /// shows up on Windows and only at scale: a test binary finishes in tens of
    /// milliseconds, so the process exits long before a detached thread's second
    /// attempt, and the directories leaked anyway -- 174 of them, before this was
    /// fixed.
    ///
    /// The tempting diagnosis is also wrong, and it is worth recording because the
    /// obvious one sends you to the wrong file. It looks like a sharing violation:
    /// the `Store` lives in an `Arc`, `axum` **clones** the `Router` per connection,
    /// and `JoinHandle::abort` only *requests* cancellation of the serving task, so
    /// one would expect an open handle to block the delete. But a `#[tokio::test]`
    /// drops its runtime immediately after the body, which drops every task it
    /// spawned, and the runtime is long gone by the time anything else runs. The
    /// lock is not what blocks the removal.
    ///
    /// What blocks it is that the whole teardown happens inside process exit. The
    /// synchronous attempt, on the dropping thread, while the harness's own `Store`
    /// handle has already been released -- see [`TestServer`]'s field order -- is
    /// what works.
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

/// A `sh_nexus_server` listening on an OS-assigned loopback port.
///
/// # Teardown
///
/// Dropping the handle aborts the serving task, which closes the listener. **That
/// is enough to run the server and not enough to clean up after it**, and the
/// reason is worth stating because it is a Windows-only behaviour a reader would
/// otherwise have to rediscover:
///
/// - `Store` holds its `Connection` in an `Arc`, and `axum` **clones** the `Router`
///   for every incoming connection. So the file on disk has an open handle in the
///   serving task *and* in each live WebSocket task.
/// - `JoinHandle::abort` only *requests* cancellation. The serving task's future is
///   dropped the next time the runtime polls it, which on a single-threaded test
///   runtime is after the test body has finished -- and the connection tasks are
///   not polled at all, because nothing wakes them once the client sockets are gone.
/// - `#[tokio::test]` drops its runtime immediately after the body, so by the time
///   any of that happens the process is on its way out.
///
/// So [`shutdown`](Self::shutdown) exists, and a socket test calls it after
/// dropping its clients. [`Drop`] remains as the panic-path fallback: it aborts, and
/// the directory is removed if it can be, which is always true for a test that never
/// opened a socket.
///
/// The owned fields are `Option` for one reason: a type that implements `Drop`
/// cannot be destructured, and `shutdown` needs to take its parts. That is the
/// whole cost, and it is the standard shape for the pattern.
pub struct TestServer {
    /// The harness's own store handle, released first.
    store: Option<Store>,
    /// The task, so `abort` runs before the directory is removed.
    task: Option<JoinHandle<()>>,
    /// The unique temporary directory holding the database file.
    directory: Option<TempDir>,
    address: std::net::SocketAddr,
    database: PathBuf,
    hub: Hub,
    /// A live session token for the bootstrapped administrator.
    ///
    /// Minted through the *store*, not through `POST /auth/login`, and the reason
    /// is that most suites here are about the socket and not about HTTP: a login
    /// round trip per client would add a request to every test to prove something
    /// `tests/auth.rs` proves directly. The production minting path is the same
    /// code in both cases -- `auth::mint_session_token` and
    /// `Store::insert_session` -- so nothing about the token under test differs.
    admin_token: String,
    /// The bootstrapped administrator's account id, for assertions about authorship.
    admin_user_id: String,
}

impl TestServer {
    /// Binds port 0, bootstraps an administrator, starts the real router, and
    /// returns once it is listening.
    ///
    /// The database is a file in this server's own temporary directory, opened by
    /// the same [`Store`] production uses -- no test-only storage path exists, so
    /// there is nothing for a test to pass that production cannot.
    ///
    /// # Panics
    ///
    /// If the bootstrap or the argon2 hashing fails, or the listener cannot be
    /// bound. Both are fixture faults rather than product faults: the bootstrap is
    /// the same call `main.rs` makes, and a failure here means the arguments are
    /// wrong, which a silent skip would hide.
    pub async fn start() -> Self {
        let directory = TempDir::new("server").expect("a unique temporary directory");
        let database = directory.path().join("sh_nexus.sqlite3");

        let store = Store::open(&database).expect("a migrated database");
        assert!(
            auth::bootstrap(&store, ADMIN_USERNAME, ADMIN_PASSWORD).expect("a bootstrapped admin"),
            "a fresh database must bootstrap its first administrator"
        );
        let admin_user_id = store
            .credential_for_username(ADMIN_USERNAME)
            .expect("a readable credential")
            .expect("the bootstrapped account")
            .account
            .id;
        let admin_token =
            issue_session(&store, &admin_user_id).expect("a stored session for the administrator");

        let hub = Hub::new();
        let state = AppState::new(store.clone(), hub.clone());

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback listener");
        let address = listener.local_addr().expect("the bound address");

        let task = tokio::spawn(async move {
            // The server is expected to outlive the test that started it; an error
            // here is reported by the test's own timeouts, with the server's
            // stderr already in the test output.
            axum::serve(listener, sh_nexus_server::router(state))
                .await
                .expect("axum::serve to keep running for the life of the test");
        });

        Self {
            store: Some(store),
            task: Some(task),
            directory: Some(directory),
            address,
            database,
            hub,
            admin_token,
            admin_user_id,
        }
    }

    /// The address the OS assigned, including the port.
    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// The database file this server opened.
    pub fn database(&self) -> &Path {
        &self.database
    }

    /// The server's hub, for assertions about what it thinks is connected.
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    /// A live session token for the bootstrapped administrator.
    pub fn admin_token(&self) -> &str {
        &self.admin_token
    }

    /// The bootstrapped administrator's account id.
    ///
    /// The other half of "messages are attributed to a real user": a suite asserts
    /// `message.user_id == server.admin_user_id()` and knows that value came from
    /// the account row, not from a constant somebody typed twice.
    pub fn admin_user_id(&self) -> &str {
        &self.admin_user_id
    }

    /// How many connections the server has subscribed.
    ///
    /// Worth a test of its own, because it is the visible form of an ordering
    /// property `ws.rs` depends on: the subscription happens before the upgrade
    /// response is written, so this is already correct the instant
    /// [`TestClient::connect`] returns, with no window to poll through. **And it is
    /// how the auth milestone proves the 401 is pre-upgrade**: a refused handshake
    /// must leave this at zero, because `ws.rs` subscribes *after* it has
    /// authenticated.
    pub fn connection_count(&self) -> usize {
        self.hub.connection_count()
    }

    /// Opens an authenticated WebSocket connection to this server.
    pub async fn connect(&self) -> TestClient {
        TestClient::connect(self.address, Some(&self.admin_token))
            .await
            .expect("the server to accept an authenticated websocket upgrade")
    }

    /// Opens a WebSocket connection **without** a token.
    ///
    /// Named for what it is rather than left as `connect(.., None)`, because the
    /// only correct expectation from it is a refusal, and a call site that says
    /// `connect` reads like a success.
    pub async fn connect_anonymous(&self) -> TestClient {
        TestClient::connect(self.address, None)
            .await
            .expect("the server to answer an unauthenticated upgrade")
    }

    /// Creates an account directly in the store and returns its id.
    ///
    /// For suites that need a *second* identity -- a non-member, or a second
    /// member whose messages must be distinguishable from the administrator's.
    /// Builds an account for a socket-level suite.
    ///
    /// Goes through [`Store::create_account`] rather than through
    /// `POST /admin/users` so that a suite about the socket does not also have to
    /// be a suite about HTTP.
    ///
    /// # Panics
    ///
    /// If the account cannot be created or granted its channel. Both are fixture
    /// faults: a name collision inside one test's own temporary database means the
    /// test asked for the same account twice.
    pub fn create_account(&self, username: &str, join_default_channel: bool) -> String {
        let store = self.store.as_ref().expect("a live store");
        let hash = auth::hash_password(FIXTURE_PASSWORD).expect("a hash for the fixture account");
        let account = store
            .create_account(username, username, &hash, false)
            .expect("a fixture account");
        if join_default_channel {
            store
                .add_channel_member(&account.id, sh_nexus_server::DEFAULT_CHANNEL_ID)
                .expect("membership of the seeded channel");
        }
        account.id
    }

    /// Mints a live session token for `user_id`, through the production path.
    pub fn session_for(&self, user_id: &str) -> String {
        issue_session(self.store.as_ref().expect("a live store"), user_id)
            .expect("a stored session")
    }

    /// `POST /auth/login` over a real socket, and returns the token.
    ///
    /// # Panics
    ///
    /// If the request cannot be sent, the server answers anything other than 200,
    /// or the response body carries no `token`. A login suite that silently got
    /// `None` would assert nothing.
    pub async fn login(&self, username: &str, password: &str) -> String {
        let response = post_json(
            self.address,
            auth::LOGIN_PATH,
            None,
            &serde_json::json!({ "username": username, "password": password }),
        )
        .await
        .expect("a login request that reaches the server");
        assert_eq!(
            response.status, 200,
            "a login with the right credentials must be answered with 200; the server \
             said {:?}",
            response.body
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&response.body).expect("a JSON login response");
        parsed["token"]
            .as_str()
            .expect("a token in the login response")
            .to_owned()
    }

    /// `POST /auth/logout` over a real socket, returning the status and body.
    pub async fn logout(&self, token: &str) -> Response {
        post_json(
            self.address,
            auth::LOGOUT_PATH,
            Some(token),
            &serde_json::json!({}),
        )
        .await
        .expect("a logout request that reaches the server")
    }

    /// `POST /admin/users` over a real socket, returning the status and body.
    pub async fn create_account_over_http(
        &self,
        administrator_token: &str,
        username: &str,
        display_name: &str,
        password: &str,
    ) -> Response {
        post_json(
            self.address,
            auth::ADMIN_USERS_PATH,
            Some(administrator_token),
            &serde_json::json!({
                "username": username,
                "display_name": display_name,
                "password": password,
            }),
        )
        .await
        .expect("an account creation request that reaches the server")
    }

    /// Stops the server and removes its database file.
    ///
    /// **Call this at the end of a test that opened a socket, after dropping its
    /// clients.** The type's docs give the whole reason; the short version is that
    /// an open `Connection` on Windows makes the file undeletable, and the handle
    /// outlives both the client sockets and `JoinHandle::abort`.
    ///
    /// The bounded wait is teardown, not synchronisation: nothing here is being
    /// waited *for* as a precondition of an assertion, the outcome is the same
    /// either way, and it is bounded at a few milliseconds. What it buys is that
    /// the reactor gets to observe the closed client sockets and drop the
    /// per-connection `Router` clones that hold the file open. `AGENTS.md` §4.3
    /// forbids a `sleep()` standing in for a condition a test should be waiting on;
    /// this is not that, and the alternative measured worse.
    ///
    /// Consuming `self` means it cannot be called twice, and dropping it here rather
    /// than in [`Drop`] is what puts the removal *inside* the runtime's lifetime.
    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            // Awaiting an aborted handle is what makes the cancellation take
            // effect: the task's future is dropped when this resolves.
            let _ = task.await;
        }
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        self.store.take();
        self.directory.take();
    }
}

impl Drop for TestServer {
    /// The panic-path fallback for [`TestServer::shutdown`].
    ///
    /// Aborts the serving task, releases this struct's store handle, and lets the
    /// field drops remove the directory if the file is not open. A test that
    /// panicked with a live socket leaves the directory behind; a panic is worth a
    /// readable message and not worth a lock.
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Reads one `i64` from the database file on a connection of its own.
///
/// The server holds the file open in WAL mode, which is what makes this possible
/// at all: WAL readers do not block the writer, so a test can look at the rows a
/// running server has committed without shutting it down first.
///
/// The statement comes from the caller and the value is bound through `?1` --
/// there is no string interpolation into SQL anywhere in this crate, including
/// here.
pub fn query_i64(database: &Path, statement: &str, parameter: &str) -> i64 {
    let connection =
        rusqlite::Connection::open(database).expect("the database file the server created");
    connection
        .query_row(statement, [parameter], |row| row.get(0))
        .expect("the query to return exactly one row")
}

/// Reads one `i64` from a statement that takes no parameters.
///
/// Separate from [`query_i64`] because `rusqlite` rejects a statement bound to
/// more parameters than it declares (`InvalidParameterCount`), so passing a
/// placeholder to `SELECT COUNT(*)` is an error rather than a no-op.
pub fn query_count(database: &Path, statement: &str) -> i64 {
    let connection =
        rusqlite::Connection::open(database).expect("the database file the server created");
    connection
        .query_row(statement, [], |row| row.get(0))
        .expect("the query to return exactly one row")
}

/// Reads one `i64` from a statement that takes one `?1` parameter.
///
/// A count is almost always "how many rows match this id", and
/// [`query_i64`]'s name does not say so -- it is `COUNT(*)` as often as it is a
/// value. Naming the difference here keeps every call site from growing an
/// `#[allow]` or a comment.
pub fn query_count_where(database: &Path, statement: &str, parameter: &str) -> i64 {
    let connection =
        rusqlite::Connection::open(database).expect("the database file the server created");
    connection
        .query_row(statement, [parameter], |row| row.get(0))
        .expect("the query to return exactly one row")
}

/// Mints and stores a live session for `user_id`, through the production path.
///
/// **The same three calls `auth::login` makes**, in the same order, and the
/// difference is only that it happens in-process instead of over a socket. That
/// matters: a fixture that invented its own token format would test a server
/// against tokens the server never mints.
pub fn issue_session(store: &Store, user_id: &str) -> io::Result<String> {
    let token = auth::mint_session_token();
    let now = time::now_unix_millis();
    let expires_at = now.saturating_add(auth::SESSION_TTL_MILLIS);
    store
        .insert_session(&auth::hash_session_token(&token), user_id, now, expires_at)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(token)
}

/// One HTTP response, read whole.
///
/// `head` is kept as well as `status` because a test may need a response *header* --
/// the auth suites assert on `WWW-Authenticate`, which is the whole point of a 401
/// carrying one -- and `body` because every refusal here is a JSON document whose
/// `code` is the assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The numeric status from the status line.
    pub status: u16,
    /// The full response head, headers included, with CRLFs intact.
    pub head: String,
    /// The body, read according to `Content-Length`. Empty when there was none.
    pub body: String,
}

impl Response {
    /// Whether this response is a WebSocket upgrade.
    pub fn is_upgrade(&self) -> bool {
        self.status == 101
    }

    /// The `code` field of a JSON refusal body, if the body carries one.
    ///
    /// Owned rather than borrowed: the value comes out of a parsed document that
    /// has to be built first, and returning a `&str` into it would borrow a
    /// temporary.
    pub fn code(&self) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()?
            .get("code")?
            .as_str()
            .map(str::to_owned)
    }
}

/// Attempts a WebSocket upgrade and returns the HTTP answer, closing the socket.
///
/// **This is how a suite observes a *refusal* rather than a connection.** It reads
/// the status line, the headers and the body, then drops the socket -- so a test can
/// assert "401, no 101, `WWW-Authenticate: Bearer`" without a socket ever existing,
/// which is precisely the claim `ws.rs`'s pre-upgrade check makes.
///
/// # Arguments
///
/// * `address` - the server's address.
/// * `token` - the bearer token to present, or `None` to present no header at all.
pub async fn attempt_upgrade(
    address: std::net::SocketAddr,
    token: Option<&str>,
) -> io::Result<Response> {
    let (response, mut stream, _) = upgrade(address, token).await?;
    // The socket is closed immediately: a 401 has no body a test needs to keep
    // reading frames from, and an upgrade answer that a suite is inspecting is one
    // it has decided not to use.
    let _ = stream.shutdown().await;
    Ok(response)
}

/// Sends one `POST` with a JSON body, and reads the answer.
///
/// # Arguments
///
/// * `address` - the server's address.
/// * `path` - from the crate's own constants, so a test cannot drift from them.
/// * `token` - the bearer token to present, or `None` for an unauthenticated call.
/// * `body` - the JSON document to send, encoded by the caller.
pub async fn post_json(
    address: std::net::SocketAddr,
    path: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> io::Result<Response> {
    let mut stream = Socket::connect(address).await?;
    let payload = body.to_string();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n",
        payload.len()
    );
    if let Some(token) = token {
        // Verbatim, with no validation: `tests/auth.rs` needs to present
        // deliberately malformed values, and a fixture that sanitised its input
        // could not test the production shape check.
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);

    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    read_response(&mut stream).await
}

/// Performs the HTTP/1.1 upgrade handshake, returning the answer and the socket.
///
/// Shared by [`attempt_upgrade`] and [`TestClient::connect`] so that there is one
/// spelling of the request bytes in this file and therefore one thing to fix when
/// the header set changes.
async fn upgrade(
    address: std::net::SocketAddr,
    token: Option<&str>,
) -> io::Result<(Response, Socket, Vec<u8>)> {
    let mut stream = Socket::connect(address).await?;
    let mut request = format!(
        "GET {WS_PATH} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n"
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");

    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut pending = Vec::new();
    let head_end = read_until_header_end(&mut stream, &mut pending).await?;
    let head = String::from_utf8_lossy(&pending[..head_end]).into_owned();
    let remainder = pending.split_off(head_end + 4);

    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the server's status line was not a status line: {head:?}"),
            )
        })?;

    // A 101 has no body and no `Content-Length`: frames follow it immediately.
    // Anything else from axum is a `Full` body with a known length, which is what
    // makes reading it here a `read_exactly` rather than a guess.
    if status == 101 {
        return Ok((
            Response {
                status,
                head,
                body: String::new(),
            },
            stream,
            Vec::new(),
        ));
    }

    let declared = content_length(&head).unwrap_or(0);
    let mut body_bytes = remainder;
    while body_bytes.len() < declared {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..read]);
    }

    Ok((
        Response {
            status,
            head,
            body: String::from_utf8_lossy(&body_bytes).into_owned(),
        },
        stream,
        Vec::new(),
    ))
}

/// Reads until the end of an HTTP response's headers, returning that offset.
async fn read_until_header_end(stream: &mut Socket, pending: &mut Vec<u8>) -> io::Result<usize> {
    loop {
        if let Some(offset) = find_subslice(pending, b"\r\n\r\n") {
            return Ok(offset);
        }
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the server closed the connection during the websocket handshake",
            ));
        }
        pending.extend_from_slice(&chunk[..read]);
    }
}

/// The `Content-Length` a response head declares, if it declares one.
fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())?
    })
}

/// Reads one HTTP response head and body from an open socket.
async fn read_response(stream: &mut Socket) -> io::Result<Response> {
    let mut pending = Vec::new();
    let head_end = read_until_header_end(stream, &mut pending).await?;
    let head = String::from_utf8_lossy(&pending[..head_end]).into_owned();
    let mut body_bytes = pending.split_off(head_end + 4);

    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the server's status line was not a status line: {head:?}"),
            )
        })?;

    let declared = content_length(&head).unwrap_or(0);
    while body_bytes.len() < declared {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..read]);
    }

    Ok(Response {
        status,
        head,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
    })
}

/// One frame received by a [`TestClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A text frame, as UTF-8. The protocol's frames are all text frames.
    Text(String),
    /// A binary frame. This client never sends one; the server must reject it.
    Binary(Vec<u8>),
    /// A ping. The server answers these itself; this client only records them.
    Ping,
    /// A pong.
    Pong,
    /// A close frame, carrying the code if the peer sent one.
    Close(Option<u16>),
}

/// A minimal WebSocket client, enough to drive this server and no more.
///
/// See the module docs for why it exists and what it deliberately does not do.
pub struct TestClient {
    stream: TcpStream,
    pending: Vec<u8>,
    frames_sent: u32,
    open: bool,
}

impl TestClient {
    /// Performs the HTTP/1.1 upgrade handshake against `address`.
    ///
    /// # Arguments
    ///
    /// * `address` - the server's address.
    /// * `token` - the bearer token to present, or `None` to present no
    ///   `Authorization` header at all. **`None` is expected to fail**, because this
    ///   server refuses an unauthenticated upgrade before it produces a socket --
    ///   which is why [`TestServer::connect_anonymous`] is the named way to ask for
    ///   it and why [`attempt_upgrade`] is the way to inspect the refusal.
    ///
    /// # Errors
    ///
    /// `io::Error` if the connection cannot be made or the server does not answer
    /// with `101`. A non-101 answer carries the server's status and body into the
    /// error text, because the most likely cause by far is a credential problem and
    /// "connection refused" would send the reader to the wrong file.
    pub async fn connect(address: std::net::SocketAddr, token: Option<&str>) -> io::Result<Self> {
        let (response, stream, _) = upgrade(address, token).await?;
        if !response.is_upgrade() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!(
                    "expected a 101 upgrade, got {} with {:?}",
                    response.status, response.body
                ),
            ));
        }
        Ok(Self {
            stream,
            pending: Vec::new(),
            frames_sent: 0,
            open: true,
        })
    }

    /// Sends `payload` as one masked text frame.
    pub async fn send_text(&mut self, payload: &str) -> io::Result<()> {
        self.frames_sent += 1;
        let frame = encode_text(payload.as_bytes(), self.frames_sent);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await
    }

    /// The next frame, waiting up to [`FRAME_BUDGET`].
    ///
    /// # Panics
    ///
    /// If no frame arrives in time, or the socket fails. A suite that times out
    /// here has a server bug, and the panic text says which expectation was
    /// outstanding.
    pub async fn expect_event(&mut self, expected: &str) -> Event {
        match tokio::time::timeout(FRAME_BUDGET, self.next_event()).await {
            Ok(Ok(event)) => event,
            Ok(Err(error)) => panic!("waiting for {expected} failed: {error}"),
            Err(_) => panic!("timed out after {FRAME_BUDGET:?} waiting for {expected}"),
        }
    }

    /// The next frame's text, waiting up to [`FRAME_BUDGET`].
    ///
    /// # Panics
    ///
    /// If the next frame is not a text frame, or if nothing arrives in time.
    pub async fn expect_text(&mut self, expected: &str) -> String {
        match self.expect_event(expected).await {
            Event::Text(text) => text,
            other => panic!("expected a text frame for {expected}, got {other:?}"),
        }
    }

    /// The next frame's text, if one arrives within `budget`.
    ///
    /// # Panics
    ///
    /// If a *non-text* frame arrives, because that is never expected by the
    /// suites that call this and would otherwise look like a server sending
    /// nonsense rather than a test asserting an absence.
    pub async fn next_text_within(&mut self, budget: Duration) -> Option<String> {
        match tokio::time::timeout(budget, self.next_event()).await {
            Ok(Ok(Event::Text(text))) => Some(text),
            Ok(Ok(other)) => panic!("expected silence or a text frame, got {other:?}"),
            Ok(Err(error)) => panic!("the socket failed while waiting: {error}"),
            Err(_) => None,
        }
    }

    /// Waits for the peer to close, within [`FRAME_BUDGET`].
    ///
    /// **"Closed" deliberately covers four different observations**, because the
    /// platform decides which one happens and this suite has to pass on Windows:
    ///
    /// - a close frame, which is the graceful handshake;
    /// - end of stream, `UnexpectedEof`, which is a peer that dropped the socket
    ///   with nothing outstanding;
    /// - `ConnectionAborted` or `ConnectionReset`, which is what **Windows** does
    ///   when a socket is closed with unread data still in its receive buffer. A
    ///   server that refuses an oversized frame is exactly that case, so on Windows
    ///   the peer sees an RST rather than a FIN. Treating it as "not closed" would
    ///   make the size-ceiling test Windows-only-failing for a reason that has
    ///   nothing to do with the server.
    /// - a frame before the close, which is normal: tungstenite can flush queued
    ///   frames ahead of the close handshake.
    ///
    /// # Panics
    ///
    /// If the connection is still readable-and-open when the budget expires.
    pub async fn expect_closed(&mut self, expected: &str) {
        loop {
            match tokio::time::timeout(FRAME_BUDGET, self.next_event()).await {
                Ok(Ok(Event::Close(_))) => {
                    self.open = false;
                    return;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(error)) if means_the_connection_is_gone(&error) => {
                    self.open = false;
                    return;
                }
                Ok(Err(error)) => panic!("waiting for the close after {expected} failed: {error}"),
                Err(_) => panic!("timed out waiting for the connection to close after {expected}"),
            }
        }
    }

    /// Reads the next frame off the wire.
    ///
    /// Returns `Err` with `UnexpectedEof` at end of stream, which is what a peer
    /// that drops its socket looks like from here.
    pub async fn next_event(&mut self) -> io::Result<Event> {
        if !self.open {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "this client has already seen the close handshake",
            ));
        }
        let header = self.read_exactly(2).await?;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let short_length = usize::from(header[1] & 0x7f);

        let length = match short_length {
            126 => usize::from(u16::from_be_bytes(
                self.read_exactly(2).await?.try_into().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a 16-bit length must be two bytes",
                    )
                })?,
            )),
            127 => usize::try_from(u64::from_be_bytes(
                self.read_exactly(8).await?.try_into().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a 64-bit length must be eight bytes",
                    )
                })?,
            ))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the frame claims a length that does not fit in usize",
                )
            })?,
            other => other,
        };

        let mask = if masked {
            self.read_exactly(4).await?.try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "a mask must be four bytes")
            })?
        } else {
            [0_u8; 4]
        };

        let mut payload = self.read_exactly(length).await?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }

        match opcode {
            0x1 => Ok(Event::Text(String::from_utf8(payload).map_err(
                |error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("a text frame was not UTF-8: {error}"),
                    )
                },
            )?)),
            0x2 => Ok(Event::Binary(payload)),
            0x8 => Ok(Event::Close(read_close_code(&payload))),
            0x9 => Ok(Event::Ping),
            0xa => Ok(Event::Pong),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected websocket opcode {other:#x}"),
            )),
        }
    }

    /// Reads exactly `length` bytes, from the buffer first and then the socket.
    async fn read_exactly(&mut self, length: usize) -> io::Result<Vec<u8>> {
        while self.pending.len() < length {
            let mut chunk = [0_u8; 4096];
            let read = self.stream.read(&mut chunk).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("the peer closed the socket with {length} bytes outstanding"),
                ));
            }
            self.pending.extend_from_slice(&chunk[..read]);
        }
        Ok(self.pending.drain(..length).collect())
    }
}

/// Whether `error` means the peer is no longer there.
///
/// See [`TestClient::expect_closed`] for why the set is this wide rather than just
/// `UnexpectedEof`.
fn means_the_connection_is_gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
    )
}

/// Reads the close code out of a close frame's payload.
///
/// RFC 6455 §5.5.1: an empty payload means "no status given", and the first two
/// bytes are the code when there is one.
fn read_close_code(payload: &[u8]) -> Option<u16> {
    match payload.len() {
        0 => None,
        1 => None,
        _ => Some(u16::from_be_bytes([payload[0], payload[1]])),
    }
}

/// Encodes one masked text frame.
///
/// Masked because RFC 6455 §5.1 requires every client-to-server frame to be, and
/// because tungstenite enforces it: an unmasked frame from a client closes the
/// connection as a protocol error, which is the correct behaviour and would make
/// this fixture useless if it got it wrong.
fn encode_text(payload: &[u8], seed: u32) -> Vec<u8> {
    let mut frame = vec![0x81_u8];
    let length = payload.len();
    if length < 126 {
        frame.push(0x80 | length as u8);
    } else if length <= usize::from(u16::MAX) {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(length as u64).to_be_bytes());
    }

    let mask = mask_key(seed);
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

/// The masking key for one frame: four bytes derived from the frame counter.
///
/// Fresh per frame, as §5.3 requires, and predictable -- see the module docs for
/// why that is acceptable here and nowhere else.
fn mask_key(seed: u32) -> [u8; 4] {
    let value = seed.wrapping_mul(0x9E37_79B9);
    [
        value as u8,
        (value >> 8) as u8,
        (value >> 16) as u8,
        (value >> 24) as u8,
    ]
}

/// The offset of `needle` in `haystack`, if present.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
