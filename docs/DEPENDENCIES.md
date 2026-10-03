# Dependency audit — `AGENTS.md` §7.2

`AGENTS.md` §7.1 states the rule this project is most often tempted to break:

> *No new dependencies without justification — every dependency is compile time
> and supply chain risk.*

§7.2 then gives the process, five criteria, and a required artefact: a
justification comment **above the dependency in `Cargo.toml`**. This file is the
other half — the audit itself, one row per crate, and the record of what was
actually verified rather than assumed.

`PLAN.md` §7 (L118) names this file as a **Phase 1 exit criterion**, so it
covers the tree as Phase 1 leaves it, not as some later state.

## How to read the verification column

Three states, and they are not interchangeable:

| Mark | Means |
|---|---|
| **verified** | Checked against something in this repository or on this machine, and the command is named. |
| **recorded** | Carried from a prior audit, with the date and method it was taken. Re-verify before relying on it for a *new* dependency. |
| **unverified** | Not checked. Named, with the method that would settle it. |

The distinction matters because `PLAN.md` §7 (L495) already recorded a lesson
this project paid for: an assumption that was **unverifiable rather than merely
unmeasured** is worse than one that was never made. A row that says *unverified*
is honest; a row that guesses is not.

## The tree

**683 packages** in the resolved graph. **11 are direct** — 9 in production
across both crates, `serde` in the wire crate only, and 2 dev-dependencies. The
other 672 arrive transitively, almost all of them through `gpui`; auditing those
individually is not this file's job, and `gpui`'s own rev is what controls them.

| # | Crate | Version | Kind | Why it is here | §7.2.1 No std solution | License | State |
|---|---|---|---|---|---|---|---|
| 1 | `gpui` | `0.2.2` @ `e683fd7` | prod (both) | The UI framework. `PLAN.md` ADR-001. | Named by the constitution | `Apache-2.0` | verified |
| 2 | `gpui_platform` | `0.1.0` @ `e683fd7` | prod (client) | The OS-backend dispatcher. **Not optional** — since upstream `e683fd7`'s extraction, `gpui` alone yields no window on Windows. | Named by the constitution | `Apache-2.0` | verified |
| 3 | `sh_nexus_wire` | `0.1.0` | prod (client) | The protocol, shared with the server. ADR-002. | Internal | `MIT` | verified |
| 4 | `thiserror` | `2.0.21` | prod (both) | `§3.3` defines `ShNexusError` as a `#[derive(Error)]` enum and `§2.2` mandates `thiserror`. | The constitution names it | `MIT OR Apache-2.0` | verified |
| 5 | `serde` | `1.0.229` | prod (**wire only**) | The wire boundary. **The client does not depend on it** — see below. | §5 puts serialization at the wire/domain boundary | `MIT OR Apache-2.0` | verified |
| 6 | `serde_json` | `1.0.151` | prod (both) | The encoding `sh_nexus_wire` speaks. In the client: `errors.rs`'s `Serialization` variant, and `core/theme.rs`'s JSON parsing. | §2.2 mandates it; hand-writing a JSON parser is not an option | `MIT OR Apache-2.0` | verified |
| 7 | `chrono` | `0.4.45` | prod (both) | `§2.1` requires `DateTime<Utc>` for every timestamp and forbids hand-formatted RFC-3339. **`clock` enabled for the client only** — see below. | std has no timezone-aware calendar | `MIT OR Apache-2.0` | verified |
| 8 | `uuid` | `1.26.1` | prod (both) | `§7.4` requires a client-generated UUID as `client_msg_id`. **`v4` enabled for the client only** — see below. | `std` has no UUID type | `Apache-2.0 OR MIT` | verified |
| 9 | `smallvec` | `1.16.2` | prod (both) | `§2.3` mandates it for small collections and names "reactions on a message". No deps, no build script, no C. | Inline `[Reaction; 2]` is the only zero-cost alternative | `MIT OR Apache-2.0` | verified |
| 10 | `pulldown-cmark` | `0.13.4` | prod (client) | `§4.2` names markdown as a mandatory test target. `default-features = false` to drop the HTML renderer. | `§4.2` names the behaviour; no std equivalent | `MIT` | verified |
| 11 | `proptest` | `1.11.0` | dev (both) | `§4.4` mandates property tests. `default-features = false`, `features = ["std"]`. | The constitution names it | `MIT OR Apache-2.0` | verified |
| 12 | `rstest` | `0.27.0` | dev (both) | `§4.3` mandates it for parameterized tests. ADR-008. | The constitution names it | `MIT OR Apache-2.0` | verified |
| 13 | `axum` | `0.8.9` | prod (**server**) | The HTTP and WebSocket server. `PLAN.md` ADR-002, `§7.4`. `features = ["ws"]`. | `std` has no HTTP or WebSocket server | `MIT` | verified |
| 14 | `rusqlite` | `0.40.2` | prod (**server**) | SQLite bindings, no ORM. `features = ["bundled"]` — the workspace's one C dependency. | ADR-010 chose SQLite; an ORM would be 4 statements of overhead | `MIT` | verified |
| 15 | `tracing-subscriber` | `0.3.23` | prod (**server**) | The `tracing` sink. `§7.5`'s level table is unimplementable on bare `tracing`. | `§7.1` bans `println!`; a level table needs a subscriber | `MIT OR Apache-2.0` | verified |

