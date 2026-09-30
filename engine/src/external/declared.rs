//! Declared knowledge (LF.4a): the `.glia/overlay.toml` `[[constraint]]`,
//! `[[decision]]` and `[[note]]` stanzas become CONSTRAINT, DECISION and CONV
//! entries on the node each one anchors on; the reflexion model's
//! `[[component]]` and `[[layer]]` stanzas (CC.5a) become CONSTRAINT entries
//! of kind `component` / `layer`, and `[[constraint]] kind = "allow"` one of
//! kind `allow`.
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
//! THE MODEL'S ANCHORS (CC.5a), after rule 1 (`anchor`, on a component or an
//! allow; a layer takes none):
//! - a component without `anchor` binds the PROJECT at its first resolved
//!   path, else the repo root's PROJECT, else the smallest-id MODULE of this
//!   repo whose POSITION file lies under that path (a repo with no root
//!   manifest has no root PROJECT); nothing -> orphaned;
//! - a layer, and an allow without `anchor`, bind the repo root's PROJECT,
//!   else the node their first named component (a layer's first component,
//!   an allow's `from`) anchored on; a component that did not anchor orphans
//!   them.
//!
//! A component path resolves like a scope (a project label works) and is
//! stored resolved beside `paths_raw`. Two paths resolving to one path (a
//! label and its directory) are one path claimed twice: the later component
//! is rejected, so no two stored components own an equal path.
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
//!   defaulting to `note#<n>` (the note's 1-based position in the file);
//! - component -> CONSTRAINT
//!   `{"source":"overlay","id":"component:<name>","kind":"component","name","paths":[resolved],"paths_raw":[..],"text"?,"origin","decl"}`;
//! - layer -> CONSTRAINT
//!   `{"source":"overlay","id":"layer:<name>","kind":"layer","name","rank","components","strict","decl"}`,
//!   `rank` its 0-based position among the kept layers (top first);
//! - allow -> CONSTRAINT
//!   `{"source":"overlay","id","kind":"allow","from","to","categories"?,"text"?,"origin","decl"}`,
//!   `from` / `to` the component names as written.
//!
//! `decl` is `.glia/overlay.toml:<line>` of the stanza's header. Stanzas apply
//! in file order per section - components, then layers (their anchors can
//! depend on a component's), then constraints, decisions and notes; every
//! instance of the anchor node (one id can sit in several language graphs)
//! gets the entry.
//!
//! fired_on markers. Once per repo whose file declares a constraint, decision
//! or note (an allow is a constraint):
//!   `[declared] repo=<label> constraint=<c> decision=<d> note=<n> anchored=<a> orphaned=<o> (anchor_qname=<q> anchor_project=<p>)`
//! with ` anchor_module=<m>` inside the parentheses only when an allow bound
//! through a component's MODULE, and a trailing ` rejected=<x>` only when an
//! entry failed its rules or met a cell that is not an entry array. Once per
//! repo whose file declares a model (a component, a layer or an allow):
//!   `[declared] model repo=<label> components=<c> layers=<l> allows=<a> anchored=<n> orphaned=<o>`
//! (+ ` rejected=<x>`), counting those three kinds. Then one detail line per
//! stanza that did not anchor, at most [`MAX_DETAIL_LINES`] per repo:
//!   `[declared] orphaned <section> id=<id> <decl> anchor=<qname>`,
//!   `[declared] orphaned <section> id=<id> <decl> scope=<raw> -> <path> (no PROJECT)`,
//!   `[declared] orphaned component id=<id> <decl> path=<raw> -> <path> (no PROJECT or MODULE)` or
//!   `[declared] orphaned <layer|constraint> id=<id> <decl> component=<name> (no root PROJECT; the component did not anchor)`.
//!
//! [`declared_constraints`] is the single reader of the rules: every
//! CONSTRAINT entry in the graph (overlay or API) as `(anchor, rule)` pairs.
//! After the stages ran, the build reports what it reads back:
//!   `[declared] rules=<r> (forbid_edge=<f> no_cycle=<c> invariant=<i>)`,
//! where `r` also counts the allows; component / layer entries are model
//! declarations, not rules, and are reported on the model line.

