//! Build-parity capture instrument (L0.6). Not a gate on its own: it writes the
//! sharded store of a fixed set of repos so two builds of the engine (before and
//! after a refactor) can be compared with `diff -r`. It uses only the engine's
//! public API, so the same file compiles against both sides of a move.
//!
//! ```text
//! GLIA_PARITY_REPOS=<dir>:<dir>:… GLIA_PARITY_OUT=<out> \
//!   cargo test -p glia-engine --test build_parity -- --ignored --nocapture 2> <out>.stderr
//! ```
//!
//! For each repo `i` (0-based, in argument order) it writes
//! `generate_one(repo)` into `<out>/<i>`, then one `generate_many` over every
//! repo into `<out>/all`. `manifest.json` records the build stamp, which moves
//! whenever engine sources change, so compare with `diff -r -x manifest.json`.
//! Engine marker lines go to stderr; the `[parity]` summary lines below go there
//! too, so the captured stderr pins node / edge / parse-error counts as well.
//!
//! Reused by LD.13, LD.14a, LG.1a and LG.1b instead of a second parity test.

use std::path::{Path, PathBuf};

use glia_engine::{GenerateResult, generate_many, generate_one};
use glia_store::write_merged_sharded;

fn report(tag: &str, r: &GenerateResult) {
    eprintln!(
        "[parity] {tag} nodes={} edges={} cross_edges={} graphs={} parse_errors={}",
        r.total_nodes,
        r.total_edges,
        r.merged.cross_edges.len(),
        r.merged.graphs.len(),
        r.parse_errors.len()
    );
    for e in &r.parse_errors {
        eprintln!("[parity] {tag} parse_error: {e}");
    }
}

fn capture(tag: &str, r: &GenerateResult, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    write_merged_sharded(&r.merged, dir)
        .unwrap_or_else(|e| panic!("write {tag} into {}: {e:?}", dir.display()));
    report(tag, r);
}

#[test]
#[ignore = "capture instrument: needs GLIA_PARITY_REPOS and GLIA_PARITY_OUT"]
fn capture_merged_shards() {
    let repos: Vec<String> = std::env::var("GLIA_PARITY_REPOS")
        .expect("GLIA_PARITY_REPOS: colon-separated repo dirs")
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    assert!(!repos.is_empty(), "GLIA_PARITY_REPOS names no repo");
    let out = PathBuf::from(
        std::env::var("GLIA_PARITY_OUT").expect("GLIA_PARITY_OUT: output directory"),
    );

    for (i, repo) in repos.iter().enumerate() {
        let tag = format!("one[{i}]");
        let r = generate_one(repo).unwrap_or_else(|e| panic!("generate_one {repo}: {e}"));
        capture(&tag, &r, &out.join(i.to_string()));
    }
    let all = generate_many(&repos).unwrap_or_else(|e| panic!("generate_many: {e}"));
    capture("all", &all, &out.join("all"));
}
