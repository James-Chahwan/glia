//! The start rules of `resolver:host`: what a host starts, bound to the code
//! it runs (GH.20 manifest binaries, GH.18 host starts).
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
//! GH.18, who starts this. The IaC readers put a start cell on each host
//! resource (GH.15 a Dockerfile's stages on `infra:image:<n>`, GH.16 a compose
//! service's and a k8s container's overrides, GH.17 a Procfile type, a
//! systemd unit, a supervisord program). This pass computes each start's
//! effective argv, reads it with `hosts::normalise_argv` and binds the target
//! it names:
//!
//! - **Effective argv.** An image starts its final stage's ENTRYPOINT ++ CMD
//!   as the cell says (a HEALTHCHECK is never the main process); a compose
//!   service `entrypoint ?? image ENTRYPOINT ++ (command ?? (entrypoint set ?
//!   [] : image CMD))`; a k8s container `command ?? image ENTRYPOINT ++ (args
//!   ?? (command set ? [] : image CMD))`, its image `infra:image:<basename>`;
//!   a process manager's argv as written. A workload that overrides nothing
//!   is no start: it reaches its image's start over INFRA_REFERENCES, a
//!   carry edge. A field the reader refused (a Helm `{{ }}`) and a runtime
//!   value (`$APP`) are `dynamic=`; a workload whose image the repo does not
//!   build is `external_image=`.
//! - **Path mapping.** A script / module path is joined to the working
//!   directory and mapped to the build context through the image's COPY map
//!   (FACT), else it is the one longest path suffix of a walked file under
//!   the build context (DERIVED; two at that length are `ambiguous=`). A
//!   process manager's relative path is joined to its file's directory (a
//!   Procfile: FACT; a systemd / supervisor `WorkingDirectory=` /
//!   `directory=` makes it a suffix), else the one longest suffix in the repo.
//! - **Dispatch by form.** Script -> the file's MODULE (a JavaScript launcher
//!   also tries `.js` / `.mjs` / `.cjs` and a directory's `index.*`);
//!   PyModule / ModuleAttr / CeleryApp / FlaskApp -> the module's file
//!   (`a/b.py`, `a/b/__main__.py` for `-m`, `a/b/__init__.py`); Binary / Jar /
//!   Dll -> the build line that wrote the started path (go: FUNCTION `main` of
//!   the package dir, FACT; cargo: the `[[bin]]` GH.20 bound, else
//!   `src/bin/<n>.rs`, FACT, or `src/main.rs`, DERIVED; dotnet: the published
//!   project's `Main` METHOD or its `Program.cs` MODULE, FACT; gradle / maven:
//!   the `main` of the class named like the start script, else the one `main`
//!   under the build, DERIVED: the build file's mainClass is not recorded),
//!   else the started file itself, else the manifest binary of that name
//!   (a `cli_invoke:<n>` CLI_INVOKES pairing, then GH.20's HANDLED_BY;
//!   DERIVED), else a Go main package whose directory is named like it
//!   (DERIVED); JavaMain -> `main` of the class at `a/b/Main.<ext>`
//!   (DERIVED); Framework -> `framework=`; a package-manager script
//!   (GH.19's) -> `missing=`.
//! - **Writes.** `<INFRA_RESOURCE> -HANDLED_BY-> target`, EVIDENCE emitter
//!   `resolver:host`, rule `<host>:<form>` (`docker:script`,
//!   `compose:module_attr`, `k8s:binary`, `procfile:celery_app`, ..), located
//!   at the start's declaration; Strong for FACT, Medium for DERIVED; and the
//!   target's ENTRYPOINT cell `{"decl":"<file>:<row>","source":"host:<host>",
//!   "host":"<infra qname>","trigger":"<trigger>"}`, first wins (a node that
//!   already carries one keeps it).
//!
//! fired_on, once per build holding a start or a manifest binary:
//! `[host-start] starts=<n> bound=<b> (fact=<f> derived=<d>) unresolved=<u>
//! (dynamic=<y> ambiguous=<a> framework=<w> missing=<m> external_image=<x>
//! refused=<r>) forms=(script= py_module= module_attr= celery_app= flask_app=
//! binary= pkg_script= jar= java_main= dll= framework=) entries=<e>
//! manifest_bins=<bound>/<declared> (cargo=<b>/<n> pyproject=<b>/<n>
//! setup=<b>/<n> npm=<b>/<n> gemspec=<b>/<n> composer=<b>/<n>) attr_fallback=<f>
//! bins_unresolved=<u>` (`refused=`: an inline, too-deep or target-less argv).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::hosts::{Refusal, StartTarget, TargetForm, normalise_argv};
use glia_code_domain::{GRAPH_TYPE, cell_type, edge_category, infra, node_kind};
use glia_code_extractors::cli::parse_manifest_bin_cell;
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId, NodeKindId, RepoId};

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

/// The start rules of `resolver:host` (module doc): GH.20's manifest binaries,
/// then GH.18's host starts, which read the binaries GH.20 bound. Prints the
/// one `[host-start]` line. Runs inside the `resolver:host` pass.
pub(super) fn bind_manifest_bins(merged: &mut MergedGraph) {
    let bins = bind_bins(merged);
    let starts = bind_host_starts(merged, &bins.bound);
    if bins.declared == 0 && starts.starts == 0 {
        return;
    }
    eprintln!("[host-start] {} {}", starts.marker(), bins.marker());
}

/// What [`bind_bins`] did: the marker's counts and each bound binary.
struct BinReport {
    /// `(cli:<bin>, its target)`, in collection order.
    bound: Vec<(NodeId, NodeId)>,
    declared: usize,
    per_form: Vec<String>,
    bound_n: usize,
    fallback: usize,
    unresolved: usize,
}

impl BinReport {
    fn marker(&self) -> String {
        format!(
            "manifest_bins={}/{} ({}) attr_fallback={} bins_unresolved={}",
            self.bound_n,
            self.declared,
            self.per_form.join(" "),
            self.fallback,
            self.unresolved
        )
    }
}

