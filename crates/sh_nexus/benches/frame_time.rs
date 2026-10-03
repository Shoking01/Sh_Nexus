//! The frame-time and RAM bench: `AGENTS.md` section 6.2's two message-list
//! figures, measured in a real window running the real frame loop.
//!
//! This is `docs/ARCHITECTURE.md` ADR-006 step 6. It measures:
//!
//! 1. **Scroll frame time at 10 000 messages**, compared against section 6.2's
//!    `< 8 ms`, at the **99th percentile** -- `docs/BASELINES.md` fixes that
//!    statistic explicitly, because section 1's priority is *zero jank* and a
//!    mean hides exactly the frames that are the complaint.
//! 2. **Idle RAM** and **RAM with 10k cached messages**, which are *not*
//!    measured here: the bench dwells in two fixed idle phases so that an
//!    external OS process monitor -- section 6.2's own tool column -- can poll
//!    this process at rest. Sampling RAM from inside the process would add a
//!    dependency to answer a question section 6.2 already assigns to the OS.
//!
//! # The two modes, and which one section 6.2's row is about
//!
//! `--mode list` (the default) puts the production [`MessageList`] in a root this
//! bench owns and nothing else. `--mode app` puts the real [`Shell`] in the
//! window -- constructed by the shipped `Shell::new`, focused the way
//! [`app::open`] focuses it -- and drives its list through the shell's own public
//! API. The report names the mode on its own line and prints the window's own
//! root-view type name beside it, read back from the window handle, so a number
//! cannot be quoted as the wrong one.
//!
//! **Both are kept, and neither replaces the other.** `docs/BASELINES.md` records
//! the list-only figures as floors -- what they establish is that *the message
//! list is not the frame-time bottleneck* -- and that question still needs
//! answering, because it is the question that says where a regression in the list
//! would show up first. Section 6.2's row, though, is written about the window:
//! *"Scroll frame time (10k messages, virtualized) | < 8 ms"*, and in the
//! application that frame also lays out and paints the shell, the theme and the
//! pump. So app mode is the mode the row is *about*, and list mode is the mode
//! that *attributes*.
//!
//! # The soak mode, and the time base the other two do not have
//!
//! `--mode soak` is the third mode, and it exists because of one clause in
//! `AGENTS.md` §6.2: *"Idle RAM usage < 80 MB"*, measured *"over 30 minutes of
//! active chatting"*. The two modes above dwell 6 s and 8 s, which measures a
//! **level**; sixty repetitions of a level is still a level, and stretching
//! them over half an hour would be the exact failure this file exists to
//! prevent — thirty minutes of elapsed time wrapped around a measurement with
//! nothing in it to grow.
//!
//! **So the soak does real work per iteration: one optimistic send through the
//! production path, then the ACK that resolves it.** The send is
//! [`MessageList::begin_send`] reached through [`Shell::list`], the same door
//! `InputBar::send` uses, so the row, the `outgoing` entry and the
//! reconciliation all go through code the shipped client runs. The ACK is a
//! [`DomainEvent::MessageAcked`] delivered through the sender the shell
//! retains. **There is no server**, which is the honest limit and is printed in
//! the report rather than left for a reader to discover.
//!
//! ## Who drains, and what the bench waits on
//!
//! **The shell's own 50 ms pump, exactly as in app mode — this bench does not
//! call `bridge::drain` while a shell is in the window.** The cycle delivers
//! one ACK and then waits for *the renderer*: a row is only built, and only
//! updated, by `render_row` inside a frame, so the retained row's spec
//! reaching `Pending` and then `Acked` is a signal that can only be true once
//! the pump drained, the pump notified, and a frame drew. Waiting on the state
//! instead would prove strictly less and would still look like a passing run.
//!
//! **The `client_msg_id` is a fresh `Uuid::new_v4()` every iteration, and that
//! is load-bearing rather than tidy.** A reused identity hits
//! `AlreadyPending`/`AlreadyHeld`, `begin_send` returns `false`, and the cycle
//! quietly stops doing work — in a run that looks calm. The bench treats that
//! `false` as a hard failure for exactly that reason.
//!
//! ## The three figures, and why three
//!
//! A level alone cannot tell a leak from a cache that has filled.
//!
//! | Figure | What it answers |
//! |---|---|
//! | **Baseline** — idle, settled, before the first send | the level everything is measured against |
//! | **Under load** — the peak across the run | whether sustained use grows the working set |
//! | **After** — idle again, same dwell, once the sends stop | **whether it comes back down.** A cache returns; a leak does not. This is the figure that makes the other two mean something. |
//!
//! The slope is reported per iteration and per minute as well as the levels,
//! because a delta that extrapolates says more than a level that cannot.
//!
//! ## What a soak publishes, and what it deliberately does not
//!
//! **No `< 8 ms` verdict is printed by a soak run, and none should be read into
//! one.** §6.2's frame row is a *scroll* measurement — 16 s of continuous
//! scrolling, the p99 of `draw_duration`, bracketed by two snapshots because
//! gpui's histograms are cumulative. A soak scrolls nothing, so its frame
//! numbers answer a question nobody asked; the two modes above are where that
//! figure comes from and `docs/BASELINES.md` records it. The soak does print
//! how many frames the window drew, because *that* is the evidence its own
//! waits were satisfied by a live renderer rather than by a stalled one.
//!
//! **What app mode still does not close, said here rather than discovered later.**
//! `app.rs`'s own module docs, section 5, record that the channel rail and the
//! input bar are not constructible today, so "the real application shell" is the
//! shell as it exists at this commit: a themed root container, the list, the drain
//! pump and the key handler. When the rail and the composer land, the app-level
//! number has to be taken again and this section has to move with it. And
//! section 6.2's RAM row asks for idle RAM *"over 30 minutes of active
//! chatting"*, which the two dwells of 6 s and 8 s cannot produce in either
//! mode: the figures those modes print are levels, not trends. **The soak mode
//! is the answer to that half, and only to that half** — see the section
//! above, and see the honest limit it prints.
//!
//! # Why this is a bench target and not `src/main.rs`
//!
//! `src/main.rs` opens the shell and quits, and `src/lib.rs::run` is its four
//! lines. Neither is a place to spend thirty-five seconds scrolling, so the
//! instrument is a bench target behind `required-features = ["profiling"]` and it
//! reaches no product artifact -- sections 6.1 and 6.3 both track the *release
//! binary*, and `main.rs` is that binary.
//!
//! # How to run it
//!
//! ```text
//! cargo bench --bench frame_time --features profiling                  # list mode
//! cargo bench --bench frame_time --features profiling -- --mode app    # app mode
//! cargo bench --bench frame_time --features profiling -- --mode soak   # the RAM time base
//! cargo bench --bench frame_time --features profiling -- --help
//! ```
//!
//! The two frame modes open one window, run for roughly thirty-five seconds,
//! print a report, and exit 0. The soak opens one window, runs for
//! [`SOAK_DEFAULT_MINUTES`] minutes of send/ACK cycles between two idle dwells,
//! prints its own report, and exits 0. Every phase change is printed with an
//! elapsed timestamp and the process id, so an external monitor can be pointed
//! at the right pid during the idle dwells.
//!
//! **A note on the command line, because it is not the obvious one.** `cargo bench`
//! appends its own `--bench` to a `harness = false` target's arguments -- the
//! process is invoked as `frame_time-<hash>.exe --mode app --bench` -- so this file
//! reads them itself and drops that one flag. See [`parse_args`].
//!
//! # Why the app mode does not call `app::open`
//!
//! **It cannot, and the reason is the ordering `bridge::install` requires.** This
//! bench installs the state itself, in `main`, before any window exists, and a
//! second install is *refused* with `InstallError::AlreadyInstalled` rather than
//! allowed to replace the global -- a replacement would drop the first inbox and
//! its queued events (`bridge.rs`, `install`). `app::open` installs and then opens
//! its own window, so calling it would either be the second install or a second
//! window. The app mode therefore reproduces `app::open`'s *build callback* --
//! construct `Shell::new(sender, cx)`, focus the root on open -- inside a window
//! this bench opened, and installs nothing. Every line inside that callback is a
//! line `app::open` has, which is the whole reason the mode is worth having: the
//! root view is the shipped root view, built by the shipped constructor, and the
//! only thing between it and the real thing is who opened the window.
//!
//! The one line it does not reproduce is `app::open`'s own `window_options`, which
//! is private on purpose (`bridge.rs` section 5: the number of ways to do
//! something is the audit trail). This bench uses its own, and
//! `window_geometry_matches_the_shell` below turns "they happen to agree today"
//! into a build failure rather than a promise -- an app-level figure measured in a
//! differently-sized window would not be comparable with the floor
//! `docs/BASELINES.md` already recorded.
//!
//! # Who owns the drain
//!
//! **In app mode: the shell's pump, and this bench never calls
//! `bridge::drain`.** `Shell::new` arms a repeating 50 ms task that drains the
//! inbox itself, and `app.rs` section 3 says in as many words why a second drain
//! is not available: `bridge::drain` is a `try_recv` loop, so two callers would
//! race for every event and the resulting schedule would be one nobody can
//! reason about. A bench that drained as well would be measuring a client with
//! two schedulers, which is not a client.
//!
//! **So the app mode's seeding path delivers and then waits, and the wait is for
//! the renderer rather than for the state.** `MessageList::sync` runs at the top
//! of the list's own `render`, so a batch is confirmed applied when the list's
//! item count reaches it -- which is only true once the pump drained, the pump
//! notified, and a frame drew. That costs one pump interval (50 ms) per batch
//! instead of this bench's 16 ms tick, and the cadence it buys is the cadence the
//! application has, which is the point of the mode. It also means the bench never
//! sees the pump's `DrainReport`, so a refused event is not counted here: it
//! shows up as a batch that never reaches its expected count, and the run fails
//! printing that count.
//!
//! **In list mode the bench drains itself**, because the list-only window is this
//! bench's own root and owns no schedule at all -- there is no shell and no pump
//! to defer to. The bound is identical in both modes: a batch is never larger
//! than [`bridge::MAX_PENDING_EVENTS`], because a full inbox *refuses* delivery
//! (`AGENTS.md` section 7.1 forbids unbounded growth) and a refusal is a reported
//! value, not a queue that grows.
//!
//! # The channel each mode seeds, and why it is not a constant
//!
//! In list mode the bench chooses the channel and says so with
//! `MessageList::show_channel`, before any message arrives, so every batch is
//! reconciled by the production path rather than by a `reset` that hands the list
//! a finished count.
//!
//! **In app mode the bench must seed `app::STARTUP_CHANNEL`, because that is the
//! channel the shell chose.** `Shell::new` calls `show_channel(STARTUP_CHANNEL)`
//! itself, and it chose that because nothing populates a channel list yet
//! (`app.rs` section 5). Seeding `c_bench` into a shell showing `c_startup` would
//! put 10 000 messages in a channel nothing renders: the list's count would stay
//! at zero and the run would fail on its own consistency check rather than
//! measure an empty channel. So the mode's channel is read from the shell's own
//! constant, the app mode verifies after opening that the list really is showing
//! it, and the banner prints which one was used.
//!
//! # Where the snapshot is taken, and why it is not inside a view's render
//!
//! Every figure is read from `Window::frame_duration_snapshot`, and the read is
//! armed with `Window::on_next_frame` rather than from a `Render` impl. Two
//! reasons, and the second is why this is not a preference:
//!
//! 1. **The app mode's window root is the shell, and a probe inside a render would
//!    have to live in the shell's render.** Adding a measurement hook to
//!    `src/app.rs` puts instrumentation in the product in order to measure the
//!    product, and the alternative -- wrapping the shell in a bench root -- means
//!    the window's root is not the shell, which is the one thing the mode exists
//!    to establish. `on_next_frame` needs neither: it is a window-level facility,
//!    and the histogram it reads belongs to the window rather than to a view.
//! 2. **The capture still cannot inflate the figure it takes.** gpui dispatches
//!    next-frame callbacks at the start of a frame request and draws afterwards
//!    (`window.rs`: the callbacks are taken immediately before the
//!    `measure("frame duration", ...)` block), while `draw_duration` is written by
//!    `end_draw` at the end of that draw. A snapshot taken in the callback
//!    therefore holds every frame up to and including the last one drawn, and
//!    never the cost of the frame the callback is running in.
//!
//! **One difference from the captures recorded in `docs/BASELINES.md`, stated so
//! the two sets of numbers are not compared as if they were identical.** Arming a
//! capture asks the platform for a frame but does not dirty the window, so where
//! the earlier runs forced a frame before each snapshot, this one adds none: the
//! "before scrolling" sample count is the frames the window had actually drawn,
//! and the bracket is one frame tighter per capture. Nothing else about the
//! measurement moved, and every run prints its own scroll share, so the dilution
//! argument below is re-checkable per run rather than carried over from a table.
//!
//! # Which number is compared against `< 8 ms`, and why
//!
//! `draw_duration` is the figure compared against the threshold. It is the time
//! GPUI spends *building* the frame -- layout, paint, present submission -- which
//! is the half of the frame this client controls and the one that grows when the
//! message list stops being virtualized. `dirty_to_present` is reported alongside
//! it, but it additionally contains the platform's wait for the next present: on a
//! 60 Hz display that wait alone is up to 16.7 ms, so no amount of optimization
//! can put an end-to-end latency figure under 8 ms. Both are printed, both at
//! p50/p95/p99/max, so the reader can see the difference rather than take the
//! choice of metric on trust.
//!
//! # The one measurement caveat, stated rather than hidden
//!
//! gpui's frame histograms are **cumulative for the life of the window** and
//! expose no reset: every frame ever drawn by this window is in them. So the
//! "after scrolling" snapshot also contains the frames drawn while the window
//! opened and while the 10 000 messages were seeded. The bench therefore takes a
//! **"before scrolling" snapshot** too and prints both. The difference of the two
//! sample counts is the number of frames the scroll phase actually drew, and the
//! pre-scroll count is printed as a percentage, so the dilution of the p99 is a
//! number a reviewer can check rather than a claim to accept. The scroll phase is
//! sized (16 s of continuous scrolling) so that those frames are the
//! overwhelming majority of the samples. Across the **list-only** runs whose full
//! reports were retained, the scroll phase drew 1 035 frames against 12 drawn
//! before scrolling, and 1 062 against 13 -- 98.9% and 98.8% -- so the pre-scroll
//! frames occupy about 1.1 to 1.2 percentile points of the distribution the p99 is
//! taken over. That is small, and the verdict does not rest on it: in both runs
//! the pre-scroll frames were the *slower* population (their own p99 was 9.896 ms
//! and 3.391 ms, falling to 1.740 ms and 1.257 ms once the scroll frames were
//! added), so including them can only have pushed the compared-against figure
//! **up**, never down. An earlier draft of this note called the dilution "less
//! than a hundredth of a percentile point" from an estimate of ~2 000 frames; the
//! measurements said otherwise, and the estimate was the thing that was wrong.
//!
//! # Why the profiler's runtime trace is left off
//!
//! `gpui::profiler::set_trace_enabled` exists and this bench deliberately does
//! not call it. `WindowProfiler` documents *"Aggregate histograms are always
//! populated when the `profiler` feature is compiled in"*, and
//! `draw_duration_histogram` -- the histogram this bench's verdict is read from --
//! is one of them: `end_draw` writes it. So for the one figure `AGENTS.md` section
//! 6.2 actually bounds, the compile-time `profiling` feature is the whole switch
//! this measurement needs.
//!
//! Turning the trace on would only add `record_frame_event`, which runs **after**
//! `end_draw` has already computed `draw_duration`: work the measured figure would
//! then exclude, so the number would understate the frame that actually ran.
//!
//! # Why one histogram can be empty
//!
//! `dirty_to_present_histogram` is not in that category, and this bench prints it
//! as a value only when it holds one. Its `dirty_at` is recorded only if
//! `journal::frame_sample_is_valid` accepts it, which drops a frame whose window
//! was not visible at its start, or across which the system's power state changed
//! (`profiler/journal.rs`). That filter is **not** under this bench's control and
//! does not behave consistently: the same binary, with no change to any code,
//! produced 0 samples on one run and 1 043 on the next. So a zero here means "no
//! sample survived the filter", and the report prints it as `NOT MEASURED` rather
//! than as `0.000 ms`, because those two readings are otherwise indistinguishable
//! to a reader.
//!
//! Section 6.2's row is bounded by `draw_duration` either way:
//! `dirty_to_present` additionally contains the wait for the next present (up to
//! 16.7 ms at 60 Hz), making it an end-to-end latency figure that no amount of
//! rendering optimization can bring under 8 ms. What the report therefore owes the
//! reader is an honest blank, not a second verdict on a number the threshold does
//! not govern.
//!
//! # What this bench does NOT measure, stated next to what it does
//!
//! Four limits, and the first two are the reason the two modes exist at all:
//!
//! - **List mode measures a floor.** `BenchRoot` holds one `MessageList` -- no
//!   shell, no theme provider, no drain pump, no key handler. What it establishes
//!   is that the list is not the bottleneck. Saying this for RAM and not for
//!   frame time would be the worse error, because the frame number is the one a
//!   reader is tempted to quote as the result.
//! - **App mode measures the shell, not the finished application.** See its own
//!   section above: the rail and the composer are not constructible yet.
//! - **Focus differs between the modes, because the application focuses its root
//!   and `BenchRoot` is never focused.** The app mode reproduces that on purpose
//!   -- `app::open` focuses the shell on open, so a run that skipped it would be
//!   measuring a client in a state the client never occupies. It is stated because
//!   focus is a rendering state and the two windows are not otherwise identical.
//! - **The two frame modes' RAM rows are levels, not trends.** Six seconds and
//!   eight seconds are enough for a process monitor to sample a level; they
//!   cannot show a leak. `--mode soak` is what gives §6.2's idle-RAM row a time
//!   base, and it measures something else from these two, so their figures are
//!   not superseded by it.
//!
//! Two further limits on what the report can claim:
//!
//! - **Dropped samples are invisible.** gpui's `record_draw_duration` discards a
//!   `hdrhistogram` rejection with `.ok()`, so the bench cannot know whether the
//!   histogram it reads silently refused a value. At 3 significant figures and
//!   ~1 000 samples there is no reason to expect any, but the report cannot prove
//!   there were none.
//! - **A short `dirty_to_present` is not a percentile.** In the runs recorded
//!   here it held as few as 11 samples, where "p99" is the maximum and nothing
//!   more. The `samples=` column is printed next to it for exactly that reason;
//!   read the two together or do not read it at all.
//!
//! # Why `println!` is here
//!
//! `AGENTS.md` section 7.1 bans `println!` in `src/`, where output would be noise
//! in a product. This file is a bench target under `benches/`, outside `src/`, and
//! printing the report *is* the deliverable: the bench's only job is to put these
//! numbers on stdout where the orchestrator can record them.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use gpui::profiler::FrameDurationSnapshot;
use gpui::{
    div, prelude::*, px, rgb, size, AnyWindowHandle, App, AsyncApp, Bounds, Context, Entity,
    Focusable, Render, Window, WindowBounds, WindowOptions,
};
use sh_nexus::app::{self, Shell};
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::Message;
use sh_nexus::state::bridge::{self, Delivery, EventSender};
use sh_nexus::state::{DeliveryState, MAX_MESSAGES_PER_CHANNEL};
use sh_nexus::ui::views::message_list::MessageList;
use sh_nexus::UNSIGNED_IN_USER;
use smallvec::SmallVec;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// What is measured
// ---------------------------------------------------------------------------

