//! An end-to-end soak: a real `sh_nexus_server`, a real socket, real ACKs.
//!
//! # The gap this file closes
//!
//! `docs/BASELINES.md` records the 30-minute soak as driving *"one optimistic
//! send + one ACK per cycle … then `DomainEvent::MessageAcked`"*. **Every ACK was
//! injected.** No run had ever put a frame on a socket, started
//! `sh_nexus_server`, run `WsTransport`, or exercised `actions::flush_outbox` — so
//! the whole send path that landed in #43–#47 (socket, bridge seam, bounded
//! outbox, ack-released dequeue, undelivered re-drive) had no sustained-load
//! evidence at all. This file is the first run that has one.
//!
//! # Why a test and not another bench mode
//!
//! **Because it asserts, and 30 minutes of real traffic is also a correctness
//! run.** `benches/frame_time.rs --mode soak` prints numbers and cannot fail; the
//! invariants below are claims about the client's own state, and a bench has
//! nowhere to put them. `tests/support/mod.rs::ServerProcess` is the deciding
//! factor besides: it already knows how to find the server binary
//! (`current_exe()`'s grandparent, so debug and release both work), pick a free
//! port, bootstrap the administrator, log in over a real socket, **and restart on
//! the same port against the same database** — which is the only way to hold a
//! valid token across an outage. A new bench mode would mean a second process
//! launcher, and `state/bridge.rs`'s doctrine (*"the number of ways to do
//! something is the audit trail"*) is against that. **No second launcher is
//! written here.**
//!
//! # Why `println!` is allowed in this file and nowhere else
//!
//! **`AGENTS.md` §7.1 forbids `println!` in production, and this is not
//! production.** This is an `#[ignore]`d, thirty-minute, externally-sampled
//! harness: the documented method polls the process's working set every 500 ms
//! from a separate monitor and **slices the series on the markers this file
//! prints**, so the markers are the instrument rather than a diagnostic. The
//! rules that do apply here are kept: no message content in any line
//! (`AGENTS.md` §7.5 — ids, counts and durations only), and every figure carries
//! the platform and profile it was taken on.
//!
//! # The markers, and why the pid
//!
//! ```text
//! SOAK-PHASE name=<phase> elapsed_s=<n> pid=<n> cycle=<n> <extra key=value pairs>
//! ```
//!
//! Phases, in the order they occur: `startup`, `steady`, `offline`,
//! `reconnecting`, `flushing`, `drained`, `idle_after`, `done`. **The pid is
//! printed** because the historical 30-minute run recorded `pid 19972` — an
//! external sampler attaches to a number, and a marker line without one is a
//! marker nobody can join on. Three other line shapes exist and are never
//! confused with a phase:
//!
//! | Prefix | When |
//! |---|---|
//! | `SOAK-INFO` | the banner, and one progress line every [`PROGRESS_EVERY`] cycles |
//! | `SOAK-STALL` | the watchdog, immediately before it aborts |
//! | `SOAK-SUMMARY` | the last line, with the whole run's counters |
//!
//! # The watchdog, and what it does not cover
//!
//! **A hang produces no data.** The 120-minute run was killed by a stall at cycle
//! 73 691 with the cause never established; here a stall **aborts the run, names
//! the phase and the cycle, and exits non-zero** through a `panic!` rather than
//! through `std::process::exit`. That is deliberate: `process::exit` skips
//! destructors, which would leave the `sh_nexus_server` child running against its
//! temporary database, and an orphaned server on the operator's machine is a
//! worse outcome than a failed run.
//!
//! **What it covers:** no observable progress — no new ack, no frame read, no new
//! connect failure, no change in the connection state, the outbox depth, the
//! in-flight count or the held-row count — for [`NO_PROGRESS_BUDGET`]. That is
//! precisely the shape of the 120-minute failure, where the loop was alive and
//! the wait condition simply never became true.
//!
//! **What it does not cover, stated rather than implied:** a call that never
//! returns. `run_until_parked()` wedged inside the renderer would hold the main
//! thread and no in-loop check could fire, because catching that needs a watchdog
//! *thread* — which brings back the orphaned-child problem above. The phases where
//! progress is not expected (`idle_after`, `done`) are disarmed, because "nothing
//! is moving" is their design; they are bounded by wall clock instead.
//!
//! [`NO_PROGRESS_BUDGET`] is deliberately generous. The longest legitimate single
//! step in this harness is a server restart (bounded by `support::READY_BUDGET`,
//! 20 s) plus one backoff step (≤ 3.2 s) plus a drain of a few dozen
//! acknowledgements; 90 s is several multiples of the largest of those, so a
//! loaded machine is not a false positive.
//!
//! # What is asserted, and against what
//!
//! Every assertion reads **`AppState` through `bridge::try_read`**, never an
//! internal counter, and every send goes through **`MessageList::begin_send`** —
//! the door `InputBar::send` uses. The wiring is the production one:
//! `bridge::install`, then `WsTransport::start` plus `bridge::install_transport`,
//! which is `Shell::start_transport`'s body verbatim (it is spelled out here
//! rather than called because the harness also needs the `WsTransport` handle for
//! its counters).
//!
//! **The reconnect flush is not driven by this harness at all**, which was the one
//! thing it got wrong first time round: `Shell::apply_inbox` — the body of the
//! 50 ms timer `Shell::new` arms — calls `bridge::try_flush_outbox` on every tick,
//! so `actions::flush_outbox` already runs in production and it runs continuously
//! rather than on the transition to `Connected`. [`flush_and_drain`] makes one call
//! for the report it prints and then waits for the pump; every frame that reaches
//! the wire after the reconnect is put there by the shipped client.
//!
//! | Invariant | Assertion |
//! |---|---|
//! | the outbox does not leak | `outbox_len() == 0` after the outage, having held exactly `OFFLINE_BATCH` at its peak |
//! | one row per `client_msg_id` under real ACKs | a single pass over the channel finds no identity held twice, and every composed identity is `Acked` or retired |
//! | nothing left `Pending` the server acknowledged | `pending == 0`, `failed == 0`, and no held row still carries an empty server id |
//! | outage messages arrive **in enqueue order** | `outbox_ids()` equals the composed order before the flush, and the **server's own message ids increase strictly** in that order afterwards |
//!
//! The ordering assertion is strict rather than the `timestamps.windows(2).all(…
//! <= …)` that `tests/shell_connection.rs` settles for, and the reason is on the
//! server: `db.rs::mint_message_id` packs `(accepted_at_unix_millis << 64) |
//! per-insert sequence`, and `ws.rs` serves one connection from **one task** whose
//! receive branch `await`s `message::accept` before reading the next frame. So
//! server ids are strictly increasing in acceptance order, a strictly increasing
//! run in enqueue order is a real proof of delivery order, and a flush that drove
//! the queue backwards would invert it.
//!
//! # What this does not measure
//!
//! **The channel is not seeded to `MAX_MESSAGES_PER_CHANNEL`.** The server has no
//! `resync` yet (`ADR-010` is the next milestone), so a client cannot fetch
//! history, and seeding it through the sender's own inbox would be exactly the
//! injection this file exists to stop doing. History therefore grows from real
//! ACKs alone and reaches the cap on its own in a long run — which is a *better*
//! test of eviction than a pre-seeded cap, and a narrower claim: nothing here
//! exercises `db::repository.rs`, and nothing here measures bytes.
//!
//! The harness prints numbers. **It does not decide whether they pass** — the
//! thresholds are the project's (`AGENTS.md` §6.2) and the judgement belongs to
//! whoever records the run in `docs/BASELINES.md`, with its platform and profile.
//!
//! # Running it
//!
//! ```text
//! cargo build -p sh_nexus_server --release
//! SH_NEXUS_SOAK_MINUTES=1 cargo test --release -p sh_nexus --test soak_e2e -- --ignored --nocapture
//! ```
//!
//! **The length comes from the environment, not from a flag, and that is not a
//! preference — `libtest` refuses an unrecognised `--` option before any test body
//! runs**, so `-- --ignored --nocapture --soak-minutes 1` exits 101 with
//! `error: Unrecognized option: 'soak-minutes'` and runs nothing. The flag is still
//! read from `std::env::args()` and still wins where it survives; see
//! [`soak_minutes`] for the measurements behind that sentence.
//!
//! Without [`SOAK_MINUTES_ENV`] the run is [`DEFAULT_SOAK_MINUTES`] long, which is
//! §6.2's own time base. `cargo test --workspace` never reaches this file: the test
//! is `#[ignore]`d and needs the server binary beside the test executable.

