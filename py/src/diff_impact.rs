//! **diff_impact** (LE.2): what a change affects, in one call — the changed
//! nodes of a change and ONE multi-seed, located, ranked blast radius around
//! them, over the engine's `diff_impact::{diff_impact_from_diff,
//! diff_impact_vs_rev}`.
//!
//! `PyGraph.diff_impact(diff_text, ..)` answers a pasted unified diff (or a
//! changed-file list) over a built graph; the module function
//! `diff_impact_vs_rev(repo_path, base="HEAD", ..)` builds the repo's working
//! tree and the rev itself (LE.1b's graph delta), so it takes a path. Each
//! returns the engine's answer as a native dict (LD.2); a bad `direction` or
//! an engine `Err` raises `ValueError`. A row standing for an identity class
//! names its other members' repos by the build's labels (DD.10), as
//! `review_vs_rev` does.
//!
//! Transport only: the seeds, the radius, the location of every row and the
//! `[diff-impact] mode=..` marker live in the engine. The helpers the pyo3
//! entry points delegate to are pyo3-free, so `cargo test -p glia-py`
//! covers them (see the crate doc).

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::BlastOptions;
use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::diff_impact::{DiffImpact, diff_impact_from_delta, diff_impact_from_diff};
use glia_engine::identity::Collapse;
use glia_engine::rank::{RankCache, Ranking};
use glia_graph::MergedGraph;

use crate::blast::{ranking_selected, reach_named};
use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The 0.5.3 ranking keywords of both entry points (EA.7): the file ranking
/// and whether the calibration selects it ([`ranking_selected`], EC.8), how many files `impact.files` lists, and the held
/// ranking cache (`PyGraph`'s; `None` for `diff_impact_vs_rev`, whose engine
/// call builds one over the working tree's roots, EA.5).
#[derive(Clone)]
struct RankKw {
    ranking: Ranking,
    select: bool,
    top_files: usize,
    cache: Option<Arc<RankCache>>,
}

impl Default for RankKw {
    /// The pyo3 signature's defaults: `ranking=None, cochange=True,
    /// top_files=10` (the calibrated pick, else `ppr`), no held cache.
    fn default() -> Self {
        let blast = BlastOptions::default();
        RankKw { ranking: blast.ranking, select: blast.select, top_files: blast.top_files, cache: None }
    }
}

impl RankKw {
    /// The keywords as given; `Err` names an unknown `ranking`.
    fn named(ranking: Option<&str>, cochange: bool, top_files: usize, cache: Option<Arc<RankCache>>) -> Result<Self, String> {
        let (ranking, select) = ranking_selected(ranking, cochange)?;
        Ok(RankKw { ranking, select, top_files, cache })
    }
}

/// The engine's options from the keyword arguments both entry points take;
/// `Err` names a bad `direction`. The caller sets the collapse's repo labels
/// from its build ([`labelled`]).
fn options(
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
    rank: &RankKw,
) -> Result<BlastOptions, String> {
    let mut opts = BlastOptions::default();
    opts.direction = reach_named(direction)?;
    opts.depth = depth;
    opts.top_k = top_k;
    opts.live_only = live_only;
    opts.scope = scope.map(str::to_string);
    opts.ranking = rank.ranking;
    opts.select = rank.select;
    opts.top_files = rank.top_files;
    opts.rank_cache = rank.cache.clone();
    Ok(opts)
}

/// `opts` collapsing with the build's repo `labels`, so a collapsed row's
/// `also_in` names each repo by its label, not `repo#<id % 1000>` (DD.10).
fn labelled(mut opts: BlastOptions, labels: &BTreeMap<u64, String>) -> BlastOptions {
    opts.collapse = Collapse::with_labels(labels);
    opts
}

/// The body of [`PyGraph::diff_impact`], minus pyo3; `labels` are the
/// graph's (`PyGraph::repo_labels`).
#[allow(clippy::too_many_arguments)]
fn diff_answer(
    merged: &MergedGraph,
    labels: &BTreeMap<u64, String>,
    diff_text: &str,
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
    rank: &RankKw,
) -> Result<DiffImpact, String> {
    let opts = labelled(options(direction, depth, top_k, live_only, scope, rank)?, labels);
    Ok(diff_impact_from_diff(merged, diff_text, &opts))
}

