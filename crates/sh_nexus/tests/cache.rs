//! `core/cache.rs`: an LRU cache bounded by entry count, reporting its total cost.
//!
//! Work unit 1C-2a. `AGENTS.md` 4.2 names five behaviours for this module --
//! *"cache insertion, LRU eviction, memory ceiling, hit/miss ratio, thread
//! safety"* -- and this file covers **three of them**: insertion, LRU eviction,
//! and the hit/miss ratio. The **memory ceiling** is work unit 1C-2b, and so is
//! the thread-safety decision, for the reasons `core/cache.rs`'s module docs set
//! out. This suite is deliberately scoped to the half that is here, and it is a
//! complete suite for it: nothing below is a placeholder for 1C-2b.
//!
//! # Why the tests are in this file and not in `core/`
//!
//! `crates/sh_nexus/tests/layer_boundary.rs`'s
//! `core_contains_no_panicking_construct` scans every file under `core/` for
//! `unwrap(`, `expect(`, `panic!` and `unsafe`. It strips comments first but
//! does **not** skip `#[cfg(test)]` modules, and its own "Known limit" note
//! says so: a future unit that puts its tests inside `core/` "will fail this
//! test on its own test helpers. That is the right failure to get wrong."
//! Every test in this project so far lives in `tests/`, and this unit keeps
//! that. `AGENTS.md` 4.3 allows `expect` in tests; the scanner has no way to
//! know that.
//!
//! # Three things asserted here that a coverage number cannot substitute for
//!
//! 1. **The order is never observed, only its consequences.** Nothing in this
//!    file reads the cache's recency order, because the cache does not expose
//!    it. Every claim about which entry survives is made by asking
//!    `contains_key`, which is a non-mutating probe. A cache that exposed its
//!    order would let these tests pass while the order itself was wrong, because
//!    the tests would be checking the implementation against itself.
//! 2. **The eviction order is computed independently.** The properties below
//!    keep their own `BTreeMap<u8, u64>` of key to last-use tick and derive the
//!    expected victim from it. It shares no code and no data structure with the
//!    module under test, so agreement between them is information.
//! 3. **Two properties assert what the compiler can.**
//!    `a_value_that_cannot_be_cloned_still_round_trips_through_the_cache` is a
//!    compile-time proof that no value is ever copied, because the value type
//!    has no `Clone` impl at all, and
//!    `inserting_clones_the_key_exactly_once_and_looking_one_up_never_does`
//!    uses a key that counts its own clones, so the "one clone at insert, none
//!    at access" claim in the module docs is measured rather than asserted.
//!
//! # Parameterized tests
//!
//! `#[rstest]` with `#[case]` throughout, per `AGENTS.md` 4.3 and ADR-008. A
//! `#[case]` failure names the case that broke; a `for` table over the same
//! cases would print an index and leave the reader counting back by hand.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use proptest::prelude::*;
use rstest::rstest;
use sh_nexus::core::cache::LruCache;

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// A key that counts how many times it has been cloned.
///
/// `K: Clone` is a real bound on the cache, because the key is held twice -- in
/// the lookup map and in the recency order. This type turns "how many times is
/// it really cloned" from a claim in a doc comment into a number.
#[derive(Debug)]
struct CountedKey {
    id: u8,
    clones: Rc<Cell<u32>>,
}

impl CountedKey {
    /// A fresh key, not yet cloned.
    fn new(id: u8) -> Self {
        Self {
            id,
            clones: Rc::new(Cell::new(0)),
        }
    }

    /// How many times this key has been cloned.
    fn clone_count(&self) -> u32 {
        self.clones.get()
    }
}

impl Clone for CountedKey {
    fn clone(&self) -> Self {
        self.clones.set(self.clones.get() + 1);
        Self {
            id: self.id,
            clones: Rc::clone(&self.clones),
        }
    }
}

impl PartialEq for CountedKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for CountedKey {}

