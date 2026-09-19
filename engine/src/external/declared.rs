//! Declared knowledge (LF.4a): the `.glia/overlay.toml` `[[constraint]]`,
//! `[[decision]]` and `[[note]]` stanzas become CONSTRAINT, DECISION and CONV
//! entries on the node each one anchors on.
//!
//! The stanzas come from [`RepoInputs::config`], loaded once per build
//! (LF.1a); this stage never re-reads the file. Declared knowledge is a rule
//! or a record, not an inferred edge, so `--no-overlay` does not switch it
//! off.
//!
//! ANCHOR, shared by the three kinds:
//! 1. `anchor = "<qname>"` binds through the repo's
//!    [`QnameIndex`](glia_graph::cells::QnameIndex); several nodes
//!    sharing the qname take the smallest NodeId. An anchor that binds nothing
//!    orphans the stanza (it never falls back to a scope: a stale qname must
//!    be visible).
//! 2. Otherwise the scope field - a constraint's `from` for forbid_edge, its
//!    `scope` for no_cycle / invariant, a decision's `scope` - resolves
//!    through `answers::resolve_scope` (a project path, label or qname ->
//!    its path) and binds the PROJECT node of this repo whose path it is. An
//!    absent, `.` or empty scope is the repo root's PROJECT, when one exists.
//!    A path no PROJECT sits at orphans the stanza: declare a `[[project]]`
//!    (LF.3a) to give it one.
//!
//! ENTRIES, upserted through `external_inputs::validate_entry` +
//! `merge_entry` (source `overlay`), so they share one canonical array per
//! cell with the API sidecar's (`api`) and the ADR pass's (`adr`) entries,
//! keyed by `(source, id)`:
//! - constraint -> CONSTRAINT
//!   `{"source":"overlay","id","kind","from"?,"to"?,"from_raw"?,"to_raw"?,"scope"?,"scope_raw"?,"categories"?,"text"?,"origin":"human|llm","decl"}`,
//!   every scope stored RESOLVED to a path beside its raw value, so `glia
//!   check` (LE.8) never re-resolves a label;
//! - decision -> DECISION `{"source":"overlay","id","title"?,"status"?,"text"?,"decl"}`;
//! - note -> CONV `{"source":"overlay","id","text","by"?,"decl"}`, `id`
//!   defaulting to `note#<n>` (the note's 1-based position in the file).
//!
//! `decl` is `.glia/overlay.toml:<line>` of the stanza's header. Stanzas apply
//! in file order (constraints, then decisions, then notes); every instance of
//! the anchor node (one id can sit in several language graphs) gets the entry.
//!
//! fired_on marker, once per repo whose file declares any of the three:
//!   `[declared] repo=<label> constraint=<c> decision=<d> note=<n> anchored=<a> orphaned=<o> (anchor_qname=<q> anchor_project=<p>)`
//! with a trailing ` rejected=<x>` only when an entry failed its rules or met
//! a cell that is not an entry array. Then one detail line per stanza that
//! did not anchor, at most [`MAX_DETAIL_LINES`] per repo:
//!   `[declared] orphaned <section> id=<id> <decl> anchor=<qname>` or
//!   `[declared] orphaned <section> id=<id> <decl> scope=<raw> -> <path> (no PROJECT)`.
//!
//! [`declared_constraints`] is the single reader of the rules: every
//! CONSTRAINT entry in the graph (overlay or API) as `(anchor, rule)` pairs.
//! After the stages ran, the build reports what it reads back:
//!   `[declared] rules=<r> (forbid_edge=<f> no_cycle=<c> invariant=<i>)`.

use std::collections::{BTreeMap, HashMap, HashSet};

use glia_code_domain::cell_type;
use glia_code_domain::external_inputs::{
    ConstraintKind, ConstraintRule, merge_entry, parse_constraints, validate_entry,
};
use glia_code_domain::glia_config::{LoadedConfig, NOTE_ID_PREFIX, Origin};
use glia_core::{Cell, CellTypeId, NodeId};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex};
use serde_json::{Map, Value};

use super::RepoInputs;
use crate::answers::{ProjectInfo, project_roots, resolve_scope};

/// Detail lines printed per repo before the rest are summarised.
const MAX_DETAIL_LINES: usize = 32;

