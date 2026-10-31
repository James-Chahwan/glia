//! GH.6: resolver:host. A host's configuration names a script file it runs
//! (nginx / OpenResty: `content_by_lua_file lua/hello.lua;`), recorded by the
//! reader as a [`NavFact::HostScriptRef`] on the scope that runs it (a ROUTE,
//! or the conf's MODULE). This pass binds each ref's path to a MODULE of the
//! same repo by file path and draws the edge the ref names (HANDLED_BY for a
//! content handler, USES for every other phase), EVIDENCE emitter
//! `resolver:host`, rule the directive (`content_by_lua_file`, ..), located at
//! the directive.
//!
//! The path is tried, in order, against the walked MODULE files of the script's
//! extension: joined to the conf's directory, then from the repo root (both
//! Strong), then as a path suffix (Medium): the files whose path ends with the
//! ref's, or that the ref's (absolute) path ends with, keeping the longest
//! match; two files at that length are `ambiguous=` and bind nothing. A path
//! that leaves the repo (`../..` above the root) or names no walked file is
//! `unresolved=`. A file never binds into another repo.
//!
//! fired_on, once per build that read a ref:
//! `[host-resolve] refs=<r> bound=<b> (<rule>=<n>,..) unresolved=<u> ambiguous=<a>`.

use std::collections::{BTreeMap, HashMap};

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::{NavFact, node_kind};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId, RepoId};

use super::CrossGraphResolver;
use crate::merged::MergedGraph;

/// The `resolver:host` pass (module doc).
pub struct HostResolver;

/// One ref to bind: its scope, repo and fact fields.
struct Ref {
    from: NodeId,
    repo: RepoId,
    conf: String,
    path: String,
    category: EdgeCategoryId,
    rule: String,
    line: u32,
}

/// How a ref's path met a file.
enum Bound {
    /// The conf's directory or the repo root: an exact file.
    Exact(NodeId),
    /// The one longest path suffix.
    Suffix(NodeId),
    Ambiguous,
    Unresolved,
}

impl CrossGraphResolver for HostResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        // GH.20: the start rules' manifest binaries (`host_start`).
        super::host_start::bind_manifest_bins(merged);
        let refs = collect_refs(merged);
        if refs.is_empty() {
            return;
        }
        let files = module_files(merged);
        let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
        let (mut bound, mut unresolved, mut ambiguous) = (0usize, 0usize, 0usize);
        let mut seen: std::collections::HashSet<(NodeId, NodeId, u32)> =
            std::collections::HashSet::new();
        for r in &refs {
            let empty = BTreeMap::new();
            let repo_files = files.get(&r.repo).unwrap_or(&empty);
            let (to, confidence) = match bind(repo_files, &r.conf, &r.path) {
                Bound::Exact(id) => (id, Confidence::Strong),
                Bound::Suffix(id) => (id, Confidence::Medium),
                Bound::Ambiguous => {
                    ambiguous += 1;
                    continue;
                }
                Bound::Unresolved => {
                    unresolved += 1;
                    continue;
                }
            };
            bound += 1;
            *by_rule.entry(r.rule.clone()).or_default() += 1;
            if !seen.insert((r.from, to, r.category.0)) {
                continue;
            }
            let mut e = Edge::new(r.from, to, r.category, confidence);
            evidence::attach(
                &mut e,
                Evidence::emitter("resolver:host")
                    .rule(r.rule.clone())
                    .at(r.conf.clone(), r.line),
            );
            merged.cross_edges.push(e);
        }
        let rules: Vec<String> = by_rule.iter().map(|(k, n)| format!("{k}={n}")).collect();
        eprintln!(
            "[host-resolve] refs={} bound={bound} ({}) unresolved={unresolved} ambiguous={ambiguous}",
            refs.len(),
            rules.join(",")
        );
    }
}

/// Every [`NavFact::HostScriptRef`], in graph then node order (never map order).
fn collect_refs(merged: &MergedGraph) -> Vec<Ref> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        if g.nav.nav_facts.is_empty() {
            continue;
        }
        for n in &g.nodes {
            let Some(facts) = g.nav.nav_facts.get(&n.id) else {
                continue;
            };
            for f in facts {
                if let NavFact::HostScriptRef {
                    conf,
                    path,
                    category,
                    rule,
                    line,
                    ..
                } = f
                {
                    out.push(Ref {
                        from: n.id,
                        repo: g.repo,
                        conf: conf.clone(),
                        path: path.clone(),
                        category: *category,
                        rule: rule.clone(),
                        line: *line,
                    });
                }
            }
        }
    }
    out
}

/// Each repo's MODULE files: file -> the least-qname MODULE located there.
fn module_files(merged: &MergedGraph) -> HashMap<RepoId, BTreeMap<String, (String, NodeId)>> {
    let mut out: HashMap<RepoId, BTreeMap<String, (String, NodeId)>> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let Some((file, _)) = evidence::locate(&n.cells) else {
                continue;
            };
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            let slot = out.entry(g.repo).or_default().entry(file);
            match slot {
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert((qname, n.id));
                }
                std::collections::btree_map::Entry::Occupied(mut o) => {
                    if qname < o.get().0 {
                        o.insert((qname, n.id));
                    }
                }
            }
        }
    }
    out
}

