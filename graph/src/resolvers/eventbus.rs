//! Event-bus resolver — emitter → handler by event name.
//!
//! OWNERS (LB.8b). An event side inside a nested project carries LB.4a's
//! ` @<project path>` owner (engine `http_owner`). An in-process bus (Node
//! EventEmitter, NestJS `@OnEvent`, CQRS, Spring / MediatR, RxJS, DOM)
//! delivers only inside one process, and two nested projects are two build
//! artefacts, so an emitter pairs a handler only when:
//! - both carry the SAME owner in the same repo (`same-owner`);
//! - either side is unowned (`unowned-side`): a file under no nested project
//!   may be linked into any of them, so it keeps pairing with every owner
//!   (and a build without nested projects is unchanged);
//! - either side is transport-scoped (`transport`): the extractor marked it
//!   with an ORIGIN `"delivery":"transport"` (NestJS microservices
//!   `@EventPattern` and its `ClientProxy.emit`, AWS EventBridge), a network
//!   bus that crosses processes like a queue.
//!
//! - the two owners link into ONE process (`same-process`, CB.5): a project
//!   whose manifest depends on another nested project of the same repo
//!   (`apps/api/package.json` DEPENDS_ON `package:npm:@shop/orders`, the
//!   label of `project:libs/orders`) runs that library's code in its own
//!   process, so a library's emit reaches the app's handler. See
//!   [`ProcessClosure`]; its edges carry the EVIDENCE rule `same_process`.
//!
//! Every other pair is dropped and counted (`cross-owner-dropped`). Owners are
//! compared together with the RepoId, so the same rel path in two repos is two
//! projects.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{CodeNav, cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, RuleTally, ServiceTarget, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// EventBusResolver — matches event emitter → handler by event name
// ============================================================================

pub struct EventBusResolver;

/// EVENT_* nodes come from two places: the cross-cutting extractor, whose
/// qnames carry an `event_emit:` / `event_handle:` prefix, and language parsers
/// (Solidity today) whose qnames are ordinary code qnames like
/// `Auction::Auction::BidPlaced`. Key on the prefix when it is there and on the
/// node's simple NAME when it is not, so a Solidity `BidPlaced` can reach a
/// Java `@EventListener(BidPlaced)`. `build_kind_index` is deliberately left
/// alone — GraphQL, WebSocket and CLI all share it and none of them has a
/// parser-side producer of the same kind.
///
/// LB.8b: the owner segment is split off first and returned beside the key.
/// A code qname never carries one, so a Solidity event is unowned.
fn event_key<'q>(
    nav: &CodeNav,
    id: NodeId,
    qname: &'q str,
    prefix: &str,
) -> Option<(String, Option<&'q str>)> {
    let (bare, owner) = split_owner(qname);
    let key = match bare.strip_prefix(prefix) {
        Some(rest) => rest.to_string(),
        None => nav.name_by_id.get(&id).cloned()?,
    };
    Some((key, owner))
}

/// LB.8b: does this side travel over a transport? The extractor records it as
/// an ORIGIN cell carrying `"delivery":"transport"`; a substring test (the
/// `http::extract_method_field` precedent), no serde_json in the graph crate.
fn is_transport(cells: &[Cell]) -> bool {
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) if j.contains(r#""delivery":"transport""#))
    })
}

/// One event side as the owner rule reads it.
#[derive(Clone, Copy)]
struct Side<'g> {
    repo: RepoId,
    owner: Option<&'g str>,
    transport: bool,
}

/// One indexed handler.
struct HandlerEntry<'g> {
    target: ServiceTarget,
    side: Side<'g>,
}

/// Why an emitter / handler pair was kept, or that it was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pairing {
    /// Both owned, by the same project of the same repo.
    SameOwner,
    /// Either side is under no nested project.
    UnownedSide,
    /// Two different owners, and either side is transport-scoped.
    Transport,
    /// CB.5: two different owners of one repo on in-process buses, linked
    /// into one process by a workspace dependency ([`ProcessClosure`]).
    SameProcess,
    /// Two different owners on in-process buses, in no common process.
    Dropped,
}

