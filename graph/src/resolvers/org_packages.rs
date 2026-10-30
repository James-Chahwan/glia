//! OrgPackageResolver (FA.22, FA.23): org-internal package routing over the
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
//! **Calls and references (FA.23).** The importer's call sites the language
//! build left unresolved (`RepoGraph.unresolved_calls`) whose caller's
//! nearest importing MODULE (its nav parent chain) bound the name are bound
//! to the sibling's declaration: `Bare(x)` where `x` is a symbol import's
//! local name (`import { greet }`, `from m import shout`, `use m::helper`)
//! to the declaration of the imported name in the routed module; and
//! `Attribute { base, name }` where `base` is a whole-module import's local
//! name (`import * as lib`, Go `text`, Python `import m as tx`, a Python
//! `from pkg import mod` or Rust `use crate_name::module` naming a
//! submodule) to that module's (a Go package directory's, every non-test
//! file) top-level declaration `name`. Each is a CALLS cross edge from the
//! caller, Medium, rule `org_call`, at the importer's file and the call's
//! line; an unresolved USES / INJECTS / HANDLED_BY reference
//! (`unresolved_refs`, read through its `from_module`) is bound the same way
//! as an edge of its own category, rule `org_ref`. A whole-module import's
//! local name is its alias, else the language's default binding
//! (`glia_code_domain::import_default_binding`: Go's last path segment,
//! Python's first dotted one, Rust's last); a TS default (`default`) or glob
//! (`*`) import binds nothing. A declaration is a module's own top-level
//! name (`SymbolTable.module_symbols`, persisted, so a layout merge reads
//! what the joint build read); a TS barrel that re-exports it (DT.1q, the
//! module's IMPORTS edges of EVIDENCE rule `reexport`) is followed one level
//! at a time, at most 8 levels, and a level naming two distinct
//! declarations binds nothing (`ambiguous`), as do two Go files of one
//! package declaring the name. A bound site stays in `unresolved_calls`
//! (the language build did not bind it; the cross edge is the binding), so
//! each per-repo graph is unchanged and a layout merge re-running this pass
//! over the union draws exactly the joint build's edges.
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
//! ecosystem's (the TS / JS family for npm, Go for gomod, Rust for cargo,
//! Python for pypi). A row matches a class when its specifier is the package
//! name or starts with `<name>/` (npm), or is the module path or starts with
//! `<modpath>/` (Go, whose parser stores the path `::`-joined); of the
//! declared classes a row matches, the longest name wins (a nested module).
//! Cargo: the specifier's first `::` segment is the crate name, matched to
//! the class's normalised name (`org-lib` and `org_lib` are one). PyPI: the
//! specifier's first dotted segment, PEP 503-normalised, is the
//! distribution name (`org_util` -> `org-util`), else the one declared class
//! whose project holds a top-level package directory of that name
//! (`<root>/<pkg>` or `<root>/src/<pkg>`, a distribution imported under
//! another name); a row matching neither routes nothing and is not counted.
//!
//! **The target.** npm: the entry the sibling's `package.json` declares,
//! recorded on its PROJECT's ORIGIN cell by the walk (`entry`: `exports`
//! when it is a string or its `"."` entry is one, else `main`; `subpaths`:
//! an `exports` map's string `"./x"` entries, for `name/x`), mapped back to
//! source (`dist/index.js` -> `src/index.ts`); else by convention: `name`
//! routes to `<root>/index`, else `<root>/src/index`; `name/sub/path` to
//! `<root>/sub/path`, `<root>/sub/path/index`, `<root>/src/sub/path`, then
//! `<root>/src/sub/path/index` - each a TS / JS file MODULE of the project's
//! repo, matched by its file path without the extension (two files of one
//! stem: the least path). Conditional `exports` objects, `module` and
//! `types` are not read: such a package routes by the convention or stays
//! `unrouted`. Go: `modpath/sub` is the package directory `<root>/sub`,
//! bound as `resolve_imports_go` binds an in-repo package directory
//! (`GoPackages::import_target`): the file named after the directory, else
//! the first non-test file by stem, else the first test file; one edge per
//! imported package. Cargo: the crate root `<root>/src/lib.rs`, then each
//! path segment as a child module (`util` -> `<root>/src/util.rs` or
//! `<root>/src/util/mod.rs`). PyPI: `pkg.mod` -> `<root>/pkg/mod.py`,
//! `<root>/pkg/mod/__init__.py`, then the same under `<root>/src/`.
//!
//! **Fills only what the build left unbound.** No edge when the importer
//! already has an IMPORTS edge to the target (the TS workspace pass, a Go
//! package-directory import or a tsconfig path bound it in-repo), or the
//! caller already has an edge of that category to the declaration; a row
//! whose target is absent emits nothing and counts `unrouted` (a package
//! whose entry sits elsewhere). A class with two PROJECTs names no single
//! sibling: its rows are unrouted too. Maven / Gradle classes do not form yet
//! (a pom's label is its bare artifactId, FD.31's handoff; 0.5.5).
//!
//! fired_on, stderr, once per resolve that saw a `manifest_name` class:
//!   `[org-packages] classes=<c> importers=<i> rows=<r> routed=<n> unrouted=<u> (npm=<a> go=<b> cargo=<k> pypi=<p>)`
//!   `[org-packages] calls bound=<c> refs bound=<r> ambiguous=<a> (npm=<n> go=<g> cargo=<k> pypi=<p>)`
//! (`rows` = rows matching a declared class, `routed` = rows whose target
//! module exists, the first parenthesis routed rows by ecosystem; `calls` /
//! `refs` = org_call / org_ref edges drawn, `ambiguous` = sites whose name
//! two declarations answer, the second parenthesis the edges by the
//! importer's ecosystem), then, when an edge was emitted,
//! `[evidence-rules] resolver=org_packages org_import=<n> org_call=<n> org_ref=<n>`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::profile::node_ext;
use glia_code_domain::same_as::{self, RULE_MANIFEST_NAME};
use glia_code_domain::{
    CallQualifier, cell_type, edge_category, import_default_binding, node_kind,
    read_import_bindings,
};
use glia_core::{CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, RepoId};

