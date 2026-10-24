//! The cell write API (LF.1b): [`write_cell`] and [`remove_cell_entry`] put a
//! CONSTRAINT / DECISION / CONV entry in `.glia/cells.jsonl` (a VECTOR in
//! `.glia/vectors.jsonl`), the durable home the build applies (LF.1a), and
//! write it through into a persisted layout when, and only when, that layout
//! is fresh right now.
//!
//! A VECTOR is the exception (FB.3): vectors are query-time, so the build
//! never applies `vectors.jsonl` and a layout holds no VECTOR cell. A VECTOR
//! write or removal changes the sidecar only and reports
//! [`WriteThrough::SidecarOnly`]; a fresh layout is still read to bind the
//! qname for the report (`target` bound / ambiguous / orphaned as for any
//! cell), but the gmap is never rewritten (a write-through would put back
//! what the next rebuild drops). The vectors file is outside the layout
//! fingerprint, so the layout stays fresh after the write.
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
//! `(qname, cell, source, id)`, vectors by `(qname, kind)`, FB.2), rows
//! sorted by that key;
//! then rewrite the layout (only the touched shard's bytes change).
//!
//! Where the write lands is reported, never guessed: `target` is `bound`,
//! `rekeyed` (a HEURISTIC re-bind through the hint, with its `tier`),
//! `hint_differs` (bound by the qname and applied, as the next build applies
//! it, but the caller's hint is not the bound node's own: `node_hint` and
//! `hint_differs` say which fields differ), `ambiguous` (several nodes,
//! nothing applied), `orphaned` (no node), or `pending` when no fresh layout
//! was read (the next build binds the row; there is no node to compare a
//! hint against). The binding rule is the build's (`QnameIndex::resolve`):
//! the exact qname (and kind) wins, and the hint only narrows several qname
//! matches or re-binds a moved node, so a disagreeing hint is reported, never
//! obeyed against the qname.
//!
//! A caller's hint must be an `identity::Identity` v1 hint
//! (`v1|<kind id>|<file>|<body:016x>|<local>`) naming a registered node kind
//! that agrees with the write's `kind`; any other is refused before the lock
//! (`StoreError::Invalid`), so nothing is written. Rows already in a
//! hand-edited sidecar are not re-validated: the build and `ls --check` keep
//! ignoring a hint they cannot parse.
//!
//! Scale: the write-through deserialises the whole layout, which suits single
//! tool writes; a bulk import writes the sidecar (no `gmap_dir`) and rebuilds.
//! The store prints nothing here: each surface prints its own
//! `[cells] write ...` marker from the outcome (pyo3 in `py/src/cells.rs`).

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use glia_code_domain::external_inputs::{
    CELLS_FILE, CellRow, CellWrite, ENTRY_SOURCES, VECTORS_FILE, VectorRow, WRITABLE, WritePayload,
    b64_decode, b64_encode, canonical, check_vector, read_rows, remove_entry, text_hash_ok,
    validate_entry, vector_key, write_rows,
};
use glia_code_domain::snapshots::redact_untrusted;
use glia_code_domain::{cell_type, node_kind};
use glia_core::{CellTypeId, NodeId, RepoId};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex, apply_cell_write};
use glia_graph::identity::{Identity, identity_of};
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
/// `target` when the row bound by its qname and the bound node's own v1 hint
/// differs from the caller's.
const HINT_DIFFERS: &str = "hint_differs";
/// The `hint_differs` field of a bound node with no v1 identity (an
/// ENDPOINT, a ROUTE: no MODULE ancestor or no POSITION file).
const NO_IDENTITY: &str = "no_identity";

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
    /// A VECTOR write or removal (FB.3): the sidecar changed and the layout
    /// was never written, since layouts hold no VECTOR cells.
    SidecarOnly,
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
            WriteThrough::SidecarOnly => "sidecar_only",
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
    /// `bound` | `rekeyed` | `hint_differs` | `ambiguous` | `orphaned` |
    /// `pending` (see the module doc); a surface that applies the write in
    /// memory as well may report its own binding ([`Self::set_binding`]).
    /// `hint_differs`: bound by the qname and applied, as the next build
    /// applies it, but the bound node's own hint differs from the caller's
    /// (see `node_hint` / `hint_differs`).
    pub target: String,
    /// The evidence tier of a `rekeyed` binding (`identical`, `same-name`).
    pub tier: Option<String>,
    /// The node the row bound (or the smallest candidate when `ambiguous`).
    pub node: Option<u64>,
    pub write_through: WriteThrough,
    /// Secret spans the A13.7 redaction replaced in the free text.
    pub redacted: usize,
    /// The bound (or rekeyed) node's own v1 hint, read from the layout the
    /// write bound in; `None` with no fresh layout, no node, or a node with no
    /// v1 identity.
    pub node_hint: Option<String>,
    /// The caller's hint fields that differ from the bound node's own, in the
    /// order `kind`, `file`, `body`, `local` (`no_identity` when the node has
    /// none); empty unless `target` is `hint_differs`.
    pub hint_differs: Vec<String>,
}

