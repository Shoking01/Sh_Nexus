//! The layer boundary holds, and is enforced rather than documented.
//!
//! `AGENTS.md` §3.2 says `core/` "must not import `gpui`, `tokio`, or any
//! UI/platform code", and `PLAN.md` §4 adds the filesystem. Those are the rules
//! that make the ≥90% coverage floor for `core/` (§4.1) *reachable* rather than
//! aspirational: a layer that opens a socket or reads a file cannot be tested
//! hermetically at that rate, so the floor is either kept by keeping the layer
//! pure, or waived.
//!
//! Documentation of a boundary nothing enforces is a comment. This file makes it
//! a test failure.
//!
//! # What is checked
//!
//! | Rule | Source | Checked here |
//! |---|---|---|
//! | `core/` names no I/O, platform, or serialization-derive crate | §3.2, `PLAN.md` §4, §5 | `core_names_no_forbidden_dependency` |
//! | `core/` may name the `serde_json` *parser* but never the `serde` *derive* crate | `PLAN.md` §5 | `core_admits_serde_json_and_still_rejects_serde` |
//! | `core/models` derives no `Serialize`/`Deserialize` | `PLAN.md` §5 | `core_models_derives_no_serde_traits` |
//! | `core/` touches no filesystem | `PLAN.md` §4 | `core_names_no_forbidden_dependency` |
//! | `core/` reads no clock and starts no thread | `core/models/mod.rs` | `core_names_no_clock_and_no_thread` |
//! | `core/` contains no panicking construct | §7.1 | `core_contains_no_panicking_construct` |
//! | `network/` names no `gpui` | §3.2 | `network_names_no_gpui` |
//! | `ui/` reaches the layers below it only through the seam | §3.2, `PLAN.md` §4, ADR-006 step 1 | `ui_reaches_gpui_and_the_bridge_and_nothing_below_them` |
//! | that rule can still catch a violation | — | `the_ui_boundary_rejects_the_layers_below_it` |
//! | The protocol does not depend on the client | ADR-002 | `the_wire_crate_does_not_depend_on_the_client` |
//! | The client depends on the protocol | ADR-002 | `the_client_depends_on_the_wire_crate` |
//!
//! # How the source is read
//!
//! **Comments are stripped before the scan**, and that is load-bearing rather
//! than a nicety: `core/models/mod.rs` says in prose that these types "know
//! nothing about serde", and a naive substring scan would fail on its own
//! documentation. Stripping first means a comment can *discuss* a forbidden
//! dependency, which is exactly what good documentation of a boundary should do,
//! and the scan still holds.
//!
//! The alternative -- a `use`-statement allow-list -- is stricter but produces
//! false positives on `use` items inside functions and on macro-generated paths,
//! and a test that cries wolf gets deleted.

use std::fs;
use std::path::{Path, PathBuf};

/// The client's `src/` directory.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `directory`, recursively.
///
/// A `PathBuf` throughout, per `AGENTS.md` §2.1: "no `String` for paths".
fn rust_files_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files_under(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// A file's source with every comment removed.
///
/// Handles `//` to end of line -- which covers `///` and `//!` -- and `/* */`
/// blocks. Sufficient for this tree: `core/` contains no string literal with a
/// `//` in it, and if one ever did, the scan would strip to the closing quote and
/// the test would report a false positive naming the exact file and line, which
/// is a two-second fix.
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

/// Every `use` statement in a file, as (line number, first path segment).
///
/// The segment is the crate name for an external crate, or `crate` / `super` /
/// `self` for an internal path. Enough to answer "does this file depend on X?",
/// which is the question the boundary is about.
fn use_statements(stripped: &str) -> Vec<(usize, String)> {
    stripped
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            let rest = trimmed
                .strip_prefix("pub use ")
                .or_else(|| trimmed.strip_prefix("use "))?;
            let path = rest.split("::").next().unwrap_or_default();
            let segment = path
                .trim_start_matches("::")
                .split("::")
                .next()
                .unwrap_or_default()
                .to_owned();
            (!segment.is_empty()).then(|| (index + 1, segment))
        })
        .collect()
}

/// Crates `core/` may name.
///
/// An allow-list, not a deny-list, and that is the stronger choice: a crate added
/// to `Cargo.toml` tomorrow is rejected here until somebody adds it to this list
/// and says why. A deny-list has to be updated in the same commit as the
/// dependency or it silently permits the new one.
///
/// `crate` and `super` are allowed because `core/` necessarily refers to its own
/// submodules; [`core_reaches_only_its_own_modules`] is the separate test that
/// checks *which* internal paths it reaches.
///
/// **`pulldown-cmark` was added here in work unit 1C-1, in the same commit that
/// added it to `Cargo.toml`.** That is the property that makes an allow-list
/// worth maintaining: the dependency was *rejected* until the decision was
/// recorded, so the two files cannot drift. It is also the narrowest addition
/// this list could take -- `core/markdown.rs` is the only file that names it, it
/// parses a string and returns data, and it has no I/O, no clock, no thread and
/// no handle on anything the caller does not already hold. The parsing crates
/// `PLAN.md` section 2 lists for other layers stay out on purpose: `syntect`
/// (syntax highlighting) is a Phase 5 concern and `notify` is a filesystem
/// watcher, which is `platform/file_watch.rs`.
///
/// **`serde_json` was added here in work unit 1D, in the same commit that
/// started `core/theme.rs` naming it.** It is the second parsing crate admitted,
/// and it was admitted for the same narrow reason as the first: `core/theme.rs`
/// is the only file that names it, it parses a theme document's bytes into data
/// and hands back data, and it has no I/O, no clock, no thread, no file handle
/// and no dependency on anything the caller does not already hold. It is a
/// *parser*, not a *serialization framework*: nothing in `core/` derives a trait
/// from it, which is the property `the_client_does_not_depend_on_serde` and
/// `core_models_derives_no_serde_traits` between them guard. Admitting it here
/// without splitting the `"serde"` token would have been a no-op, because
/// `"serde_json".contains("serde")` -- see the note on that token.
const CORE_ALLOWED_CRATES: [&str; 10] = [
    "std",
    "core",
    "alloc",
    "chrono",
    "smallvec",
    "uuid",
    "pulldown_cmark",
    "serde_json",
    "crate",
    "super",
];

