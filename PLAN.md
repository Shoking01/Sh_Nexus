# Sh_Nexus â€” Native Team Chat App (Rust + GPUI)

> **Precedence rule:** `AGENTS.md` is the constitution of this project. This document is
> subordinate to it. Where the two ever disagree, `AGENTS.md` wins and this file is the bug.
>
> A subordinate document may **request** an amendment to the constitution through the ADR path
> (`AGENTS.md` Â§9.2). It may **not** pre-apply one. Rev 2 broke that rule and was corrected.
>
> **Revision history**
> - Rev 1 â€” original. Contradicted `AGENTS.md` in eight places.
> - Rev 2 â€” reconciled to `AGENTS.md`. Independent conformance audit found 4 blockers,
>   13 major and 21 minor residual defects, including two self-contradictions and a test
>   layout that would have silently voided all ten mandatory integration flows.
> - Rev 3 â€” audit corrections applied. Tracked in `docs/ARCHITECTURE.md` as ADR-004.
- Rev 4 â€” Phase 0 spike passed (`491bd0f`). GPUI confirmed building and running on Windows from
  the pinned rev. The Rev 3 binary-size hypothesis was **refuted by measurement** and withdrawn
  (Â§11). Four operational findings recorded (Â§8).

## Goal

Build a native desktop team chat application (Slack-like) using Rust and GPUI, with a real
backend for real-time messaging. The project demonstrates: native UI, networking, concurrency,
persistence, and full-stack capability.

---

## 1. Architecture Decision: GPUI distribution channel

**Status:** Decided. This is the highest-consequence choice in the project and it gates all work.
Recorded as **ADR-001**.

### Context

The published `gpui` crate on crates.io is version `0.2.2`, last published 2025-10-22 â€” roughly
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

- `gpui` â€” the platform-agnostic core
- `gpui_platform` â€” the dispatcher that selects the OS backend

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
  `cargo build --timings` output as the baseline required by `AGENTS.md` Â§7.2.

### Alternatives considered

| Option | Â§7.2 verdict | Why not chosen |
|---|---|---|
| `gpui = "0.2.2"` from crates.io | Passes maintenance bars | Stale by ~11 months; no Windows backend. Disqualifying. |
| `gpui-component` / `gpui-kit` (longbridge) | Passes all five criteria; 0.6.6 published 2026-09-21, ~82k downloads in 90 days | Actively maintained and cross-platform. Rejected because it is a third-party opinionated design system, and this project builds its own UI layer anyway. **Reconsider if pin upkeep becomes a burden.** |
| `gpui-unofficial` | **FAILS Â§7.2**: its Windows backend crate `gpui-windows-gpui-unofficial` has 1,583 all-time downloads â€” far under the 500/month bar the constitution sets. Maintained by one person and explicitly not by the Zed team. | Rejected on Â§7.2 grounds, not on preference. |
| `open-gpui-platform`, `gpui-ce` | Incomplete assessment | Available republishes; would need the same Â§7.2 audit before adoption. |
| egui / iced / Slint | Passes all five criteria | Mature Windows support on crates.io, but abandons the portfolio's "built on Zed's framework" premise. |

**Fallback:** if the Phase 0 spike fails, fall back to `gpui-kit` or `gpui-unofficial`. That
requires **revising ADR-001 first** â€” it is not an in-flight substitution. The rest of the
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
| Identifiers | uuid | `client_msg_id` per Â§7.4 |
| Time | chrono | `DateTime<Utc>` per Â§2.1 |
| Small collections | smallvec | `SmallVec` for reactions/attachments per Â§2.3 |
| Markdown | pulldown-cmark | CommonMark parsing into a styled segment tree |
| Syntax highlighting | syntect (`default-fancy`) | Code blocks in messages |
| **Logging** | **tracing + tracing-subscriber** | **Structured logging per Â§2.2/Â§7.5** |
| File watching | notify | Theme hot reload (Â§10.2) |
| Auth | jsonwebtoken | Server-side JWT issue/verify |
| Audio | rodio | Notification sounds |
| Notifications | notify-rust | System notifications |
| Backend | Rust + Axum, **workspace member** | WebSocket server, REST API, SQLite |

### Dependency governance (AGENTS.md Â§7.2)

The plan states *why* each non-obvious choice exists. The full five-criterion audit per crate â€”
version, license, last-commit date, downloads/month, `cargo build --timings` delta, and the
justification comment â€” lives in **`docs/DEPENDENCIES.md`**, and is a **Phase 1 exit criterion**.
A dependency is not added until that row exists.

Decisions recorded here:

- **`tracing` + `tracing-subscriber` are mandatory, not optional.** Â§7.1 prohibits `println!` in
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

Recorded because `AGENTS.md` Â§7.2 requires checking compile-time impact, and because one of
these gates the entire test strategy:

| Crate | macOS | Linux | Windows |
|---|---|---|---|
| `gpui_platform` | `font-kit` | `wayland`, `x11` | **no features required** |
| `gpui` | â€” | â€” | â€” |

**`test-support` is required.** `gpui_platform`'s `test-support` feature enables
`gpui/test-support`, `gpui_macos/test-support` and `gpui_windows/test-support`. Without it the
headless test harness in Â§10 does not exist. Enabling it is part of the Phase 0 spike.

---

## 3. Code Philosophy and Observability

Restating `AGENTS.md` Â§2.1, Â§2.2, Â§7.1 and Â§7.5 as binding implementation rules. Rev 2 omitted
this section entirely, which left Â§7.1's `println!` ban with no sanctioned alternative and left
the highest-risk safety rules unstated.

### Safety (non-negotiable)

- **No `unsafe` without a `// SAFETY:` comment** explaining why the block is sound. Prefer safe
  abstractions. This applies most heavily to `platform/` (keychain FFI, `rodio`, `notify-rust`)
  and the virtualized list â€” exactly where `unsafe` creeps in.
- **No `unwrap()` or `expect()` in production paths.** Use `?`, `ok_or()`, `expect()` with
  context, or `match`. Permitted in tests or with a proven invariant.