use super::host::{quoted_body, str_field};
use super::{CrossGraphResolver, RuleTally};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// The pass name: the EVIDENCE emitter is `resolver:org_packages`.
const PASS: &str = "org_packages";
/// An importer -IMPORTS-> sibling module edge (FA.22).
const RULE_ORG_IMPORT: &str = "org_import";
/// A caller -CALLS-> sibling declaration edge (FA.23).
const RULE_ORG_CALL: &str = "org_call";
/// A USES / INJECTS / HANDLED_BY reference into a sibling (FA.23).
const RULE_ORG_REF: &str = "org_ref";

/// The TS / JS family's file extensions, an npm package's module files.
const NPM_EXTS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
/// A Python distribution's module files.
const PY_EXTS: &[&str] = &["py", "pyi"];
/// The re-export levels a barrel lookup follows (FA.23).
const REEXPORT_DEPTH: usize = 8;
/// The nav parent hops from a caller to its importing MODULE, at most.
const PARENT_HOPS: usize = 64;
/// The reference categories `org_ref` binds.
const REF_CATEGORIES: [EdgeCategoryId; 3] = [
    edge_category::USES,
    edge_category::INJECTS,
    edge_category::HANDLED_BY,
];

/// FA.22 / FA.23: routes an importer's IMPORT_BINDINGS row naming a sibling
/// project's package (its `manifest_name` SAME_AS class) to that project's
/// module, and the importer's unresolved calls and references through the
/// names it bound to the sibling's declarations (see the module docs).
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
    Cargo,
    Pypi,
}

impl Eco {
    /// The PACKAGE_DEP ecosystem (`same_as::dep_identity_key`'s prefix).
    fn of(key_eco: &str) -> Option<Self> {
        match key_eco {
            "npm" => Some(Self::Npm),
            "gomod" => Some(Self::Go),
            "cargo" => Some(Self::Cargo),
            "pypi" => Some(Self::Pypi),
            _ => None,
        }
    }

    /// The ecosystem a file of extension `ext` imports through.
    fn of_ext(ext: &str) -> Option<Self> {
        if NPM_EXTS.contains(&ext) {
            Some(Self::Npm)
        } else if ext == "go" {
            Some(Self::Go)
        } else if ext == "rs" {
            Some(Self::Cargo)
        } else if PY_EXTS.contains(&ext) {
            Some(Self::Pypi)
        } else {
            None
        }
    }

    /// The language name `import_default_binding` reads.
    fn lang(self) -> &'static str {
        match self {
            Self::Npm => "typescript",
            Self::Go => "go",
            Self::Cargo => "rust",
            Self::Pypi => "python",
        }
    }

    /// The ecosystem's index in a [`PerEco`] tally.
    fn slot(self) -> usize {
        self as usize
    }
}

/// A count per ecosystem, in [`Eco`] order.
type PerEco = [usize; 4];

/// `npm=<a> go=<b> cargo=<k> pypi=<p>`.
fn per_eco(c: &PerEco) -> String {
    format!("npm={} go={} cargo={} pypi={}", c[0], c[1], c[2], c[3])
}

/// The sibling PROJECT a class routes to: its repo, root dir and, for npm,
/// the entry files its ORIGIN records (FA.23).
#[derive(Clone)]
struct Project {
    repo: RepoId,
    root: String,
    entry: Option<String>,
    subpaths: Vec<(String, String)>,
}

