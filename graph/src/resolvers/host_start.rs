//! The start rules of `resolver:host`: what a host starts, bound to the code
//! it runs.
//!
//! GH.20, manifest binaries. A packaging manifest declares a binary
//! (`cli:<bin>`, minted by `glia_code_extractors::cli`) and names its
//! implementation: a Cargo `[[bin]]` `path` (or Cargo's default), an npm /
//! composer `bin` path, a gemspec executable under its `bindir`, or a Python
//! object reference `pkg.mod:attr[.sub]` (pyproject `[project.scripts]` /
//! poetry scripts, setup.py `console_scripts`). The extractor cannot bind it:
//! a manifest is a synthetic parse in its own language group, and refs never
//! resolve across language graphs. It writes the target on the command as a
//! [`manifest_bin_cell`](glia_code_extractors::cli::manifest_bin_cell); this
//! pass reads it and draws `cli:<bin> -HANDLED_BY->` the code, EVIDENCE
//! emitter `resolver:host`, rule `manifest_bin`, located at the declaring
//! manifest line:
//!
//! - a path form binds the walked file the path names, joined to the
//!   manifest's directory (Cargo also tries `src/bin/<n>/main.rs` for its
//!   `src/bin/<n>.rs` default): a Rust bin's `main` FUNCTION, any other
//!   file's MODULE (an npm / composer / gemspec bin is a script), Strong;
//! - an object reference binds the module's file, `<dir>/<a>/<b>.py` or
//!   `<dir>/<a>/<b>/__init__.py`, then the same under `<dir>/src/`, `<dir>`
//!   being the manifest's directory, and walks each `attr` segment through
//!   that MODULE's children (a FUNCTION, CLASS, METHOD or STATE_VAR of that
//!   name, the least qname on a tie), Strong; an attr that is no node there
//!   (`app` re-exported from another module) falls back to the MODULE,
//!   Medium (`attr_fallback=`). A reference with no `:attr` names the MODULE,
//!   Strong.
//!
//! Nothing is matched by name across the repo: a target that names no walked
//! file of the manifest's repo, an absolute path or one that climbs above the
//! root is `unresolved=`. The table only says where to look; the edge needs
//! the manifest's own declaration and a walked file at its path.
//!
//! fired_on, once per build holding a manifest-declared binary with a target:
//! `[host-start] manifest_bins=<bound>/<declared> (cargo=<b>/<n> pyproject=<b>/<n>
//! setup=<b>/<n> npm=<b>/<n> gemspec=<b>/<n> composer=<b>/<n>) attr_fallback=<f>
//! unresolved=<u>`.

use std::collections::{BTreeMap, HashMap, HashSet};

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::{edge_category, node_kind};
use glia_code_extractors::cli::parse_manifest_bin_cell;
use glia_core::{Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::host_scripts::normalise;
use crate::merged::MergedGraph;

/// The manifest forms, in marker order.
const FORMS: [&str; 6] = ["cargo", "pyproject", "setup", "npm", "gemspec", "composer"];

/// The kinds an object reference's attr segment may name.
const ATTR_KINDS: [NodeKindId; 4] = [
    node_kind::FUNCTION,
    node_kind::CLASS,
    node_kind::METHOD,
    node_kind::STATE_VAR,
];

/// The edge rule (module doc).
const RULE: &str = "manifest_bin";

/// One manifest-declared binary with a target.
struct Bin {
    from: NodeId,
    repo: RepoId,
    form: String,
    /// The repo-relative manifest (or setup.py) path.
    file: String,
    line: u32,
    target: String,
}

/// A walked MODULE: its node and the graph holding it.
#[derive(Clone, Copy)]
struct Module {
    id: NodeId,
    graph: usize,
}

/// GH.20: bind every manifest-declared binary (module doc). Runs inside the
/// `resolver:host` pass.
pub(super) fn bind_manifest_bins(merged: &mut MergedGraph) {
    let bins = collect_bins(merged);
    if bins.is_empty() {
        return;
    }
    let files = module_files(merged);
    let mut counts: BTreeMap<&str, (usize, usize)> = FORMS.iter().map(|f| (*f, (0, 0))).collect();
    let (mut fallback, mut unresolved) = (0usize, 0usize);
    let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut edges = Vec::new();
    for b in &bins {
        if let Some(slot) = counts.get_mut(b.form.as_str()) {
            slot.1 += 1;
        }
        let empty = BTreeMap::new();
        let repo_files = files.get(&b.repo).unwrap_or(&empty);
        let Some((to, confidence)) = bind(merged, repo_files, b) else {
            unresolved += 1;
            continue;
        };
        if confidence == Confidence::Medium {
            fallback += 1;
        }
        if let Some(slot) = counts.get_mut(b.form.as_str()) {
            slot.0 += 1;
        }
        if !seen.insert((b.from, to)) {
            continue;
        }
        let mut e = Edge::new(b.from, to, edge_category::HANDLED_BY, confidence);
        evidence::attach(
            &mut e,
            Evidence::emitter("resolver:host")
                .rule(RULE)
                .at(b.file.clone(), b.line),
        );
        edges.push(e);
    }
    merged.cross_edges.extend(edges);
    let (bound, declared) = counts
        .values()
        .fold((0, 0), |(b, n), (fb, fnn)| (b + fb, n + fnn));
    let per_form: Vec<String> = FORMS
        .iter()
        .map(|f| {
            let (b, n) = counts.get(f).copied().unwrap_or_default();
            format!("{f}={b}/{n}")
        })
        .collect();
    eprintln!(
        "[host-start] manifest_bins={bound}/{declared} ({}) attr_fallback={fallback} unresolved={unresolved}",
        per_form.join(" ")
    );
}

/// Every CLI_COMMAND carrying a manifest-bin cell, in graph then node order
/// (never map order). A setup.py binary's file is its parent MODULE's.
fn collect_bins(merged: &MergedGraph) -> Vec<Bin> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CLI_COMMAND) {
                continue;
            }
            for t in n.cells.iter().filter_map(parse_manifest_bin_cell) {
                let file = t.file.or_else(|| {
                    let parent = g.nav.parent_of.get(&n.id)?;
                    let m = g.nodes.iter().find(|m| m.id == *parent)?;
                    evidence::locate(&m.cells).map(|(file, _)| file)
                });
                let Some(file) = file else {
                    continue;
                };
                out.push(Bin {
                    from: n.id,
                    repo: g.repo,
                    form: t.manifest,
                    file,
                    line: t.line,
                    target: t.target,
                });
            }
        }
    }
    out
}

