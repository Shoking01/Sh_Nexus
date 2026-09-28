# Sh_Nexus — Measured Coverage Baseline

This file records the **measured** coverage of the workspace, most recently as of work unit 1C-2b, so
the numbers are not lost and later work has something concrete to regress against.

`AGENTS.md` §2.3: *"Profile before optimizing. Measure, don't guess."* `AGENTS.md` §6.3 makes a
coverage drop merge-blocking, which requires a baseline to drop *from*. This file is that baseline.

It is a **record of measurement, not a claim of compliance.** Where a floor cannot yet be measured,
§3 says so explicitly rather than letting an absent row read as a pass.

**Recorded:** 2026-09-28 · **work unit 1D** · Windows, MSVC, rustc 1.98.1
**Prior:** 2026-09-28 · work unit 1C-2b · same environment
**Prior:** 2026-09-27 · work unit 1C-2a · same environment
**Prior:** 2026-09-27 · work unit 1C-1 · same environment
**Prior:** 2026-09-27 · work unit 1B · same environment
**Baseline recorded:** 2026-09-27 · work unit 1A · same environment

`AGENTS.md` §6.3 makes a coverage drop merge-blocking, and a drop is only
meaningful against a prior number, so **every** recording is kept: 1B's figures are
in §2.5 as a delta table against 1A, 1C-1's delta is §2.7, 1C-2a's live figures were the
§2.1–§2.2 tables this recording replaced, 1C-2b's delta is **§2.8**, and 1D's is
**§2.10**. Where a prior
recording made a prediction that measurement then refuted, that is recorded
as a refutation rather than quietly corrected — `PLAN.md` Rev 2's whole history is predictions that
measurement did not support.

**The headline of 1D is the largest single-work-unit rise in this file's history, and the
interesting part is a coverage report finding two real gaps that 785 passing tests had
missed.** `core/theme.rs` entered at **98.93% regions and 99.78% lines** on its first
measurement, but the first measurement was of an *incomplete* suite: `cargo llvm-cov
--show-missing-lines` named three unexecuted regions, and two of them were not unreachable
arms at all — a label field holding a JSON number, and the `Display` of the
oversize error. Both were closed with four tests, and the file finished at **98.93% /
99.78%** with a single missed line: the arm §6 of its own module documentation calls
unreachable. **`core/` rose 0.81 points to 97.76%** and the workspace total rose 1.41 to
**95.80%**, both far more than 1D's new file alone can account for, and §2.11 shows where
the rest came from: ten regions of previously-uncovered `errors.rs`, which entered the
report for the first time because 1D gave it a `From` impl.

**The mutation table (§5.6) is the more interesting half of 1D, and its first draft was
worse than the table it replaces.** Three of nine deliberate defects were caught by exactly
**one** test each, against `cache.rs`'s minimum of five — and two of the three landed on
decisions this file's own §4 calls load-bearing. That is reported rather than smoothed, and
the two cheapest of them were then strengthened and **re-measured**, with both numbers
recorded.

**The headline of 1C-2b is that no aggregate regressed, and the interesting part is a pair of
arithmetic errors in the recording immediately before this one.** `core/cache.rs` grew from 125
regions to 178 by being extended with the memory-ceiling policy and the thread-safety decision, and
it grew coverage with it: 99.20% → 99.44%, with the missed-region count unchanged at one. `core/`
therefore **rose** 0.14 points to 96.95%, the workspace total rose 0.16 to 94.39% with its
missed-region count unchanged at 106, and `sh_nexus` as a crate rose 0.20 — **the last of those
only after its baseline was corrected**, which is the finding worth reading this file for.

**Two arithmetic corrections are recorded rather than applied silently, and both are in the 1C-2a
recording.** §2.4's `core/` **line** figure was published as `818/848 = 96.46%`; the denominator
should have been **849**, so the correct figure is `818/849 = 96.35%`. §2.2's `sh_nexus` crate
figure was published as `93.96% (1571/1672)` on both columns; **neither number in either fraction
can be reproduced from the per-file table beside it**, and the correct figures are
`1543/1647 = 93.69%` regions and `1114/1184 = 94.09%` lines. §4.9 gives the arithmetic and four
independent cross-checks. **The `core/` region column, which is what ADR-004's floors rest on, was
correct throughout.** §1 records the rule this recording adopts as a result: every aggregate is
recomputed from §2.1's per-file rows rather than carried forward, and the crates are checked to sum
to the tool's TOTAL.

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

The `--all-targets` flag is kept from 1A so every recording shares a
denominator. It makes no difference to the numbers here — there are no benches or
examples in the workspace — and `cargo llvm-cov --workspace --summary-only` was
run as well and produced **byte-identical** per-file figures in 1C-2b, the third
consecutive recording in which the two agree. That is worth one
sentence because a flag whose absence would have silently changed the
denominator is exactly the sort of thing §6.3's regression gate should not
depend on.

**Tool versions verified for this recording:** `cargo-llvm-cov 0.9.1`, same as
1A, 1B, 1C-1 and 1C-2a. `rustc 1.98.1 (MSVC)`, 12 logical CPUs, MSVC Build Tools 2022, Windows SDK
10.0.26100.0. **1C-2b added no dependency**, so the instrumented region set is
comparable by construction rather than by inspection — the first work unit since 1C-1 that can say
that, and the reason the comparison needed no re-verification this time. The `--all-targets`
denominator and the nine reportable files are the same shape as 1C-2a's.

**One thing that did change the measurement inputs, and it is recorded because it affects
reproducibility rather than the denominator:** 1C-2b's proptest properties found failing cases during
development, and proptest wrote them to
`crates/sh_nexus/tests/cache_ceiling.proptest-regressions`, which is committed. Those seeds are
replayed before any novel case is generated, so the suite is *more* reproducible than it was before
them, and the file must not be deleted as noise. §2.6's recommendation of a fixed seed is unaffected
— the regressions file is the per-case mechanism, not a global one.

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

### The rule this recording adopted after finding two arithmetic errors

**Every aggregate in §2.2 and every delta in §2.5/§2.7/§2.8 is recomputed from §2.1's per-file
rows at the moment of writing, and the two crates are checked to sum to the tool's TOTAL.** It is
recorded as a rule rather than a remark because §4.9 found two wrong aggregates in the immediately
preceding recording, and both were *derived* figures — hand-computed from a transcription rather than
read from the tool. A propagated aggregate has no transcription step to catch a mistake, which is
the mechanism of both. §4.9 gives the arithmetic and the four cross-checks; §5.5 carries the audit
of the older recordings as an open action, and deliberately does not do it here, because rewriting a
prior recording's figures destroys the record the file exists to keep.

---

## 2. Measured results

### 2.1 Per file

Modal run, per §2.6's practice: the case that most runs produce. Two runs were taken in 1C-2b and
**every row was byte-identical between them**, including `mapping.rs`, which is the file 1B
identified as the fluctuating one. That is the first recording in which §2.6's instability did not
reproduce, and §2.6 says so rather than leaving the claim standing unchallenged.

| File | Regions | Missed regions | Region cover | Missed lines | Line cover |
|---|---|---|---|---|---|
| `sh_nexus\src\core\cache.rs` | 178 | 1 | **99.44%** | 0 | **100.00%** |
| `sh_nexus\src\core\markdown.rs` | 846 | 36 | **95.74%** | 31 | **94.98%** |
| `sh_nexus\src\core\ordering.rs` | 190 | 0 | **100.00%** | 0 | 100.00% |
| `sh_nexus\src\lib.rs` | 148 | 58 | 60.81% | 32 | 67.68% |
| `sh_nexus\src\main.rs` | 7 | 7 | 0.00% | 7 | 0.00% |
| `sh_nexus\src\network\mapping.rs` | 331 | 2 | 99.40% | 0 | 100.00% |
| `sh_nexus_wire\src\error.rs` | 4 | 0 | 100.00% | 0 | 100.00% |
| `sh_nexus_wire\src\frame.rs` | 169 | 2 | 98.82% | 0 | 100.00% |
| `sh_nexus_wire\src\version.rs` | 17 | 0 | 100.00% | 0 | 100.00% |
| **TOTAL** | **1890** | **106** | **94.39%** | **70** | **94.95%** |

Paths are as the tool reports them — Windows separators, workspace-relative to each crate.

**Both columns reconcile for the first time in this file's history, and the arithmetic is here so
it can be checked rather than believed.**

```text
regions: 178 + 846 + 190 + 148 +   7 + 331 +   4 + 169 +  17 = 1890
missed:   1 +  36 +   0 +  58 +   7 +   2 +   0 +   2 +   0 =  106
covered: 1890 − 106 = 1784        1784 / 1890 = 94.39%

lines:  132 + 618 + 143 +  99 +   7 + 229 +   3 + 139 +  17 = 1387
missed:   0 +  31 +   0 +  32 +   7 +   0 +   0 +   0 +   0 =   70
covered: 1387 −  70 = 1317        1317 / 1387 = 94.95%
```

