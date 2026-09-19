//! The cell write API (LF.1b): `PyGraph.set_cell` / `remove_cell` /
//! `node_cell_bytes`, and the module functions `write_cell` / `remove_cell`
//! for a caller that holds no graph.
//!
//! A write lands in three places, so the MCP wrapper (which serves the warm
//! path from the persisted layout) stays coherent without a rebuild:
//! `.glia/cells.jsonl` (`.glia/vectors.jsonl` for a VECTOR), the durable home
//! every build applies; the repo's default layout `<repo>/.glia/graph`,
//! rewritten only while it is fresh (`repo_graph_store::write_cell`); and, for
//! `set_cell`, the live `PyGraph`, bound through the same resolver
//! (`repo_graph_graph::cells`), so the in-memory graph equals the next build.
//!
//! `set_cell` finds the repo that owns the node in the graph itself and its
//! root in the graph's `repo_roots` (a fresh build's paths, or the roots a
//! loaded layout records); the stored row carries the node's move-stable hint.
//!
//! Payload: a `str` is one JSON entry object for CONSTRAINT / DECISION / CONV
//! (`source` defaults to `api`; a CONV without `id` gets the next `000001`-style
//! id); `bytes` is a VECTOR (`model`, `dims` optional). The answer is the
//! store's `CellWriteOutcome` as a `dict` (the LD.2 convention). Every write or
//! removal prints the fired_on marker from its pyo3-free helper, the path the
//! unit tests run:
//! `[cells] write|remove qname=<q> cell=<C> id=<id|-> rows=<n> target=<t> write_through=<w> surface=pyo3`
//! (plus ` redacted=<k>` when the A13.7 redaction replaced a secret).
//!
//! The helpers the bindings delegate to are pyo3-free (`cell_write`,
//! `set_cell_in`, `write_cell_at`, `remove_cell_in`, `remove_cell_at`,
//! `node_cell_bytes_of`, `marker`) so `cargo test -p repo-graph-py` covers
//! them (see the crate doc).

use std::collections::BTreeMap;
use std::path::Path;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};

use repo_graph_code_domain::cell_type;
use repo_graph_code_domain::external_inputs::{CellWrite, cell_type_id};
use repo_graph_core::{CellPayload, NodeId, RepoId};
use repo_graph_engine::persist::default_layout_dir;
use repo_graph_graph::MergedGraph;
use repo_graph_graph::cells::{CellTarget, QnameIndex, apply_cell_write};
use repo_graph_graph::identity::identity_of;
use repo_graph_store::{
    CellRemoval, CellWriteOutcome, apply_cell_removal, remove_cell_entry,
    write_cell as store_write_cell,
};

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The `surface=` of the marker.
const SURFACE: &str = "pyo3";
const NO_ROOT: &str =
    "no repo root known for this graph; call repo_graph_py.write_cell(repo_path, ...)";

/// A write's payload as Python passed it.
pub(crate) enum Payload {
    /// A `str`: one JSON entry object.
    Entry(String),
    /// `bytes`: a VECTOR.
    Vector(Vec<u8>),
}

/// The `CellWrite` for `cell` (a cell type NAME) on `qname`. The payload shape
/// must fit the cell: bytes only for VECTOR, a JSON entry for the others.
pub(crate) fn cell_write(
    qname: &str,
    cell: &str,
    payload: Payload,
    kind: Option<String>,
    model: Option<String>,
    dims: Option<u32>,
) -> Result<CellWrite, String> {
    let id = cell_type_id(cell).ok_or_else(|| format!("unknown cell type {cell:?} (see cell_type_names())"))?;
    let is_vector = id == cell_type::VECTOR;
    match payload {
        Payload::Entry(_) if is_vector => Err("a VECTOR payload is bytes, not a JSON str".into()),
        Payload::Vector(_) if !is_vector => Err(format!("a {cell} payload is a JSON entry str; bytes are a VECTOR")),
        Payload::Entry(_) if model.is_some() || dims.is_some() => {
            Err("model / dims apply to a VECTOR (bytes) payload only".into())
        }
        Payload::Entry(text) => {
            let v = serde_json::from_str(&text).map_err(|e| format!("the {cell} payload is not JSON: {e}"))?;
            Ok(CellWrite::entry(qname, id, v).with_kind(kind))
        }
        Payload::Vector(bytes) => Ok(CellWrite::vector(qname, bytes, model, dims).with_kind(kind)),
    }
}

