# Sh_Nexus — Native Team Chat App (Rust + GPUI)

> **Precedence rule:** `AGENTS.md` is the constitution of this project. This document is
> subordinate to it. Where the two ever disagree, `AGENTS.md` wins and this file is the bug.
>
> A subordinate document may **request** an amendment to the constitution through the ADR path
> (`AGENTS.md` §9.2). It may **not** pre-apply one. Rev 2 broke that rule and was corrected.
>
> **Revision history**
> - Rev 1 — original. Contradicted `AGENTS.md` in eight places.
> - Rev 2 — reconciled to `AGENTS.md`. Independent conformance audit found 4 blockers,
>   13 major and 21 minor residual defects, including two self-contradictions and a test
>   layout that would have silently voided all ten mandatory integration flows.
> - Rev 3 — audit corrections applied. Tracked in `docs/ARCHITECTURE.md` as ADR-004.

## Goal

Build a native desktop team chat application (Slack-like) using Rust and GPUI, with a real
backend for real-time messaging. The project demonstrates: native UI, networking, concurrency,
persistence, and full-stack capability.

---

## 1. Architecture Decision: GPUI distribution channel

**Status:** Decided. This is the highest-consequence choice in the project and it gates all work.
Recorded as **ADR-001**.

### Context

The published `gpui` crate on crates.io is version `0.2.2`, last published 2025-10-22 — roughly
eleven months stale while Zed ships daily releases (1.19.1 as of 2026-09-04, per zed.dev).
Its README states the user must "be on macOS or Linux", and its `windows-manifest` feature is
an empty list that only embeds an app manifest. It provides no Windows platform backend.

Zed *itself* does run on Windows, but that is the monorepo build, not the standalone crate.
Conflating the two is the most common error made about GPUI platform support. For accuracy:
upstream's Windows path uses **DirectX 11 for rendering** (a backend Zed built specifically to
cover Windows 7+ and VMs), with **Win32 for windowing and DirectWrite for text**.

### Decision

Depend on GPUI directly from `zed-industries/zed` via a **git dependency with a pinned `rev`**,
using two crates:

- `gpui` — the platform-agnostic core
- `gpui_platform` — the dispatcher that selects the OS backend