/// The scroll-frame budget `AGENTS.md` section 6.2 puts on this figure
/// ("Scroll frame time (10k messages, virtualized) | < 8 ms"), in milliseconds.
///
/// 8 ms is the frame interval of the 120 fps target section 1 leads with, so the
/// threshold is not an arbitrary round number: a frame that takes longer than this
/// cannot be produced at 120 Hz.
const FRAME_BUDGET_MS: f64 = 8.0;

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

/// Which window this run measures.
///
/// **A parameter rather than a second bench target**, for the reason
/// `bridge.rs` section 5 states generally: two targets would be two copies of the
/// four phases, and a fix applied to one of them would leave the other measuring
/// something else. One target, one report format, one code path, and the mode
/// named on every printed line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The production [`MessageList`] in a root this bench owns, and nothing else.
    ///
    /// The floor `docs/BASELINES.md` already recorded, and the mode that attributes
    /// a frame cost to the list rather than to the window.
    List,
    /// The shipped [`Shell`], built and focused the way [`app::open`] does it.
    ///
    /// The mode section 6.2's row is about, subject to the limits the module docs
    /// put next to it.
    App,
    /// The shipped shell, driven by real sends, for §6.2's idle-RAM time base.
    ///
    /// **App mode's window with a different experiment on it, and that is the
    /// whole of the difference.** The root is the same [`Shell`], built by the
    /// same constructor and focused the same way, and the channel is the same
    /// one app mode seeds — so the two figures are about the same client. What
    /// changes is what drives it: a send and its ACK per iteration instead of a
    /// scroll, and a report of three levels and a slope instead of a p99. See
    /// the module docs' soak section for why a third arm is here rather than a
    /// second bench target.
    Soak,
}

impl Mode {
    /// The name printed on every line this mode can change.
    ///
    /// "app-level" rather than "app", because the whole point is that a reader who
    /// sees the word has been told which window produced the number.
    const fn label(self) -> &'static str {
        match self {
            Mode::List => "list-only",
            Mode::App => "app-level",
            Mode::Soak => "app-level soak",
        }
    }

    /// The channel this mode's fixtures belong to.
    ///
    /// **Read from the shell's own constant in app mode rather than from this
    /// file.** The channel the application shows is the shell's decision, and a
    /// bench that seeded a different one would be measuring an empty channel
    /// behind a convincing-looking report. See the module docs.
    const fn channel(self) -> &'static str {
        match self {
            Mode::List => CHANNEL,
            Mode::App | Mode::Soak => app::STARTUP_CHANNEL,
        }
    }

    /// What is still running during an idle dwell, for the RAM sampler's benefit.
    ///
    /// A dwell is only "at rest" if the reader knows what is still ticking, and in
    /// app mode something is: the shell's drain pump, twenty times a second,
    /// applying nothing.
    const fn idle_note(self) -> &'static str {
        match self {
            Mode::List => "no timers at all: the process is genuinely at rest",
            Mode::App | Mode::Soak => {
                "the shell's 50 ms drain pump is ticking on an empty inbox, which \
                 applies nothing and asks for no frame"
            }
        }
    }

    /// How each batch got from the inbox into the renderer.
    ///
    /// Printed with the seeding announcement, because the two modes' seeding paths
    /// are genuinely different and a reader comparing two runs should not have to
    /// guess which one they are looking at.
    const fn seed_note(self) -> &'static str {
        match self {
            Mode::List => {
                "this bench drained the inbox itself, once per batch, on a \
                 SEED_TICK cadence"
            }
            Mode::App | Mode::Soak => {
                "the shell's own 50 ms pump drained each batch, and each batch was \
                 confirmed by the list drawing it"
            }
        }
    }

    /// What this mode's verdict is and is not.
    ///
    /// Two different paragraphs for two different windows, printed under the
    /// verdict. Quoting the list-only figure as the application figure is the
    /// error this file exists to prevent, and the cheapest prevention is saying
    /// which of the two the reader is holding.
    const fn scope(self) -> &'static [&'static str] {
        match self {
            Mode::List => &[
                "scope: this window hosted the production message list and nothing",
                "else -- no shell, no theme provider, no drain pump, no key handler.",
                "The figure establishes that the LIST is not the frame-time",
                "bottleneck, which is a floor and not the application figure.",
                "Run `--mode app` for that; the two are not interchangeable.",
            ],
            Mode::App => &[
                "scope: this window's root was the shipped shell, built by",
                "Shell::new and focused the way app::open focuses it, and its list",
                "was driven through Shell::list(). That is the window section 6.2's",
                "row is about, minus the parts that do not exist yet -- see below.",
            ],
            Mode::Soak => &[
                "scope: this window's root was the shipped shell, the same one",
                "`--mode app` measures, and the sends went through",
                "MessageList::begin_send exactly as the composer's Enter does.",
                "What differs is what drove it and what is reported: no scroll, and",
                "three levels and a slope instead of a frame-time percentile.",
            ],
        }
    }

    /// Parses the value of `--mode`.
    ///
    /// # Errors
    ///
    /// A message naming the three accepted values, because a bench that answers
    /// `unknown mode` with nothing else makes the reader go and read this file.
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "list" => Ok(Mode::List),
            "app" => Ok(Mode::App),
            "soak" => Ok(Mode::Soak),
            other => Err(format!(
                "unknown mode `{other}`; this bench measures `list` (the message list \
                 alone), `app` (the shipped shell) or `soak` (the shell driven by \
                 sends, for section 6.2's idle-RAM time base)"
            )),
        }
    }

    /// Whether this mode runs the send/ACK cycle rather than the scroll.
    ///
    /// **A predicate rather than `matches!(self, Mode::Soak)` at each of the
    /// three call sites**, so that a fourth mode with a different driver is one
    /// edit here rather than three edits that could disagree about what "the
    /// other kind of run" means.
    const fn is_soak(self) -> bool {
        matches!(self, Mode::Soak)
    }
}

/// What the command line asked for.
enum Request {
    /// Print the usage and exit 0.
    Help,
    /// Open a window and measure it in this mode.
    Run(Mode),
    /// Open the shell and run the send/ACK cycle for this many minutes.
    Soak {
        /// How long the load phase runs, in minutes.
        minutes: f64,
    },
}

/// Reads the bench's command line.
///
/// **Hand-rolled rather than a dependency, and that is `AGENTS.md` section 7.2
/// priced rather than asserted:** a flag parser for one optional value and a help
/// flag does not justify a crate in a workspace whose dependency table is an audit
/// trail. `harness = false` in `Cargo.toml` also means `libtest` never sees these
/// arguments, so this function is the only thing that does -- which is why the two
/// flags cargo appends to a bench target's own command line are named and dropped
/// below instead of being reported as mistakes.
///
/// **`--soak-minutes` implies `--mode soak` and is refused alongside either
/// other mode.** A flag that quietly did nothing would be worse than one that
/// refuses: a reader who asked for a two-minute smoke run and got the thirty-five
/// second app measurement would have a report with the wrong time base and no way
/// to tell from the numbers.
///
/// # Errors
///
/// A message naming what was wrong. `--help` is not an error: help is the answer
/// to an unrecognised flag.
fn parse_args() -> Result<Request, String> {
    let mut mode = None;
    let mut soak_minutes = None;
    let mut arguments = std::env::args().skip(1);

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Request::Help),
            // **The two flags cargo itself appends, dropped rather than read.**
            // Checked against this cargo rather than assumed: `cargo bench --bench
            // frame_time --features profiling -- --mode app` invokes the binary as
            // `frame_time-<hash>.exe --mode app --bench`, and the target is
            // declared `harness = false` precisely so that cargo's own harness
            // never interprets the flags meant for this file. They are the only
            // arguments dropped -- anything else is still an error, so a mistyped
            // `--model app` cannot become a silent run in the default mode.
            "--bench" | "--test" => {}
            "--mode" => {
                let value = arguments.next().ok_or_else(|| {
                    "`--mode` needs a value: `list`, `app` or `soak`. See `--help`.".to_owned()
                })?;
                mode = Some(Mode::parse(&value)?);
            }
            // `--mode=app`, accepted because a reader who types it should not have
            // to learn that this bench has an opinion about the spelling.
            other => match other
                .strip_prefix("--mode=")
                .or_else(|| other.strip_prefix("--soak-minutes="))
            {
                Some(value) => {
                    if other.starts_with("--mode=") {
                        mode = Some(Mode::parse(value)?);
                    } else {
                        soak_minutes = Some(parse_minutes(value)?);
                    }
                }
                None if other == "--soak-minutes" => {
                    let value = arguments.next().ok_or_else(|| {
                        "`--soak-minutes` needs a value, in minutes. See `--help`.".to_owned()
                    })?;
                    soak_minutes = Some(parse_minutes(&value)?);
                }
                None => {
                    return Err(format!(
                        "unrecognised argument `{other}`; this bench takes no \
                         positional arguments. See `--help`."
                    ))
                }
            },
        }
    }

    // `--soak-minutes` on its own means the soak, because that is the only thing
    // it can change. With an explicit mode it is only legal against the soak, and
    // saying so is the difference between a refused combination and a report
    // whose time base is not the one that was asked for.
    if let Some(minutes) = soak_minutes {
        if let Some(mode) = mode.filter(|mode| !mode.is_soak()) {
            return Err(format!(
                "`--soak-minutes` sets the length of the send/ACK soak, and this run \
                 also asked for `--mode {}`. The two are different experiments; pick \
                 one",
                mode.label()
            ));
        }
        return Ok(Request::Soak { minutes });
    }

    // Absent means list, and the default is the mode `docs/BASELINES.md` already
    // recorded, so the command in that file still measures what it measured.
    Ok(Request::Run(mode.unwrap_or(Mode::List)))
}