mod support;

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::time::{Duration, Instant};

use chrono::Utc;
use gpui::{Entity, TestAppContext, VisualTestContext};
use sh_nexus::app::{Shell, DRAIN_INTERVAL, STARTUP_CHANNEL};
use sh_nexus::core::models::events::ConnectionState;
use sh_nexus::network::ws::{TransportConfig, WsTransport, MAX_OUTBOUND_FRAMES};
use sh_nexus::state::actions::{IgnoreReason, SendOutcome};
use sh_nexus::state::bridge::{self, EventSender, FlushReport};
use sh_nexus::state::DeliveryState;
use sh_nexus::UNSIGNED_IN_USER;
use uuid::Uuid;

use support::{ServerProcess, READY_BUDGET};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The run's length in minutes when `--soak-minutes` is absent.
///
/// **Thirty, and not a smaller default**, because thirty minutes is the time base
/// `AGENTS.md` §6.2 asks for and the one `docs/BASELINES.md` already records. A
/// default that quietly measured something else would make a bare
/// `cargo test --release … -- --ignored` a run whose figures could not be compared
/// with anything.
const DEFAULT_SOAK_MINUTES: u64 = 30;

/// The environment variable that carries the run's length in minutes.
///
/// **This is the channel that works today**, and its reason is in
/// [`soak_minutes`]: `libtest` refuses an unrecognised `--` option before the test
/// body runs, so a command-line flag cannot reach `std::env::args()` at all. The
/// flag is still read first and still wins, so nothing about the documented
/// spelling is given up.
const SOAK_MINUTES_ENV: &str = "SH_NEXUS_SOAK_MINUTES";

/// How long the harness tolerates *no* observable progress before it aborts.
///
/// **Ninety seconds, and the number is an argument rather than a guess.** The
/// longest legitimate step here is `ServerProcess::restart`'s own readiness wait
/// (`support::READY_BUDGET`, 20 s), then one step of the reconnect backoff (at
/// most 3.2 s on its third attempt), then a drain of at most [`OFFLINE_BATCH`]
/// acknowledgements over loopback. Ninety is several multiples of the largest, so
/// a machine slow enough to be running the compiler and the server and the test
/// at once is not a false positive — and a real stall still costs a minute and a
/// half rather than the two hours the historical run lost.
const NO_PROGRESS_BUDGET: Duration = Duration::from_secs(90);

/// The gap between two passes of any poll.
///
/// **A yield gap, not a wait.** Every wait in this file returns the instant its
/// condition holds; the gap only exists so the worker thread holding the socket
/// gets a core, which is the same 1 ms `support::wait_until` uses and for the
/// same reason. `AGENTS.md` §4.3's rule — no `sleep()` standing in for logic —
/// is honoured by every wait being a poll, never by the absence of sleeping.
const POLL_GAP: Duration = Duration::from_millis(1);

/// How many sends are composed while the server is away.
///
/// **Twenty-four, and the arithmetic that matters is `OFFLINE_BATCH * 2`.** Each
/// offline send is put on the transport's outbound queue by
/// `MessageList::enqueue` *and* re-driven by `actions::flush_outbox` after the
/// reconnect, so the queue has to hold two copies of the batch at once — which is
/// why the constant is asserted against [`MAX_OUTBOUND_FRAMES`] below rather than
/// left to the reader to check. Twenty-four is also more than a person types in
/// an outage and few enough that every row is still held at the end of the run,
/// which is what lets the ordering assertion read every one of them.
const OFFLINE_BATCH: usize = 24;

/// The `body()` counter's offset for the offline batch, so an outage message is
/// never byte-identical to a steady one.
const OFFLINE_BODY_BASE: u64 = 1_000_000;

/// How many pump ticks the outbox may need to drain after a reconnect.
///
/// **A stuck-queue detector, not a retry budget.** `Shell::apply_inbox` re-drives
/// the queue on every tick, so re-driving is the design and a large count is not by
/// itself a fault — but a healthy loopback answers a re-drive within a couple of
/// round trips, so a few seconds' worth of ticks means the acknowledgements are not
/// arriving. Two hundred ticks is ten seconds of real time, which is generous for a
/// machine also running a compiler and a server and this test.
const MAX_DRAIN_TICKS: u64 = 200;

/// How often a `SOAK-INFO` progress line is printed inside a long phase.
///
/// **Two hundred cycles, which is ten seconds of steady state at the cadence
/// below.** Progress lines exist so an operator watching a thirty-minute run can
/// see it moving and so a stall has a "last seen alive" line to sit next to; one
/// per cycle would be 28 000 lines, and the phase markers are what a sampler
/// actually joins on.
const PROGRESS_EVERY: u64 = 200;

/// A message body, which is composed and never printed.
///
/// **`AGENTS.md` §7.5 forbids message content in output, so this string reaches
/// exactly two places: the optimistic row and the frame on the wire.** It is
/// distinguishable per send so that a duplication bug would be visible as two
/// rows carrying the same text to anyone reading the state in a debugger — and it
/// is never formatted into a marker, a progress line or a panic message.
fn body(sequence: u64) -> String {
    format!("soak {sequence}")
}

// ---------------------------------------------------------------------------
// Phases and the watchdog
// ---------------------------------------------------------------------------

/// The eight phases an external sampler slices the series on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// Server, token, socket, state, window — the run's wiring.
    Startup,
    /// Sustained send and real acknowledgement.
    Steady,
    /// The server is away and the user keeps typing.
    Offline,
    /// The server is back and the transport is walking its backoff.
    Reconnecting,
    /// `actions::flush_outbox` is putting the queue on the wire.
    Flushing,
    /// The queue is empty and every queued row is reconciled.
    Drained,
    /// No sends: the level a sampler compares the slope against.
    IdleAfter,
    /// Every invariant, and the summary line.
    Done,
}

impl Phase {
    /// Every phase, in order — used to print the banner's `phases=` count.
    const ALL: [Phase; 8] = [
        Phase::Startup,
        Phase::Steady,
        Phase::Offline,
        Phase::Reconnecting,
        Phase::Flushing,
        Phase::Drained,
        Phase::IdleAfter,
        Phase::Done,
    ];

    /// The name an external sampler joins on.
    ///
    /// **The exact eight names the feature document specifies**, and `ALL` is the
    /// check that they are all here: a phase that existed only as a comment would
    /// leave a gap in a sampler's series with nothing to say so.
    const fn name(self) -> &'static str {
        match self {
            Phase::Startup => "startup",
            Phase::Steady => "steady",
            Phase::Offline => "offline",
            Phase::Reconnecting => "reconnecting",
            Phase::Flushing => "flushing",
            Phase::Drained => "drained",
            Phase::IdleAfter => "idle_after",
            Phase::Done => "done",
        }
    }

    /// Whether a stall in this phase is a defect rather than the design.
    ///
    /// **`idle_after` and `done` are disarmed, and that is the whole answer to
    /// "why does the watchdog not fire during the idle window".** Their correct
    /// behaviour *is* that nothing moves, so arming a no-progress check there
    /// would abort a healthy run. They are bounded by wall clock instead — the
    /// loop's own deadline — which is why they are safe to leave unarmed.
    const fn expects_progress(self) -> bool {
        !matches!(self, Phase::IdleAfter | Phase::Done)
    }
}