- **No panics in user-facing code.** Every fallible operation returns `Result<T, ShNexusError>`.
  Network failures surface as recoverable states â€” a reconnecting banner, offline mode â€” never
  as crashes or silent drops.
- **All incoming WebSocket and REST payloads are validated against schemas before touching
  state.** No unvalidated payload reaches `state/`.
- **No plaintext storage of tokens.** Use the OS keychain via the `platform/` abstraction.

### Types

- **No `String` for paths.** Use `PathBuf` / `Path` â€” relevant to theme discovery under
  `~/.config/sh_nexus/themes/` and the SQLite file path.
- **All timestamps are `chrono::DateTime<Utc>`.** Never a raw string. Serialize via chrono's
  serde impl; do not hand-format RFC-3339.
- Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`, `impl Into<String>` for id parameters.
- Use `SmallVec` / `ArrayVec` for small collections â€” reactions and attachments on a message are
  the canonical case.

### Logging (AGENTS.md Â§7.5)

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
- Log retention is bounded (Â§7.1: no unbounded in-memory growth).

### Documentation

All public items carry `///` doc comments explaining the *why*, following the `AGENTS.md` Â§9.1
template (`# Arguments`, `# Returns`, `# Errors`, `# Example`).

### TLS

**TLS certificate validation is never disabled in release builds.** No `danger_accept_invalid_certs`
outside tests, and no code path that can enable it at runtime.

---

## 4. Project Structure

`AGENTS.md` Â§3.1's tree, applied **per crate** (recorded as **ADR-003**), inside a Cargo
workspace so the client, the wire crate and the server share one toolchain, one `cargo test`,
and one CI matrix.

```
Sh_Nexus/
â”œâ”€â”€ Cargo.toml                 # workspace root
â”œâ”€â”€ Cargo.lock
â”œâ”€â”€ .gitignore
â”œâ”€â”€ .github/
â”‚   â””â”€â”€ workflows/
â”‚       â””â”€â”€ ci.yml             # AGENTS.md Â§6.1 "Automated Metrics (CI)"
â”œâ”€â”€ crates/
â”‚   â”œâ”€â”€ sh_nexus/              # the client binary
â”‚   â”‚   â”œâ”€â”€ Cargo.toml
â”‚   â”‚   â”œâ”€â”€ src/
â”‚   â”‚   â”‚   â”œâ”€â”€ main.rs        # entry point only; initializes app, opens window. Max ~50 lines
â”‚   â”‚   â”‚   â”œâ”€â”€ app.rs         # root component, global state, theme provider, key handling
â”‚   â”‚   â”‚   â”œâ”€â”€ errors.rs      # ShNexusError (AGENTS.md Â§3.3)
â”‚   â”‚   â”‚   â”œâ”€â”€ core/          # PURE logic. No gpui, no tokio, no I/O, no filesystem.
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ mod.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ models/    # user.rs, channel.rs, message.rs, events.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ markdown.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ cache.rs   # LRU: avatars, rendered segments, attachment previews
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ ordering.rs
â”‚   â”‚   â”‚   â”‚   â””â”€â”€ theme.rs   # parse, validate, apply  (parsing only â€” no file I/O)
â”‚   â”‚   â”‚   â”œâ”€â”€ ui/            # presentation only. No business logic.
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ mod.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ components/
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ views/     # sidebar, chat view, thread panel, input bar
â”‚   â”‚   â”‚   â”‚   â””â”€â”€ theme/     # theme application + hot-reload wiring
â”‚   â”‚   â”‚   â”œâ”€â”€ state/
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ mod.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ app_state.rs    # unread counts, DeliveryState, selection
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ actions.rs      # ALL mutations go through here
â”‚   â”‚   â”‚   â”‚   â””â”€â”€ bridge.rs       # the ONLY owner of cx.update_global / cx.update
â”‚   â”‚   â”‚   â”œâ”€â”€ network/       # protocol only. No GPUI imports whatsoever.
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ mod.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ rest.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ websocket.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ auth.rs
â”‚   â”‚   â”‚   â”‚   â””â”€â”€ reconnect.rs
â”‚   â”‚   â”‚   â”œâ”€â”€ db/
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ mod.rs
â”‚   â”‚   â”‚   â”‚   â”œâ”€â”€ schema.rs
â”‚   â”‚   â”‚   â”‚   â””â”€â”€ repository.rs
â”‚   â”‚   â”‚   â””â”€â”€ platform/
â”‚   â”‚   â”‚       â”œâ”€â”€ mod.rs
â”‚   â”‚   â”‚       â”œâ”€â”€ notifications.rs
â”‚   â”‚   â”‚       â”œâ”€â”€ sounds.rs
â”‚   â”‚   â”‚       â”œâ”€â”€ keychain.rs
â”‚   â”‚   â”‚       â””â”€â”€ file_watch.rs  # theme hot reload â€” I/O lives here, never in core/
â”‚   â”‚   â””â”€â”€ tests/             # FLAT: Cargo only auto-discovers tests/*.rs and tests/*/main.rs
â”‚   â”‚       â”œâ”€â”€ login_flow.rs
â”‚   â”‚       â”œâ”€â”€ auth_failure_flow.rs
â”‚   â”‚       â”œâ”€â”€ realtime_flow.rs
â”‚   â”‚       â”œâ”€â”€ reconnect_flow.rs
â”‚   â”‚       â”œâ”€â”€ offline_flow.rs
â”‚   â”‚       â”œâ”€â”€ optimistic_send_flow.rs
â”‚   â”‚       â”œâ”€â”€ channel_switch_flow.rs
â”‚   â”‚       â”œâ”€â”€ typing_indicator_flow.rs
â”‚   â”‚       â”œâ”€â”€ history_pagination_flow.rs
â”‚   â”‚       â”œâ”€â”€ error_flow.rs
â”‚   â”‚       â””â”€â”€ fixtures/      # AGENTS.md Â§8.2
â”‚   â”œâ”€â”€ sh_nexus_wire/         # wire DTOs â€” single source of truth for the protocol
â”‚   â””â”€â”€ sh_nexus_server/       # Axum backend
â”‚       â”œâ”€â”€ Cargo.toml
â”‚       â””â”€â”€ src/
â”‚           â”œâ”€â”€ main.rs
â”‚           â”œâ”€â”€ auth.rs
â”‚           â”œâ”€â”€ routes/{auth,channels,messages}.rs
â”‚           â”œâ”€â”€ ws.rs
â”‚           â””â”€â”€ db.rs
â”œâ”€â”€ assets/
â”‚   â”œâ”€â”€ sounds/notification.wav
â”‚   â””â”€â”€ themes/                 # 3 built-in themes (dark, light, high-contrast)
â”œâ”€â”€ docs/
â”‚   â”œâ”€â”€ ARCHITECTURE.md         # ADRs, per AGENTS.md Â§9.2
â”‚   â”œâ”€â”€ API.md                  # protocol + version negotiation
â”‚   â””â”€â”€ DEPENDENCIES.md         # the Â§7.2 audit, per crate
â”œâ”€â”€ odd/tasks/                  # in-flight work units
â”œâ”€â”€ CHANGELOG.md                # AGENTS.md Â§11
â”œâ”€â”€ README.md
â”œâ”€â”€ AGENTS.md                   # the constitution
â””â”€â”€ PLAN.md
```

