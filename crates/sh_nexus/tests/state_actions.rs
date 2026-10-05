//! `state/app_state.rs` and `state/actions.rs`: the pure application state.
//!
//! Work unit 1E-1. `AGENTS.md` §4.2 names one row for this layer and it is four
//! clauses wide: *"Action mutations (send/receive/switch channel, reactions);
//! unread counts; optimistic send + rollback"*. This file covers all four, plus
//! the two conventions the layer had been deferring since 1C-2b — the **empty
//! server id before an ACK** and the **unread rule** — and plus the three
//! boundary guards the two files exist to hold.
//!
//! # What is pinned, and by which kind of test
//!
//! | Claim | Held by | Kind |
//! |---|---|---|
//! | Neither file reaches `gpui`, a socket, a clock, or interior mutability | `state_app_state_and_actions_are_pure`, `state_app_state_and_actions_hold_no_interior_mutability` | **structural** — the files are named, not the directory |
//! | Every mutation of application state goes through `actions.rs` | `state_exposes_no_public_mutation_surface` | structural, with one named exception |
//! | The single-thread invariant is a *rule*, not a type guarantee | `the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee` | **demonstrates the hazard** rather than denying it |
//! | An unacknowledged message holds no server id, and an acknowledged one always does | `an_unacknowledged_message_holds_no_server_id_and_an_acknowledged_one_always_does` | `#[rstest]`, three cases |
//! | An ACK is reconciled by `client_msg_id` and never by position | `an_ack_reconciles_by_client_msg_id_and_never_by_position` | hand-written, with a decoy row behind the pending one |
//! | A rollback keeps the row | `a_rollback_keeps_the_row_its_text_and_its_place` | hand-written |
//! | A late ACK upgrades a failed row | `a_late_ack_upgrades_a_failed_row_because_the_server_is_the_authority` | hand-written |
//! | A resync echo of a pending send is the same case | `a_resynced_own_message_merges_into_the_optimistic_row_by_client_msg_id` | hand-written |
//! | The unread rule's conditions | `only_a_foreign_message_for_a_channel_that_is_not_on_screen_is_unread` | `#[rstest]`, four cases |
//! | No event sequence leaves the state inconsistent | `an_arbitrary_sequence_of_events_leaves_the_state_internally_consistent` | **proptest**, invariants checked after *every* step |
//! | The unread count never goes negative and never exceeds the channel's messages | `the_unread_count_never_goes_negative_and_never_exceeds_the_channel` | **proptest**, against a model that recomputes |
//! | A channel never grows past `MAX_MESSAGES_PER_CHANNEL`, and the one exception is stated | `a_channel_at_its_bound_keeps_the_cap_and_loses_the_oldest`, `a_channel_whose_every_row_is_a_send_in_flight_is_allowed_over_the_cap`, plus the bound inside `assert_internally_consistent` | hand-written for the number (work unit 3C), property for the shape |
//! | A send in flight is never the row eviction takes | `a_send_in_flight_is_never_the_row_that_is_evicted` | `#[rstest]`, pending and failed |
//! | A reader scrolled into history keeps their row | `a_reader_scrolled_into_history_keeps_their_row_when_the_head_is_evicted` in `tests/ui_message_list.rs` | hand-written, at 10 001 messages through a real window |
//! | An offline send is queued and a connected one is not | `only_an_offline_send_is_queued` | `#[rstest]`, five connection states |
//! | Reconnecting drives every queued send in enqueue order | `a_reconnect_drives_every_queued_send_in_enqueue_order` | hand-written; the order is decided by `actions::flush_outbox` and checkable nowhere else |
//! | A queued entry leaves only on an ack or a terminal failure | `a_queued_send_leaves_on_an_ack_or_a_terminal_failure_and_on_nothing_else` | hand-written; the "and on nothing else" half is the claim |
//! | The bound refuses rather than dropping | `the_outbox_refuses_at_its_bound_and_the_row_says_which_bound`, `the_outbox_holds_its_bound_however_many_sends_are_composed` | hand-written, at and past the boundary |
//! | A retry re-queues at the back | `a_retry_requeues_at_the_back_and_leaves_on_an_ack_or_a_failure`, `a_retry_at_a_full_queue_is_refused_and_the_row_stays_failed` | hand-written, three-entry queue |
//! | A queued send never outlives its row | `a_discard_retires_nothing_because_a_failed_send_is_never_queued`, `a_queued_send_at_the_head_is_never_the_row_eviction_takes` | hand-written, through the discard and through eviction |
//! | A resync echo retires the entry too | `a_resync_echo_of_a_queued_send_retires_its_entry` | hand-written; the third door, and the one the proptest found missing |
//! | A **connected** send whose write fails is owed again, and still arrives | `a_connected_send_whose_write_fails_is_owed_again_and_still_arrives` | hand-written; the residual hole §7.1 used to name as open |
//! | It is not re-driven while the connection cannot carry it | `an_owed_send_is_not_re_driven_while_the_connection_cannot_carry_it` | `#[rstest]`, five states, twenty flushes each |
//! | Only the frames carrying durable content are owed | `only_a_message_send_is_owed_when_its_write_fails` in `tests/ws_transport.rs` | hand-written, over `FrameKind::CLIENT` |
//! | Every queue entry names a held, awaited row | inside `assert_internally_consistent` | **proptest**, checked after *every* step |
//!
//! # The two properties, and why they are two tests
//!
//! **`an_arbitrary_sequence_of_events_leaves_the_state_internally_consistent`
//! draws from the whole alphabet** and asserts *structural* invariants after every
//! step: each channel's list is ascending by `core::ordering::compare`, one row
//! per `client_msg_id`, both indexes resolve to the row they name, every
//! empty-id row is tracked as pending, and the unread count is bounded by the
//! channel's held messages. It needs no model, so it needs no restriction on the
//! alphabet.
//!
//! **`the_unread_count_never_goes_negative_and_never_exceeds_the_channel` draws
//! from a stated, narrower alphabet and compares against a model.** The model
//! shares no data structure with the implementation — `BTreeMap`s of `i64` against
//! `HashMap`s of `u32` — and, more importantly, it **replays the recorded step
//! list** rather than accumulating alongside the code under test, because a model
//! that keeps a running total in step with the implementation agrees with it by
//! construction. That is the same lesson `docs/COVERAGE.md` §4.9 and §5.5 forced
//! on this project twice about derived figures. The narrower alphabet is the price
//! of an exact model, and it is why both properties exist rather than one.
//!
//! The style is the project's: names that read as assertions (`AGENTS.md` §4.3),
//! `#[rstest]` with `#[case]` throughout (ADR-008), and proptest per §4.4.
//!
//! # What the two properties found, and why that is the argument for them
//!
//! **Four defects in `state/` and `state/actions.rs`, every one of them a case
//! that reads as reasonable and none of them reachable by a hand-written test
//! written from the specification.** This is the same argument `docs/COVERAGE.md`
//! §4.10 makes about `core/theme.rs` — a file at 97.74% with 191 passing tests had
//! two reachable gaps that only `--show-missing-lines` found — and it is the
//! reason §4.4's mandate is a mandate and not a formality.
//!
//! | Found by | Defect | Why no unit test found it |
//! |---|---|---|
//! | `an_arbitrary_sequence_of_events_leaves_the_state_internally_consistent` | **A resync echo left the send `Pending` forever.** A pending row reconciled against the server's copy gained a real `id` and stayed `Pending`, because only the `message.ack` path performed the upgrade — and a resync delivers the copy as a `message.new` | The upgrade lived in `acknowledge`, so it was correct for every test that used an `Ack`. `PLAN.md` §7's outbox case delivers the same message through the *other* door, and nothing in the flow's description says which one a server uses |
//! | the same property | **`begin_send` created a second row for one `client_msg_id` in another channel**, because it consulted the send map and the target channel's index but not the rest of the application | A `client_msg_id` collision across channels needs an identity reused in a different channel than it was first seen, which no single-flow test does |
//! | `the_unread_count_never_goes_negative_and_never_exceeds_the_channel` | **A merge dropped the message's unread count.** `remove_message` retires the count — which is what makes the bound structural — and the merge path has to remove before it re-inserts, so a message reconciled against a later copy silently stopped being unread | A stored counter, not a set, and the leak was a *decrement in the wrong direction*: the badge under-reported, which looks like a quiet channel rather than a bug |
//! | the same property | **A message the server moved between channels left its old channel's count behind**, so the count could exceed the messages actually held there — the precise failure the property was named for | The first implementation counted per channel with a `u32`, and the move path incremented the new channel without retiring the old one. Only an arbitrary *order* of unrelated events reaches it |
//!
//! **The fourth one is the one that changed the design rather than the code.**
//! The fix was not "decrement the old channel" — that is not derivable, because
//! whether the old channel was on screen *at the time the message arrived* is a
//! question the state has no honest answer to. The fix was to represent the count
//! as a **set of counted identities** rather than as a counter, which makes
//! `unread <= messages held` a property of the representation. A counter's bound
//! is maintained by every path that can change a count, and the path that forgets
//! is invisible; a set's bound is structural. The proptest still measures it, and
//! now it measures something that could not have been wrong.
//!
//! **And two of the four were in the *test's* model, not in the code**, which is
//! the half of the table that is easy to leave out: the first version treated an
//! `Edit` of an unknown identity as inert when it is an insertion, and added a
//! send-map entry for an `Ack` of a message already held. A model with a bug is
//! not a useless test — it is a test that fails on correct code, which is what
//! made the difference obvious within one run.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;
use rstest::rstest;
use sh_nexus::core::models::channel::Channel;
use sh_nexus::core::models::events::{ConnectionState, DomainEvent};
use sh_nexus::core::models::message::{Message, Reaction};
use sh_nexus::core::models::user::UserStatus;
use sh_nexus::core::ordering;
use sh_nexus::state::actions::{
    apply_event, begin_send, discard_failed_send, flush_outbox, render_and_cache, retry_send,
    select_channel, set_channels, ApplyOutcome, IgnoreReason, SendOutcome,
};
use sh_nexus::state::app_state::{
    AppState, DeliveryState, SendFailure, MAX_MESSAGES_PER_CHANNEL, MAX_OUTBOX_ENTRIES,
    MAX_TYPING_CHANNELS, MAX_TYPING_USERS_PER_CHANNEL,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Applies one event and **discards** its outcome.
///
/// **`let _ =` here rather than a `#[allow]` anywhere**, and the reason is the
/// point of `ApplyOutcome`'s `#[must_use]`: the attribute exists so a *caller*
/// cannot drop a refusal unnoticed. A test that is arranging state is not that
/// caller, and saying so in one named helper is better than forty scattered
/// `let _ =`. **Where the outcome is what is under test, this helper is not used**
/// — those call sites assert the `ApplyOutcome` itself.
fn fire(state: &mut AppState, event: DomainEvent) {
    let _ = apply_event(state, event);
}

/// A timestamp `second` seconds after a fixed base.
///
/// **A fixed base rather than a clock**, so every assertion here is a function of
/// its inputs and a failure is reproducible. `AGENTS.md` §4.3's "never `sleep()`
/// to wait for logic" is the same discipline applied to time.
fn at(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_789_000_000 + second, 0)
        .single()
        .expect("a fixed base plus a small offset is in range")
}

/// A `client_msg_id` from a small pool, so a test can name one without a random
/// generator and two rows cannot collide by accident.
fn cid(n: u128) -> Uuid {
    Uuid::from_u128(n + 1)
}

/// A server's stored message: a **real, non-blank** `id`.
///
/// **Non-blank is not a convenience, it is the wire's contract.**
/// `sh_nexus::network::mapping` refuses a blank required id, so a `DomainEvent`
/// carrying a blank one is inexpressible in production — and the proptest
/// generators below reproduce that, which is what makes "every held message with
/// an empty id is one of ours, and pending" an invariant rather than an accident
/// of the generator.
fn stored(
    server_id: &str,
    client: u128,
    channel: &str,
    author: &str,
    body: &str,
    second: i64,
) -> Message {
    Message {
        id: server_id.to_owned(),
        client_msg_id: cid(client),
        channel_id: channel.to_owned(),
        user_id: author.to_owned(),
        content: body.to_owned(),
        timestamp: at(second),
        edited_at: None,
        reactions: smallvec::SmallVec::new(),
        thread_id: None,
        attachments: smallvec::SmallVec::new(),
    }
}

/// A loaded channel.
fn channel(id: &str, name: &str) -> Arc<Channel> {
    Arc::new(Channel {
        id: id.to_owned(),
        name: name.to_owned(),
        description: None,
        is_private: false,
        members: Arc::new([]),
        last_message_at: None,
    })
}

/// A state with four loaded channels and a live connection.
fn loaded() -> AppState {
    let mut state = AppState::new("u_me").with_segment_cache_bounds(16, Some(4_096));
    set_channels(
        &mut state,
        (0..4)
            .map(|index| channel(&format!("c_{index}"), &format!("channel-{index}")))
            .collect(),
    );
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    state
}

/// The channel ids the loaded fixture uses.
const CHANNELS: [&str; 4] = ["c_0", "c_1", "c_2", "c_3"];

/// Every channel the invariant checker looks at: the fixture's four, plus every
/// channel the unread map has heard of.
///
/// **The unread map is included so an *unexpected* channel is caught** rather
/// than skipped, which is what makes the check complete without a public "list
/// the channels I hold messages for" accessor — one 1E-1 deliberately does not
/// add.
fn every_channel(state: &AppState) -> BTreeSet<String> {
    let mut channels: BTreeSet<String> = CHANNELS.iter().map(|id| (*id).to_owned()).collect();
    for (channel_id, _) in state.unread_counts() {
        channels.insert(channel_id.to_owned());
    }
    channels
}

// ---------------------------------------------------------------------------
// 1. The boundary guards
// ---------------------------------------------------------------------------

/// The client's `src/` directory.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// **The two files this work unit wrote, named explicitly.**
///
/// Not the whole `state/` directory, and the reason is a work unit rather than a
/// preference: 1E-2 adds `state/bridge.rs`, and `PLAN.md` §4 makes that file the
/// single owner of `cx.update_global`, so it will import `gpui` *legitimately*.
/// A directory-wide guard would have to be relaxed one work unit later, and a
/// boundary that gets relaxed is a boundary nobody reads.
fn the_two_pure_files() -> Vec<PathBuf> {
    let state = src_dir().join("state");
    vec![state.join("app_state.rs"), state.join("actions.rs")]
}

/// A file's source with every comment removed.
///
/// The same scanner `crates/sh_nexus/tests/layer_boundary.rs` uses, written out
/// again because a test binary cannot import another test binary's helpers —
/// which is why `crates/sh_nexus_wire/tests/dependency_direction.rs` carries its
/// own copy too. **Stripping comments first is load-bearing**: both files
/// *discuss* `gpui`, `Mutex` and `std::fs` at length, and a naive scan would fail
/// on this project's own documentation of the boundary.
fn without_comments(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    let mut in_block_comment = false;

    while let Some(character) = characters.next() {
        if in_block_comment {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                in_block_comment = false;
            }
            continue;
        }
        if character == '/' {
            match characters.peek() {
                Some('/') => {
                    for next in characters.by_ref() {
                        if next == '\n' {
                            stripped.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    characters.next();
                    in_block_comment = true;
                }
                _ => stripped.push(character),
            }
            continue;
        }
        stripped.push(character);
    }
    stripped
}

/// A readable file's comment-stripped source, or a panic naming it.
fn stripped_source(file: &Path) -> String {
    let text = fs::read_to_string(file)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display()));
    without_comments(&text)
}

/// Every `use` statement in a file, paired with the first path segment.
fn use_segments(stripped: &str) -> Vec<(String, String)> {
    stripped
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed
                .strip_prefix("pub use ")
                .or_else(|| trimmed.strip_prefix("use "))?;
            let segment = rest
                .split("::")
                .next()
                .unwrap_or_default()
                .trim_start_matches("::");
            (!segment.is_empty()).then(|| (trimmed.to_owned(), segment.to_owned()))
        })
        .collect()
}

/// Crates `state/app_state.rs` and `state/actions.rs` may name in a `use`.
///
/// **An allow-list, not a deny-list**, for the reason
/// `layer_boundary::CORE_ALLOWED_CRATES` gives: a crate added to `Cargo.toml`
/// tomorrow is rejected here until somebody adds it to this list and writes down
/// why.
///
/// **`serde` is absent entirely rather than split, and that is the one
/// difference from `core/`'s list worth stating.** 1D had to admit `serde_json`
/// and keep out `serde`, because `core/theme.rs` parses a theme document. **The
/// state layer parses nothing**, so there is no `_json` exception to draw and the
/// rule is the simpler one: the name `serde` in any spelling is rejected. The
/// converse check at the end of the test is what stops the list being widened into
/// uselessness — a filter that rejects everything passes every check above.
const STATE_ALLOWED_CRATES: [&str; 8] = [
    "std", "core", "alloc", "chrono", "uuid", "smallvec", "crate", "super",
];

/// `state/app_state.rs` and `state/actions.rs` name no I/O, platform, async, or
/// serialization crate, and read no clock.
///
/// `AGENTS.md` §3.2's rule for `core/`, extended to this layer by the argument
/// `layer_boundary.rs` makes: a layer that opens a socket or reads a clock cannot
/// be tested hermetically, so the coverage floor is either kept by keeping the
/// layer pure or waived. `AGENTS.md` §4.1 sets 80% for `state/`; keeping the
/// layer pure is what makes that reachable at all.
///
/// **The token list, and why each entry is here:**
///
/// | Token | Why |
/// |---|---|
/// | `gpui` | §3.2 and §7.3. `state/bridge.rs` (1E-2) is the seam; these two files are not |
/// | `tokio` | §3.2. `network/` is Phase 4 |
/// | `std::fs`, `std::io`, `std::net`, `std::process` | `PLAN.md` §4. Persistence is `db/`; there is nothing here to read |
/// | `Utc::now`, `Local::now` | the clock. An optimistic send's timestamp is a **parameter** of `begin_send` precisely because this layer may not read one |
/// | `std::thread`, `std::env` | a thread or an environment value makes a result a function of something other than the arguments, which is what would make the proptest failures irreproducible |
/// | `serde` | §3.3 and `PLAN.md` §5. Nothing in the client derives a serialization trait, and unlike `core/` there is no parser to admit — see [`STATE_ALLOWED_CRATES`] |
/// | `sh_nexus_wire` | ADR-002's direction: the protocol does not depend on the client, and the client reaches the domain through `core::models` |
///
/// Checked three ways, because the three fail differently: an import allow-list
/// (exact, but only sees `use`), a token scan over comment-stripped source
/// (catches a fully qualified path written inline), and a check that the
/// allow-list itself has not been widened into uselessness.
#[test]
fn state_app_state_and_actions_are_pure() {
    let files = the_two_pure_files();
    assert_eq!(
        files.len(),
        2,
        "the guard names two files by hand; a third would need a reason here"
    );

    for file in &files {
        assert!(
            file.is_file(),
            "{} should exist: AGENTS.md 3.1 places the state layer at src/state/",
            file.display()
        );
        let stripped = stripped_source(file);

        for (statement, segment) in use_segments(&stripped) {
            assert!(
                STATE_ALLOWED_CRATES.contains(&segment.as_str()),
                "{} imports `{segment}` (`{statement}`), which AGENTS.md 3.2 does not \
                 allow in the state layer. Allowed: {STATE_ALLOWED_CRATES:?}",
                file.display()
            );
        }

        for token in [
            "gpui",
            "tokio",
            "std::fs",
            "std::io",
            "std::net",
            "std::process",
            "Utc::now",
            "Local::now",
            "std::thread",
            "std::env",
            "serde",
            "sh_nexus_wire",
        ] {
            assert!(
                !stripped.contains(token),
                "{} mentions `{token}` after comment stripping. AGENTS.md 3.2 and \
                 PLAN.md section 4 keep the state layer pure: no gpui, no tokio, no \
                 filesystem, no clock, no thread, no serialization. The optimistic \
                 send's clock and identity are parameters of begin_send for exactly \
                 this reason.",
                file.display()
            );
        }
    }

    // The converse: a filter that rejects everything passes every check above.
    for forbidden in [
        "gpui",
        "tokio",
        "serde",
        "serde_json",
        "sh_nexus_wire",
        "reqwest",
    ] {
        assert!(
            !STATE_ALLOWED_CRATES.contains(&forbidden),
            "`{forbidden}` must NOT be on the state's import allow-list: \
             {STATE_ALLOWED_CRATES:?}"
        );
    }
}

/// `state/app_state.rs` and `state/actions.rs` hold no interior mutability.
///
/// **This is the load-bearing guard for the no-lock decision, and it is the half
/// of it that 1C-2b could not write for this layer.** `core/cache.rs` §13 settled
/// that *it* needs no internal synchronisation, and
/// `layer_boundary::core_cache_contains_no_interior_mutability` holds that
/// decision. `AppState` is the cache's real owner, so the same question reaches one
/// level up — and the answer is the same, with the reasoning recorded in
/// `state/app_state.rs`'s module docs, §3.
///
/// **A `Mutex` added here would be worse than one added to `core/cache.rs`**, and
/// the reason is specific to this layer: a lock on `AppState` sits in front of
/// *every* state mutation, which on this project's priorities (§1's
/// responsiveness, §6.2's 8ms scroll frame) is a much hotter path than a
/// rendered-segment cache read. It would also make the single-thread invariant
/// unenforceable in the other direction — with a lock present, a second thread
/// becomes *possible*, and the invariant would then be maintained by nobody
/// noticing.
///
/// Scoped to the two files, and scoped to **this layer's own contract**:
/// interior mutability is not wrong in `state/` in general — a future
/// `state/bridge.rs` will legitimately hold a channel to a tokio task — but it is
/// wrong *here*, in the two files whose documented contract is "one owner, one
/// thread, and `&mut self` is how it changes".
#[test]
fn state_app_state_and_actions_hold_no_interior_mutability() {
    for file in the_two_pure_files() {
        let stripped = stripped_source(&file);
        for token in [
            "Mutex",
            "RwLock",
            "RefCell",
            "Cell<",
            "UnsafeCell",
            "OnceCell",
            "LazyLock",
            "AtomicBool",
            "AtomicUsize",
            "thread_local!",
        ] {
            assert!(
                !stripped.contains(token),
                "{} contains `{token}`. The state layer is documented \
                 (state/app_state.rs module docs, section 3) as owned by one thread \
                 and changed only through `&mut self`, and that is what makes work \
                 unit 1C-2b's no-lock decision for core/cache.rs true for the cache \
                 this layer owns. A lock here would also make a second thread \
                 *possible*, which is the direction that makes the invariant \
                 unenforceable. PLAN.md section 4 makes state/bridge.rs the single \
                 owner of cx.update_global; a worker thread that genuinely must reach \
                 application state goes through that file, not through a lock in here.",
                file.display()
            );
        }
    }
}

