//! The cell write API (LF.1b): [`write_cell`] and [`remove_cell_entry`] put a
//! CONSTRAINT / DECISION / CONV entry in `.glia/cells.jsonl` (a VECTOR in
//! `.glia/vectors.jsonl`), the durable home the build applies (LF.1a), and
//! write it through into a persisted layout when, and only when, that layout
//! is fresh right now.
//!
//! WHY FRESH-ONLY. Rewriting `manifest.json` moves its mtime, and
//! `is_gmap_stale`'s scan compares every source against that mtime: a source
//! edited between the build and the write would suddenly predate the manifest
//! and a stale graph would be served as fresh. So the write-through runs only
//! when [`is_gmap_stale`] is false immediately before the sidecar changes (the
//! build stamp, the `.glia` fingerprint and the mtime scan all agree the
//! layout matches the tree); every source then still predates the rewritten
//! manifest, which records the new sidecar's fingerprint, so the layout stays
//! fresh and holds exactly what a rebuild would build. A stale layout is left
//! alone: its next load rebuilds it anyway. The one residual race is a source
//! edit landing in the few milliseconds between that check and the manifest
//! rewrite.
//!
//! THE WRITE, in order: validate and normalise the entry (its `source`
//! defaults to `api`; a CONV without an `id` gets the next zero-padded 6-digit
//! id among its qname's rows of that source, append semantics, no clock; the
//! free-text `text` / `title` fields go through the A13.7 redaction, since
//! `cells.jsonl` is checked in); take `.glia/cells.lock`; check freshness;
//! when fresh, read the layout, bind the row through `graph::cells` (its
//! `hint` is the caller's, else the bound node's move-stable
//! `identity::Identity::hint`) and apply it; upsert the row (cells keyed by
//! `(qname, cell, source, id)`, vectors by `qname`), rows sorted by that key;
//! then rewrite the layout (only the touched shard's bytes change).
//!
//! Where the write lands is reported, never guessed: `target` is `bound`,
//! `rekeyed` (a HEURISTIC re-bind through the hint, with its `tier`),
//! `ambiguous` (several nodes, nothing applied), `orphaned` (no node), or
//! `pending` when no fresh layout was read (the next build binds the row).
//!
//! Scale: the write-through deserialises the whole layout, which suits single
//! tool writes; a bulk import writes the sidecar (no `gmap_dir`) and rebuilds.
//! The store prints nothing here: each surface prints its own
//! `[cells] write ...` marker from the outcome (pyo3 in `py/src/cells.rs`).

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use repo_graph_code_domain::external_inputs::{
    CELLS_FILE, CellRow, CellWrite, ENTRY_SOURCES, VECTORS_FILE, VectorRow, WRITABLE, WritePayload,
    b64_encode, canonical, check_vector, read_rows, remove_entry, validate_entry, write_rows,
};
use repo_graph_code_domain::snapshots::redact_untrusted;
use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::{CellTypeId, NodeId, RepoId};
use repo_graph_graph::MergedGraph;
use repo_graph_graph::cells::{CellTarget, QnameIndex, apply_cell_write};
use repo_graph_graph::identity::identity_of;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::StoreError;
use crate::layout::{
    LayoutMeta, MANIFEST_NAME, is_gmap_stale, read_layout_extras, read_merged_sharded_meta,
    repo_root_of_gmap_dir, write_merged_sharded_extras, write_merged_sharded_for_repo,
};

/// Repo-relative path of the lock every sidecar writer takes. Excluded from
/// the `.glia` fingerprint (`*.lock` is store output), so taking it never
/// makes a layout stale.
pub const CELLS_LOCK: &str = ".glia/cells.lock";
/// Attempts to take [`CELLS_LOCK`], [`LOCK_WAIT`] apart.
const LOCK_TRIES: u32 = 20;
const LOCK_WAIT: Duration = Duration::from_millis(25);
/// A lock this old belongs to a crashed writer and is removed.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
/// Entry fields that hold free text, redacted before they are stored.
const FREE_TEXT_FIELDS: [&str; 2] = ["text", "title"];
/// `target` when no fresh layout was read: the next build binds the row.
const PENDING: &str = "pending";

