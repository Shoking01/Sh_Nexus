# Sh_Nexus — Measured Coverage Baseline

This file records the **measured** coverage of the workspace, most recently as of work unit 1C-1, so
the numbers are not lost and later work has something concrete to regress against.

`AGENTS.md` §2.3: *"Profile before optimizing. Measure, don't guess."* `AGENTS.md` §6.3 makes a
coverage drop merge-blocking, which requires a baseline to drop *from*. This file is that baseline.

It is a **record of measurement, not a claim of compliance.** Where a floor cannot yet be measured,
§3 says so explicitly rather than letting an absent row read as a pass.

**Recorded:** 2026-09-27 · **work unit 1C-1** · Windows, MSVC, rustc 1.98.1
**Prior:** 2026-09-27 · work unit 1B · same environment
**Baseline recorded:** 2026-09-27 · work unit 1A · same environment

`AGENTS.md` §6.3 makes a coverage drop merge-blocking, and a drop is only
meaningful against a prior number, so **all three** recordings are kept: 1B's figures are
in §2.5 as a delta table against 1A, 1C-1's delta is §2.7, and 1C-1's are the live tables in
§2.1–§2.2. Where a prior recording made a prediction that measurement then refuted, that is recorded
as a refutation rather than quietly corrected — `PLAN.md` Rev 2's whole history is predictions that
measurement did not support.

**The headline of 1C-1 is a number that went *down*, and it is the interesting
one.** `core/`'s aggregate fell from 100.00% to 96.53% because a second file
entered the denominator at 95.74% rather than at 100%. The floor is still met
with 6.53 points to spare. §2.7 sets out the full arithmetic, because a coverage
file that only recorded improvements would be a coverage file nobody could use
as a baseline.

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
1A and 1B. `rustc 1.98.1 (MSVC)`, 12 logical CPUs, MSVC Build Tools 2022, Windows SDK
10.0.26100.0. **1C-1 added a dependency to the workspace** (`pulldown-cmark 0.13.4`, plus `unicase`
2.9.0 as its only new non-optional runtime dependency) and the tool was re-verified rather than
assumed, because a different instrumentation set would invalidate the comparison against 1B's
figures. It did not: the `--all-targets` denominator and the eight reportable files are the same
shape, with `core/markdown.rs` added.

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

Modal run, per §2.6's practice: the case that most runs produce. Two runs were taken and
`mapping.rs` differed between them; `core/markdown.rs` was **byte-identical in both**, which is
recorded in §2.6 because it is the first evidence that the fluctuation is confined to the file 1B
identified.

| File | Regions | Missed regions | Region cover | Missed lines | Line cover |
|---|---|---|---|---|---|
| `sh_nexus\src\core\cache.rs` | 125 | 1 | **99.20%** | 0 | **100.00%** |
| `sh_nexus\src\core\markdown.rs` | 846 | 36 | **95.74%** | 31 | **94.98%** |
| `sh_nexus\src\core\ordering.rs` | 190 | 0 | **100.00%** | 0 | **100.00%** |
| `sh_nexus\src\lib.rs` | 148 | 58 | 60.81% | 32 | 67.68% |
| `sh_nexus\src\main.rs` | 7 | 7 | 0.00% | 7 | 0.00% |
| `sh_nexus\src\network\mapping.rs` | 331 | 2 | 99.40% | 0 | 100.00% |
| `sh_nexus_wire\src\error.rs` | 4 | 0 | 100.00% | 0 | 100.00% |
| `sh_nexus_wire\src\frame.rs` | 169 | 2 | 98.82% | 0 | 100.00% |
| `sh_nexus_wire\src\version.rs` | 17 | 0 | 100.00% | 0 | 100.00% |
| **TOTAL** | **1837** | **106** | **94.23%** | **70** | **94.79%** |

Paths are as the tool reports them — Windows separators, workspace-relative to each crate.

**`core/cache.rs` is the new row: 125 regions, 1 missed, 20 of 20 functions executed, 88 of 88
lines.** The single missed region is the `None` arm of an `if let` in `promote`, unreachable
because every caller has already established the key is live. Removing it would trade a §2.1
safety property for one region, which is the wrong trade; §4.8 records it.

