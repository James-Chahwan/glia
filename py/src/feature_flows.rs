//! **feature_flows** (LG.3c): `PyGraph.feature_flows` and
//! `PyGraph.write_feature_flows`, the Python surface of the engine's
//! `feature_flows::{feature_flows, write_feature_flows}` (LG.3a) — LD.4b's
//! entry flows grouped into per-feature records, and the writer of the
//! `<feature>.yaml` + `index.json` files the dogfood repos' agents read. The
//! repo-graph wrapper's `_build_flows` / `nodes_for_feature` can call these
//! instead of assembling flows in Python.
//!
//! `feature_flows` returns the records as a native list of dicts (LD.2);
//! `write_feature_flows` returns `{written, unchanged, removed, dir}`. An
//! unknown `group_by`, a refused dir or a failed write raises `ValueError`.
//!
//! The writer's walked-tree refusal needs the build's repo roots: the graph's
//! `repo_roots` (a fresh build's paths, or the roots a loaded layout's
//! manifest recorded). `out_dir=None` means the one repo's
//! `default_flows_dir` (`<repo>/.glia/graph/flows`); a graph of several repos
//! has no single default and raises, as does one with no recorded roots. With
//! no roots and an explicit `out_dir`, the check is skipped and
//! `[feature-flows] note: repo roots unknown - walked-tree check skipped` is
//! printed on stderr.
//!
//! Transport only: grouping, the files, the `[feature-flows] features=..` and
//! `[feature-flows] wrote ..` markers live in the engine. The helpers the pyo3
//! entry points delegate to are pyo3-free, so `cargo test -p glia-py`
//! covers them (see the crate doc).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::feature_flows::{
    FlowGrouping, FlowOptions, default_flows_dir, feature_flows, write_feature_flows,
};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// What `write_feature_flows` returns: the engine's counts and the dir.
#[derive(Debug, PartialEq, Eq)]
struct Written {
    written: usize,
    unchanged: usize,
    removed: usize,
    dir: String,
}

impl Written {
    /// `{"written", "unchanged", "removed", "dir"}` in that order, for
    /// [`to_py`]. Spelled out rather than derived: this crate has no `serde`
    /// dependency, and a `serde_json::Value` map would sort the keys.
    fn to_json(&self) -> Result<String, serde_json::Error> {
        Ok(format!(
            "{{\"written\":{},\"unchanged\":{},\"removed\":{},\"dir\":{}}}",
            self.written,
            self.unchanged,
            self.removed,
            serde_json::to_string(&self.dir)?
        ))
    }
}

/// The engine's options from the keyword arguments. The pyo3 signatures
/// spell `depth`'s default as the literal `6`, so Python introspection shows
/// it; the unit test pins it to the engine's `DEFAULT_DEPTH`.
fn options(
    group_by: &str,
    depth: usize,
    feature: Option<&str>,
    scope: Option<&str>,
) -> Result<FlowOptions, String> {
    let grouping = FlowGrouping::from_name(group_by).ok_or_else(|| {
        format!("feature_flows: group_by must be \"feature\" or \"entry\", not {group_by:?}")
    })?;
    let mut opts = FlowOptions::default()
        .with_grouping(grouping)
        .with_depth(depth);
    if let Some(f) = feature {
        opts = opts.with_feature(f);
    }
    if let Some(s) = scope {
        opts = opts.with_scope(s);
    }
    Ok(opts)
}

/// The body of [`PyGraph::feature_flows`], minus pyo3: the records as the
/// JSON text [`to_py`] decodes.
fn feature_flows_json(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    opts: &FlowOptions,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&feature_flows(merged, repo_labels, opts))
}

/// Where `write_feature_flows` writes: `out_dir`, else the one repo's
/// `default_flows_dir`.
fn target_dir(roots: &BTreeMap<u64, String>, out_dir: Option<&str>) -> Result<PathBuf, String> {
    if let Some(d) = out_dir {
        return Ok(PathBuf::from(d));
    }
    let mut it = roots.values();
    match (it.next(), it.next()) {
        (Some(root), None) => Ok(default_flows_dir(Path::new(root))),
        (None, _) => Err(
            "write_feature_flows: this graph records no repo roots, so there is no default \
             flows dir; pass out_dir"
                .to_string(),
        ),
        (Some(_), Some(_)) => Err(format!(
            "write_feature_flows: this graph holds {} repos, so there is no single default \
             flows dir; pass out_dir (e.g. the first repo's <repo>/.glia/graph/flows)",
            roots.len()
        )),
    }
}

