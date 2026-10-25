//! Text → nodes: `find` (LD.3b's ranked fuzzy find, the one name / qname
//! lookup) and `resolve` (P3: a failure / change signal → located records);
//! text → files: `find_files` (EB.8, the files a free-text query is about,
//! LX[IDR,T1s]). Each returns the LD.8a envelope `{results, absence}` as a
//! native `dict`.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use glia_code_domain::node_kind;
use glia_core::{NodeId, NodeKindId};
use glia_engine::absence::Answer;
use glia_engine::find::{
    FindFilesOptions, FindOptions, FoundFile, FoundNode, find_files, find_nodes_with_live,
};
use glia_engine::identity::Collapse;
use glia_engine::rank::RankCache;
use glia_engine::semantic::{
    FUSED_RECIPE, SemanticSource, VECTORS_FILE, VectorIndex, surface_model,
};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::embed::{surface_embedder, worker_dirs};
use crate::graph::PyGraph;

// `find`'s `top_k=20` below is a literal so `__text_signature__` shows it;
// this keeps it the engine's default.
const _: () = assert!(glia_engine::find::DEFAULT_TOP_K == 20);

/// Node-kind names → ids, case-insensitively, as `glia find --kind` reads
/// them. An unknown name is an error naming every valid one.
fn kinds_by_name(names: &[String]) -> Result<Vec<NodeKindId>, String> {
    names
        .iter()
        .map(|n| {
            node_kind::ALL
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(n))
                .map(|(id, _)| *id)
                .ok_or_else(|| {
                    let valid: Vec<&str> = node_kind::ALL.iter().map(|(_, name)| *name).collect();
                    format!("unknown kind '{n}'; valid kinds: {}", valid.join(", "))
                })
        })
        .collect()
}

/// The whole body of [`PyGraph::find`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). An empty
/// `kinds` list filters nothing, as `glia find` with no `--kind`. `live` is
/// the graph's cached `entrypoint_reachable` set (`PyGraph::live`);
/// `collapse` is `Collapse::with_labels(&PyGraph::repo_labels)` or
/// `Collapse::off()` (DD.9). `repo_roots` (`PyGraph::repo_roots`) are the
/// roots whose vocabulary snapshots a no-match absence reads (FB.13c:
/// `named_in`).
#[allow(clippy::too_many_arguments)]
fn find_answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    query: &str,
    top_k: usize,
    kinds: &[String],
    scope: Option<String>,
    collapse: Collapse,
    unparsed_files: usize,
    repo_roots: &BTreeMap<u64, String>,
) -> Result<Answer<FoundNode>, String> {
    let kinds = kinds_by_name(kinds)?;
    let mut opts = FindOptions::default();
    opts.top_k = top_k;
    opts.kinds = (!kinds.is_empty()).then_some(kinds);
    opts.scope = scope;
    opts.collapse = collapse;
    opts.vocab_roots = Some(
        repo_roots
            .iter()
            .map(|(id, p)| (*id, std::path::PathBuf::from(p)))
            .collect(),
    );
    let mut answer = find_nodes_with_live(merged, live, query, &opts);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

/// The `selection.reason` of a `find_files(.., semantic=True)` answer whose
/// repo holds no vector of the surface model (FB.7b): the fallback it ranks
/// by is the unfused one.
const NO_VECTORS: &str = "no_vectors";

/// The vector model name a surface model name records on its rows.
fn recorded_model(surface: &str) -> &'static str {
    if surface == "test-hash" {
        glia_embed::MODEL_TEST_HASH
    } else {
        glia_embed::MODEL_BGE_SMALL
    }
}

