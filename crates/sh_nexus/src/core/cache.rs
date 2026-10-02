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
//! between work unit 1C-2a and work unit 1C-2b was a real seam rather than a
//! convenience:
//!
//! | `AGENTS.md` 4.2 clause | Where it lives |
//! |---|---|
//! | cache insertion | **1C-2a** |
//! | LRU eviction | **1C-2a** |
//! | hit/miss ratio | **1C-2a** |
//! | memory ceiling -- evict by cost until under a byte budget | **1C-2b** |
//! | thread safety -- internal vs external synchronisation | **1C-2b** |
//!
//! **All five are here.** 1C-2a built the mechanism -- insertion, eviction by
//! recency, hit/miss accounting, and a per-entry cost reported to the caller --
//! bounded by entry count. 1C-2b added the **policy** on top of it, and this
//! module is now bounded by two independent limits: the number of entries it
//! holds, and the total of their declared costs. Sections 12 and 13 are the two
//! decisions 1C-2b made; section 14 bounds what they cost per operation.
//!
//! What 1C-2b needed from 1C-2a, and what 1C-2a therefore provided on purpose:
//!
//! - **[`LruCache::total_cost`]**, the running sum of the costs the caller
//!   declared. A ceiling is a comparison against a budget, and there is nothing to
//!   compare without it.
//! - **The cost of each live entry, sitting next to its recency.** The policy
//!   evicts *by cost*, walking the recency order from its front, so the cost is
//!   stored alongside the recency rather than only in the lookup map. That is a
//!   decision 1C-2a made for its successor, and it is why the private
//!   representation is shaped the way it is.
//! - **Saturating arithmetic**, so a cost sum can neither panic in a debug build
//!   nor wrap in a release one.
//!
//! What 1C-2b added: the budget itself ([`LruCache::with_budget`],
//! [`LruCache::budget`], [`LruCache::set_budget`]), the loop that evicts until
//! the total is under it (`LruCache::trim_to_bounds`), the refusal of an
//! entry too large to ever fit ([`InsertOutcome`]), an eviction counter to make
//! the trim bound measurable ([`LruCache::evictions`]), and the thread-safety
//! decision of section 13.
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
//! | **Type-level shareability** | **1C-2a** | `LruCache<K, V>` is `Send + Sync` whenever `K` and `V` are, so it can be *held* across threads and moved between them. Asserted at compile time by `the_cache_is_send_and_sync` in `tests/cache.rs`, and re-asserted for the bounded cache by `the_bounded_cache_is_send_and_sync` in `tests/cache_ceiling.rs`. |
//! | **Concurrent mutation** | **1C-2b, settled in section 13** | Whether two threads may hold `&mut` to the same cache at once, and therefore whether the cache needs internal synchronisation or must be wrapped by its owner. **The answer is no internal synchronisation**, and the reasoning, the compile-time assertions, the structural guard and the rule a caller must follow instead are all in section 13. |
//!
//! The first half is free and is proved by the absence of interior mutability.
//! The second half is a genuine design decision with a real cost either way --
//! an internal lock makes the cache usable from anywhere at the price of a lock
//! on every frame-path access, and external synchronisation keeps the hot path
//! lock-free at the price of a rule its owner must not break. Section 13 makes
//! that call; the only thing section 3 forecloses is the `Rc` key, which would
//! have made the type-level half unreachable.
//!
//! # 10. The properties, and what they pin
//!
//! `AGENTS.md` 4.4 mandates property-based tests here, and they are not
//! optional for this structure: an LRU's bug class is **plausible wrong output**
//! rather than a crash, so a bug can sit in a green test suite for a long time.
//! **That is the whole argument for the mutation table in `docs/COVERAGE.md`
//! 5.3:** a ceiling bug produces output that looks entirely reasonable, and a
//! coverage number cannot see it.
//!
//! **1C-2a's eight properties, in `tests/cache.rs`, pinning the mechanism:**
//!
//! | Property | The claim it pins |
//! |---|---|
//! | `the_next_eviction_removes_the_least_recently_used_entry` | section 2's recency order, against an independent key-to-tick model, checked after every operation |
//! | `an_access_protects_an_entry_for_exactly_one_more_eviction` | the property the structure exists for, asserted exactly: the read saves one entry and the entry behind it pays |
//! | `the_cache_fills_to_exactly_its_capacity_and_never_past_it` | both halves of the count ceiling -- never over, and exactly at |
//! | `the_reported_total_cost_equals_the_sum_of_the_live_entries_costs` | section 5's exactness, which the cost ceiling depends on |
//! | `replaying_the_same_access_sequence_reproduces_the_same_state` | purity: the state is a function of the input and of nothing else |
//! | `a_membership_probe_and_a_miss_do_not_change_what_survives` | section 6, interleaved into a random sequence rather than asserted once |
//! | `repeated_misses_never_change_the_recency_order` | section 6, named directly |
//! | `no_sequence_of_operations_can_panic_the_cache` | `AGENTS.md` 2.1 over the degenerate inputs a table forgets |
//!
//! **1C-2b's eight properties, in `tests/cache_ceiling.rs`, pinning the policy.**
//! Each is checked after **every single step** rather than at the end, because a
//! ceiling that holds only at the end of a sequence has a hole in the middle of
//! one:
//!
//! | Property | The claim it pins |
//! |---|---|
//! | `no_operation_sequence_can_leave_the_cache_over_its_budget` | **the ceiling invariant**, over sequences mixing insert, access, remove, clear and budget change, compared against the budget the cache reports in force |
//! | `no_operation_sequence_can_disagree_with_an_independently_derived_model` | the live set and the total against a model that recomputes the sum and re-derives the survivors; the composition of the two bounds |
//! | `an_oversized_entry_is_refused_at_any_point_of_any_sequence` | the refusal rule as a universal claim, both branches -- admitted and refused -- asserted |
//! | `a_refused_insert_changes_nothing_at_any_point_of_any_sequence` | a refusal is total: no cost, no length, no counter, no live-set change |
//! | `a_lowered_budget_is_honoured_wherever_it_lands_in_a_sequence` | no window in which the ceiling is not a ceiling |
//! | `a_raised_or_removed_budget_evicts_nothing_at_any_point_of_a_sequence` | the direction the invariant already satisfies, so a trim loop reading the old budget cannot survive |
//! | `one_insert_never_evicts_more_entries_than_it_displaced` | section 14's trim bound, instrumented by the eviction counter |
//! | `a_budget_loose_enough_never_to_bind_changes_nothing` | **the composition claim with 1C-2a**: a non-binding budget is indistinguishable from no budget, key for key and eviction for eviction |
//!
//! The expected eviction order is computed by the test's own `BTreeMap<u8, u64>`
//! of key to last-use tick, which shares no code and no data structure with this
//! module. A model that asked the cache what order it was in would agree with it
//! by construction, and the cache deliberately exposes no way to ask. 1C-2b's
//! model is shaped differently again -- a `Vec` in recency order, with the total
//! **recomputed by folding** rather than accumulated -- because the trim is what
//! is under test and an order-preserving structure is what makes the eviction loop
//! checkable.
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
//!
//! # 12. The memory ceiling
//!
//! ## The two bounds, and that they are independent
//!
//! The cache is bounded twice, and the bounds do not know about each other:
//!
//! | Bound | Field | Enforced by |
//! |---|---|---|
//! | at most `capacity` entries | [`capacity`](LruCache::capacity) | the count half of the trim loop |
//! | at most `budget` of declared cost | [`budget`](LruCache::budget) | the cost half of the same loop |
//!
//! Both are applied by one loop after every insertion and after every budget
//! change, so **the live set is always the longest suffix of the recency order
//! that satisfies both.** They cannot disagree about the result, because there is
//! only one loop; what they can disagree about is *which* one is binding, and
//! that is a fact about the caller's numbers rather than a conflict. With
//! unit-cost entries the two coincide and the cache holds exactly
//! `min(capacity, budget)` of them, which is what
//! `the_ceiling_and_the_capacity_bound_the_cache_together` asserts.
//!
//! **An absent budget is a real state, and it is not `Some(u64::MAX)`.**
//! `LruCache::new` produces `None`; a maximal budget is behaviourally identical
//! (`cost > u64::MAX` is never true) and is treated as such. The difference is
//! what the cache can *report*: `budget()` distinguishes "no ceiling is
//! configured" from "the ceiling is eighteen exabytes", and only the first of
//! those is an actionable sentence in a bug report. `Option<u64>` costs one
//! predictable branch in a loop that already does a `Vec::remove(0)`, and
//! `no_budget_and_a_maximal_budget_enforce_the_same_rule` records that the two
//! are the same rule so the next reader does not go looking for a difference.
//!
//! ## The rule: an oversized entry is never admitted
//!
//! **An entry whose own declared cost exceeds the entire budget is refused. Not
//! admitted, not evicted -- refused, before anything is mutated.** That is the
//! only rule under which `total_cost() <= budget()` is true unconditionally, and
//! the alternative is worth spelling out because it is what a first
//! implementation reaches for.
//!
//! The naive policy is *"evict the least recently used entry until the total is
//! under budget"*, applied after inserting. Consider a 40KB budget holding
//! 40KB of avatars, and one avatar that renders at 5MB:
//!
//! ```text
//! insert(oversized_avatar)
//!   total = 5MB + 40KB
//!   evict -> 5MB + 30KB ... evict -> 5MB
//!   evict -> 0, having just evicted the oversized avatar too
//! ```
//!
//! The pre-insert total is never negative, so the arithmetic bottoms out at
//! "empty", and the entry that was most worth caching is the last one to go. The
//! cache ends up **holding nothing at the exact moment it received the most
//! expensive thing anyone asked it for** -- and it does so having destroyed every
//! useful entry on the way.
//!
//! Three alternatives were considered and rejected, and the reason is the same
//! in all three: each breaks an invariant this module promises.
//!
//! | Alternative | What it breaks |
//! |---|---|
//! | Admit it alone, over budget | `total_cost() <= budget` becomes "usually", and a caller reading `total_cost()` for a memory decision is reading a lie |
//! | Treat the budget as a soft target, checked on the *next* insert | the same hole, deferred; `total_cost() > budget()` becomes observable, and a ceiling with an observable exception is not a ceiling |
//! | Evict everything, then admit it | the cliff above: the cache empties and the ceiling still ends up violated at the moment the entry arrives |
//!
//! ## The cost of the rule, stated rather than swallowed
//!
//! **A legitimately large item is never cached, and is re-produced every time it
//! is asked for.** An avatar that renders at 5MB in a 40KB budget is not held,
//! not in whole and not in part. A rendered segment that will not fit is
//! re-rendered on every scroll past it. This is a real cost, it is paid on the
//! frame path, and no amount of cleverness inside the cache removes it -- the
//! caller chose a budget, and the only way to cache a large item under a small
//! budget is to cache it smaller or not at all.
//!
//! **`AGENTS.md` 7.1's "no unbounded growth" is a floor on what a caller may
//! choose, not a licence to override it.** A caller that finds itself refusing a
//! large item on a hot path has three honest options: raise the budget, cache a
//! cheaper representation, or accept the re-production. Silently exceeding the
//! budget is not among them, and that is the whole content of this rule.
//!
//! ## How a caller learns an insert was refused
//!
//! **Silently is not one of the answers.** Two mechanisms, and the cheap one is
//! the right one:
//!
//! 1. **Ask first.** [`budget`](LruCache::budget) is public precisely so a caller
//!    can compare the cost it is about to declare against the ceiling and skip
//!    the work. A caller that knows an avatar is 5MB does not need to render it
//!    to find out, and this is the answer that costs nothing at run time.
//! 2. **Read the outcome afterwards.**
//!    [`insert_with_outcome`](LruCache::insert_with_outcome) returns
//!    [`InsertOutcome`], whose `refused` flag is `#[must_use]` through its type.
//!    A caller that cannot avoid producing a value learns that the value was not
//!    kept, and can log it, count it, or fall back to not caching at all.
//!
//! The terse [`insert`](LruCache::insert) is kept as the third path, and it is
//! **not** silent: a refusal through it is still visible in
//! [`evictions`](LruCache::evictions) *not* having moved, and visible in
//! [`contains_key`](LruCache::contains_key) answering `false`. What the terse
//! path does not give is a way to tell a refusal from a first insertion in the
//! same call, which is why the reporting path exists and why the two share one
//! implementation rather than being written twice.
//!
//! ## A zero-cost entry, and the two bounds' blind spot
//!
//! **A declared cost of zero is legal, and it makes the budget blind to that
//! entry.** In a budget of 100, a hundred entries of cost 0 are free, and the
//! ceiling will never notice them. That is not a hole in this module, and the
//! distinction is worth making: the module's contract is that the *declared* cost
//! is the ceiling's unit, and a caller that declares 0 for something that really
//! weighs a megabyte is the thing that is wrong. The count bound still applies --
//! those entries occupy a slot in both collections -- so they are bounded, just
//! bounded by `capacity` rather than by `budget`.
//!
//! **Flooring a declared 0 to 1 was considered and rejected.** It would make the
//! trim loop tight and the budget honest, and it would also falsify
//! [`total_cost`](LruCache::total_cost), which is documented to be *the exact sum
//! of the declared costs of the live entries* and is what the ceiling is built
//! on. A ceiling computed from a number the module has adjusted is a ceiling on
//! something other than what it reports. `AGENTS.md` 2.3's "measure, don't
//! guess" and this section agree: the honest response to an under-declared cost
//! is to say so, not to quietly correct it.
//!
//! ## A lowered budget is honoured on the same call
//!
//! [`set_budget`](LruCache::set_budget) **evicts immediately, down to the new
//! budget, before it returns.** The two other defensible answers were considered
//! and rejected for one reason: both leave a window in which
//! `total_cost() > budget()` is observable.
//!
//! - *Evict at the next insertion.* Between the two calls the cache is over
//!   budget. A caller that lowered the budget to relieve memory pressure and then
//!   does not insert -- which is exactly what a user sitting on an idle screen
//!   does -- keeps the memory. The lowering did nothing.
//! - *Not honoured until the next natural trim.* The same hole, weaker: the
//!   contract becomes "eventually", which is not a contract a memory decision
//!   can be built on.
//!
//! So the budget is honoured when it is set, and the live set is the longest
//! recency suffix fitting *both* bounds immediately after the call. **The cost of
//! that answer is that `set_budget` can be a bulk eviction** -- up to
//! `capacity` entries in one call, which is why it returns how many it took, and
//! why a caller reconfiguring a cache on a settings screen is doing bulk work on
//! the main thread. That is a real cost and it is the reason to set a budget once
//! at construction rather than repeatedly at run time.
//!
//! Raising a budget, or removing it with `None`, evicts nothing: a total under the
//! old budget is under the new one, so the loop never runs.
//!
//! ## The one place the arithmetic is not exact, and it was found by a test
//!
//! **The ceiling is checked against the *reported* total, and the reported total
//! is a lower bound on the true sum of the live entries' costs once that sum would
//! exceed `u64::MAX`.** Section 5's saturating arithmetic is what makes this so,
//! and it is a real limit rather than a rounding detail:
//!
//! - With a `u64::MAX` budget, an entry declaring `u64::MAX` and a second
//!   declaring `1` are both admitted -- `1 > u64::MAX` is false -- and the
//!   reported total reads back as `u64::MAX`. The ceiling is satisfied by the
//!   report while the true sum of `2^64` is genuinely over budget. **The error is
//!   in the direction of under-stating**, so a cache can in truth hold more than
//!   its budget says, and that is worth being plain about.
//! - Worse for modelling and not for correctness: **the accumulator is
//!   path-dependent.** Remove the saturating entry and the reported total becomes
//!   `0` while the one live entry really costs `1`. So in the saturating regime
//!   `total_cost()` is not a function of the live set at all, and no reference
//!   model can predict it. That is why
//!   `the_ceiling_is_enforced_on_the_reported_total_even_when_the_true_sum_overflows`
//!   pins the behaviour and why the model property in
//!   `tests/cache_ceiling.rs` bounds its cost generator instead of making its
//!   model replicate a fiction.
//!
//! **It is unreachable for any budget this project would configure, and the
//! arithmetic is the proof rather than the assurance.** The refusal rule caps
//! every admitted cost at the budget and the count bound caps the entry count, so
//! the true sum is at most `entries x budget`. `AGENTS.md` 6.2's rows are 80MB
//! and 200MB -- budgets around `2^27` -- and exceeding `u64::MAX` at that budget
//! needs about `2^37` entries, which no machine has the memory for and which the
//! count bound is set well below.
//! `the_saturating_regime_needs_a_budget_no_client_would_configure` is the test
//! that does the multiplication.
//!
//! # 13. Thread safety: no interior synchronisation, and why
//!
//! **The decision: this module has no `Mutex`, no `RwLock`, no `Arc<Mutex<_>>`,
//! no atomics and no interior mutability of any kind. `LruCache<K, V>` is
//! `Send + Sync` whenever `K` and `V` are, and its owner is responsible for
//! confining access to one thread.** Section 9 recorded the two halves of
//! `AGENTS.md` 4.2's "thread safety" clause and deferred the second; this is the
//! answer to it.
//!
//! ## The reasoning, which is an architecture claim and not a preference
//!
//! **1. The project's architecture already guarantees single-threaded access, by
//! naming an owner.** `PLAN.md` 4 makes `state/bridge.rs` the single seam: the
//! only module permitted to call `cx.update_global` or `cx.update`, and
//! `AGENTS.md` 7.3 requires both to happen on the main thread. `network/` emits
//! plain domain events and never imports GPUI. So **every network callback
//! reaches application state through the main thread**, and a cache reached only
//! from `AppState` is main-thread-owned. The guarantee is not this module's; it
//! is the one `state/bridge.rs` exists to provide, and this module is downstream
//! of it.
//!
//! **2. A lock here would be paid on the hot path, for a property the
//! architecture already has.** `AGENTS.md` 2.3 forbids blocking the frame loop
//! and 6.2 budgets 8ms for a scroll frame. A cache of rendered message segments
//! is read on that path -- every visible message is a lookup. An internal lock
//! puts an atomic operation and a possible contention stall in front of every
//! message render, to protect against a race the architecture has already
//! excluded.
//!
//! **3. A lock would also make this module worse at the one thing 7.1 forbids.**
//! `AGENTS.md` 2.1 bans `unwrap`/`expect` in production paths, and a `Mutex` has a
//! poisoned state whose only sensible handling is to either recover with
//! `unwrap_or_else(|e| e.into_inner())` or to give up. Both belong to a *caller*'s
//! error policy, not to a pure bookkeeping structure that cannot fail. A caller
//! that wraps this cache for cross-thread use owns that decision, and can make it
//! with its own error type.
//!
//! **4. It removes a whole class of hazard rather than managing it.** An
//! internal lock brings a lock-ordering question with the rest of `state/`, a
//! re-entrancy question if a drop or an eviction could call back out, and a
//! poison question per instance. None of those can arise in a module with no
//! interior mutability and no callbacks.
//!
//! ## What the reader gets instead of a lock
//!
//! **A compile-time assertion, and a mechanical one.** `LruCache<K, V>` stays
//! `Send + Sync` whenever its parameters are, asserted by
//! `the_bounded_cache_is_send_and_sync` in `tests/cache_ceiling.rs` and by
//! `the_cache_is_send_and_sync` in `tests/cache.rs`. Those two tests cannot
//! compile if a `Mutex`, an `AtomicUsize` or a `Cell` ever appears, so the
//! property is not something a reader has to take on trust.
//!
//! **And a structural guard, which is the stronger of the two.**
//! `core_cache_contains_no_interior_mutability` in
//! `crates/sh_nexus/tests/layer_boundary.rs` scans `core/cache.rs` after
//! stripping comments and fails on `Mutex`, `RwLock`, `RefCell`, `Cell<`,
//! `UnsafeCell`, `OnceCell` or `LazyLock`. **The no-lock decision is therefore
//! enforced by a test rather than held by a review**, and adding a lock "to make
//! some caller convenient" is now a build failure that names the decision it
//! would revoke. 1C-2a wrote that test to guard the *type-level* half; 1C-2b
//! relies on it for the *concurrent-mutation* half, which is the part a compiler
//! cannot check.
//!
//! Note that this is a boundary test and not a general ban: a pure `core/` value
//! type may legitimately want a `Cell` for a cache of its own. What is forbidden
//! *here* is a lock in a structure whose documented contract is "no interior
//! mutability, and therefore `Send + Sync` whenever the parameters are."
//!
//! ## What a caller must do if it ever shares one across threads
//!
//! **This is the rule, stated so that it is actionable rather than a refusal:**
//!
//! 1. **Wrap the cache, outside `core/`, in its owner's own `Mutex`** (or
//!    `RwLock`, or a `crossbeam` channel carrying operations rather than
//!    references). `LruCache` is `Send + Sync`, so the wrapper is too, and
//!    `a_cache_its_owner_has_wrapped_is_send_and_sync_and_usable` in
//!    `tests/cache_ceiling.rs` is that statement, written so it compiles.
//! 2. **The wrapper's owner owns the poisoned-lock policy.** Not this module's
//!    problem and not its right to decide -- see reason 3 above.
//! 3. **Prefer not to.** The project as designed has one thread that touches
//!    application state. A `Mutex` here is a workaround for a caller that has
//!    already stepped outside the architecture, and the honest response to
//!    finding one is to ask which layer moved.
//!
//! **And if the reasoning above is ever wrong, the fix is to say so rather than
//! to add a lock quietly.** The reasoning rests on `PLAN.md` 4 naming
//! `state/bridge.rs` as the sole owner of `cx.update_global`. If a future work
//! unit introduces a cache that a *worker thread* must reach directly -- an
//! image decoder populating attachment previews, say -- then this decision is
//! refuted for that cache, the answer is an `Arc<Mutex<LruCache<..>>>` in the
//! layer that owns the thread, and the note belongs in this section so the next
//! reader learns it rather than re-deriving it.
//!
//! # 14. What the policy costs per operation, and the bound on it
//!
//! `AGENTS.md` 2.3 forbids blocking the frame loop and asks for a measurement
//! rather than an assurance, so this is a number.
//!
//! **The trim loop runs at most once per entry resident immediately after the
//! insert, and therefore at most `capacity + 1` times.** Each iteration removes
//! one entry, and `order` can only shrink, so the loop is bounded by the length
//! of the thing it is shrinking -- there is no unbounded loop here and no
//! iteration that can be repeated. Each iteration is an O(n) `Vec::remove(0)`
//! plus a `HashMap::remove`, so one operation is O(k·n) with `k <= capacity + 1`
//! and `n <= capacity + 1`.
//!
//! **Two facts tighten it, and both are consequences of the refusal rule rather
//! than separate guards:**
//!
//! - **The budget half never evicts the entry just inserted.** After every other
//!   entry is gone the total is exactly the inserted entry's cost, and the
//!   refusal rule guarantees that cost is within the budget -- so the loop
//!   condition is already false before the newest entry can be reached. The
//!   budget half therefore evicts at most `resident_before`, never
//!   `resident_before + 1`.
//! - **The count half can evict the entry just inserted,** and does so exactly at
//!   capacity 0. That is 1C-2a's section 8 behaviour and is unchanged.
//!
//! **The measured worst case, and it is not a comfortable number.** A single
//! insertion can displace *every* resident entry, and
//! `the_worst_case_eviction_cascade_is_exactly_what_the_adversarial_shape_predicts`
//! measures it rather than asserting an upper bound: `capacity - 1` entries that
//! declare a cost of 0, then one entry of cost 1, then one entry costing the
//! whole budget. The trim can relieve exactly one unit of pressure and that unit
//! sits behind every free entry, so **all `capacity` entries go**, and the test
//! asserts the exact count.
//!
//! **So the bound is tight, and the shape that reaches it is a caller
//! misdeclaring costs.** A real caller declares what an avatar weighs. Every
//! resident entry costing at least 1 unit means the budget half evicts at most
//! `cost_of_insertion` entries, and since the refusal rule caps that cost at the
//! budget, **a cache whose costs are all non-zero is bounded by
//! `min(capacity, budget)` evictions per insertion** -- a number the caller
//! chose, in the caller's own unit. The pathological cascade needs resident
//! entries that declare less than the incoming one, which is a bug in the
//! caller's cost function and not something this module can detect: the declared
//! cost is the only number it has (section 5, and section 12's zero-cost
//! discussion).
//!
//! **In wall-clock terms, for the population this cache actually serves:**
//! section 2's arithmetic already puts a `position` scan over 500 short keys at
//! roughly a microsecond, and `AGENTS.md` 6.2 budgets 8ms for a scroll frame.
//! The worst case above is 500 evictions x 500 elements of memmove, which is a few
//! hundred microseconds -- visible, and well inside the budget, and reached only
//! by the misdeclaring caller. **The honest reading is that this cache is safe to
//! use in a frame at the capacities it is intended for, that the worst case is
//! bounded and measured, and that the bound degrades linearly with capacity** --
//! which is the same threshold section 2 already sets at around 10,000 entries,
//! and for the same reason.