/// One `--soak-minutes` value, in minutes.
///
/// **Parsed here rather than at the use site so a typo is a command-line error
/// and not a zero-length run.** `f64` and not an integer because a smoke run
/// wants to be shorter than a minute and a whole number of minutes is too coarse
/// to iterate with; the finiteness and positivity checks are the ones that turn
/// `"NaN"`, `"inf"` and `"0"` into messages rather than into a run that finishes
/// before it starts.
///
/// # Errors
///
/// A message naming the value that could not be used, and the range that can.
fn parse_minutes(value: &str) -> Result<f64, String> {
    let minutes = value.parse::<f64>().map_err(|_| {
        format!("`--soak-minutes` takes a number of minutes, and `{value}` is not one")
    })?;
    if !minutes.is_finite() || minutes <= 0.0 {
        return Err(format!(
            "`--soak-minutes` must be a finite number greater than zero, and `{value}` \
             is not: the default is {SOAK_DEFAULT_MINUTES} minutes, which is \
             AGENTS.md 6.2's own time base"
        ));
    }
    Ok(minutes)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The channel list mode seeds and shows.
///
/// Not used in app mode, which seeds [`app::STARTUP_CHANNEL`]; see [`Mode::channel`].
const CHANNEL: &str = "c_bench";

/// The colleague every seeded message is attributed to.
///
/// Everything is somebody else's message on purpose: this bench measures the frame
/// cost of *rendering* history, and a split of self/peer messages would change the
/// row content without changing the question.
const PEER: &str = "u_bench_peer";

/// How many messages the scroll phase scrolls through.
///
/// `AGENTS.md` section 6.2's row is explicitly "10k messages", and
/// `state/app_state.rs` now bounds a channel at
/// [`sh_nexus::state::MAX_MESSAGES_PER_CHANNEL`]. **These are the same number,
/// deliberately**: the cap is the project's own figure (section 6.2's
/// *"RAM with 10k cached messages < 200 MB"* row), and seeding exactly the cap
/// means the bench measures the state the budget row is written about rather
/// than a state one message short of it.
///
/// **So this fixture sits exactly on the boundary, and the first insert past it
/// is the first eviction.** That is a useful property rather than a hazard: the
/// figure `docs/BASELINES.md` records was measured on a channel that had never
/// evicted, which is still the right figure for a *load* of 10 000, and the
/// bench's own conclusion about which end of the array is the bottleneck does not
/// depend on what happens at 10 001. It is stated here because a reader who
/// assumes the two numbers are independent would be wrong about one of them.
const TOTAL_MESSAGES: usize = MAX_MESSAGES_PER_CHANNEL;

/// Bodies the fixtures use, cycled by index.
///
/// A deliberate mix: plain text, inline markdown, and a fenced code block. The
/// code block makes some rows taller than others, which is the property that made
/// ADR-006 choose `gpui::List` over `UniformList` -- a bench whose rows were all
/// one line would never exercise the differently-sized measurement the design
/// rests on.
const BODIES: [&str; 5] = [
    "deploy is ready",
    "can you take a look at **PR #42**? the diff is small",
    "running `cargo clippy -- -D warnings` now",
    "no blockers here, see you tomorrow",
    "```rust\nlet measured = frame.draw_duration();\nassert!(measured < budget);\n```",
];

// ---------------------------------------------------------------------------
// The soak's own numbers
// ---------------------------------------------------------------------------

/// How long a soak runs when the command line does not say.
///
/// **§6.2's own words rather than a round number this bench chose**: the row
/// asks for idle RAM "over 30 minutes of active chatting", so 30 is the time
/// base being reproduced. `--soak-minutes` exists so a smoke run can be shorter;
/// **a shorter run is not the 30-minute figure and the report says how long it
/// actually ran**, because the slope is the only part of a short run that
/// extrapolates and the level is not.
const SOAK_DEFAULT_MINUTES: f64 = 30.0;

/// How long each of the two idle dwells runs.
///
/// **The same length at both ends, and that is the point rather than symmetry
/// for its own sake**: the two dwells are the two ends of one comparison, and a
/// reader who wants to check that the "after" figure really did come back down
/// can only do it if the windows are the same size.
const SOAK_DWELL: Duration = Duration::from_secs(15);

/// How often the soak says where it is.
///
/// A thirty-minute run that printed nothing would be indistinguishable from a
/// hung one, and the reader deciding whether to kill it is the only person who
/// can make that call.
const SOAK_ANNOUNCE: Duration = Duration::from_secs(60);

/// How many of the run's own sends are re-read at each census.
///
/// **A bounded sample rather than all of them, and the bound is load-bearing.**
/// Keeping every identity would put a `Vec<Uuid>` of tens of thousands of
/// entries inside the very process whose working set is being measured, so the
/// bench would be one of the things it is measuring — and it would be the only
/// thing growing on a schedule of its own. Sixty-four answers the only question
/// the sample is asked, "does the delivery map still hold a send from twenty
/// minutes ago?", for a kilobyte.
const SOAK_PROBE_EVERY: usize = 64;

/// How many cycles between censuses during the load phase.
///
/// The census is a handful of O(1) reads plus one O(messages) sweep for
/// `pending_sends`, so a few hundred across half an hour costs nothing, and
/// taking one per cycle would put the measurement's own bookkeeping in the
/// path it is measuring.
const SOAK_CENSUS_EVERY: usize = 250;

/// How long a renderer wait is given before the run calls the renderer broken.
///
/// 600 x [`PUMP_POLL`] is three seconds, which is sixty of the shell's 50 ms
/// drain ticks. A row that has not been drawn in sixty ticks was not drawn
/// slowly, and the failure says so — because the alternative, carrying on, would
/// report a stalled client as a quiet one.
const SOAK_DRAWN_POLLS: usize = 600;

/// The `client_msg_id` of the oldest seeded message, as a probe for eviction.
///
/// **Read rather than inferred, and that is the whole reason it exists.** The
/// channel is seeded to exactly [`MAX_MESSAGES_PER_CHANNEL`], so the first send
/// is one over the bound and `evict_one_over_cap` must retire something — but
/// "must" is a claim about code, and this asks the state instead. The answer
/// also dates the eviction: the cycle records the iteration at which this id
/// stopped being held, so the report can say when the cap started biting rather
/// than reasoning that it did.
const SEEDED_HEAD: u128 = 1;

// ---------------------------------------------------------------------------
// Phase lengths -- fixed, so an external monitor can poll at rest
// ---------------------------------------------------------------------------

/// Phase 1 dwell: window open, empty channel, no input.
const IDLE_DWELL: Duration = Duration::from_secs(6);

/// Phase 2 dwell: window open, 10 000 messages loaded, no input.
const SEEDED_DWELL: Duration = Duration::from_secs(8);

/// Phase 3 length: continuous scrolling, half of it upwards and half downwards.
///
/// Sized so the scroll frames outnumber the pre-scroll frames by roughly two
/// orders of magnitude (see the module docs: gpui has no histogram reset), and
/// long enough that an external monitor gets a stable sample even at the slow end
/// of the frame loop.
const SCROLL_DURATION: Duration = Duration::from_secs(16);

/// How long to wait between scroll steps.
///
/// 8 ms is the 120 Hz frame interval; the loop effectively runs at the display's
/// refresh rate because the redraw is coalesced, so this is a *cap* on how often
/// the bench asks for a frame rather than a claim about how often one is
/// produced.
const SCROLL_TICK: Duration = Duration::from_millis(8);

/// How far to scroll per tick, in logical pixels.
///
/// ~48 px is well under one row (rows measure ~68 px for a single-line body), so
/// the list walks rather than jumps: a jump would re-measure a screenful and
/// measure something no gesture produces.
const SCROLL_STEP_PX: f32 = 48.0;

/// How long to wait between seeding batches in **list** mode.
///
/// Only so the frame loop can draw the growth between batches instead of
/// coalescing all ten into one frame -- the seeded state should be a state the
/// renderer actually rendered, not one it was handed at once. App mode does not use
/// it: there the cadence is the shell's 50 ms pump, and the batch is confirmed by
/// the list drawing it rather than by a timer expiring.
const SEED_TICK: Duration = Duration::from_millis(16);

/// How often the app mode checks whether the pump has had a batch drawn.
const PUMP_POLL: Duration = Duration::from_millis(5);

/// How many of those checks before the app mode gives up on a batch.
///
/// 400 x 5 ms is two seconds, which is eighty of the shell's 50 ms pump ticks. The
/// arithmetic is in the failure message: a batch that has not been drawn in eighty
/// ticks did not arrive slowly, it did not arrive, and saying which is the
/// difference between a diagnosis and an observation.
const PUMP_POLLS: usize = 400;

/// How long a capture waits for the window to take the snapshot it asked for.
///
/// Ten seconds of patience: a window that has not produced a frame by then is a
/// window that is not drawing at all, and waiting longer would only turn a broken
/// measurement into a slow one.
const CAPTURE_POLLS: usize = 400;

/// How long to wait between those checks.
const CAPTURE_POLL: Duration = Duration::from_millis(25);

/// Window width, logical pixels.
const WINDOW_WIDTH: f32 = 1024.0;

/// Window height, logical pixels.
///
/// Chosen rather than inherited from the spike's 480x320 on purpose: rows per
/// frame -- and therefore this measurement -- scale with viewport height, and a
/// spike-sized window would render four rows and publish a number that says
/// nothing about a chat window.
const WINDOW_HEIGHT: f32 = 768.0;

/// The bench's window and the shell's window are the same size, checked by the
/// compiler rather than by a reader.
///
/// **This is the one assumption the two modes share, and it is load-bearing in the
/// direction that hides nothing.** Rows per frame scale with viewport height, so
/// an app-level figure taken in a differently-sized window would be incomparable
/// with the list-only floor `docs/BASELINES.md` recorded -- and would look
/// perfectly publishable while being meaningless. `app::WINDOW_WIDTH` says in its
/// own documentation that it exists to match this bench, and this assertion is
/// what makes that sentence true instead of aspirational. If the shell's window is
/// ever resized on purpose, the bench's constants move with it in the same change
/// and every figure is re-measured.
const _: () = assert!(
    WINDOW_WIDTH == app::WINDOW_WIDTH && WINDOW_HEIGHT == app::WINDOW_HEIGHT,
    "the bench's window geometry and the shell's have diverged; the app-level \
     figure would not be comparable with the list-only floor, so fix both in one \
     change and re-measure"
);

// ---------------------------------------------------------------------------
// The probe: percentiles computed inside a frame callback, plain numbers out
// ---------------------------------------------------------------------------

/// One histogram's numbers, reduced to plain integers.
///
/// Deliberately carries no histogram: `frame_duration_snapshot` hands out a
/// `hdrhistogram::Histogram`, which gpui does not re-export, and neither that type
/// nor anything owning it is moved out of the frame it was read in.
#[derive(Clone, Copy)]
struct Stats {
    /// How many frames the histogram holds. The work order requires this to be
    /// non-zero before any percentile is worth printing.
    samples: u64,
    /// p50, nanoseconds.
    p50_ns: u64,
    /// p95, nanoseconds.
    p95_ns: u64,
    /// p99, nanoseconds -- the statistic the threshold is judged at.
    p99_ns: u64,
    /// The single worst frame, nanoseconds.
    max_ns: u64,
}

impl Stats {
    /// The value every field takes when a histogram holds no samples.
    ///
    /// Rather than asking `value_at_quantile` what the p99 of nothing is: the
    /// report treats `samples == 0` as a failed measurement, and a fabricated zero
    /// must never be printable as if it were one.
    const EMPTY: Stats = Stats {
        samples: 0,
        p50_ns: 0,
        p95_ns: 0,
        p99_ns: 0,
        max_ns: 0,
    };
}

/// One snapshot of the window's frame histograms, at one point in the run.
///
/// `Clone` and not `Copy` because it is handed out of a `RefCell` borrow by the
/// soak's report lookups, and a `&Report` into that temporary is a lifetime the
/// callers would have to carry. Every field is `Copy`, so the clone is five
/// register moves rather than a heap allocation.
#[derive(Clone)]
struct Report {
    /// Which phase this snapshot was taken at.
    label: &'static str,
    /// When it was taken, measured from the bench's start.
    at: Duration,
    /// `Window::draw` durations -- the work the client does per frame.
    draw: Stats,
    /// First-invalidation-to-present durations -- work plus the wait to show it.
    dirty_to_present: Stats,
    /// Whether the window was visible when this snapshot was read.
    ///
    /// Recorded because `draw_duration` carries **no visibility filter**: `end_draw`
    /// records it unless the system's power state changed across the frame
    /// (`profiler.rs`), and no visibility condition is consulted at all, while
    /// `dirty_to_present` only survives `journal::frame_sample_is_valid`, which
    /// drops frames whose window was hidden. So a run whose window lost visibility
    /// mid-scroll puts its samples in the histogram that *is* judged, and nothing
    /// else in the report would reveal it.
    visible: bool,
}

/// Where the snapshots go.
///
/// An `Rc<RefCell<_>>` rather than a channel because both ends run on the main
/// thread: the window's frame callback and the driver's awaits are never
/// concurrent, so a `Send` boundary would buy nothing and cost an allocation per
/// capture.
#[derive(Default)]
struct FrameProbe {
    /// Every snapshot taken so far, in order.
    reports: Vec<Report>,
}

/// Turns one frame-duration snapshot into plain numbers.
///
/// The histograms are read **field by field inside this function rather than
/// through a helper taking `&Histogram<u64>`**, because writing that signature
/// would mean naming `hdrhistogram::Histogram` -- a type gpui does not re-export,
/// so naming it means adding a dependency solely to spell a parameter
/// (`AGENTS.md` section 7.2 prices exactly that). The struct being held *is*
/// nameable, and reaching through its public fields needs no import.
///
/// The empty-histogram branch exists because a window's first snapshot can
/// precede its first `end_draw`: a snapshot taken there legitimately holds zero
/// samples, and asking a `hdrhistogram` for the p99 of nothing is not a question
/// this bench should depend on the answer to.
fn snapshot_report(
    label: &'static str,
    started: Instant,
    visible: bool,
    snapshot: &FrameDurationSnapshot,
) -> Report {
    let draw_samples = snapshot.draw_duration_histogram.len();
    let draw = if draw_samples == 0 {
        Stats::EMPTY
    } else {
        Stats {
            samples: draw_samples,
            p50_ns: snapshot.draw_duration_histogram.value_at_quantile(0.50),
            p95_ns: snapshot.draw_duration_histogram.value_at_quantile(0.95),
            p99_ns: snapshot.draw_duration_histogram.value_at_quantile(0.99),
            max_ns: snapshot.draw_duration_histogram.max(),
        }
    };

    let dirty_samples = snapshot.dirty_to_present_histogram.len();
    let dirty_to_present = if dirty_samples == 0 {
        Stats::EMPTY
    } else {
        Stats {
            samples: dirty_samples,
            p50_ns: snapshot.dirty_to_present_histogram.value_at_quantile(0.50),
            p95_ns: snapshot.dirty_to_present_histogram.value_at_quantile(0.95),
            p99_ns: snapshot.dirty_to_present_histogram.value_at_quantile(0.99),
            max_ns: snapshot.dirty_to_present_histogram.max(),
        }
    };

    Report {
        label,
        at: started.elapsed(),
        draw,
        dirty_to_present,
        visible,
    }
}

// ---------------------------------------------------------------------------
// The roots
// ---------------------------------------------------------------------------

/// The list-only window's root: the production [`MessageList`], and nothing else.
///
/// The message list itself is untouched production code, driven through its public
/// API (`show_channel`, `list_state`), for the reason
/// `tests/ui_message_list.rs` gives for going through the bridge: a bench that
/// wired the layers together differently from the application would be measuring
/// the bench.
///
/// **It holds no probe.** The snapshots are read from the *window*, not from a
/// view, which is what lets the app mode's root be the shipped shell -- see the
/// module docs, "Where the snapshot is taken".
struct BenchRoot {
    /// The list under measurement.
    list: Entity<MessageList>,
}

impl BenchRoot {
    /// Marks this view **and the list** dirty, so the next frame rebuilds both.
    ///
    /// Both halves are required, and the second is the one that is easy to miss.
    /// gpui rebuilds a view only when that view is in `window.dirty_views`, and
    /// `Window::mark_view_dirty` marks a notified view's *ancestors* as well -- so
    /// notifying the list alone would rebuild the list and, with it, this root.
    /// Notifying **only this root** would do the reverse: rebuild the root while
    /// `gpui/src/view.rs`'s prepaint cache hands the list back its previous layout
    /// unchanged, which for a scroll means changing the list's state and painting
    /// nothing -- a frame that is drawn, recorded, and shows the row the user is no
    /// longer looking at.
    ///
    /// The app mode does exactly the same two things to the same two views; the
    /// rule is gpui's, and the shell is a root like any other.
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.list.update(cx, |_list, list_cx| list_cx.notify());
    }
}