/// The removal of the `(source, entry_id)` entry of `cell` on `qname`; for a
/// VECTOR, of the node's vector (`source` and `entry_id` unused).
fn removal(qname: &str, cell: &str, entry_id: &str, source: &str) -> Result<CellRemoval, String> {
    let id = cell_type_id(cell).ok_or_else(|| format!("unknown cell type {cell:?} (see cell_type_names())"))?;
    Ok(if id == cell_type::VECTOR {
        CellRemoval::vector(qname)
    } else {
        CellRemoval::entry(qname, id, source, entry_id)
    })
}

/// The `[cells] <verb>` marker line of one outcome.
pub(crate) fn marker(verb: &str, o: &CellWriteOutcome) -> String {
    let mut line = format!(
        "[cells] {verb} qname={} cell={} id={} rows={} target={} write_through={} surface={SURFACE}",
        o.qname,
        o.cell,
        o.entry_id.as_deref().unwrap_or("-"),
        o.rows,
        o.target,
        o.write_through.as_str()
    );
    if o.redacted > 0 {
        line.push_str(&format!(" redacted={}", o.redacted));
    }
    line
}

/// The repo of the graph that holds node `id`.
fn repo_of(merged: &MergedGraph, id: NodeId) -> Option<RepoId> {
    merged.graphs.iter().find(|g| g.nav.kind_by_id.contains_key(&id)).map(|g| g.repo)
}

/// The repo a write on `qname` belongs to, and the node it binds when it
/// binds one: the node the whole graph binds; else the graph's only repo;
/// else the one repo that binds it. Err when several repos (or none of
/// several) do: the caller names the repo through `write_cell(repo_path, ..)`.
fn owning_repo(
    merged: &MergedGraph,
    qname: &str,
    kind: Option<&str>,
    hint: Option<&str>,
) -> Result<(RepoId, Option<NodeId>), String> {
    let bound = |t: CellTarget| match t {
        CellTarget::Bound(id) | CellTarget::Rekeyed { id, .. } => Some(id),
        _ => None,
    };
    if let Some(id) = bound(QnameIndex::build(merged, None).resolve(qname, kind, hint))
        && let Some(repo) = repo_of(merged, id)
    {
        return Ok((repo, Some(id)));
    }
    let mut repos: Vec<RepoId> = merged.graphs.iter().map(|g| g.repo).collect();
    repos.sort_by_key(|r| r.0);
    repos.dedup();
    if let [only] = repos.as_slice() {
        return Ok((*only, None));
    }
    let hits: Vec<(RepoId, NodeId)> = repos
        .iter()
        .filter_map(|r| bound(QnameIndex::build(merged, Some(*r)).resolve(qname, kind, hint)).map(|id| (*r, id)))
        .collect();
    match hits.as_slice() {
        [(repo, id)] => Ok((*repo, Some(*id))),
        [] => Err(format!(
            "no node {qname:?} in this graph's {} repos; call repo_graph_py.write_cell(repo_path, ...) to write it anyway",
            repos.len()
        )),
        _ => Err(format!(
            "{qname:?} names a node in {} of this graph's repos; pass kind=, or call repo_graph_py.write_cell(repo_path, ...) for the one meant",
            hits.len()
        )),
    }
}

/// The root `roots` records for `repo`.
fn root_of(roots: &BTreeMap<u64, String>, repo: RepoId) -> Result<&Path, String> {
    roots.get(&repo.0).map(Path::new).ok_or_else(|| NO_ROOT.to_string())
}