/// Crates `core/` may never name.
///
/// **`serde` is not in this list, and that is the second half of the split
/// described on [`forbidden_serde_mention`].** It was here until work unit 1D,
/// and a flat substring list cannot express "not `serde`, but `serde_json`":
/// the one entry rejected both crates, so the boundary was simultaneously too
/// strict about a parser it should permit and imprecise about the crate it
/// actually forbids.
const CORE_FORBIDDEN_TOKENS: [&str; 9] = [
    "gpui",
    "tokio",
    "sh_nexus_wire",
    "reqwest",
    "rusqlite",
    "std::fs",
    "std::io",
    "std::net",
    "std::process",
];

/// The literal forbidden in `core/`: the `serde` crate, which brings the derive
/// macros with it.
const SERDE_TOKEN: &str = "serde";

/// The one suffix that turns [`SERDE_TOKEN`] into a permitted crate name.
///
/// `serde` + `_json` = `serde_json`, the JSON *parser*. One entry rather than a
/// list of permitted `serde*` crates, because the rule is about a prefix and a
/// deny-list of every crate starting with `serde` would have to be extended
/// every time somebody published `serde_json_derive`.
const PERMITTED_SERDE_SUFFIX: &str = "_json";

/// A mention of the `serde` crate in comment-stripped `core/` source, if any.
///
/// **The split, and why it is a tightening rather than a loosening.** Until work
/// unit 1D, `core/`'s ban on `serde` was one entry in a substring scan, and
/// `"serde"` is a substring of `"serde_json"`. That single fact made the old
/// check wrong in two directions at once:
///
/// - **Too strict where it should have been permissive.** `core/theme.rs` must
///   parse a JSON theme document, and there is no way to spell that without
///   naming a JSON parser. A fully qualified `serde_json::from_slice(..)` -- the
///   spelling the token scan exists precisely to catch -- was rejected too,
///   because the parser and the derive crate were indistinguishable to a
///   `contains` call.
/// - **Imprecise where it mattered.** The check could not say *which* crate it
///   had found, so a violation reported "mentions `serde`" whether the source
///   said `serde`, `serde_json` or `serde_derive`.
///   `core_models_derives_no_serde_traits` exists precisely because such a check
///   is not sufficient, and this is the half of that insufficiency.
///
/// The rule now: **every occurrence of the literal `serde` in `core/` must be
/// part of the single permitted name `serde_json`.** That is strictly more
/// rejections than the old check could express -- `use serde;`, `serde::Serialize`,
/// `#[derive(serde::Serialize)]` and `serde_derive::` are all now rejected *for
/// a stated reason* rather than incidentally -- and it is one fewer false
/// positive, the parser, which is a real dependency this crate already had.
///
/// Returns the text that followed the offending `serde`, so a failure can name
/// what it found instead of only that it found something.
fn forbidden_serde_mention(stripped: &str) -> Option<String> {
    let mut offset = 0;
    while let Some(found) = stripped[offset..].find(SERDE_TOKEN) {
        let after = offset + found + SERDE_TOKEN.len();
        if !stripped[after..].starts_with(PERMITTED_SERDE_SUFFIX) {
            return Some(stripped[after..].chars().take(24).collect());
        }
        offset = after;
    }
    None
}

/// `core/` names no I/O, platform, or serialization crate.
///
/// Checked three times over, because the three checks fail differently:
///
/// - a **`use` allow-list**, which is exact but only sees imports;
/// - a **token scan** over comment-stripped source, which catches a fully
///   qualified path (`chrono::Utc` inline, or `serde_json::from_str(..)` written
///   without a `use`) that no import list would show;
/// - a **precise `serde` check**, because the token scan cannot tell `serde` from
///   `serde_json` and the boundary has to. See [`forbidden_serde_mention`].
#[test]
fn core_names_no_forbidden_dependency() {
    let core_dir = src_dir().join("core");
    let files = rust_files_under(&core_dir);
    assert!(
        !files.is_empty(),
        "no Rust files found under {} -- the scan is looking in the wrong place",
        core_dir.display()
    );

    for file in &files {
        let source = fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display()));
        let stripped = without_comments(&source);

        for (line, segment) in use_statements(&stripped) {
            assert!(
                CORE_ALLOWED_CRATES.contains(&segment.as_str()),
                "{}:{line} imports `{segment}`, which AGENTS.md 3.2 does not allow in \
                 core/. Allowed: {CORE_ALLOWED_CRATES:?}",
                file.display()
            );
        }

        for token in CORE_FORBIDDEN_TOKENS {
            assert!(
                !stripped.contains(token),
                "{} mentions `{token}` after comment stripping. AGENTS.md 3.2 and \
                 PLAN.md section 4 keep core/ pure: no gpui, no tokio, no \
                 serialization, no filesystem, no sockets.",
                file.display()
            );
        }

        assert!(
            forbidden_serde_mention(&stripped).is_none(),
            "{} names `serde` outside the permitted `serde_json`. PLAN.md section 5 \
             gives serialization to sh_nexus_wire and keeps the domain unaware of \
             it; the derive macros are one `use` away from core/models, which is \
             what `core_models_derives_no_serde_traits` also asserts.",
            file.display()
        );
    }
}

