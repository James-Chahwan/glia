//! `glia cell set / rm / ls` (LF.1c) — the CLI surface of the cell write API,
//! and the one surface that audits the sidecars.
//!
//! `set` and `rm` go through `glia_store::write_cell` /
//! `remove_cell_entry` for the repo's default layout dir (`.glia/graph`), so
//! a write lands where a pyo3 or MCP write lands: `.glia/cells.jsonl`
//! (`.glia/vectors.jsonl` for a VECTOR) under `.glia/cells.lock`, and the
//! layout only while it is fresh (the store captures the bound node's
//! move-stable hint into the row then). Each prints the pyo3 marker shape
//! with `surface=cli`:
//! `[cells] write|remove qname=<q> cell=<C> id=<id|-> rows=<n> target=<t> write_through=<w> surface=cli`
//! (plus ` redacted=<k>` when the A13.7 redaction replaced a secret).
//!
//! `ls` lists the sidecar rows. `ls --check` builds the graph the way a normal
//! build does (`generate_one_opts` with the global `--no-overlay` flag, so
//! overlays re-key ENDPOINT qnames as they do in the build) and binds every
//! row through the build's one apply path, `QnameIndex::build(&merged, None)`
//! then `apply_cell_write`, which validates the row as the build does and
//! resolves it with `QnameIndex::resolve` — never a CLI-local lookup. A row is
//! `bound`, `rekeyed` (a HEURISTIC re-bind through its hint, with the tier),
//! `ambiguous` (every candidate listed, none picked), `orphaned` (with the
//! hint's candidates when it names several) or `rejected`. Re-applying a row
//! the build already applied is an upsert of the same entry, so the outcome is
//! the build's. Marker:
//! `[cells] ls rows=R bound=B rekeyed=K ambiguous=A orphaned=O rejected=X surface=cli`.
//!
//! `--rekey` rewrites each rekeyed row to its node's current qname and hint:
//! the new row is written, then the old one removed, each by the store under
//! the lock and sorted writer. Both touch the sidecar only (no gmap dir): a
//! write-through of the removal would bind the old row's hint to the same
//! moved node and strip the entry just written, so the layout is left to read
//! stale and its next load rebuilds it from the rewritten sidecar. A rewrite
//! whose target key already holds a different entry is not made (a CONV takes
//! the next free id instead, as a fresh CONV write does); it prints
//! `[cells] rekey conflict ...`.
//!
//! Exit codes: 0 ok; 1 when `--check` finds a row that does not apply
//! (ambiguous, orphaned or rejected), or `rm` finds no such row; 2 for a
//! usage or I/O error (nothing is written).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{ArgGroup, Subcommand};
use glia_code_domain::external_inputs::{
    CELLS_FILE, CellRow, CellWrite, VECTORS_FILE, VectorRow, b64_decode, canonical, cell_type_id,
    read_rows,
};
use glia_code_domain::{cell_type, node_kind};
use glia_core::NodeId;
use glia_engine::generate_one_opts;
use glia_engine::persist::default_layout_dir;
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex, apply_cell_write};
use glia_graph::identity::{IdentityIndex, Rebind, identity_of};
use glia_store::{CellRemoval, CellWriteOutcome, WriteThrough, remove_cell_entry, write_cell};
use serde_json::{Value, json};

use crate::common::build_options;

const EXIT_OK: i32 = 0;
/// `--check` found a row that does not apply, or `rm` found no such row.
const EXIT_FAILED: i32 = 1;
/// A usage or I/O error (clap's own usage errors exit 2 as well).
const EXIT_USAGE: i32 = 2;
/// The `surface=` of every marker this command prints.
const SURFACE: &str = "cli";

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand, Debug)]
enum Action {
    /// Write one CONSTRAINT / DECISION / CONV entry (`--json`, `--text` for a
    /// CONV note, or `--file`) or a VECTOR (`--file`, raw little-endian f32
    /// bytes) on the node <qname> names. The entry's `source` defaults to
    /// `api`; a CONV without an `id` gets the next one (000001, 000002, ...).
    Set(SetArgs),
    /// Remove one entry (`--id`, `--source`) or a node's VECTOR from the
    /// sidecar and, while it is fresh, the default layout. Exits 1 when the
    /// sidecar holds no such row.
    Rm(RmArgs),
    /// List the `.glia/cells.jsonl` and `.glia/vectors.jsonl` rows.
    /// `--check` binds each row against a fresh build (bound / rekeyed /
    /// ambiguous / orphaned / rejected) and exits 1 when one does not apply;
    /// `--rekey` rewrites rekeyed rows to their node's current qname.
    Ls(LsArgs),
}

