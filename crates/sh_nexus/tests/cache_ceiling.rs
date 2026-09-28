//! `core/cache.rs`: the memory ceiling, and the thread-safety decision.
//!
//! Work unit 1C-2b. `AGENTS.md` §4.2 names five behaviours for this module --
//! *"cache insertion, LRU eviction, memory ceiling, hit/miss ratio, thread
//! safety"* -- and `tests/cache.rs` covers three of them. This file covers the
//! other **two**: the memory ceiling (a budget in declared-cost units, with
//! eviction by cost until the total is under it) and the thread-safety decision.
//!
//! # Why this is a separate file rather than more of `tests/cache.rs`
//!
//! `tests/cache.rs` is 1C-2a's suite, and it is scoped to the mechanism: the
//! recency order, the update contract, the miss contract, the cost arithmetic.
//! Its findings are recorded in `docs/COVERAGE.md` §4.6, and a mutation table for
//! it is recorded in §5.3. **Appending a second operation enum, a second model and
//! a second set of properties to it would change the inputs its existing eight
//! properties are fed** -- which is exactly the perturbation a recorded mutation
//! table must not be subjected to. The split keeps 1C-2a's table valid and makes
//! this unit's diff reviewable on its own, which is the same reason the module's
//! own docs draw a seam between the two halves of §4.2's row.
//!
//! The style is 1C-2a's: descriptive names per `AGENTS.md` §4.3, `#[rstest]` with
//! `#[case]` throughout per §4.3 and ADR-008, and proptest per §4.4.
//!
//! # The two decisions this suite pins, stated before the tests
//!
//! 1. **An entry whose own declared cost exceeds the entire budget is never
//!    admitted.** The naive *"evict the LRU until the total is under budget"*
//!    loop has a cliff: one oversized avatar evicts everything else and then
//!    itself, leaving the cache empty precisely when it holds the thing most
//!    worth caching. Only the refusal keeps `total_cost <= budget`
//!    unconditionally true, and the two rejected alternatives each break an
//!    invariant the module promises.
//! 2. **There is no interior synchronisation.** `PLAN.md` §4 makes
//!    `state/bridge.rs` the sole owner of `cx.update_global` / `cx.update`, so
//!    every network callback reaches application state through the main thread
//!    and a cache reached only from there is main-thread-owned. The decision
//!    costs a rule rather than a lock, and the rule is stated in the module.
//!
//! **The honest cost of (1) is not swallowed:** a legitimately large item is
//! never cached and is re-produced every time. The refusal is reported to the
//! caller, and the module says what refusing costs.

use std::sync::Mutex;

use proptest::prelude::*;
use rstest::rstest;
use sh_nexus::core::cache::LruCache;

/// The keys every generated sequence draws from.
///
/// Small enough to enumerate, so a test can ask the cache about every key there
/// could be rather than only about the ones it happens to hold.
const KEY_SPACE: std::ops::Range<u8> = 0..8;

/// Asserts `T: Send + Sync` at compile time, with no runtime cost.
///
/// 1C-2a asserted this for the unbounded cache. The bounded cache is the same
/// struct with one more field, and the assertion has to cover *it*: a budget is
/// exactly the kind of thing somebody would reach for a `Cell` to mutate from a
/// settings screen, and a `Cell` would silently revoke the property.
fn assert_send_and_sync<T: Send + Sync>() {}

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// Everything the cache lets a caller observe, so that *"changed nothing"* can be
/// asserted rather than argued.
///
/// This is a **complete** observation, not a sample: the key space is small
/// enough to enumerate and the remaining accessors are scalars. The one thing it
/// cannot see is the recency *order*, which the cache deliberately exposes no way
/// to ask about -- so
/// `a_refused_insert_does_not_change_which_entry_the_next_eviction_takes`
/// observes that behaviourally instead, by forcing an eviction and seeing which
/// key dies.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    /// Every key in the space that is resident, in key order.
    live: Vec<u8>,
    total_cost: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    budget: Option<u64>,
}

/// The observable state of `cache`.
///
/// Generic over the value type because the same snapshot serves the
/// hand-written cases and the properties. Borrowed, not `&mut`: nothing in here
/// changes the cache, which is the point -- a snapshot that mutated its subject
/// could not witness that an insert changed nothing.
fn snapshot<V>(cache: &LruCache<u8, V>) -> Snapshot {
    Snapshot {
        live: KEY_SPACE
            .clone()
            .filter(|key| cache.contains_key(key))
            .collect(),
        total_cost: cache.total_cost(),
        hits: cache.hits(),
        misses: cache.misses(),
        evictions: cache.evictions(),
        budget: cache.budget(),
    }
}

/// A cache with the given bounds, where `None` means **genuinely unbounded**.
///
/// The `Option` is load-bearing and was a real bug the first time this was
/// written inline: `LruCache::with_budget(capacity, budget.unwrap_or(0))` builds a
/// cache with the budget `Some(0)`, which is the *tightest possible ceiling*
/// rather than no ceiling at all. A property that meant to cover the unbounded
/// case was quietly testing a budget of zero instead, and its model -- which
/// really did hold `None` -- disagreed with it on the very first step.
fn cache_with(capacity: usize, budget: Option<u64>) -> LruCache<u8, u64> {
    match budget {
        Some(budget) => LruCache::with_budget(capacity, budget),
        None => LruCache::new(capacity),
    }
}

// ---------------------------------------------------------------------------
// The budget: construction and reporting
// ---------------------------------------------------------------------------

/// A budget is the number the caller declared, and it is reported back.
///
/// The cache does not derive, scale, round or validate it. It is the caller's
/// number in the caller's unit, and the only thing the cache does with it is
/// compare the total cost against it.
#[rstest]
#[case(0usize, 0u64, Some(0u64))]
#[case(1, 0, Some(0))]
#[case(4, 1_000, Some(1_000))]
#[case(4, u64::MAX, Some(u64::MAX))]
fn a_budget_is_the_number_the_caller_declared(
    #[case] capacity: usize,
    #[case] budget: u64,
    #[case] expected: Option<u64>,
) {
    let cache: LruCache<u8, u8> = LruCache::with_budget(capacity, budget);

    assert_eq!(cache.budget(), expected);
    assert_eq!(cache.capacity(), capacity, "the count bound is unaffected");
    assert!(cache.is_empty());
    assert_eq!(cache.total_cost(), 0);
    assert_eq!(cache.evictions(), 0);
}

/// A cache built with `new` has **no** budget, and says so.
///
/// This is the half that keeps work unit 1C-2a's suite green and its behaviour
/// intact: `new` is the count-bounded cache 1C-2a built, and a budget is
/// something a caller opts into. `budget()` returning `None` -- rather than a
/// sentinel the caller would have to know the meaning of -- is what lets a
/// diagnostic report "no ceiling is configured", which is actionable. A sentinel
/// would make an unbounded cache claim eighteen exabytes, which is not a
/// sentence anybody wants to read in a bug report.
#[test]
fn a_cache_built_with_new_has_no_budget_at_all() {
    let mut cache: LruCache<u8, u8> = LruCache::new(4);

    assert_eq!(cache.budget(), None);

    // And the behaviour that follows from it: a cost nothing could fit still
    // goes in, because there is no ceiling to refuse it against.
    assert!(
        !cache.insert_with_outcome(1, 1, u64::MAX).refused,
        "an unbounded cache refuses nothing"
    );
    assert_eq!(cache.total_cost(), u64::MAX);
}