/// One `manifest_name` class this pass can route: the package name an
/// importer's specifier starts with, the sibling PROJECT (`None` when the
/// class holds two PROJECTs) and the repos that declare the dependency.
struct PackageIndex {
    eco: Eco,
    name: String,
    project: Option<Project>,
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

/// What one resolve found, for the markers.
#[derive(Default)]
struct Report {
    classes: usize,
    importers: usize,
    rows: usize,
    routed: usize,
    unrouted: usize,
    routed_by: PerEco,
    calls: usize,
    refs: usize,
    ambiguous: usize,
    bound_by: PerEco,
    tally: Option<RuleTally>,
}

impl Report {
    fn line(&self) -> Option<String> {
        (self.classes > 0).then(|| {
            format!(
                "[org-packages] classes={} importers={} rows={} routed={} unrouted={} ({})",
                self.classes,
                self.importers,
                self.rows,
                self.routed,
                self.unrouted,
                per_eco(&self.routed_by)
            )
        })
    }

    fn calls_line(&self) -> Option<String> {
        (self.classes > 0).then(|| {
            format!(
                "[org-packages] calls bound={} refs bound={} ambiguous={} ({})",
                self.calls,
                self.refs,
                self.ambiguous,
                per_eco(&self.bound_by)
            )
        })
    }