/// GH.20: bind every manifest-declared binary (module doc).
fn bind_bins(merged: &mut MergedGraph) -> BinReport {
    let bins = collect_bins(merged);
    let mut report = BinReport {
        bound: Vec::new(),
        declared: 0,
        per_form: Vec::new(),
        bound_n: 0,
        fallback: 0,
        unresolved: 0,
    };
    if bins.is_empty() {
        return report;
    }
    let files = module_files(merged);
    let mut counts: BTreeMap<&str, (usize, usize)> = FORMS.iter().map(|f| (*f, (0, 0))).collect();
    let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut edges = Vec::new();
    for b in &bins {
        if let Some(slot) = counts.get_mut(b.form.as_str()) {
            slot.1 += 1;
        }
        let empty = BTreeMap::new();
        let repo_files = files.get(&b.repo).unwrap_or(&empty);
        let Some((to, confidence)) = bind(merged, repo_files, b) else {
            report.unresolved += 1;
            continue;
        };
        if confidence == Confidence::Medium {
            report.fallback += 1;
        }
        if let Some(slot) = counts.get_mut(b.form.as_str()) {
            slot.0 += 1;
        }
        report.bound.push((b.from, to));
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
    report.bound_n = bound;
    report.declared = declared;
    report.per_form = FORMS
        .iter()
        .map(|f| {
            let (b, n) = counts.get(f).copied().unwrap_or_default();
            format!("{f}={b}/{n}")
        })
        .collect();
    report
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

// ---------------------------------------------------------------------------
// GH.18: who starts this. The host start cells (GH.15 docker, GH.16 compose /
// k8s, GH.17 Procfile / systemd / supervisor) bound to the code they run.
// ---------------------------------------------------------------------------

/// The unresolved reasons, in marker order.
const REASONS: [&str; 6] = [
    "dynamic",
    "ambiguous",
    "framework",
    "missing",
    "external_image",
    "refused",
];

/// The target forms the marker counts, in marker order.
const MARKER_FORMS: [TargetForm; 11] = [
    TargetForm::Script,
    TargetForm::PyModule,
    TargetForm::ModuleAttr,
    TargetForm::CeleryApp,
    TargetForm::FlaskApp,
    TargetForm::Binary,
    TargetForm::PkgScript,
    TargetForm::Jar,
    TargetForm::JavaMain,
    TargetForm::Dll,
    TargetForm::Framework,
];

/// The kinds a binary's entry may be (a `main` FUNCTION, a `Main` METHOD).
const ENTRY_FN_KINDS: [NodeKindId; 2] = [node_kind::FUNCTION, node_kind::METHOD];

/// The marker's counts of [`bind_host_starts`].
#[derive(Debug, Default)]
struct StartReport {
    starts: usize,
    fact: usize,
    derived: usize,
    unresolved: BTreeMap<&'static str, usize>,
    forms: BTreeMap<&'static str, usize>,
    entries: usize,
}

impl StartReport {
    fn marker(&self) -> String {
        let n = |m: &BTreeMap<&'static str, usize>, k: &str| m.get(k).copied().unwrap_or(0);
        let reasons: Vec<String> = REASONS
            .iter()
            .map(|r| format!("{r}={}", n(&self.unresolved, r)))
            .collect();
        let forms: Vec<String> = MARKER_FORMS
            .iter()
            .map(|f| format!("{}={}", f.as_str(), n(&self.forms, f.as_str())))
            .collect();
        let unresolved: usize = self.unresolved.values().sum();
        format!(
            "starts={} bound={} (fact={} derived={}) unresolved={unresolved} ({}) forms=({}) entries={}",
            self.starts,
            self.fact + self.derived,
            self.fact,
            self.derived,
            reasons.join(" "),
            forms.join(" "),
            self.entries
        )
    }
}

/// A build-output RUN line of a docker start cell.
#[derive(Debug, Clone)]
struct BuildLine {
    tool: String,
    out: String,
    pkg: Option<String>,
    bin: Option<String>,
    proj: Option<String>,
    /// The RUN line's working directory as a build-context path.
    src: Option<String>,
}

/// One docker start cell (GH.15) on an image node.
#[derive(Debug, Clone)]
struct DockerStart {
    image: NodeId,
    repo: RepoId,
    is_final: bool,
    container_start: bool,
    stage: String,
    argv: Vec<String>,
    entrypoint: Vec<String>,
    cmd: Vec<String>,
    workdir: Option<String>,
    context: String,
    copies: Vec<(String, String)>,
    builds: Vec<BuildLine>,
    file: String,
    row: u32,
}

/// One compose service start cell (GH.16).
struct ComposeStart {
    node: NodeId,
    repo: RepoId,
    command: Option<Vec<String>>,
    entrypoint: Option<Vec<String>>,
    workdir: Option<String>,
    image: Option<String>,
    built_image: Option<String>,
    context: Option<String>,
    dockerfile: Option<String>,
    target_stage: Option<String>,
    refused: Vec<String>,
    file: String,
    row: u32,
}

/// One k8s container start cell (GH.16).
struct K8sStart {
    node: NodeId,
    repo: RepoId,
    trigger: String,
    image: Option<String>,
    command: Option<Vec<String>>,
    args: Option<Vec<String>>,
    workdir: Option<String>,
    refused: Vec<String>,
    file: String,
    row: u32,
}

/// One process-manager start cell (GH.17).
struct ProcStart {
    node: NodeId,
    repo: RepoId,
    host: &'static str,
    trigger: String,
    argv: Vec<String>,
    workdir: Option<String>,
    dir: String,
    file: String,
    row: u32,
}

/// A `{"build":{..}}` cell (GH.16: skaffold, or compose `image:` + `build:`)
/// on an image node: the context its Dockerfile is built from.
struct ImageBuild {
    image: NodeId,
    dockerfile: Option<String>,
    context: String,
}

/// Every start cell of the build, in graph then node then cell order.
#[derive(Default)]
struct StartCells {
    docker: Vec<DockerStart>,
    compose: Vec<ComposeStart>,
    k8s: Vec<K8sStart>,
    procs: Vec<ProcStart>,
    builds: Vec<ImageBuild>,
}

/// Where a launch runs: in an image built in the repo, or on a machine
/// from a declaring file's directory.
enum Env<'a> {
    Container {
        image: &'a DockerStart,
        context: String,
        workdir: Option<String>,
    },
    Local {
        dir: String,
        workdir: Option<String>,
        /// A relative target joined to the declaring file's directory is
        /// FACT (a Procfile's processes run in its directory), else DERIVED.
        dir_is_fact: bool,
    },
}

/// One start to bind: the effective argv and where it runs.
struct Launch<'a> {
    host: &'static str,
    from: NodeId,
    repo: RepoId,
    trigger: String,
    argv: Vec<String>,
    file: String,
    row: u32,
    env: Env<'a>,
}

/// A resolved target: its node and whether the binding is FACT.
type Bound = (NodeId, bool);

/// A located symbol of one repo, for the binary entry lookups.
struct Sym {
    file: String,
    kind: NodeKindId,
    name: String,
    parent_name: Option<String>,
    qname: String,
    id: NodeId,
}

/// One repo's lookups, built once.
#[derive(Default)]
struct RepoIndex {
    /// file -> its least-qname MODULE.
    modules: BTreeMap<String, NodeId>,
    /// FUNCTION / METHOD nodes, in graph then node order.
    syms: Vec<Sym>,
    /// `(cli_invoke:<name>, id)` CLI_INVOCATION nodes, in node order.
    invocations: Vec<(String, NodeId)>,
    /// Manifest binaries GH.20 bound: `(name, form, manifest dir, target)`.
    bins: Vec<(String, String, String, NodeId)>,
}

/// GH.18: bind every host start to the code it runs (module doc) and mark
/// the target an entry. `bins` are GH.20's bound manifest binaries.
fn bind_host_starts(merged: &mut MergedGraph, bins: &[(NodeId, NodeId)]) -> StartReport {
    let cells = collect_starts(merged);
    let mut report = StartReport::default();
    if cells.docker.is_empty()
        && cells.compose.is_empty()
        && cells.k8s.is_empty()
        && cells.procs.is_empty()
    {
        return report;
    }
    let index = repo_index(merged, bins);
    let adjacency = adjacency(merged);
    let launches = launches(&cells);
    let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut edges = Vec::new();
    let mut entries: Vec<(NodeId, String)> = Vec::new();
    let empty = RepoIndex::default();
    for (launch, verdict) in launches {
        report.starts += 1;
        let ix = index.get(&launch.repo).unwrap_or(&empty);
        let (form, outcome) = match verdict {
            Err(reason) => (None, Err(reason)),
            Ok(()) => resolve_launch(ix, &adjacency, &launch),
        };
        if let Some(f) = form {
            *report.forms.entry(f.as_str()).or_default() += 1;
        }
        let (to, fact) = match outcome {
            Ok(b) => b,
            Err(reason) => {
                *report.unresolved.entry(reason).or_default() += 1;
                continue;
            }
        };
        if fact {
            report.fact += 1;
        } else {
            report.derived += 1;
        }
        let form = form.map_or("binary", TargetForm::as_str);
        if seen.insert((launch.from, to)) {
            let confidence = if fact {
                Confidence::Strong
            } else {
                Confidence::Medium
            };
            let mut e = Edge::new(launch.from, to, edge_category::HANDLED_BY, confidence);
            evidence::attach(
                &mut e,
                Evidence::emitter("resolver:host")
                    .rule(format!("{}:{form}", launch.host))
                    .at(launch.file.clone(), launch.row.saturating_sub(1)),
            );
            edges.push(e);
        }
        let host_qname = qname_of(merged, launch.from);
        let payload = format!(
            r#"{{"decl":{},"source":{},"host":{},"trigger":{}}}"#,
            json_string(&format!("{}:{}", launch.file, launch.row)),
            json_string(&format!("host:{}", launch.host)),
            json_string(&host_qname),
            json_string(&launch.trigger),
        );
        entries.push((to, payload));
    }
    merged.cross_edges.extend(edges);
    report.entries = write_entries(merged, &entries);
    report
}