**`core/cache.rs` is the changed row: 178 regions, 1 missed, 30 of 30 functions executed, 132 of 132
lines.** At 1C-2a it was 125 regions, 1 missed, 20 of 20 functions, 88 of 88 lines. The 53 new
regions are the memory-ceiling policy and the thread-safety decision, and **every one of them is
covered** — the missed-region count did not move, which is the number that says the new code was
exercised rather than merely present. The one missed region is still the `None` arm of an `if let`
in `promote`, unreachable because every caller has already established the key is live. Removing it
would trade an `AGENTS.md` §2.1 safety property for one region, which is the wrong trade; §4.8
records it. §4.8 is where the ceiling's *behaviour* is characterised, because behaviour is what
coverage cannot see.

**An arithmetic correction to the previous recording, recorded rather than quietly fixed — and it is
in the line column, not the region column.** §2.2 at 1C-2a published `core/` at `818/848 = 96.46%`
lines. The denominator is wrong: `cache.rs` had **88** lines at 1C-2a, so the layer's line count was
`88 + 618 + 143 = 849`, not 848. The correct 1C-2a figure is `818/849 = 96.35%`. The cause is
identified in §2.4: the hand arithmetic there listed `618 + 143 + 99 + 7 + 229 + 3 + 139 + 17 =
1255` and **omitted `cache.rs` entirely** — the file had not been written when §2.4's arithmetic was
first performed, and it was never revised. §2.1's own TOTAL line figure at 1C-2a was correct
(`1273/1343 = 94.79%`), so the tool never disagreed; the document did. **The region column, which is
what ADR-004's floors rest on, was correct at every recording and is unchanged.**

`core/ordering.rs` is unchanged to the decimal for the **third** consecutive work unit, which is
the control that makes the rest of the table readable.

### 2.1.1 The `core/` layer arithmetic, stated so it can be checked

| | 1B | 1C-1 | 1C-2a | 1C-2b |
|---|---|---|---|---|
| files | 1 | 2 | 3 | 3 |
| regions | 190 | 1036 | 1161 | **1214** |
| missed | 0 | 36 | 37 | **37** |
| covered | 190 | 1000 | 1124 | **1177** |
| **region cover** | **100.00%** | **96.53%** | **96.81%** | **96.95%** |
| floor | 90% | 90% | 90% | 90% |
| margin | +10.00 | +6.53 | +6.81 | **+6.95** |
| lines | 143 | 761 | 849 | **893** |
| missed lines | 0 | 31 | 31 | **31** |
| line cover | 100.00% | 95.93% | 96.35% | **96.53%** |

Each figure from its own numerator and denominator, spelled out:

```text
1C-2b regions:  178 (cache) + 846 (markdown) + 190 (ordering) = 1214
1C-2b missed:     1        +  36         +    0            =   37
1C-2b covered: 1214 − 37 = 1177          1177 / 1214 = 96.95%

1C-2b lines:    132       + 618         + 143             =  893
1C-2b missed:     0       +  31         +   0             =   31
1C-2b covered:  893 − 31 = 862            862 / 893 = 96.53%
```

**The denominator moved by 53 and the numerator by 53, and the missed count by zero.** That is the
whole story of 1C-2b's region figure: `cache.rs` gained 53 regions of policy and every one of them
was executed, so the layer's ratio rose from 96.81% to 96.95% and its margin over the floor grew
from 6.81 to 6.95 points. **1C-1's recorded prediction is still standing as a refutation** — it
compared a new *file's* score against a *layer total*, which is the wrong comparison, and 1C-2b
adds a third data point against it: 53 new regions at 99.44% lifted the total by 0.14 points, where
an equal-to-average addition would have given roughly 0.3.

### 2.2 Crate and layer aggregates

| Aggregate | Region cover | Line cover |
|---|---|---|
| **`core/`** | **96.95%** (1177/1214 regions) | **96.53%** (862/893 lines) |
| **`sh_nexus_wire` as a crate** | **98.95%** (188/190 regions) | **100.00%** (159/159 lines) |
| `sh_nexus` as a crate | 93.88% (1596/1700 regions) | 94.30% (1158/1228 lines) |

`core/` now covers **three** files and every one of them is a logic module: `ordering.rs` at 100%
since 1B, `markdown.rs` at 95.74% since 1C-1, and `cache.rs` at **99.44%**, up from 99.20% at 1C-2a
because the 53 regions of ceiling policy were added *and covered*. The layer is **6.95 points above
the floor**, up from 6.81, and the movement is a genuine improvement rather than a denominator
effect in the flattering direction: 1C-1 and 1C-2a both moved this aggregate the *other* way, and
both are recorded in §2.7 rather than quietly overwritten.

`sh_nexus` as a crate is at **93.88% (1596/1700)**, up 0.20 points from a **corrected** 1C-2a
baseline of 93.69% (1543/1647). **§4.9 records why the correction was necessary and why 1C-2b nearly
published a fall built on the uncorrected figure.** The crate aggregate is still dominated by
`lib.rs` at 60.81% and `main.rs` at 0.00% — the window-opening path §4.1–§4.2 characterise and
Phase 2 moves — so it remains arithmetic rather than a signal, and §4.9 says what to gate on instead.

It is 6.95 points above the floor. `ordering.rs` remains the highest-scoring file in the
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

**This is the list that made §3.3 unmeasurable in 1A, and it is now three files shorter.**
Three
files have left this category: `core/ordering.rs` in 1B (now §2.1 at 100.00%), `core/markdown.rs` in
1C-1 (now §2.1 at 95.74%) and `core/cache.rs` in 1C-2a (now §2.1 at 99.44%, and §4.8).
**Every `core/` file that has ever contained executable code is in the region report**, and every
remaining member of the layer is still type definitions and `pub mod` declarations. `core/theme.rs`
(1D) is the only logic module left to write; §2.9 states the falsifiable prediction for when it lands,
in both directions.
### 2.4 Reconciliation note on the line column

**The tool's own TOTAL has always reconciled in both columns, and this recording is the first in
which the *document's* hand arithmetic reconciles too.** That distinction matters, because 1A
reported a non-reconciling *line* column and three later recordings recorded that the anomaly had
gone away "without explaining it". The explanation is now available and it is not in the tool.

**§2.4's original hand arithmetic omitted `core/cache.rs` from the line column entirely.** It read:

```text
lines:        618 + 143 +  99 +   7 + 229 +   3 + 139 +  17 = 1255   <- cache.rs absent
missed:        31 +   0 +  32 +   7 +   0 +   0 +   0 +   0 =   70
covered:     1255 −  70 = 1185       1185 / 1255 = 94.42%
```

`core/cache.rs` did not exist when that arithmetic was first written — it was composed at 1C-1, when
the layer held `ordering.rs` and `markdown.rs` — and it was never revised when 1C-2a added the file.
**That is the whole cause of 1A's anomaly: a stale hand sum in this document, not a measurement
defect.** §2.1's TOTAL line figure was computed by the tool and was correct at every recording
(`1273/1343 = 94.79%` at 1C-2a), which is why the two figures in this file disagreed with each other
rather than with the tool.

Corrected, with `cache.rs` at its 1C-2a size of 88 lines:

```text
lines:         88 + 618 + 143 +  99 +   7 + 229 +   3 + 139 +  17 = 1343
missed:         0 +  31 +   0 +  32 +   7 +   0 +   0 +   0 +   0 =   70
covered:     1343 −  70 = 1273       1273 / 1343 = 94.79%   <- matches 2.1's TOTAL exactly
```

The **region** column, the one ADR-004's floors rest on, was never affected:

```text
regions:      178 + 846 + 190 + 148 +   7 + 331 +   4 + 169 +  17 = 1890
missed:        1 +  36 +   0 +  58 +   7 +   2 +   0 +   2 +   0 =  106
covered:     1890 − 106 = 1784       1784 / 1890 = 94.39%   <- matches 2.1's TOTAL exactly
```

**The consequence, unchanged and now on firmer ground.** ADR-004's floors are stated as region
coverage, the region column reconciles to the last decimal from a single report, and **§3's verdicts
rest on regions**. The line column now reconciles as well, so it is no longer disqualified from
gating — but it is still recorded as secondary, because the lesson of §2.4 is that a hand-kept sum
in a coverage document decays silently, and that is a property of the document rather than of the
column.


### 2.5 The 1B baseline, retained for regression

1B's figures, kept verbatim so §6.3's "any coverage drop blocks the merge" has
something to drop from. **Every aggregate improved; no aggregate regressed.**

> **Scope of verification, stated rather than implied.** §4.9 found two arithmetic errors in the
> 1C-2a recording, which is the one 1C-2b compares against, and corrected both. **1C-2b did not
> re-derive the aggregates in this table or in §2.7** — they are reproduced verbatim because they are
> the historical record, and rewriting a prior recording's figures destroys the thing the record is
> for. **No claim is made that they are correct.** A full audit of 1A, 1B and 1C-1's aggregates is
> an open action, and it is recorded as one in §5.5 rather than left implied by the fact that the two
> most recent recordings are now clean.

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
| 2 missed regions (1C-2b, both runs) | 4 | **94.39%** |

Ten measurements, different commits, same tool, same environment, mostly different numbers — with
the two most recent agreeing, which is the anomaly this section now has to account for. The
workspace TOTAL column is not comparable across rows, because the denominator grew with each work
unit; only the `mapping.rs` column is, and it is the one that matters.

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

**The tables in §2.1–§2.2 report the 1C-2b measurement (1890 regions, 106 missed, 94.39%),** and
**the two runs taken in 1C-2b were byte-identical — including `mapping.rs`, the file this whole
section is about.** That is the first recording in which the fluctuation did not reproduce, across
ten measurements on four commits, and it is recorded here rather than being quietly dropped,
because a section that quietly drops its own most consequential finding is worse than one that
never had it.

