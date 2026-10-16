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
//! off. `glia check` re-reads it (DT.7b, [`declared_misses`]): it binds
//! every rule stanza again through the same [`stanzas`] + [`bind_stanza`]
//! and lists those that reached no CONSTRAINT cell, after the rule stanzas
//! the loader itself dropped and a whole file it ignored (DT.7c, the
//! loader's [`Rejection`](glia_code_domain::glia_config::Rejection)s).
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
//!    its path); an absent, `.` or empty scope is the repo root, `.`.
//!    - A constraint (DT.7a) binds the PROJECT of this repo at that path,
//!      else the deepest PROJECT of this repo above it (a root PROJECT
//!      encloses every path: `backend/internal/handlers` hangs on
//!      `project:backend`), else the smallest-id MODULE of this repo whose
//!      POSITION file lies under it (the CC.5a component fallback, for a repo
//!      with no root manifest; `.` takes any file); with none of the three it
//!      is orphaned. Only where the entry is STORED depends on this: the
//!      entry carries its scopes resolved, and `glia check` reads those,
//!      never the node.
//!    - A decision binds the PROJECT node of this repo whose path it is; a
//!      path no PROJECT sits at orphans it: declare a `[[project]]` (LF.3a)
//!      to give it one.
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
//! ENTRIES, upserted through `external_inputs::validate_declared_entry` +
//! `merge_entry` (source `overlay`), so they share one canonical array per
//! cell with the API sidecar's (`api`) and the ADR pass's (`adr`) entries,
//! keyed by `(source, id)`. This stage is the ONE caller of
//! `validate_declared_entry`, the check that admits a constraint's `rule`
//! (0.5.2 Q9: rule text is overlay-only; every cell-API path checks through
//! `validate_entry`, which refuses it):
//! - constraint -> CONSTRAINT
//!   `{"source":"overlay","id","kind","from"?,"to"?,"from_raw"?,"to_raw"?,"scope"?,"scope_raw"?,"categories"?,"text"?,"transitive"?,"via"?,"via_raw"?,"rule"?,"min_tier"?,"origin":"human|llm","decl"}`,
//!   every scope (a transitive forbid_edge's `via` facades included) stored
//!   RESOLVED to a path beside its raw value, so `glia check` (LE.8) never
//!   re-resolves a label. The version-2 keys (DB.1) are written only when the
//!   stanza sets them (`transitive` only as `true`), so a version-1 stanza's
//!   entry is byte-identical to 0.5.1's;
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
//! with ` anchor_enclosing=<e>` inside the parentheses only when a constraint
//! bound through a PROJECT above its scope, then ` anchor_module=<m>` only
//! when a constraint bound through a MODULE under its scope or an allow
//! through its component's MODULE, and a trailing ` rejected=<x>` only when an
//! entry failed its rules or met a cell that is not an entry array. Once per
//! repo whose file declares a model (a component, a layer or an allow):
//!   `[declared] model repo=<label> components=<c> layers=<l> allows=<a> anchored=<n> orphaned=<o>`
//! (+ ` rejected=<x>`), counting those three kinds. Then one detail line per
//! stanza that did not anchor, at most [`MAX_DETAIL_LINES`] per repo:
//!   `[declared] orphaned <section> id=<id> <decl> anchor=<qname>`,
//!   `[declared] orphaned constraint id=<id> <decl> scope=<raw> -> <path> (no PROJECT at or above it, no MODULE under it)`,
//!   `[declared] orphaned decision id=<id> <decl> scope=<raw> -> <path> (no PROJECT)`,
//!   `[declared] orphaned component id=<id> <decl> path=<raw> -> <path> (no PROJECT or MODULE)` or
//!   `[declared] orphaned <layer|constraint> id=<id> <decl> component=<name> (no root PROJECT; the component did not anchor)`.
//!
//! [`declared_constraints`] is the single reader of the rules: every
//! CONSTRAINT entry in the graph (overlay or API) as `(anchor, rule)` pairs.
//! After the stages ran, the build reports what it reads back:
//!   `[declared] rules=<r> (forbid_edge=<f> no_cycle=<c> invariant=<i>)`,
//! where `r` also counts the allows; component / layer entries are model
//! declarations, not rules, and are reported on the model line.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use glia_code_domain::external_inputs::{
    ConstraintKind, ConstraintRule, merge_entry, parse_constraints, validate_declared_entry,
};
use glia_code_domain::glia_config::{
    self, COMPONENT_ID_PREFIX, LAYER_ID_PREFIX, LoadedConfig, MODEL_KINDS, NOTE_ID_PREFIX, Origin,
};
use glia_code_domain::{cell_type, node_kind};
use glia_core::{Cell, CellPayload, CellTypeId, NodeId, RepoId};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex};
use serde_json::{Map, Value};