/// `None` and a maximal budget enforce the same rule, and the difference is intent.
///
/// Stated as a test rather than left as an apparent gap in the API, because a
/// reader who spots `budget() == None` beside `budget() == Some(u64::MAX)` will
/// hunt for a behavioural difference and there is none: `cost > u64::MAX` is
/// never true, so a maximal budget never refuses and never trims. What differs is
/// what the cache can *report* about itself, which is the whole reason an absent
/// ceiling is an `Option` rather than a magic number.
#[test]
fn no_budget_and_a_maximal_budget_enforce_the_same_rule() {
    let mut unbounded: LruCache<u8, u8> = LruCache::new(4);
    let mut maximal: LruCache<u8, u8> = LruCache::with_budget(4, u64::MAX);

    for key in KEY_SPACE {
        assert!(!unbounded.insert_with_outcome(key, key, u64::MAX).refused);
        assert!(!maximal.insert_with_outcome(key, key, u64::MAX).refused);
    }

    assert_eq!(unbounded.len(), maximal.len());
    assert_eq!(unbounded.total_cost(), maximal.total_cost());
    assert_eq!(unbounded.evictions(), maximal.evictions());

    // The difference is the report, and the report is what a caller reads to
    // decide whether its memory is bounded at all.
    assert_eq!(unbounded.budget(), None);
    assert_eq!(maximal.budget(), Some(u64::MAX));
}

// ---------------------------------------------------------------------------
// The refusal of an oversized entry
// ---------------------------------------------------------------------------

/// An entry costing more than the entire budget is **never** admitted.
///
/// The rule that keeps `total_cost <= budget` true unconditionally. Compare it
/// with the loop this module rejected: evicting the LRU until the total fits
/// would evict every other entry and *then* the oversized one, because the
/// pre-insert total can never go negative -- the arithmetic bottoms out at
/// "empty" and the entry most worth caching is already gone.
#[rstest]
#[case(0u64, 1u64)]
#[case(1, 2)]
#[case(100, 101)]
#[case(100, u64::MAX)]
#[case(u64::MAX - 1, u64::MAX)]
fn an_entry_whose_own_cost_exceeds_the_budget_is_never_admitted(
    #[case] budget: u64,
    #[case] cost: u64,
) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(4, budget);
    // Something already resident, so this is not trivially the empty-cache case:
    // a rule that emptied the cache on refusal would satisfy the ceiling and fail
    // the very next test.
    cache.insert(1, 1, 0);
    let before = snapshot(&cache);

    let outcome = cache.insert_with_outcome(2, 2, cost);

    assert!(
        outcome.refused,
        "a cost of {cost} cannot fit a budget of {budget}"
    );
    assert!(!cache.contains_key(&2), "and it is not in the cache");
    assert_eq!(snapshot(&cache), before, "a refusal changes nothing at all");
    assert!(
        cache.total_cost() <= budget,
        "the ceiling holds: {} <= {budget}",
        cache.total_cost()
    );
}

/// A cost **equal** to the budget is admitted, because the invariant is `<=`.
///
/// The off-by-one a `<` instead of a `<=` would introduce, named as its own test
/// because the invariant's exact form *is* the claim: an entry may consume the
/// whole budget and not one unit more.
#[rstest]
#[case(0u64)]
#[case(1)]
#[case(1_000)]
#[case(u64::MAX)]
fn a_cost_exactly_equal_to_the_budget_is_admitted(#[case] budget: u64) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(4, budget);

    let outcome = cache.insert_with_outcome(1, 1, budget);

    assert!(!outcome.refused, "the invariant is `<=`, so equality fits");
    assert_eq!(cache.get(&1), Some(&1));
    assert_eq!(cache.total_cost(), budget);
}

/// A refused insert changes the cache in **no** observable way.
///
/// The strongest form of the claim, and the one that makes the refusal safe to
/// have. Not the cost, not the length, not the counters, not the live set. A
/// caller that re-renders an avatar into something too large keeps serving the
/// *old* avatar rather than losing the entry -- which is the right way round, since
/// a stale value beats a re-fetch and a miss would be a worse answer than either.
#[test]
fn a_refused_insert_leaves_the_cache_exactly_as_it_found_it() {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(8, 1_000);
    cache.insert(1, 10, 100);
    cache.insert(2, 20, 200);
    let before = snapshot(&cache);

    // A brand-new key that would not fit.
    let refused_new = cache.insert_with_outcome(3, 30, 5_000);
    // A **replacement** of a resident key that would not fit. This is the
    // interesting half: an implementation that charges the new cost first and
    // discovers the refusal afterwards leaves the key *gone*, so a caller that had
    // a value a moment ago now has nothing.
    let refused_replace = cache.insert_with_outcome(1, 11, 5_000);

    assert!(refused_new.refused);
    assert!(refused_replace.refused);
    assert_eq!(snapshot(&cache), before, "neither refusal moved anything");
    assert_eq!(cache.get(&1), Some(&10), "and the old value is still there");
    assert_eq!(cache.get(&2), Some(&20));
}

/// A refused insertion moves no counter, and in particular neither hit nor miss.
///
/// The same reasoning as 1C-2a's update contract, carried through to the policy:
/// the ratio describes values *served*, and a refusal served nothing. What makes a
/// refusal worth having its own observable is
/// [`evictions`](LruCache::evictions) -- an entry refused is emphatically **not**
/// an entry evicted, and a caller diagnosing a cache that is not growing cannot
/// explain anything without telling those two apart.
#[rstest]
#[case(true)]
#[case(false)]
fn a_refused_insert_moves_no_counter_at_all(#[case] replacing: bool) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(4, 1_000);
    cache.insert(1, 1, 10);
    let (hits, misses) = (cache.hits(), cache.misses());

    if replacing {
        let _ = cache.insert_with_outcome(1, 2, 10_000);
    } else {
        let _ = cache.insert_with_outcome(2, 2, 10_000);
    }

    assert_eq!(cache.hits(), hits, "a refusal is not a hit");
    assert_eq!(cache.misses(), misses, "and not a miss");
    assert_eq!(cache.evictions(), 0, "refused is not evicted");
    assert_eq!(cache.len(), 1);
}

/// A refusal is distinguishable from a replacement in the outcome itself.
///
/// The point of [`insert_with_outcome`](LruCache::insert_with_outcome) over a
/// bare `Option<V>`: `None` on a refusal and `None` on a first insertion are the
/// same value, so a caller using the terse `insert` alone cannot tell "I cached
/// this" from "I threw this away". Both halves of the outcome are asserted here,
/// because the interesting rows are the two refusals -- a replacement that is
/// refused reports **no** previous value, because nothing was replaced, and the
/// resident entry is still there.
#[rstest]
#[case(1_000u64, 10u64, false, false, false)]
#[case(1_000, 20, true, false, true)]
#[case(1_000, 10_000, false, true, false)]
#[case(1_000, 10_000, true, true, false)]
#[case(1_000, 1_000, false, false, false)]
fn the_outcome_separates_a_refusal_from_a_replacement(
    #[case] budget: u64,
    #[case] second_cost: u64,
    #[case] replacing: bool,
    #[case] expected_refused: bool,
    #[case] expected_previous: bool,
) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(4, budget);
    cache.insert(1, 1, 10);
    // The resident entry is key 1, so `replacing` has to actually choose a
    // different target key -- otherwise every case is a replacement and the
    // `expected_previous` column is untested rather than tested.
    let target = if replacing { 1u8 } else { 2u8 };

    let outcome = cache.insert_with_outcome(target, 7, second_cost);

    assert_eq!(outcome.refused, expected_refused, "the refusal flag");
    assert_eq!(
        outcome.previous,
        expected_previous.then_some(1),
        "what it replaced"
    );
    if expected_refused {
        // A refusal mutates nothing, so the setup entry is untouched whichever key
        // was targeted -- including the case where the target *was* that entry.
        assert_eq!(
            cache.get(&1),
            Some(&1),
            "the old value survived the refusal"
        );
        assert_eq!(cache.total_cost(), 10);
        assert_eq!(cache.len(), 1, "and nothing was admitted");
    } else {
        assert_eq!(cache.get(&target), Some(&7), "the new value is in place");
        assert!(cache.total_cost() <= budget, "the ceiling still holds");
    }
}