use std::collections::{BTreeMap, HashMap, HashSet};

use glia_code_domain::external_inputs::{
    ConstraintKind, ConstraintRule, merge_entry, parse_constraints, validate_entry,
};
use glia_code_domain::glia_config::{
    COMPONENT_ID_PREFIX, LAYER_ID_PREFIX, LoadedConfig, MODEL_KINDS, NOTE_ID_PREFIX, Origin,
};
use glia_code_domain::{cell_type, node_kind};
use glia_core::{Cell, CellPayload, CellTypeId, NodeId};
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
    /// A component's smallest-id MODULE under its first path (CC.5a).
    Module,
}

/// Outcome counts of one repo's declared stanzas: the LF.4a line counts the
/// constraint / decision / note stanzas, the model line (CC.5a) the component
/// / layer / allow ones - an allow is in both.
#[derive(Debug, Default)]
struct Tally {
    constraint: usize,
    decision: usize,
    note: usize,
    anchor_qname: usize,
    anchor_project: usize,
    anchor_module: usize,
    orphaned: usize,
    rejected: usize,
    component: usize,
    layer: usize,
    allow: usize,
    model_anchored: usize,
    model_orphaned: usize,
    model_rejected: usize,
    details: Vec<String>,
}

impl Tally {
    fn anchored(&mut self, st: &Stanza, via: Via) {
        if st.declared {
            match via {
                Via::Qname => self.anchor_qname += 1,
                Via::Project => self.anchor_project += 1,
                Via::Module => self.anchor_module += 1,
            }
        }
        if st.model {
            self.model_anchored += 1;
        }
    }

    fn orphaned(&mut self, st: &Stanza, detail: String) {
        self.orphaned += usize::from(st.declared);
        self.model_orphaned += usize::from(st.model);
        self.details.push(detail);
    }

    fn rejected(&mut self, st: &Stanza, detail: String) {
        self.rejected += usize::from(st.declared);
        self.model_rejected += usize::from(st.model);
        self.details.push(detail);
    }
}

/// Where a stanza asks to hang.
enum Target {
    /// `anchor = <qname>`: binds, or orphans the stanza (never a fallback).
    Qname(String),
    /// The scope that picks a PROJECT: `None` is the repo root.
    Scope(Option<String>),
    /// A component without `anchor`: the PROJECT at `path` (its first
    /// resolved path, `raw` as written), else the root PROJECT, else this
    /// repo's smallest-id MODULE under `path`.
    Component { raw: String, path: String },
    /// A layer, or an allow without `anchor`: the root PROJECT, else where
    /// component `component` anchored.
    Model { component: String },
}

/// One stanza's cell entry and where it asks to hang.
struct Stanza {
    section: &'static str,
    cell: CellTypeId,
    id: String,
    decl: String,
    target: Target,
    /// Counted on the LF.4a line (constraint / decision / note).
    declared: bool,
    /// Counted on the model line (component / layer / allow).
    model: bool,
    /// A component's name: where it anchors is recorded for its layer and
    /// its allows.
    component: Option<String>,
    /// Why the build rejects the stanza before anchoring it, if it does.
    reject: Option<String>,
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
    /// This repo's MODULE nodes with their POSITION file, sorted by NodeId:
    /// the component fallback anchor. Built on first use.
    modules: Option<Vec<(NodeId, String)>>,
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
            modules: None,
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

