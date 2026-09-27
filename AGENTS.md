# AGENTS.MD — Sh_Nexus

> Working rules for AI Agents contributing to Sh_Nexus
> Project: Native Team Chat App built with Rust + GPUI (Zed's framework)
> Version: 1.0.0
> Stack: Rust + GPUI (https://github.com/zed-industries/zed/tree/main/crates/gpui) — No JavaScript, no Electron, no web views, no runtime GC.

---

## 1. Project Context

Sh_Nexus is a native, GPU-accelerated team chat application (Slack-like) built with **Rust + GPUI**: declarative UI rendered directly to the GPU through Metal (macOS), DirectX (Windows), or Vulkan (Linux). This document defines the constraints, QA procedures, quality metrics, and coding standards every agent must follow when contributing to the project.

**Core priorities (in order):**
1. **Responsiveness**: 120fps UI, <16ms message render, zero jank while receiving messages
2. **Reliability**: No crashes, no lost messages, graceful reconnection, type safety
3. **Real-time performance**: Sub-100ms end-to-end message delivery, correct ordering
4. **Efficiency**: <80MB RAM idle, bounded message cache, minimal CPU
5. **Customization**: User-defined themes via JSON with hot reload

---

## 2. Code Philosophy

### 2.1 Safety First
- **No `unsafe` without justification**: Every `unsafe` block must have a `// SAFETY:` comment explaining why it's sound. Prefer safe abstractions.
- **No `unwrap()` in production paths**: Use `?`, `ok_or()`, `expect()` with context, or `match`. `unwrap()` is acceptable only in tests or with proven invariants.
- **No panics in user-facing code**: Recover from errors gracefully. Use `Result<T, ShNexusError>` for all fallible operations.
- **Strict types**: Leverage Rust's type system. No `String` for paths (use `PathBuf`/`Path`), no `f64` for pixel coordinates. Use `chrono::DateTime<Utc>` for all timestamps — never store raw strings.
- **Input validation**: Every public function must validate preconditions and return typed errors (see `errors.rs`). All incoming WebSocket and REST payloads must be validated against schemas before touching state.

### 2.2 Idiomatic Rust
- Follow the **Rust API Guidelines** and **Rust Style Guide**.
- Types: `PascalCase`. Functions/variables: `snake_case`. Constants: `SCREAMING_SNAKE_CASE`. Files: `snake_case.rs`.
- Run `cargo clippy -- -D warnings` before every commit. CI fails on warnings.
- Run `cargo fmt` and resolve all diffs before considering a task done.
- Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`, `impl Into<String>` for id parameters.
- Document all public items with `///` doc comments. Explain the *why*, not just the *what*.
- Use `thiserror` for error types, `serde` (+ `serde_json`) for serialization, `tracing` for logging.

### 2.3 Performance Conscious
- **Zero-cost abstractions**: Prefer iterators over loops, avoid unnecessary allocations, use `SmallVec`/`ArrayVec` for small collections (e.g., reactions on a message).
- **No blocking the frame loop**: All I/O (network, SQLite, disk) must be async or on worker threads. The GPUI main thread never blocks. Use `tokio` as the async runtime and `cx.spawn()` for async work triggered from the UI.
- **Memory efficiency**: Use `Arc<T>` for shared state (users, channels), `Cow<str>` for borrowed strings, avoid deep clones of message history. Virtualize the message list — never render 10,000 DOM-equivalent elements; render only visible messages.
- **Cache-friendly**: Structure data for cache locality. Use `Vec<T>` with contiguous storage over `HashMap` when iteration matters.
- **Profile before optimizing**: Use `cargo flamegraph`, `perf`, or `valgrind` to identify bottlenecks. Measure, don't guess.

---

## 3. Mandatory Code Structure

### 3.1 Module Organization

```
src/
├── main.rs              # Entry point only. Initializes App, opens window. Max ~50 lines.
├── app.rs               # Root component: global state, theme provider, key handling.
├── core/                # Pure business logic (no GPUI imports).
│   ├── models/          # user.rs, channel.rs, message.rs, events.rs
│   ├── markdown.rs      # Markdown parsing/rendering to a styled segment tree
│   ├── cache.rs         # LRU cache for avatars, rendered segments, attachment previews
│   ├── ordering.rs      # Message ordering, dedup, gap detection
│   └── theme.rs         # Theme parsing, validation, application
├── ui/                  # GPUI components (depend on gpui crate).
│   ├── components/      # Reusable widgets (buttons, reaction chips, typing dots, badges)
│   ├── views/           # Main views (sidebar, chat view, thread panel, input bar)
│   └── theme/           # Theme system integration
├── state/               # Global state management (app state, session, actions)
│   ├── app_state.rs     # Channels, messages, presence, current selection
│   └── actions.rs       # State mutations (send message, switch channel, add reaction)
├── network/             # Networking layer (tokio).
│   ├── rest.rs          # reqwest HTTP client (auth, channels, history)
│   ├── websocket.rs     # tokio-tungstenite WS client
│   ├── auth.rs          # JWT handling, token refresh
│   └── reconnect.rs     # Exponential backoff reconnection state machine
├── db/                  # Persistence layer (rusqlite).
│   ├── schema.rs        # SQLite schema + migrations
│   └── repository.rs    # CRUD operations (typed, async via spawn_blocking)
└── platform/            # OS-specific code (notifications, sounds, keychains)
```

### 3.2 Separation of Concerns
- **`core/`**: Pure, testable logic with no side effects. Must not import `gpui`, `tokio`, or any UI/platform code.
- **`ui/`**: Presentation only. Reads state, renders elements, dispatches actions. No business logic.
- **`state/`**: Global state containers. Uses `gpui::Global` for app-wide state. All mutations go through `actions.rs` so they are auditable and testable.
- **`network/`**: Protocol handling only. Parses wire formats into `core::models`. Emits domain events; never touches GPUI state directly — events flow into `state/` via `cx.update_global`.
- **`db/`**: Persistence only. All queries parameterized. Migrations are versioned and must never destroy user data silently.
- **`platform/`**: OS-specific implementations behind traits (notifications, sounds, secure token storage).

### 3.3 Error Types
- Define a global error enum in `src/errors.rs`:
  ```rust
  use thiserror::Error;

  #[derive(Error, Debug)]
  pub enum ShNexusError {
      #[error("network error: {0}")]
      Network(#[from] reqwest::Error),

      #[error("websocket error: {0}")]
      WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

      #[error("database error: {0}")]
      Database(#[from] rusqlite::Error),

      #[error("serialization error: {0}")]
      Serialization(#[from] serde_json::Error),

      #[error("auth error: {0}")]
      Auth(String),

      #[error("protocol error: {0}")]
      Protocol(String),

      #[error("config error: {0}")]
      Config(String),

      #[error("theme error: {0}")]
      Theme(String),

      #[error("unknown error: {0}")]
      Unknown(String),
  }

  pub type Result<T> = std::result::Result<T, ShNexusError>;
  ```
- Each module may define narrower error types that convert into `ShNexusError`.
- Never panic in production paths. Use `Result` everywhere.
- All network failures must surface to the user as recoverable states (reconnecting banner, offline mode), never as crashes or silent drops.

---

## 4. Unit Tests — Mandatory Standards

### 4.1 Minimum Coverage
- Business logic (core/): ≥ 90% coverage.
- Networking (network/): ≥ 80% coverage.
- State management (state/): ≥ 80% coverage.
- Persistence (db/): ≥ 85% coverage.
- Utilities: ≥ 85% coverage.
- UI components: Integration tests for critical flows.

### 4.2 What MUST Always Be Tested

| Component        | Required Tests                                                                                              |
| ---------------- | ----------------------------------------------------------------------------------------------------------- |
| `models/`        | Serde round-trips for every wire format; malformed payload rejection; timestamp ordering                    |
| `ordering.rs`    | Message ordering with out-of-order delivery; duplicate suppression; gap detection; pagination cursors       |
| `markdown.rs`    | Bold/italic/code/links/code blocks; nested formatting; malformed input never panics; injection safety       |
| `cache.rs`       | Insertion, LRU eviction, memory limit enforcement, hit/miss ratio, thread safety                            |
| `reconnect.rs`   | Backoff schedule (1s → 2s → 4s … capped at 60s); jitter; reset on success; max-attempt behavior             |
| `websocket.rs`   | Connect, send, receive; ping/pong keepalive; graceful close; malformed frame handling                       |
| `rest.rs`        | Request construction; auth header injection; error status mapping; retry on 5xx/network (never on 4xx)      |
| `auth.rs`        | Token expiry checks; refresh flow; logout clears all credentials                                            |
| `repository.rs`  | Schema migrations; insert/query messages; channel switching; foreign key integrity; concurrent read/write   |
| `theme.rs`       | JSON parsing; schema validation; color format validation; fallback behavior                                 |
| `app_state.rs`   | Action mutations (send/receive/switch channel, reactions); unread counts; optimistic send + rollback        |

### 4.3 Test Style
- Use descriptive names: receiving_duplicate_message_is_idempotent() instead of test_message().
- Use rstest or test-case for parameterized tests.
- Every test must be independent: no reliance on execution order or shared state. Use `tempfile` for SQLite tests and unique DB paths per test.
- Mock the network when possible for faster, hermetic tests: define a `Transport` trait so `websocket.rs` and `rest.rs` can be tested without sockets. Use `wiremock` for HTTP and a tokio channel pair for WS.
- Use fake clocks for time-dependent logic (backoff, typing timeout, token expiry). Never `sleep()` in unit tests to "wait" for logic.

### 4.4 Property-Based Tests
- Use proptest for ordering/dedup logic: "for any shuffle of a message batch, the ordered result is identical and contains no duplicates."
- Use proptest for markdown parsing: "parsing never panics for arbitrary input."

---

## 5. QA Procedures

### 5.1 Pre-Commit Checklist (Mandatory)
Before marking any task as "complete", the agent must verify:
- [ ] `cargo check` passes with zero errors
- [ ] `cargo clippy -- -D warnings` passes with zero warnings
- [ ] `cargo fmt --check` passes
- [ ] `cargo test` passes (all unit, integration, and doc tests)
- [ ] `cargo build --release` succeeds
- [ ] New public items have `///` doc comments
- [ ] No `unsafe` without `// SAFETY:` justification
- [ ] No `unwrap()`/`expect()` in production paths without proven invariant
- [ ] No blocking operations on the UI thread
- [ ] Test coverage of new code ≥ 80%
- [ ] Benchmarks show no regression (> 5% degradation)
- [ ] No TODOs or FIXMEs in new code

### 5.2 Manual QA (for UI features)
For each UI feature, perform these manual checks (document in the PR):
- **Functionality**: Does it do what it should?
- **Edge cases**: Empty channel, 10,000-message history, 500-character message, emoji-only message, very long single-word message, offline start.
- **Real-time**: Two instances exchanging messages — ordering, no duplicates after reconnect, typing indicators appear/disappear correctly.
- **Cross-platform**: Does it work on Windows, macOS, and Linux?
- **Accessibility**: Is it usable with keyboard only? (Verify focus order: input bar → send, Ctrl+K switcher, Escape closes panels.)
- **Performance**: No perceptible lag when messages arrive during scroll? Check frame time histogram.
- **Memory**: No leaks? Monitor RSS over 30 minutes of active chatting.

### 5.3 Code Review (Agent ↔ Agent)
- Any change to `core/`, `network/`, or `db/` requires review from another agent.
- Any change that modifies the architecture requires updating `docs/ARCHITECTURE.md`.
- Any change to the WebSocket protocol requires updating `docs/API.md` and version negotiation notes.
- Reviews must verify: correct logic, adequate tests, documentation, performance implications, and adherence to this AGENTS.MD.

---

## 6. Quality Metrics and Thresholds

### 6.1 Automated Metrics (CI)
| Metric                  | Minimum Threshold | Target Threshold | Tool                     |
| ----------------------- | ----------------- | ---------------- | ------------------------ |
| Test coverage (total)   | 75%               | 85%              | `cargo tarpaulin`        |
| Test coverage (`core/`) | 85%               | 95%              | `cargo tarpaulin`        |
| Clippy warnings         | 0                 | 0                | `cargo clippy`           |
| Compiler warnings       | 0                 | 0                | `cargo check`            |
| Format conformance      | 100%              | 100%             | `cargo fmt --check`      |
| Build time (dev, cold)  | < 120s            | < 60s            | CI timer                 |
| Build time (release)    | < 5 min           | < 3 min          | CI timer                 |
| Binary size (release)   | < 30MB            | < 20MB           | `ls -lh target/release/` |
- Note: GPUI statically links the renderer. Binary size is larger than typical Rust apps but smaller than Electron. Validate against release builds.

### 6.2 Performance Metrics (Benchmarks)
| Metric                                            | Maximum Threshold | Tool                           |
| ------------------------------------------------- | ----------------- | ------------------------------ |
| Cold start to interactive (cached session)        | < 300ms           | `cargo bench`                  |
| Cold start to interactive (login required)        | < 500ms           | `cargo bench`                  |
| Render latency: message appears after WS receive  | < 16ms            | `cargo bench`                  |
| Render latency: channel switch (cached, 100 msgs) | < 16ms            | `cargo bench`                  |
| Render latency: channel switch (cold, 500 msgs)   | < 100ms           | `cargo bench`                  |
| Scroll frame time (10k messages, virtualized)     | < 8ms (120fps)    | GPUI frame instrumentation     |
| Send latency: Enter press → message on screen     | < 16ms            | `cargo bench`                  |
| End-to-end delivery (two local instances)         | < 100ms           | Integration bench              |
| Reconnect time after network drop                 | < 5s              | Integration bench              |
| Idle RAM usage                                    | < 80MB            | `/usr/bin/time -v` or OS tools |
| RAM with 10k cached messages                      | < 200MB           | OS process monitor             |
| CPU usage (idle, connected)                       | < 2%              | OS process monitor             |
| SQLite query (paged history, 100 rows)            | < 5ms             | `cargo bench`                  |

### 6.3 Regressions
- Any regression > 10% in performance metrics blocks the merge.
- Any drop in test coverage blocks the merge.
- Any new clippy warning blocks the merge.
- Any increase > 5MB in binary size requires justification.

---

## 7. Agent Restrictions

### 7.1 Absolute Prohibitions
| Restriction                                       | Reason                                                |
| ------------------------------------------------- | ----------------------------------------------------- |
| ❌ No `unsafe` without `// SAFETY:` justification | Memory safety is priority #1                          |
| ❌ No `unwrap()`/`expect()` in production paths   | Panics kill UX and reliability                        |
| ❌ No blocking I/O on the main thread             | The app must feel fluid at all times                  |
| ❌ No deep clones in hot paths                    | Performance degradation                               |
| ❌ No `println!` in production                    | Use `tracing` with levels                             |
| ❌ No new dependencies without justification      | Every dependency is compile time + supply chain risk  |
| ❌ No `TODO` without a ticket/issue               | Every TODO must have an associated issue              |
| ❌ No unbounded growth of in-memory state         | Message history, typing users, and logs must be bounded |
| ❌ No plaintext storage of tokens                 | Use the OS keychain via `platform/` abstraction       |
| ❌ No JavaScript, no WASM, no web tech            | This is a native Rust application                     |

### 7.2 Dependencies — Approval Process
- Before adding any crate to Cargo.toml:
- Verify no solution exists with current dependencies or std library.
- Evaluate: Is it maintained? > 500 downloads/month? Last commit < 6 months?
- Verify the license (must be compatible: MIT, Apache-2.0, BSD, etc.).
- Check compile time impact (`cargo build --timings`).
- Document the justification in a comment above the dependency in Cargo.toml:
```toml
# syntect: Pure Rust syntax highlighting, Sublime Text syntaxes, no C deps
syntect = "5.2"
```

### 7.3 GPUI-Specific Rules
- Element tree is code: UI is built with method chaining, not markup. Keep trees readable with helper functions.
- Text requires explicit color: GPUI does not inherit color from parents. Always set `.text_color()` on text elements.
- Async by default: Use `cx.spawn()` for async operations. Never block the UI thread.
- Global state: Use `cx.set_global()` / `cx.global::<T>()` for app-wide state (session, theme, caches).
- Virtualized lists: Message lists must render only visible items. Use a uniform row estimator and recycle.
- No DOM APIs: This is not a web app. No document, no window, no fetch.
- No blocking `cx.update_global` from non-UI threads: schedule updates on the main thread via `cx.update`/`cx.update_global`.

### 7.4 Networking Rules
- All wire payloads are versioned: include a `v` field in WS envelopes; reject unknown major versions explicitly.
- Every WS message must have a client-generated `client_msg_id` (UUID) for idempotent dedup across reconnects.
- Reconnection must resume from the last known `last_message_at` cursor — never rely solely on "live" delivery during a gap.
- Timeouts on everything: connect (5s), read (30s keepalive → ping), request (10s).
- Never retry non-idempotent POSTs blindly without an idempotency key.
- TLS certificate validation must never be disabled in release builds.

### 7.5 Logging and Observability
- Use tracing (structured logging) instead of println!.
- **Never log message content, tokens, or credentials.** Log ids, sizes, and outcomes only.
- Levels:
  - error!: Errors affecting functionality (WS failure, DB corruption, auth failure).
  - warn!: Recoverable situations (reconnect scheduled, malformed payload dropped, cache eviction).
  - info!: Significant user events (login, channel switch, reconnect success, notification sent).
  - debug!: Development details (cache hits/misses, frame times, render timings).
  - trace!: Very detailed info (per-frame UI events, element tree diffs, wire payloads sans content).
- In release builds, the minimum level must be info (set via RUST_LOG or config).

---

## 8. Mandatory Integration Tests

### 8.1 Critical Flows
Each of these flows must have an integration test:

- **Login Flow:**
  - Launch app → Login screen → Valid credentials → Token stored in keychain → Channels load → Chat view
- **Auth Failure Flow:**
  - Invalid credentials → Clear, actionable error → No crash → Retry works
- **Real-time Flow:**
  - Two clients → A sends message → B receives < 100ms → Correct order, sender, timestamp
- **Reconnect Flow:**
  - Connected → Kill server → Client shows "reconnecting" with backoff → Restart server → Resumes from cursor → No duplicates, no gaps
- **Offline Flow:**
  - Start with no network → Cached channels/messages render → Queued sends marked pending → Deliver on reconnect in order
- **Optimistic Send Flow:**
  - Send → Message appears instantly (pending state) → ACK updates state → Failure rolls back with visible error
- **Channel Switch Flow:**
  - Switch channel → History loads from SQLite → Unread badge clears → Correct scroll position
- **Typing Indicator Flow:**
  - A types → B sees indicator within 1s → A stops → Indicator clears after timeout (no stuck indicators)
- **History Pagination Flow:**
  - Scroll to top → Older page loads prepended → Scroll position preserved → No duplicates at boundary
- **Error Flow:**
  - Server returns 500 / WS closes abnormally → App degrades gracefully → User-facing state shown

### 8.2 Test Fixtures
- Store test assets in `tests/fixtures/`.
- Include: sample theme JSON (valid + invalid), a recorded WS session (JSONL of envelopes), a populated SQLite DB per schema version, message payloads for every protocol version, and large generated message histories (via proptest generators, not checked-in files).

## 9. Mandatory Documentation

### 9.1 Doc Comments
```rust
/// Sends a message to a channel with optimistic UI update.
///
/// The message is assigned a client-side UUID, rendered immediately in a
/// pending state, and reconciled when the server ACK (or failure) arrives
/// on the WebSocket.
///
/// # Arguments
///
/// * `channel_id` - Target channel. Must be a channel the user has joined.
/// * `content` - Plain-text message body. Markdown is rendered, never executed.
///
/// # Returns
///
/// The client-generated message id, usable for cancel/retry.
///
/// # Errors
///
/// Returns `ShNexusError::Network` if the socket is not connected; the
/// message is then queued in the outbox instead of failing.
///
/// # Example
///
/// ```no_run
/// let id = send_message(&state, "chan_123", "hello team").await?;
/// ```
pub async fn send_message(state: &AppState, channel_id: &str, content: &str) -> Result<String> {
    // ...
}
```

### 9.2 Architectural Decisions
- Every significant architectural decision (framework choice, WS protocol, caching strategy, offline model, theme system, etc.) must be documented in `docs/ARCHITECTURE.md` using the ADR (Architecture Decision Record) format:
  - Context
  - Decision
  - Consequences
  - Alternatives considered

---

## 10. Theme System Requirements

### 10.1 Theme Format
- Themes are JSON files with schema validation:
```json
{
  "name": "Midnight Terminal",
  "author": "username",
  "version": 1,
  "colors": {
    "background": "#1e1e2e",
    "surface": "#313244",
    "sidebar": "#181825",
    "text": "#cdd6f4",
    "text_muted": "#7f849c",
    "accent": "#89b4fa",
    "accent_hover": "#b4befe",
    "danger": "#f38ba8",
    "success": "#a6e3a1",
    "mention": "#f9e2af",
    "code_block_bg": "#11111b",
    "bubble_self": "#45475a",
    "bubble_other": "#313244"
  },
  "spacing": { "xs": 4, "sm": 8, "md": 16, "lg": 24 },
  "radii": { "sm": 4, "md": 8, "lg": 16 },
  "typography": {
    "family": "Inter",
    "sizes": { "caption": 11, "timestamp": 10, "body": 14, "title": 16 }
  }
}
```

### 10.2 Theme Requirements
- Hot reload: Changes to theme files apply without restart.
- Validation: Invalid themes fall back to default with an error message in-app.
- Distribution: Users can share themes as single JSON files.
- Discovery: App scans `~/.config/sh_nexus/themes/` for user themes.
- Built-in: At least 3 high-quality themes included in the binary (dark, light, high-contrast).

---

## 11. Final Checklist Before Delivery
Before considering a task or sprint complete:

### Code Verification
- [ ] All new code has unit tests
- [ ] Tests pass locally (`cargo test`)
- [ ] Tests pass in CI
- [ ] Clippy passes with zero warnings
- [ ] Format is correct (`cargo fmt --check`)
- [ ] No `unsafe` without justification
- [ ] No `unwrap()` in production paths
- [ ] Doc comments on public items

### Quality Verification
- [ ] Test coverage ≥ 80% for new code
- [ ] Benchmarks show no regression
- [ ] No memory leaks (verify with valgrind or OS tools over a 30-min chat session)
- [ ] App does not crash on network loss, malformed payloads, or server errors
- [ ] Binary size increase justified

### Performance Verification
- [ ] Frame time < 8ms under load (message burst during scroll)
- [ ] RAM usage within thresholds
- [ ] No blocking operations on UI thread
- [ ] CPU usage < 2% at idle

### Documentation Verification
- [ ] `docs/ARCHITECTURE.md` updated if applicable
- [ ] `docs/API.md` updated for protocol changes
- [ ] CHANGELOG.md updated
- [ ] README.md updated if usage changed
- [ ] Theme schema documented if modified

### UX Verification
- [ ] Feature works with keyboard
- [ ] Feature works with mouse/trackpad
- [ ] Error messages are clear to the user
- [ ] No perceptible lag
- [ ] Theme changes apply instantly
- [ ] Offline/degraded states are clearly communicated

---

## 12. Quick Reference Commands
```bash
# Verify everything before commit
cargo check && cargo clippy -- -D warnings && cargo fmt --check && cargo test

# Run tests with coverage
cargo tarpaulin --out Html

# Run benchmarks
cargo bench

# Development (debug build with logging)
RUST_LOG=debug cargo run

# Type check only
cargo check

# Production build (optimized)
cargo build --release

# Check binary size
ls -lh target/release/sh_nexus

# Profile performance
cargo flamegraph --bin sh_nexus

# Clippy with all features
cargo clippy --all-targets --all-features -- -D warnings
```

---

> Remember: Reliability over features. A chat app that never loses a message and never freezes beats one with more features. Sh_Nexus must be an example of idiomatic, safe, performant Rust on a truly native GPU stack. Every millisecond of latency and every lost message matters.