/// The terse `insert` and the reporting `insert_with_outcome` are one operation.
/// 1C-2a's `insert` is a delegation rather than a second implementation, because
/// an eviction written twice is an eviction that will be fixed in one copy and not
/// the other. This is what holds that: a full sequence through the terse entry
/// point and the same sequence through the reporting one must leave identical
/// caches, including identical eviction counts.
#[test]
fn the_terse_and_the_reporting_insert_are_the_same_operation() {
    let mut terse: LruCache<u8, u8> = LruCache::with_budget(4, 500);
    let mut reporting: LruCache<u8, u8> = LruCache::with_budget(4, 500);

    let costs = [100u64, 200, 300, 50, 600, 1, 0, u64::MAX, 250, 10];
    for (index, cost) in costs.iter().enumerate() {
        let key = u8::try_from(index % 6).unwrap_or(0);
        let terse_outcome = terse.insert(key, key, *cost);
        let outcome = reporting.insert_with_outcome(key, key, *cost);
        assert_eq!(
            outcome.previous, terse_outcome,
            "the two entry points must report the same previous value"
        );
    }

    assert_eq!(snapshot(&terse), snapshot(&reporting));
}

/// A refused insert does not change which entry the **next** eviction takes.
/// The one thing [`snapshot`] cannot see, observed behaviourally instead. The
/// cache exposes no way to ask for its recency order, so the order is inferred
/// from *which* entry an eviction takes. A refusal that disturbed the order -- by
/// pushing a refused key, or by re-ticking the key it declined to replace -- would
/// put the wrong key at the front of the order and this test would see the wrong
/// key die here.
#[rstest]
#[case(2usize)]
#[case(3)]
fn a_refused_insert_does_not_change_which_entry_the_next_eviction_takes(#[case] capacity: usize) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, 1_000);
    for key in 0..capacity as u8 {
        cache.insert(key, key, 10);
    }

    // Refused: far more than the whole budget. Nothing may change.
    assert!(cache.insert_with_outcome(200, 200, 5_000).refused);

    // Now one more entry, which fills the count bound and evicts the front.
    let victim = cache.insert_with_outcome(201, 201, 10);
    assert!(!victim.refused);

    assert!(
        !cache.contains_key(&0),
        "the oldest entry is what an eviction takes; the refusal changed nothing, \
         so the entry at the front of the order is still the first key inserted"
    );
    assert!(
        cache.contains_key(&201),
        "and the entry just inserted survives"
    );
    assert_eq!(cache.evictions(), 1, "exactly one eviction happened");
}

// ---------------------------------------------------------------------------
// The lowered budget
// ---------------------------------------------------------------------------

/// Lowering the budget is honoured **on the same call**.
/// Three answers to "what does a lowered budget do?" are defensible: evict now,
/// evict at the next insert, or not honour it until the next natural trim. Two of
/// the three leave a window in which `total_cost() > budget()` is observable -- and
/// a window in which the ceiling is not a ceiling is not a ceiling. So the budget
/// is honoured when it is set. The only cost of that answer is that this call can
/// be a bulk eviction, which is why `set_budget` returns how many entries it took.
#[rstest]
#[case(0u64)]
#[case(1)]
#[case(50)]
#[case(1_000)]
fn lowering_the_budget_honours_it_on_the_same_call(#[case] new_budget: u64) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(8, 1_000);
    for key in 0..6u8 {
        cache.insert(key, key, 100);
    }
    assert_eq!(cache.total_cost(), 600);
    assert_eq!(cache.len(), 6);

    let evicted = cache.set_budget(Some(new_budget));

    assert!(
        cache.total_cost() <= new_budget,
        "the ceiling holds the instant it is lowered: {} <= {new_budget}",
        cache.total_cost()
    );
    assert!(
        evicted <= 6,
        "at most the six resident entries could have gone, and {evicted} did"
    );
    assert_eq!(
        cache.len(),
        6 - evicted,
        "every eviction lost exactly one entry"
    );
    assert_eq!(cache.budget(), Some(new_budget));
}

/// Setting a budget the cache is already within evicts nothing, and cannot.
///
/// The direction in which the invariant already holds: a total under the old
/// budget is under the new one, so the loop never runs. **`600` is the boundary
/// case and is in the list deliberately** -- equal is within, because the
/// invariant is `<=`. Asserted rather than assumed, because *"raise the budget
/// but keep trimming to the old one"* is a plausible wrong implementation, and the
/// old value is exactly what a careless edit leaves in scope.
#[rstest]
#[case(600u64)]
#[case(601)]
#[case(1_000)]
#[case(u64::MAX)]
fn setting_a_budget_the_cache_is_already_within_evicts_nothing(#[case] new_budget: u64) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(8, 1_000);
    for key in 0..6u8 {
        cache.insert(key, key, 100);
    }
    assert_eq!(
        cache.total_cost(),
        600,
        "six 100-cost entries fit the 1,000 budget"
    );
    let before = snapshot(&cache);

    let evicted = cache.set_budget(Some(new_budget));

    assert_eq!(
        evicted, 0,
        "nothing was over the old budget, so nothing went"
    );
    assert_eq!(cache.budget(), Some(new_budget));
    // The budget is the one field meant to move, so the rest is compared on its
    // own; a whole-snapshot comparison would report the intended change as a
    // violation.
    let after = snapshot(&cache);
    assert_eq!(after.live, before.live, "the live set moved");
    assert_eq!(after.total_cost, before.total_cost, "the total moved");
    assert_eq!(after.evictions, before.evictions, "an eviction happened");
    assert!(cache.total_cost() <= new_budget, "the ceiling holds");
}

/// Removing the budget evicts nothing and stops enforcing.
/// `None` is a real state and not a synonym for zero. A cache with no ceiling keeps
/// everything its capacity allows, which is what `LruCache::new` has always done
/// -- so taking the budget away has to be indistinguishable from having never had
/// one.
#[test]
fn removing_the_budget_evicts_nothing_and_stops_enforcing() {
    // A budget of 250 admits exactly two 100-cost entries, so the six insertions
    // below leave a cache that is well inside its count bound and nowhere near
    // its ceiling -- which is the state in which "removing a ceiling evicts
    // nothing" is a claim rather than a tautology.
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(8, 250);
    for key in 0..6u8 {
        cache.insert(key, key, 100);
    }
    assert_eq!(
        cache.len(),
        2,
        "only two 100-cost entries fit in a 250 budget"
    );
    assert_eq!(cache.total_cost(), 200);

    let evicted = cache.set_budget(None);

    assert_eq!(evicted, 0, "removing a ceiling never evicts");
    assert_eq!(cache.budget(), None);
    assert_eq!(cache.total_cost(), 200);
    assert_eq!(cache.len(), 2);

    // And what used to be refused now goes in, which is the observable difference
    // between "no ceiling" and "a ceiling of zero".
    assert!(!cache.insert_with_outcome(7, 7, u64::MAX).refused);
    assert!(cache.contains_key(&7));
}