use std::collections::HashMap;
use std::hash::Hash;

/// What an insertion did: what it replaced, and whether it was refused.
///
/// Returned by [`LruCache::insert_with_outcome`] so that a caller can tell **"I
/// replaced something"** from **"I threw this away"**. The terse
/// [`insert`](LruCache::insert) returns `Option<V>` alone, and `None` on a
/// refusal is the same value as `None` on a first insertion -- so a caller that
/// must not silently drop a large value uses this type instead.
///
/// # Why this is not a `Result`
///
/// A refusal is a **policy outcome, not an error**: it is the ordinary result of
/// offering the cache something that does not fit, and this module's
/// [`LruCache`] documents that it has no error conditions at all. A `Result`
/// would need an error type, and `crates/sh_nexus/tests/layer_boundary.rs` keeps
/// `core/` from reaching `crate::errors` -- so a `Result` here would mean a
/// `core`-local error enum for exactly one variant, which is a heavier
/// representation of a fact the caller already knows.
///
/// # The cost of a refusal, and it is on the caller
///
/// **The value passed to a refused insertion is dropped, not handed back.** A
/// caller that has produced a 5MB avatar and been refused it has paid for
/// rendering it and does not get to keep it. That is the honest cost of the rule
/// (module docs, section 12) and it is deliberately *not* softened by returning
/// the value: a caller that is told "refused, and here is your value" would be
/// invited to hold an uncached value, and **whether to hold it is a memory
/// decision the caller should make explicitly** rather than one the cache makes
/// on its behalf. The cheap way to avoid the cost entirely is to compare against
/// [`LruCache::budget`] before producing the value at all.
///
/// # Example
///
/// ```
/// use sh_nexus::core::cache::LruCache;
///
/// // A 1MB budget: a 40KB avatar fits, a 5MB one cannot fit at all.
/// let mut cache: LruCache<&str, &str> = LruCache::with_budget(4, 1_000_000);
/// let outcome = cache.insert_with_outcome("avatar", "40KB", 40_960);
///
/// assert!(!outcome.refused);
/// assert_eq!(outcome.previous, None);
///
/// // One that cannot fit the whole budget is refused, and the cache is unchanged.
/// let refused = cache.insert_with_outcome("huge", "5MB", 5_000_000);
/// assert!(refused.refused);
/// assert_eq!(refused.previous, None, "nothing was replaced");
/// assert!(!cache.contains_key(&"huge"));
/// assert_eq!(cache.total_cost(), 40_960);
/// ```
#[must_use = "an insertion's outcome carries the refusal flag; discarding it is how a caller loses track of an entry it never cached"]
pub struct InsertOutcome<V> {
    /// The value that was previously held under the key, if the insertion
    /// replaced one.
    ///
    /// `None` on a **refusal**, and that is not an oversight: a refused insertion
    /// is refused *before* any mutation, so there was no previous value to
    /// replace and the resident entry is untouched. A caller that needs to know
    /// whether the old value survived should read [`refused`](Self::refused) --
    /// a `None` here with `refused` set means the old value is *still there*, not
    /// that it is gone.
    pub previous: Option<V>,
    /// Whether the entry was refused for being larger than the whole budget.
    ///
    /// `false` for every insertion that was admitted, including one that was
    /// admitted and then displaced a run of other entries. **Refused is not
    /// evicted**: a refusal moves neither [`LruCache::len`] nor
    /// [`LruCache::evictions`] nor the total cost.
    pub refused: bool,
}

