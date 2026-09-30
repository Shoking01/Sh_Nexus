# Sh_Nexus — Architecture Decision Records

This file holds every Architecture Decision Record for Sh_Nexus, in the format mandated by
`AGENTS.md` §9.2: **Context, Decision, Consequences, Alternatives considered.**

## Precedence

`AGENTS.md` is the constitution of this project. `PLAN.md` and this file are subordinate to it. Where
any two disagree, `AGENTS.md` wins and the other document is the bug.

A subordinate document may **request** an amendment to the constitution through the ADR path
(`AGENTS.md` §9.2). It may **not** pre-apply one. `PLAN.md` Rev 2 broke that rule by declaring
`AGENTS.md`'s performance thresholds "targets, not gates", and was corrected — see ADR-004 and
ADR-005.

## Status values

| Status | Meaning |
|---|---|
| **Accepted** | In force. Implementation must comply. |
| **Proposed** | Written up and open for review. **Not** in force. Do not implement as settled. |
| **Withdrawn** | Raised, then refuted or reversed. **Retained as a record**, never silently deleted. A withdrawn ADR is a decision that was considered and why it lost. |

## Index

| ADR | Title | Status |
|---|---|---|
| [ADR-001](#adr-001--gpui-distribution-channel) | GPUI distribution channel | **Accepted** |
| [ADR-002](#adr-002--backend-in-rust-axum-as-a-workspace-member-not-python) | Backend in Rust (Axum) as a workspace member, not Python | **Accepted** |
| [ADR-003](#adr-003--cargo-workspace-agentsmd-31s-tree-applied-per-crate) | Cargo workspace; `AGENTS.md` §3.1's tree applied per crate | **Accepted** |
| [ADR-004](#adr-004--extending-agentsmds-coverage-floors-and-reconciling-planmd) | Extending `AGENTS.md`'s coverage floors and reconciling `PLAN.md` | **Accepted** |
| [ADR-005](#adr-005--relaxing-agentsmd-61-62-performance-thresholds) | Relaxing `AGENTS.md` §6.1/§6.2 performance thresholds | **WITHDRAWN** |
| [ADR-006](#adr-006--row-estimator-for-the-virtualized-message-list) | Row estimator for the virtualized message list | **Proposed** |
| [ADR-007](#adr-007--deferring-the-from-error-payloads-in-shnexuserror) | Deferring the `#[from]` error payloads in `ShNexusError` | **Accepted (with a dated obligation)** |
| [ADR-008](#adr-008--adopting-rstest-for-parameterized-tests) | Adopting `rstest` for parameterized tests | **Accepted** |

---

### ADR-001 — GPUI distribution channel

**Status:** Accepted

#### Context

The published `gpui` crate on crates.io is version `0.2.2`, last published 2025-10-22 — roughly
eleven months stale while Zed ships daily releases (1.19.1 as of 2026-09-04, per zed.dev). Its
README states the user must "be on macOS or Linux", and its `windows-manifest` feature is an empty
list that only embeds an app manifest. It provides no Windows platform backend.

Zed *itself* does run on Windows, but that is the monorepo build, not the standalone crate.
Conflating the two is the most common error made about GPUI platform support. For accuracy:
upstream's Windows path uses **DirectX 11 for rendering** (a backend Zed built specifically to
cover Windows 7+ and VMs), with **Win32 for windowing and DirectWrite for text**.

This is the highest-consequence choice in the project: it gates all work. It was resolved by the
Phase 0 spike, a hard gate with no estimate attached (`PLAN.md` §8).

#### Decision

Depend on GPUI directly from `zed-industries/zed` via a **git dependency with a pinned `rev`**,
using two crates:

- `gpui` — the platform-agnostic core
- `gpui_platform` — the dispatcher that selects the OS backend

`gpui_platform` is **not optional**; depending on `gpui` alone yields no window on Windows. Upstream
extracted the platform backends out of `gpui` (commit "gpui: Extract gpui_platform out of gpui",
2026-02-19) into `gpui_apple`, `gpui_macos`, `gpui_linux`, `gpui_windows`, `gpui_wgpu`, and
`gpui_web`. On `main`, the `wayland` and `x11` features of `gpui` no longer carry `blade-graphics` /
`cosmic-text` / `x11rb` / `objc2-metal`, and `macos-blade` is gone.

The pin in use is **`rev = e683fd7b465ecfb42b1da88ff685d204c2781076`**, recorded in the root
`Cargo.toml` with a comment naming the upstream date (2026-09-27T17:37:29Z), the upstream subject
line, and the upstream release context (Zed 1.19.1, 2026-09-04).

**License:** Apache-2.0, verified against the repository root `LICENSE` and
`crates/gpui/LICENSE-APACHE` ("Copyright 2022-2025 Zed Industries, Inc."). No copyleft obligations.
Some third-party crate aggregators incorrectly report Zed as GPL-3.0-or-later; that claim is false.
Do not source license facts from aggregators.

#### Consequences

- Every upstream sync is a **deliberate upgrade task**, not a `cargo update`. Review breaking
  changes across the dependency tree before moving the pin.
- A `git` dependency is not publishable to crates.io and expresses no semver range. Accepted; see
  ADR-002.
- The pinned `rev` is the upgrade boundary. Record it in `Cargo.toml` with a comment naming the
  date and the upstream version it corresponds to.
- **Compile time is a real cost.** GPUI's tree is large, and `AGENTS.md` §7.2 requires compile-time
  impact to be measured. The Phase 0 baseline is 2m 12s dev cold, 3m 10s release cold on 12 logical
  CPUs, across 675 packages and 424 release compile units.
- **Release builds require a shader compiler that debug builds do not.** `gpui_windows/build.rs`
  compiles HLSL under `#[cfg(not(debug_assertions))]` and panics with `"Failed to find fxc.exe"` when
  it cannot locate one, so `cargo build` succeeds on a machine where `cargo build --release` fails
  outright. This must be provisioned in CI setup, not discovered in a release job. Documented in
  `README.md` and `PLAN.md` §8.
- **`test-support` is not free.** It enables `leak-detection` (pulling `backtrace`) and `proptest`.
  Measured cost: 9.5s of release wall clock and ~155KB of binary (190.5s → 181.0s;
  10,401,792 → 10,246,656 B) — about 1.5% size and 5% build time. Moving `gpui_platform` to
  `[dev-dependencies]` reclaims it. Not worth deviating from `AGENTS.md` §2 at that price; recorded
  so the decision is reversible with numbers behind it.
- **`gpui_windows` is pure Rust over the `windows` crate**, with a hand-written D3D11 renderer and
  DirectWrite text. No Vulkan, no DXC, no `blade`, no `shaderc`. Only `fxc.exe` (shader model 4.1
  targets) is needed beyond a C++-free toolchain.
- **Some testing capability is unavailable on Windows.** `current_headless_renderer()` returns
  `Ok(None)` there, so screenshot-diffing tests are macOS/Linux only. Headless render, simulated
  pointer and keyboard input, and assertions on state, focus, layout and accessibility all work.
  See `PLAN.md` §8, finding 3.

**Validation — the Phase 0 result.** The spike passed and the decision is confirmed by measurement,
not by argument (`PLAN.md` §8, commit `491bd0f` on `spike/gpui-windows`):

- **A window opens on Windows** from a clean checkout at the pinned rev (non-zero window handle).
- **4/4 headless tests pass**, covering render, click and keystroke.
- **`cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test` and
  `cargo build --release` are all green.**
- **Release binary: 9.92 MB** (10,401,792 B) — 3× under `AGENTS.md` §6.1's 30MB maximum and 2×
  under its 20MB target.

ADR-001 holds. No fallback was needed.

#### Alternatives considered

| Option | §7.2 verdict | Why not chosen |
|---|---|---|
| `gpui = "0.2.2"` from crates.io | Passes maintenance bars | Stale by ~11 months; no Windows backend. Disqualifying. |
| `gpui-component` / `gpui-kit` (longbridge) | Passes all five criteria; 0.6.6 published 2026-09-21, ~82k downloads in 90 days | Actively maintained and cross-platform. Rejected because it is a third-party opinionated design system, and this project builds its own UI layer anyway. **Reconsider if pin upkeep becomes a burden.** |
| `gpui-unofficial` | **FAILS §7.2**: its Windows backend crate `gpui-windows-gpui-unofficial` has 1,583 all-time downloads — far under the 500/month bar the constitution sets. Maintained by one person and explicitly not by the Zed team. | Rejected on §7.2 grounds, not on preference. |
| `open-gpui-platform`, `gpui-ce` | Incomplete assessment | Available republishes; would need the same §7.2 audit before adoption. |
| egui / iced / Slint | Passes all five criteria | Mature Windows support on crates.io, but abandons the portfolio's "built on Zed's framework" premise. |

**Fallback:** if the Phase 0 spike had failed, fall back to `gpui-kit` or `gpui-unofficial`. That
would have required **revising this ADR first** — it is not an in-flight substitution. The rest of
the architecture is unaffected either way, because GPUI is confined to `ui/` and `app.rs`.

---

### ADR-002 — Backend in Rust (Axum) as a workspace member, not Python

**Status:** Accepted

#### Context

Sh_Nexus needs a real backend, not a mock: JWT auth, channel and message REST, and a WebSocket
server. `AGENTS.md` §1 frames the project as demonstrating "full-stack capability", and §8.1
mandates integration flows (Login, Real-time two-client delivery, Reconnect, Offline) that cannot be
tested against a stub.

`PLAN.md` Rev 1 listed the backend as **optional** and proposed **Python + FastAPI**. That is
inconsistent with the constitution in two ways. It demotes a mandatory part of the system to a
"nice to have", and it introduces a second language into a project whose constitution bans web
technology outright (`AGENTS.md` §7.1: "No JavaScript, no WASM, no web tech").

The wire protocol is not a detail here. `AGENTS.md` §7.4 requires a `v` field on every WS envelope
with explicit rejection of unknown major versions, a client-generated `client_msg_id` on every
client frame for idempotent dedup across reconnects, per-cursor resumption from `last_message_at`,
and mandatory timeouts. `AGENTS.md` §4.2 requires serde round-trips for every wire format in **both**
directions. A hand-duplicated protocol definition on the server side is a standing invitation to
violate both.

#### Decision

**Rust + Axum, as a member of the same Cargo workspace** (`crates/sh_nexus_server`).

The wire protocol is defined exactly once, in a third workspace crate, `sh_nexus_wire`. Both the
client and the server compile against it. There is one language, one toolchain, one `cargo test`, and
one CI matrix.

#### Consequences

- **The wire protocol is defined once and cannot drift.** `sh_nexus_wire` owns the serde DTOs, the
  `v` envelope, and the version negotiation. Client and server are the same compiler invocation over
  the same types, so a protocol change that breaks one side cannot compile on the other. This makes
  "wire protocol drift" a category of bug that is structurally impossible rather than merely
  unlikely.
- **One language, one toolchain, one `cargo test`, one CI matrix.** Backend tests are ordinary Cargo
  tests. `AGENTS.md` §5.1's checklist, §6.1's CI metrics and §6.3's merge-blocking rules apply to
  the server with no separate pipeline, no second dependency audit, and no second formatter.
- **One `§7.2` audit surface.** Every crate the project depends on is audited in the single
  `docs/DEPENDENCIES.md`, regardless of which side of the wire it serves.
- **Server and client share a build.** Changing the wire crate recompiles both. For a project of
  this size that is a feature, not a cost: the two sides are always consistent and the CI signal is
  honest. It does mean a wire change cannot be built and tested in isolation.
- **The client and the server ship from one build pipeline**, so a `cargo build --release` produces
  both artifacts and both are covered by the same binary-size accounting.
- **Slower backend authoring than FastAPI.** This is a real cost. Axum, tokio and `sqlx`/`rusqlite`
  are more ceremony than an async Python function; the request/handler/router/state plumbing is
  explicit rather than implicit. Accepted deliberately: this is a portfolio project whose value is
  the architecture, and the protocol guarantee is worth more than the authoring speed.
- **The coverage floor extends to the server.** `sh_nexus_server` is held to ≥80% — see ADR-004.

**Revisit condition.** This is the first ADR to revisit if backend velocity becomes the binding
constraint on delivering the mandatory flows in `AGENTS.md` §8.1. If the server is the schedule
risk, the question to reopen is *this* ADR — not a workaround inside it.

#### Alternatives considered

| Option | Why not chosen |
|---|---|
| **Python + FastAPI** (as proposed in `PLAN.md` Rev 1) | Faster to author, and FastAPI's ergonomics are genuinely better for a small REST surface. But it **duplicates the wire types by hand in a second language**, which guarantees drift and violates `AGENTS.md` §7.4's versioning requirements and §4.2's round-trip requirements. It also introduces a second runtime, a second dependency ecosystem, a second CI matrix and a second audit into a project whose constitution bans web technology in the first place, and into a portfolio whose stated premise is native Rust. |
| Rust + Axum as a **separate repository** | Shares the language, but not the build. The wire crate would have to be published or path-referenced across a repo boundary, so the "defined once" guarantee degrades to "defined once, by convention" — the exact failure mode Python introduces, in a weaker form. Also splits the CI matrix and the §7.2 audit, which is a cost paid for no benefit. |
| Go, Node, or Elixir server | Same two-language drift problem as Python, with a worse story: none of them can share `sh_nexus_wire` as Rust types. Node is additionally banned outright by `AGENTS.md` §7.1. |
| **No backend** — mock or fixture the server side (as `PLAN.md` Rev 1 proposed) | Leaves `AGENTS.md` §8.1's Real-time, Reconnect and Offline flows untestable against a real server, and §7.4's resumption and idempotency requirements unverifiable. The constitution mandates those flows as integration tests; a mock cannot exercise them. |
| Rust + Actix / Rocket | Functionally comparable to Axum. No decisive technical difference; Axum's tower/middleware composition and tokio alignment made it the lower-friction fit beside a tokio-based client. |

---

### ADR-003 — Cargo workspace; `AGENTS.md` §3.1's tree applied per crate

**Status:** Accepted

#### Context

`AGENTS.md` §3.1 specifies a module organization as a single `src/` tree, and §3.2 specifies layer
rules for `core/`, `ui/`, `state/`, `network/`, `db/` and `platform/`. Read literally, that is a
single-crate layout.

The project needs three things: the client application, a wire crate shared with the server
(ADR-002), and the server. Three crates with the client/server split cannot live in one
`src/` tree.

The risk in resolving this is losing the layer discipline. `AGENTS.md` §3.2's rules and §4.1's
coverage floors are the mechanism that keeps business logic out of `ui/` and out of `network/`, and
that mechanism is stated in terms of directory names, not crate names. A workspace must not become
an excuse to relax them.

#### Decision

**A Cargo workspace.** The client lives at `crates/sh_nexus/`, alongside `crates/sh_nexus_wire/`
and `crates/sh_nexus_server/`. **`AGENTS.md` §3.1's `src/` tree is applied inside each crate**, not
at the repository root.

The root `Cargo.toml` declares `[workspace] members = ["crates/sh_nexus"]` with shared
`[workspace.package]` metadata and shared `[workspace.dependencies]` — which is where the pinned GPUI
`rev` lives, so a crate can never drift onto a different pin.

#### Consequences

- **`AGENTS.md` §3.2's layer rules hold per crate.** Inside `crates/sh_nexus/src/`, the `core/` /
  `ui/` / `state/` / `network/` / `db/` / `platform/` split is exactly as §3.1 specifies. The
  boundary is not weakened by the workspace; it is reproduced in each crate that needs it.
- **`core/` stays pure, and that is what makes the coverage floor reachable.** §3.2 forbids
  `core/` from importing `gpui` or `tokio` and from touching the filesystem. A test that asserts
  dependency direction is part of the plan, because the ≥90% floor for `core/` is unreachable if the
  layer is impure. Theme *file watching* therefore lives in `platform/file_watch.rs`, and
  `core/theme.rs` only parses and validates bytes it is handed.
- **`state/bridge.rs` is the single seam.** `AGENTS.md` §7.3 forbids blocking `cx.update_global`
  from non-UI threads while §3.2 forbids `network/` from importing GPUI. A named owner resolves the
  tension: `network/` emits plain domain events and `bridge.rs` is the only module that calls
  `cx.update_global` / `cx.update`, always on the main thread. Without a named owner, the obvious
  implementation is the one that is prohibited.
- **All mutations go through `state/actions.rs`**, per §3.2, so they stay auditable and testable.
- **`AGENTS.md` §4.1's coverage floors apply to the client crate** — `core/` ≥90%, `network/` ≥80%,
  `state/` ≥80%, `db/` ≥85%, utilities ≥85% — and are **extended to `sh_nexus_wire` and
  `sh_nexus_server`**; see ADR-004 for why, and for the floor values.
- **`§6.1`'s workspace-total floors apply across the workspace**: ≥75% minimum, ≥85% target.
- **One `cargo test` covers all three crates.** No per-crate invocation to forget in CI.
- **The pinned GPUI `rev` is declared once**, in `[workspace.dependencies]`, so the client cannot
  accidentally depend on a different revision than the one ADR-001 pins.
- **Test discovery is now workspace-wide and unforgiving.** Cargo auto-discovers `tests/*.rs` and
  `tests/*/main.rs` **only**. Files at `tests/integration/*.rs` are silently never compiled or run —
  a green build with all ten mandatory integration flows from `AGENTS.md` §8.1 missing. The
  integration tests are therefore flat in `crates/sh_nexus/tests/`, named for their flows. This is
  recorded as a risk in `PLAN.md` §14 and as a note in `crates/sh_nexus/tests/spike_render.rs`.

#### Alternatives considered

| Option | Why not chosen |
|---|---|
| **Single crate, all modules flat under `src/`** (a literal reading of `§3.1`) | The client and the server cannot share Rust types across a module boundary without the server becoming a library the client links — which would pull `axum` and the server's entire dependency tree into the client binary, against `AGENTS.md` §6.1's binary-size row and §6.3's 5MB-justification rule. Protocol drift between "two copies of the same type" becomes possible, which is the failure ADR-002 exists to prevent. |
| Server in a **separate repository** (discussed under ADR-002) | Cannot share `sh_nexus_wire` as types. Weakens the one-definition guarantee and splits the CI matrix. |
| **Nested workspaces** (a workspace inside a workspace) | No technical benefit here. Doubles the configuration surface for three crates, and `AGENTS.md` §2.2's "idiomatic Rust" standard argues against it. |
| A flat repository with **three independent Cargo projects** and no workspace | Requires separate dependency declarations, so the GPUI pin could drift between them; requires three `cargo test` invocations; and no shared target directory, so the dependency tree is built more than once. Directly contradicts ADR-002's "one toolchain, one `cargo test`, one CI matrix". |
| Keep the client at the repository root (`src/` at top level) and add `crates/` for the rest | Legal in Cargo, but it breaks the symmetry of "the client is a crate like the others" and makes the client's own root the odd one out. The uniform `crates/<name>/` layout is easier to reason about and is what `PLAN.md` §4 already documents. |

---

### ADR-004 — Extending `AGENTS.md`'s coverage floors and reconciling `PLAN.md`

**Status:** Accepted

#### Context

`AGENTS.md` is the constitution. `PLAN.md` is subordinate. Where the two disagree, `AGENTS.md` wins
and `PLAN.md` is the bug.

**Rev 1** contradicted `AGENTS.md` in eight places. **Rev 2** was written to reconcile them, and an
**independent conformance audit** of Rev 2 against `AGENTS.md` then found **4 blockers, 13 major and
21 minor residual defects.** Rev 3 applied the audit corrections; Rev 4 records the Phase 0 result.

Two of the four blockers were **self-inflicted** — introduced by Rev 2 while trying to fix Rev 1, and
both are failure modes that produce a *green build over a void*:

1. **`client_msg_id` omitted from PLAN's own frames.** Rev 2 asserted that every client WebSocket
   frame must carry a client-generated `client_msg_id`, then omitted it from **three of its own five**
   client frames. `AGENTS.md` §7.4 requires it on *every* client message for idempotent dedup across
   reconnects. A plan that states the rule and then violates it in its own protocol listing is worse
   than a plan that never states the rule, because the reader inherits the contradiction.
2. **Integration tests in a path Cargo does not auto-discover.** Rev 2 placed the ten mandatory flows
   from `AGENTS.md` §8.1 in `tests/integration/*.rs`. Cargo auto-discovers `tests/*.rs` and
   `tests/*/main.rs` **only**. Files in `tests/integration/` are **silently never compiled or never
   run** — a green build with all ten mandatory integration flows absent, and no error to indicate it.

A third defect was a governance failure rather than a technical one: **Rev 2 declared `AGENTS.md`'s
performance thresholds "targets, not gates."** A subordinate document may request an amendment
through the ADR path; it may never pre-apply one. And doing so selectively — keeping the metrics one
likes while suspending the two one does not — is renegotiation, not reconciliation. Corrected in
Rev 3.

Separately, `AGENTS.md` has an **internal** conflict: §4.1 requires `core/` ≥90% while §6.1 lists
85% minimum / 95% target for the same area. Recorded in the appendix below.

#### Decision

1. **`AGENTS.md` is the constitution; `PLAN.md` is subordinate.** A subordinate document may
   *request* an amendment to the constitution through the ADR path (`AGENTS.md` §9.2). It may
   **never pre-apply one.** Rev 2's selective relaxation of §6.1/§6.2 is withdrawn; those thresholds
   stand as written.
2. **The constitution's internal conflict is resolved by taking the stricter value: `core/` ≥90%.**
   §4.1's 90% is the binding floor. §6.1's 85% minimum / 95% target is read as a workspace-level
   summary, not a competing figure. The conflict is recorded for amendment.
3. **Coverage floors are extended to the two new crates:**

   | Area | Minimum | Target |
   |---|---|---|
   | `core/` | **90%** | 95% |
   | `network/` | 80% | — |
   | `state/` | 80% | — |
   | `db/` | 85% | — |
   | Utilities | 85% | — |
   | **`sh_nexus_wire/`** | **80%** | — |
   | **`sh_nexus_server/`** | **80%** | — |
   | **Workspace total** | **75%** | **85%** |
   | New code, any task | 80% | — |

   Both new floors are ≥80% because the wire crate and the server are new code, and
   `AGENTS.md` §5.1 already sets "test coverage of new code ≥ 80%" as a pre-commit gate. The floor
   follows the code; the code is new.

4. **`sh_nexus_wire` ≥80% is not a formality — it is where the mandate actually lives.**
   `AGENTS.md` §4.2 requires "Serde round-trips for every wire format; malformed payload rejection"
   and `AGENTS.md` §7.4 requires explicit rejection of unknown major versions. Those requirements
   are *about the wire crate*. §4.1's floors are keyed to client directory names (`core/`,
   `network/`, `state/`, `db/`) and so do not reach a crate that contains no such directory.
   Without an extension, the most protocol-critical code in the project would be the only code
   exempt from a coverage floor — a gap that follows directly from §4.1's directory-keyed wording,
   not from any disagreement about intent.

#### Consequences

- **Every `AGENTS.md` threshold stands as written.** Nothing in `PLAN.md` suspends a merge-blocking
  gate. §6.1 and §6.2 are currently being met with room to spare (see ADR-005).
- **The two test-void traps are closed and are now documented where they will be encountered**:
  `PLAN.md` §4 carries a prominent warning about Cargo's test discovery, and
  `crates/sh_nexus/tests/spike_render.rs` repeats it in its own module docs. Both `client_msg_id`
  requirements and per-channel resync are now stated in `PLAN.md` §6 and §7, consistent with §7.4.
- **The protocol is self-consistent.** `PLAN.md` §6 states the `client_msg_id` rule and every
  client frame in the listing carries one. `PLAN.md` §7 makes resync **per channel**, because a
  single global timestamp cannot reconstruct per-channel history when each channel has its own
  `last_message_at` and its own unread count — which is what `AGENTS.md` §8.1's Reconnect Flow
  ("no duplicates, no gaps") requires.
- **Coverage enforcement covers the whole workspace**, and the two new crates are not a coverage
  holiday. `cargo tarpaulin` runs across the workspace.
- **`AGENTS.md` needs a follow-up amendment pass**, not a rewrite. Five of its own issues were found
  during this work and are listed in the appendix below. None of them blocks Phase 1; all of them
  will mislead an implementer who reads only the constitution.
- **The precedent is the durable output.** A subordinate document that pre-applies its own
  preferences is the failure mode this ADR exists to prevent, and it is now named in `PLAN.md`'s own
  precedence rule at the top of the file.

#### Alternatives considered

| Option | Why not chosen |
|---|---|
| **Let `PLAN.md` relax the §6.1/§6.2 thresholds**, as Rev 2 did | Only the constitution may set or relax its own gates, and only through the ADR path. A subordinate document voiding merge-blocking thresholds — and doing it selectively — is renegotiation. Rejected on governance grounds regardless of whether the numbers would have been met. (In the event they would have been: see ADR-005.) |
| **Take §6.1's 85% for `core/`** over §4.1's 90% | §6.1 is the summary table; §4.1 is the normative requirement and is the stricter of the two. When a document conflicts with itself, take the stricter value — it is the one that cannot be satisfied by doing less. |
| **Leave `sh_nexus_wire` and `sh_nexus_server` with no floor**, arguing §4.1 does not mention them | Technically a literal reading of §4.1. Rejected because it exempts the most protocol-critical code in the project, and because §5.1's "new code ≥80%" gate already applies. The floor exists; §4.1 just cannot see it. |
| **Add the wire/server coverage rows to `AGENTS.md` directly** | `AGENTS.md` was not modified — this ADR is the request, and the constitution is amended separately through the ADR path. Editing the constitution from a subordinate document is precisely the failure Rev 2 committed. |
| **Rewrite `PLAN.md` from scratch** | The Rev 1 → Rev 2 → Rev 3 → Rev 4 sequence and its revision history are themselves an audit trail: they show which defects were found, by what check, and how they were closed. Discarding it would destroy the evidence and re-expose the project to the same defects. |

---

### ADR-005 — Relaxing `AGENTS.md` §6.1/§6.2 performance thresholds

**Status:** **WITHDRAWN**

> This ADR is **retained deliberately**. It records a relief request that was raised, and the
> measurement that refuted it. Deleting it would leave the same hypothesis available to be
> re-raised by the next person who guesses at GPUI's binary size.

#### Context

`PLAN.md` Rev 2 predicted the release binary would land in **tens of megabytes**, declared
`AGENTS.md` §6.1's **30MB ceiling unachievable**, and **pre-drafted this relief ADR** in advance of
measuring anything.

The hypothesis behind it was written down in the constitution itself, in the note under §6.1's
table:

> "Note: GPUI statically links the renderer. Binary size is larger than typical Rust apps but
> smaller than Electron."

The plan read that note, inferred a large static renderer, and treated the inference as settled
fact. It was an assumption, not a measurement. `AGENTS.md` §2.3 is explicit on the difference:
*"Profile before optimizing. Measure, don't guess."*

The predicted remedy — amending §6.1's binary-size row — is exactly the kind of action `AGENTS.md`
§9.2 reserves for the ADR path, and exactly the kind of pre-application `PLAN.md`'s own precedence
rule forbids. Writing the ADR early was, in fairness, an attempt to do it through the proper path.
The defect was not the process; it was that the premise was never tested.

#### Decision

**None.**

#### Consequences

**The hypothesis was refuted by measurement.** The Phase 0 spike measured the release binary at
**9.92 MB** (10,401,792 B) — **3× under the 30MB maximum and 2× under the 20MB target.** The
ceiling `AGENTS.md` §6.1 calls unachievable is met with more than 20MB to spare.

The underlying factual claim is also false: **GPUI does not statically link its renderer on
Windows.** `gpui_windows` is pure Rust over the `windows` crate, with a hand-written **D3D11**
renderer and **DirectWrite** text. There is no `blade`, no `shaderc`, no Vulkan, and no DXC. The
only shader tool is `fxc.exe` (shader model 4.1), required for release builds only.

**`AGENTS.md` §6.1 needed no amendment.** Its numbers stand as written and are currently being met
with room to spare. The relief request is withdrawn in full — context, decision, and the draft
amendment it would have produced.

**The note under `AGENTS.md` §6.1's table is the surviving loose end.** The binary-size row itself is
correct and met; the explanatory note carries the false premise. That note should be corrected
through the amendment path. It is deliberately *not* listed in the appendix below, which is scoped
to defects that affect implementation.

**The remaining open item is the delta, not the total.** The spike linked only `gpui` +
`gpui_platform`. The dependencies the real application needs were not measured: `syntect`, `reqwest`
with TLS, bundled SQLite, `rodio`, `notify-rust`. The 9.92MB figure is a **floor**, not a
projection, and the delta is genuinely unmeasured. It is a **Phase 3 exit criterion**, to be
measured when those crates actually land (`PLAN.md` §11).

**`AGENTS.md` §6.3's rules apply from here.** Any increase >5MB in binary size requires
justification, and a regression >10% in any performance metric blocks the merge — against the
measured baseline, not against a guess.

**The methodological lesson is recorded where it will be read next.** A constitutional note asserting
a fact about a third-party framework is a hypothesis with the authority of a rule attached. It gets
tested before it gets obeyed.

#### Alternatives considered

| Option | Why not chosen |
|---|---|
| **Raise the §6.1 binary-size ceiling to ~50MB** (the pre-drafted amendment) | Refuted by measurement. The ceiling is not merely met, it is met with a 3× margin. Amending a working threshold to accommodate a hypothesis that measurement disproved would be a governance failure with an invented justification. |
| **Lower the ceiling to reflect the 9.92MB result** | Tempting, and rejected. The 9.92MB figure excludes `syntect`, `reqwest`, bundled SQLite, `rodio` and `notify-rust`. Lowering the ceiling now would set a bar against a partial build and invite a future amendment request when the real number lands. §6.1's current numbers are already met; leave them. |
| **Delete this ADR** | A withdrawn decision is a decision. Removing it destroys the record of *why* the hypothesis was rejected and makes the next guess cheaper to make. The record is the output. |
| **Amend §6.2's latency rows too**, on the argument that GPUI's unproven 120fps path invalidates them | No evidence. The latency rows are not yet measured at all, and nothing in the spike contradicts them. Guessing at a second, unrelated set of thresholds is the same error as the first. |

---

### ADR-006 — Row estimator for the virtualized message list

**Status:** **Accepted.** Supersedes the Proposed text of 2026-09-27; the
decision below is a different one, and the reason it is different is recorded in
`#### Context` because the original premise turned out to be false.

> **What changed, in one line:** this ADR was written assuming `UniformList` was
> the only virtualized list available. At the pinned `rev e683fd7` it is not, and
> the framework's own chat panel does not use it.

#### Context

**The premise this ADR originally rested on was wrong, and the correction is
verifiable in the pinned revision.** GPUI ships **two** list elements, and they
are not variants of one idea — they are answers to two different questions:

| Component | What its own docs say | What Zed uses it for |
|---|---|---|
| `elements/uniform_list.rs` | *"A scrollable list of elements with **uniform height**… measures the first element and then lays out all remaining elements in a line based on that measurement… **only works for elements with uniform height**"* | `picker.rs` (18), `data_table.rs` (12), `git_graph.rs` (9), `lsp_store.rs` (9) |
| `elements/list.rs` | *"A list element that can be used to render a large number of **differently sized** elements efficiently… If all of your elements are the same height, see `crate::UniformList` for a simpler API"* | **`agent.rs` (37) — Zed's chat panel**, `sidebar_tests.rs` (88), `persistence.rs` (58), `acp_thread.rs` (57) |

`uniform_list.rs` points the reader at `List` for the other case, in its own
documentation. **So the variable-height virtualized list is not something this
project has to build; it is a component it already depends on, and the reference
implementation of a chat list in the framework this project pins uses it.**

**What that does to the original proposal.** The previous text offered a table of
six alternatives, every one of which assumed the choice was between *strict
uniform* and *build a measured-height overlay on top of it*. On that assumption
the overlay was the honest answer and its cost had to be argued. **On the
verified premise there is no overlay to build**, and proposing one would have
been months of work re-deriving a component that already exists.

**The conflict inside the constitution is real, and it is unchanged.** What
changes is that it is now a conflict with a known resolution rather than an open
design question.

`AGENTS.md` §7.3 is explicit and mandatory:

> "Virtualized lists: Message lists must render only visible items. **Use a uniform
> row estimator and recycle.**"

A **uniform** row estimator assumes every row is the same height. That is correct
for a list of fixed-height items. It does not hold for a chat message list.

Markdown and code blocks make true message row heights **genuinely variable**
within a single channel: a one-word message, a five-line paragraph, a fenced code
block, a message with reaction chips underneath — all in the same viewport, all
different heights, and none of them predictable from the message's metadata
without laying the text out. **Wrapping width alone makes height a function of
viewport width**, so heights are not even stable across window resizes.

`AGENTS.md` §1 lists the project's core priorities in order, and the first is
**Responsiveness**: "120fps UI, <16ms message render, **zero jank while receiving
messages**." §6.2 sets scroll frame time at **<8ms** for 10k virtualized messages.
A uniform estimator against variable-height content forces the scroll position to
be computed from an assumption the data contradicts: the scrollbar thumb is
wrong, `scroll_to` overshoots or undershoots, and a newly-arrived message lands at
the wrong offset. **One tall first row makes every subsequent row's position
wrong** — that is not approximate, it is structurally incorrect rendering.
That is visible jank, against the project's highest priority, caused by a rule
written for a different data shape.

This is a genuine conflict inside the constitution, not a preference: §7.3's
uniform estimator directly threatens §1's zero-jank priority and §6.2's <8ms
frame-time target. `AGENTS.md` §3.2 already anticipates layer rules adapting to
context; the same latitude is warranted here.

#### Decision

**Build the message list on `gpui::List`, and amend `AGENTS.md` §7.3 to permit a
measured-height estimator for lists whose content is variable-height.**

1. **`List` for the message list.** It virtualizes — it renders only the visible
   subset, which is §7.3's substantive requirement and the one that protects
   memory against §2.3's "render 10,000 DOM-equivalent elements". It measures
   real heights, so scroll position is correct rather than estimated.
2. **`AGENTS.md` §7.3 is amended**, not overridden, to read: *"Use a uniform row
   estimator and recycle; for lists whose content is genuinely variable-height,
   a measured-height estimator is permitted."* The uniform estimator **stays
   mandatory for every list that is uniform** — Zed's own pickers, tables, git
   graph and LSP store are all uniform and all use `UniformList`, and this ADR
   does not touch them.
3. **Recycling is unaffected.** §7.3's recycle requirement stands as written and
   is not in question. Only the height source changes.
4. **Height changes must be reported.** `List` is explicit that *"clients… need
   to ensure that elements outside of the scrolled area do not change their
   height… If your elements do change height, notify the list element via
   `ListState::splice` or `ListState::reset`."* Editing a message, a reaction
   changing a chip row, or a markdown segment re-parsing all change a row's
   height, and each is a `splice`. **This is a real obligation the previous
   proposal did not have**, because the previous proposal did not know `List`
   existed.

**Why the amendment is the honest framing rather than a convenience:** §7.3's
substantive requirement is *"render only visible items"*, and `List` satisfies it.
The word *"uniform"* is the part that is wrong, and it is wrong for a reason the
framework's own authors documented and then routed their chat panel around. An
ADR that proposes building a better version of a component the project already
depends on is proposing avoidable work; an ADR that points at the component and
proposes amending the rule it mis-specifies is proposing the actual fix.

#### Consequences

- **Scroll position becomes correct**, for every row, from first paint, not only
  after a row has been seen once. The previous proposal's residual cost —
  *"first paint of a scrolled-to-middle channel remains estimate-based"* — is
  **eliminated**, not accepted.
- **`List` is the slower of the two.** `uniform_list.rs` says so directly: it
  exists to avoid *"the full taffy layout system"* because that *"is much faster
  but only works for elements with uniform height"*. §6.2's **<8ms** scroll
  frame time at 10k messages is therefore the thing to measure first, and it is
  the measurement Phase 2 owes `docs/BASELINES.md`. **If `List` misses the
  budget, the answer is not to go back to a uniform estimator** — a uniform
  estimator on chat content is incorrect, not merely slow — but to raise the
  budget with evidence, as ADR-005 refused to lower it without one.
- **A height cache is not needed, and that removes a whole class of problem.**
  The previous proposal had to specify a bound and an eviction policy for
  measured heights, and had to discard them on resize. `List` keeps its own
  layout state intrusively on the view, *"so that your code can coordinate
  directly with the list element's cached state"*. §7.1's prohibition on
  unbounded growth is the component's problem now, not a design decision this
  project has to get right.
- **Responsibility shifts to the caller.** `List` is not a pure virtualizer: it
  requires the caller to keep a row's height stable or report the change. Every
  height-mutating action in `state/actions.rs` and `core/markdown.rs` now has a
  corresponding `splice`, and forgetting one produces a list that slowly drifts
  out of alignment — a new failure mode that did not exist under
  `uniform_list`, which is worth naming as the cost.
- **§6.2's <8ms scroll frame time becomes measurable for the first time.**

#### Alternatives considered

| Option | Why not chosen |
|---|---|
| **`UniformList` + a measured-height overlay** (the previous proposal) | **Rejected on a verified premise, not on judgement.** It was a plan to re-derive a component the project already depends on. Its problem — the estimate is wrong until a row has been seen — is `List`'s absence rather than a gap to fill. |
| **Strict uniform estimator**, as §7.3 says | Correct for fixed-height lists; wrong for chat content, structurally. A tall first row misplaces every row after it. Kept as mandatory for genuinely uniform lists by decision 2. |
| **Measure everything up front** — lay out all N rows before first paint | Correct position immediately, and correct for every row. Rejected on performance: O(N) layout work on channel switch, against §6.2's <100ms cold channel-switch budget for 500 messages, and it is exactly the "render 10,000 elements" anti-pattern §2.3 forbids. This is also what `List` avoids by measuring lazily. |
| **A per-content-class estimator** (one for text, one for code blocks, one for reactions) | Genuinely better than a single uniform value. **Obsolete**: `List` measures the real thing rather than estimating it by class, so there is no class to infer. |
| **Ignore variable heights**, normalizing every bubble to a fixed height | Avoids the problem by removing the feature. Rejected: §10 and §4.2 require Markdown and code-block rendering, so variable height is a requirement, not an accident. |
| **Defer until Phase 2 shows measurable jank** | Rejected. §1's zero-jank priority is stated as a priority, not a nice-to-have, and this ADR now has a verified answer rather than a preference to test. |

#### Amendment filed

`AGENTS.md` §7.3 is amended as stated in decision 2. The amendment is one clause
long and changes no other list in the project. **Every list this ADR does not
name — the sidebar, the channel list, the Ctrl+K switcher — has genuinely
uniform rows and keeps the strict uniform estimator.** Those are `UniformList`
rows, which is what the framework's own pickers and tables are.


### ADR-007 — Deferring the `#[from]` error payloads in `ShNexusError`

**Status:** **Accepted (with a dated obligation)** · 2026-09-27

> The base status is **Accepted** — the decision below is in force and work unit 1A complies with it.
> The parenthetical is a **condition attached to the acceptance**, not a fourth status value: the
> three values in the table above are unchanged, and the obligation is stated, dated and binding in
> **Consequences** below. An Accepted-with-obligation is Accepted, so implementation proceeds — the
> obligation is a dated commitment, and its deadline is enforced by review rather than by the
> compiler. The reason that distinction matters here is the last bullet of **Consequences**.

#### Context

`AGENTS.md` §3.3 specifies `ShNexusError` with `#[from]` payloads for its three I/O variants:

```rust
#[error("network error: {0}")]
Network(#[from] reqwest::Error),

#[error("websocket error: {0}")]
WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

#[error("database error: {0}")]
Database(#[from] rusqlite::Error),
```

Work unit 1A implemented the enum with **`String` payloads for those three variants**, and wired
`Serialization(#[from] serde_json::Error)` for real.

**The reason is dependency sequencing, not preference.** `reqwest` and `tokio-tungstenite` are
**Phase 4** work (`PLAN.md` §8) and `rusqlite` is **Phase 3** (`PLAN.md` §8). **None of the three
crates exists in work unit 1A.** A `#[from]` attribute is a claim that the source crate is a
dependency; writing one for a crate that is not in `Cargo.toml` does not compile, so the typed form
is not available as an option — it is simply not yet reachable.

`rusqlite/bundled` makes the same point sharper. It **compiles SQLite's C amalgamation**, which
would put a **C toolchain requirement** inside a work unit whose entire content is type
definitions. `PLAN.md` §2 already records `rusqlite/bundled` as a deliberately accepted C build
(`PLAN.md` §2, Dependency governance) — but accepted *in Phase 3*, where a database is the point, not
as a hidden cost of the type-foundation unit.

**What was implemented matches §3.3 everywhere else.** The variant **names** are §3.3's
(`Network`, `WebSocket`, `Database`, `Serialization`, `Auth`, `Protocol`, `Config`, `Theme`,
`Unknown`). The **payload meanings** are §3.3's. **All nine `Display` strings are exactly what
§3.3 specifies** — the `#[error("network error: {0}")]` text is byte-identical to the constitution's.
Only the *type inside* three of the nine payloads differs, and only in those three whose source
crate has not landed.

#### Decision

1. **The three I/O variants carry `String` for now.** `Network(String)`, `WebSocket(String)`,
   `Database(String)`. All three are constructed by `String` in every call site, so no call site
   changes shape when the typed form lands.
2. **`Serialization` carries its real `#[from]`** — `Serialization(#[from] serde_json::Error)` —
   because `serde_json` **is** a dependency of this work unit for the wire boundary. One of the four
   is real now rather than deferred with the other three, because for this one the crate is present.
3. **The change to the typed form is a two-line diff per variant**, and no call site moves. When
   `reqwest` lands, `Network(String)` becomes `Network(#[from] reqwest::Error)`; the variant's
   constructors change from a `String` to the error value the dependency already produces, and every
   other line in the crate is untouched.

#### Consequences

- **The real cost, stated plainly: a `String` payload loses the source error.** Two things are lost,
  and they are different losses:
  - **Pattern matching is gone.** `ShNexusError::Network` cannot be matched on
    `reqwest::Error`'s **kind** — timeout, connect, decode, body, redirect. A caller cannot ask
    "was this a timeout?" and branch on the answer, because after the `String` the kind does not
    exist. The reconnect logic `PLAN.md` §8 Phase 4 and `AGENTS.md` §7.4 require — backoff, retry on
    transient, do not retry on a non-transient — has less to work with than §3.3 intends it to.
  - **The source chain is not preserved.** `thiserror`'s `#[from]` generates a `#[source]`, and
    `std::error::Error::source()` walks it. With a `String` there is nothing to walk: the chain ends
    at the variant. `AGENTS.md` §7.5 requires that `error!` records *"Errors affecting
    functionality"* — a network failure is the canonical such error — and an `error!` that cannot
    reach the source can only record a rendered sentence.
- **The mitigation today is documentation plus a boundary rule, and it is partial.** Each of the
  three variants' **doc comments names the deferral explicitly** and states that the typed form is
  the right end state: `ShNexusError::Network` (the bolded *"The payload is a `String` in this
  revision, not a `#[from] reqwest::Error`"* paragraph), `ShNexusError::WebSocket` (*"A `String`
  payload for the same reason"*), and `ShNexusError::Database` (*"A `String` payload for the same
  reason"*, including the C-amalgamation note). The enum's own doc comment states that *"The payload
  of the three I/O variants is where this implementation deviates, and it is called out on each."*
  On the GPUI boundary, `errors.rs`'s module docs and rule 3 require the **source chain to be logged,
  not stringified into the domain**. Those four places are the reminder.
- **OBLIGATION — dated, explicit, and binding. The typed `#[from]` payloads MUST be wired in the same
  work unit that adds each dependency:**

  | Dependency | Phase (`PLAN.md` §8) | MUST be wired in |
  |---|---|---|
  | `rusqlite` | **Phase 3** — Persistence | the work unit that adds `rusqlite` |
  | `reqwest` | **Phase 4** — Networking | the work unit that adds `reqwest` |
  | `tokio-tungstenite` | **Phase 4** — Networking | the work unit that adds `tokio-tungstenite` |

  Not "in Phase 3" or "in Phase 4". **In the same work unit as the dependency.** Accepted 2026-09-27;
  the deadline is the phase, because `PLAN.md` carries estimates and no calendar schedule, so a
  calendar date would be a fiction. Each of the three doc comments above must be updated to remove
  the deferral note in that same work unit, so the reminder cannot outlive the thing it reminds
  about.
- **The failure mode of missing that obligation is silent, and that is the whole reason this
  obligation needs writing down.** **Nothing fails to compile** if `reqwest` lands and nobody wires
  the `#[from]`. The variant still says `String`; `String` is a perfectly valid payload; every call
  site still builds; `cargo check` is green; `cargo clippy` is green; `cargo test` is green. **The
  error just keeps flattening to a `String`** and the loss of the source chain goes unnoticed, because
  the symptom is not a failure — it is an error message that is slightly less useful than it should
  be, in a log nobody reads until the day they need it. This is the "green build over a void" shape
  that ADR-004 identified twice in `PLAN.md` Rev 2, and it is the reason the obligation is written
  as a review item rather than trusted to a reviewer to notice.
- **The check for it is a review item, not a compiler error.** There is no lint that fails on a
  `String` where §3.3 wanted a typed payload; the type is legal. So the enforcement point is
  `AGENTS.md` §5.3's review — and specifically a review that reads `crates/sh_nexus/src/errors.rs`
  **against the dependency list of the work unit under review**, asking one question: *did this work
  unit add a crate that `errors.rs` names, and if so, was the `#[from]` wired?* The three doc
  comments named above are what that reviewer reads.
- **The residual risk if the obligation is met late** is bounded and worth naming: during the gap, a
  `network/` or `db/` call site has the concrete error value in hand and discards it to a `String` at
  the `ShNexusError::` constructor. That is a one-line site to improve later, and the deferral is
  visible in the diff when it happens. The unbounded risk is the opposite — the flattening happening
  so early and so quietly that nobody remembers the typed form was ever specified. That is what
  §3.3 and this ADR are for.

#### Alternatives considered

| Option | Why not chosen |
|---|---|
| **(a) Add `reqwest`, `tokio-tungstenite` and `rusqlite` in work unit 1A, to match §3.3 exactly** | This pulls a **TLS stack**, a **WebSocket client** and a **C SQLite build** into a work unit whose entire content is type definitions and a wire-protocol crate. It contradicts the phase sequencing in `PLAN.md` §8 — where these crates are Phase 3 and Phase 4 work, each with its own exit criteria — and `AGENTS.md` §7.2 criterion 5, which requires compile-time impact to be measured before a dependency is added: `AGENTS.md` §2.3's "measure, don't guess" cannot be satisfied for a TLS stack that landed as a side effect of an error enum. The compliance gained is three payload types; the cost is a C toolchain requirement, a materially larger build, and a §7.2 audit for three crates that have no user yet. Rejected. |
| **(b) Omit the three variants until their crates exist** | `PLAN.md` §6's protocol needs `Network` and `Protocol` **from the first commit**: the wire boundary's whole job is to reduce a `WireError` into the domain vocabulary, and a `ShNexusError` with no `Network` variant cannot represent a connection failure — which is the single most common failure a chat client has. Beyond the immediate need, an enum that **grows variants as phases land** makes every `match` site non-exhaustive, so each later phase breaks every earlier `match` on an unrelated concern, and each break is a compiler error that a reader must triage against the phase that caused it. Adding variants is a breaking change to every consumer; adding them once, up front, is not. Rejected. |
| **(c) Keep `String` permanently** | A **permanent** loss of the source error, and it directly weakens `AGENTS.md` §7.5's observability — `error!` exists to record errors affecting functionality, and a log line that cannot reach `reqwest::Error::source()` records a rendered sentence rather than a cause. `AGENTS.md` §1 lists Reliability second and states the project's priorities in order; an error type that discards its cause is a reliability cost paid on every failure, forever, in exchange for avoiding a two-line diff three times. Rejected. The deferral is a **sequencing** decision with an end state, not a preference for the weaker type. |

---

## Appendix — Defects found in `AGENTS.md`

The constitution's own issues, surfaced during this work. **Not this file's to fix** — the
constitution is not modified from a subordinate document (ADR-004) — but recorded here so they are
not rediscovered later, and so an amendment pass has a queue to work from.

1. **§4.1 contradicts §6.1 on the same metric.** §4.1 requires `core/` ≥90% coverage; §6.1's table
   lists 85% minimum / 95% target for "Test coverage (`core/`)". Two figures for one requirement.
   **Resolution taken:** the stricter **90%** is binding (ADR-004, decision 2). §6.1's row is read as
   a workspace-level summary rather than a competing figure.

2. **§1's platform description is incomplete, and the omission is what caused ADR-001's central
   confusion.** §1 attributes Windows rendering to "DirectX" and does not mention that **windowing
   uses Win32** and **text uses DirectWrite**. The DirectX 11 claim is correct for *rendering* — the
   Windows backend is D3D11 — but a reader who takes "DirectX" as the whole platform picture
   confuses the renderer with the windowing and text stacks. The same sentence names Vulkan for
   Linux, where `gpui_wgpu` is in fact wgpu-based; the macOS "Metal" claim is correct. Upstream's
   Windows path is **D3D11 + Win32 + DirectWrite**.

3. **§12's `ls -lh target/release/sh_nexus` is Unix-only**, in a project whose §5.2 requires manual
   QA on **Windows, macOS and Linux**. The command works on two of the three required platforms and
   produces `command not found` on the third. A cross-platform equivalent is needed
   (`Get-Item target\release\sh_nexus.exe | Select-Object Length` on Windows/PowerShell). §6.1's table
   has the same `ls -lh` problem in its Tool column. §6.2's `/usr/bin/time -v` is Unix-only for the
   same reason.

4. **§4.1's coverage floors are keyed to client directory names** — `core/`, `network/`, `state/`,
   `db/`, "utilities" — and therefore do not cover `sh_nexus_wire` or `sh_nexus_server`, which contain
   none of those directories. §4.2's "serde round-trips for every wire format" mandate lives in the
   wire crate, so a literal reading of §4.1 exempts the most protocol-critical code in the project
   from any floor. **Extension recorded in ADR-004** (both new crates ≥80%); the constitution should
   say so rather than leaving the gap implicit.

5. **§7.3's "uniform row estimator" is likely wrong for variable-height chat lists.** Markdown and
   code blocks make row heights genuinely unpredictable, so a uniform estimator produces incorrect
   scroll positions and jank against §1's zero-jank priority and §6.2's <8ms frame-time target. The
   rule is correct for fixed-height lists and wrong for this one. **ADR-006 proposes the amendment**
   and is **Proposed, not Accepted**; `PLAN.md` §8, Phase 2 complies with §7.3 as written in the
   meantime.

Additionally, and outside the numbering above because it affects no implementation: the **note under
§6.1's table** — "Note: GPUI statically links the renderer" — is **factually false** and was the
origin of the refuted hypothesis in ADR-005. The binary-size row itself is correct and currently
met; the note is what needs correcting. See ADR-005.

---

### ADR-008 - Adopting `rstest` for parameterized tests

**Status:** Accepted.

#### Context

`AGENTS.md` §4.3 mandates it: *"Use `rstest` or `test-case` for parameterized tests."*
Work unit 1A did not comply. It wrote table-driven `for (name, case, expected)`
loops over named case tables instead, on the reading that §7.2's first criterion -
"verify no solution exists with current dependencies or the std library" - governs,
and a `for` loop is a std solution.

**That reading was wrong, and the error is worth recording because it is the same
error this project made twice already.** Rev 2 of `PLAN.md` silently negated
`AGENTS.md` §7.3's "uniform row estimator" mandate, and Rev 2 also declared
§6.2's merge-blocking thresholds to be "targets, not gates". In both cases a
subordinate document judged the constitution technically naive and overrode it.
The conformance audit of Rev 2 named that pattern as the sharpest evidence that
the reconciliation was not settled. Allowing it a third time, on a tooling
preference, would make the same point three times over.

The two sections do not actually conflict. §7.2 is a *process*: it governs
adding a dependency, and its first criterion asks whether a solution already
exists. §4.3 is a *technique*: it names the solution. A `for` loop does exist in
std, but the constitution has already answered the question of which solution to
use, and §7.2's job is then to record *why* the dependency is justified - which
here is "the constitution mandates it", the strongest possible answer to
criterion 1.

#### Decision

Add `rstest` as a dev-dependency of both workspace crates and use it for
parameterized tests from work unit 1B onward.

The §7.2 audit, for the record:

| Criterion | Finding |
|---|---|
| 1. No std solution | The constitution names this one. `AGENTS.md` §4.3. |
| 2. Maintained | 0.27.0 published 2026-09-06; prior release 0.26.1 in July 2025. Actively maintained. |
| 2. >500 downloads/month | 28,006,626 in the last 90 days. |
| 3. License | `MIT OR Apache-2.0`, compatible with this project's MIT. |
| 4. Compile-time impact | **One new compile unit**: `rstest_macros`, a proc-macro crate. The other two runtime dependencies (`futures-timer`, `futures-util`) are optional and stay off - this project has no async tests. Crate size 57,880 bytes. |
| 5. Justification comment | Present above the declaration in the root `Cargo.toml`. |

MSRV is 1.85.0; the toolchain is 1.98.1.

#### Consequences

The 1A test suite is **not** rewritten. It is 4,600 lines of green tests whose
cases are already named in their assertion messages, and converting them to
`#[case]` attributes is a large diff that changes no behaviour and risks no
defect. `docs/COVERAGE.md` records the grandfathering. The rule applies from 1B.

The benefit is real rather than cosmetic. A `for` loop over twenty cases reports
as one test that failed at some index; `#[rstest]` with `#[case]` reports
twenty distinct tests, so a failure names the case that broke and the other
nineteen still show as passing. For a suite with a coverage floor attached to it,
that difference in signal is worth one proc-macro dependency.

The honest cost: `rstest` will appear in this codebase's test style and not in
`AGENTS.md`'s examples of it, and a reader who knows only §4.3 will not find an
example of the mandated form until they reach 1B.

#### Alternatives considered

- **`test-case`** (the other crate §4.3 names) - rejected as less actively
  maintained and far smaller; `rstest` is the one the ecosystem uses and the one
  with the download volume to satisfy §7.2's threshold comfortably.
- **Keep std `for` loops and record a standing deviation** - rejected. It is the
  third instance of the same override, and a deviation that is always available
  is not a deviation, it is an amendment this project is not allowed to make.
- **Rewrite 1A's suite to match** - rejected as churn with no behavioural gain.
  Grandfathered and recorded instead.

### ADR-009 - One main thread, by construction rather than by lock

**Status:** Accepted.

#### Context

`AGENTS.md` 7.3 requires `cx.set_global()` / `cx.global::<T>()` for app-wide
state, and 3.2 puts the mutations in `state/`. Work unit 1C-2b then decided that
`core/cache.rs` needs **no `Mutex`**, and rested that on two pillars. Only one of
them survives contact with the pinned revision.

`core/cache.rs` 3 rejected `Rc<K>` because "a cache behind an `Rc` could not be
held by `gpui::Global`". **That is false.** At rev `e683fd7`,
`crates/gpui/src/global.rs:22` is

```rust
pub trait Global: 'static {
    // This trait is intentionally left empty, by virtue of being a marker trait.
```

with upstream's own comment saying so. There is no `Send` bound and no `Sync`
bound, and the storage is `TypeIdHashMap<Box<dyn Any>>` (`app.rs:796`). An `Rc`
**can** be held as a global. `Send + Sync` was this crate's choice, not a platform
requirement.

**The second pillar is true, and stronger than 1C-2b wrote it.** `Context<'a, T>`
holds `&'a mut App` (`gpui/src/app/context.rs:22`), and `App` holds
`Weak<AppCell>` (`app.rs:748`), `Rc<dyn Platform>` (`app.rs:749`) and
`Rc<ActionRegistry>` (`app.rs:752`). `Rc` is not `Send`, so `App` is not `Send`,
so `Context` is not `Send`. **A worker thread cannot call `cx.update_global`; the
program does not compile.** `AGENTS.md` 7.3 is enforced by the compiler here, not
by a code review.

**What that does not give, stated exactly.** The compiler prevents crossing the
*context*, not the *value*. `AppState` remains `Send + Sync` as a matter of fact,
and a module that built its own state and applied events to it on a worker thread
would still compile. Work unit 1E-1 already recorded this and guarded it with
three tests; 1E-2 is where the question becomes answerable, because until
`state/bridge.rs` existed every caller of `actions.rs` had to be trusted with the
invariant and there were going to be a dozen of them.

#### Decision

`state/bridge.rs` becomes the only owner of `cx.update_global` and `cx.update`,
and it reaches the state through exactly one door. Three mechanisms, each doing a
different job:

**1. `AppStateGlobal` holds a `Receiver`, so the global is `!Sync`.** The state is
reachable only through a `gpui::Global`; a global is reachable only through
`&App`; and `&App` is only obtainable from a main-thread context. With the
`Receiver` inside, no other thread can hold a reference to the state even if one
wanted to. This is the piece 1C-2b was reaching for and did not have.

**2. A bounded channel carries events, and values cross while state stays.**
`EventSender` is `Send + Sync + Clone` and holds nothing but the sender half. It
has no method returning a reference to anything this crate owns, so a worker
thread can put a finished description into a slot and can do nothing else.

The reason the *event* may travel and the *state* may not is not tidiness.
Whether a message increments its channel's unread count depends on **which
channel was selected at the moment it was applied** (`state/app_state.rs` 5,
condition 2). That is a fact about the main thread at one instant, and it is
inside no event. An implementation that applied events to a *copy* of the state on
a worker thread would have to decide each unread count against a `selected` that
may already be stale and then merge the copy back - a lost-update race with no
compiler protection and no reproducible failure. The same argument covers the
rendered-segment cache, whose recency order is a statement about what the user is
looking at *right now*.

**3. No `Mutex`, and the distinction is the point.** A `Mutex<T>` is a *shared
handle to mutable state*: two threads hold one object and may both enter. A
bounded channel is a *queue of owned values*. The `AppState` in this module is
never behind anything a worker thread owns.

#### Consequences

**A lock would have been three separate mistakes at once.** It would make a
second thread *possible*, and the invariant is "one owner, one thread" - a
`Mutex<AppState>` is not a cheaper way to hold that invariant, it is standing
permission to stop holding it. It would sit in front of every state mutation, on
the path `AGENTS.md` 6.2's 8ms scroll-frame budget measures. And it would need a
poisoning policy, which 2.1 forbids a module like this from having.

**The residual gap was named here, and is now closed. Both the closure and the
reason this ADR's own proposed answer was wrong are worth keeping.**

ADR-009 left open the case of a module that is *handed* an `AppState` by value,
and proposed closing it by making `AppState::new` crate-private. **That answer
would have moved the gap rather than closing it:** the integration tests in
`tests/` are a *separate crate*, so a crate-private constructor breaks the six
call sites in `tests/state_actions.rs` that legitimately build a state to drive
`actions::apply_event` with.

**What closed it instead is one condition, not a visibility change:** no file
under `src/` outside `state/` may name the type `AppState` at all
(`no_module_outside_state_names_the_state_type` in `tests/bridge.rs`). That
forecloses all three doors in one rule — constructing one, holding one as a
field, and taking `&mut` to one — and it leaves the external test API untouched.
The rule is verified to fire by a mutation, not merely to pass.

**What still stands open is narrower, and it is not "nothing".** The confinement
is now a build failure for every shape that *owns* the state off the main
thread. It is still not a type-level guarantee: `AppState` is `Send + Sync` as a
matter of fact, and `bridge::try_read` hands an `&AppState` to a closure. That
borrow is main-thread by construction, so a closure outliving the call is the
next shape to rule out — and it is not what these guards rule out. Closing that
too would mean changing what `try_read` hands out, which is a different
decision about a different file.

**The queue is bounded, and its cost is declared.** `AGENTS.md` 7.1 forbids
unbounded in-memory state, and an unbounded `mpsc::channel` would break it in the
one place a remote peer can outrun this client, so the bridge uses `sync_channel`
and `try_send`. A legitimate burst larger than `MAX_PENDING_EVENTS` loses the
events past the bound, each reported to its producer as `DeliveryRefusal::InboxFull`
**with the event handed back**. A client that hits that has a rendering problem,
and the refusals are how it becomes visible instead of becoming a quietly stale
sidebar.

**The absence of a general mutator is the feature.** There is no
`with_state_mut`. 3.2 asks for every mutation to be auditable, and 1E-1 achieved
that with `pub(crate)` mutators; one public `&mut AppState` would have undone it
behind a single door.

**The one guarantee that is a compile error rather than a rule is asserted in both
directions** and cannot be lost silently when GPUI is re-pinned:
`a_main_thread_context_cannot_be_sent_to_another_thread` checks `App`,
`Context` and `AsyncApp` are `!Send` and that a `u32` is `Send` (so the probe
itself is not vacuous), and a `compile_fail` doctest on `install` demonstrates the
failure. That doctest carries a **control block**: a `compile_fail` doctest passes
for *any* compile error, so a typo in the failing block would make it pass for the
wrong reason, and the `no_run` twin proves the only difference is the thread.

#### Alternatives considered

- **Rely on `AppState: Send + Sync` as the safety property** - rejected. It was
  never a property; it is a fact about a type whose confinement is a rule, and
  `the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee` says
  so in the test name.
- **`Mutex<AppState>`** - rejected on the three counts above.
- **Applying events to a copy on a worker thread and merging back** - rejected as
  the lost-update race it is, in the unread counter specifically.
- **An unbounded channel** - rejected by 7.1; it converts a slow render into
  unbounded growth instead of a visible refusal.
- **Promoting the confinement to the type system** - the correct long-term move
  and **not attempted here**, because it means making `AppState::new`
  crate-private and reworking a file 1E-1 owns. Named as the next step rather
  than half-done.