**An arithmetic correction, recorded rather than quietly fixed.** Work unit 1C-2a first reported
`core/` at 95.94% using a denominator of 1,171. That denominator is a summation error: the layer
is `ordering.rs` 190 + `markdown.rs` 846 + `cache.rs` 125 = **1,161**, not 1,171. With the correct
denominator the layer is 1,124 / 1,161 = **96.81%**, a **rise** of 0.28 points rather than the
0.59-point fall that was reported and built a narrative on. The underlying advice — gate a
per-layer threshold on the floor, not on the previous total — survives the correction; the
evidence offered for it did not, and the "second consecutive fall" claim was false.

`core/ordering.rs` is unchanged to the decimal for the second consecutive work unit, which is
the control that makes the rest of the table readable.

### 2.1.1 The `core/` layer arithmetic, stated so it can be checked

| | 1B | 1C-1 | 1C-2a |
|---|---|---|---|
| files | 1 | 2 | 3 |
| regions | 190 | 1036 | 1161 |
| missed | 0 | 36 | 37 |
| covered | 190 | 1000 | 1124 |
| **region cover** | **100.00%** | **96.53%** | **96.81%** |
| floor | 90% | 90% | 90% |
| margin | +10.00 | +6.53 | +6.81 |

1C-1's recorded prediction was that a new file landing near `markdown.rs`'s ~96% would settle the
layer near 97%, and one landing at `ordering.rs`'s 100% would settle above 98%. `cache.rs` landed
at 99.20% and the layer settled at 96.81% — inside neither band. The prediction compared a new
file's own score against a *layer total*, and a new file is a minority of the denominator: 125 new
regions at 99.20% lift the total by 0.28 points, where an equal-to-average addition would give
roughly 1.5. The prediction is left standing as a refutation.

### 2.2 Crate and layer aggregates

| Aggregate | Region cover | Line cover |
|---|---|---|
| **`core/`** | **96.81%** (1124/1161 regions) | **96.46%** (818/848 lines) |
| **`sh_nexus_wire` as a crate** | **98.95%** (188/190 regions) | **100.00%** (159/159 lines) |
| `sh_nexus` as a crate | 93.96% (1571/1672 regions) | 94.06% (1077/1145 lines) |

`core/` now covers **three** files. One in 1B at 100%, two in 1C-1 at 96.53%, three in 1C-2a at
**96.81%** — a rise of 0.28 points, because `cache.rs` landed at 99.20% with a single missed
region, which is above the layer average and therefore lifts the total. The full three-column
arithmetic is in §2.1.1 so it can be checked rather than believed; the first report of this figure
used a denominator of 1,171 instead of 1,161 and stated a fall, and that correction is recorded in
§2.1 rather than applied silently.

It is 6.81 points above the floor. `ordering.rs` remains the highest-scoring file in the
workspace at 100% regions, 100% functions and 100% lines.


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

**This is the list that made §3.3 unmeasurable in 1A, and it is now two files shorter.** Two
files have left this category: `core/ordering.rs` in 1B (now §2.1 at 100.00%) and
`core/markdown.rs` in 1C-1 (now §2.1 at 95.74%). **Both are the only `core/` files to have ever
appeared in the region report**, and every remaining member of the layer is still type definitions
and `pub mod` declarations. `core/cache.rs` (1C-2) and `core/theme.rs` (1D) remain in this category
until they are written, and when they land they enter §2.1 — and §2.7 is the arithmetic for what
that will do to the aggregate.

### 2.4 Reconciliation note on the line column

**Both columns reconcile in 1C-1, and continue to.** 1A found the *line* column
non-reconciling and 1B recorded that the contradiction had gone away without
explaining it; 1C-1 adds a second consecutive clean run, which is worth one
sentence because the anomaly 1A reported was in the column this project would
have used as a gate.

The **region** column:

```text
regions:      846 + 190 + 148 +   7 + 331 +   4 + 169 +  17 = 1712
missed:        36 +   0 +  58 +   7 +   2 +   0 +   2 +   0 =  105
covered:     1712 − 105 = 1607       1607 / 1712 = 93.87%
```

The **line** column:

```text
lines:        618 + 143 +  99 +   7 + 229 +   3 + 139 +  17 = 1255
missed:        31 +   0 +  32 +   7 +   0 +   0 +   0 +   0 =   70
covered:     1255 −  70 = 1185       1185 / 1255 = 94.42%
```