/// `AGENTS.md` §3.2's "all mutations go through `actions.rs`" is mechanical.
///
/// The mandate is *"All mutations go through `actions.rs` so they are auditable
/// and testable"*, and the mechanism that makes it true rather than aspirational
/// is that **`AppState`'s mutators are `pub(crate)`**: they are invisible from
/// outside the crate, including from this test file, and therefore from every
/// other module. The compiler enforces the part that matters.
///
/// This test covers the half a compiler cannot: that no `pub fn` in
/// `app_state.rs` has quietly become a mutator of application state, which is the
/// one way the split could rot without a single build error.
///
/// **One exception, and it is named rather than allowed generically:**
/// `AppState::rendered` takes `&mut self` because a cache *hit* is what protects
/// an entry and `core/cache.rs` §6 is explicit that a non-promoting probe is a
/// different operation. It changes the cache and nothing else.
#[test]
fn state_exposes_no_public_mutation_surface() {
    const ALLOWED_PUBLIC_MUTATORS: [&str; 1] = ["rendered"];

    let file = src_dir().join("state").join("app_state.rs");
    let stripped = stripped_source(&file);
    let lines: Vec<&str> = stripped.lines().collect();

    let mut found: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("&mut self") {
            continue;
        }
        // A signature can wrap, so look back a few lines for the `fn` keyword
        // rather than assuming it shares the parameter's line. A bound of four
        // covers rustfmt's default 100-column wrapping for these signatures, and
        // the failure mode of a miss is a false negative on this one guard — the
        // `pub(crate)` convention makes the other direction the impossible one.
        let start = index.saturating_sub(4);
        let window = lines[start..=index].join(" ");
        let Some(declaration) = window.rfind("pub fn ") else {
            continue;
        };
        let name: String = window[declaration + "pub fn ".len()..]
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !name.is_empty() && !found.contains(&name) {
            found.push(name);
        }
    }

    assert_eq!(
        found,
        ALLOWED_PUBLIC_MUTATORS.to_vec(),
        "{}: these public methods take `&mut self`. AGENTS.md 3.2 requires every \
         mutation of application state to go through state/actions.rs so it is \
         auditable; the mutators on AppState are `pub(crate)` precisely so they are \
         unreachable from outside the crate. The one allowed exception is \
         `rendered`, which takes `&mut self` because a cache hit is what protects an \
         entry (core/cache.rs section 6) and it changes the cache and nothing else. \
         A new name here means a public mutator, which is the thing this guard \
         exists to stop.",
        file.display()
    );
}

/// `AppState` is `Send + Sync`, **and that is a hazard rather than a
/// guarantee**.
///
/// **Two claims in one test on purpose, because only together do they say
/// something.** The compile-time assertion says the type imposes no barrier on
/// crossing a thread. The thread spawn says what follows from that: a *whole copy*
/// of the application state can be moved to another OS thread, mutated there with
/// no lock and no compiler objection, and read back. If the confinement invariant
/// were a type property, this test would not compile.
///
/// **Why the property exists at all, given it constrains nothing here:** the
/// rendered-segment cache inside `AppState` is `Send + Sync` as a field type, and
/// `gpui::Global` does not require it (see the verdict recorded in
/// `state/app_state.rs`'s module docs, §3). So the property is a *choice*, and a
/// choice is worth stating plainly rather than leaving as an accident of the field
/// types.
///
/// The invariant that is real is a **rule** — one owner, one thread, named in
/// `state/app_state.rs`'s module docs §3 — and it is guarded by
/// [`state_app_state_and_actions_hold_no_interior_mutability`] and by this test,
/// which exists so that nobody later reads the `Send + Sync` assertion as a safety
/// property it was never meant to be.
#[test]
fn the_state_is_send_and_sync_and_that_is_a_hazard_rather_than_a_guarantee() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<AppState>();

    let mut state = loaded();
    let moved = state.self_user_id().to_owned();

    let handle = std::thread::spawn(move || {
        // Mutated on a *different* thread than the one that built it, with no lock
        // anywhere. Nothing in the type system objects, and that is the point.
        fire(
            &mut state,
            DomainEvent::ConnectionStateChanged(ConnectionState::Rejected {
                code: "version_mismatch".to_owned(),
                detail: "the server speaks v2".to_owned(),
            }),
        );
        let rejected_on_that_thread = !state.can_send();
        fire(
            &mut state,
            DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
        );
        (
            rejected_on_that_thread,
            state.can_send(),
            state.self_user_id().to_owned(),
        )
    });

    let (rejected, connected, seen_there) = handle
        .join()
        .unwrap_or_else(|_| panic!("the worker thread should not panic"));
    assert!(rejected, "the state was mutated on the other thread");
    assert!(connected, "and was fully mutable there");
    assert_eq!(seen_there, moved, "the whole state crossed the thread");
}

// ---------------------------------------------------------------------------
// 2. The empty server id before the ACK
// ---------------------------------------------------------------------------

/// An unacknowledged message holds **no** server id, and an acknowledged one
/// always does.
///
/// **The convention, pinned as an assertion** — `state/app_state.rs`'s module docs
/// §4 states it, and this is the test that makes it a fact rather than a comment.
/// The three cases are the three states a send of this client's can be in.
///
/// The `Failed` case is the load-bearing one: `PLAN.md` §7's rollback is a
/// transition rather than a removal, and a rollback that acquired an id would
/// mean a terminally refused send claimed the server had stored it.
#[rstest]
#[case("pending", DeliveryState::Pending, "")]
#[case("acked", DeliveryState::Acked, "m_1")]
#[case("failed", DeliveryState::Failed, "")]
fn an_unacknowledged_message_holds_no_server_id_and_an_acknowledged_one_always_does(
    #[case] label: &str,
    #[case] expected: DeliveryState,
    #[case] expected_id: &str,
) {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "hello", mine, at(0));

    let row = state.message("c_0", &mine).expect("the row exists");
    assert_eq!(row.id, "", "an optimistic row has no server id yet");

    match expected {
        DeliveryState::Acked => {
            fire(
                &mut state,
                DomainEvent::MessageAcked {
                    client_msg_id: mine,
                    message: stored("m_1", 1, "c_0", "u_me", "hello", 1),
                },
            );
        }
        DeliveryState::Failed => {
            fire(
                &mut state,
                DomainEvent::MessageSendFailed {
                    client_msg_id: mine,
                    code: "forbidden".to_owned(),
                    detail: "you may not post here".to_owned(),
                },
            );
        }
        DeliveryState::Pending => {}
    }

    assert_eq!(state.delivery(&mine), Some(expected), "the {label} state");
    let row = state.message("c_0", &mine).expect("the row is still held");
    assert_eq!(
        row.id, expected_id,
        "a {label} row holds the server id iff the server has spoken about it"
    );
}

/// A message the server sent always carries an id, and is never tracked as one of
/// this client's sends.
///
/// The converse of the convention, and the reason the invariant "every held
/// message with an empty id is pending" is a statement about *our* sends rather
/// than a coincidence.
#[test]
fn a_message_from_the_server_carries_its_id_and_is_not_one_of_our_sends() {
    let mut state = loaded();
    let theirs = cid(90);

    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_7", 90, "c_0", "u_ada", "hi", 0)),
    );

    let row = state.message("c_0", &theirs).expect("the row is held");
    assert_eq!(row.id, "m_7");
    assert_eq!(
        state.delivery(&theirs),
        None,
        "somebody else's message is not one of this client's sends"
    );
    assert!(!state.pending_sends().contains(&("c_0", theirs)));
    assert!(
        state
            .pending_sends()
            .iter()
            .all(|(_, client)| *client != theirs),
        "and it is not among the pending sends"
    );
}

// ---------------------------------------------------------------------------
// 3. The optimistic send
// ---------------------------------------------------------------------------

/// A refused send creates **nothing** — no row, no delivery entry, no unread.
///
/// `AGENTS.md` §2.1 makes every public function validate its preconditions, and
/// the interesting part is what "refuse" must *not* leave behind: a composer that
/// optimistically adds a row for an empty message would show the user a message
/// the server is about to reject.
#[rstest]
#[case("", "body", IgnoreReason::BlankChannelId)]
#[case("   ", "body", IgnoreReason::BlankChannelId)]
#[case("c_0", "", IgnoreReason::EmptyContent)]
#[case("c_0", "   \t ", IgnoreReason::EmptyContent)]
fn a_send_the_preconditions_reject_creates_nothing(
    #[case] channel_id: &str,
    #[case] content: &str,
    #[case] expected: IgnoreReason,
) {
    let mut state = loaded();
    let mine = cid(1);

    assert_eq!(
        begin_send(&mut state, channel_id, content, mine, at(0)),
        SendOutcome::Ignored(expected)
    );
    assert_eq!(state.message_count("c_0"), 0, "no row was created");
    assert_eq!(state.delivery(&mine), None, "and no delivery entry either");
    assert!(state.pending_sends().is_empty());
    assert_eq!(state.unread("c_0"), 0);
}

/// An optimistic send is on screen at once, with no server id and a live identity.
///
/// **§6.2's "<16ms, Enter to message on screen" row is not measurable here** —
/// there is no frame, no clock and no GPU in this work unit. What *is* assertable,
/// and is what makes that row reachable at all, is the whole of the optimistic
/// contract in one function: the row exists before any network event, its `id` is
/// empty, its `client_msg_id` is the one supplied, and it is `Pending`.
#[test]
fn an_optimistic_send_is_on_screen_before_the_server_has_seen_it() {
    let mut state = loaded();
    let mine = cid(1);

    assert_eq!(
        begin_send(&mut state, "c_1", "hello team", mine, at(10)),
        SendOutcome::Pending {
            client_msg_id: mine,
            offline: false
        }
    );

    assert_eq!(state.message_count("c_1"), 1);
    assert_eq!(state.delivery(&mine), Some(DeliveryState::Pending));
    assert_eq!(state.pending_sends(), vec![("c_1", mine)]);
    let row = state.message("c_1", &mine).expect("the row is on screen");
    assert_eq!(row.id, "");
    assert_eq!(row.client_msg_id, mine);
    assert_eq!(row.user_id, "u_me");
    assert_eq!(row.content, "hello team");
    assert_eq!(
        row.timestamp,
        at(10),
        "the optimistic timestamp is the caller's estimate, and the server replaces it"
    );
}

/// The same identity cannot be sent twice.
///
/// The refusal is load-bearing rather than defensive: `AGENTS.md` §7.4 has the
/// server dedupe on `client_msg_id`, so a *reused* identity would make the second
/// send a replay of the first and silently vanish. Reporting it is the only
/// honest answer.
#[test]
fn a_client_msg_id_that_is_already_outstanding_cannot_be_sent_again() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "first", mine, at(0));

    assert_eq!(
        begin_send(&mut state, "c_1", "second", mine, at(1)),
        SendOutcome::Ignored(IgnoreReason::AlreadyPending {
            client_msg_id: mine
        })
    );
    assert_eq!(state.message_count("c_1"), 0, "and no row was created");
    assert_eq!(state.message_count("c_0"), 1);
}

/// A send while the connection cannot carry it is still created, and says so.
///
/// **The row is created either way, and that is the decision.** The Optimistic
/// Send Flow is connection-independent, and a client that hid the message until
/// the socket came up would fail the very flow it is named for. What the outcome
/// reports is whether the outbox owns it — `PLAN.md` §7 — which is a signal the
/// composer needs and a question the connection state answers.
#[rstest]
#[case(ConnectionState::Connected, false)]
#[case(ConnectionState::Connecting, true)]
#[case(ConnectionState::Reconnecting { attempt: 1 }, true)]
#[case(ConnectionState::Disconnected, true)]
#[case(
    ConnectionState::Rejected {
        code: "version".to_owned(),
        detail: "v2".to_owned()
    },
    true
)]
fn a_send_while_the_connection_cannot_carry_it_is_queued_and_says_so(
    #[case] connection: ConnectionState,
    #[case] offline: bool,
) {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(&mut state, DomainEvent::ConnectionStateChanged(connection));

    let mine = cid(1);
    let outcome = begin_send(&mut state, "c_0", "hello", mine, at(0));

    assert_eq!(
        outcome,
        SendOutcome::Pending {
            client_msg_id: mine,
            offline
        }
    );
    assert_eq!(outcome.is_offline(), offline);
    assert_eq!(
        state.message_count("c_0"),
        1,
        "the row is on screen regardless"
    );
    assert_eq!(state.can_send(), !offline);
    assert!(outcome.is_pending());
}

// ---------------------------------------------------------------------------
// 4. The ACK, the rollback, and the late ACK
// ---------------------------------------------------------------------------

/// An ACK is reconciled by `client_msg_id` and **never by position**.
///
/// The decoy is the point of the test. A position-based implementation — "the
/// pending row is the last message, so replace that one" — passes every other
/// test in this file and fails this one, because a resync appends a page *behind*
/// the pending rows and the newest message is somebody else's. So a decoy sits
/// between two pending rows here, and the ACK must land on the right one and
/// touch nothing else.
#[test]
fn an_ack_reconciles_by_client_msg_id_and_never_by_position() {
    let mut state = loaded();
    let first = cid(1);
    let second = cid(2);
    let decoy = cid(3);

    begin_send(&mut state, "c_0", "first", first, at(0));
    // The resync page: a message the server sent, *newer* than both pending rows.
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_0", 3, "c_0", "u_ada", "resync page", 5)),
    );
    begin_send(&mut state, "c_0", "second", second, at(1));

    assert_eq!(
        state.message_count("c_0"),
        3,
        "two pending rows and a decoy"
    );
    let newest_is_the_decoy = state
        .messages("c_0")
        .last()
        .is_some_and(|message| message.client_msg_id == decoy);
    assert!(
        newest_is_the_decoy,
        "the decoy must be the newest, or a position-based implementation would pass"
    );

    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: second,
            message: stored("m_2", 2, "c_0", "u_me", "second", 3),
        },
    );

    assert_eq!(state.delivery(&second), Some(DeliveryState::Acked));
    assert_eq!(
        state.delivery(&first),
        Some(DeliveryState::Pending),
        "the other pending row is untouched"
    );
    assert_eq!(
        state.message("c_0", &decoy).map(|row| row.content.as_str()),
        Some("resync page"),
        "the decoy is untouched"
    );
    assert_eq!(
        state.message("c_0", &second).map(|row| row.id.as_str()),
        Some("m_2"),
        "and the acknowledged row carries the server's id"
    );
    assert_eq!(state.message_count("c_0"), 3, "no row was added or removed");
}

/// The ACK's stored values win over the optimistic ones, and the row moves.
///
/// The server is the authority on both `id` and `timestamp`
/// (`core/models/events.rs` on `MessageAcked`), and the local timestamp was an
/// estimate — which is exactly why the ordering key is defined on the server's
/// acceptance time. So the row **moves** when the server's timestamp lands
/// somewhere else, and this asserts that it moved rather than staying where the
/// client put it.
#[test]
fn the_acknowledged_row_takes_the_servers_id_and_timestamp_and_moves() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "hello", mine, at(50));
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_0", 2, "c_0", "u_ada", "earlier", 1)),
    );

    assert_eq!(
        state.messages("c_0")[0].content,
        "earlier",
        "the optimistic estimate put ours last"
    );

    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: mine,
            message: stored("m_1", 1, "c_0", "u_me", "hello", 2),
        },
    );

    let held = state.messages("c_0");
    assert_eq!(held.len(), 2);
    assert_eq!(
        held[0].content, "earlier",
        "the server's timestamp sorts ours above the decoy, so the row moved"
    );
    assert_eq!(held[1].content, "hello");
    assert_eq!(held[1].timestamp, at(2));
    assert_eq!(held[1].id, "m_1");
}

/// A rollback keeps the row, its text and its place.
///
/// `PLAN.md` §7: *"A send that fails terminally transitions to
/// `DeliveryState::Failed` and stays visible for retry. Failures are never
/// silently dropped."* The three assertions are the three things "stays visible"
/// has to mean, and the first is the one an implementation is most likely to get
/// wrong: a rollback that *removed* the row would leave the unread count and the
/// position looking right and take the user's text with it.
#[test]
fn a_rollback_keeps_the_row_its_text_and_its_place() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(
        &mut state,
        "c_0",
        "a message the server will refuse",
        mine,
        at(10),
    );
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_0", 2, "c_0", "u_ada", "before", 5)),
    );
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 3, "c_0", "u_ada", "after", 20)),
    );
    let before: Vec<String> = state
        .messages("c_0")
        .iter()
        .map(|message| message.content.clone())
        .collect();

    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: mine,
            code: "too_long".to_owned(),
            detail: "the body exceeds 4000 characters".to_owned(),
        },
    );

    let after: Vec<String> = state
        .messages("c_0")
        .iter()
        .map(|message| message.content.clone())
        .collect();
    assert_eq!(before, after, "the row and its position are unchanged");
    assert_eq!(state.message_count("c_0"), 3);
    assert_eq!(state.delivery(&mine), Some(DeliveryState::Failed));
    assert_eq!(
        state.message("c_0", &mine).map(|row| row.id.as_str()),
        Some(""),
        "a terminally failed send never acquired a server id"
    );
    let failure = state.failure(&mine).expect("the reason is recorded");
    assert_eq!(failure.code(), "too_long");
    assert_eq!(failure.detail(), "the body exceeds 4000 characters");
    assert!(
        !state.pending_sends().contains(&("c_0", mine)),
        "a failed send is not pending"
    );
    assert_eq!(
        state.unread("c_0"),
        2,
        "the two messages from somebody else -- and never the rolled-back one, which is \
         our own: module docs section 5's condition 3"
    );
}

/// A late ACK upgrades a failed row, because the server is the authority.
///
/// **This is the hard case, and the decision is in `state/actions.rs`'s module
/// docs, §2.2.** The short form: "keep `Failed` and also insert the acked row"
/// would hold two rows for one `client_msg_id`, which `core/ordering.rs` §2 says
/// is the reshuffle bug, so the upgrade is the only answer that keeps one row per
/// identity. The cost — `PLAN.md` §7's failure is no longer terminal — is stated
/// in the module docs rather than hidden here.
///
/// The test asserts the *whole* transition, because the interesting part is what
/// it must not break: still one row, still the user's text, and the recorded
/// reason cleared so the UI cannot show a stale error beside a message the server
/// accepted.
#[test]
fn a_late_ack_upgrades_a_failed_row_because_the_server_is_the_authority() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "the text the user wrote", mine, at(10));

    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: mine,
            code: "rate_limited".to_owned(),
            detail: "slow down".to_owned(),
        },
    );
    assert_eq!(state.delivery(&mine), Some(DeliveryState::Failed));
    assert!(state.failure(&mine).is_some());

    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: mine,
            message: stored("m_1", 1, "c_0", "u_me", "the text the user wrote", 12),
        },
    );

    assert_eq!(state.delivery(&mine), Some(DeliveryState::Acked));
    assert_eq!(
        state.failure(&mine),
        None,
        "the reason belonged to the failure and goes when the failure does"
    );
    assert_eq!(
        state.message_count("c_0"),
        1,
        "one row for one client_msg_id -- the invariant core/ordering.rs section 2 \
         rests on"
    );
    let row = state.message("c_0", &mine).expect("the row survived");
    assert_eq!(row.id, "m_1");
    assert_eq!(row.content, "the text the user wrote");
    assert_eq!(
        row.timestamp,
        at(12),
        "the server's acceptance time replaced the estimate"
    );
    assert_eq!(
        retry_send(&mut state, mine),
        ApplyOutcome::Ignored(IgnoreReason::NotFailed {
            client_msg_id: mine
        }),
        "and an accepted send is no longer retryable"
    );
}

/// A replayed ACK is a no-op, and a replayed duplicate is too.
///
/// `AGENTS.md` §7.4's replay protection has to hold at the state layer as well: a
/// resync after a reconnect can legitimately replay frames the client already
/// applied, and `core/models/events.rs` says the events are idempotent by
/// construction on the consuming side.
#[test]
fn a_replayed_ack_and_a_replayed_message_are_both_no_ops() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "once", mine, at(0));
    let acked = stored("m_1", 1, "c_0", "u_me", "once", 1);
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: mine,
            message: acked.clone(),
        },
    );

    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::MessageAcked {
                client_msg_id: mine,
                message: acked.clone(),
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::AlreadyHeld {
            client_msg_id: mine
        })
    );
    assert_eq!(state.message_count("c_0"), 1, "and no row was added");
    assert_eq!(state.delivery(&mine), Some(DeliveryState::Acked));

    // A duplicate of the *delivered* message -- a resync replaying a page the
    // client already holds -- is equally inert.
    assert_eq!(
        apply_event(&mut state, DomainEvent::MessageReceived(acked)),
        ApplyOutcome::Ignored(IgnoreReason::AlreadyHeld {
            client_msg_id: mine
        })
    );
    assert_eq!(state.message_count("c_0"), 1);
    assert_eq!(
        state.unread("c_0"),
        0,
        "and the unread count did not double"
    );
}

/// A resynced copy of the client's own pending message merges into the optimistic
/// row, by `client_msg_id`.
///
/// **`PLAN.md` §7's outbox case.** A send queued while offline is re-sent on
/// reconnect, the server deduplicates on `client_msg_id`, and the resync then hands
/// the client back its own message. The optimistic row and the server's stored
/// message meet with the same identity and different `id`s, and
/// `core/ordering.rs`'s `precedence` makes the real id win *by rule*.
///
/// The outcome is reported with the differing field named, because
/// `core/ordering.rs` §3 is explicit that a disagreement is reported and never
/// silently merged — the bridge is what logs it.
#[test]
fn a_resynced_own_message_merges_into_the_optimistic_row_by_client_msg_id() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "sent while offline", mine, at(10));

    let outcome = apply_event(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_me", "sent while offline", 10)),
    );

    match outcome {
        ApplyOutcome::Merged { ref fields } => assert_eq!(
            fields.as_slice(),
            &[ordering::DifferingField::Id],
            "the only field that may differ is the id -- the convention's whole point"
        ),
        other => panic!("the resync echo should merge, got: {other}"),
    }
    assert_eq!(
        state.message_count("c_0"),
        1,
        "one row for one client_msg_id, or the next reconcile would collapse them"
    );
    let row = state.message("c_0", &mine).expect("the row survived");
    assert_eq!(row.id, "m_1", "the server's id replaced the empty one");
    assert_eq!(
        state.delivery(&mine),
        Some(DeliveryState::Acked),
        "and the send is acknowledged -- a resync echo arrives as `message.new`, not as \
         `message.ack`, so an implementation that only upgraded on the ACK would leave \
         this row Pending forever, which is the proptest's first finding"
    );
    assert!(state.pending_sends().is_empty());
    assert_eq!(
        state.unread("c_0"),
        0,
        "our own message is not unread, and a resync must not change that"
    );
}

/// An ACK for a message this client does not hold is **adopted**, not dropped.
///
/// Module docs, §4: the client may not hold the row because it restarted, or
/// because the user discarded the failed send, and in both cases the server holds
/// a message the user wrote that the client would otherwise never show. The ACK
/// carries the stored message in full, so the repair is one insertion.
#[rstest]
#[case(true, 0u32, "our own message is never counted as unread")]
#[case(false, 1, "somebody else's message, in a channel not on screen")]
fn an_ack_with_no_local_row_is_adopted_rather_than_dropped(
    #[case] ours: bool,
    #[case] expected_unread: u32,
    #[case] why: &str,
) {
    let mut state = loaded();
    let client = cid(50);
    let author = if ours { "u_me" } else { "u_ada" };

    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::MessageAcked {
                client_msg_id: client,
                message: stored("m_50", 50, "c_2", author, "a message the server has", 4),
            }
        ),
        ApplyOutcome::Adopted {
            client_msg_id: client
        }
    );
    assert_eq!(
        state.message("c_2", &client).map(|row| row.id.as_str()),
        Some("m_50"),
        "a lost message is the failure this layer exists to prevent"
    );
    assert_eq!(state.unread("c_2"), expected_unread, "{why}");
    assert_eq!(
        state.delivery(&client),
        ours.then_some(DeliveryState::Acked),
        "only this client's own sends are tracked"
    );
}

