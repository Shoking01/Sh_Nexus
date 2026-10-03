# Measured baselines — `AGENTS.md` §11

`PLAN.md` L609 makes this a **Phase 1 exit criterion**: *"Measured baselines
recorded for build time, binary size, idle RAM."*

`AGENTS.md` §6.3 then says what a baseline is *for*: *"Any increase > 5MB in
binary size requires justification"*, and *"Any regression > 10% in performance
metrics blocks the merge."* Those rules are unenforceable without a recorded
starting point, which is what this file is.

**All three are recorded here with the exact conditions of measurement. The idle-RAM
row is now measured at **both** levels *and* over §6.2's own 30-minute time base; it
remains open on two axes, and says which, rather than claiming more than it measured.**

## Binary size — recorded

| | Phase 0 (`491bd0f`, spike) | Phase 1 (`65995e4`) | Δ | §6.1 ceiling | §6.3 threshold |
|---|---|---|---|---|---|
| `sh_nexus.exe` | **9.92 MB** (10,401,792 B) | **9.95 MB** (10,430,464 B) | **+0.03 MB** (+28,672 B) | < 30 MB | > 5 MB needs justification |

**Conditions:** `cargo build --release --workspace`, 12 logical cores, `-j 6`,
`x86_64-pc-windows-msvc`. `rustc 1.98.1`.

**+0.03 MB across all of Phase 1** — five modules of `core/`, the whole state
layer, and the theme system. Under the §6.3 threshold by two orders of
magnitude, and it is the expected shape: `AGENTS.md` §6.1's own note says
*"GPUI statically links the renderer"* and that the figure is **a floor, not a
ceiling**. `PLAN.md` ADR-005 recorded the same decision at Phase 0 — *do not
lower the ceiling to reflect the 9.92 MB result*, because a 9.92 MB number that
came from a stub is not the binary this project will ship.

**The number that matters later is not this one.** Everything still missing
(`rusqlite` + bundled SQLite, TLS, `notify`, `notify-rust`, the WebSocket client)
is not in it yet, and `PLAN.md` already anticipated 12–20 MB after those land.
§6.3's 5 MB rule is measured against **this** 9.95 MB, and re-recording the
baseline at the end of each subsequent phase is what makes the comparison mean
anything.

## Build time — recorded, with a caveat that is the actual finding

| Operation | Time | Conditions |
|---|---|---|
| `cargo build --workspace -j 6`, warm, nothing to do | 14 s | 12 logical cores |
| `cargo build --workspace -j 6`, after a `gpui` touch | 50 s | `gpui` + `gpui_platform` + client |
| `cargo test --workspace --tests -j 6` | 5–12 s | 890 tests |
| `cargo test --doc --workspace -j 6` | 109–170 s | 49 doctests, no incremental cache for rustdoc |
| **`cargo build --release --workspace -j 6`** | **265–288 s** | **the number §6.1 bounds** |

**The finding is not the number, it is the margin.** `AGENTS.md` §6.1 sets
*"Build time (release): < 5 min"* as a maximum threshold. A 288 s release build is
**4 minutes 48 seconds — 12 seconds inside the limit.** That is not headroom;
that is a threshold that will be crossed by ordinary growth.

Almost all of it is `gpui` and `gpui_platform`, which are not optional (see
`docs/DEPENDENCIES.md`), so the cost is not reducible by removing a crate from
this project. **Recorded as a risk, not solved here:** either the 5-minute
threshold is a floor that a GPUI application cannot meet on this hardware, or
§6.1's release-build gate needs a stated hardware baseline alongside it. Which
one it is cannot be answered from inside the repository, and `PLAN.md` ADR-005
already refused to quietly reinterpret the thresholds — the same answer applies.

**Two measurement conditions, stated because they change the number:**

- These are **warm-incremental** builds. A true cold build (`cargo clean` first)
  is longer by an unknown amount and was **not measured**, because it destroys a
  developer's warm target directory and that is not this project's decision to
  make. `AGENTS.md` §6.1's "< 120s dev cold / < 5 min release" is therefore
  unverified against a cold build.
- `-j 6` was chosen with the user, not `-j 12`, to keep peak CPU down. On all 12
  cores both numbers above would be lower. **The recorded baseline and the
  command that produced it have to travel together**, or the comparison is
  against nothing.

## Idle RAM — not recorded at the end of Phase 1, and why that was the honest answer

> **Superseded by work units 2B, 3A and 4A. The row's own 30-minute time base
> is now measured too.**
> A bench shell yields a measured floor (51.6 MB empty, 64.4 MB with 10 000
> messages); the *application shell* is measured at **51.7 MB empty and 64.5 MB
> with 10 000 messages** by `benches/frame_time.rs --mode app` — the run that puts
> the real `app::Shell` in the window rather than a list and nothing else; and
> `benches/frame_time.rs --mode soak --soak-minutes 30` supplies the **trend** the
> two 6 s/8 s dwells could never give: 30 minutes of sustained send/ACK cycles,
> **+4.4 MB** working set, bounded caches settled, with an external monitor on the
> real pid. See §"Measured: idle RAM" and §"The 30-minute time base" further down.
> The row is closed on its *time base*; it stays open on the window's *contents*
> (the channel rail and input bar are Phase 4) and on whether that 4.4 MB is a
> plateau or a climb.