Per-file missed lines sum to the TOTAL in both columns, and the per-file line counts sum to the
TOTAL line count. **The cause of 1A's anomaly is still not verified** and this file does not claim
it is; what has changed is that there are now two independent clean measurements rather than one.

**The consequence, unchanged from 1A.** ADR-004's floors are stated as region
coverage, the region column reconciles to the last decimal, and **§3's verdicts
rest on regions**. The line column is recorded for completeness and must still be
re-derived from a single report before it gates anything under §6.3.

### 2.5 The 1B baseline, retained for regression

1B's figures, kept verbatim so §6.3's "any coverage drop blocks the merge" has
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
unmodified** tree gives **two different answers**. 1B recorded six measurements
across two commits; 1C-1 adds two more on a different commit, and the pattern
holds.

| Recorded runs | `mapping.rs` | Workspace TOTAL |
|---|---|---|
| 2 missed regions | 4 | **92.03%** |
| 3 missed regions | 2 | **91.92%** |
| 2 missed regions (1C-1, run 1) | 4 | **92.03%** |
| 3 missed regions (1C-1, run 1) | 2 | **91.92%** |

Eight measurements, different commits, same tool, same environment, different numbers.

**What is verified, and how:**

| Question | Answer | How |
|---|---|---|
| Did 1B touch `mapping.rs`? | **No.** `git status` shows it unmodified | working tree |
| Did 1C-1 touch `mapping.rs`? | **No.** 1C-1 touched `core/markdown.rs`, `core/mod.rs`, the two `Cargo.toml`s, and two test files | working tree |
| Is the line column stable? | **Yes** — 229 lines, 0 missed, every run | `report` summary |
| Is function execution stable? | **Yes** — 22 of 22, every run | `report` summary |
| Do the JSON region sets differ between a 2- and a 3-run? | **No** — the region-entry line sets and the zero-count segment sets are **identical** | `--json` export, compared directly |
| Can the extra region be attributed to a line? | **No**, with the tooling in this environment | the export itemises nothing that differs |

**`core/markdown.rs` is the new control, and it is the useful part of this
section.** Its two 1C-1 runs were **identical to every digit** — 846 regions, 36
missed, 95.74%, 618 lines, 31 missed, 79 of 79 functions — on a file whose test
suite is dominated by proptest. So the fluctuation is **not** a property of
running proptest at all; it is confined to the one file 1B identified, which
narrows the cause further than 1B could and supports its inference that the
trigger is proptest's per-run randomness *in `mapping.rs` specifically* —
exercised by `tests/wire_boundary.rs` and `tests/proptest_boundary.rs`, which
generate fresh values on every run and therefore enter some regions only
sometimes.

**The cause remains an inference, not a measurement**, and it is recorded as
one. The evidence is consistent with that and with nothing else observed; the
alternative would be non-determinism in the tool itself, which the stable line
and function columns and the now-stable `markdown.rs` row both argue against.

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

**The tables in §2.1–§2.2 report the modal case (2 missed, 93.87%),** because
that is what most runs produce, it is what both prior recordings recorded for
every other row, and `core/`'s figure — the one §3.3 turns on — is identical in
every run and is not affected by any of this. Where a figure could be read either
way, §2.6 says so. **The 1C-1 range for the workspace TOTAL is 93.81–93.87%**,
and every floor in §3 passes at the lower end as well.

### 2.7 The 1C-1 delta, including the regression

The one number in this file that moved against 1B, stated first and without
softening.