/// Each repo's walked MODULE files: file -> the least-qname MODULE there.
fn module_files(merged: &MergedGraph) -> HashMap<RepoId, BTreeMap<String, (String, Module)>> {
    let mut out: HashMap<RepoId, BTreeMap<String, (String, Module)>> = HashMap::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let Some((file, _)) = evidence::locate(&n.cells) else {
                continue;
            };
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            let module = Module {
                id: n.id,
                graph: gi,
            };
            let slot = out.entry(g.repo).or_default().entry(file);
            match slot {
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert((qname, module));
                }
                std::collections::btree_map::Entry::Occupied(mut o) => {
                    if qname < o.get().0 {
                        o.insert((qname, module));
                    }
                }
            }
        }
    }
    out
}

/// The node `b` binds to and the edge's confidence; `None` when unresolved.
fn bind(
    merged: &MergedGraph,
    files: &BTreeMap<String, (String, Module)>,
    b: &Bin,
) -> Option<(NodeId, Confidence)> {
    let dir = b.file.rsplit_once('/').map_or("", |(d, _)| d);
    let file = |rel: &str| files.get(rel).map(|(_, m)| *m);
    match b.form.as_str() {
        "pyproject" | "setup" => {
            // `pkg.mod:attr.sub [extra]`: the extras never name code.
            let reference = b.target.split('[').next().unwrap_or("").trim();
            let (module, attr) = match reference.split_once(':') {
                Some((m, a)) => (m.trim(), Some(a.trim())),
                None => (reference, None),
            };
            if module.is_empty() || module.split('.').any(str::is_empty) {
                return None;
            }
            let path = module.replace('.', "/");
            let m = ["", "src"].iter().find_map(|base| {
                let base = normalise(dir, base).unwrap_or_default();
                [format!("{path}.py"), format!("{path}/__init__.py")]
                    .iter()
                    .find_map(|cand| file(&normalise(&base, cand)?))
            })?;
            let Some(attr) = attr.filter(|a| !a.is_empty()) else {
                return Some((m.id, Confidence::Strong));
            };
            Some(match walk_attr(merged, m, attr) {
                Some(id) => (id, Confidence::Strong),
                None => (m.id, Confidence::Medium),
            })
        }
        form => {
            if b.target.starts_with('/') {
                return None;
            }
            let target = b.target.replace('\\', "/");
            let rel = normalise(dir, &target)?;
            let mut hit = file(&rel);
            if hit.is_none()
                && form == "cargo"
                && let Some(stem) = rel.strip_suffix(".rs")
            {
                hit = file(&format!("{stem}/main.rs"));
            }
            let m = hit?;
            let main = (form == "cargo")
                .then(|| child(merged, m, "main", &[node_kind::FUNCTION]))
                .flatten();
            Some((main.unwrap_or(m.id), Confidence::Strong))
        }
    }
}

