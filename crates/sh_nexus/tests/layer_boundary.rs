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
//! | `core/` names no I/O, platform, or serialization crate | §3.2, `PLAN.md` §4, §5 | `core_names_no_forbidden_dependency` |
//! | `core/models` derives no `Serialize`/`Deserialize` | `PLAN.md` §5 | `core_models_derives_no_serde_traits` |
//! | `core/` touches no filesystem | `PLAN.md` §4 | `core_names_no_forbidden_dependency` |
//! | `network/` names no `gpui` | §3.2 | `network_names_no_gpui` |
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
const CORE_ALLOWED_CRATES: [&str; 8] = [
    "std", "core", "alloc", "chrono", "smallvec", "uuid", "crate", "super",
];

/// Crates `core/` may never name.
const CORE_FORBIDDEN_TOKENS: [&str; 10] = [
    "serde",
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

/// `core/` names no I/O, platform, or serialization crate.
///
/// Checked twice over, because the two checks fail differently:
///
/// - a **`use` allow-list**, which is exact but only sees imports;
/// - a **token scan** over comment-stripped source, which catches a fully
///   qualified path (`chrono::Utc` inline, or `serde_json::from_str(..)` written
///   without a `use`) that no import list would show.
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

/// `network/` names no `gpui`.
///
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

/// `errors.rs` sits outside `core/`, which is why it may mention serialization.
///
/// `AGENTS.md` §3.3 makes `ShNexusError` the global error type, and
/// `Serialization(#[from] serde_json::Error)` is part of it. That is only
/// coherent because `errors.rs` is a sibling of `core/`, not a member of it --
/// an error enum carrying an I/O payload is not domain vocabulary, and §3.3's
/// "each module may define narrower error types that convert into
/// `ShNexusError`" is the sanctioned direction of travel.
///
/// Without this test, moving `errors.rs` into `core/` would compile, pass
/// `core_names_no_forbidden_dependency` (which scans `core/`, and would then
/// catch it) -- but the reverse mistake is not caught: a `use serde_json` in
/// `core/` is caught, while a `core/` that *defines* its own error type and
/// nothing else is fine. The assertion worth having is the structural one.
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