/// Detects the absence of progress and turns it into a bounded failure.
///
/// **A stamp, not a clock reading, is what counts as progress** — see
/// [`observe`]. A watchdog that watched elapsed time alone would fire on a phase
/// that is legitimately waiting, and one that watched a harness counter alone
/// would miss the case where the counters are fine and the state is not moving.
struct Watchdog {
    /// When the run began, for every `elapsed_s=`.
    started: Instant,
    /// Which phase the run is in.
    phase: Phase,
    /// How many sends have been composed, for the stall diagnosis.
    cycle: u64,
    /// The process a sampler attaches to.
    pid: u32,
    /// The last observation stamp that differed from the one before it.
    stamp: u64,
    /// When that stamp last moved.
    last_progress: Instant,
    /// How long silence is tolerated.
    budget: Duration,
    /// Whether this phase can be expected to make progress at all.
    armed: bool,
}

impl Watchdog {
    /// A watchdog for a run that has just begun.
    fn new(pid: u32) -> Self {
        Self {
            started: Instant::now(),
            phase: Phase::Startup,
            cycle: 0,
            pid,
            stamp: 0,
            last_progress: Instant::now(),
            budget: NO_PROGRESS_BUDGET,
            armed: true,
        }
    }

    /// Enters a phase: prints its marker and (re)arms or disarms the check.
    ///
    /// **The marker is printed on entry rather than on exit**, so a series is
    /// sliced by when a phase *began* — which is the only reading that survives a
    /// run that aborts, since an exit marker for the phase that stalled is exactly
    /// the marker that would be missing.
    fn enter(&mut self, phase: Phase, extra: &str) {
        self.phase = phase;
        self.armed = phase.expects_progress();
        self.last_progress = Instant::now();
        announce(&format!(
            "SOAK-PHASE name={} elapsed_s={} pid={} cycle={} {extra}",
            phase.name(),
            self.elapsed_s(),
            self.pid,
            self.cycle
        ));
    }

    /// Records one observation, and treats a change as progress.
    fn note(&mut self, stamp: u64) {
        if stamp != self.stamp {
            self.stamp = stamp;
            self.last_progress = Instant::now();
        }
    }

    /// Records that a send was composed, which is progress whatever else is idle.
    fn counted(&mut self, cycle: u64) {
        self.cycle = cycle;
        self.last_progress = Instant::now();
    }

    /// Whole seconds since the run began.
    fn elapsed_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// How long nothing has moved.
    fn quiet_for(&self) -> Duration {
        self.last_progress.elapsed()
    }

    /// Aborts the run if this phase has made no progress for longer than budget.
    ///
    /// **Prints a machine-readable `SOAK-STALL` line and then panics**, in that
    /// order, so the diagnosis is on stdout in the same format a sampler reads
    /// even though the panic message is the human-readable one.
    fn check(&self) {
        if !self.armed {
            return;
        }
        let quiet = self.quiet_for();
        if quiet <= self.budget {
            return;
        }
        announce(&format!(
            "SOAK-STALL name={} elapsed_s={} pid={} cycle={} quiet_s={} budget_s={}",
            self.phase.name(),
            self.elapsed_s(),
            self.pid,
            self.cycle,
            quiet.as_secs(),
            self.budget.as_secs(),
        ));
        panic!(
            "the soak made no observable progress for {}s in phase `{}` at cycle {}, \
             which is longer than the {budget_s}s watchdog allows; the run was aborted \
             rather than allowed to hang. The SOAK-STALL line above carries the same \
             diagnosis in machine-readable form.",
            quiet.as_secs(),
            self.phase.name(),
            self.cycle,
            budget_s = self.budget.as_secs(),
        );
    }
}

/// One pass of a wait: what was observed, and whether the wait is over.
///
/// **A struct rather than a bare `bool` because the two answers are independent.**
/// "Not done yet" is the common case and must still report what it saw, or the
/// watchdog would have nothing to compare against and every wait would look like
/// a stall.
struct Pass {
    /// The observation stamp for this pass.
    stamp: u64,
    /// Whether the condition being waited on now holds.
    done: bool,
}

/// Polls until `pass` reports `done`, the budget is spent, or the watchdog trips.
///
/// **This is the harness's only waiting primitive, and it is a condition poll**
/// rather than a `sleep()` — `AGENTS.md` §4.3's distinction, and the reason this
/// file has no `sleep()` standing in for anything. `support::wait_until` is the
/// same shape and is used verbatim for the first connect; every wait after that
/// goes through here **because only here can a stall be reported with the phase
/// and the cycle attached**, which is the entire point of the watchdog.
///
/// The 1 ms gap is a yield so the worker thread holding the socket gets a core,
/// exactly as `support::wait_until` does.
fn poll_until(guard: &mut Watchdog, what: &str, budget: Duration, mut pass: impl FnMut() -> Pass) {
    let deadline = Instant::now() + budget;
    loop {
        let observed = pass();
        guard.note(observed.stamp);
        if observed.done {
            return;
        }
        guard.check();
        assert!(
            Instant::now() < deadline,
            "timed out after {budget:?} waiting for {what}; the last observation was \
             {:?} old, in phase `{}` at cycle {}",
            guard.quiet_for(),
            guard.phase.name(),
            guard.cycle
        );
        std::thread::yield_now();
        std::thread::sleep(POLL_GAP);
    }
}

/// Waits for the wall clock to reach `slot`, checking the watchdog on the way.
///
/// **Spelled out rather than reusing [`poll_until`], because the cadence wait has
/// no observation to report** — there is nothing to watch, only a deadline — and
/// borrowing the phase's stamp for it would feed the watchdog a value that
/// changes on every cycle and so reports progress that never happened.
fn wait_until_slot(guard: &Watchdog, slot: Instant) {
    while Instant::now() < slot {
        guard.check();
        std::thread::yield_now();
        std::thread::sleep(POLL_GAP);
    }
}

/// Prints one machine-readable line, flushed.
///
/// **Flushed rather than left to the line buffer**, because the reader is an
/// external sampler attaching while the run is in flight — the documented method
/// polls every 500 ms — and a marker sitting in a buffer is a marker nobody can
/// slice on.
fn announce(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Reading the client's own state
// ---------------------------------------------------------------------------

/// Ticks the shell's real drain pump, draws a frame, and stamps what moved.
///
/// **This is `app.rs`'s schedule, not a second drainer.** `Shell::new` arms a
/// [`DRAIN_INTERVAL`] timer that empties the inbox and asks for a frame; walking
/// the test clock one interval forward fires that timer once, and
/// `run_until_parked` runs the frame it asked for. So the state this harness
/// reads has been through the production drain and the production renderer, which
/// is the difference between measuring the client and measuring a fixture.
///
/// The stamp folds in everything observable: the connection state (including
/// which reconnection attempt it is on), every counter that moves on its own, the
/// outbox depth, the number of sends still awaiting an answer, and the number of
/// rows held. **Anything moving is progress; nothing moving for
/// [`NO_PROGRESS_BUDGET`] is a stall** — and each of those is a real observable,
/// not a harness counter, so the watchdog cannot be satisfied by the harness
/// merely running.
fn observe(cx: &VisualTestContext, shell: &Entity<Shell>, transport: &WsTransport) -> u64 {
    cx.executor().advance_clock(DRAIN_INTERVAL);
    cx.run_until_parked();

    let stats = transport.stats();
    let (connection, outbox, awaiting, held) = shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| {
            (
                state.connection().clone(),
                state.outbox_len(),
                state.pending_sends().len(),
                state.message_count(STARTUP_CHANNEL),
            )
        })
        .expect("the state is installed: `bridge::install` ran before any window existed")
    });

    let mut stamp = connection_stamp(&connection);
    for counter in [
        stats.connects,
        stats.connect_failures,
        stats.frames_read,
        stats.frames_sent,
        stats.events_delivered,
        stats.events_refused,
        stats.frames_dropped,
        stats.writes_failed,
        stats.resyncs_sent,
        stats.pings_sent,
    ] {
        stamp = stamp.wrapping_mul(1_000_003).wrapping_add(counter);
    }
    stamp = stamp
        .wrapping_mul(1_000_003)
        .wrapping_add(u64::try_from(outbox).unwrap_or(u64::MAX));
    stamp = stamp
        .wrapping_mul(1_000_003)
        .wrapping_add(u64::try_from(awaiting).unwrap_or(u64::MAX));
    stamp = stamp
        .wrapping_mul(1_000_003)
        .wrapping_add(u64::try_from(held).unwrap_or(u64::MAX));
    stamp
}