use super::RepoInputs;
use crate::answers::{ProjectInfo, project_roots, resolve_scope};
use crate::check::{ABSENT, ORPHANED, REJECTED};

/// Detail lines printed per repo before the rest are summarised.
const MAX_DETAIL_LINES: usize = 32;

/// How a stanza found its node.
#[derive(Debug, Clone, Copy)]
enum Via {
    Qname,
    /// The PROJECT at the scope (a component's: at its first path, else the
    /// repo root's).
    Project,
    /// A constraint's deepest PROJECT above its scope (DT.7a).
    Enclosing,
    /// The smallest-id MODULE under a component's first path (CC.5a) or a
    /// constraint's scope (DT.7a).
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
    anchor_enclosing: usize,
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
                Via::Enclosing => self.anchor_enclosing += 1,
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
    /// A decision's scope: the PROJECT at it, `None` the repo root's.
    Scope(Option<String>),
    /// A `[[constraint]]` (not an allow) without `anchor`: forbid_edge's
    /// `from`, no_cycle / invariant's `scope` (`None` is the repo root). The
    /// PROJECT at it, else the deepest PROJECT above it, else the smallest-id
    /// MODULE under it (DT.7a).
    RuleScope(Option<String>),
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
    /// `repo`'s resolvers: only its id is read, so check's re-bind (DT.7b)
    /// builds them from a `(repo id, root)` pair, as the build does from its
    /// [`RepoInputs`].
    fn build(merged: &MergedGraph, repo: RepoId) -> Self {
        let mut at: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
        for (gi, g) in merged.graphs.iter().enumerate() {
            if g.repo != repo {
                continue;
            }
            for (ni, n) in g.nodes.iter().enumerate() {
                at.entry(n.id).or_default().push((gi, ni));
            }
        }
        Anchors {
            idx: QnameIndex::build(merged, Some(repo)),
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

    /// The deepest PROJECT of this repo at or above `path`: among the build's
    /// PROJECT paths that bind in THIS repo (as [`Self::project_at`] binds),
    /// the one with the most segments that is `.` or `path` lies under. A
    /// PROJECT only another repo of a `--with` build declares never binds
    /// here; one repo has one PROJECT per path, so there is no tie (the
    /// smaller NodeId would win).
    fn enclosing_project(&self, path: &str) -> Option<NodeId> {
        let depth = |p: &str| if p == "." { 0 } else { p.split('/').count() };
        self.projects
            .iter()
            .filter(|p| p.path == "." || is_under(path, &p.path))
            .filter_map(|p| match self.idx.resolve(&p.qname, Some("PROJECT"), None) {
                CellTarget::Bound(id) | CellTarget::Ambiguous(id) => Some((depth(&p.path), id)),
                _ => None,
            })
            .max_by_key(|&(d, id)| (d, std::cmp::Reverse(id.0)))
            .map(|(_, id)| id)
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

    /// Where a `[[constraint]]` without `anchor` hangs, its scope resolved
    /// to `path` (see the module docs, ANCHOR rule 2).
    fn rule_anchor(&mut self, merged: &MergedGraph, path: &str) -> Option<(NodeId, Via)> {
        self.project_at(path)
            .map(|id| (id, Via::Project))
            .or_else(|| self.enclosing_project(path).map(|id| (id, Via::Enclosing)))
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
        // A transitive forbid_edge's facades resolve like its from / to.
        let (via, via_raw) = if d.via.is_empty() {
            (None, None)
        } else {
            let resolved: Vec<String> = d.via.iter().map(|raw| anchors.path_of(merged, raw)).collect();
            (strings(&resolved), strings(&d.via))
        };
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
            ("transitive", d.transitive.then_some(Value::Bool(true))),
            ("via", via),
            ("via_raw", via_raw),
            ("rule", text(&d.rule)),
            ("min_tier", text(&d.min_tier)),
            ("origin", s(origin_name(d.origin))),
            ("decl", s(&decl)),
        ]);
        let scope = if d.kind == "forbid_edge" { d.from.clone() } else { d.scope.clone() };
        out.push(Stanza {
            section: "constraint",
            cell: cell_type::CONSTRAINT,
            id: d.id.clone(),
            decl,
            target: d.anchor.clone().map_or(Target::RuleScope(scope), Target::Qname),
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

/// Why a stanza found no node: the build's `[declared] orphaned` detail
/// ([`Self::detail`]) and `glia check`'s reason ([`Self::reason`], DT.7b).
enum Unbound {
    /// `anchor = <qname>` binds nothing in this repo.
    Anchor(String),
    /// A decision's scope: no PROJECT of this repo at the path.
    DecisionScope { raw: String, path: String },
    /// A constraint's scope: no PROJECT at or above it, no MODULE under it.
    RuleScope { raw: String, path: String },
    /// A component's first path: no PROJECT there or at the root, no MODULE
    /// under it.
    ComponentPath { raw: String, path: String },
    /// A layer, or an allow without `anchor`: no root PROJECT, and its
    /// component did not anchor.
    Model { component: String },
}

impl Unbound {
    /// The `[declared] orphaned <section> id=<id> <decl> <detail>` tail.
    fn detail(&self) -> String {
        match self {
            Unbound::Anchor(q) => format!("anchor={q}"),
            Unbound::DecisionScope { raw, path } => format!("scope={raw} -> {path} (no PROJECT)"),
            Unbound::RuleScope { raw, path } => {
                format!("scope={raw} -> {path} (no PROJECT at or above it, no MODULE under it)")
            }
            Unbound::ComponentPath { raw, path } => format!("path={raw} -> {path} (no PROJECT or MODULE)"),
            Unbound::Model { component } => {
                format!("component={component} (no root PROJECT; the component did not anchor)")
            }
        }
    }

    /// `glia check`'s reason: `orphaned: <detail>`, an anchor's with ` binds
    /// no node`.
    fn reason(&self) -> String {
        match self {
            Unbound::Anchor(q) => format!("orphaned: anchor={q} binds no node"),
            other => format!("orphaned: {}", other.detail()),
        }
    }
}

/// The node `st` hangs on (module docs "ANCHOR" and "THE MODEL'S ANCHORS"),
/// or why it hangs nowhere. `components` holds where each component already
/// anchored (a layer's / an allow's fallback). The build's
/// [`apply_declared_cells`] and check's [`declared_misses`] both bind
/// through it, so check's verdict is the build's.
fn bind_stanza(
    st: &Stanza,
    merged: &MergedGraph,
    anchors: &mut Anchors,
    components: &BTreeMap<String, (NodeId, Via)>,
) -> Result<(NodeId, Via), Unbound> {
    match &st.target {
        Target::Qname(q) => anchors.by_qname(q).map(|id| (id, Via::Qname)).ok_or_else(|| Unbound::Anchor(q.clone())),
        Target::Scope(scope) => {
            let raw = scope.clone().unwrap_or_default();
            let path = anchors.path_of(merged, &raw);
            match anchors.project_at(&path) {
                Some(id) => Ok((id, Via::Project)),
                None => Err(Unbound::DecisionScope { raw, path }),
            }
        }
        Target::RuleScope(scope) => {
            let raw = scope.clone().unwrap_or_default();
            let path = anchors.path_of(merged, &raw);
            match anchors.rule_anchor(merged, &path) {
                Some(bound) => Ok(bound),
                None => Err(Unbound::RuleScope { raw, path }),
            }
        }
        Target::Component { raw, path } => anchors
            .component_anchor(merged, path)
            .ok_or_else(|| Unbound::ComponentPath { raw: raw.clone(), path: path.clone() }),
        Target::Model { component } => anchors
            .project_at(".")
            .map(|id| (id, Via::Project))
            .or_else(|| components.get(component).copied())
            .ok_or_else(|| Unbound::Model { component: component.clone() }),
    }
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
    let mut anchors = Anchors::build(merged, input.repo);
    // component name -> where it anchored: a layer's / an allow's fallback.
    let mut components: BTreeMap<String, (NodeId, Via)> = BTreeMap::new();
    let mut wrote = false;
    for st in stanzas(cfg, merged, &mut anchors) {
        let what = format!("{} id={} {}", st.section, st.id, st.decl);
        if let Some(e) = &st.reject {
            t.rejected(&st, format!("rejected {what}: {e}"));
            continue;
        }
        if let Err(e) = validate_declared_entry(st.cell, &st.entry) {
            t.rejected(&st, format!("rejected {what}: {e}"));
            continue;
        }
        let (id, via) = match bind_stanza(&st, merged, &mut anchors, &components) {
            Ok(b) => b,
            Err(unbound) => {
                t.orphaned(&st, format!("orphaned {what} {}", unbound.detail()));
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
            "[declared] repo={label} constraint={} decision={} note={} anchored={} orphaned={} (anchor_qname={} anchor_project={}{}{}){}",
            t.constraint,
            t.decision,
            t.note,
            t.anchor_qname + t.anchor_project + t.anchor_enclosing + t.anchor_module,
            t.orphaned,
            t.anchor_qname,
            t.anchor_project,
            nonzero("anchor_enclosing", t.anchor_enclosing),
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
/// which leaves a path unchanged - a forbid_edge's `from` / `to` and `via`
/// facades (DB.1), a no_cycle's `scope` and a component's `paths` (CC.5a; an
/// allow's `from` / `to` are component names and stay as stored). One id in
/// several language graphs is read once.
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
            ConstraintKind::ForbidEdge { from, to, via, .. } => {
                path(from);
                path(to);
                for p in via {
                    path(p);
                }
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

/// One declared rule stanza of a repo's `.glia/overlay.toml` that reached
/// no CONSTRAINT cell of the graph (DT.7b): `glia check` lists it among the
/// rules that check nothing ([`crate::check::Unanchored`]).
#[derive(Debug, Clone)]
pub(crate) struct DeclaredMiss {
    /// The root of the repo whose overlay declares it, as the roots list
    /// gave it.
    pub(crate) root: String,
    /// The entry id (`component:<name>` / `layer:<name>` for the model's).
    pub(crate) id: String,
    /// `constraint` (an allow included), `component` or `layer`; `overlay`
    /// for a whole file the loader ignored (DT.7c).
    pub(crate) section: &'static str,
    /// `.glia/overlay.toml:<line>` of its header (the whole file: of its
    /// error, or `.glia/overlay.toml` when the error names no line).
    pub(crate) decl: String,
    /// [`ORPHANED`], [`REJECTED`] or [`ABSENT`].
    pub(crate) status: &'static str,
    /// `orphaned: <where>`, `rejected: <why>` (the loader's message for a
    /// stanza it dropped; for a whole file, its message and the rule
    /// headers it did not read) or [`ABSENT_REASON`].
    pub(crate) reason: String,
    /// What would make it check something.
    pub(crate) fix: String,
}

/// The reason of a stanza that binds but whose rule the graph does not hold.
pub(crate) const ABSENT_REASON: &str = "absent: declared in the overlay but not in this graph \
     (rebuild; or the build refused to merge it, see its [declared] rejected= line)";

const ABSENT_FIX: &str =
    "rebuild the graph (`glia build <repo>`); if the row stays, the build's `[declared] rejected` line says why";
const CLAIM_FIX: &str =
    "give each path to one component (a project label and its directory are one path)";
const VALIDATOR_FIX: &str = "correct the stanza as the message says, then rebuild";

/// Every declared rule stanza of each repo's overlay that reached no
/// CONSTRAINT cell of `merged`, in `roots` order. `roots` are `(RepoId.0,
/// repo root)` pairs (a build's `GenerateResult::repo_roots`). Each root's
/// `.glia/overlay.toml` is read again, and its rows come in two runs:
///
/// 1. The loader's (DT.7c), [`REJECTED`]: each `[[constraint]]` /
///    `[[component]]` / `[[layer]]` stanza `glia_config` dropped (a
///    version-2 key in a version-1 file, an exact path two components claim,
///    a layer or allow naming a dropped component, a bad kind, ..), in the
///    loader's order (components, layers, constraints, each in file order):
///    id the stanza's (`<section>@<line>` when it has none), reason
///    `rejected: <loader message>`, fix `Rejection::fix`. Or, when the loader
///    ignored the WHOLE file (a TOML or schema error, an unsupported or
///    missing `version`, unreadable bytes) and its text holds a rule header
///    ([`rule_headers`]) or could not be read again, one row: id
///    `.glia/overlay.toml`, section `overlay`, reason `rejected: <message>
///    (rule stanzas not read: constraint=<c> component=<m> layer=<l>)`. A
///    file with no rule header (only `[[edge]]` / `[[wrapper]]` ..) gives no
///    row: its `[overlay] error:` line and `glia gaps` report it. A dropped
///    decision or note is a record, not a rule: not listed.
/// 2. The binder's (DT.7b): the kept stanzas bound against `merged` through
///    the build's own [`stanzas`] + [`bind_stanza`], in the build's stanza
///    order, so a miss is the build's verdict: a `reject` (a path two
///    components claim through a label and its directory) or a
///    `validate_declared_entry` refusal is [`REJECTED`], a stanza that binds
///    no node [`ORPHANED`], and one that binds but whose `(repo, overlay,
///    id)` rule the graph does not hold [`ABSENT`] (a layout older than the
///    overlay, or an entry the build could not merge). A stanza the loader
///    dropped never reaches [`stanzas`], so it is never a row twice.
///
/// Decisions and notes are records, not rules: never listed. Writes no cell
/// and prints no `[declared]` line (resolving a scope label may print its
/// `[scope]` line again). fired_on, once per root with a loader row, before
/// the binder runs:
/// `[check] overlay rejected root=<root> whole_file=<true|false> constraint=<c> component=<m> layer=<l>`
/// (`<root>` as `roots` gives it; the counts the rows per section, or the
/// ignored file's rule headers) - grep `^\[check\] overlay rejected`.
pub(crate) fn declared_misses(merged: &MergedGraph, roots: &[(u64, PathBuf)]) -> Vec<DeclaredMiss> {
    let mut out = Vec::new();
    let mut held_rules: Option<BTreeSet<(u64, String)>> = None;
    for (repo, root) in roots {
        let Some(cfg) = glia_config::load(root) else { continue };
        out.extend(loader_misses(root, &cfg));
        let c = &cfg.config;
        if c.constraint.is_empty() && c.component.is_empty() && c.layer.is_empty() {
            continue;
        }
        let held = held_rules.get_or_insert_with(|| overlay_rules_held(merged));
        let mut anchors = Anchors::build(merged, RepoId(*repo));
        let all = stanzas(&cfg, merged, &mut anchors);
        let declared: BTreeSet<&str> = all.iter().filter_map(|st| st.component.as_deref()).collect();
        let mut components: BTreeMap<String, (NodeId, Via)> = BTreeMap::new();
        for st in &all {
            if st.cell != cell_type::CONSTRAINT {
                continue;
            }
            let (status, reason, fix) = if let Some(e) = &st.reject {
                (REJECTED, format!("rejected: {e}"), CLAIM_FIX.to_string())
            } else if let Err(e) = validate_declared_entry(st.cell, &st.entry) {
                (REJECTED, format!("rejected: {e}"), VALIDATOR_FIX.to_string())
            } else {
                match bind_stanza(st, merged, &mut anchors, &components) {
                    Err(unbound) => {
                        let fix = fix_for(st, &unbound, merged, &mut anchors, &declared);
                        (ORPHANED, unbound.reason(), fix)
                    }
                    Ok(bound) => {
                        if let Some(name) = &st.component {
                            components.insert(name.clone(), bound);
                        }
                        if held.contains(&(*repo, st.id.clone())) {
                            continue;
                        }
                        (ABSENT, ABSENT_REASON.to_string(), ABSENT_FIX.to_string())
                    }
                }
            };
            out.push(DeclaredMiss {
                root: root.to_string_lossy().into_owned(),
                id: st.id.clone(),
                section: st.section,
                decl: st.decl.clone(),
                status,
                reason,
                fix,
            });
        }
    }
    out
}

/// The sections whose dropped stanzas `glia check` lists: the rules and
/// the reflexion model's declarations (a decision or note is a record).
const RULE_SECTIONS: [&str; 3] = ["constraint", "component", "layer"];

/// The loader's rows of one root ([`declared_misses`] run 1), and the
/// `[check] overlay rejected` marker when there is one.
fn loader_misses(root: &Path, cfg: &LoadedConfig) -> Vec<DeclaredMiss> {
    let root_label = root.to_string_lossy().into_owned();
    let decl_at = |line: Option<u32>| match line {
        Some(l) => format!("{}:{l}", glia_config::OVERLAY_FILE),
        None => glia_config::OVERLAY_FILE.to_string(),
    };
    let mut out = Vec::new();
    let mut counts = [0usize; 3];
    let mut whole_file = false;
    for r in &cfg.rejections {
        match r.section {
            Some(section) => {
                let Some(i) = RULE_SECTIONS.iter().position(|s| *s == section) else {
                    continue;
                };
                counts[i] += 1;
                let line = r.line.map_or_else(|| "?".to_string(), |l| l.to_string());
                out.push(DeclaredMiss {
                    root: root_label.clone(),
                    id: r.id.clone().unwrap_or_else(|| format!("{section}@{line}")),
                    section: RULE_SECTIONS[i],
                    decl: decl_at(r.line),
                    status: REJECTED,
                    reason: format!("rejected: {}", r.message),
                    fix: r.fix(),
                });
            }
            None => {
                let reason = match std::fs::read(root.join(glia_config::OVERLAY_FILE)) {
                    Ok(bytes) => {
                        counts = rule_headers(&String::from_utf8_lossy(&bytes));
                        if counts.iter().all(|n| *n == 0) {
                            continue;
                        }
                        format!(
                            "rejected: {} (rule stanzas not read: constraint={} component={} layer={})",
                            r.message, counts[0], counts[1], counts[2]
                        )
                    }
                    Err(_) => format!("rejected: {} (no stanza of it is read)", r.message),
                };
                whole_file = true;
                out.push(DeclaredMiss {
                    root: root_label.clone(),
                    id: glia_config::OVERLAY_FILE.to_string(),
                    section: "overlay",
                    decl: decl_at(r.line),
                    status: REJECTED,
                    reason,
                    fix: r.fix(),
                });
            }
        }
    }
    if !out.is_empty() {
        eprintln!(
            "[check] overlay rejected root={root_label} whole_file={whole_file} constraint={} component={} layer={}",
            counts[0], counts[1], counts[2]
        );
    }
    out
}

/// The `[[constraint]]`, `[[component]]` and `[[layer]]` table headers of an
/// overlay's text, counted per section in that order: a line whose trimmed
/// text is `[[`, optional spaces, the section name, optional spaces, `]]`,
/// then nothing or a `# comment`.
fn rule_headers(text: &str) -> [usize; 3] {
    let mut out = [0; 3];
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("[[") else {
            continue;
        };
        let rest = rest.trim_start();
        for (i, section) in RULE_SECTIONS.iter().enumerate() {
            let Some(tail) = rest.strip_prefix(section) else {
                continue;
            };
            let Some(after) = tail.trim_start().strip_prefix("]]") else {
                continue;
            };
            let after = after.trim_start();
            if after.is_empty() || after.starts_with('#') {
                out[i] += 1;
            }
        }
    }
    out
}

/// `(repo, id)` of every overlay CONSTRAINT entry `merged` holds, read as
/// [`declared_constraints`] reads them (an entry it cannot read is not held).
fn overlay_rules_held(merged: &MergedGraph) -> BTreeSet<(u64, String)> {
    let mut out = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(cell) = n.cells.iter().find(|c| c.kind == cell_type::CONSTRAINT) else {
                continue;
            };
            for r in parse_constraints(&cell.payload) {
                if r.source == "overlay" {
                    out.insert((g.repo.0, r.id));
                }
            }
        }
    }
    out
}

/// The proper ancestors of a repo-relative path, deepest first, `.` last
/// (none for `.`).
fn ancestors(path: &str) -> Vec<String> {
    if path == "." {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut p = path;
    while let Some((head, _)) = p.rsplit_once('/') {
        out.push(head.to_string());
        p = head;
    }
    out.push(".".to_string());
    out
}

/// The `::`-prefixes of a qname, longest first.
fn qname_ancestors(qname: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut q = qname.trim();
    while let Some((head, _)) = q.rsplit_once("::") {
        out.push(head);
        q = head;
    }
    out
}

/// What places `st` once its `anchor` is dropped.
fn placement(st: &Stanza) -> &'static str {
    match st.section {
        "component" => "its first path",
        _ if st.model => "its `from` component",
        _ if st.entry.get("kind").and_then(Value::as_str) == Some("forbid_edge") => "its `from` scope",
        _ => "its `scope`",
    }
}

/// A suggested fix for `st`, orphaned as `unbound`: the nearest qname or
/// path that would anchor it, else what to declare. `declared` names the
/// overlay's components.
fn fix_for(
    st: &Stanza,
    unbound: &Unbound,
    merged: &MergedGraph,
    anchors: &mut Anchors,
    declared: &BTreeSet<&str>,
) -> String {
    match unbound {
        Unbound::Anchor(q) => {
            let place = placement(st);
            match qname_ancestors(q).into_iter().find(|p| anchors.by_qname(p).is_some()) {
                Some(p) => format!(
                    "drop `anchor` to place it by {place}, or set anchor = \"{p}\" (the nearest qname above it that this graph holds)"
                ),
                None => format!(
                    "drop `anchor` to place it by {place}, or set it to a qname this graph holds (`glia find <name>` lists them)"
                ),
            }
        }
        Unbound::RuleScope { path, .. } => {
            let forbid = st.entry.get("kind").and_then(Value::as_str) == Some("forbid_edge");
            let field = if forbid { "from" } else { "scope" };
            match ancestors(path).into_iter().find(|a| anchors.rule_anchor(merged, a).is_some()) {
                Some(a) if a != "." => format!(
                    "check the spelling of `{field}` (the nearest path above it that anchors a rule is `{a}`), or declare a [[project]] at `{path}`"
                ),
                Some(_) => format!(
                    "check the spelling of `{field}` (no directory above it holds code; `glia projects` lists the project paths), or declare a [[project]] at `{path}`"
                ),
                None => format!(
                    "this repo has no PROJECT and no located MODULE to hang a rule on: declare a [[project]] at `{path}`"
                ),
            }
        }
        Unbound::ComponentPath { path, .. } => {
            let near = ancestors(path)
                .into_iter()
                .find(|a| anchors.project_at(a).is_some() || anchors.module_under(merged, a).is_some());
            match near {
                Some(a) if a != "." => format!(
                    "check the spelling of the component's first path (the nearest path above it that holds code is `{a}`), or declare a [[project]] at `{path}`"
                ),
                Some(_) => format!(
                    "check the spelling of the component's first path (no directory above it holds code; `glia projects` lists the project paths), or declare a [[project]] at `{path}`"
                ),
                None => format!(
                    "this repo has no PROJECT and no located MODULE to hang a component on: declare a [[project]] at `{path}`"
                ),
            }
        }
        Unbound::DecisionScope { path, .. } => {
            format!("declare a [[project]] at `{path}`, or set `anchor`")
        }
        Unbound::Model { component } if !declared.contains(component.as_str()) => {
            format!("declare a [[component]] named `{component}`, or name a declared component first")
        }
        Unbound::Model { component } => {
            let anchor_too = if st.section == "constraint" { ", or set `anchor` on this stanza" } else { "" };
            format!(
                "anchor component `{component}` first (its own row says why), or give the repo a root PROJECT (a root manifest, or a [[project]] at `.`){anchor_too}"
            )
        }
    }
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
        assert_eq!(
            by_id("w"),
            ConstraintKind::ForbidEdge { from: "web".into(), to: "services/api".into(), transitive: false, via: vec![] }
        );
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

    /// DB.1: an API rule's `via` facades come back resolved like its from /
    /// to (a decorated path normalised, a project label as its path); an
    /// overlay rule's were stored resolved beside `via_raw`.
    #[test]
    fn via_resolves_like_from_and_to() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let overlay = "version = 2

[[constraint]]
id = \"no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"api\"
transitive = true
via = [\"./services/api/public/\"]
";
        for (path, body) in [
            ("web/package.json", "{\"name\": \"web\"}\n"),
            ("web/src/ui.ts", "export function render() { return 1; }\n"),
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
            ("services/api/app.py", "def charge(order_id):\n    return order_id\n"),
            (".glia/overlay.toml", overlay),
            (
                ".glia/cells.jsonl",
                "{\"qname\":\"project:web\",\"cell\":\"CONSTRAINT\",\"entry\":{\"source\":\"api\",\"id\":\"w\",\"kind\":\"forbid_edge\",\"from\":\"./web\",\"to\":\"api\",\"transitive\":true,\"via\":[\"./services/api/public/\",\"api\"]}}\n",
            ),
        ] {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let r = crate::generate_one(root.to_str().unwrap()).unwrap();
        let rules = declared_constraints(&r.merged);
        let by_id = |id: &str| rules.iter().find(|(_, x)| x.id == id).map(|(_, x)| x.kind.clone()).unwrap();
        assert_eq!(
            by_id("w"),
            ConstraintKind::ForbidEdge {
                from: "web".into(),
                to: "services/api".into(),
                transitive: true,
                via: vec!["services/api/public".into(), "services/api".into()],
            }
        );
        assert_eq!(
            by_id("no-api"),
            ConstraintKind::ForbidEdge {
                from: "web".into(),
                to: "services/api".into(),
                transitive: true,
                via: vec!["services/api/public".into()],
            }
        );
        let web = rules.iter().find(|(_, x)| x.id == "no-api").map(|(id, _)| *id).unwrap();
        let stored = r.merged.graphs.iter().flat_map(|g| &g.nodes).find(|n| n.id == web).unwrap();
        let payload = stored.cells.iter().find(|c| c.kind == cell_type::CONSTRAINT).unwrap();
        let CellPayload::Json(s) = &payload.payload else { panic!("CONSTRAINT is JSON") };
        assert!(
            s.contains(r#""transitive":true,"via":["services/api/public"],"via_raw":["./services/api/public/"]"#),
            "{s}"
        );
    }

    /// DT.7a: the enclosing PROJECT is the deepest of THIS repo at or above
    /// the path (web/admin over web over .), and a deeper PROJECT that only
    /// another repo of the build declares never binds.
    #[test]
    fn enclosing_project_picks_the_deepest_of_this_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        for (root, path, body) in [
            (&a, "pyproject.toml", "[project]\nname = \"shop\"\n"),
            (&a, "web/pyproject.toml", "[project]\nname = \"web\"\n"),
            (&a, "web/app.py", "def show():\n    return 1\n"),
            (&a, "web/admin/pyproject.toml", "[project]\nname = \"admin\"\n"),
            (&a, "web/admin/panel.py", "def panel():\n    return 2\n"),
            (&b, "web/admin/panel/pyproject.toml", "[project]\nname = \"panel\"\n"),
            (&b, "web/admin/panel/x.py", "def x():\n    return 3\n"),
        ] {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let paths = [a.to_str().unwrap().to_string(), b.to_str().unwrap().to_string()];
        let r = crate::generate_many(&paths).unwrap();
        let repo_with = |qname: &str| {
            r.merged
                .graphs
                .iter()
                .find(|g| g.nav.qname_by_id.values().any(|q| q == qname))
                .map(|g| g.repo)
                .unwrap()
        };
        let anchors_of = |repo| Anchors::build(&r.merged, repo);
        let (repo_a, repo_b) = (repo_with("project:web/admin"), repo_with("project:web/admin/panel"));
        assert_ne!(repo_a, repo_b);
        let qname_in = |repo, id: Option<NodeId>| {
            let id = id?;
            r.merged.graphs.iter().filter(|g| g.repo == repo).find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        };
        let in_a = anchors_of(repo_a);
        // Every PROJECT of the build is in the vocabulary, b's included.
        assert!(in_a.projects.iter().any(|p| p.path == "web/admin/panel"));
        for (path, want) in [
            ("web/admin/panel/views", "project:web/admin"),
            ("web/admin", "project:web/admin"),
            ("web/src", "project:web"),
            ("webapp", "project:."),
            (".", "project:."),
        ] {
            assert_eq!(qname_in(repo_a, in_a.enclosing_project(path)).as_deref(), Some(want), "{path}");
        }
        let in_b = anchors_of(repo_b);
        assert_eq!(
            qname_in(repo_b, in_b.enclosing_project("web/admin/panel/x.py")).as_deref(),
            Some("project:web/admin/panel")
        );
        // b has no root manifest, and a's projects do not bind in b.
        assert_eq!(in_b.enclosing_project("web/admin"), None);
        assert_eq!(in_b.enclosing_project("."), None);
    }

    #[test]
    fn ancestors_walk_up_to_the_root() {
        assert_eq!(ancestors("backend/internal/gone"), ["backend/internal", "backend", "."]);
        assert_eq!(ancestors("web"), ["."]);
        assert!(ancestors(".").is_empty());
        assert_eq!(qname_ancestors("backend::internal::gone::Thing"), ["backend::internal::gone", "backend::internal", "backend"]);
        assert!(qname_ancestors("Thing").is_empty());
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

    #[test]
    fn rule_headers_counts_rule_sections() {
        let text = "\u{feff}[[component]]\nname = \"a\"\n  [[ constraint ]] # x\n[[constraint]]\n[[layer]]#top\n[[edge]]\n[constraint]\n[[constraints]]\n[[constraint]] x\n# [[layer]]\n";
        assert_eq!(rule_headers(text), [2, 1, 1]);
        assert_eq!(rule_headers("version = 1\n[[edge]]\nfrom = \"a\"\n"), [0, 0, 0]);
    }
}
