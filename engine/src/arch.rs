//! A9.2 — the service-level view of a stack.
//!
//! Three things live here, in dependency order:
//!
//! 1. [`node_file`] — *where is this node?* `locate_node` reads only the
//!    POSITION cell, and ENDPOINT / ROUTE nodes have none (their nav parent is
//!    `None` too), so today every endpoint and route in a `trace` or
//!    `blast-radius` answer is unlocated. Their file lives on the
//!    `ENDPOINT_HIT` / `ROUTE_METHOD` cell instead, and this reads all three.
//! 2. [`ServiceKeying`] + [`service_of`] — *what is a service?* One repo per
//!    service is wrong for a monorepo: `generate_one` on a stack renders as ONE
//!    service with ZERO links even while cross-edges sit in the graph. When
//!    there is a single repo, the key is the enclosing manifest project root
//!    (A8.5's PROJECT anchors), and every file under no root shares ONE
//!    `(outside projects)` bucket (A8.6). A repo with no roots below its top
//!    level falls back to the top-level path segment. The native shells a
//!    Flutter / React Native app generates (`android/`, `ios/`, …) are never
//!    keyed: they fold into the app that owns them (LA.5).
//! 3. [`service_map`] — the rendered answer: services, the links between them
//!    (via `glia_graph::cross_links`), and the honest residuals
//!    (`self_links`, `unlocated_nodes`).
//!
//! Nothing here reaches `MergedGraph`: the human repo label rides on
//! `GenerateResult`, deliberately, so the `.gmap` bytes are unchanged.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Confidence, EdgeCategoryId, Node, NodeId};
use glia_graph::{MergedGraph, cross_links};

// ============================================================================
// 1. node → file
// ============================================================================

/// Read the repo-relative file a node lives in, first hit wins:
///
/// - **tier 1** `POSITION` — every parsed entity (the `locate_node` path);
/// - **tier 2** `ENDPOINT_HIT` / `ROUTE_METHOD` — the client call site or the
///   route registration site, which is the ONLY file an ENDPOINT / ROUTE node
///   carries.
///
/// Tier 2 is best-effort by construction: 14 of the 16 ROUTE_METHOD emitters
/// write `CellPayload::Text("GET")`, which fails `from_str` cleanly and falls
/// through to `None` rather than panicking. Only the Go emitter (and the
/// ENDPOINT_HIT payload, always) writes a JSON object with a `file` key.
///
/// Returns an owned `String`: `serde_json::Value` owns its strings, so a
/// borrowed `&str` would need zero-copy deserialization that errors on any
/// escape in the path — a silent miss traded for a clone.
pub fn node_file(n: &Node) -> Option<String> {
    for c in &n.cells {
        if c.kind == cell_type::POSITION {
            if let Some(f) = json_file(&c.payload) {
                return Some(f);
            }
        }
    }
    for c in &n.cells {
        if c.kind == cell_type::ENDPOINT_HIT || c.kind == cell_type::ROUTE_METHOD {
            if let Some(f) = json_file(&c.payload) {
                return Some(f);
            }
        }
    }
    None
}

/// `payload["file"]`, or `None` for any payload that is not a JSON object with
/// a non-empty `file` string. Total — no `unwrap`, no panic on `Text("GET")`.
fn json_file(payload: &CellPayload) -> Option<String> {
    let s = match payload {
        CellPayload::Json(s) | CellPayload::Text(s) => s,
        CellPayload::Bytes(_) => return None,
    };
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    v.get("file")
        .and_then(serde_json::Value::as_str)
        .filter(|f| !f.is_empty())
        .map(String::from)
}

/// Edge categories the adjacency fallback may travel. **Load-bearing**: allow
/// `HTTP_CALLS` here and every ROUTE inherits its *caller's* directory, which
/// silently merges the whole stack into one service — the exact bug this
/// packet exists to fix.
const ADJACENCY_EDGES: &[EdgeCategoryId] = &[
    edge_category::HANDLED_BY,
    edge_category::CALLS,
    edge_category::DEFINES,
    edge_category::CONTAINS,
];