`AGENTS.md` §6.2 asks for **idle RAM < 80 MB**, measured *"over 30 minutes of
active chatting"*, and `PLAN.md` L609 lists it as a Phase 1 exit criterion.

**It was not measurable at the end of Phase 1, and a number produced then would
have been worse than no number.** `src/main.rs` is a 20-line entry point that
calls `sh_nexus::run()`. There is no window, no sidebar, no message list, no
input bar — those are Phase 2. Measuring the resident set of a stub would record
something like 15 MB and put a green-looking number against a §6.2 threshold it
has no standing to test.

**The baseline takes its meaning when there is a UI to be idle while.** The
method is settled and can be run the moment Phase 2 lands: run the release
binary, attach the OS process monitor, let it settle, sample the working set at
rest, then exercise it for 30 minutes and record both the idle figure and the
growth. §6.2's companion line — *"RAM with 10k cached messages < 200 MB"* — has
the same dependency: it needs a message list, and `core/cache.rs` has not had
anything to cache yet.

**Carried forward as the one Phase 1 exit criterion not satisfied, with the
reason attached.** A criterion recorded as met on the strength of a stub
measurement is the exact failure mode this project has been correcting for six
work units.

## What *is* recorded at the end of Phase 1

| Metric | Value | Where |
|---|---|---|
| Tests | **973** unit/integration + **49** doctests = **1 022**, 0 failures | — |
| `core/` coverage | **97.76%** regions (floor: 90%) | `docs/COVERAGE.md` |
| `state/` coverage | **98.16%** regions | `docs/COVERAGE.md` |
| `state/bridge.rs` | **100%** (185/185 regions, 29/29 functions) | `docs/COVERAGE.md` |
| Workspace coverage | **96.42%** regions | `docs/COVERAGE.md` |
| Release binary | **10.87 MB** (was 10.85 MB before work unit 3C) | this file |
| Release build | **288 s** at `-j 6` | this file |
| Dev build (warm) | 14 s | this file |
| Source | 53 `.rs` files, 37 403 lines (+941 this unit, work unit 3C) | — |
| Direct dependencies | 12 rows | `docs/DEPENDENCIES.md` |
| Idle RAM | **51.6 MB** empty / **64.4 MB** with 10k, bench shell only (floor) | this file, §"Measured: idle RAM" — added by work unit 2B |
| Idle RAM, app level | **51.7 MB** empty / **64.5 MB** with 10k, real `app::Shell` | this file, §"Measured: idle RAM" — added with `--mode app` |
| Idle RAM, 30-minute trend | **63.1 → 68.5 MB** working set (**+4.4 MB**), 3 560 samples, send/ACK cycles, bounded caches settled | this file, §"The 30-minute time base" — added with `--mode soak` |
| Scroll frame time, 10k | **1.087 ms** warm / **1.740 ms** cold, p99, bench shell only (floor) | this file, §"Measured: scroll frame time" — added by work unit 2B |
| Scroll frame time, 10k, app level | **1.239–1.614 ms** p99 over 8 runs, real `app::Shell` | this file, §"Measured: scroll frame time" — added with `--mode app` |

The coverage and test figures are re-derived from `cargo llvm-cov
--workspace --summary-only` rather than transcribed, per the two reconciliation
rules in `docs/COVERAGE.md` §5.5 — a percentage that disagrees with its own
fraction survived four correct reconciliations in work unit 1D, so every number
above is either measured here or points at the file where it is measured.

**Work unit 3C — the history bound — grew the binary by 19,968 B, which is 0.02 MB
and 0.18%, against §6.3's 5 MB threshold.** Measured: 11,373,568 B before,
**11,393,536 B** after, same command and same machine. A `pub` constant, an
eviction pass, two index-maintaining helpers, and eight tests cost about a
fiftieth of the margin. **The figure that matters for this unit is not the size
but the shape: nothing was added to a dependency graph and no crate was linked
that was not already there**, so the cost is code rather than supply chain.

**And the frame-time figure did not move, which is the number this unit could
have broken.** `benches/frame_time.rs` seeds `MAX_MESSAGES_PER_CHANNEL` — it
names the constant rather than restating 10 000, so the fixture is *exactly* the
cap and the first insert past it is the first eviction. Run after the change,
list mode: `draw_duration` p99 = **1.204 ms** over 1 030 scroll frames of 1 042
(98.8%), against §6.2's `< 8 ms` and against the 1.087–1.740 ms range this file
already records for list mode. The published measurement is therefore the point
just before the first eviction, and the p99 is inside the same bracket as before.
**Why it could not have moved is the point worth recording:** at the cap, an
arrival is one insertion and one eviction, the item count is unchanged, and
`MessageList::sync`'s extra work is two O(1) `try_read`s of a `Uuid` and an
`Option<usize>` — a scroll frame is 1 042 of those before the reader scrolls a
pixel. The eviction itself is on the *arrival* path, not the frame path, and the
bench's scroll phase produces no arrivals at all.

**The binary grew 0.86 MB in work unit 3A, and §6.3's justification threshold is
5 MB.** Measured: 9,946,368 B before, 11,325,440 B after — **+899,072 B,
+8.6%**, built with `cargo build --release --workspace -j 6` on the same machine
for both arms.