/// A bounded, least-recently-used cache that reports the total cost of its
/// contents and, optionally, refuses to exceed a budget of that cost.
///
/// # What it guarantees
///
/// - **[`len`](Self::len) never exceeds [`capacity`](Self::capacity)**, and reaches it
///   exactly once enough distinct keys have been inserted.
/// - **[`total_cost`](Self::total_cost) never exceeds [`budget`](Self::budget)** when a budget is
///   set, after every operation *including* a
///   [`set_budget`](Self::set_budget) that lowers it.
/// - An entry that is read, replaced or re-inserted becomes the **most recently
///   used**. An entry that is merely *looked for and not found* changes nobody's
///   recency; see the module docs, section 6.
/// - **[`total_cost`](Self::total_cost) is always the sum of the declared costs of the live
///   entries** -- across insertions, replacements, removals and evictions.
/// - An entry costing more than the whole budget is **refused**, not admitted and
///   not evicted; see the module docs, section 12.
/// - Nothing in the cache is interior-mutable, so it is `Send + Sync` whenever
///   its parameters are, and every method that changes it takes `&mut self`. See
///   the module docs, section 13, for the decision that rests on that.
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
/// would be an error type with exactly one impossible variant. A refused
/// insertion is a policy outcome rather than an error, and is reported through
/// [`InsertOutcome`] instead.
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
///
/// With a budget, the ceiling holds and an oversized entry is refused:
///
/// ```
/// use sh_nexus::core::cache::LruCache;
///
/// let mut cache: LruCache<&str, &str> = LruCache::with_budget(2, 500);
/// cache.insert("a", "x", 300);
/// cache.insert("b", "y", 200);
///
/// assert_eq!(cache.total_cost(), 500, "exactly at the ceiling");
/// assert!(cache.insert_with_outcome("c", "z", 501).refused,
///         "one unit over the whole budget cannot be held");
/// assert_eq!(cache.total_cost(), 500, "and refusing it changed nothing");
/// ```
pub struct LruCache<K, V> {
    /// Live keys with their declared costs, least recently used first.
    ///
    /// The cost lives here and not only in `entries` because the eviction policy
    /// walks this order from the front and needs the cost of the entry it is
    /// about to drop. See the module docs, section 1.
    order: Vec<(K, u64)>,
    /// The values, reachable by key. Never the sole home of a cost.
    entries: HashMap<K, V>,
    /// The hard ceiling on `order.len()`. May be zero; see the module docs,
    /// section 8.
    capacity: usize,
    /// The ceiling on `total_cost`, in the same declared-cost units, or `None` for
    /// a cache bounded by entry count alone.
    ///
    /// `None` rather than a sentinel such as `u64::MAX`: the two enforce the same
    /// rule, and only `Option` lets [`budget`](Self::budget) report *which* of
    /// them a cache is using. See the module docs, section 12.
    budget: Option<u64>,
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
    /// Entries dropped by the trim loop since construction.
    ///
    /// **A diagnostic, and the thing that makes the trim bound of section 14
    /// measurable rather than merely asserted.** It counts *evictions only* -- a
    /// refused insertion is not an eviction, and a caller diagnosing a cache that
    /// is not growing cannot explain anything without telling those two apart.
    /// Not reset by [`clear`](Self::clear), for the same reason the hit and miss
    /// counters are not: they measure the cache's whole life.
    evictions: u64,
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
            budget: None,
            total_cost: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// Creates an empty cache bounded by **both** an entry count and a cost
    /// budget.
    ///
    /// This is the constructor for the memory ceiling of `AGENTS.md` 4.2, and it
    /// is a separate constructor rather than a defaulted second argument to
    /// [`new`](Self::new) so that a cache without a ceiling and a cache with one
    /// are visibly different things at the call site. `LruCache::new(512)` says
    /// "512 entries, cost unbounded"; `LruCache::with_budget(512, 8 << 20)` says
    /// what a caller actually means.
    ///
    /// # Arguments
    ///
    /// * `capacity` - the most entries this cache may hold at once. Zero is
    ///   allowed, and means "hold nothing"; see the module docs, section 8.
    /// * `budget` - the ceiling on the sum of the declared costs, in the same
    ///   unit the caller declares costs in. **Zero is legal** and means "admit
    ///   only entries that declare no cost at all"; see the module docs, section
    ///   12. The cache does not scale, round or validate it.
    ///
    /// # Returns
    ///
    /// An empty cache within both bounds, with no cost, no hits, no misses and no
    /// evictions.
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
    /// // 8MB of rendered segments, and never more than 256 of them.
    /// let mut cache: LruCache<&str, &str> = LruCache::with_budget(256, 8 << 20);
    /// assert_eq!(cache.capacity(), 256);
    /// assert_eq!(cache.budget(), Some(8 << 20));
    ///
    /// // A value that fits is admitted and displaces the oldest until it fits.
    /// cache.insert("seg-1", "a", 3 << 20);
    /// cache.insert("seg-2", "b", 3 << 20);
    /// cache.insert("seg-3", "c", 3 << 20);
    /// assert!(cache.total_cost() <= 8 << 20);
    ///
    /// // One that cannot fit the whole budget is refused rather than admitted.
    /// assert!(cache.insert_with_outcome("huge", "x", 9 << 20).refused);
    /// assert!(!cache.contains_key(&"huge"));
    /// ```
    pub fn with_budget(capacity: usize, budget: u64) -> Self {
        // `new` rather than a struct literal, so the two constructors cannot
        // drift apart in their initial field values -- a second literal is a
        // second place to forget a field, and the only difference between the two
        // caches is the one this call adds.
        let mut cache = Self::new(capacity);
        cache.budget = Some(budget);
        cache
    }

