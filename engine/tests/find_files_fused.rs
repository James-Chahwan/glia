//! FB.7a — `find --files` ranked by the gated blend (ruling FB-7-FUSED,
//! FB.6b's PASS): with a [`SemanticSource`] (the repo's t2 vectors from
//! `embed_repo` and the deterministic `test-hash` embedder) the uncalibrated
//! order is `LX[IDR,FUSE[T1s,SEMp:t2]]`.
//!
//! The tree is find_files.rs's crate (`src/a.rs` .. `src/f.rs`, a spec-kit
//! task `T012` naming `src/b.rs`) plus `src/h.rs`, whose one function is
//! named for the query's words but which no past commit message mentions.
//!
//! What is pinned:
//! (1) no source: the answer is EB.8 / EC.13's (`LX[IDR,T1s]`, no `sem` key);
//! (2) with one: fusion FIND_FILES_FUSED, every row with an embedded node
//!     carries `sem`, and `src/h.rs` (semantic match, no text) ranks above a
//!     text-only match;
//! (3) ids still route first: the T012-named file stays rank 1;
//! (4) `select = false` is the fused order as an explicit choice;
//! (5) an embedder error drops to the lexical answer, byte for byte;
//! (6) two runs answer identically;
//! (7) a vector whose node moved is stale: the moved file (no text either)
//!     is no row until the next embed, and the index counts it missing.

mod git_fixture;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use git_fixture::GitRepo;
use glia_code_domain::snapshots::{
    HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, write_history_with_messages,
};
use glia_core::RepoId;
use glia_embed::{Embedder, HashEmbedder, MODEL_TEST_HASH};
use glia_engine::GenerateResult;
use glia_engine::absence::Answer;
use glia_engine::find::{
    FIND_FILES_FUSED, FIND_FILES_FUSION, FindFilesOptions, FoundFile, find_files,
};
use glia_engine::semantic::{
    DEFAULT_RECIPE, EmbedOptions, EmbedRecipe, FUSED_RECIPE, QUERY_PREFIX, SemanticSource,
    embed_repo,
};

const CARGO: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str =
    "pub mod a;\npub mod b;\npub mod c;\npub mod d;\npub mod e;\npub mod f;\npub mod h;\n";
const A: &str =
    "use crate::b::bill;\nuse crate::c::other;\n\npub fn run() {\n    other();\n    bill();\n}\n";
const TASKS: &str = "# Tasks\n\n- [ ] T012 billing export in src/b.rs\n";

fn fixture() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("Cargo.toml", CARGO);
    repo.write("src/lib.rs", LIB);
    for (p, t) in [
        ("src/a.rs", A),
        ("src/b.rs", "pub fn bill() {}\n"),
        ("src/c.rs", "pub fn other() {}\n"),
        ("src/d.rs", "pub fn misc() {}\n"),
        ("src/e.rs", "pub fn e() {}\n"),
        ("src/f.rs", "pub fn f() {}\n"),
        (
            "src/h.rs",
            "/// Fix the invoice rounding bug.\npub fn fix_invoice_rounding_bug(total: f64) -> f64 {\n    (total * 100.0).round() / 100.0\n}\n",
        ),
    ] {
        repo.write(p, t);
    }
    repo.write("specs/001-billing/tasks.md", TASKS);
    repo.commit("clean");
    repo
}

fn row(n: usize, t: i64, files: &[&str], subject: &str) -> (HistoryCommit, HistoryMessage) {
    let c = format!("{n:02}{}", "0".repeat(38));
    let hc = HistoryCommit {
        c: c.clone(),
        t,
        files: files
            .iter()
            .map(|p| HistoryFile {
                p: p.to_string(),
                a: Some(1),
                d: Some(0),
                from: None,
            })
            .collect(),
    };
    let m = HistoryMessage {
        c,
        at: t,
        s: subject.to_string(),
        b: String::new(),
        br: None,
    };
    (hc, m)
}

/// Past commits, newest first; `src/h.rs` is in none of them.
fn snapshot(repo: &GitRepo) {
    let rows = vec![
        row(4, 1_767_225_604, &["src/e.rs"], "invoice totals"),
        row(
            3,
            1_767_225_603,
            &["src/c.rs", "src/d.rs"],
            "invoice export rounding",
        ),
        row(2, 1_767_225_602, &["src/f.rs"], "export ledger cleanup"),
        row(
            1,
            1_767_225_601,
            &["src/a.rs", "src/b.rs"],
            "initial shop skeleton",
        ),
    ];
    let (commits, messages): (Vec<HistoryCommit>, Vec<HistoryMessage>) = rows.into_iter().unzip();
    let meta = HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new());
    write_history_with_messages(repo.root(), meta, &commits, &[], Some(&messages))
        .expect("snapshot");
}