impl Render for BenchRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("bench-root")
            .flex()
            .flex_col()
            .size_full()
            // Explicit colours, per `AGENTS.md` section 7.3: GPUI does not
            // inherit text colour from parents, and this container is the one
            // every row draws on top of. The hard-coded palette is the point of
            // list mode: the theme is the shell's job, and a shell that also
            // carried one would answer a different question.
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(self.list.clone())
    }
}

/// The window's root view, which is the only thing the two modes disagree about.
///
/// An enum with one arm per mode rather than a trait or a generic: there are two
/// roots, they are different types, and a `match` whose arms are one line each can
/// be checked against `app::open` in a single reading -- which is the property this
/// file needs and the one a trait object would hide behind a dispatch.
enum Root {
    /// List mode: this bench's own root, holding one list.
    Bench(Entity<BenchRoot>),
    /// App mode: the shipped shell.
    Shell(Entity<Shell>),
}

/// One run: the window under measurement, plus what the four phases drive it with.
struct Run {
    /// Which window this is. Printed on every line the mode can change.
    mode: Mode,
    /// The root view, reached through the mode's own type.
    root: Root,
    /// The window whose histograms this run reads, with its type erased.
    ///
    /// `AnyWindowHandle` rather than `WindowHandle<BenchRoot>` or
    /// `WindowHandle<Shell>` because the capture needs only the window, and naming
    /// either root type here would make the frame callback a different function per
    /// mode. `root_entity_type_name` is the read-back the banner and the report
    /// print, so the mode label is a claim the window itself confirms.
    window: AnyWindowHandle,
    /// The producer handle the seeding path delivers through.
    ///
    /// **The bench's own handle in both modes.** In app mode the shell holds a
    /// *clone* of it, and that is what keeps the inbox open for the life of the
    /// window -- `bridge.rs` section 2 is explicit that a dropped sender closes it.
    /// In list mode nothing else holds one at all, which is precisely why this
    /// bench drains the inbox itself there.
    sender: EventSender,
    /// Where the snapshots go.
    probe: Rc<RefCell<FrameProbe>>,
    /// When the run started, so every report says when it was captured.
    started: Instant,
}

impl Run {
    /// The channel this run seeds.
    fn channel(&self) -> &'static str {
        self.mode.channel()
    }

    /// Puts the list on this run's channel, or checks that the shell already did.
    ///
    /// **The two arms are genuinely different, and the difference is the mode.**
    /// List mode owns its window and so chooses the channel, with `show_channel`
    /// called *before* any message arrives so that every seeded batch is reconciled
    /// by the production path (`MessageList::sync` splicing on the next frame)
    /// rather than by a `reset` that hands the list a finished count. App mode
    /// cannot choose: the shell chose in `Shell::new`, so the bench's only job is
    /// to confirm it -- seeding a channel nothing renders would otherwise produce
    /// an empty-list measurement that looked like a result.
    ///
    /// # Errors
    ///
    /// App mode only: the list is not showing `app::STARTUP_CHANNEL`, which means
    /// the window was built differently from `app::open` and every figure below
    /// would describe a channel nobody sees.
    fn open_channel(&self, cx: &mut AsyncApp) -> Result<(), String> {
        match &self.root {
            Root::Bench(root) => root.update(cx, |bench, view_cx| {
                bench
                    .list
                    .update(view_cx, |list, list_cx| list.show_channel(CHANNEL, list_cx));
            }),
            Root::Shell(shell) => {
                let showing = shell.read_with(cx, |shell, _app| {
                    shell
                        .list()
                        .read_with(_app, |list, _| list.channel().map(str::to_owned))
                });
                if showing.as_deref() != Some(app::STARTUP_CHANNEL) {
                    return Err(format!(
                        "the shell's list is showing {showing:?}, not the channel this \
                         mode seeds ({:?}); a run that measured a channel the window \
                         does not show would be measuring nothing",
                        app::STARTUP_CHANNEL
                    ));
                }
            }
        }
        Ok(())
    }

    /// Asks for a frame that rebuilds the root and the list.
    ///
    /// **Both halves, in both modes, for `BenchRoot::invalidate`'s reason**: a
    /// frame that rebuilds only one of the two is a frame that changes state and
    /// paints the old one.
    fn invalidate(&self, cx: &mut AsyncApp) {
        match &self.root {
            Root::Bench(root) => root.update(cx, |bench, view_cx| bench.invalidate(view_cx)),
            Root::Shell(shell) => shell.update(cx, |shell, view_cx| {
                view_cx.notify();
                shell
                    .list()
                    .update(view_cx, |_list, list_cx| list_cx.notify());
            }),
        }
    }

    /// Scrolls the list by `step` logical pixels and asks for the frame.
    ///
    /// **Both arms go through the public accessor the application would use** --
    /// `BenchRoot`'s handle in list mode, `Shell::list()` in app mode -- and
    /// neither reaches into the list to move it. `read` rather than a direct method
    /// call, because gpui's `Entity<T>` is deliberately not
    /// `Deref<Target = T>`: the list's state is only reachable through an explicit
    /// read, and the read is one statement so its borrow of the context ends before
    /// the notify takes `&mut` of it.
    fn scroll(&self, step: f32, cx: &mut AsyncApp) {
        match &self.root {
            Root::Bench(root) => root.update(cx, |bench, view_cx| {
                bench.list.read(view_cx).list_state().scroll_by(px(step));
                bench.invalidate(view_cx);
            }),
            Root::Shell(shell) => shell.update(cx, |shell, view_cx| {
                shell.list().read(view_cx).list_state().scroll_by(px(step));
                shell
                    .list()
                    .update(view_cx, |_list, list_cx| list_cx.notify());
            }),
        }
    }

    /// How many messages the list is actually holding, read from the view.
    ///
    /// Read from the **view** rather than from the state, because the figure that
    /// matters to this measurement is the one the renderer is drawing: a state that
    /// holds 10 000 messages and a list that reconciled none of them would measure
    /// an empty channel and publish it as a 10 000-message result.
    fn rendered(&self, cx: &AsyncApp) -> usize {
        match &self.root {
            Root::Bench(root) => root.read_with(cx, |bench, app| {
                bench.list.read_with(app, |list, _| list.item_count())
            }),
            Root::Shell(shell) => shell.read_with(cx, |shell, app| {
                shell.list().read_with(app, |list, _| list.item_count())
            }),
        }
    }

    /// Arms one snapshot, to be taken at the start of this window's next frame.
    ///
    /// `on_next_frame` rather than a flag read from a `render`, and the reason is
    /// the one the module docs give: a render-side probe would have to live inside
    /// the shell's render to reach the app mode's frames, and instrumenting `src/`
    /// to measure `src/` is not a trade this project makes.
    ///
    /// # Errors
    ///
    /// The window is gone: `AnyWindowHandle::update` fails if the window has
    /// closed, and a capture that can never be taken is reported rather than
    /// waited on.
    fn arm_capture(&self, cx: &mut AsyncApp, label: &'static str) -> Result<(), String> {
        let probe = Rc::clone(&self.probe);
        let started = self.started;
        self.window
            .update(cx, move |_view, window, _app| {
                window.on_next_frame(move |window, _app| {
                    let snapshot = window.frame_duration_snapshot();
                    let visible = window.is_visible();
                    probe
                        .borrow_mut()
                        .reports
                        .push(snapshot_report(label, started, visible, &snapshot));
                });
            })
            .map_err(|error| format!("the window could not arm the `{label}` snapshot: {error}"))
    }

    /// The production message list this window is showing.
    ///
    /// **Reached through the shell's own accessor and returned as a handle, for
    /// the reason every other handle in this file is a handle:** the borrow a
    /// `read` would hand back ends with the statement, and the soak needs to
    /// hold this across an `await`. A clone of an `Entity` is one refcount.
    ///
    /// # Errors
    ///
    /// List mode. The soak's whole premise is the shipped shell — the pump that
    /// drains, the `Shell::sender` the ACKs go through, and the `outgoing`
    /// bookkeeping a real composer drives — and a list in a window this bench
    /// owns has none of those, so a soak there would be a different experiment
    /// wearing this one's name.
    fn list(&self, cx: &AsyncApp) -> Result<Entity<MessageList>, String> {
        match &self.root {
            Root::Shell(shell) => Ok(shell.read_with(cx, |shell, _app| shell.list().clone())),
            Root::Bench(_) => Err(
                "the send/ACK soak runs in the shipped shell, and this window hosted \
                 the list alone; run it with `--mode soak`"
                    .to_owned(),
            ),
        }
    }

    /// What one row is drawing, read from the renderer.
    ///
    /// **This is the soak's wait condition, and it is a renderer read on
    /// purpose.** A row is created by `MessageList::render_row`, and its spec is
    /// replaced by `RowCache::row` — both of which run inside a frame, both of
    /// which only run for a row in the visible range. So a spec that says
    /// `Acked` can only be true once the shell's pump drained the event, the
    /// pump notified, and a frame drew the reconciliation. Reading
    /// `AppState::delivery` instead would answer "did the state change", which
    /// is true one step earlier and would let a run that never drew a frame pass
    /// its own consistency checks.
    ///
    /// `None` covers both "no such row is retained" and "the row is retained and
    /// has no delivery state", collapsed, because the wait only ever asks
    /// "has this reached the state I am waiting for" and `Pending`/`Acked` are
    /// the only answers it acts on.
    fn drawn_delivery(
        &self,
        list: &Entity<MessageList>,
        client_msg_id: &Uuid,
        cx: &AsyncApp,
    ) -> Option<DeliveryState> {
        list.read_with(cx, |list, app| {
            list.retained_row(client_msg_id)
                .and_then(|row| row.read_with(app, |row, _| row.spec().delivery))
        })
    }

    /// How many messages the channel holds, right now.
    ///
    /// **One accessor rather than the whole census, and the reason is the moment
    /// it is read.** The history bound is exceeded for exactly as long as a send
    /// is outstanding and its eviction has not landed — the length of one pump
    /// tick. A periodic census would miss that state every time, and a report
    /// that then said "the held count never exceeded the cap" would be reporting
    /// its own sampling interval rather than the client's behaviour. This is
    /// called once per cycle at the one instant the state is over the bound, and
    /// it is one hash probe and a `len`.
    ///
    /// # Errors
    ///
    /// When the state is not installed, which for a run that has already seeded
    /// 10 000 messages would mean something has gone badly wrong.
    fn held(&self, cx: &AsyncApp) -> Result<usize, String> {
        let channel = self.channel();
        cx.update(|app| bridge::try_read(app, |state| state.message_count(channel)))
            .ok_or_else(|| {
                "the application state was not installed when a cycle's send was still \
                 outstanding, so the history bound could not be read"
                    .to_owned()
            })
    }

    /// One read-only census of the client's own structures.
    ///
    /// **Every field is a public accessor reached through `bridge::try_read`,
    /// or a list accessor, and nothing here names the state type** — the same
    /// seam `tests/bridge.rs` holds `ui/` to. That is what makes these figures
    /// about the shipped client rather than about this file: there is no second
    /// source for them, and a reader can re-derive every one by hand.
    fn census(
        &self,
        list: &Entity<MessageList>,
        probes: &[Uuid],
        cx: &AsyncApp,
    ) -> Result<Census, String> {
        let channel = self.channel();
        let (held, outstanding, unread, segments, item_count, retained_rows, tracked, head_held) =
            list.read_with(cx, |list, app| {
                // Every read is `?` rather than a `let...else`, and that is the point:
                // one `None` at the end of the tuple is the whole of "the state is not
                // installed", and a `?` cannot be forgotten when a seventh field is
                // added -- which is the failure a `let...else` per field invites.
                let held = bridge::try_read(app, |state| state.message_count(channel))?;
                let outstanding = bridge::try_read(app, |state| state.pending_sends().len())?;
                let unread = bridge::try_read(app, |state| state.unread(channel))?;
                let segments = bridge::try_read(app, |state| state.segment_cache_stats())?;
                // The two probes. `tracked` is how much of the delivery map this run's
                // own sends still occupy, which is the one growing structure the public
                // surface can be asked about directly; `head_held` is whether the oldest
                // seeded row survived, which is the eviction witness on `SEEDED_HEAD`.
                let tracked = bridge::try_read(app, |state| {
                    probes
                        .iter()
                        .filter(|id| state.delivery(id).is_some())
                        .count()
                })?;
                let head_held = bridge::try_read(app, |state| {
                    state
                        .message(channel, &Uuid::from_u128(SEEDED_HEAD))
                        .is_some()
                })?;
                Some((
                    held,
                    outstanding,
                    unread,
                    segments,
                    list.item_count(),
                    list.retained_rows(),
                    tracked,
                    head_held,
                ))
            })
            .ok_or_else(|| {
                "the application state was not installed when the census was taken, so \
                 every figure below would be a zero standing for nothing"
                    .to_owned()
            })?;
        Ok(Census {
            held,
            outstanding,
            unread,
            segments: segments.0,
            segment_bytes: segments.1,
            item_count,
            retained_rows,
            tracked,
            head_held,
        })
    }
}

// ---------------------------------------------------------------------------
// The soak: what one cycle is, and what it is read against
// ---------------------------------------------------------------------------

/// One read of the client's own structures, at one instant.
///
/// **Counts, not bytes, and that is the honest split rather than a shortcut.**
/// The working set is a property of the process and belongs to an external
/// monitor — `AGENTS.md` §6.2's own tool column, and the reason this bench adds
/// no dependency to read its own memory. What the client *owns* is a set of
/// bounded containers, and every one of them has a length this struct can read
/// through the public seam. So the report can say what grew and by how much per
/// cycle, and the CSV beside it says what the process did about it.
#[derive(Clone, Copy, Debug, Default)]
struct Census {
    /// Messages the channel holds. Bounded by `MAX_MESSAGES_PER_CHANNEL` except
    /// for the documented all-sends-in-flight case, which this cycle cannot reach.
    held: usize,
    /// Sends still awaiting an answer. One at a time in this cycle, because each
    /// is acknowledged before the next is made — which is the property that
    /// keeps the eviction guard's over-cap case out of the run.
    outstanding: usize,
    /// Unread elements naming a held row, for this channel.
    ///
    /// A *count of the set*, not of the channels: `AppState::unread` derives it
    /// by walking `counted`, so it is a real read of the shipped rule rather than
    /// a figure this bench keeps.
    unread: u32,
    /// Entries in the rendered-segment LRU.
    segments: usize,
    /// The declared cost that LRU is holding, in bytes.
    segment_bytes: u64,
    /// Rows the list is showing, read from the list state.
    item_count: usize,
    /// Rows the list's row cache is holding. Bounded by `MAX_RETAINED_ROWS`.
    retained_rows: usize,
    /// How many of the sampled identities from this run are still tracked by the
    /// delivery map. See [`SOAK_PROBE_EVERY`] for why this is a sample.
    tracked: usize,
    /// Whether the oldest seeded message is still held — the eviction witness.
    head_held: bool,
}