/// Every start the cells declare, with its effective argv, and a verdict:
/// `Err(reason)` when it is unresolved before any lookup (a refused field,
/// an image the repo does not build). A workload that overrides nothing is
/// no start: it reaches its image's start through INFRA_REFERENCES.
fn launches(cells: &StartCells) -> Vec<(Launch<'_>, Result<(), &'static str>)> {
    let mut out = Vec::new();
    let context_of = |d: &DockerStart| {
        cells
            .builds
            .iter()
            .find(|b| b.image == d.image && b.dockerfile.as_deref() == Some(d.file.as_str()))
            .map_or_else(|| d.context.clone(), |b| b.context.clone())
    };
    for d in cells
        .docker
        .iter()
        .filter(|d| d.is_final && d.container_start)
    {
        out.push((
            Launch {
                host: "docker",
                from: d.image,
                repo: d.repo,
                trigger: "container_start".to_string(),
                argv: d.argv.clone(),
                file: d.file.clone(),
                row: d.row,
                env: Env::Container {
                    image: d,
                    context: context_of(d),
                    workdir: d.workdir.clone(),
                },
            },
            Ok(()),
        ));
    }
    let images = |repo: RepoId, name: &str| -> Vec<&DockerStart> {
        let id = image_id(repo, name);
        cells
            .docker
            .iter()
            .filter(|d| d.image == id && d.container_start)
            .collect()
    };
    for c in &cells.compose {
        // A Helm / `{{ }}` field is refused by the reader: an override it
        // refused is a start whose argv is a runtime value.
        let refused_argv = c
            .refused
            .iter()
            .any(|f| matches!(f.as_str(), "command" | "entrypoint" | "working_dir"));
        if c.command.is_none() && c.entrypoint.is_none() && !refused_argv {
            continue;
        }
        let refused = refused_argv
            || c.refused
                .iter()
                .any(|f| matches!(f.as_str(), "build" | "image"));
        let own = || {
            let mut argv = c.entrypoint.clone().unwrap_or_default();
            argv.extend(c.command.clone().unwrap_or_default());
            argv
        };
        let base = |argv: Vec<String>, env| Launch {
            host: "compose",
            from: c.node,
            repo: c.repo,
            trigger: "container_start".to_string(),
            argv,
            file: c.file.clone(),
            row: c.row,
            env,
        };
        if refused {
            out.push((
                base(
                    own(),
                    Env::Local {
                        dir: String::new(),
                        workdir: None,
                        dir_is_fact: false,
                    },
                ),
                Err("dynamic"),
            ));
            continue;
        }
        let name = c
            .built_image
            .clone()
            .or_else(|| c.image.as_deref().map(image_basename));
        let candidates: Vec<&DockerStart> = name
            .as_deref()
            .map(|n| images(c.repo, n))
            .unwrap_or_default()
            .into_iter()
            .filter(|d| c.dockerfile.as_deref().is_none_or(|f| f == d.file))
            .filter(|d| match &c.target_stage {
                Some(stage) => &d.stage == stage,
                None => d.is_final,
            })
            .collect();
        // One start per candidate image start: several only when two
        // Dockerfiles give one image name and the service names neither.
        let Some(d) = pick_one(&candidates) else {
            let verdict = external_verdict(&own());
            out.push((
                base(
                    own(),
                    Env::Local {
                        dir: String::new(),
                        workdir: None,
                        dir_is_fact: false,
                    },
                ),
                Err(verdict),
            ));
            continue;
        };
        let argv = match &c.entrypoint {
            Some(ep) => {
                let mut argv = ep.clone();
                argv.extend(c.command.clone().unwrap_or_default());
                argv
            }
            None => {
                let mut argv = d.entrypoint.clone();
                argv.extend(c.command.clone().unwrap_or_else(|| d.cmd.clone()));
                argv
            }
        };
        let env = Env::Container {
            image: d,
            context: c.context.clone().unwrap_or_else(|| context_of(d)),
            workdir: c.workdir.clone().or_else(|| d.workdir.clone()),
        };
        out.push((base(argv, env), Ok(())));
    }
    for k in &cells.k8s {
        let refused_argv = k
            .refused
            .iter()
            .any(|f| matches!(f.as_str(), "command" | "args" | "workingDir"));
        if k.command.is_none() && k.args.is_none() && !refused_argv {
            continue;
        }
        let refused = refused_argv || k.refused.iter().any(|f| f == "image");
        let own = || {
            let mut argv = k.command.clone().unwrap_or_default();
            argv.extend(k.args.clone().unwrap_or_default());
            argv
        };
        let base = |argv: Vec<String>, env| Launch {
            host: "k8s",
            from: k.node,
            repo: k.repo,
            trigger: k.trigger.clone(),
            argv,
            file: k.file.clone(),
            row: k.row,
            env,
        };
        let local = || Env::Local {
            dir: String::new(),
            workdir: None,
            dir_is_fact: false,
        };
        if refused {
            out.push((base(own(), local()), Err("dynamic")));
            continue;
        }
        let candidates: Vec<&DockerStart> = k
            .image
            .as_deref()
            .map(|i| images(k.repo, &image_basename(i)))
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.is_final)
            .collect();
        let Some(d) = pick_one(&candidates) else {
            let verdict = external_verdict(&own());
            out.push((base(own(), local()), Err(verdict)));
            continue;
        };
        let argv = match &k.command {
            Some(cmd) => {
                let mut argv = cmd.clone();
                argv.extend(k.args.clone().unwrap_or_default());
                argv
            }
            None => {
                let mut argv = d.entrypoint.clone();
                argv.extend(k.args.clone().unwrap_or_else(|| d.cmd.clone()));
                argv
            }
        };
        let env = Env::Container {
            image: d,
            context: context_of(d),
            workdir: k.workdir.clone().or_else(|| d.workdir.clone()),
        };
        out.push((base(argv, env), Ok(())));
    }
    for p in &cells.procs {
        out.push((
            Launch {
                host: p.host,
                from: p.node,
                repo: p.repo,
                trigger: p.trigger.clone(),
                argv: p.argv.clone(),
                file: p.file.clone(),
                row: p.row,
                env: Env::Local {
                    dir: p.dir.clone(),
                    workdir: p.workdir.clone(),
                    dir_is_fact: p.host == "procfile",
                },
            },
            Ok(()),
        ));
    }
    out
}

/// The one image start a workload runs: the first when several Dockerfiles
/// of one image name agree on the argv, `None` when there is none or they
/// disagree (the workload's own argv is then all there is).
fn pick_one<'a>(candidates: &[&'a DockerStart]) -> Option<&'a DockerStart> {
    let first = *candidates.first()?;
    candidates
        .iter()
        .all(|d| d.argv == first.argv && d.context == first.context)
        .then_some(first)
}

/// Why a workload whose image the repo does not build binds nothing: its
/// own argv is a runtime value, or the image is external.
fn external_verdict(argv: &[String]) -> &'static str {
    let t = normalise_argv(argv);
    if t.refused == Some(Refusal::Dynamic) {
        "dynamic"
    } else {
        "external_image"
    }
}

/// Resolve one launch: the normalised form and the target or the reason.
fn resolve_launch(
    ix: &RepoIndex,
    adjacency: &HashMap<NodeId, Vec<(NodeId, glia_core::EdgeCategoryId)>>,
    l: &Launch<'_>,
) -> (Option<TargetForm>, Result<Bound, &'static str>) {
    let t = normalise_argv(&l.argv);
    if let Some(why) = t.refused {
        let reason = if why == Refusal::Dynamic {
            "dynamic"
        } else {
            "refused"
        };
        return (Some(t.form), Err(reason));
    }
    let outcome = match t.form {
        TargetForm::Script => map_module(ix, &l.env, &script_candidates(&t)),
        TargetForm::PyModule
        | TargetForm::ModuleAttr
        | TargetForm::CeleryApp
        | TargetForm::FlaskApp => map_module(ix, &l.env, &python_candidates(&t)),
        TargetForm::Binary | TargetForm::Jar | TargetForm::Dll | TargetForm::JavaMain => {
            binary(ix, adjacency, l, &t)
        }
        TargetForm::Framework => Err("framework"),
        _ => Err("missing"),
    };
    (Some(t.form), outcome)
}

/// The files a script start may name: as written, and for a JavaScript
/// launcher an extensionless path's `.js` / `.mjs` / `.cjs` file or its
/// directory's index.
fn script_candidates(t: &StartTarget) -> Vec<String> {
    let target = t.target.trim_end_matches('/').to_string();
    let base = target.rsplit('/').next().unwrap_or(&target);
    let has_ext = base
        .rsplit_once('.')
        .is_some_and(|(stem, _)| !stem.is_empty());
    if t.lang != Some("typescript") || has_ext {
        return vec![target];
    }
    let mut out = Vec::new();
    for ext in ["js", "mjs", "cjs"] {
        out.push(format!("{target}.{ext}"));
    }
    for ext in ["js", "mjs", "cjs"] {
        out.push(format!("{target}/index.{ext}"));
    }
    out
}

/// The files a Python module start may name: `a.b` is `a/b.py`, then (for
/// `python -m`) `a/b/__main__.py`, then the package `a/b/__init__.py`. A
/// target written as a `.py` path is that file.
fn python_candidates(t: &StartTarget) -> Vec<String> {
    let target = t.target.trim();
    if target.ends_with(".py") || target.contains('/') {
        return vec![target.to_string()];
    }
    let path = target.replace('.', "/");
    let mut out = vec![format!("{path}.py")];
    if t.form == TargetForm::PyModule {
        out.push(format!("{path}/__main__.py"));
    }
    out.push(format!("{path}/__init__.py"));
    out
}

