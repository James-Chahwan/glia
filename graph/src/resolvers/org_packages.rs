//! OrgPackageResolver (FA.22): org-internal package routing over the
//! `manifest_name` identity class.
//!
//! IdentityResolver's `manifest_name` rule (FACT) joins a manifest-labelled
//! PROJECT and the PACKAGE_DEPs naming the package it declares, across repos
//! and inside a monorepo: the class says "this dependency IS that sibling
//! project". This pass routes the importing statements through it. A MODULE
//! whose IMPORT_BINDINGS cell (FA.21) holds a row naming the class's package
//! gets one IMPORTS cross edge to the module the import resolves to in the
//! sibling project, Confidence::Medium (DERIVED: two FACT links joined by a
//! rule), EVIDENCE `resolver:org_packages` rule `org_import`, at the
//! importer's file and the row's 0-based line.
//!
//! **Reads another resolver's edges.** The classes are the SAME_AS edges the
//! `identity` pass drew (EVIDENCE rule `manifest_name` or
//! `manifest_name/diverged`), so the engine's PassSpec runs this pass after
//! `identity`. A layout merge re-runs both over the union, so merge == joint
//! build.
//!
//! **Which rows route.** The importer's repo must hold a PACKAGE_DEP member
//! of the class (the dependency is DECLARED where the import is: the FACT the
//! class rests on), and the importer's language must be the class's
//! ecosystem's (the TS / JS family for npm, Go for gomod). A row matches a
//! class when its specifier is the package name or starts with `<name>/`
//! (npm), or is the module path or starts with `<modpath>/` (Go, whose
//! parser stores the path `::`-joined); of the declared classes a row
//! matches, the longest name wins (a nested module).
//!
//! **The target.** npm, by convention only (the graph holds no manifest
//! text, so package.json `main` / `module` / `types` / `exports` are not
//! read here; the TS build's workspace pass reads them for an in-repo
//! package): `name` routes to `<root>/index`, else `<root>/src/index`;
//! `name/sub/path` to `<root>/sub/path`, `<root>/sub/path/index`,
//! `<root>/src/sub/path`, then `<root>/src/sub/path/index` - each a TS / JS
//! file MODULE of the project's repo, matched by its file path without the
//! extension (two files of one stem: the least path). Go: `modpath/sub` is
//! the package directory `<root>/sub`, bound as `resolve_imports_go` binds
//! an in-repo package directory (`GoPackages::import_target`): the file
//! named after the directory, else the first non-test file by stem, else the
//! first test file; one edge per imported package.
//!
//! **Fills only what the build left unbound.** No edge when the importer
//! already has an IMPORTS edge to the target (the TS workspace pass, a Go
//! package-directory import or a tsconfig path bound it in-repo); a row
//! whose target is absent emits nothing and counts `unrouted` (a package
//! whose entry sits elsewhere). A class with two PROJECTs names no single
//! sibling: its rows are unrouted too. Maven / Gradle classes do not form yet
//! (a pom's label is its bare artifactId, FD.31's handoff); cargo and pypi
//! routing is FA.23's.
//!
//! fired_on, stderr, once per resolve that saw a `manifest_name` class:
//!   `[org-packages] classes=<c> importers=<i> rows=<r> routed=<n> unrouted=<u> (npm=<a> go=<b>)`
//! (`rows` = rows matching a declared class, `routed` = rows whose target
//! module exists, `npm` / `go` = routed rows by ecosystem), then, when an
//! edge was emitted, `[evidence-rules] resolver=org_packages org_import=<n>`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::profile::node_ext;
use glia_code_domain::same_as::{self, RULE_MANIFEST_NAME};
use glia_code_domain::{cell_type, edge_category, node_kind, read_import_bindings};
use glia_core::{CellPayload, Confidence, Edge, Node, NodeId, RepoId};

use super::host::str_field;
use super::{CrossGraphResolver, RuleTally};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// The pass name: the EVIDENCE emitter is `resolver:org_packages`.
const PASS: &str = "org_packages";
/// The one rule this pass stamps.
const RULE_ORG_IMPORT: &str = "org_import";