    fn print(&self) {
        for line in [self.line(), self.calls_line()].into_iter().flatten() {
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

/// A PROJECT's ORIGIN `project_root` payload.
fn project_origin(n: &Node) -> Option<&str> {
    n.cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .filter_map(payload)
        .find(|p| str_field(p, "provenance") == Some("project_root"))
}

/// The repo-relative root dir of a PROJECT, from its ORIGIN `project_root`
/// cell (`""` for the repo root). A path JSON had to escape is not read.
fn project_root(origin: &str) -> Option<String> {
    str_field(origin, "path")
        .filter(|p| !p.contains('\\'))
        .map(|p| p.trim_matches('/').to_string())
}

/// The ORIGIN's `subpaths` object (`{"./x":"src/x.ts",..}`, written compact
/// by the walk, FA.23) as `(key, file)` pairs; a pair JSON had to escape is
/// skipped, and anything unexpected ends the read.
fn origin_subpaths(origin: &str) -> Vec<(String, String)> {
    const KEY: &str = "\"subpaths\":{";
    let Some(start) = origin.find(KEY) else {
        return Vec::new();
    };
    let mut rest = &origin[start + KEY.len()..];
    let mut out = Vec::new();
    while let Some(r) = rest.strip_prefix('"') {
        let Some(key) = quoted_body(r) else { break };
        let Some(r) = r[key.len() + 1..].strip_prefix(":\"") else {
            break;
        };
        let Some(file) = quoted_body(r) else { break };
        if !key.contains('\\') && !file.contains('\\') {
            out.push((key.to_string(), file.to_string()));
        }
        rest = &r[file.len() + 1..];
        match rest.strip_prefix(',') {
            Some(r) => rest = r,
            None => break,
        }
    }
    out
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
        // (repo, root) -> the npm entry fields of its ORIGIN.
        let mut projects: BTreeMap<(u64, String), NpmEntry> = BTreeMap::new();
        let mut dep_repos: BTreeSet<u64> = BTreeSet::new();
        for id in ids {
            let Some(&(g, n)) = found.get(id) else {
                continue;
            };
            match g.nav.kind_by_id.get(id) {
                Some(&k) if k == node_kind::PROJECT => {
                    let Some(origin) = project_origin(n) else {
                        continue;
                    };
                    if let Some(root) = project_root(origin) {
                        let entry = str_field(origin, "entry")
                            .filter(|p| !p.contains('\\'))
                            .map(str::to_string);
                        projects.insert((n.repo.0, root), (entry, origin_subpaths(origin)));
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
                .map(|((repo, root), (entry, subpaths))| Project {
                    repo: RepoId(repo),
                    root,
                    entry,
                    subpaths,
                }),
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

/// A PROJECT's recorded npm entry and `exports` subpaths (FA.23).
type NpmEntry = (Option<String>, Vec<(String, String)>);

/// A Go file MODULE of a package dir: (stem, id, file).
type GoFile = (String, NodeId, String);

/// One org_import edge to draw: importer, target, least row line, importer
/// file.
type Route = (NodeId, NodeId, u32, String);

/// The file MODULEs a target can be: npm, cargo and pypi files by (repo,
/// ecosystem, path without its extension) -> the least (path, id), and by
/// (repo, path); Go files by (repo, dir) -> (stem, id) sorted by stem; the
/// directories holding a Python file, by (repo, dir).
#[derive(Default)]
struct Modules {
    stems: HashMap<(u64, Eco, String), (String, NodeId)>,
    files: HashMap<(u64, String), NodeId>,
    go: HashMap<(u64, String), Vec<GoFile>>,
    py_dirs: HashSet<(u64, String)>,
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
                let Some(eco) = Eco::of_ext(ext) else {
                    continue;
                };
                out.files.insert((n.repo.0, file.clone()), n.id);
                if eco == Eco::Go {
                    let (dir, stem) = stem_path.rsplit_once('/').unwrap_or(("", stem_path));
                    out.go
                        .entry((n.repo.0, dir.to_string()))
                        .or_default()
                        .push((stem.to_string(), n.id, file.clone()));
                    continue;
                }
                if eco == Eco::Pypi {
                    let mut dir = stem_path;
                    while let Some((parent, _)) = dir.rsplit_once('/') {
                        out.py_dirs.insert((n.repo.0, parent.to_string()));
                        dir = parent;
                    }
                }
                let slot = out
                    .stems
                    .entry((n.repo.0, eco, stem_path.to_string()))
                    .or_insert_with(|| (file.clone(), n.id));
                if file < slot.0 {
                    *slot = (file.clone(), n.id);
                }
            }
        }
        for files in out.go.values_mut() {
            files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
        }
        out
    }

    /// The first of `stems` that is a file MODULE of `eco` in `repo`.
    fn first(&self, repo: RepoId, eco: Eco, stems: &[String]) -> Option<(NodeId, String)> {
        stems.iter().find_map(|p| {
            self.stems
                .get(&(repo.0, eco, p.clone()))
                .map(|(f, id)| (*id, f.clone()))
        })
    }

    /// The file MODULE at exactly `file` of `repo`.
    fn file(&self, repo: RepoId, file: &str) -> Option<(NodeId, String)> {
        self.files
            .get(&(repo.0, file.to_string()))
            .map(|id| (*id, file.to_string()))
    }

    /// The npm module `name[/sub]` names in `project` (the module docs'
    /// order: the recorded entry, then the convention), with its file.
    fn npm_target(&self, project: &Project, sub: &str) -> Option<(NodeId, String)> {
        let (repo, root) = (project.repo, project.root.as_str());
        let declared = if sub.is_empty() {
            project.entry.clone()
        } else {
            let key = format!("./{sub}");
            project
                .subpaths
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, f)| f.clone())
        };
        if let Some(hit) = declared.and_then(|f| self.file(repo, &f)) {
            return Some(hit);
        }
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
        self.first(repo, Eco::Npm, &candidates)
    }

    /// The crate module `sub` (`/`-joined path segments, `""` the crate
    /// root) names in `project`: `src/lib`, else `src/<sub>`, then
    /// `src/<sub>/mod`.
    fn cargo_target(&self, project: &Project, sub: &str) -> Option<(NodeId, String)> {
        let src = join(&project.root, "src");
        let candidates = if sub.is_empty() {
            vec![join(&src, "lib")]
        } else {
            let p = join(&src, sub);
            vec![p.clone(), format!("{p}/mod")]
        };
        self.first(project.repo, Eco::Cargo, &candidates)
    }

    /// The Python module of dotted path `path` (`/`-joined, top package
    /// first) in `project`: `<root>/<path>`, `<root>/<path>/__init__`, then
    /// both under `<root>/src`.
    fn pypi_target(&self, project: &Project, path: &str) -> Option<(NodeId, String)> {
        let candidates: Vec<String> = [project.root.clone(), join(&project.root, "src")]
            .iter()
            .flat_map(|base| {
                let p = join(base, path);
                [p.clone(), format!("{p}/__init__")]
            })
            .collect();
        self.first(project.repo, Eco::Pypi, &candidates)
    }

    /// Whether `project` holds a top-level Python package directory `top`.
    fn has_py_package(&self, project: &Project, top: &str) -> bool {
        [
            join(&project.root, top),
            join(&project.root, &format!("src/{top}")),
        ]
        .into_iter()
        .any(|d| self.py_dirs.contains(&(project.repo.0, d)))
    }

    /// The Go files of package dir `dir` of `repo` other than `importer`,
    /// in stem order.
    fn go_files<'a>(
        &'a self,
        repo: RepoId,
        dir: &str,
        importer: NodeId,
    ) -> impl Iterator<Item = &'a GoFile> + 'a {
        self.go
            .get(&(repo.0, dir.to_string()))
            .into_iter()
            .flatten()
            .filter(move |(_, id, _)| *id != importer)
    }

    /// The file an import of Go package dir `dir` of `repo` binds
    /// (`GoPackages::import_target`'s choice), never `importer`, with its
    /// file.
    fn go_target(&self, repo: RepoId, dir: &str, importer: NodeId) -> Option<(NodeId, String)> {
        let last = dir.rsplit('/').next().filter(|s| !s.is_empty());
        let others = || self.go_files(repo, dir, importer);
        others()
            .find(|(stem, _, _)| !stem.ends_with("_test") && Some(stem.as_str()) == last)
            .or_else(|| others().find(|(stem, _, _)| !stem.ends_with("_test")))
            .or_else(|| others().next())
            .map(|(_, id, file)| (*id, file.clone()))
    }

    /// The modules whose top-level declarations a whole-module import of Go
    /// package dir `dir` reaches: its non-test files (every file when it has
    /// none), never `importer`.
    fn go_scope(&self, repo: RepoId, dir: &str, importer: NodeId) -> Vec<NodeId> {
        let code: Vec<NodeId> = self
            .go_files(repo, dir, importer)
            .filter(|(stem, _, _)| !stem.ends_with("_test"))
            .map(|(_, id, _)| *id)
            .collect();
        if code.is_empty() {
            self.go_files(repo, dir, importer)
                .map(|(_, id, _)| *id)
                .collect()
        } else {
            code
        }
    }
}

/// A row's routed target: the module an import edge points at, and the
/// modules whose declarations its names reach (the module itself; a Go
/// package directory's files).
struct Routed {
    module: (NodeId, String),
    scope: Vec<NodeId>,
}

/// One importer's routable package rows matched to their classes.
struct Matched<'a> {
    class: &'a PackageIndex,
    /// The routed path inside the package: npm / Go a `/` subpath, cargo
    /// the module path after the crate, pypi the whole dotted path
    /// `/`-joined (top package first).
    sub: String,
}

/// The class a row of ecosystem `eco` names, and its routed path.
fn match_row<'a>(
    eco: Eco,
    spec: &str,
    importer_repo: u64,
    by_name: &HashMap<(Eco, &str), &'a PackageIndex>,
    pypi: &[&'a PackageIndex],
    modules: &Modules,
) -> Option<Matched<'a>> {
    let declared = |c: &&'a PackageIndex| c.dep_repos.contains(&importer_repo);
    match eco {
        Eco::Npm | Eco::Go => {
            // Go's parser stores the import path `::`-joined.
            let spec = match eco {
                Eco::Go => spec.replace("::", "/"),
                _ => spec.to_string(),
            };
            name_prefixes(&spec).find_map(|(name, sub)| {
                let class = *by_name.get(&(eco, name))?;
                declared(&class).then(|| Matched {
                    class,
                    sub: sub.to_string(),
                })
            })
        }
        Eco::Cargo => {
            let mut segs = spec.split("::").filter(|s| !s.is_empty());
            let krate = segs.next()?;
            let key = same_as::dep_identity_key(&format!("package:cargo:{krate}"))?;
            let class = *by_name.get(&(eco, key.strip_prefix("cargo:")?))?;
            declared(&class).then(|| Matched {
                class,
                sub: segs.collect::<Vec<_>>().join("/"),
            })
        }
        Eco::Pypi => {
            let segs: Vec<&str> = spec.split('.').filter(|s| !s.is_empty()).collect();
            let top = *segs.first()?;
            let key = same_as::dep_identity_key(&format!("package:pypi:{top}"))?;
            let by_dist = key
                .strip_prefix("pypi:")
                .and_then(|name| by_name.get(&(eco, name)).copied())
                .filter(declared);
            // A distribution imported under another name: the one declared
            // class whose project holds a top-level package `top`.
            let class = by_dist.or_else(|| {
                let mut dirs = pypi.iter().copied().filter(declared).filter(|c| {
                    c.project
                        .as_ref()
                        .is_some_and(|p| modules.has_py_package(p, top))
                });
                let first = dirs.next()?;
                dirs.next().is_none().then_some(first)
            })?;
            Some(Matched {
                class,
                sub: segs.join("/"),
            })
        }
    }
}

/// The module a matched row of `eco` routes to, never `importer`.
fn route(eco: Eco, project: &Project, sub: &str, importer: NodeId, m: &Modules) -> Option<Routed> {
    let single = |module: Option<(NodeId, String)>| {
        module.filter(|t| t.0 != importer).map(|module| Routed {
            scope: vec![module.0],
            module,
        })
    };
    match eco {
        Eco::Npm => single(m.npm_target(project, sub)),
        Eco::Cargo => single(m.cargo_target(project, sub)),
        Eco::Pypi => single(m.pypi_target(project, sub)),
        Eco::Go => {
            let dir = join(&project.root, sub);
            let module = m.go_target(project.repo, &dir, importer)?;
            Some(Routed {
                module,
                scope: m.go_scope(project.repo, &dir, importer),
            })
        }
    }
}

/// What a local name of an importer binds (FA.23).
#[derive(Clone, PartialEq, Eq)]
enum Binding {
    /// A symbol import: the sibling declaration it names.
    Decl(NodeId),
    /// A whole-module import (or a submodule named by a symbol import): the
    /// modules whose top-level declarations `name.f` reaches.
    Scope(Vec<NodeId>),
    /// Two declarations answer the name: binds nothing, counted.
    Ambiguous,
    /// Two rows bind the name to different targets: binds nothing.
    Conflict,
}

/// One importer's bindings: local name -> what it binds, its ecosystem and
/// file. Looked up only, never iterated into an output.
struct Bindings {
    eco: Eco,
    file: String,
    names: HashMap<String, Binding>,
}

impl Bindings {
    fn bind(&mut self, local: String, b: Binding) {
        match self.names.get(&local) {
            Some(old) if *old != b => {
                self.names.insert(local, Binding::Conflict);
            }
            Some(_) => {}
            None => {
                self.names.insert(local, b);
            }
        }
    }
}

/// A declaration lookup's outcome.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lookup {
    Found(NodeId),
    Ambiguous,
    Missing,
}