impl Hash for CountedKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// A value with no `Clone` impl at all.
///
/// If `LruCache` required `V: Clone` this type would not compile against it,
/// which is a stronger statement than any test that counts copies at runtime.
#[derive(Debug, PartialEq, Eq)]
struct NotClone(&'static str);

/// Asserts `T: Send + Sync` at compile time, with no runtime cost.
///
/// This is the half of `AGENTS.md` 4.2's "thread safety" row that 1C-2a
/// satisfies, and it is a *type* property rather than a behavioural one: the
/// cache has no interior mutability and no lock, so it is `Send + Sync`
/// whenever its parameters are, and every method that changes it takes
/// `&mut self`. See `core/cache.rs`'s module docs for the half 1C-2b owns.
fn assert_send_and_sync<T: Send + Sync>() {}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// A new cache is empty, holds nothing, and has served no requests.
///
/// Every part of that is a claim: an empty cache whose `hit_ratio` were `Some`
/// would be reporting a 0% hit rate for a cache nobody has used, which is a
/// different fact.
#[test]
fn a_new_cache_is_empty_and_has_served_no_requests() {
    let cache: LruCache<String, String> = LruCache::new(8);

    assert_eq!(cache.capacity(), 8);
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
    assert_eq!(cache.total_cost(), 0);
    assert_eq!(cache.hits(), 0);
    assert_eq!(cache.misses(), 0);
    assert_eq!(cache.hit_ratio(), None, "no request means no ratio, not 0%");
}

/// The cache is `Send + Sync` whenever its parameters are.
///
/// `AGENTS.md` 4.2's "thread safety" row, the half 1C-2a owns. Asserted by
/// calling a function whose bound is the assertion, so this test cannot compile
/// if the cache ever grows a `Mutex`, a `RefCell`, or any other interior
/// mutability.
#[test]
fn the_cache_is_send_and_sync() {
    assert_send_and_sync::<LruCache<String, String>>();
    assert_send_and_sync::<LruCache<u64, Vec<u8>>>();
    assert_send_and_sync::<LruCache<String, NotClone>>();
}

/// `is_empty` and `len` cannot disagree, at any capacity.
///
/// `clippy::len_without_is_empty` requires both to exist; this requires them to
/// tell the same story, which a cache that kept a separate counter could not.
#[rstest]
#[case(0usize)]
#[case(1)]
#[case(2)]
#[case(64)]
fn len_and_is_empty_always_agree(#[case] capacity: usize) {
    let mut cache: LruCache<u8, u8> = LruCache::new(capacity);

    for key in 0..8u8 {
        cache.insert(key, key, 1);
        // A capacity of zero keeps nothing, so it is empty right after an
        // insertion; every other capacity is holding the entry it was just given.
        assert_eq!(cache.is_empty(), capacity == 0);
        assert!(cache.len() <= capacity);
        // And the two accessors tell the same story, stated through `is_empty`
        // rather than a length comparison so that `clippy`'s `len_zero` does not
        // fire on a test whose subject is `is_empty` itself. For a non-zero
        // capacity the entry just inserted must be findable; for zero, nothing
        // is.
        if capacity > 0 {
            assert!(cache.contains_key(&key), "the entry just inserted is gone");
        }
    }
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

/// An inserted entry is stored under its key and costs what the caller said.
///
/// The cost is the caller's number, not the cache's: the cache has no way to
/// know what a rendered message segment weighs, and the memory ceiling in 1C-2b
/// is only as good as the costs the caller declares. The zero case is included
/// because a cost of zero is legal, and a cache that treated it as
/// "uncountable" would be a bug in the ceiling rather than here.
#[rstest]
#[case(0u64)]
#[case(1)]
#[case(4_096)]
#[case(65_536)]
#[case(u64::MAX)]
fn an_inserted_entry_is_stored_and_charged_exactly_its_declared_cost(#[case] cost: u64) {
    let mut cache: LruCache<&str, &str> = LruCache::new(4);

    assert_eq!(cache.insert("avatar:u_1", "png-bytes", cost), None);
    assert!(cache.contains_key(&"avatar:u_1"));
    assert_eq!(cache.get(&"avatar:u_1"), Some(&"png-bytes"));
    assert_eq!(cache.total_cost(), cost);
}

/// A value that cannot be cloned still round-trips through the cache.
///
/// `NotClone` has no `Clone` impl, so this is a compile-time statement that
/// `LruCache` never copies a value -- `get` borrows, `remove` moves out, and
/// `insert` takes ownership. A cache that cloned on insert or on access would
/// have needed a `V: Clone` bound and this file would not compile.
#[test]
fn a_value_that_cannot_be_cloned_still_round_trips_through_the_cache() {
    let mut cache: LruCache<u8, NotClone> = LruCache::new(2);

    cache.insert(1, NotClone("first"), 10);
    cache.insert(2, NotClone("second"), 20);

    assert_eq!(cache.get(&1), Some(&NotClone("first")));
    assert_eq!(cache.get(&2), Some(&NotClone("second")));
    assert_eq!(cache.total_cost(), 30);

    // And out again, by move rather than by copy.
    assert_eq!(cache.remove(&1), Some(NotClone("first")));
    assert_eq!(cache.remove(&1), None);
    assert_eq!(cache.total_cost(), 20);
}

/// Inserting the same key twice replaces the value and does not grow the cache.
///
/// The no-duplicate half of the update contract. An LRU that appended the key a
/// second time would evict correctly but hold two entries for one key, and
/// `len()` would disagree with the number of distinct keys -- the first symptom
/// being a total cost that counts one entry twice.
#[test]
fn inserting_a_key_that_already_exists_replaces_it_without_growing_the_cache() {
    let mut cache: LruCache<&str, &str> = LruCache::new(4);

    cache.insert("k", "first", 10);
    cache.insert("k", "second", 10);

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get(&"k"), Some(&"second"));
    assert_eq!(cache.total_cost(), 10, "one entry, charged once");
}

/// Re-inserting a key returns the value it replaced.
///
/// One hash probe for "replace this", rather than a `get` -- which would move
/// the key's recency as a side effect -- followed by a `remove`.
#[test]
fn reinserting_a_key_hands_back_the_value_it_replaced() {
    let mut cache: LruCache<&str, &str> = LruCache::new(4);

    cache.insert("k", "first", 10);
    assert_eq!(cache.insert("k", "second", 10), Some("first"));
    assert_eq!(cache.len(), 1);
}

/// Re-inserting a key charges the difference between the old and new cost.
///
/// This is where a naive `total += new_cost` accumulator goes wrong: it would
/// charge the entry twice, and the reported total would climb on every update of
/// the same key. The reported figure is the sum of the costs of the **live**
/// entries, which is the only definition a ceiling can be built on.
#[rstest]
#[case(10u64, 25u64, 25u64)]
#[case(25, 10, 10)]
#[case(25, 0, 0)]
#[case(0, 25, 25)]
#[case(u64::MAX, 0, 0)]
#[case(0, u64::MAX, u64::MAX)]
fn reinserting_a_key_charges_the_difference_between_the_old_and_new_cost(
    #[case] first_cost: u64,
    #[case] second_cost: u64,
    #[case] expected_total: u64,
) {
    let mut cache: LruCache<&str, &str> = LruCache::new(4);

    cache.insert("k", "first", first_cost);
    cache.insert("k", "second", second_cost);

    assert_eq!(cache.total_cost(), expected_total);
    assert_eq!(cache.len(), 1);
}

/// A total that would overflow saturates instead of panicking.
///
/// `AGENTS.md` 2.1 forbids a panic in a production path, and a debug build
/// overflows on `+` while a release build wraps -- so a cache that summed
/// naively would be correct in one profile and wrong in the other. Saturating
/// means the reported total is a **lower bound** on the true cost, which is the
/// safe direction for a ceiling: it over-reports pressure rather than
/// under-reporting it.
#[rstest]
#[case(1u8)]
#[case(2)]
#[case(4)]
fn a_total_that_would_overflow_saturates_rather_than_panicking(#[case] entries: u8) {
    let mut cache: LruCache<u8, u8> = LruCache::new(8);

    for key in 0..entries {
        cache.insert(key, key, u64::MAX);
    }

    assert_eq!(
        cache.total_cost(),
        u64::MAX,
        "saturating, and never a debug-build overflow panic"
    );
}

/// The key is cloned once per insert and never on a lookup.
///
/// The module docs claim the key is held twice -- once in the lookup map, once
/// in the recency order -- and that an access **moves** the key rather than
/// copying it. Both halves are measured here rather than asserted, because an
/// implementation that cloned the key on every `get` would still be correct and
/// would be quietly paying an allocation per message render.
#[test]
fn inserting_clones_the_key_exactly_once_and_looking_one_up_never_does() {
    let mut cache: LruCache<CountedKey, &str> = LruCache::new(2);

    let key = CountedKey::new(7);
    // The caller keeps a handle so the key can be used again after being moved
    // into the cache. That handle is the caller's own business and is the first
    // clone; everything after it belongs to the cache.
    let handle = key.clone();
    assert_eq!(
        handle.clone_count(),
        1,
        "one clone: the caller's own handle"
    );

    cache.insert(key, "value", 10);
    assert_eq!(
        handle.clone_count(),
        2,
        "exactly one more: the key is held twice, because the cache stores it in \
         the lookup map and in the recency order"
    );

    let after_insert = handle.clone_count();
    for _ in 0..5 {
        assert_eq!(cache.get(&handle), Some(&"value"));
    }
    assert_eq!(
        handle.clone_count(),
        after_insert,
        "five lookups must not clone the key even once"
    );

    // An eviction removes the key from both collections, still without cloning.
    cache.insert(CountedKey::new(8), "other", 10);
    cache.insert(CountedKey::new(9), "third", 10);
    assert!(
        !cache.contains_key(&handle),
        "key 7 was the least recently used"
    );
    assert_eq!(
        handle.clone_count(),
        after_insert,
        "eviction does not clone either"
    );
}

// ---------------------------------------------------------------------------
// Eviction by recency
// ---------------------------------------------------------------------------

/// The entry that goes is the one that was used least recently.
///
/// `AGENTS.md` 4.2's "LRU eviction" clause. Exactly `capacity` keys are inserted
/// first, so the cache is full and the overflow insertion evicts exactly one --
/// which is what makes "all of them went" a claim about recency rather than a
/// claim about arithmetic. The capacities are the interesting shapes: 1 leaves
/// no choice at all, 2 is the smallest cache where a wrong answer is possible,
/// and 8 is where "least recently used" and "oldest inserted" first come apart.
#[rstest]
#[case(1usize, 1u8)]
#[case(2, 2)]
#[case(4, 4)]
#[case(8, 8)]
fn the_entry_that_goes_is_the_one_that_was_used_least_recently(
    #[case] capacity: usize,
    #[case] insert_count: u8,
) {
    let mut cache: LruCache<u8, u8> = LruCache::new(capacity);

    for key in 0..insert_count {
        cache.insert(key, key, 1);
    }
    assert_eq!(
        cache.len(),
        capacity,
        "the cache is full before the overflow"
    );
    let overflow = insert_count;

    cache.insert(overflow, overflow, 1);

    assert_eq!(cache.len(), capacity);
    // Exactly one entry goes, and it is the least recently used: key 0, which
    // was inserted first and not touched since.
    assert!(
        !cache.contains_key(&0),
        "the least recently used key should have been evicted"
    );
    for key in 1..insert_count {
        assert!(
            cache.contains_key(&key),
            "only the least recently used entry should have gone, but key {key} did"
        );
    }
    assert!(cache.contains_key(&overflow));
    assert_eq!(
        cache.total_cost(),
        capacity as u64,
        "every survivor is charged exactly once"
    );
}

/// A cache that is not full evicts nothing.
///
/// The other half of the eviction contract, and the one a cache written as "evict
/// before inserting" would get wrong: filling to less than capacity and then
/// overflowing must not drop anything, because nothing has to be dropped.
#[rstest]
#[case(4usize, 1u8)]
#[case(4, 2)]
#[case(4, 3)]
#[case(8, 1)]
fn a_cache_that_is_not_full_evicts_nothing(#[case] capacity: usize, #[case] insert_count: u8) {
    let mut cache: LruCache<u8, u8> = LruCache::new(capacity);

    for key in 0..insert_count {
        cache.insert(key, key, 1);
    }
    cache.insert(insert_count, insert_count, 1);

    assert_eq!(cache.len(), usize::from(insert_count) + 1);
    for key in 0..=insert_count {
        assert!(
            cache.contains_key(&key),
            "key {key} was evicted from a cache with room"
        );
    }
    assert_eq!(cache.total_cost(), u64::from(insert_count) + 1);
}

/// An access makes an entry survive one more eviction.
///
/// This is the property the whole structure exists for, asserted **directly**
/// rather than as a corollary of "eviction follows recency". Two caches are
/// built from the identical sequence; one reads the least recently used key
/// immediately before the eviction and the other does not. The only difference in
/// the outcome is that the read key is still there and the entry *behind* it is
/// the one that went.
///
/// Filling the cache first is load-bearing rather than incidental: an eviction
/// that happens mid-sequence is what makes "one more eviction" a countable
/// thing.
#[rstest]
#[case(2usize)]
#[case(3)]
#[case(8)]
fn an_access_makes_an_entry_survive_one_more_eviction(#[case] capacity: usize) {
    let build = |protect: bool| {
        let mut cache: LruCache<u8, u8> = LruCache::new(capacity);
        for key in 0..capacity as u8 {
            cache.insert(100 + key, 100 + key, 1);
        }
        assert_eq!(cache.len(), capacity, "the cache is full before the read");
        if protect {
            // 100 is the least recently used, and reading it must hit.
            assert_eq!(cache.get(&100), Some(&100));
        }
        cache
    };

    let untouched = build(false);
    let protected = build(true);

    // Push both one entry past capacity.
    let mut untouched = untouched;
    let mut protected = protected;
    untouched.insert(200, 200, 1);
    protected.insert(200, 200, 1);

    assert!(
        protected.contains_key(&100),
        "an accessed entry must outlive an identical, never-accessed one"
    );
    assert!(
        !untouched.contains_key(&100),
        "the control: the same entry without an access does not"
    );
    assert!(
        !protected.contains_key(&101),
        "the accessed entry displaced the next-least-recently-used one instead"
    );
    assert!(
        untouched.contains_key(&101),
        "and the control evicted 100 instead"
    );
    assert_eq!(protected.len(), capacity);
    assert_eq!(untouched.len(), capacity);
    assert_eq!((protected.hits(), protected.misses()), (1, 0));
    assert_eq!((untouched.hits(), untouched.misses()), (0, 0));
}

/// Re-inserting a key protects it, on the same grounds as reading it.
///
/// The update half of the recency contract, and the reason it is stated: a
/// caller that has just produced a value has just used it.
#[test]
fn reinserting_an_entry_resets_its_recency() {
    let mut cache: LruCache<u8, u8> = LruCache::new(3);

    cache.insert(1, 1, 1);
    cache.insert(2, 2, 1);
    cache.insert(3, 3, 1);
    // 1 is the least recently used. Replacing it makes it the most recent.
    cache.insert(1, 11, 1);
    cache.insert(4, 4, 1);

    assert!(cache.contains_key(&1), "the replaced key was protected");
    assert!(
        !cache.contains_key(&2),
        "and 2 became the least recently used"
    );
    assert!(cache.contains_key(&3));
    assert!(cache.contains_key(&4));
    assert_eq!(cache.len(), 3);
    assert_eq!(cache.get(&1), Some(&11), "with the value it was given");
}

/// Reading an entry in a full cache protects it and evicts the one behind it.
///
/// The everyday shape for a chat client: a message scrolled back into view is
/// read, and something nobody has looked at for a while goes instead.
#[test]
fn reading_an_entry_in_a_full_cache_evicts_the_entry_behind_it() {
    let mut cache: LruCache<&str, &str> = LruCache::new(3);

    cache.insert("a", "a", 1);
    cache.insert("b", "b", 1);
    cache.insert("c", "c", 1);

    assert_eq!(cache.get(&"a"), Some(&"a"));
    cache.insert("d", "d", 1);

    assert!(cache.contains_key(&"a"), "the entry just read is protected");
    assert!(!cache.contains_key(&"b"), "the one behind it is not");
    assert!(cache.contains_key(&"c"));
    assert!(cache.contains_key(&"d"));
    assert_eq!(cache.len(), 3);
}

// ---------------------------------------------------------------------------
// capacity: 0 and capacity: 1
// ---------------------------------------------------------------------------

/// A cache of capacity zero is legal and is a cache that keeps nothing.
///
/// This is the edge case where caches are usually wrong, because "capacity" is
/// usually read as "how many". Here it is a hard ceiling that an insertion may
/// not exceed, so an insertion into a zero-capacity cache evicts the entry it
/// has just inserted. That is a well-defined **disabled cache** rather than a
/// panic, and rather than a silent no-op that would leave the caller believing
/// something was cached.
#[test]
fn a_cache_of_capacity_zero_keeps_nothing_and_costs_nothing() {
    let mut cache: LruCache<u8, u8> = LruCache::new(0);

    assert_eq!(cache.capacity(), 0);
    assert!(cache.is_empty());

    assert_eq!(
        cache.insert(1, 10, 100),
        None,
        "there was nothing to replace"
    );
    assert!(
        cache.is_empty(),
        "the entry it was given is immediately gone"
    );
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.total_cost(), 0, "and so is its cost");
}