/// The TS / JS family's file extensions, an npm package's module files.
const NPM_EXTS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// FA.22: routes an importer's IMPORT_BINDINGS row naming a sibling
/// project's package (its `manifest_name` SAME_AS class) to that project's
/// module (see the module docs).
pub struct OrgPackageResolver;

impl CrossGraphResolver for OrgPackageResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let (edges, report) = org_edges(&merged.graphs, &merged.cross_edges);
        report.print();
        merged.cross_edges.extend(edges);
    }
}

/// An ecosystem this pass routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Eco {
    Npm,
    Go,
}

impl Eco {
    /// The PACKAGE_DEP ecosystem (`same_as::dep_identity_key`'s prefix).
    fn of(key_eco: &str) -> Option<Self> {
        match key_eco {
            "npm" => Some(Self::Npm),
            "gomod" => Some(Self::Go),
            _ => None,
        }
    }

    /// The ecosystem a file of extension `ext` imports through.
    fn of_ext(ext: &str) -> Option<Self> {
        if NPM_EXTS.contains(&ext) {
            Some(Self::Npm)
        } else if ext == "go" {
            Some(Self::Go)
        } else {
            None
        }
    }
}

/// One `manifest_name` class this pass can route: the package name an
/// importer's specifier starts with, the sibling PROJECT (its repo and
/// root dir; `None` when the class holds two PROJECTs) and the repos that
/// declare the dependency.
struct PackageIndex {
    eco: Eco,
    name: String,
    project: Option<(RepoId, String)>,
    dep_repos: BTreeSet<u64>,
}

/// The `(package name, subpath)` readings of a specifier, longest name
/// first: `spec` itself (subpath `""`), then each cut at a `/`
/// (`@acme/lib/util` -> `@acme/lib/util`, `@acme/lib` + `util`, `@acme` +
/// `lib/util`). A row names the longest declared package it starts with.
fn name_prefixes(spec: &str) -> impl Iterator<Item = (&str, &str)> {
    std::iter::once((spec, ""))
        .chain(
            spec.rmatch_indices('/')
                .map(move |(i, _)| (&spec[..i], &spec[i + 1..])),
        )
        .filter(|(name, _)| !name.is_empty())
}

/// What one resolve found, for the marker.
#[derive(Default)]
struct Report {
    classes: usize,
    importers: usize,
    rows: usize,
    routed: usize,
    unrouted: usize,
    npm: usize,
    go: usize,
    tally: Option<RuleTally>,
}

impl Report {
    fn line(&self) -> Option<String> {
        (self.classes > 0).then(|| {
            format!(
                "[org-packages] classes={} importers={} rows={} routed={} unrouted={} (npm={} go={})",
                self.classes,
                self.importers,
                self.rows,
                self.routed,
                self.unrouted,
                self.npm,
                self.go
            )
        })
    }

    fn print(&self) {
        if let Some(line) = self.line() {
            eprintln!("{line}");
        }
        if let Some(t) = &self.tally {
            t.report();
        }
    }
}

/// The ORIGIN / POSITION-style JSON payload of a cell, if it has one.
fn payload(c: &glia_core::Cell) -> Option<&str> {
    match &c.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => Some(s),
        _ => None,
    }
}

/// The repo-relative root dir of a PROJECT, from its ORIGIN `project_root`
/// cell (`""` for the repo root). A path JSON had to escape is not read.
fn project_root(n: &Node) -> Option<String> {
    n.cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .filter_map(payload)
        .find(|p| str_field(p, "provenance") == Some("project_root"))
        .and_then(|p| str_field(p, "path"))
        .filter(|p| !p.contains('\\'))
        .map(|p| p.trim_matches('/').to_string())
}

/// `dir` joined with `rel` (`/`-separated, either may be empty).
fn join(dir: &str, rel: &str) -> String {
    match (dir.is_empty(), rel.is_empty()) {
        (true, _) => rel.to_string(),
        (_, true) => dir.to_string(),
        _ => format!("{dir}/{rel}"),
    }
}