> **Test layout is not cosmetic.** Cargo auto-discovers `tests/*.rs` and `tests/*/main.rs` only.
> Files written as `tests/integration/login_flow.rs` are **silently never compiled or run** â€” a
> green build with all ten mandatory integration flows missing. The ten files above are flat and
> named for the flows in `AGENTS.md` Â§8.1.

### Layer rules (AGENTS.md Â§3.2)

- `core/` â€” pure and testable, no side effects. **Must not import `gpui`, `tokio`, or any
  UI/platform code, and must not touch the filesystem.** This is what makes the â‰¥90% coverage
  floor achievable. Theme *file watching* therefore lives in `platform/file_watch.rs`, and
  `core/theme.rs` only parses and validates bytes it is handed.
- `ui/` â€” presentation only. Reads state, renders, dispatches actions. No business logic.
- `state/` â€” `gpui::Global` for app-wide state. **All mutations go through `actions.rs`** so they
  stay auditable and testable. Unread counts and `DeliveryState` are client state and live here,
  not in `core/models/`.
- `network/` â€” parses wire formats into `core::models`, emits domain events. **Never imports
  GPUI.**
- `state/bridge.rs` â€” the single seam. `network/` emits plain domain events; `bridge.rs` is the
  only module that calls `cx.update_global` / `cx.update`, always on the main thread. This exists
  because `AGENTS.md` Â§7.3 forbids blocking `cx.update_global` from non-UI threads while Â§3.2
  forbids `network/` from importing GPUI. Without a named owner, the obvious implementation is
  the one that is prohibited.
- `db/` â€” persistence only. Parameterized queries. Versioned migrations, never destructive.
- `platform/` â€” OS-specific behind traits. Tokens to the OS keychain, never plaintext.

### GPUI-specific rules (AGENTS.md Â§7.3)

- **Always set `.text_color()` on text elements.** GPUI does not inherit color from parents.
  Every text element in `ui/` sets it explicitly.
- **Use `cx.spawn()` for async operations.** Never block the UI thread.
- **Use `cx.set_global()` / `cx.global::<T>()`** for app-wide state.
- **Virtualized message lists** â€” see Â§8 Phase 2 for the estimator question.
- **No DOM APIs.** No `document`, no `window`, no `fetch`.
- **No blocking `cx.update_global` from non-UI threads** â€” see `state/bridge.rs` above.

---

## 5. Data Models

Revised to satisfy AGENTS.md Â§2.1 (strict types), Â§2.3 (no deep clones in hot paths) and Â§4.2
(serde round-trip coverage).

### The wire/domain boundary

`AGENTS.md` Â§3.2 says `network/` "parses wire formats into `core::models`" â€” which presupposes
**two** layers, not one. Rev 2 conflated them. The decision:

- **`sh_nexus_wire` owns the wire DTOs.** They are `serde`-only, carry the `v` envelope, and are
  what actually crosses the socket. Both client and server compile against them, so they cannot
  drift.
- **`core/models` owns domain types.** They are what the UI renders and what `state/` mutates.
  They know nothing about serialization.
- **Explicit `TryFrom<wire::â€¦>` conversions** at the `network/` boundary, with round-trip tests
  in both directions per Â§4.2. A malformed wire payload fails the conversion and never reaches
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

/// Client state, not domain state. Lives in `state/app_state.rs` per Â§3.2,
/// and is an explicit Â§4.2 test subject for `app_state.rs`.
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
    /// Client-generated UUID, per Â§7.4. Survives optimistic send, ACK reconciliation
    /// and reconnect, so the server can dedupe a replayed send.
    pub client_msg_id: Uuid,
    pub channel_id: String,
    pub user_id: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    /// Â§2.3 names reactions on a message as the canonical `SmallVec` case.
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
| `DateTime` â†’ `chrono::DateTime<Utc>` | Â§2.1 forbids raw strings; an untyped `DateTime` invites time-zone ambiguity. |
| `Vec<String>` â†’ `Arc<[String]>` for members | Â§2.3 forbids deep clones in hot paths. |
| `reactions`/`attachments` â†’ `SmallVec` | Â§2.3 names reactions on a message as the example case. |
| Added `client_msg_id: Uuid` | Â§7.4 requires a client-generated UUID on every message. Rev 1's protocol had no way to dedupe across reconnects. |
| Added `DeliveryState` and `unread_count` as client state | Â§8.1 Optimistic Send Flow and unread badges need them; Â§3.2 puts them in `state/`. |
| Wire DTOs split from domain types | Â§3.2's "parses wire formats into `core::models`" implies two layers. |
| Ids remain `String` | Â§2.2 explicitly permits `impl Into<String>` for id parameters. A newtype-id refactor is available later but is not mandated; Rev 2's claim to have "fixed bare String ids" was false and is withdrawn. |

