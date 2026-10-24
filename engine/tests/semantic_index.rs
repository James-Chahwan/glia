//! FB.4: the embed text recipes and the vector index over
//! `.glia/vectors.jsonl`.
//!
//! A tmp Python repo is built with `generate_one`; vectors are written with the
//! deterministic `test-hash` model over `embed_text`, as the embed run will.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use glia_code_domain::external_inputs::{VectorRow, b64_encode};
use glia_code_domain::node_kind as nk;
use glia_core::NodeId;
use glia_embed::{DIMS_TEST_HASH, HashEmbedder, MODEL_TEST_HASH, codec};
use glia_engine::semantic::{
    DEFAULT_RECIPE, EmbedRecipe, RECIPES, VECTORS_FILE, VectorIndex, embed_text, text_hash,
};
use glia_engine::{GenerateResult, generate_one};
use glia_graph::MergedGraph;

fn long_doc() -> String {
    // 60 words of 9 chars + space = 600 chars: the cap cuts it at 500.
    (0..60)
        .map(|i| format!("word{i:05}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn app_py(doc: &str) -> String {
    format!(
        "import requests\nimport numpy\nimport fetch_tools\nimport invoice\n\n\n\
         def fetch_invoice(invoice_id):\n    \"\"\"{doc}\"\"\"\n    return requests.get(invoice_id)\n\n\n\
         def render_chart(points):\n    \"\"\"Draw the chart of the points.\"\"\"\n    return numpy.array(points)\n\n\n\
         def twin_a():\n    return 1\n\n\n\
         def twin_b():\n    return 1\n"
    )
}

fn build(src: &str) -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    std::fs::write(td.path().join("app.py"), src).expect("write app.py");
    let r = generate_one(td.path().to_str().expect("utf-8")).expect("generate_one");
    (td, r)
}

fn rebuild(td: &tempfile::TempDir, src: &str) -> GenerateResult {
    std::fs::write(td.path().join("app.py"), src).expect("write app.py");
    generate_one(td.path().to_str().expect("utf-8")).expect("generate_one")
}

fn roots(r: &GenerateResult) -> BTreeMap<u64, PathBuf> {
    r.repo_roots
        .iter()
        .map(|(k, v)| (*k, PathBuf::from(v)))
        .collect()
}

fn id_of(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nodes
                .iter()
                .find(|n| g.nav.qname_by_id.get(&n.id).map(String::as_str) == Some(qname))
                .map(|n| n.id)
        })
        .unwrap_or_else(|| panic!("no node {qname}"))
}

fn row(qname: &str, model: &str, text_hash: Option<String>, v: &[f32]) -> VectorRow {
    VectorRow {
        qname: qname.to_string(),
        kind: Some("FUNCTION".to_string()),
        hint: None,
        model: Some(model.to_string()),
        dims: Some(v.len() as u32),
        text_hash,
        device: None,
        b64: b64_encode(&codec::to_le_bytes(v)),
    }
}

/// A fresh test-hash row for `qname` under `recipe`, as the embed run writes.
fn fresh(m: &MergedGraph, qname: &str, recipe: EmbedRecipe) -> VectorRow {
    let text = embed_text(m, id_of(m, qname), recipe).expect("embeddable");
    row(
        qname,
        MODEL_TEST_HASH,
        Some(text_hash(recipe, &text)),
        &HashEmbedder::embed_one(&text),
    )
}

fn write_rows(root: &Path, rows: &[VectorRow]) {
    let path = root.join(VECTORS_FILE);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let body: String = rows
        .iter()
        .map(|r| serde_json::to_string(r).expect("row json") + "\n")
        .collect();
    std::fs::write(path, body).expect("write vectors");
}

#[test]
fn t1_and_t2_texts() {
    let doc = long_doc();
    let (_td, r) = build(&app_py(&doc));
    let id = id_of(&r.merged, "app::fetch_invoice");
    let t1 = embed_text(&r.merged, id, EmbedRecipe::T1).expect("t1");
    let t2 = embed_text(&r.merged, id, EmbedRecipe::T2).expect("t2");
    let lines: Vec<&str> = t1.lines().collect();
    assert_eq!(lines[0], "fetch_invoice", "{t1}");
    assert_eq!(lines[1], "app::fetch_invoice", "{t1}");
    let cut: String = doc.chars().take(500).collect();
    assert_eq!(lines[2], cut, "doc capped at 500 chars");
    // The IMPORTS cell's names in stored order; `invoice` is contained in
    // the name and dropped, `fetch_tools` is not.
    assert_eq!(lines[3], "fetch_tools numpy requests", "{t1}");
    let chart = embed_text(
        &r.merged,
        id_of(&r.merged, "app::render_chart"),
        EmbedRecipe::T1,
    )
    .expect("t1");
    assert!(
        chart.ends_with("\nfetch_tools invoice numpy requests"),
        "{chart}"
    );
    assert_eq!(lines.len(), 4, "t1 has no code: {t1}");
    assert!(!t1.contains("return requests.get"), "{t1}");
    assert!(t2.starts_with(&format!("{t1}\n")), "t2 = t1 + code:\n{t2}");
    assert!(t2.contains("return requests.get(invoice_id)"), "{t2}");
    let code_at = t2.find("def fetch_invoice").expect("code in t2");
    assert!(code_at > t1.len(), "code goes last");
    assert_eq!(DEFAULT_RECIPE, EmbedRecipe::T1);
    assert_eq!(RECIPES, [EmbedRecipe::T1, EmbedRecipe::T2]);
}

