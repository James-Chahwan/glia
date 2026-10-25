//! FB.8 — pack's fused fallback (ruling FB-7-FUSED): when find matches
//! nothing by name and `PackArgs::semantic` holds a `SemanticSource`, the
//! seeds are `find --files`' fused order's files (`LX[IDR,FUSE[T1s,SEMp:t2]]`),
//! each file's best-matching node by cosine, tier `heuristic`, `matched`
//! `semantic` with its `similarity`, and the pack keeps find's lexical
//! absence as `seed_absence`.
//!
//! The tree: `shop/ledger.py` (`export_ledger_rows`, documented as writing
//! the ledger out), `shop/report.py` (`format_report`, which calls it) and
//! `shop/cache.py` (`warm_cache`, unrelated), embedded with the deterministic
//! `test-hash` model.
//!
//! What is pinned:
//! (1) a query with a name match packs byte-identically with or without the
//!     source;
//! (2) a query with none seeds from the fused files: the ledger function
//!     first, tier heuristic, matched semantic, similarity set, seed_absence
//!     = find's no_match, absence None, the title noting the fallback;
//! (3) no source (a repo with no sidecar has none) or no rank cache: the
//!     empty pack with find's absence and neither new key;
//! (4) pack_ids ignores the source;
//! (5) a two-repo merge never falls back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glia_core::{NodeId, RepoId};
use glia_embed::{Embedder, HashEmbedder, MODEL_TEST_HASH};
use glia_engine::GenerateResult;
use glia_engine::find::{FindOptions, find_nodes};
use glia_engine::pack::{Pack, PackArgs, pack, pack_ids};
use glia_engine::rank::RankCache;
use glia_engine::semantic::{EmbedOptions, SemanticSource, embed_repo, open_for_surface};

const LEDGER_PY: &str = "def export_ledger_rows(rows):
    \"\"\"Export the ledger rows to a csv file on disk.\"\"\"
    out = []
    for r in rows:
        out.append(r)
    return out
";

const REPORT_PY: &str = "from shop.ledger import export_ledger_rows


def format_report(rows):
    return export_ledger_rows(rows)
";

const CACHE_PY: &str = "def warm_cache():
    return 1
";

/// A query no node is named for, whose words the ledger function's text holds.
const NO_NAME: &str = "write the ledger rows out to a csv file";

fn tree(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("glia-fb8-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, src) in [
        ("pyproject.toml", "[project]\nname = \"shop\"\n"),
        ("shop/__init__.py", ""),
        ("shop/ledger.py", LEDGER_PY),
        ("shop/report.py", REPORT_PY),
        ("shop/cache.py", CACHE_PY),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("dir");
        std::fs::write(p, src).expect("write");
    }
    root
}

fn build(root: &Path) -> GenerateResult {
    glia_engine::generate_one(root.to_str().expect("utf-8 temp path")).expect("build")
}

fn embed(root: &Path, b: &GenerateResult) {
    let id = *b.repo_roots.keys().next().expect("one repo");
    let rep = embed_repo(
        root,
        &b.merged,
        RepoId(id),
        &HashEmbedder,
        &EmbedOptions::default(),
    )
    .expect("embed");
    assert!(rep.rows_written > 0, "{rep:?}");
}

fn roots(b: &GenerateResult) -> BTreeMap<u64, PathBuf> {
    b.repo_roots
        .iter()
        .map(|(k, v)| (*k, PathBuf::from(v)))
        .collect()
}

fn source(b: &GenerateResult) -> Arc<SemanticSource> {
    let e: Arc<dyn Embedder> = Arc::new(HashEmbedder);
    Arc::new(SemanticSource::load(&b.merged, &roots(b), MODEL_TEST_HASH, e).expect("rows"))
}

fn args(b: &GenerateResult, src: Option<Arc<SemanticSource>>) -> PackArgs {
    let mut a = PackArgs::default();
    a.rank_cache = Some(Arc::new(RankCache::new(
        "test",
        &b.repo_roots,
        &b.repo_labels,
    )));
    a.semantic = src;
    a
}

fn json(p: &Pack) -> String {
    serde_json::to_string(p).expect("json")
}

/// The embedded tree, built.
fn embedded(tag: &str) -> (PathBuf, GenerateResult) {
    let root = tree(tag);
    let b = build(&root);
    embed(&root, &b);
    (root, b)
}