/// The top-level declarations of the target repos' modules and the TS
/// barrels' re-export edges (DT.1q), read from persisted structure only.
struct Decls<'a> {
    /// module -> (name -> declaration).
    symbols: HashMap<NodeId, &'a HashMap<String, NodeId>>,
    /// barrel module -> the modules it re-exports from, in edge order.
    reexports: HashMap<NodeId, Vec<NodeId>>,
}

impl<'a> Decls<'a> {
    fn of(graphs: &'a [RepoGraph], repos: &BTreeSet<u64>) -> Self {
        let mut symbols = HashMap::new();
        let mut reexports: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for g in graphs.iter().filter(|g| repos.contains(&g.repo.0)) {
            for (m, names) in &g.symbols.module_symbols {
                symbols.entry(*m).or_insert(names);
            }
            for e in g
                .edges
                .iter()
                .filter(|e| e.category == edge_category::IMPORTS)
            {
                if Evidence::of(e).and_then(|ev| ev.rule).as_deref() == Some("reexport") {
                    reexports.entry(e.from).or_default().push(e.to);
                }
            }
        }
        Self { symbols, reexports }
    }

    /// The distinct declarations `name` names among `modules`' own
    /// top-level names.
    fn level(&self, modules: &[NodeId], name: &str) -> BTreeSet<u64> {
        modules
            .iter()
            .filter_map(|m| self.symbols.get(m)?.get(name))
            .map(|d| d.0)
            .collect()
    }