/// What a write did to the persisted layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WriteThrough {
    /// The layout was fresh and was rewritten to hold the write (also for an
    /// ambiguous row: the graph is unchanged but the manifest records the new
    /// sidecar, so the layout stays fresh).
    Applied,
    /// No layout: no `gmap_dir` was given, or it holds no manifest.
    NoGmap,
    /// The layout was stale (or unreadable) before the write and is left as
    /// is: its next load rebuilds it, sidecar included.
    GmapStale,
    /// The layout was fresh but no node matches the row; the layout is left
    /// as is (it now reads stale) and the next build reports the row orphaned.
    NodeMissing,
    /// The layout holds several repos and the caller named none, so there is
    /// no one repo to bind in; the layout is left as is.
    MultiRepo,
}

impl WriteThrough {
    /// The marker spelling (`applied`, `no_gmap`, ...), as serialised.
    pub fn as_str(self) -> &'static str {
        match self {
            WriteThrough::Applied => "applied",
            WriteThrough::NoGmap => "no_gmap",
            WriteThrough::GmapStale => "gmap_stale",
            WriteThrough::NodeMissing => "node_missing",
            WriteThrough::MultiRepo => "multi_repo",
        }
    }
}

/// What [`write_cell`] / [`remove_cell_entry`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct CellWriteOutcome {
    pub qname: String,
    /// Cell type NAME (`CONV`).
    pub cell: String,
    /// The row's NodeKind NAME filter, when it has one.
    pub kind: Option<String>,
    /// The move-stable hint stored with the row (`None` when neither the
    /// caller nor a fresh layout supplied one).
    pub hint: Option<String>,
    /// The entry's id; `None` for a VECTOR.
    pub entry_id: Option<String>,
    /// The entry as stored (defaulted, id-assigned, redacted); for a removal,
    /// the entry removed. `None` for a VECTOR.
    pub entry: Option<Value>,
    /// Rows in the sidecar file after the write.
    pub rows: usize,
    /// `bound` | `rekeyed` | `ambiguous` | `orphaned` | `pending` (see the
    /// module doc); a surface that applies the write in memory as well may
    /// report its own binding.
    pub target: String,
    /// The evidence tier of a `rekeyed` binding (`identical`, `same-name`).
    pub tier: Option<String>,
    /// The node the row bound (or the smallest candidate when `ambiguous`).
    pub node: Option<u64>,
    pub write_through: WriteThrough,
    /// Secret spans the A13.7 redaction replaced in the free text.
    pub redacted: usize,
}

impl CellWriteOutcome {
    /// Record where the row bound.
    pub fn set_target(&mut self, t: &CellTarget) {
        let (target, tier, node) = target_fields(t);
        self.target = target;
        self.tier = tier;
        self.node = node;
    }
}

/// `(target, tier, node)` of a binding.
fn target_fields(t: &CellTarget) -> (String, Option<String>, Option<u64>) {
    match t {
        CellTarget::Bound(id) => ("bound".into(), None, Some(id.0)),
        CellTarget::Rekeyed { id, tier } => ("rekeyed".into(), Some(tier.as_str().into()), Some(id.0)),
        CellTarget::Ambiguous(id) => ("ambiguous".into(), None, Some(id.0)),
        CellTarget::Orphaned => ("orphaned".into(), None, None),
        CellTarget::Rejected(_) => ("rejected".into(), None, None),
        _ => ("unknown".into(), None, None),
    }
}