/// `NodeId → file` for every node that can be placed, plus the count that
/// cannot.
///
/// Pass 1 is [`node_file`] over `graphs[..].nodes[..]` in declaration order.
/// Pass 2 is a SINGLE adjacency hop over `ADJACENCY_EDGES`: an edge with
/// exactly one placed endpoint lends its file to the other. One pass only — a
/// fixpoint would drag `REGION` and `DOC_SECTION` nodes (no POSITION cell,
/// arbitrary neighbours) into whichever service reached them first.
fn file_index(merged: &MergedGraph) -> (HashMap<NodeId, String>, usize) {
    let mut idx: HashMap<NodeId, String> = HashMap::new();
    let mut all: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            // A PROJECT anchor (A8.5) is a service boundary, not a member of a
            // service. It is never placed and never counted as unlocated.
            if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT) {
                continue;
            }
            all.insert(n.id);
            if !idx.contains_key(&n.id) {
                if let Some(f) = node_file(n) {
                    idx.insert(n.id, f);
                }
            }
        }
    }

    let mut cand: HashMap<NodeId, Vec<(String, u64)>> = HashMap::new();
    let edges = merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(merged.cross_edges.iter());
    for e in edges {
        if !ADJACENCY_EDGES.contains(&e.category) {
            continue;
        }
        match (idx.get(&e.from), idx.get(&e.to)) {
            (Some(f), None) => cand.entry(e.to).or_default().push((f.clone(), e.from.0)),
            (None, Some(f)) => cand.entry(e.from).or_default().push((f.clone(), e.to.0)),
            _ => {}
        }
    }
    // Each entry writes its own key and its value is decided by the sort, so
    // the `HashMap` iteration order here cannot reach the output.
    for (id, mut v) in cand {
        v.sort();
        if let Some((f, _)) = v.first() {
            idx.entry(id).or_insert_with(|| f.clone());
        }
    }

    let unlocated = all.iter().filter(|id| !idx.contains_key(id)).count();
    (idx, unlocated)
}

// ============================================================================
// 2. what is a service?
// ============================================================================

/// How node files are partitioned into services.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceKeying {
    /// One service per repo — only correct when ≥2 repos were merged.
    PerRepo,
    /// First path segment of the file (`web/src/api.ts` → `web`). The
    /// single-repo default: a `generate_one` monorepo otherwise renders as one
    /// service with zero links.
    TopLevelDir,
    /// Explicit project roots, longest prefix wins. [`default_keying`] selects
    /// this for a single repo whose graph carries PROJECT anchors below its
    /// root (A8.5). A file under none of them lands in [`OUTSIDE_PROJECTS`].
    ProjectRoots(Vec<String>),
}

impl ServiceKeying {
    pub fn name(&self) -> &'static str {
        match self {
            Self::PerRepo => "per_repo",
            Self::TopLevelDir => "top_level_dir",
            Self::ProjectRoots(_) => "project_roots",
        }
    }
}

/// `PerRepo` when ≥2 distinct repos are merged. For a single repo,
/// `ProjectRoots` when it has PROJECT anchors below its root, else
/// `TopLevelDir`.
pub fn default_keying(merged: &MergedGraph) -> ServiceKeying {
    let repos: BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    if repos.len() >= 2 {
        return ServiceKeying::PerRepo;
    }
    let roots = project_root_paths(merged);
    if roots.is_empty() {
        ServiceKeying::TopLevelDir
    } else {
        ServiceKeying::ProjectRoots(roots)
    }
}

/// The repo-relative dir of every PROJECT anchor (A8.5), sorted. The dirs come
/// from the qname `project:<rel_path>`, which the nav carries on a fresh build
/// and on a loaded `.gmap` alike. The repo root (`project:.`) is left out:
/// every file is under it, so a repo whose only manifest is at the top keeps
/// `TopLevelDir`. Platform-host shells ([`platform_host_roots`]) are left out
/// too (LA.5), so their files fall to the owning app by longest prefix.
fn project_root_paths(merged: &MergedGraph) -> Vec<String> {
    let hosts = platform_host_roots(merged);
    let mut roots = BTreeSet::new();
    for g in &merged.graphs {
        for (id, kind) in &g.nav.kind_by_id {
            if *kind != node_kind::PROJECT {
                continue;
            }
            let path = g.nav.qname_by_id.get(id).and_then(|q| q.strip_prefix("project:"));
            if let Some(p) = path.filter(|p| !p.is_empty() && *p != "." && !hosts.contains_key(*p)) {
                roots.insert(p.to_string());
            }
        }
    }
    roots.into_iter().collect()
}

// ----------------------------------------------------------------------------
// 2a. platform-host shells (LA.5)
//
// A cross-platform app framework generates native shells INSIDE the app —
// Flutter's `android/` `ios/` `macos/` `linux/` `windows/`, React Native's
// `android/` `ios/` — and each shell carries its own build manifest, so A8.5
// rightly emits a PROJECT anchor for it (`glia projects` and `--scope` name a
// Gradle build legitimately). As a SERVICE it is wrong: `quokka_android/
// android/app` listed as a one-file service holding Flutter's generated
// MainActivity.kt. The fold is structural, never a size threshold — a shell
// with real code is still not a service, and a one-file lambda still is.
// ----------------------------------------------------------------------------

