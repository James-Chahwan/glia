//! Compute `GLIA_PARSER_STAMP`: a 64-bit content hash of every source file in
//! the workspace that can change what ends up in a `.gmap`.
//!
//! Why a content hash rather than a hand-bumped constant: the parse cache used
//! to key on the release version alone, so a parser fix merged without a
//! workspace version bump was invisible to every incremental consumer — the
//! cache kept serving pre-fix `FileParse`s while a cold `bench/substrate-gap`
//! run reported the cell fixed. A content hash needs no bump discipline, is
//! exact, and is reproducible across machines and checkouts.
//!
//! Over-invalidation is cheap on purpose: `.gmap` is write-once /
//! rebuild-whole-file, so a false invalidation costs exactly one full rebuild.

use std::collections::BTreeMap;
use std::env;
use std::path::{Component, Path, PathBuf};

/// Crates whose source can change node/edge/cell content or `FileParse` shape.
///
/// Deliberately NOT `py`, `cli` or `doc-sources`: they are transport and
/// orchestration wrappers that cannot change stored bytes, and including `cli`
/// would invalidate every user's parse cache on a CLI tweak. Deliberately NOT
/// `projection-text` or `activation`: both are query-time projections over an
/// already-built graph, not inputs to it.
const HASHED_ROOTS: &[&str] = &["core", "code-domain", "graph", "engine", "store", "parsers"];

/// Committed files outside any crate `src` that still change parse output.
/// `Cargo.lock` is the only thing that captures a tree-sitter GRAMMAR bump —
/// which changes parse results with zero `.rs` diff.
const HASHED_FILES: &[&str] = &["Cargo.lock"];

/// Floor below which the walk is assumed broken. A silently-empty hash would be
/// the exact hazard this crate exists to kill, so falling short fails the build.
const MIN_INPUTS: usize = 30;

fn die(msg: &str) -> ! {
    println!("cargo:warning=repo-graph-stamp: {msg}");
    println!("cargo:warning=repo-graph-stamp: refusing to emit a stamp that cannot invalidate caches");
    std::process::exit(1)
}

/// FNV-1a (64-bit), inlined so this crate needs no build-dependency.
fn fnv1a(state: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *state ^= u64::from(*b);
        *state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Workspace-root-relative path rendered with `/` separators, built from
/// `Path::components()` rather than `Display` so a Windows checkout produces the
/// same key (and therefore the same stamp) as a Unix one.
fn rel_key(ws_root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(ws_root).ok()?;
    let mut parts: Vec<&str> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Every `Cargo.toml`, plus every `.rs` living under a `src` component.
fn is_input(p: &Path) -> bool {
    if p.file_name().and_then(|s| s.to_str()) == Some("Cargo.toml") {
        return true;
    }
    if p.extension().and_then(|s| s.to_str()) != Some("rs") {
        return false;
    }
    p.components()
        .any(|c| matches!(c, Component::Normal(s) if s == "src"))
}

fn visit(dir: &Path, ws_root: &Path, files: &mut BTreeMap<String, PathBuf>, dirs: &mut Vec<PathBuf>) {
    dirs.push(dir.to_path_buf());
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // `read_dir` order is filesystem-dependent; the BTreeMap key sort is what
    // makes the final hash deterministic, and `dirs` only feeds rerun-if-changed.
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            // Never descend into build output.
            if entry.file_name() == "target" {
                continue;
            }
            visit(&path, ws_root, files, dirs);
        } else if ft.is_file()
            && is_input(&path)
            && let Some(key) = rel_key(ws_root, &path)
        {
            files.insert(key, path);
        }
    }
}

fn main() {
    // Resolve the workspace root from the manifest dir, NEVER from cwd: maturin
    // builds the wheel with `working-directory: py`.
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        die("CARGO_MANIFEST_DIR is not set")
    };
    let manifest_dir = PathBuf::from(manifest_dir);
    let Some(ws_root) = manifest_dir.parent() else {
        die("CARGO_MANIFEST_DIR has no parent — cannot locate the workspace root")
    };

    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in HASHED_ROOTS {
        let dir = ws_root.join(root);
        if !dir.is_dir() {
            die(&format!("hashed root {root:?} is missing under {}", ws_root.display()));
        }
        visit(&dir, ws_root, &mut files, &mut dirs);
    }
    for name in HASHED_FILES {
        let path = ws_root.join(name);
        if path.is_file() {
            if let Some(key) = rel_key(ws_root, &path) {
                files.insert(key, path);
            }
        } else {
            // Not fatal (an sdist may prune it), but never silent: a missing
            // Cargo.lock means a tree-sitter grammar bump stops moving the stamp.
            println!("cargo:warning=repo-graph-stamp: {name} not found — grammar bumps will not move the stamp");
        }
    }

    if files.len() < MIN_INPUTS {
        die(&format!(
            "only {} hashed inputs found under {} (expected >= {MIN_INPUTS})",
            files.len(),
            ws_root.display()
        ));
    }

    // BTreeMap iterates in key order, so the stamp is independent of readdir order.
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for (key, path) in &files {
        let Ok(mut bytes) = std::fs::read(path) else {
            die(&format!("could not read hashed input {}", path.display()))
        };
        // A CRLF checkout must hash identically to an LF one.
        bytes.retain(|b| *b != b'\r');
        fnv1a(&mut state, key.as_bytes());
        fnv1a(&mut state, &[0]);
        // Length framing: without it "ab" + "c" and "a" + "bc" collide.
        fnv1a(&mut state, &(bytes.len() as u64).to_le_bytes());
        fnv1a(&mut state, &bytes);
    }

    println!("cargo:rustc-env=GLIA_PARSER_STAMP={state:016x}");

    println!("cargo:rerun-if-changed=build.rs");
    for path in files.values() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // Directory mtimes too: that is what changes when a file is ADDED, and
    // without these a brand-new parser source would never retrigger this script.
    for dir in &dirs {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
}