/// `core/models` derives no `Serialize` or `Deserialize`.
///
/// This is the boundary `PLAN.md` §5 exists to create, asserted directly. The
/// allow-list above already rejects `use serde::...`, so a derive would need a
/// fully qualified `#[derive(serde::Serialize)]` to get past it -- which is
/// exactly the case this test exists to catch.
///
/// The consequence of a violation is not a style problem. A domain type that can
/// be both the model and the wire format cannot be changed in one without
/// changing the other, which is how a protocol and an application model drift
/// apart while every test stays green.
#[test]
fn core_models_derives_no_serde_traits() {
    let models_dir = src_dir().join("core").join("models");
    let files = rust_files_under(&models_dir);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        models_dir.display()
    );

    for file in &files {
        let source = fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display()));
        let stripped = without_comments(&source);

        for trait_name in ["Serialize", "Deserialize"] {
            assert!(
                !stripped.contains(trait_name),
                "{} mentions `{trait_name}`. Domain types must not be \
                 serializable: PLAN.md section 5 gives serialization to \
                 sh_nexus_wire and keeps the domain unaware of it.",
                file.display()
            );
        }
    }
}

/// `core/` reaches only its own modules, and never the boundary or the network.
///
/// The allow-list check permits `crate::` and `super::` because `core/` must be
/// able to refer to its own submodules. This is the test that says *which* ones,
/// and it is the one that would catch the mistake that actually happens: a
/// `core/` module reaching sideways into `crate::network` or `crate::errors`,
/// which is pure, compiles, passes the allow-list, and quietly turns the domain
/// into something that knows about the protocol.
#[test]
fn core_reaches_only_its_own_modules() {
    let core_dir = src_dir().join("core");
    for file in rust_files_under(&core_dir) {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display()));
        let stripped = without_comments(&source);

        for (line, statement) in stripped.lines().enumerate().filter(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with("use crate::") || trimmed.starts_with("pub use crate::")
        }) {
            for forbidden in [
                "crate::network",
                "crate::errors",
                "crate::ui",
                "crate::state",
            ] {
                assert!(
                    !statement.contains(forbidden),
                    "{}:{} reaches `{forbidden}`. AGENTS.md 3.2 makes core/ the \
                     innermost layer: it must not depend on the network, on the \
                     error type, or on anything above it.",
                    file.display(),
                    line + 1
                );
            }
        }
    }
}

/// Sources of non-determinism `core/` may not reach for.
///
/// `core/models/mod.rs` states the rule before it states the reason: *"Nothing
/// here performs I/O, reads a clock, or spawns a thread."* Nothing enforced
/// any of the three, because until work unit 1B `core/` held type definitions
/// and a type definition cannot read a clock.
///
/// Now it can, so the rule is a test. The two that matter most are the clock
/// and the thread, and they matter for the same reason a socket would: both make
/// a result depend on something other than the arguments, which is exactly what
/// makes `core/ordering.rs`'s permutation property untestable and its failures
/// irreproducible. A `Utc::now()` inside the order key would make every run of
/// the same batch produce a different order, and a test asserting equality would
/// fail on a Tuesday and pass on a Wednesday.
///
/// `std::env` is here for the same family of reasons: a value read from the
/// environment is not a function of the arguments either. `std::net` and
/// `std::process` are already covered by `CORE_FORBIDDEN_TOKENS` above and are
/// deliberately not repeated.
#[test]
fn core_names_no_clock_and_no_thread() {
    let core_dir = src_dir().join("core");
    let files = rust_files_under(&core_dir);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        core_dir.display()
    );

    for file in &files {
        let stripped = without_comments(
            &fs::read_to_string(file)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display())),
        );

        for token in [
            "Utc::now",
            "Local::now",
            "SystemTime::now",
            "Instant::now",
            "std::thread",
            "std::env",
            "rayon",
            "rand::",
        ] {
            assert!(
                !stripped.contains(token),
                "{} names `{token}`. core/ is pure by AGENTS.md 3.2 and \
                 PLAN.md section 4, and a clock, a thread or the environment \
                 makes a result depend on something other than the arguments -- \
                 which is what would make core/ordering.rs's permutation \
                 property untestable and its failures irreproducible.",
                file.display()
            );
        }
    }
}