| Aggregate | 1B | 1C-1 | Δ |
|---|---|---|---|
| `sh_nexus\src\core\markdown.rs` | *did not exist* | **95.74%** regions / 94.98% lines | new |
| `sh_nexus\src\core\ordering.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| `sh_nexus\src\lib.rs` | 60.81% / 67.68% | 60.81% / 67.68% | **none** |
| `sh_nexus\src\main.rs` | 0.00% / 0.00% | 0.00% / 0.00% | **none** |
| `sh_nexus\src\network\mapping.rs` | 99.40% / 100.00% | 99.40% / 100.00% | **none** (modal; see §2.6) |
| `sh_nexus_wire\src\error.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| `sh_nexus_wire\src\frame.rs` | 98.82% / 100.00% | 98.82% / 100.00% | **none** |
| `sh_nexus_wire\src\version.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| **`core/` as a layer** | **100.00%** (190/190) | **96.53%** (1000/1036) | **−3.47** |
| `sh_nexus_wire` as a crate | 98.95% / 100.00% | 98.95% / 100.00% | **none** |
| `sh_nexus` as a crate | 90.10% / 88.32% | **93.23%** / **93.61%** | **+3.13 / +5.29** |
| **Workspace total** | **92.03% / 93.88%** | **93.87% / 94.42%** | **+1.84 / +0.54** |

**Seven rows are unchanged to the decimal**, and that is the control that makes
the `core/` row readable. 1C-1 touched no line of `ordering.rs`, `lib.rs`,
`main.rs`, `mapping.rs`, or any wire-crate file, and the tool reports exactly
what 1B reported for all seven.

**Why `core/` fell, in one sentence:** the aggregate is over every executable
thing in the layer, `ordering.rs` is still 190 of 1036 regions, and
`markdown.rs` entered at 95.74% rather than at 100%. **No existing region became
uncovered** — `ordering.rs` is 190/190 in both recordings — so this is a
denominator effect and not a regression in code that was already measured. It is
still a fall, it still counts under `AGENTS.md` §6.3's rule, and it is recorded
as one rather than explained away by the fact that the floor is met.

**What it predicts, and the prediction is falsifiable.** `core/cache.rs` (1C-2)
and `core/theme.rs` (1D) are the remaining two logic modules. If they land at
markdown.rs's ~96%, the layer settles near 97%; if they land at ordering.rs's
100%, it settles above 98%. Either way the floor holds, and §6.3's regression
gate should be keyed to the **layer** and not to the 1B figure of 100.00%, which
was a statement about a single file.


---

## 3. Verdict against each floor

### 3.1 `AGENTS.md` §6.1 — workspace total: 75% minimum / 85% target

**93.87% region coverage — PASSES THE TARGET.** 8.87 points above the 85% target and 18.87 points
above the 75% minimum, and 1.84 points above the 1B baseline. The line column reads 94.42% and points
the same way.

**The regional figure is 93.81–93.87%** depending on the run, per §2.6, and **passes the target by
8.81 points at the lower end as well.** The verdict does not depend on which run produced it; the
decimal does, and §2.6 says so.

### 3.2 ADR-004 — `sh_nexus_wire` ≥80%

**98.95% region coverage — PASSES.** 18.95 points above the floor, unchanged from 1B. See §2.2. This
is the floor that matters most (§3.4), because `AGENTS.md` §4.2's serde round-trip mandate and
§7.4's version rejection both live in this crate.

**It is not 100%, and the reason is recorded in §4.4.** 1A predicted that closing the three named
residuals would take this crate to 100% region coverage. Measurement says otherwise: 188/190. The
prediction is refuted by 2 regions, and §4.4 says what is and is not known about them.

**1C-1 changed nothing in this crate and is recorded as unchanged**, which is the useful outcome: the
two figures `frame.rs`'s residual produces are the same two regions 1B found, on a tree where the
dependency graph grew by two crates.

### 3.3 `AGENTS.md` §4.1 — `core/` ≥90%: **MEASURABLE, AND MET — for two files now**

**`core/` measures 96.53% — 1000 of 1036 regions, 730 of 761 lines, 97 of 97 functions executed,
36 regions missed. The 90% floor passes with 6.53 points of margin.** `core/ordering.rs` is still
190/190 and `core/markdown.rs` is 810/846.

What is new in 1C-1, stated precisely so the number is not read as more than it is:

- **The floor is met for a second module, and the layer aggregate moved the *wrong* way.** 1B
  recorded `core/` at 100.00% on the strength of one file. Adding a second file at 95.74% took the
  layer to 96.53%. §2.7 is the arithmetic. It is a denominator effect and not a regression —
  `ordering.rs` is unchanged to the decimal — but it **is** a fall under §6.3's rule, and it is
  recorded as one. The lesson for §6.3 is that a per-layer gate must be keyed to the layer, not to
  whatever figure a single-file layer happened to report.
- **The strictest floor in the project now has a denominator worth the name.** 90% against pure
  logic with no excuse available, met with two logic modules in the denominator rather than one.
- **The security-relevant work is the part coverage cannot see, and this file says so rather than
  letting 95.74% stand in for it.** `core/markdown.rs` is a parser for attacker-controlled text.
  Its correctness is pinned by 241 tests, of which the mandatory ones are named in `AGENTS.md` §4.2
  — bold, italic, code, links, nested formatting, never panics, injection safety — and by six
  proptest properties over arbitrary input. §4.7 says which claim each one carries. A coverage
  number would be the wrong instrument for all of it, and 1A §4.6 already made this point about
  `ordering.rs`.
- **`core/`'s architectural obligations are enforced, and the allow-list had to be widened.** ADR-003
  keeps the layer free of `gpui` and `tokio`. `core/markdown.rs` is the first `core/` module to
  depend on anything outside `std` and the four domain crates, so
  `crates/sh_nexus/tests/layer_boundary.rs`'s `CORE_ALLOWED_CRATES` allow-list was extended with
  `pulldown_cmark` **in the same commit that added it to `Cargo.toml`** — which is the property
  that makes an allow-list worth maintaining: the dependency was rejected until somebody recorded
  the decision. All 11 boundary tests pass, including the three 1B added to hold the no-panic and
  no-clock rules mechanically.
- **`core/`'s remaining two logic modules are `cache.rs` (1C-2) and `theme.rs` (1D).** They do not
  exist, so this floor is met for half the layer. §2.7 states the falsifiable prediction for when
  they land.

### 3.4 Summary

| Floor | Source | Measured | Verdict |
|---|---|---|---|
| Workspace total ≥75% min / ≥85% target | `AGENTS.md` §6.1 | **93.87%** regions (93.81–93.87%) | **PASSES target** |
| `sh_nexus_wire` ≥80% | ADR-004 decision 3 | **98.95%** regions | **PASSES** |
| `core/` ≥90% | `AGENTS.md` §4.1, ADR-004 decision 2 | **96.53%** regions (1000/1036) | **PASSES — 36 missed, all defensive (§4.7)** |
| New code ≥80% | `AGENTS.md` §5.1, ADR-004 | `markdown.rs` **95.74%** | **PASSES** |
| `network/` ≥80% | `AGENTS.md` §4.1 | `mapping.rs` **99.40%** regions (99.09–99.40%, §2.6) | **PASSES** |
| `state/`, `db/`, utilities ≥80/85% | `AGENTS.md` §4.1 | directories do not exist yet | Not applicable in 1C-1 |

**Every floor with a denominator passes in 1C-1**, and none of them passes by a
margin that depends on a file nobody can test: the two sub-80% files are
`lib.rs` and `main.rs`, both characterised in §4.1–§4.2 as the window-opening
path that Phase 2 moves, and neither is in a directory §4.1 names.



---

## 4. The sub-floor files, characterised

**Two files sit below 80% in 1C-1, down from four in 1A.** Both are structural: the window-opening
path that Phase 2 restructures into `src/app.rs`, in a file §4.1's directory-keyed floors do not
name. None of them is a quality problem in the code that was written, and neither is new in 1C-1 —
both are byte-identical to 1A and measure identically (§2.7).

The three that 1A characterised here and that are now **closed or nearly so** —
`version.rs` at 76.47%, `frame.rs` at 94.08%, and the nine-line residual of §5 — are §4.3, §4.4 and
§5 below. The file that 1A called *"the strongest file in the baseline"* is still strongest, at
`mapping.rs` §4.5, and is unchanged to the decimal.

### 4.1 `sh_nexus\src\lib.rs` — 60.81% regions, 67.68% lines

**Unchanged in 1C-1, and unchanged for the reason 1A gave:** no work unit since has touched a line
of this file, and the tool reports exactly what it reported then (§2.7).

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

**Unchanged in 1C-1, to the modal figure** (§2.7): 1C-1 touched no line of it, and the two missed regions
remain the two 1A recorded. **The one file whose figure is not reproducible to the decimal — see
§2.6**, which is why this sentence says "modal" and not "exactly".

### 4.6 `sh_nexus\src\core\ordering.rs` — 100.00% regions, 100.00% lines

**190 regions, 0 missed, 143 lines, 0 missed, 18 functions executed. Unchanged to the decimal in
1C-1**, which is the control §2.7 relies on. It is in this section because §4 is where the *detail*
behind a number lives.

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

### 4.7 `sh_nexus\src\core\markdown.rs` — 95.74% regions, 94.98% lines

**The new file, and the one §3.3 turns on.** 846 regions, 36 missed, 618 lines, 31 missed, and
**79 of 79 functions executed**. It is in this section because 36 missed regions is a number a
reader should be able to interrogate rather than accept.

**All 31 missed lines are defensive arms, and the enumeration is the claim.** They fall into exactly
four groups, and in every case the guard is kept because the *cost of being wrong* is losing an
author's text — so the list is here for a reader to disagree with, not to be impressed by.

| # | Lines | The arm | Why it cannot be reached |
|---|---|---|---|
| 1 | 1020 | strip the `\r` of a closing `\r\n` in a code block | **Verified**: pulldown normalises CRLF to LF before this module sees it. `parse("```rust\r\nx\r\n```")` yields `"x"`, confirmed by test and by probe. The arm is correct and currently unreachable. |
| 2 | 1292, 1301, 1679, 1700, 1738–1740, 1801, 1868–1869, 1881, 1897–1901 | ten guards on states pulldown excludes | listed individually below |
| 3 | 1443 | `Event::InlineMath \| DisplayMath \| FootnoteReference` | the `Options` in `parse` do not enable math or footnotes, so these events cannot be emitted. Documented in the module as a deliberate narrowing. |
| 4 | 1539–1543, 1671–1673, 1704–1708 | three "this cannot happen, but do not lose the content" paths | listed individually below |

Group 2, spelled out:

| Line | The arm | Why it cannot be reached |
|---|---|---|
| 1292 | a `Frame::Literal` receiving a bare block | an HTML block holds runs, not blocks |
| 1301 | `as_list_host` returning `None` | an `Item` is only ever emitted with a `List` directly beneath it |
| 1679 | a `List` frame closing with no items | a list always has at least one item |
| 1700 | the brace of `if let Some(items) = host` | **a region-attribution artefact**: `items.push(item)` and the `return` beside it are both covered. This is the same class as `frame.rs`'s two unattributable regions (§4.4) — the instrumenter counts a region the lcov export cannot itemise |
| 1738–1740 | joining buffered runs into a code block's text | a code block's body arrives as `Text` events and goes straight into its `text`; `loose` is always empty |
| 1801 | `push_text_with` with empty text | no parser event in this configuration carries an empty `Text` |
| 1868–1869 | a link inside a link | **CommonMark forbids it**, which is why `MAX_INLINE_DEPTH` is 2 and is documented as structural rather than enforced |
| 1881 | `Frame::Literal` in `push_inline`'s sink check | no link is emitted inside an HTML block |
| 1897–1901 | `finish`'s two drain loops | **pulldown's event stream is balanced by guarantee**, so no frame or inline frame is ever still open at the end. This is the single most consequential line in the table: it means the "malformed input" defence lives *inside pulldown*, and this module's own teardown is a belt-and-braces path that no input reaches. |

**Two of the 36 regions were closed during 1C-1 rather than explained away**, and that is the part
of this entry worth copying. The first measurement was 93.31% with 59 missed and **one missed
function**; investigation found three things that were *reachable and untested*:

1. `Style`'s `Binary` impl — public API no test called;
2. the `\r` branch of the code-block trim — reachable only via a CRLF message;
3. the empty-heading guard — reachable via a bare `#`.