impl CellWriteOutcome {
    /// Record where the row bound.
    pub fn set_target(&mut self, t: &CellTarget) {
        let (target, tier, node) = target_fields(t);
        self.target = target;
        self.tier = tier;
        self.node = node;
    }

    /// Record where the row bound in `merged` and compare `caller_hint` (the
    /// hint the caller passed, not one filled from the graph) with the bound
    /// node's own: `node_hint`, `hint_differs`, and `target` `hint_differs`
    /// when they differ. A surface that applies the write to its own graph
    /// calls this with that graph's binding.
    pub fn set_binding(&mut self, merged: &MergedGraph, t: &CellTarget, caller_hint: Option<&str>) {
        self.set_target(t);
        self.set_hint_report(hint_report(merged, t, caller_hint));
    }

    fn set_hint_report(&mut self, (node_hint, differs): (Option<String>, Vec<String>)) {
        self.node_hint = node_hint;
        if !differs.is_empty() {
            self.target = HINT_DIFFERS.into();
        }
        self.hint_differs = differs;
    }
}

/// `(node_hint, hint_differs)` of a binding: the bound or rekeyed node's own
/// v1 hint, and, only for a `Bound` row whose caller gave a hint, the fields
/// that differ (`kind`, `file`, `body`, `local`; `no_identity` for a node
/// with none). A rekeyed row keeps its own target and tier: the hint did the
/// binding there. Ambiguous / orphaned / rejected rows have no node.
fn hint_report(
    merged: &MergedGraph,
    t: &CellTarget,
    caller: Option<&str>,
) -> (Option<String>, Vec<String>) {
    let (id, bound) = match t {
        CellTarget::Bound(id) => (*id, true),
        CellTarget::Rekeyed { id, .. } => (*id, false),
        _ => return (None, Vec::new()),
    };
    let own = identity_of(merged, id);
    let node_hint = own.as_ref().map(Identity::hint);
    let differs = match (bound, caller) {
        (true, Some(h)) => match (&own, Identity::parse_hint(h)) {
            (None, _) => vec![NO_IDENTITY.to_string()],
            (Some(own), Some(given)) => differing_fields(own, &given),
            // write_cell refuses an unparseable caller hint; a surface
            // passing one anyway learns nothing it can compare.
            (Some(_), None) => Vec::new(),
        },
        _ => Vec::new(),
    };
    (node_hint, differs)
}

/// The fields of `given` that differ from `own`, in hint order.
fn differing_fields(own: &Identity, given: &Identity) -> Vec<String> {
    [
        ("kind", own.kind != given.kind),
        ("file", own.file != given.file),
        ("body", own.body != given.body),
        ("local", own.local != given.local),
    ]
    .into_iter()
    .filter(|(_, d)| *d)
    .map(|(f, _)| f.to_string())
    .collect()
}