/// Every operation on a zero-capacity cache behaves, and none of them panics.
///
/// Parameterized because the interesting part is the *combination*: a `get` that
/// misses, a `remove` that finds nothing, and a `clear` of nothing are all
/// ordinary calls that return ordinary answers.
#[rstest]
#[case(1u8)]
#[case(2)]
#[case(255)]
fn every_operation_on_a_zero_capacity_cache_behaves(#[case] key: u8) {
    let mut cache: LruCache<u8, u8> = LruCache::new(0);

    cache.insert(key, key, 5);
    assert_eq!(cache.get(&key), None, "a disabled cache never hits");
    assert!(!cache.contains_key(&key));
    assert_eq!(cache.remove(&key), None);
    cache.clear();

    assert!(cache.is_empty());
    assert_eq!(cache.total_cost(), 0);
    assert_eq!(cache.misses(), 1, "the one lookup was counted as a miss");
    assert_eq!(cache.hits(), 0);
}

/// A cache of capacity one holds exactly the most recent entry.
///
/// The smallest cache that can still be right, and the one where a
/// "least-recently-used" bug is invisible unless the wrong entry is the one that
/// stays.
#[rstest]
#[case(1u8, 2u8)]
#[case(0, 1)]
#[case(7, 8)]
fn a_cache_of_capacity_one_holds_exactly_the_most_recent_entry(
    #[case] first: u8,
    #[case] second: u8,
) {
    let mut cache: LruCache<u8, u8> = LruCache::new(1);

    cache.insert(first, first, 10);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key(&first));

    cache.insert(second, second, 20);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key(&second), "only the newest may remain");
    assert!(!cache.contains_key(&first));
    assert_eq!(cache.total_cost(), 20, "and only the newest is charged");
}