/// `core/` contains no construct that can panic.
///
/// `AGENTS.md` §7.1 bans `unsafe` without a `// SAFETY:` comment and §2.1 bans
/// `unwrap`/`expect` in production paths; `core/ordering.rs` is the first module
/// in this layer with executable code, so it is the first place either could be
/// violated without a compiler objecting. Until now the rule was true by
/// accident — there was nothing to panic in.
///
/// The token list is a **deny-list**, deliberately, unlike the allow-list
/// `core_names_no_forbidden_dependency` uses. An allow-list of permitted
/// constructs is not expressible: the point is not which functions `core/` may
/// call, it is that none of these specific ways of giving up may appear.
///
/// **Comments are stripped first, which is load-bearing here and is the whole
/// reason `expect(` does not trip on `core/ordering.rs`'s doctests.** Those
/// doctests use `expect`, correctly — `AGENTS.md` §2.1 permits it in tests, and
/// a doctest is a test.
///
/// **Known limit, recorded rather than left for the next reader to trip over:**
/// this scans source text, so a future unit that puts `#[cfg(test)] mod tests`
/// *inside* `core/` will fail this test on its own test helpers. That is the
/// right failure to get wrong: fixing it means teaching the scanner to skip
/// `#[cfg(test)]` blocks, which means teaching it to parse Rust, and a
/// hand-written parser that is subtly wrong is worse than a test that has to be
/// extended. Every test in this project so far lives in `tests/`, so the case
/// has not arisen.
#[test]
fn core_contains_no_panicking_construct() {
    let core_dir = src_dir().join("core");
    let files = rust_files_under(&core_dir);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        core_dir.display()
    );

    for file in &files {
        let stripped = without_comments(
            &fs::read_to_string(file)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display())),
        );

        for token in [
            "unwrap(",
            "expect(",
            "panic!(",
            "todo!(",
            "unimplemented!(",
            "unreachable!(",
            "unsafe ",
        ] {
            assert!(
                !stripped.contains(token),
                "{} contains `{token}` after comment stripping. AGENTS.md 2.1 \
                 forbids unwrap/expect in production paths and 7.1 forbids \
                 unsafe without a SAFETY comment; a pure domain module has no \
                 error condition that justifies either, and a panic in core/ \
                 is a crash on a path with nothing to recover it.",
                file.display()
            );
        }
    }
}

/// Paths inside this crate that `ui/` is allowed to name.
///
/// `docs/ARCHITECTURE.md` ADR-006's step 1 states the rule as *"`ui/` may import
/// `gpui` and `state::bridge`, and **may not** import `network/`, `db/` or
/// `core::cache` directly"*. This is that rule written as an **allow-list**
/// rather than as a deny-list of those three, and the reason is the same one
/// `CORE_ALLOWED_CRATES` gives: a deny-list has to be extended in the same
/// commit as the new import or it silently permits it, while an allow-list
/// rejects a layer nobody has thought about yet — which is exactly the shape of
/// the mistake a Phase 3 author makes at 2am with `db::repository` open.
///
/// The three entries beyond the ADR's sentence are each a deliberate call:
///
/// - **`crate::core::{markdown, models, theme}`.** The ADR's rule names what
///   `ui/` may not reach; these are the pure `core/` modules it renders. They
///   hold no I/O and no handle on anything, so admitting them costs the
///   boundary nothing — while `crate::core::cache` stays out, because the
///   segment cache is reached through the bridge and going around the seam is
///   what would make its recency order a statement about a thread nobody
///   named.
/// - **`crate::state::DeliveryState`.** `PLAN.md` §5 puts delivery state in
///   `state/` *and* has the UI display it, so a row that says "sending…" has to
///   name it. It is client state, not a domain type; the same argument does not
///   extend to `state::app_state` or `state::actions`, and neither is on the
///   list. `tests/bridge.rs` independently fails the build on `ui/` naming the
///   state type itself.
const UI_ALLOWED_CRATE_PATHS: [&str; 6] = [
    "crate::core::markdown",
    "crate::core::models",
    "crate::core::theme",
    "crate::state::bridge",
    "crate::state::DeliveryState",
    "crate::ui",
];

/// Paths that must never appear in `ui/`, even fully qualified.
///
/// The allow-list above governs `use` statements; this catches a path written
/// inline, which no import list would show. One entry per layer the ADR names,
/// plus the two `state/` modules whose mutators `PLAN.md` §4 keeps behind the
/// named doors in `bridge.rs`.
const UI_FORBIDDEN_TOKENS: [&str; 5] = [
    "crate::network",
    "crate::db",
    "crate::core::cache",
    "crate::state::app_state",
    "crate::state::actions",
];

/// Whether `ui/` may name an intra-crate path.
fn ui_may_name(path: &str) -> bool {
    UI_ALLOWED_CRATE_PATHS
        .iter()
        .any(|allowed| path.starts_with(allowed))
}

/// Every `use` statement that reaches into this crate, as (line, path).
///
/// Simpler than [`use_statements`], which keeps only the first path segment: the
/// question here is which *subtree* a file reaches, so the whole path is what
/// has to come back.
fn internal_use_paths(stripped: &str) -> Vec<(usize, String)> {
    stripped
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            let rest = trimmed
                .strip_prefix("pub use ")
                .or_else(|| trimmed.strip_prefix("use "))?;
            rest.starts_with("crate::").then(|| {
                (
                    index + 1,
                    rest.split(';').next().unwrap_or_default().trim().to_owned(),
                )
            })
        })
        .collect()
}