Those are now tested, which is also how the CRLF normalisation above was *verified* rather than
assumed. Two pieces of genuinely dead code were **deleted** rather than left as unreachable defence:
`Builder::close_leaf` and the "fill the open leaf" branch of `flush_pending` (whose two states are
exclusive, so it could never be taken). Deleting them moved 846 regions' worth of code from
93.31% to 95.74% without adding a single test, which is the honest measure of how much of the
original 59 was untested *decisions* rather than unreachable branches.

**What coverage cannot see here, and is not asked to.** This is a parser for
attacker-controlled text, and its security properties are not line coverage. `AGENTS.md` §4.2's
markdown row is the specification, and each clause has a test named after it:

| `AGENTS.md` §4.2 clause | Test | `#[case]`s |
|---|---|---|
| bold, italic, code, links | `a_markup_construct_renders_as_its_style` | 10 |
| nested formatting | `a_triple_asterisk_run_carries_both_bold_and_italic`, `nested_formatting_produces_one_span_carrying_the_union_of_its_ancestors`, `three_spellings_of_bold_italic_produce_the_same_tree` (4) | 5 |
| code — block | `a_code_blocks_content_is_never_parsed` (4), `a_code_block_keeps_the_authors_blank_lines` (5), `a_code_block_reports_its_declared_language` (7) | 16 |
| code — inline vs block | `inline_code_and_a_code_block_are_structurally_different` | 1 |
| never panics | `arbitrary_markdown_never_panics_the_parser` | **proptest** |
| malformed input | `unclosed_markup_still_yields_the_authors_text`, `a_hostile_single_token_is_bounded_and_does_not_panic` (8), `an_empty_message_produces_no_blocks` (4) | 30 |
| injection safety — raw HTML | `raw_html_is_preserved_as_literal_text` | 8 |
| injection safety — dangerous schemes | `a_dangerous_link_scheme_never_reaches_a_link_target` — **one per scheme** | 4 |
| injection safety — unlisted schemes | `an_unlisted_scheme_is_refused` (`tel:`, `ftp:`, `ms-msdt:`, `search-ms:`, `customapp:`, `about:`, `jar:`) | 7 |
| injection safety — obfuscation | `a_scheme_obfuscated_with_a_control_character_is_refused` (10), `a_target_with_no_scheme_is_refused` (6) | 16 |
| injection safety — the **converse** | `a_legitimate_link_survives` — a filter that refuses everything passes every test above | 5 |
| injection safety — images | `an_image_contributes_its_alt_text_and_never_a_target` | 4 |
| injection safety — Unicode | `a_bidi_control_is_replaced_and_a_bidi_mark_is_kept` (12), `a_bidi_mark_and_a_zero_width_joiner_survive` (6), `ordinary_text_neutralizes_nothing` (7) | 25 |
| the text a reader sees | `a_markup_spelling_renders_to_exactly_the_text_the_reader_sees` | 11 |
| the declared limits | `a_message_over_the_size_limit_is_truncated_on_a_character_boundary` (5), `deeply_nested_quotes_are_flattened_without_losing_text` (4), `truncation_never_splits_a_multi_byte_character` | 10 |