/// How a stanza found its node.
#[derive(Debug, Clone, Copy)]
enum Via {
    Qname,
    Project,
}

/// Outcome counts of one repo's declared stanzas.
#[derive(Debug, Default)]
struct Tally {
    constraint: usize,
    decision: usize,
    note: usize,
    anchor_qname: usize,
    anchor_project: usize,
    orphaned: usize,
    rejected: usize,
    details: Vec<String>,
}

/// One stanza's cell entry and where it asks to hang.
struct Stanza {
    section: &'static str,
    cell: CellTypeId,
    id: String,
    decl: String,
    /// `anchor = <qname>`, when given.
    anchor: Option<String>,
    /// The scope that picks a PROJECT when there is no `anchor`: `None` is
    /// the repo root.
    scope: Option<String>,
    entry: Value,
}

/// The repo's anchor resolvers, built once per repo.
struct Anchors {
    idx: QnameIndex,
    /// Every PROJECT anchor of the build (all repos): `resolve_scope`'s
    /// vocabulary. Bound to THIS repo's node through `idx`.
    projects: Vec<ProjectInfo>,
    /// Every in-repo instance of a node, looked up by id only.
    at: HashMap<NodeId, Vec<(usize, usize)>>,
    /// `raw scope -> resolved path`, so a scope shared by several stanzas is
    /// resolved (and its `[scope]` line printed) once.
    scopes: BTreeMap<String, String>,
}

impl Anchors {
    fn build(merged: &MergedGraph, input: &RepoInputs) -> Self {
        let mut at: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
        for (gi, g) in merged.graphs.iter().enumerate() {
            if g.repo != input.repo {
                continue;
            }
            for (ni, n) in g.nodes.iter().enumerate() {
                at.entry(n.id).or_default().push((gi, ni));
            }
        }
        Anchors {
            idx: QnameIndex::build(merged, Some(input.repo)),
            projects: project_roots(merged),
            at,
            scopes: BTreeMap::new(),
        }
    }

    /// The node `qname` names in this repo; the smallest NodeId on ties.
    fn by_qname(&self, qname: &str) -> Option<NodeId> {
        match self.idx.resolve(qname.trim(), None, None) {
            CellTarget::Bound(id) | CellTarget::Ambiguous(id) => Some(id),
            _ => None,
        }
    }

    /// `raw` resolved to a repo-relative path (`.` = the root).
    fn path_of(&mut self, merged: &MergedGraph, raw: &str) -> String {
        if let Some(p) = self.scopes.get(raw) {
            return p.clone();
        }
        let path = resolve_scope(merged, &normalise_scope(raw));
        self.scopes.insert(raw.to_string(), path.clone());
        path
    }

    /// This repo's PROJECT node at `path`.
    fn project_at(&self, path: &str) -> Option<NodeId> {
        self.projects.iter().filter(|p| p.path == path).find_map(|p| {
            match self.idx.resolve(&p.qname, Some("PROJECT"), None) {
                CellTarget::Bound(id) | CellTarget::Ambiguous(id) => Some(id),
                _ => None,
            }
        })
    }
}

/// A scope string as a path: no leading `./` or `/`, no trailing `/`; empty
/// is `.`. A label or qname passes through (only its ends are trimmed).
fn normalise_scope(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix("./").unwrap_or(s);
    let s = s.trim_start_matches('/').trim_end_matches('/');
    if s.is_empty() { ".".to_string() } else { s.to_string() }
}

fn origin_name(o: Origin) -> &'static str {
    match o {
        Origin::Human => "human",
        Origin::Llm => "llm",
    }
}

/// An object from the non-empty `(key, value)` pairs.
fn object(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    }
    Value::Object(m)
}

fn text(s: &Option<String>) -> Option<Value> {
    s.as_deref().filter(|t| !t.trim().is_empty()).map(|t| Value::String(t.to_string()))
}