/// One row of the soak's census table: what it is called, and what it reads.
///
/// **A named type because the table is a list of these, and a list of
/// `(&str, fn(&Census) -> u64)` is exactly the shape clippy calls "too complex"
/// — which is the compiler saying the pair is a thing rather than a
/// coincidence.** The two halves travel together on purpose: a table that pairs
/// labels with readers can be mistranscribed, and a mistranscribed row is a
/// plausible, confident and completely wrong figure.
type CensusRow = (&'static str, fn(&Census) -> u64);

impl Census {
    /// The larger of two censuses, field by field.
    ///
    /// Used for the "under load" figure, which is a maximum over the run rather
    /// than an end value: for a container bounded by a cap, the maximum *is* the
    /// level, and for a container that is not, the maximum is the only reading
    /// that answers "did this ever get worse".
    fn peak(&self, other: &Census) -> Census {
        Census {
            held: self.held.max(other.held),
            outstanding: self.outstanding.max(other.outstanding),
            unread: self.unread.max(other.unread),
            segments: self.segments.max(other.segments),
            segment_bytes: self.segment_bytes.max(other.segment_bytes),
            item_count: self.item_count.max(other.item_count),
            retained_rows: self.retained_rows.max(other.retained_rows),
            tracked: self.tracked.max(other.tracked),
            head_held: self.head_held && other.head_held,
        }
    }
}

/// The body one soak iteration sends.
///
/// **Unique per iteration, and that is load-bearing rather than tidy.** A
/// constant string would hit the rendered-segment cache on every repeat, the
/// parse a real send does would never happen, and the cache would sit at
/// whatever size the seed left it — so the run would measure a structure that
/// never changed. The body still cycles [`BODIES`], so the rows a send produces
/// are the same mix of one-liners, inline markdown and a code block the seeded
/// history is, and a row's height is as variable as a colleague's would be.
fn soak_body(iteration: usize) -> String {
    format!("soak {iteration}: {}", BODIES[iteration % BODIES.len()])
}

/// The server's answer to one soak send, as `message.ack` carries it.
///
/// **Same `client_msg_id` and same `timestamp` as the optimistic row, and both
/// matter.** The identity is what makes the ACK a reconciliation rather than a
/// second row for one send — `actions::acknowledge` asks `channel_holding(..)`
/// before anything else. The timestamp is what keeps the row where it is:
/// `core/ordering::compare` sorts on timestamp first, so an acceptance time even
/// a second later would move the reconciled row, and the wait below is watching
/// one row's spec.
fn soak_ack(
    channel: &str,
    client_msg_id: Uuid,
    content: &str,
    at: DateTime<Utc>,
    iteration: usize,
) -> Message {
    Message {
        // A real server id, which is what `insert_message` requires before it
        // will index the message at all, and what `actions::ingest` reads to
        // decide the send left `Pending`.
        id: format!("m_soak_{iteration}"),
        client_msg_id,
        channel_id: channel.to_owned(),
        // This client. The bench installs with `UNSIGNED_IN_USER`, so a row
        // attributed to anyone else would stop being `is_self` and the delivery
        // bookkeeping would skip it.
        user_id: UNSIGNED_IN_USER.to_owned(),
        content: content.to_owned(),
        timestamp: at,
        edited_at: None,
        reactions: SmallVec::new(),
        thread_id: None,
        attachments: SmallVec::new(),
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The timestamp of the `index`-th fixture message.
///
/// A fixed epoch rather than a clock read: `AGENTS.md` section 3.2 gives no clock
/// to this layer's callers in general, and a bench whose ordering depended on
/// wall-clock resolution would be a bench that could reorder its own fixtures.
/// Strictly increasing in `index`, so `insert_at_order` appends every one of them.
fn at(index: usize) -> DateTime<Utc> {
    let seconds = 1_789_000_000 + index as i64;
    Utc.timestamp_opt(seconds, 0).single().unwrap_or_else(|| {
        // Unreachable: the range above is comfortably inside `DateTime`'s.
        // Stated rather than `unwrap`ed because a fixture is still production
        // code's neighbour, and the honest failure of a bench is a message.
        eprintln!("frame_time: fixture timestamp {seconds}s is not representable");
        DateTime::<Utc>::MIN_UTC
    })
}

/// The `index`-th fixture message, as the server would deliver it.
///
/// `channel` is a parameter rather than a constant because it is mode-dependent:
/// see [`Mode::channel`] and the module docs.
fn message_at(index: usize, channel: &str) -> Message {
    Message {
        id: format!("m_{index}"),
        // `Uuid::from_u128` rather than a random id: nothing in production
        // generates ids yet (`AGENTS.md` section 7.4 puts that in `state/` and
        // `network/`), and a deterministic id makes a failed run reproducible.
        client_msg_id: Uuid::from_u128(index as u128 + 1),
        channel_id: channel.to_owned(),
        user_id: PEER.to_owned(),
        content: BODIES[index % BODIES.len()].to_owned(),
        timestamp: at(index),
        edited_at: None,
        reactions: SmallVec::new(),
        thread_id: None,
        attachments: SmallVec::new(),
    }
}

/// The window this bench opens: centred, windowed, [`WINDOW_WIDTH`] x
/// [`WINDOW_HEIGHT`], in either mode.
///
/// **The bench's own options rather than `app::open`'s**, which are private so
/// that there is one answer to "how big is this window" (`bridge.rs` section 5).
/// The two are kept equal by the compile-time assertion above.
fn bench_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Prints the usage, naming what each mode measures.
///
/// **The modes are described by the root view they put in the window**, read
/// back from gpui's own name for it, because that is the fact a reader has to be
/// able to check before quoting anything this bench prints. The soak names the
/// same root as app mode on purpose: it is the same window, used differently, and
/// a help text that implied otherwise would be selling a difference the run does
/// not have.
fn print_help() {
    println!("================================================================");
    println!(" Sh_Nexus frame-time bench -- usage");
    println!();
    println!("   cargo bench --bench frame_time --features profiling");
    println!("   cargo bench --bench frame_time --features profiling -- --mode list");
    println!("   cargo bench --bench frame_time --features profiling -- --mode app");
    println!("   cargo bench --bench frame_time --features profiling -- --mode soak");
    println!("   cargo bench --bench frame_time --features profiling -- --soak-minutes 2");
    println!();
    println!(
        "   --mode list   (default)  window root: {}",
        window_root_name(Mode::List)
    );
    println!("                           The production message list and nothing else.");
    println!("                           A floor: what it establishes is that the LIST");
    println!("                           is not the frame-time bottleneck. This is the");
    println!("                           figure docs/BASELINES.md already recorded.");
    println!(
        "   --mode app               window root: {}",
        window_root_name(Mode::App)
    );
    println!("                           The shipped shell, built by Shell::new and");
    println!("                           focused the way app::open focuses it. This is");
    println!("                           the window AGENTS.md 6.2's row is about, minus");
    println!("                           the parts that do not exist yet.");
    println!(
        "   --mode soak              window root: {}",
        window_root_name(Mode::Soak)
    );
    println!("                           The same shell, driven by real work: one");
    println!("                           optimistic send and one ACK per cycle, for");
    println!("                           AGENTS.md 6.2's 30-minute idle-RAM time base.");
    println!("                           Reports three levels and a slope, and publishes");
    println!("                           NO frame-time figure -- it does not scroll.");
    println!("   --soak-minutes <m>      the load phase's length in minutes; implies");
    println!("                           --mode soak. Default {SOAK_DEFAULT_MINUTES}, which is");
    println!("                           AGENTS.md 6.2's own time base. A shorter run is a");
    println!("                           smoke test, not the figure.");
    println!("   -h, --help              this text");
    println!();
    println!("   All three modes open one window and name the mode on every line they");
    println!("   print. The two frame modes run about 35 s; the soak runs its stated");
    println!("   duration. Every phase marker carries elapsed time, and the soak's");
    println!("   carries epoch seconds too, so an external RAM monitor's CSV lines up");
    println!("   with the phases without a guess.");
    println!();
    println!("   Exit codes: 0 report printed | 1 measurement failed | 2 bad command line");
    println!("================================================================");
}

/// gpui's own name for the root view type a mode installs.
///
/// **Read from `std::any::type_name` rather than hard-coded here**, so the string
/// the help and the banner print cannot drift from the string the window itself
/// reports: `AnyWindowHandle::root_entity_type_name` is the same
/// `type_name::<V>()`, read back from the handle in the report. The bench's root is
/// named after this crate's bench target, `frame_time`, which is why list mode
/// reads `frame_time::BenchRoot`.
///
/// A plain function and not a `const fn` because `type_name` is not const-callable
/// on this toolchain; it is only ever needed at print time, which is where the
/// claim it makes belongs.
fn window_root_name(mode: Mode) -> &'static str {
    match mode {
        Mode::List => std::any::type_name::<BenchRoot>(),
        // **The same type name as app mode, and that is not a copy-paste: the
        // soak puts the identical root in the window.** The mode label is what
        // distinguishes the two reports, and the label says "app-level soak".
        Mode::App | Mode::Soak => std::any::type_name::<Shell>(),
    }
}

/// Prints the banner an external monitor needs before the run starts.
///
/// The mode and the window's root view come first, because a number with no window
/// in front of it is the failure this file exists to prevent. The pid and the
/// executable path follow for the same reason `AGENTS.md` section 6.2's RAM rows
/// are sampled with the OS's own tooling: a tool pointed at the wrong process
/// publishes a figure about that process.
fn print_banner(mode: Mode) {
    let executable = std::env::current_exe().map_or_else(
        |error| format!("<unavailable: {error}>"),
        |path| path.display().to_string(),
    );

    println!("================================================================");
    println!(" Sh_Nexus frame-time bench -- AGENTS.md 6.2, ADR-006 step 6");
    println!(
        " mode: {} -- window root view: {}",
        mode.label(),
        window_root_name(mode)
    );
    println!(" pid: {} | exe: {executable}", std::process::id());
    println!(
        " window: {WINDOW_WIDTH}x{WINDOW_HEIGHT} logical px | messages: \
         {TOTAL_MESSAGES} | channel: {}",
        mode.channel()
    );
    if mode.is_soak() {
        // The threshold line is deliberately absent for a soak, and so is any
        // suggestion that there is one to judge: 6.2's `< 8 ms` row is a scroll
        // measurement, this run scrolls nothing, and a banner that named the
        // threshold would put it in the reader's head for a report that never
        // produces the figure it belongs to.
        println!(
            " NO FRAME-TIME THRESHOLD: this mode measures AGENTS.md 6.2's idle-RAM time \
             base, not the < {FRAME_BUDGET_MS} ms scroll figure"
        );
        println!(
            " phases: connect -> seed -> {SOAK_DWELL:?} baseline -> send/ACK cycle for the \
             stated duration (default {SOAK_DEFAULT_MINUTES} min; --soak-minutes overrides) \
             -> {SOAK_DWELL:?} after -> report"
        );
        println!(" every phase marker carries epoch seconds as well as elapsed, so an");
        println!(" external monitor's t,ws CSV can be sliced on them without guessing when");
        println!(" the run started");
    } else {
        println!(
            " threshold: draw_duration p99 < {FRAME_BUDGET_MS} ms, measured over a \
             {SCROLL_DURATION:?} scroll"
        );
        println!(
            " phases: {IDLE_DWELL:?} idle empty -> seed -> {SEEDED_DWELL:?} idle loaded \
             -> scroll -> report"
        );
    }
    println!(" RAM: sample THIS pid from an external OS monitor during the idle dwells");
    println!("================================================================");
}

/// Prints a phase marker with the elapsed time.
///
/// The markers double as the RAM sampler's schedule: they say what state the
/// process is in, how long it will stay there, and -- in app mode -- what is still
/// ticking, which is what an external monitor needs to know before it polls a
/// working set.
fn announce(started: Instant, message: &str) {
    // `AGENTS.md` section 7.1 bans `println!` in `src/`. This file is a bench
    // target outside `src/`, and the printed report is the entire deliverable
    // -- see the module docs.
    println!("[{:>6.1}s] {message}", started.elapsed().as_secs_f64());
}

/// Formats nanoseconds as milliseconds with three decimals.
fn ms(nanoseconds: u64) -> f64 {
    // `as`, not `From`: `f64: From<u64>` does not exist, and the lossy
    // conversion is the point -- a nanosecond count rendered as milliseconds is a
    // display concern, and every count here is far below the 2^53 where a
    // `u64 as f64` stops being exact.
    nanoseconds as f64 / 1_000_000.0
}

/// Prints one line of the per-histogram table.
///
/// A histogram with no samples prints **no percentile at all** rather than a row
/// of zeros: `Stats::EMPTY`'s fields are zero, and a `0.000 ms` that means
/// "nothing was measured" is indistinguishable from a `0.000 ms` that means
/// "every frame was instantaneous". The reader must be able to tell the two apart
/// without trusting the bench.
fn print_stats(name: &str, stats: Stats) {
    if stats.samples == 0 {
        println!("  {name:<18} samples=0     -- nothing recorded, so no percentile is reported");
        return;
    }

    println!(
        "  {name:<18} samples={:<6} p50={:>8.3}ms p95={:>8.3}ms p99={:>8.3}ms \
         max={:>9.3}ms",
        stats.samples,
        ms(stats.p50_ns),
        ms(stats.p95_ns),
        ms(stats.p99_ns),
        ms(stats.max_ns),
    );
}

/// Prints the final report and says whether the measurement is usable.
///
/// # Arguments
///
/// * `run`      - The run being reported, for the mode, the channel, the window
///   and the snapshots.
/// * `rendered` - How many messages the list held when scrolling began, read back
///   from the view rather than assumed from the seeding loop. The *when* of each
///   snapshot is on the snapshot itself ([`Report::at`]), taken at capture time
///   rather than looked up here.
///
/// # Errors
///
/// Returns an error -- which the caller turns into a non-zero exit -- when the run
/// produced no report, or when the compared-against histogram held zero samples: a
/// bench that printed a percentile of nothing would publish a false figure, and a
/// false figure is the one outcome this work unit must not produce.
fn print_report(run: &Run, rendered: usize) -> Result<(), String> {
    let reports = run.probe.borrow();
    let before = reports
        .reports
        .iter()
        .find(|report| report.label == "before-scroll")
        .ok_or_else(|| "the pre-scroll snapshot was never taken".to_owned())?;
    let after = reports
        .reports
        .iter()
        .find(|report| report.label == "after-scroll")
        .ok_or_else(|| "the post-scroll snapshot was never taken".to_owned())?;

    if after.draw.samples == 0 {
        return Err(
            "the window recorded no draw_duration samples, so there is no frame time \
             to report; was it ever drawn?"
                .to_owned(),
        );
    }

    let scroll_draw_samples = after.draw.samples.saturating_sub(before.draw.samples);
    let scroll_share = 100.0 * scroll_draw_samples as f64 / after.draw.samples as f64;
    let draw_p99_ms = ms(after.draw.p99_ns);
    let dirty_p99_ms = ms(after.dirty_to_present.p99_ns);
    let within_budget = draw_p99_ms < FRAME_BUDGET_MS;

    println!();
    println!("================================================================");
    println!(" Sh_Nexus frame-time bench -- report");
    println!(
        " mode: {} | window root view (as the window itself names it): {}",
        run.mode.label(),
        run.window.root_entity_type_name()
    );
    println!(
        " window: {WINDOW_WIDTH}x{WINDOW_HEIGHT} logical px | messages: {rendered} | \
         channel: {}",
        run.channel()
    );
    println!(
        " captured: before-scroll at +{:.1}s -> after-scroll at +{:.1}s",
        before.at.as_secs_f64(),
        after.at.as_secs_f64()
    );
    println!("----------------------------------------------------------------");
    println!(" BEFORE scrolling (every frame the window had drawn until then)");
    print_stats("draw_duration", before.draw);
    print_stats("dirty_to_present", before.dirty_to_present);
    println!(" AFTER scrolling (cumulative: gpui has no histogram reset)");
    print_stats("draw_duration", after.draw);
    print_stats("dirty_to_present", after.dirty_to_present);
    println!(
        " scroll-phase frames: {scroll_draw_samples} of {} ({scroll_share:.1}% of the \
         samples)",
        after.draw.samples
    );
    println!("----------------------------------------------------------------");
    println!(" AGENTS.md 6.2 -- scroll frame time at 10k messages, < {FRAME_BUDGET_MS} ms");
    println!("   statistic: p99 (docs/BASELINES.md: a mean hides the janky frames)");
    println!(
        "   compared against: draw_duration p99 = {draw_p99_ms:.3} ms  -> {}",
        if within_budget {
            "WITHIN BUDGET"
        } else {
            "OVER BUDGET"
        }
    );
    // The verdict is read from `draw_duration`, which gpui records with no
    // visibility filter, so a window that lost focus mid-run would otherwise
    // publish its frames as if they had been measured in the foreground.
    // `Window::is_visible` is public precisely so this can be checked; a run that
    // trips it is reported as compromised rather than as a passing measurement.
    if !before.visible || !after.visible {
        println!(
            "   WARNING: window visibility was {} at capture time -- the figure above",
            if before.visible && !after.visible {
                "lost DURING the run"
            } else {
                "not visible"
            }
        );
        println!("   is NOT trustworthy. Leave the window in the foreground for a whole");
        println!("   run and re-run before quoting it.");
    }
    for line in run.mode.scope() {
        println!("   {line}");
    }
    println!("   still owed in either mode: app.rs section 5 records that the channel");
    println!("   rail and the input bar are not constructible yet, so the application");
    println!("   window is this shell and no more.");
    println!("   what these two dwells do NOT measure is 6.2's RAM time base: a level,");
    println!("   not a trend, because 6 s and 8 s cannot show growth. `--mode soak` is");
    println!("   that measurement -- a send and its ACK per cycle for 30 minutes -- and");
    println!("   it reports three levels and a slope rather than a frame percentile.");
    println!("   why draw_duration: it is the time GPUI spends building the frame,");
    println!("   which is the half this client controls -- the one that grows if the");
    println!("   list stops being virtualized. dirty_to_present additionally contains");
    println!("   the wait for the next present (up to 16.7 ms at 60 Hz), so it is an");
    println!("   end-to-end latency figure and cannot be bounded by 8 ms by any");
    println!("   amount of rendering optimization.");
    // Reported the same way `print_stats` reports it, and for the same reason
    // that function documents: a `0.000 ms` standing for "nothing was measured"
    // is indistinguishable from a `0.000 ms` standing for "instantaneous", and a
    // reader comparing the two lines above would otherwise find one of them
    // saying "no samples" and this one quietly saying zero.
    if after.dirty_to_present.samples == 0 {
        println!("   dirty_to_present: NOT MEASURED (0 samples survived");
        println!("   journal::frame_sample_is_valid, which drops frames across a power");
        println!("   change or a visibility change). See the module docs. Not a zero,");
        println!("   and not a figure section 6.2 bounds anyway.");
    } else {
        println!("   Reported for completeness: dirty_to_present p99 = {dirty_p99_ms:.3} ms");
    }
    println!("----------------------------------------------------------------");
    println!(
        " RAM: sample this process (pid {}) from an external OS monitor during the",
        std::process::id()
    );
    println!(" two idle phases announced above -- AGENTS.md 6.2's tool column, and the");
    println!(" reason this bench adds no dependency to read it.");
    println!("================================================================");

    Ok(())
}

/// Prints the soak's report: the three figures, the slope, and the honest limit.
///
/// **No frame-time verdict appears here, and its absence is the first thing
/// printed under the header rather than a footnote.** §6.2's `< 8 ms` row is a
/// scroll measurement; this run scrolls nothing, and a percentile printed
/// beside a RAM figure is a percentile somebody will quote. What it does print
/// from the histograms is the *count* of frames drawn, because that is the
/// evidence the cycle's own waits were satisfied by a live renderer.
///
/// # Errors
///
/// With whatever the run could not establish, so a partial soak fails as loudly
/// as a missing one rather than printing a table with holes in it.
fn print_soak_report(soak: &Soak<'_>) -> Result<(), String> {
    let Some(baseline_frame) = soak.report("soak-baseline") else {
        return Err("the baseline frame snapshot was never taken".to_owned());
    };
    let Some(after_frame) = soak.report("soak-after") else {
        return Err("the closing frame snapshot was never taken".to_owned());
    };
    if after_frame.draw.samples < baseline_frame.draw.samples {
        return Err(format!(
            "the window reported fewer drawn frames at the end ({}) than at the \
             baseline ({}), so the run's own waits cannot have been satisfied by a \
             live renderer and nothing below is trustworthy",
            after_frame.draw.samples, baseline_frame.draw.samples
        ));
    }

    let iterations = soak.iterations;
    let minutes = soak.elapsed.as_secs_f64() / 60.0;
    let frames = after_frame.draw.samples - baseline_frame.draw.samples;

    println!();
    println!("================================================================");
    println!(" Sh_Nexus frame-time bench -- SOAK report (AGENTS.md 6.2 idle-RAM time base)");
    println!(
        " mode: {} | window root view (as the window itself names it): {}",
        soak.run.mode.label(),
        soak.run.window.root_entity_type_name()
    );
    println!(
        " window: {WINDOW_WIDTH}x{WINDOW_HEIGHT} logical px | channel: {} | seeded: \
         {TOTAL_MESSAGES} | pid: {}",
        soak.run.channel(),
        std::process::id()
    );
    println!("----------------------------------------------------------------");
    println!(" NO FRAME-TIME VERDICT IS PRINTED BY THIS RUN, and none should be read");
    println!("   into it. AGENTS.md 6.2's `< 8 ms` row is a SCROLL measurement: 16 s of");
    println!("   continuous scrolling, p99 of draw_duration, bracketed by two snapshots");
    println!("   because gpui's histograms are cumulative. This run scrolls nothing. Its");
    println!("   purpose is the RAM time base. `--mode list` and `--mode app` are where");
    println!("   the frame figure comes from, and docs/BASELINES.md records it.");
    println!("----------------------------------------------------------------");
    println!(" THE CYCLE, and how long it ran");
    println!(
        "   iterations: {iterations} in {:.1}s ({:.2} min), one send + one ACK each",
        soak.elapsed.as_secs_f64(),
        minutes
    );
    println!("   per iteration: MessageList::begin_send through Shell::list() -- the same");
    println!("   door InputBar::send uses -- then DomainEvent::MessageAcked through the");
    println!("   sender the shell retains. client_msg_id is Uuid::new_v4() per iteration;");
    println!("   a reused one would be refused and the cycle would stop doing work.");
    println!("   each iteration waited for the RENDERER, twice: the retained row's spec");
    println!("   reaching Pending, then Acked. Only a frame builds or updates a row, so");
    println!("   that is drain -> notify -> frame, and not a state read that would have");
    println!("   passed one step earlier.");
    println!(
        "   frames the window drew across the run: {frames} ({:.1} per iteration)",
        if iterations == 0 {
            0.0
        } else {
            frames as f64 / iterations as f64
        }
    );
    if iterations > 0 {
        println!(
            "   cadence: {:.1} cycles/minute. That is the shell's own 50 ms drain pump, \
             which is what a cycle waits for -- NOT a person's typing rate, which is \
             orders of magnitude slower. So the per-cycle costs below are the \
             transferable figures and the 30-minute total is a stress bound on them.",
            iterations as f64 / minutes.max(f64::MIN_POSITIVE)
        );
    }
    if !baseline_frame.visible || !after_frame.visible {
        println!(
            "   WARNING: window visibility was {} at snapshot time. The cycle's waits",
            if baseline_frame.visible && !after_frame.visible {
                "lost DURING the run"
            } else {
                "not visible"
            }
        );
        println!("   still completed, so frames were drawn -- but a window that was not in");
        println!("   the foreground is not the client a user is sitting in front of, and");
        println!("   the working set beside it is the working set of a backgrounded one.");
    }
    println!("----------------------------------------------------------------");
    println!(" THE THREE FIGURES -- the client's own structures, through the public seam");
    println!(
        "   {:<32} {:>12} {:>12} {:>12}   slope, baseline -> after",
        "what is counted", "baseline", "under load", "after"
    );
    // The label and the field it reads travel together as one pair, so a row
    // cannot end up labelled "unread elements" over the row count. That is a
    // plausible and completely wrong report, and it is the failure this shape
    // removes rather than the failure a `match` in the loop would have.
    let rows: [CensusRow; 7] = [
        ("messages held in the channel", |c| c.held as u64),
        ("rows the list is showing", |c| c.item_count as u64),
        ("rows the row cache retains", |c| c.retained_rows as u64),
        ("rendered-segment entries", |c| c.segments as u64),
        ("rendered-segment declared bytes", |c| c.segment_bytes),
        ("unread elements", |c| u64::from(c.unread)),
        ("sends awaiting an answer", |c| c.outstanding as u64),
    ];
    for (label, read) in rows {
        let (baseline, peak, after) = (read(&soak.baseline), read(&soak.peak), read(&soak.after));
        let delta = after as f64 - baseline as f64;
        let per_iteration = if iterations == 0 {
            0.0
        } else {
            delta / iterations as f64
        };
        let per_minute = if minutes == 0.0 { 0.0 } else { delta / minutes };
        println!(
            "   {label:<32} {baseline:>12} {peak:>12} {after:>12}   {delta:>+7.0} \
             {per_iteration:>+8.4}/iter {per_minute:>+8.2}/min"
        );
    }
    println!(
        "   {:<32} {:>12} {:>12} {:>12}",
        "sampled sends still tracked", soak.baseline.tracked, soak.peak.tracked, soak.after.tracked
    );
    println!("   the row above is a sample of this run's own sends, not a count of the whole");
    println!("   delivery map: see SOAK_PROBE_EVERY. It RISES under load and FALLS BACK after,");
    println!("   and that shape is the point: evict_one_over_cap retires an ACKNOWLEDGED send's");
    println!("   entry together with the row it evicts, so the map's bound is structural --");
    println!("   entries can never outnumber held rows plus sends still in flight.");
    println!("   A run where `after` is 0 and `under load` was 64 is that fix holding under a");
    println!("   real load. A run where `after` equals `under load` is the LEAK SHAPE: the entry");
    println!("   is outliving its row, and that is the regression this row exists to catch.");
    println!("   A Pending or Failed send is NOT retired by eviction and is not supposed to be --");
    println!(
        "   the user is still looking at that row and can still retry it. discard_failed_send"
    );
    println!("   is its retirement path, so a failed send can also never accumulate here.");
    println!("   HOW TO READ THE THREE COLUMNS: `after` == `under load` means the structure");
    println!("   settled; `after` < `under load` means it came back down, which is what a");
    println!("   cache does; `after` > `under load` is the leak shape. The slope column is");
    println!("   baseline -> after, so a bounded cache shows a one-off fill as a non-zero");
    println!("   slope and nothing after that -- which is why the fill is in the peak");
    println!("   column and the slope is not the thing to read on its own.");
    println!("----------------------------------------------------------------");
    println!(" THE {} CAP", MAX_MESSAGES_PER_CHANNEL);
    match soak.evicted_at {
        Some(iteration) => println!(
            "   the channel was seeded to exactly MAX_MESSAGES_PER_CHANNEL, so the first \
             send is one over the bound. It bit: the oldest seeded message \
             (client_msg_id {}) was no longer held after iteration {iteration}.",
            Uuid::from_u128(SEEDED_HEAD)
        ),
        None => println!(
            "   the oldest seeded message (client_msg_id {}) was still held when the run \
             ended, so the cap never bit. Nothing evicted, and the 'after' figure below \
             is a cache that filled rather than a history that slid.",
            Uuid::from_u128(SEEDED_HEAD)
        ),
    }
    println!(
        "   highest held count observed: {}, read on every cycle while a send was still \
         outstanding (cap is {}). That is cap+1, which is the first eviction; a value of \
         cap+2 would mean an insert evicted nothing and the channel is over its bound by \
         a row it cannot give back.",
        soak.max_held, MAX_MESSAGES_PER_CHANNEL
    );
    println!("   eviction skips a row this client has an outstanding send for, and skips");
    println!("   to the next oldest rather than stopping short. This cycle acknowledges");
    println!("   each send before making the next, so at most one row was ever outstanding");
    println!("   -- and it is the newest, which eviction never reaches. The documented");
    println!("   over-cap case (every row a send in flight) therefore did not arise here,");
    println!("   which is a property of this workload and NOT evidence that the case is");
    println!("   harmless: a client that queued many sends before any ACK would meet it.");
    println!("----------------------------------------------------------------");
    println!(" WHAT THIS DOES NOT MEASURE, printed here so it cannot be quoted past");
    println!("   THERE IS NO SERVER. Every ACK in this run was injected by this bench, so");
    println!("   the timeline is not a real network's and the absolute figures are not a");
    println!("   real session's. What this run can measure is whether the CLIENT'S OWN");
    println!("   structures grow under sustained use: the held history, the outgoing and");
    println!("   failures maps, the rendered-segment LRU, the row cache, the unread set.");
    println!("   What it cannot measure is a server-driven leak, a reconnection storm, a");
    println!("   resync, or anything network/ will do in Phase 4 -- there is no network/");
    println!("   in this build, and none of this says whether one would be bounded.");
    println!("   A figure that does not say what it does not cover is a number waiting to");
    println!("   be quoted as more than it is.");
    println!("----------------------------------------------------------------");
    println!(" RAM: sample THIS pid from an external OS monitor, on the three windows above.");
    println!("   They are printed with epoch seconds as well as elapsed, so a monitor's");
    println!("   t,ws CSV can be sliced on them without guessing when the run started.");
    println!("   That CSV -- not this table -- is where AGENTS.md 6.2's working-set row");
    println!("   lives; this bench adds no dependency to read its own memory.");
    println!("================================================================");

    Ok(())
}

// ---------------------------------------------------------------------------
// The phases
// ---------------------------------------------------------------------------

/// Waits until the list is holding at least `expected` messages, or gives up.
///
/// **This is how app mode learns that the shell's pump did its work**, and the
/// choice of signal is the point: `MessageList::sync` runs at the top of the list's
/// own `render`, so the count reaching a batch means the pump drained, the pump
/// notified, and a frame drew. Waiting on the state instead would prove less --
/// the events could be applied and the list still be showing the previous count --
/// and waiting on the pump's `DrainReport` is not available, because the report
/// stays inside the shell.
///
/// The app mode's own frame request is deliberately absent: the pump notifies the
/// list, and that notification is the application's frame request. Adding one from
/// the bench would be the second scheduler this mode exists to avoid.
///
/// # Errors
///
/// With the count that was reached and the arithmetic of the patience spent, so the
/// message distinguishes "slow" from "never arrived" without the reader having to
/// re-derive either.
async fn wait_for_drawn(run: &Run, expected: usize, cx: &mut AsyncApp) -> Result<(), String> {
    for _ in 0..PUMP_POLLS {
        if run.rendered(cx) >= expected {
            return Ok(());
        }
        cx.background_executor().timer(PUMP_POLL).await;
    }

    Err(format!(
        "the list drew {} of the {expected} messages in this batch after {} x \
         {}ms of patience -- {} of the shell's 50 ms drain ticks. Nothing else \
         could have applied them: this mode does not call bridge::drain, because \
         the shell's pump owns the inbox. A refused event would land here too, \
         since a refusal never reaches the state.",
        run.rendered(cx),
        PUMP_POLLS,
        PUMP_POLL.as_millis(),
        PUMP_POLLS as u128 * PUMP_POLL.as_millis() / 50,
    ))
}

/// Delivers one batch of fixtures and returns once the renderer has drawn them.
///
/// The batch size is [`bridge::MAX_PENDING_EVENTS`] rather than a literal: the
/// inbox is a bounded `sync_channel` and a full one *refuses* delivery
/// (`AGENTS.md` section 7.1 forbids unbounded growth), so "10 000 events at once"
/// is not a thing this run can do -- it must deliver, drain, and repeat, and the
/// bound it must respect is the one the bridge publishes.
///
/// # Errors
///
/// A refusal from the inbox, a refused event from the state (list mode, where the
/// bench can see the `DrainReport`), or a batch the renderer never drew (app mode).
async fn deliver_batch(
    run: &Run,
    channel: &str,
    batch: Range<usize>,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    for index in batch.clone() {
        // `Display`, not `Debug`: a refusal owns the event it could not queue, and
        // `AGENTS.md` section 7.5 forbids logging content. The `Display` impl is
        // documented to name the outcome and nothing else.
        match run
            .sender
            .deliver(DomainEvent::MessageReceived(message_at(index, channel)))
        {
            Delivery::Queued => {}
            refusal => {
                return Err(format!(
                    "message {index} was refused by the inbox ({refusal}); the batch \
                     must stay within bridge::MAX_PENDING_EVENTS"
                ));
            }
        }
    }

    match &run.root {
        // List mode: this bench owns the schedule, because there is no shell and no
        // pump to defer to. The frame request is the caller's job --
        // `bridge::drain` applies events and schedules no redraw
        // (`MessageList::sync` documentation says so) -- and without it the ten
        // batches would be handed to a renderer that never saw nine of them
        // arrive, and the "loaded" state would be a state that was never rendered
        // as it grew.
        Root::Bench(_) => {
            let report = cx.update(bridge::drain).ok_or_else(|| {
                "the bridge was not installed before the window opened".to_owned()
            })?;
            if report.refused() != 0 {
                return Err(format!(
                    "the state refused {} of the messages it was handed ({report:?})",
                    report.refused()
                ));
            }
            run.invalidate(cx);
            cx.background_executor().timer(SEED_TICK).await;
            Ok(())
        }
        // App mode: the shell's pump owns the inbox, so this arm delivers and
        // waits. See the module docs.
        Root::Shell(_) => wait_for_drawn(run, batch.end, cx).await,
    }
}

/// Delivers [`TOTAL_MESSAGES`] messages through the production bridge, in batches
/// no larger than the inbox, and waits for each one to be drawn.
async fn seed_messages(run: &Run, cx: &mut AsyncApp) -> Result<(), String> {
    let channel = run.channel();
    let mut seeded = 0usize;
    while seeded < TOTAL_MESSAGES {
        let batch_end = (seeded + bridge::MAX_PENDING_EVENTS).min(TOTAL_MESSAGES);
        deliver_batch(run, channel, seeded..batch_end, cx).await?;
        seeded = batch_end;
    }

    announce(
        run.started,
        &format!(
            "seeding complete: {seeded} messages in {channel} through \
             bridge::deliver -- {}",
            run.mode.seed_note()
        ),
    );
    Ok(())
}

/// Scrolls the list continuously, up for half the phase and down for the rest.
///
/// The direction flips rather than running one way forever so the list never
/// parks: a list clamped against the top of the history still redraws when it is
/// notified, but it redraws *static content*, and a run whose second half measured
/// no movement would be publishing a number about the wrong thing. Scrolling up
/// also stops tail-following (gpui's `scroll_by` does it for a negative distance),
/// which is what makes the list traverse rows the tail would never show.
///
/// # Errors
///
/// Never returns one today, but the signature is kept uniform with the other
/// phases so `drive` reads as one sequence of fallible steps rather than a mix.
async fn scroll_phase(run: &Run, cx: &mut AsyncApp) -> Result<(), String> {
    let began = Instant::now();
    let half = SCROLL_DURATION / 2;
    while began.elapsed() < SCROLL_DURATION {
        let step = if began.elapsed() < half {
            -SCROLL_STEP_PX
        } else {
            SCROLL_STEP_PX
        };
        run.scroll(step, cx);
        cx.background_executor().timer(SCROLL_TICK).await;
    }
    Ok(())
}

/// Asks the window for a snapshot and waits until it has taken one.
///
/// The wait is a poll because the only place a snapshot is taken is a frame
/// callback, and nothing else signals that one has run: the report appearing in
/// the probe is the whole acknowledgement.
///
/// # Errors
///
/// The window never produced a frame, which is a window that is not drawing at
/// all. Waiting longer would only turn a broken measurement into a slow one.
async fn capture(run: &Run, cx: &mut AsyncApp, label: &'static str) -> Result<(), String> {
    run.arm_capture(cx, label)?;

    for _ in 0..CAPTURE_POLLS {
        if run
            .probe
            .borrow()
            .reports
            .iter()
            .any(|report| report.label == label)
        {
            return Ok(());
        }
        cx.background_executor().timer(CAPTURE_POLL).await;
    }
    Err(format!(
        "the window never rendered after `{label}` was requested"
    ))
}

/// Runs the four phases in order and prints the report.
///
/// # Errors
///
/// Any phase that cannot complete returns the reason instead of panicking, so a
/// broken run fails with a message on stderr and a non-zero exit rather than with
/// a stack trace and no diagnosis (`AGENTS.md` section 2.1).
async fn drive(run: &Run, cx: &mut AsyncApp) -> Result<(), String> {
    // Phase 1 -- idle, empty. In list mode nothing is requested during the dwell, so
    // the window draws no frames; in app mode the shell's pump ticks and finds an
    // empty inbox, which applies nothing and asks for no frame. Either way the
    // process is at rest for the RAM sample, and the announcement says which
    // mechanism is running so the reader can judge "at rest" rather than assume it.
    run.open_channel(cx)?;
    announce(
        run.started,
        &format!(
            "phase 1/4 idle empty ({}): channel {}, no history, dwelling {IDLE_DWELL:?} \
             -- {}; an external monitor can sample this pid now",
            run.mode.label(),
            run.channel(),
            run.mode.idle_note(),
        ),
    );
    cx.background_executor().timer(IDLE_DWELL).await;

    // Phase 2 -- load, then idle again at the loaded state.
    seed_messages(run, cx).await?;
    announce(
        run.started,
        &format!(
            "phase 2/4 idle loaded: dwelling {SEEDED_DWELL:?} at rest -- an external \
             monitor can sample this pid now"
        ),
    );
    cx.background_executor().timer(SEEDED_DWELL).await;

    let rendered = run.rendered(cx);
    if rendered != TOTAL_MESSAGES {
        return Err(format!(
            "the list holds {rendered} of the {TOTAL_MESSAGES} seeded messages, so the \
             figure that follows would be measured against the wrong history"
        ));
    }

    // Phase 3 -- the measurement itself. Both snapshots bracket the scroll; see the
    // module docs for why the histograms need bracketing at all.
    capture(run, cx, "before-scroll").await?;
    announce(
        run.started,
        &format!(
            "phase 3/4 scrolling for {SCROLL_DURATION:?} in {} mode",
            run.mode.label()
        ),
    );
    scroll_phase(run, cx).await?;
    capture(run, cx, "after-scroll").await?;

    // Phase 4 -- the deliverable.
    print_report(run, rendered)?;
    announce(run.started, "phase 4/4 report printed; quitting");
    Ok(())
}

// ---------------------------------------------------------------------------
// The soak's phases
// ---------------------------------------------------------------------------

/// What a soak run accumulates while it is going.
///
/// **Its own bookkeeping, held apart from [`Run`]** because it is the only part
/// of this file that grows: `Run` is one window and a probe, and this is the
/// list of the run's own figures, the identities it chose to remember, and the
/// two numbers that date the history bound's first eviction.
struct Soak<'a> {
    /// The window and the probe, borrowed from the caller.
    run: &'a Run,
    /// The list, reached once so the loop holds a handle rather than a borrow
    /// that cannot survive an `await`.
    list: Entity<MessageList>,
    /// How long the load phase actually ran.
    elapsed: Duration,
    /// How many send/ACK cycles completed.
    iterations: usize,
    /// The census at the baseline dwell, before the first send.
    baseline: Census,
    /// The largest census seen across the load window.
    peak: Census,
    /// The census at the closing dwell, once the sends stopped.
    after: Census,
    /// The identities this run chose to keep re-reading. Bounded by
    /// [`SOAK_PROBE_EVERY`], and bounded for the reason that constant gives.
    probes: Vec<Uuid>,
    /// The iteration at which the oldest seeded message stopped being held.
    evicted_at: Option<usize>,
    /// The highest message count the channel was ever read at.
    max_held: usize,
}

impl Soak<'_> {
    /// The frame snapshot this run took under `label`, if it took one.
    ///
    /// **The first match rather than the last, and returned by value.** Each
    /// label is armed once, so a second match would be a bug the count would
    /// hide; and a `&Report` out of a `RefCell` borrow would be a reference to a
    /// temporary, which is a lifetime the report does not need — `Report` is
    /// five `Copy` fields and a static label, so a copy is cheaper than a lease.
    fn report(&self, label: &str) -> Option<Report> {
        self.run
            .probe
            .borrow()
            .reports
            .iter()
            .find(|report| report.label == label)
            .cloned()
    }
}