    /// The most entries this cache will hold at once. Never changes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The ceiling on the total declared cost, or `None` for a cache bounded by
    /// entry count alone.
    ///
    /// **Public so that a caller can check a cost before producing a value.** That
    /// is the cheap way to avoid a refusal: a caller that knows an avatar
    /// renders at 5MB does not have to render it to discover that a 40KB budget
    /// will refuse it. Comparing against this is a `u64` test; discovering it by
    /// being refused is a wasted render on the frame path.
    ///
    /// `None` and `Some(u64::MAX)` enforce the same rule -- `cost > u64::MAX` is
    /// never true -- and they are told apart here so that a diagnostic can say
    /// "no ceiling is configured", which is actionable. See the module docs,
    /// section 12.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let unbounded: LruCache<u8, u8> = LruCache::new(4);
    /// let bounded: LruCache<u8, u8> = LruCache::with_budget(4, 1_000);
    ///
    /// assert_eq!(unbounded.budget(), None);
    /// assert_eq!(bounded.budget(), Some(1_000));
    /// ```
    pub fn budget(&self) -> Option<u64> {
        self.budget
    }

    /// Installs, changes or removes the cost ceiling, honouring it **before this
    /// call returns**.
    ///
    /// # What a lowered budget does, and why it is immediate
    ///
    /// **A budget below the current total evicts immediately, least recently used
    /// first, until the total is under it.** The two alternatives -- evict at the
    /// next insertion, or not honour the change until the next natural trim --
    /// were both rejected for one reason: each leaves a window in which
    /// `total_cost() > budget()` is observable, and a ceiling with an observable
    /// exception is not a ceiling. See the module docs, section 12.
    ///
    /// **A budget at or above the current total evicts nothing**, because the
    /// cache is already within it and the trim loop never runs. That includes
    /// `None`, which is not a synonym for `Some(0)`: removing a ceiling evicts
    /// nothing and stops enforcing.
    ///
    /// # The cost, and it is a bulk operation
    ///
    /// **This call can evict up to `capacity` entries**, which is what
    /// [`evictions`](Self::evictions) reports and the reason this method returns
    /// a count. A caller reconfiguring a cache on a settings screen is doing
    /// `AGENTS.md` 2.3's discouraged work on the main thread, and the way to avoid
    /// it is to set a budget once at construction rather than repeatedly at run
    /// time. The bound on the work is in the module docs, section 14.
    ///
    /// # Arguments
    ///
    /// * `budget` - the new ceiling in declared-cost units, or `None` to remove
    ///   the ceiling and bound the cache by entry count alone.
    ///
    /// # Returns
    ///
    /// **How many entries this call evicted.** `0` for a raised or removed
    /// budget, and for a lowered budget that the cache was already within.
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
    /// let mut cache: LruCache<u8, u8> = LruCache::with_budget(8, 1_000);
    /// for key in 0..6u8 {
    ///     cache.insert(key, key, 100);
    /// }
    /// assert_eq!(cache.total_cost(), 600);
    ///
    /// // Halving the budget takes effect on this call, not on the next insert.
    /// let evicted = cache.set_budget(Some(300));
    /// assert_eq!(evicted, 3);
    /// assert_eq!(cache.total_cost(), 300);
    /// assert!(cache.total_cost() <= 300);
    ///
    /// // Raising it evicts nothing, and so does removing it.
    /// assert_eq!(cache.set_budget(Some(9_000)), 0);
    /// assert_eq!(cache.set_budget(None), 0);
    /// assert_eq!(cache.budget(), None);
    /// ```
    pub fn set_budget(&mut self, budget: Option<u64>) -> usize {
        self.budget = budget;
        // Trim *after* storing the new budget, for the same reason `insert` trims
        // after inserting: the check has to be against the value now in force, and
        // a careless edit that read the old one would keep trimming to a ceiling
        // the cache no longer has. `raising_the_budget_evicts_nothing` is the
        // test that holds it.
        self.trim_to_bounds()
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

    /// How many entries the trim loop has dropped, since construction.
    ///
    /// **Counts evictions and nothing else.** An entry dropped by
    /// [`remove`](Self::remove) or [`clear`](Self::clear) was asked for; an entry
    /// refused for being oversized was never admitted. A caller asking "why is my
    /// cache not growing" needs all three numbers, and this is the one that says
    /// whether a ceiling is doing anything.
    ///
    /// It exists partly as a diagnostic and partly as **the instrument that makes
    /// the trim bound measurable**. `AGENTS.md` 2.3 asks for a measurement rather
    /// than an assurance, and "the trim loop runs at most once per resident entry"
    /// is not measurable without a count of the evictions. The bound itself, and
    /// the shape that reaches it, are in the module docs, section 14.
    ///
    /// Not reset by [`clear`](Self::clear), for the same reason the hit and miss
    /// counters are not: they measure the cache's whole life rather than an epoch
    /// of it.
    ///
    /// # Example
    ///
    /// ```
    /// use sh_nexus::core::cache::LruCache;
    ///
    /// let mut cache: LruCache<&str, &str> = LruCache::with_budget(2, 1_000);
    /// cache.insert("a", "x", 100);
    /// cache.insert("b", "y", 100);
    /// assert_eq!(cache.evictions(), 0);
    ///
    /// cache.insert("c", "z", 100);
    /// assert_eq!(cache.evictions(), 1, "the oldest went to make room");
    ///
    /// // A refusal is not an eviction, and this counter says so.
    /// assert!(cache.insert_with_outcome("huge", "x", 10_000).refused);
    /// assert_eq!(cache.evictions(), 1);
    /// ```
    pub fn evictions(&self) -> u64 {
        self.evictions
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
    /// **The terse form.** It reports only what it replaced, which is what
    /// [`InsertOutcome::previous`] carries -- so a caller using this method
    /// **cannot tell a refusal from a first insertion**, because both report
    /// `None`. A caller that must not silently drop a large value uses
    /// [`insert_with_outcome`](Self::insert_with_outcome), which shares this
    /// method's implementation rather than duplicating it.
    ///
    /// Three behaviours, each of which has a test named after it:
    ///
    /// 1. **An existing key is replaced, not duplicated.** `len` does not grow,
    ///    and the cost becomes the new one rather than the sum of the two.
    /// 2. **A replacement is a use, so it resets recency**, on the same grounds as
    ///    [`get`](Self::get): a caller that has just produced a value has just
    ///    used it.
    /// 3. **The cache is trimmed back to both bounds after the insertion**, not
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
    /// None; see [`LruCache`]. An entry too large for the whole budget is
    /// **refused**, which is a policy outcome reported by
    /// [`insert_with_outcome`](Self::insert_with_outcome) and observable here as
    /// `contains_key` answering `false` afterwards.
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
        // A delegation and not a second implementation. An eviction written
        // twice is an eviction that will be fixed in one copy and not the other,
        // and the refusal check in particular is the kind of decision that must
        // have exactly one implementation.
        self.insert_with_outcome(key, value, cost).previous
    }

    /// Stores `value` under `key` at a declared `cost` and **reports what
    /// happened**.
    ///
    /// The same operation as [`insert`](Self::insert), plus the refusal flag. The
    /// difference is that a caller can tell **"I cached this"** from **"I threw
    /// this away"**, which the terse form's `Option<V>` cannot express: both report
    /// `None`.
    ///
    /// ## The one behaviour `insert` does not show you
    ///
    /// **An entry whose own declared cost exceeds the whole budget is refused
    /// before anything is mutated.** Not admitted and then trimmed: refused, with
    /// the cache left byte-for-byte as it was. That is what keeps
    /// `total_cost() <= budget()` true unconditionally, and the reason it matters
    /// in a *replacement* is the sharp half -- an implementation that charges the
    /// new cost first and discovers the refusal afterwards leaves the key
    /// **gone**, so a caller that had a value a moment ago now has nothing.
    /// Comparing the cost against [`budget`](Self::budget) first is the cheap
    /// way to avoid all of it.
    ///
    /// The cost of the rule is real and is stated rather than swallowed: **a
    /// legitimately large item is never cached, and is re-produced every time it
    /// is asked for.** See the module docs, section 12.
    ///
    /// Neither an insertion nor a replacement is a hit or a miss, and **a refusal
    /// moves no counter at all** -- not the hit count, not the miss count, and not
    /// [`evictions`](Self::evictions), because refused is not evicted.
    ///
    /// # Arguments
    ///
    /// * `key` - the key. Cloned once into the lookup map; see the module docs,
    ///   section 3.
    /// * `value` - the value, moved in. Never cloned, and **dropped rather than
    ///   returned if the insertion is refused**; see [`InsertOutcome`].
    /// * `cost` - what this entry costs, in the caller's own unit. **The cache
    ///   cannot compute this**; see the module docs, section 5.
    ///
    /// # Returns
    ///
    /// An [`InsertOutcome`]: what it replaced, and whether it was refused.
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
    /// let mut cache: LruCache<&str, &str> = LruCache::with_budget(4, 100);
    /// cache.insert_with_outcome("seg", "small", 40);
    ///
    /// // A replacement that fits: the old value comes back.
    /// let replaced = cache.insert_with_outcome("seg", "new", 50);
    /// assert!(!replaced.refused);
    /// assert_eq!(replaced.previous, Some("small"));
    ///
    /// // A replacement that cannot fit: refused, and the old value *survives*.
    /// let refused = cache.insert_with_outcome("seg", "huge", 101);
    /// assert!(refused.refused);
    /// assert_eq!(refused.previous, None, "nothing was replaced");
    /// assert_eq!(cache.get(&"seg"), Some(&"new"), "and what was there is still there");
    /// assert_eq!(cache.total_cost(), 50);
    /// ```
    pub fn insert_with_outcome(&mut self, key: K, value: V, cost: u64) -> InsertOutcome<V> {
        // **The refusal check, and it is first.** Nothing above this line has
        // touched the cache, which is what makes a refusal total: no cost charged,
        // no recency changed, no resident entry dropped, no counter moved. The
        // alternative -- charge, trim, notice, and put it back -- cannot restore
        // the recency order or the evicted entries, and that is not a hypothetical:
        // the naive trim evicts every other entry before reaching the oversized
        // one. See the module docs, section 12.
        if self.exceeds_budget_on_its_own(cost) {
            // `previous: None` rather than the resident value, because a refusal
            // *replaced nothing*. The value the caller passed in is dropped, which
            // is the honest cost of the rule and is why `budget()` is public.
            return InsertOutcome {
                previous: None,
                refused: true,
            };
        }

        // The recency order is updated first, and that is not an ordering
        // preference: it is the only place the entry's previous cost is knowable.
        // Finding the key here yields the cost it was carrying and removes it in
        // the same pass, which de-duplicates and resets recency at once.
        //
        // A key that is not in the order was not in the map either -- that is the
        // invariant -- so `map_or(0, ...)` is not a defensive guess: zero is
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
        // and capacity 1 keeps what it already held, and so the cost check is
        // against a total that includes the new entry. One loop for both bounds;
        // see `trim_to_bounds` and the module docs, section 12.
        self.trim_to_bounds();

        InsertOutcome {
            previous,
            refused: false,
        }
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
        // Counted here, at the single semantic point, rather than by each caller
        // of the trim loop. `trim_to_bounds` counts separately because it answers
        // a different question -- what *this* call took, which is what
        // `set_budget` hands back -- and the two are reconciled by the same
        // reasoning that reconciles a per-window hit ratio against a lifetime
        // one in `hit_ratio`.
        self.evictions = self.evictions.saturating_add(1);
    }

    /// Drops entries from the front until **both** bounds hold, and reports how
    /// many it dropped.
    ///
    /// The one implementation of the policy. `insert_with_outcome` and
    /// `set_budget` both reach the bounds through here, and they must: a trim loop
    /// written twice is a trim loop whose two copies are fixed at different times.
    ///
    /// # Why the loop terminates
    ///
    /// **Because `order` only shrinks.** Every iteration removes one entry, so
    /// there is no state in which the condition stays true with nothing left to
    /// remove. In particular the loop cannot run on an empty `order`, which is what
    /// makes `Vec::remove(0)` below sound without a guard: an empty cache has
    /// `len() == 0 <= capacity` and `total_cost() == 0 <= budget`, so the
    /// condition is already false. `AGENTS.md` 2.1 forbids a branch guarding a
    /// state that cannot be reached, and this is the one such branch deliberately
    /// *not* written; `no_operation_sequence_can_leave_the_cache_over_its_budget`
    /// is the test that exercises the degenerate configurations.
    ///
    /// # Why the entry just inserted survives the *budget* half
    ///
    /// **It does not, and that is a consequence of the refusal rule rather than a
    /// separate guard.** Once every other entry is gone the total is exactly the
    /// inserted entry's cost, and `insert_with_outcome` refused anything larger
    /// than the budget -- so the condition is false before the newest entry can be
    /// reached. The *count* half has no such protection and does evict the just
    /// inserted entry at capacity 0, which is 1C-2a's section 8 behaviour and is
    /// unchanged.
    fn trim_to_bounds(&mut self) -> usize {
        let mut evicted = 0usize;
        while self.order.len() > self.capacity || self.exceeds_budget() {
            self.evict_least_recently_used();
            evicted += 1;
        }
        evicted
    }

    /// Whether the cache is currently over its ceiling.
    ///
    /// `false` for an unbounded cache. Read after every mutation, and the only
    /// thing standing between `insert_with_outcome` and an over-budget cache, so
    /// it is deliberately one line rather than a policy anyone can edit in
    /// isolation.
    fn exceeds_budget(&self) -> bool {
        self.budget.is_some_and(|budget| self.total_cost > budget)
    }

    /// Whether an entry of `cost` could never fit, whatever else is evicted.
    ///
    /// The refusal rule, in one line, and the condition is `>` and not `>=` because
    /// the invariant is `total_cost() <= budget`: an entry may consume the whole
    /// budget and not one unit more.
    ///
    /// **`None` never exceeds anything**, so an unbounded cache refuses nothing --
    /// which is what keeps `LruCache::new` behaving exactly as 1C-2a built it.
    fn exceeds_budget_on_its_own(&self, cost: u64) -> bool {
        self.budget.is_some_and(|budget| cost > budget)
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
