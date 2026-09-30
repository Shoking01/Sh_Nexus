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
//! # Why this is a bench target and not `src/main.rs`
//!
//! `src/lib.rs::run` still opens the Phase 0 spike's `RootView`, and `app.rs`
//! (the real shell) is a later work unit in `PLAN.md` section 4. Building
//! `app.rs` in order to measure it would fold a planned feature into a
//! measurement one, and sections 6.1 and 6.3 both track the *release binary* --
//! so the instrument lives behind `required-features = ["profiling"]`, next to
//! nothing else, and reaches no product artifact.
//!
//! # How to run it
//!
//! ```text
//! cargo bench --bench frame_time --features profiling
//! ```
//!
//! It opens one window, runs for roughly thirty seconds, prints a report, and
//! exits 0. Every phase change is printed with an elapsed timestamp and the
//! process id, so an external monitor can be pointed at the right pid during
//! the idle dwells.
//!
//! # Which number is compared against `< 8 ms`, and why
//!
//! `draw_duration` is the figure compared against the threshold. It is the time
//! GPUI spends *building* the frame -- layout, paint, present submission --
//! which is the half of the frame this client controls and the one that grows
//! when the message list stops being virtualized. `dirty_to_present` is
//! reported alongside it, but it additionally contains the platform's wait for
//! the next present: on a 60 Hz display that wait alone is up to 16.7 ms, so no
//! amount of optimization can put an end-to-end latency figure under 8 ms. Both
//! are printed, both at p50/p95/p99/max, so the reader can see the difference
//! rather than take the choice of metric on trust.
//!
//! # The one measurement caveat, stated rather than hidden
//!
//! gpui's frame histograms are **cumulative for the life of the window** and
//! expose no reset: every frame ever drawn by this window is in them. So the
//! "after scrolling" snapshot also contains the frames drawn while the window
//! opened and while the 10 000 messages were seeded. The bench therefore takes
//! a **"before scrolling" snapshot** too and prints both. The difference of the
//! two sample counts is the number of frames the scroll phase actually drew, and
//! the pre-scroll count is printed as a percentage, so the dilution of the p99
//! is a number a reviewer can check rather than a claim to accept. The scroll
//! phase is sized (16 s of continuous scrolling) so that those frames are the
//! overwhelming majority of the samples. Across the runs whose full reports
//! were retained, the scroll phase drew 1 035 frames against 12 drawn before
//! scrolling, and 1 062 against 13 -- 98.9% and 98.8% -- so the pre-scroll
//! frames occupy about 1.1 to 1.2 percentile points of the distribution the
//! p99 is taken over. That is small, and the verdict does not rest on it: in
//! both runs the pre-scroll frames were the *slower* population (their own p99
//! was 9.896 ms and 3.391 ms, falling to 1.740 ms and 1.257 ms once the scroll
//! frames were added), so including them can only have pushed the
//! compared-against figure **up**, never down. An earlier draft of this note
//! called the dilution "less than a hundredth of a percentile point" from an
//! estimate of ~2 000 frames; the measurements said otherwise, and the estimate
//! was the thing that was wrong.
//!
//! # Why the profiler's runtime trace is left off
//!
//! `gpui::profiler::set_trace_enabled` exists and this bench deliberately does
//! not call it. `WindowProfiler` documents *"Aggregate histograms are always
//! populated when the `profiler` feature is compiled in"*, and
//! `draw_duration_histogram` -- the histogram this bench's verdict is read from
//! -- is one of them: `record_draw_timing` writes it unconditionally. So for
//! the one figure `AGENTS.md` section 6.2 actually bounds, the compile-time
//! `profiling` feature is the whole switch this measurement needs.
//!
//! Turning the trace on would only add `record_frame_event`, which runs
//! **after** `end_draw` has already computed `draw_duration`: work the measured
//! figure would then exclude, so the number would understate the frame that
//! actually ran.
//!
//! # Why one histogram can be empty
//!
//! `dirty_to_present_histogram` is not in that category, and this bench prints
//! it as a value only when it holds one. It is recorded through
//! `journal::frame_sample_is_valid`, which drops a frame whose window was not
//! visible at its start, or across which the system's power state changed
//! (`profiler/journal.rs`). That filter is **not** under this bench's control
//! and does not behave consistently: the same binary, with no change to any
//! code, produced 0 samples on one run and 1 043 on the next. So a zero here
//! means "no sample survived the filter", and the report prints it as
//! `NOT MEASURED` rather than as `0.000 ms`, because those two readings are
//! otherwise indistinguishable to a reader.
//!
//! Section 6.2's row is bounded by `draw_duration` either way:
//! `dirty_to_present` additionally contains the wait for the next present (up
//! to 16.7 ms at 60 Hz), making it an end-to-end latency figure that no amount
//! of rendering optimization can bring under 8 ms. What the report therefore
//! owes the reader is an honest blank, not a second verdict on a number the
//! threshold does not govern.
//!
//! # Why `println!` is here
//!
//! `AGENTS.md` section 7.1 bans `println!` in `src/`, where output would be
//! noise in a product. This file is a bench target under `benches/`, outside
//! `src/`, and printing the report *is* the deliverable: the bench's only job is
//! to put these numbers on stdout where the orchestrator can record them.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use gpui::profiler::FrameDurationSnapshot;
use gpui::{
    div, prelude::*, px, rgb, size, App, AsyncApp, Bounds, Context, Entity, Render, Window,
    WindowBounds, WindowOptions,
};
use sh_nexus::core::models::events::DomainEvent;
use sh_nexus::core::models::message::Message;
use sh_nexus::state::bridge::{self, Delivery, EventSender};
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
/// 8 ms is the frame interval of the 120 fps target section 1 leads with, so
/// the threshold is not an arbitrary round number: a frame that takes longer
/// than this cannot be produced at 120 Hz.
const FRAME_BUDGET_MS: f64 = 8.0;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The channel every seeded message belongs to.
const CHANNEL: &str = "c_bench";

