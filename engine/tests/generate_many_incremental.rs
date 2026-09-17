//! A1.4 gate: `generate_many` threads a per-repo `ParseCache` — opt-in.
//!
//! The multi-repo path is what `glia merge`, every `--with` query and pyo3
//! `generate_many` run on, and it used to hardcode `cache: None` (audit
//! 2026-06-10 #14). `generate_many_incremental` gives each repo path its OWN
//! `<repo>/.ai/repo-graph/parse_cache.bin`. Plain `generate_many` must stay
//! cold and sidecar-free: `bench/substrate-gap/grade.py` grades every multi-dir
//! fixture through it with no way to opt out, and `GLIA_NO_PERSIST=1` gates
//! only the gmap write, never the parse-cache sidecar.

use std::path::{Path, PathBuf};

use repo_graph_engine::cache::content_hash;
use repo_graph_engine::{ParseCache, generate_many, generate_many_incremental};
use repo_graph_store::write_merged_sharded;

/// `<repo>/.ai/repo-graph/parse_cache.bin` — mirrors the engine's private
/// `cache::gmap_dir` + `CACHE_FILE`.
fn sidecar(repo: &Path) -> PathBuf {
    repo.join(".ai").join("repo-graph").join("parse_cache.bin")
}

const A_PY: &str = "import requests\n\ndef foo():\n    \"\"\"Frobnicates.\"\"\"\n    return requests.get(\"http://svc-b/api/items\")\n\ndef bar():\n    return foo()\n";
const B_PY: &str = "class Widget:\n    def spin(self):\n        return 2\n";

/// Repo A: three python files + markdown, so the doc graph is exercised.
fn write_repo_a(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("a.py"), A_PY).unwrap();
    std::fs::write(dir.join("b.py"), B_PY).unwrap();
    std::fs::write(dir.join("c.py"), "def gone():\n    return None\n").unwrap();
    std::fs::write(dir.join("README.md"), "# Service A\n\nUse `foo` and `Widget.spin`.\n")
        .unwrap();
}

/// Repo B: two go files under a go.mod, so the go-prefix build context and a
/// second shard are exercised. Two parser files vs A's three, so the two
/// sidecars have different sizes.
fn write_repo_b(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("go.mod"), "module example.com/svcb\n").unwrap();
    std::fs::write(
        dir.join("main.go"),
        "package main\n\nimport \"net/http\"\n\n// Items lists items.\nfunc Items(w http.ResponseWriter, r *http.Request) {}\n\nfunc main() {\n\thttp.HandleFunc(\"/api/items\", Items)\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("util.go"), "package main\n\nfunc helper() int { return 1 }\n").unwrap();
}

fn two_repos(tmp: &Path) -> (PathBuf, PathBuf, Vec<String>) {
    let a = tmp.join("svc-a");
    let b = tmp.join("svc-b");
    write_repo_a(&a);
    write_repo_b(&b);
    let paths = vec![a.to_str().unwrap().to_string(), b.to_str().unwrap().to_string()];
    (a, b, paths)
}

/// Map of file name → bytes for every file in a sharded output dir (same
/// helper as `byte_identical.rs`).
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

/// The hermeticity guard for grade.py: the default multi-repo build writes
/// nothing into the repos it reads.
#[test]
fn generate_many_writes_no_sidecar_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, paths) = two_repos(tmp.path());
    let result = generate_many(&paths).unwrap();
    assert!(result.total_nodes > 0, "fixture must produce a graph");
    assert!(!sidecar(&a).exists(), "generate_many wrote a sidecar into {}", a.display());
    assert!(!sidecar(&b).exists(), "generate_many wrote a sidecar into {}", b.display());
    assert!(!a.join(".ai").exists(), "generate_many created {}/.ai", a.display());
    assert!(!b.join(".ai").exists(), "generate_many created {}/.ai", b.display());
}