/// `(target, tier, node)` of a binding.
fn target_fields(t: &CellTarget) -> (String, Option<String>, Option<u64>) {
    match t {
        CellTarget::Bound(id) => ("bound".into(), None, Some(id.0)),
        CellTarget::Rekeyed { id, tier } => {
            ("rekeyed".into(), Some(tier.as_str().into()), Some(id.0))
        }
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

    /// The VECTOR row of `qname`; with [`CellRemoval::with_kind`], the row of
    /// that kind (a kind-less removal matches only a kind-less row, FB.2).
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
                let source = entry
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                entry.insert(
                    "id".into(),
                    Value::String(next_conv_id(rows, &w.qname, &source)),
                );
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
    let (bound, hint, report) = match &mut layout.state {
        LayoutState::Fresh { merged, idx, .. } => {
            let hint = w
                .hint
                .clone()
                .or_else(|| hint_of(merged, idx, &w.qname, w.kind.as_deref()));
            let target = apply_cell_write(merged, idx, &stored.clone().with_hint(hint.clone()))
                .map_err(invalid)?;
            if let CellTarget::Rejected(e) = &target {
                return Err(invalid(e.clone()));
            }
            // Compared while the layout is held (`finish` consumes it), with
            // the caller's hint only: a `hint_of` fill is the node's own.
            let report = hint_report(merged, &target, w.hint.as_deref());
            (Some(target), hint, Some(report))
        }
        LayoutState::Skip(_) => (None, w.hint.clone(), None),
    };

    sidecar.upsert(&stored.with_hint(hint.clone()))?;
    let rows = sidecar.save()?;
    let write_through = finish(layout, w.cell, bound.as_ref())?;
    let mut out = CellWriteOutcome {
        qname: w.qname.clone(),
        cell: cell_name(w.cell).to_string(),
        kind: w.kind.clone(),
        hint,
        entry_id: entry
            .as_ref()
            .and_then(|e| e.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        entry,
        rows,
        target: PENDING.into(),
        tier: None,
        node: None,
        write_through,
        redacted: prepared.redacted,
        node_hint: None,
        hint_differs: Vec::new(),
    };
    if let Some(t) = &bound {
        out.set_target(t);
    }
    if let Some(report) = report {
        out.set_hint_report(report);
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
    let write_through = finish(layout, removed.cell, bound.as_ref())?;
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
        node_hint: None,
        hint_differs: Vec::new(),
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
pub fn apply_cell_removal(
    merged: &mut MergedGraph,
    idx: &QnameIndex,
    r: &CellRemoval,
) -> CellTarget {
    if !WRITABLE.contains(&r.cell) {
        return CellTarget::Rejected(format!(
            "cell type {} is not externally writable",
            cell_name(r.cell)
        ));
    }
    let target = idx.resolve(&r.qname, r.kind.as_deref(), r.hint.as_deref());
    let id = match target {
        CellTarget::Bound(id) | CellTarget::Rekeyed { id, .. } => id,
        other => return other,
    };
    for node in merged
        .graphs
        .iter_mut()
        .flat_map(|g| g.nodes.iter_mut())
        .filter(|n| n.id == id)
    {
        let Some(at) = node.cells.iter().position(|c| c.kind == r.cell) else {
            continue;
        };
        let keep = if r.cell == cell_type::VECTOR {
            None
        } else {
            node.cells
                .get(at)
                .and_then(|c| remove_entry(&c.payload, &r.source, &r.id))
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

/// The write-through of one write of `cell`: a VECTOR never writes the
/// layout (its in-memory copy only bound the qname, FB.3); any other cell
/// goes through [`Layout::finish`].
fn finish(
    layout: Layout<'_>,
    cell: CellTypeId,
    bound: Option<&CellTarget>,
) -> Result<WriteThrough, StoreError> {
    if cell == cell_type::VECTOR {
        return Ok(WriteThrough::SidecarOnly);
    }
    layout.finish(bound)
}

enum LayoutState {
    Fresh {
        dir: PathBuf,
        merged: Box<MergedGraph>,
        meta: LayoutMeta,
        idx: Box<QnameIndex>,
    },
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
            Some(dir) if !dir.join(MANIFEST_NAME).is_file() => {
                LayoutState::Skip(WriteThrough::NoGmap)
            }
            Some(dir) if is_gmap_stale(dir, repo_root) => {
                LayoutState::Skip(WriteThrough::GmapStale)
            }
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
                            let idx = Box::new(QnameIndex::build(&merged, scope));
                            let merged = Box::new(merged);
                            LayoutState::Fresh {
                                dir: dir.to_path_buf(),
                                merged,
                                meta,
                                idx,
                            }
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
            LayoutState::Fresh {
                dir, merged, meta, ..
            } => (dir, merged, meta),
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
fn hint_of(
    merged: &MergedGraph,
    idx: &QnameIndex,
    qname: &str,
    kind: Option<&str>,
) -> Option<String> {
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
    /// `.glia/vectors.jsonl`: one VECTOR row per `(qname, kind)` (FB.2).
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
                let row = VectorRow {
                    qname: w.qname.clone(),
                    kind: w.kind.clone(),
                    hint: w.hint.clone(),
                    model: model.clone(),
                    dims: *dims,
                    // The cell API carries no text hash; FB.5a's bulk writer does.
                    text_hash: None,
                    b64: b64_encode(bytes),
                };
                let key = vector_key(&row);
                rows.retain(|r| vector_key(r) != key);
                rows.push(row);
            }
            _ => {
                return Err(invalid(
                    "the payload does not fit its sidecar file".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Remove the row `r` names: `(kind, hint, entry)` of the row removed
    /// (`entry` is `None` for a vector), or `None` when there is none.
    fn take(&mut self, r: &CellRemoval) -> Option<(Option<String>, Option<String>, Option<Value>)> {
        match self {
            Sidecar::Cells(_, rows) => {
                let key = (
                    r.qname.clone(),
                    cell_name(r.cell).to_string(),
                    r.source.clone(),
                    r.id.clone(),
                );
                let at = rows.iter().position(|x| row_key(x) == key)?;
                let row = rows.remove(at);
                Some((row.kind, row.hint, Some(row.entry)))
            }
            Sidecar::Vectors(_, rows) => {
                // (qname, kind): a kind-less removal keeps rows that carry a kind.
                let key = (r.qname.clone(), r.kind.clone().unwrap_or_default());
                let at = rows.iter().position(|x| vector_key(x) == key)?;
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
                rows.sort_by_key(vector_key);
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
    if let Some(h) = &w.hint {
        check_hint(h, w.kind.as_deref())?;
    }
    let is_vector = w.cell == cell_type::VECTOR;
    match &w.payload {
        WritePayload::Entry(v) if !is_vector => {
            let mut entry = v.as_object().cloned().ok_or_else(|| {
                invalid(format!(
                    "a {} entry must be a JSON object",
                    cell_name(w.cell)
                ))
            })?;
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
            Ok(Prepared {
                entry: Some(entry),
                redacted,
            })
        }
        WritePayload::Vector { bytes, dims, .. } if is_vector => {
            check_vector(bytes, *dims).map_err(invalid)?;
            Ok(Prepared {
                entry: None,
                redacted: 0,
            })
        }
        _ => Err(invalid(format!(
            "cell type {} does not take this payload: VECTOR takes bytes, CONSTRAINT / DECISION / CONV a JSON entry",
            cell_name(w.cell)
        ))),
    }
}

/// A caller's hint must be a v1 [`Identity`] hint naming a registered node
/// kind, and that kind must be the write's `kind` when it names one (a hint
/// of the other kind would be ignored on re-bind while `kind` picked the
/// other twin, with no word).
fn check_hint(h: &str, kind: Option<&str>) -> Result<(), StoreError> {
    let ident = Identity::parse_hint(h).ok_or_else(|| {
        invalid(format!(
            "hint {h:?} is not a v1 identity hint (v1|<kind id>|<file>|<body:016x>|<local>, the form `glia cell ls --json` and glia-export-engram's <out>.hints.jsonl carry)"
        ))
    })?;
    let name = node_kind::ALL
        .iter()
        .find(|(id, _)| *id == ident.kind)
        .map(|(_, n)| *n)
        .ok_or_else(|| {
            invalid(format!(
                "hint {h:?} names node kind id {}, which is not registered",
                ident.kind.0
            ))
        })?;
    match kind {
        Some(k) if k != name => Err(invalid(format!(
            "hint {h:?} names a {name} but kind is {k}"
        ))),
        _ => Ok(()),
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
    let field = |k: &str| {
        r.entry
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (
        r.qname.clone(),
        r.cell.clone(),
        field("source"),
        field("id"),
    )
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
    cell_type::ALL
        .iter()
        .find(|(id, _)| *id == c)
        .map_or("?", |(_, n)| n)
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
// The bulk vector writer (FB.5a)
// ---------------------------------------------------------------------------

/// How [`write_vector_rows`] treats what `.glia/vectors.jsonl` already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VectorWriteMode {
    /// The file must hold rows of the incoming rows' model only (re-read
    /// under the lock): a row of another model, or one naming none, is an
    /// Err and nothing is written. The incoming rows are then the file.
    MergeSameModel,
    /// Write the incoming rows whatever the file held.
    Replace,
}

/// Rewrite `<repo_root>/.glia/vectors.jsonl` with `rows` in one write under
/// `.glia/cells.lock` (the embed run's bulk writer; the cell API's
/// [`write_cell`] takes the lock once per row). The rows are the caller's
/// whole file: they must name one model, carry no two rows of one
/// `vector_key`, hold a well-formed `text_hash` when they record one and a
/// payload of `dims * 4` bytes. Under [`VectorWriteMode::MergeSameModel`] the
/// file is re-read after the lock is taken, so a writer of another model that
/// landed since the caller read it is refused rather than overwritten. Rows
/// are written sorted by `vector_key` through `write_rows` (tmp + rename);
/// `.glia/` is created when absent, its `.gitignore` never (history sync owns
/// it, and it already lists `vectors.jsonl`). Returns the rows written.
pub fn write_vector_rows(
    repo_root: &Path,
    mut rows: Vec<VectorRow>,
    mode: VectorWriteMode,
) -> Result<usize, StoreError> {
    let model = check_vector_rows(&rows)?;
    let _lock = CellsLock::take(repo_root)?;
    let path = repo_root.join(VECTORS_FILE);
    if mode == VectorWriteMode::MergeSameModel {
        let on_disk: Vec<VectorRow> = readable_rows(&path)?;
        if let Some(other) = on_disk
            .iter()
            .find(|r| model.is_some() && r.model.as_deref() != model)
        {
            return Err(invalid(format!(
                "{} holds vectors of model {} (row {:?}); rewrite it with Replace",
                path.display(),
                other.model.as_deref().unwrap_or("<none>"),
                other.qname
            )));
        }
    }
    rows.sort_by_key(vector_key);
    write_rows(&path, &rows)?;
    Ok(rows.len())
}

/// The one model `rows` name (None for no rows), or Err on a row that breaks
/// [`write_vector_rows`]' contract.
fn check_vector_rows(rows: &[VectorRow]) -> Result<Option<&str>, StoreError> {
    let mut model: Option<&str> = None;
    let mut keys: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::with_capacity(rows.len());
    for r in rows {
        let Some(m) = r.model.as_deref() else {
            return Err(invalid(format!("vector row {:?} names no model", r.qname)));
        };
        match model {
            Some(have) if have != m => {
                return Err(invalid(format!(
                    "vector rows name two models ({have} and {m}); one model per file"
                )));
            }
            _ => model = Some(m),
        }
        if let Some(h) = &r.text_hash
            && !text_hash_ok(h)
        {
            return Err(invalid(format!(
                "vector row {:?} has a malformed text_hash {h:?}",
                r.qname
            )));
        }
        let bytes =
            b64_decode(&r.b64).map_err(|e| invalid(format!("vector row {:?}: {e}", r.qname)))?;
        check_vector(&bytes, r.dims)
            .map_err(|e| invalid(format!("vector row {:?}: {e}", r.qname)))?;
        if !keys.insert(vector_key(r)) {
            return Err(invalid(format!(
                "two vector rows for {:?} (kind {:?})",
                r.qname,
                r.kind.as_deref().unwrap_or("")
            )));
        }
    }
    Ok(model)
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
            format!(
                "{} is held by another writer (retry, or remove it if no writer is running)",
                path.display()
            ),
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
        assert_eq!(
            prepare(&given).unwrap().entry.unwrap().get("source"),
            Some(&json!("adr"))
        );
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
        let kind = CellWrite::entry("a", cell_type::CONV, json!({"text": "x"}))
            .with_kind(Some("NOPE".into()));
        assert!(msg(kind).contains("NOPE"));
    }

    #[test]
    fn prepare_refuses_a_hint_that_is_not_v1_or_contradicts_kind() {
        let msg = |w: CellWrite| prepare(&w).err().map(|e| e.to_string()).unwrap_or_default();
        let conv = |hint: &str| {
            CellWrite::entry("a", cell_type::CONV, json!({"text": "x"}))
                .with_hint(Some(hint.into()))
        };
        assert!(msg(conv("a.py:1:0")).contains("is not a v1 identity hint"));
        assert!(msg(conv("v2|3|a.py|0000000000000000|f")).contains("is not a v1 identity hint"));
        assert!(
            msg(conv("v1|999|a.py|0000000000000000|f"))
                .contains("node kind id 999, which is not registered")
        );
        let function = "v1|3|a.py|0000000000000000|f";
        assert!(
            msg(conv(function).with_kind(Some("CLASS".into())))
                .contains("names a FUNCTION but kind is CLASS")
        );
        assert!(prepare(&conv(function)).is_ok());
        assert!(prepare(&conv(function).with_kind(Some("FUNCTION".into()))).is_ok());
    }

    #[test]
    fn differing_fields_come_in_hint_order() {
        let own = Identity {
            kind: node_kind::FUNCTION,
            file: "a.py".into(),
            local: "f".into(),
            body: 0,
        };
        assert!(differing_fields(&own, &own).is_empty());
        let given = Identity {
            kind: node_kind::CLASS,
            file: "b.py".into(),
            local: "g".into(),
            body: 7,
        };
        assert_eq!(
            differing_fields(&own, &given),
            ["kind", "file", "body", "local"]
        );
        let body = Identity {
            body: 7,
            ..own.clone()
        };
        assert_eq!(differing_fields(&own, &body), ["body"]);
    }

    fn vrow(qname: &str, model: Option<&str>, text_hash: Option<&str>) -> VectorRow {
        let v = [0.6f32, 0.8];
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        VectorRow {
            qname: qname.into(),
            kind: Some("FUNCTION".into()),
            hint: None,
            model: model.map(str::to_string),
            dims: Some(2),
            text_hash: text_hash.map(str::to_string),
            b64: b64_encode(&bytes),
        }
    }

    #[test]
    fn write_vector_rows_sorts_and_writes_under_the_lock() {
        let td = tempfile::tempdir().unwrap();
        let rows = vec![
            vrow("b::f", Some("m"), Some("t1:0123456789abcdef")),
            vrow("a::f", Some("m"), None),
        ];
        let n = write_vector_rows(td.path(), rows, VectorWriteMode::MergeSameModel).unwrap();
        assert_eq!(n, 2);
        let path = td.path().join(VECTORS_FILE);
        let body = std::fs::read_to_string(&path).unwrap();
        let qnames: Vec<String> = body
            .lines()
            .map(|l| serde_json::from_str::<VectorRow>(l).unwrap().qname)
            .collect();
        assert_eq!(qnames, ["a::f", "b::f"]);
        assert!(!td.path().join(CELLS_LOCK).exists(), "lock released");
        assert!(
            !td.path().join(".glia/.gitignore").exists(),
            "the writer never creates the gitignore"
        );
        // Same input, same bytes.
        let again = vec![
            vrow("a::f", Some("m"), None),
            vrow("b::f", Some("m"), Some("t1:0123456789abcdef")),
        ];
        write_vector_rows(td.path(), again, VectorWriteMode::MergeSameModel).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    #[test]
    fn write_vector_rows_refuses_another_model_unless_replace() {
        let td = tempfile::tempdir().unwrap();
        write_vector_rows(
            td.path(),
            vec![vrow("a::f", Some("old"), None)],
            VectorWriteMode::Replace,
        )
        .unwrap();
        let err = write_vector_rows(
            td.path(),
            vec![vrow("a::f", Some("new"), None)],
            VectorWriteMode::MergeSameModel,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("model old"), "{err}");
        let body = std::fs::read_to_string(td.path().join(VECTORS_FILE)).unwrap();
        assert!(body.contains("\"old\""), "nothing written: {body}");
        write_vector_rows(
            td.path(),
            vec![vrow("a::f", Some("new"), None)],
            VectorWriteMode::Replace,
        )
        .unwrap();
        let body = std::fs::read_to_string(td.path().join(VECTORS_FILE)).unwrap();
        assert!(
            body.contains("\"new\"") && !body.contains("\"old\""),
            "{body}"
        );
    }

    #[test]
    fn write_vector_rows_checks_the_row_contract() {
        let td = tempfile::tempdir().unwrap();
        let cases: Vec<(Vec<VectorRow>, &str)> = vec![
            (vec![vrow("a", None, None)], "names no model"),
            (
                vec![vrow("a", Some("m"), None), vrow("b", Some("n"), None)],
                "two models",
            ),
            (
                vec![vrow("a", Some("m"), Some("t1:XYZ"))],
                "malformed text_hash",
            ),
            (
                vec![vrow("a", Some("m"), None), vrow("a", Some("m"), None)],
                "two vector rows",
            ),
        ];
        for (rows, want) in cases {
            let err = write_vector_rows(td.path(), rows, VectorWriteMode::Replace)
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
        let mut bad = vrow("a", Some("m"), None);
        bad.dims = Some(3);
        let err = write_vector_rows(td.path(), vec![bad], VectorWriteMode::Replace)
            .unwrap_err()
            .to_string();
        assert!(err.contains("dims=3"), "{err}");
        assert!(!td.path().join(VECTORS_FILE).exists(), "nothing written");
    }

    #[test]
    fn write_through_spells_as_serialised() {
        for w in [
            WriteThrough::Applied,
            WriteThrough::NoGmap,
            WriteThrough::GmapStale,
            WriteThrough::NodeMissing,
            WriteThrough::MultiRepo,
            WriteThrough::SidecarOnly,
        ] {
            assert_eq!(serde_json::to_value(w).unwrap(), json!(w.as_str()));
        }
    }
}