/// One entry (or a node's VECTOR) to remove: built by
/// [`CellRemoval::entry`] / [`CellRemoval::vector`]. `kind` and `hint` bind it
/// in a graph ([`apply_cell_removal`]); [`remove_cell_entry`] takes them from
/// the row it removes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CellRemoval {
    pub qname: String,
    pub kind: Option<String>,
    pub hint: Option<String>,
    pub cell: CellTypeId,
    /// Ignored for a VECTOR.
    pub source: String,
    /// Ignored for a VECTOR.
    pub id: String,
}

impl CellRemoval {
    /// The `(source, id)` entry of `cell` on `qname`.
    pub fn entry(
        qname: impl Into<String>,
        cell: CellTypeId,
        source: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        CellRemoval {
            qname: qname.into(),
            kind: None,
            hint: None,
            cell,
            source: source.into(),
            id: id.into(),
        }
    }

    /// The VECTOR of `qname`.
    pub fn vector(qname: impl Into<String>) -> Self {
        CellRemoval::entry(qname, cell_type::VECTOR, "", "")
    }

    /// Bind only nodes of this NodeKind NAME.
    pub fn with_kind(mut self, kind: Option<String>) -> Self {
        self.kind = kind;
        self
    }

    /// The move-stable hint of the node meant.
    pub fn with_hint(mut self, hint: Option<String>) -> Self {
        self.hint = hint;
        self
    }
}

/// Write one cell for the repo at `repo_root` (see the module doc).
///
/// `gmap_dir` is the layout to keep coherent (normally
/// `default_gmap_dir(repo_root)`); `None` writes the sidecar only. `repo` is
/// the RepoId the row binds in: callers that hold a graph pass its repo. With
/// `None` the write-through binds in the layout's one repo, and reports
/// [`WriteThrough::MultiRepo`] for a layout of several. The store never
/// derives a repo's identity from its path.
///
/// Err ([`StoreError::Invalid`]) for a cell type outside `WRITABLE`, a
/// payload that does not fit its cell, an entry that breaks its rules, an
/// unknown kind name, or a sidecar with unreadable lines (a rewrite would drop
/// them); `Io` when the lock is held past its retries or a file cannot be
/// written. Nothing is written on any Err. A node's cell ORDER in a
/// written-through layout follows the write order (payloads equal a rebuild's;
/// a rebuild orders a node's sidecar cells by the sorted rows).
pub fn write_cell(
    repo_root: &Path,
    gmap_dir: Option<&Path>,
    repo: Option<RepoId>,
    w: &CellWrite,
) -> Result<CellWriteOutcome, StoreError> {
    let prepared = prepare(w)?;
    let _lock = CellsLock::take(repo_root)?;
    let mut sidecar = Sidecar::read(repo_root, w.cell)?;
    let entry = match (prepared.entry, &sidecar) {
        (Some(mut entry), Sidecar::Cells(_, rows)) => {
            if w.cell == cell_type::CONV && entry.get("id").is_none_or(Value::is_null) {
                let source = entry.get("source").and_then(Value::as_str).unwrap_or_default().to_string();
                entry.insert("id".into(), Value::String(next_conv_id(rows, &w.qname, &source)));
            }
            let entry = canonical(&Value::Object(entry));
            validate_entry(w.cell, &entry).map_err(invalid)?;
            Some(entry)
        }
        _ => None,
    };
    let stored = match &entry {
        Some(e) => CellWrite::entry(w.qname.clone(), w.cell, e.clone()).with_kind(w.kind.clone()),
        None => w.clone(),
    };

    let mut layout = Layout::open(repo_root, gmap_dir, repo);
    let (bound, hint) = match &mut layout.state {
        LayoutState::Fresh { merged, idx, .. } => {
            let hint = w.hint.clone().or_else(|| hint_of(merged, idx, &w.qname, w.kind.as_deref()));
            let target = apply_cell_write(merged, idx, &stored.clone().with_hint(hint.clone()))
                .map_err(invalid)?;
            if let CellTarget::Rejected(e) = &target {
                return Err(invalid(e.clone()));
            }
            (Some(target), hint)
        }
        LayoutState::Skip(_) => (None, w.hint.clone()),
    };

    sidecar.upsert(&stored.with_hint(hint.clone()))?;
    let rows = sidecar.save()?;
    let write_through = layout.finish(bound.as_ref())?;
    let mut out = CellWriteOutcome {
        qname: w.qname.clone(),
        cell: cell_name(w.cell).to_string(),
        kind: w.kind.clone(),
        hint,
        entry_id: entry.as_ref().and_then(|e| e.get("id")).and_then(Value::as_str).map(str::to_string),
        entry,
        rows,
        target: PENDING.into(),
        tier: None,
        node: None,
        write_through,
        redacted: prepared.redacted,
    };
    if let Some(t) = &bound {
        out.set_target(t);
    }
    Ok(out)
}