    /// The smallest-id MODULE of this repo whose POSITION file lies under
    /// `path` (`.` = every file).
    fn module_under(&mut self, merged: &MergedGraph, path: &str) -> Option<NodeId> {
        let modules = self.modules.get_or_insert_with(|| {
            let mut out: Vec<(NodeId, String)> = Vec::new();
            for &(gi, ni) in self.at.values().flatten() {
                let Some(g) = merged.graphs.get(gi) else { continue };
                let Some(n) = g.nodes.get(ni) else { continue };
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                    continue;
                }
                if let Some(file) = position_file(&n.cells) {
                    out.push((n.id, file));
                }
            }
            // NodeId then file: HashMap order never picks the winner.
            out.sort_by(|a, b| (a.0.0, &a.1).cmp(&(b.0.0, &b.1)));
            out.dedup();
            out
        });
        modules.iter().find(|(_, file)| is_under(file, path)).map(|(id, _)| *id)
    }

    /// Where a component without `anchor` hangs (see the module docs).
    fn component_anchor(&mut self, merged: &MergedGraph, path: &str) -> Option<(NodeId, Via)> {
        self.project_at(path)
            .or_else(|| self.project_at("."))
            .map(|id| (id, Via::Project))
            .or_else(|| self.module_under(merged, path).map(|id| (id, Via::Module)))
    }
}

/// The `file` of a node's first POSITION cell.
fn position_file(cells: &[Cell]) -> Option<String> {
    cells.iter().filter(|c| c.kind == cell_type::POSITION).find_map(|c| match &c.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => serde_json::from_str::<Value>(s)
            .ok()?
            .get("file")?
            .as_str()
            .map(String::from),
        CellPayload::Bytes(_) => None,
    })
}