#[test]
fn a_name_match_packs_the_same_with_or_without_vectors() {
    let (root, b) = embedded("name");
    let src = source(&b);
    for q in ["export_ledger_rows", "format_report", "warm"] {
        let plain = pack(&b.merged, q, &args(&b, None));
        let with = pack(&b.merged, q, &args(&b, Some(src.clone())));
        assert!(!plain.nodes.is_empty(), "{q}: {}", json(&plain));
        assert_eq!(json(&plain), json(&with), "{q}");
        assert!(with.seed_absence.is_none());
        assert!(with.nodes.iter().all(|n| n.similarity.is_none()));
        assert!(!json(&with).contains("\"similarity\""));
        assert!(!json(&with).contains("\"seed_absence\""));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn no_name_match_seeds_from_the_fused_files() {
    let (root, b) = embedded("fused");
    let found = find_nodes(&b.merged, NO_NAME, &FindOptions::default());
    assert!(found.results.is_empty(), "{:?}", found.results);
    let src = source(&b);
    let p = pack(&b.merged, NO_NAME, &args(&b, Some(src)));
    let out = json(&p);
    assert!(p.absence.is_none(), "{out}");
    let seed_absence = p.seed_absence.as_ref().expect("seed_absence");
    assert_eq!(seed_absence.reason, "no_match");
    let seeds: Vec<_> = p.nodes.iter().filter(|n| n.reason == "seed").collect();
    assert!(!seeds.is_empty(), "{out}");
    for s in &seeds {
        assert_eq!(
            (s.tier, s.matched),
            ("heuristic", Some("semantic")),
            "{out}"
        );
        let sim = s.similarity.expect("similarity");
        assert_eq!((sim * 10_000.0).round() / 10_000.0, sim);
    }
    // The ledger file ranks first in the fused order; its seed is the file's
    // best match by cosine (here its MODULE, whose text holds the function),
    // one seed per file.
    assert_eq!(seeds[0].file.as_deref(), Some("shop/ledger.py"), "{out}");
    let files: std::collections::BTreeSet<_> = seeds.iter().map(|s| s.file.clone()).collect();
    assert_eq!(files.len(), seeds.len(), "{out}");
    // Neighbours stay derived, with no similarity.
    assert!(
        p.nodes
            .iter()
            .filter(|n| n.reason == "neighbour")
            .all(|n| n.tier == "derived" && n.similarity.is_none() && n.matched.is_none())
    );
    assert!(
        p.text.starts_with(&format!(
            "# context for {NO_NAME} (seeded by the fused file order: nothing matches by name)"
        )),
        "{}",
        p.text
    );
    assert!(out.contains("\"similarity\":"), "{out}");
    assert!(out.contains("\"seed_absence\":{"), "{out}");
    // Deterministic: a second run answers byte for byte.
    let again = pack(&b.merged, NO_NAME, &args(&b, Some(source(&b))));
    assert_eq!(out, json(&again));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn no_source_or_no_cache_is_the_empty_pack() {
    let root = tree("plain");
    let b = build(&root);
    // No sidecar: the surface loader gives no source.
    let e = || -> Result<Arc<dyn Embedder>, String> { Ok(Arc::new(HashEmbedder)) };
    assert!(open_for_surface(&b.merged, &b.repo_roots, "test-hash", e).is_none());
    let plain = pack(&b.merged, NO_NAME, &args(&b, None));
    assert!(plain.nodes.is_empty());
    assert_eq!(plain.absence.as_ref().map(|a| a.reason), Some("no_match"));
    assert!(plain.seed_absence.is_none());
    let out = json(&plain);
    assert!(!out.contains("seed_absence") && !out.contains("similarity"));

    // Embedded, but no rank cache: the fallback has no repo root to rank.
    embed(&root, &b);
    let mut a = PackArgs::default();
    a.semantic = Some(source(&b));
    let no_cache = pack(&b.merged, NO_NAME, &a);
    assert_eq!(json(&no_cache), out);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn pack_ids_ignores_the_source() {
    let (root, b) = embedded("ids");
    let found = find_nodes(&b.merged, "warm_cache", &FindOptions::default());
    let id = NodeId(found.results.first().expect("warm_cache").id);
    for ids in [vec![id], vec![], vec![NodeId(7)]] {
        let plain = pack_ids(&b.merged, &ids, &args(&b, None));
        let with = pack_ids(&b.merged, &ids, &args(&b, Some(source(&b))));
        assert_eq!(json(&plain), json(&with));
        assert!(with.seed_absence.is_none());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_multi_repo_merge_never_falls_back() {
    let (root, _) = embedded("multi-a");
    let other = std::env::temp_dir().join(format!("glia-fb8-multi-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&other);
    std::fs::create_dir_all(other.join("svc")).expect("dir");
    std::fs::write(other.join("svc/main.py"), "def serve():\n    return 0\n").expect("write");
    let paths = vec![
        root.to_str().expect("utf-8").to_string(),
        other.to_str().expect("utf-8").to_string(),
    ];
    let b = glia_engine::generate_many(&paths).expect("build");
    assert_eq!(b.repo_roots.len(), 2);
    let e: Arc<dyn Embedder> = Arc::new(HashEmbedder);
    let src =
        Arc::new(SemanticSource::load(&b.merged, &roots(&b), MODEL_TEST_HASH, e).expect("rows"));
    let plain = pack(&b.merged, NO_NAME, &args(&b, None));
    let with = pack(&b.merged, NO_NAME, &args(&b, Some(src)));
    assert!(with.nodes.is_empty());
    assert_eq!(json(&plain), json(&with));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

/// A pack whose find matches by name never builds the query embedder: the
/// source opened for a surface defers it to the first query (FB.8's
/// close-out), so a repo with vectors starts no model on such a pack. The
/// fallback builds it once and embeds the query once.
#[test]
fn a_name_match_never_builds_the_query_embedder() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (root, b) = embedded("lazy");
    let built = Arc::new(AtomicUsize::new(0));
    let counter = built.clone();
    let src = open_for_surface(&b.merged, &b.repo_roots, "test-hash", move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(HashEmbedder) as Arc<dyn Embedder>)
    })
    .expect("a source over the test-hash vectors");
    let named = pack(
        &b.merged,
        "export_ledger_rows",
        &args(&b, Some(src.clone())),
    );
    assert!(!named.nodes.is_empty());
    assert_eq!(
        built.load(Ordering::SeqCst),
        0,
        "a name match built the embedder"
    );
    let fallback = pack(&b.merged, NO_NAME, &args(&b, Some(src)));
    assert!(fallback.seed_absence.is_some(), "{}", json(&fallback));
    assert_eq!(built.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(&root);
}