/// `set_cell` minus pyo3: store the write for the repo that owns the node
/// (sidecar + fresh default layout), then apply the stored entry to `merged`
/// through the same resolver. The outcome's `target` is the live binding.
pub(crate) fn set_cell_in(
    merged: &mut MergedGraph,
    roots: &BTreeMap<u64, String>,
    w: CellWrite,
) -> Result<CellWriteOutcome, String> {
    let (repo, node) = owning_repo(merged, &w.qname, w.kind.as_deref(), w.hint.as_deref())?;
    let root = root_of(roots, repo)?;
    let hint = w.hint.clone().or_else(|| node.and_then(|id| identity_of(merged, id)).map(|i| i.hint()));
    let w = w.with_hint(hint);
    let mut out = store_write_cell(root, Some(&default_layout_dir(root)), Some(repo), &w).map_err(|e| e.to_string())?;
    let stored = match &out.entry {
        Some(entry) => CellWrite::entry(w.qname.clone(), w.cell, entry.clone()),
        None => w,
    }
    .with_kind(out.kind.clone())
    .with_hint(out.hint.clone());
    let idx = QnameIndex::build(merged, Some(repo));
    let live = apply_cell_write(merged, &idx, &stored)?;
    out.set_target(&live);
    eprintln!("{}", marker("write", &out));
    Ok(out)
}

/// `remove_cell` minus pyo3: remove the row for the repo that owns the node,
/// then the entry from `merged`. `None` when the sidecar holds no such row.
pub(crate) fn remove_cell_in(
    merged: &mut MergedGraph,
    roots: &BTreeMap<u64, String>,
    r: CellRemoval,
) -> Result<Option<CellWriteOutcome>, String> {
    let (repo, _) = owning_repo(merged, &r.qname, None, None)?;
    let root = root_of(roots, repo)?;
    let dir = default_layout_dir(root);
    let Some(mut out) = remove_cell_entry(root, Some(&dir), Some(repo), &r).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let live = r.with_kind(out.kind.clone()).with_hint(out.hint.clone());
    let idx = QnameIndex::build(merged, Some(repo));
    out.set_target(&apply_cell_removal(merged, &idx, &live));
    eprintln!("{}", marker("remove", &out));
    Ok(Some(out))
}

/// The module `write_cell` minus pyo3: the repo at `repo_path`, its default
/// layout written through when fresh and single-repo.
pub(crate) fn write_cell_at(repo_path: &str, w: &CellWrite) -> Result<CellWriteOutcome, String> {
    let root = Path::new(repo_path);
    let out = store_write_cell(root, Some(&default_layout_dir(root)), None, w).map_err(|e| e.to_string())?;
    eprintln!("{}", marker("write", &out));
    Ok(out)
}

/// The module `remove_cell` minus pyo3.
pub(crate) fn remove_cell_at(repo_path: &str, r: &CellRemoval) -> Result<Option<CellWriteOutcome>, String> {
    let root = Path::new(repo_path);
    let out = remove_cell_entry(root, Some(&default_layout_dir(root)), None, r).map_err(|e| e.to_string())?;
    if let Some(o) = &out {
        eprintln!("{}", marker("remove", o));
    }
    Ok(out)
}

/// The bytes of node `node_id`'s `cell` (a cell type NAME) when that cell
/// holds bytes; `None` for an unknown node, a missing cell or a text payload.
pub(crate) fn node_cell_bytes_of(merged: &MergedGraph, node_id: u64, cell: &str) -> Result<Option<Vec<u8>>, String> {
    let t = cell_type_id(cell).ok_or_else(|| format!("unknown cell type {cell:?} (see cell_type_names())"))?;
    let id = NodeId(node_id);
    Ok(merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .find(|n| n.id == id)
        .and_then(|n| n.cells.iter().find(|c| c.kind == t))
        .and_then(|c| match &c.payload {
            CellPayload::Bytes(b) => Some(b.clone()),
            _ => None,
        }))
}