fn build(repo: &GitRepo) -> GenerateResult {
    glia_engine::generate_one(repo.path()).expect("build")
}

fn roots(b: &GenerateResult) -> BTreeMap<u64, PathBuf> {
    b.repo_roots
        .iter()
        .map(|(k, v)| (*k, PathBuf::from(v)))
        .collect()
}

/// The embedded repo: snapshot, build, `embed_repo` under the default recipe.
fn embedded() -> (GitRepo, GenerateResult) {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let id = *b.repo_roots.keys().next().expect("one repo");
    let rep = embed_repo(
        repo.root(),
        &b.merged,
        RepoId(id),
        &HashEmbedder,
        &EmbedOptions::default(),
    )
    .expect("embed");
    assert!(rep.rows_written > 0, "{rep:?}");
    (repo, b)
}

fn source(b: &GenerateResult, embedder: Arc<dyn Embedder>) -> Arc<SemanticSource> {
    Arc::new(SemanticSource::load(&b.merged, &roots(b), MODEL_TEST_HASH, embedder).expect("rows"))
}

fn opts(src: Option<Arc<SemanticSource>>) -> FindFilesOptions {
    let mut o = FindFilesOptions::default();
    o.semantic = src;
    o
}

fn ask(b: &GenerateResult, q: &str, o: &FindFilesOptions) -> Answer<FoundFile> {
    find_files(&b.merged, &b.repo_roots, q, o).expect("answer")
}

fn files(rows: &[FoundFile]) -> Vec<&str> {
    rows.iter().map(|r| r.file.as_str()).collect()
}

fn pos(rows: &[FoundFile], p: &str) -> usize {
    rows.iter()
        .position(|r| r.file == p)
        .unwrap_or_else(|| panic!("{p} not in {:?}", files(rows)))
}

/// An embedder that always fails, under the test model's name.
struct Broken;

impl Embedder for Broken {
    fn model(&self) -> &str {
        MODEL_TEST_HASH
    }
    fn dims(&self) -> u32 {
        64
    }
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err("model offline".into())
    }
}

#[test]
fn the_default_recipe_is_the_gated_one() {
    assert_eq!(DEFAULT_RECIPE, EmbedRecipe::T2);
    assert_eq!(FUSED_RECIPE, EmbedRecipe::T2);
    assert_eq!(EmbedRecipe::default(), EmbedRecipe::T2);
    assert_eq!(
        QUERY_PREFIX,
        "Represent this sentence for searching relevant passages: "
    );
    assert_eq!(FIND_FILES_FUSED, "LX[IDR,FUSE[T1s,SEMp:t2]]");
}

#[test]
fn no_source_is_the_lexical_answer() {
    let (_repo, b) = embedded();
    let a = ask(&b, "invoice rounding bug", &FindFilesOptions::default());
    let sel = a.selection.as_ref().expect("selection");
    assert_eq!(sel.pick, FIND_FILES_FUSION);
    // text only: c.rs / d.rs (two tokens), e.rs (one); h.rs never matched
    let got = files(&a.results);
    assert_eq!(got.len(), 3, "{:?}", a.results);
    assert!(!got.contains(&"src/h.rs"));
    assert!(a.results.iter().all(|r| r.sem.is_none()));
    let json = serde_json::to_string(&a).expect("json");
    assert!(!json.contains("\"sem\""), "{json}");
}

#[test]
fn a_semantic_match_ranks_above_a_text_only_one() {
    let (_repo, b) = embedded();
    let src = source(&b, Arc::new(HashEmbedder));
    assert!(src.stats().fresh > 0, "{:?}", src.stats());
    assert_eq!(src.stats().missing, 0, "{:?}", src.stats());
    let a = ask(&b, "invoice rounding bug", &opts(Some(src)));
    let sel = a.selection.as_ref().expect("selection");
    assert_eq!(
        (sel.pick.as_str(), sel.source),
        (FIND_FILES_FUSED, "fallback")
    );
    // h.rs: no commit names it, its function is named for the query
    let h = &a.results[pos(&a.results, "src/h.rs")];
    assert_eq!((h.text, h.idr), (0.0, 0.0));
    let best = a
        .results
        .iter()
        .filter_map(|r| r.sem)
        .fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(h.sem, Some(best), "{:?}", a.results);
    assert!(
        pos(&a.results, "src/h.rs") < pos(&a.results, "src/e.rs"),
        "{:?}",
        a.results
    );
    // every row's file has an embedded node: each carries its raw score, 4 dp
    assert!(
        a.results.iter().all(|r| r
            .sem
            .is_some_and(|s| (s * 10_000.0).round() / 10_000.0 == s)),
        "{:?}",
        a.results
    );
    let json = serde_json::to_string(&a).expect("json");
    assert!(json.contains("\"sem\":"), "{json}");
    assert!(a.results.iter().enumerate().all(|(i, r)| r.rank == i + 1));
}

