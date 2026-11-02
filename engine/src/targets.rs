//! Build-target graphs (0.5.7, HT.6): the targets and their dependencies read
//! from the targets snapshot (`.glia/targets-snapshot/`, written from `bazel
//! query` output by the snapshot step, HT.3), kept as a layout-dir sidecar,
//! and the answers over them, joined to the code graph's files. Reads a
//! snapshot only: it never runs bazel and never refreshes the snapshot.
//!
//! The sidecar is `<repo>/.glia/graph/targets.gmap` (the layout dir, beside
//! its self-ignoring `.gitignore`, as `timeline.gmap` is): the snapshot's
//! rows parsed by `glia_parser_bazel` and run through
//! [`BUILD_PROFILE`]'s passes (ROLE cells, the determinism sort), its
//! [`LABELS_SECTION`](glia_build_domain::LABELS_SECTION) metadata holding
//! the snapshot's `data_hash` and the writing glia's
//! [`BUILD_STAMP`]. [`load_or_build`] reuses it when
//! both match and rebuilds it otherwise. Only an answer call writes it:
//! `glia build` and `persist` never do, and persist's orphan sweep removes
//! shard names only, so it leaves the sidecar alone. The graph's node ids
//! hash the fixed repo [`targets_repo`], so two checkouts of one commit write
//! the same bytes.
//!
//! The answers, every row tier `fact` (read off the build system's own query
//! output), ordered by (depth, workspace, label), never by a node id:
//! - [`TargetsView::owners`]: the targets whose `srcs` / `hdrs` /
//!   `textual_hdrs` name a repo file (HAS_SRC predecessors of its SOURCE);
//! - [`TargetsView::deps`] / [`TargetsView::rdeps`]: what a target reaches /
//!   what reaches it over the carry edges (DEPENDS_ON, RUNTIME_DEP, HAS_SRC),
//!   each row with the attributes (`via`) of the edge that reached it; an
//!   unknown label is a fact absence;
//! - [`TargetsView::affected`] / [`affected_vs_rev`]: the targets (and test
//!   targets) a set of changed files reaches backwards, all depths, with a
//!   caveat when a BUILD / `.bzl` / MODULE / WORKSPACE file changed;
//! - [`TargetsView::code_modules`]: a target's sources joined to the code
//!   MODULEs located at the same repo path.
//!
//! A SOURCE is a repo file only when it is a main-workspace label (`//pkg:f`)
//! whose LOCATION is the file the label names (`<workspace>/<pkg>/<f>`): an
//! external label joins nothing, and a generated file (LOCATION its BUILD
//! line) is never mistaken for one.
//!
//! fired_on markers (stderr):
//! - `[targets] sidecar built|reused nodes=<n> edges=<e> data_hash=<h> path=<p>`
//!   once per [`load_or_build`] that found a snapshot;
//! - `[targets] answer kind=<owners|deps|rdeps|affected|code_modules> rows=<r>
//!   tests=<t>` once per answer;
//! - `[absence] primitive=targets_<deps|rdeps> reason=<r> ..` per absence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use glia_activation::algo::reach::bfs;
use glia_activation::algo::{Adjacency, Walk};
use glia_activation::profile::Tier;
use glia_build_domain::cell_type::{ATTRS, LOCATION};
use glia_build_domain::edge_category::{self as bec, HAS_SRC};
use glia_build_domain::node_kind::{self as bnk, SOURCE, TARGET};
use glia_build_domain::{BUILD_PROFILE, BUILD_TABLES, BuildGraph, GRAPH_TYPE, read_targets};
use glia_code_domain::node_kind;
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_graph::MergedGraph;
use glia_stamp::BUILD_STAMP;

use crate::absence::{Absence, Answer};
use crate::answers::{Located, Locator};
use crate::git_rev::{changed_files, resolve_rev};
use crate::persist::{default_layout_dir, write_self_ignore};

/// The sidecar's file name in the layout dir.
pub const TARGETS_GMAP: &str = "targets.gmap";

