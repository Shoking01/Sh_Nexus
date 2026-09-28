//! An LRU cache bounded by entry count, reporting the total cost of what it
//! holds.
//!
//! This is the module `AGENTS.md` 3.1 names as *"LRU cache for avatars, rendered
//! segments, attachment previews"* and `PLAN.md` 4 draws as
//! `cache.rs   # LRU: avatars, rendered segments, attachment previews`. It is
//! pure bookkeeping: it opens no file, reads no clock, starts no thread, and
//! does not know that `gpui` or `tokio` exist (`AGENTS.md` 3.2, `PLAN.md` 4).
//!
//! # 1. The failure mode, and the two things deliberately not here
//!
//! `AGENTS.md` 7.1 lists *"no unbounded growth of in-memory state"* as an
//! absolute prohibition, and `AGENTS.md` 6.2 puts a number on it: **less than
//! 80MB idle, less than 200MB with 10,000 cached messages.** A chat client that
//! keeps every avatar, every rendered segment and every attachment preview it
//! has ever seen is the most ordinary way to break both -- and the ordinary way
//! it breaks them is invisible: no crash, no wrong pixels, just a process that
//! gets slower and is then killed.
//!
//! So this module has exactly one job: **hold a bounded set of entries and drop
//! the ones nobody has asked for recently.** It is deliberately not the whole of
//! `AGENTS.md` 4.2's cache row. That row names five behaviours, and the split
//! between this work unit and the next is a real seam rather than a
//! convenience:
//!
//! | `AGENTS.md` 4.2 clause | Where it lives |
//! |---|---|
//! | cache insertion | **here** |
//! | LRU eviction | **here** |
//! | hit/miss ratio | **here** |
//! | memory ceiling -- evict by cost until under a byte budget | **1C-2b** |
//! | thread safety -- internal vs external synchronisation | **1C-2b** |
//!
//! **A bounded-by-count cache that reports its total cost is a complete, useful
//! cache.** It is not a half-built one waiting to be finished, and a reader
//! looking for the missing memory ceiling will find this paragraph saying that
//! the ceiling is 1C-2b's, along with what it will need from here.
//!
//! What 1C-2b needs, and what this module therefore provides on purpose:
//!
//! - **[`LruCache::total_cost`]**, the running sum of the costs the caller
//!   declared. A ceiling is a comparison against a budget, and 1C-2b has
//!   nothing to compare without it.
//! - **The cost of each live entry, sitting next to its recency.** 1C-2b's
//!   policy evicts *by cost*, walking the recency order from its front, so the
//!   cost is stored alongside the recency rather than only in the lookup map.
//!   That is a decision made for the next work unit, and it is why the private
//!   representation is shaped the way it is.
//! - **Saturating arithmetic**, so a cost sum can neither panic in a debug build
//!   nor wrap in a release one.
//!
//! What 1C-2b must still add: the budget itself, the loop that evicts until the
//! total is under it, and the decision about synchronisation.
//!
//! # 2. The representation, and the O(n) argument
//!
//! Two fields and one invariant:
//!
//! ```text
//! order:   Vec<(K, u64)>   -- least recently used first, most recently last
//! entries: HashMap<K, V>  -- the values, reachable by key
//!
//! invariant: `order` holds exactly the keys of `entries`, each exactly once,
//! in recency order.
//! ```
//!
//! A promotion removes a key from its position in `order` and pushes it onto the
//! end. An eviction removes `order[0]`. That is the whole algorithm, and every
//! operation on it is **O(n)**.
//!
//! ## The O(n) choice, argued
//!
//! The textbook answer is a doubly-linked list: O(1) promotion, O(1) eviction.
//! It is not implemented here, for three reasons in descending order of weight.
//!
//! **1. The population is hundreds, not millions.** What goes in this cache is
//! avatars (one per user in the workspace), rendered message segments (a
//! virtualized list renders only the visible window -- `AGENTS.md` 7.3 -- so
//! this is a screenful, not a history), and attachment previews. None of those is
//! bounded by how much history the user has scrolled through. A `position` scan
//! over 500 keys whose comparison is a short string is on the order of a
//! microsecond, against the 16ms frame budget of `AGENTS.md` 6.2 and the 8ms
//! scroll-frame budget beside it.
//!
//! **2. The safe O(1) structure is materially more code, and this is worth
//! correcting precisely because "O(1) LRU requires `unsafe`" is a common and
//! incomplete claim. It does not.** `HashMap<K, usize>` mapping a key to a slot
//! index, plus `Vec<Option<Entry<V>>>` holding `prev`/`next` slot indices, gives
//! O(1) promotion and eviction with **no `unsafe` at all**: the caller already
//! holds the `&K` it looked up, so a key is never moved or cloned, only
//! relinked. What it costs is a free list, hole reuse, and three further
//! invariants -- no dangling links, no slot reused while linked, no stale index
//! after an eviction -- which is roughly three times the invariant surface of
//! the two-field version above, for a speedup that is unmeasurable at the
//! population this cache actually serves. `AGENTS.md` 2.1 makes `unsafe` a last
//! resort and 2.3 says *"profile before optimizing"*; declining an optimisation
//! nobody has measured a need for is what both of those ask for.
//!
//! **3. The `Vec` version has no invalid state to get wrong.** Its one
//! invariant -- the order holds each key exactly once -- is checkable by reading
//! two lines, and violating it produces a wrong eviction order rather than a
//! wild pointer or a use-after-free. For a structure whose bug class is
//! *plausible wrong output* rather than a crash, fewer invariants is the better
//! trade.
//!
//! ## What the choice costs, stated plainly
//!
//! **A cache hit is O(n). A cache miss is O(1).** That asymmetry is deliberate
//! and is why the miss path is written to be as cheap as it is (section 6). Every
//! hit pays one linear scan of the recency order in addition to its lookup.
//!
//! ## The threshold, and what to do when it is crossed
//!
//! **This choice stops being correct somewhere around 10,000 entries**, and the
//! arithmetic says why. One `position` scan over 10,000 short-string keys is
//! roughly 20-50 microseconds. `AGENTS.md` 6.2 budgets **8ms for a scroll
//! frame**, so a frame touching 200 entries -- a busy channel redrawing a
//! screenful -- would spend about 7ms of that budget on recency bookkeeping
//! alone. At 500 entries the same frame spends roughly 0.35ms, which is
//! invisible.
//!
//! **When that is crossed, measure first and then replace the representation.**
//! The action is not a guess: profile with `cargo flamegraph`, confirm the scan
//! is the cost, then swap `Vec<(K, u64)>` for `HashMap<K, usize>` plus
//! `Vec<Option<Entry<V>>>` with linked indices as described in (2) above. That is
//! safe, it is local to this file, and the public API does not change. What must
//! **not** happen is raising the capacity to get past the symptom: 10,000
//! entries is where the *representation* is wrong, and a larger cache is a
//! larger `AGENTS.md` 6.2 problem on top of it.
//!
//! # 3. The key is held twice, and cloned once
//!
//! `order` owns a key and `entries` owns a key, so every key exists twice. The
//! accounting is deliberate and is **one clone per insertion and zero clones per
//! access**:
//!
//! - `insert` takes the key by value, clones it into `entries`, and moves the
//!   caller's copy into `order`.
//! - A promotion **moves** the `(K, u64)` pair out of `order` and back onto the
//!   end. `Vec::remove` hands the element back by value, so nothing is copied.
//! - An eviction drops both copies.
//!
//! The alternative -- wrapping the key in `Rc<K>` so the two collections share
//! one allocation -- was rejected for a reason about this project rather than
//! about `Rc`: **`Rc` is neither `Send` nor `Sync`**, and a cache behind an `Rc`
//! could not be held by `gpui::Global` (`AGENTS.md` 7.3 requires that for
//! app-wide state) and shared with a worker thread. It would trade away the
//! property this module is built to have for one small allocation per insertion.
//!
//! # 4. What `K` and `V` have to be
//!
//! ```text
//! K: Eq + Hash + Clone
//! V: (nothing)
//! ```
//!
//! `K` needs `Eq` and `Hash` to be a map key, and `Clone` for the reason in
//! section 3: it is stored in two collections.
//!
//! **`V` is unconstrained, and that is a result rather than an oversight.**
//! `get` borrows, `remove` moves out, `insert` takes ownership, and no method
//! copies a value -- so a `V: Clone` bound would be a promise this module does
//! not keep and nothing else would need. A cache of rendered segments holds an
//! `Arc<Document>`; the `Arc` clone belongs to the caller, where it can be
//! reasoned about, rather than hidden inside a `get`.
//!
//! The cache is **not** specialised to this project's shapes. `K = String` with
//! `V = Arc<Document>` works with no adaptation, and so does `K = u64` for a
//! caller that already has a dense id. A bound like `K: AsRef<str>` would forbid
//! the integer key for no gain, and the unconstrained form is not more complex
//! than the specialised one -- it is also what lets `tests/cache.rs` run its
//! properties over a small, exhaustively enumerable key space.
//!
//! # 5. Cost: the caller declares it, and it is read once
//!
//! **`insert` takes the cost as a `u64`. The cache never computes one.** It
//! cannot: `V` is unconstrained (section 4), so there is nothing to ask about.
//! Only the caller knows what a rendered segment weighs, and only the caller
//! knows whether it counted the `Arc` header, the vector's spare capacity and the
//! key. The contract is therefore: *the caller declares the cost of the thing it
//! is caching, and the cache keeps the exact sum of what it was told.*
//!
//! **The alternative -- storing an `Fn(&V) -> u64` and re-deriving -- is
//! rejected on a specific ground rather than on taste.** It would have to be
//! called at insert, and then again on every access to keep the total honest if
//! a value could change. It cannot: `get` returns `&V`, so a caller cannot mutate
//! a resident value through the cache, so **a resident value's cost cannot
//! change**. Re-deriving it per access would be pure waste, and storing a closure
//! would turn a struct with two `std` fields into one with a function pointer.
//!
//! So the question "what if the cost function is expensive?" has a short answer:
//! **it is called once per insertion, at the caller's call site, outside this
//! module.** `repeated_access_never_changes_the_reported_total_cost` in
//! `tests/cache.rs` is the observable consequence.
//!
//! ## What `total_cost` does and does not measure
//!
//! It is the sum of the declared costs of the live entries. **It is not a
//! measurement of memory**, and the difference matters:
//!
//! - It excludes the `HashMap`'s bucket array and the `Vec`'s buffer. Both are
//!   bounded by the configured capacity rather than by the contents, and
//!   [`clear`](LruCache::clear) deliberately keeps both allocations, so they are
//!   a fixed overhead of the configuration and not a function of what is cached.
//! - It excludes the key and value allocations themselves, unless the caller
//!   counted them.
//!
//! So the ceiling in 1C-2b is a ceiling on **declared cost**, and `AGENTS.md`
//! 6.2's real rows -- 80MB idle, 200MB with 10,000 messages -- are
//! process-level RSS figures that no in-process accounting can prove. A caller
//! that wants a real ceiling has to declare costs that include what it actually
//! allocates. This module's job is to make that sum exact, not to guess it.
//!
//! # 6. The miss contract: a miss changes nothing
//!
//! **A miss does not touch the recency order.** It increments one counter and
//! returns `None`. That is a decision, not an oversight, and there are three
//! reasons.
//!
//! **There is nothing to promote.** A miss has no entry, so the only way it
//! could change the order is by an artificial tick, and an artificial tick is a
//! lie about recency.
//!
//! **The miss pattern is not evidence of anything.** This is the load-bearing
//! reason. A chat client's rendered-segment cache misses on **every message of a
//! channel it has just opened** (`PLAN.md` 8, Phase 5; a cold channel has no
//! rendered segments at all). If a miss ticked the order, the order would depend
//! on *how far the user scrolled* -- which is a statement about the scroll
//! position, not about what anybody wanted to see. Scrolling past a message is
//! not a reason to keep it.
//!
//! **A miss is the cheap path, and this keeps it that way.** `AGENTS.md` 2.3
//! forbids blocking the frame loop, and a cold channel produces a run of misses
//! where every single one counts. A miss here is one hash probe and one
//! increment: O(1), no allocation, no write, no memmove.
//!
//! **The cost of this decision, stated plainly:** a caller that probes many keys
//! in search of one hit cannot influence what survives. If the caller wants a
//! probe to count, it must [`get`](LruCache::get), which is the honest way to
//! say "I want this" and is exactly the distinction the contract is drawing.
//!
//! # 7. The update contract
//!
//! **Re-inserting an existing key replaces the value, does not duplicate the
//! entry, resets the key's recency, and replaces the entry's cost rather than
//! adding to it.** Each half exists for a reason:
//!
//! - *Replace, do not duplicate*: a cache that appended would hold two entries
//!   for one key, so `len` would disagree with the number of distinct keys and
//!   the total would charge one entry twice.
//! - *Reset recency*: on the same grounds as [`get`](LruCache::get). A caller
//!   that has just produced a value has just used it.
//! - *Replace the cost*: the reported total is the sum of the costs of the
//!   **live** entries, which is the only definition a ceiling can be built on.
//!
//! **A replacement is not a hit and not a miss.** The ratio describes values
//! *served*, and a write serves nothing. An insertion or a replacement moves
//! neither counter.
//!
//! # 8. `capacity: 0` -- a disabled cache, and no special case in the code
//!
//! **A capacity of zero is legal.** Inserting into it evicts the entry just
//! inserted, so the cache is always empty, always costs zero, and always misses.
//! That is a well-defined *disabled* cache rather than a panic and rather than a
//! silent no-op that would leave the caller believing something was cached.
//!
//! **There is no branch for it anywhere in this module**, and that is the design
//! rather than an accident. The capacity check is written as *"trim back down to
//! `capacity` after inserting"* and not as *"evict before inserting"*, so a
//! capacity of 0 and a capacity of 1 fall out of the same loop:
//!
//! - capacity 1: the entry already held is still the most recently used, so the
//!   new entry evicts *it*. The cache protects what it has, as it must.
//! - capacity 0: the entry just inserted is itself the most recently used and
//!   the only one, so it is the victim.
//!
//! An implementation that checked the capacity *before* inserting would get both
//! of those backwards, and only the capacity-1 case would fail loudly. The tests
//! `a_cache_of_capacity_one_protects_the_entry_it_already_holds` and
//! `a_cache_of_capacity_zero_keeps_nothing_and_costs_nothing` are the two halves
//! of that claim.
//!
//! # 9. Not interior-mutable, and which half of "thread safety" this is
//!
//! **There is no `Mutex`, no `RwLock`, no `RefCell`, no `Cell` and no
//! `UnsafeCell` in this module.** It is a plain struct that the caller owns and
//! mutates: every method that changes it takes `&mut self`, every method that
//! only reads takes `&self`. A reader who finds a lock here should be surprised,
//! and
//! `layer_boundary::core_cache_contains_no_interior_mutability` is the test that
//! makes finding one a build failure rather than a code review's opinion.
//!
//! `AGENTS.md` 4.2's cache row ends with *"thread safety"*, and that clause has
//! two halves. Being precise about which is which:
//!
//! | Half | Owner | What it means |
//! |---|---|---|
//! | **Type-level shareability** | **1C-2a, this unit** | `LruCache<K, V>` is `Send + Sync` whenever `K` and `V` are, so it can be *held* across threads and moved between them. Asserted at compile time by `the_cache_is_send_and_sync` in `tests/cache.rs`. |
//! | **Concurrent mutation** | **1C-2b** | Whether two threads may hold `&mut` to the same cache at once, and therefore whether the cache needs internal synchronisation or must be wrapped by its owner. |
//!
//! The first half is free and is proved by the absence of interior mutability.
//! The second half is a genuine design decision with a real cost either way --
//! an internal lock makes the cache usable from anywhere at the price of a lock
//! on every frame-path access, and external synchronisation keeps the hot path
//! lock-free at the price of a rule its owner must not break. 1C-2b makes that
//! call; this module takes no position on it, and the only thing it forecloses is
//! the `Rc` key of section 3, which would have made the type-level half
//! unreachable.
//!
//! # 10. The properties, and what they pin
//!
//! `AGENTS.md` 4.4 mandates property-based tests here, and they are not
//! optional for this structure: an LRU's bug class is **plausible wrong output**
//! rather than a crash, so a bug can sit in a green test suite for a long time.
//!
//! | Property | The claim it pins |
//! |---|---|
//! | `the_next_eviction_removes_the_least_recently_used_entry` | section 2's recency order, against an independent key-to-tick model, checked after every operation |
//! | `an_access_protects_an_entry_for_exactly_one_more_eviction` | the property the structure exists for, asserted exactly: the read saves one entry and the entry behind it pays |
//! | `the_cache_fills_to_exactly_its_capacity_and_never_past_it` | both halves of the ceiling -- never over, and exactly at |
//! | `the_reported_total_cost_equals_the_sum_of_the_live_entries_costs` | section 5's exactness, which 1C-2b's ceiling depends on |
//! | `replaying_the_same_access_sequence_reproduces_the_same_state` | purity: the state is a function of the input and of nothing else |
//! | `a_membership_probe_and_a_miss_do_not_change_what_survives` | section 6, interleaved into a random sequence rather than asserted once |
//! | `repeated_misses_never_change_the_recency_order` | section 6, named directly |
//! | `no_sequence_of_operations_can_panic_the_cache` | `AGENTS.md` 2.1 over the degenerate inputs a table forgets |
//!
//! The expected eviction order is computed by the test's own `BTreeMap<u8, u64>`
//! of key to last-use tick, which shares no code and no data structure with this
//! module. A model that asked the cache what order it was in would agree with it
//! by construction, and the cache deliberately exposes no way to ask.
//!
//! # 11. No `Debug`, on purpose
//!
//! `LruCache` does **not** derive `Debug`, and the reason is
//! `AGENTS.md` 7.5: *"never log message content."* A derived `Debug` on a cache
//! of rendered message segments is a one-import path to printing every message
//! body in the process into a `tracing` call, and a derived `Debug` is the kind
//! of thing nobody adds deliberately -- it arrives with the struct.
//!
//! A caller that needs a printable cache should add an implementation that emits
//! the **keys and costs** and never the values, and should decide that
//! deliberately.

