# Sh_Nexus

A native, GPU-rendered team chat client (Slack-like) written in Rust with
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), Zed's UI framework. The UI is
a declarative element tree rendered directly to the GPU — no JavaScript, no web views, no Electron,
no runtime garbage collector. Real-time delivery runs over WebSocket with optimistic send, an
offline outbox, and per-channel resume-from-cursor; local history is cached in SQLite. The backend is
a Rust + Axum service in the same Cargo workspace, so client and server compile against one shared
wire crate and the protocol cannot drift.

GPUI is consumed as an Apache-2.0 git dependency from Zed Industries, pinned to a specific `rev`.
`PLAN.md` is the implementation plan, `AGENTS.md` is the project constitution, and
`docs/ARCHITECTURE.md` holds the Architecture Decision Records.

---

## Prerequisites

### Rust

Rust **stable**, no nightly features and no `RUSTC_BOOTSTRAP`. The pinned workspace was built and
measured with **rustc 1.98.1** (`48a229cea`, 2026-09-01); all figures quoted in this README come
from that toolchain.

The toolchain is now **1.99.0** (raised 2026-10-02 and re-verified clean), so every figure below
is one minor version behind the compiler rather than measured on it. The measurements have not
been retaken on 1.99.0, and this line is here so that gap is stated instead of assumed.

```powershell
rustup update stable
rustc --version   # expect 1.99.0 or newer
```

### Windows toolchain

- **MSVC Build Tools 2022** (Visual Studio Build Tools, workload *Desktop development with C++*)
- **Windows SDK** (10.0.26100.0 at the time of measurement)

### A shader compiler — REQUIRED FOR RELEASE BUILDS ONLY

**`fxc.exe` is not optional. Without it `cargo build` succeeds and `cargo build --release` fails
outright.** This is a toolchain requirement, not a code defect, and it is the single most common
first-run failure on this project.

`crates/gpui_windows/build.rs` in the pinned GPUI checkout compiles HLSL under
`#[cfg(not(debug_assertions))]` and panics with `"Failed to find fxc.exe"` when it cannot locate a
compiler. A debug build needs no shader compiler at all, so the two commands disagree: the same
machine, the same source tree, one succeeds and one dies. **The failure mode is this:** a plain
`cargo build` is green, `cargo test` is green, and then the release build stops with a build-script
panic that reads like a bug in the shader or the renderer. It is not. It is a missing tool.

`fxc.exe` ships with the **Windows SDK** — no separate download, no Visual Studio C++ workload for
the shader itself, and no C++ or shader toolchain beyond this one binary.

Resolution order inside `gpui_windows/build.rs` (`find_fxc_compiler`):

1. The **`GPUI_FXC_PATH`** environment variable, if set and the path exists.
2. **`where.exe fxc.exe`** on `PATH`.
3. The **newest installed Windows SDK**, discovered through the registry key
   `HKLM\SOFTWARE\WOW6432Node\Microsoft\Microsoft SDKs\Windows\v10.0\InstallationFolder`
   (value `InstallationFolder`), then `bin\<highest version>\<arch>\fxc.exe` where `<arch>` is
   `x64` or `arm64`.

If all three miss, the build script panics. Pin the path explicitly when step 3 is unreliable:

```powershell
$env:GPUI_FXC_PATH = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe"
```

**This belongs in CI setup.** A release job that does not provision the Windows SDK will fail on
the build script, not on the code, and the log will point at `gpui_windows` rather than at the
runner image.

### What is *not* needed

`gpui_windows` is **pure Rust over the `windows` crate**, with a hand-written **D3D11** renderer and
**DirectWrite** text shaping. There is no Vulkan, no DXC, no DXIL signing, no C++ or HLSL toolchain
beyond `fxc.exe`, and no `blade` or `shaderc`. Upstream's shader targets are `vs_4_1` / `ps_4_1` —
Direct3D 11 shader model 4.1 — which is precisely why FXC rather than DXC is the tool. The crate's
only build-dependency is `windows-registry`, used to read the SDK path.

---

## Build and test

```powershell
# Debug build — no shader compiler required
cargo build

# Release build — REQUIRES fxc.exe (see Prerequisites)
cargo build --release

# Test suite
cargo test

# Lints — CI fails on any warning
cargo clippy --all-targets -- -D warnings

# Format check
cargo fmt --check

# Run the app
cargo run
```

For development logging, `PLAN.md` §3 installs a `tracing` subscriber in Phase 1:

```powershell
$env:RUST_LOG = "debug"; cargo run
```