/// Remove one entry (or a node's VECTOR) from the sidecar, and from a fresh
/// layout the same way [`write_cell`] writes one; the row's own `kind` and
/// `hint` bind it. `Ok(None)` when the sidecar holds no such row: nothing is
/// written, the layout included (an entry that came from somewhere other than
/// the sidecar is not the API's to remove).
pub fn remove_cell_entry(
    repo_root: &Path,
    gmap_dir: Option<&Path>,
    repo: Option<RepoId>,
    r: &CellRemoval,
) -> Result<Option<CellWriteOutcome>, StoreError> {
    if !WRITABLE.contains(&r.cell) {
        return Err(not_writable(r.cell));
    }
    let _lock = CellsLock::take(repo_root)?;
    let mut sidecar = Sidecar::read(repo_root, r.cell)?;
    let Some((kind, hint, entry)) = sidecar.take(r) else {
        return Ok(None);
    };
    let removed = r.clone().with_kind(kind).with_hint(hint);

    let mut layout = Layout::open(repo_root, gmap_dir, repo);
    let bound = match &mut layout.state {
        LayoutState::Fresh { merged, idx, .. } => Some(apply_cell_removal(merged, idx, &removed)),
        LayoutState::Skip(_) => None,
    };
    let rows = sidecar.save()?;
    let write_through = layout.finish(bound.as_ref())?;
    let mut out = CellWriteOutcome {
        qname: removed.qname.clone(),
        cell: cell_name(removed.cell).to_string(),
        kind: removed.kind.clone(),
        hint: removed.hint.clone(),
        entry_id: entry.is_some().then(|| removed.id.clone()),
        entry,
        rows,
        target: PENDING.into(),
        tier: None,
        node: None,
        write_through,
        redacted: 0,
    };
    if let Some(t) = &bound {
        out.set_target(t);
    }
    Ok(Some(out))
}

/// Remove `r` from the node it binds in `merged` through `idx` (built over
/// this same graph): the `(source, id)` entry leaves the node's entry array,
/// and the cell goes when the array empties; a VECTOR cell goes whole. Every
/// instance of the node is changed. A payload that is not an entry array
/// belongs to an extractor and is left alone. Returns where it bound; only
/// `Bound` and `Rekeyed` changed anything.
pub fn apply_cell_removal(merged: &mut MergedGraph, idx: &QnameIndex, r: &CellRemoval) -> CellTarget {
    if !WRITABLE.contains(&r.cell) {
        return CellTarget::Rejected(format!("cell type {} is not externally writable", cell_name(r.cell)));
    }
    let target = idx.resolve(&r.qname, r.kind.as_deref(), r.hint.as_deref());
    let id = match target {
        CellTarget::Bound(id) | CellTarget::Rekeyed { id, .. } => id,
        other => return other,
    };
    for node in merged.graphs.iter_mut().flat_map(|g| g.nodes.iter_mut()).filter(|n| n.id == id) {
        let Some(at) = node.cells.iter().position(|c| c.kind == r.cell) else {
            continue;
        };
        let keep = if r.cell == cell_type::VECTOR {
            None
        } else {
            node.cells.get(at).and_then(|c| remove_entry(&c.payload, &r.source, &r.id))
        };
        match keep {
            Some(payload) => {
                if let Some(c) = node.cells.get_mut(at) {
                    c.payload = payload;
                }
            }
            None => {
                node.cells.remove(at);
            }
        }
    }
    target
}