#[derive(clap::Args, Debug)]
#[command(group(ArgGroup::new("payload").required(true).args(["json", "text", "file"])))]
struct SetArgs {
    /// Repo root (its `.glia/` holds the sidecars).
    repo: String,
    /// The node's qname (`app::charge`).
    qname: String,
    /// Cell type NAME: CONSTRAINT, DECISION, CONV or VECTOR.
    cell: String,
    /// The entry as a JSON object (`{"id":"d1","title":"..."}`).
    #[arg(long, value_name = "ENTRY")]
    json: Option<String>,
    /// A CONV note: the entry `{"text": TEXT}`.
    #[arg(long)]
    text: Option<String>,
    /// Read the payload from a file: raw bytes for a VECTOR, a JSON entry
    /// otherwise.
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Bind only a node of this NodeKind NAME (`FUNCTION`), for a qname
    /// several nodes share.
    #[arg(long)]
    kind: Option<String>,
    /// VECTOR only: the embedding model.
    #[arg(long)]
    model: Option<String>,
    /// VECTOR only: the dimension count (the payload is dims * 4 bytes).
    #[arg(long)]
    dims: Option<u32>,
}

#[derive(clap::Args, Debug)]
struct RmArgs {
    /// Repo root (its `.glia/` holds the sidecars).
    repo: String,
    /// The node's qname, as the row stores it.
    qname: String,
    /// Cell type NAME: CONSTRAINT, DECISION, CONV or VECTOR.
    cell: String,
    /// The entry's id (a VECTOR has none: one per node).
    #[arg(long)]
    id: Option<String>,
    /// The entry's source.
    #[arg(long, default_value = "api")]
    source: String,
}

#[derive(clap::Args, Debug)]
struct LsArgs {
    /// Repo root (its `.glia/` holds the sidecars).
    repo: String,
    /// Only rows on this qname (under `--check`, also rows that bind a node
    /// of this qname).
    #[arg(long)]
    qname: Option<String>,
    /// Emit JSON: `{"rows": [...]}`, plus `check` counts under `--check`.
    #[arg(long)]
    json: bool,
    /// Bind every row against a fresh build of the repo.
    #[arg(long)]
    check: bool,
    /// Rewrite rekeyed rows to their node's current qname and hint.
    #[arg(long, requires = "check")]
    rekey: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let out = match args.action {
        Action::Set(a) => set(a),
        Action::Rm(a) => rm(a),
        Action::Ls(a) => ls(a),
    };
    out.unwrap_or_else(|e| {
        eprintln!("error: {e}");
        EXIT_USAGE
    })
}

// ---------------------------------------------------------------------------
// set / rm
// ---------------------------------------------------------------------------

fn set(a: SetArgs) -> Result<i32, String> {
    let root = repo_root(&a.repo)?;
    let w = cell_write(a)?;
    let out = write_cell(&root, Some(&default_layout_dir(&root)), None, &w).map_err(|e| e.to_string())?;
    eprintln!("{}", marker("write", &out));
    println!(
        "wrote {} {} for {} (write-through: {})",
        out.cell,
        out.entry_id.as_deref().unwrap_or("-"),
        out.qname,
        spoken(out.write_through)
    );
    Ok(EXIT_OK)
}