**What 1C-2b's non-reproduction does and does not establish.** It does *not* refute the finding:
two agreeing runs after eight disagreeing ones is weak evidence, and the honest position is the one
1A took about `frame.rs`'s residuals — *not verified either way*. It does establish that **the
fluctuation is intermittent rather than constant**, which is what §2.6's inference about proptest's
per-run randomness already predicted and could not confirm. The most likely explanation remains the
one §2.6 gives, and the file says so.

**Practical consequence, unchanged and now better supported.** A gate on the exact decimal of
`mapping.rs` would have fired a phantom regression in roughly one run in three across 1B, 1C-1 and
1C-2a, and did not fire in 1C-2b — which is the worst possible property for a merge gate, because it
is unreliable in the one direction that wastes a reviewer's time. **The two recommendations stand:**

1. **A fixed seed, or the committed regressions files.** 1C-2b took the second option for free:
   `crates/sh_nexus/tests/cache_ceiling.proptest-regressions` now exists and is committed, and
   `ordering.proptest-regressions` and `proptest_boundary.proptest-regressions` already did. A
   regressions file makes the *recorded* cases deterministic and leaves novel generation random, so
   it is a partial fix and a cheap one — **which is why it is recommended rather than a global
   fixed seed.**
2. **The per-file gate on `mapping.rs` should be the floor, not the decimal.** ADR-004 states
   `network/`'s floor at 80%; the measurement is 99.40% at the modal figure. The floor is stable, the
   decimal is not, and §6.3 should be expressed in terms of the one that does not move.

**The 1C-1 range for the workspace TOTAL was 93.81–93.87%,** and every floor in §3 passed at the
lower end as well. **The 1C-2b figure has no range**, because both runs agreed; the floors would pass
at any figure this tool has produced for this tree.

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


### 2.8 The 1C-2b delta, including the one aggregate that fell

The new numbers first, then the one that moved the wrong way, stated without softening.

| Aggregate | 1C-2a | 1C-2b | Δ |
|---|---|---|---|
| `sh_nexus\src\core\cache.rs` | 99.20% / 100.00% (125 regions, 1 missed, 88 lines) | **99.44% / 100.00%** (178 regions, 1 missed, 132 lines) | **+0.24 / none** |
| `sh_nexus\src\core\markdown.rs` | 95.74% / 94.98% | 95.74% / 94.98% | **none** |
| `sh_nexus\src\core\ordering.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| `sh_nexus\src\lib.rs` | 60.81% / 67.68% | 60.81% / 67.68% | **none** |
| `sh_nexus\src\main.rs` | 0.00% / 0.00% | 0.00% / 0.00% | **none** |
| `sh_nexus\src\network\mapping.rs` | 99.40% / 100.00% (modal) | 99.40% / 100.00% | **none** (see §2.6) |
| `sh_nexus_wire\src\error.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| `sh_nexus_wire\src\frame.rs` | 98.82% / 100.00% | 98.82% / 100.00% | **none** |
| `sh_nexus_wire\src\version.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| **`core/` as a layer** | **96.81%** (1124/1161) | **96.95%** (1177/1214) | **+0.14** |
| `sh_nexus_wire` as a crate | 98.95% / 100.00% | 98.95% / 100.00% | **none** |
| `sh_nexus` as a crate | **93.69% / 94.09%** (1543/1647) — **corrected, see §4.9** | **93.88% / 94.30%** (1596/1700) | **+0.20 / +0.21** |
| **Workspace total** | **94.23% / 94.79%** (1837 regions, 106 missed) | **94.39% / 94.95%** (1890 regions, 106 missed) | **+0.16 / +0.17** |

**Eight rows are unchanged to the decimal**, and that is the control. 1C-2b touched no line of
`ordering.rs`, `markdown.rs`, `lib.rs`, `main.rs`, `mapping.rs` or any wire-crate file, and the tool
reports exactly what 1C-2a reported for all eight. A coverage table whose untouched rows moved would
mean the denominator moved, and then no delta in the table could be read.

**`core/` rose by 0.14 and that is a real improvement, not a denominator effect.** Both the
numerator and the denominator grew by 53 and the missed count grew by **zero**: the 53 regions of
ceiling policy and thread-safety decision were all executed. This is the mirror image of 1C-1 and
1C-2a, where a new file entered the denominator below the layer average and pushed the aggregate
down; here an existing file gained well-covered regions and pushed it up. The distinction is only
visible because the missed count is reported alongside the ratio, and **a ratio without its missed
count cannot tell the two apart** — which is the practical form of the lesson of §2.1.1.

**`sh_nexus` as a crate rose 0.20 points, and only after its 1C-2a baseline was corrected.** The
published 1C-2a figure was 93.96% (1571/1672), and **neither number in that fraction matches the
1C-2a per-file table**: the table sums to 1647 regions with 104 missed, which is `1543/1647 =
93.69%`. On that corrected baseline the crate rose to 93.88%, and the line column rose from
`1114/1184 = 94.09%` to 94.30%. **§4.9 is the entry, and it is the more useful half of this
recording**: 1C-2b nearly published a second wrong number, and the reason it caught it is that it
recomputed the baseline from the per-file table instead of carrying the previous recording's
aggregate forward.

**A crate-level aggregate is still not a useful regression signal for this repository, and the
corrected figures say so for a better reason than the incorrect ones did.** It is dominated by
`lib.rs` at 60.81% and `main.rs` at 0.00% — the window-opening path that §4.1–§4.2 characterise and
that Phase 2 moves — so it moves when the denominator moves rather than when the code does. **The
per-layer aggregate (`core/` at 96.95%) and the per-file figures are the signals; the crate figure
is arithmetic.**

### 2.9 The 1C-2b prediction, and whether it held

1C-1 and 1C-2a both left a falsifiable prediction standing: `core/cache.rs` (1C-2) and
`core/theme.rs` (1D) would move the `core/` aggregate, and the direction was stated rather than left
to be discovered. **1C-2a falsified the first half of that prediction** by extending `cache.rs`
rather than only adding it, so the question for 1C-2b was whether 53 well-covered regions inside an
existing file would lift the layer as they would have lifted a new one. **They did: +0.14 points,
and the prediction that a well-covered addition lifts the total held.**

`core/theme.rs` (1D) is the remaining half and is untouched. Its prediction stands unchanged: a new
file at `markdown.rs`'s ~96% would pull the layer down by roughly 0.5 points and a file at
`ordering.rs`'s 100% would push it up by roughly 0.4, on a denominator of about 1,214. **Both
directions are recorded in advance so that 1D cannot quietly report whichever one it likes.**

### 2.10 The 1D delta, and the score of §2.9's prediction

| Aggregate | 1C-2b | 1D | Δ |
|---|---|---|---|
| `sh_nexus\src\core\theme.rs` | *did not exist* | **98.93%** regions / 99.78% lines (842 regions, 9 missed, 463 lines, 1 missed) | new |
| `sh_nexus\src\core\cache.rs` | 99.44% / 100.00% | 99.44% / 100.00% | **none** |
| `sh_nexus\src\core\markdown.rs` | 95.74% / 94.98% | 95.74% / 94.98% | **none** |
| `sh_nexus\src\core\ordering.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| `sh_nexus\src\errors.rs` | **no report row** — see §2.10.1 | 100.00% / 100.00% (3 regions, 0 missed) | new |
| `sh_nexus\src\lib.rs` | 60.81% / 67.68% | 60.81% / 67.68% | **none** |
| `sh_nexus\src\main.rs` | 0.00% / 0.00% | 0.00% / 0.00% | **none** |
| `sh_nexus\src\network\mapping.rs` | 99.40% / 100.00% (modal) | 99.40% / 100.00% | **none** (see §2.6) |
| `sh_nexus_wire\src\error.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| `sh_nexus_wire\src\frame.rs` | 98.82% / 100.00% | 98.82% / 100.00% | **none** |
| `sh_nexus_wire\src\version.rs` | 100.00% / 100.00% | 100.00% / 100.00% | **none** |
| **`core/` as a layer** | **96.95%** (1177/1214) | **97.76%** (2010/2056) | **+0.81** |
| `sh_nexus_wire` as a crate | 98.95% / 100.00% | 98.95% / 100.00% | **none** |
| `sh_nexus` as a crate | 93.88% / 94.30% (1596/1700) | **95.56% / 95.81%** (2432/2545) | **+1.68 / +1.51** |
| **Workspace total** | **94.39% / 94.95%** (1890 regions, 106 missed) | **95.80% / 96.17%** (2735 regions, 115 missed) | **+1.41 / +1.22** |

**Eight rows are unchanged to the decimal, and that is the control.** 1D touched no line
of `cache.rs`, `markdown.rs`, `ordering.rs`, `lib.rs`, `main.rs`, `mapping.rs` or any
wire-crate file, and the tool reports exactly what 1C-2b reported for all eight. §2.6's
instability in `mapping.rs` did **not** reproduce: two runs in 1D were byte-identical
including that row, at its recorded modal value of 2 missed regions.

**Every aggregate rose, and the crate-level rise is mostly not `theme.rs`.** `sh_nexus` as
a crate gained 1.70 points, while `theme.rs` alone is 842 of 2545 crate regions at 98.93% —
which contributes about +0.4. The remaining ~1.3 points come from §2.10.1, and reading the
crate figure as "1D's new file was good" would be the same misreading §2.8 warned about.
**The per-layer and per-file rows are the signals; the crate figure is arithmetic** — and
this recording is a second demonstration of why, in the direction that flatters.

**§2.9's prediction, scored.** 1C-1 and 1C-2a left a falsifiable two-branch prediction:
*"a new file at `markdown.rs`'s ~96% would pull the layer down by roughly 0.5 points and a
file at `ordering.rs`'s 100% would push it up by roughly 0.4, on a denominator of about
1,214. Both directions are recorded in advance so that 1D cannot quietly report whichever
one it likes."*

`theme.rs` landed at **98.93%**, between the two anchors, and pushed `core/` **up 0.81**.

- **The sign was predicted correctly.** The prediction's real content was "this will not
  fall", and it did not fall.
- **The magnitude fell outside both stated branches**, and above the optimistic one. +0.81
  against a predicted +0.4 is roughly double. The prediction's arithmetic assumed the new
  file's size, and it under-estimated it by a factor of seven: 842 regions against the
  ~1,214 *whole layer* the note used as its denominator, where 1C-1's `markdown.rs` had
  been 846 at a comparable scale but arrived on a layer three times smaller.
- **So the prediction is recorded as half-refuted.** It got the direction right on
  evidence, and it framed the outcome as binary between two anchors that were never going
  to bracket it. A falsifiable prediction that survives only its sign is worth less than
  one that states a range — which is the form the next one should take.

#### 2.10.1 `errors.rs` left §2.3's list, and why that is a real change

`sh_nexus\src\errors.rs` was in §2.3's category of files that produce **no report row at
all**, because until 1D it held no executable code: a `#[derive(Error)]` enum, a type alias,
and a doctest. 1D added `impl From<ThemeError> for ShNexusError`, which is three regions
and is executed by `a_theme_rejection_converts_into_the_project_error_type`.