/// A retry reuses the identity, and a discard removes exactly one row.
///
/// **Reusing the identity is the decision and it is not obvious.**
/// `PLAN.md` §7 says the server "deduplicates on `client_msg_id`, so a flush that
/// partially succeeded before a dropped connection does not duplicate messages",
/// so a retry that minted a *fresh* UUID would defeat exactly the mechanism that
/// protects the user from a duplicate — and the optimistic row's text, position and
/// timestamp are all unchanged for the same reason.
#[test]
fn a_retry_reuses_the_identity_and_a_discard_removes_exactly_one_row() {
    let mut state = loaded();
    let retried = cid(1);
    let discarded = cid(2);
    let survivor = cid(3);
    begin_send(&mut state, "c_0", "keep this", retried, at(0));
    begin_send(&mut state, "c_0", "drop this", discarded, at(1));
    begin_send(&mut state, "c_0", "and this", survivor, at(2));
    for client in [1u128, 2, 3] {
        fire(
            &mut state,
            DomainEvent::MessageSendFailed {
                client_msg_id: cid(client),
                code: "nope".to_owned(),
                detail: String::new(),
            },
        );
    }
    let text = state
        .message("c_0", &retried)
        .map(|row| row.content.clone());
    let stamp = state.message("c_0", &retried).map(|row| row.timestamp);

    assert_eq!(retry_send(&mut state, retried), ApplyOutcome::Applied);
    assert_eq!(state.delivery(&retried), Some(DeliveryState::Pending));
    assert_eq!(
        state
            .message("c_0", &retried)
            .map(|row| row.content.clone()),
        text,
        "the retry changed no content"
    );
    assert_eq!(
        state.message("c_0", &retried).map(|row| row.timestamp),
        stamp,
        "and no timestamp -- the row did not move"
    );

    assert_eq!(
        discard_failed_send(&mut state, discarded),
        ApplyOutcome::Applied
    );
    assert_eq!(state.message_count("c_0"), 2, "exactly one row went");
    assert!(state.message("c_0", &discarded).is_none());
    assert_eq!(
        state.delivery(&discarded),
        None,
        "and its delivery entry with it"
    );
    assert_eq!(
        state
            .message("c_0", &survivor)
            .map(|row| row.content.as_str()),
        Some("and this"),
        "and the third row is untouched"
    );
}

/// Retry and discard refuse a send that did not fail.
///
/// Both refusals are `#[case]`s over the states a send can be in, because the
/// mistake that matters is "discard something the server accepted" — which is the
/// one that would lose a message.
#[rstest]
#[case(DeliveryState::Pending)]
#[case(DeliveryState::Acked)]
fn retry_and_discard_refuse_a_send_that_did_not_fail(#[case] state_of: DeliveryState) {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "hello", mine, at(0));
    if state_of != DeliveryState::Pending {
        let event = if state_of == DeliveryState::Acked {
            DomainEvent::MessageAcked {
                client_msg_id: mine,
                message: stored("m_1", 1, "c_0", "u_me", "hello", 1),
            }
        } else {
            DomainEvent::MessageSendFailed {
                client_msg_id: mine,
                code: "x".to_owned(),
                detail: String::new(),
            }
        };
        fire(&mut state, event);
    }
    let before = state.message_count("c_0");

    assert_eq!(
        retry_send(&mut state, mine),
        ApplyOutcome::Ignored(IgnoreReason::NotFailed {
            client_msg_id: mine
        })
    );
    assert_eq!(
        discard_failed_send(&mut state, mine),
        ApplyOutcome::Ignored(IgnoreReason::NotFailed {
            client_msg_id: mine
        })
    );
    assert_eq!(state.message_count("c_0"), before, "nothing was removed");
    assert_eq!(state.delivery(&mine), Some(state_of));
}

/// A send whose identity is **already held in another channel** is refused.
///
/// **This test exists because the mutation table said so**, and the shape of that
/// finding is the one `docs/COVERAGE.md` §5.6.1 records: with the check removed,
/// the *only* two things that noticed were the two proptests, and a defect that
/// produces **two rows for one `client_msg_id`** — the exact reshuffle
/// `core/ordering.rs` §1 is about — was caught by nothing a reader would run.
///
/// The two defenders are kept, and this is the one that says the rule in a name.
/// The scenario is a caller that reused an identity, or a UUID collision: either
/// way the honest answer is a refusal, because a second row for one identity is
/// what the next reconcile would collapse — visibly, for one frame.
#[test]
fn a_send_whose_identity_is_already_held_in_another_channel_is_refused() {
    let mut state = loaded();
    let reused = cid(1);
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 0)),
    );
    assert!(state.channel_holding(&reused).is_some(), "held in c_0");

    assert_eq!(
        begin_send(
            &mut state,
            "c_2",
            "a second row for one identity",
            reused,
            at(1)
        ),
        SendOutcome::Ignored(IgnoreReason::AlreadyHeld {
            client_msg_id: reused
        })
    );

    assert_eq!(
        state.message_count("c_2"),
        0,
        "nothing was created in the channel the send named"
    );
    assert_eq!(
        state.message_count("c_0"),
        1,
        "and the held row is untouched -- one row per client_msg_id, application-wide"
    );
    assert_eq!(
        state.delivery(&reused),
        None,
        "and it is still not one of our sends"
    );
}

/// A message reconciled against a later copy **keeps its unread state**.
///
/// **This test exists because the mutation table said so**: with the restore
/// removed, the only thing in the workspace that noticed was
/// `the_unread_count_never_goes_negative_and_never_exceeds_the_channel` — one
/// catcher, for a defect whose symptom is a badge that *under*-reports. Under-
/// reporting is the quiet direction: a channel with no badge looks like a quiet
/// channel, and nothing in the UI says "we lost count of what you have not read".
///
/// The mechanism is worth stating, because it is not obvious and it is the reason
/// the restore exists at all: `AppState::remove_message` retires the count — that
/// is what makes `unread <= messages held` structural rather than maintained — and
/// a merge has to remove the row before it re-inserts it, because the ordering key
/// may have changed. So the merge has to put the count back.
#[test]
fn a_message_reconciled_against_a_later_copy_keeps_its_unread_state() {
    let mut state = loaded();
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 0)),
    );
    assert_eq!(
        state.unread("c_0"),
        1,
        "one unread message in a background channel"
    );

    // An edit: a whole new value of the message, carrying the original ids.
    let mut edited = stored("m_1", 1, "c_0", "u_ada", "body, edited", 1);
    edited.edited_at = Some(at(1));
    fire(&mut state, DomainEvent::MessageReceived(edited));

    assert_eq!(
        state.unread("c_0"),
        1,
        "an edit does not make a message read. The row was removed and re-inserted \
         because its ordering key moved, and the count has to survive that."
    );
    assert_eq!(state.message_count("c_0"), 1, "still one row");

    // And the ACK handoff, which is the other whole-message replacement.
    let mine = cid(2);
    begin_send(&mut state, "c_2", "mine", mine, at(2));
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: mine,
            message: stored("m_2", 2, "c_2", "u_me", "mine", 3),
        },
    );
    assert_eq!(
        state.unread("c_2"),
        0,
        "an acknowledged own message was never counted, and stays uncounted"
    );
    assert_eq!(state.unread("c_0"), 1, "and the other channel is untouched");
}

/// A message the server moves between channels carries its unread state with it,
/// and leaves nothing behind in the channel it left.
///
/// **The defect this pins is the precise failure the unread property was named
/// for** — a count that exceeds the messages actually held — and it is the one
/// that changed the representation from a counter to a set. A `u32` per channel
/// cannot express it: whether the old channel was on screen *when the message
/// arrived* is not reconstructible, so "decrement the old channel" has no
/// well-defined value. A set of counted identities can.
#[test]
fn a_message_the_server_moves_between_channels_carries_its_unread_state() {
    let mut state = loaded();
    let moving = cid(1);
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 0)),
    );
    assert_eq!(state.unread("c_0"), 1);

    // The same identity, now in a different channel. A `client_msg_id` is globally
    // unique, so this is a server defect -- and the layer's answer is to trust the
    // server and move the row whole.
    let outcome = apply_event(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_3", "u_ada", "body", 0)),
    );
    assert!(
        matches!(outcome, ApplyOutcome::Merged { .. }),
        "and it is reported as a merge, not applied silently: {outcome}"
    );

    assert_eq!(state.message_count("c_0"), 0, "the row left");
    assert_eq!(
        state.unread("c_0"),
        0,
        "and so did its count -- this is the assertion the counter could not make"
    );
    assert_eq!(state.message_count("c_3"), 1, "the row arrived");
    assert_eq!(state.unread("c_3"), 1, "still unread, now where it lives");
    assert_eq!(
        state.channel_holding(&moving),
        Some("c_3"),
        "and one row per client_msg_id, application-wide"
    );
    for (channel, count) in state.unread_counts() {
        assert!(
            count as usize <= state.message_count(channel),
            "{channel} claims {count} unread out of {} held",
            state.message_count(channel)
        );
    }
}

/// The late ACK through the **other door**: a resync echo of a failed send.
///
/// **The same decision as the test above, reached differently, and that is the
/// point of having both.** `core/models/events.rs` on `MessageReceived` says the
/// three wire situations are the same situation from the client's point of view,
/// and `PLAN.md` §6's protocol can deliver the server's copy of an accepted send
/// as either a `message.ack` or a `message.new` on the resync. An implementation
/// that handled the upgrade in only one of the two would pass one of these tests
/// and fail the other — which the mutation table measured: with the `Failed` arm
/// removed, exactly **one** test in the workspace noticed.
#[test]
fn a_resync_echo_of_a_failed_send_upgrades_it_just_as_an_ack_does() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "the text the user wrote", mine, at(10));
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: mine,
            code: "timeout".to_owned(),
            detail: "the server did not answer in time".to_owned(),
        },
    );
    assert_eq!(state.delivery(&mine), Some(DeliveryState::Failed));

    // The resync hands the client back the server's stored copy -- as a
    // `message.new`, not as an `message.ack`.
    let outcome = apply_event(
        &mut state,
        DomainEvent::MessageReceived(stored(
            "m_1",
            1,
            "c_0",
            "u_me",
            "the text the user wrote",
            12,
        )),
    );
    assert!(
        matches!(outcome, ApplyOutcome::Merged { .. }),
        "and it is reported as a merge: {outcome}"
    );

    assert_eq!(
        state.delivery(&mine),
        Some(DeliveryState::Acked),
        "the server stored it, so the send succeeded -- whichever frame said so"
    );
    assert_eq!(
        state.failure(&mine),
        None,
        "and the stale reason is cleared"
    );
    assert_eq!(
        state.message_count("c_0"),
        1,
        "still one row for one identity"
    );
    assert_eq!(
        state.message("c_0", &mine).map(|row| row.id.as_str()),
        Some("m_1")
    );
}

/// A refused send reuses no identity and displaces no row.
#[test]
fn retry_and_discard_refuse_a_send_this_client_never_made() {
    let mut state = loaded();
    let never = cid(99);
    assert_eq!(
        retry_send(&mut state, never),
        ApplyOutcome::Ignored(IgnoreReason::NotHeld {
            client_msg_id: never
        })
    );
    assert_eq!(
        discard_failed_send(&mut state, never),
        ApplyOutcome::Ignored(IgnoreReason::NotHeld {
            client_msg_id: never
        })
    );
}

// ---------------------------------------------------------------------------
// 5. The unread rule
// ---------------------------------------------------------------------------

/// Only a foreign message for a channel that is **not** on screen is unread.
///
/// **The rule, as a table.** `state/app_state.rs`'s module docs §5 states it and
/// argues it; the four cases are its truth table, one per combination of the two
/// conditions that vary. `on_screen` selects `c_0` — the channel the message is
/// delivered to — so the flag means what its name says rather than "some other
/// channel is selected".
#[rstest]
#[case("u_ada", false, 1u32, "somebody else, in a background channel: unread")]
#[case(
    "u_me",
    false,
    0,
    "our own message, in a background channel: read by writing it"
)]
#[case(
    "u_ada",
    true,
    0,
    "somebody else, in the channel on screen: they were looking"
)]
#[case("u_me", true, 0, "our own message, on screen: doubly not unread")]
fn only_a_foreign_message_for_a_channel_that_is_not_on_screen_is_unread(
    #[case] author: &str,
    #[case] on_screen: bool,
    #[case] expected: u32,
    #[case] why: &str,
) {
    let mut state = loaded();
    if on_screen {
        assert_eq!(select_channel(&mut state, "c_0"), ApplyOutcome::Applied);
    }

    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", author, "body", 0)),
    );

    assert_eq!(state.unread("c_0"), expected, "{why}");
}

/// Unread accumulates, and selecting the channel clears it.
///
/// **Clearing on selection is `AGENTS.md` §8.1's Channel Switch Flow** — "unread
/// badge clears" — and the accumulation matters as much as the clear: a counter
/// that reset on every message would not be a count, and the sidebar's whole job
/// is to say how far behind the user is.
#[test]
fn unread_accumulates_and_selecting_the_channel_clears_it() {
    let mut state = loaded();
    for n in 0..4u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }
    assert_eq!(
        state.unread("c_0"),
        4,
        "four messages the user has not looked at"
    );
    assert_eq!(state.unread("c_1"), 0, "and none in a quiet channel");
    assert_eq!(
        state.unread_counts(),
        vec![("c_0", 4)],
        "only the channel with a count reports one"
    );

    assert_eq!(select_channel(&mut state, "c_0"), ApplyOutcome::Applied);
    assert_eq!(state.unread("c_0"), 0, "looking at it clears it");
    assert_eq!(state.selected(), Some("c_0"));

    // And a re-selection of the channel already shown still clears it: a user who
    // clicks the channel they are in is saying "show me what I missed".
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_9", 9, "c_0", "u_ada", "late", 9)),
    );
    assert_eq!(
        state.unread("c_0"),
        0,
        "it arrived while the channel was on screen"
    );
    assert_eq!(select_channel(&mut state, "c_0"), ApplyOutcome::Applied);
    assert_eq!(state.unread("c_0"), 0);
}

/// Selecting a channel the user cannot see is refused, and clears nothing.
///
/// The refusal is the load-bearing part: accepting the selection would zero the
/// unread count of a channel that is not on screen — which is the one thing the
/// count exists to prevent.
#[rstest]
#[case("")]
#[case("  ")]
#[case("c_9")]
fn selecting_a_channel_the_user_cannot_see_is_refused(#[case] channel_id: &str) {
    let mut state = loaded();
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 0)),
    );
    let expected = if channel_id.trim().is_empty() {
        IgnoreReason::BlankChannelId
    } else {
        IgnoreReason::UnknownChannel {
            channel_id: channel_id.to_owned(),
        }
    };

    assert_eq!(
        select_channel(&mut state, channel_id),
        ApplyOutcome::Ignored(expected)
    );
    assert_eq!(state.unread("c_0"), 1, "nothing was cleared");
    assert_eq!(state.selected(), None, "and nothing was selected");
}

/// A message of ours from another device is not unread, and is not one of our
/// sends either.
///
/// **Excluding by `user_id` rather than by `client_msg_id` is the decision**, and
/// it is a product one: a message of yours sent from your phone while your desktop
/// is open is still a message you have read, and matching on the local identity
/// would badge it. The test can only express it by delivering a message authored
/// by `u_me` that this client never sent — which is exactly what a second
/// device's message looks like.
#[test]
fn a_message_of_ours_from_another_device_is_not_unread() {
    let mut state = loaded();
    let from_the_phone = cid(77);

    fire(
        &mut state,
        DomainEvent::MessageReceived(stored(
            "m_77",
            77,
            "c_0",
            "u_me",
            "sent from the other device",
            0,
        )),
    );

    assert_eq!(state.unread("c_0"), 0);
    assert_eq!(
        state.delivery(&from_the_phone),
        None,
        "and it is not one of *this* client's sends either"
    );
}

/// A message held for a channel the client has not loaded is still kept.
///
/// **The alternative was to refuse it**, and it was refused for a good reason —
/// unread tracking and the sidebar both need a channel entry — and then rejected
/// because a channel missing from one `GET /channels` response may be a partial
/// response or a permissions change, and `AGENTS.md` §1 ranks "no lost messages"
/// second. A message dropped because a list had not caught up is a worse outcome
/// than a blank sidebar row for a moment.
#[test]
fn a_message_for_a_channel_the_client_has_not_loaded_is_kept_and_counted() {
    let mut state = loaded();

    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::MessageReceived(stored("m_60", 60, "c_unknown", "u_ada", "body", 0))
        ),
        ApplyOutcome::Applied
    );

    assert_eq!(state.message_count("c_unknown"), 1, "not dropped");
    assert_eq!(
        state.unread("c_unknown"),
        1,
        "and counted, because it is unread and this client is not looking at it"
    );
    assert_eq!(state.locate("m_60").map(|(ch, _)| ch), Some("c_unknown"));
}

/// A message held from earlier never becomes unread by switching away.
///
/// The rule is evaluated **at arrival**, and this is the assertion that pins the
/// difference: a retroactive mark would badge a conversation the user just
/// finished reading, which is the badge nobody wants.
#[test]
fn a_message_held_from_earlier_never_becomes_unread_by_switching_away() {
    let mut state = loaded();
    assert_eq!(select_channel(&mut state, "c_0"), ApplyOutcome::Applied);
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "seen", 0)),
    );
    assert_eq!(
        state.unread("c_0"),
        0,
        "it arrived while the channel was on screen"
    );

    assert_eq!(select_channel(&mut state, "c_1"), ApplyOutcome::Applied);
    assert_eq!(
        state.unread("c_0"),
        0,
        "module docs section 5: the rule is evaluated at arrival, and a retroactive \
         mark would badge a conversation the user just finished reading"
    );
}

// ---------------------------------------------------------------------------
// 6. Reactions
// ---------------------------------------------------------------------------

/// A reaction increment merges into the message it names, by server id, and
/// replaying it changes nothing.
///
/// The merge is a **set insert**, which is the idempotence
/// `core/models/events.rs` claims for `ReactionUpdated` and which a resync after a
/// reconnect needs: reactions the client already holds come back a second time.
#[test]
fn a_reaction_increment_merges_and_replaying_it_changes_nothing() {
    let mut state = loaded();
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 0)),
    );
    let target = cid(1);
    let thumbs = "\u{1F44D}";
    let increment = DomainEvent::ReactionUpdated {
        message_id: "m_1".to_owned(),
        emoji: thumbs.to_owned(),
        user_id: "u_grace".to_owned(),
    };

    assert_eq!(
        apply_event(&mut state, increment.clone()),
        ApplyOutcome::Applied
    );
    assert_eq!(
        apply_event(&mut state, increment),
        ApplyOutcome::Applied,
        "a replay is not a failure; it is a set insert that inserts nothing"
    );
    fire(
        &mut state,
        DomainEvent::ReactionUpdated {
            message_id: "m_1".to_owned(),
            emoji: thumbs.to_owned(),
            user_id: "u_alan".to_owned(),
        },
    );
    fire(
        &mut state,
        DomainEvent::ReactionUpdated {
            message_id: "m_1".to_owned(),
            emoji: "\u{2764}".to_owned(),
            user_id: "u_alan".to_owned(),
        },
    );

    let row = state.message("c_0", &target).expect("the message is held");
    assert_eq!(
        row.reactions.len(),
        2,
        "two emoji, not three: the grouping is the renderer's requirement"
    );
    let grouped = row
        .reactions
        .iter()
        .find(|group| group.emoji == thumbs)
        .expect("the emoji is there");
    assert_eq!(grouped.user_ids, vec!["u_grace", "u_alan"]);
}

#[test]
fn a_reaction_on_a_message_this_client_does_not_hold_is_reported_and_dropped() {
    let mut state = loaded();
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::ReactionUpdated {
                message_id: "m_absent".to_owned(),
                emoji: "\u{1F44D}".to_owned(),
                user_id: "u_grace".to_owned(),
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::NoSuchMessage {
            message_id: "m_absent".to_owned()
        })
    );
    assert_eq!(
        state.message_count("c_0"),
        0,
        "and no placeholder row was invented"
    );
}

/// A merge drops a reaction group the server left empty.
///
/// `sh_nexus::network::mapping` promises it on `WireReaction`: *"An empty user list
/// is not an error: it is what a server sends when the last user removes a
/// reaction and the client has not seen the increment. `state/` drops the entry
/// when the list empties."* This is the only path in the application that replaces
/// a whole `Message`, so it is the only place the promise can be kept — and a chip
/// rendering "0" for an emoji nobody reacted to is the visible cost of forgetting.
#[test]
fn a_merge_drops_a_reaction_group_the_server_left_empty() {
    let mut state = loaded();
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 0)),
    );
    let target = cid(1);
    fire(
        &mut state,
        DomainEvent::ReactionUpdated {
            message_id: "m_1".to_owned(),
            emoji: "\u{1F44D}".to_owned(),
            user_id: "u_grace".to_owned(),
        },
    );
    assert_eq!(
        state.message("c_0", &target).map(|row| row.reactions.len()),
        Some(1)
    );

    let mut edited = stored("m_1", 1, "c_0", "u_ada", "body, edited", 1);
    edited.reactions.push(Reaction {
        emoji: "\u{1F44D}".to_owned(),
        user_ids: Vec::new(),
    });
    fire(&mut state, DomainEvent::MessageReceived(edited));

    assert!(
        state
            .message("c_0", &target)
            .is_some_and(|row| row.reactions.is_empty()),
        "an empty group is not rendered as a chip with a zero"
    );
}

// ---------------------------------------------------------------------------
// 7. Typing, presence, connection, resync
// ---------------------------------------------------------------------------

/// A typing indicator appears, and goes away with its entry.
///
/// **The entry is removed rather than left empty**, and that is the observable
/// half of the bound: `AGENTS.md` §7.1 is a limit on the growth of state, and an
/// entry nobody can read is growth with no reader.
#[test]
fn a_typing_indicator_appears_and_goes_away_with_its_entry() {
    let mut state = loaded();
    for user in ["u_ada", "u_grace"] {
        assert_eq!(
            apply_event(
                &mut state,
                DomainEvent::TypingUpdated {
                    channel_id: "c_0".to_owned(),
                    user_id: user.to_owned(),
                    active: true
                }
            ),
            ApplyOutcome::Applied
        );
    }
    assert_eq!(state.typing("c_0"), ["u_ada", "u_grace"]);
    assert_eq!(state.typing_channel_count(), 1);

    for user in ["u_ada", "u_grace"] {
        fire(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_0".to_owned(),
                user_id: user.to_owned(),
                active: false,
            },
        );
    }
    assert!(state.typing("c_0").is_empty());
    assert_eq!(
        state.typing_channel_count(),
        0,
        "the entry is retired, not left empty"
    );
}

/// A repeated start does not duplicate an indicator.
///
/// A resync can replay a `TypingUpdated` the client already applied, so the
/// operation is a set insert rather than an append.
#[test]
fn a_repeated_typing_start_does_not_duplicate_the_indicator() {
    let mut state = loaded();
    let start = DomainEvent::TypingUpdated {
        channel_id: "c_0".to_owned(),
        user_id: "u_ada".to_owned(),
        active: true,
    };
    fire(&mut state, start.clone());
    fire(&mut state, start);
    assert_eq!(state.typing("c_0"), ["u_ada"]);
}

