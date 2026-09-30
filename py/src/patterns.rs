//! **patterns** (LE.7b, promoted out of experimental by CC.12b): the
//! pattern-conformance engine (`glia_engine::patterns`, LE.7a) — route
//! handlers grouped per service, each handler's role chain to its first
//! effect sink as a signature, a population's most frequent signature as its
//! convention, and every handler off it a located DIVERGENCE (tier
//! heuristic).
//!
//! `PyGraph.patterns(..)` judges a built graph; the module function
//! `patterns_vs_rev(repo_path, base="HEAD", ..)` builds the working tree and
//! the rev itself (LE.1b's graph delta) and lists only the divergences the
//! change touched. Each returns the engine's `PatternReport` as a native dict
//! (LD.2); a `min_share` above 100, a `group_by` other than `"service"` /
//! `"package"` (CA.5b) or an engine `Err` raises `ValueError`.
//!
//! 0.5.0 named them `patterns_experimental` / `patterns_vs_rev_experimental`
//! so no consumer adopted the answer before the engine met its promotion
//! criterion (the engine module docs, "Promotion criterion"). Those names stay
//! until 0.5.2 as aliases: the same parameters, the same helper, the same
//! dict, after a `DeprecationWarning` naming the new name (an error under
//! `warnings.simplefilter("error")`, so nothing is computed then). Removing
//! them in 0.5.2 is deleting the two alias fns and their snapshot lines.
//!
//! Transport only: populations, signatures, verdicts and locations live in
//! the engine. The helpers the pyo3 entry points delegate to are pyo3-free,
//! so `cargo test -p glia-py` covers them (see the crate doc); they
//! print this surface's marker, `[patterns] surface=pyo3 mode=<graph|delta>`
//! (0.5.0: `[patterns] experimental surface=pyo3 ..`).

use std::collections::BTreeMap;

use pyo3::exceptions::{PyDeprecationWarning, PyValueError};
use pyo3::prelude::*;

use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::patterns::{
    GroupBy, PatternArgs, PatternReport, pattern_conformance, pattern_conformance_delta,
};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The engine's options from the keyword arguments; `Err` names a
/// `min_share` that is no percentage or a `group_by` that is neither choice.
fn options(
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
    group_by: &str,
) -> Result<PatternArgs, String> {
    if min_share > 100 {
        return Err(format!("min_share is a percentage (0-100), got {min_share}"));
    }
    let Some(group_by) = GroupBy::parse(group_by) else {
        return Err(format!(
            "group_by is {}, got {group_by:?}",
            GroupBy::CHOICES.map(|c| format!("{c:?}")).join(" or ")
        ));
    };
    let mut p = PatternArgs::default();
    p.min_support = min_support;
    p.min_share_pct = min_share;
    p.scope = scope.map(str::to_string);
    p.group_by = group_by;
    Ok(p)
}

/// The body of [`PyGraph::patterns`] (and its alias), minus pyo3.
fn graph_report(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
    group_by: &str,
) -> Result<PatternReport, String> {
    let p = options(min_support, min_share, scope, group_by)?;
    let report = pattern_conformance(merged, repo_labels, &p);
    eprintln!("[patterns] surface=pyo3 mode=graph");
    Ok(report)
}

/// The body of [`patterns_vs_rev`] (and its alias), minus pyo3. `Err` is a bad
/// `min_share` or `group_by`, or the engine's message (not a directory, git missing, not a
/// git work tree, an unknown rev, a failed build).
fn rev_report(
    repo_path: &str,
    base: &str,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
    group_by: &str,
) -> Result<PatternReport, String> {
    let p = options(min_support, min_share, scope, group_by)?;
    let d = graph_delta_vs_rev(repo_path, base)?;
    let report = pattern_conformance_delta(&d.after.merged, &d.after.repo_labels, &d.delta, &p);
    eprintln!("[patterns] surface=pyo3 mode=delta");
    Ok(report)
}

/// A pre-promotion alias's `DeprecationWarning`, naming its replacement.
const GRAPH_ALIAS_WARNING: &std::ffi::CStr =
    c"patterns_experimental is deprecated: use patterns (removed in 0.5.2)";
/// [`patterns_vs_rev_experimental`]'s warning.
const REV_ALIAS_WARNING: &std::ffi::CStr =
    c"patterns_vs_rev_experimental is deprecated: use patterns_vs_rev (removed in 0.5.2)";

/// Emit a pre-promotion alias's `DeprecationWarning` at the caller's line;
/// `Err` when the warning filter turns it into an exception.
fn deprecated(py: Python<'_>, message: &std::ffi::CStr) -> PyResult<()> {
    PyErr::warn(py, py.get_type::<PyDeprecationWarning>().as_any(), message, 1)
}

