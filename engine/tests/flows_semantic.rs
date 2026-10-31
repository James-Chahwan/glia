//! GB.3 — the flow search's semantic tier in fused form (ruling
//! FB-7-FUSED): when no lexical concept tier matches any flow and
//! `ConceptOptions::semantic` holds a `SemanticSource` (with a rank cache
//! over the one repo), the flows through the files `find --files`' fused
//! order ranks with a `sem` score answer at tier `semantic`, ordered by that
//! file rank then the matched node's depth, each with its matched hop and
//! the hop's `similarity` (an annotation, never the order).
//!
//! The tree: `shop/ledger.py` (`export_ledger_rows`, documented as writing
//! the ledger out), `shop/report.py` (`format_report`, which calls it),
//! `shop/cache.py` (`warm_cache`, unrelated), and two entries: `main` in
//! `shop/cli.py` (main -> format_report -> export_ledger_rows) and
//! `test_warm` in `tests/test_warm.py` (-> warm_cache), embedded with the
//! deterministic `test-hash` model.
//!
//! What is pinned:
//! (1) a word no lexical tier matches finds the flow through the file the
//!     fused order ranks first: tier semantic, the matched hop, its
//!     similarity, one `matched` line naming the file and its rank, rows in
//!     file-rank order, no absence;
//! (2) a lexical match suppresses the semantic tier (the answer is
//!     byte-identical with or without the source);
//! (3) no source, or a source with no rank cache, is GB.1's answer byte for
//!     byte (the `no_match` absence), and a repo with no sidecar has no
//!     source;
//! (4) trace's concept fallback seeds from the top semantic flow's entry
//!     (`resolved_by` `concept_semantic`) only for a feature that resolved to
//!     no node; without the source the trace stays the `unknown_symbol`
//!     absence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glia_core::RepoId;
use glia_embed::{Embedder, HashEmbedder, MODEL_TEST_HASH};
use glia_engine::GenerateResult;
use glia_engine::absence::Answer;
use glia_engine::rank::RankCache;
use glia_engine::semantic::{EmbedOptions, SemanticSource, embed_repo, open_for_surface};
use glia_engine::trace::{
    ConceptHit, ConceptOptions, ConceptTier, TraceOptions, concept_flows, cross_stack_trace,
};

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

const CLI_PY: &str = "from shop.report import format_report


def main():
    return format_report([])
";

const TEST_WARM_PY: &str = "from shop.cache import warm_cache


def test_warm():
    assert warm_cache() == 1
";

/// A query no flow touches lexically (`write` names nothing), whose words
/// the ledger function's text holds.
const NO_NAME: &str = "write the ledger rows out to a csv file";