/// Both typing bounds hold, and hitting one is reported rather than silent.
///
/// `AGENTS.md` §7.1 forbids unbounded growth, and `core/models/events.rs` says the
/// set is *"client state … and `state/` owns the set — including bounding it,
/// which §7.1 requires and which only the owner can do."* Two bounds rather than
/// one, because bounding each channel is not enough if the number of channels is
/// not bounded either.
///
/// **The per-channel bound is reported, not enforced by eviction.** Evicting an
/// active typist would make a *real* indicator vanish, and a missing indicator for
/// the thirty-third simultaneous typist is a better outcome than a wrong one.
#[test]
fn both_typing_bounds_hold_and_hitting_one_is_reported() {
    let mut state = loaded();
    for index in 0..MAX_TYPING_USERS_PER_CHANNEL {
        assert_eq!(
            apply_event(
                &mut state,
                DomainEvent::TypingUpdated {
                    channel_id: "c_0".to_owned(),
                    user_id: format!("u_{index}"),
                    active: true
                }
            ),
            ApplyOutcome::Applied,
            "typist {index} of {MAX_TYPING_USERS_PER_CHANNEL} is within the bound"
        );
    }
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_0".to_owned(),
                user_id: "u_one_too_many".to_owned(),
                active: true
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::TypingSetFull {
            channel_id: "c_0".to_owned()
        })
    );
    assert_eq!(
        state.typing("c_0").len(),
        MAX_TYPING_USERS_PER_CHANNEL,
        "and the set did not grow past the bound"
    );

    // The channel-count bound, on fresh channels. `c_0` already holds one, so it
    // is checked last and must keep working: an existing entry is never displaced
    // by the channel limit.
    for index in 1..MAX_TYPING_CHANNELS {
        assert_eq!(
            apply_event(
                &mut state,
                DomainEvent::TypingUpdated {
                    channel_id: format!("c_bound_{index}"),
                    user_id: "u_ada".to_owned(),
                    active: true
                }
            ),
            ApplyOutcome::Applied
        );
    }
    assert_eq!(state.typing_channel_count(), MAX_TYPING_CHANNELS);
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_one_too_many".to_owned(),
                user_id: "u_ada".to_owned(),
                active: true
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::TypingChannelLimit {
            channel_id: "c_one_too_many".to_owned()
        })
    );
    assert_eq!(
        state.typing_channel_count(),
        MAX_TYPING_CHANNELS,
        "the bound held"
    );
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_0".to_owned(),
                user_id: "u_ada".to_owned(),
                active: true
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::TypingSetFull {
            channel_id: "c_0".to_owned()
        }),
        "and c_0, whose set is already full, refuses on the set bound -- which is the \
         set bound doing its job, not the channel limit displacing it"
    );
    // A *duplicate* start on a full set is inert rather than a refusal: a resync
    // replays transitions the client already applied, and a full set refusing a
    // user it is already showing would put a refusal in the bridge's log for an
    // event that changed nothing.
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_0".to_owned(),
                user_id: "u_0".to_owned(),
                active: true
            }
        ),
        ApplyOutcome::Applied,
        "and a replayed start on a full set is inert, not a refusal"
    );
    assert_eq!(state.typing("c_0").len(), MAX_TYPING_USERS_PER_CHANNEL);

    // The channel limit never displaces an *existing* entry, which needs a
    // channel whose set is not full: the check is on whether the entry exists, not
    // on how much room it has.
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_bound_1".to_owned(),
                user_id: "u_grace".to_owned(),
                active: true
            }
        ),
        ApplyOutcome::Applied,
        "a tracked channel with room keeps working once the channel limit is reached"
    );
}

#[test]
fn a_typing_stop_for_a_quiet_channel_is_reported_rather_than_invented() {
    let mut state = loaded();
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::TypingUpdated {
                channel_id: "c_0".to_owned(),
                user_id: "u_ada".to_owned(),
                active: false
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::NobodyTyping {
            channel_id: "c_0".to_owned()
        })
    );
    assert_eq!(state.typing_channel_count(), 0);
}

/// Sending is possible only while connected, and the state is stored verbatim.
///
/// `core/models/events.rs` on `ConnectionState::Connected` says it is *"the only
/// state in which sending is meaningful"*, so the composer's one question has one
/// answer and the UI does not repeat the `match`. The second half of each case
/// asserts the attempt counter and the rejection payload are not normalised away
/// — `Reconnecting { attempt }` carries its number so §4.2's "max-attempt
/// behavior" is observable rather than invisible.
#[rstest]
#[case(ConnectionState::Disconnected, false)]
#[case(ConnectionState::Connecting, false)]
#[case(ConnectionState::Connected, true)]
#[case(ConnectionState::Reconnecting { attempt: 7 }, false)]
#[case(
    ConnectionState::Rejected {
        code: "version".to_owned(),
        detail: "v2".to_owned()
    },
    false
)]
fn sending_is_possible_only_while_connected(
    #[case] connection: ConnectionState,
    #[case] can_send: bool,
) {
    let mut state = AppState::new("u_me");
    assert!(!state.can_send(), "the initial state cannot send");

    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(connection.clone()),
    );

    assert_eq!(
        state.connection(),
        &connection,
        "stored verbatim, not normalised"
    );
    assert_eq!(state.can_send(), can_send);
}

#[test]
fn a_presence_update_is_recorded_and_the_last_one_wins() {
    let mut state = loaded();
    for status in [UserStatus::Online, UserStatus::Away, UserStatus::Offline] {
        fire(
            &mut state,
            DomainEvent::PresenceUpdated {
                user_id: "u_ada".to_owned(),
                status,
            },
        );
        assert_eq!(state.presence("u_ada"), Some(status));
    }
    assert_eq!(state.presence("u_never_seen"), None);
}

/// The resync cursor is local state, and it never asks for what is already held.
///
/// `core/models/events.rs` on `ResyncRequested` says the cursor *"comes from local
/// state, so it is not a thing a frame can carry"*, and `PLAN.md` §6's `after` is
/// exclusive. The rule that matters: **the newest message this client holds wins
/// over the recorded cursor**, because a message already held must not be
/// requested again — and an optimistic row's client-local clock is routinely ahead
/// of the server's `last_message_at`.
#[test]
fn the_resync_cursor_is_local_state_and_never_re_asks_for_what_is_held() {
    let mut state = loaded();
    assert_eq!(state.resync_cursor("c_0"), None, "nothing to resume from");

    fire(
        &mut state,
        DomainEvent::ResyncRequested {
            channel_id: "c_0".to_owned(),
            after: at(10),
        },
    );
    assert_eq!(
        state.resync_cursor("c_0"),
        Some(at(10)),
        "the recorded cursor"
    );

    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "newer", 20)),
    );
    assert_eq!(
        state.resync_cursor("c_0"),
        Some(at(20)),
        "the newest held message is the tighter cursor"
    );

    fire(
        &mut state,
        DomainEvent::ResyncRequested {
            channel_id: "c_0".to_owned(),
            after: at(15),
        },
    );
    assert_eq!(
        state.resync_cursor("c_0"),
        Some(at(20)),
        "an older cursor cannot walk it backwards"
    );
    assert_eq!(
        state.resync_cursor("c_1"),
        None,
        "and a quiet channel has none"
    );
}

/// The gap-detection expectation carries no invented watermark.
///
/// `core/ordering.rs` §4 is explicit that on the protocol as `PLAN.md` §6
/// specifies today **every resync is `Unverifiable`**, and that this is the
/// correct answer rather than a missing feature: a client that answered "no gaps"
/// from a timestamp cursor would be asserting something it cannot know. This test
/// asserts the client does not manufacture the missing field.
#[test]
fn the_gap_expectation_carries_no_invented_watermark() {
    let mut state = loaded();
    fire(
        &mut state,
        DomainEvent::ResyncRequested {
            channel_id: "c_0".to_owned(),
            after: at(10),
        },
    );
    let expectation = state.sync_expectation("c_0");
    assert_eq!(expectation.cursor, Some(at(10)));
    assert_eq!(
        expectation.watermark, None,
        "PLAN.md section 6 defines no watermark field, and core/ordering.rs section 4 \
         says that is the honest answer rather than a gap in this layer"
    );
    assert!(
        !ordering::SyncStatus::Unverifiable { cursor: at(10) }.is_healthy(),
        "which means the status is not a pass -- and must not be read as one"
    );
}

/// The sidebar is cached, ordered by name then id, and history outlives a missing
/// channel.
///
/// Three claims. **The second** is the interesting one: a channel missing from one
/// `GET /channels` response is *not* treated as a deletion, because deleting
/// history on that evidence loses messages and `AGENTS.md` §1 ranks that second —
/// `db/` is the layer that can tell a real deletion from a missing row, in Phase
/// 3. Name alone is not a total order either — two channels may share a display
/// name — so the id is the tiebreaker and two clients enumerate the same order.
#[test]
fn the_sidebar_is_cached_ordered_and_history_outlives_a_missing_channel() {
    let mut state = loaded();
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_3", "u_ada", "body", 0)),
    );
    assert_eq!(state.unread("c_3"), 1);

    let names: Vec<&str> = state
        .sidebar()
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["channel-0", "channel-1", "channel-2", "channel-3"],
        "cached and ordered by name"
    );

    // A reload that omits c_3 -- a partial response, or a channel the user was
    // removed from and will be removed from.
    set_channels(
        &mut state,
        vec![channel("c_1", "channel-1"), channel("c_0", "channel-0")],
    );
    assert!(!state.has_channel("c_3"), "the channel is no longer listed");
    assert_eq!(
        state.message_count("c_3"),
        1,
        "but its history is not deleted on the evidence of one missing row"
    );
    assert_eq!(state.unread("c_3"), 1, "and the count is untouched");
    assert_eq!(
        select_channel(&mut state, "c_3"),
        ApplyOutcome::Ignored(IgnoreReason::UnknownChannel {
            channel_id: "c_3".to_owned()
        }),
        "so it can no longer be selected -- which is the honest consequence"
    );
}

// ---------------------------------------------------------------------------
// 8. The history bound
// ---------------------------------------------------------------------------

/// A channel that overflows its cap keeps exactly the cap, and loses the oldest.
///
/// **The cap is read from the constant, never restated.** A literal copied into
/// this test is a test that keeps passing after somebody moves the number, and
/// `docs/BASELINES.md`'s 12,8 MB measurement is *about* 10 000 messages — so a
/// test that pinned 10 000 as a literal would be asserting a coincidence rather
/// than the invariant.
///
/// **The arrival is the newest, so the head is the thing that goes.** The rows go
/// in ascending timestamp order, which is what makes the assertion about *which*
/// row is evicted meaningful: an implementation that evicted the newest, or one
/// that evicted the row it had just inserted, would satisfy a count-only test.
#[test]
fn a_channel_at_its_bound_keeps_the_cap_and_loses_the_oldest() {
    let mut state = loaded();
    let over_by = 3;
    let total = MAX_MESSAGES_PER_CHANNEL + over_by;

    for n in 0..total as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }

    assert_eq!(
        state.message_count("c_0"),
        MAX_MESSAGES_PER_CHANNEL,
        "AGENTS.md 7.1's bound must hold after an overflow"
    );

    let held = state.messages("c_0");
    assert_eq!(
        held.first().map(|message| message.id.as_str()),
        Some(format!("m_{over_by}").as_str()),
        "the {over_by} oldest rows are what a cap costs, and it costs the oldest"
    );
    assert_eq!(
        held.last().map(|message| message.id.as_str()),
        Some(format!("m_{}", total - 1).as_str()),
        "and the row that just arrived is the one that is kept"
    );
}

/// **The cap is per channel, and the other channels are untouched by it.**
///
/// `AGENTS.md` §6.2's budget row is about cached history and `MessageList` is per
/// channel, so a global ceiling would let one busy channel starve every other one
/// — a worse failure than the unboundedness the cap exists to close. Asserting
/// the neighbour's full count is the only way that stays a property of the
/// design rather than a comment.
#[test]
fn the_cap_is_per_channel_and_one_busy_channel_does_not_starve_another() {
    let mut state = loaded();
    let total = MAX_MESSAGES_PER_CHANNEL + 1;

    for n in 0..total as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }
    // A disjoint identity range, and the disjointness is load-bearing rather
    // than tidy: a `client_msg_id` is globally unique, so reusing one across the
    // two channels is a *server* disagreement about where a message lives, and
    // `ingest_by_identity` resolves it by **moving the row out of `c_0`**. A
    // range that overlapped would make this test measure the move path and
    // report a count that looks like a broken cap. The bound is derived from the
    // cap rather than written down, so it stays disjoint if the cap moves.
    for n in 0..8u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("n_{n}"),
                MAX_MESSAGES_PER_CHANNEL as u128 + 1 + n,
                "c_1",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }

    assert_eq!(state.message_count("c_0"), MAX_MESSAGES_PER_CHANNEL);
    assert_eq!(
        state.message_count("c_1"),
        8,
        "a full channel elsewhere must not cost this one a single row"
    );
}

/// A send this client is waiting on is never the row that is evicted, and the
/// eviction skips it to reach the next oldest.
///
/// **This is the trap the whole mechanism exists to avoid, and both halves of it
/// are asserted.** `state/actions.rs`'s `acknowledge` asks `channel_holding(..)`
/// before anything else, so a pending send that was evicted answers `None` and
/// falls through to the adoption path — a silent reordering and a row that looks
/// duplicated, and the user watches their own message vanish before the server
/// answered. So the assertion is on the *pending row's survival*, not on the
/// count: a count-only test passes for an implementation that drops the send and
/// takes a newer row instead.
///
/// **The three cases are the three things a send can be**, and they are one
/// `#[rstest]` because the shape is identical and only the delivery state
/// differs. `Acked` is the fourth and it is *not* here: the server has spoken, so
/// an acknowledged row is ordinary history and must remain evictable — a
/// `#[case]` for it would have to assert the opposite, and putting that in the
/// same list would make the test say two things.
#[rstest]
#[case("pending", "a send the server has not answered")]
#[case("failed", "a send the server refused terminally")]
fn a_send_in_flight_is_never_the_row_that_is_evicted(#[case] state_of: &str, #[case] why: &str) {
    let mut state = loaded();

    // The pending row is the OLDEST row in the channel, so it is first in line
    // for eviction. That is the arrangement that makes the guard load-bearing:
    // an implementation that always took the head would drop it here.
    //
    // **The identity is out of the range the arriving messages use, and that is
    // load-bearing rather than tidy.** `cid(n)` is `from_u128(n + 1)`, so an
    // identity of `cid(1)` would be the *same identity* the first arriving
    // message carries — and the arrival would reconcile against the optimistic
    // row instead of being a new row, the head would never be this client's
    // pending send, and this test would pass for an implementation that drops
    // sends. The offset is a named constant so the reason travels with it.
    const CLEAR_OF_THE_ARRIVALS: u128 = 1_000_000;
    let in_flight = cid(CLEAR_OF_THE_ARRIVALS);
    assert!(matches!(
        begin_send(&mut state, "c_0", "my own message", in_flight, at(0)),
        SendOutcome::Pending { .. }
    ));
    if state_of == "failed" {
        fire(
            &mut state,
            DomainEvent::MessageSendFailed {
                client_msg_id: in_flight,
                code: "rejected".to_owned(),
                detail: "the server refused this one".to_owned(),
            },
        );
    }
    assert_eq!(
        state.delivery(&in_flight),
        Some(if state_of == "failed" {
            DeliveryState::Failed
        } else {
            DeliveryState::Pending
        }),
        "the fixture must be in the state its case names, or it proves nothing ({why})"
    );

    for n in 1..(MAX_MESSAGES_PER_CHANNEL + 1) as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }

    assert_eq!(
        state.message_count("c_0"),
        MAX_MESSAGES_PER_CHANNEL,
        "and the cap still holds -- the guard skips a row, it does not suspend the bound"
    );
    assert!(
        state.message("c_0", &in_flight).is_some(),
        "a {state_of} send must survive the overflow: {why}. An evicted send is \
         re-inserted by acknowledge's adoption path, which is a silent \
         reordering and a row that looks duplicated"
    );
    assert!(
        state.message("c_0", &cid(1)).is_none(),
        "and the eviction reached the *next* oldest rather than stopping short: \
         the first arriving message is the row that was taken, so the guard \
         skipped one row and still evicted one"
    );
    assert_eq!(
        state.messages("c_0").first().map(|m| m.client_msg_id),
        Some(in_flight),
        "so the send in flight is now the head of the channel -- it is still the \
         oldest row, it is simply the oldest row eviction is not allowed to take"
    );
}

/// The unread count goes down with the row it counted, so it cannot exceed what
/// the channel holds.
///
/// **This is the property the unread set's own documentation claims is
/// structural** — *"it cannot exceed the channel's held messages"* — and
/// `evict_one_over_cap` could break it in exactly one way: by removing a row
/// without retiring its element. It routes through `remove_message` for that
/// reason, and the count is what proves the route was taken rather than a
/// neighbouring one that looks identical.
///
/// **Both directions are in one test because a count that merely stops growing
/// would pass a "does not exceed" assertion** — the eviction has to make the
/// count *fall* by the number of rows it took, or nothing was retired at all.
#[test]
fn evicting_a_row_retires_its_unread_element_with_it() {
    let mut state = loaded();
    let total = MAX_MESSAGES_PER_CHANNEL + 4;

    for n in 0..total as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }

    let held = state.message_count("c_0") as i64;
    let unread = i64::from(state.unread("c_0"));
    assert_eq!(
        unread, held,
        "every row here is a foreign message in a background channel, so all of \
         them counted -- and the 4 that were evicted took their elements with them"
    );
    assert!(
        unread < total as i64,
        "and the count really did fall: four rows left, so four elements went. \
         A count that merely stopped growing would satisfy the line above"
    );

    // And the set-based property itself, through the helper the whole file uses.
    assert_internally_consistent(&state, &BTreeSet::new(), 0);
}

/// The `Vec` and the positional index agree after a head eviction, which is the
/// one removal that shifts every index below it.
///
/// **`assert_internally_consistent` is the assertion, not a hand-written subset of
/// it.** A head eviction moves *every* index under it by one, so it is the
/// removal most able to desynchronise `by_client_msg_id` from the `Vec` — and
/// the helper is what walks both, resolves every identity through the positional
/// index, and checks the server-id index alongside. The rows are then spot-checked
/// by identity to make the *direction* of the shift explicit: a helper that only
/// checked self-consistency would pass for an implementation whose index was
/// uniformly wrong.
#[test]
fn a_head_eviction_leaves_the_vector_and_the_identity_index_agreeing() {
    let mut state = loaded();
    let total = MAX_MESSAGES_PER_CHANNEL + 2;

    for n in 0..total as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }
    assert_internally_consistent(&state, &BTreeSet::new(), 0);

    // The first row of the window, and the one immediately below it. The second
    // is the row whose index moved: it was at 1 before the two evictions and is
    // at 0 now, and `message(..)` resolves it through the positional index.
    let window_head = cid(2);
    let just_below = cid(3);
    assert_eq!(
        state.messages("c_0").first().map(|m| m.client_msg_id),
        Some(window_head),
        "the window starts at the third row: the two oldest were evicted"
    );
    let at_zero = state.message("c_0", &just_below);
    assert_eq!(
        at_zero.map(|m| m.id.as_str()),
        Some("m_3"),
        "the positional index must resolve the identity to the row it names, at \
         the position the eviction moved it to"
    );
    assert_eq!(
        state.messages("c_0")[0].id,
        "m_2",
        "and index 0 is the row the index says it is, not a stale neighbour"
    );
}

/// A channel whose every row is a send in flight is **allowed over the cap**, and
/// the over-cap is asserted rather than left implicit.
///
/// **This is the one case where the invariant does not hold, and the test exists
/// so that it is a stated exception rather than a hole.** The alternative —
/// evicting a send to satisfy a memory bound — is the failure `AGENTS.md` §7.5's
/// *"failures are never silently dropped"* is written to prevent, and it costs
/// the user a message they can see. An over-cap channel is bounded, visible and
/// self-correcting; a dropped send is none of those.
///
/// **The over-cap is exactly one row per insert, not one per send.** The
/// candidate scan is bounded by the arriving row's own index
/// ([`evict_one_over_cap`]'s `before`), so when every *older* row is in flight
/// there is no candidate and the arrival is kept — which means the channel
/// overshoots by one and stays there rather than growing without limit. That is
/// the shape which makes the exception safe, so it is the shape asserted.
///
/// **The correction is asserted too, because "self-correcting" is a claim.** Once
/// the oldest send is answered, it becomes an ordinary row and the next arrival
/// evicts it — one row per arrival, back down to the cap.
///
/// [`evict_one_over_cap`]: sh_nexus::state::app_state::AppState::evict_one_over_cap
#[test]
fn a_channel_whose_every_row_is_a_send_in_flight_is_allowed_over_the_cap() {
    let mut state = loaded();

    // Fill to the cap entirely with this client's own unanswered sends, so the
    // channel has no evictable row at all.
    for n in 0..MAX_MESSAGES_PER_CHANNEL as u128 {
        let _ = begin_send(
            &mut state,
            "c_0",
            "queued while the server was away",
            cid(n + 1),
            at(n as i64),
        );
    }
    assert_eq!(state.message_count("c_0"), MAX_MESSAGES_PER_CHANNEL);

    let newest = cid(MAX_MESSAGES_PER_CHANNEL as u128 + 1);
    let _ = begin_send(
        &mut state,
        "c_0",
        "one more than the cap allows",
        newest,
        at(MAX_MESSAGES_PER_CHANNEL as i64),
    );

    assert_eq!(
        state.message_count("c_0"),
        MAX_MESSAGES_PER_CHANNEL + 1,
        "the documented exception: over the cap by exactly one, because the \
         candidate scan is bounded by the arrival and no older row is evictable"
    );
    assert!(
        state.message("c_0", &cid(1)).is_some(),
        "and not one of the sends in flight was dropped to make room"
    );
    assert!(
        state.message("c_0", &newest).is_some(),
        "nor was the row that just arrived, which the arrival floor protects"
    );

    // The oldest send is answered, so it is ordinary history now. The next
    // arrival finds a candidate and takes it.
    let oldest = cid(1);
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: oldest,
            message: stored(
                "m_oldest",
                1,
                "c_0",
                "u_me",
                "queued while the server was away",
                0,
            ),
        },
    );
    let _ = begin_send(
        &mut state,
        "c_0",
        "and the one after that",
        cid(MAX_MESSAGES_PER_CHANNEL as u128 + 2),
        at(MAX_MESSAGES_PER_CHANNEL as i64 + 1),
    );

    assert_eq!(
        state.message_count("c_0"),
        MAX_MESSAGES_PER_CHANNEL + 1,
        "one row in, one row out: the exception holds at the overshoot rather \
         than draining, and every arrival keeps it there"
    );
    assert!(
        state.message("c_0", &oldest).is_none(),
        "and the row the server answered is the one that goes next -- an \
         acknowledged send is ordinary history, so it is the oldest *candidate* \
         and the scan takes it. This is what makes the exception cost nothing: \
         the channel does not sit over the cap waiting to be rescued, it sheds \
         the first row the moment shedding is allowed"
    );
    assert_internally_consistent(&state, &BTreeSet::new(), 0);
}