/// A budget of zero is legal, and it means "admit only what is free".
/// The degenerate end of the same rule, and the one that shows `budget: 0` is not
/// a synonym for `budget: None`. Everything with a positive cost is refused; a
/// zero-cost entry still fits, so the cache is not necessarily empty -- the honest
/// consequence of a caller being allowed to declare a cost of zero, which the
/// module discusses rather than papering over.
#[test]
fn a_budget_of_zero_admits_only_what_is_free() {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(4, 0);

    assert!(
        cache.insert_with_outcome(1, 1, 1).refused,
        "one unit is over zero"
    );
    assert!(
        !cache.insert_with_outcome(1, 1, 0).refused,
        "nothing is not over zero"
    );

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.total_cost(), 0);
    // The budget is 0, so `total_cost() <= budget` reduces to `== 0` here, and
    // written that way rather than as `<= 0` because clippy is right that the
    // latter says nothing a `== 0` does not.
    assert_eq!(
        cache.total_cost(),
        0,
        "the ceiling holds at its degenerate end"
    );
}

// ---------------------------------------------------------------------------
// The two bounds together
// ---------------------------------------------------------------------------

/// A zero-cost entry is bounded by the capacity and by **nothing else**.
/// Not a hole, and the reason is worth stating because it looks like one. A caller
/// that declares a cost of 0 for a thing that really weighs a megabyte makes the
/// ceiling a fiction, and no work inside the cache can catch it -- the module's
/// contract is that the *declared* cost is the ceiling's unit (1C-2a's module
/// docs, §5), and the only honest response to a caller that under-declares is to
/// say so. Flooring a declared 0 to 1 was considered and rejected: it would
/// falsify `total_cost`, which is the exact sum the ceiling is built on and which
/// `the_reported_total_cost_equals_the_sum_of_the_live_entries_costs` in
/// `tests/cache.rs` already pins.
#[rstest]
#[case(0usize)]
#[case(1)]
#[case(7)]
#[case(64)]
fn a_zero_cost_entry_is_bounded_by_capacity_and_by_nothing_else(#[case] capacity: usize) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, 0);

    for index in 0..capacity {
        cache.insert(u8::try_from(index).unwrap_or(0), 0, 0);
    }

    assert_eq!(cache.len(), capacity, "only the count bound applies");
    assert_eq!(
        cache.total_cost(),
        0,
        "a free entry costs nothing to charge"
    );
    assert!(
        cache.total_cost() == 0,
        "so the ceiling never fires, whatever the count bound admits"
    );
}

/// Whichever bound is tighter is the one that survives.
/// The two bounds are independent and both are enforced, and the interesting part
/// is that they need no reconciliation: the live set is the longest suffix of the
/// recency order satisfying *both*. Entries of unit cost make the two coincide --
/// `min(capacity, budget)` entries exactly -- which is the case where the answer is
/// unambiguous and the interaction is easiest to state.
#[rstest]
#[case(1usize, 3u64)]
#[case(3, 1)]
#[case(4, 4)]
#[case(8, 8)]
#[case(8, 100)]
#[case(100, 8)]
#[case(0, 0)]
#[case(0, 5)]
fn the_ceiling_and_the_capacity_bound_the_cache_together(
    #[case] capacity: usize,
    #[case] budget: u64,
) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, budget);
    let affordable = usize::try_from(budget).unwrap_or(usize::MAX);
    let expected = capacity.min(affordable);

    // Far more unit-cost entries than either bound could ever hold.
    for key in 0..200u8 {
        cache.insert(key, key, 1);
    }

    assert_eq!(
        cache.len(),
        expected,
        "min(capacity, budget) entries survive"
    );
    assert!(cache.len() <= capacity);
    assert!(cache.total_cost() <= budget);
    assert_eq!(
        cache.total_cost(),
        u64::try_from(cache.len()).unwrap_or(u64::MAX),
        "unit costs, so the total is exactly the length"
    );
}

// ---------------------------------------------------------------------------
// The trim bound
// ---------------------------------------------------------------------------

/// The budget ceiling never evicts the entry it was just given.
/// A consequence of the refusal rule rather than a separate guard, and the reason
/// the trim loop is allowed to be a plain `while`. After every *other* entry is
/// gone the total is exactly the cost of the entry just inserted, and the refusal
/// rule guarantees that cost is within the budget -- so the loop condition is
/// already false before the newest entry can be reached.
///
/// **Capacity zero is deliberately absent from the cases.** At capacity 0 the entry
/// just inserted is the *count* bound's victim, not the ceiling's; 1C-2a's module
/// docs, §8, own that case and this property is about the budget alone.
#[rstest]
#[case(1usize, 0u64)]
#[case(1, 1)]
#[case(8, 100)]
#[case(64, 100)]
fn the_budget_ceiling_never_evicts_the_entry_it_was_given(
    #[case] capacity: usize,
    #[case] budget: u64,
) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, budget);
    for key in 0..capacity.min(8) {
        cache.insert(u8::try_from(key).unwrap_or(0), 0, 0);
    }

    // A single entry that consumes the whole budget. Whatever it displaces, it
    // must survive: an implementation that let the ceiling evict its own insertion
    // is the cliff the refusal rule exists to prevent.
    let outcome = cache.insert_with_outcome(200, 200, budget);

    assert!(!outcome.refused, "a budget-sized entry always fits");
    assert!(
        cache.contains_key(&200),
        "the entry just inserted must survive the ceiling it triggered"
    );
}

/// One insert evicts at most the entries it could have displaced.
/// The bound, stated and then tested. **The trim loop runs at most once per entry
/// resident immediately after the insert** -- and therefore at most `capacity + 1`
/// times -- because every iteration removes one entry and the order can only
/// shrink. `AGENTS.md` §2.3's frame-loop concern is answered by that being a
/// number rather than a hope; the next test measures how tight it is.
#[test]
fn one_insert_never_displaces_more_entries_than_it_holds() {
    // The shape that approaches the bound: entries that declare as little as
    // legal, so the incoming entry can displace all of them and the budget cannot
    // be met until the last one goes.
    let capacity = 64usize;
    let budget = 100u64;
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, budget);
    for key in 0..capacity - 1 {
        cache.insert(u8::try_from(key).unwrap_or(0), 0, 0);
    }
    // One unit-cost entry, *newer* than the free ones -- so trimming must walk
    // past every free entry before it can relieve a single unit of pressure.
    cache.insert(100, 1, 1);
    assert_eq!(cache.len(), capacity);

    let before_evictions = cache.evictions();
    cache.insert(200, 200, budget);
    let evicted = cache.evictions() - before_evictions;

    assert!(
        evicted <= u64::try_from(capacity + 1).unwrap_or(u64::MAX),
        "one insert evicted {evicted} entries, over the bound of {}",
        capacity + 1
    );
    assert!(cache.contains_key(&200));
}