/// Seconds since the Unix epoch, for aligning an external monitor's CSV.
///
/// **`SystemTime` and not `Utc::now()`, for one reason: this is a wall-clock
/// reading for a sampler to line up against, not a timestamp in a message.**
/// The bench's own fixtures use a fixed epoch because ordering must not depend
/// on when a run happened; the opposite is true here, and the two are different
/// questions so they are different functions.
///
/// `0.0` is the honest "the clock is before 1970", which is not a thing that
/// happens on any platform this builds for and is printed rather than hidden
/// so that a reader who sees it knows the alignment below cannot be trusted.
fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Prints one soak phase marker, with the elapsed time and the epoch.
///
/// **Both clocks on every marker, and that is the whole reason this is not
/// [`announce`].** The elapsed column is for a human reading the log; the epoch
/// column is what an external sampler needs to slice its `t,ws` CSV onto the
/// phase it was actually sampling, because the sampler's `t` and this bench's
/// `Instant` share nothing. Without the epoch a reader has to guess when the run
/// started relative to the monitor, and a guess here silently mislabels the
/// baseline.
fn announce_window(started: Instant, message: &str) {
    println!(
        "[{:>7.1}s | epoch {:.3}] {message}",
        started.elapsed().as_secs_f64(),
        epoch_seconds()
    );
}