/// The write `set`'s flags describe; the payload flag must fit the cell.
fn cell_write(a: SetArgs) -> Result<CellWrite, String> {
    let cell = cell_type_id(&a.cell).ok_or_else(|| unknown_cell(&a.cell))?;
    if cell == cell_type::VECTOR {
        let path = a.file.ok_or("a VECTOR payload is raw bytes: pass --file <path>")?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(CellWrite::vector(a.qname, bytes, a.model, a.dims).with_kind(a.kind));
    }
    if a.model.is_some() || a.dims.is_some() {
        return Err("--model / --dims apply to a VECTOR payload only".into());
    }
    let entry = match (a.json, a.text, a.file) {
        (Some(text), _, _) => parse_entry(&a.cell, &text)?,
        (_, Some(text), _) => json!({ "text": text }),
        (_, _, Some(path)) => {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse_entry(&a.cell, &text)?
        }
        (None, None, None) => return Err("pass one of --json, --text, --file".into()),
    };
    Ok(CellWrite::entry(a.qname, cell, entry).with_kind(a.kind))
}

fn parse_entry(cell: &str, text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("the {cell} entry is not JSON: {e}"))
}

fn rm(a: RmArgs) -> Result<i32, String> {
    let root = repo_root(&a.repo)?;
    let cell = cell_type_id(&a.cell).ok_or_else(|| unknown_cell(&a.cell))?;
    let (removal, what) = if cell == cell_type::VECTOR {
        if a.id.is_some() {
            return Err("a VECTOR has no entry id (one per node): drop --id".into());
        }
        (CellRemoval::vector(a.qname.as_str()), "VECTOR".to_string())
    } else {
        let id = a.id.ok_or_else(|| format!("removing a {} entry needs --id <id>", a.cell))?;
        let what = format!("{} {}/{id}", a.cell, a.source);
        (CellRemoval::entry(a.qname.as_str(), cell, a.source, id), what)
    };
    let dir = default_layout_dir(&root);
    match remove_cell_entry(&root, Some(&dir), None, &removal).map_err(|e| e.to_string())? {
        Some(out) => {
            eprintln!("{}", marker("remove", &out));
            println!(
                "removed {} {} from {} (write-through: {})",
                out.cell,
                out.entry_id.as_deref().unwrap_or("-"),
                out.qname,
                spoken(out.write_through)
            );
            Ok(EXIT_OK)
        }
        None => {
            println!("no {what} on {}", a.qname);
            Ok(EXIT_FAILED)
        }
    }
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

/// One sidecar line.
enum Row {
    Cell(CellRow),
    Vector(VectorRow),
    /// A line that is not a row: `"<file>:<line>: <error>"`.
    Unreadable(String),
}

impl Row {
    fn qname(&self) -> Option<&str> {
        match self {
            Row::Cell(r) => Some(&r.qname),
            Row::Vector(r) => Some(&r.qname),
            Row::Unreadable(_) => None,
        }
    }

    /// `<qname> <CELL> <source>/<id>` (`<qname> VECTOR` for a vector).
    fn label(&self) -> String {
        match self {
            Row::Cell(r) => format!("{} {} {}/{}", r.qname, r.cell, field(&r.entry, "source"), field(&r.entry, "id")),
            Row::Vector(r) => format!("{} VECTOR", r.qname),
            Row::Unreadable(e) => format!("unreadable {e}"),
        }
    }

    /// The listing line: the label, the kind filter, the payload.
    fn line(&self) -> String {
        let kind = |k: &Option<String>| k.as_ref().map(|k| format!(" kind={k}")).unwrap_or_default();
        match self {
            Row::Cell(r) => format!("{}{} {}", self.label(), kind(&r.kind), r.entry),
            Row::Vector(r) => {
                let mut line = format!("{}{} {}", self.label(), kind(&r.kind), vector_size(r));
                if let Some(m) = &r.model {
                    line.push_str(&format!(" model={m}"));
                }
                if let Some(d) = r.dims {
                    line.push_str(&format!(" dims={d}"));
                }
                line
            }
            Row::Unreadable(_) => self.label(),
        }
    }

    fn json(&self) -> Value {
        match self {
            Row::Cell(r) => json!({
                "file": CELLS_FILE, "qname": r.qname, "cell": r.cell, "kind": r.kind,
                "hint": r.hint, "entry": r.entry,
            }),
            Row::Vector(r) => {
                let (bytes, error) = match b64_decode(&r.b64) {
                    Ok(b) => (Some(b.len()), None),
                    Err(e) => (None, Some(e)),
                };
                let mut v = json!({
                    "file": VECTORS_FILE, "qname": r.qname, "cell": "VECTOR", "kind": r.kind,
                    "hint": r.hint, "model": r.model, "dims": r.dims, "bytes": bytes,
                });
                if let (Some(e), Some(o)) = (error, v.as_object_mut()) {
                    o.insert("error".into(), Value::String(e));
                }
                v
            }
            Row::Unreadable(e) => json!({ "error": e }),
        }
    }
}

/// A node, for a report: `(qname, kind NAME, id)`.
#[derive(Clone)]
struct NodeRef {
    qname: String,
    kind: &'static str,
    id: u64,
}

impl NodeRef {
    fn json(&self) -> Value {
        json!({ "qname": self.qname, "kind": self.kind, "id": self.id })
    }
}

/// What `--check` found for one row.
struct Checked {
    /// `bound` | `rekeyed` | `ambiguous` | `orphaned` | `rejected`.
    status: &'static str,
    /// The node a bound or rekeyed row lands on.
    node: Option<NodeRef>,
    /// The evidence tier of a rekeyed row.
    tier: Option<&'static str>,
    /// Every node an ambiguous row could mean; for an orphaned row, the
    /// nodes its hint names when it names several.
    candidates: Vec<NodeRef>,
    /// Why a row is rejected.
    reason: Option<String>,
    /// The qname `--rekey` rewrote the row to.
    rewritten_to: Option<String>,
}

impl Checked {
    fn new(status: &'static str) -> Self {
        Checked { status, node: None, tier: None, candidates: Vec::new(), reason: None, rewritten_to: None }
    }

    fn rejected(reason: String) -> Self {
        Checked { reason: Some(reason), ..Checked::new("rejected") }
    }

    /// The report line after the status and the row label.
    fn detail(&self) -> String {
        let names = |c: &[NodeRef]| c.iter().map(|n| format!("{} {}", n.qname, n.kind)).collect::<Vec<_>>().join(", ");
        let mut out = String::new();
        if self.status == "rekeyed"
            && let Some(n) = &self.node
        {
            out.push_str(&format!(" -> {} tier={}", n.qname, self.tier.unwrap_or("?")));
            if self.rewritten_to.is_some() {
                out.push_str(" (rewritten)");
            }
        }
        if !self.candidates.is_empty() {
            let what = if self.status == "orphaned" { "hint candidates" } else { "candidates" };
            out.push_str(&format!(" {what}: {}", names(&self.candidates)));
        }
        if let Some(r) = &self.reason {
            out.push_str(&format!(": {r}"));
        }
        out
    }
}

/// Per-status counts, in the marker's order.
#[derive(Default)]
struct Counts {
    rows: usize,
    bound: usize,
    rekeyed: usize,
    ambiguous: usize,
    orphaned: usize,
    rejected: usize,
}

impl Counts {
    fn of(checked: &[&Checked]) -> Self {
        let mut c = Counts { rows: checked.len(), ..Counts::default() };
        for x in checked {
            match x.status {
                "bound" => c.bound += 1,
                "rekeyed" => c.rekeyed += 1,
                "ambiguous" => c.ambiguous += 1,
                "orphaned" => c.orphaned += 1,
                _ => c.rejected += 1,
            }
        }
        c
    }

    fn json(&self) -> Value {
        json!({
            "rows": self.rows, "bound": self.bound, "rekeyed": self.rekeyed,
            "ambiguous": self.ambiguous, "orphaned": self.orphaned, "rejected": self.rejected,
        })
    }
}

fn ls(a: LsArgs) -> Result<i32, String> {
    let root = repo_root(&a.repo)?;
    let rows = read_sidecars(&root);
    if !a.check {
        let rows: Vec<&Row> = rows.iter().filter(|r| a.qname.as_deref().is_none_or(|q| r.qname() == Some(q))).collect();
        let unreadable = rows.iter().filter(|r| matches!(r, Row::Unreadable(_))).count();
        eprintln!("[cells] ls rows={} unreadable={unreadable} surface={SURFACE}", rows.len());
        if a.json {
            println!("{}", json!({ "rows": rows.iter().map(|r| r.json()).collect::<Vec<_>>() }));
        } else {
            rows.iter().for_each(|r| println!("{}", r.line()));
        }
        return Ok(EXIT_OK);
    }

    let result = generate_one_opts(&a.repo, false, &build_options())?;
    let mut merged = result.merged;
    let mut checked = check_rows(&mut merged, &rows);
    let keep: Vec<bool> = rows
        .iter()
        .zip(&checked)
        .map(|(r, c)| {
            a.qname.as_deref().is_none_or(|q| r.qname() == Some(q) || c.node.as_ref().is_some_and(|n| n.qname == q))
        })
        .collect();
    let counts = Counts::of(&checked.iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| c).collect::<Vec<_>>());
    eprintln!(
        "[cells] ls rows={} bound={} rekeyed={} ambiguous={} orphaned={} rejected={} surface={SURFACE}",
        counts.rows, counts.bound, counts.rekeyed, counts.ambiguous, counts.orphaned, counts.rejected
    );
    let rekey = if a.rekey { Some(rekey_rows(&root, &merged, &rows, &mut checked, &keep)?) } else { None };

    let listed = rows.iter().zip(&checked).zip(&keep).filter(|(_, k)| **k).map(|(rc, _)| rc);
    if a.json {
        let out: Vec<Value> = listed
            .map(|(r, c)| {
                let mut v = r.json();
                if let Some(o) = v.as_object_mut() {
                    o.insert("status".into(), json!(c.status));
                    o.insert("node".into(), c.node.as_ref().map_or(Value::Null, NodeRef::json));
                    o.insert("tier".into(), json!(c.tier));
                    o.insert("candidates".into(), c.candidates.iter().map(NodeRef::json).collect());
                    o.insert("reason".into(), json!(c.reason));
                    o.insert("rewritten_to".into(), json!(c.rewritten_to));
                }
                v
            })
            .collect();
        let mut doc = json!({ "rows": out, "check": counts.json() });
        if let (Some((rewritten, conflicts)), Some(o)) = (rekey, doc.as_object_mut()) {
            o.insert("rekey".into(), json!({ "rewritten": rewritten, "conflicts": conflicts }));
        }
        println!("{doc}");
    } else {
        for (r, c) in listed {
            println!("{} {}{}", c.status, r.label(), c.detail());
        }
    }
    let failed = counts.ambiguous + counts.orphaned + counts.rejected;
    Ok(if failed > 0 { EXIT_FAILED } else { EXIT_OK })
}