This is a **new row in §2.1** rather than a change to an existing one, so §2.3 is not
edited. It is worth its own paragraph because the mechanism is the one §5.3's closing note
is about: a coverage table can only work on code that exists, and the single most
consequential structural decision 1D made — that `core/` cannot name `ShNexusError`, so the
conversion lives on the other side of the boundary — is enforced by *tests*
(`core_reaches_only_its_own_modules`, `errors_is_a_sibling_of_core_not_a_member_of_it`) and
therefore has no coverage row either. **The `From` impl is the visible half; the absence of
a `use crate::errors` in `core/` is the invisible half, and only the second one is
load-bearing.**

#### 2.10.2 1D's proptest regressions file, and what is in it

`crates/sh_nexus/tests/theme.proptest-regressions` is **committed, and §1's rule about
the 1C-2b file applies to it unchanged**: the seeds are replayed before any novel case
is generated, so the suite is more reproducible than it was before them, and the file
must not be deleted as noise.

It holds **two** seeds, and both are defects in the *test suite*, not in the code under
test — which is the first time this project has recorded that, and the reason is worth
stating:

| Seed | The failure it pins | How it was found |
|---|---|---|
| `954b9f15…` | The generator emitted `"colors":{… thirteen entries…,}` — a **trailing comma**, which JSON forbids. Every generated document was invalid | `a_generated_valid_theme_always_parses_to_what_it_encoded` |
| `08a08fde…` | `arbitrary_label` could produce a **whitespace-only** label, which the schema correctly refuses — so the property was failing *for the correct reason* with a misleading message | the same property |

**The second is the one worth keeping as a lesson, and it is the same shape as M9's in
§5.6.1.** A generator that produces inputs the specification *forbids* will fail its
property for the right reason with a message that points at the wrong thing, and the
instinct is to blame the validator. Here the validator was right and the generator was
wrong, and the fix was to anchor the generated label on a leading letter. **A property's
generator has to be checked against the specification, not only against the
implementation** — the implementation is what the property is testing, so agreement with
it is not evidence the generator is right.

Both seeds are now replayed on every run, so neither defect can come back silently.

### 2.11 The four reconciliation sums, printed

The invariant §1 adopted after two arithmetic errors: **crate + wire must equal the tool's
TOTAL in the denominator and in the covered count, in both columns.** Derived aggregates
are demonstrated against the TOTAL before they are written. Four sums, from §2.1's rows:

```text
regions:  178 + 846 + 190 + 842 +   3 + 148 +   7 + 331 +   4 + 169 +  17 = 2735
missed:     1 +  36 +   0 +   9 +   0 +  58 +   7 +   2 +   0 +   2 +   0 =  115
covered: 2735 − 115 = 2620          2620 / 2735 = 95.80%   <- matches 2.1's TOTAL exactly

lines:    132 + 618 + 143 + 463 +   3 +  99 +   7 + 229 +   3 + 139 +  17 = 1853
missed:     0 +  31 +   0 +   1 +   0 +  32 +   7 +   0 +   0 +   0 +   0 =   71
covered: 1853 −  71 = 1782          1782 / 1853 = 96.17%   <- matches 2.1's TOTAL exactly
```

And the two crates against each other, in both columns:

```text
sh_nexus      2545 regions (113 missed)  +  sh_nexus_wire   190 regions (  2 missed)  =  2735 (115)  OK
               1694 lines   ( 71 missed)  +                  159 lines   (  0 missed)  =  1853 ( 71)  OK
               2432 covered               +                  188 covered               =  2620         OK
```

**The three aggregates §2.2 publishes, each derived from the rows above rather than
carried forward, and each cross-checked against the TOTAL:**

```text
core/ as a layer:    178 + 846 + 190 + 842 = 2056 regions;  1 + 36 + 0 + 9 = 46 missed
                     2056 − 46 = 2010       2010 / 2056 = 97.76%
                       132 + 618 + 143 + 463 = 1356 lines;  0 + 31 + 0 + 1 = 32 missed
                       1356 − 32 = 1324       1324 / 1356 = 97.64%

sh_nexus as a crate: 2545 regions, 113 missed  ->  2432 / 2545 = 95.56%
                      1694 lines,    71 missed  ->  1623 / 1694 = 95.81%
                      (2545 + 190 = 2735 and 113 + 2 = 115: the crates sum to the TOTAL above)

sh_nexus_wire crate:  190 regions,  2 missed  ->   188 /  190 = 98.95%
                       159 lines,    0 missed  ->   159 /  159 = 100.00%
```

**Both columns reconcile, for the fourth consecutive recording, and the line column
reconciles as §2.4 established rather than by accident.** The region column — the one
ADR-004's floors rest on — is the one every verdict in §3 uses.

---

## 3. Verdict against each floor

### 3.1 `AGENTS.md` §6.1 — workspace total: 75% minimum / 85% target

**94.39% region coverage — PASSES THE TARGET.** 9.39 points above the 85% target and 19.39 points
above the 75% minimum, and 0.16 points above the 1C-2a baseline. The line column reads 94.95% and
points the same way.

**The two runs taken in 1C-2b were byte-identical**, so unlike 1C-1 through 1C-2a there is no range
to quote: the figure is 94.39% and not 94.39%-something. §2.6 explains what changed, and it is worth
saying that the file's most consequential finding **failed to reproduce** rather than being quietly
dropped.

### 3.2 ADR-004 — `sh_nexus_wire` ≥80%

**98.95% region coverage — PASSES.** 18.95 points above the floor, unchanged since 1B. See §2.2.
This is the floor that matters most (§3.4), because `AGENTS.md` §4.2's serde round-trip mandate and
§7.4's version rejection both live in this crate.

**It is not 100%, and the reason is recorded in §4.4.** 1A predicted that closing the three named
residuals would take this crate to 100% region coverage. Measurement says otherwise: 188/190. The
prediction is refuted by 2 regions, and §4.4 says what is and is not known about them.

**1C-2b changed nothing in this crate**, which is the fourth consecutive recording in which it is
unchanged. 1C-2b added no dependency and touched no wire-crate line, and the two figures
`frame.rs`'s residual produces are the same two regions 1B found.

### 3.3 `AGENTS.md` §4.1 — `core/` ≥90%: **MEASURABLE, AND MET — for all three files**

**`core/` measures 96.95% — 1177 of 1214 regions, 862 of 893 lines, 127 of 127 functions executed,
37 regions missed. The 90% floor passes with 6.95 points of margin, up from 6.81.**
`core/ordering.rs` is still 190/190, `core/markdown.rs` is still 810/846, and `core/cache.rs` is
now **177/178** across a file that grew by 53 regions since 1C-2a.

What is new in 1C-2b, stated precisely so the number is not read as more than it is:

- **The floor is now met for every logic module in the layer, and the aggregate moved the *right*
  way.** `core/cache.rs` gained 53 regions of memory-ceiling policy and gained coverage with them:
  1 missed region at 1C-2a, 1 missed region at 1C-2b. §2.8 is the arithmetic. **A ratio reported
  without its missed count could not distinguish this from 1C-1's fall**, and that is the practical
  form of §2.1.1's lesson.
- **The strictest floor in the project has a denominator worth the name, and every file in it is
  logic.** 90% against pure code with no excuse available, met with three logic modules — not one,
  not two — in the denominator. The two remaining `core/` files contain no executable code (§2.3).