/// FB.7b, minus pyo3: the fused-recipe vectors of `surface` (the caller
/// passes `semantic::surface_model()`) under `repo_roots` and the query
/// embedder (`embed::surface_embedder`'s, the
/// process's one bge-small session; its worker looked for in `dirs`), or
/// None: not one repo, no `.glia/vectors.jsonl` (nothing read), no fresh
/// vector of the model, or an embedder that fails to load (a `warning:`
/// stderr line).
fn semantic_source(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    surface: &str,
    embedder: impl FnOnce() -> Result<Arc<dyn glia_embed::Embedder>, String>,
) -> Option<Arc<SemanticSource>> {
    if repo_roots.len() != 1 {
        return None;
    }
    let roots: BTreeMap<u64, PathBuf> = repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    if !roots.values().any(|r| r.join(VECTORS_FILE).is_file()) {
        return None;
    }
    let model = recorded_model(surface);
    let index = VectorIndex::load(merged, &roots, model, FUSED_RECIPE);
    if index.is_empty() {
        return None;
    }
    let loaded = (|| {
        if surface == "bge-small" {
            for root in roots.values() {
                glia_embed::refuse_repo_cache(root)?;
            }
        }
        embedder()
    })();
    let embedder = match loaded {
        Ok(e) if e.model() == model => e,
        Ok(e) => {
            eprintln!(
                "warning: the query embedder embeds {}, the vectors are {model}; ranking without vectors",
                e.model()
            );
            return None;
        }
        Err(e) => {
            eprintln!(
                "warning: the query embedder ({surface}) did not load, ranking without vectors: {e}"
            );
            return None;
        }
    };
    SemanticSource::from_index(index, embedder).map(Arc::new)
}