/// The body of [`PyGraph::write_feature_flows`], minus pyo3.
fn write_answer(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    roots: &BTreeMap<u64, String>,
    out_dir: Option<&str>,
    group_by: &str,
    depth: usize,
) -> Result<Written, String> {
    let opts = options(group_by, depth, None, None)?;
    let dir = target_dir(roots, out_dir)?;
    if roots.is_empty() {
        eprintln!("[feature-flows] note: repo roots unknown - walked-tree check skipped");
    }
    let flows = feature_flows(merged, repo_labels, &opts);
    let root_paths: Vec<&Path> = roots.values().map(Path::new).collect();
    let w = write_feature_flows(&root_paths, &dir, &flows, opts.grouping)
        .map_err(|e| format!("write_feature_flows({}): {e}", dir.display()))?;
    Ok(Written {
        written: w.written,
        unchanged: w.unchanged,
        removed: w.removed,
        dir: dir.display().to_string(),
    })
}

#[pymethods]
impl PyGraph {
    /// **feature_flows** (LG.3c): the graph's entry points grouped into
    /// per-feature records. `group_by="feature"` keys a record by the
    /// channel's feature word (the first static path segment of a route, so
    /// a page `/orders` and the API route `/api/orders/:id` are one feature
    /// `orders`; `queue-<topic>`, `grpc-<service>`, `cli-<word>`, ... for the
    /// other mechanisms); `"entry"` gives one record per entry point, keyed
    /// like `entry_flows`. `depth` bounds each entry's forward walk;
    /// `feature` keeps only the record with that key; `scope` (a path or
    /// project label) keeps only entries under it.
    ///
    /// Returns a list of dicts `{feature, services, entries, data_sources}`
    /// sorted by `feature`: `entries` rows `{entry, weakest, callers, steps}`,
    /// where `entry` and every caller / step is `{qname, kind, service, via,
    /// confidence, cross_service, depth, file, line}` (1-based `line`);
    /// `data_sources` rows `{qname, kind, via, tier, from}` with `tier`
    /// `FACT` or `HEURISTIC`. Raises ValueError on an unknown `group_by`.
    #[pyo3(signature = (group_by="feature", depth=6, feature=None, scope=None))]
    fn feature_flows(
        &self,
        py: Python<'_>,
        group_by: &str,
        depth: usize,
        feature: Option<&str>,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let opts = options(group_by, depth, feature, scope).map_err(PyValueError::new_err)?;
        to_py(
            py,
            feature_flows_json(&self.merged, &self.repo_labels, &opts),
        )
    }

    /// **write_feature_flows** (LG.3c): write every `feature_flows` record
    /// to `out_dir` as `<feature>.yaml` plus `index.json` — the files
    /// `glia flows --out` writes. Only files whose bytes change are rewritten;
    /// only files the previous `index.json` listed are removed. `out_dir=None`
    /// is the repo's `<repo>/.glia/graph/flows` (a graph of one repo only).
    /// A dir inside one of the graph's repos but not under its `.glia/` is
    /// refused: the next build would read the files as sources.
    ///
    /// Returns `{written, unchanged, removed, dir}`. Raises ValueError on an
    /// unknown `group_by`, a refused dir, a failed write, or `out_dir=None`
    /// on a graph with no recorded roots or several repos.
    #[pyo3(signature = (out_dir=None, group_by="feature", depth=6))]
    fn write_feature_flows(
        &self,
        py: Python<'_>,
        out_dir: Option<&str>,
        group_by: &str,
        depth: usize,
    ) -> PyResult<Py<PyAny>> {
        let answer = write_answer(
            &self.merged,
            &self.repo_labels,
            &self.repo_roots,
            out_dir,
            group_by,
            depth,
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, answer.to_json())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use glia_engine::GenerateResult;
    use glia_engine::feature_flows::DEFAULT_DEPTH;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg3c-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        Scratch(root)
    }

    fn fixture(repo: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/flows_stack")
            .join(repo)
            .to_string_lossy()
            .into_owned()
    }

    fn build() -> GenerateResult {
        glia_engine::generate_many(&[fixture("web"), fixture("api")]).expect("build")
    }