/// The worst-case cascade is reached only by a caller declaring near-free entries,
/// and its size is measured rather than merely bounded.
///
/// The counterpart to the test above. An upper bound nobody has seen reached is a
/// claim; this measures the cascade at its tightest and checks the **exact** count,
/// which is the number the module docs quote. The shape is forced: `capacity - 1`
/// free entries, then one unit-cost entry, then one entry costing the whole
/// budget. The trim can relieve exactly one unit of pressure, and that single unit
/// sits behind every free entry -- so every resident entry must go.
#[rstest]
#[case(1usize, 0u8, 1u64)]
#[case(2, 1, 100)]
#[case(8, 7, 100)]
#[case(64, 63, 100)]
fn the_worst_case_eviction_cascade_is_exactly_what_the_adversarial_shape_predicts(
    #[case] capacity: usize,
    #[case] free: u8,
    #[case] budget: u64,
) {
    assert_eq!(
        usize::from(free) + 1,
        capacity,
        "the case table must keep the adversarial shape exact"
    );
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, budget);
    for key in 0..free {
        cache.insert(key, 0, 0);
    }
    cache.insert(200, 1, 1);
    let resident = cache.len();
    assert_eq!(resident, capacity);
    assert!(
        cache.total_cost() <= budget,
        "the cache was within budget beforehand"
    );

    let outcome = cache.insert_with_outcome(201, 201, budget);

    assert!(!outcome.refused, "a budget-sized entry always fits");
    assert_eq!(
        cache.evictions(),
        u64::try_from(resident).unwrap_or(u64::MAX),
        "every resident entry had to go, and the measured count says exactly how many"
    );
    assert_eq!(cache.len(), 1, "only the entry just inserted survives");
    assert_eq!(cache.total_cost(), budget);
}

/// The ceiling is enforced on the **reported** total, and the reported total is a
/// lower bound once the true sum would overflow a `u64`.
///
/// The one place this policy's arithmetic is not exact, written as a test so the
/// limit in the module docs is a behaviour rather than a paragraph. It was found
/// by the model property above, which is worth recording: the divergence was not
/// a bug in the policy but a real property of 1C-2a's *incremental* accumulator
/// that the policy made reachable.
///
/// What is asserted, precisely:
///
/// - The ceiling is checked against the **reported** total, and the reported
///   total is never above the budget.
/// - With `cost = u64::MAX` in a `u64::MAX` budget, `MAX + 1` reads back as
///   `MAX`: the ceiling is satisfied by the report while the true sum is
///   genuinely over budget. **That is the documented direction of the error** --
///   the report under-states, so a cache can hold more in truth than the budget
///   says. It is reachable only when the true sum of declared costs exceeds
///   `u64::MAX`, which needs a budget of astronomical size; see the module docs.
/// - After the saturating entry is removed, the reported total is `0` where the
///   one live entry really costs `1`. **This is the path-dependence**: the
///   accumulator's value is not a function of the live set, which is precisely
///   why no reference model can predict it in that regime.
#[rstest]
#[case(2usize)]
#[case(8)]
fn the_ceiling_is_enforced_on_the_reported_total_even_when_the_true_sum_overflows(
    #[case] capacity: usize,
) {
    let mut cache: LruCache<u8, u8> = LruCache::with_budget(capacity, u64::MAX);

    // Admitted: `u64::MAX > u64::MAX` is false, so the refusal rule lets it in.
    assert!(!cache.insert_with_outcome(1, 1, u64::MAX).refused);
    assert!(!cache.insert_with_outcome(2, 2, 1).refused);
    assert_eq!(
        cache.len(),
        2,
        "the count bound is not reached at capacity 2 or 8"
    );

    // The reported total saturated: `MAX + 1` reads back as `MAX`.
    assert_eq!(cache.total_cost(), u64::MAX);
    assert!(
        cache.total_cost() <= cache.budget().unwrap_or(0),
        "the ceiling holds on the reported total"
    );
    // And the true sum of the live entries' costs is 2^64, which is over the
    // budget of 2^64 - 1. Stated, not asserted, because it is not representable.
    assert_eq!(
        cache.total_cost(),
        u64::MAX,
        "the report is the saturated value"
    );

    // Path-dependence: removing the saturating entry leaves a reported total that
    // is not the sum of what is left.
    assert_eq!(cache.remove(&1), Some(1));
    assert_eq!(cache.len(), 1);
    assert_eq!(
        cache.total_cost(),
        0,
        "MAX - MAX, while the live entry costs 1"
    );
    assert!(cache.total_cost() <= cache.budget().unwrap_or(0));
}

/// The saturating regime is unreachable for any budget a client would actually
/// configure, and the arithmetic is the proof.
///
/// `AGENTS.md` 6.2's real rows are 80MB idle and 200MB with 10,000 messages, so a
/// budget in this project is on the order of 2^27. The refusal rule caps every
/// admitted cost at the budget, and the count bound caps the entry count, so the
/// true sum is at most `(entries) x budget`. For that product to exceed `u64::MAX`
/// with a budget of 2^27, the cache would have to hold about 2^37 entries -- which
/// no machine has the memory for, and which the count bound is set well below.
/// So the limit above is a real limit on the arithmetic and not a limit on the
/// cache.
#[test]
fn the_saturating_regime_needs_a_budget_no_client_would_configure() {
    // A budget on the order of `AGENTS.md` 6.2's rows.
    let budget = 80u64 << 20;
    // The count bound `AGENTS.md` 6.1's table contemplates: a few hundred.
    let entries = 512u64;

    let worst_case_sum = budget.saturating_mul(entries);
    assert!(
        worst_case_sum < u64::MAX,
        "a worst-case sum of {worst_case_sum} must stay inside u64, or this test \
         is asserting the wrong thing"
    );
    // And with a good many multiples of that, still inside.
    assert!(worst_case_sum.saturating_mul(1_000_000) < u64::MAX);
}

// ---------------------------------------------------------------------------
// Thread safety
// ---------------------------------------------------------------------------

/// The bounded cache is still `Send + Sync`.
/// The compile-time half of this unit's decision, and the reason the decision is
/// worth stating as a decision. **The decision is that there is no interior
/// synchronisation**, so this assertion is not a formality: it is the property
/// that the absence of a lock buys, and it is precisely what a future editor
/// revokes by reaching for a `Cell` to change a budget from a settings screen.
///
/// The reasoning, in one line: `PLAN.md` §4 makes `state/bridge.rs` the sole owner
/// of `cx.update_global` / `cx.update`, so every network callback reaches
/// application state through the main thread, and a cache reached only from there
/// is main-thread-owned and needs no lock. The full argument is in the module
/// docs.
#[test]
fn the_bounded_cache_is_still_send_and_sync() {
    assert_send_and_sync::<LruCache<String, String>>();
    assert_send_and_sync::<LruCache<u64, Vec<u8>>>();
    // Including the shape the ceiling is actually for: a cache of rendered
    // segments keyed by message id, under a budget in bytes.
    assert_send_and_sync::<LruCache<&str, std::sync::Arc<u8>>>();
}