// ---------------------------------------------------------------------------
// The layout side of one write
// ---------------------------------------------------------------------------

enum LayoutState {
    Fresh { dir: PathBuf, merged: MergedGraph, meta: LayoutMeta, idx: QnameIndex },
    Skip(WriteThrough),
}

/// A persisted layout, read when it is fresh, BEFORE the sidecar changes.
struct Layout<'a> {
    repo_root: &'a Path,
    state: LayoutState,
}

impl<'a> Layout<'a> {
    fn open(repo_root: &'a Path, gmap_dir: Option<&Path>, repo: Option<RepoId>) -> Self {
        let state = match gmap_dir {
            None => LayoutState::Skip(WriteThrough::NoGmap),
            Some(dir) if !dir.join(MANIFEST_NAME).is_file() => LayoutState::Skip(WriteThrough::NoGmap),
            Some(dir) if is_gmap_stale(dir, repo_root) => LayoutState::Skip(WriteThrough::GmapStale),
            Some(dir) => match read_merged_sharded_meta(dir) {
                // A layout that reads fresh but cannot be decoded needs the
                // rebuild its next load does.
                Err(_) => LayoutState::Skip(WriteThrough::GmapStale),
                Ok((merged, meta)) => {
                    let scope = match repo {
                        Some(r) => Some(Some(r)),
                        None => (repo_count(&merged) <= 1).then_some(None),
                    };
                    match scope {
                        None => LayoutState::Skip(WriteThrough::MultiRepo),
                        Some(scope) => {
                            let idx = QnameIndex::build(&merged, scope);
                            LayoutState::Fresh { dir: dir.to_path_buf(), merged, meta, idx }
                        }
                    }
                }
            },
        };
        Layout { repo_root, state }
    }

    /// Write the layout back when the row bound (or was ambiguous), after the
    /// sidecar changed, preserving everything its manifest records.
    fn finish(self, bound: Option<&CellTarget>) -> Result<WriteThrough, StoreError> {
        let (dir, merged, meta) = match self.state {
            LayoutState::Skip(w) => return Ok(w),
            LayoutState::Fresh { dir, merged, meta, .. } => (dir, merged, meta),
        };
        match bound {
            Some(CellTarget::Bound(_) | CellTarget::Rekeyed { .. } | CellTarget::Ambiguous(_)) => {}
            _ => return Ok(WriteThrough::NodeMissing),
        }
        let rooted = meta.repos.iter().any(|r| r.root.is_some());
        if repo_root_of_gmap_dir(&dir).is_some() || rooted {
            // The writer that made this layout records the fingerprint of the
            // default dir's repo, of the one recorded root, or of each of
            // several (LC.10b), and carries foreign shards and merge members.
            let extras = read_layout_extras(&dir)?;
            write_merged_sharded_extras(&merged, &meta, &extras.foreign, &extras.members, &dir)?;
        } else {
            write_merged_sharded_for_repo(&merged, &meta, &dir, self.repo_root)?;
        }
        Ok(WriteThrough::Applied)
    }
}

fn repo_count(merged: &MergedGraph) -> usize {
    let mut repos: Vec<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    repos.sort_unstable();
    repos.dedup();
    repos.len()
}

/// The move-stable hint of the node `qname` binds to exactly.
fn hint_of(merged: &MergedGraph, idx: &QnameIndex, qname: &str, kind: Option<&str>) -> Option<String> {
    let id: NodeId = match idx.resolve(qname, kind, None) {
        CellTarget::Bound(id) => id,
        _ => return None,
    };
    identity_of(merged, id).map(|i| i.hint())
}

