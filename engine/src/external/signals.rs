//! The one reader of the ATTN and FAIL cells the external stages write
//! (CC.2): git-history churn and blame ([`super::history`], LF.5b) and
//! test-report failures ([`super::test_reports`], LF.6b), typed once, so the
//! answers that rank by them (flags, tests-for, hotspots, co-change) never
//! hand-roll a `serde_json::Value` walk that drifts from the writer.
//!
//! Every reader takes the FIRST cell of its type in the slice (each writer
//! keeps one per node / edge: the history stage skips a node that already
//! carries ATTN, the test-report stage merges into the first FAIL cell),
//! reads a [`CellPayload::Json`] or [`CellPayload::Text`] payload (a `Bytes`
//! payload is never one of these), deserialises it into a private mirror of
//! the writer's field names and checks its discriminator. A payload that is
//! not a JSON object, lacks a field the writer always writes, or carries
//! another `source` reads as `None`, never as zeros; a field the writer does
//! not know yet is ignored, so a later payload key is not a parse failure.
//!
//! | reader | cell | payload | discriminator |
//! |---|---|---|---|
//! | [`module_churn`] | ATTN on a MODULE | `{"source":"git","commits","lines_added","lines_deleted","first","last","window_commits","head"}` | `source == "git"` |
//! | [`symbol_blame`] | ATTN on a FUNCTION / METHOD / CLASS | `{"source":"git-blame","last","span_changes","head"}` | `source == "git-blame"` |
//! | [`pair_counts`] | ATTN on a CO_CHANGES edge | `{"cochanges","ratio_permille","window_commits"}` | has `cochanges` (no `source` key) |
//! | [`fail_entries`] | FAIL on a test or an implicated node | the canonical entry array of `external_inputs::merge_entry` | `role` is `test` or `implicated` |
//!
//! The history payloads hold integers only and no age relative to now (the
//! consumer computes recency): [`history_now`] is the one reference time, the
//! newest `last` any history ATTN in the graph carries, so a recency rule is
//! a function of the graph, never of the wall clock.
//!
//! Crate-private and silent: the readers print nothing. The
//! `[cochange] pairs=..` marker of `gaps::cochange_audit`, fed by
//! [`pair_counts`], is the fired_on line.

use glia_code_domain::cell_type;
use glia_core::{Cell, CellPayload, CellTypeId};
use glia_graph::MergedGraph;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::history::{SOURCE_BLAME, SOURCE_GIT};

/// A MODULE's churn over the history snapshot's window (LF.5b's module ATTN).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModuleChurn {
    /// Commits of the window that touched the module's file.
    pub(crate) commits: u32,
    pub(crate) lines_added: u64,
    pub(crate) lines_deleted: u64,
    /// Commit time (unix seconds) of the oldest and newest such commit.
    pub(crate) first: i64,
    pub(crate) last: i64,
    /// Commits in the snapshot's window.
    pub(crate) window_commits: usize,
}

/// A symbol's blame recency (LF.5b's symbol ATTN).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SymbolBlame {
    /// The newest commit time (unix seconds) of a line in the symbol's span.
    pub(crate) last: i64,
    /// Distinct commit times among the span's lines.
    pub(crate) span_changes: usize,
}

/// A CO_CHANGES edge's counts (LF.5b's pair ATTN).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PairCounts {
    /// Commits of the window that touched both files.
    pub(crate) cochanges: u32,
    /// `cochanges` per mille of the commits of the file that changes less.
    pub(crate) ratio_permille: u64,
    /// Commits in the snapshot's window.
    pub(crate) window_commits: usize,
}

/// What a FAIL entry says of the node that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailRole {
    /// The node is the failing test.
    Test,
    /// The node is a frame of a failing test's trace.
    Implicated,
}

/// One FAIL entry (LF.6b), in the fields a ranking reads; `report`,
/// `message` and `redacted` stay in the cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailEntry {
    /// `<run|latest>:<test>`, plus `#<frame>` on an implicated entry.
    pub(crate) id: String,
    pub(crate) role: FailRole,
    /// `<classname>::<name>`, or the bare name.
    pub(crate) test: String,
    /// `junit` or `log`.
    pub(crate) source: String,
    /// `failed` or `error`.
    pub(crate) status: String,
    /// The snapshot's run label, when it has one.
    pub(crate) run: Option<String>,
    /// The frame's index in the resolved trace; implicated entries only.
    pub(crate) frame: Option<u32>,
    /// How the test was mapped (`file_line`, `qname`, `name`); test entries only.
    pub(crate) via: Option<String>,
}