/// The owner rule (module doc). `process` is consulted only for a pair the
/// LB.8b arms would drop, so it builds nothing for a build whose owned pairs
/// are all same-owner, unowned or transport.
fn pairing(e: Side<'_>, h: Side<'_>, process: &LazyProcess<'_>) -> Pairing {
    match (e.owner, h.owner) {
        (Some(a), Some(b)) if a == b && e.repo == h.repo => Pairing::SameOwner,
        (None, _) | (_, None) => Pairing::UnownedSide,
        _ if e.transport || h.transport => Pairing::Transport,
        (Some(a), Some(b)) if e.repo == h.repo && process.shares_process(e.repo, a, b) => {
            Pairing::SameProcess
        }
        _ => Pairing::Dropped,
    }
}

/// Per-build tallies behind the `[eventbus-owner]` line.
#[derive(Default)]
struct OwnerTally {
    same_owner: usize,
    unowned_side: usize,
    transport: usize,
    same_process: usize,
    dropped: usize,
    /// Any indexed handler or matched emitter carried an owner.
    owned: bool,
}

impl OwnerTally {
    /// The LB.8b fired_on marker, once per build that holds an owned event
    /// side (a repo with nested projects); `None` otherwise, so an owner-free
    /// build prints exactly what it did before. CB.5's `same-process=` sits
    /// before `cross-owner-dropped=`, which keeps the line's tail stable:
    /// `... 2>&1 | grep '^\[eventbus-owner\] '`.
    fn line(&self) -> Option<String> {
        self.owned.then(|| {
            format!(
                "[eventbus-owner] same-owner={} unowned-side={} transport={} \
                 same-process={} cross-owner-dropped={}",
                self.same_owner, self.unowned_side, self.transport, self.same_process, self.dropped
            )
        })
    }
}

// ============================================================================
// ProcessClosure — which nested projects link into one process (CB.5)
// ============================================================================

/// The workspace-dependency closure over one build's nested projects.
///
/// A library project linked into an app runs in the app's process, so an
/// in-process event emitted in the library reaches the app's handler. The
/// graph already holds both halves of that link:
/// - every nested project is a PROJECT node whose ORIGIN names its `path` (the
///   owner string LB.4a writes after ` @`), its `label` (the manifest's package
///   name, `@shop/orders`) and its repo-relative `manifest`;
/// - every manifest dependency is a DEPENDS_ON edge from the manifest's
///   synthetic MODULE (qname: the manifest path with `/` read as `::`,
///   `apps::api::package.json`, engine `extract::synthetic_module_qname`) to a
///   PACKAGE_DEP `package:<eco>:<name>`.
///
/// A dependency links its depender project (the one whose manifest is the
/// edge's MODULE) to every project of the same repo whose label is the
/// dependency's name in the dependency's ecosystem. The manifest basename maps
/// the PROJECT ecosystem (`python`, `go`, `php`, ...) onto the PACKAGE_DEP one
/// (`pypi`, `gomod`, `composer`, ...), see [`dep_ecosystem`]; pypi names
/// compare PEP 503-normalised, every other ecosystem exactly.
///
/// A project `p`'s process holds `p` and every project reachable from it over
/// those links. Two owners share a process when some project's process holds
/// both: the app and its library, two libraries of one app, a library and the
/// library it depends on. The repo-root project is never an owner (LB.4a) and
/// never a depender here: a root manifest that lists every workspace would
/// otherwise put every project in one process.
///
/// The table is one entry per nested project (tens per repo), so the closure
/// is a local BFS keyed by owner strings; `activation::algo` walks node ids of
/// a `GraphSource`, which this is not.
#[derive(Debug, Default)]
struct ProcessClosure {
    /// `repo -> project path -> the linking projects whose process holds it`.
    /// Only projects that reach at least one other project are roots, so an
    /// unlinked project has no entry and shares no process with another.
    process_of: HashMap<RepoId, BTreeMap<String, BTreeSet<String>>>,
}