/// A Python payload: `str` (a JSON entry) or `bytes` (a VECTOR).
fn payload_of(payload: &Bound<'_, PyAny>) -> PyResult<Payload> {
    if let Ok(s) = payload.cast::<PyString>() {
        return Ok(Payload::Entry(s.to_str()?.to_string()));
    }
    if let Ok(b) = payload.cast::<PyBytes>() {
        return Ok(Payload::Vector(b.as_bytes().to_vec()));
    }
    Err(PyTypeError::new_err(
        "payload must be str (a JSON entry for CONSTRAINT / DECISION / CONV) or bytes (a VECTOR)",
    ))
}

#[pymethods]
impl PyGraph {
    /// Write one cell on the node `qname` names: a CONSTRAINT / DECISION /
    /// CONV entry (`payload` a JSON object `str`) or a VECTOR (`payload`
    /// `bytes`, with optional `model` and `dims`). `kind` (a NodeKind NAME)
    /// picks between nodes sharing a qname. The write goes to the repo's
    /// `.glia` sidecar, into its default layout when that is fresh, and into
    /// this graph. Returns the outcome dict (`entry_id`, `rows`, `target`,
    /// `write_through`, ...). Raises ValueError for a cell type that is not
    /// writable, a payload that breaks the entry rules, or a graph with no
    /// root for the node's repo.
    #[pyo3(signature = (qname, cell_type, payload, kind=None, model=None, dims=None))]
    #[allow(clippy::too_many_arguments)]
    fn set_cell(
        &mut self,
        py: Python<'_>,
        qname: &str,
        cell_type: &str,
        payload: &Bound<'_, PyAny>,
        kind: Option<String>,
        model: Option<String>,
        dims: Option<u32>,
    ) -> PyResult<Py<PyAny>> {
        let w = cell_write(qname, cell_type, payload_of(payload)?, kind, model, dims).map_err(PyValueError::new_err)?;
        let out = set_cell_in(&mut self.merged, &self.repo_roots, w).map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&out))
    }

    /// Remove the `(source, entry_id)` entry of `cell_type` from the node
    /// `qname` names (for VECTOR, the node's vector; `entry_id` is then
    /// unused): from the repo's sidecar, its fresh default layout and this
    /// graph. False when the sidecar holds no such entry.
    #[pyo3(signature = (qname, cell_type, entry_id, source="api"))]
    fn remove_cell(&mut self, qname: &str, cell_type: &str, entry_id: &str, source: &str) -> PyResult<bool> {
        let r = removal(qname, cell_type, entry_id, source).map_err(PyValueError::new_err)?;
        let out = remove_cell_in(&mut self.merged, &self.repo_roots, r).map_err(PyValueError::new_err)?;
        Ok(out.is_some())
    }

    /// The raw bytes of a node's `cell_type` cell (a cell type NAME, e.g.
    /// `"VECTOR"`), or None when the node, the cell or a bytes payload is
    /// missing. `node_cells` keeps returning "" for a bytes payload.
    fn node_cell_bytes(&self, py: Python<'_>, node_id: u64, cell_type: &str) -> PyResult<Option<Py<PyBytes>>> {
        let bytes = node_cell_bytes_of(&self.merged, node_id, cell_type).map_err(PyValueError::new_err)?;
        Ok(bytes.map(|b| PyBytes::new(py, &b).unbind()))
    }
}

/// Write one cell for the repo at `repo_path` without a graph: the `.glia`
/// sidecar, and the default layout when it is fresh and holds one repo (for a
/// multi-repo layout use `PyGraph.set_cell`). Same payloads and outcome dict
/// as `PyGraph.set_cell`; `target` is `pending` when no fresh layout was read.
#[pyfunction]
#[pyo3(signature = (repo_path, qname, cell_type, payload, kind=None, model=None, dims=None))]
#[allow(clippy::too_many_arguments)]
fn write_cell(
    py: Python<'_>,
    repo_path: &str,
    qname: &str,
    cell_type: &str,
    payload: &Bound<'_, PyAny>,
    kind: Option<String>,
    model: Option<String>,
    dims: Option<u32>,
) -> PyResult<Py<PyAny>> {
    let w = cell_write(qname, cell_type, payload_of(payload)?, kind, model, dims).map_err(PyValueError::new_err)?;
    let out = write_cell_at(repo_path, &w).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&out))
}