/// The metadata key holding the snapshot's `data_hash`.
pub const META_DATA_HASH: &str = "data_hash";

/// The metadata key holding the writing glia's `BUILD_STAMP`.
pub const META_BUILD_STAMP: &str = "build_stamp";

/// The caveat [`TargetsView::affected`] adds when a changed file is one bazel
/// reads to define targets: the snapshot predates the change.
pub const BUILD_FILES_CAVEAT: &str =
    "BUILD files changed; re-run glia targets sync for exact owners";

/// The repo every build node id hashes: fixed, so the sidecar does not
/// depend on where the repo is checked out.
pub fn targets_repo() -> RepoId {
    RepoId::from_canonical(GRAPH_TYPE)
}

/// `<root>/.glia/graph/targets.gmap`.
pub fn sidecar_path(root: &Path) -> PathBuf {
    default_layout_dir(root).join(TARGETS_GMAP)
}

/// One row of a targets answer: a target (or, in `deps`, a source file or
/// external label it reaches), where it is declared, and how it was reached.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TargetHit {
    /// The bazel workspace the label is relative to (a repo-relative dir,
    /// `""` for the repo root).
    pub workspace: String,
    /// The label as bazel prints it (`//main:hello-world`).
    pub label: String,
    /// `TARGET`, `SOURCE`, `EXTERNAL` or `PACKAGE` (the build registry name).
    pub kind: &'static str,
    /// The rule class (`cc_binary`), targets only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_class: Option<String>,
    /// `test`, `binary`, `library` or `alias`, targets only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The attributes that declared the edge that reached this row
    /// (`deps`, `embed`, `srcs`, comma-joined); empty for none.
    pub via: String,
    /// Hops from the seed (an owner is 1 from its file).
    pub depth: usize,
    /// Where it is declared: a target's BUILD file, a source file itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The 1-based line of `file`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Always `fact`: read off the build system's query output.
    pub tier: &'static str,
}

/// What a set of changed files affects.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Affected {
    /// The changed files, sorted and deduplicated.
    pub changed: Vec<String>,
    /// The targets naming a changed file in `srcs` / `hdrs` / `textual_hdrs`.
    pub owners: Vec<TargetHit>,
    /// Every target that reaches a changed file over the carry edges, the
    /// owners included (depth 1), by (depth, workspace, label).
    pub targets: Vec<TargetHit>,
    /// The `targets` whose ROLE is `test`.
    pub tests: Vec<TargetHit>,
    /// Changed files no SOURCE of the snapshot is (new files, files no
    /// target names, files outside every workspace).
    pub unowned: Vec<String>,
    /// [`BUILD_FILES_CAVEAT`] when a BUILD / `.bzl` / MODULE / WORKSPACE file
    /// changed; empty otherwise.
    pub caveats: Vec<String>,
}

/// A loaded build-target graph and its indexes.
#[non_exhaustive]
pub struct TargetsView {
    /// The graph, as the sidecar holds it.
    pub graph: BuildGraph,
    /// The sidecar's path.
    pub path: PathBuf,
    /// The snapshot's `data_hash` the sidecar was built from.
    pub data_hash: String,
    /// Whether [`load_or_build`] reused the sidecar (false: it rebuilt it).
    pub reused: bool,
    /// The snapshot's row count and workspaces, for an absence's note.
    rows: usize,
    workspaces: Vec<String>,
    /// The carry edges (DEPENDS_ON, RUNTIME_DEP, HAS_SRC).
    carry: Adjacency,
    /// Repo-relative path -> the SOURCE nodes that are that file.
    files: BTreeMap<String, Vec<NodeId>>,
}

