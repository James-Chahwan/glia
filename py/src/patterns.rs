//! **patterns** (LE.7b, EXPERIMENTAL): the pattern-conformance engine
//! (`repo_graph_engine::patterns`, LE.7a) — route handlers grouped per
//! service, each handler's role chain to its first effect sink as a
//! signature, a population's most frequent signature as its convention, and
//! every handler off it a located DIVERGENCE (tier heuristic).
//!
//! Both names carry `experimental`, so no consumer adopts the answer by
//! accident before it is promoted (a promotion is a rename):
//! `PyGraph.patterns_experimental(..)` judges a built graph;
//! the module function `patterns_vs_rev_experimental(repo_path, base="HEAD",
//! ..)` builds the working tree and the rev itself (LE.1b's graph delta) and
//! lists only the divergences the change touched. Each returns the engine's
//! `PatternReport` as a native dict (LD.2); a `min_share` above 100 or an
//! engine `Err` raises `ValueError`.
//!
//! Transport only: populations, signatures, verdicts and locations live in
//! the engine. The helpers the pyo3 entry points delegate to are pyo3-free,
//! so `cargo test -p repo-graph-py` covers them (see the crate doc); they
//! print this surface's marker,
//! `[patterns] experimental surface=pyo3 mode=<graph|delta>`.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_engine::delta::graph_delta_vs_rev;
use repo_graph_engine::patterns::{
    PatternArgs, PatternReport, pattern_conformance, pattern_conformance_delta,
};
use repo_graph_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The engine's options from the keyword arguments; `Err` names a
/// `min_share` that is no percentage.
fn options(min_support: usize, min_share: usize, scope: Option<&str>) -> Result<PatternArgs, String> {
    if min_share > 100 {
        return Err(format!("min_share is a percentage (0-100), got {min_share}"));
    }
    let mut p = PatternArgs::default();
    p.min_support = min_support;
    p.min_share_pct = min_share;
    p.scope = scope.map(str::to_string);
    Ok(p)
}

/// The body of [`PyGraph::patterns_experimental`], minus pyo3.
fn graph_report(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
) -> Result<PatternReport, String> {
    let p = options(min_support, min_share, scope)?;
    let report = pattern_conformance(merged, repo_labels, &p);
    eprintln!("[patterns] experimental surface=pyo3 mode=graph");
    Ok(report)
}

/// The body of [`patterns_vs_rev_experimental`], minus pyo3. `Err` is a bad
/// `min_share` or the engine's message (not a directory, git missing, not a
/// git work tree, an unknown rev, a failed build).
fn rev_report(
    repo_path: &str,
    base: &str,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
) -> Result<PatternReport, String> {
    let p = options(min_support, min_share, scope)?;
    let d = graph_delta_vs_rev(repo_path, base)?;
    let report = pattern_conformance_delta(&d.after.merged, &d.after.repo_labels, &d.delta, &p);
    eprintln!("[patterns] experimental surface=pyo3 mode=delta");
    Ok(report)
}

#[pymethods]
impl PyGraph {
    /// **patterns_experimental** (LE.7b, EXPERIMENTAL: the name changes when
    /// the engine is promoted): pattern conformance over this graph. The
    /// route handlers of each service (the services `service_map` shows) form
    /// a population; each handler's signature is its role chain to the first
    /// effect sink (`handler>service>repository>db`, `handler>(no effect)`
    /// when the graph follows none); a population of at least `min_support`
    /// handlers whose most frequent sink-reaching signature holds `min_share`
    /// percent of it has that signature as its convention, and every handler
    /// off it is a DIVERGENCE (tier `heuristic`: observed, never a rule).
    /// `scope` (a path or project label) keeps only the handlers located
    /// under it.
    ///
    /// Returns a dict `{experimental, delta_mode, handlers, judged,
    /// skipped_small, excluded, role_sources, populations, divergences}`:
    /// `experimental` is always True; `excluded` counts the handlers left out
    /// by reason (`test_fixture`, `generated`, `generated_proto`, `unplaced`,
    /// `out_of_scope`); each population `{service, role, size, status,
    /// convention, matching, verdict, signatures, role_sources, exceptions}`
    /// has `status` `judged` | `no_convention` | `too_small`; each divergence
    /// `{verdict, tier, service, handler, file, line, route_method,
    /// route_path, signature, convention, matching, population, path,
    /// role_sources}` is located (lines 1-based), `path` its hops to the sink.
    /// Raises ValueError on a `min_share` above 100.
    #[pyo3(signature = (min_support=5, min_share=75, scope=None))]
    fn patterns_experimental(
        &self,
        py: Python<'_>,
        min_support: usize,
        min_share: usize,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let report = graph_report(&self.merged, &self.repo_labels, min_support, min_share, scope)
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&report))
    }
}

