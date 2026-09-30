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

## Idle RAM — not recorded, and why that is the honest answer

`AGENTS.md` §6.2 asks for **idle RAM < 80 MB**, measured *"over 30 minutes of
active chatting"*, and `PLAN.md` L609 lists it as a Phase 1 exit criterion.

**It is not measurable at the end of Phase 1, and a number produced now would be
worse than no number.** `src/main.rs` is a 20-line entry point that calls
`sh_nexus::run()`. There is no window, no sidebar, no message list, no input bar
— those are Phase 2. Measuring the resident set of a stub would record something
like 15 MB and put a green-looking number against a §6.2 threshold it has no
standing to test.

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
| Idle RAM | **not recorded** | this file, with reason |

The coverage and test figures are re-derived from `cargo llvm-cov
--workspace --summary-only` rather than transcribed, per the two reconciliation
rules in `docs/COVERAGE.md` §5.5 — a percentage that disagrees with its own
fraction survived four correct reconciliations in work unit 1D, so every number
above is either measured here or points at the file where it is measured.

## Message list: what is measured, and the two figures still owed

`docs/ARCHITECTURE.md` ADR-006's step 6 makes two `AGENTS.md` §6.2 figures owed
by the message list: the **<8 ms scroll frame time at 10 000 messages**, and the
**idle RAM** this file has been carrying as deliberately unmeasured. Work unit 2A
landed steps 1–5 (the `ui/` layer, `gpui::List`, row recycling, Markdown to GPUI
elements, the state seam), so the list now exists and the two figures are
**measurable for the first time**. Neither is recorded here yet, and the reason
is narrow rather than the old one.

### Owed: scroll frame time at 10 000 messages — §6.2, `< 8 ms`

**Not measured.** A frame-time histogram is a property of a real window with a
real frame loop, and the headless harness has no renderer at all on Windows:
`current_headless_renderer()` returns `Ok(None)` there, because `TestAppContext`
uses GPUI's pure-Rust `TestPlatform` and never touches a GPU (Phase 0 finding 3).
So neither `TestAppContext` nor CI on this platform can produce this number, and
a figure produced some other way would not be the figure §6.2 bounds.

**Method, when it is taken:** `cargo build --release` (which needs `fxc.exe` on
`PATH` — Phase 0 finding 1, and the build fails with an error that reads like a
code defect), run the release binary, open a channel with 10 000 messages, and
record the frame time histogram while scrolling continuously. The number to
compare against §6.2 is the **99th percentile**, not the mean: §1's priority is
*zero jank while receiving messages*, and a mean hides exactly the frames that
are the complaint.

### Owed: idle RAM — §6.2, `< 80 MB`, and it is no longer impossible

**Still not measured, and the old reason is now spent.** The Phase 1 section
above says the figure was not measurable because *"there is no window, no
sidebar, no message list, no input bar"* and measuring a stub would put a
green-looking number against a threshold it has no standing to test. There is
now a message list, so what is missing is not the UI but a **release binary that
opens it**: `src/main.rs` still opens the Phase 0 spike's `RootView`, which is
the same stub in a different file. Measuring that process would be the failure
mode this file already names, so the figure stays owed until `app.rs` (which
`PLAN.md` §4 places next) opens the real shell.

**Method, unchanged from §5.2:** release build, attach the OS process monitor,
let it settle, sample the working set at rest, then exercise it for 30 minutes of
active chatting and record **both** the idle figure and the growth. §6.2's
companion line — *"RAM with 10k cached messages < 200 MB"* — has the same
dependency and is measured in the same session.

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
figure and cannot test the budget is worse than no number.
