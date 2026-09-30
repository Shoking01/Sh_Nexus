# Measured baselines — `AGENTS.md` §11

`PLAN.md` L609 makes this a **Phase 1 exit criterion**: *"Measured baselines
recorded for build time, binary size, idle RAM."*

`AGENTS.md` §6.3 then says what a baseline is *for*: *"Any increase > 5MB in
binary size requires justification"*, and *"Any regression > 10% in performance
metrics blocks the merge."* Those rules are unenforceable without a recorded
starting point, which is what this file is.

**Two of the three are recorded here with the exact conditions of measurement.
The third is not, and says so with a reason rather than being quietly omitted.**

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

> **Superseded in part by work unit 2B.** A bench shell now yields a measured
> floor (51.6 MB empty, 64.4 MB with 10 000 messages); the *app-level* figure
> below remains owed. See §"Measured as a floor: idle RAM" further down.

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
| Tests | **890** unit/integration + **49** doctests = **939**, 0 failures | — |
| `core/` coverage | **97.76%** regions (floor: 90%) | `docs/COVERAGE.md` |
| `state/` coverage | **98.16%** regions | `docs/COVERAGE.md` |
| `state/bridge.rs` | **100%** (185/185 regions, 29/29 functions) | `docs/COVERAGE.md` |
| Workspace coverage | **96.42%** regions | `docs/COVERAGE.md` |
| Release binary | **9.95 MB** | this file |
| Release build | **288 s** at `-j 6` | this file |
| Dev build (warm) | 14 s | this file |
| Source | 42 `.rs` files, 31,341 lines | — |
| Direct dependencies | 12 rows | `docs/DEPENDENCIES.md` |
| Idle RAM | **51.6 MB** empty / **64.4 MB** with 10k, bench shell only | this file, §"Measured as a floor: idle RAM" — added by work unit 2B |
| Scroll frame time, 10k | **1.087 ms** warm / **1.740 ms** cold, p99, bench shell only | this file, §"Measured: scroll frame time" — added by work unit 2B |

The coverage and test figures are re-derived from `cargo llvm-cov
--workspace --summary-only` rather than transcribed, per the two reconciliation
rules in `docs/COVERAGE.md` §5.5 — a percentage that disagrees with its own
fraction survived four correct reconciliations in work unit 1D, so every number
above is either measured here or points at the file where it is measured.

## Message list: one figure measured, one measured only as a floor