`License` was read from **each crate's own manifest** via `cargo metadata`, not
from a summary. Every one is MIT or Apache-2.0, both compatible with this
project's `MIT`.

## `sh_nexus_server`'s dependencies, measured

The rows above are the *declarations*. This is what they cost, because `§7.2`
criterion 5 asks for a compile-time and graph figure and not an opinion.

**31 new packages in `Cargo.lock`**, all of them in rows 13-15 above and none of
them already reachable through `gpui`:

| Group | Packages |
|---|---|
| axum's HTTP stack | `axum`, `axum-core`, `hyper`, `hyper-util`, `http-body-util`, `httparse`, `httpdate`, `mime`, `tower`, `tower-layer`, `tower-service`, `matchit`, `matchers`, `sync_wrapper`, `socket2`, `mio`, `data-encoding`, `serde_path_to_error` |
| WebSocket | `tungstenite`, `tokio-tungstenite`, `sha1` |
| rusqlite's bundled SQLite | `rusqlite`, `libsqlite3-sys`, `vcpkg`, `sqlite-wasm-rs`, `hashlink`, `fallible-iterator`, `fallible-streaming-iterator`, `rsqlite-vfs` |
| The new workspace member | `sh_nexus_server`, `tokio-macros` |

`serde`, `serde_json`, `chrono`, `uuid`, `tokio`, `tracing` and `tracing-subscriber`
were **already** in the graph through `gpui`, so naming them cost nothing new —
which is exactly the argument the root `Cargo.toml` makes for them.

**The client binary is untouched.** `cargo tree -p sh_nexus --edges normal` lists
**289 unique crates** and contains **none** of `axum`, `axum-core`, `hyper`,
`hyper-util`, `tower`, `tungstenite`, `tokio-tungstenite`, `rusqlite` or
`libsqlite3-sys`. `tracing-subscriber` was already there before this milestone.
The server is a separate binary; `§6.1`'s release-binary row for the client cannot
move because of it.

**The server binary is 4.03 MiB** (`target/release/sh_nexus_server.exe`, 4,229,120
bytes). Its own resolved tree is **94 unique crates**. `§6.1`'s <30 MiB ceiling is
met with room to spare, and the figure is recorded rather than left to be
discovered, because the next milestone adds `argon2` and `jsonwebtoken` and both
are new subtrees.

### Three things the server needs that this milestone could not have

Named here rather than left as an unexplained absence in
`crates/sh_nexus_server/tests/support/mod.rs`, which says the same thing in more
detail:

1. **A WebSocket client.** `axum` is server-side only; it has no client, and
   `tokio-tungstenite` is a *transitive* dependency of it, not one of ours. A test
   that drives a real socket therefore needs either a declared
   `tokio-tungstenite` dev-dependency or a hand-written RFC 6455 client. The
   manifest is audited as it stands, so this milestone ships the ~150-line client
   in `tests/support/`. **A §7.2 audit for `tokio-tungstenite` as a
   dev-dependency is the change that deletes it.**
2. **`tempfile`.** In `Cargo.lock` through `gpui`'s tree, not declared here, for
   the same reason. `tests/support/mod.rs` rolls a `TempDir` in 40 lines.