`AGENTS.md` §12 is the canonical pre-commit command set:

```powershell
cargo check; if ($?) { cargo clippy -- -D warnings }
```

Full checklist before calling any work unit done: `AGENTS.md` §5.1.

---

## Platform gotchas

Three findings from the Phase 0 spike that cost time if you do not know them. All three are
recorded in `PLAN.md` §8 and in the risk table at `PLAN.md` §14.

### (a) `fxc.exe` is not on `PATH`, and is only needed for release

**Symptom.** `cargo build`, `cargo test`, `cargo clippy` and `cargo run` all pass. Then
`cargo build --release` stops with a panic from the build script:

```
thread 'main' panicked at ...\gpui_windows\build.rs:138:
Failed to find fxc.exe
```

**Cause.** `crates/gpui_windows/build.rs` gates shader compilation behind
`#[cfg(not(debug_assertions))]`. Debug builds skip it entirely; release builds run it, and
`find_fxc_compiler` searches `GPUI_FXC_PATH`, then `where.exe fxc.exe`, then the newest SDK via the
registry key `HKLM\SOFTWARE\WOW6432Node\Microsoft\Microsoft SDKs\Windows\v10.0\InstallationFolder`.
`fxc.exe` is not on `PATH` by default even with the SDK installed, so the first two probes fail
often enough to be the common case.

**This does not look like a code bug, and it is not one.** It is a missing toolchain component.
The confusing part is that the same checkout builds fine in debug, so the code is demonstrably
correct and the release profile is demonstrably broken — a combination that sends you hunting in
the wrong place.

**Fix.** Install the Windows SDK (it carries `fxc.exe`), or set `GPUI_FXC_PATH` to an explicit
`fxc.exe` before building:

```powershell
$env:GPUI_FXC_PATH = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe"
cargo build --release
```

In CI, provision the Windows SDK on the runner — or set `GPUI_FXC_PATH` — as part of environment
setup, not as a build step.

### (b) A headless test can pass while asserting nothing

**Symptom.** A test using `cx.debug_bounds("x")` compiles, runs, and is green. It asserts nothing
at all. This is a **green build over a void**, and it is the same class of failure as the Cargo
test-discovery trap in a different disguise.

**Cause.** `.id("x")` alone does **not** register debug bounds. `cx.debug_bounds(id)` returns
`None` unless the element *also* carries `.debug_selector(|| id.to_owned())`, a method on the
`InteractiveElement` trait (re-exported through `gpui::prelude`). Only `debug_selector` sets
`Interactivity::debug_selector`, and only that field is read during paint to call
`Frame::record_debug_bounds`.

The recording path is gated behind `#[cfg(any(test, feature = "test-support"))]` in
`Interactivity`, in `Frame::debug_bounds`, and in the paint step — so **reading the type signatures
will not reveal this.** The method exists with a plausible signature in every build; whether it
actually records anything is invisible until a test asserts on the result.

**Fix.** The element must carry both calls:

```rust
div()
    .id("increment")
    .debug_selector(|| "increment".to_owned())
    .on_click(cx.listener(/* ... */))
```

And the test must **assert `Some(..)`, never tolerate `None`.** If `debug_bounds` returns `None`,
that is a test failure, not an absence of content:

```rust
let increment = cx.debug_bounds("increment").expect("hit-testable");
cx.simulate_click(increment.center(), gpui::Modifiers::default());
```

`expect` here is correct and required. Code that reads `if cx.debug_bounds("x").is_some() { /* … */ }`,
or that unwraps-or-skips, converts a real defect into a silent skip.

> **Rule: any test using `cx.debug_bounds` must assert `Some(..)`, never tolerate `None`.**
> Enforce it in code review. `crates/sh_nexus/tests/spike_render.rs` is the reference pattern.

Two further traps on the same API, both present in the spike test:

- **`read_with` cannot return a borrow into the view.** It hands you a temporary `&View`, so
  `view.read_with(cx, |view, _| view.last_key())` will not compile — the reference would outlive
  the closure. Copy the value out: `view.read_with(cx, |view, _| view.last_key().map(str::to_owned))`.
- **`Pixels` is a newtype, not a primitive.** `assert!(size.width > 0)` does not typecheck and
  `assert_eq!(pos, (0, 0))` is meaningless. Compare with `px(..)`: `assert!(size.width > px(0.0))`.

### (c) Visual / image-diffing tests do not work on Windows

**Symptom.** A test built on `VisualTestAppContext` — anything that compares a rendered frame
against a reference image — cannot render on Windows. The headless renderer is simply absent
there.