- **The `core/` architectural obligations needed no widening in 1C-2b, and that is a result rather
  than an omission.** The ceiling policy is `Option<u64>` comparisons and a `while` loop: no new
  dependency, so `CORE_ALLOWED_CRATES` is untouched. ADR-003's boundary held for 53 new regions of
  policy without a single edit to the allow-list that guards it.
- **The thread-safety decision is enforced mechanically, and coverage is the wrong instrument for
  it.** `AGENTS.md` §4.2's cache row ends with "thread safety" and 1C-2b decided it: no internal
  synchronisation, on the grounds that `PLAN.md` §4 makes `state/bridge.rs` the sole owner of
  `cx.update_global`. Two tests hold that, and neither is a coverage number:
  `the_bounded_cache_is_send_and_sync` is a **compile-time** assertion that fails to build if a
  `Cell` or an atomic ever appears, and
  `core_cache_contains_no_interior_mutability` is a **structural** guard that fails the build if the
  tokens `Mutex`, `RwLock`, `RefCell`, `Cell<`, `UnsafeCell`, `OnceCell` or `LazyLock` ever appear
  in `core/cache.rs`. 1C-2b rewrote the second one's failure message, because it still said the
  decision was 1C-2b's and that had stopped being true.
- **The security-relevant work is the part coverage cannot see, and this file says so rather than
  letting 95.74% stand in for it.** `core/markdown.rs` is a parser for attacker-controlled text.
  Its correctness is pinned by 241 tests, of which the mandatory ones are named in `AGENTS.md` §4.2
  — bold, italic, code, links, nested formatting, never panics, injection safety — and by six
  proptest properties over arbitrary input. §4.7 says which claim each one carries. A coverage
  number would be the wrong instrument for all of it, and 1A §4.6 already made this point about
  `ordering.rs`.
- **`core/`'s remaining logic module is `theme.rs` (1D).** It does not exist, so this floor is met
  for three of the layer's four files and §2.9 states the falsifiable prediction for when it lands —
  in **both** directions, so that 1D cannot report whichever one it likes.

### 3.4 Summary

| Floor | Source | Measured | Verdict |
|---|---|---|---|
| Workspace total ≥75% min / ≥85% target | `AGENTS.md` §6.1 | **94.39%** regions (two identical runs) | **PASSES target** |
| `sh_nexus_wire` ≥80% | ADR-004 decision 3 | **98.95%** regions | **PASSES** |
| `core/` ≥90% | `AGENTS.md` §4.1, ADR-004 decision 2 | **96.95%** regions (1177/1214) | **PASSES — 37 missed, 36 of them markdown's defensive arms (§4.7)** |
| New code ≥80% | `AGENTS.md` §5.1, ADR-004 | `cache.rs` **99.44%** (177/178) | **PASSES** |
| `network/` ≥80% | `AGENTS.md` §4.1 | `mapping.rs` **99.40%** regions | **PASSES** |
| `state/`, `db/`, utilities ≥80/85% | `AGENTS.md` §4.1 | directories do not exist yet | Not applicable in 1C-2b |

**Every floor with a denominator passes in 1C-2b**, and none of them passes by a
margin that depends on a file nobody can test: the two sub-80% files are
`lib.rs` and `main.rs`, both characterised in §4.1–§4.2 as the window-opening
path that Phase 2 moves, and neither is in a directory §4.1 names.

### 3.5 The 1D verdicts, added rather than folded into §3.1–§3.4

§3.1–§3.4 are the 1C-2b verdicts and are left as they were. These are the 1D
measurements against the same floors, each recomputed from §2.1's rows at the
moment of writing per §1's rule.

| Floor | Source | 1C-2b | **1D** | Verdict |
|---|---|---|---|---|
| Workspace total ≥75% min / ≥85% target | `AGENTS.md` §6.1 | 94.39% | **95.80%** regions (2620/2735, two identical runs) | **PASSES target, +10.80** |
| `sh_nexus_wire` ≥80% | ADR-004 decision 3 | 98.95% | **98.95%** (188/190) | **PASSES — unchanged, no wire file touched** |
| `core/` ≥90% | `AGENTS.md` §4.1, ADR-004 decision 2 | 96.95% (1177/1214) | **97.76%** (2010/2056) | **PASSES — +7.76 over the floor** |
| New code ≥80% | `AGENTS.md` §5.1, ADR-004 | `cache.rs` 99.44% | **`theme.rs` 98.93%** (833/842) | **PASSES** |
| `network/` ≥80% | `AGENTS.md` §4.1 | `mapping.rs` 99.40% | `mapping.rs` **99.40%** | **PASSES — unchanged** |
| `state/`, `db/`, utilities ≥80/85% | `AGENTS.md` §4.1 | n/a | directories do not exist yet | Not applicable in 1D |

**Three things the 1D numbers say that the 1C-2b ones did not.**

1. **The `core/` floor's margin grew for the right reason, and the reason is visible in
   the missed count.** 1C-2b's 37 missed regions were 36 of them `markdown.rs`'s defensive
   arms. 1D's 46 are 36 of those, 9 of `theme.rs`'s, and 1 of `cache.rs`'s — so of the 842
   regions 1D added, **833 were executed and 9 were not**, and the layer's numerator grew by
   833 against a denominator that grew by 842. That ratio is the honest statement of what
   "the new code is well covered" means, and §2.10's is the number a ratio alone would hide.
2. **The workspace total is now 10.80 points above its target, having been 9.39 above it
   three recordings ago**, and the line column agrees (96.17%). Both columns reconcile in
   §2.11, which is the fourth consecutive recording where they do.
3. **`core/` at 97.76% is the first time the layer has cleared `cache.rs`'s 99.44%-class
   files on a *sustained* basis rather than as a one-file artefact.** 1B's 100.00% was a
   statement about a single file (§2.7 says so explicitly); the layer has now been measured
   at 96.95%, 96.95% and 97.76% across three recordings with two, three and four logic
   modules in it. **That is the number ADR-004's floor should be gated on**, and it is the
   first figure in this file that means what §2.10's header claims for it.

**Still not applicable, and still not a pass:** §4.1 and §4.2's two sub-80% files are
untouched by 1D and are Phase 2's to move (§5.4).


---

## 4. The sub-floor files, characterised

**Two files sit below 80% in 1C-2b, down from four in 1A, and the count has not moved in three
work units.** Both are structural: the window-opening
path that Phase 2 restructures into `src/app.rs`, in a file §4.1's directory-keyed floors do not
name. None of them is a quality problem in the code that was written, and **neither has been touched
since 1A** — both are byte-identical to 1A and measure identically (§2.8).

The three that 1A characterised here and that are now **closed or nearly so** —
`version.rs` at 76.47%, `frame.rs` at 94.08%, and the nine-line residual of §5 — are §4.3, §4.4 and
§5 below. The file that 1A called *"the strongest file in the baseline"* is still strongest, at
`mapping.rs` §4.5, and is unchanged to the decimal.

### 4.1 `sh_nexus\src\lib.rs` — 60.81% regions, 67.68% lines

**Unchanged in 1C-2b, and unchanged for the reason 1A gave:** no work unit since 1A has touched a
line of this file, and the tool reports exactly what it reported then (§2.8). Four consecutive
recordings now report 60.81% / 67.68% to the decimal, which is the strongest control in this file.

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

**Unchanged in 1C-2b, to the modal figure** (§2.8): no work unit since 1A has touched a line of
it, and the two missed regions remain the two 1A recorded. **This was the one file whose figure was
not reproducible to the decimal — see §2.6** — and 1C-2b's two runs did not reproduce the
fluctuation either, so the "modal" qualifier is retained rather than upgraded to "exactly" on the
strength of two agreeing runs.

### 4.6 `sh_nexus\src\core\ordering.rs` — 100.00% regions, 100.00% lines

**190 regions, 0 missed, 143 lines, 0 missed, 18 functions executed. Unchanged to the decimal in
1C-1, in 1C-2a and in 1C-2b** — three consecutive recordings, which is the control §2.8 relies on. It
is in this section because §4 is where the *detail* behind a number lives.

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

### 4.8 `sh_nexus\src\core\cache.rs` — 99.44% regions, 100.00% lines

**178 regions, 1 missed, 30 of 30 functions executed, 132 of 132 lines, 0 missed lines.** At 1C-2a
this file was 125 regions, 1 missed, 20 of 20 functions, 88 of 88 lines. It is in this section
because a coverage number is a particularly poor description of what a *cache policy* has to get
right, and because the single missed region is the same one 1C-2a recorded.

**The one missed region is unchanged and still deliberate.** It is the `None` arm of the `if let` in
`promote`, unreachable because every caller has already established the key is live. Removing it
would trade an `AGENTS.md` §2.1 safety property for one region, which is the wrong trade.

**What 100% of lines does not tell you about a policy, stated as a table rather than a paragraph.**
Every region the instrumenter emitted was executed, which for this file means the refusal check, the
combined trim loop, `set_budget`'s immediate eviction, both over-budget tests and the eviction
counter all ran. It does **not** mean the policy is right: a cache that evicted the wrong entry, or
admitted an oversized one, or forgot to discharge a cost, would execute every one of those regions
and report the same 177/178. **That is the whole reason §5.3 exists**, and §5.3's table is the
instrument for the claims below.