---

## 6. Wire Protocol

Single source of truth: `crates/sh_nexus_wire`. Every frame carries a version. **An unknown major
version is rejected explicitly** â€” the server responds with an `error` frame and closes
(AGENTS.md Â§7.4). Timestamps serialize as `DateTime<Utc>` via chrono's serde impl, never as
hand-formatted strings.

```jsonc
// ---- Client -> Server ----
// Â§7.4: EVERY client frame carries a client-generated client_msg_id for idempotent
// dedup across reconnects. This includes ephemeral frames.
{ "v": 1, "type": "message.send",    "client_msg_id": "<uuid>", "channel_id": "...", "content": "..." }
{ "v": 1, "type": "reaction.add",    "client_msg_id": "<uuid>", "message_id": "...", "emoji": "ðŸ‘" }
{ "v": 1, "type": "typing.start",    "client_msg_id": "<uuid>", "channel_id": "..." }
{ "v": 1, "type": "typing.stop",     "client_msg_id": "<uuid>", "channel_id": "..." }

// Resync is PER CHANNEL. A single global timestamp cannot reconstruct per-channel
// history: each channel has its own last_message_at and its own unread count.
{ "v": 1, "type": "resync",          "client_msg_id": "<uuid>", "channel_id": "...", "after": "2026-09-27T12:00:00Z" }

// ---- Server -> Client ----
{ "v": 1, "type": "message.ack",     "client_msg_id": "<uuid>", "message": { /* wire Message */ } }
{ "v": 1, "type": "message.new",     "message": { /* wire Message */ } }
{ "v": 1, "type": "message.error",   "client_msg_id": "<uuid>", "code": "...", "detail": "..." }
{ "v": 1, "type": "reaction.update", "message_id": "...", "emoji": "ðŸ‘", "user_id": "..." }
{ "v": 1, "type": "typing.update",   "user_id": "...", "channel_id": "...", "active": true }
{ "v": 1, "type": "presence.update", "user_id": "...", "status": "online" }
{ "v": 1, "type": "error",           "code": "unsupported_version", "detail": "..." }
```

**What Rev 1 got wrong, and why each matters:**

- **No `v` field.** Â§7.4 requires versioned payloads; without it there is no negotiation.
- **No `client_msg_id`.** Â§7.4 requires it on *every* client message. Rev 2 asserted this and
  then omitted it from three of its own five frames.
- **No ACK or error envelope.** Rev 1 had nowhere to confirm or reject a send, making Optimistic
  Send Flow unimplementable.
- **No resume cursor.** Â§7.4 requires resumption from `last_message_at`. Rev 1 relied on live
  delivery during a gap, which loses messages.
- **Global resync cursor.** Rev 2 added a resync frame but scoped it globally, contradicting its
  own per-channel `last_message_at` model and making Â§8.1's Reconnect Flow ("no duplicates, no
  gaps") unsatisfiable.
- **Malformed JSON.** Rev 1 read `{ "type: "presence", ...` â€” unquoted key.

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

`POST` retries are never blind (Â§7.4): the client sends an `Idempotency-Key` and the server
deduplicates on it.

---

## 7. Offline Model and Outbox

Rev 1 omitted this entirely, which made Â§8.1's Offline Flow untestable. The outbox is what
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

### Phase 0 â€” Spike (hard gate, no estimate)

**Nothing else starts until this passes.** It exists because the highest-consequence risk in
this project is whether GPUI compiles standalone on Windows from a pinned git rev. Only
compiling answers that, and it is cheap.

- `git init` on a feature branch, `.gitignore`, workspace skeleton
- Client crate depending on `gpui` + `gpui_platform` at a pinned `rev`, with
  `gpui_platform/test-support` enabled
- A `main.rs` that opens a window and renders one styled element with explicit `.text_color()`
- **A headless render + simulated keyboard/pointer input** â€” this validates the `test-support`
  feature on which all of Â§10 depends
- `cargo build` green on Windows; `cargo clippy` clean
- **`cargo build --timings` recorded** as the Â§7.2 compile-time baseline

**Exit criteria:** a window opens on Windows from a clean checkout, and a headless test renders
and accepts simulated input. On failure, capture the exact error and revisit ADR-001 before
writing any application code.

#### Phase 0 results â€” PASSED

Commit `491bd0f` on `spike/gpui-windows`. Window handle non-zero; 4/4 headless tests pass
covering render, click and keystroke. `cargo build`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check`, `cargo test` and `cargo build --release` all green. ADR-001 holds; no
fallback needed.

Four findings from the spike that constrain later phases:

1. **`fxc.exe` is required for release builds only, and it is not on `PATH`.**
   `crates/gpui_windows/build.rs` compiles HLSL under `#[cfg(not(debug_assertions))]` and
   panics with "Failed to find fxc.exe" if it cannot locate it. A debug build needs no shader
   compiler, so `cargo build` succeeds on a machine where `cargo build --release` fails outright
   â€” a failure that reads like a code bug. Resolution order: `GPUI_FXC_PATH`, then `where.exe`,
   then the newest SDK via the registry key
   `HKLM\SOFTWARE\WOW6432Node\Microsoft\Microsoft SDKs\Windows\v10.0\InstallationFolder`.
   **This belongs in CI setup.** Document it in `README.md` and `docs/ARCHITECTURE.md`.

2. **The headless test harness needs `.debug_selector()`, not just `.id()`.** `cx.debug_bounds(id)`
   returns `None` unless the element also carries `.debug_selector(|| id.to_owned())` on the
   `InteractiveElement` trait. A test using only `.id()` compiles, runs, and passes while
   asserting nothing â€” a green build over a void, the same failure mode as the Â§4 test-layout
   trap in a different disguise. Recording is gated behind `#[cfg(any(test, feature =
   "test-support"))]`, so this cannot be caught by reading the type signatures.