**Cause.** `gpui_platform::current_headless_renderer()` has three arms: macOS returns
`MetalHeadlessRenderer`, Linux returns `WgpuHeadlessRenderer`, and everything else —
`#[cfg(not(any(target_os = "macos", target_os = "linux")))]` — returns `Ok(None)`. On Windows it
is `Ok(None)`. `TestAppContext` does not need it: it uses GPUI's own pure-Rust `TestPlatform` and
never touches a GPU. Only `VisualTestAppContext`, the image-diffing harness, needs a real
renderer, and that path is macOS/Linux only.

**Consequence for the test strategy.** What works on Windows, and is what this project uses:

- headless render and element layout (real `Bounds<Pixels>`);
- simulated pointer and keyboard input reaching the element tree and mutating state;
- assertions on state, focus, layout and accessibility.

What does **not** work on Windows:

- **screenshot / pixel comparison.** No renderer, no comparison.

**Recommended strategy — which is what `PLAN.md` §10 already specifies:** render headlessly, drive
pointer and keyboard input, and assert on **state, focus, layout and accessibility** with real
assertions. Do not plan screenshot tests as the regression gate; they are unavailable on the
primary development target, so a gate that depends on them is a gate that is never enforced.

The Phase 0 spike's four tests are exactly this shape and all four pass on Windows.

---

## Measured baseline

Phase 0 spike, recorded 2026-09-27 on Windows (12 logical CPUs, MSVC Build Tools 2022, Windows SDK
10.0.26100.0, rustc 1.98.1), commit `491bd0f` on `spike/gpui-windows`. Full discussion in
`PLAN.md` §11.

| Metric | Measured | `AGENTS.md` §6.1/§6.2 threshold | Verdict |
|---|---|---|---|
| Release binary | **9.92 MB** (10,401,792 B) | <20 MB target, <30 MB max | **PASSES both** — 3× under the max, 2× under the target |
| Idle RAM | 48.6 MB working set / 41.5 MB private | <80 MB | **PASSES** |
| Idle CPU | 2.42% of one core (20 s sample) | <2% | **Misses** — sample too short to count; re-measure over ≥30 min per §5.2 |
| Dev cold build | 2m 12s | <60 s target, <120 s max | **Exceeds max** — cold figure on a 12-thread machine; re-measure in CI |
| Release cold build | 3m 10s | <3 min target | **Just over** — same caveat as above |
| `Cargo.lock` packages | 675 | — | Informational |
| Compile units (release) | 424 | — | Informational |

**The release binary is 9.92 MB**, which comfortably passes both the <20 MB target and the <30 MB
maximum in `AGENTS.md` §6.1.

This refuted a standing hypothesis. `PLAN.md` Rev 2 predicted the binary would land in the tens of
megabytes on the assumption that GPUI statically links its renderer, called the 30 MB ceiling
unachievable, and pre-drafted a relief ADR. It does not. `gpui_windows` is pure Rust with a
hand-written D3D11 renderer and DirectWrite text — no `blade`, no `shaderc`, no Vulkan. See ADR-005
in `docs/ARCHITECTURE.md`, recorded as **withdrawn**.

The still-open measurement is the *delta* from the dependencies the spike did not link — `syntect`,
`reqwest` with TLS, bundled SQLite, `rodio`, `notify-rust`. That gets measured in Phase 3, when
those crates actually land. Until then `AGENTS.md` §6.1's numbers stand as written.

---

## Project layout

`AGENTS.md` §3.1's module tree, applied **per crate** inside a Cargo workspace (ADR-003). Full tree
in `PLAN.md` §4.