#[test]
fn ids_still_route_first() {
    let (_repo, b) = embedded();
    let src = source(&b, Arc::new(HashEmbedder));
    let a = ask(&b, "T012 export", &opts(Some(src)));
    let first = &a.results[0];
    assert_eq!(
        (first.rank, first.file.as_str(), first.idr),
        (1, "src/b.rs", 1.0)
    );
    assert_eq!(first.ids, ["T012"]);
    assert!(a.results[1..].iter().all(|r| r.idr == 0.0));
}

#[test]
fn select_off_is_the_fused_order_explicitly() {
    let (_repo, b) = embedded();
    let src = source(&b, Arc::new(HashEmbedder));
    let on = ask(&b, "invoice rounding bug", &opts(Some(src.clone())));
    let mut o = opts(Some(src));
    o.select = false;
    let off = ask(&b, "invoice rounding bug", &o);
    let sel = off.selection.as_ref().expect("selection");
    assert_eq!(
        (sel.pick.as_str(), sel.source),
        (FIND_FILES_FUSED, "explicit")
    );
    assert_eq!(files(&on.results), files(&off.results));
}

#[test]
fn an_embedder_error_drops_to_the_lexical_answer() {
    let (_repo, b) = embedded();
    let src = source(&b, Arc::new(Broken));
    for q in ["invoice rounding bug", "T012 export"] {
        let broken = ask(&b, q, &opts(Some(src.clone())));
        let lexical = ask(&b, q, &FindFilesOptions::default());
        assert_eq!(
            serde_json::to_string(&broken).expect("json"),
            serde_json::to_string(&lexical).expect("json"),
            "{q}"
        );
    }
}

#[test]
fn a_mismatched_model_or_no_sidecar_is_no_source() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    assert!(
        SemanticSource::load(
            &b.merged,
            &roots(&b),
            MODEL_TEST_HASH,
            Arc::new(HashEmbedder)
        )
        .is_none(),
        "no vectors.jsonl"
    );
    let (_repo, b) = embedded();
    assert!(
        SemanticSource::load(
            &b.merged,
            &roots(&b),
            "another/model",
            Arc::new(HashEmbedder)
        )
        .is_none()
    );
}

#[test]
fn two_runs_answer_identically() {
    let (_repo, b) = embedded();
    let src = source(&b, Arc::new(HashEmbedder));
    for q in ["invoice rounding bug", "T012 export", "nothing here"] {
        let x = serde_json::to_string(&ask(&b, q, &opts(Some(src.clone())))).expect("json");
        let y = serde_json::to_string(&ask(&b, q, &opts(Some(src.clone())))).expect("json");
        assert_eq!(x, y, "{q}");
    }
}

#[test]
fn a_moved_files_vectors_are_stale_until_reembedded() {
    let (repo, _b) = embedded();
    repo.git_mv("src/h.rs", "src/g.rs");
    repo.write("src/lib.rs", &LIB.replace("pub mod h;", "pub mod g;"));
    repo.remove("specs/001-billing/tasks.md");
    let b = build(&repo);
    let src = source(&b, Arc::new(HashEmbedder));
    assert!(src.stats().missing > 0, "{:?}", src.stats());
    let a = ask(&b, "invoice rounding bug", &opts(Some(src)));
    // g.rs has no fresh vector (scored as the lowest present, min-max 0) and
    // no text: its fused key is 0, so it is no row
    assert!(!files(&a.results).contains(&"src/g.rs"), "{:?}", a.results);
    // the files that kept their vectors still score
    assert!(a.results.iter().all(|r| r.sem.is_some()));
    // with T012's task gone nothing routes
    assert!(a.results.iter().all(|r| r.idr == 0.0));
}