/// **patterns_vs_rev_experimental** (LE.7b, EXPERIMENTAL): pattern
/// conformance on the working tree's change against git rev `base` (a branch,
/// tag, sha or `"HEAD~N"`; default `"HEAD"`) of the repo at `repo_path`.
/// Populations and conventions come from the whole working-tree graph;
/// `divergences` lists only the exceptions the change touched (the handler or
/// a node on its path added, modified or moved, or a hop of its path an added
/// edge), while each population's `exceptions` still lists them all. Same
/// keywords and dict as `PyGraph.patterns_experimental`, with `delta_mode`
/// True. Builds both sides itself and saves the parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as
/// `generate(incremental=True)` does, never a `.gmap` layout. Raises
/// ValueError on a `min_share` above 100 or a git or build failure.
#[pyfunction]
#[pyo3(signature = (repo_path, base="HEAD", min_support=5, min_share=75, scope=None))]
fn patterns_vs_rev_experimental(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let report = rev_report(repo_path, base, min_support, min_share, scope)
        .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&report))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(patterns_vs_rev_experimental, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "patterns", add: register } }

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

    /// `(name, route, service fn or None for a direct call, repository fn)`
    /// per gin handler; the last one skips the service layer.
    const HANDLERS: [(&str, &str, Option<&str>, &str); 6] = [
        ("GetUserHandler", "GET(\"/users/:id\"", Some("GetUser"), "FindUser"),
        ("CreateUserHandler", "POST(\"/users\"", Some("CreateUser"), "InsertUser"),
        ("GetOrderHandler", "GET(\"/orders/:id\"", Some("GetOrder"), "FindOrder"),
        ("ListProductsHandler", "GET(\"/products\"", Some("ListProducts"), "AllProducts"),
        ("CreatePaymentHandler", "POST(\"/payments\"", Some("CreatePayment"), "InsertPayment"),
        ("RawOrderHandler", "POST(\"/orders\"", None, "SaveOrder"),
    ];
    const CONVENTION: &str = "handler>service>repository>db";
    const DIRECT: &str = "handlers::handlers::RawOrderHandler";

    /// The Go shop of `engine/tests/patterns.rs` with the first `n` handlers:
    /// gin routes in `handlers/handlers.go`, service functions in
    /// `service/service.go`, raw SQL in `repository/repository.go`.
    fn write_shop(top: &Path, n: usize) {
        let hs = &HANDLERS[..n];
        let mut handlers = String::from(
            "package handlers\n\nimport (\n\t\"net/http\"\n\n\t\"example.com/shop/repository\"\n\t\"example.com/shop/service\"\n\t\"github.com/gin-gonic/gin\"\n)\n\nfunc Register(r *gin.Engine) {\n",
        );
        for (name, route, _, _) in hs {
            handlers.push_str(&format!("\tr.{route}, {name})\n"));
        }
        handlers.push_str("}\n");
        let mut service = String::from("package service\n\nimport \"example.com/shop/repository\"\n");
        let mut repository =
            String::from("package repository\n\nimport \"database/sql\"\n\nvar db *sql.DB\n");
        for (i, (name, _, svc, repo)) in hs.iter().enumerate() {
            let callee = match svc {
                Some(f) => {
                    service.push_str(&format!(
                        "\nfunc {f}(id string) string {{\n\treturn repository.{repo}(id)\n}}\n"
                    ));
                    format!("service.{f}")
                }
                None => format!("repository.{repo}"),
            };
            handlers.push_str(&format!(
                "\nfunc {name}(c *gin.Context) {{\n\tv := {callee}(c.Param(\"id\"))\n\tc.JSON(http.StatusOK, v)\n}}\n"
            ));
            repository.push_str(&format!(
                "\nfunc {repo}(id string) string {{\n\tdb.Exec(\"INSERT INTO t{i} (v) VALUES ($1)\", id)\n\treturn id\n}}\n"
            ));
        }
        if !hs.iter().any(|h| h.2.is_none()) {
            handlers = handlers.replace("\t\"example.com/shop/repository\"\n", "");
        }
        for (rel, text) in [
            ("go.mod", "module example.com/shop\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.1\n".to_string()),
            ("handlers/handlers.go", handlers),
            ("service/service.go", service),
            ("repository/repository.go", repository),
        ] {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("scratch dir");
            std::fs::write(&p, text).expect("write fixture");
        }
    }

    fn shop(name: &str, n: usize) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-le7b-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        write_shop(&top, n);
        (Scratch(root), top)
    }

    /// The object every entry point returns, as `to_py` hands it to Python.
    fn value(r: &PatternReport) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(r).expect("serialises")).expect("valid JSON")
    }

    fn divergent(v: &serde_json::Value) -> Vec<String> {
        v["divergences"]
            .as_array()
            .expect("divergences")
            .iter()
            .map(|d| d["handler"].as_str().unwrap_or("").to_string())
            .collect()
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
            .unwrap_or_else(|e| panic!("LE.7b patterns_vs_rev_experimental test needs a `git` binary: {e}"));
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// LE.7b: the helper behind `PyGraph.patterns_experimental` returns the
    /// engine's report with the documented keys, `experimental` true, the
    /// one located divergence, and the keywords reach the engine. The
    /// conformance itself is covered by `engine/tests/patterns.rs`.
    #[test]
    fn graph_helper_returns_the_documented_object() {
        let (_scratch, top) = shop("graph", 6);
        let built = repo_graph_engine::generate_one(top.to_str().expect("utf-8")).expect("build");
        let r = graph_report(&built.merged, &built.repo_labels, 5, 75, None).expect("report");
        let v = value(&r);
        for key in ["experimental", "populations", "divergences", "skipped_small", "delta_mode"] {
            assert!(v.get(key).is_some(), "{key} missing: {v}");
        }
        let text = serde_json::to_string(&r).expect("serialises");
        let order: Vec<usize> = [
            "\"experimental\":",
            "\"delta_mode\":",
            "\"handlers\":",
            "\"judged\":",
            "\"skipped_small\":",
            "\"excluded\":",
            "\"role_sources\":",
            "\"populations\":",
            "\"divergences\":",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        assert_eq!(v["experimental"], true);
        assert_eq!(v["delta_mode"], false);
        assert_eq!(v["skipped_small"], 0);
        assert_eq!(divergent(&v), [DIRECT]);
        let p = &v["populations"][0];
        assert_eq!((p["status"].as_str(), p["verdict"].as_str()), (Some("judged"), Some("5/6")), "{p}");
        assert_eq!(p["convention"], CONVENTION);
        assert_eq!(v["divergences"][0]["file"], "handlers/handlers.go");
        assert_eq!(v["divergences"][0]["route_path"], "/orders");

        // min_support above the population: too small, nothing judged.
        let small = value(&graph_report(&built.merged, &built.repo_labels, 7, 75, None).expect("report"));
        assert_eq!((small["skipped_small"].as_u64(), small["judged"].as_u64()), (Some(1), Some(0)));
        assert_eq!(small["populations"][0]["status"], "too_small");
        assert!(divergent(&small).is_empty());
        // A 5/6 share is below 90%: no convention.
        let split = value(&graph_report(&built.merged, &built.repo_labels, 5, 90, None).expect("report"));
        assert_eq!(split["populations"][0]["status"], "no_convention");
        assert!(divergent(&split).is_empty());
        // A scope no handler sits under: every handler excluded.
        let scoped = value(&graph_report(&built.merged, &built.repo_labels, 5, 75, Some("service")).expect("report"));
        assert_eq!(scoped["excluded"]["out_of_scope"], 6, "{scoped}");
        assert_eq!(scoped["handlers"], 0);

        let err = graph_report(&built.merged, &built.repo_labels, 5, 101, None).expect_err("bad share");
        assert!(err.contains("101"), "{err}");
    }

    /// LE.7b: the helper behind `patterns_vs_rev_experimental` judges the
    /// working tree and lists only the divergence the change added; an
    /// unknown rev is the engine's error.
    #[test]
    fn rev_helper_lists_the_touched_divergence() {
        let (scratch, top) = shop("rev", 5);
        let gitconfig = scratch.0.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "five layered handlers"]);
        write_shop(&top, 6);
        let repo = top.to_str().expect("utf-8");
        let v = value(&rev_report(repo, "HEAD", 5, 75, None).expect("report"));
        assert_eq!((v["experimental"].as_bool(), v["delta_mode"].as_bool()), (Some(true), Some(true)));
        assert_eq!(divergent(&v), [DIRECT]);
        assert_eq!(v["populations"][0]["verdict"], "5/6");

        let err = rev_report(repo, "no-such-rev", 5, 75, None).expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
        let err = rev_report(repo, "HEAD", 5, 200, None).expect_err("bad share");
        assert!(err.contains("200"), "{err}");
    }
}