/// Every declared stanza of `cfg` as an entry, in file order per section.
/// Scopes of constraints are resolved here (they are stored resolved).
fn stanzas(cfg: &LoadedConfig, merged: &MergedGraph, anchors: &mut Anchors) -> Vec<Stanza> {
    let c = &cfg.config;
    let mut out = Vec::with_capacity(c.constraint.len() + c.decision.len() + c.note.len());
    let s = |v: &str| Some(Value::String(v.to_string()));
    for spanned in &c.constraint {
        let d = spanned.get_ref();
        let decl = cfg.decl_of(spanned.span());
        let mut resolve = |raw: &Option<String>| -> (Option<Value>, Option<Value>) {
            match raw.as_deref().filter(|r| !r.trim().is_empty()) {
                Some(r) => (s(&anchors.path_of(merged, r)), s(r)),
                None => (None, None),
            }
        };
        let (from, from_raw) = resolve(&d.from);
        let (to, to_raw) = resolve(&d.to);
        let (scope, scope_raw) = resolve(&d.scope);
        let categories = (!d.categories.is_empty())
            .then(|| Value::Array(d.categories.iter().map(|n| Value::String(n.clone())).collect()));
        let entry = object(vec![
            ("source", s("overlay")),
            ("id", s(&d.id)),
            ("kind", s(&d.kind)),
            ("from", from),
            ("from_raw", from_raw),
            ("to", to),
            ("to_raw", to_raw),
            ("scope", scope),
            ("scope_raw", scope_raw),
            ("categories", categories),
            ("text", text(&d.text)),
            ("origin", s(origin_name(d.origin))),
            ("decl", s(&decl)),
        ]);
        let scope = if d.kind == "forbid_edge" { d.from.clone() } else { d.scope.clone() };
        out.push(Stanza {
            section: "constraint",
            cell: cell_type::CONSTRAINT,
            id: d.id.clone(),
            decl,
            anchor: d.anchor.clone(),
            scope,
            entry,
        });
    }
    for spanned in &c.decision {
        let d = spanned.get_ref();
        let decl = cfg.decl_of(spanned.span());
        let entry = object(vec![
            ("source", s("overlay")),
            ("id", s(&d.id)),
            ("title", text(&d.title)),
            ("status", text(&d.status)),
            ("text", text(&d.text)),
            ("decl", s(&decl)),
        ]);
        out.push(Stanza {
            section: "decision",
            cell: cell_type::DECISION,
            id: d.id.clone(),
            decl,
            anchor: d.anchor.clone(),
            scope: d.scope.clone(),
            entry,
        });
    }
    for (i, spanned) in c.note.iter().enumerate() {
        let d = spanned.get_ref();
        let decl = cfg.decl_of(spanned.span());
        let id = d.id.clone().unwrap_or_else(|| format!("{NOTE_ID_PREFIX}{}", i + 1));
        let entry = object(vec![
            ("source", s("overlay")),
            ("id", s(&id)),
            ("text", s(&d.text)),
            ("by", text(&d.by)),
            ("decl", s(&decl)),
        ]);
        out.push(Stanza {
            section: "note",
            cell: cell_type::CONV,
            id,
            decl,
            anchor: Some(d.anchor.clone()),
            scope: None,
            entry,
        });
    }
    out
}

/// Apply `input`'s declared stanzas to `merged`. True when a cell was written.
pub(super) fn apply_declared_cells(
    merged: &mut MergedGraph,
    input: &RepoInputs,
    cfg: &LoadedConfig,
) -> bool {
    let c = &cfg.config;
    if c.constraint.is_empty() && c.decision.is_empty() && c.note.is_empty() {
        return false;
    }
    let mut t = Tally {
        constraint: c.constraint.len(),
        decision: c.decision.len(),
        note: c.note.len(),
        ..Tally::default()
    };
    let mut anchors = Anchors::build(merged, input);
    let mut wrote = false;
    for st in stanzas(cfg, merged, &mut anchors) {
        let what = format!("{} id={} {}", st.section, st.id, st.decl);
        if let Err(e) = validate_entry(st.cell, &st.entry) {
            t.rejected += 1;
            t.details.push(format!("rejected {what}: {e}"));
            continue;
        }
        let bound = match &st.anchor {
            Some(q) => anchors.by_qname(q).map(|id| (id, Via::Qname)).ok_or_else(|| format!("anchor={q}")),
            None => {
                let raw = st.scope.clone().unwrap_or_default();
                let path = anchors.path_of(merged, &raw);
                anchors
                    .project_at(&path)
                    .map(|id| (id, Via::Project))
                    .ok_or_else(|| format!("scope={raw} -> {path} (no PROJECT)"))
            }
        };
        let (id, via) = match bound {
            Ok(b) => b,
            Err(where_) => {
                t.orphaned += 1;
                t.details.push(format!("orphaned {what} {where_}"));
                continue;
            }
        };
        match write_entry(merged, &anchors, id, st.cell, &st.entry) {
            Ok(()) => {
                wrote = true;
                match via {
                    Via::Qname => t.anchor_qname += 1,
                    Via::Project => t.anchor_project += 1,
                }
            }
            Err(e) => {
                t.rejected += 1;
                t.details.push(format!("rejected {what}: {e}"));
            }
        }
    }
    report(&input.label, &t);
    wrote
}