/// The node `attr` (`a.b`) names under MODULE `m`, one child per segment.
fn walk_attr(merged: &MergedGraph, m: Module, attr: &str) -> Option<NodeId> {
    let mut at = m;
    for seg in attr.split('.') {
        at = Module {
            id: child(merged, at, seg.trim(), &ATTR_KINDS)?,
            graph: at.graph,
        };
    }
    Some(at.id)
}

/// The least-qname child of `parent` named `name` with one of `kinds`.
fn child(merged: &MergedGraph, parent: Module, name: &str, kinds: &[NodeKindId]) -> Option<NodeId> {
    let nav = &merged.graphs.get(parent.graph)?.nav;
    nav.children_of
        .get(&parent.id)?
        .iter()
        .filter(|c| nav.name_by_id.get(c).map(String::as_str) == Some(name))
        .filter(|c| nav.kind_by_id.get(c).is_some_and(|k| kinds.contains(k)))
        .min_by(|a, b| nav.qname_by_id.get(a).cmp(&nav.qname_by_id.get(b)))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolvers::HostResolver;
    use crate::types::{RepoGraph, SymbolTable};
    use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type};
    use glia_code_extractors::cli::manifest_bin_cell;
    use glia_core::{Cell, CellPayload, Node};

    /// A graph builder: nodes with their nav rows.
    struct G {
        repo: RepoId,
        nav: CodeNav,
        nodes: Vec<Node>,
    }

    impl G {
        fn new(repo: RepoId) -> Self {
            G {
                repo,
                nav: CodeNav::default(),
                nodes: Vec::new(),
            }
        }

        fn node(
            &mut self,
            kind: NodeKindId,
            qname: &str,
            parent: Option<NodeId>,
            cells: Vec<Cell>,
        ) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            self.nav.record(id, name, qname, kind, parent);
            self.nodes.push(Node {
                id,
                repo: self.repo,
                confidence: Confidence::Strong,
                cells,
            });
            id
        }

        /// A located MODULE.
        fn module(&mut self, file: &str, qname: &str) -> NodeId {
            let pos = Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    r#"{{"file":"{file}","start_line":0,"end_line":0}}"#
                )),
            };
            self.node(node_kind::MODULE, qname, None, vec![pos])
        }

        fn child(&mut self, kind: NodeKindId, parent: NodeId, qname: &str) -> NodeId {
            self.node(kind, qname, Some(parent), vec![])
        }

        /// A manifest-declared `cli:<name>` under `parent`.
        fn bin(&mut self, parent: NodeId, name: &str, cell: Cell) -> NodeId {
            self.node(
                node_kind::CLI_COMMAND,
                &format!("cli:{name}"),
                Some(parent),
                vec![cell],
            )
        }

        fn done(self) -> RepoGraph {
            RepoGraph {
                repo: self.repo,
                nodes: self.nodes,
                edges: vec![],
                symbols: SymbolTable::default(),
                nav: self.nav,
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: HashSet::new(),
            }
        }
    }

    /// `(from qname, to qname, confidence)` of every edge drawn.
    fn bound(merged: &MergedGraph) -> Vec<(String, String, Confidence)> {
        let q = |id: &NodeId| {
            merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(id).cloned())
                .unwrap_or_default()
        };
        merged
            .cross_edges
            .iter()
            .map(|e| (q(&e.from), q(&e.to), e.confidence))
            .collect()
    }

    fn edge(from: &str, to: &str, c: Confidence) -> (String, String, Confidence) {
        (from.into(), to.into(), c)
    }

    #[test]
    fn binds_manifest_bins_by_path_and_object_reference() {
        let repo = RepoId(7);
        let mut code = G::new(repo);
        let api = code.module("svc/src/bin/api.rs", "svc::src::bin::api");
        code.child(node_kind::FUNCTION, api, "svc::src::bin::api::main");
        code.child(node_kind::FUNCTION, api, "svc::src::bin::api::helper");
        let seed = code.module("svc/src/bin/seed/main.rs", "svc::src::bin::seed::main");
        code.child(node_kind::FUNCTION, seed, "svc::src::bin::seed::main::main");
        let aliases = code.module("examples/aliases/aliases.py", "examples::aliases::aliases");
        code.child(
            node_kind::FUNCTION,
            aliases,
            "examples::aliases::aliases::cli",
        );
        code.module("src/pkg/cli/__init__.py", "src::pkg::cli");
        let tool = code.module("tools/tool/main.py", "tools::tool::main");
        let cls = code.child(node_kind::CLASS, tool, "tools::tool::main::App");
        code.child(node_kind::METHOD, cls, "tools::tool::main::App::run");
        code.module("web/bin/tool.js", "web::bin::tool");
        let setup = code.module("tools/setup.py", "tools::setup");
        code.bin(
            setup,
            "tool",
            manifest_bin_cell("setup", None, 5, "tool.main:App.run"),
        );

        let mut manifests = G::new(repo);
        let cargo = manifests.node(node_kind::MODULE, "svc::Cargo.toml", None, vec![]);
        let c = |line, t| manifest_bin_cell("cargo", Some("svc/Cargo.toml"), line, t);
        manifests.bin(cargo, "api", c(4, "src/bin/api.rs"));
        manifests.bin(cargo, "seed", c(8, "src/bin/seed.rs"));
        manifests.bin(cargo, "gone", c(9, "src/bin/gone.rs"));
        manifests.bin(cargo, "abs", c(10, "/usr/local/bin/abs"));
        manifests.bin(cargo, "up", c(11, "../../../up.rs"));
        let ex = manifests.node(
            node_kind::MODULE,
            "examples::aliases::pyproject.toml",
            None,
            vec![],
        );
        let ex_file = Some("examples/aliases/pyproject.toml");
        manifests.bin(
            ex,
            "aliases",
            manifest_bin_cell("pyproject", ex_file, 10, "aliases:cli"),
        );
        let root = manifests.node(node_kind::MODULE, "pyproject.toml", None, vec![]);
        let root_file = Some("pyproject.toml");
        manifests.bin(
            root,
            "pf",
            manifest_bin_cell("pyproject", root_file, 3, "pkg.cli:app [x]"),
        );
        manifests.bin(
            root,
            "pm",
            manifest_bin_cell("pyproject", root_file, 4, "pkg.cli"),
        );
        manifests.bin(
            root,
            "nope",
            manifest_bin_cell("pyproject", root_file, 5, "nope.x:y"),
        );
        let web = manifests.node(node_kind::MODULE, "web::package.json", None, vec![]);
        manifests.bin(
            web,
            "webtool",
            manifest_bin_cell("npm", Some("web/package.json"), 2, "./bin/tool.js"),
        );

        // Another repo's file at the same path binds nothing for this repo.
        let mut other = G::new(RepoId(8));
        let o = other.module("web/bin/other.js", "web::bin::other");
        other.child(node_kind::FUNCTION, o, "web::bin::other::main");
        let mut om = G::new(RepoId(8));
        let opkg = om.node(node_kind::MODULE, "web::package.json", None, vec![]);
        om.bin(
            opkg,
            "o",
            manifest_bin_cell("npm", Some("web/package.json"), 2, "bin/tool.js"),
        );

        let mut merged =
            MergedGraph::new(vec![code.done(), manifests.done(), other.done(), om.done()]);
        merged.run(&HostResolver);
        use Confidence::{Medium, Strong};
        assert_eq!(
            bound(&merged),
            [
                edge("cli:tool", "tools::tool::main::App::run", Strong),
                edge("cli:api", "svc::src::bin::api::main", Strong),
                edge("cli:seed", "svc::src::bin::seed::main::main", Strong),
                edge("cli:aliases", "examples::aliases::aliases::cli", Strong),
                edge("cli:pf", "src::pkg::cli", Medium),
                edge("cli:pm", "src::pkg::cli", Strong),
                edge("cli:webtool", "web::bin::tool", Strong),
            ]
        );
        let ev = |i: usize| Evidence::of(&merged.cross_edges[i]).expect("evidence");
        for i in 0..merged.cross_edges.len() {
            assert_eq!(merged.cross_edges[i].category, edge_category::HANDLED_BY);
            assert_eq!(ev(i).emitter, "resolver:host");
            assert_eq!(ev(i).rule.as_deref(), Some("manifest_bin"));
        }
        assert_eq!(
            (ev(0).file.as_deref(), ev(0).line),
            (Some("tools/setup.py"), Some(5)),
            "a setup.py binary is located in its parent MODULE's file"
        );
        assert_eq!(
            (ev(1).file.as_deref(), ev(1).line),
            (Some("svc/Cargo.toml"), Some(4))
        );
    }

    #[test]
    fn a_command_without_a_manifest_cell_binds_nothing() {
        let repo = RepoId(7);
        let mut g = G::new(repo);
        let m = g.module("src/main.rs", "src::main");
        g.child(node_kind::FUNCTION, m, "src::main::main");
        g.node(node_kind::CLI_COMMAND, "cli:main", Some(m), vec![]);
        let mut merged = MergedGraph::new(vec![g.done()]);
        merged.run(&HostResolver);
        assert!(merged.cross_edges.is_empty());
    }
}