#[test]
fn imports_contained_in_the_name_are_dropped() {
    let (_td, r) =
        build("import requests\n\n\ndef requests_wrapper():\n    return requests.get('x')\n");
    let id = id_of(&r.merged, "app::requests_wrapper");
    let t1 = embed_text(&r.merged, id, EmbedRecipe::T1).expect("t1");
    assert_eq!(t1, "requests_wrapper\napp::requests_wrapper", "{t1}");
}

#[test]
fn project_is_never_embedded_or_counted() {
    let (_td, r) = build(&app_py("Doc."));
    for g in &r.merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) == Some(&nk::PROJECT) {
                assert_eq!(embed_text(&r.merged, n.id, EmbedRecipe::T1), None);
            }
        }
    }
    let idx = VectorIndex::load(&r.merged, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T1);
    let missing: HashSet<NodeId> = idx.missing_ids().into_iter().collect();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id);
            if kind.is_some_and(|k| !glia_code_domain::profile::EMBED_KINDS.contains(k)) {
                assert!(!missing.contains(&n.id), "{kind:?} counted missing");
            }
        }
    }
    let s = idx.stats();
    assert_eq!(
        (s.rows, s.fresh, s.dims),
        (0, 0, 0),
        "no sidecar: missing only"
    );
    assert!(s.missing >= 5, "module + 4 functions: {s:?}");
    assert!(idx.is_empty());
}

#[test]
fn rows_sort_into_fresh_stale_other_model_and_missing() {
    let (td, r) = build(&app_py("Fetch one invoice by its id."));
    let m = &r.merged;
    let mut rows = vec![
        fresh(m, "app::fetch_invoice", EmbedRecipe::T1),
        fresh(m, "app::render_chart", EmbedRecipe::T1),
    ];
    let mut other = fresh(m, "app::twin_a", EmbedRecipe::T1);
    other.model = Some("Xenova/bge-small-en-v1.5".to_string());
    rows.push(other);
    rows.push(row(
        "app::no_such_fn",
        MODEL_TEST_HASH,
        None,
        &HashEmbedder::embed_one("x"),
    ));
    write_rows(td.path(), &rows);

    let idx = VectorIndex::load(m, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T1);
    let s = idx.stats().clone();
    assert_eq!(
        (
            s.rows,
            s.fresh,
            s.stale,
            s.other_model,
            s.unbound,
            s.bad,
            s.dims
        ),
        (4, 2, 0, 1, 1, 0, DIMS_TEST_HASH),
        "{s:?}"
    );
    assert_eq!(
        s.fresh + s.stale + s.other_model + s.unbound + s.bad,
        s.rows
    );
    let missing = idx.missing_ids();
    assert!(
        missing.contains(&id_of(m, "app::twin_a")),
        "other model is no vector"
    );
    assert!(!missing.contains(&id_of(m, "app::fetch_invoice")));

    // Under t2 the same rows embed other text: stale, and their nodes missing.
    let t2 = VectorIndex::load(m, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T2);
    assert_eq!(
        (t2.stats().fresh, t2.stats().stale),
        (0, 2),
        "{:?}",
        t2.stats()
    );

    // Edit the docstring: the row's text_hash no longer matches.
    let r2 = rebuild(&td, &app_py("Fetch one invoice, now by its number."));
    let idx2 = VectorIndex::load(&r2.merged, &roots(&r2), MODEL_TEST_HASH, EmbedRecipe::T1);
    let s2 = idx2.stats();
    assert_eq!((s2.fresh, s2.stale), (1, 1), "{s2:?}");
    let fetch = id_of(&r2.merged, "app::fetch_invoice");
    assert!(
        idx2.missing_ids().contains(&fetch),
        "a stale vector's node is missing"
    );
    assert_eq!(s2.missing, s.missing + 1);
}

