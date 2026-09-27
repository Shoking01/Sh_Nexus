//! The dependency direction is enforced, not merely documented.
//!
//! ADR-002's whole argument is that the protocol is "defined once and cannot
//! drift" because the client and the server are the same compiler invocation
//! over the same types. That argument has one load-bearing assumption, and it is
//! easy to break without noticing: **the protocol crate must not depend on
//! either of its two implementations.**
//!
//! ```text
//!     sh_nexus  ─────┐
//!                    ├──> sh_nexus_wire
//!     sh_nexus_server ┘
//! ```
//!
//! Add `sh_nexus` to this crate's dependencies and the diagram becomes a cycle.
//! It would still compile -- Cargo permits dev-dependency cycles, and even
//! ordinary ones under some feature arrangements -- and the damage would be
//! invisible until someone noticed that the protocol's types had started
//! carrying client concepts.
//!
//! So it is a test. It reads `Cargo.toml` and fails on a back-edge. A reviewer
//! has to notice a missing line; a test does not.
//!
//! # Why this reads a file
//!
//! Rust has no compile-time way to assert "this crate does not depend on X" --
//! an absent dependency is not a thing the type system can be told about. The
//! next best thing is a test that reads the manifest. `AGENTS.md` §2.1's "no
//! `String` for paths" applies, so the path is a [`PathBuf`] built from
//! `CARGO_MANIFEST_DIR`.
//!
//! This is I/O in a *test*, which `AGENTS.md` §3.2 permits: the prohibition is
//! on `core/` touching the filesystem in production, and this file is not
//! `core/`.

use std::fs;
use std::path::{Path, PathBuf};

/// The project crates that this crate must never depend on.
///
/// `sh_nexus_server` does not exist yet (Phase 4). It is listed anyway, because
/// the moment it is created someone will consider depending on it -- for a
/// shared JWT helper, say -- and this test should be the thing that says no.
const FORBIDDEN_BACK_EDGES: [&str; 2] = ["sh_nexus", "sh_nexus_server"];

/// The platform and transport crates that must never appear here.
///
/// Not strictly a back-edge, but the same reasoning: a `tokio` or `reqwest`
/// dependency here would mean the protocol crate is no longer a pure
/// transformation over bytes, which is what makes its ≥80% coverage floor
/// (ADR-004) and its safety on both sides of a socket hold.
const FORBIDDEN_INFRASTRUCTURE: [&str; 7] = [
    "gpui",
    "gpui_platform",
    "tokio",
    "reqwest",
    "tokio-tungstenite",
    "rusqlite",
    "axum",
];

/// This crate's manifest.
fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

/// This crate's manifest, as text.
fn manifest() -> String {
    let path = manifest_path();
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()))
}

/// Every crate name declared as a dependency, in any dependency table.
///
/// Deliberately crude: a line-oriented scan for `name = ` or `name = {` on a
/// table other than `[package]`, `[lib]`, `[features]` or `[[bin]]`. It has to be
/// crude to stay correct when the manifest grows -- a precise TOML parse would
/// need a TOML dependency, and `AGENTS.md` §7.2's first criterion says to check
/// whether the standard library solves it first. It does not, and a hand-rolled
/// scan is the smaller cost, so the false-positive risk is managed instead: the
/// scan ignores known non-dependency tables, and a false positive would be a
/// test that names the exact table it found, which is a two-second fix.
fn declared_dependency_names(manifest: &str) -> Vec<String> {
    const IGNORED_TABLES: [&str; 4] = ["[package]", "[lib]", "[features]", "[[bin]]"];

    let mut names = Vec::new();
    let mut in_dependency_table = false;

    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependency_table = !IGNORED_TABLES.contains(&line)
                && !line.starts_with("[[") // [[bin]] and friends are targets, not deps
                && (line.starts_with("[dependencies")
                    || line.starts_with("[dev-dependencies")
                    || line.starts_with("[build-dependencies"));
            continue;
        }
        if !in_dependency_table || line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Either `name = "1.0"` or `name = { workspace = true }`.
        if let Some((name, _)) = line.split_once('=') {
            let name = name.trim().trim_matches('"');
            if !name.is_empty() {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// This crate declares no dependency on the client or the server.
///
/// The back-edge ADR-002 forbids. There is no legitimate reason for the protocol
/// to know which side is speaking it.
#[test]
fn the_wire_crate_does_not_depend_on_its_implementations() {
    let names = declared_dependency_names(&manifest());
    assert!(
        !names.is_empty(),
        "the scan found no dependencies at all, so it is not looking at the \
         right place -- a test that cannot fail is worse than no test"
    );

    for forbidden in FORBIDDEN_BACK_EDGES {
        assert!(
            !names.iter().any(|name| name == forbidden),
            "sh_nexus_wire must not depend on `{forbidden}`: the protocol cannot \
             depend on an implementation of itself (ADR-002). Found in {}",
            manifest_path().display()
        );
    }
}

/// This crate declares no transport, platform, or async runtime.
///
/// The reason is not purity for its own sake. It is that a `tokio` or `reqwest`
/// dependency would make every function here fallible for reasons that have
/// nothing to do with the protocol, would put the crate's own test suite behind
/// a runtime, and would mean the server linked the client's I/O stack.
#[test]
fn the_wire_crate_depends_on_no_transport_or_platform_crate() {
    let names = declared_dependency_names(&manifest());
    for forbidden in FORBIDDEN_INFRASTRUCTURE {
        assert!(
            !names.iter().any(|name| name == forbidden),
            "sh_nexus_wire must not depend on `{forbidden}`. Found in {}",
            manifest_path().display()
        );
    }
}

/// This crate's dependency set is exactly the four the crate root documents.
///
/// A fifth would be a decision that belongs in the root manifest's §7.2 audit,
/// not slipped in here. The list is spelled out rather than derived so that
/// adding a dependency breaks this test.
#[test]
fn the_wire_crate_dependency_set_is_the_audited_one() {
    let mut names = declared_dependency_names(&manifest());
    names.sort();
    assert_eq!(
        names,
        vec!["chrono", "proptest", "serde", "serde_json", "thiserror"],
        "sh_nexus_wire's dependency set changed. A new dependency needs an \
         AGENTS.md section 7.2 audit in the root Cargo.toml, and the crate root \
         docs updated if it changes what this crate is allowed to depend on."
    );
}

/// The client depends on this crate, which is the other half of the arrow.
///
/// Without this, `crates/sh_nexus` could quietly stop using the shared protocol
/// and define its own DTOs -- the exact drift ADR-002 exists to make
/// structurally impossible.
#[test]
fn the_client_depends_on_the_wire_crate() {
    let client_manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the wire crate lives under crates/")
        .join("sh_nexus")
        .join("Cargo.toml");
    let text = fs::read_to_string(&client_manifest).unwrap_or_else(|error| {
        panic!("{} should be readable: {error}", client_manifest.display())
    });

    assert!(
        declared_dependency_names(&text)
            .iter()
            .any(|name| name == "sh_nexus_wire"),
        "the client must depend on sh_nexus_wire (ADR-002). Not found in {}",
        client_manifest.display()
    );
}