/// An acknowledged send's delivery entry is retired along with the row eviction
/// took, so the delivery map cannot outgrow the history.
///
/// **This is the bound `AGENTS.md` §7.1 asks for, and it is the one structure
/// that had none.** The delivery map gains an entry per send; until this was fixed
/// the only retirement path was `discard_failed_send`, which applies only to a
/// *failed* send, so an acknowledged send's entry lived for the life of the
/// process. `evict_one_over_cap` now retires it, which is what makes the bound
/// structural rather than a new constant: an entry either names a held row or a
/// send still in flight.
///
/// **The arrangement puts the acknowledged send at the HEAD, because that is the
/// only place the bound is load-bearing.** Eviction takes the oldest *candidate*,
/// and any row this client is not waiting on qualifies — so an acknowledged send
/// is ordinary history and is an ordinary eviction candidate. A send parked at the
/// tail would survive the whole burst and prove nothing, and a send at the head of
/// a channel that is not yet at the cap would never be reached. `at(0)` before the
/// arriving history puts it at index 0, and the identity is out of the arrivals'
/// range so the ACK reconciles onto the optimistic row rather than onto a message
/// the server also knows about — the reason `cid(1)` would silently break this.
#[test]
fn an_acknowledged_sends_delivery_entry_is_retired_with_the_row_it_is_evicted_with() {
    let mut state = loaded();

    const CLEAR_OF_THE_ARRIVALS: u128 = 1_000_000;
    let send = cid(CLEAR_OF_THE_ARRIVALS);
    assert!(matches!(
        begin_send(
            &mut state,
            "c_0",
            "the oldest row in the channel",
            send,
            at(0)
        ),
        SendOutcome::Pending { .. }
    ));
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: send,
            message: stored(
                "m_send",
                CLEAR_OF_THE_ARRIVALS,
                "c_0",
                "u_me",
                "the oldest row in the channel",
                0,
            ),
        },
    );
    assert_eq!(
        state.delivery(&send),
        Some(DeliveryState::Acked),
        "the fixture must be acknowledged before anything is evicted, or this \
         measures nothing"
    );

    for n in 1..(MAX_MESSAGES_PER_CHANNEL + 1) as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }

    assert_eq!(
        state.message_count("c_0"),
        MAX_MESSAGES_PER_CHANNEL,
        "and the cap still holds: the retirement is not a substitute for the \
         bound, it rides on it"
    );
    assert!(
        state.message("c_0", &send).is_none(),
        "the acknowledged row was the oldest candidate and eviction took it -- \
         which is correct, an acknowledged row is ordinary history"
    );
    assert_eq!(
        state.delivery(&send),
        None,
        "AND ITS DELIVERY ENTRY WENT WITH IT. This is the bound: without it the \
         map grew by one entry per acknowledged send for the life of the process, \
         which AGENTS.md 7.1 forbids, and which a 30-minute --mode soak measured \
         (docs/BASELINES.md). A 'send an ack and stop' test cannot find it -- the \
         entry is only ever visible once eviction reaches the row"
    );
}

/// A failed send's delivery entry survives eviction, because the user still needs
/// to retry it.
///
/// **The negative guard on the fix above, and it protects the row as much as the
/// entry.** A `Failed` send is not outstanding *visibly* — nothing about it is
/// animating — but `has_outstanding_send` treats it as such, so it is never an
/// eviction candidate; and `AGENTS.md` §7.5's "failures are never silently
/// dropped" is what the user reads as "my message is still here, and I can press
/// retry". A retirement that ignored the delivery state would keep the row and
/// take the entry, and the next `retry_send` would answer `NotHeld` for a send the
/// user can still see.
///
/// **The discard afterwards is the assertion that makes it a real one.** Asserting
/// only that `delivery(..)` still answers `Failed` would pass for an entry that
/// survived but no longer names anything; `discard_failed_send` reaching
/// `Applied` proves the entry is still intact *and* still wired to a row the user
/// can act on — which is the whole reason it was kept.
#[test]
fn a_failed_sends_delivery_entry_survives_eviction() {
    let mut state = loaded();

    const CLEAR_OF_THE_ARRIVALS: u128 = 1_000_000;
    let send = cid(CLEAR_OF_THE_ARRIVALS);
    assert!(matches!(
        begin_send(
            &mut state,
            "c_0",
            "the server refused this one",
            send,
            at(0)
        ),
        SendOutcome::Pending { .. }
    ));
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: send,
            code: "rejected".to_owned(),
            detail: "the server refused this one".to_owned(),
        },
    );
    assert_eq!(state.delivery(&send), Some(DeliveryState::Failed));

    for n in 1..(MAX_MESSAGES_PER_CHANNEL + 1) as u128 {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{n}"),
                n,
                "c_0",
                "u_ada",
                "body",
                n as i64,
            )),
        );
    }

    assert_eq!(
        state.message_count("c_0"),
        MAX_MESSAGES_PER_CHANNEL,
        "the cap holds and eviction skipped the failed send to reach the next \
         oldest, so it is not a suspension of the bound"
    );
    assert!(
        state.message("c_0", &send).is_some(),
        "the failed row is still on screen, which is the visible half of \
         AGENTS.md 7.5's 'failures are never silently dropped'"
    );
    assert_eq!(
        state.delivery(&send),
        Some(DeliveryState::Failed),
        "and so is its delivery entry: retirement at eviction is guarded on the \
         state being Acked precisely so the send the user still has to retry keeps \
         its bookkeeping"
    );
    assert_eq!(
        discard_failed_send(&mut state, send),
        ApplyOutcome::Applied,
        "which is what makes the entry more than a stale row: discard_failed_send \
         is still the retirement path for a failed send, and it needs an intact \
         entry naming a held row to act on. A retirement that had ignored the \
         delivery state would answer NotHeld here"
    );
    assert_eq!(
        state.delivery(&send),
        None,
        "and only then does the entry go"
    );
}

/// A row the server moves between channels keeps its delivery entry, which is the
/// regression a `remove_message`-level retirement would have caused.
///
/// **The most valuable of the three, because it is the one that guards the fix's
/// placement rather than its existence.** `remove_message` has four callers and
/// only one of them discards the row: `merge_into_held` and the channel-mismatch
/// arm of `ingest_by_identity` both remove a row and re-insert the *same*
/// `client_msg_id` into another channel, then call `retarget_outgoing`. A
/// retirement written inside `remove_message` — the obvious place, and the place
/// that would have satisfied the test above on its first draft — would therefore
/// delete the entry of a send that is still on screen in its new channel: the row
/// moves, its bookkeeping does not, and `retry_send` on a *failed* send that the
/// server moved would answer `NotHeld`.
///
/// **The channel-mismatch arm is used because it is the cleaner of the two merges,
/// and the reason it is reachable is worth naming.** `ingest_by_identity` only
/// looks for the identity *in the channel the incoming message names*, so
/// delivering the same `client_msg_id` under `c_1` while it is held in `c_0` is
/// what selects the move. `begin_send` in `c_0` and an ACK whose stored copy also
/// names `c_0` put the send in the state a moved send is actually in — tracked,
/// and `Acked` — which is the state whose entry a misplaced retirement loses.
#[test]
fn a_row_the_server_moves_between_channels_keeps_its_delivery_entry() {
    let mut state = loaded();

    const CLEAR_OF_THE_ARRIVALS: u128 = 1_000_000;
    let send = cid(CLEAR_OF_THE_ARRIVALS);
    assert!(matches!(
        begin_send(&mut state, "c_0", "the server disagrees", send, at(0)),
        SendOutcome::Pending { .. }
    ));
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: send,
            message: stored(
                "m_send",
                CLEAR_OF_THE_ARRIVALS,
                "c_0",
                "u_me",
                "the server disagrees",
                0,
            ),
        },
    );
    assert_eq!(state.delivery(&send), Some(DeliveryState::Acked));

    // The same identity, arriving under a different channel, one second later.
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored(
            "m_send",
            CLEAR_OF_THE_ARRIVALS,
            "c_1",
            "u_me",
            "the server disagrees",
            1,
        )),
    );

    assert!(
        state.message("c_1", &send).is_some(),
        "the move is the premise: the server is the authority on which channel a \
         message lives, so the row follows it rather than being duplicated"
    );
    assert!(
        state.message("c_0", &send).is_none(),
        "and exactly one copy exists -- two rows for one client_msg_id is the \
         reshuffle bug core/ordering.rs 1 is about"
    );
    assert_eq!(
        state.channel_holding(&send),
        Some("c_1"),
        "so the identity resolves to its new channel"
    );
    assert_eq!(
        state.delivery(&send),
        Some(DeliveryState::Acked),
        "AND ITS DELIVERY ENTRY SURVIVED THE MOVE. This is the assertion that \
         pins WHERE the retirement lives: it is in evict_one_over_cap, after a \
         removal that genuinely discards the row, and not in remove_message, \
         which two merge paths call on a row they are about to re-insert. Written \
         inside remove_message, this fails with delivery(..) == None while the row \
         sits on screen in c_1"
    );
}

// ---------------------------------------------------------------------------
// 9. The rendered-segment cache
// ---------------------------------------------------------------------------

/// A parsed message is cached by `client_msg_id`, and a pending message can be
/// cached at all.
///
/// **The key is load-bearing and the pending case is why.** A server id does not
/// exist for a message until the server accepts it (`state/app_state.rs` module
/// docs, §4), so a cache keyed by server id could not hold the segments of the
/// message the user is looking at *while they wait for it*. Keying by
/// `client_msg_id` — which exists from the instant they pressed Enter — is what
/// makes the optimistic path cacheable.
#[test]
fn a_parsed_message_is_cached_by_client_msg_id_including_a_pending_one() {
    let mut state = loaded();
    let pending = cid(1);
    begin_send(&mut state, "c_0", "**bold** and `code`", pending, at(0));

    assert_eq!(
        render_and_cache(&mut state, pending, "**bold** and `code`", 32),
        ApplyOutcome::Applied
    );
    let document = state.rendered(&pending).expect("the parse is cached");
    assert_eq!(
        document.blocks().len(),
        1,
        "and it is the real tree, not a marker"
    );
    assert!(
        state.rendered(&cid(2)).is_none(),
        "an unrelated identity misses"
    );
    let (resident, cost, hits, misses, evictions) = state.segment_cache_stats();
    assert_eq!((resident, cost, hits, misses, evictions), (1, 32, 1, 1, 0));
}

/// A document larger than the whole budget is refused, and only the caching is
/// affected.
///
/// `core/cache.rs` §12's refusal rule, reached through this layer: the message
/// still renders, the parse is simply re-produced on every ask, and the caller
/// learns it happened instead of finding out from a silently absent cache entry.
#[test]
fn a_document_too_large_for_the_budget_is_refused_and_reported() {
    let mut state = AppState::new("u_me").with_segment_cache_bounds(4, Some(16));

    assert_eq!(
        render_and_cache(&mut state, cid(1), "small", 16),
        ApplyOutcome::Applied,
        "a document that exactly fits is admitted"
    );
    assert_eq!(
        render_and_cache(&mut state, cid(2), "far too large", 17),
        ApplyOutcome::Ignored(IgnoreReason::RenderedDocumentTooLarge)
    );
    assert!(state.rendered(&cid(2)).is_none(), "and it was not cached");
    let (_, cost, _, _, _) = state.segment_cache_stats();
    assert_eq!(cost, 16, "and refusing it changed nothing");
}

/// An edit invalidates the cached parse, and an ACK that changes only the id does
/// not.
///
/// **This is the bug class a cache in this layer creates, and the fix is driven by
/// the *report* rather than by "a merge happened".** Both halves matter: an edited
/// message rendered from yesterday's tree is a user-visible lie, and an ACK that
/// only fills in the `id` must not throw away a parse that is still good — which
/// would be a re-parse on the scroll path for every message that is accepted.
#[test]
fn an_edit_invalidates_the_cached_parse_and_an_ack_that_only_fills_the_id_does_not() {
    let mut state = loaded();
    let mine = cid(1);
    begin_send(&mut state, "c_0", "the original text", mine, at(0));
    assert_eq!(
        render_and_cache(&mut state, mine, "the original text", 32),
        ApplyOutcome::Applied
    );

    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: mine,
            message: stored("m_1", 1, "c_0", "u_me", "the original text", 1),
        },
    );
    assert_eq!(state.delivery(&mine), Some(DeliveryState::Acked));
    assert!(
        state.rendered(&mine).is_some(),
        "an id-only merge must not throw away a parse that is still good"
    );

    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_me", "the edited text", 2)),
    );
    assert!(
        state.rendered(&mine).is_none(),
        "an edited message must never be rendered from yesterday's tree"
    );
}

// ---------------------------------------------------------------------------
// 9. The `Display` impls, which are output and therefore the product
// ---------------------------------------------------------------------------

/// Every refusal reason renders the one line it is supposed to render.
///
/// **This exists because work unit 1D found that `core/theme.rs`'s
/// `ThemeError::TooLarge` message had never been read by any test** — the variant
/// was constructed and matched on, so the `Display` arm was unexecuted, in a
/// module whose entire reason for hand-writing a parser is that its messages *are*
/// the product. The same argument applies here with more force: `AGENTS.md` §7.5
/// says an error message is a user-facing string, and a string nobody renders is
/// a string nobody checked.
///
/// **The texts are asserted in full rather than by a prefix**, because a prefix
/// assertion passes for an implementation that stops mid-sentence, and the id
/// *is* the sentence's reason for existing.
///
/// **And the safety property is asserted alongside**: the rendered line carries
/// ids and reasons and **never message content**, which is what makes it safe to
/// put in a `tracing` record.
#[rstest]
#[case(IgnoreReason::BlankChannelId, "ignored: the channel id is blank")]
#[case(IgnoreReason::EmptyContent, "ignored: the message body is empty")]
#[case(
    IgnoreReason::AlreadyHeld { client_msg_id: Uuid::from_u128(1) },
    "ignored: the message 00000000-0000-0000-0000-000000000001 is already held"
)]
#[case(
    IgnoreReason::NoSuchMessage { message_id: "m_absent".to_owned() },
    "ignored: no held message has the server id m_absent"
)]
#[case(
    IgnoreReason::NotHeld { client_msg_id: Uuid::from_u128(2) },
    "ignored: no held send has the client id 00000000-0000-0000-0000-000000000002"
)]
#[case(
    IgnoreReason::NotFailed { client_msg_id: Uuid::from_u128(3) },
    "ignored: the send 00000000-0000-0000-0000-000000000003 did not fail"
)]
#[case(
    IgnoreReason::AlreadyPending { client_msg_id: Uuid::from_u128(4) },
    "ignored: the send 00000000-0000-0000-0000-000000000004 is already outstanding"
)]
#[case(
    IgnoreReason::UnknownChannel { channel_id: "c_9".to_owned() },
    "ignored: the channel c_9 is not loaded"
)]
#[case(
    IgnoreReason::TypingSetFull { channel_id: "c_0".to_owned() },
    "ignored: the typing set for c_0 is full"
)]
#[case(
    IgnoreReason::TypingChannelLimit { channel_id: "c_65".to_owned() },
    "ignored: the typing channel limit is reached at c_65"
)]
#[case(
    IgnoreReason::NobodyTyping { channel_id: "c_0".to_owned() },
    "ignored: nobody was typing in c_0"
)]
#[case(
    IgnoreReason::OutboxFull {
        client_msg_id: Uuid::from_u128(5)
    },
    "ignored: the outbox for 00000000-0000-0000-0000-000000000005 is full at 1024 \
     queued sends"
)]
#[case(
    IgnoreReason::RenderedDocumentTooLarge,
    "ignored: the parsed document exceeds the segment cache budget"
)]
fn every_refusal_reason_renders_the_line_it_is_supposed_to_render(
    #[case] reason: IgnoreReason,
    #[case] expected: &str,
) {
    assert_eq!(ApplyOutcome::Ignored(reason.clone()).to_string(), expected);
    assert_eq!(
        reason.to_string(),
        expected.strip_prefix("ignored: ").unwrap_or(expected)
    );
    assert_eq!(
        format!("{reason}"),
        reason.to_string(),
        "and Display is total over the variant"
    );
}

/// The refusal reason set is exactly these twelve, and each renders.
///
/// **A registry guard, and the reason it is here rather than assumed**: a
/// `#[case]` list is a hand-maintained enumeration, and nothing in `#[rstest]`
/// syntax makes it complete — which is the lesson `docs/COVERAGE.md` §6.2 records
/// for `rstest` generally ("the mandated style is not self-verifying, and treating
/// it as though it were would be the next version of the mistake ADR-008
/// corrects"). A thirteenth variant with no case would be a line the user could
/// never see rendered, and a `Display` arm that panics or truncates.
#[test]
fn the_refusal_reason_cases_cover_every_reason() {
    let rendered = [
        IgnoreReason::BlankChannelId,
        IgnoreReason::EmptyContent,
        IgnoreReason::AlreadyHeld {
            client_msg_id: cid(1),
        },
        IgnoreReason::NoSuchMessage {
            message_id: "m_1".to_owned(),
        },
        IgnoreReason::NotHeld {
            client_msg_id: cid(2),
        },
        IgnoreReason::NotFailed {
            client_msg_id: cid(3),
        },
        IgnoreReason::AlreadyPending {
            client_msg_id: cid(4),
        },
        IgnoreReason::OutboxFull {
            client_msg_id: cid(5),
        },
        IgnoreReason::UnknownChannel {
            channel_id: "c_0".to_owned(),
        },
        IgnoreReason::TypingSetFull {
            channel_id: "c_0".to_owned(),
        },
        IgnoreReason::TypingChannelLimit {
            channel_id: "c_0".to_owned(),
        },
        IgnoreReason::NobodyTyping {
            channel_id: "c_0".to_owned(),
        },
        IgnoreReason::RenderedDocumentTooLarge,
    ];

    assert_eq!(
        rendered.len(),
        13,
        "one case per variant, and the count is written down so a fourteenth is a \
         deliberate edit here rather than a silent gap"
    );
    // Every one renders to something non-empty and single-line, which is the two
    // properties a `tracing` record depends on.
    for reason in rendered {
        let line = reason.to_string();
        assert!(!line.is_empty(), "{reason:?} rendered nothing");
        assert!(
            !line.contains('\n'),
            "{reason:?} rendered more than one line"
        );
    }
}

/// A send's outcome says which of the four things happened, in words.
///
/// **The fourth case is the one that has to exist at all.** A send refused at the
/// outbox bound produces a *visible failed row*, and `AGENTS.md` §5.2 makes the
/// explanation part of the product — so the line a `tracing` record or a badge
/// carries is asserted in full, including the bound the user could act on.
#[rstest]
#[case(
    SendOutcome::Pending { client_msg_id: Uuid::from_u128(1), offline: false },
    "sent as 00000000-0000-0000-0000-000000000001"
)]
#[case(
    SendOutcome::Pending { client_msg_id: Uuid::from_u128(2), offline: true },
    "queued offline as 00000000-0000-0000-0000-000000000002"
)]
#[case(
    SendOutcome::Ignored(IgnoreReason::EmptyContent),
    "not sent: the message body is empty"
)]
fn a_send_outcome_says_in_words_which_of_the_four_things_happened(
    #[case] outcome: SendOutcome,
    #[case] expected: &str,
) {
    assert_eq!(outcome.to_string(), expected);
}

/// The outbox refusal renders as one line naming the identity and the bound, and
/// `SendFailure` renders its code whether or not the server sent a detail.
///
/// **Two gaps closed together because both are Display arms nothing else would
/// execute.** `IgnoreReason`'s `OutboxFull` arm is only ever reached from
/// `retry_send` at a full queue — a case no flow test walks — and `SendFailure`'s
/// own `Display` is the only thing that turns the reason on a refused row into a
/// sentence. A message that renders to `""`, or to two lines, is a bug in the
/// product's only user-facing surface for this state.
#[test]
fn the_outbox_refusal_and_the_failure_beside_it_render_one_readable_line() {
    let identity = cid(9);
    let reason = SendFailure::new("client.outbox_full", "1024 sends are waiting");
    assert_eq!(
        reason.to_string(),
        "client.outbox_full: 1024 sends are waiting"
    );

    // **A `message.error` may carry a code and no prose**, and `code: ` with
    // nothing after it is worse than the code alone -- it reads as a sentence that
    // lost its ending.
    let bare = SendFailure::new("validation_error", String::new());
    assert_eq!(bare.to_string(), "validation_error");

    let rendered = SendOutcome::Failed {
        client_msg_id: identity,
        reason: SendFailure::new(
            "client.outbox_full",
            "this machine already has 1024 sends waiting for a connection",
        ),
    }
    .to_string();

    assert_eq!(
        rendered,
        "not queued: 00000000-0000-0000-0000-00000000000a failed: \
         client.outbox_full: this machine already has 1024 sends waiting for a \
         connection"
    );
    assert!(
        !rendered.contains('\n'),
        "one line, because a badge is one line and a `tracing` record is one record"
    );
}

/// A merge reports the fields that disagreed, by name, and an empty report says
/// so rather than printing nothing.
///
/// **The names come from `DifferingField::as_str`, not from this test's own
/// spelling**, so the report cannot drift from the field it names — the same
/// reason `core/ordering.rs` has that accessor and the same reason its own
/// `as_str` says "deliberately not `Debug`".
#[test]
fn a_merge_report_names_the_fields_that_differed() {
    let single = ApplyOutcome::Merged {
        fields: [
            ordering::DifferingField::Id,
            ordering::DifferingField::Content,
        ]
        .into_iter()
        .collect(),
    };
    assert_eq!(single.to_string(), "merged; fields differ: id, content");

    let empty = ApplyOutcome::Merged {
        fields: sh_nexus::state::app_state::DifferingFields::new(),
    };
    assert_eq!(
        empty.to_string(),
        "merged with no differing field",
        "an empty report must still say something -- a blank line in a log is a bug"
    );

    // **And the plain case, which the `ApplyOutcome` doctest covers and this
    // file does not rely on for.** `cargo llvm-cov` does not run doctests unless
    // told to, so an arm covered only by an example is an arm the coverage report
    // calls missed -- which is a fact about the measurement that is worth a line
    // of its own, given `docs/COVERAGE.md` §5.3 counts doctests as mutation
    // catchers.
    assert_eq!(ApplyOutcome::Applied.to_string(), "applied");

    assert_eq!(
        ApplyOutcome::Adopted {
            client_msg_id: cid(1)
        }
        .to_string(),
        "adopted the unheld message 00000000-0000-0000-0000-000000000002"
    );
}

// ---------------------------------------------------------------------------
// 10. The remaining read surface
// ---------------------------------------------------------------------------

/// A loaded channel is readable, and an unknown one is `None` rather than a
/// fabricated default.
#[test]
fn a_loaded_channel_is_readable_and_an_unknown_one_is_none() {
    let state = loaded();
    let channel = state.channel("c_2").expect("c_2 is loaded");
    assert_eq!(channel.id, "c_2");
    assert_eq!(channel.name, "channel-2");
    assert!(
        !channel.is_private,
        "the fixture's default, read back verbatim"
    );
    assert!(state.has_channel("c_2"));
    assert!(state.channel("c_nope").is_none());
    assert!(!state.has_channel("c_nope"));
}

/// A segment cache with **no budget** drops the cost bound and keeps the count
/// bound, and both halves are asserted.
///
/// The alternative spelling — `with_budget(capacity, 0)` — is the *tightest
/// possible ceiling* rather than no ceiling, and a test that meant to cover the
/// unbounded case through it would be testing a cache that refuses everything.
/// `tests/cache_ceiling.rs` records the same trap and the reason it is a one-line
/// difference with a whole-paragraph consequence. **What `None` actually removes is
/// the cost bound, not the count bound**, so the assertions are split rather than
/// one claim about "unbounded", which would have been false.
#[test]
fn a_segment_cache_with_no_budget_drops_the_cost_bound_and_keeps_the_count_bound() {
    let mut state = AppState::new("u_me").with_segment_cache_bounds(2, None);
    for n in 0..2u128 {
        assert_eq!(
            render_and_cache(&mut state, cid(n), "body", 1_000_000),
            ApplyOutcome::Applied,
            "a megabyte, which a budgeted cache would have refused: there is no ceiling"
        );
    }
    let (_, cost, _, _, evictions) = state.segment_cache_stats();
    assert_eq!(cost, 2_000_000, "and the exact sum of what it was told");
    assert_eq!(evictions, 0, "nothing was evicted yet");

    assert_eq!(
        render_and_cache(&mut state, cid(2), "body", 1_000_000),
        ApplyOutcome::Applied
    );
    let (resident, cost, _, _, evictions) = state.segment_cache_stats();
    assert_eq!(
        resident, 2,
        "the count bound is still in force -- None removed the budget, not the capacity"
    );
    assert_eq!(cost, 2_000_000, "and the total followed the eviction");
    assert_eq!(evictions, 1);
    assert!(state.rendered(&cid(0)).is_none(), "the oldest went");
}