/// The whole body of [`PyGraph::find_files`], minus pyo3: the engine's
/// `find_files` on surface `py` over the PyGraph's held [`RankCache`] (EA.7)
/// and, when given, its [`SemanticSource`] (FB.7b), the absence counting the
/// build's parse errors. `wanted_semantic` (`semantic=True`) with no source
/// marks a fallback selection's reason [`NO_VECTORS`]. Err when the graph
/// holds more than one repo.
#[allow(clippy::too_many_arguments)]
fn find_files_answer(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    cache: Arc<RankCache>,
    semantic: Option<Arc<SemanticSource>>,
    wanted_semantic: bool,
    query: &str,
    top_k: usize,
    unparsed_files: usize,
) -> Result<Answer<FoundFile>, String> {
    let mut opts = FindFilesOptions::default();
    opts.top_k = top_k;
    opts.surface = "py";
    opts.rank_cache = Some(cache);
    let none = semantic.is_none();
    opts.semantic = semantic;
    let mut answer = find_files(merged, repo_roots, query, &opts)?;
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    if wanted_semantic
        && none
        && let Some(s) = answer.selection.as_mut()
        && s.source == "fallback"
    {
        s.reason = Some(NO_VECTORS);
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **find_files** (EB.8): the files of this graph's one repo that the
    /// free-text `query` (a task title, a ticket line) is about, ranked
    /// `LX[IDR,T1s]`: files the query's ids reach first (typed id routing
    /// through the repo's task / requirement rows, trackers and citing
    /// comments, read from the working tree), then the query's BM25 over past
    /// commit messages (the `.glia/history-snapshot/` `glia history sync`
    /// writes), ties by the file's degree, then path. Returns a dict
    /// `{results, absence}`: `results` is the records `{rank, file, idr,
    /// text, degree, ids}` (`rank` 1-based; `ids` the routed ids that reached
    /// the file); `absence` is `None` when there are results, else a
    /// `no_match` dict whose `note` names what was missing (no id resolved,
    /// no history messages, no matching message). `top_k` keeps the first
    /// rows (0 keeps all). A graph of several repos raises ValueError.
    ///
    /// EC.13: with a calibration record (`history_calibrate`, group `find`)
    /// the files follow the repo's `SELf2t` pick instead of `LX[IDR,T1s]`;
    /// the dict gains `selection` `{set, shape, pick, source, reason,
    /// test_commits}` (`source` `calibrated` or `fallback` with its
    /// `reason`).
    ///
    /// FB.7b: `semantic` blends the repo's embeddings in. `None` (auto) and
    /// `True` read the repo's `.glia/vectors.jsonl` (`embed`) for vectors of
    /// the surface model (env `GLIA_EMBED_MODEL`, default `bge-small`); with
    /// some, the uncalibrated order is `LX[IDR,FUSE[T1s,SEMp:t2]]` and each
    /// row with an embedded node carries `sem` (its raw similarity, 4 dp).
    /// With none the answer is the unfused one; under `True` its fallback
    /// `selection.reason` reads `no_vectors`. `False` never reads the
    /// sidecar. The vectors are read once per PyGraph (make a new PyGraph to
    /// see a later `embed`); the GIL is released while the query embeds.
    #[pyo3(signature = (query, top_k=20, semantic=None))]
    fn find_files(
        &self,
        py: Python<'_>,
        query: &str,
        top_k: usize,
        semantic: Option<bool>,
    ) -> PyResult<Py<PyAny>> {
        let source = if semantic == Some(false) {
            None
        } else if let Some(held) = self.semantic.get() {
            held.clone()
        } else {
            let dirs = worker_dirs(py);
            py.detach(|| {
                self.semantic
                    .get_or_init(|| {
                        semantic_source(&self.merged, &self.repo_roots, surface_model(), || {
                            surface_embedder(&dirs)
                        })
                    })
                    .clone()
            })
        };
        let answer = py
            .detach(|| {
                find_files_answer(
                    &self.merged,
                    &self.repo_roots,
                    self.rank.clone(),
                    source,
                    semantic == Some(true),
                    query,
                    top_k,
                    self.parse_errors.len(),
                )
            })
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }

    /// **find** (LD.3b): the ranked, located nodes whose name or qname
    /// `query` names, in one call — the one lookup (it replaced `find_node`
    /// and `find_nodes_by_qname`). Returns a dict `{results, absence}`:
    /// `results` is the records `{id, qname, name, kind, live, file, line,
    /// match}` (`live`: an entrypoint reaches the node, `false` = likely dead;
    /// `line` is 1-based; `match` names the tier that matched: `exact_qname`,
    /// `exact_name`, `exact_ci`, `qname_suffix`, `name_prefix`, `name_word`,
    /// `name_substring`, `qname_substring`, `subsequence`); `absence` is
    /// `None` when there are results, else a `no_match` dict with
    /// `unparsed_files` set to `len(parse_errors)`; with a repo root's
    /// vocabulary snapshot (`glia vocab sync`, FB.13c) it also carries
    /// `named_in`, `[{token, occurrences, files, file_count, common}]`, one
    /// per identifier of `query` a tracked file holds but no node is named.
    ///
    /// Rows rank by tier, then — inside `exact_qname` / `exact_name` — by the
    /// same key the seeded answers resolve a name with (a declaration before
    /// a container, then degree): when `query` is a node's exact qname or
    /// name, `find(query, top_k=1)['results'][0]` is the node
    /// `blast_radius(query)` starts from.
    ///
    /// `top_k` keeps the first rows (0 keeps every match). `kinds` keeps only
    /// nodes of those kind names (`["FUNCTION", "CLASS"]`, any case; `None` or
    /// `[]` keeps every kind); an unknown name raises ValueError listing the
    /// valid ones. `scope` (a repo-relative path or a project label, see
    /// `project_roots`) is a FILTER applied before `top_k`: a node whose file
    /// is outside it is dropped, one with no file is kept.
    ///
    /// `collapse` (DD.9, default `True`): a FACT / DERIVED identity class (one
    /// package, message or contract op in several repos or files) is ONE row,
    /// the best-ranked matching member, folded before `top_k` so a class never
    /// takes two slots; that row gains an `identity` dict `{tier, rules,
    /// diverged, also_in}` (`also_in`: `{repo, qname, file, line}` of every
    /// other member, `repo` a build label). Rows outside a class have no
    /// `identity` key. `collapse=False` is one row per member.
    #[pyo3(signature = (query, top_k=20, kinds=None, scope=None, collapse=true))]
    fn find(
        &self,
        py: Python<'_>,
        query: &str,
        top_k: usize,
        kinds: Option<Vec<String>>,
        scope: Option<String>,
        collapse: bool,
    ) -> PyResult<Py<PyAny>> {
        let collapse = if collapse {
            Collapse::with_labels(&self.repo_labels)
        } else {
            Collapse::off()
        };
        let answer = find_answer(
            &self.merged,
            self.live(),
            query,
            top_k,
            kinds.as_deref().unwrap_or_default(),
            scope,
            collapse,
            self.parse_errors.len(),
            &self.repo_roots,
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }

    /// **resolve** (P3, handoff v6): a failure/change signal (stacktrace, diff,
    /// test id, or `auto`-sniffed) → the ranked, LOCATED nodes it points at, in
    /// one call. Resolution order preserved. Returns a dict `{results,
    /// absence}` (LD.8a): `results` is the records
    /// `{id, qname, name, kind, score, live, file, line}` (`live`: an
    /// entrypoint reaches the node, `false` = likely dead; `line` is 1-based);
    /// `absence` is `None` when there are results, else the FACT-tier reason —
    /// `no_signal_match` (the signal resolved to no node; the note counts what
    /// it held) or `no_match` (`scope` or `top_k=0` removed every node) — with
    /// `unparsed_files` set to `len(parse_errors)`.
    ///
    /// `scope` (optional, default `None` = no-op; a path or a project label —
    /// see `project_roots`) filters the SEEDS before the
    /// PPR run, not the rendered result: frames resolve by file BASENAME, so an
    /// unscoped `utils.py` frame seeds every `utils.py` in the monorepo and
    /// those bogus seeds shape the scores of the real one. Scoped scores
    /// therefore legitimately differ from unscoped ones.
    #[pyo3(signature = (text, kind="auto", top_k=None, scope=None))]
    fn resolve(
        &self,
        py: Python<'_>,
        text: &str,
        kind: &str,
        top_k: Option<usize>,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let mut answer = glia_engine::resolve_signal_located_with_live(
            &self.merged,
            self.live(),
            text,
            kind,
            top_k,
            scope,
        );
        if let Some(a) = answer.absence.as_mut() {
            a.unparsed_files = self.parse_errors.len();
        }
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_engine::find::find_nodes;

    fn built(tag: &str) -> MergedGraph {
        let root = std::env::temp_dir().join(format!("glia-ld2-find-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("svc")).expect("temp dir");
        std::fs::write(
            root.join("app.py"),
            "class Helper:\n    pass\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n",
        )
        .expect("write fixture");
        std::fs::write(
            root.join("svc/other.py"),
            "def helper_two():\n    return 2\n",
        )
        .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        built.expect("build").merged
    }

    /// LD.2: pyo3 `find` is the engine's `find_nodes` — the same rows, in the
    /// same order, as `glia find` — with kind NAMES validated (any case), the
    /// scope a filter, and the parse-error count on an empty answer.
    #[test]
    fn find_is_the_engine_find_with_kind_names() {
        let merged = built("kinds");
        let live = glia_engine::entrypoint_reachable(&merged);
        let all = find_answer(
            &merged,
            &live,
            "helper",
            20,
            &[],
            None,
            Collapse::default(),
            0,
            &BTreeMap::new(),
        )
        .expect("answer");
        let engine = find_nodes(&merged, "helper", &FindOptions::default());
        assert_eq!(all.results, engine.results);
        assert_eq!(
            all.results.first().map(|r| r.qname.as_str()),
            Some("app::helper")
        );
        // LD.6: `main` calls `helper`, so both are live; the rows carry it.
        assert!(
            all.results.first().is_some_and(|r| r.live),
            "{:?}",
            all.results
        );

        let funcs = find_answer(
            &merged,
            &live,
            "helper",
            0,
            &["function".into()],
            None,
            Collapse::default(),
            0,
            &BTreeMap::new(),
        )
        .expect("answer");
        assert!(!funcs.results.is_empty());
        assert!(
            funcs.results.iter().all(|r| r.kind == "FUNCTION"),
            "{:?}",
            funcs.results
        );

        let bad = find_answer(
            &merged,
            &live,
            "helper",
            20,
            &["FUNCTOIN".into()],
            None,
            Collapse::default(),
            0,
            &BTreeMap::new(),
        );
        let err = bad.expect_err("an unknown kind name is an error");
        assert!(
            err.contains("unknown kind 'FUNCTOIN'") && err.contains("FUNCTION"),
            "{err}"
        );

        let scoped = find_answer(
            &merged,
            &live,
            "helper",
            0,
            &[],
            Some("svc".into()),
            Collapse::default(),
            0,
            &BTreeMap::new(),
        )
        .expect("answer");
        assert!(!scoped.results.is_empty());
        assert!(
            scoped
                .results
                .iter()
                .all(|r| r.file.as_deref().is_none_or(|f| f.starts_with("svc/"))),
            "scope is a filter: {:?}",
            scoped.results
        );

        let none = find_answer(
            &merged,
            &live,
            "zzqqxx",
            20,
            &[],
            None,
            Collapse::default(),
            7,
            &BTreeMap::new(),
        )
        .expect("answer");
        assert!(none.results.is_empty());
        let absence = none.absence.expect("an empty answer carries its absence");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_match", 7));
    }

    /// DD.9: `collapse=True` (the build's labels) folds a package two repos
    /// declare to one row whose `also_in` names the other repo by label;
    /// `collapse=False` is the two raw rows, with no `identity` key.
    #[test]
    fn collapse_folds_a_shared_package() {
        let root = std::env::temp_dir().join(format!("glia-dd9-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (repo, dep) in [("web", "left-pad"), ("admin", "typescript")] {
            std::fs::create_dir_all(root.join(repo)).expect("temp dir");
            std::fs::write(
                root.join(repo).join("package.json"),
                format!(
                    "{{\"name\": \"{repo}\", \"dependencies\": {{\"react\": \"^18.2.0\", \"{dep}\": \"1.0.0\"}}}}\n"
                ),
            )
            .expect("write manifest");
            std::fs::write(root.join(repo).join("index.js"), "export const x = 1;\n")
                .expect("write source");
        }
        let paths =
            [root.join("web"), root.join("admin")].map(|p| p.to_string_lossy().into_owned());
        let built = glia_engine::generate_many(&paths);
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");
        let live = glia_engine::entrypoint_reachable(&built.merged);
        let q = "package:npm:react";

        let on = Collapse::with_labels(&built.repo_labels);
        let one = find_answer(
            &built.merged,
            &live,
            q,
            20,
            &[],
            None,
            on,
            0,
            &BTreeMap::new(),
        )
        .expect("answer");
        assert_eq!(one.results.len(), 1, "{:?}", one.results);
        let id = one.results[0].identity.as_ref().expect("identity");
        assert_eq!(
            (id.tier, id.rules.as_slice()),
            ("fact", ["package_name"].as_slice())
        );
        assert_eq!(id.also_in.len(), 1, "{:?}", id.also_in);
        let labels: Vec<&str> = built.repo_labels.values().map(String::as_str).collect();
        assert!(
            labels.contains(&id.also_in[0].repo.as_str()),
            "{:?} vs {labels:?}",
            id.also_in
        );
        let text = serde_json::to_string(&one).expect("json");
        assert!(text.contains("\"identity\":{\"tier\":\"fact\""), "{text}");

        let two = find_answer(
            &built.merged,
            &live,
            q,
            20,
            &[],
            None,
            Collapse::off(),
            0,
            &BTreeMap::new(),
        )
        .expect("answer");
        assert_eq!(two.results.len(), 2, "{:?}", two.results);
        assert!(
            !serde_json::to_string(&two)
                .expect("json")
                .contains("\"identity\"")
        );

        // EB.8: find_files ranks one repo's files; two repos are refused.
        let cache = Arc::new(RankCache::new(
            "pygraph",
            &built.repo_roots,
            &built.repo_labels,
        ));
        let err = find_files_answer(
            &built.merged,
            &built.repo_roots,
            cache,
            None,
            false,
            "T012",
            20,
            0,
        )
        .expect_err("two repos");
        assert_eq!(err, "find --files ranks one repo's files; got 2 repos");
    }

    fn engine_with_errors(built: &glia_engine::GenerateResult, n: usize) -> Answer<FoundFile> {
        let opts = FindFilesOptions::default();
        let mut x = find_files(&built.merged, &built.repo_roots, "export the ledger", &opts)
            .expect("answer");
        if let Some(a) = x.absence.as_mut() {
            a.unparsed_files = n;
        }
        x
    }

    /// EB.8: `find_files` on the held cache is the engine's answer; with no
    /// history snapshot and no id it is an absence naming both, with the
    /// parse-error count.
    #[test]
    fn find_files_is_the_engine_answer_on_the_held_cache() {
        let root = std::env::temp_dir().join(format!("glia-eb8-find-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(root.join("app.py"), "def helper(x):\n    return x\n")
            .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");
        let held = Arc::new(RankCache::new(
            "pygraph",
            &built.repo_roots,
            &built.repo_labels,
        ));
        let a = find_files_answer(
            &built.merged,
            &built.repo_roots,
            held.clone(),
            None,
            false,
            "export the ledger",
            20,
            3,
        )
        .expect("answer");
        assert!(a.results.is_empty());
        let ab = a.absence.expect("absence");
        assert_eq!((ab.reason, ab.unparsed_files), ("no_match", 3));
        assert!(
            ab.note
                .contains("no id in the query resolved to a defining row"),
            "{}",
            ab.note
        );
        assert!(
            ab.note
                .contains("no history messages (run glia history sync)"),
            "{}",
            ab.note
        );
        let again = find_files_answer(
            &built.merged,
            &built.repo_roots,
            held,
            None,
            false,
            "export the ledger",
            20,
            3,
        )
        .expect("answer");
        assert_eq!(
            serde_json::to_string(&again).expect("json"),
            serde_json::to_string(&engine_with_errors(&built, 3)).expect("json")
        );
    }

    fn test_hash() -> Result<Arc<dyn glia_embed::Embedder>, String> {
        crate::embed::embedder_for("test-hash", &[])
    }

    /// FB.7b: no sidecar is no source (nothing read) and the plain answer;
    /// `semantic=True` without vectors marks the fallback `no_vectors`; after
    /// a test-hash embed the source loads, the order is the fused one and
    /// rows carry `sem`; vectors of another model are no source.
    #[test]
    fn find_files_blends_the_vectors_when_the_repo_has_them() {
        let root =
            std::env::temp_dir().join(format!("glia-fb7b-find-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(
            root.join("ledger.py"),
            "def export_ledger(rows):\n    \"\"\"Export the ledger rows.\"\"\"\n    return rows\n",
        )
        .expect("write fixture");
        std::fs::write(root.join("cache.py"), "def warm_cache():\n    return 1\n")
            .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let built = built.expect("build");
        let held = || {
            Arc::new(RankCache::new(
                "pygraph",
                &built.repo_roots,
                &built.repo_labels,
            ))
        };
        let q = "export the ledger";
        assert!(
            semantic_source(&built.merged, &built.repo_roots, "test-hash", test_hash).is_none()
        );
        let plain = find_files_answer(
            &built.merged,
            &built.repo_roots,
            held(),
            None,
            false,
            q,
            20,
            0,
        )
        .expect("answer");
        let wanted = find_files_answer(
            &built.merged,
            &built.repo_roots,
            held(),
            None,
            true,
            q,
            20,
            0,
        )
        .expect("answer");
        let sel = wanted.selection.as_ref().expect("selection");
        assert_eq!(
            (sel.pick.as_str(), sel.source, sel.reason),
            ("LX[IDR,T1s]", "fallback", Some(NO_VECTORS))
        );
        assert_eq!(
            plain.selection.as_ref().and_then(|s| s.reason),
            Some("absent")
        );

        let (&repo, _) = built.repo_roots.iter().next().expect("one repo");
        let model = glia_embed::named("test-hash").expect("test model");
        glia_engine::semantic::embed_repo(
            &root,
            &built.merged,
            glia_core::RepoId(repo),
            model.as_ref(),
            &glia_engine::semantic::EmbedOptions::default(),
        )
        .expect("embed");
        // bge-small is the surface model: the test-hash rows are not its.
        assert!(
            semantic_source(&built.merged, &built.repo_roots, "bge-small", test_hash).is_none()
        );
        let src = semantic_source(&built.merged, &built.repo_roots, "test-hash", test_hash)
            .expect("a source over the test-hash vectors");
        let fused = find_files_answer(
            &built.merged,
            &built.repo_roots,
            held(),
            Some(src),
            true,
            q,
            20,
            0,
        )
        .expect("answer");
        let _ = std::fs::remove_dir_all(&root);
        let sel = fused.selection.as_ref().expect("selection");
        assert_eq!(
            (sel.pick.as_str(), sel.source, sel.reason),
            ("LX[IDR,FUSE[T1s,SEMp:t2]]", "fallback", Some("absent"))
        );
        assert!(
            fused.results.iter().any(|r| r.sem.is_some()),
            "{:?}",
            fused.results
        );
        let text = serde_json::to_string(&fused).expect("json");
        assert!(text.contains(",\"sem\":"), "{text}");
        assert!(
            !serde_json::to_string(&plain)
                .expect("json")
                .contains("\"sem\"")
        );
    }
}