| Claim | Held by | Kind |
|---|---|---|
| An entry costing more than the whole budget is never admitted | `an_entry_whose_own_cost_exceeds_the_budget_is_never_admitted` (5 cases), `an_oversized_entry_is_refused_at_any_point_of_any_sequence` | 5 cases + **proptest** |
| A refusal changes *nothing* — not cost, length, counters, or the live set | `a_refused_insert_leaves_the_cache_exactly_as_it_found_it`, `a_refused_insert_moves_no_counter_at_all` (2), `a_refused_insert_changes_nothing_at_any_point_of_any_sequence` | hand-written + **proptest** |
| A refused *replacement* leaves the old value resident | `a_refused_insert_leaves_the_cache_exactly_as_it_found_it`, `the_outcome_separates_a_refusal_from_a_replacement` (5) | hand-written |
| A refusal does not disturb the recency order | `a_refused_insert_does_not_change_which_entry_the_next_eviction_takes` (2) | behavioural, because the order is not exposed |
| A lowered budget is honoured on the same call, with no window | `lowering_the_budget_honours_it_on_the_same_call` (4), `a_lowered_budget_is_honoured_wherever_it_lands_in_a_sequence` | 4 cases + **proptest** |
| A raised or removed budget evicts nothing | `setting_a_budget_the_cache_is_already_within_evicts_nothing` (4), `removing_the_budget_evicts_nothing_and_stops_enforcing`, `a_raised_or_removed_budget_evicts_nothing_at_any_point_of_a_sequence` | cases + **proptest** |
| The two bounds compose; `min(capacity, budget)` is what survives | `the_ceiling_and_the_capacity_bound_the_cache_together` (8), `no_operation_sequence_can_disagree_with_an_independently_derived_model` | 8 cases + **proptest** |
| A costless entry is bounded by capacity alone — and this is a stated limit, not a hole | `a_zero_cost_entry_is_bounded_by_capacity_and_by_nothing_else` (4) | cases |
| One insert evicts at most the entries it displaced | `one_insert_never_displaces_more_entries_than_it_holds`, `one_insert_never_evicts_more_entries_than_it_displaced` | hand-written + **proptest** |
| The worst-case cascade is reached only by near-free declared costs, and its size is *measured* | `the_worst_case_eviction_cascade_is_exactly_what_the_adversarial_shape_predicts` (4) | cases, asserting an **exact** count |
| The ceiling holds after **every** step, not only at the end of a sequence | `no_operation_sequence_can_leave_the_cache_over_its_budget` | **proptest** |
| Adding the policy did not perturb 1C-2a's mechanism | `a_budget_loose_enough_never_to_bind_changes_nothing` | **proptest** |

**The one row that is a test of a *limit* rather than of a behaviour**, and it is the most
interesting thing 1C-2b found: `total_cost` is an *incremental* accumulator with saturating
arithmetic, and saturation is **path-dependent**. With no ceiling, insert `u64::MAX` then `1` and
the accumulator reads `MAX + 1` as `MAX`; remove the `MAX` entry and it reads `MAX − MAX` as `0`
while the one live entry really costs `1`. So in the saturating regime `total_cost()` is not a
function of the live set at all, **and no reference model can predict it.** This was found by the
model property failing, not by inspection, and it is handled three ways rather than one:

1. **Stated** in `core/cache.rs` §12, with the direction of the error named: the report
   *under*-states, so a cache can in truth hold more than its budget says.
2. **Pinned** by `the_ceiling_is_enforced_on_the_reported_total_even_when_the_true_sum_overflows`
   (2 cases) and by `the_saturating_regime_needs_a_budget_no_client_would_configure`, which does
   the multiplication: at `AGENTS.md` §6.2's ~2^27 budgets, exceeding `u64::MAX` needs about
   `2^37` entries.
3. **Kept out of the model property's cost generator** — and this is the part worth arguing about.
   Bounding the generator is the standard way to avoid testing a fiction, and the alternative was to
   make the model replicate a path-dependent accumulator, at which point it agrees by construction
   and tests nothing. **The cost of that choice is stated rather than hidden: the model property
   does not cover the `u64::MAX` cost, and the coverage it gives up is bought back by (2) naming the
   limit explicitly.** The three proptest seeds that recorded the original failures are committed in
   `crates/sh_nexus/tests/cache_ceiling.proptest-regressions`.

### 4.9 A second arithmetic error in the previous recording, found by recomputing its baseline

**The 1C-2a recording published `sh_nexus` as a crate at `93.96% (1571/1672 regions)` and
`94.06% (1077/1145 lines)`. Both fractions are wrong, and neither numerator nor denominator can be
reproduced from the 1C-2a per-file table it sits beside.** Recomputed from that table:

```text
regions:  125 (cache) + 846 (markdown) + 190 (ordering) + 148 (lib) +   7 (main) + 331 (mapping) = 1647
missed:     1        +  36         +    0        +  58      +   7        +   2               =  104
covered:  1647 - 104 = 1543          1543 / 1647 = 93.69%      <- published 93.96% (1571/1672)

lines:     88        + 618         + 143        +  99       +   7        + 229               = 1184
missed:     0        +  31         +   0        +  32       +   7        +   0               =   70
covered:  1184 -  70 = 1114          1114 / 1184 = 94.09%      <- published 94.06% (1077/1145)
```

**Two independent cross-checks confirm 1543/1647 and 1114/1184, and refute the published figures.**
A crate aggregate plus the other crate must equal the workspace TOTAL the tool printed, in the
denominator and in the covered count, on both columns:

```text
regions:  1647 + 190 = 1837  == the tool's TOTAL 1837            published: 1672 + 190 = 1862 != 1837
covered:  1543 + 188 = 1731  == 1837 - 106 = 1731               published: 1571 + 188 = 1759 != 1731
lines:    1184 + 159 = 1343  == the tool's TOTAL 1343            published: 1145 + 159 = 1304 != 1343
covered:  1114 + 159 = 1273  == 1343 -  70 = 1273               published: 1077 + 159 = 1236 != 1273
```

**All four checks fail for the published figures and all four pass for the recomputed ones.** The
published 1C-2a `sh_nexus` crate row was not a rounding difference; it was a different set of numbers.

**This is the second arithmetic defect in this file, and the first was in the *same* recording's
line column (§2.4).** Both are recorded rather than applied silently, and the pattern is worth
naming because it is a process failure and not a carelessness one: **§2.1's tables are written by
reading the tool's output, and §2.2's and §5.3's aggregates are written by carrying the previous
recording's aggregate forward and adjusting it.** The first is a transcription; the second is a
propagation step with no transcription to catch it. 1C-2a reported a *fall* in `sh_nexus` built on
the wrong baseline, and 1C-2b nearly reported a *fall* in the same aggregate because it trusted that
number instead of recomputing it.

**The rule this recording therefore adopts, stated so the next one cannot skip it: every aggregate
in §2.2 and every Δ in §2.5/§2.7/§2.8 is recomputed from §2.1's per-file rows, and the sum of the
crates is checked against the tool's TOTAL.** A coverage document's aggregates are derived data, and
derived data is exactly what a hand-maintained document gets wrong. §2.4 is the precedent that this
is not hypothetical: its error sat unnoticed for two work units.

**What it cost and what it changed.** The 1C-2a `sh_nexus` crate figure is 0.27 points lower than
published, so 1C-2b's movement in that aggregate is a **rise of 0.20 points**, not a fall of 0.08 —
and **no aggregate regressed in 1C-2b**: `core/` +0.14, `sh_nexus_wire` unchanged, `sh_nexus` +0.20,
workspace +0.16. That is a better result than 1C-2b was about to publish, and it is also a warning:
**the correction made the work unit look better, which is exactly the situation in which a
verification step is most likely to be skipped.**

**And the limit of what this entry establishes.** 1C-2b verified the recording it was comparing
against, and found two errors in it. **It did not verify the recordings before that**: §2.5's and
§2.7's aggregates are reproduced verbatim and are not claimed to be correct. §5.5 carries that as an
open action, because the honest generalisation from two errors in one recording is not "the others
are fine" but "aggregates in this document have been derived by hand for five work units and two of
the four most recent are wrong."

### 4.10 `sh_nexus\src\core\theme.rs` — 98.93% regions, 99.78% lines

**842 regions, 9 missed, 57 of 57 functions executed, 463 of 463 lines, 1 missed line.** In
this section for the same reason §4.8 is: a coverage number is a particularly poor
description of what a *validator* has to get right, and this file is the only one in
`core/` whose bug class is **silently accepting the wrong thing**.

**The single missed line is the documented-unreachable arm, and it is guarded by a test
rather than by a comment.** Line 1127 is `Err(failure) => Err(failure)` in
`load_or_default` — the path taken only if the *built-in default* itself fails to parse.
`every_built_in_theme_parses_and_validates` (via `every_built_in_carries_its_own_id_and_name`
and the three per-theme cases) fails the build if any of the three embedded themes stops
validating, so the arm is unreachable in a shipped binary. It is kept rather than `unwrap`ed
because `AGENTS.md` §2.1 forbids unwrapping in production paths and a build-time fixture
corrupted by a bad merge should degrade rather than crash at startup; `core/theme.rs` §6
argues the point in full. **This is the same shape as `cache.rs`'s one missed region
(§4.8): a region kept for a safety property, at the cost of one uncovered line.**