3. **Visual (image-diffing) tests are macOS/Linux only.** `current_headless_renderer()` returns
   `Ok(None)` on Windows, because `TestAppContext` uses GPUI's own pure-Rust `TestPlatform` and
   never touches a GPU. Only `VisualTestAppContext` needs a real renderer. So the state, focus,
   layout and accessibility assertions PLAN.md Â§10 relies on all work on Windows, but
   **screenshot-comparison tests do not.** PLAN.md Â§10 is correct to specify headless render +
   simulated input rather than image diffing.

4. **`gpui/test-support` is not free.** It enables `leak-detection` (pulling `backtrace`) and
   `proptest`. Measured cost: **9.5s of release wall clock and ~155KB of binary** (190.5s â†’
   181.0s; 10,401,792 â†’ 10,246,656 B) â€” about 1.5% size and 5% build time. Moving
   `gpui_platform` to `[dev-dependencies]` reclaims it. Not worth deviating from Â§2 at that
   price; recorded so the decision is reversible with numbers behind it.

### Phase 1 â€” Foundations

- `errors.rs` with the `ShNexusError` enum (Â§3.3)
- `sh_nexus_wire` â€” protocol types, version negotiation, `TryFrom` conversions
- `core/models/` â€” the types from Â§5
- `core/ordering.rs` â€” ordering, dedup, gap detection
- `core/markdown.rs` â€” Markdown to styled segment tree
- `core/cache.rs` â€” LRU with memory ceiling
- `core/theme.rs` â€” parse, validate, apply
- `state/app_state.rs`, `state/actions.rs`, `state/bridge.rs`
- `docs/DEPENDENCIES.md` â€” the Â§7.2 audit, one row per crate
- Measured baselines recorded for build time, binary size, idle RAM (Â§11)

Strict TDD applies to `core/` and `sh_nexus_wire` (pure, â‰¥90% floor, no excuses).

### Phase 2 â€” Core UI

- App shell: sidebar + chat area + input bar
- **Virtualized message list** â€” render only visible items (Â§7.3). **The row estimator:** Â§7.3
  mandates "a uniform row estimator and recycle". Markdown and code blocks make true row
  heights genuinely variable, and a uniform estimator in a chat list produces scroll jank
  against Â§1's zero-jank priority. **Plan: implement the Â§7.3 uniform estimator as the
  specified default and first-pass fallback, and layer measured per-row heights on top as a
  superset once a row has been rendered.** This complies rather than overrides, and an ADR
  request to amend Â§7.3 for measured-height chat lists is filed alongside it.
- Channel list (from cached `AppState`)
- Message bubbles: sender, timestamp, text â€” each with explicit `.text_color()`
- Input bar with send on Enter
- Theming wired to `core/theme.rs` + `platform/file_watch.rs` for hot reload
- Keyboard-only operation and focus order (input bar â†’ send, Ctrl+K switcher, Escape closes
  panels), Â§5.2. Rev 1 omitted accessibility entirely. Mouse and trackpad operation verified
  per Â§11's UX checklist.

### Phase 3 â€” Persistence

- SQLite schema: `users`, `channels`, `messages`, `outbox`, `migrations`
- Versioned, non-destructive migrations
- Load history on startup, paged with cursors
- Save sent messages
- Channel switching preserving scroll position
- Outbox persistence (Â§7)

### Phase 4 â€” Networking

- `sh_nexus_wire` version negotiation against the server
- Axum backend: auth (JWT), channels, messages, WebSocket
- REST client; retry on 5xx/network, **never** on 4xx
- **Timeouts (Â§7.4): connect 5s, keepalive ping every 30s, REST request 10s**
- **All incoming payloads schema-validated before touching state** (Â§2.1)
- **TLS certificate validation never disabled in release** (Â§7.5)
- WebSocket client with ping/pong keepalive and graceful close
- **Reconnection with exponential backoff**: 1s â†’ 2s â†’ 4s â€¦ capped at 60s, with jitter, reset on
  success
- Per-channel resume from cursor; no duplicates, no gaps
- Typing indicators; online/offline presence
- Idempotent optimistic send with rollback

### Phase 5 â€” Rich features (cut to a chosen subset)

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

### Phase 6 â€” Polish

- [ ] Dark/light/high-contrast theme switching (see Â§9)
- [ ] Notification sounds (rodio)
- [ ] System notifications (notify-rust)
- [ ] Keyboard shortcuts
- [ ] Settings panel
- [ ] Message appear / typing animations â€” **gated on the frame budget** (Â§11). Animations
      compete directly with the Â§6.2 latency targets; not started until a baseline exists and
      the budget has headroom.

---

## 9. Theme System

AGENTS.md Â§10 specifies a full JSON theme system. Rev 1 reduced this to "dark/light toggle in
Phase 6", which does not satisfy Â§10. Corrected scope:

- JSON theme files with the schema in Â§10.1 of AGENTS.md
- **Hot reload** â€” changes apply without restart. **Mechanism:** the `notify` crate watches
  `~/.config/sh_nexus/themes/`; the watcher lives in `platform/file_watch.rs` because
  `core/theme.rs` must stay free of I/O per Â§3.2. `core/theme.rs` parses and validates the
  bytes it is handed.
- Validation with fallback to the default theme and an in-app error message
- Discovery of user themes in `~/.config/sh_nexus/themes/` (paths typed `PathBuf` per Â§2.1)
- At least 3 built-in themes compiled into the binary: dark, light, high-contrast
- Single shareable JSON file per theme
- **The schema is documented in `docs/API.md`** when modified, per Â§11's documentation checklist

---

## 10. Testing Strategy

Rev 1's stated mitigation â€” *"Manual testing + screenshot comparison; GPUI test support is
limited"* â€” is **factually wrong** and was weaker than the constitution it was meant to serve.
GPUI ships a real test harness: the `gpui::test` macro, `TestAppContext`, and facilities for
simulating platform input. Components render headlessly and accept simulated pointer and
keyboard input, with assertions on state, focus, layout, and accessibility.