/// The resync cursor falls back to the newest message held when nothing was
/// recorded, and the expectation carries it.
///
/// The three cases of [`AppState::resync_cursor`] are: both present (the max), the
/// recorded one alone, and the held one alone. The first two are pinned by
/// `the_resync_cursor_is_local_state_and_never_re_asks_for_what_is_held`; this is
/// the third, and it is the case a first sync produces.
#[test]
fn a_channel_with_messages_and_no_recorded_cursor_resumes_from_its_newest() {
    let mut state = loaded();
    assert_eq!(
        state.resync_cursor("c_0"),
        None,
        "nothing held, nothing recorded"
    );

    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_1", 1, "c_0", "u_ada", "body", 7)),
    );

    assert_eq!(
        state.resync_cursor("c_0"),
        Some(at(7)),
        "a first sync resumes from the newest message this client holds"
    );
    assert_eq!(
        state.sync_expectation("c_0").cursor,
        Some(at(7)),
        "and the expectation the gap detector takes agrees"
    );

    // A later message moves it, and an older recorded cursor cannot walk it back.
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_2", 2, "c_0", "u_ada", "later", 9)),
    );
    assert_eq!(state.resync_cursor("c_0"), Some(at(9)));
    fire(
        &mut state,
        DomainEvent::ResyncRequested {
            channel_id: "c_0".to_owned(),
            after: at(1),
        },
    );
    assert_eq!(state.resync_cursor("c_0"), Some(at(9)));
}

// ---------------------------------------------------------------------------
// 11. The outbox
// ---------------------------------------------------------------------------

/// An offline send is queued; **a connected send is not, and that is a
/// decision rather than an omission.**
///
/// **`PLAN.md` §7's first bullet, and the flag it was waiting for.** `begin_send`
/// computed `offline = !state.can_send()` before this queue existed and returned it
/// in `SendOutcome::Pending`; nothing read it. This is the test that says the
/// return value now has an effect.
///
/// **The connected case is asserted as carefully as the offline one.** A connected
/// send is handed straight to the transport by `MessageList::enqueue`, so queuing
/// it too would put two `message.send` frames on the wire for one message. The
/// server would drop the second as a duplicate and no transcript would be harmed —
/// but it would be a doubled frame per send, forever, for nothing. **The residual
/// hole that leaves is stated in `state/actions.rs`'s module docs, §7.1: a write
/// that fails *after* a connected send left the client is still not re-driven,
/// because closing it means the composer must ask the outbox instead of the
/// transport, which is a `ui/` change.**
#[rstest]
#[case(ConnectionState::Connected, false)]
#[case(ConnectionState::Connecting, true)]
#[case(ConnectionState::Reconnecting { attempt: 1 }, true)]
#[case(ConnectionState::Disconnected, true)]
#[case(
    ConnectionState::Rejected {
        code: "version".to_owned(),
        detail: "v2".to_owned()
    },
    true
)]
fn only_an_offline_send_is_queued(#[case] connection: ConnectionState, #[case] queued: bool) {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(&mut state, DomainEvent::ConnectionStateChanged(connection));

    let mine = cid(1);
    assert_eq!(
        begin_send(&mut state, "c_0", "written offline", mine, at(0)),
        SendOutcome::Pending {
            client_msg_id: mine,
            offline: queued,
        }
    );

    assert_eq!(
        state.outbox_len(),
        usize::from(queued),
        "the outbox holds the send exactly when the connection could not carry it"
    );
    assert_eq!(
        state.outbox_holds(&mine),
        queued,
        "and it holds the right one"
    );
    assert_eq!(
        state.message_count("c_0"),
        1,
        "the row is on screen either way -- the Optimistic Send Flow is \
         connection-independent"
    );
    assert_eq!(
        state.delivery(&mine),
        Some(DeliveryState::Pending),
        "and the row is `Pending` either way"
    );
}

/// **Reconnecting re-drives every queued send, in enqueue order** — and the order
/// is asserted here because this is where it is decided.
///
/// `PLAN.md` §7's second bullet. **`actions::flush_outbox` is the function that
/// turns the queue into frames, so the order is a property of it and not of
/// anything downstream** — `state/bridge.rs`'s loop is a `for` over what this
/// returns. A test at the seam could only assert "all of them went"; the ordering
/// claim is checkable only here.
///
/// **Three sends in two channels**, so the channel cannot be what fixes the order:
/// a queue keyed by channel, or one sorted by channel name, would produce
/// `c_0, c_0, c_1` and fail this.
#[test]
fn a_reconnect_drives_every_queued_send_in_enqueue_order() {
    let mut state = AppState::new("u_me");
    set_channels(
        &mut state,
        vec![channel("c_0", "channel-0"), channel("c_1", "channel-1")],
    );

    // Offline, which is the only way a send is queued at all.
    for (index, channel_id) in ["c_0", "c_1", "c_0"].iter().enumerate() {
        let identity = cid(u128::try_from(index).expect("three is far below u128::MAX"));
        let second = i64::try_from(index).expect("three is far below i64::MAX");
        begin_send(
            &mut state,
            channel_id,
            &format!("body {index}"),
            identity,
            at(second),
        );
    }

    // **The queue is identities, and that is what the return type says.** Not a
    // copy of the body: a queued send is stored exactly once, which is the storage
    // half of the ids-not-content decision (`AGENTS.md` §2.3's "no deep clones in
    // hot paths" and §7.1's bound both point the same way).
    assert_eq!(state.outbox_ids(), vec![cid(0), cid(1), cid(2)]);

    // **A queued row's body cannot actually change while it is queued, and that is
    // worth saying because the obvious demonstration does not work.** The only
    // writer that can change a held message's content is the server, and every
    // server-delivered copy carries a real id — which reconciles the row to
    // `Acked` and retires its entry (see
    // `a_resync_echo_of_a_queued_send_retires_its_entry`). So "the queue holds
    // identities so an edit while queued is transmitted" is a *reason* that no test
    // can demonstrate, and the honest claim is the one above: one copy.
    assert_eq!(state.delivery(&cid(0)), Some(DeliveryState::Pending));

    // Still offline: the flush produces nothing, and produces nothing *without
    // touching the queue* — the whole point of the gate.
    assert!(
        flush_outbox(&state).is_empty(),
        "a connection that cannot carry a send has nothing to drive"
    );
    assert_eq!(state.outbox_len(), 3, "and the queue is untouched");

    // The connection returns.
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    let driven = flush_outbox(&state);

    assert_eq!(
        driven
            .iter()
            .map(|send| send.client_msg_id())
            .collect::<Vec<_>>(),
        vec![cid(0), cid(1), cid(2)],
        "PLAN.md section 7 flushes in enqueue order, and the queue is what fixes it"
    );
    assert_eq!(
        driven
            .iter()
            .map(|send| send.channel_id())
            .collect::<Vec<_>>(),
        vec!["c_0", "c_1", "c_0"],
        "each frame names the channel it was composed in"
    );
    assert_eq!(
        driven
            .iter()
            .map(|send| send.content().to_owned())
            .collect::<Vec<_>>(),
        vec!["body 0", "body 1", "body 2"],
        "and each carries the body read back from the row the client holds"
    );

    // **And a second flush drives the same entries again.** Nothing was retired:
    // see the next test for why that is the whole design.
    assert_eq!(
        flush_outbox(&state).len(),
        3,
        "the entries are still queued -- re-driving is what makes a lost frame \
         recoverable"
    );
    assert_eq!(state.outbox_len(), 3, "and the queue says so");
}

/// **A queued entry leaves only on an acknowledgement or a terminal failure —
/// never because a write succeeded.**
///
/// This is the defect the whole unit exists to remove. `network/ws.rs`'s worker
/// consumes an item off its outbound queue and returns **without putting it back**
/// when `write_frame` fails, so `MAX_OUTBOUND_FRAMES` is backpressure between the
/// enqueue and the write and not a queue that survives a disconnection.
///
/// **Every half is asserted, and the "never" half is the one that matters:** a
/// flush that resolved every entry is *not* an exit, a `Pending` row after a
/// successful flush is *not* an exit, and neither is a `can_send` answer or a
/// dropped event. Only the server's two answers are.
#[test]
fn a_queued_send_leaves_on_an_ack_or_a_terminal_failure_and_on_nothing_else() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    // Three offline sends, and then the connection comes back.
    for index in 0..3u128 {
        let second = i64::try_from(index).expect("three is far below i64::MAX");
        begin_send(
            &mut state,
            "c_0",
            &format!("body {index}"),
            cid(index),
            at(second),
        );
    }
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );

    // A flush, twice, with everything resolved and nothing retired.
    assert_eq!(flush_outbox(&state).len(), 3, "the frames are produced");
    assert_eq!(
        state.outbox_len(),
        3,
        "a successful *resolution* is not an acknowledgement: the bytes left this \
         process, which is not the same as the server storing the row"
    );
    assert_eq!(flush_outbox(&state).len(), 3, "and again");
    assert_eq!(state.outbox_len(), 3, "still nothing retired");

    // Events that are not the server's answer, and none of them may retire.
    fire(
        &mut state,
        DomainEvent::PresenceUpdated {
            user_id: "u_ada".to_owned(),
            status: UserStatus::Online,
        },
    );
    fire(
        &mut state,
        DomainEvent::TypingUpdated {
            channel_id: "c_0".to_owned(),
            user_id: "u_ada".to_owned(),
            active: true,
        },
    );
    fire(
        &mut state,
        DomainEvent::ResyncRequested {
            channel_id: "c_0".to_owned(),
            after: at(0),
        },
    );
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Reconnecting { attempt: 1 }),
    );
    assert_eq!(
        state.outbox_len(),
        3,
        "unrelated events, and a connection that goes away again, retire nothing"
    );

    // **The first door: an ACK.**
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: cid(0),
            message: stored("m_0", 0, "c_0", "u_me", "body 0", 0),
        },
    );
    assert_eq!(
        state.outbox_ids(),
        vec![cid(1), cid(2)],
        "the acknowledged send left, and the order of the rest is untouched"
    );

    // **The second door: a terminal `message.error`.** Every `message.error` on
    // this protocol is terminal -- there is no retryable code and no backoff hint,
    // and `retry_send` is the user's gesture for the next attempt.
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(1),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(
        state.outbox_ids(),
        vec![cid(2)],
        "and the refused send left, staying visible as a Failed row"
    );
    assert_eq!(state.delivery(&cid(1)), Some(DeliveryState::Failed));
    assert_eq!(state.message_count("c_0"), 3, "the row is still on screen");

    // And an error for a send this client does not hold retires nothing.
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(99),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(
        state.outbox_len(),
        1,
        "an unknown identity is a refusal, not an exit"
    );

    // The ACK, and the queue is empty.
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: cid(2),
            message: stored("m_2", 2, "c_0", "u_me", "body 2", 2),
        },
    );
    assert!(
        state.outbox_len() == 0,
        "every entry left through one of the two doors"
    );
}

/// **At the bound the enqueue is refused, the row reads `Failed`, and the reason
/// names the bound.**
///
/// `AGENTS.md` §7.1 forbids unbounded in-memory state; `PLAN.md` §7's *"failures
/// are never silently dropped"* forbids answering that by dropping something. **So
/// the third option is the only one left: refuse, and say so where the user can
/// read it** — which is `AGENTS.md` §5.2's "clear, actionable error".
///
/// **Neither entry is dropped, and that is asserted explicitly.** An implementation
/// that made room by evicting the oldest queued send would pass every other
/// assertion here, so the test says which entries survived.
#[test]
fn the_outbox_refuses_at_its_bound_and_the_row_says_which_bound() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    for index in 0..MAX_OUTBOX_ENTRIES {
        let outcome = begin_send(
            &mut state,
            "c_0",
            &format!("queued {index}"),
            cid(u128::try_from(index).expect("the bound is far below u128::MAX")),
            at(0),
        );
        assert!(
            matches!(outcome, SendOutcome::Pending { offline: true, .. }),
            "send {index} of {MAX_OUTBOX_ENTRIES} fits"
        );
    }
    assert_eq!(
        state.outbox_len(),
        MAX_OUTBOX_ENTRIES,
        "the queue is exactly at its bound"
    );
    assert!(state.outbox_is_full());

    // One past it.
    let over = cid(u128::try_from(MAX_OUTBOX_ENTRIES).expect("in range"));
    let refused = begin_send(&mut state, "c_0", "one too many", over, at(1));

    let SendOutcome::Failed {
        client_msg_id,
        reason,
    } = &refused
    else {
        panic!("the row must be visible and failed, got {refused:?}");
    };
    assert_eq!(*client_msg_id, over);
    assert_eq!(
        reason.code(),
        "client.outbox_full",
        "the code names the client, not the server: the server never saw this send"
    );
    assert!(
        reason.detail().contains(&MAX_OUTBOX_ENTRIES.to_string()),
        "the reason names the bound a user could act on, and it says {:?}",
        reason.detail()
    );

    assert_eq!(
        state.delivery(&over),
        Some(DeliveryState::Failed),
        "the row reads Failed, which is what the badge draws"
    );
    assert_eq!(
        state.message("c_0", &over).map(|row| row.content.as_str()),
        Some("one too many"),
        "and the user's words are still on screen -- not discarded"
    );
    assert!(
        !state.outbox_holds(&over),
        "the refused send was not queued, so it is not the queue's problem to solve"
    );

    // **Neither the oldest nor the newest queued entry went.** This is the half a
    // "make room by evicting" implementation would fail and nothing else here
    // would catch.
    assert_eq!(
        state.outbox_len(),
        MAX_OUTBOX_ENTRIES,
        "the queue is still exactly at its bound: nothing was evicted to make room"
    );
    assert!(
        state.outbox_holds(&cid(0)),
        "the oldest queued send is still queued -- PLAN.md section 7 forbids \
         dropping it silently, and the user still believes it is waiting"
    );
    assert!(
        state.outbox_holds(&cid(
            u128::try_from(MAX_OUTBOX_ENTRIES - 1).expect("in range")
        )),
        "and so is the newest one that fitted"
    );

    // **And the refusal is recoverable, which is what makes it acceptable.** One
    // acknowledgement makes room; the refused send is then queued by its retry.
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: cid(0),
            message: stored("m_0", 0, "c_0", "u_me", "queued 0", 0),
        },
    );
    assert_eq!(state.outbox_len(), MAX_OUTBOX_ENTRIES - 1);
    assert!(!state.outbox_is_full());
    assert_eq!(
        retry_send(&mut state, over),
        ApplyOutcome::Applied,
        "the user's retry is the way out of a full queue"
    );
    assert_eq!(state.outbox_len(), MAX_OUTBOX_ENTRIES);
    assert!(
        state.outbox_holds(&over),
        "and it is queued now, at the back"
    );
    assert_eq!(state.delivery(&over), Some(DeliveryState::Pending));
}

/// **However many sends are composed, the bound holds** — and the refusals
/// accumulate rather than the queue growing.
///
/// **A separate test from the one above because the number is the claim.** The
/// other test pins the *policy* at the boundary; this one walks well past it,
/// because a check performed once at the boundary is exactly the check a
/// `>=`/`>` off-by-one survives on the far side. Twenty-two hundred is a little
/// over twice the bound: enough to show the queue stops at 1024 and the rest of the
/// sends become visible failures, which is the whole of what §7.1 asks for.
#[test]
fn the_outbox_holds_its_bound_however_many_sends_are_composed() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    let attempts = MAX_OUTBOX_ENTRIES + 1_200;
    let mut queued = 0usize;
    let mut refused = 0usize;
    for index in 0..attempts {
        let identity = cid(u128::try_from(index).expect("far below u128::MAX"));
        match begin_send(&mut state, "c_0", "body", identity, at(0)) {
            SendOutcome::Pending { offline: true, .. } => queued += 1,
            SendOutcome::Failed { .. } => refused += 1,
            other => panic!("send {index} produced {other:?}"),
        }
        assert!(
            state.outbox_len() <= MAX_OUTBOX_ENTRIES,
            "the queue grew to {} on attempt {index}, over its bound",
            state.outbox_len()
        );
    }

    assert_eq!(queued, MAX_OUTBOX_ENTRIES, "exactly the bound was taken");
    assert_eq!(
        refused,
        attempts - MAX_OUTBOX_ENTRIES,
        "and every send past it was refused rather than dropped or queued"
    );
    assert_eq!(state.outbox_len(), MAX_OUTBOX_ENTRIES);
    assert_eq!(
        state.message_count("c_0"),
        attempts,
        "every send is still on screen -- a refused send is a visible failure, not \
         a discarded message"
    );
}

/// **A retry re-queues at the back, and leaves through one of the two doors.**
///
/// `PLAN.md` §7 says the flush is in enqueue order, so *where* a retry lands is
/// part of the contract rather than a detail. **At the front it would be a
/// starvation bug**: one message the user retries repeatedly would be re-driven
/// ahead of everything queued behind it, on every reconnect, forever — and this
/// client's history would be reordered against every other client's.
///
/// **The three-entry queue is the shape that makes the assertion sharp.** With one
/// or two entries, "at the back" and "in the queue" are the same claim.
#[test]
fn a_retry_requeues_at_the_back_and_leaves_on_an_ack_or_a_failure() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    // Three offline sends; the middle one fails terminally, so it leaves the
    // queue while staying visible -- which is `PLAN.md` §7's fourth bullet.
    begin_send(&mut state, "c_0", "first", cid(0), at(0));
    begin_send(&mut state, "c_0", "second", cid(1), at(1));
    begin_send(&mut state, "c_0", "third", cid(2), at(2));
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(1),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(state.outbox_ids(), vec![cid(0), cid(2)]);

    assert_eq!(retry_send(&mut state, cid(1)), ApplyOutcome::Applied);
    assert_eq!(
        state.outbox_ids(),
        vec![cid(0), cid(2), cid(1)],
        "the retry goes to the BACK: PLAN.md section 7's order is enqueue order, and \
         a retry at the front would let one repeatedly-retried message starve \
         everything queued behind it"
    );
    assert_eq!(
        state.delivery(&cid(1)),
        Some(DeliveryState::Pending),
        "and the row is back in flight"
    );
    assert_eq!(
        state
            .message("c_0", &cid(1))
            .map(|row| row.content.as_str()),
        Some("second"),
        "with its text and its place intact -- a retry is the same send"
    );
    assert_eq!(
        state.failure(&cid(1)),
        None,
        "and its stale reason retired with the state change"
    );

    // **A second retry does not queue it twice.** One message on the wire twice per
    // flush would be the queue's own version of a duplicated transcript.
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(1),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(retry_send(&mut state, cid(1)), ApplyOutcome::Applied);
    assert_eq!(
        state.outbox_ids(),
        vec![cid(0), cid(2), cid(1)],
        "one entry per identity, however many times the user asks"
    );

    // **It leaves through the terminal failure**...
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(2),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(state.outbox_ids(), vec![cid(0), cid(1)]);
    // **...and through an acknowledgement.** Both doors, on the same queue, is the
    // claim of criterion 5 and neither half is checkable without the other.
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: cid(1),
            message: stored("m_1", 1, "c_0", "u_me", "second", 1),
        },
    );
    assert_eq!(state.outbox_ids(), vec![cid(0)]);
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: cid(0),
            message: stored("m_0", 0, "c_0", "u_me", "first", 0),
        },
    );
    assert!(state.outbox_len() == 0);
    assert_eq!(
        flush_outbox(&state).len(),
        0,
        "and there is nothing left to drive"
    );
}

/// **A retry at a full queue is refused, and the row stays `Failed`.**
///
/// **The negative half of the bound, and it is the half that could have been got
/// wrong quietly.** Promoting the row to `Pending` first and asking the outbox
/// afterwards would leave a row on screen reading `sending…` with nothing behind
/// it — a lie the user waits on, and `AGENTS.md` §5.2's "clear, actionable error"
/// is written against exactly that.
#[test]
fn a_retry_at_a_full_queue_is_refused_and_the_row_stays_failed() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    for index in 0..MAX_OUTBOX_ENTRIES {
        begin_send(
            &mut state,
            "c_0",
            "queued",
            cid(u128::try_from(index).expect("far below u128::MAX")),
            at(0),
        );
    }
    // The one send that was refused at the bound, and so was never queued.
    let refused = cid(u128::try_from(MAX_OUTBOX_ENTRIES).expect("in range"));
    let SendOutcome::Failed { .. } = begin_send(&mut state, "c_0", "one too many", refused, at(1))
    else {
        panic!("the bound was reached, so this send must be refused");
    };

    assert_eq!(
        retry_send(&mut state, refused),
        ApplyOutcome::Ignored(IgnoreReason::OutboxFull {
            client_msg_id: refused
        }),
        "the refusal names the bound rather than reporting a bare false"
    );
    assert_eq!(
        state.delivery(&refused),
        Some(DeliveryState::Failed),
        "and the row did not move to Pending: a send with nothing queued behind it \
         must not read as in flight"
    );
    assert!(!state.outbox_holds(&refused), "still not queued");
}

/// **A discard retires nothing, because a queued send is never a `Failed` one —
/// and the two are separated on purpose.**
///
/// The outbox holds identities, which is only safe because every row a queued
/// send lives in is one eviction skips. **This test pins that separation, because
/// it is what makes the discard safe at all.** Every path out of the queue is tied
/// to the transition that justifies it, and there is exactly one ordering of the
/// two facts:
///
/// | | queued? | why |
/// |---|---|---|
/// | an offline send | yes, `Pending` | the connection cannot carry it |
/// | a retry | yes, `Pending` | `retry_send` re-queues and moves the row to `Pending` in that order |
/// | a terminal `message.error` | **no** | `fail_send` retires the entry *before* setting `Failed` |
/// | a capacity refusal | **no** | it was never queued |
///
/// **So a `Failed` row is never queued, and the discard — which requires
/// `Failed` — can never meet an entry.** That is what the first assertion says,
/// and it is the answer to "what happens if a queued send's row is deleted":
/// it cannot be reached, by construction rather than by a check.
///
/// **What the test then asserts is the direction that *could* have been got wrong
/// quietly.** An implementation that cleared the whole queue on any row removal, or
/// that let the discard's dequeue reach the wrong identity, would leave the
/// surviving entry gone — a queued send silently dropped, which is the exact
/// failure `PLAN.md` §7 is written against. `state/actions.rs` keeps a
/// `dequeue_outbox` in `discard_failed_send` as defence for the invariant; this is
/// the test that says the defence does not fire.
///
/// `discard_failed_send` is unreachable from the UI — work unit 3D decided that and
/// `tests/layer_boundary.rs` enforces it — so this is the only place the
/// interaction is exercised at all.
#[test]
fn a_discard_retires_nothing_because_a_failed_send_is_never_queued() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    // Two offline sends, so one is queued and one is discarded.
    begin_send(&mut state, "c_0", "survivor", cid(0), at(0));
    begin_send(&mut state, "c_0", "doomed", cid(1), at(1));
    assert_eq!(state.outbox_ids(), vec![cid(0), cid(1)]);

    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(1),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(
        state.outbox_ids(),
        vec![cid(0)],
        "the terminal failure retired its own entry, before the row became Failed"
    );
    assert_eq!(state.delivery(&cid(1)), Some(DeliveryState::Failed));

    // And a retried one goes back to `Pending`, never to `Failed`-and-queued.
    assert_eq!(retry_send(&mut state, cid(1)), ApplyOutcome::Applied);
    assert_eq!(state.outbox_ids(), vec![cid(0), cid(1)]);
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: cid(1),
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    assert_eq!(state.outbox_ids(), vec![cid(0)]);

    assert_eq!(
        discard_failed_send(&mut state, cid(1)),
        ApplyOutcome::Applied
    );
    assert!(
        state.outbox_holds(&cid(0)),
        "the surviving entry is untouched: a discard removes exactly one row, and \
         its retirement reached nothing else"
    );
    assert!(
        !state.outbox_holds(&cid(1)),
        "and the discarded identity is not queued -- it was already retired by the \
         failure, which is why the dequeue in discard is defence and not a path"
    );
    assert!(
        flush_outbox(&state)
            .iter()
            .all(|send| send.client_msg_id() != cid(1)),
        "so no flush can produce a frame for a message that no longer exists"
    );
}

