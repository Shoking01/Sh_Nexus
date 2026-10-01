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
//! **What app mode still does not close, said here rather than discovered later.**
//! `app.rs`'s own module docs, section 5, record that the channel rail and the
//! input bar are not constructible today, so "the real application shell" is the
//! shell as it exists at this commit: a themed root container, the list, the drain
//! pump and the key handler. When the rail and the composer land, the app-level
//! number has to be taken again and this section has to move with it. And
//! section 6.2's RAM row asks for idle RAM *"over 30 minutes of active chatting"*,
//! which a bench whose two dwells are 6 s and 8 s cannot produce in either mode:
//! the growth half of that row stays owed, and the figures here are levels, not
//! trends.
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
//! cargo bench --bench frame_time --features profiling -- --help
//! ```
//!
//! Either way it opens one window, runs for roughly thirty-five seconds, prints a
//! report, and exits 0. Every phase change is printed with an elapsed timestamp
//! and the process id, so an external monitor can be pointed at the right pid
//! during the idle dwells.
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
//! - **The RAM row's time base is not here.** Six seconds and eight seconds are
//!   enough for a process monitor to sample a level; they cannot show a leak.
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
use sh_nexus::core::models::events::DomainEvent;
use sh_nexus::core::models::message::Message;
use sh_nexus::state::bridge::{self, Delivery, EventSender};
use sh_nexus::state::MAX_MESSAGES_PER_CHANNEL;
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
            Mode::App => app::STARTUP_CHANNEL,
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
            Mode::App => {
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
            Mode::App => {
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
        }
    }

    /// Parses the value of `--mode`.
    ///
    /// # Errors
    ///
    /// A message naming the two accepted values, because a bench that answers
    /// `unknown mode` with nothing else makes the reader go and read this file.
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "list" => Ok(Mode::List),
            "app" => Ok(Mode::App),
            other => Err(format!(
                "unknown mode `{other}`; this bench measures `list` (the message list \
                 alone) or `app` (the shipped shell)"
            )),
        }
    }
}

/// What the command line asked for.
enum Request {
    /// Print the usage and exit 0.
    Help,
    /// Open a window and measure it in this mode.
    Run(Mode),
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
/// # Errors
///
/// A message naming what was wrong. `--help` is not an error: help is the answer
/// to an unrecognised flag.
fn parse_args() -> Result<Request, String> {
    let mut mode = None;
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
                    "`--mode` needs a value: `list` or `app`. See `--help`.".to_owned()
                })?;
                mode = Some(Mode::parse(&value)?);
            }
            // `--mode=app`, accepted because a reader who types it should not have
            // to learn that this bench has an opinion about the spelling.
            other => match other.strip_prefix("--mode=") {
                Some(value) => mode = Some(Mode::parse(value)?),
                None => {
                    return Err(format!(
                        "unrecognised argument `{other}`; this bench takes no \
                         positional arguments. See `--help`."
                    ))
                }
            },
        }
    }

    // Absent means list, and the default is the mode `docs/BASELINES.md` already
    // recorded, so the command in that file still measures what it measured.
    Ok(Request::Run(mode.unwrap_or(Mode::List)))
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
/// **The two modes are described by the root view they put in the window**, read
/// back from gpui's own name for it, because that is the fact a reader has to be
/// able to check before quoting anything this bench prints.
fn print_help() {
    println!("================================================================");
    println!(" Sh_Nexus frame-time bench -- usage");
    println!();
    println!("   cargo bench --bench frame_time --features profiling");
    println!("   cargo bench --bench frame_time --features profiling -- --mode list");
    println!("   cargo bench --bench frame_time --features profiling -- --mode app");
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
    println!("   -h, --help              this text");
    println!();
    println!("   Both modes open one window, run about 35 s, print a report, and name");
    println!("   the mode on every line they print.");
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
        Mode::App => std::any::type_name::<Shell>(),
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
    println!(
        " threshold: draw_duration p99 < {FRAME_BUDGET_MS} ms, measured over a \
         {SCROLL_DURATION:?} scroll"
    );
    println!(
        " phases: {IDLE_DWELL:?} idle empty -> seed -> {SEEDED_DWELL:?} idle loaded \
         -> scroll -> report"
    );
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
    println!("   window is this shell and no more; and 6.2's RAM row asks for idle RAM");
    println!("   over 30 minutes of active chatting, which two dwells of 6 s and 8 s");
    println!("   cannot produce. This bench measures a level, not a trend.");
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
        Mode::App => {
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
    let mode = match parse_args() {
        Ok(Request::Help) => {
            print_help();
            return;
        }
        Ok(Request::Run(mode)) => mode,
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
            let outcome = drive(&run, async_cx).await;
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

    // A quit that did not come from `drive` -- the window closed by hand, or the
    // platform ending the loop early -- reports no failure and prints no report. A
    // bench that exits 0 without publishing numbers has published nothing while
    // looking successful, so the snapshots are counted here.
    let snapshots = probe.borrow().reports.len();
    if snapshots < 2 {
        eprintln!(
            "frame_time: FAILED -- the run ended after {snapshots} of the 2 snapshots \
             the report needs, so no report was printed"
        );
        std::process::exit(1);
    }
}