/// The body of [`diff_impact_vs_rev_py`], minus pyo3: the engine's
/// `diff_impact_vs_rev` in its two steps, so the radius gets the working
/// tree's labels between them.
#[allow(clippy::too_many_arguments)]
fn rev_answer(
    repo_path: &str,
    base: &str,
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
    rank: &RankKw,
) -> Result<DiffImpact, String> {
    let opts = options(direction, depth, top_k, live_only, scope, rank)?;
    let rev = graph_delta_vs_rev(repo_path, base)?;
    Ok(diff_impact_from_delta(&rev, &labelled(opts, &rev.after.repo_labels)))
}

#[pymethods]
impl PyGraph {
    /// **diff_impact** (LE.2): what the change a unified diff describes
    /// affects, over this graph (the diff's new side). The narrowest node
    /// spanning each added line (a changed-file list: every node of each
    /// file) seeds ONE blast radius, `blast_radius`'s walk and ranking over
    /// all the seeds at once. `direction` ∈ {`forward`, `backward`, `both`},
    /// `depth` hops, `top_k` rows, `live_only` and `scope` as in
    /// `blast_radius`.
    ///
    /// Returns a dict `{base, changed, edges_added, edges_removed, impact,
    /// unresolved_diff_files}`: `base` is `None` here; `changed` rows `{id,
    /// qname, kind, file, line, change, seed}` with `change` `diff_hit` and
    /// `seed` false for a module beside a finer changed node of its file;
    /// `impact` is `blast_radius`'s dict, each result carrying the `seed`
    /// whose wave reached it first; `unresolved_diff_files` names the diff's
    /// files that place no node (a hunk that only deletes lines, a deleted
    /// file, an unparsed file). The edge lists are empty in this mode. Lines
    /// are 1-based. `ranking`, `cochange` and `top_files` (0.5.3) order the
    /// impact rows and fill `impact.files` as in `blast_radius`, over this
    /// PyGraph's held ranking indexes; `ranking=None` (the default, EC.8)
    /// applies the repo's calibrated pick, else `ppr`, and `impact` gains
    /// `selection`. Raises ValueError on a bad `direction` or `ranking`.
    #[pyo3(signature = (diff_text, direction="both", depth=4, top_k=None, live_only=false, scope=None, ranking=None, cochange=true, top_files=10))]
    #[allow(clippy::too_many_arguments)]
    fn diff_impact(
        &self,
        diff_text: &str,
        direction: &str,
        depth: usize,
        top_k: Option<usize>,
        live_only: bool,
        scope: Option<&str>,
        ranking: Option<&str>,
        cochange: bool,
        top_files: usize,
    ) -> PyResult<Py<PyAny>> {
        let rank = RankKw::named(ranking, cochange, top_files, Some(self.rank.clone())).map_err(PyValueError::new_err)?;
        let answer = diff_answer(&self.merged, &self.repo_labels, diff_text, direction, depth, top_k, live_only, scope, &rank)
            .map_err(PyValueError::new_err)?;
        // The token comes from `attach` (the GIL is already held), not a
        // parameter: the keyword list is the Python surface.
        Python::attach(|py| to_py(py, serde_json::to_string(&answer)))
    }
}