/// Native shells a cross-platform app framework generates inside the app
/// (Flutter: all five; React Native / Capacitor: `android`, `ios`).
const PLATFORM_HOST_DIRS: &[&str] = &["android", "ios", "macos", "linux", "windows"];
/// Ecosystems whose project owns such shells (Flutter's `pubspec.yaml`, React
/// Native's / Capacitor's `package.json`).
const HOST_OWNER_ECOSYSTEMS: &[&str] = &["dart", "npm"];
/// Ecosystems a shell's own build manifest uses (`build.gradle[.kts]`, a
/// `CMakeLists.txt` with `project(`, `Package.swift`, `pom.xml`).
const HOST_SHELL_ECOSYSTEMS: &[&str] = &["gradle", "cmake", "swift", "maven"];

/// `path` relative to `dir` when `dir` is a PROPER ancestor directory of it;
/// `.` is the repo root and an ancestor of every other path. Segment-bounded:
/// `mobilex/android` is not under `mobile`.
fn rel_under<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    if dir == "." {
        return (path != ".").then_some(path);
    }
    path.strip_prefix(dir)?
        .strip_prefix('/')
        .filter(|rest| !rest.is_empty())
}

/// Platform-host shell root → the app root that owns it, sorted by host path.
///
/// A root `R` whose ecosystem is in [`HOST_SHELL_ECOSYSTEMS`] is a host of app
/// `A` iff `A` is the NEAREST root with an ecosystem in
/// [`HOST_OWNER_ECOSYSTEMS`] that is a proper ancestor of `R` (`A` may be the
/// repo root `.`), the first component of `R` below `A` is in
/// [`PLATFORM_HOST_DIRS`], and every root strictly between the two is itself a
/// host — so once the hosts leave the keyed root list, `service_of`'s longest
/// prefix really does land a host file in `A`. Nested shells therefore fold
/// straight to the app (`mobile/android/app → mobile`), and a Gradle plugin in
/// an npm `tools/plugin` dir stays a service.
///
/// Reads only PROJECT nodes and their ORIGIN cells (via
/// [`crate::answers::project_roots`]), so a fresh build and a loaded `.gmap`
/// agree. The rule itself is [`platform_host_owners`].
fn platform_host_roots(merged: &MergedGraph) -> BTreeMap<String, String> {
    let roots = crate::answers::project_roots(merged);
    let pairs: Vec<(&str, &str)> =
        roots.iter().map(|r| (r.path.as_str(), r.ecosystem.as_str())).collect();
    platform_host_owners(&pairs)
}

/// The pure core of [`platform_host_roots`], over `(path, ecosystem)` pairs
/// with the repo root spelled `.`. Shared with the LB.4a HTTP owner pass
/// (`crate::http_owner`), which reads the walk's roots before any graph
/// exists, so an owner segment names exactly the service `glia arch` prints.
///
/// The pairs are sorted by path here, so an ancestor is always decided before
/// its descendants whatever order the caller passes them in.
pub(crate) fn platform_host_owners(roots: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut roots: Vec<(&str, &str)> = roots.to_vec();
    roots.sort_unstable();
    let owners: Vec<&str> = roots
        .iter()
        .filter(|(_, eco)| HOST_OWNER_ECOSYSTEMS.contains(eco))
        .map(|(path, _)| *path)
        .collect();
    let mut hosts: BTreeMap<String, String> = BTreeMap::new();
    for &(path, eco) in &roots {
        if !HOST_SHELL_ECOSYSTEMS.contains(&eco) {
            continue;
        }
        // Nearest owner = the deepest ancestor; the root `.` is the shallowest.
        let nearest = owners
            .iter()
            .filter_map(|o| rel_under(path, o).map(|rest| (*o, rest)))
            .max_by_key(|(o, _)| if *o == "." { 0 } else { o.len() });
        let Some((owner, rest)) = nearest else { continue };
        let first = rest.split('/').next().unwrap_or(rest);
        if !PLATFORM_HOST_DIRS.contains(&first) {
            continue;
        }
        let between_all_hosts = roots.iter().all(|&(m, _)| {
            m == path
                || m == owner
                || rel_under(path, m).is_none()
                || rel_under(m, owner).is_none()
                || hosts.contains_key(m)
        });
        if between_all_hosts {
            hosts.insert(path.to_string(), owner.to_string());
        }
    }
    hosts
}