/// The colleague every seeded message is attributed to.
///
/// Everything is somebody else's message on purpose: this bench measures the
/// frame cost of *rendering* history, and a split of self/peer messages would
/// change the row content without changing the question.
const PEER: &str = "u_bench_peer";

/// How many messages the scroll phase scrolls through.
///
/// `AGENTS.md` section 6.2's row is explicitly "10k messages", and
/// `state/app_state.rs` has no history bound (discovered in this work unit,
/// fixed in its own), so all 10 000 are loadable through the production path.
const TOTAL_MESSAGES: usize = 10_000;

/// Bodies the fixtures use, cycled by index.
///
/// A deliberate mix: plain text, inline markdown, and a fenced code block. The
/// code block makes some rows taller than others, which is the property that
/// made ADR-006 choose `gpui::List` over `UniformList` -- a bench whose rows
/// were all one line would never exercise the differently-sized measurement the
/// design rests on.
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
/// long enough that an external monitor gets a stable sample even at the slow
/// end of the frame loop.
const SCROLL_DURATION: Duration = Duration::from_secs(16);

/// How long to wait between scroll steps.
///
/// 8 ms is the 120 Hz frame interval; the loop effectively runs at the
/// display's refresh rate because the redraw is coalesced, so this is a *cap*
/// on how often the bench asks for a frame rather than a claim about how often
/// one is produced.
const SCROLL_TICK: Duration = Duration::from_millis(8);

/// How far to scroll per tick, in logical pixels.
///
/// ~48 px is well under one row (rows measure ~68 px for a single-line body),
/// so the list walks rather than jumps: a jump would re-measure a screenful and
/// measure something no gesture produces.
const SCROLL_STEP_PX: f32 = 48.0;

/// How long to wait between seeding batches.
///
/// Only so the frame loop can draw the growth between batches instead of
/// coalescing all ten into one frame -- the seeded state should be a state the
/// renderer actually rendered, not one it was handed at once.
const SEED_TICK: Duration = Duration::from_millis(16);

/// Window width, logical pixels.
const WINDOW_WIDTH: f32 = 1024.0;

