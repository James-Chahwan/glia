//! The cell-sidecar stage (LF.1a): `.glia/cells.jsonl` (checked in) and
//! `.glia/vectors.jsonl` (local) applied to their nodes at build, through the
//! graph crate's one resolver and apply function (`glia_graph::cells`),
//! so a build binds a row exactly as a live write and a persisted
//! write-through do.
//!
//! Rows apply in file order, cells file first; writers keep the files sorted,
//! so a node's cell order is stable. A row that does not parse, names an
//! unknown cell type or fails its entry rules is counted `rejected` and never
//! sinks the build.
//!
//! Marker, once per repo that has either file (the fired_on line):
//!   `[cells] sidecar repo=<label> rows=<r> bound=<b> rekeyed=<k> ambiguous=<a> orphaned=<o> rejected=<x> (CONSTRAINT=<n> DECISION=<n> CONV=<n> VECTOR=<n>)`
//! where the per-type counts are the rows applied (bound + rekeyed). Then one
//! detail line per row that did not simply bind, at most
//! [`MAX_DETAIL_LINES`] per repo:
//!   `[cells] rejected|orphaned|ambiguous|rekeyed <file> qname=<q> cell=<C> ...`.

use glia_code_domain::cell_type;
use glia_code_domain::external_inputs::{
    CELLS_FILE, CellRow, CellWrite, VECTORS_FILE, VectorRow, WRITABLE, read_rows,
};
use glia_core::{CellTypeId, NodeId};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex, apply_cell_write};

use super::RepoInputs;

/// Detail lines printed per repo before the rest are summarised.
const MAX_DETAIL_LINES: usize = 32;

/// Outcome counts of one repo's sidecars.
#[derive(Debug, Default)]
struct Tally {
    rows: usize,
    bound: usize,
    rekeyed: usize,
    ambiguous: usize,
    orphaned: usize,
    rejected: usize,
    /// Applied rows per [`WRITABLE`] type, in its order.
    applied: [usize; 4],
    details: Vec<String>,
}

impl Tally {
    fn detail(&mut self, line: String) {
        self.details.push(line);
    }

    /// Count one write's outcome; `what` names the row in a detail line.
    fn count(&mut self, merged: &MergedGraph, cell: CellTypeId, what: &str, target: Result<CellTarget, String>) {
        match target {
            Ok(CellTarget::Bound(_)) => {
                self.bound += 1;
                self.applied_one(cell);
            }
            Ok(CellTarget::Rekeyed { id, tier }) => {
                self.rekeyed += 1;
                self.applied_one(cell);
                self.detail(format!("rekeyed {what} -> {} tier={}", qname_of(merged, id), tier.as_str()));
            }
            Ok(CellTarget::Ambiguous(id)) => {
                self.ambiguous += 1;
                self.detail(format!("ambiguous {what} (not applied; smallest candidate {})", qname_of(merged, id)));
            }
            Ok(CellTarget::Orphaned) => {
                self.orphaned += 1;
                self.detail(format!("orphaned {what}"));
            }
            Ok(CellTarget::Rejected(e)) | Err(e) => self.reject(&format!("{what}: {e}")),
            Ok(other) => self.reject(&format!("{what}: unhandled target {other:?}")),
        }
    }

    fn reject(&mut self, what: &str) {
        self.rejected += 1;
        self.detail(format!("rejected {what}"));
    }

    fn applied_one(&mut self, cell: CellTypeId) {
        if let Some(slot) = WRITABLE.iter().position(|c| *c == cell).and_then(|i| self.applied.get_mut(i)) {
            *slot += 1;
        }
    }
}

/// Apply `input`'s sidecars to `merged`. True when a cell was written.
pub(super) fn apply_sidecar(merged: &mut MergedGraph, input: &RepoInputs) -> bool {
    let cells_path = input.root.join(CELLS_FILE);
    let vectors_path = input.root.join(VECTORS_FILE);
    if !cells_path.is_file() && !vectors_path.is_file() {
        return false;
    }
    let (cell_rows, cell_errors) = read_rows::<CellRow>(&cells_path);
    let (vector_rows, vector_errors) = read_rows::<VectorRow>(&vectors_path);
    let mut t = Tally::default();
    for e in cell_errors.iter().chain(&vector_errors) {
        t.rows += 1;
        t.reject(e);
    }
    if !cell_rows.is_empty() || !vector_rows.is_empty() {
        let idx = QnameIndex::build(merged, Some(input.repo));
        for row in &cell_rows {
            t.rows += 1;
            let what = format!("{CELLS_FILE} qname={} cell={}", row.qname, row.cell);
            match CellWrite::try_from(row) {
                Ok(w) => {
                    let target = apply_cell_write(merged, &idx, &w);
                    t.count(merged, w.cell, &what, target);
                }
                Err(e) => t.reject(&format!("{what}: {e}")),
            }
        }
        for row in &vector_rows {
            t.rows += 1;
            let what = format!("{VECTORS_FILE} qname={} cell=VECTOR", row.qname);
            let target = CellWrite::try_from(row).and_then(|w| apply_cell_write(merged, &idx, &w));
            t.count(merged, cell_type::VECTOR, &what, target);
        }
    }
    report(&input.label, &t);
    t.bound + t.rekeyed > 0
}

fn report(label: &str, t: &Tally) {
    let per_type: Vec<String> = WRITABLE
        .iter()
        .zip(t.applied)
        .map(|(c, n)| format!("{}={n}", cell_name(*c)))
        .collect();
    eprintln!(
        "[cells] sidecar repo={label} rows={} bound={} rekeyed={} ambiguous={} orphaned={} rejected={} ({})",
        t.rows,
        t.bound,
        t.rekeyed,
        t.ambiguous,
        t.orphaned,
        t.rejected,
        per_type.join(" ")
    );
    for line in t.details.iter().take(MAX_DETAIL_LINES) {
        eprintln!("[cells] {line}");
    }
    if t.details.len() > MAX_DETAIL_LINES {
        eprintln!("[cells] ... {} more detail lines", t.details.len() - MAX_DETAIL_LINES);
    }
}

fn cell_name(c: CellTypeId) -> &'static str {
    cell_type::ALL.iter().find(|(id, _)| *id == c).map_or("?", |(_, n)| n)
}

/// The qname of `id`, for a detail line; its numeric id when no nav names it.
fn qname_of(merged: &MergedGraph, id: NodeId) -> String {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id))
        .cloned()
        .unwrap_or_else(|| id.0.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_sidecar_writes_nothing_and_prints_nothing() {
        let d = std::env::temp_dir().join(format!("glia_sidecar_none_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let input = RepoInputs {
            repo: glia_core::RepoId::from_canonical("test://none"),
            root: d.clone(),
            label: "none".into(),
            config: None,
        };
        assert!(!d.join(CELLS_FILE).is_file());
        let mut m = MergedGraph::new(Vec::new());
        assert!(!apply_sidecar(&mut m, &input));
        std::fs::remove_dir_all(&d).ok();
    }
}