/// The `manifest_name` classes in `cross`, read through the nodes of
/// `graphs`, sorted by (ecosystem, name); and how many classes there were
/// in all (routable ecosystem or not).
fn package_classes(graphs: &[RepoGraph], cross: &[Edge]) -> (Vec<PackageIndex>, usize) {
    // center -> members, the center included. Lookup-keyed only: the
    // classes are re-sorted by name below.
    let mut members: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for e in cross
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
    {
        let Some(rule) = Evidence::of(e).and_then(|ev| ev.rule) else {
            continue;
        };
        if same_as::split_evidence_rule(&rule).0 != RULE_MANIFEST_NAME {
            continue;
        }
        members
            .entry(e.to)
            .or_insert_with(|| vec![e.to])
            .push(e.from);
    }
    if members.is_empty() {
        return (Vec::new(), 0);
    }
    let wanted: HashSet<NodeId> = members.values().flatten().copied().collect();
    let mut found: HashMap<NodeId, (&RepoGraph, &Node)> = HashMap::new();
    for g in graphs {
        for n in g.nodes.iter().filter(|n| wanted.contains(&n.id)) {
            found.entry(n.id).or_insert((g, n));
        }
    }
    let total = members.len();
    let mut out: Vec<PackageIndex> = Vec::new();
    for ids in members.values() {
        let mut key: Option<String> = None;
        let mut projects: BTreeSet<(u64, String)> = BTreeSet::new();
        let mut dep_repos: BTreeSet<u64> = BTreeSet::new();
        for id in ids {
            let Some(&(g, n)) = found.get(id) else {
                continue;
            };
            match g.nav.kind_by_id.get(id) {
                Some(&k) if k == node_kind::PROJECT => {
                    if let Some(root) = project_root(n) {
                        projects.insert((n.repo.0, root));
                    }
                }
                Some(&k) if k == node_kind::PACKAGE_DEP => {
                    let dep_key = g
                        .nav
                        .qname_by_id
                        .get(id)
                        .and_then(|q| same_as::dep_identity_key(q));
                    if let Some(dep_key) = dep_key {
                        key.get_or_insert(dep_key);
                        dep_repos.insert(n.repo.0);
                    }
                }
                _ => {}
            }
        }
        let Some((eco, name)) = key.as_deref().and_then(|k| k.split_once(':')) else {
            continue;
        };
        let Some(eco) = Eco::of(eco) else {
            continue;
        };
        let project = match projects.len() {
            1 => projects
                .into_iter()
                .next()
                .map(|(repo, root)| (RepoId(repo), root)),
            _ => None,
        };
        out.push(PackageIndex {
            eco,
            name: name.to_string(),
            project,
            dep_repos,
        });
    }
    out.sort_by(|a, b| (a.eco, &a.name).cmp(&(b.eco, &b.name)));
    (out, total)
}

/// A Go file MODULE of a package dir: (stem, id, file).
type GoFile = (String, NodeId, String);

/// One org_import edge to draw: importer, target, least row line, importer
/// file.
type Route = (NodeId, NodeId, u32, String);

/// The file MODULEs a target can be: npm files by (repo, path without its
/// extension) -> the least (path, id); Go files by (repo, dir) -> (stem,
/// id) sorted by stem.
#[derive(Default)]
struct Modules {
    npm: HashMap<(u64, String), (String, NodeId)>,
    go: HashMap<(u64, String), Vec<GoFile>>,
}

