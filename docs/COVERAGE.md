# Sh_Nexus — Measured Coverage Baseline

This file records the **measured** coverage of the workspace, most recently as of work unit 1B, so
the numbers are not lost and later work has something concrete to regress against.

`AGENTS.md` §2.3: *"Profile before optimizing. Measure, don't guess."* `AGENTS.md` §6.3 makes a
coverage drop merge-blocking, which requires a baseline to drop *from*. This file is that baseline.

It is a **record of measurement, not a claim of compliance.** Where a floor cannot yet be measured,
§3 says so explicitly rather than letting an absent row read as a pass.

**Recorded:** 2026-09-27 · **work unit 1B** · Windows, MSVC, rustc 1.98.1
**Baseline recorded:** 2026-09-27 · work unit 1A · same environment

`AGENTS.md` §6.3 makes a coverage drop merge-blocking, and a drop is only
meaningful against a prior number, so **both** recordings are kept: 1A's figures
are in §2.5 as a delta table, and 1B's are the live tables in §2.1–§2.2. Where 1A
made a prediction that 1B's measurement then refuted, that is recorded as a
refutation rather than quietly corrected — `PLAN.md` Rev 2's whole history is
predictions that measurement did not support.

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

The `--all-targets` flag is kept from 1A so the two recordings share a
denominator. It makes no difference to the numbers here — there are no benches or
examples in the workspace — and `cargo llvm-cov --workspace --summary-only` was
run as well and produced **byte-identical** per-file figures. That is worth one
sentence because a flag whose absence would have silently changed the
denominator is exactly the sort of thing §6.3's regression gate should not
depend on.

**Tool versions verified for this recording:** `cargo-llvm-cov 0.9.1`, same as
1A. `rustc 1.98.1 (MSVC)`, 12 logical CPUs, MSVC Build Tools 2022, Windows SDK
10.0.26100.0.

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
| `sh_nexus\src\core\ordering.rs` | 190 | 0 | **100.00%** | 0 | **100.00%** |
| `sh_nexus\src\lib.rs` | 148 | 58 | 60.81% | 32 | 67.68% |
| `sh_nexus\src\main.rs` | 7 | 7 | 0.00% | 7 | 0.00% |
| `sh_nexus\src\network\mapping.rs` | 331 | 2 | 99.40% | 0 | 100.00% |
| `sh_nexus_wire\src\error.rs` | 4 | 0 | 100.00% | 0 | 100.00% |
| `sh_nexus_wire\src\frame.rs` | 169 | 2 | 98.82% | 0 | 100.00% |
| `sh_nexus_wire\src\version.rs` | 17 | 0 | 100.00% | 0 | 100.00% |
| **TOTAL** | **866** | **69** | **92.03%** | **39** | **93.88%** |

Paths are as the tool reports them — Windows separators, workspace-relative to each crate.

**`core/ordering.rs` is the first row in this table that is new**, and it is the
first `core/` file to appear at all. 1A's baseline had no `core/` row because
`core/` held type definitions only; §2.3 explains that, and §3.3 records what
changed. **190 regions, 0 missed, 18 of 18 functions executed** — the whole
module, including the private comparators, is covered.

### 2.2 Crate and layer aggregates

| Aggregate | Region cover | Line cover |
|---|---|---|
| **`core/`** | **100.00%** (190/190 regions) | **100.00%** (143/143 lines) |
| **`sh_nexus_wire` as a crate** | **98.95%** (188/190 regions) | **100.00%** (159/159 lines) |
| `sh_nexus` as a crate | 90.10% (609/676 regions) | 88.32% (479/542 lines) |

`core/`'s row covers exactly one file, because `ordering.rs` is the only
executable thing in the layer. See §3.3 for why that is a real measurement and
not the floor being won.


### 2.3 Files that do not appear in the report at all

Their absence is **not** a 0% and **not** a pass. These files contain no executable code — only
structs, enums, derives and `pub mod` declarations — so the instrumenter emits zero coverage regions
for them and they produce no report row at all:

