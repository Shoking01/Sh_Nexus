# Sh_Nexus — Measured Coverage Baseline

This file records the **measured** coverage of the workspace as of work unit 1A, so the numbers
are not lost and later work has something concrete to regress against.

`AGENTS.md` §2.3: *"Profile before optimizing. Measure, don't guess."* `AGENTS.md` §6.3 makes a
coverage drop merge-blocking, which requires a baseline to drop *from*. This file is that baseline.

It is a **record of measurement, not a claim of compliance.** Where a floor cannot yet be measured,
§3 says so explicitly rather than letting an absent row read as a pass.

**Recorded:** 2026-09-27 · work unit 1A · Windows, MSVC, rustc 1.98.1

---

## 1. How it was measured

**Tool: `cargo llvm-cov` 0.9.1**, with the `llvm-tools-preview` rustup component installed.

**Toolchain:** rustc 1.98.1 (MSVC), Windows. 12 logical CPUs, MSVC Build Tools 2022, Windows SDK
10.0.26100.0 — the same environment as the `PLAN.md` §11 Phase 0 performance baseline.

```text
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.1 --locked
cargo llvm-cov --workspace --all-targets --summary-only
```

### Tool substitution — `cargo-tarpaulin` → `cargo llvm-cov`

`AGENTS.md` §4.1 names **`cargo tarpaulin`** as the coverage tool, and `PLAN.md` §10 inherits the
name. **`cargo-tarpaulin` was not available in this environment** and the measurement was taken with
`cargo-llvm-cov` instead. The substitution is recorded here rather than made silently, because a
different instrumentation backend can produce a different denominator:

| | `cargo tarpaulin` (named by §4.1) | `cargo llvm-cov` (used) |
|---|---|---|
| Instrumentation | `llvm-profdata` / ptrace (Linux), `-C instrument-coverage` | LLVM source-based coverage, `.profraw` + `.profdata` |
| Denominator | per-crate, per-test | workspace-wide, region-based |

Both are LLVM source-based coverage instruments, so the *definition* of a covered region is the
same. The **set of instrumented regions is not guaranteed to be identical**, so this baseline must
be re-measured with the same tool before it is compared against a number produced by the other.
`PLAN.md` §10's `cargo tarpaulin` reference should be corrected through the amendment path in the
same way `AGENTS.md` §6.1's false binary-size note is handled (`docs/ARCHITECTURE.md`, Appendix) —
`AGENTS.md` is not edited from a subordinate document.

**Every figure in this file is from the `cargo-llvm-cov` run above.** No figure is estimated,
interpolated, or rounded.

---

## 2. Measured results

### 2.1 Per file

| File | Regions | Missed regions | Region cover | Missed lines | Line cover |
|---|---|---|---|---|---|
| `sh_nexus\src\lib.rs` | 148 | 58 | 60.81% | 32 | 67.68% |
| `sh_nexus\src\main.rs` | 7 | 7 | 0.00% | 7 | 0.00% |
| `sh_nexus\src\network\mapping.rs` | 331 | 2 | 99.40% | 0 | 100.00% |
| `sh_nexus_wire\src\error.rs` | 4 | 0 | 100.00% | 3 | 100.00% |
| `sh_nexus_wire\src\frame.rs` | 169 | 10 | 94.08% | 6 | 95.68% |
| `sh_nexus_wire\src\version.rs` | 17 | 4 | 76.47% | 3 | 82.35% |
| **TOTAL** | **676** | **81** | **88.02%** | **48** | **90.28%** |

Paths are as the tool reports them — Windows separators, workspace-relative to each crate.

### 2.2 Crate aggregate for `sh_nexus_wire`

| Aggregate | Region cover | Line cover |
|---|---|---|
| **`sh_nexus_wire` as a crate** | **92.63%** (176/190 regions) | **94.34%** (200/212 lines) |

This is the figure ADR-004's `sh_nexus_wire` floor is stated against, and §3.2 below tests it.

### 2.3 Files that do not appear in the report at all