#[test]
fn search_ranks_shared_words_first_and_ties_by_qname() {
    let (td, r) = build(&app_py("Fetch one invoice by its id."));
    let m = &r.merged;
    // twin_b written first: file order must not decide the tie.
    let rows: Vec<VectorRow> = [
        "app::twin_b",
        "app::render_chart",
        "app::twin_a",
        "app::fetch_invoice",
    ]
    .iter()
    .map(|q| fresh(m, q, EmbedRecipe::T1))
    .collect();
    write_rows(td.path(), &rows);
    let idx = VectorIndex::load(m, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T1);
    assert_eq!(idx.len(), 4);

    let q = HashEmbedder::embed_one("fetch invoice");
    let hits = idx.search(&q, 2, &HashSet::new());
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].qname, "app::fetch_invoice", "{hits:?}");
    assert!(hits[0].similarity >= hits[1].similarity);

    // twin_a and twin_b have one text but for the name: query their shared
    // vector exactly by embedding a twin with the name the hash shares.
    let twin_a = embed_text(m, id_of(m, "app::twin_a"), EmbedRecipe::T1).expect("t");
    let twin_b = embed_text(m, id_of(m, "app::twin_b"), EmbedRecipe::T1).expect("t");
    assert_ne!(twin_a, twin_b);
    // Identical vectors: write twin_b's row with twin_a's vector.
    let mut rows2 = rows.clone();
    let va = HashEmbedder::embed_one(&twin_a);
    rows2[0].b64 = b64_encode(&codec::to_le_bytes(&va));
    write_rows(td.path(), &rows2);
    let idx2 = VectorIndex::load(m, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T1);
    let tie = idx2.search(&va, 2, &HashSet::new());
    assert_eq!(
        tie.iter().map(|h| h.qname.as_str()).collect::<Vec<_>>(),
        ["app::twin_a", "app::twin_b"],
        "equal similarity ties by qname: {tie:?}"
    );
    assert_eq!(tie[0].similarity, tie[1].similarity);
    assert!((tie[0].similarity_rounded() - 1.0).abs() < 1e-4);

    // exclude skips a node; a query of other dims finds nothing.
    let excl: HashSet<NodeId> = [id_of(m, "app::twin_a")].into_iter().collect();
    let hits = idx2.search(&va, 1, &excl);
    assert_eq!(hits[0].qname, "app::twin_b");
    assert!(idx2.search(&[1.0, 0.0], 3, &HashSet::new()).is_empty());
}

#[test]
fn two_loads_are_identical() {
    let (td, r) = build(&app_py("Fetch one invoice by its id."));
    let m = &r.merged;
    let rows: Vec<VectorRow> = [
        "app::render_chart",
        "app::twin_a",
        "app::fetch_invoice",
        "app::twin_b",
    ]
    .iter()
    .map(|q| fresh(m, q, EmbedRecipe::T1))
    .collect();
    write_rows(td.path(), &rows);
    let a = VectorIndex::load(m, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T1);
    let b = VectorIndex::load(m, &roots(&r), MODEL_TEST_HASH, EmbedRecipe::T1);
    assert_eq!(a.stats(), b.stats());
    assert_eq!(a.missing_ids(), b.missing_ids());
    let q = HashEmbedder::embed_one("chart points");
    assert_eq!(
        a.search(&q, 10, &HashSet::new()),
        b.search(&q, 10, &HashSet::new())
    );
}

/// FB.14: a row's device never decides freshness. A fresh row stays fresh
/// whichever device wrote it (the devices agree to 4e-7), and a stale one
/// stays stale.
#[test]
fn a_rows_device_never_decides_freshness() {
    let (td, r) = build(&app_py("Fetch one invoice."));
    let m = &r.merged;
    let roots = roots(&r);
    let mut rows = vec![
        fresh(m, "app::fetch_invoice", DEFAULT_RECIPE),
        fresh(m, "app::render_chart", DEFAULT_RECIPE),
    ];
    write_rows(td.path(), &rows);
    let before = VectorIndex::load(m, &roots, MODEL_TEST_HASH, DEFAULT_RECIPE).missing_ids();
    rows[0].device = Some("webgpu".to_string());
    rows[1].device = Some("cuda".to_string());
    write_rows(td.path(), &rows);
    let index = VectorIndex::load(m, &roots, MODEL_TEST_HASH, DEFAULT_RECIPE);
    assert_eq!(index.missing_ids(), before, "a device changes nothing");
    for q in ["app::fetch_invoice", "app::render_chart"] {
        assert!(!index.missing_ids().contains(&id_of(m, q)), "{q} is fresh");
    }
    // A stale hash stays stale on any device.
    rows[0].text_hash = Some(text_hash(DEFAULT_RECIPE, "another text"));
    write_rows(td.path(), &rows);
    let index = VectorIndex::load(m, &roots, MODEL_TEST_HASH, DEFAULT_RECIPE);
    assert!(
        index
            .missing_ids()
            .contains(&id_of(m, "app::fetch_invoice"))
    );
}