`gpui_platform` is **not optional**; depending on `gpui` alone yields no window on Windows.
Upstream extracted the platform backends out of `gpui` (commit "gpui: Extract gpui_platform out
of gpui", 2026-02-19) into `gpui_apple`, `gpui_macos`, `gpui_linux`, `gpui_windows`,
`gpui_wgpu`, and `gpui_web`. On `main`, the `wayland` and `x11` features of `gpui` no longer
carry `blade-graphics` / `cosmic-text` / `x11rb` / `objc2-metal`, and `macos-blade` is gone.

**License:** Apache-2.0, verified against the repository root `LICENSE` and
`crates/gpui/LICENSE-APACHE` ("Copyright 2022-2025 Zed Industries, Inc."). No copyleft
obligations. Some third-party crate aggregators incorrectly report Zed as GPL-3.0-or-later;
that claim is false. Do not source license facts from aggregators.

### Consequences

- Every upstream sync is a **deliberate upgrade task**, not a `cargo update`. Review breaking
  changes across the dependency tree before moving the pin.
- A `git` dependency is not publishable to crates.io and expresses no semver range. Accepted;
  see ADR-002.
- The pinned `rev` is the upgrade boundary. Record it in `Cargo.toml` with a comment naming the
  date and the upstream version it corresponds to.
- **Compile time is a real cost.** GPUI's tree is large. The Phase 0 spike must record
  `cargo build --timings` output as the baseline required by `AGENTS.md` §7.2.

### Alternatives considered

| Option | §7.2 verdict | Why not chosen |
|---|---|---|
| `gpui = "0.2.2"` from crates.io | Passes maintenance bars | Stale by ~11 months; no Windows backend. Disqualifying. |
| `gpui-component` / `gpui-kit` (longbridge) | Passes all five criteria; 0.6.6 published 2026-09-21, ~82k downloads in 90 days | Actively maintained and cross-platform. Rejected because it is a third-party opinionated design system, and this project builds its own UI layer anyway. **Reconsider if pin upkeep becomes a burden.** |
| `gpui-unofficial` | **FAILS §7.2**: its Windows backend crate `gpui-windows-gpui-unofficial` has 1,583 all-time downloads — far under the 500/month bar the constitution sets. Maintained by one person and explicitly not by the Zed team. | Rejected on §7.2 grounds, not on preference. |
| `open-gpui-platform`, `gpui-ce` | Incomplete assessment | Available republishes; would need the same §7.2 audit before adoption. |
| egui / iced / Slint | Passes all five criteria | Mature Windows support on crates.io, but abandons the portfolio's "built on Zed's framework" premise. |

**Fallback:** if the Phase 0 spike fails, fall back to `gpui-kit` or `gpui-unofficial`. That
requires **revising ADR-001 first** — it is not an in-flight substitution. The rest of the
architecture is unaffected, because GPUI is confined to `ui/` and `app.rs`.

---

## 2. Tech Stack

| Layer | Technology | Purpose |
|---|---|---|
| UI | GPUI via pinned git `rev` (`gpui` + `gpui_platform`) | Native rendering, layout, input |
| Runtime | tokio + `gpui_tokio` | Async runtime, concurrency |
| Networking | tokio-tungstenite | WebSocket client for real-time messaging |
| HTTP | reqwest | REST API calls (auth, channels, history) |
| Persistence | rusqlite (SQLite, `bundled`) | Local message cache, outbox, preferences |
| Serialization | serde + serde_json | Message encoding/decoding |
| Identifiers | uuid | `client_msg_id` per §7.4 |
| Time | chrono | `DateTime<Utc>` per §2.1 |
| Small collections | smallvec | `SmallVec` for reactions/attachments per §2.3 |
| Markdown | pulldown-cmark | CommonMark parsing into a styled segment tree |
| Syntax highlighting | syntect (`default-fancy`) | Code blocks in messages |
| **Logging** | **tracing + tracing-subscriber** | **Structured logging per §2.2/§7.5** |
| File watching | notify | Theme hot reload (§10.2) |
| Auth | jsonwebtoken | Server-side JWT issue/verify |
| Audio | rodio | Notification sounds |
| Notifications | notify-rust | System notifications |
| Backend | Rust + Axum, **workspace member** | WebSocket server, REST API, SQLite |

### Dependency governance (AGENTS.md §7.2)

The plan states *why* each non-obvious choice exists. The full five-criterion audit per crate —
version, license, last-commit date, downloads/month, `cargo build --timings` delta, and the
justification comment — lives in **`docs/DEPENDENCIES.md`**, and is a **Phase 1 exit criterion**.
A dependency is not added until that row exists.

Decisions recorded here:

- **`tracing` + `tracing-subscriber` are mandatory, not optional.** §7.1 prohibits `println!` in
  production; without a sanctioned logger that ban is unimplementable and an implementer will
  either violate it or invent a substitute.
- **`syntect` must use `default-features = false, features = ["default-fancy"]`.** The default
  `onig` feature pulls the C library `oniguruma`, requiring a C toolchain and complicating the
  MSVC build. `default-fancy` is pure Rust.
- **Two C dependencies are accepted deliberately, with the tradeoff stated:** `syntect` is
  configured to avoid `oniguruma`; `rusqlite/bundled` compiles SQLite's C amalgamation, which
  buys a self-contained, version-locked database at the cost of a C build step. Both are
  recorded in `docs/DEPENDENCIES.md` with measured compile-time deltas.
- **`pulldown-cmark`**, pure Rust, CommonMark-compliant, no C dependencies; its event stream
  maps cleanly onto `core/markdown.rs`.
- **`notify`** is the filesystem watcher for theme hot reload and is **distinct from
  `notify-rust`**, which is the OS notification crate. Both are present; they are not
  substitutes for each other.
- No dependency is added without a justification comment above it in `Cargo.toml`.

### GPUI feature flags per target

Recorded because `AGENTS.md` §7.2 requires checking compile-time impact, and because one of
these gates the entire test strategy:

| Crate | macOS | Linux | Windows |
|---|---|---|---|
| `gpui_platform` | `font-kit` | `wayland`, `x11` | **no features required** |
| `gpui` | — | — | — |

**`test-support` is required.** `gpui_platform`'s `test-support` feature enables
`gpui/test-support`, `gpui_macos/test-support` and `gpui_windows/test-support`. Without it the
headless test harness in §10 does not exist. Enabling it is part of the Phase 0 spike.

---

## 3. Code Philosophy and Observability

Restating `AGENTS.md` §2.1, §2.2, §7.1 and §7.5 as binding implementation rules. Rev 2 omitted
this section entirely, which left §7.1's `println!` ban with no sanctioned alternative and left
the highest-risk safety rules unstated.

### Safety (non-negotiable)

- **No `unsafe` without a `// SAFETY:` comment** explaining why the block is sound. Prefer safe
  abstractions. This applies most heavily to `platform/` (keychain FFI, `rodio`, `notify-rust`)
  and the virtualized list — exactly where `unsafe` creeps in.
- **No `unwrap()` or `expect()` in production paths.** Use `?`, `ok_or()`, `expect()` with
  context, or `match`. Permitted in tests or with a proven invariant.
- **No panics in user-facing code.** Every fallible operation returns `Result<T, ShNexusError>`.
  Network failures surface as recoverable states — a reconnecting banner, offline mode — never
  as crashes or silent drops.
- **All incoming WebSocket and REST payloads are validated against schemas before touching
  state.** No unvalidated payload reaches `state/`.
- **No plaintext storage of tokens.** Use the OS keychain via the `platform/` abstraction.

### Types

- **No `String` for paths.** Use `PathBuf` / `Path` — relevant to theme discovery under
  `~/.config/sh_nexus/themes/` and the SQLite file path.
- **All timestamps are `chrono::DateTime<Utc>`.** Never a raw string. Serialize via chrono's
  serde impl; do not hand-format RFC-3339.
- Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`, `impl Into<String>` for id parameters.
- Use `SmallVec` / `ArrayVec` for small collections — reactions and attachments on a message are
  the canonical case.

### Logging (AGENTS.md §7.5)

`tracing` with levels, never `println!`:

| Level | Use |
|---|---|
| `error!` | Errors affecting functionality (WS failure, DB corruption, auth failure) |
| `warn!` | Recoverable situations (reconnect scheduled, malformed payload dropped, cache eviction) |
| `info!` | Significant user events (login, channel switch, reconnect success, notification sent) |
| `debug!` | Development detail (cache hits/misses, frame times, render timings) |
| `trace!` | Very detailed (per-frame UI events, element tree diffs, wire payloads sans content) |

- **Never log message content, tokens, or credentials.** Log ids, sizes, and outcomes only.
- **In release builds the minimum level is `info`**, via `RUST_LOG` or config.
- Log retention is bounded (§7.1: no unbounded in-memory growth).

### Documentation

All public items carry `///` doc comments explaining the *why*, following the `AGENTS.md` §9.1
template (`# Arguments`, `# Returns`, `# Errors`, `# Example`).

### TLS

**TLS certificate validation is never disabled in release builds.** No `danger_accept_invalid_certs`
outside tests, and no code path that can enable it at runtime.

---

## 4. Project Structure

`AGENTS.md` §3.1's tree, applied **per crate** (recorded as **ADR-003**), inside a Cargo
workspace so the client, the wire crate and the server share one toolchain, one `cargo test`,
and one CI matrix.

```
Sh_Nexus/
├── Cargo.toml                 # workspace root
├── Cargo.lock
├── .gitignore
├── .github/
│   └── workflows/
│       └── ci.yml             # AGENTS.md §6.1 "Automated Metrics (CI)"
├── crates/
│   ├── sh_nexus/              # the client binary
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── main.rs        # entry point only; initializes app, opens window. Max ~50 lines
│   │   │   ├── app.rs         # root component, global state, theme provider, key handling
│   │   │   ├── errors.rs      # ShNexusError (AGENTS.md §3.3)
│   │   │   ├── core/          # PURE logic. No gpui, no tokio, no I/O, no filesystem.
│   │   │   │   ├── mod.rs
│   │   │   │   ├── models/    # user.rs, channel.rs, message.rs, events.rs
│   │   │   │   ├── markdown.rs
│   │   │   │   ├── cache.rs   # LRU: avatars, rendered segments, attachment previews
│   │   │   │   ├── ordering.rs
│   │   │   │   └── theme.rs   # parse, validate, apply  (parsing only — no file I/O)
│   │   │   ├── ui/            # presentation only. No business logic.
│   │   │   │   ├── mod.rs
│   │   │   │   ├── components/
│   │   │   │   ├── views/     # sidebar, chat view, thread panel, input bar
│   │   │   │   └── theme/     # theme application + hot-reload wiring
│   │   │   ├── state/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── app_state.rs    # unread counts, DeliveryState, selection
│   │   │   │   ├── actions.rs      # ALL mutations go through here
│   │   │   │   └── bridge.rs       # the ONLY owner of cx.update_global / cx.update
│   │   │   ├── network/       # protocol only. No GPUI imports whatsoever.
│   │   │   │   ├── mod.rs
│   │   │   │   ├── rest.rs
│   │   │   │   ├── websocket.rs
│   │   │   │   ├── auth.rs
│   │   │   │   └── reconnect.rs
│   │   │   ├── db/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── schema.rs
│   │   │   │   └── repository.rs
│   │   │   └── platform/
│   │   │       ├── mod.rs
│   │   │       ├── notifications.rs
│   │   │       ├── sounds.rs
│   │   │       ├── keychain.rs
│   │   │       └── file_watch.rs  # theme hot reload — I/O lives here, never in core/
│   │   └── tests/             # FLAT: Cargo only auto-discovers tests/*.rs and tests/*/main.rs
│   │       ├── login_flow.rs
│   │       ├── auth_failure_flow.rs
│   │       ├── realtime_flow.rs
│   │       ├── reconnect_flow.rs
│   │       ├── offline_flow.rs
│   │       ├── optimistic_send_flow.rs
│   │       ├── channel_switch_flow.rs
│   │       ├── typing_indicator_flow.rs
│   │       ├── history_pagination_flow.rs
│   │       ├── error_flow.rs
│   │       └── fixtures/      # AGENTS.md §8.2
│   ├── sh_nexus_wire/         # wire DTOs — single source of truth for the protocol
│   └── sh_nexus_server/       # Axum backend
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── auth.rs
│           ├── routes/{auth,channels,messages}.rs
│           ├── ws.rs
│           └── db.rs
├── assets/
│   ├── sounds/notification.wav
│   └── themes/                 # 3 built-in themes (dark, light, high-contrast)
├── docs/
│   ├── ARCHITECTURE.md         # ADRs, per AGENTS.md §9.2
│   ├── API.md                  # protocol + version negotiation
│   └── DEPENDENCIES.md         # the §7.2 audit, per crate
├── odd/tasks/                  # in-flight work units
├── CHANGELOG.md                # AGENTS.md §11
├── README.md
├── AGENTS.md                   # the constitution
└── PLAN.md
```

> **Test layout is not cosmetic.** Cargo auto-discovers `tests/*.rs` and `tests/*/main.rs` only.
> Files written as `tests/integration/login_flow.rs` are **silently never compiled or run** — a
> green build with all ten mandatory integration flows missing. The ten files above are flat and
> named for the flows in `AGENTS.md` §8.1.

### Layer rules (AGENTS.md §3.2)

- `core/` — pure and testable, no side effects. **Must not import `gpui`, `tokio`, or any
  UI/platform code, and must not touch the filesystem.** This is what makes the ≥90% coverage
  floor achievable. Theme *file watching* therefore lives in `platform/file_watch.rs`, and
  `core/theme.rs` only parses and validates bytes it is handed.
- `ui/` — presentation only. Reads state, renders, dispatches actions. No business logic.
- `state/` — `gpui::Global` for app-wide state. **All mutations go through `actions.rs`** so they
  stay auditable and testable. Unread counts and `DeliveryState` are client state and live here,
  not in `core/models/`.
- `network/` — parses wire formats into `core::models`, emits domain events. **Never imports
  GPUI.**
- `state/bridge.rs` — the single seam. `network/` emits plain domain events; `bridge.rs` is the
  only module that calls `cx.update_global` / `cx.update`, always on the main thread. This exists
  because `AGENTS.md` §7.3 forbids blocking `cx.update_global` from non-UI threads while §3.2
  forbids `network/` from importing GPUI. Without a named owner, the obvious implementation is
  the one that is prohibited.
- `db/` — persistence only. Parameterized queries. Versioned migrations, never destructive.
- `platform/` — OS-specific behind traits. Tokens to the OS keychain, never plaintext.

### GPUI-specific rules (AGENTS.md §7.3)

- **Always set `.text_color()` on text elements.** GPUI does not inherit color from parents.
  Every text element in `ui/` sets it explicitly.
- **Use `cx.spawn()` for async operations.** Never block the UI thread.
- **Use `cx.set_global()` / `cx.global::<T>()`** for app-wide state.
- **Virtualized message lists** — see §8 Phase 2 for the estimator question.
- **No DOM APIs.** No `document`, no `window`, no `fetch`.
- **No blocking `cx.update_global` from non-UI threads** — see `state/bridge.rs` above.

---

## 5. Data Models

Revised to satisfy AGENTS.md §2.1 (strict types), §2.3 (no deep clones in hot paths) and §4.2
(serde round-trip coverage).

### The wire/domain boundary

`AGENTS.md` §3.2 says `network/` "parses wire formats into `core::models`" — which presupposes
**two** layers, not one. Rev 2 conflated them. The decision:

- **`sh_nexus_wire` owns the wire DTOs.** They are `serde`-only, carry the `v` envelope, and are
  what actually crosses the socket. Both client and server compile against them, so they cannot
  drift.
- **`core/models` owns domain types.** They are what the UI renders and what `state/` mutates.
  They know nothing about serialization.
- **Explicit `TryFrom<wire::…>` conversions** at the `network/` boundary, with round-trip tests
  in both directions per §4.2. A malformed wire payload fails the conversion and never reaches
  `state/`.

```rust
use chrono::{DateTime, Utc};
use smallvec::SmallVec;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Online,
    Away,
    Offline,
}

/// Client state, not domain state. Lives in `state/app_state.rs` per §3.2,
/// and is an explicit §4.2 test subject for `app_state.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    Pending,
    Acked,
    Failed,
}

pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub status: UserStatus,
}

pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_private: bool,
    /// Shared immutable slice, so cloning a `Channel` is O(1) and allocates nothing.
    /// The sidebar iterates the *cached* `Vec<Arc<Channel>>` in `AppState`; it does
    /// not re-read or re-clone the member list per frame.
    pub members: Arc<[String]>,
    pub last_message_at: Option<DateTime<Utc>>,
    // NOTE: `unread_count` is client state and lives in `state/app_state.rs`, not here.
}

pub struct Message {
    pub id: String,
    /// Client-generated UUID, per §7.4. Survives optimistic send, ACK reconciliation
    /// and reconnect, so the server can dedupe a replayed send.
    pub client_msg_id: Uuid,
    pub channel_id: String,
    pub user_id: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    /// §2.3 names reactions on a message as the canonical `SmallVec` case.
    pub reactions: SmallVec<[Reaction; 2]>,
    /// `Some(parent_id)` marks this message a thread reply.
    pub thread_id: Option<String>,
    pub attachments: SmallVec<[Attachment; 1]>,
}

pub struct Reaction {
    pub emoji: String,
    pub user_ids: Vec<String>,
}

pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub url: String,
    pub mime_type: String,
    pub size: u64,
}
```

**Changes from Rev 1, and why:**

| Change | Reason |
|---|---|
| `DateTime` → `chrono::DateTime<Utc>` | §2.1 forbids raw strings; an untyped `DateTime` invites time-zone ambiguity. |
| `Vec<String>` → `Arc<[String]>` for members | §2.3 forbids deep clones in hot paths. |
| `reactions`/`attachments` → `SmallVec` | §2.3 names reactions on a message as the example case. |
| Added `client_msg_id: Uuid` | §7.4 requires a client-generated UUID on every message. Rev 1's protocol had no way to dedupe across reconnects. |
| Added `DeliveryState` and `unread_count` as client state | §8.1 Optimistic Send Flow and unread badges need them; §3.2 puts them in `state/`. |
| Wire DTOs split from domain types | §3.2's "parses wire formats into `core::models`" implies two layers. |
| Ids remain `String` | §2.2 explicitly permits `impl Into<String>` for id parameters. A newtype-id refactor is available later but is not mandated; Rev 2's claim to have "fixed bare String ids" was false and is withdrawn. |

---

## 6. Wire Protocol

Single source of truth: `crates/sh_nexus_wire`. Every frame carries a version. **An unknown major
version is rejected explicitly** — the server responds with an `error` frame and closes
(AGENTS.md §7.4). Timestamps serialize as `DateTime<Utc>` via chrono's serde impl, never as
hand-formatted strings.

```jsonc
// ---- Client -> Server ----
// §7.4: EVERY client frame carries a client-generated client_msg_id for idempotent
// dedup across reconnects. This includes ephemeral frames.
{ "v": 1, "type": "message.send",    "client_msg_id": "<uuid>", "channel_id": "...", "content": "..." }
{ "v": 1, "type": "reaction.add",    "client_msg_id": "<uuid>", "message_id": "...", "emoji": "👍" }
{ "v": 1, "type": "typing.start",    "client_msg_id": "<uuid>", "channel_id": "..." }
{ "v": 1, "type": "typing.stop",     "client_msg_id": "<uuid>", "channel_id": "..." }