**That harness is feature-gated behind `gpui_platform/test-support`.** The Phase 0 spike
validates it exists and works on Windows before any of this strategy depends on it.

### Coverage floors

`AGENTS.md` Â§4.1 sets floors keyed to client directories; Â§6.1 sets a workspace total. Both
apply.

| Area | Minimum | Target |
|---|---|---|
| `core/` | 90% | 95% |
| `network/` | 80% | â€” |
| `state/` | 80% | â€” |
| `db/` | 85% | â€” |
| Utilities | 85% | â€” |
| `sh_nexus_wire/` | 80% | â€” |
| `sh_nexus_server/` | 80% | â€” |
| **Workspace total** | **75%** | **85%** |
| New code, any task | 80% | â€” |

> The constitution contradicts itself: Â§4.1 requires `core/` â‰¥90% while Â§6.1 lists 85% min / 95%
> target. This plan takes the stricter 90% floor and records the conflict for amendment.
> `sh_nexus_wire` and `sh_nexus_server` floors are an extension of Â§4.1's intent â€” the wire
> crate is where Â§4.2's "serde round-trips for every wire format" actually lives, so leaving
> it without a floor would exempt the most protocol-critical code in the project.

### Mandatory test targets (AGENTS.md Â§4.2)

Serde round-trips and malformed-payload rejection for every wire format, in **both** directions
across the `TryFrom` boundary Â· ordering with out-of-order delivery, duplicate suppression, gap
detection, pagination cursors Â· markdown bold/italic/code/links, nested formatting, never
panics, injection safety Â· cache insertion, LRU eviction, memory ceiling, hit/miss ratio,
thread safety Â· backoff schedule (1s â†’ 2s â†’ 4s â€¦ capped 60s), jitter, reset on success, max
attempts Â· WebSocket connect, send, receive, **connect 5s timeout**, **30s keepalive ping**,
graceful close, malformed frames Â· REST request construction, auth header injection, status
mapping, **10s request timeout**, retry on 5xx but not 4xx Â· token expiry, refresh, logout
clearing all credentials Â· schema migrations, insert/query, channel switching, foreign key
integrity, concurrent read/write Â· theme parsing, schema validation, **color format
validation**, fallback Â· state action mutations, unread counts, optimistic send and rollback.

### Technique

- **Hermetic network tests via a `Transport` trait**, so `websocket.rs` and `rest.rs` are
  testable without sockets. `wiremock` for HTTP; a tokio channel pair for WS.
- **Fake clocks** for backoff, typing timeout, and token expiry. Never `sleep()` to wait for
  logic in a unit test.
- **Property-based tests (proptest):** any shuffle of a message batch produces an identical
  ordered result with no duplicates; markdown parsing never panics for arbitrary input.
- **Parameterized tests** with `rstest` or `test-case`.
- **Descriptive test names** (Â§4.3): `receiving_duplicate_message_is_idempotent()`, not
  `test_message()`.
- `tempfile` and a unique DB path per SQLite test. Every test independent; no reliance on
  execution order.
- **UI components:** render headlessly, drive pointer and keyboard input, assert state, focus
  and layout.

### Test fixtures (AGENTS.md Â§8.2)

`tests/fixtures/` must contain:

1. Sample theme JSON â€” **one valid, one invalid**
2. A recorded WebSocket session as **JSONL of envelopes**
3. A populated SQLite DB **per schema version**
4. Message payloads for **every protocol version**
5. Large generated message histories â€” **via proptest generators, not checked-in files**

### Mandatory integration flows (AGENTS.md Â§8.1)

Login Â· auth failure Â· real-time two-client delivery (<100ms, correct order) Â· reconnect with
backoff and no duplicates/gaps Â· **offline with queued sends delivered in order** Â· optimistic
send with ACK and rollback Â· channel switch with scroll preserved Â· typing indicator appear and
clear Â· history pagination with no boundary duplicates Â· server 500 and abnormal WS close
degrading gracefully.

One flat file per flow, in `crates/sh_nexus/tests/`, named for the flow â€” see the warning in Â§4
about Cargo's test discovery.

---

## 11. Performance Targets â€” Measurement Required

`AGENTS.md` Â§2.3 says *"Profile before optimizing. Measure, don't guess."* This section requests
measurement. It does **not** suspend any threshold.

> **This plan does not relax `AGENTS.md` Â§6.1/Â§6.2.** Rev 2 wrote "the figures in AGENTS.md are
> targets, not gates". That was a subordinate document voiding merge-blocking thresholds, and it
> was wrong on two counts: only the constitution may relax them, and doing so selectively â€”
> keeping the metrics one likes while suspending the two one does not â€” is renegotiation, not
> reconciliation. Corrected.

### Measured baseline â€” Phase 0

Recorded 2026-09-27 on Windows (12 logical CPUs, MSVC Build Tools 2022, Windows SDK
10.0.26100.0, rustc 1.98.1) from commit `491bd0f` on `spike/gpui-windows`.

| Metric | Measured | Â§6.1/Â§6.2 threshold | Verdict |
|---|---|---|---|
| Release binary | **9.92 MB** (10,401,792 B) | <20MB target, <30MB max | **PASSES both** |
| Idle RAM | 48.6 MB working set / 41.5 MB private | <80MB | **PASSES** |
| Idle CPU | 2.42% of one core (20s sample) | <2% | Misses â€” sample too short to count |
| Dev cold build | 2m 12s | <60s target, <120s max | Exceeds max |
| Release cold build | 3m 10s | <3min target | Just over |
| `Cargo.lock` packages | 675 | â€” | â€” |
| Compile units (release) | 424 | â€” | â€” |

**The binary-size hypothesis in Rev 2 was wrong, and this corrects it.** Rev 2 predicted "tens
of megabytes", called the 30MB ceiling unachievable, and pre-drafted a relief ADR. The measured
figure is **9.92 MB — 3× under the maximum and 2× under the target**. GPUI does *not* statically
link its renderer on Windows: `gpui_windows` is pure Rust over the `windows` crate with a
hand-written D3D11 renderer and DirectWrite text. No `blade`, no `shaderc`, no Vulkan.