    /// The declaration `name` names in `scope`: its modules' own top-level
    /// names, then (npm) the modules they re-export from, one level at a
    /// time up to [`REEXPORT_DEPTH`]; two distinct declarations on the first
    /// level naming any is [`Lookup::Ambiguous`].
    fn lookup(&self, eco: Eco, scope: &[NodeId], name: &str) -> Lookup {
        let mut level: Vec<NodeId> = scope.to_vec();
        let mut seen: HashSet<NodeId> = level.iter().copied().collect();
        for depth in 0..=REEXPORT_DEPTH {
            let found = self.level(&level, name);
            match found.len() {
                0 => {}
                1 => {
                    return found
                        .first()
                        .map_or(Lookup::Missing, |d| Lookup::Found(NodeId(*d)));
                }
                _ => return Lookup::Ambiguous,
            }
            if eco != Eco::Npm || depth == REEXPORT_DEPTH {
                break;
            }
            level = level
                .iter()
                .filter_map(|m| self.reexports.get(m))
                .flatten()
                .copied()
                .filter(|m| seen.insert(*m))
                .collect();
            if level.is_empty() {
                break;
            }
        }
        Lookup::Missing
    }
}

/// The importer MODULE whose bindings a site of `from` reads: the nearest
/// one on its nav parent chain (itself included).
fn importer_of(
    g: &RepoGraph,
    from: NodeId,
    importers: &HashMap<NodeId, Bindings>,
) -> Option<NodeId> {
    let mut cur = from;
    for _ in 0..PARENT_HOPS {
        if importers.contains_key(&cur) {
            return Some(cur);
        }
        cur = *g.nav.parent_of.get(&cur)?;
    }
    None
}

/// What `q` names through an importer's bindings.
fn resolve_site(b: &Bindings, q: &CallQualifier, decls: &Decls) -> Lookup {
    match q {
        CallQualifier::Bare(name) => match b.names.get(name) {
            Some(Binding::Decl(d)) => Lookup::Found(*d),
            Some(Binding::Ambiguous) => Lookup::Ambiguous,
            _ => Lookup::Missing,
        },
        CallQualifier::Attribute { base, name } => match b.names.get(base) {
            Some(Binding::Scope(scope)) => decls.lookup(b.eco, scope, name),
            Some(Binding::Ambiguous) => Lookup::Ambiguous,
            _ => Lookup::Missing,
        },
        _ => Lookup::Missing,
    }
}

/// One call / reference edge to draw: (from, to, category, rule, least line,
/// importer file, ecosystem).
type Bound = (
    NodeId,
    NodeId,
    EdgeCategoryId,
    &'static str,
    u32,
    String,
    Eco,
);