/// Upsert `entry` into the `cell` array of every in-repo instance of `id`.
/// Every instance is checked before any is written, so a rejection leaves
/// the node untouched.
fn write_entry(
    merged: &mut MergedGraph,
    anchors: &Anchors,
    id: NodeId,
    cell: CellTypeId,
    entry: &Value,
) -> Result<(), String> {
    let places = anchors.at.get(&id).map_or(&[][..], Vec::as_slice);
    let mut payloads = Vec::with_capacity(places.len());
    for &(gi, ni) in places {
        let node = merged
            .graphs
            .get(gi)
            .and_then(|g| g.nodes.get(ni))
            .filter(|n| n.id == id)
            .ok_or_else(|| format!("stale index: node {} is not at graph {gi} node {ni}", id.0))?;
        let existing = node.cells.iter().find(|c| c.kind == cell).map(|c| &c.payload);
        payloads.push(merge_entry(existing, entry)?);
    }
    for (&(gi, ni), payload) in places.iter().zip(payloads) {
        if let Some(node) = merged.graphs.get_mut(gi).and_then(|g| g.nodes.get_mut(ni)) {
            match node.cells.iter_mut().find(|c| c.kind == cell) {
                Some(c) => c.payload = payload,
                None => node.cells.push(Cell { kind: cell, payload }),
            }
        }
    }
    Ok(())
}

fn report(label: &str, t: &Tally) {
    let rejected = if t.rejected > 0 { format!(" rejected={}", t.rejected) } else { String::new() };
    eprintln!(
        "[declared] repo={label} constraint={} decision={} note={} anchored={} orphaned={} (anchor_qname={} anchor_project={}){rejected}",
        t.constraint,
        t.decision,
        t.note,
        t.anchor_qname + t.anchor_project,
        t.orphaned,
        t.anchor_qname,
        t.anchor_project,
    );
    for line in t.details.iter().take(MAX_DETAIL_LINES) {
        eprintln!("[declared] {line}");
    }
    if t.details.len() > MAX_DETAIL_LINES {
        eprintln!("[declared] ... {} more detail lines", t.details.len() - MAX_DETAIL_LINES);
    }
}

/// Every CONSTRAINT rule in `merged` - declared in an overlay or written
/// through the cell API - with the node it hangs on, in NodeId order (a
/// node's rules in stored `(source, id)` order). The one entry point `glia
/// check` (LE.8) reads rules through.
///
/// A rule's scopes come back as paths: an overlay rule stored them resolved,
/// and an API rule's are resolved here through the same `resolve_scope`,
/// which leaves a path unchanged. One id in several language graphs is read
/// once.
pub(crate) fn declared_constraints(merged: &MergedGraph) -> Vec<(NodeId, ConstraintRule)> {
    let mut seen = HashSet::new();
    let mut out: Vec<(NodeId, ConstraintRule)> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(cell) = n.cells.iter().find(|c| c.kind == cell_type::CONSTRAINT) else {
                continue;
            };
            if seen.insert(n.id) {
                out.extend(parse_constraints(&cell.payload).into_iter().map(|r| (n.id, r)));
            }
        }
    }
    // Stable: a node's rules keep their stored order.
    out.sort_by_key(|(id, _)| id.0);
    let mut paths: BTreeMap<String, String> = BTreeMap::new();
    let mut path = |raw: &mut String| {
        let resolved = paths
            .entry(raw.clone())
            .or_insert_with(|| resolve_scope(merged, &normalise_scope(raw)))
            .clone();
        *raw = resolved;
    };
    for (_, rule) in &mut out {
        if rule.source == "overlay" {
            continue;
        }
        match &mut rule.kind {
            ConstraintKind::ForbidEdge { from, to } => {
                path(from);
                path(to);
            }
            ConstraintKind::NoCycle { scope: Some(scope) } => path(scope),
            _ => {}
        }
    }
    out
}