3. **`thiserror`.** Declared for `sh_nexus` and `sh_nexus_wire`; not declared for
   the server, so `error.rs` writes nine `Display` arms by hand. The trade is
   recorded in that module's docs: if the enum grows materially, the audit is a
   small pull request.

## Two feature declarations that were wrong, and the reason it took this long to notice

**Neither `chrono` nor `uuid` gained a crate in this audit. Both gained a
*feature*, and the reason that is recorded as its own section rather than a
footnote on the two rows is that the failure mode is invisible.**

Cargo unifies features per crate version, so a crate in the graph that asks for
`chrono/clock` or `uuid/v4` hands those features to every dependent — including
this one, whose own manifest did not. Both calls therefore **compiled before this
project declared them**, and both are called from code this project owns:

| Call | Site | Feature | Who was really enabling it |
|---|---|---|---|
| `Utc::now()` | `ui/views/input_bar.rs` | `chrono/clock` | `gpui`, which enables chrono's `default` set |
| `Uuid::new_v4()` | `ui/views/input_bar.rs` | `uuid/v4` → `uuid/rng` | gpui's tree, via `accesskit` |

**That is the failure mode `PLAN.md` §7's "an assumption that was unverifiable
rather than merely unmeasured is worse than one that was never made" names, and
it ran in the direction the file's own §7.2 row 8 used to warn about.** The old
row said *"Defaults only; `rng` is deliberately off"*, and the old manifest said
`rng` "will be added when the first real generator lands" — while the build
already had `rng`, and the only thing missing was the first generator. So the
declaration was a statement about intent that the resolved graph contradicted,
and no test could see it: `cargo build` was green either way.

**What changed is that both features are now declared by the crate that uses
them**, in `crates/sh_nexus/Cargo.toml`, and `cargo metadata` is the check:

```
sh_nexus      chrono features: ["std","serde","clock"]   uuid features: ["v4"]
sh_nexus_wire chrono features: ["std","serde"]
```

**The wire crate's row is the part worth noticing, and it is the boundary
holding.** It reads no clock and generates no id — `PLAN.md` §5 puts the wall
clock and the identity on the client side of the boundary, and
`crates/sh_nexus_wire/tests/dependency_direction.rs` is what keeps it that way.
Declaring `clock` and `v4` on the client therefore does not make the wire crate
non-deterministic, and its `mapping` tests, whose `timestamp_floor` and
`timestamp_ceiling` rules depend on a supplied timestamp rather than a wall
clock, still do.

**The compile-time cost is zero and the binary-size cost is unmeasured, stated as
such.** Both features were already in the resolved graph and in the built binary;
§7.2.4's numbers below are therefore unchanged, and the release-binary figure in
`docs/BASELINES.md` is the one that would have moved if this had been a real
dependency. `uuid/v4` adds a `getrandom` call **per generated id** — one per
`Enter` — which is a per-gesture cost and not a per-frame one, and it is the cost
`PLAN.md`'s idempotent-send design is built on: a `client_msg_id` that exists
before the server has seen the message is what makes the optimistic row and the
later `ACK` the same row.

**And the server side of the same trap, avoided rather than fixed.** When
`sh_nexus_server` was written, its manifest said `chrono = { workspace = true }`
and `uuid = { workspace = true }` — which is `["std","serde"]` and `["std"]`, with
neither `clock` nor `v4`. Both features are declared on `sh_nexus`, so under
`cargo build --workspace` **or** `cargo test --workspace` a `Utc::now()` or a
`Uuid::new_v4()` in the server would have compiled and looked correct. Under
`cargo build --release -p sh_nexus_server` — the deployment command — neither
exists, and the build fails.

So the server reads its clock from `std::time::SystemTime` and converts with
chrono's pure arithmetic (`DateTime::from_timestamp_millis`, a `const fn` needing
no feature), and mints a message id from `Uuid::from_u128` over the acceptance
instant and a per-process counter. Both are documented at their call sites, and
`cargo metadata` is the check:

```
sh_nexus_server  chrono features: ["std","serde"]   uuid features: ["std"] (default, no "v4")
```

The cost of that choice is stated at each site: the id is unique within an
instance and monotonic in acceptance order rather than globally unique, and the
timestamp conversion has a documented failure mode. Both are the right trade
against a build that passes CI and fails for whoever deploys the server.