/// Bind `path` (as the conf at `conf` wrote it) to one of `files`.
fn bind(files: &BTreeMap<String, (String, NodeId)>, conf: &str, path: &str) -> Bound {
    let path = path.replace('\\', "/");
    if !path.starts_with('/') {
        let dir = conf.rsplit_once('/').map_or("", |(d, _)| d);
        for base in [dir, ""] {
            if let Some(rel) = normalise(base, &path)
                && let Some((_, id)) = files.get(&rel)
            {
                return Bound::Exact(*id);
            }
        }
    }
    let Some(want) = normalise("", path.trim_start_matches('/')) else {
        return Bound::Unresolved;
    };
    let ext = ext_of(&want);
    let mut best: Option<(usize, NodeId)> = None;
    let mut tied = false;
    for (file, (_, id)) in files {
        if ext_of(file) != ext {
            continue;
        }
        let len = if file == &want || want.ends_with(&format!("/{file}")) {
            file.len()
        } else if file.ends_with(&format!("/{want}")) {
            want.len()
        } else {
            continue;
        };
        match best {
            Some((b, _)) if len < b => {}
            Some((b, _)) if len == b => tied = true,
            _ => {
                best = Some((len, *id));
                tied = false;
            }
        }
    }
    match best {
        Some(_) if tied => Bound::Ambiguous,
        Some((_, id)) => Bound::Suffix(id),
        None => Bound::Unresolved,
    }
}

/// `path` joined to the repo-relative directory `base`, `.` and `..` folded;
/// `None` when it climbs above the repo root or names nothing.
pub(super) fn normalise(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The lowercased extension of a file name, `""` when it has none.
fn ext_of(file: &str) -> String {
    let base = file.rsplit('/').next().unwrap_or(file);
    base.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RepoGraph, SymbolTable};
    use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category};
    use glia_core::{Cell, CellPayload, Node};
    use std::collections::HashSet;

    fn module(repo: RepoId, file: &str, qname: &str) -> (Node, String) {
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, qname);
        let cell = Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":0,"end_line":0}}"#
            )),
        };
        (
            Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![cell],
            },
            qname.to_string(),
        )
    }

    fn graph(repo: RepoId, modules: &[(&str, &str)], refs: &[(&str, &str, &str)]) -> RepoGraph {
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        for (file, qname) in modules {
            let (n, q) = module(repo, file, qname);
            nav.record(n.id, &q, &q, node_kind::MODULE, None);
            nodes.push(n);
        }
        for (i, (conf, path, rule)) in refs.iter().enumerate() {
            let qname = format!("ANY /r{i}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);
            nav.record(id, &qname, &qname, node_kind::ROUTE, None);
            nav.record_fact(
                id,
                NavFact::HostScriptRef {
                    host: "nginx".into(),
                    conf: (*conf).into(),
                    path: (*path).into(),
                    category: edge_category::HANDLED_BY,
                    rule: (*rule).into(),
                    line: 3,
                },
            );
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![],
            });
        }
        RepoGraph {
            repo,
            nodes,
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    /// `(route index, target qname, confidence)` of every edge drawn.
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

    #[test]
    fn binds_by_conf_dir_root_then_suffix() {
        let repo = RepoId(3);
        let lua = graph(
            repo,
            &[
                ("deploy/lua/a.lua", "deploy::lua::a"),
                ("lua/b.lua", "lua::b"),
                ("app/handlers/c.lua", "app::handlers::c"),
                ("x/d.lua", "x::d"),
                ("y/d.lua", "y::d"),
                ("lua/e.py", "lua::e"),
            ],
            &[],
        );
        let conf = graph(
            repo,
            &[],
            &[
                ("deploy/nginx.conf", "lua/a.lua", "content_by_lua_file"),
                ("deploy/nginx.conf", "lua/b.lua", "content_by_lua_file"),
                (
                    "deploy/nginx.conf",
                    "/srv/app/handlers/c.lua",
                    "access_by_lua_file",
                ),
                ("deploy/nginx.conf", "/srv/d.lua", "content_by_lua_file"),
                ("deploy/nginx.conf", "lua/e.lua", "content_by_lua_file"),
                (
                    "deploy/nginx.conf",
                    "../../../out.lua",
                    "content_by_lua_file",
                ),
            ],
        );
        let other = graph(RepoId(4), &[("lua/e.lua", "lua::e")], &[]);
        let mut merged = MergedGraph::new(vec![lua, conf, other]);
        merged.run(&HostResolver);
        assert_eq!(
            bound(&merged),
            [
                (
                    "ANY /r0".into(),
                    "deploy::lua::a".into(),
                    Confidence::Strong
                ),
                ("ANY /r1".into(), "lua::b".into(), Confidence::Strong),
                (
                    "ANY /r2".into(),
                    "app::handlers::c".into(),
                    Confidence::Medium
                ),
            ]
        );
        let ev = Evidence::of(&merged.cross_edges[0]).unwrap();
        assert_eq!(ev.emitter, "resolver:host");
        assert_eq!(ev.rule.as_deref(), Some("content_by_lua_file"));
        assert_eq!(
            (ev.file.as_deref(), ev.line),
            (Some("deploy/nginx.conf"), Some(3))
        );
    }

    #[test]
    fn no_ref_no_marker_no_edge() {
        let repo = RepoId(3);
        let mut merged = MergedGraph::new(vec![graph(repo, &[("lua/b.lua", "lua::b")], &[])]);
        merged.run(&HostResolver);
        assert!(merged.cross_edges.is_empty());
    }

    #[test]
    fn normalise_and_ext() {
        assert_eq!(
            normalise("deploy", "./lua/../x.lua").as_deref(),
            Some("deploy/x.lua")
        );
        assert_eq!(normalise("", "../x.lua"), None);
        assert_eq!(ext_of("a/B.LUA"), "lua");
        assert_eq!(ext_of("a/.lua"), "");
    }
}