/// A container path, absolute and cleaned: `cand` itself when absolute,
/// else joined to the working directory (`None` when that is unknown).
fn container_path(cand: &str, workdir: Option<&str>) -> Option<String> {
    let joined = if cand.starts_with('/') {
        cand.to_string()
    } else {
        format!("{}/{cand}", workdir?)
    };
    Some(format!("/{}", normalise("", &joined).unwrap_or_default()))
}

/// The build-context path a container path was copied from: the last COPY
/// (instruction order) whose destination holds it.
fn copied_from(copies: &[(String, String)], path: &str) -> Option<String> {
    copies.iter().rev().find_map(|(dst, src)| {
        let dst = format!("/{}", normalise("", dst).unwrap_or_default());
        let rest = if path == dst {
            ""
        } else if dst == "/" {
            path.strip_prefix('/')?
        } else {
            path.strip_prefix(&dst)?.strip_prefix('/')?
        };
        Some(join_rel(src, rest).unwrap_or_else(|| ".".to_string()))
    })
}

/// `base` joined with `path`, both cleaned (`.` segments dropped, `..`
/// popped); `None` when it climbs above the root or is empty.
fn join_rel(base: &str, path: &str) -> Option<String> {
    normalise("", &format!("{base}/{path}"))
}

/// The directory part of a repo-relative path (`""` at the root).
fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The last path segment.
fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// Whether repo-relative `file` sits under `dir` (`""` or `.` is the root).
fn under(file: &str, dir: &str) -> bool {
    dir.is_empty()
        || dir == "."
        || file == dir
        || file.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))
}

/// The MODULE a launch's file candidates name: through the image's COPY map
/// (FACT), the declaring file's directory (a Procfile: FACT), else the one
/// longest path suffix under the build context (container) or the repo
/// (DERIVED); two files at that length are `ambiguous`.
fn map_module(ix: &RepoIndex, env: &Env<'_>, cands: &[String]) -> Result<Bound, &'static str> {
    match env {
        Env::Container {
            image,
            context,
            workdir,
        } => {
            for c in cands {
                let Some(path) = container_path(c, workdir.as_deref()) else {
                    continue;
                };
                if let Some(rel) = copied_from(&image.copies, &path)
                    && let Some(file) = join_rel(context, &rel)
                    && let Some(id) = ix.modules.get(&file)
                {
                    return Ok((*id, true));
                }
            }
            for c in cands {
                let path = container_path(c, workdir.as_deref()).unwrap_or_else(|| c.clone());
                match suffix_module(ix, &path, context) {
                    Some(Ok(id)) => return Ok((id, false)),
                    Some(Err(())) => return Err("ambiguous"),
                    None => {}
                }
            }
            Err("missing")
        }
        Env::Local {
            dir,
            workdir,
            dir_is_fact,
        } => {
            let base = match workdir.as_deref() {
                Some(w) if w.starts_with('/') => None,
                Some(w) => join_rel(dir, w),
                None => Some(dir.clone()),
            };
            for c in cands.iter().filter(|c| !c.starts_with('/')) {
                if let Some(b) = &base
                    && let Some(file) = join_rel(b, c)
                    && let Some(id) = ix.modules.get(&file)
                {
                    return Ok((*id, *dir_is_fact && workdir.is_none()));
                }
            }
            for c in cands {
                let path = match workdir.as_deref() {
                    Some(w) if w.starts_with('/') && !c.starts_with('/') => format!("{w}/{c}"),
                    _ => c.clone(),
                };
                match suffix_module(ix, &path, "") {
                    Some(Ok(id)) => return Ok((id, false)),
                    Some(Err(())) => return Err("ambiguous"),
                    None => {}
                }
            }
            Err("missing")
        }
    }
}

/// The MODULE whose file ends with the longest suffix of `path` (whole
/// segments) among the walked files under `within`: `Some(Ok)` for exactly
/// one, `Some(Err)` for two or more at that length, `None` for none.
fn suffix_module(ix: &RepoIndex, path: &str, within: &str) -> Option<Result<NodeId, ()>> {
    let clean = normalise("", path)?;
    let segs: Vec<&str> = clean.split('/').collect();
    for k in 0..segs.len() {
        let suffix = segs[k..].join("/");
        let tail = format!("/{suffix}");
        let hits: Vec<NodeId> = ix
            .modules
            .iter()
            .filter(|(f, _)| under(f, within) && (**f == suffix || f.ends_with(&tail)))
            .map(|(_, id)| *id)
            .collect();
        match hits.len() {
            0 => {}
            1 => return Some(Ok(hits[0])),
            _ => return Some(Err(())),
        }
    }
    None
}

/// A binary start: the build line that wrote the started path (go, cargo,
/// dotnet, gradle / maven), else the started file itself, else the
/// manifest binary of that name (through a CLI_INVOKES pairing or the
/// manifest), else a Go main package named like it (DERIVED).
fn binary(
    ix: &RepoIndex,
    adjacency: &HashMap<NodeId, Vec<(NodeId, glia_core::EdgeCategoryId)>>,
    l: &Launch<'_>,
    t: &StartTarget,
) -> Result<Bound, &'static str> {
    let arg0 = t.target.as_str();
    let name = basename(arg0);
    if t.form == TargetForm::JavaMain {
        return java_main_class(ix, arg0);
    }
    let within = match &l.env {
        Env::Container {
            image,
            context,
            workdir,
        } => {
            let pathish = arg0.contains('/') || matches!(t.form, TargetForm::Jar | TargetForm::Dll);
            let path = pathish
                .then(|| container_path(arg0, workdir.as_deref()))
                .flatten();
            for b in &image.builds {
                let out = format!("/{}", normalise("", &b.out).unwrap_or_default());
                let hit = match &path {
                    Some(p) => {
                        p == &out || p.strip_prefix(&out).is_some_and(|r| r.starts_with('/'))
                    }
                    None => basename(&out) == name,
                };
                if hit {
                    return built_entry(ix, context, b, t, path.as_deref().unwrap_or(arg0));
                }
            }
            context.clone()
        }
        Env::Local { .. } => String::new(),
    };
    if arg0.contains('/') {
        match map_module(ix, &l.env, &[arg0.to_string()]) {
            Ok(b) => return Ok(b),
            Err("ambiguous") => return Err("ambiguous"),
            Err(_) => {}
        }
    }
    if matches!(t.form, TargetForm::Jar | TargetForm::Dll) {
        return Err("missing");
    }
    if let Some(r) = paired_bin(ix, adjacency, name) {
        return r;
    }
    go_main_named(ix, name, &within)
}

/// The entry a build line's output starts (`built_entry`'s tool rules).
fn built_entry(
    ix: &RepoIndex,
    context: &str,
    b: &BuildLine,
    t: &StartTarget,
    started: &str,
) -> Result<Bound, &'static str> {
    let Some(src) = b.src.as_deref().and_then(|s| join_rel(context, s)) else {
        return Err("missing");
    };
    let name = basename(started);
    match b.tool.as_str() {
        "go" => {
            let pkg = b.pkg.as_deref().unwrap_or(".");
            let mut dir = join_rel(&src, pkg).unwrap_or_default();
            if dir.ends_with(".go") {
                dir = dir_of(&dir).to_string();
            }
            least(ix.syms.iter().filter(|s| {
                s.kind == node_kind::FUNCTION
                    && s.name == "main"
                    && s.file.ends_with(".go")
                    && dir_of(&s.file) == dir
            }))
            .map(|id| (id, true))
            .ok_or("missing")
        }
        "cargo" => {
            let bin = b.bin.as_deref().unwrap_or(name);
            if let Some((_, _, _, to)) = ix
                .bins
                .iter()
                .find(|(n, form, dir, _)| n == bin && form == "cargo" && under(dir, &src))
            {
                return Ok((*to, true));
            }
            for (rel, fact) in [
                (format!("src/bin/{bin}.rs"), true),
                (format!("src/bin/{bin}/main.rs"), true),
                ("src/main.rs".to_string(), false),
            ] {
                let Some(file) = join_rel(&src, &rel) else {
                    continue;
                };
                if let Some(id) = least(ix.syms.iter().filter(|s| {
                    s.kind == node_kind::FUNCTION && s.name == "main" && s.file == file
                })) {
                    return Ok((id, fact));
                }
            }
            Err("missing")
        }
        "dotnet" => {
            let proj = b.proj.as_deref().and_then(|p| join_rel(&src, p));
            let dir = proj
                .as_deref()
                .map_or(src.clone(), |p| dir_of(p).to_string());
            if let Some(p) = &proj {
                let stem = basename(p).rsplit_once('.').map_or(basename(p), |(s, _)| s);
                let started = name
                    .strip_suffix(".dll")
                    .or_else(|| name.strip_suffix(".exe"))
                    .unwrap_or(name);
                if !stem.eq_ignore_ascii_case(started) {
                    return Err("missing");
                }
            }
            let mains: Vec<&Sym> = ix
                .syms
                .iter()
                .filter(|s| {
                    s.kind == node_kind::METHOD
                        && s.name == "Main"
                        && s.file.ends_with(".cs")
                        && under(&s.file, &dir)
                })
                .collect();
            match mains.len() {
                1 => Ok((mains[0].id, true)),
                0 => join_rel(&dir, "Program.cs")
                    .and_then(|f| ix.modules.get(&f).copied())
                    .map(|id| (id, true))
                    .ok_or("missing"),
                _ => Err("ambiguous"),
            }
        }
        "gradle" | "maven" => {
            let is_main = |s: &&Sym| {
                ENTRY_FN_KINDS.contains(&s.kind)
                    && s.name == "main"
                    && [".java", ".kt", ".scala", ".groovy"]
                        .iter()
                        .any(|e| s.file.ends_with(e))
                    && under(&s.file, &src)
            };
            let app = if t.form == TargetForm::Jar {
                None
            } else {
                Some(name)
            };
            if let Some(app) = app {
                let named: Vec<&Sym> = ix
                    .syms
                    .iter()
                    .filter(is_main)
                    .filter(|s| s.parent_name.as_deref() == Some(app))
                    .collect();
                if named.len() == 1 {
                    return Ok((named[0].id, false));
                }
            }
            let mains: Vec<&Sym> = ix.syms.iter().filter(is_main).collect();
            match mains.len() {
                1 => Ok((mains[0].id, false)),
                0 => Err("missing"),
                _ => Err("ambiguous"),
            }
        }
        _ => Err("missing"),
    }
}