/// The build-target graph of `root`'s targets snapshot, from the sidecar when
/// it is current, else parsed from the snapshot and written to the sidecar.
/// `Ok(None)` when the repo holds no complete snapshot (`read_targets` says
/// why on stderr for an incomplete one). Errors only when a rebuilt sidecar
/// cannot be written.
pub fn load_or_build(root: &Path) -> Result<Option<TargetsView>, String> {
    let Some(snapshot) = read_targets(root) else {
        return Ok(None);
    };
    let data_hash = snapshot.meta.data_hash.clone();
    let path = sidecar_path(root);
    let current = glia_build_domain::read_gmap(&path)
        .ok()
        .map(|back| back.graph)
        .filter(|g| {
            g.meta.get(META_DATA_HASH) == Some(&data_hash)
                && g.meta.get(META_BUILD_STAMP).map(String::as_str) == Some(BUILD_STAMP)
        });
    let (graph, reused) = match current {
        Some(g) => (g, true),
        None => {
            let parsed = glia_parser_bazel::parse(targets_repo(), &snapshot.rows);
            let mut g = parsed.graph;
            BUILD_PROFILE.run_passes(&mut g, &());
            g.meta.insert(META_DATA_HASH.to_string(), data_hash.clone());
            g.meta
                .insert(META_BUILD_STAMP.to_string(), BUILD_STAMP.to_string());
            let dir = default_layout_dir(root);
            std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
            write_self_ignore(&dir)
                .map_err(|e| format!("write {}/.gitignore: {e}", dir.display()))?;
            g.write_gmap(&path)?;
            (g, false)
        }
    };
    eprintln!(
        "[targets] sidecar {} nodes={} edges={} data_hash={data_hash} path={}",
        if reused { "reused" } else { "built" },
        graph.nodes.len(),
        graph.edges.len(),
        path.display()
    );
    let workspaces = snapshot
        .meta
        .workspaces
        .iter()
        .map(|w| w.workspace.clone())
        .collect();
    Ok(Some(TargetsView::new(
        graph,
        path,
        data_hash,
        reused,
        snapshot.meta.rows,
        workspaces,
    )))
}

/// What `root`'s changes since `base` (the working tree against the rev,
/// untracked files included) affect: `Ok(None)` without a targets snapshot.
pub fn affected_vs_rev(root: &Path, base: &str) -> Result<Option<Affected>, String> {
    let Some(view) = load_or_build(root)? else {
        return Ok(None);
    };
    let rev = resolve_rev(root, base)?;
    let changed = changed_files(root, &rev)?;
    Ok(Some(view.affected(&changed)))
}

/// `//pkg:name` -> (`pkg`, `name`); `//pkg` is `//pkg:<last segment>`.
/// `None` for an external (`@..`) or relative label.
fn split_main(label: &str) -> Option<(&str, &str)> {
    let rest = label.strip_prefix("//")?;
    Some(match rest.split_once(':') {
        Some((pkg, name)) => (pkg, name),
        None => (rest, rest.rsplit('/').next().unwrap_or(rest)),
    })
}

/// A label in the full form bazel prints (`//pkg` -> `//pkg:pkg`).
fn full_label(label: &str) -> String {
    match split_main(label) {
        Some((pkg, name)) if !label.contains(':') => format!("//{pkg}:{name}"),
        _ => label.to_string(),
    }
}

/// The repo-relative file a main-workspace label names.
fn label_path(workspace: &str, label: &str) -> Option<String> {
    let (pkg, name) = split_main(label)?;
    Some(
        [workspace, pkg, name]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// A file bazel reads to define targets.
fn is_build_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "BUILD"
        || name == "BUILD.bazel"
        || name == "MODULE.bazel"
        || name.ends_with(".bzl")
        || name.starts_with("WORKSPACE")
}

fn json_of(g: &BuildGraph, id: NodeId, t: glia_core::CellTypeId) -> Option<serde_json::Value> {
    match g.cell(id, t)? {
        CellPayload::Json(s) => serde_json::from_str(s).ok(),
        _ => None,
    }
}

/// A node's LOCATION: (repo-relative file, 0-based line).
fn location(g: &BuildGraph, id: NodeId) -> Option<(String, u32)> {
    let v = json_of(g, id, LOCATION)?;
    let file = v.get("file")?.as_str()?.to_string();
    let line = v
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Some((file, u32::try_from(line).unwrap_or(u32::MAX)))
}

/// The build registry's name of `kind`.
fn kind_name(kind: Option<NodeKindId>) -> &'static str {
    kind.and_then(|k| bnk::ALL.iter().find(|(id, _)| *id == k))
        .map(|(_, n)| *n)
        .unwrap_or("UNKNOWN")
}