/// `ui/` reaches the layers below the seam only through the seam.
///
/// **This lands before there is anything to violate, which is ADR-006's step 1
/// and the same order work unit 1E-1 used.** A boundary written after the code
/// is a boundary retrofitted around whatever the code already does, and the
/// retrofit is where the exception gets made.
///
/// Checked three ways, because the three fail differently: an **allow-list** over
/// intra-crate `use` statements (exact, and rejects a layer nobody thought
/// about), a **token scan** over comment-stripped source (catches a fully
/// qualified path a `use` list would not show), and two **positive** assertions
/// that `ui/` names `gpui` and the bridge at all — without which the rule could
/// be satisfied by a directory that renders nothing.
#[test]
fn ui_reaches_gpui_and_the_bridge_and_nothing_below_them() {
    let ui_dir = src_dir().join("ui");
    let files = rust_files_under(&ui_dir);
    assert!(
        !files.is_empty(),
        "{} should hold the presentation layer: PLAN.md section 4 puts ui/ in \
         AGENTS.md section 3.1's tree",
        ui_dir.display()
    );

    let mut names_gpui = false;
    let mut names_the_bridge = false;

    for file in &files {
        let source = fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display()));
        let stripped = without_comments(&source);

        names_gpui |= use_statements(&stripped)
            .iter()
            .any(|(_, segment)| segment == "gpui");

        for (line, path) in internal_use_paths(&stripped) {
            names_the_bridge |= path.starts_with("crate::state::bridge");
            assert!(
                ui_may_name(&path),
                "{}:{line} reaches `{path}`. AGENTS.md 3.2 and PLAN.md section 4 \
                 give ui/ the seam and the pure core modules and nothing else: \
                 network/ and db/ are reached through state/bridge.rs or not at \
                 all, the rendered-segment cache is reached through the bridge \
                 (which is what keeps its recency order a statement about the \
                 main thread), and the state's mutators live behind the named \
                 doors. Allowed: {UI_ALLOWED_CRATE_PATHS:?}",
                file.display()
            );
        }

        for token in UI_FORBIDDEN_TOKENS {
            assert!(
                !stripped.contains(token),
                "{} names `{token}` after comment stripping. ui/ renders what the \
                 state holds; it does not open a socket, read a database, or \
                 reach past the seam into the cache or the mutators.",
                file.display()
            );
        }
    }

    assert!(
        names_gpui,
        "no file under {} names gpui. PLAN.md section 4 confines GPUI to ui/ and \
         app.rs, so a ui/ that does not name it is not the presentation layer.",
        ui_dir.display()
    );
    assert!(
        names_the_bridge,
        "no file under {} names crate::state::bridge. PLAN.md section 4 makes \
         bridge.rs the single seam, and a view that reaches the state some other \
         way is the second seam that file exists to prevent.",
        ui_dir.display()
    );
}

/// The `ui/` rule is not vacuous: it rejects what it claims to and admits what
/// it must.
///
/// Same reasoning as [`core_admits_serde_json_and_still_rejects_serde`]: a
/// green tree proves only that nothing *currently* violates the rule, and says
/// nothing about whether the rule can still catch a violation. The rejected
/// samples are the imports a Phase 2/3 author would actually write; the admitted
/// ones are the imports this layer legitimately has, so a tightening that broke
/// one of them fails here rather than in a build nobody expected.
#[test]
fn the_ui_boundary_rejects_the_layers_below_it() {
    for violation in [
        "use crate::network::mapping::to_domain;",
        "use crate::network::websocket::Socket;",
        "use crate::db::repository::insert_message;",
        "use crate::core::cache::LruCache;",
        "use crate::state::app_state::AppState;",
        "use crate::state::actions::begin_send;",
        "pub use crate::network::rest::Client;",
    ] {
        let paths = internal_use_paths(without_comments(violation).as_str());
        assert_eq!(
            paths.len(),
            1,
            "`{violation}` should be one intra-crate path"
        );
        assert!(
            !ui_may_name(&paths[0].1),
            "`{violation}` names a layer ui/ must not reach, but the allow-list \
             admitted `{}`",
            paths[0].1
        );
    }

    for permitted in [
        "use crate::state::bridge::{self, Rendered};",
        "use crate::state::DeliveryState;",
        "use crate::core::markdown::Document;",
        "use crate::core::models::message::Message;",
        "use crate::core::theme::Palette;",
        "use crate::ui::views::message_list::MessageList;",
    ] {
        let paths = internal_use_paths(without_comments(permitted).as_str());
        assert_eq!(
            paths.len(),
            1,
            "`{permitted}` should be one intra-crate path"
        );
        assert!(
            ui_may_name(&paths[0].1),
            "`{permitted}` is a legitimate ui/ import and must be admitted, but \
             `{}` was rejected",
            paths[0].1
        );
    }

    // And a path written inline rather than imported is caught by the token
    // scan, which is the half an import allow-list cannot reach.
    assert!(
        UI_FORBIDDEN_TOKENS
            .iter()
            .any(|token| "let c = crate::core::cache::LruCache::new(1);".contains(token)),
        "a fully qualified path must still trip the token scan"
    );
}

