//! WP-D acceptance gate, the honest version (audit 2026-06-10 #10): the
//! "byte-identical" guarantee is checked against the actual bytes the store
//! writes, not a sorted node/edge fingerprint. The previous in-module test
//! sorted nodes and edges before comparing and ignored cell content entirely,
//! so it tolerated exactly the ordering nondeterminism (shard order,
//! cross-edge order, walk order) this suite now locks down.

use std::path::Path;

use repo_graph_engine::{generate_one, generate_one_with_cache, ParseCache};
use repo_graph_store::write_merged_sharded;

/// Multi-language fixture: two shards (python + go) plus markdown docs, so
/// shard naming/order, cross edges, and the docs graph are all exercised.
fn write_fixture(dir: &Path) {
    std::fs::write(
        dir.join("a.py"),
        "def foo():\n    \"\"\"Frobnicates.\"\"\"\n    return bar()\n\ndef bar():\n    return 1\n",
    )
    .unwrap();
    std::fs::write(dir.join("b.py"), "class Widget:\n    def spin(self):\n        return 2\n")
        .unwrap();
    std::fs::write(dir.join("go.mod"), "module example.com/fixture\n").unwrap();
    std::fs::write(
        dir.join("m.go"),
        "package m\n\n// F does a thing.\nfunc F() int { return G() }\n\nfunc G() int { return 1 }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("c.ts"),
        "export function tsEntry(): number {\n  return 3;\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("README.md"), "# Fixture\n\nUse `foo` and `Widget.spin`.\n").unwrap();
}

/// Map of file name → bytes for every file in a sharded output dir.
fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn assert_dirs_byte_identical(a: &Path, b: &Path, context: &str) {
    let (fa, fb) = (dir_bytes(a), dir_bytes(b));
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&fa), names(&fb), "{context}: file sets differ");
    for ((name, ba), (_, bb)) in fa.iter().zip(fb.iter()) {
        assert_eq!(ba, bb, "{context}: {name} bytes differ");
    }
}

#[test]
fn clean_builds_are_reproducible_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_fixture(&repo);
    let repo_s = repo.to_str().unwrap();

    // Two independent clean builds in the same process: every HashMap gets its
    // own RandomState, so any ordering leak shows up here without needing a
    // second process.
    let out1 = tmp.path().join("out1");
    let out2 = tmp.path().join("out2");
    write_merged_sharded(&generate_one(repo_s).unwrap().merged, &out1).unwrap();
    write_merged_sharded(&generate_one(repo_s).unwrap().merged, &out2).unwrap();
    assert_dirs_byte_identical(&out1, &out2, "clean vs clean");
}

#[test]
fn incremental_build_is_byte_identical_to_clean_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_fixture(&repo);
    let repo_s = repo.to_str().unwrap();

    // Warm the cache, then edit one file so the next build mixes reused and
    // fresh parses — the case where stale-reuse or ordering bugs would show.
    let mut cache = ParseCache::new();
    generate_one_with_cache(repo_s, &mut cache).unwrap();
    std::fs::write(
        repo.join("a.py"),
        "def foo():\n    \"\"\"Frobnicates harder.\"\"\"\n    return bar() + 1\n\ndef bar():\n    return 1\n",
    )
    .unwrap();

    let incr = generate_one_with_cache(repo_s, &mut cache).unwrap();
    assert!(cache.stats.reused > 0, "fixture must exercise the reuse path");
    assert!(cache.stats.reparsed > 0, "fixture must exercise the reparse path");
    let clean = generate_one(repo_s).unwrap();

    let out_incr = tmp.path().join("out_incr");
    let out_clean = tmp.path().join("out_clean");
    write_merged_sharded(&incr.merged, &out_incr).unwrap();
    write_merged_sharded(&clean.merged, &out_clean).unwrap();
    assert_dirs_byte_identical(&out_incr, &out_clean, "incremental vs clean");
}