**Work unit 3B — the composer — grew it by 48,128 B, which is 0.05 MB and 0.42%,
against the same 5 MB threshold.** Measured: 11,325,440 B before,
**11,373,568 B** after, same command and same machine. A whole view, an
eight-method `EntityInputHandler` implementation, and its eight tests cost less
than a hundredth of the margin, which is the number that makes
`docs/DEPENDENCIES.md`'s decision to build a text field over `ui_input::InputField`
measurable rather than rhetorical: three more crates would have been the same
kind of judgement call, and this is what it is worth.

**The two feature declarations in that unit cost nothing here, and the reason is
that they were already paid for.** `chrono/clock` and `uuid/v4` were in the
resolved graph and in the binary before this unit — gpui's tree enables both, and
Cargo unifies features per crate version — so declaring them in
`crates/sh_nexus/Cargo.toml` changed a lie in a comment into a fact in a manifest
and moved the binary by an amount this measurement cannot distinguish from zero.
`docs/DEPENDENCIES.md` has the `cargo metadata` evidence.

The likely cause is that `core/theme.rs` was already compiled into the crate but
previously unreachable from `run()`, so the linker could drop it; `src/app.rs`
now resolves `BuiltIn::Dark` at startup, which pulls the theme parser and its
`serde_json` deserialiser into the binary. **That explanation is inference, not
measurement** — it was not established by comparing symbol tables, and it should
not be quoted as a fact without doing that. What *is* measured is the 899,072
bytes.

It is still under §6.3's threshold, so no exception is required. It is recorded
here because a baseline row that moves silently is the failure mode this file
exists to prevent.

## Message list: two figures measured, the same bench in two modes

`docs/ARCHITECTURE.md` ADR-006's step 6 makes two `AGENTS.md` §6.2 figures owed
by the message list: the **<8 ms scroll frame time at 10 000 messages**, and the
**idle RAM** this file has been carrying as deliberately unmeasured. Work unit 2B
built `crates/sh_nexus/benches/frame_time.rs` — a release-window bench that seeds
10 000 messages through the real `AppState` and `bridge::` path and scrolls the
real `MessageListView` — and ran it six times **in its list-only mode**, a window
hosting the list and nothing else. `app.rs` then landed, and the same bench gained
a `--mode app` that puts the **real `app::Shell`** in the window instead.

**Both figures are now measured twice: once as a floor and once at the level
`AGENTS.md` §6.2's row is actually written about.** The list-only numbers stay,
because they are what attributes a frame cost to the list rather than to the
window; the app-level numbers are the ones to quote for the row, with the limits
§"Measured: scroll frame time" states beside them. Neither set closes the RAM row
on its own — that row asks for idle RAM *"over 30 minutes of active chatting"*, and
a bench that dwells 6 s and 8 s measures a level, not a trend.

### Provenance of these figures — read before quoting them

Both tables above come from PR #20 (`a573bec`), and they were obtained **without
a completed peer review and without CI**. That is a real limitation on how much
weight these rows carry, and it belongs next to the numbers rather than in a
commit message.

- **No native review.** Receipt-driven development was on and assessed the
  commit as `medium` / `review_due: true`. The `review-reliability` lens capture
  never succeeded — five provider-tier failures
  (`OpenCode's free tier can only be used from within OpenCode`) and three empty
  captures (`opencode_task_output_empty`) after an OpenCode restart. These are
  model-provider failures, so nothing was filed against `Gentleman-Programming/gentle-ai`;
  that tracker is for Gentle AI's own defects.
- **What was done instead.** A manual reliability pass over the bench against the
  pinned GPUI revision rather than from memory. It found and fixed five items —
  an unchecked window-visibility condition, a caveat asymmetry between these two
  sections (this section now carries the floor caveat the RAM section had), a
  duplicated phase label in the bench's output, and two limits that could only be
  documented. It is one careful pass by the agent that wrote the code, which is
  weaker than an independent reviewer.
- **No CI.** `main`'s branch protection carries no required status checks, so
  GitHub Actions ran nothing on the branch. Every figure here rests on local
  verification: `cargo fmt --check`, `cargo clippy --all-features --all-targets
  -D warnings`, 947 tests, 49 doctests, and seven real executions of the bench.
- **Re-measuring is cheap and is the right response to doubt.** Six fixed dwells
  are announced with elapsed timestamps; `cargo bench --bench frame_time
  --features profiling` reproduces the whole thing, and adding
  `-- --mode app` reproduces the app-level table below.

### Provenance of the app-level figures — read before quoting them