/// `java -cp .. a.b.Main`: the `main` of class `Main` in the file
/// `a/b/Main.<java|kt|scala>` (one, DERIVED).
fn java_main_class(ix: &RepoIndex, class: &str) -> Result<Bound, &'static str> {
    let class = class.rsplit('/').next().unwrap_or(class);
    let simple = class.rsplit('.').next().unwrap_or(class);
    let path = class.replace('.', "/");
    let hits: Vec<&Sym> = ix
        .syms
        .iter()
        .filter(|s| {
            ENTRY_FN_KINDS.contains(&s.kind)
                && s.name == "main"
                && s.parent_name.as_deref() == Some(simple)
                && ["java", "kt", "scala"].iter().any(|e| {
                    let f = format!("{path}.{e}");
                    s.file == f || s.file.ends_with(&format!("/{f}"))
                })
        })
        .collect();
    match hits.len() {
        1 => Ok((hits[0].id, false)),
        0 => Err("missing"),
        _ => Err("ambiguous"),
    }
}

/// A binary by name: the CLI_COMMAND a `cli_invoke:<name>` invocation is
/// paired with (CLI_INVOKES), else a manifest binary of that name, and what
/// that command is HANDLED_BY (GH.20). DERIVED: the pairing is by name.
fn paired_bin(
    ix: &RepoIndex,
    adjacency: &HashMap<NodeId, Vec<(NodeId, glia_core::EdgeCategoryId)>>,
    name: &str,
) -> Option<Result<Bound, &'static str>> {
    let qname = format!("cli_invoke:{name}");
    let mut targets: Vec<NodeId> = Vec::new();
    let next = |from: NodeId, cat| -> Vec<NodeId> {
        adjacency
            .get(&from)
            .map(|v| {
                v.iter()
                    .filter(|(_, c)| *c == cat)
                    .map(|(t, _)| *t)
                    .collect()
            })
            .unwrap_or_default()
    };
    for (_, inv) in ix.invocations.iter().filter(|(q, _)| *q == qname) {
        for cli in next(*inv, edge_category::CLI_INVOKES) {
            for to in next(cli, edge_category::HANDLED_BY) {
                if !targets.contains(&to) {
                    targets.push(to);
                }
            }
        }
    }
    if targets.is_empty() {
        for (_, _, _, to) in ix.bins.iter().filter(|(n, ..)| n == name) {
            if !targets.contains(to) {
                targets.push(*to);
            }
        }
    }
    match targets.len() {
        0 => None,
        1 => Some(Ok((targets[0], false))),
        _ => Some(Err("ambiguous")),
    }
}

/// A Go `main` FUNCTION whose package directory is named like the binary,
/// under the build context when one is, else anywhere in the repo (DERIVED).
fn go_main_named(ix: &RepoIndex, name: &str, within: &str) -> Result<Bound, &'static str> {
    let mains = |scope: &str| -> Vec<&Sym> {
        ix.syms
            .iter()
            .filter(|s| {
                s.kind == node_kind::FUNCTION
                    && s.name == "main"
                    && s.file.ends_with(".go")
                    && basename(dir_of(&s.file)) == name
                    && under(&s.file, scope)
            })
            .collect()
    };
    let mut hits = mains(within);
    if hits.is_empty() && !within.is_empty() {
        hits = mains("");
    }
    // One package may hold several mains behind build tags: one directory.
    let dirs: BTreeSet<&str> = hits.iter().map(|s| dir_of(&s.file)).collect();
    match dirs.len() {
        0 => Err("missing"),
        1 => least(hits.into_iter())
            .map(|id| (id, false))
            .ok_or("missing"),
        _ => Err("ambiguous"),
    }
}

/// The least-qname symbol's id.
fn least<'a>(syms: impl Iterator<Item = &'a Sym>) -> Option<NodeId> {
    syms.min_by(|a, b| a.qname.cmp(&b.qname)).map(|s| s.id)
}

/// The id of `infra:image:<name>` in `repo`.
fn image_id(repo: RepoId, name: &str) -> NodeId {
    NodeId::from_parts(
        GRAPH_TYPE,
        repo,
        node_kind::INFRA_RESOURCE,
        &infra::qname("image", name),
    )
}

/// The image name a reference names, as the IaC reader keys
/// `infra:image:<name>` (`registry/x/svc:1.0` is `svc`).
fn image_basename(image: &str) -> String {
    let no_tag = image.split(':').next().unwrap_or(image);
    no_tag.rsplit('/').next().unwrap_or(no_tag).to_string()
}