/// Waits until the row for `client_msg_id` is drawing `expected`.
///
/// **The soak's only wait, and it is on the renderer — see
/// [`Run::drawn_delivery`] for why that distinction is the measurement's
/// correctness rather than a nicety.** Two of these bracket every cycle: one for
/// the optimistic row, one for the reconciliation. A cycle that cannot satisfy
/// them is a cycle whose waits are not being met, and continuing would turn a
/// broken run into a quiet one.
///
/// # Errors
///
/// With the patience spent and what the row was last seen drawing, so the
/// message says "the renderer stopped" rather than leaving the reader to work
/// out that sixty pump ticks is not "slow".
async fn wait_for_row_drawn(
    run: &Run,
    list: &Entity<MessageList>,
    client_msg_id: Uuid,
    expected: DeliveryState,
    iteration: usize,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    for _ in 0..SOAK_DRAWN_POLLS {
        match run.drawn_delivery(list, &client_msg_id, cx) {
            Some(drawn) if drawn == expected => return Ok(()),
            _ => cx.background_executor().timer(PUMP_POLL).await,
        }
    }
    Err(format!(
        "cycle {iteration}: the row for {client_msg_id} never drew {expected:?} in {} x \
         {}ms of patience -- {} of the shell's 50ms drain ticks. Only a frame builds or \
         updates a row, so this is a renderer that stopped drawing rather than an event \
         that was slow, and every figure after this point would be a measurement of a \
         stalled client",
        SOAK_DRAWN_POLLS,
        PUMP_POLL.as_millis(),
        SOAK_DRAWN_POLLS as u128 * PUMP_POLL.as_millis() / 50,
    ))
}

/// Puts the state in [`ConnectionState::Connected`] so a send is a network send.
///
/// **One event, and it is not optional bookkeeping: `AppState::can_send` is
/// `false` in every other state**, so a send made while disconnected comes back
/// `offline: true` and the row is the outbox's — a path `PLAN.md` §7 gives to
/// `db/` in Phase 3 and which this build does not have. Measuring that path for
/// half an hour and calling it "active chatting" would be measuring a different
/// thing from the one the row asks about.
///
/// **The wait is on the state, and the difference from the cycle's waits is the
/// point**: the question here is whether the shell's pump applied the event, and
/// no frame has anything to do with that.
///
/// # Errors
///
/// When the pump never applied it, which would mean every send in the run was
/// an outbox send.
async fn connect(run: &Run, cx: &mut AsyncApp) -> Result<(), String> {
    match run.sender.deliver(DomainEvent::ConnectionStateChanged(
        ConnectionState::Connected,
    )) {
        Delivery::Queued => {}
        refusal => {
            return Err(format!(
                "the connection event was refused by the inbox ({refusal}), so every send \
                 in this run would have been an outbox send"
            ))
        }
    }
    for _ in 0..PUMP_POLLS {
        // `false` on `None` is not a claim that the state is disconnected: it is
        // the absence of an answer, and the loop below turns an absence that
        // persists into an error rather than into a run.
        if cx.update(|app| bridge::try_read(app, |state| state.can_send()).unwrap_or(false)) {
            return Ok(());
        }
        cx.background_executor().timer(PUMP_POLL).await;
    }
    Err(format!(
        "the shell's pump did not apply `ConnectionStateChanged(Connected)` in {} x {}ms, \
         so every send in this run would have been an outbox send rather than a network \
         one",
        PUMP_POLLS,
        PUMP_POLL.as_millis()
    ))
}