/// Every line of both sidecars, cells file first, each in file order (the
/// build's order); a line that is not a row is kept as `Unreadable`.
fn read_sidecars(root: &Path) -> Vec<Row> {
    let (cells, cell_errors) = read_rows::<CellRow>(&root.join(CELLS_FILE));
    let (vectors, vector_errors) = read_rows::<VectorRow>(&root.join(VECTORS_FILE));
    cells
        .into_iter()
        .map(Row::Cell)
        .chain(vectors.into_iter().map(Row::Vector))
        .chain(cell_errors.into_iter().chain(vector_errors).map(Row::Unreadable))
        .collect()
}

/// Bind every row through the build's apply path over `merged` (a fresh
/// build, which already holds the rows: re-applying one is an upsert of the
/// same entry).
fn check_rows(merged: &mut MergedGraph, rows: &[Row]) -> Vec<Checked> {
    let idx = QnameIndex::build(merged, None);
    let mut identity: Option<IdentityIndex> = None;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let write = match row {
            Row::Cell(r) => CellWrite::try_from(r),
            Row::Vector(r) => CellWrite::try_from(r),
            Row::Unreadable(e) => Err(e.clone()),
        };
        let target = write.and_then(|w| apply_cell_write(merged, &idx, &w).map(|t| (w, t)));
        let checked = match target {
            Err(e) => Checked::rejected(e),
            Ok((_, CellTarget::Bound(id))) => Checked { node: node_ref(merged, id), ..Checked::new("bound") },
            Ok((_, CellTarget::Rekeyed { id, tier })) => {
                Checked { node: node_ref(merged, id), tier: Some(tier.as_str()), ..Checked::new("rekeyed") }
            }
            Ok((w, CellTarget::Ambiguous(_))) => {
                let ident = identity.get_or_insert_with(|| IdentityIndex::build(merged));
                Checked { candidates: candidates(merged, ident, &w), ..Checked::new("ambiguous") }
            }
            Ok((w, CellTarget::Orphaned)) => {
                let candidates = match w.hint {
                    Some(_) => candidates(merged, identity.get_or_insert_with(|| IdentityIndex::build(merged)), &w),
                    None => Vec::new(),
                };
                Checked { candidates, ..Checked::new("orphaned") }
            }
            Ok((_, CellTarget::Rejected(e))) => Checked::rejected(e),
            Ok((_, other)) => Checked::rejected(format!("unhandled target {other:?}")),
        };
        out.push(checked);
    }
    out
}