impl ProcessClosure {
    fn build(graphs: &[RepoGraph]) -> Self {
        // 1. The project table, keyed by repo (the PROJECT nodes sit in their
        //    own graph, beside the manifest MODULEs of the same repo).
        let mut by_manifest: HashMap<RepoId, BTreeMap<String, String>> = HashMap::new();
        let mut by_label: HashMap<RepoId, BTreeMap<(&'static str, String), BTreeSet<String>>> =
            HashMap::new();
        for g in graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PROJECT) {
                    continue;
                }
                let Some(origin) = origin_json(&n.cells) else {
                    continue;
                };
                let Some(path) = json_string_field(origin, "path") else {
                    continue;
                };
                if path.is_empty() || path == "." {
                    continue;
                }
                let Some(manifest) = json_string_field(origin, "manifest") else {
                    continue;
                };
                if let (Some(eco), Some(label)) =
                    (dep_ecosystem(&manifest), json_string_field(origin, "label"))
                {
                    by_label
                        .entry(g.repo)
                        .or_default()
                        .entry((eco, dep_name_key(eco, &label)))
                        .or_default()
                        .insert(path.clone());
                }
                by_manifest
                    .entry(g.repo)
                    .or_default()
                    .insert(manifest.replace('/', "::"), path);
            }
        }
        if by_label.is_empty() {
            return Self::default();
        }

        // 2. The links: depender project -> the projects it depends on.
        let mut links: HashMap<RepoId, BTreeMap<String, BTreeSet<String>>> = HashMap::new();
        for g in graphs {
            let (Some(labels), Some(manifests)) = (by_label.get(&g.repo), by_manifest.get(&g.repo))
            else {
                continue;
            };
            for e in &g.edges {
                if e.category != edge_category::DEPENDS_ON
                    || g.nav.kind_by_id.get(&e.to) != Some(&node_kind::PACKAGE_DEP)
                {
                    continue;
                }
                let Some((eco, name)) = g
                    .nav
                    .qname_by_id
                    .get(&e.to)
                    .and_then(|q| q.strip_prefix("package:"))
                    .and_then(|q| q.split_once(':'))
                else {
                    continue;
                };
                let Some(eco) = PACKAGE_ECOSYSTEMS.iter().copied().find(|x| *x == eco) else {
                    continue;
                };
                let Some(dependees) = labels.get(&(eco, dep_name_key(eco, name))) else {
                    continue;
                };
                let Some(depender) = g
                    .nav
                    .qname_by_id
                    .get(&e.from)
                    .and_then(|q| manifests.get(q))
                else {
                    continue;
                };
                for d in dependees.iter().filter(|d| *d != depender) {
                    links
                        .entry(g.repo)
                        .or_default()
                        .entry(depender.clone())
                        .or_default()
                        .insert(d.clone());
                }
            }
        }

        // 3. The closure: each linking project's process, inverted.
        let mut process_of: HashMap<RepoId, BTreeMap<String, BTreeSet<String>>> = HashMap::new();
        for (repo, adj) in &links {
            let of = process_of.entry(*repo).or_default();
            for root in adj.keys() {
                let mut seen: BTreeSet<&str> = BTreeSet::from([root.as_str()]);
                let mut queue: VecDeque<&str> = VecDeque::from([root.as_str()]);
                while let Some(p) = queue.pop_front() {
                    for q in adj.get(p).into_iter().flatten() {
                        if seen.insert(q.as_str()) {
                            queue.push_back(q.as_str());
                        }
                    }
                }
                for p in seen {
                    of.entry(p.to_string()).or_default().insert(root.clone());
                }
            }
        }
        Self { process_of }
    }

    /// Do the owners `a` and `b` of `repo` run in one process?
    fn shares_process(&self, repo: RepoId, a: &str, b: &str) -> bool {
        let Some(of) = self.process_of.get(&repo) else {
            return false;
        };
        match (of.get(a), of.get(b)) {
            (Some(x), Some(y)) => !x.is_disjoint(y),
            _ => false,
        }
    }
}