/// The connection state as one number, so it can go into a stamp.
///
/// **The reconnection *attempt* is part of the value on purpose.** A client
/// walking its backoff publishes a new `Reconnecting { attempt }` on every try,
/// and those are exactly the passes where nothing else moves — so a stamp that
/// collapsed the variant to a constant would see a transport that is diligently
/// retrying look identical to one that has given up.
fn connection_stamp(connection: &ConnectionState) -> u64 {
    match connection {
        ConnectionState::Disconnected => 0,
        ConnectionState::Connecting => 1,
        ConnectionState::Connected => 2,
        ConnectionState::Reconnecting { attempt } => 3_000 + u64::from(*attempt),
        ConnectionState::Rejected { .. } => 4_000,
    }
}

/// The connection state this client currently holds.
fn connection(cx: &VisualTestContext, shell: &Entity<Shell>) -> ConnectionState {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.connection().clone())
            .expect("the state is installed: `bridge::install` ran before any window existed")
    })
}

/// Whether a send would reach a socket right now.
fn can_send(cx: &VisualTestContext, shell: &Entity<Shell>) -> bool {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.can_send())
            .expect("the state is installed: `bridge::install` ran before any window existed")
    })
}

/// How many sends the outbox is holding.
fn outbox_len(cx: &VisualTestContext, shell: &Entity<Shell>) -> usize {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.outbox_len())
            .expect("the state is installed: `bridge::install` ran before any window existed")
    })
}

/// The queued identities, **in enqueue order**.
///
/// **`AppState::outbox_ids` is documented to return them oldest-first, and that
/// order is the whole of `PLAN.md` §7's reconnect requirement** — so reading it
/// is how the harness checks the queue's half of the ordering claim rather than
/// assuming it.
fn outbox_ids(cx: &VisualTestContext, shell: &Entity<Shell>) -> Vec<Uuid> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.outbox_ids())
            .expect("the state is installed: `bridge::install` ran before any window existed")
    })
}

/// How far one of this client's sends got, if it is still tracked.
fn delivery(cx: &VisualTestContext, shell: &Entity<Shell>, id: &Uuid) -> Option<DeliveryState> {
    shell
        .read_with(cx, |_shell, app| {
            bridge::try_read(app, |state| state.delivery(id))
        })
        .expect("the state is installed: `bridge::install` ran before any window existed")
}

/// How many rows the channel holds.
fn message_count(cx: &VisualTestContext, shell: &Entity<Shell>) -> usize {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| state.message_count(STARTUP_CHANNEL))
            .expect("the state is installed: `bridge::install` ran before any window existed")
    })
}

/// The server's own id for one held row, or `None` if the row is not held.
fn server_id(cx: &VisualTestContext, shell: &Entity<Shell>, id: &Uuid) -> Option<String> {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| {
            state.message(STARTUP_CHANNEL, id).map(|row| row.id.clone())
        })
        .flatten()
    })
}

/// One pass over the client's own structures, at the end of a run.
///
/// **Every field is counted from `AppState` and never from a harness counter**,
/// which is what `odd/tasks/6a-e2e-soak.md` asks for: a counter the harness
/// incremented itself would prove the harness ran.
#[derive(Debug, Default)]
struct Census {
    /// How many sends this run composed.
    composed: usize,
    /// How many of them are `Acked` and still tracked.
    acked: usize,
    /// How many are still `Pending`. **Must be zero.**
    pending: usize,
    /// How many are `Failed`. **Must be zero** — nothing in this workload may be
    /// refused, and a failure here would be a message the user wrote and lost.
    failed: usize,
    /// How many are no longer tracked, having been acknowledged and then evicted
    /// with their delivery entry.
    retired: usize,
    /// How many rows the channel holds.
    rows_held: usize,
    /// How many of those rows still carry no server id, which is to say are still
    /// optimistic. **Must be zero**: this client is the only connection, and the
    /// server drops the origin's copy of its own broadcast, so every row here is
    /// a row this client composed and is waiting on.
    optimistic_rows: usize,
    /// Any identity held more than once, with its count. **Must be empty.**
    duplicated: Vec<(Uuid, usize)>,
}