/// A cache of capacity one still protects the entry it holds, because there is
/// nothing else to evict.
///
/// The degenerate case of the property above, worth pinning because a capacity
/// check written as "evict before inserting" rather than "evict after inserting,
/// down to capacity" would evict the wrong thing here.
#[test]
fn a_cache_of_capacity_one_protects_the_entry_it_already_holds() {
    let mut cache: LruCache<u8, u8> = LruCache::new(1);

    cache.insert(1, 1, 1);
    cache.insert(1, 11, 1);

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get(&1), Some(&11));
    assert_eq!(cache.total_cost(), 1);
}

// ---------------------------------------------------------------------------
// Hit and miss accounting
// ---------------------------------------------------------------------------

/// `get` reports a hit or a miss, and a miss is a normal outcome.
///
/// `Option` rather than `Result`, because a chat client's segment cache misses
/// on every message of a channel it has just opened, and turning that into an
/// error would mean every render path carries an error arm for something that
/// is supposed to happen.
#[test]
fn a_get_reports_a_hit_or_a_miss_and_counts_exactly_one_of_them() {
    let mut cache: LruCache<&str, &str> = LruCache::new(2);

    assert_eq!(cache.get(&"absent"), None);
    assert_eq!((cache.hits(), cache.misses()), (0, 1));

    cache.insert("k", "v", 1);
    assert_eq!(cache.get(&"k"), Some(&"v"));
    assert_eq!((cache.hits(), cache.misses()), (1, 1));
}

/// A miss never changes the recency order.
///
/// `AGENTS.md` 2.3 forbids blocking the frame loop, and a cold channel produces
/// a run of misses where every one of them counts. A miss that touched the
/// recency order would make the order depend on the *miss pattern* -- on how far
/// the user scrolled -- rather than on what was actually used, and scrolling is
/// not evidence that anything is wanted.
#[test]
fn a_miss_never_changes_the_recency_order() {
    let mut cache: LruCache<u8, u8> = LruCache::new(3);

    cache.insert(1, 1, 1);
    cache.insert(2, 2, 1);
    cache.insert(3, 3, 1);

    // 1 is the least recently used. Ask for a run of keys that are not there.
    for absent in 100..120u8 {
        assert_eq!(cache.get(&absent), None);
    }

    cache.insert(4, 4, 1);

    assert!(
        !cache.contains_key(&1),
        "the misses above must not have protected the least recently used entry"
    );
    assert!(cache.contains_key(&2));
    assert!(cache.contains_key(&3));
    assert!(cache.contains_key(&4));
    assert_eq!(cache.misses(), 20);
    assert_eq!(cache.hits(), 0, "a miss is not a hit");
}

/// Only `get` is counted; nothing else moves either counter.
///
/// The hit/miss ratio is a statement about **values served**, so anything that is
/// not a lookup of a value must leave it alone. A ratio that counted insertions
/// as hits would report a fresh cache as 100% effective, which is the kind of
/// metric that makes a cache look like it is working when it is not.
#[rstest]
#[case("insert")]
#[case("remove")]
#[case("contains")]
#[case("clear")]
fn only_a_lookup_is_counted_as_a_hit_or_a_miss(#[case] operation: &str) {
    let mut cache: LruCache<u8, u8> = LruCache::new(4);

    match operation {
        "insert" => {
            cache.insert(1, 1, 1);
        }
        "remove" => {
            cache.insert(1, 1, 1);
            let _ = cache.remove(&1);
        }
        "contains" => {
            cache.insert(1, 1, 1);
            assert!(cache.contains_key(&1));
            assert!(!cache.contains_key(&2));
        }
        _ => {
            cache.insert(1, 1, 1);
            cache.clear();
        }
    }

    assert_eq!(
        (cache.hits(), cache.misses()),
        (0, 0),
        "`{operation}` is not a lookup and must not move the ratio"
    );
}