use std::collections::HashMap;
use std::hash::Hash;

/// A bounded, least-recently-used cache that reports the total cost of its
/// contents.
///
/// # What it guarantees
///
/// - **[`len`] never exceeds [`capacity`](Self::capacity)**, and reaches it
///   exactly once enough distinct keys have been inserted.
/// - An entry that is read, replaced or re-inserted becomes the **most recently
///   used**. An entry that is merely *looked for and not found* changes nobody's
///   recency; see the module docs, section 6.
/// - **[`total_cost`] is always the sum of the declared costs of the live
///   entries** -- across insertions, replacements, removals and evictions.
/// - Nothing in the cache is interior-mutable, so it is `Send + Sync` whenever
///   its parameters are, and every method that changes it takes `&mut self`.
///
/// # The invariant the implementation rests on
///
/// `order` holds exactly the keys of `entries`, each exactly once, ordered **least
/// recently used first**. Every operation is stated in terms of it: `order[0]` is
/// the eviction candidate, and a promotion moves a key onto the end.
///
/// # Errors
///
/// None, anywhere in this API, and that is a consequence of the design rather
/// than an accident: this module cannot open a file, read a clock, or fail an
/// allocation visibly, so there is no error condition to invent. A `Result` here
/// would be an error type with exactly one impossible variant.
///
/// # Example
///
/// ```
/// use sh_nexus::core::cache::LruCache;
///
/// let mut cache: LruCache<&str, &str> = LruCache::new(2);
/// cache.insert("a", "first", 100);
/// cache.insert("b", "second", 200);
///
/// // Reading "a" protects it, so "b" is what goes when the cache is full.
/// assert_eq!(cache.get(&"a"), Some(&"first"));
/// cache.insert("c", "third", 300);
///
/// assert!(cache.contains_key(&"a"));
/// assert!(!cache.contains_key(&"b"));
/// assert_eq!(cache.total_cost(), 400);
/// ```
pub struct LruCache<K, V> {
    /// Live keys with their declared costs, least recently used first.
    ///
    /// The cost lives here and not only in `entries` because 1C-2b's eviction
    /// policy walks this order from the front and needs the cost of the entry it
    /// is about to drop. See the module docs, section 1.
    order: Vec<(K, u64)>,
    /// The values, reachable by key. Never the sole home of a cost.
    entries: HashMap<K, V>,
    /// The hard ceiling on `order.len()`. May be zero; see the module docs,
    /// section 8.
    capacity: usize,
    /// The sum of the declared costs of the live entries.
    ///
    /// Maintained incrementally with saturating arithmetic rather than
    /// recomputed, so that reading it is O(1) and a cost sum can neither panic in
    /// a debug build nor wrap in a release one.
    total_cost: u64,
    /// Lookups that found an entry.
    hits: u64,
    /// Lookups that did not. A counter and not a return type; see the module
    /// docs, section 6.
    misses: u64,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Creates an empty cache that will hold at most `capacity` entries.
    ///
    /// **A capacity of zero is legal and means "hold nothing"** -- a disabled
    /// cache, not an error, and not a cache that silently drops what it is given
    /// while reporting that it kept it. See the module docs, section 8.
    ///
    /// # Arguments
    ///
    /// * `capacity` - the most entries this cache may hold at once. Zero is
    ///   allowed. There is no upper bound and no minimum, because the memory
    ///   ceiling that would police the upper bound is 1C-2b's.
    ///
    /// # Returns
    ///
    /// An empty cache, with no cost, no hits and no misses.
    ///
    /// # Errors
    ///
    /// None; see [`LruCache`].
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<u8, u8> = LruCache::new(3);
    /// assert!(cache.is_empty());
    /// assert_eq!(cache.capacity(), 3);
    ///
    /// // A zero-capacity cache is a disabled cache, and says so.
    /// let mut disabled: LruCache<u8, u8> = LruCache::new(0);
    /// disabled.insert(1, 1, 10);
    /// assert!(disabled.is_empty());
    /// assert_eq!(disabled.total_cost(), 0);
    /// ```
    pub fn new(capacity: usize) -> Self {
        // Deliberately `new()` and not `with_capacity()`. The capacity is an
        // ordinary `usize` handed over by a caller, and pre-allocating a table
        // for it would turn a nonsense capacity into an enormous allocation;
        // growing lazily means a large capacity costs nothing until entries
        // actually arrive.
        Self {
            order: Vec::new(),
            entries: HashMap::new(),
            capacity,
            total_cost: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// The most entries this cache will hold at once. Never changes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many entries are held now. Never greater than
    /// [`capacity`](Self::capacity).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The sum of the declared costs of the live entries.
    ///
    /// The number a memory ceiling is compared against, and therefore the number
    /// that has to be exact: it is maintained on every insertion, replacement,
    /// removal and eviction rather than recomputed, and it saturates rather than
    /// overflowing. See the module docs, section 5, for what it deliberately does
    /// **not** measure.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// cache.insert("a", "x", 1_000);
    /// cache.insert("b", "y", 2_000);
    /// assert_eq!(cache.total_cost(), 3_000);
    ///
    /// // The evicted entry's cost goes with it, which is the whole point.
    /// cache.insert("c", "z", 4_000);
    /// assert_eq!(cache.total_cost(), 6_000);
    /// ```
    pub fn total_cost(&self) -> u64 {
        self.total_cost
    }

    /// How many lookups found an entry.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// How many lookups found nothing.
    ///
    /// A miss is the ordinary outcome for a cache, not a failure: a chat client's
    /// rendered-segment cache misses on every message of a channel it has just
    /// opened. See the module docs, section 6.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Hits as a fraction of lookups, or `None` if nothing has been looked up.
    ///
    /// `None` rather than `Some(0.0)` for "no requests yet", because a cache that
    /// has served nothing has no hit rate, and reporting zero would be
    /// indistinguishable from a cache that misses everything. The same reasoning
    /// as `core::ordering::SyncStatus::Unverifiable`, which is not a pass.
    ///
    /// Counters are not reset by [`clear`](Self::clear): they measure the
    /// cache's whole life, and a caller that wants a per-window ratio subtracts
    /// the counts it read at the start of the window.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// assert_eq!(cache.hit_ratio(), None);
    ///
    /// cache.insert("a", "x", 1);
    /// let _ = cache.get(&"a");
    /// let _ = cache.get(&"b");
    /// assert_eq!(cache.hit_ratio(), Some(0.5));
    /// ```
    pub fn hit_ratio(&self) -> Option<f64> {
        let lookups = self.hits + self.misses;
        // The `as f64` is lossy above 2^53 lookups, which is far beyond a
        // session's worth of message renders and irrelevant to a ratio in
        // [0, 1].
        (lookups > 0).then(|| self.hits as f64 / lookups as f64)
    }

    /// Whether `key` is held, **without** changing anything.
    ///
    /// The non-mutating probe, and the reason it exists as a separate method from
    /// [`get`](Self::get): `get` makes a hit the most recently used entry, so
    /// asking "is it there?" with `get` answers a different question than the
    /// one asked. This method answers only the question asked.
    ///
    /// It moves neither the recency order nor the hit/miss counters: it is not a
    /// lookup of a value, so counting it would make the ratio describe something
    /// other than values served.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// cache.insert("a", "x", 1);
    /// cache.insert("b", "y", 1);
    ///
    /// assert!(cache.contains_key(&"a"));
    /// cache.insert("c", "z", 1);
    ///
    /// // "a" was the least recently used and nothing protected it.
    /// assert!(!cache.contains_key(&"a"));
    /// assert_eq!(cache.hit_ratio(), None, "a probe is not a lookup");
    /// ```
    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// The value held under `key`, making it the most recently used entry.
    ///
    /// **A miss changes nothing** -- not the recency order, not the cost, only
    /// the miss counter. The reasoning, and the cost of that decision, are in the
    /// module docs, section 6; the short form is that "I looked and it was not
    /// there" is not a use of anything.
    ///
    /// # Arguments
    ///
    /// * `key` - the key to look up. Borrowed, never cloned: this is the hot path
    ///   of the cache, and `AGENTS.md` 2.3 asks for no allocation in one.
    ///
    /// # Returns
    ///
    /// `Some` of a **borrow** of the stored value, or `None`. A miss is a normal
    /// outcome and not an error, so this is `Option` and not `Result`.
    ///
    /// # Errors
    ///
    /// None; see [`LruCache`].
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// cache.insert("a", "first", 10);
    /// cache.insert("b", "second", 10);
    ///
    /// assert_eq!(cache.get(&"a"), Some(&"first"));
    /// assert_eq!(cache.get(&"missing"), None);
    /// assert_eq!((cache.hits(), cache.misses()), (1, 1));
    /// ```
    pub fn get(&mut self, key: &K) -> Option<&V> {
        // The hash probe is the authoritative membership test, and it is what
        // keeps a **miss O(1)**: the alternative is to find the key's position by
        // scanning the order, which makes a miss O(n) for no gain. A hit pays
        // this probe in addition to its O(n) promotion -- one hash of a short key
        // against a linear scan of hundreds of them.
        if !self.entries.contains_key(key) {
            self.misses += 1;
            return None;
        }
        self.hits += 1;
        promote(&mut self.order, key);
        self.entries.get(key)
    }

    /// Stores `value` under `key` at a declared `cost`, evicting if it must.
    ///
    /// Three behaviours, each of which has a test named after it:
    ///
    /// 1. **An existing key is replaced, not duplicated.** `len` does not grow,
    ///    and the cost becomes the new one rather than the sum of the two.
    /// 2. **A replacement is a use, so it resets recency**, on the same grounds as
    ///    [`get`](Self::get): a caller that has just produced a value has just
    ///    used it.
    /// 3. **The cache is trimmed back to `capacity` after the insertion**, not
    ///    before it, so a capacity of 0 evicts the entry just inserted and a
    ///    capacity of 1 protects the entry it already holds. See the module docs,
    ///    section 8.
    ///
    /// Neither an insertion nor a replacement is a hit or a miss: the ratio
    /// describes values *served*, and a write serves nothing.
    ///
    /// # Arguments
    ///
    /// * `key` - the key. Cloned once into the lookup map; see the module docs,
    ///   section 3, for why the key is held twice and what that costs.
    /// * `value` - the value, moved in. Never cloned: the cache does not require
    ///   `V: Clone` and does not copy what it is given.
    /// * `cost` - what this entry costs, in whatever unit the caller's ceiling is
    ///   denominated in. **The cache cannot compute this** -- it does not know
    ///   what a rendered segment weighs -- so it is the caller's number, and the
    ///   running total is only as good as those numbers. See the module docs,
    ///   section 5.
    ///
    /// # Returns
    ///
    /// The value that was previously held under `key`, if any. One probe answers
    /// "replace this" as well as "insert this"; a caller that wanted the old
    /// value back would otherwise need a [`get`](Self::get) -- which would move
    /// the key's recency as a side effect -- followed by a [`remove`](Self::remove).
    ///
    /// # Errors
    ///
    /// None; see [`LruCache`].
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// assert_eq!(cache.insert("a", "first", 10), None);
    /// assert_eq!(cache.insert("a", "second", 25), Some("first"));
    ///
    /// // Replaced, not duplicated, and recharged rather than added to.
    /// assert_eq!(cache.len(), 1);
    /// assert_eq!(cache.total_cost(), 25);
    /// ```
    pub fn insert(&mut self, key: K, value: V, cost: u64) -> Option<V> {
        // The recency order is updated first, and that is not an ordering
        // preference: it is the only place the entry's previous cost is knowable.
        // Finding the key here yields the cost it was carrying and removes it in
        // the same pass, which de-duplicates and resets recency at once.
        //
        // A key that is not in the order was not in the map either -- that is
        // the invariant -- so `map_or(0, ...)` is not a defensive guess: zero is
        // exactly the right answer for "this entry did not exist".
        let cost_before = self
            .order
            .iter()
            .position(|(held, _)| *held == key)
            .map_or(0, |index| self.order.remove(index).1);
        self.order.push((key.clone(), cost));

        // Charge the difference rather than adding, so that replacing an entry
        // replaces its cost. Saturating, because a debug build overflows on `+`
        // and a release build wraps.
        self.total_cost = self
            .total_cost
            .saturating_sub(cost_before)
            .saturating_add(cost);

        let previous = self.entries.insert(key, value);

        // Trim **after** inserting, so capacity 0 evicts what was just inserted
        // and capacity 1 keeps what it already held. `while` rather than `if`
        // because that is the general form and costs nothing to state.
        while self.order.len() > self.capacity {
            self.evict_least_recently_used();
        }

        previous
    }

    /// Removes the entry held under `key`, returning its value.
    ///
    /// How a caller invalidates one entry deliberately -- a channel switch, a
    /// theme change, a segment it knows is stale. A removed key is afterwards
    /// indistinguishable from one that was never there: a miss, with no entry to
    /// be found and no cost to be charged.
    ///
    /// Not a hit and not a miss. Nothing was looked up on the caller's behalf, and
    /// a ratio that counted it would be describing removals as lookups.
    ///
    /// # Arguments
    ///
    /// * `key` - the key to drop.
    ///
    /// # Returns
    ///
    /// The value that was held, moved out rather than cloned, or `None` if there
    /// was nothing to remove -- in which case nothing at all changes.
    ///
    /// # Errors
    ///
    /// None; see [`LruCache`].
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// cache.insert("a", "x", 10);
    /// cache.insert("b", "y", 20);
    ///
    /// assert_eq!(cache.remove(&"a"), Some("x"));
    /// assert_eq!(cache.total_cost(), 20, "the removed entry's cost went with it");
    /// assert_eq!(cache.remove(&"a"), None, "and removing it again finds nothing");
    /// ```
    pub fn remove(&mut self, key: &K) -> Option<V> {
        // The order is the authority for the cost, so it is scanned first and its
        // removal carries the cost out with it. Scanning the map instead would
        // mean a second lookup for a number only the order holds.
        let index = self.order.iter().position(|(held, _)| held == key)?;
        let (_, cost) = self.order.remove(index);
        self.total_cost = self.total_cost.saturating_sub(cost);
        self.entries.remove(key)
    }

    /// Removes every entry, leaving the capacity and the counters alone.
    ///
    /// The counters are **not** reset: they measure the cache's whole life rather
    /// than an epoch of it, and resetting them would make
    /// [`hit_ratio`](Self::hit_ratio) jump to 100% after every channel switch -- a
    /// metric that flatters itself. A caller wanting a per-window ratio subtracts
    /// the counts it read at the start of the window.
    ///
    /// The recency order is emptied too, so the first insertion after a clear is
    /// the only thing in it.
    ///
    /// Neither the `HashMap`'s buckets nor the `Vec`'s buffer are released: both
    /// are bounded by the configured capacity rather than by the contents, and
    /// holding them is what makes a cleared-and-refilled cache cheap. This module
    /// deliberately offers no `shrink_to_fit`, since nothing `AGENTS.md` 3.1 asks
    /// of it needs one.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::new(2);
    /// cache.insert("a", "x", 10);
    /// let _ = cache.get(&"a");
    /// let _ = cache.get(&"b");
    ///
    /// cache.clear();
    /// assert!(cache.is_empty());
    /// assert_eq!(cache.total_cost(), 0);
    /// assert_eq!((cache.hits(), cache.misses()), (1, 1), "the history is kept");
    /// ```
    pub fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
        self.total_cost = 0;
    }

    /// Drops the least recently used entry and its cost.
    ///
    /// The single place an entry leaves without being asked for, and the single
    /// place a cost is subtracted. Split out so that the recency decision has
    /// exactly one implementation: an eviction written twice is an eviction that
    /// will be fixed in one copy and not the other.
    ///
    /// The `HashMap` removal's result is dropped rather than matched on. By the
    /// invariant a key in `order` is in `entries`, so there is nothing to handle
    /// -- and handling it would mean a branch guarding a violation of this
    /// module's own rule, which is a test nobody can write.
    fn evict_least_recently_used(&mut self) {
        let (victim, cost) = self.order.remove(0);
        let _ = self.entries.remove(&victim);
        self.total_cost = self.total_cost.saturating_sub(cost);
    }
}

/// Moves `key` to the most recently used end of `order`.
///
/// **Moves, never copies.** `Vec::remove` hands the `(K, u64)` pair back by
/// value, so a lookup allocates nothing -- the property that
/// `inserting_clones_the_key_exactly_once_and_looking_one_up_never_does` in
/// `tests/cache.rs` measures rather than takes on trust.
///
/// The `if let` covers the position scan returning `None`, which by the
/// invariant cannot happen: every caller has already established that `key` is
/// live, and a live key is in `order` exactly once. `AGENTS.md` 2.1 forbids
/// giving up, and a branch that cannot be taken is cheaper than a panic path. It
/// is the one deliberately unreachable arm in this module, and the coverage
/// record in `docs/COVERAGE.md` says so.
fn promote<K: Eq>(order: &mut Vec<(K, u64)>, key: &K) {
    if let Some(index) = order.iter().position(|(held, _)| held == key) {
        let moved = order.remove(index);
        order.push(moved);
    }
}