#[pymethods]
impl PyGraph {
    /// **patterns** (LE.7b; out of experimental since CC.12b): pattern
    /// conformance over this graph. The route handlers of each service (the
    /// services `service_map` shows) form a population; each handler's
    /// signature is its role chain to the first effect sink
    /// (`handler>service>repository>db`, `handler>(no effect)` when the graph
    /// follows none); a population of at least `min_support` handlers whose
    /// most frequent sink-reaching signature holds `min_share` percent of its
    /// SIGHTED handlers has that signature as its convention, and every
    /// sighted handler off it is a DIVERGENCE (tier `heuristic`: observed,
    /// never a rule). A BLIND handler (`handler>(no effect)`, the graph
    /// follows no chain from it) counts toward the size, not the share, and
    /// is listed in the population's `blind`, never as a divergence; a
    /// population with fewer than `min_support` sighted handlers reads
    /// `blind`. `scope` (a path or project label) keeps only the handlers
    /// located under it; `group_by="package"` keys populations by (service,
    /// the handler file's directory) instead of the service alone.
    ///
    /// Returns a dict `{delta_mode, handlers, judged, skipped_small,
    /// excluded, role_sources, populations, divergences, blind}`: `excluded`
    /// counts the handlers left out by reason (`test_fixture`, `generated`,
    /// `generated_proto`, `unplaced`, `out_of_scope`); `blind` counts the
    /// blind handlers listed; each population `{service, package, role, size,
    /// sighted, status, convention, matching, verdict, signatures,
    /// role_sources, exceptions, blind}` has `status` `judged` |
    /// `no_convention` | `blind` | `too_small`, `package` None unless
    /// `group_by="package"`, `verdict` `matching/sighted`, and `blind` a
    /// located `{handler, file, line, route_method, route_path}` per blind
    /// handler; each divergence `{verdict, tier, service, handler, file,
    /// line, route_method, route_path, signature, convention, matching,
    /// population, path, role_sources}` is located (lines 1-based), `path`
    /// its hops to the sink. Raises ValueError on a `min_share` above 100 or
    /// a `group_by` other than `"service"` / `"package"`.
    #[pyo3(signature = (min_support=5, min_share=75, scope=None, group_by="service"))]
    fn patterns(
        &self,
        py: Python<'_>,
        min_support: usize,
        min_share: usize,
        scope: Option<&str>,
        group_by: &str,
    ) -> PyResult<Py<PyAny>> {
        let report =
            graph_report(&self.merged, &self.repo_labels, min_support, min_share, scope, group_by)
                .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&report))
    }

    /// **patterns_experimental** (DEPRECATED, removed in 0.5.2): the 0.5.0
    /// name of `patterns`. Same parameters and dict, after a
    /// `DeprecationWarning` (raised as an exception, before any work, under
    /// `warnings.simplefilter("error")`).
    #[pyo3(signature = (min_support=5, min_share=75, scope=None, group_by="service"))]
    fn patterns_experimental(
        &self,
        py: Python<'_>,
        min_support: usize,
        min_share: usize,
        scope: Option<&str>,
        group_by: &str,
    ) -> PyResult<Py<PyAny>> {
        deprecated(py, GRAPH_ALIAS_WARNING)?;
        self.patterns(py, min_support, min_share, scope, group_by)
    }
}

/// **patterns_vs_rev** (LE.7b; out of experimental since CC.12b): pattern
/// conformance on the working tree's change against git rev `base` (a
/// branch, tag, sha or `"HEAD~N"`; default `"HEAD"`) of the repo at
/// `repo_path`. Populations and conventions come from the whole working-tree
/// graph; `divergences` lists only the exceptions the change touched (the
/// handler or a node on its path added, modified or moved, or a hop of its
/// path an added edge), while each population's `exceptions` still lists
/// them all; a population's `blind` keeps only the touched blind handlers.
/// Same keywords and dict as `PyGraph.patterns`, with `delta_mode` True.
/// Builds both sides itself and saves the parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as
/// `generate(incremental=True)` does, never a `.gmap` layout. Raises
/// ValueError on a `min_share` above 100, a `group_by` other than
/// `"service"` / `"package"`, or a git or build failure.
#[pyfunction]
#[pyo3(signature = (repo_path, base="HEAD", min_support=5, min_share=75, scope=None, group_by="service"))]
fn patterns_vs_rev(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
    group_by: &str,
) -> PyResult<Py<PyAny>> {
    let report = rev_report(repo_path, base, min_support, min_share, scope, group_by)
        .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&report))
}