**What 99.78% of lines does not tell you about a validator — and this is the sentence
§5.6 exists to support.** Every one of those 463 lines executed, which for this file means
every rejection path in the module was taken at least once: unknown keys, missing keys,
wrong types, malformed colours, out-of-range numbers, unsupported versions, blank labels,
oversize documents, and the whole `MalformedJson` family. It does **not** mean the
validator is *correct*. A validator that accepted `#fff`, ignored `"colour"`, or let a zero
font size through would execute the very same regions and report the very same 833/842.
**A validator's failure mode is a theme that renders wrong with no error, and no coverage
number is sensitive to that.** That is the entire argument for §5.6, and §5.6's table is the
instrument for it.

**Two gaps that 785 green tests had missed, found by `--show-missing-lines` rather than by
the ratio — and this is the most transferable finding in the entry.** The first
measurement of this file was **97.74% regions / 98.49% lines**, and the report named three
unexecuted regions:

| Missed | What it actually was |
|---|---|
| `1127` | The documented-unreachable arm above. A real, accepted cost. |
| `1246`, `1248-1249` | **A genuine gap.** The `WrongType` arm inside `text()`. A *colour* field holding a JSON number was tested; a *label* field holding one was not — same rule, different helper, and the second path was never exercised. |
| `897-898` | **A genuine gap, and a worse one.** `ThemeError::TooLarge`'s `Display` arm. The variant was *constructed* by a test that matched on the enum, so the message was never rendered — **a user-facing message that no test had ever read**, in a module whose entire reason for hand-writing a parser is that its messages are the product (§2 of its own docs). |

Both were closed with four tests (`a_label_field_of_the_wrong_type_is_a_type_error`, 3
cases × 5 types; `the_size_rejection_names_the_size_and_the_limit`), taking the file to
**98.93% / 99.78%** and leaving only the unreachable arm. **The lesson is the ratio's
limit, not the two bugs:** a file at 97.74% looked fine, and 191 tests passing looked
finer, and the only thing that found either was asking the tool *which lines* rather than
*what percentage*. §5.1's whole argument — that a coverage number is not a quality
statement — has a concrete instance here, in this project's own newest file.


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

### 5.3 The `core/cache.rs` mutation table

**Why this table exists at all, and it is not decoration.** `AGENTS.md` §6.1 makes coverage a CI
metric, and §4.8's is the sharpest illustration of why that is not sufficient for this file:
**a cache policy whose eviction order is wrong, or whose refusal rule is absent, executes every
region it has and reports 177/178 all the same.** The bug class of a cache is *plausible wrong
output* — a cache that works, holding the wrong things — and a coverage number is structurally blind
to it. §5.1 and §5.2 are both about regions and lines; this is about the decisions.

**Method.** Five deliberate defects were introduced into `core/cache.rs` one at a time, the **full**
suite was run with `cargo test --no-fail-fast` (so one broken target cannot hide another), the number
of failing tests was recorded, and the defect was reverted. Each mutant is a one-line change to the
line named. **Every mutant was caught**, which is the only result that makes the table worth keeping.

| # | Deliberate defect | Line changed | Tests that caught it | From 1C-2a | From 1C-2b | Doctests |
|---|---|---|---|---|---|---|
| M1 | Evict the **most** recently used entry instead of the least | `evict_least_recently_used`: `order.remove(0)` → `order.remove(len - 1)` | **34** | 20 | 11 | 3 |
| M2 | Admit the oversized entry (refusal check disabled) | `insert_with_outcome`: `if self.exceeds_budget_on_its_own(cost)` → `if false && …` | **21** | 0 | 16 | 5 |
| M3 | Forget to discharge an evicted entry's cost | `evict_least_recently_used`: drop the `saturating_sub` | **39** | 9 | 25 | 5 |
| M4 | Store a lowered budget but do not honour it | `set_budget`: `self.trim_to_bounds()` → `let _ = …; 0` | **5** | 0 | 4 | 1 |
| M5 | Double-evict — two entries dropped per trim iteration | `trim_to_bounds`: two `evict_least_recently_used()` calls | **51** | 27 | 19 | 5 |

**Reading the table, and the three things it says that a count alone would not.**

**1. The mechanism's mutants are caught by both suites, and the policy's only by 1C-2b's — which
is the split working as intended.** M1 (wrong entry evicted) is caught 20 times by
`tests/cache.rs` and 11 times by `tests/cache_ceiling.rs`; M3 (cost not discharged) 9 and 25; M5
(double-evict) 27 and 19. **M2 and M4 are caught zero times by 1C-2a's suite, and that is correct
rather than a gap**: they are policy defects, and a suite scoped to the mechanism has no reason to
hold a policy claim. The relationship is one-directional, which is the asymmetry worth naming —
**1C-2b's suite does catch mechanism defects, because the ceiling is implemented in terms of the
eviction order, and it should.** What protects 1C-2a's mechanism from a *silent* policy change is not
that 1C-2a would notice but the dedicated property
`a_budget_loose_enough_never_to_bind_changes_nothing`, which asserts that a non-binding budget is
indistinguishable from no budget at all.

**2. M4's count of 5 is the thinnest net in the table, and it is reported rather than smoothed.** The
defect it introduces — *"a lowered budget is silently ignored"* — is precisely the kind that ships:
nothing crashes, nothing looks wrong, and the cache quietly holds 3x its stated ceiling. It is
caught by `lowering_the_budget_honours_it_on_the_same_call` (3 of its 4 cases; the fourth sets a
budget the cache is already within, so it correctly does not fire), by
`a_lowered_budget_is_honoured_wherever_it_lands_in_a_sequence` (the proptest), and by the
`set_budget` doctest. **The asymmetry with M2's 21 is the point: M2 has a whole section of tests
because the refusal rule is the decision this work unit was given, while a lowered budget is one
decision with one hand-written test and one property.** Three more catchers would cost about forty
lines; the honest reading is that 5 is enough to fail the build and thin enough that a future
refactor could plausibly thin it further without noticing.

**3. Doctests are load-bearing here, and 1C-2a did not have a single one catching a mutant.** M2 is
caught by five of them and M3 by five, because every public method on this type carries an example
that asserts on `total_cost()`. **A doctest that asserts a number is a test**, and for a structure
whose contract is a number, they are the most direct ones available. That is worth recording as a
convention rather than an accident: **the examples on `cache.rs`'s accessors are not
documentation, they are the last line of the mutation net.**

**What is *not* in this table, and should be.** No mutant targets the **thread-safety** decision,
because it is not a line of code that can be changed — it is the *absence* of a lock, and §4.8's two
guard tests (`the_bounded_cache_is_send_and_sync` and
`core_cache_contains_no_interior_mutability`) fail the **build** rather than a test, so a mutation
table has no row for them. **That is a real asymmetry in this file's methodology and it is stated
rather than papered over:** a coverage file and a mutation table both work on code that exists, and
this project's most consequential decision for `cache.rs` is code that deliberately does not.

### 5.4 Not carried forward

`lib.rs` and `main.rs` are **not** on this list, in 1C-2b exactly as in 1C-2a, 1C-1, 1B and 1A.
Their gaps are the window-opening path (§4.1, §4.2), which Phase 2 restructures into `src/app.rs`
and which no test can reach in its current shape. Adding a test for `spike_window_options` would
require a live `App`, and adding one for `main()` is not possible. These are **resolves-it-by-moving
cases, not closes-it-with-a-test cases**, and they are tracked against Phase 2's app shell instead.
Neither file has moved since 1A and both measure identically (§2.8).

### 5.5 OPEN — re-derive the aggregates of every recording before 1C-2a

**Opened by 1C-2b, and it is the only open action this recording adds.**

**1D found the second half of this, and it is a limitation of the rule 1C-2b adopted.**

1C-2b adopted the reconciliation invariant — *crate + wire equals the tool's TOTAL, in
both the denominator and the covered count, on both columns* — after §2.8 proved two
of the orchestrator's own figures unreproducible. **1D's `sh_nexus` crate row
reconciled on all four checks while its percentage was still wrong by 0.02**:
`2432 / 2545` was published as `95.58%` when it is `95.56%`.

So the invariant is **necessary and not sufficient**, and the reason is structural.
It checks *counts* against the tool, because counts are what the tool prints. A
percentage is a further derived step on top of already-verified counts, and nothing
in the invariant constrains it. The error is smaller than 1C-2a's (0.02 against
0.27) and it survives four checks that 1C-2a's did not.

**Two rules now, not one:**

1. **Reconcile the counts** against the tool's TOTAL — all four checks. This catches
   a wrong denominator, a wrong numerator, or a missing row.
2. **Derive every percentage from its own fraction, and never transcribe it
   independently.** A percentage is not a separate observation of the tool; it is
   arithmetic on two numbers that rule 1 has already verified. Writing it twice
   creates a second chance to be wrong, and that second chance is not covered.

The general lesson is the one §2.8 already stated, now with a second instance from
a different direction: **each verification rule covers the failure modes it was
built for, and the gap is always in the step just past its edge.** 1C-2a's mistake
was a hand-copied count. 1D's was a hand-copied ratio of verified counts. The
check that caught the first would never have caught the second.