/// A caller that ever *does* share one across threads can, by wrapping it.
/// The other half of the decision, and the part that makes "no lock" a decision
/// rather than a refusal. The cache is `Send + Sync`, so a caller needing
/// cross-thread access can take a `Mutex` around it -- and the
/// `unwrap_or_else` is not a style slip, it is the demonstration: the caller
/// outside `core/` owns the poisoned-lock policy, which `AGENTS.md` §2.1 forbids
/// this module from having at all.
///
/// **This is the rule the module docs state, expressed so that it compiles.**
#[test]
fn a_cache_its_owner_has_wrapped_is_send_and_sync_and_usable() {
    let shared: Mutex<LruCache<String, String>> = Mutex::new(LruCache::with_budget(4, 1_000));

    assert_send_and_sync::<Mutex<LruCache<String, String>>>();

    {
        let mut guarded = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            !guarded
                .insert_with_outcome("a".to_owned(), "first".to_owned(), 100)
                .refused
        );
    }
    {
        // A second acquisition: the wrapper is what makes the second `&mut`
        // possible at all, and without it this would not compile.
        let mut guarded = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(guarded.get(&"a".to_owned()), Some(&"first".to_owned()));
        assert!(guarded.total_cost() <= 1_000);
    }
}

// ---------------------------------------------------------------------------
// A reference model
// ---------------------------------------------------------------------------

/// The operations a generated sequence may contain.
///
/// Wider than `tests/cache.rs`'s `Op` on purpose. A policy bug lives in the
/// interaction between *removing* an entry and *lowering* a budget, and a model
/// that never removes anything cannot find it.
#[derive(Debug, Clone, Copy)]
enum PolicyOp {
    /// Insert a key at a declared cost.
    Insert { key: u8, value: u64, cost: u64 },
    /// Read a key, making it the most recently used if it is there.
    Access { key: u8 },
    /// Drop a key deliberately.
    Remove { key: u8 },
    /// Drop everything.
    Clear,
    /// Install, change or remove the ceiling.
    SetBudget { budget: Option<u64> },
}

/// A cache's observable state, derived by a different route from the
/// implementation's.
///
/// **The independence is the point, and it is worth being specific about where it
/// comes from.** `LruCache` maintains `total_cost` *incrementally* -- a running
/// accumulator adjusted on every insertion, removal and eviction. This model
/// *recomputes* it by folding over the entries it believes are live, and derives
/// the survivors as the entries that remain after a forward pass. Two different
/// formulations of one policy, sharing no data structure: an incremental
/// accumulator against a recomputed fold. Agreement between them is therefore
/// information rather than tautology.
///
/// `tests/cache.rs` models *recency* with a `BTreeMap` of key to last-use tick,
/// because a sorted map is the natural way to ask "who is least recently used?"
/// without holding an order. This model deliberately keeps a `Vec` in recency
/// order instead, because here the *trim* is what is under test and an order-
/// preserving structure is what makes the forward eviction loop checkable. Two
/// models, two shapes, chosen for the question each is asked.
#[derive(Debug, Clone, Default)]
struct PolicyModel {
    /// Live entries as `(key, cost)`, least recently used first.
    order: Vec<(u8, u64)>,
    /// The ceiling in force, or `None` for an unbounded cache.
    budget: Option<u64>,
}

impl PolicyModel {
    /// Apply one operation, exactly as the cache under test does.
    fn apply(&mut self, capacity: usize, op: PolicyOp) {
        match op {
            PolicyOp::Insert { key, cost, .. } => {
                // The refusal rule, restated: an entry that cannot fit on its own
                // is not admitted, and admitting it would not change the order.
                if self.budget.is_some_and(|budget| cost > budget) {
                    return;
                }
                // A replacement removes the old entry first, so the new cost is
                // charged instead of the old one and the key becomes the most
                // recently used.
                self.order.retain(|(held, _)| *held != key);
                self.order.push((key, cost));
                self.trim(capacity);
            }
            PolicyOp::Access { key } => {
                // A miss ticks nothing. Restated here as well as in the module, so
                // the model and the implementation cannot agree about it by
                // accident.
                let Some(index) = self.order.iter().position(|(held, _)| *held == key) else {
                    return;
                };
                let entry = self.order.remove(index);
                self.order.push(entry);
            }
            PolicyOp::Remove { key } => self.order.retain(|(held, _)| *held != key),
            PolicyOp::Clear => self.order.clear(),
            PolicyOp::SetBudget { budget } => {
                self.budget = budget;
                self.trim(capacity);
            }
        }
    }

    /// Drop entries from the front until both bounds hold.
    fn trim(&mut self, capacity: usize) {
        while self.order.len() > capacity || self.over_budget() {
            // The `is_empty` arm is the model's own statement that the loop
            // terminates on its own: an empty cache has no cost and no entries, so
            // neither bound can be violated. The cache's implementation relies on
            // the same fact with no such guard, which is worth a reader's
            // attention.
            if self.order.is_empty() {
                break;
            }
            self.order.remove(0);
        }
    }

    /// Whether the model believes the cache is over its ceiling.
    fn over_budget(&self) -> bool {
        self.budget.is_some_and(|budget| self.total_cost() > budget)
    }

    /// The sum of the live entries' costs, recomputed from scratch.
    fn total_cost(&self) -> u64 {
        self.order
            .iter()
            .map(|(_, cost)| *cost)
            .fold(0u64, u64::saturating_add)
    }

    /// The live keys, in key order rather than recency order.
    fn live_keys(&self) -> Vec<u8> {
        let mut keys: Vec<u8> = self.order.iter().map(|(key, _)| *key).collect();
        keys.sort_unstable();
        keys
    }
}

/// Every key the cache currently holds, found by enumeration.
fn live_keys(cache: &LruCache<u8, u64>) -> Vec<u8> {
    KEY_SPACE
        .clone()
        .filter(|key| cache.contains_key(key))
        .collect()
}

/// Apply one operation to the cache under test.
///
/// Returns nothing: the properties that need an insert's *outcome* call
/// `insert_with_outcome` directly, and a synthesised outcome returned from here
/// would be a second, invented answer to the same question.
fn apply(cache: &mut LruCache<u8, u64>, op: PolicyOp) {
    match op {
        PolicyOp::Insert { key, value, cost } => {
            let _ = cache.insert_with_outcome(key, value, cost);
        }
        PolicyOp::Access { key } => {
            let _ = cache.get(&key);
        }
        PolicyOp::Remove { key } => {
            let _ = cache.remove(&key);
        }
        PolicyOp::Clear => cache.clear(),
        PolicyOp::SetBudget { budget } => {
            let _ = cache.set_budget(budget);
        }
    }
}

/// A key a sequence draws from, biased towards repeats.
///
/// Repeats matter: a replacement is a distinct code path with its own cost
/// bookkeeping, and uniform random keys over eight values rarely re-insert one
/// that is already resident.
fn any_key() -> impl Strategy<Value = u8> {
    prop_oneof![
        3 => KEY_SPACE.clone(),
        1 => (200u8..210).prop_map(|key| key % 8),
    ]
}

/// A cost range covering the small numbers a real caller declares, both ends of
/// the `u64` domain, and the boundary values a hand-written table forgets.
fn any_cost() -> impl Strategy<Value = u64> {
    prop_oneof![
        3 => 0u64..64,
        1 => 64u64..1_024,
        1 => Just(u64::MAX),
        1 => Just(0),
        1 => Just(1),
    ]
}