/// **patterns_vs_rev_experimental** (DEPRECATED, removed in 0.5.2): the
/// 0.5.0 name of `patterns_vs_rev`. Same parameters and dict, after a
/// `DeprecationWarning` (raised as an exception, before any work, under
/// `warnings.simplefilter("error")`).
#[pyfunction]
#[pyo3(signature = (repo_path, base="HEAD", min_support=5, min_share=75, scope=None, group_by="service"))]
fn patterns_vs_rev_experimental(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    min_support: usize,
    min_share: usize,
    scope: Option<&str>,
    group_by: &str,
) -> PyResult<Py<PyAny>> {
    deprecated(py, REV_ALIAS_WARNING)?;
    patterns_vs_rev(py, repo_path, base, min_support, min_share, scope, group_by)
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(patterns_vs_rev, m)?)?;
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
            .unwrap_or_else(|e| panic!("LE.7b patterns_vs_rev test needs a `git` binary: {e}"));
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// LE.7b: the helper behind `PyGraph.patterns` (and its deprecated
    /// alias) returns the engine's report with the documented keys and no
    /// `experimental` key (CC.12b), the one located divergence, and the
    /// keywords reach the engine. The conformance itself is covered by
    /// `engine/tests/patterns.rs`.
    #[test]
    fn graph_helper_returns_the_documented_object() {
        let (_scratch, top) = shop("graph", 6);
        let built = glia_engine::generate_one(top.to_str().expect("utf-8")).expect("build");
        let r = graph_report(&built.merged, &built.repo_labels, 5, 75, None, "service").expect("report");
        let v = value(&r);
        for key in ["populations", "divergences", "skipped_small", "delta_mode"] {
            assert!(v.get(key).is_some(), "{key} missing: {v}");
        }
        assert!(v.get("experimental").is_none(), "promoted (CC.12b): {v}");
        let text = serde_json::to_string(&r).expect("serialises");
        assert!(text.starts_with("{\"delta_mode\":"), "{text}");
        let order: Vec<usize> = [
            "\"delta_mode\":",
            "\"handlers\":",
            "\"judged\":",
            "\"skipped_small\":",
            "\"excluded\":",
            "\"role_sources\":",
            "\"populations\":",
            "\"divergences\":",
            "\"blind\":0}",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        assert_eq!(v["delta_mode"], false);
        assert_eq!(v["skipped_small"], 0);
        assert_eq!(divergent(&v), [DIRECT]);
        let p = &v["populations"][0];
        assert_eq!((p["status"].as_str(), p["verdict"].as_str()), (Some("judged"), Some("5/6")), "{p}");
        assert_eq!(p["convention"], CONVENTION);
        assert_eq!(v["divergences"][0]["file"], "handlers/handlers.go");
        assert_eq!(v["divergences"][0]["route_path"], "/orders");

        // min_support above the population: too small, nothing judged.
        let small = value(&graph_report(&built.merged, &built.repo_labels, 7, 75, None, "service").expect("report"));
        assert_eq!((small["skipped_small"].as_u64(), small["judged"].as_u64()), (Some(1), Some(0)));
        assert_eq!(small["populations"][0]["status"], "too_small");
        assert!(divergent(&small).is_empty());
        // A 5/6 share is below 90%: no convention.
        let split = value(&graph_report(&built.merged, &built.repo_labels, 5, 90, None, "service").expect("report"));
        assert_eq!(split["populations"][0]["status"], "no_convention");
        assert!(divergent(&split).is_empty());
        // A scope no handler sits under: every handler excluded.
        let scoped = value(&graph_report(&built.merged, &built.repo_labels, 5, 75, Some("service"), "service").expect("report"));
        assert_eq!(scoped["excluded"]["out_of_scope"], 6, "{scoped}");
        assert_eq!(scoped["handlers"], 0);

        let err = graph_report(&built.merged, &built.repo_labels, 5, 101, None, "service").expect_err("bad share");
        assert!(err.contains("101"), "{err}");

        // CA.5b: group_by reaches the engine; any other value names the two.
        let by_pkg =
            value(&graph_report(&built.merged, &built.repo_labels, 5, 75, None, "package").expect("report"));
        assert_eq!(by_pkg["populations"][0]["package"], "handlers", "{by_pkg}");
        assert_eq!(v["populations"][0]["package"], serde_json::Value::Null, "{v}");
        let err = graph_report(&built.merged, &built.repo_labels, 5, 75, None, "dir").expect_err("bad group_by");
        assert!(err.contains("\"service\" or \"package\"") && err.contains("\"dir\""), "{err}");
    }

    /// LE.7b: the helper behind `patterns_vs_rev` (and its alias) judges the
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
        let v = value(&rev_report(repo, "HEAD", 5, 75, None, "service").expect("report"));
        assert!(v.get("experimental").is_none(), "{v}");
        assert_eq!(v["delta_mode"].as_bool(), Some(true));
        assert_eq!(divergent(&v), [DIRECT]);
        assert_eq!(v["populations"][0]["verdict"], "5/6");

        let err = rev_report(repo, "no-such-rev", 5, 75, None, "service").expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
        let err = rev_report(repo, "HEAD", 5, 200, None, "service").expect_err("bad share");
        assert!(err.contains("200"), "{err}");
    }
}