/// The module ATTN, field for field with `history::ModuleAttn`.
#[derive(serde::Deserialize)]
struct ModuleAttnIn {
    source: String,
    commits: u32,
    lines_added: u64,
    lines_deleted: u64,
    first: i64,
    last: i64,
    window_commits: usize,
}

/// The symbol ATTN, field for field with `history::SymbolAttn`.
#[derive(serde::Deserialize)]
struct SymbolAttnIn {
    source: String,
    last: i64,
    span_changes: usize,
}

/// The pair ATTN, field for field with `history::PairAttn`.
#[derive(serde::Deserialize)]
struct PairAttnIn {
    cochanges: u32,
    ratio_permille: u64,
    window_commits: usize,
}

/// One FAIL entry; the optional fields are the ones the writer may omit.
#[derive(serde::Deserialize)]
struct FailEntryIn {
    id: String,
    role: String,
    test: String,
    source: String,
    status: String,
    run: Option<String>,
    frame: Option<u32>,
    via: Option<String>,
}

/// The text of the first `kind` cell of `cells`: a Json or Text payload.
fn first_payload(cells: &[Cell], kind: CellTypeId) -> Option<&str> {
    match &cells.iter().find(|c| c.kind == kind)?.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => Some(s),
        CellPayload::Bytes(_) => None,
    }
}

/// `v` as `T` when it is a JSON object (a struct mirror would otherwise
/// accept an array positionally).
fn object<T: DeserializeOwned>(v: Value) -> Option<T> {
    if !v.is_object() {
        return None;
    }
    serde_json::from_value(v).ok()
}

/// The first ATTN payload of `cells`, parsed as a JSON object into `T`.
fn attn<T: DeserializeOwned>(cells: &[Cell]) -> Option<T> {
    object(serde_json::from_str(first_payload(cells, cell_type::ATTN)?).ok()?)
}

/// The churn ATTN `{"source":"git",...}` on a MODULE (LF.5b); `None` for any
/// other ATTN, a non-JSON payload or a missing field.
pub(crate) fn module_churn(cells: &[Cell]) -> Option<ModuleChurn> {
    let m: ModuleAttnIn = attn(cells)?;
    (m.source == SOURCE_GIT).then_some(ModuleChurn {
        commits: m.commits,
        lines_added: m.lines_added,
        lines_deleted: m.lines_deleted,
        first: m.first,
        last: m.last,
        window_commits: m.window_commits,
    })
}

/// The blame ATTN `{"source":"git-blame","last","span_changes"}` on a
/// FUNCTION / METHOD / CLASS (LF.5b); `None` for any other ATTN.
pub(crate) fn symbol_blame(cells: &[Cell]) -> Option<SymbolBlame> {
    let b: SymbolAttnIn = attn(cells)?;
    (b.source == SOURCE_BLAME).then_some(SymbolBlame {
        last: b.last,
        span_changes: b.span_changes,
    })
}

/// The CO_CHANGES edge cell `{"cochanges","ratio_permille","window_commits"}`
/// (LF.5b; no `source` key); `None` when a field is missing.
pub(crate) fn pair_counts(cells: &[Cell]) -> Option<PairCounts> {
    let p: PairAttnIn = attn(cells)?;
    Some(PairCounts {
        cochanges: p.cochanges,
        ratio_permille: p.ratio_permille,
        window_commits: p.window_commits,
    })
}

/// Every well-formed entry of the first FAIL cell (LF.6b), in stored order:
/// an object with string `id` / `role` / `test` / `source` / `status` and a
/// `role` of `test` or `implicated`. Anything else is skipped; a payload
/// that is not an entry array reads as no entry.
pub(crate) fn fail_entries(cells: &[Cell]) -> Vec<FailEntry> {
    let Some(Value::Array(entries)) =
        first_payload(cells, cell_type::FAIL).and_then(|s| serde_json::from_str::<Value>(s).ok())
    else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|v| {
            let e: FailEntryIn = object(v)?;
            let role = match e.role.as_str() {
                "test" => FailRole::Test,
                "implicated" => FailRole::Implicated,
                _ => return None,
            };
            Some(FailEntry {
                id: e.id,
                role,
                test: e.test,
                source: e.source,
                status: e.status,
                run: e.run,
                frame: e.frame,
                via: e.via,
            })
        })
        .collect()
}