Their absence is **not** a 0% and **not** a pass. These files contain no executable code — only
structs, enums, derives and `pub mod` declarations — so the instrumenter emits zero coverage regions
for them and they produce no report row at all:

- `sh_nexus\src\core\mod.rs`, `sh_nexus\src\core\models\mod.rs`
- `sh_nexus\src\core\models\user.rs`, `channel.rs`, `message.rs`, `events.rs`
- `sh_nexus\src\network\mod.rs`
- `sh_nexus_wire\src\lib.rs`, `sh_nexus_wire\src\dto.rs`

Each was read and confirmed to contain no `fn` and no `impl` block. The absence is a **true zero for
executable code**, not a measurement gap — but see §3.3, which is a real gap.

### 2.4 Reconciliation note on the line column

The **region** column reconciles exactly and is the column to gate on:

```text
regions:      148 +   7 + 331 +   4 + 169 +  17 =  676
missed:        58 +   7 +   2 +   0 +  10 +   4 =   81
covered:      676 − 81 = 595        595 / 676 = 88.02%
```

The **line** column does **not** reconcile, and is recorded here verbatim rather than corrected:

1. **The per-file missed-line figures sum to 51, not the 48 in the TOTAL row**
   (`32 + 7 + 0 + 3 + 6 + 3 = 51`). Within `sh_nexus_wire` alone the line column *does* reconcile
   (`3 + 6 + 3 = 12`), so the discrepancy is between the per-file rows and the workspace TOTAL.
2. **`sh_nexus_wire\src\error.rs` reports 3 missed lines at 100.00% line cover.** Those two figures
   cannot both be true of one denominator. `error.rs` is 100.00% on regions (4/4) and its single
   function `is_fatal` *is* exercised — including by the doctest on line 146 — so the 3 "missed
   lines" are almost certainly non-region-bearing lines (braces, the `matches!` continuation) being
   counted under a different rule than the region rows. That is an inference, not a verified cause.

**Consequence for the baseline.** ADR-004's floors are stated as region coverage, and the region
column reconciles to the last decimal, so **§3's verdicts rest on regions and are unaffected**. The
line column is recorded for completeness only. It must be re-derived from a single report before it
is used as a §6.3 regression gate, or the gate will compare two different denominators and report a
phantom regression. This is a finding about the measurement, not about the code.

---

## 3. Verdict against each floor

### 3.1 `AGENTS.md` §6.1 — workspace total: 75% minimum / 85% target

**88.02% region coverage — PASSES THE TARGET.** 3.02 points above the 85% target and 13.02 points
above the 75% minimum. The line column reads 90.28% and points the same way.

### 3.2 ADR-004 — `sh_nexus_wire` ≥80%

**92.63% region coverage — PASSES.** 12.63 points above the floor. See §2.2. This is the floor that
matters most (§3.4), because `AGENTS.md` §4.2's serde round-trip mandate and §7.4's version
rejection both live in this crate.

### 3.3 `AGENTS.md` §4.1 — `core/` ≥90%: **NOT YET MEASURABLE**

**This floor cannot be evaluated, and that is a finding, not a pass.** Stating it plainly so no later
reader mistakes the absence of `core/` rows for a green result.

- **What `core/` contains today is type definitions only.** `core/models/user.rs`, `channel.rs`,
  `message.rs` and `events.rs` hold structs, enums and derives. `core/`'s **executable logic does
  not exist** — `core/ordering.rs`, `core/markdown.rs`, `core/cache.rs` and `core/theme.rs` are
  Phase 1 work, units **1B** and **1C**. Type definitions emit zero coverage regions (§2.3), so the
  90% floor has **no denominator to be measured against** until 1B and 1C land.