// Resync is PER CHANNEL. A single global timestamp cannot reconstruct per-channel
// history: each channel has its own last_message_at and its own unread count.
{ "v": 1, "type": "resync",          "client_msg_id": "<uuid>", "channel_id": "...", "after": "2026-09-27T12:00:00Z" }

// ---- Server -> Client ----
{ "v": 1, "type": "message.ack",     "client_msg_id": "<uuid>", "message": { /* wire Message */ } }
{ "v": 1, "type": "message.new",     "message": { /* wire Message */ } }
{ "v": 1, "type": "message.error",   "client_msg_id": "<uuid>", "code": "...", "detail": "..." }
{ "v": 1, "type": "reaction.update", "message_id": "...", "emoji": "👍", "user_id": "..." }
{ "v": 1, "type": "typing.update",   "user_id": "...", "channel_id": "...", "active": true }
{ "v": 1, "type": "presence.update", "user_id": "...", "status": "online" }
{ "v": 1, "type": "error",           "code": "unsupported_version", "detail": "..." }
```

**What Rev 1 got wrong, and why each matters:**

- **No `v` field.** §7.4 requires versioned payloads; without it there is no negotiation.
- **No `client_msg_id`.** §7.4 requires it on *every* client message. Rev 2 asserted this and
  then omitted it from three of its own five frames.
- **No ACK or error envelope.** Rev 1 had nowhere to confirm or reject a send, making Optimistic
  Send Flow unimplementable.
- **No resume cursor.** §7.4 requires resumption from `last_message_at`. Rev 1 relied on live
  delivery during a gap, which loses messages.
- **Global resync cursor.** Rev 2 added a resync frame but scoped it globally, contradicting its
  own per-channel `last_message_at` model and making §8.1's Reconnect Flow ("no duplicates, no
  gaps") unsatisfiable.
- **Malformed JSON.** Rev 1 read `{ "type: "presence", ...` — unquoted key.

### REST Endpoints

```
POST   /auth/register
POST   /auth/login
GET    /channels
POST   /channels
GET    /channels/:id/messages?before=<cursor>&limit=<n>
POST   /channels/:id/messages      # requires an Idempotency-Key header
GET    /users/me
GET    /users/:id
```

`POST` retries are never blind (§7.4): the client sends an `Idempotency-Key` and the server
deduplicates on it.

---

## 7. Offline Model and Outbox

Rev 1 omitted this entirely, which made §8.1's Offline Flow untestable. The outbox is what
makes a chat app trustworthy.

- Sends made while disconnected are enqueued in the outbox with their `client_msg_id` and
  rendered in `DeliveryState::Pending`.
- On reconnect the client resyncs **per channel** from each channel's `last_message_at`, then
  flushes the outbox in enqueue order.
- The server deduplicates on `client_msg_id`, so a flush that partially succeeded before a
  dropped connection does not duplicate messages.
- A send that fails terminally transitions to `DeliveryState::Failed` and stays visible for
  retry. Failures are never silently dropped.

---

## 8. Feature Scope

### Phase 0 — Spike (hard gate, no estimate)

**Nothing else starts until this passes.** It exists because the highest-consequence risk in
this project is whether GPUI compiles standalone on Windows from a pinned git rev. Only
compiling answers that, and it is cheap.

- `git init` on a feature branch, `.gitignore`, workspace skeleton
- Client crate depending on `gpui` + `gpui_platform` at a pinned `rev`, with
  `gpui_platform/test-support` enabled
- A `main.rs` that opens a window and renders one styled element with explicit `.text_color()`
- **A headless render + simulated keyboard/pointer input** — this validates the `test-support`
  feature on which all of §10 depends
- `cargo build` green on Windows; `cargo clippy` clean
- **`cargo build --timings` recorded** as the §7.2 compile-time baseline

**Exit criteria:** a window opens on Windows from a clean checkout, and a headless test renders
and accepts simulated input. On failure, capture the exact error and revisit ADR-001 before
writing any application code.

### Phase 1 — Foundations

- `errors.rs` with the `ShNexusError` enum (§3.3)
- `sh_nexus_wire` — protocol types, version negotiation, `TryFrom` conversions
- `core/models/` — the types from §5
- `core/ordering.rs` — ordering, dedup, gap detection
- `core/markdown.rs` — Markdown to styled segment tree
- `core/cache.rs` — LRU with memory ceiling
- `core/theme.rs` — parse, validate, apply
- `state/app_state.rs`, `state/actions.rs`, `state/bridge.rs`
- `docs/DEPENDENCIES.md` — the §7.2 audit, one row per crate
- Measured baselines recorded for build time, binary size, idle RAM (§11)

Strict TDD applies to `core/` and `sh_nexus_wire` (pure, ≥90% floor, no excuses).

### Phase 2 — Core UI

- App shell: sidebar + chat area + input bar
- **Virtualized message list** — render only visible items (§7.3). **The row estimator:** §7.3
  mandates "a uniform row estimator and recycle". Markdown and code blocks make true row
  heights genuinely variable, and a uniform estimator in a chat list produces scroll jank
  against §1's zero-jank priority. **Plan: implement the §7.3 uniform estimator as the
  specified default and first-pass fallback, and layer measured per-row heights on top as a
  superset once a row has been rendered.** This complies rather than overrides, and an ADR
  request to amend §7.3 for measured-height chat lists is filed alongside it.
- Channel list (from cached `AppState`)
- Message bubbles: sender, timestamp, text — each with explicit `.text_color()`
- Input bar with send on Enter
- Theming wired to `core/theme.rs` + `platform/file_watch.rs` for hot reload
- Keyboard-only operation and focus order (input bar → send, Ctrl+K switcher, Escape closes
  panels), §5.2. Rev 1 omitted accessibility entirely. Mouse and trackpad operation verified
  per §11's UX checklist.

### Phase 3 — Persistence

- SQLite schema: `users`, `channels`, `messages`, `outbox`, `migrations`
- Versioned, non-destructive migrations
- Load history on startup, paged with cursors
- Save sent messages
- Channel switching preserving scroll position
- Outbox persistence (§7)

### Phase 4 — Networking

- `sh_nexus_wire` version negotiation against the server
- Axum backend: auth (JWT), channels, messages, WebSocket
- REST client; retry on 5xx/network, **never** on 4xx
- **Timeouts (§7.4): connect 5s, keepalive ping every 30s, REST request 10s**
- **All incoming payloads schema-validated before touching state** (§2.1)
- **TLS certificate validation never disabled in release** (§7.5)
- WebSocket client with ping/pong keepalive and graceful close
- **Reconnection with exponential backoff**: 1s → 2s → 4s … capped at 60s, with jitter, reset on
  success
- Per-channel resume from cursor; no duplicates, no gaps
- Typing indicators; online/offline presence
- Idempotent optimistic send with rollback

### Phase 5 — Rich features (cut to a chosen subset)

Do not attempt all nine. Pick a small number and finish them.

- [ ] Code blocks with syntax highlighting
- [ ] Markdown rendering in message bodies
- [ ] Message reactions
- [ ] Thread replies (side panel)
- [ ] Unread badges
- [ ] Search messages
- [ ] User presence display
- [ ] File attachment UI
- [ ] Emoji picker

**Recommendation for a portfolio:** reactions + threads + markdown/code blocks. Those three
produce the most visible demo surface per unit of work. Search and attachments carry the most
backend cost.

### Phase 6 — Polish

- [ ] Dark/light/high-contrast theme switching (see §9)
- [ ] Notification sounds (rodio)
- [ ] System notifications (notify-rust)
- [ ] Keyboard shortcuts
- [ ] Settings panel
- [ ] Message appear / typing animations — **gated on the frame budget** (§11). Animations
      compete directly with the §6.2 latency targets; not started until a baseline exists and
      the budget has headroom.

---

## 9. Theme System

AGENTS.md §10 specifies a full JSON theme system. Rev 1 reduced this to "dark/light toggle in
Phase 6", which does not satisfy §10. Corrected scope:

- JSON theme files with the schema in §10.1 of AGENTS.md
- **Hot reload** — changes apply without restart. **Mechanism:** the `notify` crate watches
  `~/.config/sh_nexus/themes/`; the watcher lives in `platform/file_watch.rs` because
  `core/theme.rs` must stay free of I/O per §3.2. `core/theme.rs` parses and validates the
  bytes it is handed.
- Validation with fallback to the default theme and an in-app error message
- Discovery of user themes in `~/.config/sh_nexus/themes/` (paths typed `PathBuf` per §2.1)
- At least 3 built-in themes compiled into the binary: dark, light, high-contrast
- Single shareable JSON file per theme
- **The schema is documented in `docs/API.md`** when modified, per §11's documentation checklist

---

## 10. Testing Strategy

Rev 1's stated mitigation — *"Manual testing + screenshot comparison; GPUI test support is
limited"* — is **factually wrong** and was weaker than the constitution it was meant to serve.
GPUI ships a real test harness: the `gpui::test` macro, `TestAppContext`, and facilities for
simulating platform input. Components render headlessly and accept simulated pointer and
keyboard input, with assertions on state, focus, layout, and accessibility.

**That harness is feature-gated behind `gpui_platform/test-support`.** The Phase 0 spike
validates it exists and works on Windows before any of this strategy depends on it.

### Coverage floors

`AGENTS.md` §4.1 sets floors keyed to client directories; §6.1 sets a workspace total. Both
apply.

| Area | Minimum | Target |
|---|---|---|
| `core/` | 90% | 95% |
| `network/` | 80% | — |
| `state/` | 80% | — |
| `db/` | 85% | — |
| Utilities | 85% | — |
| `sh_nexus_wire/` | 80% | — |
| `sh_nexus_server/` | 80% | — |
| **Workspace total** | **75%** | **85%** |
| New code, any task | 80% | — |

> The constitution contradicts itself: §4.1 requires `core/` ≥90% while §6.1 lists 85% min / 95%
> target. This plan takes the stricter 90% floor and records the conflict for amendment.
> `sh_nexus_wire` and `sh_nexus_server` floors are an extension of §4.1's intent — the wire
> crate is where §4.2's "serde round-trips for every wire format" actually lives, so leaving
> it without a floor would exempt the most protocol-critical code in the project.

### Mandatory test targets (AGENTS.md §4.2)

Serde round-trips and malformed-payload rejection for every wire format, in **both** directions
across the `TryFrom` boundary · ordering with out-of-order delivery, duplicate suppression, gap
detection, pagination cursors · markdown bold/italic/code/links, nested formatting, never
panics, injection safety · cache insertion, LRU eviction, memory ceiling, hit/miss ratio,
thread safety · backoff schedule (1s → 2s → 4s … capped 60s), jitter, reset on success, max
attempts · WebSocket connect, send, receive, **connect 5s timeout**, **30s keepalive ping**,
graceful close, malformed frames · REST request construction, auth header injection, status
mapping, **10s request timeout**, retry on 5xx but not 4xx · token expiry, refresh, logout
clearing all credentials · schema migrations, insert/query, channel switching, foreign key
integrity, concurrent read/write · theme parsing, schema validation, **color format
validation**, fallback · state action mutations, unread counts, optimistic send and rollback.

### Technique

- **Hermetic network tests via a `Transport` trait**, so `websocket.rs` and `rest.rs` are
  testable without sockets. `wiremock` for HTTP; a tokio channel pair for WS.
- **Fake clocks** for backoff, typing timeout, and token expiry. Never `sleep()` to wait for
  logic in a unit test.
- **Property-based tests (proptest):** any shuffle of a message batch produces an identical
  ordered result with no duplicates; markdown parsing never panics for arbitrary input.
- **Parameterized tests** with `rstest` or `test-case`.
- **Descriptive test names** (§4.3): `receiving_duplicate_message_is_idempotent()`, not
  `test_message()`.
- `tempfile` and a unique DB path per SQLite test. Every test independent; no reliance on
  execution order.
- **UI components:** render headlessly, drive pointer and keyboard input, assert state, focus
  and layout.

### Test fixtures (AGENTS.md §8.2)

`tests/fixtures/` must contain:

1. Sample theme JSON — **one valid, one invalid**
2. A recorded WebSocket session as **JSONL of envelopes**
3. A populated SQLite DB **per schema version**
4. Message payloads for **every protocol version**
5. Large generated message histories — **via proptest generators, not checked-in files**

### Mandatory integration flows (AGENTS.md §8.1)

Login · auth failure · real-time two-client delivery (<100ms, correct order) · reconnect with
backoff and no duplicates/gaps · **offline with queued sends delivered in order** · optimistic
send with ACK and rollback · channel switch with scroll preserved · typing indicator appear and
clear · history pagination with no boundary duplicates · server 500 and abnormal WS close
degrading gracefully.

One flat file per flow, in `crates/sh_nexus/tests/`, named for the flow — see the warning in §4
about Cargo's test discovery.

---

## 11. Performance Targets — Measurement Required

`AGENTS.md` §2.3 says *"Profile before optimizing. Measure, don't guess."* This section requests
measurement. It does **not** suspend any threshold.

> **This plan does not relax `AGENTS.md` §6.1/§6.2.** Rev 2 wrote "the figures in AGENTS.md are
> targets, not gates". That was a subordinate document voiding merge-blocking thresholds, and it
> was wrong on two counts: only the constitution may relax them, and doing so selectively —
> keeping the metrics one likes while suspending the two one does not — is renegotiation, not
> reconciliation. Corrected.

### Unverified hypotheses, stated as such

Two figures in the constitution look unlikely to hold. **These are estimates, not findings.
None has been built or measured.**

| Target | Hypothesis | Suspected driver |
|---|---|---|
| Binary size <30MB (§6.1) | Expected to land materially higher, plausibly in the tens of megabytes | GPUI statically links its renderer; plus reqwest with TLS, bundled SQLite, syntect's bundled Sublime syntax definitions, rodio, notify-rust |
| Idle RAM <80MB (§6.2) | Unverified; may be well under | GPU-side frame graph and texture caches; GPU memory pressure on discrete GPUs is a known class of problem on Windows |

Note that `AGENTS.md` §6.1 **already anticipates the size objection**: "GPUI statically links the
renderer. Binary size is larger than typical Rust apps but smaller than Electron. Validate
against release builds." The constitution authorized this measurement itself.

### Required action

1. **Phase 0 / Phase 1 exit criterion:** record measured values from a real release build on the
   target machine — binary size, idle RSS, cold start, dev and release build times — and capture
   `cargo build --timings`.
2. **A separate ADR** proposing the §6.1/§6.2 amendment, with the measured numbers, is written
   to `docs/ARCHITECTURE.md`.
3. **The amendment is applied to `AGENTS.md` by whoever owns the constitution**, and only then
   does any threshold change. Until that happens, the current figures stand as written.
4. If the measurements support the existing numbers, the ADR closes with no change and the plan
   is stronger for having checked.

### Targets that stand unchanged and are enforceable today

Sub-16ms render after WS receive · sub-16ms channel switch cached · sub-100ms channel switch
cold · sub-8ms scroll frame time with virtualization · sub-16ms send latency · sub-100ms
end-to-end delivery · sub-5s reconnect · sub-5ms paged SQLite query · no blocking I/O on the UI
thread · sub-2% idle CPU.

These are measurable from day one and carry no baseline caveat. §6.3's rules — >10% regression
blocks merge, any coverage drop blocks merge, any new clippy warning blocks merge — apply once
§11's measured baseline exists.

---

## 12. Repository Setup and Process

- `git init` **on a feature branch**, never directly on the default branch. Rev 1 had no
  repository at all, which makes the §5.1 checklist and work-unit cadence unenforceable.
- **Every work unit closes with the `AGENTS.md` §5.1 pre-commit checklist:**
  `cargo check` · `cargo clippy -- -D warnings` · `cargo fmt --check` · `cargo test` ·
  `cargo build --release` · doc comments on public items · no unjustified `unsafe` · no
  `unwrap`/`expect` in production paths · no blocking UI-thread operations · coverage of new
  code ≥80% · no new TODOs or FIXMEs. The §12 command set is the canonical form.
- **Work-unit commits**, Conventional Commits, tests and docs alongside behavior.
- **Second-agent review is mandatory** for any change to `core/`, `network/`, or `db/`
  (`AGENTS.md` §5.3). Manual QA results from §5.2 are recorded in the PR.
- **`CHANGELOG.md` maintained** (§11).
- **`docs/ARCHITECTURE.md`** holds ADRs in Context / Decision / Consequences / Alternatives
  format (§9.2). ADR-001 (GPUI distribution), ADR-002 (backend in Rust), ADR-003 (workspace
  layout) and ADR-004 (this reconciliation) are the first four; ADR-005 (perf re-baseline) and
  ADR-006 (row estimator) are pending Phase 0/1.
- **`docs/API.md`** documents the protocol and version negotiation, updated on any WebSocket
  change (§5.3), and records the theme schema when it changes (§11).
- **`docs/DEPENDENCIES.md`** holds the §7.2 audit.
- **CI** (`.github/workflows/ci.yml`) runs the §5.1 checklist on every push and is where §6.1's
  automated metrics and §6.3's merge-blocking rules are enforced.

---

## 13. Development Sequence and Estimate Honesty

Rev 1 claimed "7-11 weeks", then "compressible to ~4-5 weeks". Neither is credible for a
full-stack native chat application built on a pre-1.0 framework whose documentation states the
best way to learn it is by reading Zed's source.

**Honest position:** the estimate is conditional on the Phase 0 spike and is re-baselined after
it. A forecast made before the riskiest unknown is resolved is a guess wearing a confidence
rating. No duration is asserted here for that reason.

```
Phase 0 (spike)  ──► GATE
Phase 1 (core)   ──► Phase 2 (UI)  ──► Phase 3 (persistence)  ──► Phase 4 (network)
                                                                   │
                                                                   ▼
                                              Phase 5 (rich subset) ──► Phase 6 (polish)
```

`core/` must be complete and tested before UI work, because the UI renders what `core/`
produces and business logic is forbidden in `ui/`. Backend work cannot start until
`sh_nexus_wire` is fixed, since both sides compile against it.

**Scope control:** Rev 1's own risk table said "Strict MVP first". The recommended portfolio cut
is Phases 0-4 plus three Phase 5 features. A working real-time chat with persistence, optimistic
send, reconnection and an outbox is a far stronger portfolio piece than nine half-finished
features.

---

## 14. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| **GPUI does not build standalone on Windows** | Phase 0 is a hard gate. On failure, **revise ADR-001 first** — the fallback to `gpui-kit` / `gpui-unofficial` is not an in-flight substitution. The rest of the architecture is unaffected; GPUI is confined to `ui/` and `app.rs`. |
| **GPUI git pin breaks on upstream churn** | Pin the `rev`. Every sync is a deliberate upgrade task with a breaking-change review. |
| `core/` accidentally depends on `gpui`, `tokio` or the filesystem | Module-visibility boundary plus a test asserting dependency direction. The coverage floors are unreachable if the layer is impure. |
| Wire protocol drift | Impossible by construction: client and server share `sh_nexus_wire`. |
| `tests/integration/` layout silently voids the integration suite | Tests are flat and Cargo-discoverable; the §4 warning explains why. |
| No in-memory growth without bound | Bounded message cache (`core/cache.rs`), bounded typing-user set, bounded log retention (§7.1). |
| Perf targets unmeasured | §11 — measure in Phase 0/1, then amend `AGENTS.md` through the ADR path. No threshold is pre-suspended. |
| Scope creep | Phases 0-4 plus a named Phase 5 subset. Adding an item is an explicit decision, not drift. |
| UI testability | GPUI has a real headless harness behind `test-support`; validated in Phase 0. |

---

## 15. Next Steps

1. **Phase 0 spike** — `git init` on a feature branch, workspace, pinned `gpui` +
   `gpui_platform` with `test-support`, window opens on Windows, headless render + simulated
   input, `cargo build --timings` recorded. **Hard gate.**
2. Record measured baselines; open ADR-005 to amend `AGENTS.md` §6.1/§6.2 with real numbers.
3. Open ADR-006 for the §7.3 row-estimator amendment.
4. Write ADR-001 through ADR-004 into `docs/ARCHITECTURE.md`.
5. Phase 1 — `errors.rs`, `sh_nexus_wire`, `core/`, `state/`, under strict TDD.
6. Phase 2 — app shell and the virtualized message list.
7. Phase 3 — SQLite, migrations, outbox.
8. Phase 4 — Axum backend, WebSocket, reconnection, timeouts.
9. Choose the Phase 5 subset explicitly. Do not let it default to all nine.

---

## Appendix A — Defects found in `AGENTS.md` itself

Not this plan's to fix, but they will surface during implementation and belong in the
amendment queue:

1. **§4.1 vs §6.1 internal conflict.** §4.1 requires `core/` ≥90%; §6.1 lists 85% min / 95%
   target. This plan takes 90% and records the conflict.
2. **§1 platform description is incomplete.** It attributes Windows rendering to "DirectX" and
   does not mention that windowing uses Win32 and text uses DirectWrite. The DirectX 11 claim is
   correct for rendering; the omission is what caused the confusion in ADR-001's context.
3. **§12 `ls -lh` is Unix-only** in a project whose §5.2 requires Windows, macOS and Linux
   verification. Needs a cross-platform equivalent.
4. **§4.1's coverage floors are keyed to client directory names** (`core/`, `network/`,
   `state/`, `db/`) and so do not obviously cover `sh_nexus_wire` or `sh_nexus_server`. This plan
   extends them (§10); the constitution should say so.
5. **§7.3's "uniform row estimator"** is likely wrong for chat lists with variable-height
   content. ADR-006 proposes the amendment; §8 Phase 2 complies in the meantime.
