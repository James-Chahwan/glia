//! LA.12 gate: the parse cache reports WHICH files a build reused, reparsed
//! and evicted (`ParseCache::last_diff`), enumerates what it holds
//! (`ParseCache::iter`), and the `[incremental]` counters (`cache.stats`) are
//! the lengths of that diff — not a second, hand-kept tally. LE.1 (graph
//! delta) and LG.8 (`glia-export-engram --since`) stand on this file-level
//! delta.

use std::path::Path;

use glia_engine::cache::{CacheDiff, content_hash};
use glia_engine::{ParseCache, generate_one_with_cache};

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).expect("write fixture file");
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn incremental_build_records_the_file_diff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let repo = dir.to_str().expect("utf-8 tempdir");

    write(dir, "a.py", "def a():\n    return 1\n");
    write(dir, "b.py", "def b():\n    return 2\n");
    write(dir, "c.py", "def c():\n    return 3\n");

    let mut cache = ParseCache::new();
    assert!(cache.last_diff().is_none(), "a fresh cache has built nothing");

    // Cold: every input is new to the cache.
    generate_one_with_cache(repo, &mut cache).expect("cold build");
    let cold = cache.last_diff().expect("cold build recorded a diff").clone();
    assert_eq!(
        cold,
        CacheDiff { reused: vec![], reparsed: names(&["a.py", "b.py", "c.py"]), evicted: vec![] }
    );

    // Edit b, delete c, add d.
    let b_text = "def b():\n    return 2 + 2\n";
    let d_text = "def d():\n    return 4\n";
    write(dir, "b.py", b_text);
    std::fs::remove_file(dir.join("c.py")).expect("delete c.py");
    write(dir, "d.py", d_text);

    generate_one_with_cache(repo, &mut cache).expect("warm build");
    let warm = cache.last_diff().expect("warm build recorded a diff").clone();
    assert_eq!(
        warm,
        CacheDiff {
            reused: names(&["a.py"]),
            reparsed: names(&["b.py", "d.py"]),
            evicted: names(&["c.py"]),
        }
    );

    // The marker counters ARE the diff's lengths.
    assert_eq!(
        (cache.stats.reused, cache.stats.reparsed, cache.stats.evicted),
        (warm.reused.len(), warm.reparsed.len(), warm.evicted.len())
    );

    // iter(): path order, one row per surviving file, each carrying the hash
    // of the text it was parsed from.
    let texts = [("a.py", "def a():\n    return 1\n"), ("b.py", b_text), ("d.py", d_text)];
    let held: Vec<(String, u64, String)> = cache
        .iter()
        .map(|f| (f.path.to_string(), f.content_hash, f.lang.to_string()))
        .collect();
    let want: Vec<(String, u64, String)> = texts
        .iter()
        .map(|(p, t)| ((*p).to_string(), content_hash(t), "python".to_string()))
        .collect();
    assert_eq!(held, want);
    assert!(
        cache.iter().all(|f| !f.parse.nodes.is_empty()),
        "every cached file carries its parse"
    );

    // A no-change rebuild reuses everything and evicts nothing.
    generate_one_with_cache(repo, &mut cache).expect("idle build");
    assert_eq!(
        cache.last_diff(),
        Some(&CacheDiff { reused: names(&["a.py", "b.py", "d.py"]), reparsed: vec![], evicted: vec![] })
    );
    assert_eq!((cache.stats.reused, cache.stats.reparsed, cache.stats.evicted), (3, 0, 0));
}