```
Sh_Nexus/
├── Cargo.toml                 # workspace root; GPUI pinned to rev e683fd7b
├── Cargo.lock
├── AGENTS.md                  # the constitution — binding
├── PLAN.md                    # implementation plan — subordinate to AGENTS.md
├── LICENSE
├── crates/
│   ├── sh_nexus/              # the client binary
│   │   ├── src/
│   │   │   ├── main.rs        # entry point only; max ~50 lines
│   │   │   ├── app.rs         # root component, global state, theme, key handling
│   │   │   ├── errors.rs      # ShNexusError (AGENTS.md §3.3)
│   │   │   ├── core/          # PURE logic. No gpui, no tokio, no I/O, no filesystem.
│   │   │   │   ├── models/    # user, channel, message, events
│   │   │   │   ├── markdown.rs
│   │   │   │   ├── cache.rs   # LRU with a memory ceiling
│   │   │   │   ├── ordering.rs
│   │   │   │   └── theme.rs   # parse + validate only; no file I/O
│   │   │   ├── ui/            # presentation only. No business logic.
│   │   │   │   ├── components/
│   │   │   │   ├── views/     # sidebar, chat, thread panel, input bar
│   │   │   │   └── theme/     # theme application + hot-reload wiring
│   │   │   ├── state/
│   │   │   │   ├── app_state.rs   # unread counts, DeliveryState, selection
│   │   │   │   ├── actions.rs     # ALL mutations go through here
│   │   │   │   └── bridge.rs      # the ONLY owner of cx.update_global
│   │   │   ├── network/       # protocol only. No GPUI imports whatsoever.
│   │   │   │   ├── rest.rs
│   │   │   │   ├── websocket.rs
│   │   │   │   ├── auth.rs
│   │   │   │   └── reconnect.rs
│   │   │   ├── db/            # persistence only
│   │   │   │   ├── schema.rs
│   │   │   │   └── repository.rs
│   │   │   └── platform/      # OS-specific behind traits
│   │   │       ├── notifications.rs
│   │   │       ├── sounds.rs
│   │   │       ├── keychain.rs
│   │   │       └── file_watch.rs
│   │   └── tests/             # FLAT: Cargo only auto-discovers tests/*.rs
│   ├── sh_nexus_wire/         # wire DTOs — single source of truth for the protocol
│   └── sh_nexus_server/       # Axum backend
├── assets/
│   ├── sounds/notification.wav
│   └── themes/                 # 3 built-in themes: dark, light, high-contrast
└── docs/
    ├── ARCHITECTURE.md         # ADRs (AGENTS.md §9.2)
    ├── API.md                  # protocol + version negotiation
    └── DEPENDENCIES.md         # the §7.2 audit, one row per crate
```

> **Test layout is not cosmetic.** Cargo auto-discovers `tests/*.rs` and `tests/*/main.rs` **only**.
> A file at `tests/integration/login_flow.rs` is silently never compiled or never run — a green
> build with all ten mandatory integration flows from `AGENTS.md` §8.1 missing. The integration
> tests are flat and named for their flows.

---

## Status

| Phase | Scope | State |
|---|---|---|
| **0** | GPUI spike on Windows — build, window, headless test harness | **Complete** — PASSED, commit `491bd0f` (`spike/gpui-windows`) |
| **1** | Foundations — `errors.rs`, `sh_nexus_wire`, `core/`, `state/`, `docs/DEPENDENCIES.md` | **Next** |
| 2 | Core UI — app shell, virtualized message list, theming, keyboard-only operation | Pending |
| 3 | Persistence — SQLite schema, migrations, outbox, dependency-delta re-measurement | Pending |
| 4 | Networking — Axum backend, WebSocket, reconnection, timeouts | Pending |
| 5 | Rich features — a chosen subset, not all nine | Pending |
| 6 | Polish — themes, notifications, sounds, settings | Pending |

Phase 0 passed its gate: a window opens on Windows from a clean checkout at the pinned rev, and
4/4 headless tests render and accept simulated input. `cargo build`,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test` and
`cargo build --release` are all green. ADR-001 holds; no fallback was needed.

**Next up is Phase 1 (foundations)** under strict TDD for `core/` and `sh_nexus_wire`. Its exit
criterion is `docs/DEPENDENCIES.md` carrying the `AGENTS.md` §7.2 audit, one row per crate.

Read next:

- **[`PLAN.md`](PLAN.md)** — the implementation plan. §8 records the Phase 0 gate and its four
  findings; §11 records the measured baseline.
- **[`AGENTS.md`](AGENTS.md)** — the constitution. Binding on every contribution.
- **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** — ADR-001 through ADR-006.

---

## License

**MIT.** See [`LICENSE`](LICENSE), Copyright (c) 2026 Adrián Quirós.

GPUI is consumed as an **Apache-2.0 git dependency** from Zed Industries, verified against the
upstream repository root `LICENSE` and `crates/gpui/LICENSE-APACHE` ("Copyright 2022-2025 Zed
Industries, Inc."). It is **not vendored** — no upstream source is redistributed inside this
repository — so pulling it as a Cargo dependency triggers no additional notice obligation. Apache-2.0
is MIT-compatible and carries no copyleft terms.

Note: some third-party crate aggregators report Zed as GPL-3.0-or-later. That claim is false. Do
not source license facts from aggregators; check the upstream repository.