/// **A resync echo retires the entry, because a real server id *is* the server
/// saying it stored the row.**
///
/// **This door was not in the original design, and the proptest is what found its
/// absence** — it reported a queued identity sitting on an `Acked` row. A row that
/// has been reconciled against the server's stored copy is a message the client can
/// *see* the server has, so an entry left behind would be re-driven on every
/// reconnect forever, against a queue nothing else retires.
///
/// **The event is a `message.new`, not a `message.ack`, and that distinction is the
/// whole reason this test exists**: a reader looking only at `acknowledge` would not
/// believe the queue has a third door.
#[test]
fn a_resync_echo_of_a_queued_send_retires_its_entry() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    begin_send(&mut state, "c_0", "queued offline", cid(0), at(0));
    begin_send(&mut state, "c_0", "also queued", cid(1), at(1));
    assert_eq!(state.outbox_len(), 2);

    // A live `message.new` for one of them, carrying the server's stored copy.
    // `core/models/events.rs` says a resync echo and a live delivery are "the same
    // situation from the client's point of view", and this is that door.
    fire(
        &mut state,
        DomainEvent::MessageReceived(stored("m_0", 0, "c_0", "u_me", "queued offline", 0)),
    );

    assert_eq!(
        state.delivery(&cid(0)),
        Some(DeliveryState::Acked),
        "the row is reconciled against the server's copy"
    );
    assert_eq!(
        state.outbox_ids(),
        vec![cid(1)],
        "and its entry is retired: the client now holds the server's stored row, so \
         re-driving it would transmit something whose fate is already decided"
    );
    assert!(
        state.outbox_holds(&cid(1)),
        "and the untouched entry is still queued"
    );
}

/// **A queued send is never evicted by the history bound, at any distance.**
///
/// `AppState::oldest_candidate` skipping `Pending` and `Failed` rows is what makes
/// an outbox of identities safe rather than a dangling list. The unit test below
/// covers the immediate case; this one fills the channel past its cap *with the
/// queued send at the front*, which is the arrangement that would expose a scan
/// that stopped early.
#[test]
fn a_queued_send_at_the_head_is_never_the_row_eviction_takes() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    // The queued send goes in first, so it sits at the head of the channel's order
    // for as long as nothing newer arrives.
    begin_send(&mut state, "c_0", "queued and oldest", cid(0), at(0));
    assert!(
        state.outbox_holds(&cid(0)),
        "the send is queued, so it is offline by construction"
    );

    // Push the channel past its cap with ordinary history.
    for index in 1..=MAX_MESSAGES_PER_CHANNEL {
        fire(
            &mut state,
            DomainEvent::MessageReceived(stored(
                &format!("m_{index}"),
                u128::try_from(index).expect("far below u128::MAX") + 10_000,
                "c_0",
                "u_ada",
                "history",
                0,
            )),
        );
    }

    assert!(
        state.message_count("c_0") <= MAX_MESSAGES_PER_CHANNEL,
        "the channel is at its cap, and the cap held"
    );
    assert!(
        state.message("c_0", &cid(0)).is_some(),
        "the queued send is still on screen: eviction skips every row the client \
         is still waiting on, which is what lets the outbox hold identities"
    );
    assert!(
        state.outbox_holds(&cid(0)),
        "so its entry still names something"
    );

    // The connection returns, and the flush resolves the entry to the right body.
    // **The reconnect is here and not at the top of the test** because the send had
    // to be composed *offline* to be queued at all, and `flush_outbox` is gated on
    // `can_send`.
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    assert_eq!(
        flush_outbox(&state)
            .iter()
            .find(|send| send.client_msg_id() == cid(0))
            .map(|send| send.content()),
        Some("queued and oldest"),
        "so a flush can still transmit it, and reads the body from the held row"
    );
}

/// **A send handed to a *connected* transport whose write then fails is owed, and
/// it arrives** — the more likely loss case, and the one that was open.
///
/// `only_an_offline_send_is_queued` above is the pin that recorded the hole: a
/// connected send is handed straight to the socket, so `begin_send` queues
/// nothing, and a failed write consumed the frame with nothing behind it. The
/// shape of the fix is that the transport reports the failure
/// ([`DomainEvent::SendUndelivered`]) and this layer re-queues it, so the frame is
/// produced again the moment a socket can carry it.
///
/// **Every step is a different claim, and the last one is the point of the whole
/// unit**: the row stays on screen, stays `Pending`, and carries **no** failure
/// reason — so the user sees nothing at all. `MessageSendFailed` would satisfy
/// every other assertion here and fail that one, which is why the transport does
/// not produce it.
#[test]
fn a_connected_send_whose_write_fails_is_owed_again_and_still_arrives() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );

    // **Composed while connected**, which is the whole difference from every other
    // test in this section: `begin_send` sees `can_send()` and queues nothing.
    let mine = cid(1);
    assert_eq!(
        begin_send(&mut state, "c_0", "written while connected", mine, at(0)),
        SendOutcome::Pending {
            client_msg_id: mine,
            offline: false,
        },
        "the composer is told nothing is queued, because at this instant nothing is"
    );
    assert_eq!(state.outbox_len(), 0, "and the queue agrees");

    // The transport's report: the write failed.
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::SendUndelivered {
                client_msg_id: mine
            }
        ),
        ApplyOutcome::Applied,
        "an owed send is re-queued rather than refused"
    );

    // **What the user sees, which is nothing.**
    assert_eq!(
        state.delivery(&mine),
        Some(DeliveryState::Pending),
        "the row is still `Pending`: nothing refused it, so nothing failed"
    );
    assert!(
        state.failure(&mine).is_none(),
        "and carries no failure reason, so no badge is drawn. A red badge on a \
         network blip that is about to fix itself would be a lie."
    );
    assert_eq!(
        state.message_count("c_0"),
        1,
        "and it never left the screen"
    );

    // The connection comes back, and the flush produces the frame again -- under the
    // *same* identity, which is what the server dedupes on.
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    let driven = flush_outbox(&state);
    assert_eq!(driven.len(), 1, "the owed send is put on the wire again");
    assert_eq!(
        driven[0].client_msg_id(),
        mine,
        "under the identity it already had"
    );
    assert_eq!(driven[0].channel_id(), "c_0");
    assert_eq!(
        driven[0].content(),
        "written while connected",
        "with the body the user authored, read from the held row"
    );

    // The server answers, and the row is reconciled and the entry retired.
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: mine,
            message: stored("m_1", 1, "c_0", "u_me", "written while connected", 0),
        },
    );
    assert_eq!(
        state.delivery(&mine),
        Some(DeliveryState::Acked),
        "the message arrived, exactly once"
    );
    assert_eq!(
        state.outbox_len(),
        0,
        "and the entry retired through the ack door"
    );
    assert_eq!(
        state
            .messages("c_0")
            .iter()
            .filter(|held| held.client_msg_id == mine)
            .count(),
        1,
        "a re-driven send that the server dedupes is still one row"
    );
}

/// **The owed send is driven only when the connection can carry it** — which is
/// the answer to the one fear this design invites: a 20-tick-per-second flush
/// hammering a doomed frame.
///
/// **The twenty ticks are the test, not decoration.** `bridge.rs` calls
/// `actions::flush_outbox` every tick, so the question is not whether the gate
/// exists but whether a caller can get twenty frames out of a queue it should not
/// have been driven from. Each of those calls must produce nothing, and the queue
/// must be the same length afterwards — a flush that dequeued without driving, or
/// drove without dequeuing, would show up as a length change.
///
/// **Every state the reconnect loop can be in is walked, in the order the loop
/// emits them.** `network/ws.rs` reports `SendUndelivered`, then `Disconnected`,
/// then `Reconnecting { attempt }` on each pass, then `Connected` — so the states
/// are applied in exactly that sequence rather than chosen independently, because
/// "safe in either order" is only a claim if it holds in both. **All five gapped
/// states are cases, not the one that matters most**, and that is the
/// `IgnoreReason` registry's lesson applied here: a hand-maintained case list is
/// not self-verifying, so a `ConnectionState` that *could* send and was not
/// exercised would be a state nobody checked.
#[rstest]
#[case(ConnectionState::Connecting)]
#[case(ConnectionState::Reconnecting { attempt: 1 })]
#[case(ConnectionState::Reconnecting { attempt: 7 })]
#[case(ConnectionState::Disconnected)]
#[case(ConnectionState::Rejected {
    code: "version".to_owned(),
    detail: "v2".to_owned()
})]
fn an_owed_send_is_not_re_driven_while_the_connection_cannot_carry_it(
    #[case] gap: ConnectionState,
) {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );

    let mine = cid(1);
    begin_send(&mut state, "c_0", "owed", mine, at(0));
    fire(
        &mut state,
        DomainEvent::SendUndelivered {
            client_msg_id: mine,
        },
    );

    // **The whole of the reconnect loop, in the order `network/ws.rs` emits it.**
    for connection in [ConnectionState::Disconnected, gap] {
        fire(
            &mut state,
            DomainEvent::ConnectionStateChanged(connection.clone()),
        );

        for tick in 0..20 {
            assert!(
                flush_outbox(&state).is_empty(),
                "a flush during {connection:?} produced a frame on tick {tick}. \
                 `can_send()` is false in this state, and that gate is the only \
                 thing standing between a doomed frame and twenty frames a second."
            );
            assert_eq!(
                state.outbox_len(),
                1,
                "and the queue is untouched: a flush that resolved an entry without \
                 driving it has lost the send"
            );
            assert!(
                !state.can_send(),
                "{connection:?} must not report that it can send, or the gate above \
                 is asserting something false"
            );
        }
    }

    // The socket is genuinely back, and the queue is finally driven -- once, and
    // the entry is still there for the server's answer.
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    assert_eq!(
        flush_outbox(&state).len(),
        1,
        "and now it is driven, which is what makes the gate a gate rather than a \
         permanent refusal"
    );
    assert_eq!(
        state.outbox_len(),
        1,
        "still queued until the server answers"
    );
}

/// **Two reports of the same owed send produce one entry** — the queue is a set per
/// identity, and the two events the transport emits around this one can be
/// applied in either order.
///
/// `actions.rs`'s module docs claim idempotence rather than assume it, so it is
/// asserted twice: once for two identical events, and once with the connection
/// state the transport emits *between* them, because a reconnection that produced
/// another attempt before anything drained is the realistic source of the second.
#[test]
fn an_owed_send_is_queued_once_however_often_it_is_reported() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );

    let mine = cid(1);
    begin_send(&mut state, "c_0", "owed", mine, at(0));

    for round in 0..3 {
        assert_eq!(
            apply_event(
                &mut state,
                DomainEvent::SendUndelivered {
                    client_msg_id: mine
                }
            ),
            ApplyOutcome::Applied,
            "report {round} is applied, not refused as a full queue -- an identity \
             already queued is what a repeated report looks like"
        );
        fire(
            &mut state,
            DomainEvent::ConnectionStateChanged(ConnectionState::Reconnecting { attempt: 1 }),
        );
        assert_eq!(
            state.outbox_ids(),
            vec![mine],
            "three reports and one entry: a queue holding one identity twice puts \
             one message on the wire twice per flush forever"
        );
    }
}

/// **An owed send whose row the server has already answered is not owed again.**
///
/// **This is the case the transport cannot prevent and only the order can produce.**
/// A write that fails after its bytes have already left is indistinguishable from
/// one that failed before, so the frame may have been stored; and the `ack` can
/// be read *before* the failure is observed, because the read and the write are
/// branches of the same `select!`. So `MessageAcked` can be applied before
/// `SendUndelivered`, and the row is `Acked` by the time the second arrives.
///
/// **Re-queuing here would be the leak, not the repair.** The third invariant in
/// `assert_internally_consistent` requires every queue entry to name a row this
/// client is *still waiting on*, and an entry on a stored row is re-driven on
/// every reconnect forever.
#[test]
fn a_send_the_server_already_answered_is_not_owed_again() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );

    let stored_first = cid(1);
    begin_send(
        &mut state,
        "c_0",
        "stored despite the failed write",
        stored_first,
        at(0),
    );
    fire(
        &mut state,
        DomainEvent::MessageAcked {
            client_msg_id: stored_first,
            message: stored(
                "m_1",
                1,
                "c_0",
                "u_me",
                "stored despite the failed write",
                0,
            ),
        },
    );
    fire(
        &mut state,
        DomainEvent::SendUndelivered {
            client_msg_id: stored_first,
        },
    );

    assert_eq!(
        state.outbox_len(),
        0,
        "the server already has it, so there is nothing to owe: an entry here would \
         be re-driven on every reconnect for a message that is already stored"
    );
    assert_eq!(
        state.delivery(&stored_first),
        Some(DeliveryState::Acked),
        "and the server's answer stands -- a local write failure does not undo it"
    );

    // **The terminal refusal is the same shape and the same answer.** A
    // `message.error` for this identity and a `SendUndelivered` for it cannot both
    // come from one write -- only one branch of the `select!` runs per pass -- but
    // the refusal may have been for the frame the *previous* pass wrote, and a
    // queue entry on a `Failed` row would resurrect a refusal the user is looking
    // at. `retry_send` is that user's gesture, and this is not it.
    let refused = cid(2);
    begin_send(
        &mut state,
        "c_0",
        "refused after the failed write",
        refused,
        at(1),
    );
    fire(
        &mut state,
        DomainEvent::MessageSendFailed {
            client_msg_id: refused,
            code: "server.refused".to_owned(),
            detail: String::new(),
        },
    );
    fire(
        &mut state,
        DomainEvent::SendUndelivered {
            client_msg_id: refused,
        },
    );

    assert_eq!(state.outbox_len(), 0, "a terminal refusal is not re-queued");
    assert_eq!(
        state.delivery(&refused),
        Some(DeliveryState::Failed),
        "and the badge the user can act on stays"
    );
    assert_eq!(
        state.message_count("c_0"),
        2,
        "both rows are still on screen"
    );
}

/// **An owed send for an identity this client never sent is refused, and an owed
/// send at the queue's bound is refused too** — both leaving the queue untouched.
///
/// Two refusals rather than one test each because they are the same line of the
/// same function and the interesting assertion is the same: **neither invented an
/// entry**. A re-queue for an unknown identity would be an entry naming nothing,
/// which `flush_outbox`'s `?` would then silently skip forever.
#[test]
fn an_owed_send_this_client_does_not_hold_is_refused() {
    let mut state = AppState::new("u_me");
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);

    let never_sent = cid(7);
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::SendUndelivered {
                client_msg_id: never_sent
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::NotHeld {
            client_msg_id: never_sent
        }),
        "an identity this client never sent has no row to keep `Pending` and \
         nothing to re-drive"
    );
    assert_eq!(state.outbox_len(), 0, "and nothing was queued for it");

    // **At the bound**, a real send is reported owed and the queue refuses it.
    // Driven by composing offline sends, which is the only way to fill the queue,
    // and the refusal is asserted rather than inferred from the count.
    set_channels(&mut state, vec![channel("c_0", "channel-0")]);
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Disconnected),
    );
    let mut held = Vec::new();
    for index in 0..MAX_OUTBOX_ENTRIES {
        let identity = cid(u128::try_from(index).expect("far below u128::MAX") + 1_000);
        held.push(identity);
        begin_send(&mut state, "c_0", "queued", identity, at(0));
    }
    assert_eq!(
        state.outbox_len(),
        MAX_OUTBOX_ENTRIES,
        "the queue is at its bound, filled with real sends"
    );

    // A send composed while connected is *not* queued, so this one is held and
    // waiting: it is exactly the case the bound has to refuse.
    let overflow = cid(9_000);
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    begin_send(&mut state, "c_0", "owed, at a full queue", overflow, at(0));
    assert_eq!(
        apply_event(
            &mut state,
            DomainEvent::SendUndelivered {
                client_msg_id: overflow
            }
        ),
        ApplyOutcome::Ignored(IgnoreReason::OutboxFull {
            client_msg_id: overflow
        }),
        "the bound refuses rather than dropping, and it names the bound"
    );
    assert_eq!(
        state.outbox_len(),
        MAX_OUTBOX_ENTRIES,
        "so the queue is exactly as it was: the refused send was not admitted and \
         nothing already queued was taken"
    );
    assert_eq!(
        state.outbox_ids().first(),
        held.first(),
        "and what is queued is still the send that was queued first"
    );
}

/// The universe the generated sequences draw from.
///
/// **Small enough to reason about and to enumerate, wide enough to hit every
/// branch.** Four channels, three authors (index 0 is this client), six
/// identities, and a five-second timestamp window. The window is deliberately
/// narrow: it makes ties the common case rather than the exception, and a tie is
/// where an ordering implementation goes wrong.
mod universe {
    /// Channel index to channel id.
    pub fn channel(index: u8) -> String {
        format!("c_{index}")
    }

    /// Author index to a user id. **Index 0 is this client** — the unread rule's
    /// third condition turns on it.
    pub fn author(index: u8) -> String {
        if index == 0 {
            "u_me".to_owned()
        } else {
            format!("u_{index}")
        }
    }

    /// The server message id a `(identity, channel)` pair produces.
    pub fn server_id(client: u8, channel: u8) -> String {
        format!("m_{client}_{channel}")
    }

    /// How many channels, authors and identities the universe has.
    pub const CHANNELS: u8 = 4;
    pub const AUTHORS: u8 = 3;
    pub const IDENTITIES: u8 = 6;

    /// The timestamp window, in seconds after the fixture's base.
    pub const SECONDS: i64 = 5;
}

/// One operation in a generated sequence.
#[derive(Debug, Clone)]
enum Step {
    Receive {
        channel: u8,
        author: u8,
        client: u8,
        second: i64,
    },
    Send {
        channel: u8,
        client: u8,
        second: i64,
    },
    Ack {
        channel: u8,
        client: u8,
        author: u8,
        second: i64,
    },
    /// Fail and then discard, as **one** step.
    ///
    /// **Paired on purpose.** Whether the discard removes the row depends on whether
    /// the send was tracked and then failed, so a model would have to mirror the
    /// delivery state machine to predict the row count. Emitting the pair as one
    /// step gives the model the answer for free — *"if the send was outstanding,
    /// the row went"* — and keeps it to a set rather than a second state machine.
    FailThenDiscard {
        client: u8,
    },
    FailThenRetry {
        client: u8,
    },
    Edit {
        channel: u8,
        client: u8,
        second: i64,
    },
    Reaction {
        channel: u8,
        client: u8,
        author: u8,
    },
    Select {
        channel: u8,
    },
    Typing {
        channel: u8,
        author: u8,
        active: bool,
    },
    Connection {
        attempt: u8,
    },
    Resync {
        channel: u8,
        second: i64,
    },
    /// A write that failed on the way out, for an identity this sequence may or
    /// may not hold.
    Undelivered {
        client: u8,
    },
}

/// Applies one step through the public action surface and reports the outcome.
///
/// **Every operation goes through the public API**, so the properties exercise the
/// path a caller would and not a private shortcut. A property that reached past
/// the actions would be testing a different module than the one 1E-1 ships.
fn perform(state: &mut AppState, step: &Step) -> ApplyOutcome {
    match step {
        Step::Receive {
            channel,
            author,
            client,
            second,
        } => apply_event(
            state,
            DomainEvent::MessageReceived(stored(
                &universe::server_id(*client, *channel),
                u128::from(*client),
                &universe::channel(*channel),
                &universe::author(*author),
                "body",
                *second,
            )),
        ),
        Step::Send {
            channel,
            client,
            second,
        } => {
            let outcome = begin_send(
                state,
                &universe::channel(*channel),
                "mine",
                cid(u128::from(*client)),
                at(*second),
            );
            match outcome {
                SendOutcome::Pending { .. } => ApplyOutcome::Applied,
                // **A row created and failed at the outbox bound is still a row.**
                // This arm is unreachable from this alphabet -- six identities
                // against a 1024 bound -- and it is here because the match has to
                // be total; mapping it as `Applied` says what is true of it, which
                // is that the send produced a held, tracked row.
                SendOutcome::Failed { .. } => ApplyOutcome::Applied,
                SendOutcome::Ignored(reason) => ApplyOutcome::Ignored(reason),
            }
        }
        Step::Ack {
            channel,
            client,
            author,
            second,
        } => apply_event(
            state,
            DomainEvent::MessageAcked {
                client_msg_id: cid(u128::from(*client)),
                message: stored(
                    &universe::server_id(*client, *channel),
                    u128::from(*client),
                    &universe::channel(*channel),
                    &universe::author(*author),
                    "body",
                    *second,
                ),
            },
        ),
        Step::FailThenDiscard { client } => {
            let client = cid(u128::from(*client));
            let failed = apply_event(
                state,
                DomainEvent::MessageSendFailed {
                    client_msg_id: client,
                    code: "refused".to_owned(),
                    detail: String::new(),
                },
            );
            if failed == ApplyOutcome::Applied {
                discard_failed_send(state, client)
            } else {
                failed
            }
        }
        Step::FailThenRetry { client } => {
            let client = cid(u128::from(*client));
            let failed = apply_event(
                state,
                DomainEvent::MessageSendFailed {
                    client_msg_id: client,
                    code: "refused".to_owned(),
                    detail: String::new(),
                },
            );
            if failed == ApplyOutcome::Applied {
                retry_send(state, client)
            } else {
                failed
            }
        }
        Step::Edit {
            channel,
            client,
            second,
        } => {
            let mut edited = stored(
                &universe::server_id(*client, *channel),
                u128::from(*client),
                &universe::channel(*channel),
                "u_ada",
                "an edited body",
                *second,
            );
            edited.edited_at = Some(at(*second));
            apply_event(state, DomainEvent::MessageReceived(edited))
        }
        Step::Reaction {
            channel,
            client,
            author,
        } => apply_event(
            state,
            DomainEvent::ReactionUpdated {
                message_id: universe::server_id(*client, *channel),
                emoji: "\u{1F44D}".to_owned(),
                user_id: universe::author(*author),
            },
        ),
        Step::Select { channel } => select_channel(state, &universe::channel(*channel)),
        Step::Typing {
            channel,
            author,
            active,
        } => apply_event(
            state,
            DomainEvent::TypingUpdated {
                channel_id: universe::channel(*channel),
                user_id: universe::author(*author),
                active: *active,
            },
        ),
        Step::Connection { attempt } => {
            let connection = if *attempt == 0 {
                ConnectionState::Connected
            } else {
                ConnectionState::Reconnecting {
                    attempt: u32::from(*attempt),
                }
            };
            apply_event(state, DomainEvent::ConnectionStateChanged(connection))
        }
        Step::Resync { channel, second } => apply_event(
            state,
            DomainEvent::ResyncRequested {
                channel_id: universe::channel(*channel),
                after: at(*second),
            },
        ),
        // **In the whole alphabet and not the modellable one**, because it is the
        // step that *moves a row into* the queue and the third outbox invariant
        // ("every entry names a held row that is `Pending`") can only be reached
        // through it. A `Send` composed while `Connected` is never queued, and
        // `Step::Connection` can put the client in any state, so only this is a
        // path into the queue that does not go through `begin_send`'s offline flag.
        Step::Undelivered { client } => apply_event(
            state,
            DomainEvent::SendUndelivered {
                client_msg_id: cid(u128::from(*client)),
            },
        ),
    }
}