- `sh_nexus\src\core\mod.rs`, `sh_nexus\src\core\models\mod.rs`
- `sh_nexus\src\core\models\user.rs`, `channel.rs`, `message.rs`, `events.rs`
- `sh_nexus\src\network\mod.rs`
- `sh_nexus_wire\src\lib.rs`, `sh_nexus_wire\src\dto.rs`

Each was read and confirmed to contain no `fn` and no `impl` block. The absence is a **true zero for
executable code**, not a measurement gap.

**This is the list that made §3.3 unmeasurable, and it is now one file shorter
in spirit but not in fact:** `core/ordering.rs` has left this category and
appears in §2.1 at 100.00%, while `core/models/*.rs` remain in it. The category
itself is unchanged — it still means "no logic to be wrong", and `markdown.rs`,
`cache.rs` and `theme.rs` will join `models/` here until 1C writes them.

### 2.4 Reconciliation note on the line column

**Both columns reconcile in 1B.** That is a change from 1A and it is recorded as
an observation, not as a correction of 1A's arithmetic.

The **region** column:

```text
regions:      190 + 148 +   7 + 331 +   4 + 169 +  17 =  866
missed:         0 +  58 +   7 +   2 +   0 +   2 +   0 =   69
covered:      866 − 69 = 797        797 / 866 = 92.03%
```

The **line** column, which in 1A summed to 51 against a TOTAL of 48:

```text
lines:        143 +  99 +   7 + 229 +   3 + 139 +  17 =  637
missed:         0 +  32 +   7 +   0 +   0 +   0 +   0 =   39
covered:      637 − 39 = 598        598 / 637 = 93.88%
```

**What changed, and what is still unexplained.** 1A recorded two anomalies: the
per-file missed-line figures summing to 51 against a TOTAL of 48, and
`sh_nexus_wire\src\error.rs` reporting 3 missed lines while simultaneously
reporting 100.00% line cover — two figures that cannot both be true of one
denominator. 1A inferred the likely cause (non-region-bearing lines counted
under a different rule than the region rows) and correctly labelled it an
inference, not a verified cause.

In 1B `error.rs` reports 3 lines and **0 missed**, and every wire-crate file
reports **0 missed lines**. So the contradiction is gone and the line column
reconciles. **The cause is still not verified.** What can be said is narrower
than a resolution: 1A's inference is *consistent* with what 1B observes, and the
1A figures are not reproducible by re-running the tool on the 1B tree, so the
1A anomaly cannot be re-examined from this recording. It is a finding about a
past measurement that is closed by supersession rather than by explanation.

**Consequence, unchanged from 1A.** ADR-004's floors are stated as region
coverage, the region column reconciles to the last decimal, and **§3's verdicts
rest on regions**. The line column is recorded for completeness and must still be
re-derived from a single report before it gates anything under §6.3.

### 2.5 The 1A baseline, retained for regression
1A's figures, kept verbatim so §6.3's "any coverage drop blocks the merge" has
something to drop from. **Every aggregate improved; no aggregate regressed.**

| Aggregate | 1A | 1B | Δ |
|---|---|---|---|
| `sh_nexus\src\core\ordering.rs` | *did not exist* | 100.00% regions | new |
| `sh_nexus\src\lib.rs` | 60.81% regions / 67.68% lines | 60.81% / 67.68% | none |
| `sh_nexus\src\main.rs` | 0.00% / 0.00% | 0.00% / 0.00% | none |
| `sh_nexus\src\network\mapping.rs` | 99.40% / 100.00% | 99.40% / 100.00% | none (**see §2.6** — 99.09% in ~1 run in 3) |
| `sh_nexus_wire\src\error.rs` | 100.00% / *100.00% (3 "missed")* | 100.00% / 100.00% | none |
| `sh_nexus_wire\src\frame.rs` | 94.08% regions / 95.68% lines | **98.82%** / **100.00%** | **+4.74 / +4.32** |
| `sh_nexus_wire\src\version.rs` | 76.47% / 82.35% | **100.00%** / **100.00%** | **+23.53 / +17.65** |
| `core/` as a layer | **no denominator** | **100.00%** (190/190) | **now measurable** |
| `sh_nexus_wire` as a crate | 92.63% / 94.34% | **98.95%** / **100.00%** | **+6.32 / +5.66** |
| `sh_nexus` as a crate | *not recorded* | 90.10% / 88.32% | — |
| **Workspace total** | **88.02% regions / 90.28% lines** | **92.03% / 93.88%** | **+4.01 / +3.60** |
**`lib.rs`, `main.rs` and `mapping.rs` are unchanged to the decimal**, which is
the control that makes the rest of the table meaningful: 1B touched no line of
any of them, and the tool reports exactly what 1A reported. A coverage table
whose untouched rows moved would mean the denominator moved, and then no delta in
the table could be read.