/// The nodes a row could mean when the resolver picks none: the re-bind's
/// candidate list (LB.6), in id order. Never picks one.
fn candidates(merged: &MergedGraph, ident: &IdentityIndex, w: &CellWrite) -> Vec<NodeRef> {
    let kind = w.kind.as_deref().and_then(|k| node_kind::ALL.iter().find(|(_, n)| *n == k)).map(|(id, _)| *id);
    match ident.rebind(&w.qname, kind, w.hint.as_deref()) {
        Rebind::Ambiguous(ids) => ids.into_iter().filter_map(|id| node_ref(merged, id)).collect(),
        _ => Vec::new(),
    }
}

fn node_ref(merged: &MergedGraph, id: NodeId) -> Option<NodeRef> {
    let g = merged.graphs.iter().find(|g| g.nav.qname_by_id.contains_key(&id))?;
    let qname = g.nav.qname_by_id.get(&id)?.clone();
    let kind = g
        .nav
        .kind_by_id
        .get(&id)
        .and_then(|k| node_kind::ALL.iter().find(|(kid, _)| kid == k))
        .map_or("?", |(_, n)| *n);
    Some(NodeRef { qname, kind, id: id.0 })
}

/// `--rekey`: rewrite each kept rekeyed row to its node's qname and current
/// hint (sidecar only; see the module doc). `(rewritten, conflicts)`.
fn rekey_rows(
    root: &Path,
    merged: &MergedGraph,
    rows: &[Row],
    checked: &mut [Checked],
    keep: &[bool],
) -> Result<(usize, usize), String> {
    // The entries the cells file holds, by key, kept current as rows move.
    let mut entries: BTreeMap<(String, String, String, String), Value> = rows
        .iter()
        .filter_map(|r| match r {
            Row::Cell(c) => Some((cell_key(&c.qname, c), canonical(&c.entry))),
            _ => None,
        })
        .collect();
    let mut vectors: BTreeMap<String, String> = rows
        .iter()
        .filter_map(|r| match r {
            Row::Vector(v) => Some((v.qname.clone(), v.b64.clone())),
            _ => None,
        })
        .collect();
    let (mut rewritten, mut conflicts) = (0, 0);
    for ((row, c), _) in rows.iter().zip(checked.iter_mut()).zip(keep).filter(|(_, k)| **k) {
        let Some(node) = c.node.clone().filter(|_| c.status == "rekeyed") else {
            continue;
        };
        let hint = identity_of(merged, NodeId(node.id)).map(|i| i.hint());
        let tier = c.tier.unwrap_or("?");
        let (old_id, new_id) = match row {
            Row::Cell(r) => {
                let cell = cell_type_id(&r.cell).ok_or_else(|| unknown_cell(&r.cell))?;
                let mut entry = canonical(&r.entry);
                let target = cell_key(&node.qname, r);
                let old_id = field(&r.entry, "id");
                let write = match entries.get(&target) {
                    None => true,
                    Some(e) if *e == entry => false,
                    Some(_) if cell == cell_type::CONV => {
                        if let Some(o) = entry.as_object_mut() {
                            o.remove("id");
                        }
                        true
                    }
                    Some(_) => {
                        conflicts += 1;
                        eprintln!(
                            "[cells] rekey conflict qname={} -> {} cell={} id={old_id}: the target already holds a different entry; not rewritten surface={SURFACE}",
                            r.qname, node.qname, r.cell
                        );
                        continue;
                    }
                };
                let mut new_id = old_id.clone();
                if write {
                    let w = CellWrite::entry(node.qname.as_str(), cell, entry).with_kind(r.kind.clone()).with_hint(hint);
                    let out = write_cell(root, None, None, &w).map_err(|e| e.to_string())?;
                    eprintln!("{}", marker("write", &out));
                    new_id = out.entry_id.clone().unwrap_or_default();
                    if let Some(e) = &out.entry {
                        entries.insert((node.qname.clone(), r.cell.clone(), field(e, "source"), new_id.clone()), e.clone());
                    }
                }
                let removal = CellRemoval::entry(r.qname.as_str(), cell, field(&r.entry, "source"), old_id.as_str());
                if let Some(out) = remove_cell_entry(root, None, None, &removal).map_err(|e| e.to_string())? {
                    eprintln!("{}", marker("remove", &out));
                }
                entries.remove(&cell_key(&r.qname, r));
                (old_id, new_id)
            }
            Row::Vector(r) => {
                let write = match vectors.get(&node.qname) {
                    None => true,
                    Some(b64) if *b64 == r.b64 => false,
                    Some(_) => {
                        conflicts += 1;
                        eprintln!(
                            "[cells] rekey conflict qname={} -> {} cell=VECTOR id=-: the target already holds a different vector; not rewritten surface={SURFACE}",
                            r.qname, node.qname
                        );
                        continue;
                    }
                };
                if write {
                    let mut w = CellWrite::try_from(r)?;
                    w.qname = node.qname.clone();
                    w.hint = hint;
                    let out = write_cell(root, None, None, &w).map_err(|e| e.to_string())?;
                    eprintln!("{}", marker("write", &out));
                    vectors.insert(node.qname.clone(), r.b64.clone());
                }
                if let Some(out) =
                    remove_cell_entry(root, None, None, &CellRemoval::vector(r.qname.as_str())).map_err(|e| e.to_string())?
                {
                    eprintln!("{}", marker("remove", &out));
                }
                vectors.remove(&r.qname);
                ("-".to_string(), "-".to_string())
            }
            Row::Unreadable(_) => continue,
        };
        let from = row.qname().unwrap_or_default();
        let cell = match row {
            Row::Cell(r) => r.cell.as_str(),
            _ => "VECTOR",
        };
        let mut line = format!("[cells] rekey qname={from} -> {} cell={cell} id={old_id} tier={tier} surface={SURFACE}", node.qname);
        if new_id != old_id {
            line.push_str(&format!(" new_id={new_id}"));
        }
        eprintln!("{line}");
        c.rewritten_to = Some(node.qname.clone());
        rewritten += 1;
    }
    eprintln!("[cells] rekey rewritten={rewritten} conflicts={conflicts} surface={SURFACE}");
    Ok((rewritten, conflicts))
}