/// The [`ProcessClosure`] of one resolve, built at the first pair that needs
/// it and at most once.
struct LazyProcess<'g> {
    graphs: &'g [RepoGraph],
    closure: OnceCell<ProcessClosure>,
}

impl<'g> LazyProcess<'g> {
    fn new(graphs: &'g [RepoGraph]) -> Self {
        Self {
            graphs,
            closure: OnceCell::new(),
        }
    }

    fn shares_process(&self, repo: RepoId, a: &str, b: &str) -> bool {
        self.closure
            .get_or_init(|| ProcessClosure::build(self.graphs))
            .shares_process(repo, a, b)
    }
}

/// The PACKAGE_DEP ecosystems (`parsers/code/extractors/src/packages.rs`).
const PACKAGE_ECOSYSTEMS: &[&str] = &["npm", "pypi", "cargo", "gomod", "rubygems", "composer"];

/// The PACKAGE_DEP ecosystem a project's dependers name it in, by the basename
/// of the manifest that rooted it (`code_domain::project_roots::MANIFESTS`).
/// A root no dependency extractor can name (maven, gradle, .NET, an overlay
/// `[[project]]` ...) is never a dependee.
fn dep_ecosystem(manifest: &str) -> Option<&'static str> {
    match manifest.rsplit('/').next().unwrap_or(manifest) {
        "package.json" => Some("npm"),
        "go.mod" => Some("gomod"),
        "Cargo.toml" => Some("cargo"),
        "pyproject.toml" | "setup.cfg" | "setup.py" => Some("pypi"),
        "composer.json" => Some("composer"),
        "Gemfile" => Some("rubygems"),
        _ => None,
    }
}

/// A dependency / project name as the link compares it: pypi PEP 503-
/// normalised (lowercase, every run of `-`, `_`, `.` one `-`), every other
/// ecosystem exact.
fn dep_name_key(eco: &str, name: &str) -> String {
    if eco != "pypi" {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    let mut in_run = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !in_run {
                out.push('-');
            }
            in_run = true;
        } else {
            out.extend(c.to_lowercase());
            in_run = false;
        }
    }
    out
}

/// A node's ORIGIN JSON, if it carries one.
fn origin_json(cells: &[Cell]) -> Option<&str> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(j) if c.kind == cell_type::ORIGIN => Some(j.as_str()),
        _ => None,
    })
}

/// `json["key"]` as a string with its escapes decoded; the graph crate has no
/// serde_json. The shape of `identity.rs`'s module-private reader, which this
/// file cannot reach. A `\u` surrogate pair is not decoded: the field reads as
/// absent rather than wrong (a PROJECT label is printable manifest text, and
/// serde_json keeps non-ASCII literal).
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    // Only a KEY is followed by `:` (a value naming the key, `"label":"path"`,
    // is not), so the first occurrence that is is the field.
    let rest = json.match_indices(&needle).find_map(|(at, _)| {
        json[at + needle.len()..]
            .trim_start()
            .strip_prefix(':')?
            .trim_start()
            .strip_prefix('"')
    })?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() != 4 || !hex.chars().all(|h| h.is_ascii_hexdigit()) {
                        return None;
                    }
                    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                }
                other => other,
            }),
            c => out.push(c),
        }
    }
    None
}

/// Type-named keys fold: `OrderPlacedEvent` and `OrderPlaced` are one event.
/// Applied ONLY to keys that look like a TYPE — a plain identifier starting
/// uppercase — so string topics (`user.created`) and the extractor's tag
/// fallbacks (`emit`, `on`, `@OnEvent`, `Subject.next`) keep matching
/// byte-exactly. There is deliberately NO separator folding: `user.created`
/// must not become `usercreated`, which is the all-to-all shape the queue side
/// just closed.
fn normalise_event_key(raw: &str) -> String {
    let is_type = raw.chars().next().is_some_and(char::is_uppercase)
        && raw.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !is_type {
        return raw.to_string();
    }
    let low = raw.to_lowercase();
    let stripped = low
        .strip_suffix("events")
        .or_else(|| low.strip_suffix("event"));
    match stripped {
        // `Event` alone folds to nothing; keep a stem worth matching on.
        Some(stem) if stem.len() >= 3 => stem.to_string(),
        _ => low,
    }
}