// ---------------------------------------------------------------------------
// The sidecar files
// ---------------------------------------------------------------------------

/// One sidecar file and its rows, read under the lock.
enum Sidecar {
    /// `.glia/cells.jsonl`: CONSTRAINT / DECISION / CONV entry rows.
    Cells(PathBuf, Vec<CellRow>),
    /// `.glia/vectors.jsonl`: one VECTOR row per qname.
    Vectors(PathBuf, Vec<VectorRow>),
}

impl Sidecar {
    /// The file `cell` lives in, with its rows; Err on unreadable lines.
    fn read(repo_root: &Path, cell: CellTypeId) -> Result<Self, StoreError> {
        Ok(if cell == cell_type::VECTOR {
            let path = repo_root.join(VECTORS_FILE);
            let rows = readable_rows(&path)?;
            Sidecar::Vectors(path, rows)
        } else {
            let path = repo_root.join(CELLS_FILE);
            let rows = readable_rows(&path)?;
            Sidecar::Cells(path, rows)
        })
    }

    /// Replace the row with `w`'s key, else add it (`w` is normalised: an
    /// entry carries its source and id).
    fn upsert(&mut self, w: &CellWrite) -> Result<(), StoreError> {
        match (self, &w.payload) {
            (Sidecar::Cells(_, rows), WritePayload::Entry(entry)) => {
                let row = CellRow {
                    qname: w.qname.clone(),
                    kind: w.kind.clone(),
                    hint: w.hint.clone(),
                    cell: cell_name(w.cell).to_string(),
                    entry: entry.clone(),
                };
                let key = row_key(&row);
                rows.retain(|r| row_key(r) != key);
                rows.push(row);
            }
            (Sidecar::Vectors(_, rows), WritePayload::Vector { bytes, model, dims }) => {
                rows.retain(|r| r.qname != w.qname);
                rows.push(VectorRow {
                    qname: w.qname.clone(),
                    kind: w.kind.clone(),
                    hint: w.hint.clone(),
                    model: model.clone(),
                    dims: *dims,
                    b64: b64_encode(bytes),
                });
            }
            _ => return Err(invalid("the payload does not fit its sidecar file".to_string())),
        }
        Ok(())
    }

    /// Remove the row `r` names: `(kind, hint, entry)` of the row removed
    /// (`entry` is `None` for a vector), or `None` when there is none.
    fn take(&mut self, r: &CellRemoval) -> Option<(Option<String>, Option<String>, Option<Value>)> {
        match self {
            Sidecar::Cells(_, rows) => {
                let key = (r.qname.clone(), cell_name(r.cell).to_string(), r.source.clone(), r.id.clone());
                let at = rows.iter().position(|x| row_key(x) == key)?;
                let row = rows.remove(at);
                Some((row.kind, row.hint, Some(row.entry)))
            }
            Sidecar::Vectors(_, rows) => {
                let at = rows.iter().position(|x| x.qname == r.qname)?;
                let row = rows.remove(at);
                Some((row.kind, row.hint, None))
            }
        }
    }