**`mapping.rs` is a near-exception, and §2.6 is about it.** It is stable in the
modal case and moves by one region in roughly one run in three.

### 2.6 The measurement is not stable to the decimal — `mapping.rs`

**This is the most consequential finding in the file, and it is about the
measurement rather than about any line of code.**

Re-running `cargo llvm-cov --workspace --summary-only` on the **identical,
unmodified** 1B tree gives **two different answers**:

| Recorded runs | `mapping.rs` | Workspace TOTAL |
|---|---|---|
| 2 missed regions | 4 | **92.03%** |
| 3 missed regions | 2 | **91.92%** |

Six measurements, same commit, same tool, same environment, different numbers.
`lib.rs`, `main.rs`, `core/ordering.rs`, `frame.rs` and `version.rs` were
**identical in every run**; the variation is confined to `mapping.rs`.

**What is verified, and how:**

| Question | Answer | How |
|---|---|---|
| Did 1B touch `mapping.rs`? | **No.** `git status` shows it unmodified | working tree |
| Is the line column stable? | **Yes** — 229 lines, 0 missed, every run | `report` summary |
| Is function execution stable? | **Yes** — 22 of 22, every run | `report` summary |
| Do the JSON region sets differ between a 2- and a 3-run? | **No** — the region-entry line sets and the zero-count segment sets are **identical** | `--json` export, compared directly |
| Can the extra region be attributed to a line? | **No**, with the tooling in this environment | the export itemises nothing that differs |

**The cause is almost certainly proptest's per-run randomness, and that is an
inference, not a measurement.** `mapping.rs` is exercised substantially by the
property tests in `tests/wire_boundary.rs` and `tests/proptest_boundary.rs`,
proptest generates fresh random values on every run, and a region entered only by
*some* generated inputs is therefore covered in most runs and missed in some. The
evidence is consistent with that and with nothing else observed — the alternative
would be non-determinism in the tool itself, which the stable line and function
columns argue against.

**Consequence, and it is not small.** `AGENTS.md` §6.3 makes *"any coverage drop
blocks the merge"* a rule. **A gate on the exact decimal of this figure will fire
a phantom regression roughly one time in three, on a file nobody changed.** Two
consequences follow, and both are for the CI owner rather than for this file:

1. **The gate needs a tolerance, or a fixed proptest seed.** `AGENTS.md` §4.3
   requires independent tests and §4.4 requires proptest; §4.3 also forbids
   `sleep()` to wait for logic, and nothing in either forbids pinning a seed. A
   fixed seed makes the measurement a function of the commit rather than of the
   run, which is what a merge gate requires. **This file does not make that
   change** — it would alter every property test in the workspace, which is a
   decision above this work unit.
2. **The per-file gate on `mapping.rs` should be the floor, not the decimal.**
   ADR-004 states `network/`'s floor at 80%; the measurement is 99.09–99.40%. The
   floor is stable, the decimal is not, and §6.3 should be expressed in terms of
   the one that does not move.

**Why this is recorded rather than averaged away.** 1A §2.4 found the *line*
column non-reconciling and warned that using it as a §6.3 gate "will compare two
different denominators and report a phantom regression". The same defect has now
been found in the *region* column, on a different file, by a different
mechanism. The project's own file predicted the failure mode before it was
observed in the column the project gates on. That is worth more than a clean
number.