impl Modules {
    /// The modules of the repos in `repos` (the routable projects' repos).
    fn of(graphs: &[RepoGraph], repos: &BTreeSet<u64>) -> Self {
        let mut out = Self::default();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for g in graphs {
            for n in &g.nodes {
                if !repos.contains(&n.repo.0)
                    || g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE)
                    || !seen.insert(n.id)
                {
                    continue;
                }
                let Some((file, _)) = evidence::locate(&n.cells) else {
                    continue;
                };
                let Some((stem_path, ext)) = file.rsplit_once('.') else {
                    continue;
                };
                if stem_path.ends_with('/') || stem_path.is_empty() {
                    continue;
                }
                if NPM_EXTS.contains(&ext) {
                    let slot = out
                        .npm
                        .entry((n.repo.0, stem_path.to_string()))
                        .or_insert_with(|| (file.clone(), n.id));
                    if file < slot.0 {
                        *slot = (file.clone(), n.id);
                    }
                } else if ext == "go" {
                    let (dir, stem) = stem_path.rsplit_once('/').unwrap_or(("", stem_path));
                    out.go
                        .entry((n.repo.0, dir.to_string()))
                        .or_default()
                        .push((stem.to_string(), n.id, file.clone()));
                }
            }
        }
        for files in out.go.values_mut() {
            files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
        }
        out
    }

    /// The npm module `name[/sub]` names in a project at `root` of `repo`
    /// (the module docs' order), with its file.
    fn npm_target(&self, repo: RepoId, root: &str, sub: &str) -> Option<(NodeId, String)> {
        let candidates: Vec<String> = if sub.is_empty() {
            vec![join(root, "index"), join(root, "src/index")]
        } else {
            let src = join(root, &format!("src/{sub}"));
            let direct = join(root, sub);
            vec![
                direct.clone(),
                format!("{direct}/index"),
                src.clone(),
                format!("{src}/index"),
            ]
        };
        candidates
            .into_iter()
            .find_map(|p| self.npm.get(&(repo.0, p)).map(|(f, id)| (*id, f.clone())))
    }

    /// The file an import of Go package dir `dir` of `repo` binds
    /// (`GoPackages::import_target`'s choice), never `importer`, with its
    /// file.
    fn go_target(&self, repo: RepoId, dir: &str, importer: NodeId) -> Option<(NodeId, String)> {
        let files = self.go.get(&(repo.0, dir.to_string()))?;
        let others = || files.iter().filter(|(_, id, _)| *id != importer);
        let last = dir.rsplit('/').next().filter(|s| !s.is_empty());
        others()
            .find(|(stem, _, _)| !stem.ends_with("_test") && Some(stem.as_str()) == last)
            .or_else(|| others().find(|(stem, _, _)| !stem.ends_with("_test")))
            .or_else(|| others().next())
            .map(|(_, id, file)| (*id, file.clone()))
    }
}