/// The hit ratio is hits over lookups, and there is none until one is made.
///
/// `None` rather than `Some(0.0)` for "no requests": a cache that has served
/// nothing has no hit rate, and reporting 0% would be indistinguishable from a
/// cache that misses everything. Same reasoning as `SyncStatus::Unverifiable` in
/// `core/ordering.rs`, which is not a pass.
#[rstest]
#[case(0u64, 0u64, None)]
#[case(1, 0, Some(1.0))]
#[case(0, 1, Some(0.0))]
#[case(1, 1, Some(0.5))]
#[case(3, 1, Some(0.75))]
#[case(1, 3, Some(0.25))]
#[case(2, 2, Some(0.5))]
fn the_hit_ratio_is_hits_over_lookups(
    #[case] hits: u64,
    #[case] misses: u64,
    #[case] expected: Option<f64>,
) {
    let mut cache: LruCache<u64, u64> = LruCache::new(16);

    for key in 0..hits {
        cache.insert(key, key, 1);
    }
    for _ in 0..misses {
        assert_eq!(cache.get(&200), None);
    }
    for key in 0..hits {
        assert_eq!(cache.get(&key), Some(&key));
    }

    assert_eq!((cache.hits(), cache.misses()), (hits, misses));
    assert_eq!(cache.hit_ratio(), expected);
}

/// Looking up the same key repeatedly is one hit per lookup, not one per entry.
///
/// Stated because the counters are `u64` and a caller may well want the ratio
/// over a window rather than over the cache's whole life, and that is only
/// possible if every lookup is counted.
#[test]
fn a_repeated_lookup_is_counted_once_per_lookup() {
    let mut cache: LruCache<u8, u8> = LruCache::new(2);

    cache.insert(1, 1, 1);
    for _ in 0..7 {
        assert_eq!(cache.get(&1), Some(&1));
    }

    assert_eq!(cache.hits(), 7);
    assert_eq!(cache.hit_ratio(), Some(1.0));
}

/// Repeated access never changes what the cache costs.
///
/// The module docs' claim that a value's cost is read **once**, at insert, and
/// never recomputed, rests on the value being immutable while resident. This is
/// the observable consequence: a thousand reads move no counter and change no
/// total, so an expensive cost computation could never be charged twice.
#[test]
fn repeated_access_never_changes_the_reported_total_cost() {
    let mut cache: LruCache<u8, u64> = LruCache::new(4);

    cache.insert(1, 42, 4_096);
    let total = cache.total_cost();

    for _ in 0..1_000 {
        let _ = cache.get(&1);
        let _ = cache.get(&2);
    }

    assert_eq!(cache.total_cost(), total);
    assert_eq!(cache.total_cost(), 4_096);
    assert_eq!(cache.hits(), 1_000);
}

// ---------------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------------

/// Removing an entry hands back its value and frees its cost.
///
/// The accounting half matters more than the return value: a removal that
/// dropped the entry but not its cost would leave a total that only ever grows,
/// and the memory ceiling in 1C-2b is built directly on that number.
#[rstest]
#[case(1usize, 0u64)]
#[case(2, 10)]
#[case(3, 20)]
#[case(4, 30)]
fn removing_an_entry_returns_its_value_and_frees_its_cost(
    #[case] live_before: usize,
    #[case] expected_after: u64,
) {
    let mut cache: LruCache<u8, u8> = LruCache::new(8);

    for key in 1..=live_before as u8 {
        cache.insert(key, key, 10);
    }
    assert_eq!(cache.total_cost(), 10 * live_before as u64);

    assert_eq!(cache.remove(&1), Some(1));
    assert!(!cache.contains_key(&1));
    assert_eq!(cache.total_cost(), expected_after);
    assert_eq!(cache.len(), live_before - 1);
}

/// Removing a key that is not there changes nothing at all.
///
/// Including the counters: a removal asked for a key it does not hold is not a
/// miss, because nothing was looked up on the caller's behalf.
#[test]
fn removing_an_absent_key_returns_nothing_and_changes_nothing() {
    let mut cache: LruCache<u8, u8> = LruCache::new(4);

    cache.insert(1, 1, 10);
    let before = (
        cache.len(),
        cache.total_cost(),
        cache.hits(),
        cache.misses(),
    );

    assert_eq!(cache.remove(&99), None);
    assert_eq!(cache.remove(&99), None);

    assert_eq!(
        (
            cache.len(),
            cache.total_cost(),
            cache.hits(),
            cache.misses()
        ),
        before
    );
    assert!(cache.contains_key(&1));
}

/// A removed key is a miss afterwards, and re-inserting it starts fresh.
///
/// Removing an entry is how a caller invalidates one -- a channel switch, a
/// theme change, a segment the caller knows is stale -- and the entry must be
/// indistinguishable from one that was never there.
#[test]
fn a_removed_key_is_a_miss_and_can_be_inserted_again() {
    let mut cache: LruCache<&str, &str> = LruCache::new(2);

    cache.insert("k", "first", 10);
    assert_eq!(cache.remove(&"k"), Some("first"));
    assert_eq!(cache.get(&"k"), None);
    assert_eq!(cache.misses(), 1);

    assert_eq!(
        cache.insert("k", "second", 5),
        None,
        "nothing was there to replace"
    );
    assert_eq!(cache.get(&"k"), Some(&"second"));
    assert_eq!(cache.total_cost(), 5);
    assert_eq!(cache.len(), 1);
}

/// Removing every entry by hand is the same as clearing, in outcome.
///
/// Pinned so that `remove` cannot quietly leave an entry behind in the recency
/// order: a key that is out of the map but still in the order would be a
/// subsequent `contains_key` that says no and a subsequent insertion that evicts
/// a key nobody can see.
#[rstest]
#[case(1usize)]
#[case(2)]
#[case(5)]
fn removing_every_entry_by_hand_leaves_the_cache_empty(#[case] count: usize) {
    let mut cache: LruCache<u8, u8> = LruCache::new(count.max(1));

    for key in 0..count as u8 {
        cache.insert(key, key, 1);
    }
    for key in 0..count as u8 {
        assert_eq!(cache.remove(&key), Some(key));
    }

    assert!(cache.is_empty());
    assert_eq!(cache.total_cost(), 0);
    assert_eq!(cache.len(), 0);

    // And the order is genuinely clean: one more insert fills the cache alone.
    cache.insert(200, 200, 7);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.total_cost(), 7);
}

// ---------------------------------------------------------------------------
// Clearing
// ---------------------------------------------------------------------------

/// Clearing empties the cache and frees every cost.
///
/// A theme switch or a channel switch invalidates a whole set at once, and the
/// total has to come back to zero with it -- a total that survives a clear is a
/// number the ceiling in 1C-2b would then be measuring against fiction.
#[test]
fn clearing_empties_the_cache_and_frees_every_cost() {
    let mut cache: LruCache<u8, u8> = LruCache::new(4);

    for key in 0..4u8 {
        cache.insert(key, key, 1_000);
    }
    assert_eq!(cache.total_cost(), 4_000);

    cache.clear();

    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.total_cost(), 0);
    for key in 0..4u8 {
        assert!(!cache.contains_key(&key));
        assert_eq!(cache.get(&key), None);
    }
    assert_eq!(cache.misses(), 4, "the four verification lookups missed");
}