/// The org_import, org_call and org_ref edges of `graphs` given the cross
/// edges so far, and the report behind the markers.
fn org_edges(graphs: &[RepoGraph], cross: &[Edge]) -> (Vec<Edge>, Report) {
    let mut report = Report::default();
    let (classes, total) = package_classes(graphs, cross);
    report.classes = total;
    if classes.is_empty() {
        return (Vec::new(), report);
    }
    let project_repos: BTreeSet<u64> = classes
        .iter()
        .filter_map(|c| c.project.as_ref().map(|p| p.repo.0))
        .collect();
    let modules = Modules::of(graphs, &project_repos);
    let decls = Decls::of(graphs, &project_repos);
    // (ecosystem, package name) -> class: a manifest_name key is one class,
    // so a name names one class per ecosystem. Lookup only.
    let by_name: HashMap<(Eco, &str), &PackageIndex> = classes
        .iter()
        .map(|c| ((c.eco, c.name.as_str()), c))
        .collect();
    let pypi: Vec<&PackageIndex> = classes.iter().filter(|c| c.eco == Eco::Pypi).collect();
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
    let mut importers: HashMap<NodeId, Bindings> = HashMap::new();
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
            let mut bindings = Bindings {
                eco,
                file: file.clone(),
                names: HashMap::new(),
            };
            let mut counted = false;
            for row in read_import_bindings(cell) {
                let Some(m) = match_row(eco, &row.spec, n.repo.0, &by_name, &pypi, &modules) else {
                    continue;
                };
                counted = true;
                report.rows += 1;
                let routed = m
                    .class
                    .project
                    .as_ref()
                    .and_then(|p| route(eco, p, &m.sub, n.id, &modules));
                let Some(routed) = routed else {
                    report.unrouted += 1;
                    continue;
                };
                report.routed += 1;
                report.routed_by[eco.slot()] += 1;
                if let Some(project) = &m.class.project {
                    let cx = RowCtx {
                        eco,
                        project,
                        sub: &m.sub,
                        importer: n.id,
                    };
                    bind_row(&mut bindings, &cx, &row, &routed, &modules, &decls);
                }
                let (target, target_file) = routed.module;
                if existing.contains(&(n.id, target)) {
                    continue;
                }
                routes
                    .entry((gi, ni, target_file))
                    .and_modify(|r| r.2 = r.2.min(row.line))
                    .or_insert((n.id, target, row.line, file.clone()));
            }
            report.importers += usize::from(counted);
            if !bindings.names.is_empty() {
                importers.insert(n.id, bindings);
            }
        }
    }

    let bound = bind_sites(graphs, cross, &importers, &decls, &mut report);

    let mut tally = RuleTally::new(PASS, &[RULE_ORG_IMPORT, RULE_ORG_CALL, RULE_ORG_REF]);
    let mut edges = Vec::with_capacity(routes.len() + bound.len());
    for (from, to, line, file) in routes.into_values() {
        let ev = tally.evidence(RULE_ORG_IMPORT).at(file, line);
        let mut e = Edge::new(from, to, edge_category::IMPORTS, Confidence::Medium);
        evidence::attach(&mut e, ev);
        edges.push(e);
    }
    tally.add(RULE_ORG_IMPORT, edges.len());
    for (from, to, category, rule, line, file, eco) in bound {
        let ev = tally.evidence(rule).at(file, line);
        let mut e = Edge::new(from, to, category, Confidence::Medium);
        evidence::attach(&mut e, ev);
        edges.push(e);
        tally.add(rule, 1);
        report.bound_by[eco.slot()] += 1;
        if rule == RULE_ORG_CALL {
            report.calls += 1;
        } else {
            report.refs += 1;
        }
    }
    report.tally = Some(tally);
    (edges, report)
}

/// One routed row's context: the importer's ecosystem, the sibling
/// project, the row's routed path and the importer MODULE.
struct RowCtx<'a> {
    eco: Eco,
    project: &'a Project,
    sub: &'a str,
    importer: NodeId,
}