## §7.2.2 — maintenance signals, stated honestly

The criterion is "maintained? > 500 downloads/month? last commit < 6 months?".

**None of the three is verifiable from inside this repository.** They need the
registry, and this audit does not have a recorded network fetch. So:

| Crate | Maintenance signal | State |
|---|---|---|
| `rstest` | 28,006,626 downloads in the 90 days to 2026-09-06; `0.27.0` published 2026-09-06, prior release `0.26.1` in July 2025 | **recorded** — ADR-008, dated |
| `proptest`, `serde`, `serde_json`, `chrono`, `uuid`, `smallvec`, `pulldown-cmark`, `thiserror` | Not checked | **unverified** |
| `gpui`, `gpui_platform` | Not a registry crate. Its signal is the **pinned rev**, which is stricter than any of the three: see below. | **n/a by design** |

**Method to settle the unverified rows**, when a new dependency needs it rather
than when this file is written: `cargo install cargo-download` or
`cargo info <crate>`, plus the crate's own `repository` and recent release dates.
The honest position is that these eleven rows are **unverified on criterion
§7.2.2 and verified on §7.2.1, §7.2.3, §7.2.4 and §7.2.5.** They are in the tree
already, they are not a request for a new exception, and the gap is recorded
rather than papered over.

## §7.2.4 — compile-time impact

Measured on this machine, 12 logical cores, `-j 6`, warm target directory:

| Operation | Time | What it covers |
|---|---|---|
| `cargo build --workspace -j 6` (warm) | 14 s | nothing to do; the floor |
| `cargo build --workspace -j 6` (after a `gpui` change) | 50 s | `gpui` + `gpui_platform` + client |
| `cargo build --release --workspace -j 6` | 265 s | release codegen + LTO for the whole graph |
| `cargo test --workspace --tests -j 6` | 5–12 s | 890 tests |

**`gpui` and `gpui_platform` dominate every cold build, and they are not
optional.** The spike in Phase 0 established that, which is why the client's
dependency on `gpui_platform` is not `optional = true`: on Windows the window
does not exist without it. The honest reading of §7.2.4 for this project is that
**the framework is the cost, and the marginal cost of the other ten crates is not
the thing to optimise.**

## §7.2.5 — the justification comment

Every row above has a justification comment above it in the relevant
`Cargo.toml`. They are long, and that is the point: ADR-008's audit is carried in
`Cargo.toml` rather than only here, because the person adding the *next* crate
reads the manifest, not this file.

## The two findings that are not per-crate

**The client does not depend on `serde`, and that is enforced.**
`serde` appears in `sh_nexus_wire` and **not** in `sh_nexus` — verified from
`cargo metadata`. Two tests hold it:
`the_client_does_not_depend_on_serde` scans the manifest, and
`core_models_derives_no_serde_traits` scans the domain models. §5 puts
serialization at the wire/domain boundary, and a derive on a `core/models` type
would erase that boundary without any visible change to the type.

Note that `serde_json` **is** a direct dependency of the client, so the
deny-list token `"serde"` had to be split from `"serde_json"` in
`tests/layer_boundary.rs` — the substring test could not otherwise express the
distinction. See `docs/COVERAGE.md` §2.11 and the ADR-009 discussion.

**Two crates are in the tree at two versions each.**

| Crate | Ours | Also in the graph | Why |
|---|---|---|---|
| `proptest` | `1.11.0` | `1.10.0` | `gpui_platform` with `test-support` brings its own. The duplicate is the price of the headless harness the Phase 0 spike proved works. |
| `thiserror` | `2.0.21` | `1.0.69` | Transitive, from a `gpui` dependency. |

Neither is a defect, and **neither is visible in `Cargo.toml`** — a reader of the
manifest sees one version and reasonably concludes there is one. Recorded here
because the real supply chain is the resolved graph, not the manifest.

## Phase 1 exit check

`PLAN.md` L608 lists `docs/DEPENDENCIES.md` — the §7.2 audit, one row per crate —
as a Phase 1 deliverable. Twelve rows, one per direct crate, each with a
justification, a license read from the crate's own manifest, and an explicit
verification state. **Criterion §7.2.2 is unverified for eleven of them, and says
so.** That is the finding, not a gap in the finding.