impl CrossGraphResolver for EventBusResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        // Lookup-only map; each Vec keeps graph / node order, so the edges
        // come out in the same order on every run.
        let mut handler_index: HashMap<String, Vec<HandlerEntry<'_>>> = HashMap::new();
        let mut prefixed = 0usize;
        let mut by_name = 0usize;
        let mut tally = OwnerTally::default();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_HANDLER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some((key, owner)) = event_key(&g.nav, n.id, qname, "event_handle:") else {
                    continue;
                };
                if qname.starts_with("event_handle:") {
                    prefixed += 1;
                } else {
                    by_name += 1;
                }
                tally.owned |= owner.is_some();
                handler_index
                    .entry(normalise_event_key(&key))
                    .or_default()
                    .push(HandlerEntry {
                        target: ServiceTarget {
                            id: n.id,
                            confidence: n.confidence,
                        },
                        side: Side {
                            repo: g.repo,
                            owner,
                            transport: is_transport(&n.cells),
                        },
                    });
            }
        }

        let mut exact = 0usize;
        let mut folded = 0usize;
        // LC.3c: `exact` when the emitter's raw key is its normalised key,
        // `folded` when the type-name fold made it (the counters' branch).
        // CB.5: `same_process` for a pair only a workspace link keeps; the
        // exact / folded counters still count it, so `pairs` is every edge.
        let mut rules = RuleTally::new("eventbus", &["exact", "folded", "same_process"]);
        let process = LazyProcess::new(&merged.graphs);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_EMITTER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some((raw, owner)) = event_key(&g.nav, n.id, qname, "event_emit:") else {
                    continue;
                };
                let key = normalise_event_key(&raw);
                let Some(targets) = handler_index.get(&key) else {
                    continue;
                };
                tally.owned |= owner.is_some();
                let emitter = Side {
                    repo: g.repo,
                    owner,
                    transport: is_transport(&n.cells),
                };
                for h in targets {
                    let way = pairing(emitter, h.side, &process);
                    match way {
                        Pairing::SameOwner => tally.same_owner += 1,
                        Pairing::UnownedSide => tally.unowned_side += 1,
                        Pairing::Transport => tally.transport += 1,
                        Pairing::SameProcess => tally.same_process += 1,
                        Pairing::Dropped => {
                            tally.dropped += 1;
                            continue;
                        }
                    }
                    let key_rule = if key == raw {
                        exact += 1;
                        "exact"
                    } else {
                        folded += 1;
                        "folded"
                    };
                    let rule = if way == Pairing::SameProcess {
                        "same_process"
                    } else {
                        key_rule
                    };
                    let confidence = weakest(n.confidence, h.target.confidence);
                    merged.cross_edges.push(
                        Edge::new(n.id, h.target.id, edge_category::EVENT_FLOWS, confidence)
                            .with_cell(rules.cell(rule)),
                    );
                }
            }
        }

        rules.report();
        // One line per BUILD, and only when this resolver had anything to say —
        // the `[ws-resolve]` house style. `pairs` counts PUSHED edges.
        let pairs = exact + folded;
        if pairs > 0 || by_name > 0 {
            eprintln!(
                "[eventbus] {pairs} pairs (exact={exact} type-folded={folded}); \
                 handlers indexed: prefixed={prefixed} by-name={by_name}"
            );
        }
        if let Some(line) = tally.line() {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{channel_graph, cross_pairs};
    use super::*;
    use glia_code_domain::GRAPH_TYPE;
    use glia_core::{Confidence, Node};

    /// A repo's workspace as the engine builds it: the PROJECT graph (one
    /// node per `(path, manifest basename, label)`, its ORIGIN written the
    /// way `engine::walk::build_project_graph` writes it) and the manifest
    /// graph (each `(manifest path, package qname)` one DEPENDS_ON from the
    /// manifest's synthetic MODULE to its PACKAGE_DEP). Same repo as
    /// `channel_graph(tag, ..)`.
    fn workspace(
        tag: &str,
        projects: &[(&str, &str, &str)],
        deps: &[(&str, &str)],
    ) -> Vec<RepoGraph> {
        let mut roots = channel_graph(tag, &[]);
        let repo = roots.repo;
        for (path, base, label) in projects {
            let manifest = if path.is_empty() {
                base.to_string()
            } else {
                format!("{path}/{base}")
            };
            let q = format!("project:{}", if path.is_empty() { "." } else { path });
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PROJECT, &q);
            roots.nav.record(id, label, &q, node_kind::PROJECT, None);
            let origin = format!(
                r#"{{"ecosystem":"npm","label":"{label}","manifest":"{manifest}","path":"{path}","provenance":"project_root"}}"#
            );
            roots.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::ORIGIN,
                    payload: CellPayload::Json(origin),
                }],
            });
        }
        let mut manifests = channel_graph(tag, &[]);
        for (manifest, dep) in deps {
            let mq = manifest.replace('/', "::");
            let module = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, &mq);
            if !manifests.nav.qname_by_id.contains_key(&module) {
                manifests
                    .nav
                    .record(module, &mq, &mq, node_kind::MODULE, None);
                manifests.nodes.push(Node {
                    id: module,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![],
                });
            }
            let pkg = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE_DEP, dep);
            manifests
                .nav
                .record(pkg, dep, dep, node_kind::PACKAGE_DEP, Some(module));
            manifests.nodes.push(Node {
                id: pkg,
                repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            manifests.edges.push(Edge::new(
                module,
                pkg,
                edge_category::DEPENDS_ON,
                Confidence::Medium,
            ));
        }
        vec![roots, manifests]
    }

    /// Resolve the event sides `sides` of repo `tag` beside its workspace;
    /// the EVENT_FLOWS pairs and the `[eventbus-owner]` line.
    fn resolve(
        tag: &str,
        sides: &[(glia_core::NodeKindId, &str)],
        projects: &[(&str, &str, &str)],
        deps: &[(&str, &str)],
    ) -> (MergedGraph, Vec<(String, String)>) {
        let mut graphs = vec![channel_graph(tag, sides)];
        graphs.extend(workspace(tag, projects, deps));
        let mut m = MergedGraph::new(graphs);
        EventBusResolver.resolve(&mut m);
        let pairs = cross_pairs(&m, edge_category::EVENT_FLOWS);
        (m, pairs)
    }

    fn s(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    /// The EVIDENCE rule of the `from -> to` EVENT_FLOWS edge.
    fn rule_of(m: &MergedGraph, from: &str, to: &str) -> String {
        let id = |q: &str| {
            m.graphs
                .iter()
                .find_map(|g| {
                    g.nav
                        .qname_by_id
                        .iter()
                        .find(|(_, v)| *v == q)
                        .map(|(k, _)| *k)
                })
                .unwrap()
        };
        let e = m
            .cross_edges
            .iter()
            .find(|e| e.from == id(from) && e.to == id(to))
            .unwrap();
        match &e.cell(cell_type::EVIDENCE).unwrap().payload {
            CellPayload::Json(j) => json_string_field(j, "rule").unwrap(),
            other => panic!("EVIDENCE must be JSON, got {other:?}"),
        }
    }

    /// The fixture's projects: an app, the library it links, a sibling
    /// service, and the repo root listing every workspace.
    const SHOP: &[(&str, &str, &str)] = &[
        ("", "package.json", "shop"),
        ("apps/api", "package.json", "@shop/api"),
        ("libs/orders", "package.json", "@shop/orders"),
        ("services/billing", "package.json", "@shop/billing"),
    ];

    /// CB.5: apps/api depends on @shop/orders, so the library's emit reaches
    /// the app's handler, and the edge says why (`same_process`).
    #[test]
    fn a_library_emit_reaches_its_app_handler() {
        let (m, pairs) = resolve(
            "cb5-lib-app",
            &[
                (
                    node_kind::EVENT_EMITTER,
                    "event_emit:order.placed @libs/orders",
                ),
                (
                    node_kind::EVENT_HANDLER,
                    "event_handle:order.placed @apps/api",
                ),
            ],
            SHOP,
            &[
                ("apps/api/package.json", "package:npm:@shop/orders"),
                ("apps/api/package.json", "package:npm:@nestjs/event-emitter"),
            ],
        );
        assert_eq!(
            pairs,
            [s(
                "event_emit:order.placed @libs/orders",
                "event_handle:order.placed @apps/api"
            )]
        );
        assert_eq!(
            rule_of(
                &m,
                "event_emit:order.placed @libs/orders",
                "event_handle:order.placed @apps/api"
            ),
            "same_process"
        );
    }

    /// services/billing never depends on the library: its handler is another
    /// process. The root manifest listing every workspace links nothing (the
    /// root is never an owner), and the marker counts both pairs.
    #[test]
    fn a_sibling_service_stays_dropped() {
        let sides = [
            (
                node_kind::EVENT_EMITTER,
                "event_emit:order.placed @libs/orders",
            ),
            (
                node_kind::EVENT_HANDLER,
                "event_handle:order.placed @apps/api",
            ),
            (
                node_kind::EVENT_HANDLER,
                "event_handle:order.placed @services/billing",
            ),
        ];
        let deps = [
            ("apps/api/package.json", "package:npm:@shop/orders"),
            ("package.json", "package:npm:@shop/orders"),
            ("package.json", "package:npm:@shop/billing"),
        ];
        let (_, pairs) = resolve("cb5-sibling", &sides, SHOP, &deps);
        assert_eq!(
            pairs,
            [s(
                "event_emit:order.placed @libs/orders",
                "event_handle:order.placed @apps/api"
            )]
        );

        let graphs = {
            let mut g = vec![channel_graph("cb5-sibling", &sides)];
            g.extend(workspace("cb5-sibling", SHOP, &deps));
            g
        };
        let closure = ProcessClosure::build(&graphs);
        let repo = graphs[0].repo;
        assert!(closure.shares_process(repo, "libs/orders", "apps/api"));
        assert!(
            closure.shares_process(repo, "apps/api", "libs/orders"),
            "symmetric"
        );
        assert!(!closure.shares_process(repo, "libs/orders", "services/billing"));
        assert!(
            !closure.shares_process(repo, "", "services/billing"),
            "the root is no owner"
        );
        let other = RepoId::from_canonical("test://cb5-elsewhere");
        assert!(
            !closure.shares_process(other, "libs/orders", "apps/api"),
            "per repo"
        );

        let tally = OwnerTally {
            same_process: 1,
            dropped: 1,
            owned: true,
            ..OwnerTally::default()
        };
        assert_eq!(
            tally.line().as_deref(),
            Some(
                "[eventbus-owner] same-owner=0 unowned-side=0 transport=0 same-process=1 cross-owner-dropped=1"
            )
        );
        assert_eq!(
            OwnerTally::default().line(),
            None,
            "silent without an owned side"
        );
    }

    /// Two libraries linked into one app share its process, both ways; a
    /// third library nobody links stays out.
    #[test]
    fn two_libs_linked_into_one_app_share_it() {
        let projects = [
            ("apps/api", "package.json", "@shop/api"),
            ("libs/a", "package.json", "@shop/a"),
            ("libs/b", "package.json", "@shop/b"),
            ("libs/c", "package.json", "@shop/c"),
        ];
        let (_, pairs) = resolve(
            "cb5-two-libs",
            &[
                (node_kind::EVENT_EMITTER, "event_emit:stock.low @libs/a"),
                (node_kind::EVENT_HANDLER, "event_handle:stock.low @libs/b"),
                (node_kind::EVENT_HANDLER, "event_handle:stock.low @libs/c"),
                (node_kind::EVENT_EMITTER, "event_emit:stock.high @libs/b"),
                (node_kind::EVENT_HANDLER, "event_handle:stock.high @libs/a"),
            ],
            &projects,
            &[
                ("apps/api/package.json", "package:npm:@shop/a"),
                ("apps/api/package.json", "package:npm:@shop/b"),
            ],
        );
        assert_eq!(
            pairs,
            [
                s(
                    "event_emit:stock.high @libs/b",
                    "event_handle:stock.high @libs/a"
                ),
                s(
                    "event_emit:stock.low @libs/a",
                    "event_handle:stock.low @libs/b"
                ),
            ]
        );
    }

    /// app -> lib1 -> lib2: the app's process holds lib2 too, and lib1's
    /// holds lib2 whoever links lib1.
    #[test]
    fn transitive_dependency() {
        let projects = [
            ("apps/api", "package.json", "@shop/api"),
            ("libs/one", "package.json", "@shop/one"),
            ("libs/two", "package.json", "@shop/two"),
        ];
        let (_, pairs) = resolve(
            "cb5-transitive",
            &[
                (node_kind::EVENT_EMITTER, "event_emit:tick @libs/two"),
                (node_kind::EVENT_HANDLER, "event_handle:tick @apps/api"),
                (node_kind::EVENT_HANDLER, "event_handle:tick @libs/one"),
            ],
            &projects,
            &[
                ("apps/api/package.json", "package:npm:@shop/one"),
                ("libs/one/package.json", "package:npm:@shop/two"),
            ],
        );
        assert_eq!(
            pairs,
            [
                s("event_emit:tick @libs/two", "event_handle:tick @apps/api"),
                s("event_emit:tick @libs/two", "event_handle:tick @libs/one"),
            ]
        );
    }

    /// A pypi dependency `My_Lib` names the project labelled `my-lib` (PEP
    /// 503); npm compares exactly, so the same spelling there links nothing.
    #[test]
    fn pypi_name_normalisation() {
        let projects = [
            ("apps/worker", "pyproject.toml", "worker"),
            ("libs/mylib", "pyproject.toml", "my-lib"),
            ("web/app", "package.json", "web-app"),
            ("web/lib", "package.json", "my-lib"),
        ];
        let (_, pairs) = resolve(
            "cb5-pypi",
            &[
                (node_kind::EVENT_EMITTER, "event_emit:job.done @libs/mylib"),
                (
                    node_kind::EVENT_HANDLER,
                    "event_handle:job.done @apps/worker",
                ),
                (node_kind::EVENT_EMITTER, "event_emit:ui.ready @web/lib"),
                (node_kind::EVENT_HANDLER, "event_handle:ui.ready @web/app"),
            ],
            &projects,
            &[
                ("apps/worker/pyproject.toml", "package:pypi:My_Lib"),
                ("web/app/package.json", "package:npm:My_Lib"),
            ],
        );
        assert_eq!(
            pairs,
            [s(
                "event_emit:job.done @libs/mylib",
                "event_handle:job.done @apps/worker"
            )]
        );
        assert_eq!(dep_name_key("pypi", "My__Lib.core"), "my-lib-core");
        assert_eq!(dep_name_key("npm", "My_Lib"), "My_Lib");
    }

    /// The ORIGIN reader decodes escapes and never reads a value that names
    /// the key as the key.
    #[test]
    fn origin_field_reader() {
        let j = r#"{"label":"path","manifest":"a/b\"cé","path":"libs/x"}"#;
        assert_eq!(json_string_field(j, "path").as_deref(), Some("libs/x"));
        assert_eq!(json_string_field(j, "manifest").as_deref(), Some("a/b\"cé"));
        assert_eq!(json_string_field(j, "missing"), None);
        assert_eq!(
            json_string_field(r#"{"k":"\uD83D"}"#, "k"),
            None,
            "lone surrogate"
        );
    }
}