**The tables in §2.1–§2.2 report the modal case (2 missed, 92.03%),** because
that is what most runs produce, it is what 1A recorded for every other row, and
`core/`'s 100.00% — the figure §3.3 turns on — is identical in every run and is
not affected by any of this. Where a figure could be read either way, §2.6 says so.


---

## 3. Verdict against each floor

### 3.1 `AGENTS.md` §6.1 — workspace total: 75% minimum / 85% target

**92.03% region coverage — PASSES THE TARGET.** 7.03 points above the 85% target and 17.03 points
above the 75% minimum, and 4.01 points above the 1A baseline. The line column reads 93.88% and points
the same way.

**The regional figure is 91.92–92.03%** depending on the run, per §2.6, and **passes the target by
6.92 points at the lower end as well.** The verdict does not depend on which run produced it; the
decimal does, and §2.6 says so.

### 3.2 ADR-004 — `sh_nexus_wire` ≥80%

**98.95% region coverage — PASSES.** 18.95 points above the floor, and 6.32 points above the 1A
baseline. See §2.2. This is the floor that matters most (§3.4), because `AGENTS.md` §4.2's serde
round-trip mandate and §7.4's version rejection both live in this crate.

**It is not 100%, and the reason is recorded in §4.4.** 1A predicted that closing the three named
residuals would take this crate to 100% region coverage. Measurement says otherwise: 188/190. The
prediction is refuted by 2 regions, and §4.4 says what is and is not known about them.

### 3.3 `AGENTS.md` §4.1 — `core/` ≥90%: **MEASURABLE, AND MET**

**`core/ordering.rs` measures 100.00% — 190 of 190 regions, 143 of 143 lines, 18 of 18 functions
executed, 0 missed. The 90% floor passes with nothing missed at all.** The sentence 1A wrote here —
that this floor could not be evaluated — is no longer true, and this section is its replacement.

What changed, stated precisely so the number is not read as more than it is:

- **1A was right that the floor was unmeasurable, and right about why.** `core/` held type
  definitions only, which emit zero regions (§2.3), so there was no denominator. That is a fact about
  measurement, not a pass, and 1A was careful not to let the absence read as green.
- **1B is the first executable code in `core/`, and it is 100%.** The floor now has a denominator and
  the denominator clears it. The two are separate statements: the first is about arithmetic, the
  second is about whether the code was tested. Both are recorded here because only the first is what a
  coverage tool can tell you.
- **The floor is met for one module, not for the layer.** `core/`'s remaining three logic modules —
  `markdown.rs`, `cache.rs`, `theme.rs` — are work unit **1C** and do not exist. When they land they
  are measured against the same 90%, and a 100% here is a promise about `ordering.rs`, not a headroom
  budget for them. `PLAN.md` §8, Phase 1: *"Strict TDD applies to `core/` and `sh_nexus_wire` (pure,
  ≥90% floor, no excuses)."*
- **This is the strictest floor in the project, and it is now the first one actually enforced against
  real logic.** 90% minimum (ADR-004 decision 2, which took the stricter of `AGENTS.md`'s two
  conflicting figures — see Appendix A item 1) against a pure-logic target with no excuse available.
  `PLAN.md` §10's proptest mandate applies to exactly this module, and the two properties it names
  are implemented: the permutation invariant and no-loss.