`docs/ARCHITECTURE.md` ADR-006's step 6 makes two `AGENTS.md` §6.2 figures owed
by the message list: the **<8 ms scroll frame time at 10 000 messages**, and the
**idle RAM** this file has been carrying as deliberately unmeasured. Work unit 2B
built `crates/sh_nexus/benches/frame_time.rs` — a release-window bench that seeds
10 000 messages through the real `AppState` and `bridge::` path and scrolls the
real `MessageListView` — and ran it six times. The frame-time figure is now
**measured and recorded**. The RAM figure is measured, but only as a **floor**,
and the app-level number stays owed: read the caveat below rather than the
number alone.

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
  --features profiling` reproduces the whole thing.

### Measured: scroll frame time at 10 000 messages — §6.2, `< 8 ms`, and a floor rather than the app figure

**`draw_duration` p99 = 1.087 ms warm, 1.740 ms cold — within budget in all six
runs.** The worst of the six, 1.740 ms, is 21.8% of the 8 ms ceiling; the warm
runs settle at 1.087–1.136 ms.

> **This figure is a floor too, exactly like the RAM figure below.** The bench
> opens a window hosting `MessageListView` and nothing else: no sidebar, no input
> bar, no composer, no SQLite, no network. §6.2's row says *scroll frame time*, and
> in the application that frame also lays out and paints the rest of the shell.
> A number measured here is therefore **optimistic**. What it establishes is that
> **the message list is not the frame-time bottleneck** — which is the claim ADR-006
> actually rests on. What it cannot establish is that the shipped application
> meets 8 ms; that stays owed until `app.rs` opens the real shell. Read the RAM
> section's "Why this does not close the row" for the same argument in its own
> terms; this number is the one a reader is more tempted to quote, which is why
> the caveat is here rather than only there.

| Run | `draw_duration` p99, after scroll | scroll frames |
| --- | --- | --- |
| 1 (first execution, cold) | **1.740 ms** | 1 035 of 1 047 (98.9%) |
| 2 | 1.257 ms | 1 062 of 1 075 (98.8%) |
| 3 | 1.136 ms | — |
| 4 | 1.130 ms | — |
| 5 | 1.112 ms | — |
| 6 | **1.087 ms** | 1 030 of 1 042 (98.8%) |
| 7 † | 1.283 ms | 1 032 of 1 046 (98.7%) |

† Run 7 was taken after the manual reliability pass on PR #20 (below) added the
window-visibility check. It confirms the fixed binary still measures inside budget
and drew no visibility warning; the measurement path — seeding loop, scroll loop,
snapshots, verdict arithmetic — was not touched by that change, so runs 1–6
remain valid measurements.

**Method.** `cargo bench --bench frame_time --features profiling` runs
`crates/sh_nexus/benches/frame_time.rs` in the release profile. It opens a real
GPUI window (1024×768 logical), mounts the real `MessageListView` over an
`AppState` holding 10 000 messages inserted through the `bridge::` path, and
scrolls continuously for 16 s. GPUI's profiler — enabled by our `profiling =
["gpui/profiler"]` feature, which adds no new dependency — accumulates
`draw_duration`, and the bench samples it through `Window::frame_duration_snapshot`
**inside `render()`**, so the sampling frame cannot inflate the figure it is
sampling.

**Why `draw_duration` and not `dirty_to_present`.** §6.2's row bounds the time
spent *building* the frame — the half this client controls, and the half that
grows if the list stops being virtualized. `dirty_to_present` additionally
contains the wait for the next present (up to 16.7 ms at 60 Hz), making it an
end-to-end figure no amount of rendering optimization can pull under 8 ms. Both
are printed; the verdict is taken from `draw_duration`.

**Why this is a bench and not a test.** The Phase 1 note above is still correct
that `TestAppContext` cannot produce this number — `current_headless_renderer()`
returns `Ok(None)` on Windows. That is exactly why the figure comes from a
release window rather than the headless harness.

**The p99's denominator, stated rather than hidden.** GPUI's histograms are
cumulative for the life of the window and expose no reset, so the after-scroll
snapshot also contains the frames drawn while the window opened and while the
10 000 messages were seeded. The bench prints a before-scroll snapshot as well:
the scroll phase accounts for 98.8–98.9% of the samples, so pre-scroll frames
occupy ~1.1–1.2 percentile points — and they were the *slower* population, so
including them can only push the p99 up, never down.

### Measured as a floor: idle RAM — §6.2, `< 80 MB`, still owed at app level

**51.6 MB empty, 64.4 MB with 10 000 messages loaded — but this is the bench
shell, not the app, so it is a floor and does not close the row.** Six runs, OS
working set sampled every 500 ms from an external monitor (§5.2's tool column;
the bench deliberately adds no dependency to read its own memory):

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
the application at idle. The bench opens a window hosting `MessageListView` only:
no sidebar, no input bar, no SQLite, no network stack — and, as the Phase 1
section above records, `src/main.rs` still opens the Phase 0 `RootView` stub, so
there is no real shell to measure yet. What these numbers establish is narrower
but real: **the message list itself is not the memory consumer.** Ten thousand
messages cost 12.8 MB (64.4 − 51.6), so §6.2's companion line — *"RAM with 10k
cached messages < 200 MB"* — has 6.4% of its budget spoken for by the list alone.

**What stays owed, and its method (unchanged from §5.2):** the app-level idle
figure, measured once `app.rs` (which `PLAN.md` §4 places next) opens the real
shell — release build, OS process monitor attached, let it settle, sample the
working set at rest, then 30 minutes of active chatting recording **both** the
idle figure and the growth.

**The p99's denominator, stated rather than hidden.** GPUI's histograms are
cumulative for the life of the window and expose no reset, so the after-scroll
snapshot also contains the frames drawn while the window opened and while the
10 000 messages were seeded. The bench prints a before-scroll snapshot as well:
the scroll phase accounts for 98.7–98.9% of the samples, so pre-scroll frames
occupy ~1.1–1.3 percentile points — and they were the *slower* population, so
including them can only push the p99 up, never down.

**The verdict's histogram has no upstream validity filter.** Checked against the
pinned GPUI revision `e683fd7`, not assumed: `record_draw_timing` writes
`draw_duration` unconditionally, whereas `dirty_to_present` only survives
`journal::frame_sample_is_valid`, which drops frames whose window was hidden.
That asymmetry had a real consequence — a run whose window lost focus mid-scroll
published its frames as if measured in the foreground, and nothing in the report
revealed it. The bench now records `Window::is_visible()` at each capture and
prints a **WARNING** marking the figure unquotable when the window was not
visible. None of the seven runs above tripped it.

**What stays owed, and its method.** The app-level scroll frame time, measured
once `app.rs` opens the real shell: release build, window in the foreground for
the whole run, the same 10 000 messages, the same p99 statistic.

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