/// `id`'s qname from the first graph naming it.
fn qname_of(merged: &MergedGraph, id: NodeId) -> String {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// Every start and build cell on an INFRA_RESOURCE, in graph, node and cell
/// order.
fn collect_starts(merged: &MergedGraph) -> StartCells {
    let mut out = StartCells::default();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::INFRA_RESOURCE) {
                continue;
            }
            for c in &n.cells {
                if c.kind != cell_type::CODE {
                    continue;
                }
                let CellPayload::Json(text) = &c.payload else {
                    continue;
                };
                if !text.starts_with(r#"{"start":"#) && !text.starts_with(r#"{"build":"#) {
                    continue;
                }
                let Some(v) = Json::parse(text) else {
                    continue;
                };
                if let Some(b) = v.get("build") {
                    if let Some(context) = b.str_at("context") {
                        out.builds.push(ImageBuild {
                            image: n.id,
                            dockerfile: b.str_at("dockerfile").map(str::to_string),
                            context: context.to_string(),
                        });
                    }
                    continue;
                }
                let Some(s) = v.get("start") else {
                    continue;
                };
                let Some((file, row)) = s.str_at("decl").and_then(split_decl) else {
                    continue;
                };
                let (node, repo) = (n.id, g.repo);
                match s.str_at("host") {
                    Some("docker") => out.docker.push(DockerStart {
                        image: node,
                        repo,
                        is_final: s.get("final").and_then(Json::as_bool).unwrap_or(false),
                        container_start: s.str_at("trigger") == Some("container_start"),
                        stage: s.str_at("stage").unwrap_or_default().to_string(),
                        argv: s.strs_at("argv").unwrap_or_default(),
                        entrypoint: s.strs_at("entrypoint").unwrap_or_default(),
                        cmd: s.strs_at("cmd").unwrap_or_default(),
                        workdir: s.str_at("workdir").map(str::to_string),
                        context: s.str_at("context").unwrap_or(".").to_string(),
                        copies: s
                            .get("copies")
                            .and_then(Json::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|p| {
                                        let p = p.as_array()?;
                                        Some((
                                            p.first()?.as_str()?.to_string(),
                                            p.get(1)?.as_str()?.to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        builds: s
                            .get("builds")
                            .and_then(Json::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|b| {
                                        Some(BuildLine {
                                            tool: b.str_at("tool")?.to_string(),
                                            out: b.str_at("out")?.to_string(),
                                            pkg: b.str_at("pkg").map(str::to_string),
                                            bin: b.str_at("bin").map(str::to_string),
                                            proj: b.str_at("proj").map(str::to_string),
                                            src: b.str_at("src").map(str::to_string),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        file,
                        row,
                    }),
                    Some("compose") => out.compose.push(ComposeStart {
                        node,
                        repo,
                        command: s.strs_at("command"),
                        entrypoint: s.strs_at("entrypoint"),
                        workdir: s.str_at("workdir").map(str::to_string),
                        image: s.str_at("image").map(str::to_string),
                        built_image: s.str_at("built_image").map(str::to_string),
                        context: s.str_at("context").map(str::to_string),
                        dockerfile: s.str_at("dockerfile").map(str::to_string),
                        target_stage: s.str_at("target_stage").map(str::to_string),
                        refused: s.strs_at("refused").unwrap_or_default(),
                        file,
                        row,
                    }),
                    Some("k8s") => out.k8s.push(K8sStart {
                        node,
                        repo,
                        trigger: s.str_at("trigger").unwrap_or("container_start").to_string(),
                        image: s.str_at("image").map(str::to_string),
                        command: s.strs_at("command"),
                        args: s.strs_at("args"),
                        workdir: s.str_at("workdir").map(str::to_string),
                        refused: s.strs_at("refused").unwrap_or_default(),
                        file,
                        row,
                    }),
                    Some(h @ ("procfile" | "systemd" | "supervisor")) => {
                        out.procs.push(ProcStart {
                            node,
                            repo,
                            host: match h {
                                "procfile" => "procfile",
                                "systemd" => "systemd",
                                _ => "supervisor",
                            },
                            trigger: s.str_at("trigger").unwrap_or_default().to_string(),
                            argv: s.strs_at("argv").unwrap_or_default(),
                            workdir: s.str_at("workdir").map(str::to_string),
                            dir: s.str_at("dir").unwrap_or(".").to_string(),
                            file,
                            row,
                        })
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// `"<file>:<1-based row>"` split.
fn split_decl(decl: &str) -> Option<(String, u32)> {
    let (file, row) = decl.rsplit_once(':')?;
    Some((file.to_string(), row.parse().ok()?))
}

/// Each repo's lookups ([`RepoIndex`]).
fn repo_index(merged: &MergedGraph, bins: &[(NodeId, NodeId)]) -> HashMap<RepoId, RepoIndex> {
    let mut out: HashMap<RepoId, RepoIndex> = HashMap::new();
    for (repo, files) in module_files(merged) {
        out.entry(repo).or_default().modules =
            files.into_iter().map(|(f, (_, m))| (f, m.id)).collect();
    }
    for g in &merged.graphs {
        let ix = out.entry(g.repo).or_default();
        for n in &g.nodes {
            let Some(kind) = g.nav.kind_by_id.get(&n.id).copied() else {
                continue;
            };
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            if kind == node_kind::CLI_INVOCATION {
                ix.invocations.push((qname, n.id));
                continue;
            }
            if !ENTRY_FN_KINDS.contains(&kind) {
                continue;
            }
            let Some((file, _)) = evidence::locate(&n.cells) else {
                continue;
            };
            let parent_name = g
                .nav
                .parent_of
                .get(&n.id)
                .and_then(|p| g.nav.name_by_id.get(p))
                .cloned();
            ix.syms.push(Sym {
                file,
                kind,
                name: g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default(),
                parent_name,
                qname,
                id: n.id,
            });
        }
    }
    for b in collect_bins(merged) {
        let Some((_, to)) = bins.iter().find(|(from, _)| *from == b.from) else {
            continue;
        };
        let name = qname_of(merged, b.from);
        let name = name.strip_prefix("cli:").unwrap_or(&name).to_string();
        out.entry(b.repo).or_default().bins.push((
            name,
            b.form.clone(),
            dir_of(&b.file).to_string(),
            *to,
        ));
    }
    out
}

/// HANDLED_BY and CLI_INVOKES edges by source, intra edges then cross edges
/// in order (looked up, never iterated into an output).
fn adjacency(merged: &MergedGraph) -> HashMap<NodeId, Vec<(NodeId, glia_core::EdgeCategoryId)>> {
    let mut out: HashMap<NodeId, Vec<(NodeId, glia_core::EdgeCategoryId)>> = HashMap::new();
    let edges = merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(merged.cross_edges.iter());
    for e in edges {
        if e.category == edge_category::HANDLED_BY || e.category == edge_category::CLI_INVOKES {
            out.entry(e.from).or_default().push((e.to, e.category));
        }
    }
    out
}

/// Write each target's ENTRYPOINT cell on every instance of it, first wins:
/// a node that already carries one keeps it. Returns the targets marked.
fn write_entries(merged: &mut MergedGraph, entries: &[(NodeId, String)]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let mut at: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
    let mut declared: HashSet<NodeId> = HashSet::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            at.entry(n.id).or_default().push((gi, ni));
            if n.cells.iter().any(|c| c.kind == cell_type::ENTRYPOINT) {
                declared.insert(n.id);
            }
        }
    }
    let mut marked = 0;
    for (id, payload) in entries {
        if !declared.insert(*id) {
            continue;
        }
        for &(gi, ni) in at.get(id).map_or(&[][..], Vec::as_slice) {
            if let Some(n) = merged.graphs.get_mut(gi).and_then(|g| g.nodes.get_mut(ni)) {
                n.cells.push(Cell {
                    kind: cell_type::ENTRYPOINT,
                    payload: CellPayload::Json(payload.clone()),
                });
            }
        }
        marked += 1;
    }
    marked
}

/// A JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The JSON a start cell holds. The graph crate has no serde_json, and the
/// start cells are the IaC reader's serde_json output: this reads exactly
/// JSON (RFC 8259), nothing looser.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num,
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn parse(text: &str) -> Option<Json> {
        let mut p = JsonParser {
            b: text.as_bytes(),
            i: 0,
        };
        let v = p.value(0)?;
        p.ws();
        (p.i == p.b.len()).then_some(v)
    }

    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// The string at `key` (`None` when absent, null or not a string).
    fn str_at(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    /// The string list at `key`: `None` when absent or null (unset), the
    /// strings of an array otherwise (`[]` = cleared).
    fn strs_at(&self, key: &str) -> Option<Vec<String>> {
        let a = self.get(key)?.as_array()?;
        Some(
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        )
    }
}

/// The deepest nesting [`Json::parse`] reads.
const JSON_DEPTH: usize = 64;

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.b.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn literal(&mut self, word: &[u8], v: Json) -> Option<Json> {
        self.b
            .get(self.i..self.i + word.len())
            .filter(|w| *w == word)?;
        self.i += word.len();
        Some(v)
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > JSON_DEPTH {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    if self.eat(b',').is_some() {
                        continue;
                    }
                    self.eat(b'}')?;
                    return Some(Json::Obj(m));
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    if self.eat(b',').is_some() {
                        continue;
                    }
                    self.eat(b']')?;
                    return Some(Json::Arr(a));
                }
            }
            b'"' => self.string().map(Json::Str),
            b'n' => self.literal(b"null", Json::Null),
            b't' => self.literal(b"true", Json::Bool(true)),
            b'f' => self.literal(b"false", Json::Bool(false)),
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                while self.i < self.b.len()
                    && matches!(
                        self.b[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                (self.i > start).then_some(Json::Num)
            }
            _ => None,
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return None;
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            char::from_u32(code)?
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c if c < 0x20 => return None,
                c => out.push(c),
            }
        }
    }
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

    // -----------------------------------------------------------------
    // GH.18: host starts.
    // -----------------------------------------------------------------

    use glia_code_extractors::iac::{self, IacNodes, ProcessManager};

    impl G {
        /// A FUNCTION / METHOD / CLASS located in `file`.
        fn at(&mut self, kind: NodeKindId, parent: NodeId, qname: &str, file: &str) -> NodeId {
            let pos = Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    r#"{{"file":"{file}","start_line":0,"end_line":0}}"#
                )),
            };
            self.node(kind, qname, Some(parent), vec![pos])
        }
    }

    /// One IaC reader's output as a graph of `repo`.
    fn iac_graph(repo: RepoId, out: IacNodes) -> RepoGraph {
        RepoGraph {
            repo,
            nodes: out.nodes,
            edges: out.edges,
            symbols: SymbolTable::default(),
            nav: out.nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    fn module_id(repo: RepoId, file: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, file)
    }

    fn docker(repo: RepoId, path: &str, src: &str) -> RepoGraph {
        iac_graph(
            repo,
            iac::extract_dockerfile(src, path, module_id(repo, path), repo),
        )
    }

    fn yaml(repo: RepoId, path: &str, src: &str) -> RepoGraph {
        iac_graph(
            repo,
            iac::extract_yaml(src, path, module_id(repo, path), repo),
        )
    }

    fn proc_file(repo: RepoId, kind: ProcessManager, path: &str, src: &str) -> RepoGraph {
        let out = iac::extract_process_manager(kind, src, path, module_id(repo, path), repo);
        iac_graph(repo, out.iac)
    }

    /// `(from, to, confidence, rule)` of every `resolver:host` start edge.
    fn starts(merged: &MergedGraph) -> Vec<(String, String, Confidence, String)> {
        let mut out: Vec<_> = merged
            .cross_edges
            .iter()
            .filter_map(|e| {
                let ev = Evidence::of(e)?;
                let rule = ev.rule?;
                (rule != RULE).then(|| (q(merged, e.from), q(merged, e.to), e.confidence, rule))
            })
            .collect();
        out.sort_by(|a, b| (&a.0, &a.1, &a.3).cmp(&(&b.0, &b.1, &b.3)));
        out
    }

    fn q(merged: &MergedGraph, id: NodeId) -> String {
        qname_of(merged, id)
    }

    fn start(
        from: &str,
        to: &str,
        c: Confidence,
        rule: &str,
    ) -> (String, String, Confidence, String) {
        (from.into(), to.into(), c, rule.into())
    }

    /// The ENTRYPOINT payloads of the node named `qname`.
    fn entry_cells(merged: &MergedGraph, qname: &str) -> Vec<String> {
        let mut out = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.qname_by_id.get(&n.id).map(String::as_str) != Some(qname) {
                    continue;
                }
                for c in n.cells.iter().filter(|c| c.kind == cell_type::ENTRYPOINT) {
                    if let CellPayload::Json(s) = &c.payload {
                        out.push(s.clone());
                    }
                }
            }
        }
        out
    }

    /// The probe stack: svc-py (worker / app / tasks), svc-go, svc-node,
    /// svc-rust, one compose file, one k8s file, a Procfile and a systemd
    /// unit (one row per host).
    fn stack() -> MergedGraph {
        let repo = RepoId(11);
        let mut code = G::new(repo);
        let worker = code.module("svc-py/worker.py", "svc-py::worker");
        code.at(
            node_kind::FUNCTION,
            worker,
            "svc-py::worker::loop",
            "svc-py/worker.py",
        );
        code.module("svc-py/app.py", "svc-py::app");
        code.module("svc-py/tasks.py", "svc-py::tasks");
        let server = code.module("svc-go/cmd/server/main.go", "svc-go::cmd::server::main");
        code.at(
            node_kind::FUNCTION,
            server,
            "svc-go::cmd::server::main::main",
            "svc-go/cmd/server/main.go",
        );
        let migrate = code.module("svc-go/cmd/migrate/main.go", "svc-go::cmd::migrate::main");
        code.at(
            node_kind::FUNCTION,
            migrate,
            "svc-go::cmd::migrate::main::main",
            "svc-go/cmd/migrate/main.go",
        );
        code.module("svc-node/server.js", "svc-node::server");
        let api = code.module("svc-rust/src/bin/api.rs", "svc-rust::src::bin::api");
        code.at(
            node_kind::FUNCTION,
            api,
            "svc-rust::src::bin::api::main",
            "svc-rust/src/bin/api.rs",
        );
        let mut manifests = G::new(repo);
        let cargo = manifests.node(node_kind::MODULE, "svc-rust::Cargo.toml", None, vec![]);
        manifests.bin(
            cargo,
            "api",
            manifest_bin_cell("cargo", Some("svc-rust/Cargo.toml"), 6, "src/bin/api.rs"),
        );
        let graphs = vec![
            code.done(),
            manifests.done(),
            docker(
                repo,
                "svc-py/Dockerfile",
                "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"worker.py\"]\n",
            ),
            docker(
                repo,
                "svc-go/Dockerfile",
                "FROM golang:1.22 AS build\nWORKDIR /src\nCOPY . .\nRUN go build -o /bin/server ./cmd/server\n\nFROM gcr.io/distroless/static\nCOPY --from=build /bin/server /bin/server\nENTRYPOINT [\"/bin/server\"]\n",
            ),
            docker(
                repo,
                "svc-node/Dockerfile",
                "FROM node:20\nWORKDIR /usr/src/app\nCOPY . .\nENTRYPOINT [\"node\", \"server.js\"]\n",
            ),
            docker(
                repo,
                "svc-rust/Dockerfile",
                "FROM rust:1.80 AS build\nWORKDIR /app\nCOPY . .\nRUN cargo build --release --bin api\n\nFROM debian:bookworm-slim\nCOPY --from=build /app/target/release/api /usr/local/bin/api\nCMD [\"api\"]\n",
            ),
            yaml(
                repo,
                "docker-compose.yml",
                "services:\n  api:\n    build: ./svc-py\n    command: gunicorn app:app\n  worker:\n    build: ./svc-py\n  app:\n    image: $APP\n    command: [\"$APP_CMD\"]\n",
            ),
            yaml(
                repo,
                "k8s/deploy.yaml",
                "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: py-worker\nspec:\n  template:\n    spec:\n      containers:\n        - name: worker\n          image: registry.example.com/svc-py:1.0\n          command: [\"python\"]\n          args: [\"worker.py\"]\n---\napiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: nightly\nspec:\n  schedule: \"0 3 * * *\"\n  jobTemplate:\n    spec:\n      template:\n        spec:\n          containers:\n            - name: nightly\n              image: svc-py\n              command: [\"python\", \"-m\", \"tasks\"]\n",
            ),
            proc_file(
                repo,
                ProcessManager::Procfile,
                "svc-py/Procfile",
                "web: gunicorn app:app\nworker: celery -A tasks worker\n",
            ),
            proc_file(
                repo,
                ProcessManager::Systemd,
                "deploy/worker.service",
                "[Service]\nWorkingDirectory=/srv/svc-py\nExecStart=/usr/bin/python3 /srv/svc-py/worker.py\n",
            ),
        ];
        let mut merged = MergedGraph::new(graphs);
        merged.run(&HostResolver);
        merged
    }

    #[test]
    fn host_starts_bind_one_row_per_host() {
        let merged = stack();
        use Confidence::{Medium, Strong};
        assert_eq!(
            starts(&merged),
            [
                start(
                    "infra:cronjob:nightly",
                    "svc-py::tasks",
                    Strong,
                    "k8s:py_module"
                ),
                start(
                    "infra:deployment:py-worker",
                    "svc-py::worker",
                    Strong,
                    "k8s:script"
                ),
                start(
                    "infra:image:svc-go",
                    "svc-go::cmd::server::main::main",
                    Strong,
                    "docker:binary"
                ),
                start(
                    "infra:image:svc-node",
                    "svc-node::server",
                    Strong,
                    "docker:script"
                ),
                start(
                    "infra:image:svc-py",
                    "svc-py::worker",
                    Strong,
                    "docker:script"
                ),
                start(
                    "infra:image:svc-rust",
                    "svc-rust::src::bin::api::main",
                    Strong,
                    "docker:binary"
                ),
                start(
                    "infra:process:web@svc-py",
                    "svc-py::app",
                    Strong,
                    "procfile:module_attr"
                ),
                start(
                    "infra:process:worker@svc-py",
                    "svc-py::tasks",
                    Strong,
                    "procfile:celery_app"
                ),
                start(
                    "infra:service:api",
                    "svc-py::app",
                    Strong,
                    "compose:module_attr"
                ),
                start(
                    "infra:unit:worker",
                    "svc-py::worker",
                    Medium,
                    "systemd:script"
                ),
            ],
            "compose `worker` overrides nothing (no edge: INFRA_REFERENCES reaches the \
             image); compose `app` is `$APP` (dynamic); the systemd path is a suffix"
        );
        // Every start edge is a located HANDLED_BY of resolver:host.
        for e in &merged.cross_edges {
            let ev = Evidence::of(e).expect("evidence");
            assert_eq!(e.category, edge_category::HANDLED_BY);
            assert_eq!(ev.emitter, "resolver:host");
            assert!(ev.file.is_some() && ev.line.is_some());
        }
        // The first start of a target writes its ENTRYPOINT; later ones keep it.
        assert_eq!(
            entry_cells(&merged, "svc-py::worker"),
            [
                r#"{"decl":"svc-py/Dockerfile:4","source":"host:docker","host":"infra:image:svc-py","trigger":"container_start"}"#
            ]
        );
        assert_eq!(
            entry_cells(&merged, "svc-py::tasks"),
            [
                r#"{"decl":"k8s/deploy.yaml:27","source":"host:k8s","host":"infra:cronjob:nightly","trigger":"schedule"}"#
            ]
        );
        assert_eq!(
            entry_cells(&merged, "svc-py::app"),
            [
                r#"{"decl":"docker-compose.yml:4","source":"host:compose","host":"infra:service:api","trigger":"container_start"}"#
            ]
        );
        assert!(entry_cells(&merged, "svc-go::cmd::migrate::main::main").is_empty());
    }

    #[test]
    fn the_marker_counts_starts_forms_and_reasons() {
        let mut merged = stack();
        merged.cross_edges.clear();
        for g in &mut merged.graphs {
            for n in &mut g.nodes {
                n.cells.retain(|c| c.kind != cell_type::ENTRYPOINT);
            }
        }
        let bins = bind_bins(&mut merged);
        let report = bind_host_starts(&mut merged, &bins.bound);
        assert_eq!(
            report.marker(),
            "starts=11 bound=10 (fact=9 derived=1) unresolved=1 (dynamic=1 ambiguous=0 \
             framework=0 missing=0 external_image=0 refused=0) forms=(script=4 py_module=1 \
             module_attr=2 celery_app=1 flask_app=0 binary=2 pkg_script=0 jar=0 java_main=0 \
             dll=0 framework=0) entries=6"
        );
    }

    /// A one-repo merge of `code` plus the given IaC graphs, resolved.
    fn resolved(code: G, iac: Vec<RepoGraph>) -> MergedGraph {
        let mut graphs = vec![code.done()];
        graphs.extend(iac);
        let mut merged = MergedGraph::new(graphs);
        merged.run(&HostResolver);
        merged
    }

    #[test]
    fn refused_starts_bind_nothing() {
        let repo = RepoId(12);
        let mut code = G::new(repo);
        code.module("a/worker.py", "a::worker");
        code.module("b/worker.py", "b::worker");
        let merged = resolved(
            code,
            vec![
                // Two `worker.py` files share the longest suffix: ambiguous.
                proc_file(
                    repo,
                    ProcessManager::Systemd,
                    "deploy/worker.service",
                    "[Service]\nExecStart=/usr/bin/python3 /opt/svc/worker.py\n",
                ),
                // A runtime value.
                proc_file(repo, ProcessManager::Procfile, "Procfile", "web: $APP\n"),
                // A framework runner.
                proc_file(
                    repo,
                    ProcessManager::Procfile,
                    "a/Procfile",
                    "web: rails server\n",
                ),
                // A Helm template: the reader refuses the command.
                yaml(
                    repo,
                    "chart/templates/deploy.yaml",
                    "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  template:\n    spec:\n      containers:\n        - name: web\n          image: web\n          command: {{ .Values.command }}\n",
                ),
                // An image the repo does not build.
                yaml(
                    repo,
                    "k8s/redis.yaml",
                    "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: cache\nspec:\n  template:\n    spec:\n      containers:\n        - name: redis\n          image: redis:7\n          command: [\"redis-server\", \"/etc/redis.conf\"]\n",
                ),
            ],
        );
        assert!(starts(&merged).is_empty(), "{:?}", starts(&merged));
        assert!(entry_cells(&merged, "a::worker").is_empty());
        let mut merged = merged;
        merged.cross_edges.clear();
        let report = bind_host_starts(&mut merged, &[]);
        let marker = report.marker();
        assert!(
            marker.starts_with(
                "starts=5 bound=0 (fact=0 derived=0) unresolved=5 (dynamic=2 ambiguous=1 \
                 framework=1 missing=0 external_image=1 refused=0)"
            ),
            "{marker}"
        );
    }

    #[test]
    fn built_binaries_bind_through_their_build_line() {
        let repo = RepoId(13);
        let mut code = G::new(repo);
        // .NET: `dotnet publish Cart.csproj -o /app` + `dotnet Cart.dll`.
        let program = code.module("cartsvc/Program.cs", "cartsvc::Program");
        let class = code.at(
            node_kind::CLASS,
            program,
            "Cart::Program",
            "cartsvc/Program.cs",
        );
        code.at(
            node_kind::METHOD,
            class,
            "Cart::Program::Main",
            "cartsvc/Program.cs",
        );
        // Gradle installDist: the class named like the application's script.
        let ad = code.module("adservice/src/main/java/shop/Ad.java", "adservice::Ad");
        let ad_class = code.at(
            node_kind::CLASS,
            ad,
            "adservice::Ad::Ad",
            "adservice/src/main/java/shop/Ad.java",
        );
        code.at(
            node_kind::METHOD,
            ad_class,
            "adservice::Ad::Ad::main",
            "adservice/src/main/java/shop/Ad.java",
        );
        let other = code.at(
            node_kind::CLASS,
            ad,
            "adservice::Ad::Tool",
            "adservice/src/main/java/shop/Ad.java",
        );
        code.at(
            node_kind::METHOD,
            other,
            "adservice::Ad::Tool::main",
            "adservice/src/main/java/shop/Ad.java",
        );
        // A binary no build line writes: the Go main package named like it.
        let gw = code.module("tools/gateway/main.go", "tools::gateway::main");
        code.at(
            node_kind::FUNCTION,
            gw,
            "tools::gateway::main::main",
            "tools/gateway/main.go",
        );
        let merged = resolved(
            code,
            vec![
                docker(
                    repo,
                    "cartsvc/Dockerfile",
                    "FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build\nWORKDIR /src\nCOPY . .\nRUN dotnet publish Cart.csproj -c Release -o /app\n\nFROM mcr.microsoft.com/dotnet/runtime:8.0\nWORKDIR /app\nCOPY --from=build /app .\nENTRYPOINT [\"dotnet\", \"Cart.dll\"]\n",
                ),
                docker(
                    repo,
                    "adservice/Dockerfile",
                    "FROM eclipse-temurin:21 AS builder\nWORKDIR /app\nCOPY . .\nRUN ./gradlew installDist\n\nFROM eclipse-temurin:21-jre\nWORKDIR /app\nCOPY --from=builder /app .\nENTRYPOINT [\"/app/build/install/shop/bin/Ad\"]\n",
                ),
                proc_file(
                    repo,
                    ProcessManager::Procfile,
                    "Procfile",
                    "gw: gateway --port 80\n",
                ),
            ],
        );
        use Confidence::{Medium, Strong};
        assert_eq!(
            starts(&merged),
            [
                start(
                    "infra:image:adservice",
                    "adservice::Ad::Ad::main",
                    Medium,
                    "docker:binary"
                ),
                start(
                    "infra:image:cartsvc",
                    "Cart::Program::Main",
                    Strong,
                    "docker:dll"
                ),
                start(
                    "infra:process:gw@.",
                    "tools::gateway::main::main",
                    Medium,
                    "procfile:binary"
                ),
            ]
        );
    }

    #[test]
    fn json_reads_the_start_cells_exactly() {
        let v =
            Json::parse(r#"{"a":[1,-2.5e3,true,null],"s":"q\"\\\/é😀\n","o":{}}"#).expect("parses");
        assert_eq!(v.str_at("s"), Some("q\"\\/\u{e9}\u{1F600}\n"));
        assert_eq!(
            v.get("a").and_then(Json::as_array).map(<[Json]>::len),
            Some(4)
        );
        assert_eq!(v.strs_at("missing"), None);
        assert_eq!(v.strs_at("o"), None);
        for bad in [
            r#"{"a":1"#,
            r#"{"a":1}x"#,
            r#"{'a':1}"#,
            "\"\u{1}\"",
            r#""\q""#,
        ] {
            assert_eq!(Json::parse(bad), None, "{bad}");
        }
        assert_eq!(json_string("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }

    #[test]
    fn container_paths_map_through_the_copy_map() {
        let copies = vec![
            ("/app".to_string(), ".".to_string()),
            ("/app/conf".to_string(), "deploy/conf".to_string()),
        ];
        assert_eq!(
            copied_from(&copies, "/app/worker.py").as_deref(),
            Some("worker.py")
        );
        assert_eq!(
            copied_from(&copies, "/app/conf/x.py").as_deref(),
            Some("deploy/conf/x.py"),
            "the later COPY wins"
        );
        assert_eq!(copied_from(&copies, "/srv/x.py"), None);
        assert_eq!(copied_from(&copies, "/application/x.py"), None);
        assert_eq!(
            container_path("./a/../b.py", Some("/app")).as_deref(),
            Some("/app/b.py")
        );
        assert_eq!(container_path("b.py", None), None);
        assert_eq!(join_rel(".", "./x.py").as_deref(), Some("x.py"));
        assert_eq!(image_basename("registry.example.com/team/svc:1.0"), "svc");
    }
}