> **The false premise originated in `AGENTS.md`, not in this plan.** `AGENTS.md` §6.1 carries the
> note *"Note: GPUI statically links the renderer. Binary size is larger than typical Rust apps
> but smaller than Electron."* That premise is what Rev 2 inherited and amplified into an
> allegedly-unachievable ceiling. Phase 0 measured it and it is false on Windows. See Appendix A
> item 6 — this needs the constitution owner's decision, so this plan does not edit §6.1.

**ADR-005 is withdrawn.** No amendment to Â§6.1's binary-size row is warranted. The remaining
open question is the *delta* from dependencies the spike did not link â€” syntect, reqwest with
TLS, bundled SQLite, rodio, notify-rust. That is unmeasured and gets measured in Phase 3, when
those dependencies actually land. Until then Â§6.1's numbers stand as written and are currently
being met with room to spare.

The build-time rows are the real miss: 2m12s dev against a <120s ceiling. That is a cold-build
figure on a 12-thread machine, and Â§6.1's CI rows are about a controlled environment. Re-measure
in CI before treating it as a violation.

**Idle CPU needs a longer sample.** 2.42% came from a 20-second window, which is too short to
distinguish a real regression from measurement noise. Re-measure over a 30-minute session per
Â§5.2 before recording any finding.

### Remaining measurement actions

1. **Phase 3 exit criterion:** re-measure binary size once syntect, reqwest, rusqlite, rodio and
   notify-rust are linked, to establish the dependency delta against the 9.92MB baseline.
2. **Phase 3 exit criterion:** idle CPU over a â‰¥30-minute session, per Â§5.2.
3. **CI exit criterion:** dev/release build times in a controlled environment, for Â§6.1's rows.

### Targets that stand unchanged and are enforceable today

Sub-16ms render after WS receive Â· sub-16ms channel switch cached Â· sub-100ms channel switch
cold Â· sub-8ms scroll frame time with virtualization Â· sub-16ms send latency Â· sub-100ms
end-to-end delivery Â· sub-5s reconnect Â· sub-5ms paged SQLite query Â· no blocking I/O on the UI
thread Â· sub-2% idle CPU.

These are measurable from day one and carry no baseline caveat. Â§6.3's rules â€” >10% regression
blocks merge, any coverage drop blocks merge, any new clippy warning blocks merge â€” apply once
Â§11's measured baseline exists.

---

## 12. Repository Setup and Process

- `git init` **on a feature branch**, never directly on the default branch. Rev 1 had no
  repository at all, which makes the Â§5.1 checklist and work-unit cadence unenforceable.
- **Every work unit closes with the `AGENTS.md` Â§5.1 pre-commit checklist:**
  `cargo check` Â· `cargo clippy -- -D warnings` Â· `cargo fmt --check` Â· `cargo test` Â·
  `cargo build --release` Â· doc comments on public items Â· no unjustified `unsafe` Â· no
  `unwrap`/`expect` in production paths Â· no blocking UI-thread operations Â· coverage of new
  code â‰¥80% Â· no new TODOs or FIXMEs. The Â§12 command set is the canonical form.
- **Work-unit commits**, Conventional Commits, tests and docs alongside behavior.
- **Second-agent review is mandatory** for any change to `core/`, `network/`, or `db/`
  (`AGENTS.md` Â§5.3). Manual QA results from Â§5.2 are recorded in the PR.
- **`CHANGELOG.md` maintained** (Â§11).
- **`docs/ARCHITECTURE.md`** holds ADRs in Context / Decision / Consequences / Alternatives
  format (Â§9.2). ADR-001 (GPUI distribution), ADR-002 (backend in Rust), ADR-003 (workspace
  layout) and ADR-004 (this reconciliation) are the first four; ADR-005 (perf re-baseline) and
  ADR-006 (row estimator) are pending Phase 0/1.
- **`docs/API.md`** documents the protocol and version negotiation, updated on any WebSocket
  change (Â§5.3), and records the theme schema when it changes (Â§11).
- **`docs/DEPENDENCIES.md`** holds the Â§7.2 audit.
- **CI** (`.github/workflows/ci.yml`) runs the Â§5.1 checklist on every push and is where Â§6.1's
  automated metrics and Â§6.3's merge-blocking rules are enforced.

---

## 13. Development Sequence and Estimate Honesty

Rev 1 claimed "7-11 weeks", then "compressible to ~4-5 weeks". Neither is credible for a
full-stack native chat application built on a pre-1.0 framework whose documentation states the
best way to learn it is by reading Zed's source.

**Honest position:** the estimate is conditional on the Phase 0 spike and is re-baselined after
it. A forecast made before the riskiest unknown is resolved is a guess wearing a confidence
rating. No duration is asserted here for that reason.