**The "converse" row is the one to insist on.** Every test above it asserts that something is
*refused*; a filter that refused all links would pass every one of them. `a_legitimate_link_survives`
is the only thing standing between this module and a link renderer that breaks every message, and it
is why the allowlist's three entries are tested positively rather than inferred from the denials.

And the six proptest properties, which are the ones coverage cannot substitute for:

| Property | The claim it pins |
|---|---|
| `arbitrary_markdown_never_panics_the_parser` | `AGENTS.md` §4.4's mandate, verbatim |
| `parsing_is_deterministic` | same input, same tree — what makes 1C-2's segment cache worth having |
| `the_tree_never_exceeds_its_declared_depth` | both limits, from the outside, over arbitrary input |
| `every_source_range_lies_inside_the_source` | §3's ranges are real, not fabricated |
| `no_link_target_escapes_the_allowlist` | the allowlist as a **universal** claim, not a table of bad URLs |
| `every_run_is_plain_text_with_no_bidi_control` | no run carries a character that changes how its surroundings read |

**The one that found a bug is the one worth noting.** `a_link_is_the_deepest_inline_nesting_there_is`
is a two-assertion test and it caught `[**a**](x)` producing a link with **no content** and its text
as a sibling run — a `Strong` frame sits above the link on the inline stack, and the builder was
looking only at the top of it. That is the same class of defect as the tight-list bug the first
draft had, found by a different route, and it is the argument for asserting on the tree rather than
on the text: `plain_text` was **correct** in that case, and only the structure was wrong.