/// The org_import edges of `graphs` given the cross edges so far, and the
/// report behind the markers.
fn org_edges(graphs: &[RepoGraph], cross: &[Edge]) -> (Vec<Edge>, Report) {
    let mut report = Report::default();
    let (classes, total) = package_classes(graphs, cross);
    report.classes = total;
    if classes.is_empty() {
        return (Vec::new(), report);
    }
    let project_repos: BTreeSet<u64> = classes
        .iter()
        .filter_map(|c| c.project.as_ref().map(|(r, _)| r.0))
        .collect();
    let modules = Modules::of(graphs, &project_repos);
    // (ecosystem, package name) -> class: a manifest_name key is one class,
    // so a name names one class per ecosystem. Lookup only.
    let by_name: HashMap<(Eco, &str), &PackageIndex> = classes
        .iter()
        .map(|c| ((c.eco, c.name.as_str()), c))
        .collect();
    let existing: HashSet<(NodeId, NodeId)> = graphs
        .iter()
        .flat_map(|g| &g.edges)
        .chain(cross)
        .filter(|e| e.category == edge_category::IMPORTS)
        .map(|e| (e.from, e.to))
        .collect();

    // (importer graph, importer node, target file) -> (importer, target, min
    // line, importer file): one edge per importer and target, in importer
    // then target-file order.
    let mut routes: BTreeMap<(usize, usize, String), Route> = BTreeMap::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for (gi, g) in graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            let Some(cell) = n
                .cells
                .iter()
                .find(|c| c.kind == cell_type::IMPORT_BINDINGS)
                .and_then(payload)
            else {
                continue;
            };
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) || !seen.insert(n.id) {
                continue;
            }
            let Some(eco) = node_ext(&n.cells).and_then(|ext| Eco::of_ext(&ext)) else {
                continue;
            };
            let Some((file, _)) = evidence::locate(&n.cells) else {
                continue;
            };
            let mut counted = false;
            for row in read_import_bindings(cell) {
                // Go's parser stores the import path `::`-joined.
                let spec = match eco {
                    Eco::Go => row.spec.replace("::", "/"),
                    Eco::Npm => row.spec,
                };
                let best = name_prefixes(&spec).find_map(|(name, sub)| {
                    let class = *by_name.get(&(eco, name))?;
                    class.dep_repos.contains(&n.repo.0).then_some((class, sub))
                });
                let Some((class, sub)) = best else {
                    continue;
                };
                counted = true;
                report.rows += 1;
                let target = class
                    .project
                    .as_ref()
                    .and_then(|(repo, root)| match class.eco {
                        Eco::Npm => modules.npm_target(*repo, root, sub),
                        Eco::Go => modules.go_target(*repo, &join(root, sub), n.id),
                    });
                let Some((target, target_file)) = target.filter(|t| t.0 != n.id) else {
                    report.unrouted += 1;
                    continue;
                };
                report.routed += 1;
                match class.eco {
                    Eco::Npm => report.npm += 1,
                    Eco::Go => report.go += 1,
                }
                if existing.contains(&(n.id, target)) {
                    continue;
                }
                routes
                    .entry((gi, ni, target_file))
                    .and_modify(|r| r.2 = r.2.min(row.line))
                    .or_insert((n.id, target, row.line, file.clone()));
            }
            report.importers += usize::from(counted);
        }
    }

    let mut tally = RuleTally::new(PASS, &[RULE_ORG_IMPORT]);
    let mut edges = Vec::with_capacity(routes.len());
    for (from, to, line, file) in routes.into_values() {
        let ev = tally.evidence(RULE_ORG_IMPORT).at(file, line);
        let mut e = Edge::new(from, to, edge_category::IMPORTS, Confidence::Medium);
        evidence::attach(&mut e, ev);
        edges.push(e);
    }
    tally.add(RULE_ORG_IMPORT, edges.len());
    report.tally = Some(tally);
    (edges, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::{Cell, NodeKindId};

    fn json(kind: glia_core::CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(s.to_string()),
        }
    }

    /// One hand-built repo graph.
    struct G {
        repo: RepoId,
        nodes: Vec<Node>,
        nav: CodeNav,
    }

    impl G {
        fn new(tag: &str) -> Self {
            Self {
                repo: RepoId::from_canonical(&format!("test://org/{tag}")),
                nodes: vec![],
                nav: CodeNav::default(),
            }
        }

        fn add(&mut self, kind: NodeKindId, qname: &str, cells: Vec<Cell>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
            self.nav.record(id, qname, qname, kind, None);
            self.nodes.push(Node {
                id,
                repo: self.repo,
                confidence: Confidence::Strong,
                cells,
            });
            id
        }

        fn module(&mut self, file: &str, bindings: Option<&str>) -> NodeId {
            let qname = file
                .rsplit_once('.')
                .map_or(file, |(s, _)| s)
                .replace('/', "::");
            let mut cells = vec![json(
                cell_type::POSITION,
                &format!(r#"{{"file":"{file}","start_line":0,"end_line":3}}"#),
            )];
            if let Some(b) = bindings {
                cells.push(json(cell_type::IMPORT_BINDINGS, b));
            }
            self.add(node_kind::MODULE, &qname, cells)
        }

        fn project(&mut self, path: &str, eco: &str, label: &str) -> NodeId {
            let q = format!("project:{}", if path.is_empty() { "." } else { path });
            let origin = format!(
                r#"{{"ecosystem":"{eco}","label":"{label}","label_source":"manifest","path":"{path}","provenance":"project_root"}}"#
            );
            self.add(
                node_kind::PROJECT,
                &q,
                vec![json(cell_type::ORIGIN, &origin)],
            )
        }

        fn dep(&mut self, qname: &str) -> NodeId {
            self.add(node_kind::PACKAGE_DEP, qname, vec![])
        }

        fn build(self) -> RepoGraph {
            RepoGraph {
                repo: self.repo,
                nodes: self.nodes,
                edges: vec![],
                nav: self.nav,
                symbols: Default::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }
        }
    }

    fn same_as(from: NodeId, to: NodeId) -> Edge {
        let mut e = Edge::new(from, to, edge_category::SAME_AS, Confidence::Strong);
        evidence::attach(
            &mut e,
            Evidence::emitter("resolver:identity").rule(RULE_MANIFEST_NAME),
        );
        e
    }

    /// (from file, to file, rule, 0-based line) of every edge.
    fn rows(graphs: &[RepoGraph], edges: &[Edge]) -> Vec<(String, String, String, u32)> {
        let file = |id: NodeId| {
            graphs
                .iter()
                .flat_map(|g| &g.nodes)
                .find(|n| n.id == id)
                .and_then(|n| evidence::locate(&n.cells))
                .map(|(f, _)| f)
                .unwrap_or_default()
        };
        edges
            .iter()
            .map(|e| {
                let ev = Evidence::of(e).unwrap();
                assert_eq!(e.category, edge_category::IMPORTS);
                assert_eq!(e.confidence, Confidence::Medium);
                assert_eq!(ev.emitter, "resolver:org_packages");
                (file(e.from), file(e.to), ev.rule.unwrap(), ev.line.unwrap())
            })
            .collect()
    }

    /// app (declares npm `@acme/lib` and gomod `example.com/acme/lib`) and
    /// lib (the two PROJECTs), each class's star drawn as identity would.
    fn two_repos(app_rows_ts: &str, app_rows_go: &str) -> (Vec<RepoGraph>, Vec<Edge>) {
        let mut app = G::new("app");
        app.module("web/src/main.ts", Some(app_rows_ts));
        app.module("svc/main.go", Some(app_rows_go));
        let npm_dep = app.dep("package:npm:@acme/lib");
        let go_dep = app.dep("package:gomod:example.com/acme/lib");
        let mut lib = G::new("lib");
        lib.module("js/src/index.ts", None);
        lib.module("js/src/util/fmt.ts", None);
        lib.module("go/text/a.go", None);
        lib.module("go/text/text.go", None);
        lib.module("go/text/text_test.go", None);
        let npm_proj = lib.project("js", "npm", "@acme/lib");
        let go_proj = lib.project("go", "go", "example.com/acme/lib");
        let cross = vec![same_as(npm_proj, npm_dep), same_as(go_proj, go_dep)];
        (vec![app.build(), lib.build()], cross)
    }

    #[test]
    fn routes_npm_entry_subpath_and_go_package_dir() {
        let (graphs, cross) = two_repos(
            r#"[{"spec":"@acme/lib","name":"greet","alias":null,"line":0},{"spec":"@acme/lib","name":"other","alias":null,"line":4},{"spec":"@acme/lib/util/fmt","name":"f","alias":null,"line":1},{"spec":"@acme/lib/missing","name":"m","alias":null,"line":2},{"spec":"@acme/library","name":"x","alias":null,"line":3}]"#,
            r#"[{"spec":"example.com::acme::lib::text","name":null,"alias":null,"line":2}]"#,
        );
        let (edges, report) = org_edges(&graphs, &cross);
        assert_eq!(
            rows(&graphs, &edges),
            [
                ("web/src/main.ts", "js/src/index.ts", "org_import", 0),
                ("web/src/main.ts", "js/src/util/fmt.ts", "org_import", 1),
                ("svc/main.go", "go/text/text.go", "org_import", 2),
            ]
            .map(|(a, b, r, l)| (a.to_string(), b.to_string(), r.to_string(), l))
        );
        assert_eq!(
            report.line().unwrap(),
            "[org-packages] classes=2 importers=2 rows=5 routed=4 unrouted=1 (npm=3 go=1)"
        );
    }

    #[test]
    fn undeclared_dependency_routes_nothing() {
        let (mut graphs, cross) = two_repos(
            r#"[{"spec":"@acme/lib","name":"greet","alias":null,"line":0}]"#,
            r#"[{"spec":"example.com::acme::lib::text","name":null,"alias":null,"line":2}]"#,
        );
        // A third repo importing the same packages without declaring them.
        let mut other = G::new("other");
        other.module(
            "main.ts",
            Some(r#"[{"spec":"@acme/lib","name":"greet","alias":null,"line":0}]"#),
        );
        graphs.push(other.build());
        let (edges, report) = org_edges(&graphs, &cross);
        assert!(rows(&graphs, &edges).iter().all(|r| r.0 != "main.ts"));
        assert_eq!(report.importers, 2);
    }

    #[test]
    fn existing_imports_edge_is_not_duplicated() {
        let (mut graphs, cross) = two_repos(
            r#"[{"spec":"@acme/lib","name":"greet","alias":null,"line":0}]"#,
            "[]",
        );
        let from = graphs[0].nodes[0].id;
        let to = graphs[1].nodes[0].id;
        graphs[0].edges.push(Edge::new(
            from,
            to,
            edge_category::IMPORTS,
            Confidence::Strong,
        ));
        let (edges, report) = org_edges(&graphs, &cross);
        assert!(edges.is_empty());
        assert_eq!((report.rows, report.routed), (1, 1));
        assert!(report.tally.as_ref().unwrap().line().is_none());
    }

    #[test]
    fn longest_declared_name_wins_and_wrong_language_is_skipped() {
        let mut app = G::new("app");
        app.module(
            "main.go",
            Some(r#"[{"spec":"example.com::acme::lib::text::v2","name":null,"alias":null,"line":5}]"#),
        );
        app.module(
            "tool.py",
            Some(r#"[{"spec":"example.com::acme::lib","name":null,"alias":null,"line":0}]"#),
        );
        let d1 = app.dep("package:gomod:example.com/acme/lib");
        let d2 = app.dep("package:gomod:example.com/acme/lib/text");
        let mut lib = G::new("lib");
        lib.module("text/x.go", None);
        lib.module("sub/v2/v2.go", None);
        let p1 = lib.project("", "go", "example.com/acme/lib");
        let p2 = lib.project("sub", "go", "example.com/acme/lib/text");
        let graphs = vec![app.build(), lib.build()];
        let cross = vec![same_as(p1, d1), same_as(p2, d2)];
        let (edges, report) = org_edges(&graphs, &cross);
        assert_eq!(
            rows(&graphs, &edges),
            [(
                "main.go".to_string(),
                "sub/v2/v2.go".to_string(),
                "org_import".to_string(),
                5
            )]
        );
        assert_eq!((report.rows, report.importers), (1, 1));
    }

    #[test]
    fn no_class_no_marker() {
        let mut app = G::new("app");
        app.module(
            "main.ts",
            Some(r#"[{"spec":"@acme/lib","name":"greet","alias":null,"line":0}]"#),
        );
        let (edges, report) = org_edges(&[app.build()], &[]);
        assert!(edges.is_empty());
        assert!(report.line().is_none());
    }

    #[test]
    fn two_projects_leave_the_class_unrouted() {
        let mut app = G::new("app");
        app.module(
            "main.ts",
            Some(r#"[{"spec":"@acme/lib","name":"greet","alias":null,"line":0}]"#),
        );
        let d = app.dep("package:npm:@acme/lib");
        let mut lib = G::new("lib");
        lib.module("index.ts", None);
        let p = lib.project("", "npm", "@acme/lib");
        let mut fork = G::new("fork");
        fork.module("index.ts", None);
        let f = fork.project("", "npm", "@acme/lib");
        let graphs = vec![app.build(), lib.build(), fork.build()];
        let star = [same_as(p, d), same_as(f, d)];
        let (edges, report) = org_edges(&graphs, &star);
        assert!(edges.is_empty());
        assert_eq!((report.rows, report.unrouted), (1, 1));
    }
}