fn tree(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("glia-gb3-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, src) in [
        ("pyproject.toml", "[project]\nname = \"shop\"\n"),
        ("shop/__init__.py", ""),
        ("shop/ledger.py", LEDGER_PY),
        ("shop/report.py", REPORT_PY),
        ("shop/cache.py", CACHE_PY),
        ("shop/cli.py", CLI_PY),
        ("tests/test_warm.py", TEST_WARM_PY),
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

fn source(b: &GenerateResult) -> Arc<SemanticSource> {
    let roots: BTreeMap<u64, PathBuf> = b
        .repo_roots
        .iter()
        .map(|(k, v)| (*k, PathBuf::from(v)))
        .collect();
    let e: Arc<dyn Embedder> = Arc::new(HashEmbedder);
    Arc::new(SemanticSource::load(&b.merged, &roots, MODEL_TEST_HASH, e).expect("rows"))
}

fn cache(b: &GenerateResult) -> Arc<RankCache> {
    Arc::new(RankCache::new("test", &b.repo_roots, &b.repo_labels))
}

fn opts(b: &GenerateResult, src: Option<Arc<SemanticSource>>) -> ConceptOptions {
    let mut o = ConceptOptions::default();
    if src.is_some() {
        o.rank_cache = Some(cache(b));
    }
    o.semantic = src;
    o
}

fn ask(b: &GenerateResult, word: &str, o: &ConceptOptions) -> Answer<ConceptHit> {
    concept_flows(&b.merged, &b.repo_labels, word, o)
}

fn json<T: serde::Serialize>(a: &T) -> String {
    serde_json::to_string(a).expect("json")
}

/// The embedded tree, built.
fn embedded(tag: &str) -> (PathBuf, GenerateResult) {
    let root = tree(tag);
    let b = build(&root);
    embed(&root, &b);
    (root, b)
}

/// The fused file rank a semantic row's `matched` line names.
fn file_rank(h: &ConceptHit) -> usize {
    let m = &h.matched[0];
    let at = m.find(" -> fused file #").expect("fused file") + " -> fused file #".len();
    m[at..]
        .split(' ')
        .next()
        .and_then(|r| r.parse().ok())
        .expect("rank")
}

#[test]
fn a_word_no_lexical_tier_matches_finds_the_flow_through_the_top_fused_file() {
    let (root, b) = embedded("fused");
    let lexical = ask(&b, NO_NAME, &opts(&b, None));
    assert!(lexical.results.is_empty(), "{}", json(&lexical));
    assert_eq!(lexical.absence.as_ref().map(|a| a.reason), Some("no_match"));

    let a = ask(&b, NO_NAME, &opts(&b, Some(source(&b))));
    let out = json(&a);
    assert!(a.absence.is_none(), "{out}");
    assert!(!a.results.is_empty(), "{out}");
    assert!(
        a.results.iter().all(|h| h.tier == ConceptTier::Semantic),
        "{out}"
    );
    let top = &a.results[0];
    assert_eq!(top.rank, 1);
    assert_eq!(top.key, "main", "{out}");
    assert_eq!(top.tier.as_str(), "semantic");
    // The ledger file ranks first in the fused order; main's flow reaches it
    // at depth 2, through export_ledger_rows.
    assert_eq!(
        top.matched,
        vec![format!(
            "{NO_NAME} -> fused file #1 shop/ledger.py (no lexical match)"
        )],
        "{out}"
    );
    let hop = top.hop.as_ref().expect("the matched hop");
    assert!(hop.to_qname.ends_with("export_ledger_rows"), "{out}");
    assert_eq!(hop.to_file.as_deref(), Some("shop/ledger.py"), "{out}");
    assert_eq!(top.depth, hop.depth);
    assert_eq!(top.depth, 2, "{out}");
    let sim = top.similarity.expect("the hop is embedded");
    assert_eq!((sim * 10_000.0).round() / 10_000.0, sim);
    assert!(out.contains("\"tier\":\"semantic\""), "{out}");
    assert!(out.contains("\"similarity\":"), "{out}");
    // Rows follow the fused file rank, then depth: never similarity alone.
    let keys: Vec<(usize, usize)> = a.results.iter().map(|h| (file_rank(h), h.depth)).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "{out}");
    for (i, h) in a.results.iter().enumerate() {
        assert_eq!(h.rank, i + 1);
    }
    // Deterministic: a fresh source and cache answer byte for byte.
    let again = ask(&b, NO_NAME, &opts(&b, Some(source(&b))));
    assert_eq!(out, json(&again));
    // limit keeps the first rows.
    let mut one = opts(&b, Some(source(&b)));
    one.limit = 1;
    let cut = ask(&b, NO_NAME, &one);
    assert_eq!(cut.results.len(), 1);
    assert_eq!(json(&cut.results[0]), json(&a.results[0]));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_lexical_match_suppresses_the_semantic_tier() {
    let (root, b) = embedded("lexical");
    for word in ["ledger", "export_ledger_rows", "warm", "main", "cache"] {
        let plain = ask(&b, word, &opts(&b, None));
        assert!(!plain.results.is_empty(), "{word}: {}", json(&plain));
        let with = ask(&b, word, &opts(&b, Some(source(&b))));
        assert_eq!(json(&plain), json(&with), "{word}");
        assert!(
            with.results
                .iter()
                .all(|h| h.tier != ConceptTier::Semantic && h.similarity.is_none()),
            "{word}"
        );
        assert!(!json(&with).contains("similarity"), "{word}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn no_source_or_no_cache_is_gb1s_answer() {
    let root = tree("plain");
    let b = build(&root);
    // No sidecar: the surface loader gives no source.
    let e = || -> Result<Arc<dyn Embedder>, String> { Ok(Arc::new(HashEmbedder)) };
    assert!(open_for_surface(&b.merged, &b.repo_roots, "test-hash", e).is_none());
    let plain = ask(&b, NO_NAME, &ConceptOptions::default());
    assert!(plain.results.is_empty());
    assert_eq!(plain.absence.as_ref().map(|a| a.reason), Some("no_match"));
    let base = json(&plain);
    assert!(!base.contains("similarity") && !base.contains("semantic"));

    // Embedded, but no rank cache: no repo root to rank files in.
    embed(&root, &b);
    let mut o = ConceptOptions::default();
    o.semantic = Some(source(&b));
    assert_eq!(json(&ask(&b, NO_NAME, &o)), base);
    // And the explicit None option is the default answer.
    assert_eq!(json(&ask(&b, NO_NAME, &opts(&b, None))), base);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn trace_falls_back_to_the_top_semantic_flow_only_for_an_unresolved_word() {
    let (root, b) = embedded("trace");
    let plain = cross_stack_trace(&b.merged, NO_NAME, &TraceOptions::default());
    assert_eq!(plain.resolved_by, "none");
    assert!(plain.paths.is_empty());

    let mut o = TraceOptions::default();
    o.semantic = Some(source(&b));
    o.rank_cache = Some(cache(&b));
    let t = cross_stack_trace(&b.merged, NO_NAME, &o);
    assert_eq!(t.resolved_by, "concept_semantic", "{}", json(&t));
    assert!(
        t.seed
            .as_ref()
            .is_some_and(|s| s.qname.ends_with("cli::main")),
        "{}",
        json(&t)
    );
    assert!(t.absence.is_none() && !t.paths.is_empty(), "{}", json(&t));

    // A word that resolves (or a lexical concept match) answers as before.
    for word in ["format_report", "export_ledger_rows", "ledger"] {
        let a = cross_stack_trace(&b.merged, word, &TraceOptions::default());
        let mut with = TraceOptions::default();
        with.semantic = Some(source(&b));
        with.rank_cache = Some(cache(&b));
        let z = cross_stack_trace(&b.merged, word, &with);
        assert_eq!(json(&a), json(&z), "{word}");
        assert_ne!(z.resolved_by, "concept_semantic", "{word}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