/// `file` is `path` or lies below it, by whole path segments.
fn is_under(file: &str, path: &str) -> bool {
    path == "." || file == path || file.strip_prefix(path).is_some_and(|rest| rest.starts_with('/'))
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

/// Every declared stanza of `cfg` as an entry, in file order per section:
/// components, layers, constraints, decisions, notes. Scopes of constraints
/// and component paths are resolved here (they are stored resolved).
fn stanzas(cfg: &LoadedConfig, merged: &MergedGraph, anchors: &mut Anchors) -> Vec<Stanza> {
    let c = &cfg.config;
    let mut out = Vec::with_capacity(
        c.component.len() + c.layer.len() + c.constraint.len() + c.decision.len() + c.note.len(),
    );
    let s = |v: &str| Some(Value::String(v.to_string()));
    let strings = |v: &[String]| Some(Value::Array(v.iter().map(|x| Value::String(x.clone())).collect()));
    // resolved path -> the component that claimed it first.
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();
    for spanned in &c.component {
        let d = spanned.get_ref();
        let decl = cfg.decl_of(spanned.span());
        let paths: Vec<String> = d.paths.iter().map(|raw| anchors.path_of(merged, raw)).collect();
        let mut reject = None;
        let mut mine: Vec<&String> = Vec::with_capacity(paths.len());
        for (raw, path) in d.paths.iter().zip(&paths) {
            let owner = claimed.get(path).cloned().or_else(|| mine.contains(&path).then(|| d.name.clone()));
            if let Some(owner) = owner {
                reject = Some(format!("path {raw} -> {path} is already claimed by component {owner}"));
                break;
            }
            mine.push(path);
        }
        if reject.is_none() {
            for path in mine {
                claimed.insert(path.clone(), d.name.clone());
            }
        }
        let id = format!("{COMPONENT_ID_PREFIX}{}", d.name);
        let entry = object(vec![
            ("source", s("overlay")),
            ("id", s(&id)),
            ("kind", s("component")),
            ("name", s(&d.name)),
            ("paths", strings(&paths)),
            ("paths_raw", strings(&d.paths)),
            ("text", text(&d.text)),
            ("origin", s(origin_name(d.origin))),
            ("decl", s(&decl)),
        ]);
        let target = match &d.anchor {
            Some(q) => Target::Qname(q.clone()),
            None => Target::Component {
                raw: d.paths.first().cloned().unwrap_or_default(),
                path: paths.first().cloned().unwrap_or_else(|| ".".to_string()),
            },
        };
        out.push(Stanza {
            section: "component",
            cell: cell_type::CONSTRAINT,
            id,
            decl,
            target,
            declared: false,
            model: true,
            component: Some(d.name.clone()),
            reject,
            entry,
        });
    }
    for (rank, spanned) in c.layer.iter().enumerate() {
        let d = spanned.get_ref();
        let decl = cfg.decl_of(spanned.span());
        let id = format!("{LAYER_ID_PREFIX}{}", d.name);
        let entry = object(vec![
            ("source", s("overlay")),
            ("id", s(&id)),
            ("kind", s("layer")),
            ("name", s(&d.name)),
            ("rank", Some(Value::from(u32::try_from(rank).unwrap_or(u32::MAX)))),
            ("components", strings(&d.components)),
            ("strict", Some(Value::Bool(d.strict))),
            ("decl", s(&decl)),
        ]);
        out.push(Stanza {
            section: "layer",
            cell: cell_type::CONSTRAINT,
            id,
            decl,
            target: Target::Model { component: d.components.first().cloned().unwrap_or_default() },
            declared: false,
            model: true,
            component: None,
            reject: None,
            entry,
        });
    }
    for spanned in &c.constraint {
        let d = spanned.get_ref();
        let decl = cfg.decl_of(spanned.span());
        let categories = (!d.categories.is_empty()).then(|| strings(&d.categories)).flatten();
        if d.kind == "allow" {
            // Component names, stored raw: they are not scopes.
            let from = d.from.clone().unwrap_or_default();
            let entry = object(vec![
                ("source", s("overlay")),
                ("id", s(&d.id)),
                ("kind", s("allow")),
                ("from", s(&from)),
                ("to", d.to.as_deref().and_then(s)),
                ("categories", categories),
                ("text", text(&d.text)),
                ("origin", s(origin_name(d.origin))),
                ("decl", s(&decl)),
            ]);
            let target = match &d.anchor {
                Some(q) => Target::Qname(q.clone()),
                None => Target::Model { component: from },
            };
            out.push(Stanza {
                section: "constraint",
                cell: cell_type::CONSTRAINT,
                id: d.id.clone(),
                decl,
                target,
                declared: true,
                model: true,
                component: None,
                reject: None,
                entry,
            });
            continue;
        }
        let mut resolve = |raw: &Option<String>| -> (Option<Value>, Option<Value>) {
            match raw.as_deref().filter(|r| !r.trim().is_empty()) {
                Some(r) => (s(&anchors.path_of(merged, r)), s(r)),
                None => (None, None),
            }
        };
        let (from, from_raw) = resolve(&d.from);
        let (to, to_raw) = resolve(&d.to);
        let (scope, scope_raw) = resolve(&d.scope);
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
            target: d.anchor.clone().map_or(Target::Scope(scope), Target::Qname),
            declared: true,
            model: false,
            component: None,
            reject: None,
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
            target: d.anchor.clone().map_or(Target::Scope(d.scope.clone()), Target::Qname),
            declared: true,
            model: false,
            component: None,
            reject: None,
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
            target: Target::Qname(d.anchor.clone()),
            declared: true,
            model: false,
            component: None,
            reject: None,
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
    if c.constraint.is_empty()
        && c.decision.is_empty()
        && c.note.is_empty()
        && c.component.is_empty()
        && c.layer.is_empty()
    {
        return false;
    }
    let mut t = Tally {
        constraint: c.constraint.len(),
        decision: c.decision.len(),
        note: c.note.len(),
        component: c.component.len(),
        layer: c.layer.len(),
        allow: c.constraint.iter().filter(|s| s.get_ref().kind == "allow").count(),
        ..Tally::default()
    };
    let mut anchors = Anchors::build(merged, input);
    // component name -> where it anchored: a layer's / an allow's fallback.
    let mut components: BTreeMap<String, (NodeId, Via)> = BTreeMap::new();
    let mut wrote = false;
    for st in stanzas(cfg, merged, &mut anchors) {
        let what = format!("{} id={} {}", st.section, st.id, st.decl);
        if let Some(e) = &st.reject {
            t.rejected(&st, format!("rejected {what}: {e}"));
            continue;
        }
        if let Err(e) = validate_entry(st.cell, &st.entry) {
            t.rejected(&st, format!("rejected {what}: {e}"));
            continue;
        }
        let bound = match &st.target {
            Target::Qname(q) => anchors.by_qname(q).map(|id| (id, Via::Qname)).ok_or_else(|| format!("anchor={q}")),
            Target::Scope(scope) => {
                let raw = scope.clone().unwrap_or_default();
                let path = anchors.path_of(merged, &raw);
                anchors
                    .project_at(&path)
                    .map(|id| (id, Via::Project))
                    .ok_or_else(|| format!("scope={raw} -> {path} (no PROJECT)"))
            }
            Target::Component { raw, path } => anchors
                .component_anchor(merged, path)
                .ok_or_else(|| format!("path={raw} -> {path} (no PROJECT or MODULE)")),
            Target::Model { component } => anchors
                .project_at(".")
                .map(|id| (id, Via::Project))
                .or_else(|| components.get(component).copied())
                .ok_or_else(|| {
                    format!("component={component} (no root PROJECT; the component did not anchor)")
                }),
        };
        let (id, via) = match bound {
            Ok(b) => b,
            Err(where_) => {
                t.orphaned(&st, format!("orphaned {what} {where_}"));
                continue;
            }
        };
        match write_entry(merged, &anchors, id, st.cell, &st.entry) {
            Ok(()) => {
                wrote = true;
                t.anchored(&st, via);
                if let Some(name) = &st.component {
                    components.insert(name.clone(), (id, via));
                }
            }
            Err(e) => t.rejected(&st, format!("rejected {what}: {e}")),
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
    let nonzero = |key: &str, n: usize| if n > 0 { format!(" {key}={n}") } else { String::new() };
    if t.constraint + t.decision + t.note > 0 {
        eprintln!(
            "[declared] repo={label} constraint={} decision={} note={} anchored={} orphaned={} (anchor_qname={} anchor_project={}{}){}",
            t.constraint,
            t.decision,
            t.note,
            t.anchor_qname + t.anchor_project + t.anchor_module,
            t.orphaned,
            t.anchor_qname,
            t.anchor_project,
            nonzero("anchor_module", t.anchor_module),
            nonzero("rejected", t.rejected),
        );
    }
    if t.component + t.layer + t.allow > 0 {
        eprintln!(
            "[declared] model repo={label} components={} layers={} allows={} anchored={} orphaned={}{}",
            t.component,
            t.layer,
            t.allow,
            t.model_anchored,
            t.model_orphaned,
            nonzero("rejected", t.model_rejected),
        );
    }
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
/// which leaves a path unchanged - a forbid_edge's `from` / `to`, a
/// no_cycle's `scope` and a component's `paths` (CC.5a; an allow's `from` /
/// `to` are component names and stay as stored). One id in several language
/// graphs is read once.
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
            ConstraintKind::Component { paths: component_paths, .. } => {
                for p in component_paths {
                    path(p);
                }
            }
            _ => {}
        }
    }
    out
}

/// The build's read-back of the rules [`declared_constraints`] hands `glia
/// check`, printed when there is at least one:
/// `[declared] rules=<r> (forbid_edge=<f> no_cycle=<c> invariant=<i>)`. `r`
/// counts every rule, the allows included; the model's component / layer
/// declarations are not rules (the `[declared] model` line counts them).
pub(super) fn report_rules(merged: &MergedGraph) {
    let mut rules = declared_constraints(merged);
    rules.retain(|(_, r)| !MODEL_KINDS.contains(&r.kind.name()));
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

    /// CC.5a: a component path given as a project label is stored resolved;
    /// a component whose path has no PROJECT, in a repo with no root PROJECT,
    /// hangs on the smallest-id MODULE under that path; a label resolving to a
    /// path another component claimed rejects the later component; an API
    /// component's paths come back resolved from `declared_constraints`.
    #[test]
    fn reflexion_model_resolves_paths_and_falls_back_to_a_module() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let overlay = "version = 1

[[component]]
name = \"api\"
paths = [\"api\"]

[[component]]
name = \"lib\"
paths = [\"lib\"]

[[component]]
name = \"dup\"
paths = [\"services/api\"]

[[layer]]
name = \"core\"
components = [\"lib\", \"api\"]
strict = true

[[constraint]]
id = \"lib-uses-api\"
kind = \"allow\"
from = \"lib\"
to = \"api\"
";
        for (path, body) in [
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
            ("services/api/app.py", "def charge(order_id):\n    return order_id\n"),
            ("lib/util.py", "def helper():\n    return 1\n"),
            ("lib/more.py", "def other():\n    return 2\n"),
            (".glia/overlay.toml", overlay),
            (
                ".glia/cells.jsonl",
                "{\"qname\":\"project:services/api\",\"cell\":\"CONSTRAINT\",\"entry\":{\"source\":\"api\",\"id\":\"component:ext\",\"kind\":\"component\",\"name\":\"ext\",\"paths\":[\"./lib/\",\"api\"]}}\n",
            ),
        ] {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let r = crate::generate_one(root.to_str().unwrap()).unwrap();
        let rules = declared_constraints(&r.merged);
        let kind_of = |id: NodeId| {
            r.merged.graphs.iter().find_map(|g| g.nav.kind_by_id.get(&id).copied()).map(node_kind::name)
        };
        let qname_of = |id: NodeId| r.merged.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&id).cloned());
        let by_id = |id: &str| rules.iter().find(|(_, x)| x.id == id).cloned();
        let mut ids: Vec<&str> = rules.iter().map(|(_, x)| x.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["component:api", "component:ext", "component:lib", "layer:core", "lib-uses-api"]);

        let (api_at, api) = by_id("component:api").unwrap();
        assert_eq!(api.kind, ConstraintKind::Component { name: "api".into(), paths: vec!["services/api".into()] });
        assert_eq!(qname_of(api_at).as_deref(), Some("project:services/api"));
        // No PROJECT at lib and no root PROJECT: the smallest-id MODULE under lib/.
        let (lib_at, _) = by_id("component:lib").unwrap();
        assert_eq!(kind_of(lib_at), Some("MODULE"));
        let modules: Vec<NodeId> = r
            .merged
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter().filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE)))
            .filter(|n| position_file(&n.cells).is_some_and(|f| is_under(&f, "lib")))
            .map(|n| n.id)
            .collect();
        assert_eq!(modules.len(), 2, "lib/util.py and lib/more.py");
        assert_eq!(lib_at.0, modules.iter().map(|m| m.0).min().unwrap());
        // The layer and the allow follow their first component (lib).
        assert_eq!(by_id("layer:core").unwrap().0, lib_at);
        assert_eq!(by_id("lib-uses-api").unwrap().0, lib_at);
        assert_eq!(
            by_id("layer:core").unwrap().1.kind,
            ConstraintKind::Layer { name: "core".into(), rank: 0, components: vec!["lib".into(), "api".into()], strict: true }
        );
        assert_eq!(by_id("lib-uses-api").unwrap().1.kind, ConstraintKind::Allow { from: "lib".into(), to: "api".into() });
        // The API entry's `./lib/` and label `api` come back as paths.
        assert_eq!(
            by_id("component:ext").unwrap().1.kind,
            ConstraintKind::Component { name: "ext".into(), paths: vec!["lib".into(), "services/api".into()] }
        );
        // The stored overlay entry keeps both the resolved and the raw path.
        let stored = r.merged.graphs.iter().flat_map(|g| &g.nodes).find(|n| n.id == api_at).unwrap();
        let payload = stored.cells.iter().find(|c| c.kind == cell_type::CONSTRAINT).unwrap();
        let CellPayload::Json(s) = &payload.payload else { panic!("CONSTRAINT is JSON") };
        assert!(s.contains(r#""paths":["services/api"],"paths_raw":["api"]"#), "{s}");
        assert!(!s.contains("component:dup"), "a path already claimed rejects the later component: {s}");
    }

    #[test]
    fn is_under_matches_whole_segments() {
        assert!(is_under("web/app.py", "web"));
        assert!(is_under("web", "web"));
        assert!(is_under("anything.py", "."));
        assert!(!is_under("webapp/app.py", "web"));
        assert!(!is_under("services/web/app.py", "web"));
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