/// Reads the end-of-run census in one pass over the state.
///
/// **One pass, and a map rather than a lookup per composed identity.** A
/// thirty-minute run composes tens of thousands of sends against a channel capped
/// at `MAX_MESSAGES_PER_CHANNEL`, so asking the state once per identity would make
/// the final assertion quadratic in the run's own length — and a soak whose
/// verification is the slowest part of it is a soak nobody re-runs. `BTreeMap`
/// rather than `HashMap` so the `duplicated` list is sorted and therefore
/// reproducible.
fn census(cx: &VisualTestContext, shell: &Entity<Shell>, composed: &HashSet<Uuid>) -> Census {
    shell.read_with(cx, |_shell, app| {
        bridge::try_read(app, |state| {
            let mut counts: BTreeMap<Uuid, usize> = BTreeMap::new();
            let mut optimistic_rows = 0usize;
            for row in state.messages(STARTUP_CHANNEL) {
                *counts.entry(row.client_msg_id).or_insert(0) += 1;
                if row.id.is_empty() {
                    optimistic_rows += 1;
                }
            }

            let mut census = Census {
                composed: composed.len(),
                rows_held: state.message_count(STARTUP_CHANNEL),
                optimistic_rows,
                ..Census::default()
            };

            for identity in composed {
                match state.delivery(identity) {
                    Some(DeliveryState::Acked) => census.acked += 1,
                    Some(DeliveryState::Pending) => census.pending += 1,
                    Some(DeliveryState::Failed) => census.failed += 1,
                    // Nothing tracks it and nothing failed: it was acknowledged and
                    // then evicted along with its delivery entry. That is
                    // `AppState::evict_one_over_cap`'s documented retirement, not a
                    // loss — which is why the assertion below is on
                    // `acked + retired == composed` rather than on `acked` alone.
                    None => census.retired += 1,
                }
            }

            census.duplicated = counts.into_iter().filter(|(_, count)| *count > 1).collect();
            census
        })
        .expect("the state is installed: `bridge::install` ran before any window existed")
    })
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// The run's length in minutes, from `--soak-minutes` or [`SOAK_MINUTES_ENV`].
///
/// **Read from `std::env::args()` rather than through a flag crate**, because
/// adding one would be a new dependency and `AGENTS.md` §7.2 makes that a decision
/// with an audit trail behind it. The harness passes its own flags through in the
/// same vector — `--ignored`, `--nocapture`, `--test-threads` — so the scanner
/// looks for the one it owns and ignores the rest. Both spellings are accepted
/// because a flag crate would accept both.
///
/// # Why there is a second channel, which is not belt and braces
///
/// **`libtest` parses its own command line before any test body runs, and it
/// rejects an option it does not know.** The invocation this file's own
/// documentation used to give —
///
/// ```text
/// cargo test --release -p sh_nexus --test soak_e2e -- --ignored --nocapture --soak-minutes 1
/// ```
///
/// — dies with `error: Unrecognized option: 'soak-minutes'` and exit code 101
/// **without running a single test**, and the `=` spelling (`--soak-minutes=1`)
/// fails identically. Measured on this toolchain, not assumed; the alternative
/// spellings are worse rather than better, because a bare
/// `soak-minutes=1` is accepted as a *test-name filter* and silently runs zero
/// tests while exiting 0.
///
/// So the flag is still read first, because it is the documented spelling and the
/// one a future `libtest`, or a wrapper that filters the vector, would honour — and
/// [`SOAK_MINUTES_ENV`] is the channel that works today. **The two are not
/// redundant: without the second one there is no way to run this harness at a
/// length other than the default.**
fn soak_minutes() -> (u64, &'static str) {
    let raw = match soak_minutes_flag() {
        Some(flag) => (flag, "flag"),
        None => match std::env::var(SOAK_MINUTES_ENV) {
            Ok(from_environment) => (from_environment, "env"),
            Err(_) => return (DEFAULT_SOAK_MINUTES, "default"),
        },
    };
    let (raw, source) = raw;

    let minutes: u64 = raw.trim().parse().unwrap_or_else(|error| {
        panic!("{source} wants a whole number of minutes, got {raw:?}: {error}")
    });
    assert!(
        minutes >= 1,
        "{source} wants at least 1: a run with no steady phase would compose nothing \
         and so assert nothing"
    );
    (minutes, source)
}

/// The `--soak-minutes` value from the command line, if one is there.
fn soak_minutes_flag() -> Option<String> {
    let arguments: Vec<String> = std::env::args().collect();
    let mut requested: Option<String> = None;
    let mut walk = arguments.iter();
    while let Some(argument) = walk.next() {
        let Some(joined) = argument.strip_prefix("--soak-minutes=") else {
            if argument == "--soak-minutes" {
                requested = walk.next().cloned();
            }
            continue;
        };
        requested = Some(joined.to_owned());
    }
    requested
}

/// The build profile, because a figure without one is not a measurement.
fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// The window the steady send loop gets, out of the whole run.
///
/// **Thirty-five per cent, and the remainder is not slack.** §6.2's time base is
/// thirty minutes of *active chatting*, and the outage has to happen inside that
/// same window — so the steady phase is cut to make room for phases where the
/// client is doing nothing. Those are the only points at which an external
/// sampler's series has a level to compare a slope against, and a run with no idle
/// window could not answer the question the previous run answered.
fn steady_share(total: Duration) -> Duration {
    (total * 35 / 100).max(Duration::from_secs(2))
}

/// The window after the run stops sending, out of the whole run.
fn idle_share(total: Duration) -> Duration {
    (total * 25 / 100).max(Duration::from_secs(2))
}

/// Asserts the outbound queue can hold two copies of the offline batch.
///
/// **At the top rather than at the point of use, because the failure it prevents
/// is a confusing one.** Each offline send is queued twice — once by
/// `MessageList::enqueue` when the composer hands it to the socket, once by
/// `actions::flush_outbox` after the reconnect — and a batch that overflowed
/// [`MAX_OUTBOUND_FRAMES`] would fail as a `TransportError::OutboundFull` on some
/// frame of a twenty-four, which says nothing about which constant was wrong.
const _: () = assert!(
    OFFLINE_BATCH * 2 <= MAX_OUTBOUND_FRAMES,
    "the offline batch must fit twice over in the transport's outbound queue: one copy \
     from the composer and one from the flush"
);

/// An outage batch long enough for the ordering claim to mean something.
///
/// **Four is the floor, and it is a floor on evidence rather than on load.** Two
/// messages can arrive in order by luck; a strictly increasing run of server ids
/// across four of them is a claim about a flush that walks its queue, and a batch
/// smaller than that would make invariant 4 an assertion about nothing.
const _: () = assert!(OFFLINE_BATCH >= 4);

/// Scenario 1 — sustained send, with a real acknowledgement for every cycle.
///
/// **Each cycle is the shipped one: `MessageList::begin_send`, then a wait for
/// the server's own answer.** The identity is a `Uuid::new_v4` because
/// `AGENTS.md` §7.4 requires a client-generated identity that exists before the
/// server has seen the message, and because a reused one would be refused by
/// `actions::begin_send` as `AlreadyHeld` — the same refusal a duplication bug
/// would produce.
///
/// **The cadence is the shell's own [`DRAIN_INTERVAL`]**, which is what makes this
/// comparable with the recorded ≈967 cycles/min: a tight loop would drive the
/// socket several times harder than the previous run did, and a RAM comparison
/// against `docs/BASELINES.md` would then be a comparison of two different
/// workloads.
fn steady(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    transport: &WsTransport,
    guard: &mut Watchdog,
    budget: Duration,
    composed: &mut Vec<Uuid>,
) {
    let deadline = Instant::now() + budget;
    let mut slot = Instant::now();
    let mut cycle: u64 = 0;

    while Instant::now() < deadline {
        wait_until_slot(guard, slot);

        let identity = Uuid::new_v4();
        let on_screen = shell.update_in(cx, |shell, _window, cx| {
            shell.list().update(cx, |list, list_cx| {
                list.begin_send(&body(cycle), identity, Utc::now(), list_cx)
            })
        });
        assert!(
            on_screen,
            "cycle {cycle}: `MessageList::begin_send` refused a fresh identity, which \
             means the state already held it. A `Uuid::new_v4` colliding is not a \
             possibility worth acting on, so this is a duplication defect."
        );
        composed.push(identity);
        cycle += 1;
        guard.counted(cycle);

        // **The wait is on the server's answer and on nothing else.** A condition
        // satisfied by our own bookkeeping would pass a step early and every figure
        // this run produces would describe a client the server never spoke to —
        // which is the defect this whole file exists to end.
        poll_until(guard, "the server's acknowledgement", READY_BUDGET, || {
            let stamp = observe(cx, shell, transport);
            Pass {
                stamp,
                done: delivery(cx, shell, &identity) == Some(DeliveryState::Acked),
            }
        });

        if cycle.is_multiple_of(PROGRESS_EVERY) {
            announce(&format!(
                "SOAK-INFO name=steady elapsed_s={} cycle={cycle} composed={} held={} \
                 outbox={}",
                guard.elapsed_s(),
                composed.len(),
                message_count(cx, shell),
                outbox_len(cx, shell),
            ));
        }
        slot += DRAIN_INTERVAL;
    }

    assert!(
        !composed.is_empty(),
        "the steady phase composed nothing in {budget:?}, so the run asserted nothing"
    );
}

/// Scenarios 2 and 3 — the server is taken away, the user keeps typing, and it
/// comes back on the same port against the same file.
///
/// **One function for both scenarios because they are one shape**, and the spec
/// says so: *"Same shape as 2 but framed as the outbox's reason for existing."* The
/// difference between them is what the reader is looking at, not what the client
/// does, and two near-identical functions would be two places for them to drift.
///
/// Returns the composed identities, in the order they were composed — which is the
/// order the next phases are checked against.
fn outage(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    transport: &WsTransport,
    server: &mut ServerProcess,
    guard: &mut Watchdog,
    composed: &mut Vec<Uuid>,
) -> Vec<Uuid> {
    // **The server goes first and the client is not told.** Nothing publishes
    // `Disconnected` here; the client has to notice a closed socket, and waiting
    // for that notice is part of what the scenario is for.
    server.stop();

    poll_until(
        guard,
        "the client to notice the server is gone",
        READY_BUDGET,
        || {
            let stamp = observe(cx, shell, transport);
            Pass {
                stamp,
                done: connection(cx, shell) != ConnectionState::Connected,
            }
        },
    );
    assert!(
        !can_send(cx, shell),
        "a client that cannot reach a server must refuse to send, because \
         `AppState::can_send` is what gates the composer and every send below \
         depends on it being false"
    );

    let mut batch: Vec<Uuid> = Vec::with_capacity(OFFLINE_BATCH);
    for index in 0..OFFLINE_BATCH {
        let identity = Uuid::new_v4();
        let on_screen = shell.update_in(cx, |shell, _window, cx| {
            shell.list().update(cx, |list, list_cx| {
                list.begin_send(
                    &body(OFFLINE_BODY_BASE + index as u64),
                    identity,
                    Utc::now(),
                    list_cx,
                )
            })
        });
        assert!(
            on_screen,
            "offline message {index}: the row must go up even with no server, which is \
             the whole claim of the Optimistic Send Flow"
        );
        assert_eq!(
            delivery(cx, shell, &identity),
            Some(DeliveryState::Pending),
            "offline message {index}: a send composed with no server reads `Pending`, \
             and is owned by the outbox rather than by the transport"
        );
        batch.push(identity);
        composed.push(identity);
    }

    // **The outage is held open until the client has actually tried and failed to
    // come back, and that is not padding.** The first retry waits
    // `backoff_window(1).start()` — 800 ms — so an outage shorter than that lets a
    // transport reconnect on its *first* attempt: a measured run of this harness
    // replaced the socket without recording a single `connect_failure`, which is a
    // real reconnection but exercises none of `network/reconnect.rs`, and the
    // scenario's whole point is the backoff. Waiting here makes the walk
    // deterministic instead of a race against a stopwatch, and it holds the client
    // offline while the backoff grows — which is also when
    // `actions::requeue_undelivered` gets its chance to run against a queue that is
    // already deep.
    poll_until(
        guard,
        "the transport to attempt a reconnect and fail",
        READY_BUDGET,
        || {
            let stamp = observe(cx, shell, transport);
            Pass {
                stamp,
                done: transport.stats().connect_failures >= 1,
            }
        },
    );
    assert!(
        connection(cx, shell) != ConnectionState::Connected,
        "nothing may be listening yet, so a `Connected` here would mean the restart \
         happened before the queue was inspected"
    );

    assert_eq!(
        outbox_ids(cx, shell),
        batch,
        "the outbox holds every message composed while down, **in enqueue order** — \
         `AppState::outbox_ids` is documented oldest-first and this is where that \
         order becomes checkable rather than assumed. A `SendUndelivered` arriving \
         mid-outage re-queues idempotently and must not move an entry."
    );
    assert_eq!(
        outbox_len(cx, shell),
        OFFLINE_BATCH,
        "and the queue is exactly as deep as the batch, so nothing was queued twice \
         and nothing was dropped"
    );

    batch
}

/// Brings the server back on the same port, and waits for the transport to find it.
///
/// **No hint is given to the client and no URL is re-read.** The transport is
/// pointed at a URL it has held since `startup`; all this does is put a server
/// back where one used to be. That is what makes it `ServerProcess::restart`
/// rather than a second transport — a fresh socket would prove nothing about
/// reconnection.
fn reconnect(
    cx: &VisualTestContext,
    shell: &Entity<Shell>,
    transport: &WsTransport,
    server: &mut ServerProcess,
    guard: &mut Watchdog,
) {
    // **The token survives because the database does.** `restart` hands the same
    // file to the new process, and a session token is bound to one database — so
    // the credential the transport already holds is still the right one. A new
    // database would make this test fail at the handshake, which is a fixture
    // fault wearing a network failure's clothes.
    server.restart();

    // **`support::READY_BUDGET`, and not longer.** The outage above is held open
    // until the transport has failed one attempt, so the server returns one or two
    // steps into the schedule and the next attempt is at most `backoff_window(3)` —
    // 3.2 s. Twenty seconds is several multiples of that.
    poll_until(
        guard,
        "the transport to reconnect on its own backoff",
        READY_BUDGET,
        || {
            let stamp = observe(cx, shell, transport);
            Pass {
                stamp,
                done: connection(cx, shell) == ConnectionState::Connected,
            }
        },
    );

    // **`connects >= 2`, which is the whole claim.** `connects` counts *established*
    // sockets, so two means the first one died and a second handshook against the
    // same URL with the same token and the same database — a real reconnection, not
    // a second transport and not a reused handle.
    //
    // `connect_failures` is deliberately **not** asserted to have grown here: the
    // outage phase already established at least one, and this function's job is the
    // recovery, not the count. Both are printed in the phase's marker so the figure
    // reaches whoever records the run.
    let stats = transport.stats();
    assert!(
        stats.connects >= 2,
        "the socket was replaced rather than reused, so this is a real \
         reconnection: {stats}"
    );
}

/// Waits for the shell's own drain pump to put the queue on the wire and empty it.
///
/// **The pump is the driver, and that is the claim worth making.** `Shell::apply_inbox`
/// — the body of the 50 ms timer `app.rs` arms in `Shell::new` — calls
/// `bridge::try_flush_outbox` on **every** tick, before its early return, which is
/// how `PLAN.md` §7's "flush on reconnect" is actually wired: not on the transition
/// to `Connected`, but continuously, so a queue is re-driven until the server
/// acknowledges it. So this harness makes **one** call to the door, to have the
/// [`FlushReport`] the phase marker prints, and then gets out of the way: every
/// subsequent frame on the wire is put there by the production pump.
///
/// **That is strictly stronger than driving it here**, and it is also why the pump
/// has to be *paced*. A first version re-drove from the harness on every pass of a
/// 1 ms poll, and separately let `observe` advance the test clock — which fires the
/// pump — on every pass too. Together they put the queue on the wire about 43 times
/// in 43 ms and moved `frames_sent` from 493 to 1 523. Harmless, because the
/// server's `ON CONFLICT(client_msg_id)` dedupe absorbs every replay and the dedupe
/// invariant is what proves it — but a figure about the harness rather than about the
/// client, and `frames_sent` is one of the numbers whoever records this run reads.
/// So the pump below fires once per [`DRAIN_INTERVAL`] of real time, which is what it
/// does in a real window.
///
/// **The tick count is asserted, and it is a stuck-queue detector rather than a
/// budget.** Re-driving is the design, so "many ticks" is not itself a fault; a
/// drain that needs more than [`MAX_DRAIN_TICKS`] of them is a queue whose
/// acknowledgements are not landing, which prints the same `driven` count as one
/// being re-driven perfectly well.
fn flush_and_drain(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    transport: &WsTransport,
    guard: &mut Watchdog,
) -> FlushReport {
    let first: FlushReport = shell
        .update(cx, |_shell, cx| bridge::try_flush_outbox(cx))
        .expect("the state is installed: `bridge::install` ran before any window existed");
    assert!(
        first.driven() > 0,
        "the reconnect drove nothing, so there was nothing queued — which contradicts \
         the outage phase having queued {OFFLINE_BATCH} messages"
    );

    let mut next_tick = Instant::now();
    let mut ticks: u64 = 0;
    poll_until(guard, "the outbox to drain to zero", READY_BUDGET, || {
        if Instant::now() < next_tick {
            // **Between ticks, on purpose.** The cheap stamp is the transport's own
            // frame counter, which moves while the acknowledgements are in flight, so
            // the watchdog still sees a client that is working.
            return Pass {
                stamp: transport.stats().frames_sent,
                done: false,
            };
        }
        next_tick += DRAIN_INTERVAL;
        ticks += 1;
        let stamp = observe(cx, shell, transport);
        Pass {
            stamp,
            done: outbox_len(cx, shell) == 0,
        }
    });

    assert!(
        ticks <= MAX_DRAIN_TICKS,
        "the shell's own drain pump needed {ticks} ticks to empty the outbox, which is \
         more than {MAX_DRAIN_TICKS} x {DRAIN_INTERVAL:?} of real time. Re-driving is \
         the design, so this is not a retry budget going wrong: it means the \
         acknowledgements are not landing."
    );

    first
}

/// Sends nothing for a while, at the shell's own schedule.
///
/// **The window exists so an external sampler's series has a level at both ends.**
/// A working set sampled only under load cannot be told apart from one that grows;
/// the recorded method needs a quiet tail to read the slope against. The pace is
/// [`DRAIN_INTERVAL`] because that is what the client does when nobody is typing:
/// one drain tick and one frame, twenty times a second.
///
/// **The watchdog is disarmed here** — see [`Phase::expects_progress`] — because a
/// run that is behaving correctly in this window makes no progress at all.
fn idle_after(
    cx: &VisualTestContext,
    shell: &Entity<Shell>,
    transport: &WsTransport,
    guard: &Watchdog,
    budget: Duration,
) -> u64 {
    let deadline = Instant::now() + budget;
    let mut slot = Instant::now();
    let mut passes: u64 = 0;

    while Instant::now() < deadline {
        let _stamp = observe(cx, shell, transport);
        passes += 1;
        slot += DRAIN_INTERVAL;
        wait_until_slot(guard, slot);
    }
    passes
}

/// One refused send, through the same door a successful one uses.
///
/// **`MessageList::begin_send` calls `bridge::try_begin_send`, which calls
/// `actions::begin_send`** — one door, three names. This reaches it and checks the
/// half a successful send cannot: that a send the state refuses creates nothing,
/// consumes no identity, and touches neither the outbox nor the channel. It is also
/// why `SendOutcome` and `IgnoreReason` appear in this file at all — the assertion
/// is in `actions`' own vocabulary, so a new refusal variant would break the build
/// here rather than pass unnoticed.
fn a_refused_send_creates_nothing(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
    let outbox_before = outbox_len(cx, shell);
    let held_before = message_count(cx, shell);
    let identity = Uuid::new_v4();

    let outcome = shell.update(cx, |_shell, cx| {
        bridge::try_begin_send(cx, STARTUP_CHANNEL, "   ", identity, Utc::now())
    });

    assert!(
        matches!(
            outcome,
            Some(SendOutcome::Ignored(IgnoreReason::EmptyContent))
        ),
        "a whitespace-only body must be refused by `actions::begin_send` rather than \
         queued, got {outcome:?}"
    );
    assert_eq!(
        outbox_len(cx, shell),
        outbox_before,
        "and a refused send must not reach the outbox"
    );
    assert_eq!(
        message_count(cx, shell),
        held_before,
        "and it must not create a row"
    );
    assert!(
        server_id(cx, shell, &identity).is_none(),
        "and the identity it was given must be unspent, so the next send may use it"
    );
}

/// Every invariant the run exists to establish, asserted against `AppState`.
fn assert_invariants(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    transport: &WsTransport,
    offline: &[Uuid],
    composed: &[Uuid],
) -> Census {
    let known: HashSet<Uuid> = composed.iter().copied().collect();
    let census = census(cx, shell, &known);

    // **1. The outbox does not leak.** `outbox_len` is `AppState`'s own counter and
    // the queue's only door out is `dequeue_outbox`, which an acknowledgement or a
    // terminal failure calls and nothing else does — so zero here means every entry
    // was answered, not that something forgot it.
    assert_eq!(
        outbox_len(cx, shell),
        0,
        "the outbox must be empty once the server has answered everything: this is the \
         first evidence the queue added in #46 does not leak. It held {OFFLINE_BATCH} \
         entries at its peak and holds {} now",
        outbox_len(cx, shell)
    );

    // **2. One row per identity, under real acknowledgements.** The census's
    // `duplicated` list is built by counting every row the channel holds, so it
    // catches a duplicate the client made *and* one the server replayed onto it.
    assert!(
        census.duplicated.is_empty(),
        "exactly one row per `client_msg_id`, after {} real acknowledgements and a \
         re-drive: {} identities are held more than once",
        census.acked,
        census.duplicated.len()
    );

    // **3. Nothing left `Pending`, and nothing failed.** A `Pending` row here is a
    // message the user wrote and is still shown as sending, with a queue that has
    // already forgotten it — the leak, wearing a different name.
    assert_eq!(
        census.pending, 0,
        "no send this run composed may still read `Pending`: the server acknowledged \
         every one of them"
    );
    assert_eq!(
        census.failed, 0,
        "no send this run composed may read `Failed`: nothing in this workload may be \
         refused, and a failure is a message the user wrote and lost"
    );
    assert_eq!(
        census.optimistic_rows, 0,
        "no held row may still carry an empty server id, which is to say still be \
         optimistic. This client is the server's only connection and the server drops \
         the origin's copy of its own broadcast, so every row here is one this run \
         composed and is still waiting on"
    );
    assert_eq!(
        census.acked + census.retired,
        census.composed,
        "every composed send is either acknowledged and tracked, or acknowledged and \
         since evicted with its delivery entry. {} acknowledged, {} retired, {} \
         composed",
        census.acked,
        census.retired,
        census.composed
    );

    // **4. Delivery in enqueue order, asserted rather than assumed.** The server's
    // message ids are strictly increasing in acceptance order — `db.rs` packs
    // `(accepted_at_unix_millis << 64) | per-insert sequence`, and `ws.rs` serves
    // one connection from one task that awaits each accept before reading the next
    // frame — so a strictly increasing run in the composed order is the server's
    // own account of the order it received them in.
    let accepted_in_order: Vec<u128> = offline
        .iter()
        .map(|identity| {
            let minted = server_id(cx, shell, identity).unwrap_or_else(|| {
                panic!(
                    "a message composed during the outage is no longer held, so its \
                     server id cannot be read and the ordering claim cannot be checked. \
                     The outage batch is the newest in the channel, so eviction should \
                     not have reached it."
                )
            });
            assert!(
                !minted.is_empty(),
                "a message composed during the outage is still optimistic, so the \
                 server never accepted it"
            );
            Uuid::parse_str(&minted)
                .unwrap_or_else(|error| {
                    panic!("the server's own message id did not parse ({error})")
                })
                .as_u128()
        })
        .collect();

    assert!(
        accepted_in_order.windows(2).all(|pair| pair[0] < pair[1]),
        "messages composed during the outage must arrive in enqueue order, and the \
         server's ids — which increase strictly in acceptance order — do not: \
         {} of {} adjacent pairs are out of order",
        accepted_in_order
            .windows(2)
            .filter(|pair| pair[0] >= pair[1])
            .count(),
        accepted_in_order.len().saturating_sub(1)
    );

    // **5. The socket was real, and stayed healthy.** Not a client-structure
    // invariant, but the one that says the run measured what it claims: a frame
    // dropped or an inbox refusal would mean the numbers below describe a client
    // that was losing traffic.
    let stats = transport.stats();
    assert_eq!(
        stats.frames_dropped, 0,
        "nothing the server sent was unreadable: {stats}"
    );
    assert_eq!(
        stats.events_refused, 0,
        "the event inbox was never full, so no frame was refused on its way to the \
         state: {stats}"
    );
    assert!(
        stats.connects >= 2,
        "the socket was established, replaced by the outage, and established again: \
         {stats}"
    );
    assert!(
        stats.connect_failures >= 1,
        "and the outage was real: {stats}"
    );

    census
}

/// The soak: a real server, a real socket, real acknowledgements, three scenarios.
///
/// **`#[ignore]`d, and the reason is not caution — it is cost.** Thirty minutes is
/// `AGENTS.md` §6.2's time base, the run needs `target/<profile>/sh_nexus_server`
/// beside the test executable, and `cargo test --workspace` must stay fast enough
/// to be run. The attribute carries its own reason so a reader who runs
/// `cargo test -- --ignored` by accident is told what they just started.
#[gpui::test]
#[ignore = "a 30-minute measurement against a real sh_nexus_server process; run SH_NEXUS_SOAK_MINUTES=1 to smoke-test it"]
fn the_send_path_survives_sustained_real_traffic_and_a_server_outage(cx: &mut TestAppContext) {
    let (minutes, minutes_from) = soak_minutes();
    let total = Duration::from_secs(minutes.saturating_mul(60));
    let pid = std::process::id();

    assert_eq!(
        Phase::ALL.len(),
        8,
        "the eight phase markers are the contract an external sampler slices on"
    );

    announce(&format!(
        "SOAK-INFO pid={pid} soak_minutes={minutes} minutes_from={minutes_from} \
         profile={} os={} arch={} watchdog_s={} phases={} steady_s={} idle_s={}",
        profile(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        NO_PROGRESS_BUDGET.as_secs(),
        Phase::ALL.len(),
        steady_share(total).as_secs(),
        idle_share(total).as_secs(),
    ));

    let mut guard = Watchdog::new(pid);
    let mut composed: Vec<Uuid> = Vec::new();

    // -----------------------------------------------------------------------
    // startup
    // -----------------------------------------------------------------------
    guard.enter(
        Phase::Startup,
        "detail=server, token, socket, state, window",
    );

    let port = support::free_port().expect("an ephemeral loopback port");
    let mut server = ServerProcess::start_on(port, "soak-e2e");
    // **A real `POST /auth/login` against the running server**, because the server
    // refuses an unauthenticated handshake and this is the client's suite: it should
    // make the request the product will make, against the endpoint the product will
    // use. The token is held in memory and printed nowhere — not in a marker, not in
    // a progress line, not in a panic message.
    let token = server.token();

    let sender: EventSender = cx
        .update(|cx| bridge::install(cx, UNSIGNED_IN_USER))
        .expect("the first install on an application must succeed");
    let (shell, cx) = cx.add_window_view(move |_, cx| Shell::new(sender, cx));

    // **`WsTransport::start` plus `bridge::install_transport`, which is
    // `Shell::start_transport`'s body verbatim.** Spelled out rather than called
    // because the harness also needs the handle for its counters and its
    // reconnection evidence; there is exactly one publication point either way, so
    // no second socket can exist. The token goes into the transport and nowhere
    // else — `TransportConfig`'s hand-written `Debug` exists so that a `{:?}` in a
    // panic message cannot print it.
    let transport = WsTransport::start(
        TransportConfig::new(server.url()).with_token(token),
        shell.read_with(cx, |shell, _| shell.sender().clone()),
    )
    .expect("starting a worker thread is not a network operation");
    shell.update_in(cx, |_shell, _window, cx| {
        bridge::install_transport(cx, Some(transport.clone()))
    });

    announce(&format!(
        "SOAK-INFO name=startup port={port} profile={}",
        profile()
    ));

    // **The first wait uses `support::wait_until` verbatim**, because it is the one
    // wait whose bound is exactly `READY_BUDGET` and which happens before any phase
    // beyond `startup` — there is no cycle to name yet, and the fixture's own
    // message already names the condition.
    support::wait_until("the client to report a connection", READY_BUDGET, || {
        cx.executor().advance_clock(DRAIN_INTERVAL);
        cx.run_until_parked();
        connection(cx, &shell) == ConnectionState::Connected
    });
    announce(&format!(
        "SOAK-INFO name=startup connected=true transport={}",
        transport.stats()
    ));

    // The state door, proved live through its own vocabulary, before the run
    // depends on it for thirty minutes.
    a_refused_send_creates_nothing(cx, &shell);

    // -----------------------------------------------------------------------
    // steady
    // -----------------------------------------------------------------------
    guard.enter(
        Phase::Steady,
        "detail=sustained send + real ack cadence_ms=50",
    );
    steady(
        cx,
        &shell,
        &transport,
        &mut guard,
        steady_share(total),
        &mut composed,
    );
    announce(&format!(
        "SOAK-INFO name=steady done cycles={} held={}",
        guard.cycle,
        message_count(cx, &shell)
    ));

    // -----------------------------------------------------------------------
    // offline — scenarios 2 and 3, one shape
    // -----------------------------------------------------------------------
    guard.enter(Phase::Offline, "detail=server stopped, composing anyway");
    let offline = outage(
        cx,
        &shell,
        &transport,
        &mut server,
        &mut guard,
        &mut composed,
    );
    announce(&format!(
        "SOAK-INFO name=offline queued={} outbox={} connection={:?}",
        offline.len(),
        outbox_len(cx, &shell),
        connection(cx, &shell)
    ));

    // -----------------------------------------------------------------------
    // reconnecting
    // -----------------------------------------------------------------------
    guard.enter(
        Phase::Reconnecting,
        "detail=server restarted, backoff walked",
    );
    reconnect(cx, &shell, &transport, &mut server, &mut guard);
    announce(&format!(
        "SOAK-INFO name=reconnecting transport={}",
        transport.stats()
    ));

    // -----------------------------------------------------------------------
    // flushing
    // -----------------------------------------------------------------------
    guard.enter(
        Phase::Flushing,
        "detail=actions::flush_outbox, in enqueue order",
    );
    let flushed = flush_and_drain(cx, &shell, &transport, &mut guard);
    announce(&format!(
        "SOAK-INFO name=flushing driven={} refused={} held={}",
        flushed.driven(),
        flushed.refused(),
        flushed.held()
    ));

    // -----------------------------------------------------------------------
    // drained
    // -----------------------------------------------------------------------
    guard.enter(
        Phase::Drained,
        "detail=outbox at zero, every row reconciled",
    );
    for identity in &offline {
        assert_eq!(
            delivery(cx, &shell, identity),
            Some(DeliveryState::Acked),
            "a message composed during the outage must be acknowledged after the \
             reconnect: the outbox emptying is not the same claim, and this is"
        );
    }
    announce(&format!(
        "SOAK-INFO name=drained outbox={} acked={} held={}",
        outbox_len(cx, &shell),
        offline.len(),
        message_count(cx, &shell)
    ));

    // -----------------------------------------------------------------------
    // idle_after — the level a sampler reads the slope against
    // -----------------------------------------------------------------------
    guard.enter(Phase::IdleAfter, "detail=activity stopped, no sends");
    let idle_passes = idle_after(cx, &shell, &transport, &guard, idle_share(total));
    announce(&format!(
        "SOAK-INFO name=idle_after drain_passes={idle_passes} held={}",
        message_count(cx, &shell)
    ));

    // -----------------------------------------------------------------------
    // done — every invariant, then the summary
    // -----------------------------------------------------------------------
    guard.enter(Phase::Done, "detail=invariants and summary");
    let census = assert_invariants(cx, &shell, &transport, &offline, &composed);

    announce(&format!(
        "SOAK-SUMMARY pid={pid} elapsed_s={} soak_minutes={minutes} \
         minutes_from={minutes_from} profile={} cycles={} composed={} \
         offline_batch={OFFLINE_BATCH} outbox_at_end={} acked={} retired={} pending={} \
         failed={} held_rows={} idle_passes={idle_passes} transport={}",
        guard.elapsed_s(),
        profile(),
        guard.cycle,
        census.composed,
        outbox_len(cx, &shell),
        census.acked,
        census.retired,
        census.pending,
        census.failed,
        census.rows_held,
        transport.stats()
    ));

    transport.shutdown();
    drop(server);
}