/// Warm both sidecars, then change BOTH repos so the second build mixes reused,
/// reparsed, new and evicted files on each side, and require the on-disk shards
/// to match a cold `generate_many` byte for byte — including the cross-repo
/// edges, which are what break if a reused parse carries the wrong RepoId.
#[test]
fn multi_repo_incremental_is_byte_identical_to_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, paths) = two_repos(tmp.path());

    generate_many_incremental(&paths).unwrap();
    assert!(sidecar(&a).exists() && sidecar(&b).exists(), "warm build must write both sidecars");

    // A: a.py reused, b.py reparsed, c.py evicted.
    std::fs::write(
        a.join("b.py"),
        "class Widget:\n    def spin(self):\n        return 3\n\n    def stop(self):\n        return 0\n",
    )
    .unwrap();
    std::fs::remove_file(a.join("c.py")).unwrap();
    // B: util.go reused, main.go reparsed, extra.go new.
    std::fs::write(
        b.join("main.go"),
        "package main\n\nimport \"net/http\"\n\n// Items lists items, now paged.\nfunc Items(w http.ResponseWriter, r *http.Request) { page() }\n\nfunc main() {\n\thttp.HandleFunc(\"/api/items\", Items)\n}\n",
    )
    .unwrap();
    std::fs::write(b.join("extra.go"), "package main\n\nfunc page() {}\n").unwrap();

    let incr = generate_many_incremental(&paths).unwrap();
    let clean = generate_many(&paths).unwrap();
    assert!(
        !clean.merged.cross_edges.is_empty(),
        "fixture must exercise a cross-repo edge (python requests -> go route)"
    );

    // Third build with nothing changed: every parse is a cache hit.
    let warm = generate_many_incremental(&paths).unwrap();

    let out_incr = tmp.path().join("out_incr");
    let out_clean = tmp.path().join("out_clean");
    let out_warm = tmp.path().join("out_warm");
    write_merged_sharded(&incr.merged, &out_incr).unwrap();
    write_merged_sharded(&clean.merged, &out_clean).unwrap();
    write_merged_sharded(&warm.merged, &out_warm).unwrap();
    assert_dirs_byte_identical(&out_incr, &out_clean, "multi-repo incremental vs clean");
    assert_dirs_byte_identical(&out_warm, &out_clean, "multi-repo fully-warm vs clean");
}

/// Each repo path owns its own sidecar, holding only its own files.
#[test]
fn sidecars_are_per_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, paths) = two_repos(tmp.path());
    generate_many_incremental(&paths).unwrap();

    assert!(sidecar(&a).exists(), "missing {}", sidecar(&a).display());
    assert!(sidecar(&b).exists(), "missing {}", sidecar(&b).display());
    let (a_s, b_s) = (a.to_str().unwrap(), b.to_str().unwrap());
    let (ca, cb) = (ParseCache::load(a_s), ParseCache::load(b_s));
    // A: a.py + b.py + c.py. B: main.go + util.go. Each holds only its own.
    assert_eq!(ca.len(), 3, "svc-a cache must hold exactly its three .py files");
    assert_eq!(cb.len(), 2, "svc-b cache must hold exactly its two .go files");
    assert_ne!(ca.len(), cb.len());

    // The sidecar is keyed to the repo identity it was built under: a cache
    // pointed at the other repo's identity discards everything (audit #2).
    let mut cross = ParseCache::load(a_s);
    cross.validate_context(&format!("file://{b_s}"), "example.com/svcb");
    assert!(cross.is_empty(), "svc-a's cache must not be reusable as svc-b's");
}

/// Proof the multi-repo path READS the sidecar rather than reparsing and
/// merely writing one: plant a.py's cached parse under b.py's key and content
/// hash. A build that consults the cache serves the planted parse, so
/// `Widget` (only in b.py) vanishes; a cold build still sees it.
#[test]
fn the_sidecar_is_actually_consulted() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, _b, paths) = two_repos(tmp.path());
    let a_s = a.to_str().unwrap();
    generate_many_incremental(&paths).unwrap();

    let mut cache = ParseCache::load(a_s);
    let a_parse = cache
        .get("a.py", content_hash(A_PY), "python")
        .expect("warm build must have cached a.py");
    cache.put("b.py".to_string(), content_hash(B_PY), "python", a_parse);
    cache.save(a_s).unwrap();

    let planted = generate_many_incremental(&paths).unwrap();
    let clean = generate_many(&paths).unwrap();
    assert!(
        !clean.merged.qnames_containing("Widget").is_empty(),
        "cold build must see Widget"
    );
    assert!(
        planted.merged.qnames_containing("Widget").is_empty(),
        "generate_many_incremental did not serve svc-a's cached parse for b.py"
    );
}

/// Audit #2 on the multi-repo path: the same directory under another spelling
/// is another RepoId, and every cached parse has the old one baked into its
/// NodeIds. The build must discard the sidecar, not serve stale identities.
#[test]
fn a_respelled_repo_path_discards_the_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let (_a, _b, paths) = two_repos(tmp.path());
    generate_many_incremental(&paths).unwrap();

    // Same directories, trailing-slash spelling: same sidecar file on disk,
    // different `file://` canonical, so a different RepoId.
    let respelled: Vec<String> = paths.iter().map(|p| format!("{p}/")).collect();
    let incr = generate_many_incremental(&respelled).unwrap();
    let clean = generate_many(&respelled).unwrap();

    let out_incr = tmp.path().join("out_incr");
    let out_clean = tmp.path().join("out_clean");
    write_merged_sharded(&incr.merged, &out_incr).unwrap();
    write_merged_sharded(&clean.merged, &out_clean).unwrap();
    assert_dirs_byte_identical(&out_incr, &out_clean, "respelled incremental vs clean");
}