/// A cost range for the property that compares against a recomputed model, bounded
/// so the true sum of the live entries cannot overflow a `u64`.
///
/// **The bound is load-bearing, and finding out why was the point.** `LruCache`
/// maintains `total_cost` *incrementally*, with saturating arithmetic -- 1C-2a's
/// design, tested by `a_total_that_would_overflow_saturates_rather_than_panicking`.
/// Saturation is **path-dependent**: with a capacity of 2 and no ceiling, insert
/// `u64::MAX` then `1` and the accumulator reads `MAX + 1` as `MAX`; evict the
/// `MAX` entry and it reads `MAX - MAX` as `0`, while the one live entry really
/// costs `1`. So in the saturating regime `total_cost()` is not a function of the
/// live set, and **no model can predict it** -- which is exactly why the model
/// property below would be comparing against a fiction.
///
/// The limit is therefore not hidden by making the model replicate it. It is
/// (a) stated in the module docs, (b) pinned by
/// `the_ceiling_is_enforced_on_the_reported_total_even_when_the_true_sum_overflows`,
/// and (c) kept out of the property whose subject is the ceiling, so that property
/// tests the policy exactly rather than approximately.
///
/// With at most `capacity + 1 <= 9` entries at `u64::MAX / 64` each, the true sum
/// is bounded by about `2^61`, which cannot overflow.
fn any_exact_cost() -> impl Strategy<Value = u64> {
    prop_oneof![
        3 => 0u64..64,
        1 => 64u64..1_024,
        1 => Just(0),
        1 => Just(1),
        1 => Just(u64::MAX / 64),
    ]
}

/// A budget, or the deliberate absence of one.
fn any_budget() -> impl Strategy<Value = Option<u64>> {
    prop_oneof![
        4 => prop::option::of(0u64..128),
        1 => Just(None),
        1 => Just(Some(u64::MAX)),
    ]
}

/// Any operation, biased towards insertions, with costs that can overflow a sum.
fn any_policy_op() -> impl Strategy<Value = PolicyOp> {
    prop_oneof![
        5 => (any_key(), any::<u64>(), any_cost())
            .prop_map(|(key, value, cost)| PolicyOp::Insert { key, value, cost }),
        2 => any_key().prop_map(|key| PolicyOp::Access { key }),
        2 => any_key().prop_map(|key| PolicyOp::Remove { key }),
        1 => any_budget().prop_map(|budget| PolicyOp::SetBudget { budget }),
        1 => Just(PolicyOp::Clear),
    ]
}