---

## 5. Carried-forward actions

### 5.1 1A's nine-line residual — **CLOSED in 1B, with one refuted prediction**

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

**1C-1 re-measured both files and both are unchanged**, on a tree where the dependency graph grew by
two crates. That is the third consecutive recording in which this residual is stable, and it is
recorded as a settled finding rather than an open action.

**The lesson is recorded because it is the second time in this project.** §6.2 below is the first:
1A's own §5 action predicted an outcome that measurement did not deliver. Both are the same shape — a
specific, confident, checkable prediction written before measuring, and a measurement after — and the
project's rule is that the prediction is not quietly edited once the measurement lands. The
alternative is a documentation file that always reads as though it were right, which is worth nothing
as a baseline.

### 5.2 `core/markdown.rs` — **NOT carried forward, and the reasoning is recorded**

1C-1's file has **36 uncovered regions**, and §4.7 enumerates every one of them. None is carried
forward as an action, for two reasons that are different from each other and both worth stating:

1. **Most are unreachable, and writing a task for an unreachable region is a task nobody can close.**
   `Builder::finish`'s drain loops cannot run because pulldown's event stream is balanced by
   guarantee; the link-inside-a-link arm cannot run because CommonMark forbids it. An action item
   against either would be theatre.
2. **Three were reachable and are now closed**, and that is what the investigation was *for*. The
   first measurement was 93.31% with a missed function; the reachable gaps turned out to be
   `Style`'s `Binary` impl, the `\r` branch of the code-block trim, and the empty-heading guard. All
   three are tested. Two further pieces of dead code were deleted rather than defended, which is
   where most of the 93.31% → 95.74% movement came from.