/// `(qname, cell, source, id)`, the key the store upserts a cell row by.
fn cell_key(qname: &str, r: &CellRow) -> (String, String, String, String) {
    (qname.to_string(), r.cell.clone(), field(&r.entry, "source"), field(&r.entry, "id"))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// The repo root, which must exist: the store's lock would create `.glia/`
/// under a mistyped path.
fn repo_root(repo: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(repo);
    if p.is_dir() { Ok(p) } else { Err(format!("not a directory: {repo}")) }
}

fn unknown_cell(name: &str) -> String {
    let names: Vec<&str> = cell_type::ALL.iter().map(|(_, n)| *n).collect();
    format!("unknown cell type {name:?} (one of {})", names.join(" | "))
}

/// A string field of an entry, `""` when absent.
fn field(entry: &Value, key: &str) -> String {
    entry.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn vector_size(r: &VectorRow) -> String {
    match b64_decode(&r.b64) {
        Ok(b) => format!("{} bytes", b.len()),
        Err(e) => format!("unreadable ({e})"),
    }
}

/// `no_gmap` -> `no gmap`, for the human line.
fn spoken(w: WriteThrough) -> String {
    w.as_str().replace('_', " ")
}

/// The `[cells] <verb>` marker of one outcome — `py/src/cells.rs`'s shape
/// with `surface=cli`.
fn marker(verb: &str, o: &CellWriteOutcome) -> String {
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