/// `core/cache.rs` contains no interior mutability.
///
/// `AGENTS.md` 4.2's cache row ends with "thread safety", and that clause has
/// two halves. Work unit 1C-2a owns the **type-level** half -- `LruCache` is
/// `Send + Sync` whenever its parameters are, and that is proved at compile time
/// by `the_cache_is_send_and_sync` in `tests/cache.rs`. This test is what
/// proves it structurally, rather than by whoever happens to remember.
///
/// **Work unit 1C-2b settled the other half, and this test is now what enforces
/// its decision rather than merely documenting it.** The decision is that the
/// cache has **no internal synchronisation**: `PLAN.md` section 4 makes
/// `state/bridge.rs` the sole owner of `cx.update_global` / `cx.update`, so every
/// network callback reaches application state through the main thread, and a
/// cache reached only from there is main-thread-owned and needs no lock. A caller
/// that ever needs cross-thread access wraps the cache in its own `Mutex`
/// *outside* `core/`, which the type-level half is what makes possible.
///
/// **So the two halves are now load-bearing for each other, and this is the
/// mechanical guard on that.** A `Mutex` added "to make some caller convenient"
/// would not merely be a style objection: it would silently revoke the property
/// that makes external wrapping work, put a lock on every frame-path read of the
/// cache against `AGENTS.md` 2.3, and add a poisoned-lock policy to a module that
/// `AGENTS.md` 2.1 forbids from having one. `the_bounded_cache_is_send_and_sync`
/// in `tests/cache_ceiling.rs` is the compile-time half; this is the structural
/// half, and between them the decision cannot be revoked by accident.
///
/// **Scoped to this one file, unlike the tests above, and the reason is
/// specific rather than lazy.** Interior mutability is not wrong in `core/` in
/// general; a pure value type may legitimately want a `Cell` for a cache of its
/// own. What is forbidden *here* is a lock or a cell in a structure whose
/// documented contract is "no interior mutability, and therefore `Send + Sync`
/// whenever the parameters are" -- a contract the documented thread-safety
/// decision now rests on.
///
/// Comments are stripped first, so this file's own module documentation may
/// discuss `Mutex` and `RwLock` at length, which `core/cache.rs` section 13 does.
#[test]
fn core_cache_contains_no_interior_mutability() {
    let cache = src_dir().join("core").join("cache.rs");
    assert!(
        cache.is_file(),
        "{} should exist: AGENTS.md 3.1 places the LRU cache at src/core/cache.rs",
        cache.display()
    );

    let stripped = without_comments(
        &fs::read_to_string(&cache)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", cache.display())),
    );

    for token in [
        "Mutex",
        "RwLock",
        "RefCell",
        "Cell<",
        "UnsafeCell",
        "OnceCell",
        "LazyLock",
    ] {
        assert!(
            !stripped.contains(token),
            "core/cache.rs contains `{token}`. The LRU cache is documented as a \
             plain struct with no interior mutability, which is what makes it \
             Send + Sync whenever its parameters are. Work unit 1C-2b decided \
             (core/cache.rs section 13) that it needs no internal \
             synchronisation: PLAN.md section 4 makes state/bridge.rs the sole \
             owner of cx.update_global, so the cache is main-thread-owned. A \
             lock here would revoke the type-level property that lets a caller \
             wrap one for cross-thread use, would put a lock on every frame-path \
             read, and would add a poisoned-lock policy that AGENTS.md 2.1 \
             forbids. If a worker thread really must reach a cache, wrap it in \
             the layer that owns the thread -- and say so in section 13."
        );
    }
}

/// `network/` names no `gpui`.///
/// §3.2's rule for `network/` is narrower than for `core/` -- `network/` *will*
/// import `tokio` in Phase 4 -- but the GPUI prohibition is absolute and is the
/// one that resolves the §3.2-versus-§7.3 tension. `PLAN.md` §4 puts the seam in
/// `state/bridge.rs`, and this test is what stops `network/` from quietly
/// becoming a second seam.
#[test]
fn network_names_no_gpui() {
    let network_dir = src_dir().join("network");
    let files = rust_files_under(&network_dir);
    assert!(
        !files.is_empty(),
        "no Rust files found under {}",
        network_dir.display()
    );

    for file in &files {
        let source = fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", file.display()));
        let stripped = without_comments(&source);

        for token in ["gpui", "cx.update_global", "cx.update", "Window", "Entity"] {
            assert!(
                !stripped.contains(token),
                "{} mentions `{token}`. AGENTS.md 3.2: network/ never touches GPUI \
                 state directly. PLAN.md section 4 names state/bridge.rs as the only \
                 module allowed to call cx.update_global.",
                file.display()
            );
        }
    }
}