```
Phase 0 (spike)  â”€â”€â–º GATE
Phase 1 (core)   â”€â”€â–º Phase 2 (UI)  â”€â”€â–º Phase 3 (persistence)  â”€â”€â–º Phase 4 (network)
                                                                   â”‚
                                                                   â–¼
                                              Phase 5 (rich subset) â”€â”€â–º Phase 6 (polish)
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
| **Release builds need `fxc.exe`, which is not on `PATH`** | `gpui_windows/build.rs` panics on release builds without it while debug builds succeed â€” a failure that reads like a code bug. Resolution via `GPUI_FXC_PATH` or the SDK registry key. **Document in README and wire into CI setup** (Phase 0 finding 1). |
| **A headless test can pass while asserting nothing** | `.id()` alone does not register debug bounds; `.debug_selector()` is also required. Any test using `cx.debug_bounds` must assert `Some`, never tolerate `None`. Lint this in review (Phase 0 finding 2). |
| **Visual/screenshot tests unavailable on Windows** | `current_headless_renderer()` returns `Ok(None)` there. Headless render + simulated input + state/focus/layout assertions all work; image diffing does not. PLAN.md Â§10 already specifies the former (Phase 0 finding 3). |
| **GPUI does not build standalone on Windows** | **RESOLVED â€” Phase 0 passed.** `gpui` + `gpui_platform` at pinned `rev e683fd7b` compile and run on Windows. ADR-001 holds. |
| **GPUI git pin breaks on upstream churn** | Pin the `rev`. Every sync is a deliberate upgrade task with a breaking-change review. |
| `core/` accidentally depends on `gpui`, `tokio` or the filesystem | Module-visibility boundary plus a test asserting dependency direction. The coverage floors are unreachable if the layer is impure. |
| Wire protocol drift | Impossible by construction: client and server share `sh_nexus_wire`. |
| `tests/integration/` layout silently voids the integration suite | Tests are flat and Cargo-discoverable; the Â§4 warning explains why. |
| No in-memory growth without bound | Bounded message cache (`core/cache.rs`), bounded typing-user set, bounded log retention (Â§7.1). |
| Perf targets unmeasured | Â§11 â€” measure in Phase 0/1, then amend `AGENTS.md` through the ADR path. No threshold is pre-suspended. |
| Scope creep | Phases 0-4 plus a named Phase 5 subset. Adding an item is an explicit decision, not drift. |
| UI testability | GPUI has a real headless harness behind `test-support`; validated in Phase 0. |

---

## 15. Next Steps

1. ~~**Phase 0 spike**~~ - **DONE. PASSED.** Commit `491bd0f`, pushed to `main`. Gate cleared;
   ADR-001 holds. See section 8 for the four findings that constrain later phases.
2. ~~Document the `fxc.exe` release-build prerequisite~~ - **DONE.** `README.md` Prerequisites
   and Platform gotchas (a). CI wiring outstanding, see item 4.
3. ~~Write ADR-001 through ADR-004 into `docs/ARCHITECTURE.md`~~ - **DONE**, along with ADR-005
   (Withdrawn, retained as a record) and ADR-006 (Proposed). ADR-005 is withdrawn: the
   measured 9.92MB binary refuted the hypothesis (section 11).
4. **CI.** `.github/workflows/ci.yml` running the section 5.1 checklist is the one section 12
   commitment with nothing behind it. Every push so far has been self-verified by hand, and
   sections 6.1/6.3 merge-blocking rules are unenforceable until the pipeline exists. **Do
   this before Phase 1** - Phase 1 lands under strict TDD with a >=90% coverage floor on `core/`,
   and a self-reported floor is not a floor.
5. **Phase 1** - `errors.rs`, `sh_nexus_wire`, `core/`, `state/`, under strict TDD. Exit
   criterion: `docs/DEPENDENCIES.md` with the section 7.2 audit, one row per crate.
6. **Phase 2** - app shell and the virtualized message list.
7. **Phase 3** - SQLite, migrations, outbox. Exit criterion: re-measure binary size against the
   9.92MB baseline, and idle CPU over a >=30-minute session.
8. **Phase 4** - Axum backend, WebSocket, reconnection, timeouts.
9. Choose the Phase 5 subset explicitly. Do not let it default to all nine.

### Open items for the constitution owner

These need the owner's decision; this plan does not act on them unilaterally.

1. **`AGENTS.md` section 6.1's "GPUI statically links the renderer"** is false on Windows and is
   the origin of ADR-005. Measured replacement text is in `docs/ARCHITECTURE.md` under ADR-005.
2. **Project license.** `LICENSE` is MIT and `Cargo.toml` now says MIT, but `AGENTS.md` never
   states a license. Worth confirming MIT is intended and recording it in `AGENTS.md`.
3. **The other `AGENTS.md` defects** listed in Appendix A items 1-5.

---

## Appendix A â€” Defects found in `AGENTS.md` itself

Not this plan's to fix, but they will surface during implementation and belong in the
amendment queue:

1. **Â§4.1 vs Â§6.1 internal conflict.** Â§4.1 requires `core/` â‰¥90%; Â§6.1 lists 85% min / 95%
   target. This plan takes 90% and records the conflict.
2. **Â§1 platform description is incomplete.** It attributes Windows rendering to "DirectX" and
   does not mention that windowing uses Win32 and text uses DirectWrite. The DirectX 11 claim is
   correct for rendering; the omission is what caused the confusion in ADR-001's context.
3. **Â§12 `ls -lh` is Unix-only** in a project whose Â§5.2 requires Windows, macOS and Linux
   verification. Needs a cross-platform equivalent.
4. **Â§4.1's coverage floors are keyed to client directory names** (`core/`, `network/`,
   `state/`, `db/`) and so do not obviously cover `sh_nexus_wire` or `sh_nexus_server`. This plan
   extends them (Â§10); the constitution should say so.
5. **Â§7.3's "uniform row estimator"** is likely wrong for chat lists with variable-height
   content. ADR-006 proposes the amendment; Â§8 Phase 2 complies in the meantime.
6. **§6.1's note "GPUI statically links the renderer" is factually false on Windows.** Phase 0
   measured a 9.92 MB release binary — 3× under §6.1's own 30 MB maximum — and confirmed
   `gpui_windows` is pure Rust over the `windows` crate with a hand-written D3D11 renderer, with
   no `blade`, no `shaderc`, and no statically linked renderer. This false premise is the origin
   of ADR-005, which was raised and then refuted by measurement. **This plan does not edit
   `AGENTS.md`; that correction belongs to the constitution's owner.** The measured replacement
   text for the note is in `docs/ARCHITECTURE.md` under ADR-005.
7. **`LICENSE` (MIT) contradicted `Cargo.toml`'s `license = "Apache-2.0"`.** Found while
   documenting Phase 0. `Cargo.toml` now says MIT with a comment separating the two facts: GPUI
   is Apache-2.0, but that license attaches to the dependency, not to this project. Worth
   confirming MIT is the intended project license — `AGENTS.md` never states one.