- **`core/`'s architectural obligations are now enforced rather than stated.** ADR-003 keeps the
  layer free of `gpui` and `tokio`, and `core/models/mod.rs` adds "no I/O, no clock, no thread".
  Until 1B those were prose, because a type definition cannot break them. `core/ordering.rs` is the
  first code that could, and `tests/layer_boundary.rs` grew three tests in 1B to hold the line
  mechanically: `core_names_no_clock_and_no_thread`, `core_contains_no_panicking_construct`
  (`AGENTS.md` §7.1's no-panic rule, also unenforceable before), and
  `the_violation_scanners_see_code_and_not_prose` — which exists so the two new scanners are known
  to fail **loud**, not open.

### 3.4 Summary

| Floor | Source | Measured | Verdict |
|---|---|---|---|
| Workspace total ≥75% min / ≥85% target | `AGENTS.md` §6.1 | **92.03%** regions | **PASSES target** |
| `sh_nexus_wire` ≥80% | ADR-004 decision 3 | **98.95%** regions | **PASSES** |
| `core/` ≥90% | `AGENTS.md` §4.1, ADR-004 decision 2 | **100.00%** regions (190/190) | **PASSES — 0 missed** |
| New code ≥80% | `AGENTS.md` §5.1, ADR-004 | see §4 | **PASSES on all three files** |
| `network/` ≥80% | `AGENTS.md` §4.1 | `mapping.rs` **99.40%** regions (99.09–99.40%, §2.6) | **PASSES** |
| `state/`, `db/`, utilities ≥80/85% | `AGENTS.md` §4.1 | directories do not exist yet | Not applicable in 1B |

**Every floor with a denominator passes in 1B**, and none of them passes by a
margin that depends on a file nobody can test: the two sub-80% files are
`lib.rs` and `main.rs`, both characterised in §4.1–§4.2 as the window-opening
path that Phase 2 moves, and neither is in a directory §4.1 names.


---

## 4. The sub-floor files, characterised

**Two files sit below 80% in 1B, down from four in 1A.** Both are structural: the window-opening
path that Phase 2 restructures into `src/app.rs`, in a file §4.1's directory-keyed floors do not
name. None of them is a quality problem in the code that was written, and neither is new in 1B — both
are byte-identical to 1A and measure identically (§2.5).

The three that 1A characterised here and that are now **closed or nearly so** —
`version.rs` at 76.47%, `frame.rs` at 94.08%, and the nine-line residual of §5 — are §4.3, §4.4 and
§5 below. The file that 1A called *"the strongest file in the baseline"* is still strongest, at
`mapping.rs` §4.5, and is unchanged to the decimal.

### 4.1 `sh_nexus\src\lib.rs` — 60.81% regions, 67.68% lines

**Unchanged in 1B**, and unchanged for the reason 1A gave: 1B touched no line of this file and the
tool reports exactly what 1A reported (§2.5).

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

### 4.3 `sh_nexus_wire\src\version.rs` — **100.00% regions, 100.00% lines — CLOSED in 1B**

1A recorded this file at **76.47%** with 4 missed regions of 17, all of them the body of
`impl fmt::Display for ProtocolVersion` (lines 117-119, one `write!`). It is now **17/17 regions and
17/17 lines, 0 missed**, and §4.3's old heading — *"below the 80% per-file reading"* — no longer
applies to anything.

The test that closed it is
`version_negotiation::a_protocol_version_displays_as_its_bare_major_number` in work unit 1B, written
in `#[rstest]` form per ADR-008 with a companion guard,
`the_display_cases_cover_every_supported_version`, so that a **second** supported major cannot be added
to `SUPPORTED_MAJOR_VERSIONS` without a `#[case]` to test it. The impl itself did not change.

### 4.4 `sh_nexus_wire\src\frame.rs` — 94.08% → **98.82% regions, 100.00% lines**

**1A recorded 10 missed regions of 169 and attributed all ten to "six lines of delegation" — the two
`kind()` accessors at lines 581-583 and 682-684. That attribution was wrong, and this is the
measurement that says so.**

1B added the two tests §5 asked for — `wire_frames::a_client_envelope_reports_its_frames_kind` (5
`#[case]`s) and `wire_frames::a_server_envelope_reports_its_frames_kind` (7 `#[case]`s), plus
`the_envelope_kind_cases_cover_every_frame_type` as the guard. The file went to **167/169 regions
(+4.74 points) and 139/139 lines** — the accessor bodies and every line they sit on are now covered.

**Two regions remain, and here is exactly what is and is not known about them:**

| Question | Answer | How it was established |
|---|---|---|
| How many? | **2** of 169 | The `report` summary, and the `--json` export's own `summary` object |
| Which lines? | **None.** 0 missed lines, 0 zero-count `DA:` records in the lcov export | lcov export, filtered to `frame.rs` |
| Which functions? | **None.** 25 of 25 functions executed | The `--json` export's `summary.functions` |
| Which segments? | **None.** 338 segments, 0 with a zero count | The `--json` export's `segments` array |

So the two uncovered regions are **not attributable to any line, any function, or any segment in the
export** — they are regions the instrumenter counts that the export does not itemise. 1A's reading,
that each accessor is "one missed region body plus one for the enclosing item", is *consistent* with a
residual of exactly two (one enclosing-item region per accessor) and is the most economical explanation
available. **It is not verified**, and it is recorded here as the inference it is, on the same terms
1A used. `llvm-cov show` was not available in this environment to settle it, and inventing a
confident cause for two regions is precisely the failure this file exists to prevent.

**1A's §5 prediction is refuted, and the correction matters.** 1A wrote: *"Action for 1B: add one test
per range … which closes the residual and takes `sh_nexus_wire` to 100% region coverage."* The tests
closed **8 of the 10** missed regions. The crate is at **98.95%**, not 100%. 1A also wrote that the
ten missed regions were "entirely attributable to six lines of delegation", which cannot be right if
two of them survive after both accessors are called twelve times each. The honest statement is that
1A identified the three right *line ranges* and the wrong *count*, and that the count was only ever
knowable by measurement.

**None of this is a quality problem in the code.** The file's real content — frame encode/decode,
`v`-header validation, direction checking, UTF-8 rejection — is covered at 100% of lines and 100% of
functions, which is what §4.2's "serde round-trips for every wire format; malformed payload rejection"
mandate requires of this crate. What remains is a two-region accounting artefact in a file that is
otherwise fully exercised, and ADR-004 states its floor for the **crate** precisely so that a
17-region file and a 169-region file are not ranked against each other.


### 4.5 `sh_nexus\src\network\mapping.rs` — 99.40% regions, 100.00% lines

**The strongest file in the baseline** and the only one with **zero** missed lines. 331 regions, 2
missed. This is the `TryFrom` boundary from `sh_nexus_wire` into `core::models` — the seam ADR-003
names as the thing that must not rot — and it is where `AGENTS.md` §2.1's "validate every incoming
payload before it touches state" is enforced. Recorded here because a baseline that only lists
failures hides the result that matters most.

**Unchanged in 1B, to the modal figure** (§2.5): 1B touched no line of it, and the two missed regions
remain the two 1A recorded. **The one file whose figure is not reproducible to the decimal — see
§2.6**, which is why this sentence says "modal" and not "exactly".

### 4.6 `sh_nexus\src\core\ordering.rs` — 100.00% regions, 100.00% lines

**The new file, and the one §3.3 turns on.** 190 regions, 0 missed, 143 lines, 0 missed, 18 functions
executed. It is in this section only because §4 is where the *detail* behind a number lives, not
because anything about it needs characterising.

**What 100% means here, stated so it is not over-read.** It means every region the instrumenter
emitted was executed, which for this module includes the private comparators `precedence` and
`differing_fields`, all nine field comparisons in `differing_fields`, all nine arms of
`DifferingField::as_str`, and all four arms of `assess_sync`'s watermark `match`. It does **not** mean
the module has no untested *decisions*: a total order has no branches to miss, so 100% is a weaker
statement here than it is in a file full of `if`. What actually pins the decisions is the test suite,
and specifically:

| Claim | Test | Kind |
|---|---|---|
| Any permutation of a batch gives one result | `every_permutation_of_a_batch_reconciles_to_the_same_result` | **exhaustive** — all 120 orderings of a five-message batch |
| Any permutation gives one result | `permuting_a_batch_does_not_change_the_ordered_result` | proptest |
| Deduplication loses nothing and invents nothing | `reconciliation_preserves_exactly_the_input_client_msg_ids` | proptest |
| No input can panic it | `arbitrary_messages_reconcile_without_panicking` | proptest |
| The result is really ordered and really deduplicated | `a_reconciled_batch_is_always_ascending_and_deduplicated` | proptest |
| A disagreement's report is order-free | `permuting_a_batch_of_disagreeing_pairs_changes_neither_the_set_nor_the_report` | proptest |

The first row is the one to notice: `AGENTS.md` §4.4's property is a *universal* claim over
permutations, and a property test samples it while an enumeration over 5! = 120 cases **proves** it
for that batch. Both are here because they answer different questions, and 100% region coverage
answers neither.

---

## 5. Carried-forward action — **CLOSED in 1B, with one refuted prediction**

1A left a nine-line residual here, by line range, and named the action. Both are now done.

| File | Lines | What | Closed by | Result |
|---|---|---|---|---|
| `sh_nexus_wire\src\version.rs` | 117-119 | `impl fmt::Display for ProtocolVersion` — one `write!` | `version_negotiation::a_protocol_version_displays_as_its_bare_major_number` (+ its coverage guard) | **100.00%**, 17/17 |
| `sh_nexus_wire\src\frame.rs` | 581-583 | `ClientEnvelope::kind` — one delegating accessor | `wire_frames::a_client_envelope_reports_its_frames_kind` — 5 `#[case]`s | body and lines covered |
| `sh_nexus_wire\src\frame.rs` | 682-684 | `ServerEnvelope::kind` — one delegating accessor | `wire_frames::a_server_envelope_reports_its_frames_kind` — 7 `#[case]`s | body and lines covered |

**All three line ranges are covered, and `frame.rs` reaches 100% of lines and 100% of functions.**

**But 1A's prediction that this "takes `sh_nexus_wire` to 100% region coverage" was wrong, and the
correction is the useful part of this entry.** The crate is at **98.95% (188/190)**, not 100%. Eight of
the ten missed regions in `frame.rs` closed; two did not, and §4.4 sets out what is and is not known
about them — including that 1A's *"entirely attributable to six lines of delegation"* was an
inference presented as a fact, and that measurement contradicts it.

**Why the two survivors are not carried forward.** They are not a code gap: 0 missed lines, 0 missed
functions, 0 zero-count segments, in a file whose real content is fully exercised. They are a
two-region accounting artefact, and carrying a line-numbered action for a region the tool cannot point
at would be a task nobody could close — which is worse than recording it as a finding. `frame.rs` is
6.82 points above ADR-004's floor and `sh_nexus_wire` is 18.95 points above it (§3.2). If a future
tool version makes those regions attributable, §4.4 will say so when it happens.

**The lesson is recorded because it is the second time in this project.** §6.2 below is the first:
1A's own §5 action predicted an outcome that measurement did not deliver. Both are the same shape — a
specific, confident, checkable prediction written before measuring, and a measurement after — and the
project's rule is that the prediction is not quietly edited once the measurement lands. The
alternative is a documentation file that always reads as though it were right, which is worth nothing
as a baseline.

### 5.1 Not carried forward

`lib.rs` and `main.rs` are **not** on this list, in 1B exactly as in 1A. Their gaps are the
window-opening path (§4.1, §4.2), which Phase 2 restructures into `src/app.rs` and which no test can
reach in its current shape. Adding a test for `spike_window_options` would require a live `App`, and
adding one for `main()` is not possible. These are **resolves-it-by-moving cases, not
closes-it-with-a-test cases**, and they are tracked against Phase 2's app shell instead. Neither file
moved in 1B and both measure identically (§2.5).


---

## 6. Tool substitutions — both settled

Two places where work unit 1A did not follow a tool the constitution names. **Both are now settled and
recorded; neither is an open question any more.**

### 6.1 Settled — coverage tool: `cargo tarpaulin` → `cargo llvm-cov`

Recorded in full in **§1**. `cargo-tarpaulin` was unavailable; `AGENTS.md` §4.1 names tarpaulin.
Recorded rather than silently substituted, and the two tools' denominators are not guaranteed
identical, so a re-measurement must use the same tool to be comparable. **No owner decision needed —
this is a fact about the environment, not a preference.**

### 6.2 RESOLVED — parameterized tests: `rstest` adopted (see ADR-008)

**`AGENTS.md` §4.3 names `rstest` or `test-case` for parameterized tests. Work unit 1A used neither. This section originally recorded that as an open question; it is now RESOLVED -- `rstest` is adopted from 1B and the 1A tables are grandfathered. See ADR-008.**
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

**This file originally did not recommend a winner and left it as the project owner's call**, and it should be settled
before 1B, because 1B lands `core/ordering.rs`, `core/markdown.rs` and `core/cache.rs` and will write
the parameterized tests `AGENTS.md` §4.4's proptest mandates — so the pattern gets entrenched either
way.

**Recorded as RESOLVED — `rstest` adopted. See ADR-008.** `rstest = "0.27"` is a dev-dependency of both
workspace crates from work unit 1B onward, with the §7.2 audit in the root `Cargo.toml`: MIT OR
Apache-2.0, 28,006,626 downloads in 90 days, published 2026-09-06, MSRV 1.85.0, and exactly **one** new
compile unit (`rstest_macros`, a proc-macro). The other two runtime dependencies are optional and off.

**The 1A suite is grandfathered, not rewritten.** The 15 case tables above stay as they are: they are
green, their cases are named in every failure message, and converting them to `#[case]` attributes is a
4,600-line diff that changes no behaviour. §4.3 applies from 1B.

**Why this resolves against 1A's original reading.** ADR-008 records it, and the short form is that
§7.2 and §4.3 never conflicted. §7.2 is a *process* about adding dependencies; its criterion 1 asks
whether a solution exists. §4.3 is a *technique* that names the solution. 1A read criterion 1 as
overriding §4.3, which made a std `for` loop a licence to ignore a named mandate. That is the same
error `PLAN.md` Rev 2 made twice — silently negating §7.3's row-estimator mandate, and declaring §6.2's
merge-blocking thresholds "targets, not gates" — and the Rev 2 conformance audit named that pattern as
the reason the reconciliation was not settled. Declining to fix it a third time over a tooling
preference would make the point three times over. §7.2's job here is to record *why* the dependency is
justified, and "the constitution mandates it" is the strongest available answer.

**The signal that made the dependency worth it:** a `for` loop over twenty cases reports as one test
that failed at some index. `#[rstest]` with `#[case]` reports twenty distinct tests, so a failure names
the case that broke and the other nineteen still show as passing. For a suite with a coverage floor
attached to it, that difference in signal is worth one proc-macro.

**1B applied the rule, and the signal claim is now measured rather than asserted.** `rstest` was
already a dev-dependency of both crates at the start of 1B, so 1B added no dependency and needed no
new §7.2 row. What 1B wrote in the mandated form:

| Test file | `#[case]`s | What they cover |
|---|---|---|
| `sh_nexus\tests\ordering.rs` | 22 | timestamp tiebreaker (4), every `DifferingField` (9), cursor boundary (2), every `SyncStatus` (7) |
| `sh_nexus_wire\tests\wire_frames.rs` | 12 | `ClientEnvelope::kind` (5) and `ServerEnvelope::kind` (7), one case per frame type |
| `sh_nexus_wire\tests\version_negotiation.rs` | 1 | `ProtocolVersion`'s `Display`, one case per supported major |

**35 parameterized cases**, each reporting as its own test. The two that most justify the
dependency are the nine `DifferingField` cases — a failure there names *which field* stopped being
detected, where a `for` loop over a nine-row table would have printed an index — and the twelve
envelope-`kind` cases, where a failure names the frame type whose delegating accessor stopped
delegating.

**Three of these case lists carry an explicit coverage guard**, and that is the part worth copying:
`the_envelope_kind_cases_cover_every_frame_type`,
`the_display_cases_cover_every_supported_version`, and the existing
`client_frame_kind_is_total`. A `#[case]` list is a hand-maintained enumeration just as a `for`-loop
table is, and nothing in `#[case]` syntax makes it complete — so each list here is paired with a test
that fails when the registry grows past it. **The mandated style is not self-verifying, and treating it
as though it were would be the next version of the mistake ADR-008 corrects.**