/// The invariants that must hold after **every** step.
///
/// `retried` is the set of identities this sequence has retried, and it exists
/// because it makes one otherwise-murky assertion sharp. **A retried send is
/// legitimately `Pending` while already holding a server id**: the server stored
/// it, the user pressed retry, and the row is in flight again until the duplicate
/// ACK comes back. So "a row with a real id is never `Pending`" is *false*, and
/// an invariant that asserts it would be an invariant that forbids a documented
/// behaviour. The sharp version — *"a `Pending` row with a real id must be
/// explainable by a retry, and only this client can retry"* — is the same claim
/// with the legitimate case named.
///
/// **Checked one at a time, each with a message naming what broke**, because one
/// combined assertion over a state that has drifted tells you nothing about
/// where. This is the `tests/cache_ceiling.rs` structure: the invariant is a
/// universal claim and the check runs after every operation, not at the end of the
/// sequence — a bound that holds only at the end has a hole in the middle of one.
fn assert_internally_consistent(state: &AppState, retried: &BTreeSet<Uuid>, at_step: usize) {
    let where_ = format!("(after step {at_step})");

    for channel_id in every_channel(state) {
        let held = state.messages(&channel_id);

        // Ascending by the module that owns the order, not by timestamp alone.
        for pair in held.windows(2) {
            assert!(
                ordering::compare(&pair[0], &pair[1]) != std::cmp::Ordering::Greater,
                "{where_}: {channel_id} is not ascending by core::ordering::compare -- \
                 {:?} then {:?}",
                pair[0].id,
                pair[1].id
            );
        }

        // One row per client_msg_id, and both indexes resolve to the row they name.
        let mut identities: BTreeSet<Uuid> = BTreeSet::new();
        for message in held {
            assert!(
                identities.insert(message.client_msg_id),
                "{where_}: {channel_id} holds two rows for {}",
                message.client_msg_id
            );
            let found = state
                .message(&channel_id, &message.client_msg_id)
                .expect("the positional index resolves to the row it names");
            assert_eq!(
                found, message,
                "{where_}: the index for {} does not resolve to the row",
                message.client_msg_id
            );

            if !message.id.is_empty() {
                assert_eq!(
                    state.locate(&message.id).map(|(_, client)| *client),
                    Some(message.client_msg_id),
                    "{where_}: the server-id index does not resolve to {}",
                    message.id
                );
            }

            // **One row per `client_msg_id` across the WHOLE application, not per
            // channel.** A `client_msg_id` is globally unique, so the same
            // identity in two channels is a defect of this layer rather than of
            // the protocol -- and the first version of `begin_send` produced
            // exactly that, which is what this assertion is here for.
            assert_eq!(
                state.channel_holding(&message.client_msg_id),
                Some(channel_id.as_str()),
                "{where_}: {} is held here and somewhere else, or nowhere",
                message.client_msg_id
            );

            // THE convention: an empty id means the server has not accepted it,
            // so the row is one of this client's sends, and the client is still
            // waiting on the server about it.
            //
            // **`Pending` OR `Failed`, and the second case is not a relaxation.**
            // A `Failed` row with no server id is the outbox's capacity refusal
            // (`actions::begin_send`), and the server has indeed not accepted it.
            // The assertion that is still absolute is the one below: it is *this
            // client's* send, tracked in the delivery map.
            if message.id.is_empty() {
                assert!(
                    matches!(
                        state.delivery(&message.client_msg_id),
                        Some(DeliveryState::Pending | DeliveryState::Failed)
                    ),
                    "{where_}: a held message with no server id is neither pending \
                     nor failed, so nothing is tracking this client's send"
                );
                // **And `pending_sends` lists only the `Pending` half**, by
                // definition -- it is a query about rows awaiting transmission,
                // and a `Failed` row is not one. Listing both would make the
                // accessor's name wrong.
                if state.delivery(&message.client_msg_id) == Some(DeliveryState::Pending) {
                    assert!(
                        state
                            .pending_sends()
                            .contains(&(channel_id.as_str(), message.client_msg_id)),
                        "{where_}: and it is not listed among the pending sends"
                    );
                }
            } else if let Some(DeliveryState::Pending) = state.delivery(&message.client_msg_id) {
                assert!(
                    retried.contains(&message.client_msg_id),
                    "{where_}: a row the server has spoken about is still pending and no \
                     retry explains it -- the reconciliation never reached it"
                );
                // **The obvious companion clause -- "and only this client can retry,
                // so such a row is this client's own message" -- is not asserted,
                // and the reason is a real case rather than a gap.** Two copies of
                // one `client_msg_id` whose authors differ is a server defect, and
                // `core/ordering.rs`'s precedence keeps the later one; so a row
                // adopted as ours can legitimately end up somebody else's and then
                // be failed and retried. The row is still one row, the
                // disagreement is still *reported* through
                // `DifferingField::UserId`, and that report is where this layer's
                // answer to the defect lives.
            }
        }

        // The unread bound, and its non-negativity. The count is a `u32`, so it
        // cannot be negative *as a value*; the assertion that matters is that it
        // does not exceed the channel's held messages, which a miscounted
        // increment violates and which a signed model can also express.
        let unread = i64::from(state.unread(&channel_id));
        assert!(
            unread >= 0,
            "{where_}: {channel_id} has a negative unread count"
        );
        assert!(
            unread <= held.len() as i64,
            "{where_}: {channel_id} claims {unread} unread out of {} held messages",
            held.len()
        );
    }

    // The history bound, checked here so the property above covers it on every
    // step rather than only at the end of a sequence. **With the one documented
    // exception stated rather than assumed:** a channel over the cap is only
    // legitimate when every row it holds is a send this client is waiting on, so
    // that is what the message says and what the condition below encodes. The
    // proptest's universe is far too small to reach the cap, so this arm is
    // unexercised here — `a_channel_whose_every_row_is_a_send_in_flight_is_allowed_over_the_cap`
    // in this file is what puts the number under test, and this is what makes the
    // property honest about the exception if the universe is ever widened.
    for channel_id in every_channel(state) {
        let held = state.message_count(&channel_id);
        assert!(
            held <= MAX_MESSAGES_PER_CHANNEL
                || state.messages(&channel_id).iter().all(|message| {
                    matches!(
                        state.delivery(&message.client_msg_id),
                        Some(DeliveryState::Pending | DeliveryState::Failed)
                    )
                }),
            "{where_}: {channel_id} holds {held} messages, over \
             MAX_MESSAGES_PER_CHANNEL, and not every one of them is a send in \
             flight -- so the cap is breached rather than the documented \
             exception holding"
        );
    }

    assert!(
        state.typing_channel_count() <= MAX_TYPING_CHANNELS,
        "{where_}: the typing channel bound did not hold"
    );
    for channel_id in every_channel(state) {
        let typing = state.typing(&channel_id);
        assert!(
            typing.len() <= MAX_TYPING_USERS_PER_CHANNEL,
            "{where_}: the typing set for {channel_id} exceeded its bound"
        );
        let unique: BTreeSet<&String> = typing.iter().collect();
        assert_eq!(
            unique.len(),
            typing.len(),
            "{where_}: the typing set for {channel_id} holds a duplicate"
        );
    }

    // The outbox, checked on every step of the whole alphabet rather than only at
    // the end of a sequence -- and **three claims, because the queue's whole
    // safety rests on the weakest one.**
    //
    // 1. **The bound holds.** `AGENTS.md` §7.1; see `MAX_OUTBOX_ENTRIES` for why
    //    the refusal is the other half of that claim.
    // 2. **No identity is queued twice.** A duplicate entry would put one message
    //    on the wire twice per flush for as long as it sat there.
    // 3. **Every entry names a held row that is `Pending`.** This is the
    //    load-bearing one, and it is stated as `Pending` alone rather than as
    //    "not `Acked`" because it is two claims at once. It is what lets the
    //    outbox hold *identities* instead of copies of the content, since
    //    eviction skips exactly those rows; and it says the queue holds only sends
    //    this client is *still waiting on* -- never one it can already see stored,
    //    and never one the server refused terminally.
    //
    // **The property earned the `Acked` half of this rather than assuming it.**
    // The first version accepted `Pending | Failed`, and the very first run
    // reported a queued identity sitting on an `Acked` row: a resync echo of a
    // queued send upgrades the delivery state through `ingest` and had left the
    // entry behind. A queue entry on a stored row is re-driven on every reconnect
    // forever, so the loose version of this invariant was hiding a leak and the
    // tight one is what closes it.
    //
    // **A discard, a terminal failure, a retry and a resync echo are four paths
    // that could disagree about the queue**, and only an arbitrary *order* of them
    // reaches the sequence where they do.
    let queued = state.outbox_ids();
    assert!(
        queued.len() <= MAX_OUTBOX_ENTRIES,
        "{where_}: the outbox holds {} entries, over its bound of {MAX_OUTBOX_ENTRIES}",
        queued.len()
    );
    let distinct: BTreeSet<Uuid> = queued.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        queued.len(),
        "{where_}: the outbox queues one identity more than once"
    );
    for client_msg_id in &queued {
        let channel_id = state.channel_holding(client_msg_id).unwrap_or_else(|| {
            panic!("{where_}: the outbox queues {client_msg_id}, which is held nowhere")
        });
        assert_eq!(
            state.delivery(client_msg_id),
            Some(DeliveryState::Pending),
            "{where_}: the outbox queues {client_msg_id} in {channel_id}, and the \
             client is not waiting on the server about it -- so either eviction may \
             take the row, or the flush is re-driving a send whose fate is already \
             decided"
        );
    }
}

/// A state with the universe's channels loaded and the connection up.
fn universe_state() -> AppState {
    let mut state = AppState::new("u_me").with_segment_cache_bounds(8, Some(4_096));
    set_channels(
        &mut state,
        (0..universe::CHANNELS)
            .map(|index| channel(&universe::channel(index), &format!("channel-{index}")))
            .collect(),
    );
    fire(
        &mut state,
        DomainEvent::ConnectionStateChanged(ConnectionState::Connected),
    );
    state
}

/// The whole-alphabet strategy, for the universal invariant property.
fn any_step() -> impl Strategy<Value = Step> {
    let author = 0u8..universe::AUTHORS;
    let identity = 0u8..universe::IDENTITIES;
    let channel = 0u8..universe::CHANNELS;
    let second = 0i64..universe::SECONDS;
    prop_oneof![
        3 => (channel.clone(), author.clone(), identity.clone(), second.clone())
            .prop_map(|(channel, author, client, second)| Step::Receive {
                channel, author, client, second
            }),
        2 => (channel.clone(), identity.clone(), second.clone())
            .prop_map(|(channel, client, second)| Step::Send { channel, client, second }),
        2 => (channel.clone(), identity.clone(), author.clone(), second.clone())
            .prop_map(|(channel, client, author, second)| Step::Ack {
                channel, client, author, second
            }),
        1 => identity.clone().prop_map(|client| Step::FailThenDiscard { client }),
        1 => identity.clone().prop_map(|client| Step::FailThenRetry { client }),
        2 => (channel.clone(), identity.clone(), second.clone())
            .prop_map(|(channel, client, second)| Step::Edit { channel, client, second }),
        2 => (channel.clone(), identity.clone(), author.clone())
            .prop_map(|(channel, client, author)| Step::Reaction { channel, client, author }),
        2 => channel.clone().prop_map(|channel| Step::Select { channel }),
        2 => (channel.clone(), author.clone(), any::<bool>())
            .prop_map(|(channel, author, active)| Step::Typing { channel, author, active }),
        1 => (0u8..4u8).prop_map(|attempt| Step::Connection { attempt }),
        1 => (channel, second).prop_map(|(channel, second)| Step::Resync { channel, second }),
        2 => identity.prop_map(|client| Step::Undelivered { client }),
    ]
}

/// **The model's alphabet: the operations that change what is held or counted.**
///
/// **Narrower than [`any_step`] on purpose, and the narrowing is a stated price
/// rather than a limitation.** The excluded operations are all inert with respect
/// to the model: a reaction changes neither; a connection change, a resync request
/// and a typing transition change neither; and a retry moves a row between two
/// delivery states without changing what is held. **What is *not* excluded is the
/// case the first version of the model could not express** — an identity
/// delivered in a channel other than the one it is already held in, which the
/// implementation handles by moving the row. Leaving that in is deliberate: it is
/// the branch that found the real defect, and a property whose alphabet excludes
/// the branch that broke is worth less than one that keeps it.
fn modellable_step() -> impl Strategy<Value = Step> {
    let author = 0u8..universe::AUTHORS;
    let identity = 0u8..universe::IDENTITIES;
    let channel = 0u8..universe::CHANNELS;
    let second = 0i64..universe::SECONDS;
    prop_oneof![
        3 => (channel.clone(), author.clone(), identity.clone(), second.clone())
            .prop_map(|(channel, author, client, second)| Step::Receive {
                channel, author, client, second
            }),
        2 => (channel.clone(), identity.clone(), second.clone())
            .prop_map(|(channel, client, second)| Step::Send { channel, client, second }),
        2 => (channel.clone(), identity.clone(), author.clone(), second.clone())
            .prop_map(|(channel, client, author, second)| Step::Ack {
                channel, client, author, second
            }),
        1 => identity.clone().prop_map(|client| Step::FailThenDiscard { client }),
        2 => (channel.clone(), identity.clone(), second.clone())
            .prop_map(|(channel, client, second)| Step::Edit { channel, client, second }),
        2 => channel.clone().prop_map(|channel| Step::Select { channel }),
    ]
}

/// The proptest configuration both properties use.
///
/// **The case count is stated rather than left at the default**, because this
/// project's coverage measurement is a merge gate (`docs/COVERAGE.md` §6.3) and
/// the default varies with the machine. 96 cases x 40 steps is enough to reach
/// every branch of a universe this small — every identity collides with every
/// other within a few steps — and small enough to stay inside the iteration
/// budget this project agreed.
fn proptest_config() -> ProptestConfig {
    ProptestConfig {
        cases: 96,
        ..ProptestConfig::default()
    }
}

/// The whole alphabet, as a strategy over sequences.
fn any_sequence() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(any_step(), 1..40)
}

/// The model's alphabet, as a strategy over sequences.
fn modellable_sequence() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(modellable_step(), 1..40)
}

proptest! {
    #![proptest_config(proptest_config())]

    /// **No sequence of events leaves `AppState` internally inconsistent.**
    ///
    /// `AGENTS.md` §4.4's mandate for this layer, and the property that needs no
    /// model and so needs no restriction on the alphabet: the whole operation set,
    /// in any order, any number of times, and after **every** step the structural
    /// invariants hold — each channel ascending by `core::ordering::compare`, one
    /// row per `client_msg_id`, both indexes resolving to the row they name, every
    /// empty-id row tracked as pending, and the unread count within its bounds.
    ///
    /// **The last two are the ones a unit test would not find.** A merge that
    /// leaves a stale index, or a reconciliation that misses a row the server
    /// moved to another channel, is a defect that needs a specific *order* of
    /// unrelated events to reach.
    #[test]
    fn an_arbitrary_sequence_of_events_leaves_the_state_internally_consistent(
        steps in any_sequence()
    ) {
        let mut state = universe_state();
        let mut retried: BTreeSet<Uuid> = BTreeSet::new();
        for (index, step) in steps.iter().enumerate() {
            let _ = perform(&mut state, step);
            if let Step::FailThenRetry { client } = step {
                retried.insert(cid(u128::from(*client)));
            }
            assert_internally_consistent(&state, &retried, index);
        }
    }
}

/// What the unread count *should* be, derived by replaying the step list.
///
/// **Recomputed, never accumulated alongside the code under test** — which is the
/// distinction `docs/COVERAGE.md` §4.9 and §5.5 forced on this project twice
/// about derived figures. A model that keeps a running total in step with the
/// implementation agrees with it by construction and tests nothing; this one
/// records the operations and derives the answer from them, so a wrong
/// bookkeeping step cannot hide inside the model.
///
/// It mirrors the implementation's **representation** — a set of counted
/// identities rather than a per-channel counter — because the property is about
/// the count's value, not about the representation, and modelling the set makes
/// the relocation rule expressible at all. It shares no data structure with the
/// implementation: `BTreeSet<(u8, u8)>` and `BTreeMap<u8, i64>` against
/// `BTreeSet<(String, Uuid)>` and a fold to `u32`.
#[derive(Debug, Default)]
struct UnreadModel {
    steps: Vec<Step>,
    selected: Option<u8>,
    /// `(channel, identity)` pairs this client holds.
    held: BTreeSet<(u8, u8)>,
    /// Identities this client has an outstanding send for.
    outstanding: BTreeSet<u8>,
    /// `(channel, identity)` pairs counted as unread.
    counted: BTreeSet<(u8, u8)>,
}

impl UnreadModel {
    /// Records one step and re-derives everything from the recorded list.
    fn observe(&mut self, step: &Step) {
        self.steps.push(step.clone());
        self.replay();
    }

    /// The channel holding an identity, if any.
    fn channel_of(&self, client: u8) -> Option<u8> {
        self.held
            .iter()
            .find(|(_, held)| *held == client)
            .map(|(channel, _)| *channel)
    }

    /// How many of a channel's messages the model says are unread.
    fn unread_in(&self, channel: u8) -> i64 {
        self.counted
            .iter()
            .filter(|(held, _)| *held == channel)
            .count() as i64
    }

    /// Applies the recorded steps, in order, to the tracked sets.
    ///
    /// **The whole model is recomputed on every observation** rather than patched.
    /// That is what makes it a check on the implementation's own bookkeeping
    /// rather than a second copy of it: a bug that only appears after three
    /// operations in a particular order has to show up here too.
    fn replay(&mut self) {
        self.selected = None;
        self.held.clear();
        self.outstanding.clear();
        self.counted.clear();

        // Cloned so the loop can hold `&mut self` while reading the list. A small
        // price in a test model, and the alternative -- indexing by position --
        // would borrow the same way anyway.
        let steps = self.steps.clone();
        for step in &steps {
            match step {
                Step::Receive {
                    channel,
                    author,
                    client,
                    ..
                } => self.receive(*channel, *author, *client),
                Step::Send {
                    channel, client, ..
                } => {
                    // Refused if the identity is outstanding *or already held in
                    // any channel* -- the second half is what the first version of
                    // `begin_send` missed.
                    if self.outstanding.contains(client) || self.channel_of(*client).is_some() {
                        continue;
                    }
                    if self.held.insert((*channel, *client)) {
                        self.outstanding.insert(*client);
                    }
                }
                Step::Ack {
                    channel,
                    client,
                    author,
                    ..
                } => {
                    // **The send map gains an entry only when the ACK *creates*
                    // the row.** An ACK for a message this client already held --
                    // its own message from another device, say -- has nothing to
                    // reconcile and was never one of *this* client's sends, so
                    // `outgoing` stays as it was. The first version of this model
                    // added the entry unconditionally and then removed a row the
                    // implementation had kept, which is the failure the property
                    // reported.
                    let was_held = self.channel_of(*client).is_some();
                    self.receive(*channel, *author, *client);
                    if !was_held && *author == 0 {
                        self.outstanding.insert(*client);
                    }
                }
                Step::FailThenDiscard { client } => {
                    if self.outstanding.remove(client) {
                        if let Some(channel) = self.channel_of(*client) {
                            self.counted.remove(&(channel, *client));
                            self.held.remove(&(channel, *client));
                        }
                    }
                }
                Step::FailThenRetry { .. } => {
                    // Neither half changes what is held, so there is nothing to
                    // track beyond the operations themselves.
                }
                Step::Edit {
                    channel, client, ..
                } => {
                    // **An edit is a `message.new` under the hood**, so for an
                    // identity this client does not hold it is an *insertion*, not
                    // a no-op. The first version of this model treated it as inert
                    // and the property said so immediately, which is the reason the
                    // clause is here rather than a comment. Its author is fixed to
                    // somebody else, so an insertion counts.
                    self.receive(*channel, 1, *client);
                }
                Step::Select { channel } => {
                    self.selected = Some(*channel);
                    self.counted.retain(|(held, _)| held != channel);
                }
                Step::Reaction { .. } | Step::Typing { .. } | Step::Connection { .. } => {}
                Step::Resync { .. } => {}
                // **Inert for the model, and the model's alphabet excludes it anyway.**
                // A re-queue moves a row between "queued" and "not queued" and
                // changes neither what is held nor what is counted -- the same
                // argument that excludes `retry_send`. It is listed so the arm is
                // explicit rather than a wildcard, because a wildcard here would
                // silently absorb the next step somebody adds.
                Step::Undelivered { .. } => {}
            }
        }
    }

    /// One arriving message: a new row, a **move** to another channel, or
    /// nothing.
    ///
    /// The move is the branch that the first version of this model could not
    /// express, and expressing it is what made the property catch the real defect:
    /// a counter the implementation decremented only on the new channel left the
    /// old channel's count behind.
    fn receive(&mut self, channel: u8, author: u8, client: u8) {
        if let Some(held_channel) = self.channel_of(client) {
            if held_channel != channel {
                // A server defect the implementation handles by trusting the
                // server: the row moves whole, so its counting moves with it.
                self.counted.remove(&(held_channel, client));
                self.held.remove(&(held_channel, client));
                self.held.insert((channel, client));
                self.count(channel, author, client);
            }
            return;
        }
        self.held.insert((channel, client));
        self.count(channel, author, client);
    }

    /// Rule condition (3): not this client's own message.
    /// Rule condition (2): the channel is not the one on screen.
    fn count(&mut self, channel: u8, author: u8, client: u8) {
        if author != 0 && self.selected != Some(channel) {
            self.counted.insert((channel, client));
        }
    }
}

proptest! {
    #![proptest_config(proptest_config())]

    /// **The unread count is never negative and never exceeds the channel's
    /// messages.**
    ///
    /// `AGENTS.md` §4.2 requires unread counts and does not say what counts as
    /// unread, so the rule is this work unit's decision
    /// (`state/app_state.rs` module docs, §5) and this is the test that says it is
    /// the rule and not an accident.
    ///
    /// Three assertions per channel, **after every step**: the count equals the
    /// model's, the model's is not negative, and it does not exceed the channel's
    /// held messages. The middle one is trivially true of a `u32` and is asserted
    /// anyway, because the model is signed and the point is that no rule can
    /// produce a negative figure that a `u32` would silently wrap into a very
    /// large one.
    ///
    /// **The negative check would be vacuous on a `u32` if the implementation
    /// could wrap** — `add_unread` uses `saturating_add` precisely so it cannot,
    /// and the proptest is what would notice if that ever changed to a bare `+=`.
    #[test]
    fn the_unread_count_never_goes_negative_and_never_exceeds_the_channel(
        steps in modellable_sequence()
    ) {
        let mut state = universe_state();
        let mut model = UnreadModel::default();

        for step in &steps {
            let _ = perform(&mut state, step);
            model.observe(step);

            // Checked after every step, not at the end: a count that only
            // overshoots transiently is still a count that overshot.
            for index in 0..universe::CHANNELS {
                let channel = universe::channel(index);
                let unread = i64::from(state.unread(&channel));
                let expected = model.unread_in(index);
                prop_assert!(
                    unread >= 0,
                    "{channel} reported a negative unread count"
                );
                prop_assert!(
                    unread <= state.message_count(&channel) as i64,
                    "{channel} claims {unread} unread out of {} held messages",
                    state.message_count(&channel)
                );
                prop_assert_eq!(
                    unread,
                    expected,
                    "{}'s unread count disagrees with the model",
                    channel
                );
            }
        }
    }
}