/// Record the local names one routed row binds (FA.23): a symbol import's
/// local name -> the declaration of its imported name in the routed scope
/// (else, cargo / pypi, the submodule it names); a whole-module import's
/// alias, else its language's default binding, -> the routed scope (a
/// Python `import pkg.mod` binds `pkg`, the top package). A TS default or
/// glob import binds nothing.
fn bind_row(
    b: &mut Bindings,
    cx: &RowCtx,
    row: &glia_code_domain::ImportBinding,
    routed: &Routed,
    modules: &Modules,
    decls: &Decls,
) {
    let (eco, project, sub, importer) = (cx.eco, cx.project, cx.sub, cx.importer);
    match &row.name {
        Some(name) if name == "*" || name == "default" => {}
        Some(name) => {
            let local = row.alias.clone().unwrap_or_else(|| name.clone());
            let binding = match decls.lookup(eco, &routed.scope, name) {
                Lookup::Found(d) => Some(Binding::Decl(d)),
                Lookup::Ambiguous => Some(Binding::Ambiguous),
                Lookup::Missing => {
                    let path = join(sub, name);
                    match eco {
                        Eco::Cargo => modules.cargo_target(project, &path),
                        Eco::Pypi => modules.pypi_target(project, &path),
                        Eco::Npm | Eco::Go => None,
                    }
                    .filter(|t| t.0 != importer)
                    .map(|t| Binding::Scope(vec![t.0]))
                }
            };
            if let Some(binding) = binding {
                b.bind(local, binding);
            }
        }
        None => {
            if let Some(alias) = &row.alias {
                b.bind(alias.clone(), Binding::Scope(routed.scope.clone()));
                return;
            }
            let Some(local) = import_default_binding(&row.spec, eco.lang()) else {
                return;
            };
            let scope = match eco {
                // `import pkg.mod` binds `pkg`: the top package's module.
                Eco::Pypi => {
                    let top = sub.split('/').next().unwrap_or(sub);
                    match modules.pypi_target(project, top) {
                        Some(t) if t.0 != importer => vec![t.0],
                        _ => return,
                    }
                }
                _ => routed.scope.clone(),
            };
            b.bind(local, Binding::Scope(scope));
        }
    }
}

/// The org_call / org_ref edges: every unresolved call site and USES /
/// INJECTS / HANDLED_BY reference whose importer bound its name, in graph
/// then site order, one edge per (from, to, category) at its least line,
/// none where an edge of that category already joins the two.
fn bind_sites(
    graphs: &[RepoGraph],
    cross: &[Edge],
    importers: &HashMap<NodeId, Bindings>,
    decls: &Decls,
    report: &mut Report,
) -> Vec<Bound> {
    if importers.is_empty() {
        return Vec::new();
    }
    let importer_repos: BTreeSet<u64> = graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| importers.contains_key(&n.id))
        .map(|n| n.repo.0)
        .collect();
    let wanted = |c: EdgeCategoryId| c == edge_category::CALLS || REF_CATEGORIES.contains(&c);
    let existing: HashSet<(NodeId, NodeId, EdgeCategoryId)> = graphs
        .iter()
        .filter(|g| importer_repos.contains(&g.repo.0))
        .flat_map(|g| &g.edges)
        .chain(cross)
        .filter(|e| wanted(e.category))
        .map(|e| (e.from, e.to, e.category))
        .collect();
    let mut out: Vec<Bound> = Vec::new();
    // (from, to, category) -> index in `out`. Lookup only.
    let mut at: HashMap<(NodeId, NodeId, EdgeCategoryId), usize> = HashMap::new();
    let mut push = |out: &mut Vec<Bound>, bound: Bound| {
        let key = (bound.0, bound.1, bound.2);
        if bound.0 == bound.1 || existing.contains(&key) {
            return;
        }
        match at.get(&key) {
            Some(&i) => out[i].4 = out[i].4.min(bound.4),
            None => {
                at.insert(key, out.len());
                out.push(bound);
            }
        }
    };
    for g in graphs.iter().filter(|g| importer_repos.contains(&g.repo.0)) {
        for c in &g.unresolved_calls {
            let Some(b) = importer_of(g, c.from, importers).and_then(|i| importers.get(&i)) else {
                continue;
            };
            match resolve_site(b, &c.qualifier, decls) {
                Lookup::Found(to) => push(
                    &mut out,
                    (
                        c.from,
                        to,
                        edge_category::CALLS,
                        RULE_ORG_CALL,
                        c.line,
                        b.file.clone(),
                        b.eco,
                    ),
                ),
                Lookup::Ambiguous => report.ambiguous += 1,
                Lookup::Missing => {}
            }
        }
        for r in g
            .unresolved_refs
            .iter()
            .filter(|r| REF_CATEGORIES.contains(&r.category))
        {
            let Some(b) = importer_of(g, r.from_module, importers).and_then(|i| importers.get(&i))
            else {
                continue;
            };
            match resolve_site(b, &r.qualifier, decls) {
                Lookup::Found(to) => push(
                    &mut out,
                    (
                        r.from,
                        to,
                        r.category,
                        RULE_ORG_REF,
                        r.line,
                        b.file.clone(),
                        b.eco,
                    ),
                ),
                Lookup::Ambiguous => report.ambiguous += 1,
                Lookup::Missing => {}
            }
        }
    }
    out
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
            "[org-packages] classes=2 importers=2 rows=5 routed=4 unrouted=1 (npm=3 go=1 cargo=0 pypi=0)"
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