§4.9 found two arithmetic errors in the 1C-2a recording: §2.4's `core/` line denominator (848 where
it should be 849) and §2.2's `sh_nexus` crate fractions (`1571/1672` and `1077/1145`, where neither
number can be reproduced from the per-file table). **Both were in derived figures — aggregates
computed by hand from a transcription of the tool's output rather than read from it** — and both sat
in the file for one to two work units before being caught, one of them by pure luck.

| What | Why it is open | What closes it |
|---|---|---|
| 1A's, 1B's and 1C-1's aggregates in §2.5 and §2.7 | **1C-2b did not verify them and makes no claim that they are correct.** Two errors in the two most recent recordings is not evidence about the three before them | Re-derive each from its own §2.1 per-file rows, and check the crates sum to the tool's TOTAL — the same four cross-checks §4.9 uses |

**This is deliberately left as an action rather than done here**, for a reason worth stating: doing
it would mean rewriting four prior recordings' figures in a document whose purpose is to be a
historical record, and **a record that is quietly corrected is no longer a record.** The 1C-2a
corrections were made because 1C-2b needed that baseline to be true; the older ones are not needed by
anything, and a reader who wants them re-derived should be able to see that they have not been.

**The cheaper structural fix, and it is the one §1 now states as a rule:** derive every aggregate
from §2.1's rows at the moment of writing, never by carrying the previous recording's figure
forward. A propagated aggregate has no transcription step to catch it, which is the whole mechanism
of both errors found here.

**1D did not touch this action and does not close it.** 1D applied §1's rule to its own
figures (§2.11 prints the four sums), and it re-derived 1C-2b's baseline from the per-file
table rather than carrying it forward — which is why §2.10's deltas are stated against
figures recomputed here rather than copied. **§2.5's and §2.7's aggregates remain
unverified**, exactly as §5.5 says, and 1D's silence on them is not a finding that they
are correct.

### 5.6 The `core/theme.rs` mutation table

**Why this table exists, and it is not decoration.** `AGENTS.md` §6.1 makes coverage a
CI metric, and §4.10 is the sharpest illustration yet of why that is not sufficient for
this file: **a validator that accepts `#fff` for a colour, ignores a misspelled key, or lets
a zero font size through executes every region it has and reports 833/842 all the same.**
The bug class of a validator is *silently accepting the wrong thing* — there is no crash,
no wrong pixel, just a theme that renders with a colour nobody chose — and a coverage number
is structurally blind to it.

**Method.** Nine deliberate defects were introduced into `core/theme.rs` one at a time, the
**full** suite was run with `cargo test --workspace --no-fail-fast` (so one broken target
cannot hide another), the number of failing tests was recorded, and the defect was reverted.
Each mutant is a one-line change to the line named, and the file's SHA-256 was compared
against the pre-mutation copy after every revert — a mistake this project has already made
once, in `docs/COVERAGE.md`, is not made twice. **Every mutant was caught**, which is the
only result that makes the table worth keeping.

| # | Deliberate defect | Line changed | Tests that caught it | Doctests |
|---|---|---|---|---|
| M1 | Accept a 3-digit shorthand colour | `from_hex`: `digits.len() != 6` → `< 6` | **4** | 1 |
| M2 | Never reject an unknown key | `reject_unknown_keys`: `unknown.clear()` after the sort | **12** | 1 |
| M3 | **Accept a zero font size** | `MIN_FONT_SIZE: u32 = 1` → `0` | **2** → **6** after §5.6.1 | 0 |
| M4 | Accept any version | `version_of`: drop the `== THEME_FORMAT_VERSION` guard | **4** | 1 |
| M5 | **Accept an over-long label** | `text()`: `if raw.chars().count() > MAX_LABEL_CHARS` → `if false && …` | **1** → **4** after §5.6.1 | 0 |
| M6 | Drop the parent prefix from a nested path | `child()`: `format!("{parent}.{key}")` → `key.to_owned()` | **77** | 0 |
| M7 | **Swap two colour fields** | `palette_from`: `surface` reads `"sidebar"` | **2** | 0 |
| M8 | **The fallback swallows the rejection** | `load_or_default`: substitute a generic `BlankText` | **1** → **9** after §5.6.1 | 0 |
| M9 | Emit an unescaped control character | `push_json_string`: `other if (other as u32) < 0x20` → `other if false` | **1** → **2** after §5.6.1 | 1 |

**The lowest number in this table is 1, and it is reported three times over rather than
smoothed away.** §5.3's minimum was 5. Three mutants here start at a single catcher, and
**two of the three landed on decisions this document and `core/theme.rs` §4 both call
load-bearing**:

- **M8 (1) — §10.2's second half.** "An invalid theme falls back to the default **with an
  error message in-app**" is two requirements, and the message is the one a fallback can
  quietly break: a generic message renders correctly and tells the user nothing. Caught by
  `an_invalid_document_yields_the_default_and_the_reason` alone.
- **M5 (1) — a §7.1 memory bound.** `MAX_LABEL_CHARS` is "no unbounded growth of
  in-memory state" applied to a hot-reloadable user file, and it was asserted for `name`
  only.
- **M9 (1) — a hand-written escaper.** A control character in a theme *name* is not
  something anyone writes, which is exactly why it needs a test.

**M3 and M7 sitting at 2 is a different and more interesting shape than M5's 1**, and it
deserves its own note. Both are *swap* or *widen* defects that keep the file compiling and
keep every region executed. M7 in particular is the mutation the schema test was designed
for — `every_colour_key_reaches_its_own_palette_field_under_its_own_path` substitutes a
distinct `#0000XX` per key precisely so that two fields exchanging values is detectable, and
it caught M7 twice (the schema test and the round-trip). **A net of 2 for a field swap is
correct rather than thin**, because the class of bug is *silent misrouting*, and the test
that catches it is a structural one that has no reason to be duplicated.

#### 5.6.1 The three thin nets, strengthened, with both numbers recorded

Three mutants were caught by exactly one test, on decisions the module's own
documentation calls load-bearing. **All three were strengthened and re-measured, and both
numbers are in the table above.** This is recorded as a process entry rather than a
results entry, because the thing worth keeping is not the improved numbers — it is that
**the mutation table found a weakness the coverage number and the test count both called
fine.**

| Mutant | Weakness the table exposed | The fix | Cost |
|---|---|---|---|
| M8 | §10.2's in-app message was asserted for **one** rejection kind (a misspelled key) and one fixture. A fallback that special-cased *that* error would pass | `the_fallback_reports_the_real_reason_not_a_generic_one` — 8 `#[case]`s, one per rejection kind, each asserting the message names the actual field | ~35 lines |
| M3 | The zero-font-size rejection was a **loop**, so it counted as one test however many fields it covered — and the loop derived its expectation from the constant it was testing, so it could not catch a change to the constant | Split into `a_zero_font_size_is_rejected_at_every_size_field` (4 `#[case]`s), keeping the constant assertion in `the_documented_numeric_bounds_are_the_bounds_in_force` | ~20 lines |
| M5 | The label-length bound was asserted for `name` only, though `author` and `typography.family` reach it through the same helper | `an_over_long_label_is_refused_at_every_label_field` and `a_label_at_the_limit_is_accepted_at_every_label_field` (3 `#[case]`s each), plus `the_label_limit_is_the_documented_number` | ~45 lines |
| M9 | `arbitrary_label` generated only `\n`, `\r` and `\t`, which have their **own arms** in `push_json_string` — so the property never reached the generic `< 0x20` arm | Added `\u{7}` and `\u{1}` to the generator, so the property now covers an encoding `to_json` does not itself emit | 2 lines |

**M9's fix is the one worth copying, and it cost two lines.** A generator that only
produces the cases the implementation already handles is testing the happy path twice:
`\n`, `\r` and `\t` have dedicated arms, so removing the generic control-character arm
changed nothing about them. **A property generator has to be checked against the branches
it is meant to reach, and the branch list is in the code under test** — which is the same
"read the code, do not assume" discipline §2.4's arithmetic error is an instance of.

#### 5.6.2 What is *not* in this table, and should be

- **No mutant targets the `serde` split.** `serde` remains forbidden in `core/` and
  `serde_json` is admitted, and that distinction is a **single function**
  (`forbidden_serde_mention`) plus a list entry — code whose *absence* is the invariant.
  It is covered structurally instead: `core_admits_serde_json_and_still_rejects_serde`
  in `tests/layer_boundary.rs` asserts both directions on synthetic sources, so a change
  that made the check vacuous fails a test rather than passing one. **This is the same
  asymmetry §5.3 records for `cache.rs`'s thread-safety decision**, and it is a real limit
  of mutation testing as a method: it works on code that exists, and this project's most
  consequential decisions for two of its files are code that deliberately does not.
- **No mutant targets `to_json`'s key *order*.** The canonical order is asserted by
  `the_canonical_json_is_compact_and_in_schema_order`, which is a claim about a string
  rather than a value, and a swap of two adjacent keys would keep the round-trip green.
  This is a known, accepted gap: key order has no functional consequence for a parser, and
  the test exists for diff stability rather than correctness.
- **No mutant was run twice for stability.** Each figure is a single full-suite run, which
  is the same protocol §5.3 used. The proptest properties are seeded per-case and replay
  their committed regressions first, so the counts are reproducible; a *global* seed is not
  fixed, and §2.6's recommendation of one still stands.



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