/// Clearing keeps the capacity and the counters.
///
/// The counters measure the cache's whole life, not an epoch of it: a caller
/// that wants a per-window ratio subtracts the counts it read at the start of
/// the window. Resetting them here would make the ratio jump to 100% after every
/// channel switch, which is a metric that flatters itself.
#[test]
fn clearing_keeps_the_capacity_and_the_counters() {
    let mut cache: LruCache<u8, u8> = LruCache::new(4);

    cache.insert(1, 1, 1);
    let _ = cache.get(&1);
    let _ = cache.get(&2);

    cache.clear();

    assert_eq!(cache.capacity(), 4);
    assert_eq!((cache.hits(), cache.misses()), (1, 1));
    assert_eq!(cache.hit_ratio(), Some(0.5));
}

/// Clearing an empty cache is not an error and does not fabricate a ratio.
///
/// The two obvious ways this could go wrong: a `clear` that reset the counters
/// (making `hit_ratio` `None` again and inventing a "no data" state that did not
/// exist) and one that panicked on nothing to clear.
#[test]
fn clearing_an_empty_cache_changes_only_what_it_should() {
    let mut cache: LruCache<u8, u8> = LruCache::new(2);

    cache.clear();
    cache.clear();

    assert!(cache.is_empty());
    assert_eq!(cache.capacity(), 2);
    assert_eq!(cache.total_cost(), 0);
    assert_eq!(cache.hit_ratio(), None);
}

/// The recency order after a clear starts from nothing.
///
/// An order that survived a clear would let an entry inserted before the clear
/// be protected by an insertion after it, which is not an eviction policy any
/// caller would expect.
#[test]
fn the_recency_order_after_a_clear_starts_from_nothing() {
    let mut cache: LruCache<u8, u8> = LruCache::new(2);

    cache.insert(1, 1, 1);
    cache.clear();

    cache.insert(3, 3, 1);
    cache.insert(4, 4, 1);
    // The order is now [3, 4] and nothing from before the clear is in it, so 3
    // is the victim. If the old order had survived, 1 would be the one consulted
    // -- and 1 is not in the cache at all, so the eviction would be a no-op.
    cache.insert(5, 5, 1);

    assert!(!cache.contains_key(&3), "3 was the least recently used");
    assert!(cache.contains_key(&4));
    assert!(cache.contains_key(&5));
    assert_eq!(cache.len(), 2);
}

// ---------------------------------------------------------------------------
// The independent model the properties are checked against
// ---------------------------------------------------------------------------

/// One operation in a generated access sequence.
///
/// `Probe` and `Access` both ask the cache about a key. They differ in exactly
/// one way, and it is the one the module docs make a contract: `Probe` uses the
/// non-mutating membership check and `Access` uses `get`, so any difference in
/// outcome between the two is a difference in the contract rather than noise.
#[derive(Debug, Clone, Copy)]
enum Op {
    /// Insert a value at a declared cost.
    Insert { key: u8, value: u64, cost: u64 },
    /// Read a value, moving the key to most recently used if it is there.
    Access { key: u8 },
    /// Ask whether a key is present, changing nothing.
    Probe { key: u8 },
}

/// The key space every property generates over.
///
/// Small and closed, so a property can ask the cache about **every** possible
/// key by enumeration. That is how the recency order is asserted without the
/// cache exposing it: the *set* of live keys is observable, the sequence is
/// not, so a wrong order cannot hide behind an accessor that reports it back.
const KEY_SPACE: std::ops::Range<u8> = 0..12;

/// An independent model of what an LRU cache should hold.
///
/// Deliberately built from the parts the implementation does **not** use: a
/// `BTreeMap` from key to a monotonically increasing *tick*, and a second
/// `BTreeMap` from key to cost. The expected victim is the live key with the
/// smallest tick, and because a tick is only ever handed to one key at a time,
/// there is exactly one minimum and no tie to break. Sharing no code and no data
/// structure with `LruCache` is the point: a model that asked the implementation
/// what order it was in would agree with it by construction.
#[derive(Debug, Default)]
struct Model {
    /// The tick counter. It is written and never read directly -- it exists so
    /// that "which key was used most recently" is a recorded fact rather than a
    /// guess -- but `Debug` is derived so a failing case prints it.
    tick: u64,
    /// Live keys, mapped to the tick at which they were last used.
    last_use: BTreeMap<u8, u64>,
    /// Live keys, mapped to the cost they were inserted with.
    costs: BTreeMap<u8, u64>,
}

impl Model {
    /// Apply one operation, returning the key it expects to be evicted.
    fn apply(&mut self, capacity: usize, op: Op) -> Option<u8> {
        match op {
            Op::Insert { key, cost, .. } => {
                self.tick += 1;
                // An insert is a use, so it ticks just like a read does.
                self.last_use.insert(key, self.tick);
                self.costs.insert(key, cost);
                if self.costs.len() > capacity {
                    let victim = self.least_recently_used()?;
                    self.last_use.remove(&victim);
                    self.costs.remove(&victim);
                    Some(victim)
                } else {
                    None
                }
            }
            Op::Access { key } => {
                if self.last_use.contains_key(&key) {
                    self.tick += 1;
                    self.last_use.insert(key, self.tick);
                }
                // A miss ticks nothing. That is the contract under test, stated
                // here as well as in the module so that the model and the
                // implementation cannot disagree about it by accident.
                None
            }
            Op::Probe { .. } => None,
        }
    }

    /// The live key the next eviction would remove.
    fn least_recently_used(&self) -> Option<u8> {
        self.last_use
            .iter()
            .min_by_key(|(_, tick)| **tick)
            .map(|(key, _)| *key)
    }

    /// The two least recently used live keys, oldest first.
    ///
    /// Needed by the property that a read protects an entry: the protected
    /// eviction must take the key *behind* the one that was read, and "behind"
    /// is only meaningful against a second entry.
    fn two_least_recently_used(&self) -> Option<(u8, u8)> {
        let mut by_tick: Vec<(u64, u8)> = self
            .last_use
            .iter()
            .map(|(key, tick)| (*tick, *key))
            .collect();
        by_tick.sort_unstable();
        match by_tick.as_slice() {
            [(first_tick, first), (second_tick, second)] if first_tick < second_tick => {
                Some((*first, *second))
            }
            _ => None,
        }
    }

    /// The sum of the costs of the live keys.
    fn total_cost(&self) -> u64 {
        self.costs.values().copied().fold(0u64, u64::saturating_add)
    }
}