/// The `serde` split holds in both directions, and this is the test for it.
///
/// The task it does and the reason it is not satisfied by
/// `core_names_no_forbidden_dependency` passing: a green tree proves only that no
/// file *currently* violates the rule. It says nothing about whether the rule can
/// still *catch* a violation, and the previous single-token check had a failure
/// mode a green tree could not reveal -- `"serde_json".contains("serde")` meant
/// that admitting the parser was impossible, and the only way to "fix" that was
/// to delete the token, which would have taken the derive ban with it.
///
/// So both halves are demonstrated on synthetic sources, plus one check against
/// the real tree:
///
/// 1. every spelling of the **derive** crate is rejected, including the
///    `serde_derive` proc-macro crate the old substring entry rejected only
///    incidentally;
/// 2. both spellings of the **parser** -- a `use` and a fully qualified path --
///    are accepted, and the `use` allow-list admits it too;
/// 3. prose *about* `serde` is stripped and does not trip the check, so this
///    file's own documentation can discuss the boundary;
/// 4. `core/theme.rs` really does name `serde_json`, so the allow-list entry
///    cannot be quietly reverted while the code still needs it.
#[test]
fn core_admits_serde_json_and_still_rejects_serde() {
    // 1. The derive crate, in every spelling the boundary has to catch. The
    //    `#[derive(..)]` line is the one `core_models_derives_no_serde_traits`
    //    exists for; a `use` cannot reach it, so the fully qualified form is
    //    the only way in and this is the only check that sees it in `core/`.
    for violation in [
        "use serde;",
        "use serde::Serialize;",
        "use serde::{Deserialize, Serialize};",
        "#[derive(serde::Serialize)]",
        "#[derive(serde::Deserialize, Clone)]",
        "use serde_derive::Serialize;",
        "let value = serde::to_string(&theme);",
        "extern crate serde;",
    ] {
        let found = forbidden_serde_mention(without_comments(violation).as_str());
        assert!(
            found.is_some(),
            "`{violation}` names the serde crate and must be rejected, but the \
             check found nothing"
        );
    }

    // The reported text names what was found, so a failure says which crate.
    assert_eq!(
        forbidden_serde_mention("use serde::Serialize;").as_deref(),
        Some("::Serialize;"),
        "the rejection should name what followed `serde`"
    );

    // 2. The parser, in both spellings, plus the allow-list that governs imports.
    for permitted in [
        "use serde_json::Value;",
        "let root: serde_json::Value = serde_json::from_slice(bytes).ok().unwrap_or(serde_json::Value::Null);",
        "use serde_json::{Map, Value};",
    ] {
        assert!(
            forbidden_serde_mention(permitted).is_none(),
            "`{permitted}` names only the serde_json parser and must be admitted, \
             but the check rejected it"
        );
    }
    assert!(
        CORE_ALLOWED_CRATES.contains(&"serde_json"),
        "serde_json must be on the import allow-list: {CORE_ALLOWED_CRATES:?}"
    );
    assert!(
        !CORE_ALLOWED_CRATES.contains(&"serde"),
        "serde must NOT be on the import allow-list: {CORE_ALLOWED_CRATES:?}"
    );

    // 3. Prose about the boundary does not trip the boundary. Without this, the
    //    paragraph above and `core/theme.rs`'s own section 2 would both fail the
    //    scan, and the fix would be to stop documenting the rule.
    let prose = concat!(
        "//! This module must not name serde, and neither must serde_derive.\n",
        "/// The derive macros come from `serde`; the parser is `serde_json`.\n",
        "use std::fmt;\n",
    );
    let stripped = without_comments(prose);
    assert!(
        !stripped.contains("serde"),
        "the comment stripper should have removed every mention, got {stripped:?}"
    );
    assert!(
        forbidden_serde_mention(&stripped).is_none(),
        "prose about serde must not be scanned as code"
    );

    // 4. The real tree. This is what makes the allow-list entry load-bearing: if
    //    `core/theme.rs` ever stops parsing JSON, this is the test that says so,
    //    and if the allow-list is reverted while it still parses, this one does.
    let theme = src_dir().join("core").join("theme.rs");
    assert!(
        theme.is_file(),
        "{} should exist: AGENTS.md 3.1 places theme parsing at src/core/theme.rs",
        theme.display()
    );
    let source = without_comments(
        &fs::read_to_string(&theme)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", theme.display())),
    );
    assert!(
        source.contains("serde_json"),
        "core/theme.rs is expected to name serde_json -- the whole reason 1D split \
         the serde token. If the parser moved out of core/, delete the allow-list \
         entry in the same commit and record why."
    );
    assert!(
        forbidden_serde_mention(&source).is_none(),
        "core/theme.rs must not name the serde derive crate, and does not."
    );
}

/// `errors.rs` sits outside `core/`, which is why it may name `ThemeError`.
///
/// `AGENTS.md` §3.3 makes `ShNexusError` the global error type, and
/// `Serialization(#[from] serde_json::Error)` is part of it. That is only
/// coherent because `errors.rs` is a sibling of `core/`, not a member of it --
/// an error enum carrying an I/O payload is not domain vocabulary, and §3.3's
/// "each module may define narrower error types that convert into
/// `ShNexusError`" is the sanctioned direction of travel.
///
/// **Work unit 1D is what made that sentence load-bearing rather than
/// descriptive.** `core/theme.rs` defines its own `ThemeError` and
/// `errors.rs` carries `impl From<ThemeError> for ShNexusError`, which is §3.3's
/// direction of travel used for real: the narrow error is defined by the domain,
/// the conversion is written by the layer that owns the vocabulary. The
/// alternative -- `core/theme.rs` returning `ShNexusError` -- is exactly what
/// `core_reaches_only_its_own_modules` below forbids, and the two would now
/// contradict each other.
///
/// Without this test, moving `errors.rs` into `core/` would compile and pass
/// `core_names_no_forbidden_dependency` (which scans `core/`, and would then
/// catch `errors.rs`'s `use crate::core::theme::ThemeError` as reaching sideways)
/// -- but the reverse mistake is not caught: a `core/` that *defines* its own
/// error type and nothing else is fine, and that is the sanctioned shape. The
/// assertion worth having is the structural one.
#[test]
fn errors_is_a_sibling_of_core_not_a_member_of_it() {
    let errors = src_dir().join("errors.rs");
    assert!(
        errors.is_file(),
        "{} should exist: AGENTS.md 3.3 places ShNexusError at src/errors.rs",
        errors.display()
    );

    let models = src_dir().join("core").join("models").join("mod.rs");
    let source = fs::read_to_string(&models)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", models.display()));
    assert!(
        !without_comments(&source).contains("errors"),
        "core/models must not refer to the error module. AGENTS.md 3.3's direction \
         of travel is module error -> ShNexusError, never the reverse."
    );
}

/// The client's dependency direction: it depends on the protocol, and the
/// protocol does not depend on the client.
///
/// The mirror image of
/// `crates/sh_nexus_wire/tests/dependency_direction.rs`, kept here as well
/// because the failure this prevents -- the client quietly growing its own DTOs
/// -- is a client-side regression, and a test the client owns is a test somebody
/// will think to run.
#[test]
fn the_client_depends_on_the_wire_crate() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", manifest.display()));
    assert!(
        text.contains("sh_nexus_wire"),
        "the client must depend on sh_nexus_wire (ADR-002). Not found in {}",
        manifest.display()
    );
}