- **The floor becomes measurable then, and it is the strictest in the project.** 90% minimum
  (ADR-004 decision 2, which took the stricter of `AGENTS.md`'s two conflicting figures) against a
  pure-logic target that has no excuse available: `PLAN.md` §8, Phase 1 states *"Strict TDD applies
  to `core/` and `sh_nexus_wire` (pure, ≥90% floor, no excuses)."* `PLAN.md` §10 requires proptest
  coverage of exactly the two modules that do not exist yet.
- **What can be said today:** the `core/` files that *do* exist introduce no measurable risk, because
  they contain no logic to be wrong. The 90% floor is **deferred to 1B/1C, not waived.** `core/`
  also carries a live architectural obligation from ADR-003: it must stay free of `gpui` and
  `tokio`, and `tests/layer_boundary.rs` and `tests/proptest_boundary.rs` exist to hold that line.

### 3.4 Summary

| Floor | Source | Measured | Verdict |
|---|---|---|---|
| Workspace total ≥75% min / ≥85% target | `AGENTS.md` §6.1 | **88.02%** regions | **PASSES target** |
| `sh_nexus_wire` ≥80% | ADR-004 decision 3 | **92.63%** regions | **PASSES** |
| `core/` ≥90% | `AGENTS.md` §4.1, ADR-004 decision 2 | no denominator | **NOT MEASURABLE — deferred to 1B/1C** |
| New code ≥80% | `AGENTS.md` §5.1, ADR-004 | see §4 | **PASSES on the two files it applies to** |
| `network/` ≥80% | `AGENTS.md` §4.1 | `mapping.rs` **99.40%** regions | **PASSES** |
| `state/`, `db/`, utilities ≥80/85% | `AGENTS.md` §4.1 | directories do not exist yet | Not applicable in 1A |

---

## 4. The sub-floor files, characterised

Four files sit below 80%. Three of them are structural and one is a nine-line residual. None is a
quality problem in the code that was written.

### 4.1 `sh_nexus\src\lib.rs` — 60.81% regions, 67.68% lines

**What it holds:** the Phase 0 spike's GPUI root view (`RootView`) and the window-startup path.

**Uncovered lines:** `96-98`, `167-173`, `185`, `188-190`, `192-195`, `198-200`, `202-204`, `206`,
`210-213`, `215`.

**What is uncovered, precisely — and it is not the `Render` impl.** The `Render` impl (lines
101-164) and **both** of its event handlers — `on_click` at 146-149 and `on_key_down` at 118-121 —
are **covered**, by the headless test in `tests/spike_render.rs`. That is the spike's entire purpose:
render, hit-test a click, deliver a keystroke. The uncovered code is the part that opens a window:

| Lines | What is there | Why it cannot run in a test |
|---|---|---|
| `96-98` | `Focusable::focus_handle` body | Same root cause as `main.rs` — it exists to serve the real window's focus path |
| `167-173` | `spike_window_options()` | Calls `Bounds::centered(None, …, cx)`, which needs a live `App` |
| `185`, `188-190`, `192-195`, `198-200`, `202-204`, `206`, `210-213`, `215` | `run()` in full | `gpui_platform::application().run(…)` **blocks on the platform event loop** and opens a real window |

**This is the same structural limit as `main.rs`, and it is the same reason for both.** One test
process cannot own a second event loop and a real window handle.

**Why it is temporary, not a defect.** This code **moves to `src/app.rs`** per `PLAN.md` §4, built in
`PLAN.md` §8 Phase 2 (App shell). The spike deliberately kept it in `lib.rs` so the spike's
footprint stayed small — `src/lib.rs`'s own module docs say so. Nothing in the gap is a
misunderstanding, a missing assertion, or an untested branch that should have been tested; it is the
window-opening path, which moves. The gap is **expected and temporary.**

### 4.2 `sh_nexus\src\main.rs` — 0.00% regions, 0.00% lines

**What it holds:** the binary entry point — 20 lines, per `AGENTS.md` §3.1's *"Entry point only …
Max ~50 lines"*. `fn main()` maps `sh_nexus::run()`'s result onto an `ExitCode`.

**Structurally untestable.** `main()` is not callable from an integration test; it is the process
entry point, it opens a window, and `run()` blocks on the platform event loop. No amount of testing
changes this — it is a property of being `main`, not of the code.

**No floor in `AGENTS.md` §4.1 applies to it.** §4.1's floors are keyed to `core/`, `network/`,
`state/`, `db/` and utilities. `main.rs` is in none of them. It is reported here so that a **0.00%
row is never read as a failing floor** and never mistaken for a regression against §6.3.

> `AGENTS.md` §5.1 requires *"no `unwrap()`/`expect()` in production paths"* and §3.1 caps `main.rs`
> at ~50 lines. Both are satisfied by reading the file; neither is a coverage matter, and coverage
> cannot demonstrate either one.

### 4.3 `sh_nexus_wire\src\version.rs` — 76.47% regions, below the 80% per-file reading

**The only uncovered block is lines 117-119: the body of
`impl fmt::Display for ProtocolVersion`.** One `write!(formatter, "{}", self.0)`. 4 missed regions
of 17.

**This is a per-file artifact of a passing crate.** `sh_nexus_wire` aggregates to **92.63%**
(§2.2), 12.63 points clear of ADR-004's floor. ADR-004's floor is stated for the **crate**, not for
each file, and correctly so: a per-file floor on a file holding 17 regions would rank a 4-region gap
above a 100-region gap.

The substantive logic in this file **is** covered — `negotiate()` and `UnsupportedVersion::detail()`
both have tests and doctests, and `negotiate` is the *only* gate between a raw frame and
deserialization, per `AGENTS.md` §7.4's "reject unknown major versions explicitly". What is missing
is a `Display` impl, which the type does not currently need at runtime.

### 4.4 `sh_nexus_wire\src\frame.rs` — 94.08% regions, 95.68% lines

**The only uncovered blocks are lines 581-583 and 682-684.** They are **two identical three-line
delegating accessors**:

```rust
// ClientEnvelope, lines 580-583
/// This envelope's frame `type` value.
pub fn kind(&self) -> FrameKind { self.frame.kind() }

// ServerEnvelope, lines 681-684
/// This envelope's frame `type` value.
pub fn kind(&self) -> FrameKind { self.frame.kind() }
```

One on `ClientEnvelope`, one on `ServerEnvelope`. No test calls either, so each is one missed region
body plus one for the enclosing item — 10 missed regions of 169. The 94% is entirely attributable to
six lines of delegation.

The file's real content — frame encode/decode, `v`-header validation, direction checking, UTF-8
rejection — is covered, which is what §4.2's "serde round-trips for every wire format; malformed
payload rejection" mandate requires of this crate.

### 4.5 `sh_nexus\src\network\mapping.rs` — 99.40% regions, 100.00% lines

**The strongest file in the baseline** and the only one with **zero** missed lines. 331 regions, 2
missed. This is the `TryFrom` boundary from `sh_nexus_wire` into `core::models` — the seam ADR-003
names as the thing that must not rot — and it is where `AGENTS.md` §2.1's "validate every incoming
payload before it touches state" is enforced. Recorded here because a baseline that only lists
failures hides the result that matters most.

---

## 5. Carried-forward action

**The exact residual gap is nine lines. Nine.**

| File | Lines | What |
|---|---|---|
| `sh_nexus_wire\src\version.rs` | 117-119 | `impl fmt::Display for ProtocolVersion` — one `write!` |
| `sh_nexus_wire\src\frame.rs` | 581-583 | `ClientEnvelope::kind` — one delegating accessor |
| `sh_nexus_wire\src\frame.rs` | 682-684 | `ServerEnvelope::kind` — one delegating accessor |

All three are untested. **Deferred to work unit 1B, not closed in 1A**, for a stated reason:

- **Both gates that apply already pass.** `sh_nexus_wire` is at 92.63% against ADR-004's 80% floor
  (§3.2), and the workspace total is at 88.02% against §6.1's 85% target (§3.1). Neither is close
  enough to its floor for nine lines of delegation to matter.
- **A separate review cycle for nine lines is not justified.** `AGENTS.md` §5.1 requires coverage of
  new code ≥80%, which `sh_nexus_wire` clears as a crate and as new code. The residual is a
  `Display` impl and two one-line accessors — no branch, no error path, no invariant, nothing that
  can be wrong in a way a test would catch. Spending a review cycle here would be
  `AGENTS.md` §2.3's "measure, don't guess" applied to effort instead of performance.
- **It is recorded rather than forgotten, which is the actual requirement.** A named, line-numbered
  residual that a later unit can close in minutes is worth more than a closed gap that leaves no
  trace of what it was.

**Action for 1B:** add one test per range — a `Display` assertion for `ProtocolVersion` and a
`kind()` round-trip for each envelope direction — which closes the residual and takes
`sh_nexus_wire` to 100% region coverage. **Tracked, not waived.**

### 5.1 Not carried forward

`lib.rs` and `main.rs` are **not** on this list. Their gaps are the window-opening path (§4.1, §4.2),
which Phase 2 restructures into `src/app.rs` and which no test can reach in its current shape. Adding
a test for `spike_window_options` would require a live `App`, and adding one for `main()` is not
possible. These are **resolves-it-by-moving cases, not closes-it-with-a-test cases**, and they are
tracked against Phase 2's app shell instead.

---

## 6. Tool substitutions and an open question for the project owner

Two places where work unit 1A did not follow a tool the constitution names. **One is settled and
recorded; one is an open question that is the project owner's call, not this file's.**

### 6.1 Settled — coverage tool: `cargo tarpaulin` → `cargo llvm-cov`

Recorded in full in **§1**. `cargo-tarpaulin` was unavailable; `AGENTS.md` §4.1 names tarpaulin.
Recorded rather than silently substituted, and the two tools' denominators are not guaranteed
identical, so a re-measurement must use the same tool to be comparable. **No owner decision needed —
this is a fact about the environment, not a preference.**

### 6.2 OPEN QUESTION — parameterized tests: `rstest` / `test-case` vs std-only case loops

**`AGENTS.md` §4.3 names `rstest` or `test-case` for parameterized tests. Work unit 1A used neither.**
It used `for (name, case, expected) in cases` loops over named case tables instead, on the reading
that `AGENTS.md` §7.2's **criterion 1** — *"Verify no solution exists with current dependencies or
std library"* — governs, and that `std` **is** the solution.

The pattern, from `crates/sh_nexus_wire/tests/wire_frames.rs`:

```rust
let cases: [(&str, ClientFrame, &str); 7] = [
    ("message.send", ClientFrame::MessageSend { .. }, r#"{"v":1,…}"#),
    ("reaction.add", ClientFrame::ReactionAdd { .. }, r#"{"v":1,…}"#),
    // …five more, each with a name
];

for (name, frame, expected) in cases {
    assert_eq!(
        envelope.encode().expect("a client frame encodes"),
        expected,
        "`{name}` does not match the JSON in PLAN.md section 6"
    );
}
```

15 such case tables exist across the 1A test files.

**The tradeoff, in two sentences.** Literal `AGENTS.md` §4.3 compliance means adding `rstest` as a
dev-dependency — and it is a real addition, not a formality: **`rstest` is not in `Cargo.lock`
today**, so adopting it is a new dependency requiring a `PLAN.md` §2 row and a `docs/DEPENDENCIES.md`
§7.2 audit row, neither of which exists. Std-only loops cost nothing, are already readable, and
already name every case in the failure message via `{name}` — but they are not what §4.3 names, and
§4.3 is the constitution.

**This file does not recommend a winner. It is the project owner's call**, and it should be settled
before 1B, because 1B lands `core/ordering.rs`, `core/markdown.rs` and `core/cache.rs` and will write
the parameterized tests `AGENTS.md` §4.4's proptest mandates — so the pattern gets entrenched either
way.

| Option | Cost | Benefit |
|---|---|---|
| **`rstest`** | New dev-dependency; `PLAN.md` §2 row + `docs/DEPENDENCIES.md` audit row required | Literal §4.3 compliance; declarative cases |
| **std case loops** (current) | Not literal §4.3 compliance; ~15 tables to maintain by hand | Zero dependencies; already names every case in the failure message |

**Recorded as open. Not decided here.**