/// Window height, logical pixels.
///
/// Chosen rather than inherited from the spike's 480x320 on purpose: rows per
/// frame -- and therefore this measurement -- scales with viewport height, and
/// a spike-sized window would render four rows and publish a number that says
/// nothing about a chat window.
const WINDOW_HEIGHT: f32 = 768.0;

// ---------------------------------------------------------------------------
// The probe: percentiles computed inside `render`, plain numbers out
// ---------------------------------------------------------------------------

/// One histogram's numbers, reduced to plain integers.
///
/// Deliberately carries no histogram: `frame_duration_snapshot` hands out a
/// `hdrhistogram::Histogram`, which gpui does not re-export, and neither that
/// type nor anything owning it is moved out of the frame it was read in.
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
    /// report treats `samples == 0` as a failed measurement, and a fabricated
    /// zero must never be printable as if it were one.
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
}

/// The hand-off between the view (which holds the only `&Window`) and the
/// driver task (which owns the run and the report).
///
/// A `Rc<RefCell<_>>` rather than a channel because both ends run on the main
/// thread: the view's `render` and the driver's awaits are never concurrent, so
/// a `Send` boundary would buy nothing and cost an allocation per capture.
struct FrameProbe {
    /// Set by the driver to ask for a snapshot on the view's next render, and
    /// taken (cleared) by the view when it honors the request.
    want: Option<&'static str>,
    /// Every snapshot taken so far, in order.
    reports: Vec<Report>,
}

/// Turns one frame-duration snapshot into plain numbers.
///
/// The histograms are read **field by field inside this function rather than
/// through a helper taking `&Histogram<u64>`**, because writing that signature
/// would mean naming `hdrhistogram::Histogram` -- a type gpui does not
/// re-export, so naming it means adding a dependency solely to spell a
/// parameter (`AGENTS.md` section 7.2 prices exactly that). The struct being
/// held *is* nameable, and reaching through its public fields needs no import.
///
/// The empty-histogram branch exists because the first render of a window can
/// precede its first `end_draw`: a snapshot taken there legitimately holds zero
/// samples, and asking a `hdrhistogram` for the p99 of nothing is not a
/// question this bench should depend on the answer to.
fn snapshot_report(
    label: &'static str,
    started: Instant,
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
    }
}

// ---------------------------------------------------------------------------
// The root view
// ---------------------------------------------------------------------------

/// The window's root view: the production [`MessageList`], plus the one thing
/// only a root view can do -- read `&Window`'s frame histograms.
///
/// The message list itself is untouched production code, driven through its
/// public API (`show_channel`, `list_state`, `scroll_by`), for the reason
/// `tests/ui_message_list.rs` gives for going through the bridge: a bench that
/// wired the layers together differently from the application would be
/// measuring the bench.
struct BenchRoot {
    /// The list under measurement.
    list: Entity<MessageList>,
    /// The hand-off used to take snapshots from inside `render`.
    probe: Rc<RefCell<FrameProbe>>,
    /// When the run started, so every report says when it was captured.
    started: Instant,
}

impl BenchRoot {
    /// Takes the pending snapshot, if one was asked for.
    ///
    /// **This runs inside `Window::draw`, which is deliberate and load-bearing
    /// for the measurement's honesty**: gpui records a frame's duration when the
    /// draw *ends*, so a snapshot read during the render of frame N contains
    /// every frame up to N-1 and never the cost of this capture itself. The
    /// capture therefore cannot inflate the very numbers it reports.
    fn capture_if_requested(&mut self, window: &Window) {
        let mut probe = self.probe.borrow_mut();
        let Some(label) = probe.want.take() else {
            return;
        };
        let snapshot = window.frame_duration_snapshot();
        probe
            .reports
            .push(snapshot_report(label, self.started, &snapshot));
    }