/// **diff_impact_vs_rev** (LE.2): `PyGraph.diff_impact` for the working
/// tree's change against git rev `base` (a branch, tag, sha or `"HEAD~N"`;
/// default `"HEAD"`) of the repo at `repo_path`. Seeds: the added, moved and
/// edited nodes and the surviving ends of every call-like edge gained or
/// lost (a caller whose call was deleted, a hunk that only deletes lines).
/// `changed` carries LE.1b's `added` / `removed` / `modified` / `moved` rows
/// (a removed row located in the base rev, never a seed) and the
/// `edge_endpoint` seeds; `edges_added` / `edges_removed` the delta's edge
/// rows. Builds both sides itself and saves the parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as
/// `generate(incremental=True)` does, never a `.gmap` layout. Same keywords
/// and dict as `PyGraph.diff_impact` (its ranking indexes built per call over
/// the working tree's repo roots, so its history snapshot orders the impact);
/// raises ValueError on a bad `direction` or `ranking` (before any build) or
/// a git or build failure.
#[pyfunction]
#[pyo3(name = "diff_impact_vs_rev", signature = (repo_path, base="HEAD", direction="both", depth=4, top_k=None, live_only=false, scope=None, ranking=None, cochange=true, top_files=10))]
#[allow(clippy::too_many_arguments)]
fn diff_impact_vs_rev_py(
    repo_path: &str,
    base: &str,
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
    ranking: Option<&str>,
    cochange: bool,
    top_files: usize,
) -> PyResult<Py<PyAny>> {
    let rank = RankKw::named(ranking, cochange, top_files, None).map_err(PyValueError::new_err)?;
    let answer = rev_answer(repo_path, base, direction, depth, top_k, live_only, scope, &rank)
        .map_err(PyValueError::new_err)?;
    Python::attach(|py| to_py(py, serde_json::to_string(&answer)))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(diff_impact_vs_rev_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "diff_impact", add: register } }

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const A_PY: &str = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n";
    const B_PY: &str = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n";
    const DIFF: &str = "--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,2 +1,2 @@\n def price(o):\n-    return o\n+    return o or 0\n";

    /// `shop/a.py` (`place` calls `price`) and `shop/b.py` (`checkout` calls
    /// `place`), under a fresh scratch dir.
    fn shop(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-le2-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        for (rel, text) in [("shop/a.py", A_PY), ("shop/b.py", B_PY)] {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("scratch dir");
            std::fs::write(&p, text).expect("write fixture");
        }
        (Scratch(root), top)
    }

    /// The object every entry point returns, as `to_py` hands it to Python.
    fn value(a: &DiffImpact) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(a).expect("serialises")).expect("valid JSON")
    }

    /// `(qname, depth, seed)` per impact row.
    fn rows(v: &serde_json::Value) -> Vec<(String, u64, String)> {
        let mut out: Vec<(String, u64, String)> = v["impact"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|r| {
                (
                    r["qname"].as_str().unwrap_or("").to_string(),
                    r["depth"].as_u64().unwrap_or(99),
                    r["seed"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn git(top: &Path, gitconfig: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .arg("-C")
            .arg(top)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("LE.2 diff_impact_vs_rev test needs a `git` binary: {e}"));
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// LE.2: the helper behind pyo3 `diff_impact` returns the documented
    /// object in field order, the keywords reach the engine, and a bad
    /// direction is an error. The seeding itself is covered by
    /// `engine/tests/diff_impact.rs`.
    #[test]
    fn diff_helper_returns_the_documented_object() {
        let (_scratch, top) = shop("diff");
        let built = glia_engine::generate_one(top.to_str().expect("utf-8")).expect("build");
        let a = diff_answer(&built.merged, &built.repo_labels, DIFF, "backward", 4, None, false, None, &RankKw::default()).expect("answer");
        let text = serde_json::to_string(&a).expect("serialises");
        let order: Vec<usize> = [
            "\"base\":",
            "\"changed\":",
            "\"edges_added\":",
            "\"edges_removed\":",
            "\"impact\":",
            "\"unresolved_diff_files\":",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        let v = value(&a);
        assert!(v["base"].is_null());
        assert_eq!(v["changed"][0]["qname"], "shop::a::price");
        assert_eq!((v["changed"][0]["change"].as_str(), v["changed"][0]["seed"].as_bool()), (Some("diff_hit"), Some(true)));
        assert_eq!(
            rows(&v),
            [
                ("shop::a::place".to_string(), 1, "shop::a::price".to_string()),
                ("shop::b::checkout".to_string(), 2, "shop::a::price".to_string()),
            ]
        );
        let cut = diff_answer(&built.merged, &built.repo_labels, DIFF, "backward", 1, None, false, None, &RankKw::default()).expect("depth 1");
        assert_eq!(rows(&value(&cut)), [("shop::a::place".to_string(), 1, "shop::a::price".to_string())]);
        let fwd = diff_answer(&built.merged, &built.repo_labels, DIFF, "forward", 4, None, false, None, &RankKw::default()).expect("forward");
        assert!(fwd.impact.results.is_empty() && fwd.impact.absence.is_some());
        let err = diff_answer(&built.merged, &built.repo_labels, DIFF, "sideways", 4, None, false, None, &RankKw::default()).expect_err("bad direction");
        assert!(err.contains("sideways"), "{err}");
        let o = options("both", 2, Some(3), true, Some("shop"), &RankKw::default()).expect("options");
        assert_eq!((o.depth, o.top_k, o.live_only, o.scope.as_deref()), (2, Some(3), true, Some("shop")));
        assert_eq!((o.ranking, o.select, o.top_files, o.rank_cache.is_none()), (Ranking::default(), true, 10, true));
        // EA.7: the ranking keywords reach the engine's options.
        let held = Arc::new(RankCache::new("pygraph", &built.repo_roots, &built.repo_labels));
        let rank = RankKw::named(Some("bfs"), false, 3, Some(held.clone())).expect("bfs");
        let o = options("both", 2, None, false, None, &rank).expect("options");
        assert_eq!((o.ranking.harness_name(), o.select, o.top_files), ("B0", false, 3));
        assert!(o.rank_cache.as_ref().is_some_and(|c| Arc::ptr_eq(c, &held)));
        let err = RankKw::named(Some("x"), true, 10, None).err().expect("x is no ranking");
        assert_eq!(err, "unknown ranking `x`: expected one of ppr, ppr-wide, bfs");
    }

    /// LE.2: the helper behind the module function `diff_impact_vs_rev`
    /// seeds from the working tree's change, and an unknown rev is the
    /// engine's error.
    #[test]
    fn rev_helper_seeds_from_the_working_tree() {
        let (scratch, top) = shop("rev");
        let gitconfig = scratch.0.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "shop"]);
        std::fs::write(top.join("shop/a.py"), A_PY.replace("return o\n", "return o or 0\n")).expect("edit");
        let repo = top.to_str().expect("utf-8");
        let a = rev_answer(repo, "HEAD", "backward", 4, None, false, None, &RankKw::default()).expect("answer");
        let v = value(&a);
        assert_eq!(v["base"], "HEAD");
        assert_eq!(
            rows(&v),
            [
                ("shop::a::place".to_string(), 1, "shop::a::price".to_string()),
                ("shop::b::checkout".to_string(), 2, "shop::a::price".to_string()),
            ]
        );
        let err = rev_answer(repo, "no-such-rev", "both", 4, None, false, None, &RankKw::default()).expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
    }

    /// The `also_in` repos of the impact's DOC_SECTION row (the class of the
    /// three `GET /api/orders` contract ops of identity-contract-ops).
    fn also_in_repos(v: &serde_json::Value) -> Vec<String> {
        let row = v["impact"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .find(|r| r["kind"] == "DOC_SECTION")
            .unwrap_or_else(|| panic!("a DOC_SECTION row: {v}"));
        row["identity"]["also_in"]
            .as_array()
            .unwrap_or_else(|| panic!("the class row carries also_in: {row}"))
            .iter()
            .map(|a| a["repo"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// DD.15 (DD.10's handoff): both entry points collapse with the build's
    /// repo labels, so `also_in` names the repo (`repo`), never the
    /// `repo#<id % 1000>` fallback the unlabelled default gave.
    #[test]
    fn identity_rows_name_the_repo() {
        let root = std::env::temp_dir().join(format!("glia-dd15-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = Scratch(root.clone());
        let top = root.join("repo");
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures/identity-contract-ops");
        for rel in ["go.mod", "main.go", "docs/swagger.yaml", "features/orders/feature.yaml"] {
            let to = top.join(rel);
            std::fs::create_dir_all(to.parent().expect("parent")).expect("scratch dir");
            std::fs::copy(src.join(rel), &to).expect("copy fixture file");
        }
        let main = std::fs::read_to_string(top.join("main.go")).expect("main.go");
        let at = main.find("\tc.JSON(200").expect("listOrders' body");
        let line = main[..at].lines().count() + 1;
        let added = "\tc.Header(\"X-Total\", \"0\")\n";
        let hunk = format!("--- a/main.go\n+++ b/main.go\n@@ -{line},2 +{line},3 @@\n+{added} \tc.JSON(200, []string{{}})\n }}\n");
        let repo = top.to_str().expect("utf-8");

        let built = glia_engine::generate_one(repo).expect("build");
        let a = diff_answer(&built.merged, &built.repo_labels, &hunk, "both", 4, None, false, None, &RankKw::default()).expect("answer");
        assert_eq!(also_in_repos(&value(&a)), ["repo", "repo"]);

        let gitconfig = scratch.0.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "orders"]);
        std::fs::write(top.join("main.go"), format!("{}{added}{}", &main[..at], &main[at..])).expect("edit");
        let a = rev_answer(repo, "HEAD", "both", 4, None, false, None, &RankKw::default()).expect("answer");
        assert_eq!(also_in_repos(&value(&a)), ["repo", "repo"]);
    }
}