/// Any operation, biased towards insertions and towards repeats.
///
/// The biases are the adversarial inputs, for the same reason
/// `tests/proptest_boundary.rs` puts them in the *generators* rather than
/// asserting against them: uniform random operations rarely re-insert a key that
/// already exists, and a replacement is a distinct code path with its own cost
/// bookkeeping.
fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => (KEY_SPACE.clone(), any::<u64>(), 0u64..1_000)
            .prop_map(|(key, value, cost)| Op::Insert { key, value, cost }),
        3 => KEY_SPACE.clone().prop_map(|key| Op::Access { key }),
        2 => KEY_SPACE.clone().prop_map(|key| Op::Probe { key }),
    ]
}

/// Apply one operation to the cache under test, mirroring [`Model::apply`].
fn apply(cache: &mut LruCache<u8, u64>, op: Op) {
    match op {
        Op::Insert { key, value, cost } => {
            let _ = cache.insert(key, value, cost);
        }
        Op::Access { key } => {
            let _ = cache.get(&key);
        }
        Op::Probe { key } => {
            let _ = cache.contains_key(&key);
        }
    }
}

/// Every key the cache currently holds, found by enumeration rather than by
/// asking the cache for its order.
fn live_keys(cache: &LruCache<u8, u64>) -> Vec<u8> {
    KEY_SPACE
        .clone()
        .filter(|key| cache.contains_key(key))
        .collect()
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

// The next eviction removes the least recently used entry.
//
// `AGENTS.md` 4.4 mandates a property, and this is the one that makes the
// structure an LRU rather than a bounded map. After **every single operation**
// the expected victim is derived from `Model` -- a map of key to last-use tick,
// sharing no code with the implementation -- and asserted to be gone. Checked
// per step rather than only at the end, so a bug that evicts the right key one
// operation later and the wrong key in between cannot hide.
//
// (`proptest!` does not attach an outer doc comment to the test it generates, so
// these explanations are ordinary comments. `tests/proptest_boundary.rs` does
// the same.)
proptest! {
    #[test]
    fn the_next_eviction_removes_the_least_recently_used_entry(
        capacity in 0usize..6,
        ops in prop::collection::vec(any_op(), 0..48),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::new(capacity);
        let mut model = Model::default();

        for op in ops {
            let expected_victim = model.apply(capacity, op);
            apply(&mut cache, op);

            if let Some(victim) = expected_victim {
                prop_assert!(
                    !cache.contains_key(&victim),
                    "key {} was the least recently used and should have been evicted",
                    victim
                );
            }
            prop_assert!(
                cache.len() <= capacity,
                "capacity {capacity} exceeded: len {}",
                cache.len()
            );
        }

        // And the whole live set matches the model's view of it, which is the
        // other half: an eviction that is too eager also satisfies "the least
        // recently used one went".
        prop_assert_eq!(
            live_keys(&cache),
            model.last_use.keys().copied().collect::<Vec<u8>>()
        );
    }
}

// An access protects an entry for exactly one more eviction.
//
// The property the whole structure exists for, asserted over arbitrary
// sequences rather than on one hand-built example, and asserted **exactly**:
// the read must save the least recently used entry, and the entry behind it
// must be the one that pays. The read is the last operation before the
// insertion, so the only difference between the two branches is the read.
proptest! {
    #[test]
    fn an_access_protects_an_entry_for_exactly_one_more_eviction(
        capacity in 2usize..6,
        ops in prop::collection::vec(any_op(), 0..32),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::new(capacity);
        let mut model = Model::default();
        for op in ops.iter().copied() {
            apply(&mut cache, op);
            let _ = model.apply(capacity, op);
        }

        // Needs two live entries for "one more eviction" to mean anything, and a
        // full cache for the insertion to evict at all.
        if cache.len() < capacity {
            return Ok(());
        }
        let Some((protected, displaced)) = model.two_least_recently_used() else {
            return Ok(());
        };

        // The control: the same sequence with no read. `protected` is the least
        // recently used, so it goes.
        let mut control: LruCache<u8, u64> = LruCache::new(capacity);
        for op in ops.iter().copied() {
            apply(&mut control, op);
        }
        prop_assert!(control.contains_key(&protected), "the control shares the state");
        control.insert(250, 0, 1);
        prop_assert!(
            !control.contains_key(&protected),
            "the control must lose its least recently used entry"
        );
        prop_assert!(control.contains_key(&displaced), "and keep the next one");

        // The branch under test: read the least recently used entry first.
        let read = cache.get(&protected).copied();
        prop_assert!(read.is_some(), "the entry being protected must be a hit");
        cache.insert(250, 0, 1);
        prop_assert!(
            cache.contains_key(&protected),
            "a key that was read must outlive an identical, never-read one"
        );
        prop_assert!(
            !cache.contains_key(&displaced),
            "and the entry behind it is what pays"
        );
    }
}

// The cache never holds more than its capacity, and holds exactly that once it
// has been reached.
//
// Both halves in one property because the second is the one that is easy to get
// wrong in the other direction: a cache that refuses to reach its capacity
// satisfies `len() <= capacity` perfectly and is useless.
proptest! {
    #[test]
    fn the_cache_fills_to_exactly_its_capacity_and_never_past_it(
        capacity in 0usize..6,
        ops in prop::collection::vec(any_op(), 0..48),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::new(capacity);

        for op in ops {
            apply(&mut cache, op);
            prop_assert!(cache.len() <= capacity, "len {} > capacity {capacity}", cache.len());
            // The two accessors must tell the same story. Stated through
            // `is_empty` against a comparison the lint cannot object to, because
            // `clippy::len_zero` fires on any `len() == 0` and this property is
            // partly *about* the difference between the two accessors.
            prop_assert_eq!(cache.is_empty(), cache.len().eq(&0));
            prop_assert_eq!(live_keys(&cache).len(), cache.len(), "a key is in two places or none");
        }

        // Fill it with distinct keys, which cannot evict each other until the
        // capacity is exceeded, and check the exact count at every step.
        let mut fresh: LruCache<u8, u64> = LruCache::new(capacity);
        for key in KEY_SPACE {
            fresh.insert(key, 0, 1);
            let expected = usize::from(key + 1).min(capacity);
            prop_assert_eq!(fresh.len(), expected, "after inserting {}", key);
        }
        prop_assert_eq!(fresh.len(), capacity);
    }
}

// The reported total is the sum of the costs of the live entries.
//
// Checked after every operation, because that is where a cost-accounting bug
// hides: a cache that forgets to subtract an evicted entry, or that charges a
// replaced key twice, reports a total that looks right until the first eviction
// and is wrong forever after. This is the property the memory ceiling in 1C-2b
// will be built on, so it is the one that has to hold exactly.
proptest! {
    #[test]
    fn the_reported_total_cost_equals_the_sum_of_the_live_entries_costs(
        capacity in 0usize..6,
        ops in prop::collection::vec(any_op(), 0..48),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::new(capacity);
        let mut model = Model::default();

        for op in ops {
            apply(&mut cache, op);
            let _ = model.apply(capacity, op);
            prop_assert_eq!(
                cache.total_cost(),
                model.total_cost(),
                "the total drifted from the live entries' costs"
            );
        }
    }
}

// The state is a pure function of the access sequence.
//
// Replaying the identical sequence on a fresh cache produces an identical set
// of live keys, an identical total cost and identical counters. A cache whose
// state depends on anything else -- a clock, a thread, a hash seed, an earlier
// run -- is a cache whose failures cannot be reproduced, which is the defect
// `core/ordering.rs`'s module docs describe for a domain that reads a clock and
// `tests/layer_boundary.rs`'s `core_names_no_clock_and_no_thread` exists to
// prevent.
proptest! {
    #[test]
    fn replaying_the_same_access_sequence_reproduces_the_same_state(
        capacity in 0usize..6,
        ops in prop::collection::vec(any_op(), 0..48),
    ) {
        let mut first: LruCache<u8, u64> = LruCache::new(capacity);
        for op in ops.iter().copied() {
            apply(&mut first, op);
        }

        let mut second: LruCache<u8, u64> = LruCache::new(capacity);
        for op in ops.iter().copied() {
            apply(&mut second, op);
        }

        prop_assert_eq!(live_keys(&first), live_keys(&second), "the live set diverged");
        prop_assert_eq!(first.total_cost(), second.total_cost(), "the total diverged");
        prop_assert_eq!(first.hits(), second.hits(), "the hit count diverged");
        prop_assert_eq!(first.misses(), second.misses(), "the miss count diverged");

        // A third cache that is cleared first must land in the same place: a
        // cache that kept state across a clear, or that leaked anything between
        // two runs, would differ here.
        let mut third: LruCache<u8, u64> = LruCache::new(capacity);
        third.clear();
        for op in ops.iter().copied() {
            apply(&mut third, op);
        }
        prop_assert_eq!(live_keys(&third), live_keys(&first), "a clear changed the outcome");
    }
}

// A membership probe and a miss do not change what survives.
//
// The contract a cold channel depends on. A `Probe` is `contains_key`; the miss
// is a `get` of a key that is not there. Interleaved at every point of a
// generated sequence, neither may change the live set -- so the operations that
// carry no information about recency have no effect at all.
proptest! {
    #[test]
    fn a_membership_probe_and_a_miss_do_not_change_what_survives(
        capacity in 0usize..6,
        ops in prop::collection::vec(any_op(), 0..32),
        noise in prop::collection::vec((100u8..200, any::<bool>()), 0..24),
    ) {
        let mut plain: LruCache<u8, u64> = LruCache::new(capacity);
        let mut noisy: LruCache<u8, u64> = LruCache::new(capacity);

        // `max(1)` rather than an empty check: with an empty `noise` vector the
        // body never runs, and a `wrapping_rem` of a zero length would panic in
        // a property test -- which is precisely the `AGENTS.md` 2.1 failure this
        // suite exists to catch in the module, not to introduce in its own test.
        let noise_len = noise.len().max(1);
        for (index, op) in ops.iter().copied().enumerate() {
            apply(&mut plain, op);
            apply(&mut noisy, op);

            // Sprinkle in up to three lookups of keys that were never inserted.
            for slot in 0..3usize {
                let Some((key, as_get)) = noise.get((index * 3 + slot) % noise_len) else {
                    continue;
                };
                if *as_get {
                    let _ = noisy.get(key);
                } else {
                    let _ = noisy.contains_key(key);
                }
            }

            prop_assert_eq!(
                live_keys(&noisy),
                live_keys(&plain),
                "an uninformative lookup changed what survived"
            );
        }
    }
}

// Repeated misses never change the recency order.
//
// Asserted directly rather than inferred from the properties above, because it
// is a decision and not a consequence: this module could have made a miss tick
// the order, and the only thing that settles which answer is right is a test
// that names it. The answer is **no** -- "I looked and it was not there" is not
// a use -- and the cost of that answer is stated in the module docs: a caller
// that probes many keys to find one hit cannot influence what survives.
proptest! {
    #[test]
    fn repeated_misses_never_change_the_recency_order(
        capacity in 1usize..6,
        ops in prop::collection::vec(any_op(), 0..32),
        absent_key in 100u8..200,
        misses in 0usize..24,
    ) {
        let mut without: LruCache<u8, u64> = LruCache::new(capacity);
        for op in ops.iter().copied() {
            apply(&mut without, op);
        }

        let mut with_misses: LruCache<u8, u64> = LruCache::new(capacity);
        for op in ops.iter().copied() {
            apply(&mut with_misses, op);
        }
        for _ in 0..misses {
            prop_assert_eq!(
                with_misses.get(&absent_key),
                None,
                "the key must really be absent for this to be a miss"
            );
        }

        prop_assert_eq!(live_keys(&with_misses), live_keys(&without), "the misses changed the order");
        prop_assert_eq!(with_misses.total_cost(), without.total_cost(), "the misses changed the cost");
        // The generated `ops` contribute misses of their own -- an `Access` of a
        // key that has not been inserted yet -- so the two caches' miss counts
        // differ by exactly the misses added here, and by nothing else.
        prop_assert_eq!(
            with_misses.misses(),
            without.misses() + misses as u64,
            "a miss moved a counter other than the one it should"
        );
        prop_assert_eq!(with_misses.hits(), without.hits(), "a miss is not a hit");
    }
}

// No sequence of operations can panic the cache, and the cache stays coherent.
//
// `AGENTS.md` 2.1 forbids a panic in a production path and 4.4 asks for a
// property over arbitrary input. The interesting inputs are the degenerate ones
// -- capacity 0, cost 0, cost `u64::MAX`, repeated replacement of one key, and
// removal of keys that were never there -- and the ones the generator reaches
// are exactly the `u64` cost extremes a hand-written table would forget. What
// this adds over the other properties is the *combinations*, which a table
// cannot enumerate.
proptest! {
    #[test]
    fn no_sequence_of_operations_can_panic_the_cache(
        capacity in 0usize..8,
        ops in prop::collection::vec(
            prop_oneof![
                (KEY_SPACE.clone(), any::<u64>(), prop_oneof![Just(0u64), Just(1), Just(u64::MAX), (0u64..4_000)])
                    .prop_map(|(key, value, cost)| Op::Insert { key, value, cost }),
                KEY_SPACE.clone().prop_map(|key| Op::Access { key }),
                KEY_SPACE.clone().prop_map(|key| Op::Probe { key }),
            ],
            0..64,
        ),
    ) {
        let mut cache: LruCache<u8, u64> = LruCache::new(capacity);
        for op in ops {
            apply(&mut cache, op);
            prop_assert!(cache.len() <= capacity, "len {} > capacity {capacity}", cache.len());
            // Every live key is findable and the count agrees, which is the
            // observable form of "the map and the recency order still agree".
            prop_assert_eq!(live_keys(&cache).len(), cache.len());
        }
    }
}