    /// Marks this view **and the list** dirty, so the next frame rebuilds both.
    ///
    /// Both halves are required, and the second is the one that is easy to miss.
    /// gpui rebuilds a view only when that view is in `window.dirty_views`, and
    /// `Window::mark_view_dirty` marks a notified view's *ancestors* as well --
    /// so notifying the list alone would rebuild the list and, with it, this
    /// root. Notifying **only this root** would do the reverse: rebuild the root
    /// while `gpui/src/view.rs`'s prepaint cache hands the list back its
    /// previous layout unchanged, which for a scroll means changing the list's
    /// state and painting nothing -- a frame that is drawn, recorded, and shows
    /// the row the user is no longer looking at.
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.list.update(cx, |_list, list_cx| list_cx.notify());
    }
}

impl Render for BenchRoot {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.capture_if_requested(window);

        div()
            .id("bench-root")
            .flex()
            .flex_col()
            .size_full()
            // Explicit colours, per `AGENTS.md` section 7.3: GPUI does not
            // inherit text colour from parents, and this container is the one
            // every row draws on top of.
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(self.list.clone())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The timestamp of the `index`-th fixture message.
///
/// A fixed epoch rather than a clock read: `AGENTS.md` section 3.2 gives no
/// clock to this layer's callers in general, and a bench whose ordering
/// depended on wall-clock resolution would be a bench that could reorder its own
/// fixtures. Strictly increasing in `index`, so `insert_at_order` appends every
/// one of them.
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
fn message_at(index: usize) -> Message {
    Message {
        id: format!("m_{index}"),
        // `Uuid::from_u128` rather than a random id: nothing in production
        // generates ids yet (`AGENTS.md` section 7.4 puts that in `state/` and
        // `network/`), and a deterministic id makes a failed run reproducible.
        client_msg_id: Uuid::from_u128(index as u128 + 1),
        channel_id: CHANNEL.to_owned(),
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
/// [`WINDOW_HEIGHT`].
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

/// Prints the banner an external monitor needs before the run starts.
///
/// The pid and the executable path are printed first because `AGENTS.md`
/// section 6.2's RAM rows are sampled with the OS's own tooling, and a tool
/// pointed at the wrong process publishes a figure about that process.
fn print_banner() {
    let executable = std::env::current_exe().map_or_else(
        |error| format!("<unavailable: {error}>"),
        |path| path.display().to_string(),
    );

    println!("================================================================");
    println!(" Sh_Nexus frame-time bench -- AGENTS.md 6.2, ADR-006 step 6");
    println!(" pid: {} | exe: {executable}", std::process::id());
    println!(
        " window: {WINDOW_WIDTH}x{WINDOW_HEIGHT} logical px | messages: \
         {TOTAL_MESSAGES} | channel: {CHANNEL}"
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
/// process is in and how long it will stay there, which is what an external
/// monitor needs to know before it polls a working set.
fn announce(started: Instant, message: &str) {
    // `AGENTS.md` section 7.1 bans `println!` in `src/`. This file is a bench
    // target outside `src/`, and the printed report is the entire deliverable
    // -- see the module docs.
    println!("[{:>6.1}s] {message}", started.elapsed().as_secs_f64());
}

/// Formats nanoseconds as milliseconds with three decimals.
fn ms(nanoseconds: u64) -> f64 {
    // `as`, not `From`: `f64: From<u64>` does not exist, and the lossy
    // conversion is the point -- a nanosecond count rendered as milliseconds is
    // a display concern, and every count here is far below the 2^53 where an
    // `u64 as f64` stops being exact.
    nanoseconds as f64 / 1_000_000.0
}

/// Prints one line of the per-histogram table.
///
/// A histogram with no samples prints **no percentile at all** rather than a row
/// of zeros: `Stats::EMPTY`'s fields are zero, and a `0.000 ms` that means
/// "nothing was measured" is indistinguishable from a `0.000 ms` that means
/// "every frame was instantaneous". The reader must be able to tell the two
/// apart without trusting the bench.
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
/// * `probe`   - The snapshots the view took from inside `render`.
/// * `rendered` - How many messages the list held when scrolling began, read
///   back from the view rather than assumed from the seeding loop. The *when*
///   of each snapshot is on the snapshot itself ([`Report::at`]), taken at
///   capture time rather than looked up here.
///
/// # Errors
///
/// Returns an error -- which the caller turns into a non-zero exit -- when the
/// run produced no report, or when the compared-against histogram held zero
/// samples: a bench that printed a percentile of nothing would publish a false
/// figure, and a false figure is the one outcome this work unit must not
/// produce.
fn print_report(probe: &RefCell<FrameProbe>, rendered: usize) -> Result<(), String> {
    let reports = probe.borrow();
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
        " window: {WINDOW_WIDTH}x{WINDOW_HEIGHT} logical px | messages: {rendered} | \
         channel: {CHANNEL}"
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

/// Shows the bench channel to the list.
///
/// Done once, *before* the messages arrive, so that every seeded batch is
/// reconciled by the production path (`MessageList::sync` splicing on the next
/// frame) rather than by a `reset` that hands the list a finished count. The
/// idle phase that follows is therefore "an open channel with no history yet",
/// which is the empty state a real client shows first.
///
/// `MessageList::show_channel` notifies the list itself, which is also what
/// marks this root dirty -- so the frame that follows is scheduled by the
/// production code rather than by anything this bench adds.
fn show_channel(root: &Entity<BenchRoot>, cx: &mut AsyncApp) {
    root.update(cx, |bench, view_cx| {
        bench
            .list
            .update(view_cx, |list, list_cx| list.show_channel(CHANNEL, list_cx));
    });
}

/// Asks the list's subtree (and, as its ancestor, the root) to be rebuilt on the
/// next frame. See [`BenchRoot::invalidate`] for why both are named.
fn request_frame(root: &Entity<BenchRoot>, cx: &mut AsyncApp) {
    root.update(cx, |bench, view_cx| bench.invalidate(view_cx));
}

/// Delivers `TOTAL_MESSAGES` messages through the production bridge, in batches
/// no larger than the inbox.
///
/// The batch size is `bridge::MAX_PENDING_EVENTS` itself rather than a literal:
/// the inbox is a bounded `sync_channel` and a full one *refuses* delivery
/// (`AGENTS.md` section 7.1 forbids unbounded growth), so "10 000 events at
/// once" is not a thing this run can do -- it must deliver, drain, and repeat,
/// and the bound it must respect is the one the bridge publishes.
///
/// Each batch is followed by a frame request, because `bridge::drain` applies
/// events to the state and schedules no redraw (`MessageList::sync`
/// documentation says the frame is the caller's job). Without it the ten
/// batches would be handed to a renderer that never saw nine of them arrive,
/// and the "loaded" state would be a state that was never rendered as it grew.
async fn seed_messages(
    root: &Entity<BenchRoot>,
    sender: &EventSender,
    cx: &mut AsyncApp,
    started: Instant,
) -> Result<(), String> {
    let mut seeded = 0usize;
    while seeded < TOTAL_MESSAGES {
        let batch_end = (seeded + bridge::MAX_PENDING_EVENTS).min(TOTAL_MESSAGES);
        for index in seeded..batch_end {
            // `Display`, not `Debug`: a refusal owns the event it could not
            // queue, and `AGENTS.md` section 7.5 forbids logging content. The
            // `Display` impl is documented to name the outcome and nothing else.
            match sender.deliver(DomainEvent::MessageReceived(message_at(index))) {
                Delivery::Queued => {}
                refusal => {
                    return Err(format!(
                        "message {index} was refused by the inbox ({refusal}); the batch \
                         must stay within bridge::MAX_PENDING_EVENTS"
                    ));
                }
            }
        }

        let report = cx
            .update(bridge::drain)
            .ok_or_else(|| "the bridge was not installed before the window opened".to_owned())?;
        if report.refused() != 0 {
            return Err(format!(
                "the state refused {} of the messages it was handed ({report:?})",
                report.refused()
            ));
        }

        seeded = batch_end;
        request_frame(root, cx);
        cx.background_executor().timer(SEED_TICK).await;
    }

    announce(
        started,
        &format!("phase 2/4 seeded {seeded} messages through bridge::deliver + drain"),
    );
    Ok(())
}

/// Reads back how many messages the list is actually holding.
///
/// Read from the **view** rather than from the state, because the figure that
/// matters to this measurement is the one the renderer is drawing: a state that
/// holds 10 000 messages and a list that reconciled none of them would measure
/// an empty channel and publish it as a 10 000-message result.
fn rendered_messages(root: &Entity<BenchRoot>, cx: &AsyncApp) -> usize {
    root.read_with(cx, |bench, app| {
        bench.list.read_with(app, |list, _| list.item_count())
    })
}

/// Scrolls the list continuously, up for half the phase and down for the rest.
///
/// The direction flips rather than running one way forever so the list never
/// parks: a list clamped against the top of the history still redraws when it
/// is notified, but it redraws *static content*, and a run whose second half
/// measured no movement would be publishing a number about the wrong thing.
/// Scrolling up also stops tail-following (gpui's `scroll_by` does it for a
/// negative distance), which is what makes the list traverse rows the tail
/// would never show.
///
/// # Errors
///
/// Never returns one today, but the signature is kept uniform with the other
/// phases so `drive` reads as one sequence of fallible steps rather than a mix.
async fn scroll_phase(root: &Entity<BenchRoot>, cx: &mut AsyncApp) -> Result<(), String> {
    let began = Instant::now();
    let half = SCROLL_DURATION / 2;
    while began.elapsed() < SCROLL_DURATION {
        let step = if began.elapsed() < half {
            -SCROLL_STEP_PX
        } else {
            SCROLL_STEP_PX
        };
        root.update(cx, |bench, view_cx| {
            // `read` rather than a direct method call: gpui's `Entity<T>` is
            // deliberately not `Deref<Target = T>`, so the list's state can
            // only be reached through an explicit read of the entity. The read
            // is a single statement so its borrow of `view_cx` ends before
            // `invalidate` takes `&mut` of it.
            bench.list.read(view_cx).list_state().scroll_by(px(step));
            bench.invalidate(view_cx);
        });
        cx.background_executor().timer(SCROLL_TICK).await;
    }
    Ok(())
}

/// Asks the view for a snapshot and waits until it has taken one.
///
/// The wait is a poll rather than a callback because the only place a snapshot
/// can be taken is inside `render`, and nothing else runs there: the request is
/// a flag, the frame request is what guarantees a frame, and the flag being
/// cleared is what says the frame happened.
async fn capture(
    probe: &Rc<RefCell<FrameProbe>>,
    root: &Entity<BenchRoot>,
    cx: &mut AsyncApp,
    label: &'static str,
) -> Result<(), String> {
    probe.borrow_mut().want = Some(label);
    request_frame(root, cx);

    // Ten seconds of patience: a frame that has not arrived by then is a
    // window that is not drawing at all, and waiting longer would only turn a
    // broken measurement into a slow one.
    for _ in 0..400 {
        if probe
            .borrow()
            .reports
            .iter()
            .any(|report| report.label == label)
        {
            return Ok(());
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
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
/// broken run fails with a message on stderr and a non-zero exit rather than
/// with a stack trace and no diagnosis (`AGENTS.md` section 2.1).
async fn drive(
    root: &Entity<BenchRoot>,
    sender: &EventSender,
    probe: &Rc<RefCell<FrameProbe>>,
    started: Instant,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    // Phase 1 -- idle, empty. Nothing is requested during the dwell, so the
    // window draws no frames and the process is genuinely at rest for the RAM
    // sample.
    show_channel(root, cx);
    announce(
        started,
        &format!(
            "phase 1/4 idle empty: channel open, no history, dwelling {IDLE_DWELL:?} \
             -- an external monitor can sample this pid now"
        ),
    );
    cx.background_executor().timer(IDLE_DWELL).await;

    // Phase 2 -- load, then idle again at the loaded state.
    seed_messages(root, sender, cx, started).await?;
    announce(
        started,
        &format!(
            "phase 2/4 idle loaded: dwelling {SEEDED_DWELL:?} at rest -- an external \
             monitor can sample this pid now"
        ),
    );
    cx.background_executor().timer(SEEDED_DWELL).await;

    let rendered = rendered_messages(root, cx);
    if rendered != TOTAL_MESSAGES {
        return Err(format!(
            "the list holds {rendered} of the {TOTAL_MESSAGES} seeded messages, so the \
             figure that follows would be measured against the wrong history"
        ));
    }

    // Phase 3 -- the measurement itself. Both snapshots bracket the scroll;
    // see the module docs for why the histograms need bracketing at all.
    capture(probe, root, cx, "before-scroll").await?;
    announce(
        started,
        &format!("phase 3/4 scrolling for {SCROLL_DURATION:?}"),
    );
    scroll_phase(root, cx).await?;
    capture(probe, root, cx, "after-scroll").await?;

    // Phase 4 -- the deliverable.
    print_report(probe, rendered)?;
    announce(started, "phase 4/4 report printed; quitting");
    Ok(())
}

/// Opens one window, drives the four phases on the main thread, and exits
/// with the report's verdict.
///
/// The launch callback handed to `Application::run` is `FnOnce(&mut App)` and
/// cannot return a value, so a failure is carried out through an
/// `Rc<RefCell<Option<String>>>` -- the same shape `src/lib.rs::run` uses, for
/// the same reason (`AGENTS.md` section 2.1: no `unwrap` on a path that
/// matters). Nothing below panics; every failure becomes a message on stderr
/// and exit code 1.
fn main() {
    let started = Instant::now();
    let failure: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let probe = Rc::new(RefCell::new(FrameProbe {
        want: None,
        reports: Vec::new(),
    }));

    print_banner();

    let failure_in_callback = Rc::clone(&failure);
    let probe_in_callback = Rc::clone(&probe);

    gpui_platform::application().run(move |cx: &mut App| {
        // The application state first, before any window exists -- the ordering
        // `src/lib.rs::run` documents, and the one `bridge::install` requires.
        let sender = match bridge::install(cx, UNSIGNED_IN_USER) {
            Ok(sender) => sender,
            Err(error) => {
                *failure_in_callback.borrow_mut() = Some(format!(
                    "the application state could not be installed: {error}"
                ));
                return;
            }
        };

        let options = bench_window_options(cx);
        let list = cx.new(MessageList::new);
        let root = cx.new(|_view_cx| BenchRoot {
            list,
            probe: Rc::clone(&probe_in_callback),
            started,
        });

        // Hoisted out of the `match` scrutinee deliberately: a temporary in a
        // scrutinee lives until the end of the whole `match`, and the closure
        // handed to `open_window` borrows `root` to clone it -- so reading the
        // result inline would keep that borrow alive across the arm that moves
        // `root` into the task below.
        let opened = cx.open_window(options, |_window, _cx| root.clone());
        match opened {
            Ok(_handle) => {
                cx.activate(true);
                let probe_in_task = Rc::clone(&probe_in_callback);
                let failure_in_task = Rc::clone(&failure_in_callback);
                cx.spawn(async move |async_cx| {
                    let outcome = drive(&root, &sender, &probe_in_task, started, async_cx).await;
                    if let Err(message) = outcome {
                        *failure_in_task.borrow_mut() = Some(message);
                    }
                    async_cx.update(|app| app.quit());
                })
                .detach();
            }
            Err(error) => {
                *failure_in_callback.borrow_mut() =
                    Some(format!("the window could not be opened: {error}"));
            }
        }
    });

    if let Some(message) = failure.borrow_mut().take() {
        eprintln!("frame_time: FAILED -- {message}");
        std::process::exit(1);
    }

    // A quit that did not come from `drive` -- the window closed by hand, or
    // the platform ending the loop early -- reports no failure and prints no
    // report. A bench that exits 0 without publishing numbers has published
    // nothing while looking successful, so the snapshots are counted here.
    let snapshots = probe.borrow().reports.len();
    if snapshots < 2 {
        eprintln!(
            "frame_time: FAILED -- the run ended after {snapshots} of the 2 snapshots \
             the report needs, so no report was printed"
        );
        std::process::exit(1);
    }
}