/// The build registry's name of `category`.
fn category_name(category: EdgeCategoryId) -> &'static str {
    bec::ALL
        .iter()
        .find(|(id, _)| *id == category)
        .map(|(_, n)| *n)
        .unwrap_or("UNKNOWN")
}

/// Rows by (depth, workspace, label, kind), each node once at its first.
fn rank(mut rows: Vec<TargetHit>) -> Vec<TargetHit> {
    rows.sort_by(|a, b| {
        (a.depth, &a.workspace, &a.label, a.kind).cmp(&(b.depth, &b.workspace, &b.label, b.kind))
    });
    let mut seen: BTreeSet<(String, String, &'static str)> = BTreeSet::new();
    rows.retain(|r| seen.insert((r.workspace.clone(), r.label.clone(), r.kind)));
    rows
}

fn log_answer(kind: &str, rows: usize, tests: usize) {
    eprintln!("[targets] answer kind={kind} rows={rows} tests={tests}");
}

fn tests_in(rows: &[TargetHit]) -> usize {
    rows.iter()
        .filter(|r| r.role.as_deref() == Some("test"))
        .count()
}

impl TargetsView {
    fn new(
        graph: BuildGraph,
        path: PathBuf,
        data_hash: String,
        reused: bool,
        rows: usize,
        workspaces: Vec<String>,
    ) -> Self {
        let carry = Adjacency::carry(&graph, &BUILD_TABLES);
        let mut files: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        for id in graph.ids_of(SOURCE) {
            let Some(l) = graph.node_label(id) else {
                continue;
            };
            let Some(path) = label_path(&l.workspace, &l.label) else {
                continue;
            };
            if location(&graph, id).is_some_and(|(file, _)| file == path) {
                files.entry(path).or_default().push(id);
            }
        }
        TargetsView {
            graph,
            path,
            data_hash,
            reused,
            rows,
            workspaces,
            carry,
            files,
        }
    }

    /// The row for `id`, reached at `depth` through an edge whose VIA is
    /// `via`.
    fn hit(&self, id: NodeId, depth: usize, via: String) -> Option<TargetHit> {
        let g = &self.graph;
        let label = g.node_label(id)?;
        let attrs = (label.kind == TARGET)
            .then(|| json_of(g, id, ATTRS))
            .flatten();
        let rule_class = attrs
            .as_ref()
            .and_then(|v| v.get("rule_class")?.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let loc = location(g, id);
        Some(TargetHit {
            workspace: label.workspace.clone(),
            label: label.label.clone(),
            kind: kind_name(Some(label.kind)),
            rule_class,
            role: g.role(id).map(str::to_string),
            via,
            depth,
            file: loc.as_ref().map(|(f, _)| f.clone()),
            line: loc.map(|(_, l)| l.saturating_add(1)),
            tier: Tier::Fact.as_str(),
        })
    }

    /// The VIA of the `category` edge `from -> to`, comma-joined.
    fn via(&self, from: NodeId, to: NodeId, category: EdgeCategoryId) -> String {
        self.graph
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.category == category)
            .map(|e| BuildGraph::via(e).join(","))
            .unwrap_or_default()
    }

    /// The SOURCE nodes that are the repo file `file`.
    fn sources_of(&self, file: &str) -> &[NodeId] {
        self.files.get(file).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The TARGETs a label names: `<workspace>|<label>` names one workspace's
    /// target, a bare label (`//main:hello-world`, `//main`) every
    /// workspace's, in workspace order.
    fn targets_named(&self, query: &str) -> Vec<NodeId> {
        let query = query.trim();
        let (workspace, label) = match query.split_once('|') {
            Some((ws, l)) if !query.starts_with("//") && !query.starts_with('@') => (Some(ws), l),
            _ => (None, query),
        };
        let label = full_label(label);
        let mut found: Vec<(String, NodeId)> = self
            .graph
            .targets()
            .into_iter()
            .filter_map(|id| {
                let l = self.graph.node_label(id)?;
                (l.label == label && workspace.is_none_or(|w| w == l.workspace))
                    .then(|| (l.workspace.clone(), id))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// The targets naming each of `files` (repo-relative) in `srcs`, `hdrs`
    /// or `textual_hdrs`, by (workspace, label); `via` is the attribute.
    pub fn owners(&self, files: &[String]) -> Vec<TargetHit> {
        let rows = self.owners_quiet(files);
        log_answer("owners", rows.len(), tests_in(&rows));
        rows
    }

    fn owners_quiet(&self, files: &[String]) -> Vec<TargetHit> {
        let mut rows = Vec::new();
        for file in files {
            for &src in self.sources_of(file) {
                for e in &self.graph.edges {
                    if e.to != src || e.category != HAS_SRC {
                        continue;
                    }
                    if self.graph.kind(e.from) != Some(TARGET) {
                        continue;
                    }
                    let via = BuildGraph::via(e).join(",");
                    rows.extend(self.hit(e.from, 1, via));
                }
            }
        }
        rank(rows)
    }

    /// What `label` reaches over the carry edges, at most `depth` hops:
    /// targets, source files and external labels, by (depth, workspace,
    /// label). An unknown label, or one reaching nothing, is a fact absence.
    pub fn deps(&self, label: &str, depth: usize) -> Answer<TargetHit> {
        self.walk(label, depth, Walk::Forward, "deps")
    }

    /// What reaches `label` over the carry edges, at most `depth` hops.
    pub fn rdeps(&self, label: &str, depth: usize) -> Answer<TargetHit> {
        self.walk(label, depth, Walk::Backward, "rdeps")
    }

    fn walk(&self, label: &str, depth: usize, walk: Walk, kind: &'static str) -> Answer<TargetHit> {
        let seeds = self.targets_named(label);
        let rows = if seeds.is_empty() {
            Vec::new()
        } else {
            self.reach(&seeds, walk, depth)
        };
        log_answer(kind, rows.len(), tests_in(&rows));
        Answer::from_results(rows, || self.absence(kind, label, depth, seeds.is_empty()))
    }

    /// Every node `seeds` reach along `walk`, as rows (the seeds left out).
    fn reach(&self, seeds: &[NodeId], walk: Walk, depth: usize) -> Vec<TargetHit> {
        let walked = bfs(&self.carry, seeds, walk, depth);
        let rows = walked
            .reached
            .iter()
            .filter_map(|r| {
                let via = match walk {
                    Walk::Backward => self.via(r.id, r.parent, r.via),
                    _ => self.via(r.parent, r.id, r.via),
                };
                self.hit(r.id, r.depth, via)
            })
            .collect();
        rank(rows)
    }

    fn absence(&self, kind: &'static str, query: &str, depth: usize, unknown: bool) -> Absence {
        let mechanisms: Vec<&'static str> = BUILD_TABLES
            .carry_edges
            .iter()
            .map(|c| category_name(*c))
            .collect();
        let workspaces = self
            .workspaces
            .iter()
            .map(|w| if w.is_empty() { "\"\"" } else { w.as_str() })
            .collect::<Vec<_>>()
            .join(",");
        let (reason, note, suggestions) = if unknown {
            let name = query.rsplit([':', '/']).next().unwrap_or(query);
            let near: BTreeSet<String> = self
                .graph
                .targets()
                .into_iter()
                .filter_map(|id| self.graph.label(id))
                .filter(|l| !name.is_empty() && l.contains(name))
                .map(str::to_string)
                .collect();
            (
                "unknown_symbol",
                format!(
                    "no target {} in the targets snapshot ({} rows, workspaces {workspaces})",
                    query.trim(),
                    self.rows
                ),
                near.into_iter().take(crate::absence::SUGGESTIONS).collect(),
            )
        } else {
            let way = if kind == "deps" { "leaves" } else { "reaches" };
            (
                "no_edges",
                format!(
                    "no {} edge {way} target {} within depth {depth} in the targets snapshot",
                    mechanisms.join(" / "),
                    query.trim()
                ),
                Vec::new(),
            )
        };
        let a = Absence {
            tier: Tier::Fact.as_str(),
            reason,
            query: query.to_string(),
            note,
            mechanisms,
            caveats: Vec::new(),
            suggestions,
            nodes_searched: self.graph.nodes.len(),
            unparsed_files: 0,
            named_in: Vec::new(),
        };
        let mechs = if a.mechanisms.is_empty() {
            "-".to_string()
        } else {
            a.mechanisms.join(",")
        };
        eprintln!(
            "[absence] primitive=targets_{kind} reason={} mechanisms={mechs} caveats=0 suggestions={}",
            a.reason,
            a.suggestions.len()
        );
        a
    }

    /// What `changed` (repo-relative files) affects: the owners, then every
    /// target reaching a changed file over the carry edges, all depths, and
    /// the tests among them.
    pub fn affected(&self, changed: &[String]) -> Affected {
        let changed: Vec<String> = changed
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut seeds: Vec<NodeId> = Vec::new();
        let mut unowned = Vec::new();
        for file in &changed {
            let srcs = self.sources_of(file);
            if srcs.is_empty() {
                unowned.push(file.clone());
            }
            seeds.extend_from_slice(srcs);
        }
        let owners = self.owners_quiet(&changed);
        let targets: Vec<TargetHit> = self
            .reach(&seeds, Walk::Backward, usize::MAX)
            .into_iter()
            .filter(|r| r.kind == kind_name(Some(TARGET)))
            .collect();
        let tests: Vec<TargetHit> = targets
            .iter()
            .filter(|r| r.role.as_deref() == Some("test"))
            .cloned()
            .collect();
        let caveats = if changed.iter().any(|f| is_build_file(f)) {
            vec![BUILD_FILES_CAVEAT.to_string()]
        } else {
            Vec::new()
        };
        log_answer("affected", targets.len(), tests.len());
        Affected {
            changed,
            owners,
            targets,
            tests,
            unowned,
            caveats,
        }
    }

    /// The code MODULEs of `label`'s source files: for each SOURCE the
    /// target names in `srcs` / `hdrs` / `textual_hdrs` that is a repo file,
    /// every MODULE of `merged` located at that path, by (file, qname). A
    /// fact: the two graphs agree on a repo-relative path. An unknown label,
    /// or one whose sources are external, joins nothing.
    pub fn code_modules(&self, merged: &MergedGraph, label: &str) -> Vec<Located> {
        let mut paths: BTreeSet<String> = BTreeSet::new();
        for t in self.targets_named(label) {
            for e in &self.graph.edges {
                if e.from != t || e.category != HAS_SRC {
                    continue;
                }
                if let Some((path, _)) = self.files.iter().find(|(_, ids)| ids.contains(&e.to)) {
                    paths.insert(path.clone());
                }
            }
        }
        let mut out: Vec<Located> = Vec::new();
        if !paths.is_empty() {
            let locator = Locator::new(merged);
            let mut seen: BTreeSet<u64> = BTreeSet::new();
            for g in &merged.graphs {
                for n in &g.nodes {
                    if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                        continue;
                    }
                    if !locator
                        .file_of(n.id)
                        .is_some_and(|f| paths.contains(f.as_str()))
                    {
                        continue;
                    }
                    if seen.insert(n.id.0) {
                        out.push(locator.locate(n.id));
                    }
                }
            }
        }
        out.sort_by(|a, b| (&a.file, &a.qname).cmp(&(&b.file, &b.qname)));
        log_answer("code_modules", out.len(), 0);
        out
    }

    /// Every SOURCE file the view knows as a repo path, sorted.
    pub fn files(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }
}