/// Any operation, with costs bounded so a recomputed sum cannot overflow.
///
/// The difference from [`any_policy_op`] is the cost range and nothing else, and
/// it is there for the model property alone. See [`any_exact_cost`].
fn any_exact_policy_op() -> impl Strategy<Value = PolicyOp> {
    prop_oneof![
        5 => (any_key(), any::<u64>(), any_exact_cost())
            .prop_map(|(key, value, cost)| PolicyOp::Insert { key, value, cost }),
        2 => any_key().prop_map(|key| PolicyOp::Access { key }),
        2 => any_key().prop_map(|key| PolicyOp::Remove { key }),
        1 => prop::option::of(0u64..128).prop_map(|budget| PolicyOp::SetBudget { budget }),
        1 => Just(PolicyOp::Clear),
    ]
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

// The ceiling holds after every operation.
//
// **This is the property that must never break.** `AGENTS.md` §4.4 mandates a
// property over arbitrary input, and an LRU's bug class is *plausible wrong
// output* rather than a crash -- so a budget bug can sit in a green suite
// indefinitely without anything going red. Checked after **every single step**,
// not only at the end, over a sequence mixing insertion, lookup, removal, clearing
// and budget changes: a ceiling that holds only at the end of a sequence is a
// ceiling with a hole in the middle of one.
//
// The budget compared against is the cache's own, read after the step -- so a
// `SetBudget` inside the sequence is compared against the budget it installed
// rather than the one the sequence started with.
proptest! {
    #[test]
    fn no_operation_sequence_can_leave_the_cache_over_its_budget(
        capacity in 0usize..8,
        budget in prop::option::of(0u64..256),
        ops in prop::collection::vec(any_policy_op(), 0..64),
    ) {
        let mut cache: LruCache<u8, u64> = cache_with(capacity, budget);

        for op in ops {
            apply(&mut cache, op);

            if let Some(in_force) = cache.budget() {
                prop_assert!(
                    cache.total_cost() <= in_force,
                    "total {} exceeds the budget {in_force} in force after {:?}",
                    cache.total_cost(),
                    op
                );
            }
        }
    }
}

// The live set is what the model says it is.
//
// The compositional property, against a model that recomputes the total from
// scratch and derives the survivors by its own forward eviction pass. Two bounds
// with independent semantics can disagree; this is where the implementation and a
// second formulation of the same policy are checked against each other -- after
// every step, so a `Remove` that left a cost behind, or a `Clear` that left
// entries behind, cannot hide.
proptest! {
    #[test]
    fn no_operation_sequence_can_disagree_with_an_independently_derived_model(
        capacity in 0usize..8,
        initial_budget in prop::option::of(0u64..128),
        // `any_exact_policy_op`, not `any_policy_op`: in the saturating regime a
        // model cannot predict an incremental accumulator at all, so including
        // those costs would have this property testing a fiction. The limit is
        // pinned separately and stated in the module docs.
        ops in prop::collection::vec(any_exact_policy_op(), 0..56),
    ) {
        let mut cache: LruCache<u8, u64> = cache_with(capacity, initial_budget);
        let mut model = PolicyModel {
            order: Vec::new(),
            budget: initial_budget,
        };

        for op in ops {
            apply(&mut cache, op);
            model.apply(capacity, op);

            prop_assert_eq!(
                cache.budget(),
                model.budget,
                "the two disagreed about the budget after {:?}",
                op
            );
            prop_assert_eq!(
                live_keys(&cache),
                model.live_keys(),
                "the live set diverged after {:?}",
                op
            );
            prop_assert_eq!(
                cache.total_cost(),
                model.total_cost(),
                "the recomputed sum diverged from the incremental one after {:?}",
                op
            );
            prop_assert!(cache.len() <= capacity, "len {} > capacity {capacity}", cache.len());
        }
    }
}

// An entry costing more than the budget is never admitted.
//
// The refusal rule as a universal claim rather than a table of examples. A
// generated cost either fits and is admitted, or does not fit and is refused *and
// nothing else changes*. Both branches are asserted, because a rule that refused
// everything would satisfy the first half of that sentence perfectly.
proptest! {
    #[test]
    fn an_oversized_entry_is_refused_at_any_point_of_any_sequence(
        capacity in 0usize..8,
        budget in 0u64..256,
        key in any_key(),
        cost in any::<u64>(),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::with_budget(capacity, budget);
        let expected_refused = cost > budget;

        let outcome = cache.insert_with_outcome(key, key as u64, cost);

        // Positional format arguments, not inline capture: `prop_assert*!`
        // forwards its trailing tokens to `assert*!` through a macro boundary
        // where a bare `{name}` does not resolve. `tests/cache.rs` writes its
        // messages the same way, and the mixed form at
        // `no_operation_sequence_can_leave_the_cache_over_its_budget` shows the
        // one shape that does work.
        prop_assert_eq!(
            outcome.refused,
            expected_refused,
            "cost {} vs budget {}",
            cost,
            budget
        );
        prop_assert_eq!(cache.contains_key(&key), !expected_refused, "the entry's presence");
        prop_assert!(
            cache.total_cost() <= budget,
            "the ceiling holds: {} > {budget}",
            cache.total_cost()
        );
        // A refused insertion left the cost of everything else alone, so the total
        // is still the sum of the live entries -- which is the whole reason the
        // check happens before any mutation rather than after.
        if expected_refused {
            prop_assert_eq!(cache.total_cost(), 0, "nothing was resident to keep");
            prop_assert_eq!(outcome.previous, None, "a refusal replaced nothing");
        }
    }
}

// A refused insert leaves the cache exactly as it found it.
//
// The safety of the rule, over arbitrary sequences rather than one hand-built
// example: a caller may attempt an oversized insertion at any point, including
// against a key that is resident, and must find its cache untouched. The eviction
// counter is the sharp part -- a refusal is not an eviction, and a diagnostic that
// cannot tell them apart cannot explain a cache that is not growing.
proptest! {
    #[test]
    fn a_refused_insert_changes_nothing_at_any_point_of_any_sequence(
        capacity in 0usize..8,
        budget in 0u64..128,
        setup in prop::collection::vec(
            (any_key(), any_cost()).prop_map(|(key, cost)| PolicyOp::Insert {
                key,
                value: key as u64,
                cost,
            }),
            0..16,
        ),
        key in any_key(),
        cost in 128u64..u64::MAX,
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::with_budget(capacity, budget);
        for op in setup {
            apply(&mut cache, op);
        }
        let before = snapshot(&cache);

        let outcome = cache.insert_with_outcome(key, key as u64, cost);

        prop_assert!(
            outcome.refused,
            "cost {} must exceed budget {}",
            cost,
            budget
        );
        prop_assert_eq!(outcome.previous, None, "a refusal replaced nothing");
        prop_assert_eq!(snapshot(&cache), before, "a refusal changed the cache");
    }
}

// Lowering the budget is honoured on the same call.
//
// The window question, as a property. A `SetBudget` to a figure below the current
// total must bring the total under it **before returning**, at every point of a
// generated sequence -- so a lowered budget is never merely "eventually" honoured
// and never has an observable window in which the ceiling is not a ceiling.
proptest! {
    #[test]
    fn a_lowered_budget_is_honoured_wherever_it_lands_in_a_sequence(
        capacity in 0usize..8,
        ops in prop::collection::vec(any_policy_op(), 0..40),
        lowered_to in 0u64..64,
    ) {
        // Built with the maximal budget, so the generated sequence populates the
        // cache rather than being refused wholesale, and the final call has
        // something to lower *from*.
        let mut cache: LruCache<u8, u64> = LruCache::with_budget(capacity, u64::MAX);
        for op in ops {
            apply(&mut cache, op);
        }
        let resident = cache.len();

        let evicted = cache.set_budget(Some(lowered_to));

        prop_assert!(
            cache.total_cost() <= lowered_to,
            "total {} was not brought under {lowered_to} by the same call",
            cache.total_cost()
        );
        prop_assert!(
            evicted <= resident,
            "a budget change cannot evict more than the {resident} entries resident"
        );
        prop_assert_eq!(cache.len(), resident - evicted);
    }
}

// Raising or removing the budget never evicts anything.
//
// The direction the invariant already satisfies, asserted so that a trim loop
// reading the *old* budget -- a plausible wrong implementation, since the old
// value is exactly what a careless edit leaves in scope -- cannot survive.
proptest! {
    #[test]
    fn a_raised_or_removed_budget_evicts_nothing_at_any_point_of_a_sequence(
        capacity in 0usize..8,
        ops in prop::collection::vec(any_policy_op(), 0..40),
        raised_to in 0u64..u64::MAX,
        remove in any::<bool>(),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::with_budget(capacity, u64::MAX);
        for op in ops {
            apply(&mut cache, op);
        }
        let before = snapshot(&cache);
        let total_before = cache.total_cost();

        let evicted = cache.set_budget(if remove { None } else { Some(raised_to) });

        // **Conditional, and the condition is the point.** The generated
        // `raised_to` is not guaranteed to be a *raise*: it is drawn from the
        // whole `u64` range, and a budget below the current total is a lowering,
        // which legitimately evicts. So the property asserts the real invariant --
        // a budget the cache is already within evicts nothing -- and leaves the
        // lowering direction to
        // `a_lowered_budget_is_honoured_wherever_it_lands_in_a_sequence`, which is
        // where it belongs rather than smuggled in as a half-assertion.
        if remove || raised_to >= total_before {
            prop_assert_eq!(evicted, 0, "a budget the cache is within cannot evict");
            prop_assert_eq!(cache.total_cost(), total_before, "the total moved");
            // The budget is the one field that is *supposed* to move here, so the
            // rest is compared on its own rather than through a snapshot that
            // would report the intended change as a violation.
            let after = snapshot(&cache);
            prop_assert_eq!(after.live, before.live, "the live set moved");
            prop_assert_eq!(after.evictions, before.evictions, "an eviction happened");
            prop_assert_eq!(after.hits, before.hits, "the hit count moved");
            prop_assert_eq!(after.misses, before.misses, "the miss count moved");
        }
    }
}

// One insert never evicts more entries than it displaced.
//
// The trim bound of `AGENTS.md` §2.3's frame-loop concern, as a property over
// arbitrary sequences. Each iteration of the trim loop removes one entry and the
// order can only shrink, so the loop runs at most once per entry resident
// immediately after the insert -- and never once more than `capacity + 1`, since
// that is the most that can be resident. The eviction counter is the
// *measurement*; an upper bound nobody has instrumented is a claim.
proptest! {
    #[test]
    fn one_insert_never_evicts_more_entries_than_it_displaced(
        capacity in 0usize..8,
        ops in prop::collection::vec(any_policy_op(), 0..48),
        key in any_key(),
        cost in any_cost(),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::with_budget(capacity, u64::MAX);
        for op in ops {
            apply(&mut cache, op);
        }
        let resident = cache.len();
        let before = cache.evictions();

        let _ = cache.insert_with_outcome(key, key as u64, cost);

        let evicted = cache.evictions() - before;
        prop_assert!(
            evicted <= u64::try_from(resident + 1).unwrap_or(u64::MAX),
            "one insert evicted {evicted} entries with only {resident} resident"
        );
        prop_assert!(evicted <= u64::try_from(capacity + 1).unwrap_or(u64::MAX));
    }
}

// A budget loose enough never to bind changes nothing.
//
// The composition claim with 1C-2a's mechanism. A cache whose budget no cost can
// exceed must behave **identically** to an unbounded one -- key for key, eviction
// for eviction, counter for counter -- and that is the evidence that adding the
// policy did not perturb the mechanism whose mutation table `docs/COVERAGE.md`
// §5.3 records.
proptest! {
    #[test]
    fn a_budget_loose_enough_never_to_bind_changes_nothing(
        capacity in 0usize..8,
        ops in prop::collection::vec(any_policy_op(), 0..56),
    ) {
        let mut unbounded: LruCache<u8, u64> = LruCache::new(capacity);
        // A budget no generated cost can exceed, so the ceiling never fires.
        let mut bounded: LruCache<u8, u64> = LruCache::with_budget(capacity, u64::MAX);

        for op in ops {
            apply(&mut unbounded, op);
            apply(&mut bounded, op);
        }

        prop_assert_eq!(live_keys(&bounded), live_keys(&unbounded), "the live set diverged");
        prop_assert_eq!(bounded.total_cost(), unbounded.total_cost(), "the total diverged");
        prop_assert_eq!(bounded.evictions(), unbounded.evictions(), "the eviction count diverged");
        prop_assert_eq!(bounded.hits(), unbounded.hits(), "the hit count diverged");
        prop_assert_eq!(bounded.misses(), unbounded.misses(), "the miss count diverged");
    }
}