/// The one service id, under `ProjectRoots` keying, for every file under no
/// declared root (A8.6). Before it, each top-level dir outside a project
/// (`docs`, `scripts`, `.ai`, `(root)`) was listed as a pseudo-service beside
/// the real projects. One bucket keeps that code PLACED — its files, nodes,
/// languages and any link from it still count — without inventing services.
const OUTSIDE_PROJECTS: &str = "(outside projects)";

/// The service id a `file` in `repo` belongs to under `keying`.
///
/// `TopLevelDir` / `ProjectRoots` ids are prefixed with the repo label once
/// ≥2 repos are in play, so two merged repos that both have a `web/` stay
/// distinct.
pub fn service_of(
    file: &str,
    repo: u64,
    keying: &ServiceKeying,
    labels: &BTreeMap<u64, String>,
) -> String {
    match keying {
        ServiceKeying::PerRepo => labels
            .get(&repo)
            .cloned()
            .unwrap_or_else(|| format!("repo{repo}")),
        ServiceKeying::TopLevelDir => prefixed(top_level_dir(file), repo, labels),
        ServiceKeying::ProjectRoots(roots) => {
            let best = roots
                .iter()
                .filter(|r| file == r.as_str() || file.starts_with(&format!("{r}/")))
                .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.as_str().cmp(b.as_str())));
            // A file under no declared root is still PLACED, never dropped.
            let key = best.cloned().unwrap_or_else(|| OUTSIDE_PROJECTS.to_string());
            prefixed(key, repo, labels)
        }
    }
}

fn top_level_dir(file: &str) -> String {
    match file.split_once('/') {
        Some((head, _)) if !head.is_empty() => head.to_string(),
        _ => "(root)".to_string(),
    }
}

fn prefixed(key: String, repo: u64, labels: &BTreeMap<u64, String>) -> String {
    if labels.len() >= 2 {
        if let Some(l) = labels.get(&repo) {
            return format!("{l}/{key}");
        }
    }
    key
}

// ============================================================================
// 2b. repo labels — `RepoId::from_canonical` xxhashes the path away, so the
// human name has to be captured where the path and the id still coexist.
// ============================================================================

/// Path split into non-empty components, canonicalised where the path exists.
fn path_components(path: &str) -> Vec<String> {
    let s = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    s.trim_end_matches('/')
        .split('/')
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .map(str::to_string)
        .collect()
}

fn label_at(comps: &[String], depth: usize) -> String {
    if comps.is_empty() {
        return "repo".to_string();
    }
    let take = depth.clamp(1, comps.len());
    comps[comps.len() - take..].join("/")
}

/// The human label for one repo path: its directory name.
pub fn repo_label_for(path: &str) -> String {
    label_at(&path_components(path), 1)
}