/// The newest `last` of any history ATTN (module churn or symbol blame) in
/// `merged`: the deterministic "now" of every recency rule. One pass over
/// the nodes in graph order; `None` when no node carries history ATTN.
pub(crate) fn history_now(merged: &MergedGraph) -> Option<i64> {
    merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter_map(|n| {
            module_churn(&n.cells)
                .map(|m| m.last)
                .or_else(|| symbol_blame(&n.cells).map(|b| b.last))
        })
        .max()
}

#[cfg(test)]
mod tests {
    use glia_code_domain::node_kind;
    use glia_core::{Confidence, Node, NodeId, RepoId};

    use super::super::history::{ModuleAttn, PairAttn, SymbolAttn, json};
    use super::*;

    fn cell(kind: CellTypeId, payload: String) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(payload),
        }
    }

    /// The acceptance's module payload: first 100, last 200.
    fn module_payload() -> String {
        module_payload_at(100, 200)
    }

    fn module_payload_at(first: i64, last: i64) -> String {
        json(&ModuleAttn {
            source: "git",
            commits: 4,
            lines_added: 10,
            lines_deleted: 2,
            first,
            last,
            window_commits: 9,
            head: "abcdef012345",
        })
    }

    fn blame_payload(last: i64) -> String {
        json(&SymbolAttn {
            source: "git-blame",
            last,
            span_changes: 3,
            head: "abcdef012345",
        })
    }

    #[test]
    fn module_churn_round_trips_the_writer() {
        let cells = [cell(cell_type::ATTN, module_payload())];
        assert_eq!(
            module_churn(&cells),
            Some(ModuleChurn {
                commits: 4,
                lines_added: 10,
                lines_deleted: 2,
                first: 100,
                last: 200,
                window_commits: 9
            })
        );
        assert_eq!(symbol_blame(&cells), None);
        assert_eq!(pair_counts(&cells), None);
        // A Text payload reads the same; another source does not.
        assert!(
            module_churn(&[Cell {
                kind: cell_type::ATTN,
                payload: CellPayload::Text(module_payload())
            }])
            .is_some()
        );
        let lint = module_payload().replace(r#""source":"git""#, r#""source":"lint""#);
        assert_eq!(module_churn(&[cell(cell_type::ATTN, lint)]), None);
        // A missing field is None, never a zero.
        let short = module_payload().replace(r#""lines_deleted":2,"#, "");
        assert_eq!(module_churn(&[cell(cell_type::ATTN, short)]), None);
        // A later payload key is ignored.
        let wider = module_payload().replace('}', r#","authors":3}"#);
        assert_eq!(
            module_churn(&[cell(cell_type::ATTN, wider)]).map(|m| m.commits),
            Some(4)
        );
        // Only ATTN is read.
        assert_eq!(
            module_churn(&[cell(cell_type::FAIL, module_payload())]),
            None
        );
    }

    #[test]
    fn symbol_blame_round_trips_the_writer() {
        let cells = [cell(cell_type::ATTN, blame_payload(300))];
        assert_eq!(
            symbol_blame(&cells),
            Some(SymbolBlame {
                last: 300,
                span_changes: 3
            })
        );
        assert_eq!(module_churn(&cells), None);
        assert_eq!(pair_counts(&cells), None);
    }

    #[test]
    fn pair_counts_reads_the_cochange_cell() {
        let payload = json(&PairAttn {
            cochanges: 5,
            ratio_permille: 833,
            window_commits: 12,
        });
        let cells = [cell(cell_type::ATTN, payload)];
        assert_eq!(
            pair_counts(&cells),
            Some(PairCounts {
                cochanges: 5,
                ratio_permille: 833,
                window_commits: 12
            })
        );
        assert_eq!(module_churn(&cells), None);
        assert_eq!(symbol_blame(&cells), None);
        let text = [Cell {
            kind: cell_type::ATTN,
            payload: CellPayload::Text("x".into()),
        }];
        assert_eq!(pair_counts(&text), None);
        let bytes = [Cell {
            kind: cell_type::ATTN,
            payload: CellPayload::Bytes(b"{}".to_vec()),
        }];
        assert_eq!(pair_counts(&bytes), None);
        // An array is not read positionally.
        assert_eq!(
            pair_counts(&[cell(cell_type::ATTN, "[5,833,12]".into())]),
            None
        );
        assert_eq!(pair_counts(&[]), None);
    }

    #[test]
    fn fail_entries_reads_test_and_implicated() {
        let payload = r#"[{"id":"latest:t::a#0","frame":0,"report":"r.xml","role":"implicated","source":"junit","status":"failed","test":"t::a"},{"id":"latest:t::a","report":"r.xml","role":"test","source":"junit","status":"failed","test":"t::a","via":"qname"}]"#;
        let entries = fail_entries(&[cell(cell_type::FAIL, payload.into())]);
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(
            entries.iter().map(|e| e.role).collect::<Vec<_>>(),
            [FailRole::Implicated, FailRole::Test]
        );
        assert_eq!(
            entries.iter().map(|e| e.frame).collect::<Vec<_>>(),
            [Some(0), None]
        );
        assert_eq!(
            entries.iter().map(|e| e.via.as_deref()).collect::<Vec<_>>(),
            [None, Some("qname")]
        );
        assert_eq!(
            entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["latest:t::a#0", "latest:t::a"]
        );
        assert!(
            entries
                .iter()
                .all(|e| e.test == "t::a" && e.source == "junit" && e.status == "failed")
        );
        assert!(entries.iter().all(|e| e.run.is_none()));

        // Not an array: no entry.
        assert!(fail_entries(&[cell(cell_type::FAIL, r#"{"id":"x"}"#.into())]).is_empty());
        assert!(fail_entries(&[cell(cell_type::FAIL, "not json".into())]).is_empty());
        assert!(fail_entries(&[]).is_empty());
        // A non-object, a missing field and an unknown role are skipped; a run is read.
        let mixed = r#"[1,{"id":"a","role":"test","source":"log","status":"error"},{"id":"b","role":"owner","source":"log","status":"error","test":"t"},{"id":"r1:t","role":"test","run":"r1","source":"log","status":"error","test":"t"}]"#;
        let entries = fail_entries(&[cell(cell_type::FAIL, mixed.into())]);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].run.as_deref(), Some("r1"));
        assert_eq!(entries[0].status, "error");
    }

    /// One graph of `nodes` (kind, cells), ids 1.. in order.
    fn graph(nodes: Vec<(glia_core::NodeKindId, Vec<Cell>)>) -> MergedGraph {
        let repo = RepoId::from_canonical("test://signals");
        let mut g = glia_graph::RepoGraph {
            repo,
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        };
        for (i, (kind, cells)) in nodes.into_iter().enumerate() {
            let id = NodeId(i as u64 + 1);
            g.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells,
            });
            g.nav.kind_by_id.insert(id, kind);
        }
        MergedGraph::new(vec![g])
    }

    #[test]
    fn history_now_is_the_newest_last() {
        let m = graph(vec![
            (
                node_kind::MODULE,
                vec![cell(cell_type::ATTN, module_payload_at(50, 100))],
            ),
            (
                node_kind::FUNCTION,
                vec![cell(cell_type::ATTN, blame_payload(250))],
            ),
        ]);
        assert_eq!(history_now(&m), Some(250));
        // The module's `last` wins when it is the newer one.
        let m = graph(vec![
            (
                node_kind::FUNCTION,
                vec![cell(cell_type::ATTN, blame_payload(250))],
            ),
            (
                node_kind::MODULE,
                vec![cell(cell_type::ATTN, module_payload_at(50, 400))],
            ),
        ]);
        assert_eq!(history_now(&m), Some(400));

        // A pair payload on a node, and a FAIL cell, carry no time.
        let pair = json(&PairAttn {
            cochanges: 5,
            ratio_permille: 833,
            window_commits: 12,
        });
        let none = graph(vec![
            (node_kind::MODULE, vec![cell(cell_type::ATTN, pair)]),
            (
                node_kind::FUNCTION,
                vec![cell(cell_type::FAIL, "[]".into())],
            ),
        ]);
        assert_eq!(history_now(&none), None);
        assert_eq!(history_now(&graph(Vec::new())), None);
    }
}