    /// Sort the rows by their key and write the file (tmp + rename): the
    /// number of rows written.
    fn save(&mut self) -> Result<usize, StoreError> {
        match self {
            Sidecar::Cells(path, rows) => {
                rows.sort_by_key(row_key);
                write_rows(path, rows)?;
                Ok(rows.len())
            }
            Sidecar::Vectors(path, rows) => {
                rows.sort_by(|a, b| a.qname.cmp(&b.qname));
                write_rows(path, rows)?;
                Ok(rows.len())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Normalising a write
// ---------------------------------------------------------------------------

struct Prepared {
    /// The entry object (source defaulted, free text redacted, id maybe
    /// missing); `None` for a VECTOR.
    entry: Option<Map<String, Value>>,
    redacted: usize,
}

fn prepare(w: &CellWrite) -> Result<Prepared, StoreError> {
    if !WRITABLE.contains(&w.cell) {
        return Err(not_writable(w.cell));
    }
    if let Some(k) = &w.kind
        && !node_kind::ALL.iter().any(|(_, n)| n == k)
    {
        return Err(invalid(format!("unknown node kind {k:?}")));
    }
    let is_vector = w.cell == cell_type::VECTOR;
    match &w.payload {
        WritePayload::Entry(v) if !is_vector => {
            let mut entry = v
                .as_object()
                .cloned()
                .ok_or_else(|| invalid(format!("a {} entry must be a JSON object", cell_name(w.cell))))?;
            if entry.get("source").is_none_or(Value::is_null) {
                entry.insert("source".into(), Value::String(ENTRY_SOURCES[0].into()));
            }
            let mut redacted = 0;
            for field in FREE_TEXT_FIELDS {
                if let Some(Value::String(s)) = entry.get_mut(field) {
                    let (clean, n) = redact_untrusted(s);
                    *s = clean;
                    redacted += n;
                }
            }
            Ok(Prepared { entry: Some(entry), redacted })
        }
        WritePayload::Vector { bytes, dims, .. } if is_vector => {
            check_vector(bytes, *dims).map_err(invalid)?;
            Ok(Prepared { entry: None, redacted: 0 })
        }
        _ => Err(invalid(format!(
            "cell type {} does not take this payload: VECTOR takes bytes, CONSTRAINT / DECISION / CONV a JSON entry",
            cell_name(w.cell)
        ))),
    }
}

/// The next id among `qname`'s CONV rows of `source`: one past the largest
/// all-digit id, zero-padded to 6.
fn next_conv_id(rows: &[CellRow], qname: &str, source: &str) -> String {
    let conv = cell_name(cell_type::CONV);
    let last = rows
        .iter()
        .filter(|r| r.qname == qname && r.cell == conv)
        .filter(|r| r.entry.get("source").and_then(Value::as_str) == Some(source))
        .filter_map(|r| r.entry.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|id| id.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{:06}", last.saturating_add(1))
}

/// `(qname, cell, source, id)`, the key a cell row is upserted and sorted by.
fn row_key(r: &CellRow) -> (String, String, String, String) {
    let field = |k: &str| r.entry.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    (r.qname.clone(), r.cell.clone(), field("source"), field("id"))
}

/// A sidecar's rows, refusing a file with unreadable lines: rewriting it would
/// drop them.
fn readable_rows<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, StoreError> {
    let (rows, errors) = read_rows::<T>(path);
    match errors.first() {
        None => Ok(rows),
        Some(first) => Err(invalid(format!(
            "{} has {} unreadable line(s) (first: {first}); fix or remove them before writing",
            path.display(),
            errors.len()
        ))),
    }
}

fn cell_name(c: CellTypeId) -> &'static str {
    cell_type::ALL.iter().find(|(id, _)| *id == c).map_or("?", |(_, n)| n)
}

fn invalid(msg: String) -> StoreError {
    StoreError::Invalid(msg)
}

fn not_writable(c: CellTypeId) -> StoreError {
    let names: Vec<&str> = WRITABLE.iter().map(|w| cell_name(*w)).collect();
    invalid(format!(
        "cell type {} is not writable (WRITABLE: {}); every other type is owned by an extractor or a build pass",
        cell_name(c),
        names.join(" | ")
    ))
}

// ---------------------------------------------------------------------------
// The sidecar lock
// ---------------------------------------------------------------------------

/// `.glia/cells.lock`, held for one write and removed on drop.
struct CellsLock {
    path: PathBuf,
}

impl CellsLock {
    /// Create the lock file exclusively: [`LOCK_TRIES`] attempts
    /// [`LOCK_WAIT`] apart; a lock older than [`LOCK_STALE_AFTER`] is a
    /// crashed writer's and is removed first.
    fn take(repo_root: &Path) -> Result<Self, StoreError> {
        let path = repo_root.join(CELLS_LOCK);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        for attempt in 0..LOCK_TRIES {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    // The holder's pid, for a human reading a stuck lock.
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(CellsLock { path });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok());
                    if age.is_some_and(|a| a > LOCK_STALE_AFTER) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if attempt + 1 < LOCK_TRIES {
                        std::thread::sleep(LOCK_WAIT);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(StoreError::Io(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("{} is held by another writer (retry, or remove it if no writer is running)", path.display()),
        )))
    }
}

impl Drop for CellsLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(qname: &str, cell: &str, source: &str, id: &str) -> CellRow {
        CellRow {
            qname: qname.into(),
            kind: None,
            hint: None,
            cell: cell.into(),
            entry: json!({"source": source, "id": id, "text": "t"}),
        }
    }

    #[test]
    fn next_conv_id_counts_digit_ids_of_one_qname_and_source() {
        let rows = vec![
            row("a", "CONV", "api", "000007"),
            row("a", "CONV", "api", "note"),
            row("a", "CONV", "adr", "000050"),
            row("a", "DECISION", "api", "000090"),
            row("b", "CONV", "api", "000070"),
        ];
        assert_eq!(next_conv_id(&rows, "a", "api"), "000008");
        assert_eq!(next_conv_id(&rows, "c", "api"), "000001");
        assert_eq!(next_conv_id(&rows, "a", "adr"), "000051");
    }

    #[test]
    fn prepare_defaults_source_and_redacts_free_text() {
        let w = CellWrite::entry(
            "a",
            cell_type::DECISION,
            json!({"id": "d", "title": "use sk_live_abcdefghijklmn", "text": "plain"}),
        );
        let p = prepare(&w).unwrap();
        let e = p.entry.unwrap();
        assert_eq!(e.get("source"), Some(&json!("api")));
        assert_eq!(p.redacted, 1);
        assert!(!e["title"].as_str().unwrap().contains("sk_live_"));
        assert_eq!(e.get("text"), Some(&json!("plain")));
        let given = CellWrite::entry("a", cell_type::CONV, json!({"source": "adr", "text": "x"}));
        assert_eq!(prepare(&given).unwrap().entry.unwrap().get("source"), Some(&json!("adr")));
    }

    #[test]
    fn prepare_refuses_shapes_that_do_not_fit() {
        let msg = |w: CellWrite| prepare(&w).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(msg(CellWrite::entry("a", cell_type::CODE, json!({}))).contains("WRITABLE"));
        assert!(msg(CellWrite::entry("a", cell_type::CONV, json!("text"))).contains("JSON object"));
        assert!(msg(CellWrite::vector("a", vec![], None, None)).contains("bytes"));
        assert!(msg(CellWrite::vector("a", vec![0; 8], None, Some(3))).contains("dims=3"));
        let mut entry_on_vector = CellWrite::vector("a", vec![0; 4], None, None);
        entry_on_vector.payload = WritePayload::Entry(json!({}));
        assert!(msg(entry_on_vector).contains("VECTOR takes bytes"));
        let kind = CellWrite::entry("a", cell_type::CONV, json!({"text": "x"})).with_kind(Some("NOPE".into()));
        assert!(msg(kind).contains("NOPE"));
    }

    #[test]
    fn write_through_spells_as_serialised() {
        for w in [
            WriteThrough::Applied,
            WriteThrough::NoGmap,
            WriteThrough::GmapStale,
            WriteThrough::NodeMissing,
            WriteThrough::MultiRepo,
        ] {
            assert_eq!(serde_json::to_value(w).unwrap(), json!(w.as_str()));
        }
    }
}