/// `RepoId.0 → human label` for a set of `(repo id, repo path)` pairs, with
/// collisions disambiguated by widening leftwards one path component at a time
/// (`api` → `x/api`) and, if the paths are exhausted, by an `#nnn` suffix.
/// Widening is applied to EVERY member of a colliding group, so two `api`
/// repos become `x/api` and `y/api` — never `api` and `y/api`.
pub fn repo_label_map(entries: &[(u64, String)]) -> BTreeMap<u64, String> {
    let mut order: Vec<(u64, Vec<String>)> = Vec::new();
    for (id, path) in entries {
        if order.iter().any(|(i, _)| i == id) {
            continue;
        }
        order.push((*id, path_components(path)));
    }
    let mut depth = vec![1usize; order.len()];
    for _ in 0..16 {
        let labels: Vec<String> = order
            .iter()
            .zip(&depth)
            .map(|((_, c), d)| label_at(c, *d))
            .collect();
        let mut dup = vec![false; labels.len()];
        for i in 0..labels.len() {
            for j in (i + 1)..labels.len() {
                if labels[i] == labels[j] {
                    dup[i] = true;
                    dup[j] = true;
                }
            }
        }
        if !dup.iter().any(|d| *d) {
            break;
        }
        let mut changed = false;
        for (i, (_, comps)) in order.iter().enumerate() {
            if dup[i] && depth[i] < comps.len() {
                depth[i] += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = BTreeMap::new();
    let mut used: BTreeMap<String, u64> = BTreeMap::new();
    for (i, (id, comps)) in order.iter().enumerate() {
        let mut label = label_at(comps, depth[i]);
        if used.get(&label).is_some_and(|prev| prev != id) {
            label = format!("{label}#{}", id % 1000);
        }
        used.insert(label.clone(), *id);
        out.insert(*id, label);
    }
    out
}

// ============================================================================
// 3. the rendered map
// ============================================================================

/// The directional mechanisms — a link over one of these is a real call from
/// `from` to `to`. Everything else `cross_links` returns (the six `SHARES_*`)
/// is a co-ownership signal, still emitted but flagged by `mechanism` so a
/// renderer can hide it by default.
///
/// LA.17: RPC_CALLS (tRPC, Connect and Twirp call -> procedure) is a flow too,
/// so `glia arch` shows those links without `--include-shared`.
pub const FLOW_MECHANISMS: &[EdgeCategoryId] = &[
    edge_category::HTTP_CALLS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::QUEUE_FLOWS,
    edge_category::GRAPHQL_CALLS,
    edge_category::WS_CONNECTS,
    edge_category::EVENT_FLOWS,
    edge_category::CLI_INVOKES,
];

#[derive(serde::Serialize, Debug, Clone, Default)]
#[non_exhaustive]
pub struct ServiceSummary {
    pub id: String,
    /// Human repo label of the first node placed here.
    pub repo: String,
    pub languages: Vec<&'static str>,
    pub files: usize,
    pub nodes: usize,
    pub routes: usize,
    pub endpoints: usize,
    pub cli_commands: usize,
    pub queue_consumers: usize,
    /// Number of surviving LINK ROWS pointing at / leaving this service (not
    /// the underlying edge count — that is `ServiceLink.count`).
    pub inbound: usize,
    pub outbound: usize,
}

#[derive(serde::Serialize, Debug, Clone, Default)]
#[non_exhaustive]
pub struct ServiceLink {
    pub from: String,
    pub to: String,
    pub mechanism: &'static str,
    pub channel: String,
    pub count: usize,
    pub confidence: &'static str,
    pub example_from_qname: String,
    pub example_to_qname: String,
}

/// The whole-stack view [`service_map`] / [`service_map_with`] return. Outside
/// this crate it is built by `Default` plus field assignment, never by a
/// struct literal (LD.9):
///
/// ```compile_fail
/// let _ = glia_engine::ServiceMap {
///     keying: "",
///     services: vec![],
///     links: vec![],
///     self_links: 0,
///     unlocated_nodes: 0,
/// };
/// ```
#[derive(serde::Serialize, Debug, Default)]
#[non_exhaustive]
pub struct ServiceMap {
    /// Which rule produced the ids — a consumer cannot otherwise tell
    /// `apps` (top-level dir) from `apps/web` (project roots).
    pub keying: &'static str,
    pub services: Vec<ServiceSummary>,
    pub links: Vec<ServiceLink>,
    /// Links dropped because both ends landed in the same service.
    pub self_links: usize,
    /// Nodes no tier and no adjacency hop could place.
    pub unlocated_nodes: usize,
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

/// The service map under [`default_keying`].
pub fn service_map(merged: &MergedGraph, repo_labels: &BTreeMap<u64, String>) -> ServiceMap {
    service_map_with(merged, repo_labels, &default_keying(merged))
}

/// The service map under an explicit keying.
pub fn service_map_with(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    keying: &ServiceKeying,
) -> ServiceMap {
    let (files, unlocated_nodes) = file_index(merged);

    struct Acc {
        repo: String,
        files: BTreeSet<String>,
        nodes: usize,
        routes: usize,
        endpoints: usize,
        cli_commands: usize,
        queue_consumers: usize,
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut svc_of: HashMap<NodeId, String> = HashMap::new();

    for g in &merged.graphs {
        let repo = g.repo.0;
        let repo_label = repo_labels
            .get(&repo)
            .cloned()
            .unwrap_or_else(|| format!("repo{repo}"));
        for n in &g.nodes {
            let Some(file) = files.get(&n.id) else { continue };
            let id = service_of(file, repo, keying, repo_labels);
            svc_of.entry(n.id).or_insert_with(|| id.clone());
            let e = acc.entry(id).or_insert_with(|| Acc {
                repo: repo_label.clone(),
                files: BTreeSet::new(),
                nodes: 0,
                routes: 0,
                endpoints: 0,
                cli_commands: 0,
                queue_consumers: 0,
            });
            e.files.insert(file.clone());
            e.nodes += 1;
            match g.nav.kind_by_id.get(&n.id).copied() {
                Some(k) if k == node_kind::ROUTE => e.routes += 1,
                Some(k) if k == node_kind::ENDPOINT => e.endpoints += 1,
                Some(k) if k == node_kind::CLI_COMMAND => e.cli_commands += 1,
                Some(k) if k == node_kind::QUEUE_CONSUMER => e.queue_consumers += 1,
                _ => {}
            }
        }
    }

    let (raw, unplaced) = cross_links(merged, &|id| svc_of.get(&id).cloned());
    let buckets = raw.len();
    let mut self_links = 0usize;
    let mut links: Vec<ServiceLink> = Vec::new();
    for l in raw {
        if l.from == l.to {
            self_links += 1;
            continue;
        }
        links.push(ServiceLink {
            from: l.from,
            to: l.to,
            mechanism: l.mechanism,
            channel: l.channel,
            count: l.count,
            confidence: confidence_name(l.confidence),
            example_from_qname: l.example_from_qname,
            example_to_qname: l.example_to_qname,
        });
    }

    let mut io: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for l in &links {
        io.entry(l.from.as_str()).or_default().1 += 1;
        io.entry(l.to.as_str()).or_default().0 += 1;
    }

    let services: Vec<ServiceSummary> = acc
        .iter()
        .map(|(id, a)| {
            let mut languages: Vec<&'static str> = a
                .files
                .iter()
                .filter_map(|f| crate::coverage::ext_to_language(f))
                .collect();
            languages.sort_unstable();
            languages.dedup();
            let (inbound, outbound) = io.get(id.as_str()).copied().unwrap_or((0, 0));
            ServiceSummary {
                id: id.clone(),
                repo: a.repo.clone(),
                languages,
                files: a.files.len(),
                nodes: a.nodes,
                routes: a.routes,
                endpoints: a.endpoints,
                cli_commands: a.cli_commands,
                queue_consumers: a.queue_consumers,
                inbound,
                outbound,
            }
        })
        .collect();

    // LA.5 fired_on marker: every platform-host shell that is NOT a keyed root
    // under `ProjectRoots`, so its files were counted under the owning app.
    // Silent when nothing folded, and under `PerRepo` / `TopLevelDir` (no
    // PROJECT root keys a service there, so there is nothing to fold).
    if let ServiceKeying::ProjectRoots(keyed) = keying {
        let folded: Vec<String> = platform_host_roots(merged)
            .into_iter()
            .filter(|(host, _)| !keyed.contains(host))
            .map(|(host, app)| format!("{host}->{app}"))
            .collect();
        if !folded.is_empty() {
            eprintln!(
                "[arch] platform hosts folded: {} ({})",
                folded.len(),
                folded.join(", ")
            );
        }
    }

    // fired_on marker. Carries A9.1's `buckets` / `unplaced` too — `cross_links`
    // has no marker of its own, and without them a dead `cross_links` would
    // still print a plausible `[arch] 2 services, 0 links`.
    eprintln!(
        "[arch] {} services, {} links (keying={} buckets={} unplaced={} self={} unlocated={})",
        services.len(),
        links.len(),
        keying.name(),
        buckets,
        unplaced,
        self_links,
        unlocated_nodes,
    );

    ServiceMap {
        keying: keying.name(),
        services,
        links,
        self_links,
        unlocated_nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::{Cell, RepoId};

    fn node(cells: Vec<Cell>) -> Node {
        Node {
            id: NodeId(1),
            repo: RepoId(1),
            confidence: Confidence::Strong,
            cells,
        }
    }

    fn cell(kind: glia_core::CellTypeId, payload: CellPayload) -> Cell {
        Cell { kind, payload }
    }

    fn labels(n: usize) -> BTreeMap<u64, String> {
        let mut m = BTreeMap::new();
        for i in 0..n {
            m.insert(i as u64, format!("r{i}"));
        }
        m
    }

    #[test]
    fn node_file_prefers_position() {
        let n = node(vec![
            cell(
                cell_type::ENDPOINT_HIT,
                CellPayload::Json(r#"{"method":"GET","path":"/u","file":"z.ts","line":1}"#.into()),
            ),
            cell(
                cell_type::POSITION,
                CellPayload::Json(r#"{"file":"a/b.go","start_line":3,"end_line":9}"#.into()),
            ),
        ]);
        assert_eq!(node_file(&n).as_deref(), Some("a/b.go"));
    }

    #[test]
    fn node_file_reads_endpoint_hit_and_go_route_method() {
        let hit = node(vec![cell(
            cell_type::ENDPOINT_HIT,
            CellPayload::Json(
                r#"{"method":"GET","path":"/users","file":"client/api.ts","line":4,"col":2,"confidence":"strong"}"#
                    .into(),
            ),
        )]);
        assert_eq!(node_file(&hit).as_deref(), Some("client/api.ts"));

        let go_route = node(vec![cell(
            cell_type::ROUTE_METHOD,
            CellPayload::Json(r#"{"method":"GET","handler":null,"file":"server/main.go","line":7,"col":1}"#.into()),
        )]);
        assert_eq!(node_file(&go_route).as_deref(), Some("server/main.go"));

        // The other 14 ROUTE_METHOD emitters write a bare method string. It
        // MUST fall through to None, not panic.
        let text_route = node(vec![cell(
            cell_type::ROUTE_METHOD,
            CellPayload::Text("GET".into()),
        )]);
        assert_eq!(node_file(&text_route), None);
    }

    #[test]
    fn service_of_top_level_dir() {
        let l = labels(1);
        assert_eq!(
            service_of("web/src/api.ts", 0, &ServiceKeying::TopLevelDir, &l),
            "web"
        );
        assert_eq!(
            service_of("main.go", 0, &ServiceKeying::TopLevelDir, &l),
            "(root)"
        );
        // ≥2 repos → the id is repo-qualified so two `web/`s stay distinct.
        let l2 = labels(2);
        assert_eq!(
            service_of("web/src/api.ts", 1, &ServiceKeying::TopLevelDir, &l2),
            "r1/web"
        );
    }

    #[test]
    fn service_of_project_roots_longest_prefix() {
        let k = ServiceKeying::ProjectRoots(vec!["services".into(), "services/api".into()]);
        let l = labels(1);
        assert_eq!(service_of("services/api/main.go", 0, &k, &l), "services/api");
        assert_eq!(service_of("services/other/x.go", 0, &k, &l), "services");
        // No declared root matches → ONE shared bucket, still placed (A8.6):
        // `tools/`, `docs/` and top-level files are not pseudo-services.
        assert_eq!(service_of("tools/z.py", 0, &k, &l), "(outside projects)");
        assert_eq!(service_of("docs/a.md", 0, &k, &l), "(outside projects)");
        assert_eq!(service_of("main.go", 0, &k, &l), "(outside projects)");
        // `servicesX/` is not under `services/` — segment boundary, not prefix.
        assert_eq!(service_of("servicesX/y.go", 0, &k, &l), "(outside projects)");
        // Two merged repos: the bucket is repo-qualified like any other id.
        assert_eq!(service_of("tools/z.py", 1, &k, &labels(2)), "r1/(outside projects)");
        assert_eq!(k.name(), "project_roots");
    }

    /// A8.5: only PROJECT anchors BELOW the repo root switch a single repo
    /// to `ProjectRoots`. A second repo still wins with `PerRepo`.
    #[test]
    fn default_keying_reads_project_anchors() {
        use glia_code_domain::project_roots::ProjectRoot;

        let root_only = [ProjectRoot::new(String::new(), "go", "go.mod", Some("m".into()))];
        let nested = [
            ProjectRoot::new(String::new(), "npm", "package.json", None),
            ProjectRoot::new("services/web".into(), "npm", "package.json", None),
            ProjectRoot::new("services/api".into(), "go", "go.mod", None),
        ];
        let graph = |roots: &[ProjectRoot], repo: u64| {
            crate::walk::build_project_graph(roots, RepoId(repo))
        };

        let m = MergedGraph::new(vec![graph(&root_only, 1)]);
        assert_eq!(default_keying(&m), ServiceKeying::TopLevelDir);

        let m = MergedGraph::new(vec![graph(&nested, 1)]);
        assert_eq!(
            default_keying(&m),
            ServiceKeying::ProjectRoots(vec!["services/api".into(), "services/web".into()])
        );
        let (idx, unlocated) = file_index(&m);
        assert_eq!((idx.len(), unlocated), (0, 0), "anchors are neither placed nor unlocated");

        let m = MergedGraph::new(vec![graph(&nested, 1), graph(&root_only, 2)]);
        assert_eq!(default_keying(&m), ServiceKeying::PerRepo);
    }

    /// LA.5: a Flutter app's native shells (Gradle `android`, `android/app`,
    /// CMake `linux`) fold into the app; a Gradle plugin under an npm dir
    /// that is not a platform dir stays its own root.
    #[test]
    fn platform_hosts_fold_into_their_app() {
        use glia_code_domain::project_roots::ProjectRoot;

        let graph = |roots: &[ProjectRoot]| {
            MergedGraph::new(vec![crate::walk::build_project_graph(roots, RepoId(1))])
        };
        let stack = [
            ProjectRoot::new("mobile".into(), "dart", "pubspec.yaml", Some("mobile_app".into())),
            ProjectRoot::new("mobile/android".into(), "gradle", "build.gradle.kts", None),
            ProjectRoot::new("mobile/android/app".into(), "gradle", "build.gradle.kts", None),
            ProjectRoot::new("mobile/linux".into(), "cmake", "CMakeLists.txt", Some("runner".into())),
            ProjectRoot::new("tools".into(), "npm", "package.json", Some("tools".into())),
            ProjectRoot::new("tools/plugin".into(), "gradle", "build.gradle", None),
            ProjectRoot::new("server".into(), "go", "go.mod", Some("example.com/server".into())),
        ];
        let m = graph(&stack);
        let hosts: Vec<(String, String)> = platform_host_roots(&m).into_iter().collect();
        assert_eq!(
            hosts,
            [
                ("mobile/android".to_string(), "mobile".to_string()),
                ("mobile/android/app".to_string(), "mobile".to_string()),
                ("mobile/linux".to_string(), "mobile".to_string()),
            ]
        );
        assert_eq!(
            default_keying(&m),
            ServiceKeying::ProjectRoots(vec![
                "mobile".into(),
                "server".into(),
                "tools".into(),
                "tools/plugin".into(),
            ])
        );

        // A root-level Flutter app whose only sub-roots are its shells has no
        // keyed root left, so it gets the same keying as any single-root repo.
        let root_app = [
            ProjectRoot::new(String::new(), "dart", "pubspec.yaml", Some("app".into())),
            ProjectRoot::new("android".into(), "gradle", "build.gradle.kts", None),
            ProjectRoot::new("android/app".into(), "gradle", "build.gradle.kts", None),
        ];
        let m = graph(&root_app);
        assert_eq!(platform_host_roots(&m).len(), 2);
        assert_eq!(platform_host_roots(&m).get("android/app").map(String::as_str), Some("."));
        assert_eq!(default_keying(&m), ServiceKeying::TopLevelDir);

        // A non-shell root between the app and a shell keeps the shell keyed:
        // folding it would claim `mobile` while longest prefix lands the file
        // in `mobile/android`.
        let blocked = [
            ProjectRoot::new("mobile".into(), "dart", "pubspec.yaml", None),
            ProjectRoot::new("mobile/android".into(), "go", "go.mod", None),
            ProjectRoot::new("mobile/android/app".into(), "gradle", "build.gradle", None),
        ];
        assert!(platform_host_roots(&graph(&blocked)).is_empty());
    }

    /// LB.4a: the pure core answers from `(path, ecosystem)` pairs exactly
    /// what the graph fold answers from PROJECT nodes, whatever the pair order.
    #[test]
    fn platform_host_owners_matches_the_graph_fold_in_any_order() {
        use glia_code_domain::project_roots::ProjectRoot;

        let stack = [
            ProjectRoot::new("mobile".into(), "dart", "pubspec.yaml", None),
            ProjectRoot::new("mobile/android".into(), "gradle", "build.gradle.kts", None),
            ProjectRoot::new("mobile/android/app".into(), "gradle", "build.gradle.kts", None),
            ProjectRoot::new("server".into(), "go", "go.mod", None),
        ];
        let m = MergedGraph::new(vec![crate::walk::build_project_graph(&stack, RepoId(1))]);
        let reversed: Vec<(&str, &str)> =
            stack.iter().rev().map(|r| (r.rel_path.as_str(), r.ecosystem)).collect();
        assert_eq!(platform_host_owners(&reversed), platform_host_roots(&m));
        assert_eq!(platform_host_owners(&reversed).len(), 2);
        assert!(platform_host_owners(&[]).is_empty());
    }

    #[test]
    fn repo_label_disambiguates_collision() {
        let m = repo_label_map(&[(7, "/tmp/x/api".into()), (9, "/tmp/y/api".into())]);
        assert_eq!(m.get(&7).map(String::as_str), Some("x/api"));
        assert_eq!(m.get(&9).map(String::as_str), Some("y/api"));
        // Single repo keeps the short label.
        let one = repo_label_map(&[(7, "/tmp/x/api".into())]);
        assert_eq!(one.get(&7).map(String::as_str), Some("api"));
        assert_eq!(repo_label_for("/tmp/x/api/"), "api");
    }

    /// LA.17: an RPC link (tRPC, Connect, Twirp call -> procedure) is a call
    /// flow, so the default `glia arch` view keeps it.
    #[test]
    fn rpc_calls_is_a_flow_mechanism() {
        assert!(FLOW_MECHANISMS.contains(&edge_category::RPC_CALLS));
        assert!(
            FLOW_MECHANISMS
                .iter()
                .all(|c| !edge_category::name(*c).starts_with("SHARES_")),
            "co-ownership signals stay out of the flow table"
        );
    }
}