The app-level rows were measured **locally, on the same machine, by the change
that added the mode**, and they carry the same two limitations the list-only rows
do: no independent review, and no CI on the branch (`main` still carries no
required status checks). What was verified is stated rather than implied:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-features
  --all-targets -j 6 -- -D warnings`, 957 tests and 49 doctests, all green.
- **Five real `--mode app` executions and two `--mode list` re-executions**, all
  publishing a report, all inside budget, none tripping the window-visibility
  warning. The reports name the mode and the window's own root-view type on every
  line, so a run labelled "app-level" is one whose window root was
  `sh_nexus::app::Shell`.
- **The two list-only re-executions exist because the capture moved.** Snapshots
  are now armed with `Window::on_next_frame` instead of being taken from inside a
  `render`, which is what lets the app mode's window root be the shell rather than
  a bench wrapper. The measured list-only p99s after that change — 1.252 ms and
  1.526 ms — sit inside the 1.087–1.740 ms range the six earlier runs recorded, so
  the floor still reads as a floor. Arming a capture no longer forces a frame, so
  the "before scrolling" sample count is one frame lower per capture than in the
  runs above; every run prints its own scroll share, and the five app-level runs
  came in at 97.9–98.7%, against 98.7–98.9% for the seven above.
- **The idle-RAM numbers in §"Measured: idle RAM" were sampled externally**, by
  polling the bench process's working set every 500 ms from a separate PowerShell
  process while the two dwells ran — §6.2's own tool column, and the reason the
  bench adds no dependency to read its own memory. The full sample series is
  reproducible; the figures quoted are the median of each dwell window.

### Measured: scroll frame time at 10 000 messages — `< 8 ms`, floor and app level

**List-only: `draw_duration` p99 = 1.087 ms warm, 1.740 ms cold — within budget in
all six runs.** The worst of the six, 1.740 ms, is 21.8% of the 8 ms ceiling; the
warm runs settle at 1.087–1.136 ms.

> **This figure is a floor, and the app-level table below is the one §6.2's row is
> about.** The list-only window hosts `MessageListView` and nothing else: no shell,
> no theme provider, no drain pump, no key handler, no sidebar, no input bar, no
> composer, no SQLite, no network. A number measured there is **optimistic**. What
> it establishes is that **the message list is not the frame-time bottleneck** —
> which is the claim ADR-006 actually rests on, and the claim a future regression
> in the list would first break. What it cannot establish is what the shipped
> window costs, and the RAM section's "Why this does not close the row" is the same
> argument in its own terms.

| Run | `draw_duration` p99, after scroll | scroll frames |
| --- | --- | --- |
| 1 (first execution, cold) | **1.740 ms** | 1 035 of 1 047 (98.9%) |
| 2 | 1.257 ms | 1 062 of 1 075 (98.8%) |
| 3 | 1.136 ms | — |
| 4 | 1.130 ms | — |
| 5 | 1.112 ms | — |
| 6 | **1.087 ms** | 1 030 of 1 042 (98.8%) |
| 7 † | 1.283 ms | 1 032 of 1 046 (98.7%) |
| 8, 9 ‡ | 1.252 ms / 1.526 ms | 1 033 of 1 047 (98.7%) / 1 034 of 1 048 (98.7%) |

† Run 7 was taken after the manual reliability pass on PR #20 (below) added the
window-visibility check. It confirms the fixed binary still measures inside budget
and drew no visibility warning; the measurement path — seeding loop, scroll loop,
snapshots, verdict arithmetic — was not touched by that change, so runs 1–6
remain valid measurements.

‡ Runs 8 and 9 are the list-only re-executions taken with `--mode app` in the tree,
to show the list-only floor still measures where it did after the capture mechanism
moved (see "Provenance of the app-level figures"). Both inside budget, neither drew
a visibility warning.

#### App level: the same window, with the real `app::Shell` as its root

**`draw_duration` p99 = 1.239 ms to 1.614 ms across eight runs — within budget in
all eight.** The worst, 1.614 ms, is **20.2% of the 8 ms ceiling.**

| Run | `draw_duration` p99, after scroll | p50 | scroll frames | `dirty_to_present` p99 |
| --- | --- | --- | --- | --- |
| A1 | 1.441 ms | 0.793 ms | 1 032 of 1 046 (98.7%) | 7.164 ms |
| A2 | 1.500 ms | 0.788 ms | 1 033 of 1 047 (98.7%) | 6.967 ms |
| A3 ‡ | 1.364 ms | 0.744 ms | 1 032 of 1 046 (98.7%) | 7.254 ms |
| A4 §§ | **1.614 ms** | 0.760 ms | 1 032 of 1 046 (98.7%) | 7.180 ms |
| A5 | 1.524 ms | 0.733 ms | 1 037 of 1 059 (**97.9%**) | 7.455 ms |
| B1 ¶ | **1.239 ms** | 0.721 ms | 1 032 of 1 044 (98.9%) | 6.758 ms |
| B2 | 1.332 ms | — | 1 030 of 1 042 (98.8%) | — |
| B3 Ψ | 1.309 ms | — | — | — |

¶ Runs **B1–B3 were produced by the orchestrator, not by the delegated writer**,
after the writer's own runs, from the merged tree — recorded because a figure only
its author can reproduce is not a baseline. B1 is the fastest of the eight and B2
the second, which is a fact about the two machines and the two moments rather
than about the bench: the writer's runs cluster at 1.36–1.61 ms and these at
1.24–1.33 ms, and **the run-to-run spread within either group is smaller than the
gap between the groups.** The honest reading is a range of roughly **1.24–1.61 ms
against a ceiling of 8 ms** — comfortably inside budget on any of them, which is
the only claim the eight runs support.

Ψ B3 is the run whose working set was sampled for the RAM table's `B3` row, so its
frame figures and its RAM figures come from the same execution.

‡ Run A3 is the run whose working set was sampled externally, so its RAM figures
come from the same execution as its frame figures.

§§ Run A4's `dirty_to_present` p99 of 15.983 ms is the *pre-scroll* population, not
the scroll phase — visible in that run's own before/after table, where the
after-scroll p99 is 7.180 ms — and it is a fair illustration of why the verdict is
read from `draw_duration` and why the before-snapshot exists.

**Run A5 is the one to read for the denominator argument.** Its pre-scroll sample
count was 22 rather than the 14 of every other run — the machine was busier, the
window drew more frames while opening and seeding — and its scroll share fell to
97.9% accordingly, with the p99 still inside budget. That is the bracket doing its
job: the dilution is a printed number per run, not a claim carried over from a
table, and the figure it dilutes moved by less than the run-to-run spread.

Each of those windows' root view was `sh_nexus::app::Shell` — the report prints the
window's own root-view type name, read back from the window handle, so the label is
a fact about the window rather than a claim about the intent. The frame measured is
the shell's: root container, theme, list. Its drain pump — a repeating 50 ms task
that ticks 320 times during the scroll phase, applying nothing and asking for no
frame — is running for the whole measurement, because that is what the application
does while a user scrolls.

> **What the app-level figure still does not say, said here rather than left to be
> discovered.** It is the *shell*, not the finished application: `app.rs`'s own
> module docs, §5, record that the channel rail and the input bar are not
> constructible today, so the application window is a themed root container, the
> message list, the drain pump and the key handler. When the rail and the composer
> land, this table has to be taken again and this paragraph has to move with it. The
> gap to §6.2's `< 8 ms` is comfortable enough — the worst of five runs is 20.2% of
> budget — that it is very unlikely to be closed by adding two more containers, but
> "unlikely" is not "measured", and the table above is the number this commit can
> defend.

**Method.** `cargo bench --bench frame_time --features profiling` (list-only) or
the same command with `-- --mode app` runs `crates/sh_nexus/benches/frame_time.rs`
in the release profile. Either way it opens a real GPUI window (1024×768 logical,
the size `app::WINDOW_WIDTH`/`WINDOW_HEIGHT` and the bench's own constants are
asserted equal at compile time) over an `AppState` holding 10 000 messages inserted
through the `bridge::` path, and scrolls continuously for 16 s. GPUI's profiler —
enabled by our `profiling = ["gpui/profiler"]` feature, which adds no new
dependency — accumulates `draw_duration`, and the bench reads it through
`Window::frame_duration_snapshot`.

**The snapshots are armed with `Window::on_next_frame`, not taken from inside a
`render`, and the reason is the app mode.** A render-side probe would have to live
in the shell's render to reach app-level frames, and instrumenting `src/` to measure
`src/` is not a trade this project makes; wrapping the shell in a bench root would
mean the window's root is not the shell, which is the one property the mode exists
to establish. `on_next_frame` needs neither. **The capture still cannot inflate the
figure it takes**: gpui dispatches next-frame callbacks at the start of a frame
request and draws afterwards, while `draw_duration` is written by `end_draw` at the
end of that draw — checked against the pinned revision `e683fd7`, in
`window.rs` and `profiler.rs` respectively, not assumed.

**Who drained the inbox is different in the two modes, and it is the mode.** In
list-only mode the bench drains the inbox itself, because the list-only window is
the bench's own root and owns no schedule. In app mode the **shell's** 50 ms pump
drains it — `Shell::new` arms it — and the bench never calls `bridge::drain`, for
the reason `app.rs` §3 gives: `bridge::drain` is a `try_recv` loop, so two callers
would race for every event. The app mode's seeding path therefore delivers a batch
and waits for the *renderer* to confirm it (`MessageList::sync` runs at the top of
the list's `render`, so the item count reaching a batch means the pump drained, the
pump notified and a frame drew). Verified rather than assumed: all ten batches were
applied by the pump in every app-level run above, and seeding completed in ~0.7 s
against the ~0.7 s the list-only mode takes on its own 16 ms tick.

**The channel the app mode seeds is the shell's, not the bench's.**
`MessageList::show_channel` is a no-op the app mode does not perform: `Shell::new`
already pointed the list at `app::STARTUP_CHANNEL` (`c_startup`), so the fixtures
are delivered there, and the run verifies after opening that the list really is
showing it. Seeding the bench's own `c_bench` into a shell showing `c_startup`
would have measured an empty channel behind a convincing report — which is why the
mode check exists rather than being left to the final item-count assertion.

**Why `draw_duration` and not `dirty_to_present`.** §6.2's row bounds the time
spent *building* the frame — the half this client controls, and the half that
grows if the list stops being virtualized. `dirty_to_present` additionally
contains the wait for the next present (up to 16.7 ms at 60 Hz), making it an
end-to-end figure no amount of rendering optimization can pull under 8 ms. Both
are printed; the verdict is taken from `draw_duration`. The app-level
`dirty_to_present` p99s (6.967–7.254 ms) are printed for completeness and are
exactly the figure the threshold does not govern: at 60 Hz that wait alone is
16.7 ms.

**Why this is a bench and not a test.** The Phase 1 note above is still correct
that `TestAppContext` cannot produce this number — `current_headless_renderer()`
returns `Ok(None)` on Windows. That is exactly why the figure comes from a
release window rather than the headless harness.

**The p99's denominator, stated rather than hidden.** GPUI's histograms are
cumulative for the life of the window and expose no reset, so the after-scroll
snapshot also contains the frames drawn while the window opened and while the
10 000 messages were seeded. The bench prints a before-scroll snapshot as well:
the scroll phase accounts for 97.9–98.9% of the samples in every run recorded in
this file, so pre-scroll frames occupy ~1.1–2.1 percentile points — and they were
the *slower* population (their own p99 was 9.896 ms and 3.391 ms in the two runs
whose full reports were kept, falling to 1.740 ms and 1.257 ms once the scroll
frames were added), so including them can only push the p99 up, never down.

### Measured: idle RAM — §6.2, `< 80 MB`, as a floor and at app level

**List-only: 51.6 MB empty, 64.4 MB with 10 000 messages loaded — a floor, because
this is the bench shell and not the app.** Six runs, OS working set sampled every
500 ms from an external monitor (§5.2's tool column; the bench deliberately adds
no dependency to read its own memory):

| Run | idle, list mounted, 0 messages | idle, 10 000 messages loaded |
| --- | --- | --- |
| 1 (first execution, cold) | 79.7 MB | 92.7 MB |
| 2 | 48.6 MB | 61.3 MB |
| 3 | 51.4 MB | 64.3 MB |
| 4 | 51.4 MB | 64.2 MB |
| 5 | 51.4 MB | 64.2 MB |
| 6 | 51.6 MB | 64.4 MB |

Steady state is 51.4–51.6 MB and 64.2–64.4 MB — runs 3–6 agree to within
0.2 MB. Run 1 is the cold outlier: the first-ever execution of that binary came
in at 79.7 MB, inside the 80 MB ceiling by 0.3 MB. A first launch sees something
close to that rather than the steady state.

**Why this does not close the row.** §6.2's *"Idle RAM usage < 80 MB"* is about
the application at idle, and these runs measure a window hosting `MessageListView`
only: no shell, no theme provider, no drain pump, no key handler, no sidebar, no
input bar, no SQLite, no network stack. What they establish is narrower but real:
**the message list itself is not the memory consumer.** Ten thousand messages cost
12.8 MB (64.4 − 51.6), so §6.2's companion line — *"RAM with 10k cached messages <
200 MB"* — has 6.4% of its budget spoken for by the list alone.

#### App level: the same measurement with the real `app::Shell` in the window

**51.7 MB empty, 64.5 MB with 10 000 messages loaded — the shell's cost over the
list-only floor is 0.2–0.3 MB.** Sampled the same way (working set polled every
500 ms from a separate process during the two dwells), the pid printed in the
banner for the monitor to attach to:

| Run | idle, shell mounted, 0 messages | idle, 10 000 messages loaded | samples per dwell |
| --- | --- | --- | --- |
| A1 (with `--mode app`) | 51.9 MB | 64.7 MB | 10 / 14 |
| B3 ‡ | **51.7 MB** | **64.5 MB** | 12 / 17 |

‡ Run B3 is the orchestrator's own, sampled against the same merged binary the
frame figures in the table above come from: **61 samples**, the full series being
9.4 MB at t=0 (the pre-window D3D ramp), 51.7 MB held flat from t≈1.1 s through the
empty dwell to t≈6.2 s, a step to 65.4 MB at t≈6.7 s as the seed lands, and
64.5–65.0 MB flat for the loaded dwell. The two runs agree to 0.2 MB, which is the
same agreement the six list-only runs showed among themselves.

The first sample of a run is taken while the process is still bringing up the D3D
device and reads 9.3–9.4 MB; it is the pre-window ramp, not an idle level, and the
level is reached by the second sample at ~1.6 s. Both figures above exclude it and
the figures quoted are the **median** of each dwell window; the full series for both
modes is a `Get-Process`-poll away from anyone who wants to re-derive them.

**So the shell — root container, theme, drain pump, key handler, focus tracking —
costs about a quarter of a megabyte, and 10 000 messages cost 12.8 MB either way.**
That is the finding worth keeping, and it is narrower than the row: it says the
things that exist are not the memory consumer. It does not close §6.2's row, for
two reasons that are both about time and existence rather than about size.

#### The 30-minute time base — measured, and it moves about 4 MB under load

`AGENTS.md` §6.2 asks for idle RAM *"over 30 minutes of active chatting"*. Every
figure above is a **level**: the two dwells are 6 s and 8 s, which a leak of any
plausible size survives. This section is the **trend** that closes that gap.

Run with `--mode soak --soak-minutes 30`, the release binary under the
`profiling` feature, on `x86_64-pc-windows-msvc`. The soak opens one real
`app::Shell` window, seeds the channel to exactly `MAX_MESSAGES_PER_CHANNEL`
(10 000), then for 30 minutes drives one **optimistic send + one ACK** per cycle
through the public seam — `MessageList::begin_send` (the same door `InputBar::send`
uses) then `DomainEvent::MessageAcked` — each iteration waiting twice for the
**renderer** to build and update a row. An external monitor sampled this process's
working set every 500 ms (**3 560 samples**, pid `19972`) and sliced the series on
the bench's own phase/epoch markers:

| Window | n | Working set (avg) | Working set (min–max) | Private (avg) |
| --- | --- | --- | --- | --- |
| baseline, at rest, 10 000 loaded (t ≤ 15 s) | 30 | **63.1 MB** | 62.9 – 63.6 | 71.2 MB |
| under load, 30 min of send/ACK (16–1815 s) | 3 508 | **65.7 MB** | 63.6 – 70.8 | 73.4 MB |
| after, activity stopped (t ≥ 1816 s) | 18 | **68.5 MB** | 68.5 | 76.1 MB |

**The 5-minute shape, because an average hides the question the row asks** (delta
against the first 5-minute bucket's average):

| min | 0 | 5 | 10 | 15 | 20 | 25 | 30 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| WS avg | 64.3 | 65.0 | 65.1 | 66.1 | 66.1 | 67.4 | 68.7 |
| Δ | — | +0.65 | +0.78 | +1.74 | +1.81 | +3.05 | **+4.40** |

**Reading it honestly: ~4.4 MB of working-set growth over 30 minutes, and the curve
is a slow staircase, not a flat line and not a clean linear leak.** It steps up and
partly settles (dips at 10 and 20 min, larger steps at 15 and 25). That shape is
consistent with the bounded caches filling once plus allocator behaviour, and it is
consistent with the run's own structural table: the row cache and the rendered-segment
LRU each climbed 168 → 512 entries and then **stopped** (see the "settled" columns
below), which is the one-off fill the slope column is warning about. The last bucket
is still rising, and that is the part not to paper over: a further hour could reveal
either a plateau (the fill completing) or a continued climb, and **30 minutes cannot
tell those apart**. The number to re-measure, not to defend.

**Structures, through the client's own public seam — baseline → under load → after:**

| What is counted | baseline | under load | after | verdict |
| --- | --- | --- | --- | --- |
| messages held in the channel | 10 000 | 10 000 | 10 000 | flat at the cap |
| rows the list is showing | 10 000 | 10 000 | 10 000 | flat |
| rows the row cache retains | 168 | 512 | 512 | **settled** (LRU full) |
| rendered-segment entries | 168 | 512 | 512 | **settled** |
| rendered-segment declared bytes | 7 378 | 28 600 | 28 574 | **settled** |
| unread elements | 10 000 | 10 000 | **0** | drained, as designed |
| sends awaiting an answer | 0 | 0 | 0 | flat — nothing leaked as pending |

**The 10 000 cap bit, on the first iteration, exactly as predicted.** The channel was
seeded to precisely `MAX_MESSAGES_PER_CHANNEL`, so the first send is one over the
bound and the oldest seeded message (`…0001`) was no longer held after iteration 1.
The highest held count observed was **10 000** — that is cap+1, the first eviction; a
cap+2 reading would have meant an insert that evicted nothing. Eviction **skips a row
this client has an outstanding send for** and skips to the next oldest; because this
cycle ACKs each send before starting the next, at most one row was ever outstanding
and it was the newest, so the documented all-rows-in-flight over-cap case did not
arise. That is a property of this workload and **not** evidence the case is harmless:
a client that queued many sends before any ACK would meet it.

**The one structure that only went up, and had no bound of its own:** the sampled
tracked-sends map grew to 64 entries (one sample of the delivery map via
`SOAK_PROBE_EVERY`, not a whole-map count). Its own report is explicit — an
acknowledged send stayed in the map for the life of the process and **eviction did
not retire it**. That was a real, named unbounded growth on the client side, out of
§6.2's row (this run's `sends awaiting an answer` is flat at 0), and it was the
honest finding of the trend rather than something to bury here.

**That growth is now closed, and the bound is structural rather than a new number.**
`AppState::evict_one_over_cap` retires the delivery entry of an `Acked` send along
with the row it evicts, which is the only place in the client that does.
Previously the sole retirement path was `AppState::clear_outgoing`, reachable only
from `actions::discard_failed_send`, which applies only to a **failed** send — so
an acknowledged send's entry was inserted once and never removed, and the map grew
by one entry per send for the life of the process. **No cap was added and no
threshold chosen**: an entry now either names a row that is held or a send the
client is genuinely still waiting on, so the map cannot outnumber held rows plus
in-flight sends. The bound moves with the client's own work instead of with the
length of the session.

**`Pending` and `Failed` sends deliberately keep their entry, and the reason is the
user rather than the map.** Eviction skips such a row anyway — that is what
`has_outstanding_send` is for, and `a_send_in_flight_is_never_the_row_that_is_evicted`
in `crates/sh_nexus/tests/state_actions.rs` is the assertion — but the retirement is
guarded on the state being `Acked` anyway, so the two facts are not silently
coupled. `AGENTS.md` §7.5's *"failures are never silently dropped"* means the
failed row stays on screen for retry, and `discard_failed_send` remains its
retirement path.

**Where the retirement could *not* go is the load-bearing part of the fix.**
`AppState::remove_message` has four callers and only one of them discards the row:
`merge_into_held` and the channel-mismatch arm of `ingest_by_identity` both remove
a row in order to **re-insert the same `client_msg_id` in another channel**, then
call `retarget_outgoing`. A retirement inside `remove_message` — the obvious place —
would therefore delete the entry of a send still on screen in its new channel, which
is a worse defect than the leak it fixed.
`a_row_the_server_moves_between_channels_keeps_its_delivery_entry` pins that, and it
lives in `tests/state_actions.rs` beside the cap tests. A defect-pinning
characterisation test for the leak, `soak_cycle.rs`'s
`an_acknowledged_send_outlives_the_row_it_was_evicted_with`, was **deleted** rather
than inverted, on its own written instruction.

**Every figure in this section predates the fix, and none of them may be read as
after it.** The 64-entry probe sample, the +4.4 MB trend, the 63.1 → 68.5 MB
working-set windows and the 3 560 samples are provenance: they measured the client
*with* the leak, which is the only reason the leak was known rather than argued
about. **Whether the fix removes part of that +4.4 MB is not established, and this
section does not claim it.** That question is owed a re-measurement — another
`--mode soak --soak-minutes 30` on the fixed build — and until that run exists the
right statement about the slope is the one already made above it: 30 minutes
cannot distinguish a plateau from a climb, and the number is to re-measure, not to
defend.

**What this still does not measure, quoted from the bench because it must not be
quoted past:** *"THERE IS NO SERVER. Every ACK in this run was injected by this
bench… What it cannot measure is a server-driven leak, a reconnection storm, a
resync, or anything network/ will do in Phase 4 — there is no network/ in this
build."* The 30 minutes were real cycles through real client code, so this **does**
answer the row's time base for the client's own structures. It does **not** measure a
real session, and the cadence (≈967 cycles/min, the shell's 50 ms drain pump) is a
stress bound orders of magnitude above a person's typing rate — the per-cycle costs
are the transferable figures, not the 30-minute total.

**What still keeps the row open, and what no longer does:**

- ~~**The row's own time base.**~~ **Closed by this section.** 30 minutes of
  sustained send/ACK cycles against the release binary, with an external monitor on
  the real pid: **+4.4 MB** working set over 30 min, bounded caches settled, pending
  sends flat, the 10 000 cap evicting from iteration 1. **Those words do not cover
  the delivery map**, which this section reports growing without bound and which the
  probe measured still leaking — so "pending sends flat" above is not evidence the
  map was flat.
- **The delivery map's post-fix slope.** The bound is structural now and is asserted
  in `crates/sh_nexus/tests/state_actions.rs`, so the leak is closed as a matter of
  design rather than of measurement. What is **not** established is that the +4.4 MB
  trend above included it or shrinks without it: every figure in this section was
  taken on the leaking build. Closing this axis means re-running
  `--mode soak --soak-minutes 30` and re-reading the probe sample, not re-reading
  this table.
- **The parts of the window that do not exist yet.** `app.rs`'s module docs, §5,
  record that the channel rail and the input bar are not constructible today, so
  the "application at idle" this file has measured is the shell as it stands at
  this commit. When those land, this table **and this 30-minute trend** are
  re-measured, exactly as the app-level frame-time table must be re-measured with it.
- **Whether 4.4 MB is a plateau or a climb.** A longer run, or a second 30-minute
  run to check reproducibility, is the way to close this — not an argument that the
  number is small enough. Until then §6.2's idle-RAM row stays open on the *window
  contents* and *longer-run* axes, and closed on the *time base* axis.

**The p99's denominator is the one stated in §"Measured: scroll frame time"**, and
it is not repeated here: GPUI's histograms are cumulative, the bench brackets the
scroll with a before/after snapshot, and the scroll phase accounts for 97.9–98.9%
of the samples in every run recorded in this file.

**The verdict's histogram carries no visibility filter.** Checked against the pinned
GPUI revision `e683fd7`, not assumed: `end_draw` records `draw_duration` unless the
system's power state changed across the frame, and no visibility condition is
consulted at all, whereas `dirty_to_present` only survives
`journal::frame_sample_is_valid`, which drops frames whose window was hidden. That
asymmetry had a real consequence — a run whose window lost focus mid-scroll
published its frames as if measured in the foreground, and nothing in the report
revealed it. The bench now records `Window::is_visible()` at each capture and prints
a **WARNING** marking the figure unquotable when the window was not visible. None
of the twenty-two frame-time runs recorded in this file tripped it.

### Measured headlessly, and what it does and does not prove

Recorded because it is the denominator the frame budget needs, and because it is
the first measurement of §7.3's virtualization claim rather than a restatement of
it. Conditions: `cargo test -p sh_nexus --test ui_message_list`, the harness
window at 1920×1080 (maximized), 40 messages in one channel, the built-in dark
theme, debug profile.

| Metric | Measured | Threshold | Verdict |
|---|---|---|---|
| Row entities built for 40 messages | **24** | — | **virtualization works: 24 rows, not 40** |
| Row height (one line, `text_sm`) | **68.5 px** | — | — |
| List viewport in the harness | 1920×1080 px | — | — |
| Rows built for 10 000 messages | not measured | — | the figure above is 40 messages, not 10 000 |

**What this proves:** `gpui::List` renders the visible range plus the 512 px
overdraw and nothing else, so §7.3's *"render only visible messages"* is a
measured property of this list rather than an assumption about the framework.
**What it does not prove:** anything about frame time. 24 rows at 68.5 px puts
roughly 685 000 px of content in the list, and that is the input a frame-time
measurement needs — not the measurement itself. The distinction is the same one
the Phase 1 section draws about the stub: a number that looks like a budget
figure and cannot test the budget is worse than no number. Work unit 2B has
since taken the frame-time measurement itself, outside this harness, in §"Measured:
scroll frame time at 10 000 messages".