    /// LG.3c: the helper behind `PyGraph.feature_flows` returns the engine's
    /// records as JSON, the keywords reach the engine, and an unknown
    /// `group_by` is an error naming the allowed values. The records
    /// themselves are covered by `engine/tests/feature_flows.rs`.
    #[test]
    fn feature_flows_json_carries_the_records() {
        let r = build();
        let opts = options("feature", DEFAULT_DEPTH, None, None).expect("options");
        let v: serde_json::Value = serde_json::from_str(
            &feature_flows_json(&r.merged, &r.repo_labels, &opts).expect("json"),
        )
        .expect("valid JSON");
        let keys: Vec<&str> = v
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|f| f["feature"].as_str())
            .collect();
        assert_eq!(keys, ["orders", "queue-orders.created"]);
        let text = feature_flows_json(&r.merged, &r.repo_labels, &opts).expect("json");
        let order: Vec<usize> = [
            "\"feature\":",
            "\"services\":",
            "\"entries\":",
            "\"data_sources\":",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");

        let one = options("feature", DEFAULT_DEPTH, Some("orders"), None).expect("options");
        let v: serde_json::Value = serde_json::from_str(
            &feature_flows_json(&r.merged, &r.repo_labels, &one).expect("json"),
        )
        .expect("valid JSON");
        assert_eq!(v.as_array().map(Vec::len), Some(1));

        let by_entry = options("entry", 2, None, Some("api")).expect("options");
        assert_eq!(
            (by_entry.grouping, by_entry.depth, by_entry.scope.as_deref()),
            (FlowGrouping::Entry, 2, Some("api"))
        );
        let err = options("service", DEFAULT_DEPTH, None, None).expect_err("unknown group_by");
        assert!(
            err.contains("\"feature\"") && err.contains("\"entry\""),
            "{err}"
        );
        assert_eq!(
            DEFAULT_DEPTH, 6,
            "the pyo3 signatures' literal depth default"
        );
    }

    /// LG.3c: the helper behind `PyGraph.write_feature_flows` writes the
    /// files and reports the counts, leaves unchanged bytes alone on a second
    /// write, refuses a dir inside a built repo, and needs `out_dir` when the
    /// graph has no single default dir.
    #[test]
    fn write_answer_writes_counts_and_refuses() {
        let r = build();
        let s = scratch("write");
        let out = s.0.join("flows");
        let out_s = out.to_str().expect("utf-8");
        let first = write_answer(
            &r.merged,
            &r.repo_labels,
            &r.repo_roots,
            Some(out_s),
            "feature",
            DEFAULT_DEPTH,
        )
        .expect("write");
        assert_eq!(
            first,
            Written {
                written: 2,
                unchanged: 0,
                removed: 0,
                dir: out_s.to_string()
            }
        );
        assert!(out.join("orders.yaml").is_file() && out.join("index.json").is_file());
        let v: serde_json::Value =
            serde_json::from_str(&first.to_json().expect("json")).expect("valid");
        assert_eq!(v["dir"], out_s);
        let text = first.to_json().expect("json");
        assert!(
            text.starts_with("{\"written\":2,\"unchanged\":0,\"removed\":0,\"dir\":"),
            "{text}"
        );
        let again = write_answer(
            &r.merged,
            &r.repo_labels,
            &r.repo_roots,
            Some(out_s),
            "feature",
            DEFAULT_DEPTH,
        )
        .expect("write");
        assert_eq!((again.written, again.unchanged, again.removed), (0, 2, 0));

        // Inside a built repo, outside its `.glia/`: refused, nothing written.
        let inside = PathBuf::from(fixture("web")).join("docs/flows");
        let err = write_answer(
            &r.merged,
            &r.repo_labels,
            &r.repo_roots,
            inside.to_str(),
            "feature",
            DEFAULT_DEPTH,
        )
        .expect_err("a walked dir is refused");
        assert!(err.contains("refusing"), "{err}");
        assert!(!inside.exists());

        // No out_dir: two repos have no single default, no roots none at all.
        let err = target_dir(&r.repo_roots, None).expect_err("two repos");
        assert!(err.contains("2 repos") && err.contains("out_dir"), "{err}");
        let err = target_dir(&BTreeMap::new(), None).expect_err("no roots");
        assert!(err.contains("no repo roots"), "{err}");
        let one = BTreeMap::from([(1u64, "/r".to_string())]);
        assert_eq!(
            target_dir(&one, None).expect("one repo"),
            default_flows_dir(Path::new("/r"))
        );
        // No roots with an explicit dir: written, the walked-tree check skipped.
        let bare = s.0.join("bare");
        let w = write_answer(
            &r.merged,
            &r.repo_labels,
            &BTreeMap::new(),
            bare.to_str(),
            "entry",
            DEFAULT_DEPTH,
        )
        .expect("write");
        assert_eq!(w.written, 3, "one file per entry: {w:?}");
    }
}