/// The client does **not** depend on `serde`.
///
/// Not an oversight -- a boundary. Nothing in the client derives
/// `Serialize`/`Deserialize`; that is `sh_nexus_wire`'s job (`PLAN.md` §5). The
/// client needs `serde_json` only because `errors.rs` wraps a
/// `serde_json::Error` and because the boundary tests decode JSON.
///
/// Pinning the absence matters because the derive macros would be one `use`
/// away from `core/models`, and the allow-list in
/// `core_names_no_forbidden_dependency` would then be protecting a door that is
/// already open.
#[test]
fn the_client_does_not_depend_on_serde() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", manifest.display()));

    // Strip comments so the manifest's own explanation of *why* serde is absent
    // is not mistaken for the dependency.
    let stripped: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    // Only `[dependencies]` is checked, and not `[dev-dependencies]`: a
    // dev-dependency on `serde` would put the derive macros within reach of a
    // test, not of `core/`, and `core_models_derives_no_serde_traits` is the
    // check that matters for the boundary. Checking dev-dependencies too would
    // forbid the wire crate's own test helpers from using serde.
    let dependencies = stripped
        .split("[dependencies]")
        .nth(1)
        .unwrap_or_default()
        .split('[')
        .next()
        .unwrap_or_default();
    assert!(
        !dependencies.contains("serde ="),
        "the client should not depend on `serde`: serialization belongs to \
         sh_nexus_wire (PLAN.md section 5). Found it in [dependencies] of {}",
        manifest.display()
    );
}

/// The comment stripper works, and fails **safe**.
///
/// A test that scans source with a hand-written scanner is only as trustworthy as
/// the scanner, and a broken scanner fails **open** -- it would report no
/// violations and every boundary test would pass for the wrong reason. This is
/// the same class of bug as a test using only `.id()` instead of
/// `.debug_selector()` in the Phase 0 spike: a green test asserting nothing.
///
/// "Fails safe" is the load-bearing word. The scanner must remove prose so that
/// documentation *about* a forbidden dependency does not trip the check, and it
/// must never remove code -- a stripper that deleted a whole line because it
/// contained `//` would hide a real violation. So a line with code and a
/// trailing comment keeps its code, and the forbidden crate on it is still
/// detected.
#[test]
fn the_comment_stripper_removes_prose_and_never_code() {
    let source = concat!(
        "//! module doc\n",
        "/// item doc\n",
        "use serde::Serialize; // a trailing comment\n",
        "/* block\n   spanning lines */\n",
        "use gpui::App;\n",
    );
    let stripped = without_comments(source);

    // Prose goes: module docs, item docs, and block comment bodies.
    assert!(
        !stripped.contains("module doc"),
        "a `//!` module doc should go"
    );
    assert!(!stripped.contains("item doc"), "a `///` item doc should go");
    assert!(
        !stripped.contains("block"),
        "a block comment's contents should go"
    );
    assert!(
        !stripped.contains("a trailing comment"),
        "a `//` comment's contents should go"
    );

    // Code stays. Both `use` lines survive, and a forbidden crate on a line that
    // happens to carry a comment is still found.
    assert!(
        stripped.contains("use serde::Serialize;"),
        "code before a `//` must survive, got {stripped:?}"
    );
    assert!(
        stripped.contains("use gpui::App;"),
        "code after a block comment must survive, got {stripped:?}"
    );

    // And the scanner the boundary tests rely on sees both, not one.
    let segments: Vec<String> = use_statements(&stripped)
        .into_iter()
        .map(|(_, segment)| segment)
        .collect();
    assert_eq!(
        segments,
        vec!["serde".to_owned(), "gpui".to_owned()],
        "a trailing comment must not hide the import it shares a line with"
    );
}

/// The two newer scanners see a violation that shares a line with a comment.
///
/// Same reasoning as
/// [`the_comment_stripper_removes_prose_and_never_code`], applied to the token
/// lists behind `core_names_no_clock_and_no_thread` and
/// `core_contains_no_panicking_construct`. Both of those pass today, and a
/// scanner that fails **open** would keep them passing forever while checking
/// nothing — which is the class of bug this file already has one test for, and
/// the reason that test exists.
///
/// The synthetic source puts each violation on a line that also carries a
/// trailing comment and, in one case, inside a doc comment, so this asserts the
/// two halves at once: prose goes, code stays, and the token is found.
#[test]
fn the_violation_scanners_see_code_and_not_prose() {
    let source = concat!(
        "//! A module doc mentioning Utc::now in prose.\n",
        "/// An item doc mentioning panic! in prose.\n",
        "let now = Utc::now(); // a trailing comment about unwrap(\n",
        "/* a block comment naming expect( */\n",
        "let value = 1; let risky = value.expect(\"boom\");\n",
        "let raw = 1; let also_risky = raw.unwrap();\n",
    );
    let stripped = without_comments(source);

    assert!(
        !stripped.contains("in prose"),
        "prose about a forbidden construct must be stripped, or every doc \
         comment about the boundary would trip the boundary"
    );
    for token in ["Utc::now", "expect(", "unwrap("] {
        assert!(
            stripped.contains(token),
            "the violation `{token}` should survive comment stripping in \
             `{}`",
            stripped.trim()
        );
    }
}