/// One send and the ACK that resolves it, through the production path.
///
/// **Four steps, and every one of them is the shipped client's.** The send is
/// `MessageList::begin_send` — the same call `InputBar::send` makes on Enter,
/// so the optimistic row, the `outgoing` entry and the history insert are the
/// real ones. The ACK is a `DomainEvent::MessageAcked` through the sender the
/// shell retains, so it arrives the way a real one would: queued, drained by the
/// pump on its own schedule, and applied by production code. The two waits are
/// on the renderer, so each cycle costs at least one frame on each side and the
/// run cannot outrun the display.
///
/// # Errors
///
/// Whatever stopped the cycle, which is a failure of the run rather than of one
/// iteration: a refused send, a refused delivery, or a renderer that stopped.
async fn send_cycle(
    run: &Run,
    list: &Entity<MessageList>,
    iteration: usize,
    cx: &mut AsyncApp,
) -> Result<(Uuid, usize), String> {
    let channel = run.channel();
    // **A fresh identity every iteration, and never a counter.** A reused one
    // hits `AlreadyPending` or `AlreadyHeld`, `begin_send` refuses, and the
    // cycle stops doing work -- silently, in a run that looks calm. The
    // `on_screen` check below is what turns that into a failure.
    let client_msg_id = Uuid::new_v4();
    let content = soak_body(iteration);
    // Past every seeded timestamp, so `insert_at_order` appends and the row is
    // the newest the list can be showing. A timestamp inside the history would
    // splice the send into the middle of 10 000 rows, out of the viewport, and
    // the wait below would then be watching a row nothing draws.
    let at = at(TOTAL_MESSAGES + iteration);

    let on_screen = list.update(cx, |list, list_cx| {
        list.begin_send(&content, client_msg_id, at, list_cx)
    });
    if !on_screen {
        return Err(format!(
            "cycle {iteration}: `MessageList::begin_send` put no row on screen, so the \
             state refused the send. Carrying on would publish a quiet run: the refusals \
             that cause this (an identity already pending or already held) leave nothing \
             to grow and nothing to draw, and a reader would see a flat line and call it \
             stability"
        ));
    }

    wait_for_row_drawn(
        run,
        list,
        client_msg_id,
        DeliveryState::Pending,
        iteration,
        cx,
    )
    .await?;

    // The one-over-the-cap state, read while the send is still outstanding: the
    // insert has run, the eviction has run, and the ACK has not yet arrived.
    // This is the only moment the bound is exceeded, and reading it here rather
    // than at a census is what makes the report's "the cap bit" an observation
    // instead of a claim about `evict_one_over_cap`.
    let held_pending_ack = run.held(cx)?;

    match run.sender.deliver(DomainEvent::MessageAcked {
        client_msg_id,
        message: soak_ack(channel, client_msg_id, &content, at, iteration),
    }) {
        Delivery::Queued => {}
        refusal => {
            return Err(format!(
                "cycle {iteration}: the ACK was refused by the inbox ({refusal}); the \
                 inbox holds {} events and this bench never lets one wait",
                run.sender.capacity()
            ))
        }
    }

    wait_for_row_drawn(
        run,
        list,
        client_msg_id,
        DeliveryState::Acked,
        iteration,
        cx,
    )
    .await?;

    Ok((client_msg_id, held_pending_ack))
}

/// Runs the soak: connect, seed, measure at rest, send and acknowledge, measure
/// at rest again, report.
///
/// **The order is the experiment.** The channel is seeded to the cap *first*, so
/// all three censuses are taken against the same 10 000-message working set and
/// the difference between them is the cycle's doing rather than the seed's. The
/// baseline dwell is long enough for an external monitor to reach a settled
/// level, and the closing dwell is the same length for the same reason.
///
/// # Errors
///
/// Whatever the run could not establish, propagated rather than absorbed: a
/// soak that cannot reach a phase has not measured anything, and a report with
/// a hole in it is the one artefact this whole file argues against.
async fn drive_soak(run: &Run, minutes: f64, cx: &mut AsyncApp) -> Result<(), String> {
    let duration = Duration::from_secs_f64(minutes * 60.0);
    let list = run.list(cx)?;

    run.open_channel(cx)?;
    announce_window(
        run.started,
        &format!(
            "soak phase 1/5: channel {} is up; connecting so a send is a network send",
            run.channel()
        ),
    );
    connect(run, cx).await?;

    announce_window(
        run.started,
        &format!(
            "soak phase 2/5: seeding {TOTAL_MESSAGES} messages through the bridge -- {}",
            run.mode.seed_note()
        ),
    );
    seed_messages(run, cx).await?;
    let rendered = run.rendered(cx);
    if rendered != TOTAL_MESSAGES {
        return Err(format!(
            "the list holds {rendered} of the {TOTAL_MESSAGES} seeded messages, so the \
             three figures below would be taken against the wrong history"
        ));
    }

    // The baseline: idle, settled, at the loaded state, with the same pump
    // ticking that will tick during the load. Sampled here and again at the
    // end, and the difference between them is the figure that means something.
    let mut soak = Soak {
        run,
        list,
        elapsed: Duration::ZERO,
        iterations: 0,
        baseline: Census::default(),
        peak: Census::default(),
        after: Census::default(),
        probes: Vec::with_capacity(SOAK_PROBE_EVERY),
        evicted_at: None,
        max_held: 0,
    };

    announce_window(
        run.started,
        &format!(
            "soak phase 3/5: baseline, dwelling {SOAK_DWELL:?} at rest on a loaded \
             channel -- an external monitor can sample this pid now"
        ),
    );
    cx.background_executor().timer(SOAK_DWELL).await;
    let baseline = run.census(&soak.list, &soak.probes, cx)?;
    soak.baseline = baseline;
    soak.peak = baseline;
    soak.max_held = baseline.held;
    capture(run, cx, "soak-baseline").await?;

    announce_window(
        run.started,
        &format!(
            "soak phase 4/5: one optimistic send and one ACK per cycle, for \
             {duration:?} -- {SOAK_DEFAULT_MINUTES} min is AGENTS.md 6.2's own time base"
        ),
    );
    let began = Instant::now();
    let mut next_announce = SOAK_ANNOUNCE;
    while began.elapsed() < duration {
        let iteration = soak.iterations;
        let (client_msg_id, held_pending_ack) = send_cycle(run, &soak.list, iteration, cx).await?;
        soak.iterations += 1;
        if soak.probes.len() < SOAK_PROBE_EVERY {
            soak.probes.push(client_msg_id);
        }
        // The true maximum, taken at the one instant the channel is over its
        // bound. See `Run::held`.
        soak.max_held = soak.max_held.max(held_pending_ack);

        // The eviction witness, read rather than reasoned about: the first time
        // the oldest seeded message is no longer held is the iteration the cap
        // started biting, and saying so from a read beats saying it from a
        // reading of `evict_one_over_cap`. Checked only until it flips, because
        // after that the answer cannot change and the probe is a hash lookup
        // per cycle for nothing.
        if soak.evicted_at.is_none() {
            let census = run.census(&soak.list, &soak.probes, cx)?;
            if !census.head_held {
                soak.evicted_at = Some(soak.iterations);
            }
        }

        if soak.iterations.is_multiple_of(SOAK_CENSUS_EVERY) {
            let census = run.census(&soak.list, &soak.probes, cx)?;
            soak.peak = soak.peak.peak(&census);
        }

        let elapsed = began.elapsed();
        if elapsed >= next_announce {
            next_announce = elapsed + SOAK_ANNOUNCE;
            let held = run.census(&soak.list, &soak.probes, cx)?;
            soak.peak = soak.peak.peak(&held);
            announce_window(
                run.started,
                &format!(
                    "   {:.0}s in: {} cycles ({:.1}/min), channel holds {}, outstanding {}, \
                     sampled sends tracked {}",
                    elapsed.as_secs_f64(),
                    soak.iterations,
                    soak.iterations as f64 / (elapsed.as_secs_f64() / 60.0).max(f64::MIN_POSITIVE),
                    held.held,
                    held.outstanding,
                    held.tracked
                ),
            );
        }
    }
    soak.elapsed = began.elapsed();
    let loaded = run.census(&soak.list, &soak.probes, cx)?;
    soak.peak = soak.peak.peak(&loaded);
    soak.max_held = soak.max_held.max(loaded.held);

    announce_window(
        run.started,
        &format!(
            "soak phase 5/5: activity has stopped; dwelling {SOAK_DWELL:?} at rest again \
             -- an external monitor can sample this pid now"
        ),
    );
    cx.background_executor().timer(SOAK_DWELL).await;
    soak.after = run.census(&soak.list, &soak.probes, cx)?;
    capture(run, cx, "soak-after").await?;

    print_soak_report(&soak)?;
    announce_window(run.started, "soak report printed; quitting");
    Ok(())
}

/// Opens the window this mode measures and returns the run wired to it.
///
/// **The app arm is `app::open`'s build callback and nothing else.** It does not
/// install the state -- `main` did that, before any window existed, which is the
/// ordering `bridge::install` requires and the reason `app::open` cannot be called
/// here -- and it opens its own window, because the bench needs to know the mode,
/// the pid and the clock before the first pixel.
///
/// The focus line is `app::open`'s own, verbatim. It is not decoration: without it
/// the shell's root holds no focus, which is a state the application never
/// occupies, and this mode's whole claim is that the window is the application's.
///
/// # Errors
///
/// A message naming what failed: the window, or the read-back of the root view the
/// window was just given.
fn open_measured_window(
    mode: Mode,
    cx: &mut App,
    sender: EventSender,
    probe: Rc<RefCell<FrameProbe>>,
    started: Instant,
) -> Result<Run, String> {
    let (root, window) = match mode {
        Mode::List => {
            let options = bench_window_options(cx);
            let list = cx.new(MessageList::new);
            let entity = cx.new(|_view_cx| BenchRoot { list });
            let handle = cx
                .open_window(options, move |_window, _cx| entity.clone())
                .map_err(|error| format!("the list-only window could not be opened: {error}"))?;
            let root = handle.root(cx).map_err(|error| {
                format!("the list-only window's root view could not be read back: {error}")
            })?;
            (Root::Bench(root), AnyWindowHandle::from(handle))
        }
        // The soak shares this arm verbatim. **The same root, built by the same
        // constructor, is the point**: a soak that opened a different window
        // would be measuring a different client, and the whole claim of the mode
        // is that this is the shipped shell being used hard.
        Mode::App | Mode::Soak => {
            let options = bench_window_options(cx);
            // A clone, because the shell keeps the handle for its life -- dropping
            // it closes the inbox (`bridge.rs` section 2) -- and the seeding path
            // needs one of its own.
            let shell_sender = sender.clone();
            let handle = cx
                .open_window(options, move |window, cx| {
                    let shell = cx.new(|cx| Shell::new(shell_sender, cx));
                    // The focus line from `app::open`'s build callback, unchanged:
                    // a window whose root holds no focus is a window whose keys go
                    // nowhere (`AGENTS.md` section 5.2).
                    shell.read(cx).focus_handle(cx).focus(window, cx);
                    shell
                })
                .map_err(|error| format!("the application window could not be opened: {error}"))?;
            let root = handle.root(cx).map_err(|error| {
                format!("the application window's root view could not be read back: {error}")
            })?;
            (Root::Shell(root), AnyWindowHandle::from(handle))
        }
    };

    Ok(Run {
        mode,
        root,
        window,
        sender,
        probe,
        started,
    })
}

/// Opens one window, drives the four phases on the main thread, and exits with the
/// report's verdict.
///
/// The launch callback handed to `Application::run` is `FnOnce(&mut App)` and
/// cannot return a value, so a failure is carried out through an
/// `Rc<RefCell<Option<String>>>` -- the same shape `src/lib.rs::run` uses, for the
/// same reason (`AGENTS.md` section 2.1: no `unwrap` on a path that matters).
/// Nothing below panics; every failure becomes a message on stderr and a non-zero
/// exit.
fn main() {
    // What the run is going to *do*, decided before the window exists: the two
    // frame modes share one driver, the soak has its own, and choosing here
    // rather than inside the driver is what keeps `drive` reading as one
    // sequence of four phases.
    enum Plan {
        /// The four-phase frame-time measurement.
        Phases,
        /// The send/ACK soak, for this many minutes.
        Soak(f64),
    }

    let (plan, mode) = match parse_args() {
        Ok(Request::Help) => {
            print_help();
            return;
        }
        // One parse, one mode: the two are derived from the same request rather
        // than from two readings of the command line, so a run cannot measure
        // the window its banner named and not the one its driver drove.
        Ok(Request::Run(mode)) => (Plan::Phases, mode),
        Ok(Request::Soak { minutes }) => (Plan::Soak(minutes), Mode::Soak),
        Err(message) => {
            // Exit 2, distinct from the 1 a failed measurement uses: a bench that
            // was asked the wrong question has not answered it, and reporting that
            // as a measurement failure would put a diagnostic error where a number
            // belongs.
            eprintln!("frame_time: {message}");
            print_help();
            std::process::exit(2);
        }
    };

    let started = Instant::now();
    let failure: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let probe = Rc::new(RefCell::new(FrameProbe::default()));

    print_banner(mode);

    let failure_in_callback = Rc::clone(&failure);
    let probe_in_callback = Rc::clone(&probe);

    gpui_platform::application().run(move |cx: &mut App| {
        // The application state first, before any window exists -- the ordering
        // `src/lib.rs::run` documents, and the one `bridge::install` requires. It
        // is also why `app::open` is not called below: a second install is refused.
        let sender = match bridge::install(cx, UNSIGNED_IN_USER) {
            Ok(sender) => sender,
            Err(error) => {
                *failure_in_callback.borrow_mut() = Some(format!(
                    "the application state could not be installed: {error}"
                ));
                return;
            }
        };

        // Bound to a local so the error arm below is one line and the `Run` is a
        // value the driver task can move.
        let opened = open_measured_window(mode, cx, sender, Rc::clone(&probe_in_callback), started);
        let run = match opened {
            Ok(run) => run,
            Err(message) => {
                *failure_in_callback.borrow_mut() = Some(message);
                return;
            }
        };

        cx.activate(true);
        let failure_in_task = Rc::clone(&failure_in_callback);
        cx.spawn(async move |async_cx| {
            let outcome = match plan {
                Plan::Phases => drive(&run, async_cx).await,
                Plan::Soak(minutes) => drive_soak(&run, minutes, async_cx).await,
            };
            if let Err(message) = outcome {
                *failure_in_task.borrow_mut() = Some(message);
            }
            async_cx.update(|app| app.quit());
        })
        .detach();
    });

    if let Some(message) = failure.borrow_mut().take() {
        eprintln!("frame_time: FAILED -- {message}");
        std::process::exit(1);
    }

    // A quit that did not come from the driver -- the window closed by hand, or
    // the platform ending the loop early -- reports no failure and prints no
    // report. A bench that exits 0 without publishing numbers has published
    // nothing while looking successful, so the snapshots are counted here. Both
    // kinds of run take exactly two: the frame modes bracket their scroll, and
    // the soak brackets its load phase, so the count is the same check for both.
    let snapshots = probe.borrow().reports.len();
    if snapshots < 2 {
        eprintln!(
            "frame_time: FAILED -- the run ended after {snapshots} of the 2 snapshots \
             the report needs, so no report was printed"
        );
        std::process::exit(1);
    }
}