**The prediction §4.7 makes is falsifiable and is left standing**: `core/cache.rs` (1C-2) and
`core/theme.rs` (1D) will move the `core/` aggregate, and the direction of that move is stated
rather than left to be discovered.

### 5.3 Not carried forward

`lib.rs` and `main.rs` are **not** on this list, in 1C-1 exactly as in 1B and 1A. Their gaps are the
window-opening path (§4.1, §4.2), which Phase 2 restructures into `src/app.rs` and which no test can
reach in its current shape. Adding a test for `spike_window_options` would require a live `App`, and
adding one for `main()` is not possible. These are **resolves-it-by-moving cases, not
closes-it-with-a-test cases**, and they are tracked against Phase 2's app shell instead. Neither file
moved in 1C-1 and both measure identically (§2.7).


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

**1C-1 wrote 241 tests — 215 of them `#[case]`s across 36 `#[rstest]` functions, 20
hand-written `#[test]`s and 6 proptest properties — in the mandated form from the first line.** No
grandfathering, no `for` table, and the file was written after ADR-008 was accepted. That is the
point worth recording: the constitutional reading was settled *before* the work unit that would have
to follow it, so 1C-1 had no decision to make and no deviation to explain.

| Test file | `#[case]`s | What they cover |
|---|---|---|
| `sh_nexus\tests\markdown.rs` | **215** in 36 functions | inline constructs, run merging, soft breaks, code blocks, links and every rejected scheme, raw HTML, bidi controls and marks, size and depth limits, malformed input, plain text, the `Style` algebra, headings, task lists, list numbering, container nesting |

**The 215-case list is a stronger argument for the dependency than 1B's 35 were**, for a specific
reason: `a_dangerous_link_scheme_never_reaches_a_link_target` has **one `#[case]` per named
scheme**. Under `#[rstest]` those are four distinct tests, so a failure says *`javascript` stopped
being refused* — a security regression with a name. Under 1A's `for (name, case, expected)` table
they would have been one test that failed at some index, and the index would have to be counted back
to the scheme by hand. `AGENTS.md` §4.2 lists injection safety as a mandatory test target, and this
is the case where the difference in signal is worth a proc-macro rather than merely convenient.

**1C-1 added two more coverage guards**, following the pattern above rather than inventing one:
`the_style_bits_are_exactly_the_four_declared_flags` (fails if a fifth style bit is added without the
list knowing, and fails in *both* directions — a bit in `Style::ALL` that no test names, and a named
style whose bit `ALL` does not carry) and `the_heading_cases_cover_every_level` (fails if a
`From<HeadingLevel>` arm and a `Heading::level` arm drift apart). **`AGENTS.md` §4.2's markdown row has
no registry to guard** — the format is not an enum the project controls — so the guards there are of a
different kind, and §4.7 lists what stands in their place: the `Style::ALL` and `Heading` guards, the
`#![case]`-paired structural assertions, and the universal proptest claims that a hand-written case
list structurally cannot make.

**One compiler fact 1C-1 learned the hard way, recorded because it will cost the next author an hour
otherwise:** **rstest 0.27 requires `#[case]` on each parameter**, not merely a `#[case(…)]`
attribute on the test. A signature written as `fn f(source: &str, expected: &str)` under
`#[case("a", "b")]` produces **one error per `#[case]`, all reading "Wrong case signature: should
match the given parameters list"** — eighty of them for 1C-1's first draft, with no indication that
the fix is on the *parameter*. `crates/sh_nexus/tests/ordering.rs:960` shows the blessed form:
`#[case] offset: i64`.