/// Remove one entry (a VECTOR: the node's vector) for the repo at
/// `repo_path` without a graph. False when the sidecar holds no such entry.
#[pyfunction]
#[pyo3(signature = (repo_path, qname, cell_type, entry_id, source="api"))]
fn remove_cell(repo_path: &str, qname: &str, cell_type: &str, entry_id: &str, source: &str) -> PyResult<bool> {
    let r = removal(qname, cell_type, entry_id, source).map_err(PyValueError::new_err)?;
    let out = remove_cell_at(repo_path, &r).map_err(PyValueError::new_err)?;
    Ok(out.is_some())
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(write_cell, m)?)?;
    m.add_function(wrap_pyfunction!(remove_cell, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "cells", add: register } }

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use repo_graph_store::{WriteThrough, is_gmap_stale, read_merged_sharded, write_merged_sharded};

    use super::*;

    const APP: &str = "def charge(order_id):\n    return order_id\n\n\ndef refund(order_id):\n    return charge(order_id)\n";

    /// A one-file repo under the temp dir, rebuilt fresh per tag.
    fn repo(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("glia-lf1b-py-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(root.join("app.py"), APP).expect("write fixture");
        root
    }

    fn built(root: &Path) -> repo_graph_engine::GenerateResult {
        repo_graph_engine::generate_one(root.to_str().expect("utf-8 temp path")).expect("build")
    }

    fn conv(text: &str) -> CellWrite {
        cell_write("app::charge", "CONV", Payload::Entry(format!("{{\"text\":\"{text}\"}}")), None, None, None)
            .expect("a CONV write")
    }

    fn conv_payload(m: &MergedGraph) -> Option<CellPayload> {
        let g = m.graphs.iter().find(|g| g.nav.qname_by_id.values().any(|q| q == "app::charge"))?;
        let id = g.nav.qname_by_id.iter().find(|(_, q)| *q == "app::charge").map(|(id, _)| *id)?;
        let n = g.nodes.iter().find(|n| n.id == id)?;
        n.cells.iter().find(|c| c.kind == cell_type::CONV).map(|c| c.payload.clone())
    }

    /// The payload shape must fit the cell, and model / dims are VECTOR-only.
    #[test]
    fn cell_write_checks_the_payload_shape() {
        let w = cell_write("q", "DECISION", Payload::Entry(r#"{"id":"d","title":"t"}"#.into()), Some("CLASS".into()), None, None)
            .expect("a DECISION entry");
        assert_eq!((w.cell, w.kind.as_deref()), (cell_type::DECISION, Some("CLASS")));
        let v = cell_write("q", "VECTOR", Payload::Vector(vec![0; 8]), None, Some("m".into()), Some(2)).expect("a vector");
        assert_eq!(v.cell, cell_type::VECTOR);
        let err = |r: Result<CellWrite, String>| r.expect_err("refused");
        assert!(err(cell_write("q", "NOPE", Payload::Entry("{}".into()), None, None, None)).contains("NOPE"));
        assert!(err(cell_write("q", "CONV", Payload::Vector(vec![0; 4]), None, None, None)).contains("bytes are a VECTOR"));
        assert!(err(cell_write("q", "VECTOR", Payload::Entry("{}".into()), None, None, None)).contains("bytes"));
        assert!(err(cell_write("q", "CONV", Payload::Entry("{}".into()), None, None, Some(2))).contains("dims"));
        assert!(err(cell_write("q", "CONV", Payload::Entry("not json".into()), None, None, None)).contains("not JSON"));
    }

    /// `set_cell` on a graph whose default layout is fresh: the sidecar, the
    /// layout and the live graph all carry the entry, and the marker names it.
    #[test]
    fn set_cell_writes_sidecar_layout_and_live_graph() {
        let root = repo("set");
        let r = built(&root);
        let dir = default_layout_dir(&root);
        write_merged_sharded(&r.merged, &dir).expect("persist");
        let mut live = r.merged;
        let a = set_cell_in(&mut live, &r.repo_roots, conv("first")).expect("write");
        let b = set_cell_in(&mut live, &r.repo_roots, conv("second")).expect("write");
        assert_eq!(b.write_through, WriteThrough::Applied);
        assert_eq!((a.entry_id.as_deref(), b.entry_id.as_deref()), (Some("000001"), Some("000002")));
        assert_eq!(
            marker("write", &b),
            "[cells] write qname=app::charge cell=CONV id=000002 rows=2 target=bound write_through=applied surface=pyo3"
        );
        assert!(b.hint.as_deref().is_some_and(|h| h.starts_with("v1|")), "{:?}", b.hint);
        let rebuilt = built(&root).merged;
        assert_eq!(conv_payload(&live), conv_payload(&rebuilt));
        assert_eq!(conv_payload(&read_merged_sharded(&dir).expect("read")), conv_payload(&rebuilt));
        assert!(!is_gmap_stale(&dir, &root));

        let gone = remove_cell_in(&mut live, &r.repo_roots, removal("app::charge", "CONV", "000001", "api").expect("removal"))
            .expect("remove")
            .expect("the row existed");
        assert_eq!((gone.rows, gone.target.as_str()), (1, "bound"));
        assert_eq!(conv_payload(&live), conv_payload(&built(&root).merged));
        let again = remove_cell_in(&mut live, &r.repo_roots, removal("app::charge", "CONV", "000001", "api").expect("removal"));
        assert_eq!(again, Ok(None));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A graph with no root for the node's repo refuses, pointing at the
    /// path-taking module function; nothing is written.
    #[test]
    fn set_cell_without_a_root_refuses() {
        let root = repo("noroot");
        let mut live = built(&root).merged;
        let err = set_cell_in(&mut live, &BTreeMap::new(), conv("x")).expect_err("no root");
        assert_eq!(err, NO_ROOT);
        assert!(!root.join(".glia").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The module functions take a path and no graph; a vector reads back
    /// through `node_cell_bytes`.
    #[test]
    fn module_functions_and_node_cell_bytes() {
        let root = repo("module");
        let path = root.to_str().expect("utf-8");
        let v = cell_write("app::charge", "VECTOR", Payload::Vector(vec![0, 0, 128, 63]), None, None, Some(1))
            .expect("a vector");
        let out = write_cell_at(path, &v).expect("write");
        assert_eq!((out.write_through, out.target.as_str(), out.entry_id.as_deref()), (WriteThrough::NoGmap, "pending", None));
        assert_eq!(marker("write", &out), "[cells] write qname=app::charge cell=VECTOR id=- rows=1 target=pending write_through=no_gmap surface=pyo3");
        let merged = built(&root).merged;
        let id = merged
            .graphs
            .iter()
            .flat_map(|g| g.nav.qname_by_id.iter())
            .find(|(_, q)| *q == "app::charge")
            .map(|(id, _)| id.0)
            .expect("app::charge");
        assert_eq!(node_cell_bytes_of(&merged, id, "VECTOR"), Ok(Some(vec![0, 0, 128, 63])));
        assert_eq!(node_cell_bytes_of(&merged, id, "CONV"), Ok(None));
        assert!(node_cell_bytes_of(&merged, id, "NOPE").is_err());
        let rm = removal("app::charge", "VECTOR", "", "api").expect("removal");
        assert!(remove_cell_at(path, &rm).expect("remove").is_some());
        assert_eq!(node_cell_bytes_of(&built(&root).merged, id, "VECTOR"), Ok(None));
        let _ = std::fs::remove_dir_all(&root);
    }
}