/// The build's read-back of the rules [`declared_constraints`] hands `glia
/// check`, printed when there is at least one:
/// `[declared] rules=<r> (forbid_edge=<f> no_cycle=<c> invariant=<i>)`.
pub(super) fn report_rules(merged: &MergedGraph) {
    let rules = declared_constraints(merged);
    if rules.is_empty() {
        return;
    }
    let count = |name: &str| rules.iter().filter(|(_, r)| r.kind.name() == name).count();
    eprintln!(
        "[declared] rules={} (forbid_edge={} no_cycle={} invariant={})",
        rules.len(),
        count("forbid_edge"),
        count("no_cycle"),
        count("invariant")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::CellPayload;

    const OVERLAY: &str = "version = 1

[[constraint]]
id = \"web-no-api-internals\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"CALLS\"]

[[constraint]]
id = \"api-acyclic\"
kind = \"no_cycle\"
scope = \"./services/api/\"
";

    #[test]
    fn declared_constraints_reads_every_rule_in_node_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (path, body) in [
            ("web/package.json", "{\"name\": \"web\"}\n"),
            ("web/src/ui.ts", "export function render() { return 1; }\n"),
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
            ("services/api/app.py", "def charge(order_id):\n    return order_id\n"),
            (".glia/overlay.toml", OVERLAY),
            (
                ".glia/cells.jsonl",
                "{\"qname\":\"project:web\",\"cell\":\"CONSTRAINT\",\"entry\":{\"source\":\"api\",\"id\":\"w\",\"kind\":\"forbid_edge\",\"from\":\"./web\",\"to\":\"api\"}}\n",
            ),
        ] {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let r = crate::generate_one(root.to_str().unwrap()).unwrap();
        let rules = declared_constraints(&r.merged);
        let got: Vec<(&str, &str, &str, Option<&str>)> = rules
            .iter()
            .map(|(_, rule)| (rule.id.as_str(), rule.source.as_str(), rule.kind.name(), rule.decl.as_deref()))
            .collect();
        let web = rules.iter().find(|(_, x)| x.id == "w").map(|(id, _)| *id).unwrap();
        let api = rules.iter().find(|(_, x)| x.id == "api-acyclic").map(|(id, _)| *id).unwrap();
        let mut want = [
            (web, ("w", "api", "forbid_edge", None)),
            (web, ("web-no-api-internals", "overlay", "forbid_edge", Some(".glia/overlay.toml:3"))),
            (api, ("api-acyclic", "overlay", "no_cycle", Some(".glia/overlay.toml:10"))),
        ];
        want.sort_by_key(|(id, _)| id.0);
        assert_eq!(got, want.iter().map(|(_, w)| *w).collect::<Vec<_>>());
        let by_id = |id: &str| rules.iter().find(|(_, x)| x.id == id).map(|(_, x)| x.kind.clone()).unwrap();
        // The API rule's `./web` and label `api` come back as paths.
        assert_eq!(by_id("w"), ConstraintKind::ForbidEdge { from: "web".into(), to: "services/api".into() });
        assert_eq!(by_id("api-acyclic"), ConstraintKind::NoCycle { scope: Some("services/api".into()) });
        // The stored overlay entry keeps both the resolved and the raw scope.
        let stored = r.merged.graphs.iter().flat_map(|g| &g.nodes).find(|n| n.id == api).unwrap();
        let payload = stored.cells.iter().find(|c| c.kind == cell_type::CONSTRAINT).unwrap();
        let CellPayload::Json(s) = &payload.payload else { panic!("CONSTRAINT is JSON") };
        assert!(s.contains(r#""scope":"services/api","scope_raw":"./services/api/""#), "{s}");
    }

    #[test]
    fn normalise_scope_strips_path_decoration() {
        assert_eq!(normalise_scope(""), ".");
        assert_eq!(normalise_scope(" . "), ".");
        assert_eq!(normalise_scope("./"), ".");
        assert_eq!(normalise_scope("./services/api/"), "services/api");
        assert_eq!(normalise_scope("/web"), "web");
        assert_eq!(normalise_scope("@shop/web"), "@shop/web");
    }
}
