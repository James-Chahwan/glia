//! Host narrowing, shared by the channel resolvers (CB.12): the service alias
//! index (A11.4 / LB.4b), the interned owner table, the ENDPOINT_HIT host
//! reader and [`narrow_by_host`], generic over the target a resolver pairs.
//!
//! The HTTP resolver built all of it as private items of `http.rs`; they
//! moved here unchanged, renamed only where the name was HTTP-specific
//! (`RouteOwners` -> [`Owners`], `endpoint_hosts` -> [`hit_hosts`],
//! `enclosing_project` -> [`owner_of_file`]). A resolver opts in by
//! implementing [`HostScoped`] for its target: the repo it lives in and its
//! owner (the nested project, an index into the resolver's [`Owners`]).
//!
//! Resolver-private: every item is `pub(crate)` at most, so the graph crate's
//! `pub use resolvers::*` never gains a name from here.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, NodeId, RepoId};

use crate::types::RepoGraph;

// ============================================================================
// Owners
// ============================================================================

/// LB.4a: the owner segments of every indexed target, interned, so a target
/// carries a `u32` instead of a string. A resolver builds one per resolve,
/// in index order (HTTP: alongside the route index).
#[derive(Debug, Default)]
pub(crate) struct Owners {
    names: Vec<String>,
    by_name: HashMap<String, u32>,
}

impl Owners {
    /// The index of `owner`, interning it on first sight. `None` only past
    /// `u32::MAX` distinct owners, which a repo cannot reach.
    pub(crate) fn intern(&mut self, owner: &str) -> Option<u32> {
        if let Some(&i) = self.by_name.get(owner) {
            return Some(i);
        }
        let i = u32::try_from(self.names.len()).ok()?;
        self.names.push(owner.to_string());
        self.by_name.insert(owner.to_string(), i);
        Some(i)
    }

    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }

    /// The owner interned as `i`. Only tests read an owner back by index:
    /// a resolver keys on the `u32` and names owners through [`Owners::id_of`].
    #[cfg(test)]
    pub(crate) fn name(&self, i: u32) -> Option<&str> {
        self.names.get(i as usize).map(String::as_str)
    }

    /// LB.4b: the index of the nested project at `rel` (a PROJECT qname's
    /// repo-relative path), or `None` when no indexed target is owned by it.
    pub(crate) fn id_of(&self, rel: &str) -> Option<u32> {
        self.by_name.get(owner_spelling(rel).as_ref()).copied()
    }
}

/// A project rel as the engine writes it into an owner segment: verbatim
/// unless it holds whitespace or a `%`, which are percent-escaped so the
/// segment survives `split_owner`. The twin of `owner_segment` in
/// `engine/src/http_owner.rs`; the two must spell a rel identically, or a
/// project whose path holds a space could never be named by a host.
pub(crate) fn owner_spelling(rel: &str) -> Cow<'_, str> {
    if !rel.chars().any(|c| c.is_whitespace() || c == '%') {
        return Cow::Borrowed(rel);
    }
    let mut out = String::with_capacity(rel.len() + 8);
    for c in rel.chars() {
        if c.is_whitespace() || c == '%' {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// The longest nested project rel enclosing `file` (a repo-relative file or
/// directory), segment-bounded (`servicesx` is not under `services`), as the
/// engine's owner pass picks it: the mirror of `engine/src/http_owner.rs`
/// `OwnerIndex::owner_of`, which the graph crate cannot call. `rels` are the
/// nested PROJECT rels of one repo (`project:<rel>` qnames). For a target
/// whose qname carries no owner segment, this is its owner.
pub(crate) fn owner_of_file<'r>(rels: &[&'r str], file: &str) -> Option<&'r str> {
    rels.iter()
        .copied()
        .filter(|r| file == *r || file.strip_prefix(*r).is_some_and(|rest| rest.starts_with('/')))
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

// ============================================================================
// ENDPOINT_HIT payload readers
// ============================================================================

/// The still-escaped `raw` value of an ENDPOINT_HIT payload (A3.3), or None.
pub(crate) fn raw_field(json: &str) -> Option<&str> {
    str_field(json, "raw")
}

/// The still-escaped value of the string field `key` in an ENDPOINT_HIT
/// payload, or None.
///
/// All three writers (`code_domain::endpoint`, the TS parser's serde_json and
/// the engine's endpoint fold) emit compact JSON, so the key is matched WITH
/// its `:"`. Inside any escaped value every `"` is preceded by `\`, so a path
/// that is literally `raw` (`"path":"raw"`) cannot be mistaken for the key, and
/// `"host":"` cannot match inside `"hosts":[`.
pub(crate) fn str_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let rest = &json[json.find(&pat)? + pat.len()..];
    quoted_body(rest)
}

/// `rest` starts just after an opening quote: everything up to the first
/// unescaped quote, or None when it is unterminated.
pub(crate) fn quoted_body(rest: &str) -> Option<&str> {
    let mut escaped = false;
    for (i, b) in rest.bytes().enumerate() {
        match b {
            b'\\' if !escaped => escaped = true,
            b'"' if !escaped => return Some(&rest[..i]),
            _ => escaped = false,
        }
    }
    None
}

// ============================================================================
// A11.4 — the service alias index
// ============================================================================

/// Where an alias points: a whole repo (`None`, A11.4) or one nested project of
/// it (`Some` index into the build's [`Owners`], LB.4b).
pub(crate) type AliasScope = (RepoId, Option<u32>);

/// `normalise_alias(name) -> every scope that declares a service by that
/// name`. A name can be known and point at no scope: the label of a nested
/// project that serves no target. A host naming it is known, so it does not
/// block narrowing, but it can keep no target.
pub(crate) type AliasIndex = HashMap<String, HashSet<AliasScope>>;

/// The INFRA_RESOURCE kinds that NAME a service (`infra:<kind>:<name>`, see
/// `parsers/code/extractors/src/iac.rs`). ConfigMaps, secrets, jobs and
/// ingresses are not addressable as an HTTP host, so they stay out.
const ALIAS_KINDS: &[&str] = &["service", "deployment", "statefulset", "image"];

/// What `dockerfile_image_name` returns for a Dockerfile at a repo's root. It
/// names nothing, and every such repo would share it.
const DEGENERATE_IMAGE: &str = "image";

/// Name endings that do not tell two services apart: `users-service`,
/// `users-svc` and `users-api` are the same service. `_` is folded to `-`
/// before these are tried, so `users_service` is covered too.
const SERVICE_SUFFIXES: &[&str] = &["-service", "-svc", "-api", "-server"];

/// Cluster-internal DNS tails. Only a name ending in one of these is cut back
/// to its first label (`users.default.svc.cluster.local` -> `users`). A dotted
/// PUBLIC hostname is left whole: cutting `api.example.com` to `api` would
/// alias it onto any compose service that happens to be called `api`.
/// (`.svc.cluster.local` ends in `.local`.)
const CLUSTER_DNS_TAILS: &[&str] = &[".svc", ".local"];

/// Every service alias in the merge, keyed by [`normalise_alias`], and how
/// many distinct names a nested project contributed. A name declared in
/// several scopes maps to all of them. That is a real ambiguity, and narrowing
/// then keeps the targets in every one of them.
///
/// Two sources:
/// - LB.4b, a nested PROJECT (qname `project:<rel>`, `rel` not the repo root):
///   its label (the nav name, last `/` segment, so the npm `@shop/web` gives
///   `web` and the Go `github.com/acme/users` gives `users`) and its directory
///   name, both scoped to that project. A project that owns no target (a
///   client app, a platform-host shell) is a known name with no scope.
/// - A11.4, a service-naming INFRA_RESOURCE: scoped to the nested project its
///   declaring file lies in (the MODULE that DEFINES it), and to the whole
///   repo when that file is outside every nested project, or in one that
///   owns no target, so an IaC alias never narrows less than it did before.
///
/// A project scope exists only for an owner in `owners`, the caller's
/// interned target owners (HTTP: the ROUTE owners), so the index is always
/// built per resolver, over that resolver's targets.
///
/// The IacResolver builds its index the same way, and this one stays separate
/// on purpose: that one pairs verbatim qnames, this one keys on a normalised
/// NAME and only for the service-naming kinds.
pub(crate) fn build_service_alias_index(graphs: &[RepoGraph], owners: &Owners) -> (AliasIndex, usize) {
    let mut index = AliasIndex::new();
    let mut project_level: HashSet<String> = HashSet::new();
    let mut nested: HashMap<RepoId, Vec<&str>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PROJECT) {
                continue;
            }
            let Some(rel) = g
                .nav
                .qname_by_id
                .get(&n.id)
                .and_then(|q| q.strip_prefix("project:"))
                .filter(|rel| !rel.is_empty() && *rel != ".")
            else {
                continue;
            };
            nested.entry(g.repo).or_default().push(rel);
            let scope = owners.id_of(rel).map(|o| (g.repo, Some(o)));
            let label = g.nav.name_by_id.get(&n.id).map(String::as_str);
            for name in label.into_iter().chain([rel]) {
                let alias = normalise_alias(last_segment(name));
                if alias.is_empty() {
                    continue;
                }
                let scopes = index.entry(alias.clone()).or_default();
                scopes.extend(scope);
                project_level.insert(alias);
            }
        }
    }
    for g in graphs {
        let declared_in = declaring_modules(g);
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::INFRA_RESOURCE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let Some((kind, name)) = qname
                .strip_prefix("infra:")
                .and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            if !ALIAS_KINDS.contains(&kind) || (kind == "image" && name == DEGENERATE_IMAGE) {
                continue;
            }
            let alias = normalise_alias(name);
            if alias.is_empty() {
                continue;
            }
            let rels = nested.get(&g.repo).map(Vec::as_slice).unwrap_or_default();
            let mut scopes: Vec<Option<u32>> = declared_in
                .get(&n.id)
                .into_iter()
                .flatten()
                .map(|module| {
                    g.nav
                        .qname_by_id
                        .get(module)
                        .and_then(|q| module_dir(q))
                        .and_then(|dir| owner_of_file(rels, &dir))
                        .and_then(|rel| owners.id_of(rel))
                })
                .collect();
            if scopes.is_empty() {
                scopes.push(None);
            }
            if scopes.iter().any(Option::is_some) {
                project_level.insert(alias.clone());
            }
            index
                .entry(alias)
                .or_default()
                .extend(scopes.into_iter().map(|s| (g.repo, s)));
        }
    }
    (index, project_level.len())
}

/// `INFRA_RESOURCE id -> the MODULEs that DEFINE it`, i.e. the files that
/// declare it (`iac.rs` `emit_resource`). IaC nodes carry no POSITION cell,
/// so this edge is where their file is.
fn declaring_modules(g: &RepoGraph) -> HashMap<NodeId, Vec<NodeId>> {
    let mut out: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for e in &g.edges {
        if e.category == edge_category::DEFINES
            && g.nav.kind_by_id.get(&e.to) == Some(&node_kind::INFRA_RESOURCE)
            && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::MODULE)
        {
            out.entry(e.to).or_default().push(e.from);
        }
    }
    out
}

/// The repo-relative directory of a MODULE, from its qname (the file path with
/// the extension dropped and `/` spelled `::`): `services::users::compose`
/// -> `services/users`. `None` for a file at the repo root.
fn module_dir(qname: &str) -> Option<String> {
    qname.rsplit_once("::").map(|(dir, _)| dir.replace("::", "/"))
}

/// The last `/` segment of a name or path.
fn last_segment(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// One spelling for a service name, applied identically to the alias and to
/// the host so `users-service` (compose) and `users-svc` (a client's base URL)
/// meet at `users`:
/// lowercase, `_` -> `-`, a cluster DNS name cut to its first label, then
/// [`SERVICE_SUFFIXES`] stripped for as long as one matches and leaves a
/// non-empty stem (so `users-api-service` and `users-api` also meet).
pub(crate) fn normalise_alias(name: &str) -> String {
    let mut s = name.trim().to_ascii_lowercase().replace('_', "-");
    if CLUSTER_DNS_TAILS.iter().any(|t| s.ends_with(t))
        && let Some((first, _)) = s.split_once('.')
    {
        s = first.to_string();
    }
    while let Some(stem) = SERVICE_SUFFIXES
        .iter()
        .find_map(|suf| s.strip_suffix(suf).filter(|stem| !stem.is_empty()))
    {
        s = stem.to_string();
    }
    s
}

/// `host[:port]` -> `host`. A bracketed IPv6 literal keeps its brackets and
/// never matches an alias, which is the right answer for it.
pub(crate) fn host_name(authority: &str) -> &str {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

// ============================================================================
// The hosts a side's calls go to
// ============================================================================

const HOSTS_KEY: &str = "\"hosts\":[";

/// The `hosts` array of an ENDPOINT_HIT payload: every authority the client's
/// base URL can take, one per deployment binding, `""` for a binding with
/// none. None when the array is malformed.
fn hosts_list(json: &str) -> Option<Vec<&str>> {
    let mut rest = &json[json.find(HOSTS_KEY)? + HOSTS_KEY.len()..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with(']') {
            return Some(out);
        }
        rest = rest.strip_prefix('"')?;
        let value = quoted_body(rest)?;
        out.push(value);
        rest = rest[value.len() + 1..].trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest);
    }
}

/// Every host a client side's calls may go to, or None when any call site
/// gives no evidence.
///
/// Graph build stacks one ENDPOINT_HIT cell per call site on the node, and two
/// call sites on one path can name different services, so the answer is the
/// union over the cells. A cell with a `hosts` array (A11.4's engine side:
/// the base URL is bound differently per deployment) contributes the whole
/// array; otherwise its `host` (A11.2). A cell with neither means one call site
/// goes somewhere unknown, and then nothing may be narrowed.
pub(crate) fn hit_hosts<'c>(cells: impl IntoIterator<Item = &'c Cell>) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut seen_hit = false;
    for c in cells {
        if c.kind != cell_type::ENDPOINT_HIT {
            continue;
        }
        let CellPayload::Json(json) = &c.payload else {
            return None;
        };
        seen_hit = true;
        let hosts = if json.contains(HOSTS_KEY) {
            hosts_list(json)?
        } else {
            vec![str_field(json, "host")?]
        };
        for h in hosts {
            if !out.iter().any(|o| o == h) {
                out.push(h.to_string());
            }
        }
    }
    (seen_hit && !out.is_empty()).then_some(out)
}

// ============================================================================
// Narrowing
// ============================================================================

/// A target [`narrow_by_host`] can keep or drop: the repo it lives in and its
/// owner, the nested project it sits under as an index into the resolver's
/// [`Owners`] (`None` outside every nested project).
pub(crate) trait HostScoped {
    fn repo(&self) -> RepoId;
    fn owner(&self) -> Option<u32>;
}

/// What [`narrow_by_host`] did to one side's target list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Narrowed {
    /// Left alone.
    No,
    /// Cut to whole repos (A11.4).
    Repo,
    /// Cut inside a repo: a dropped target shares its repo with a kept one,
    /// so the host named a nested project (LB.4b).
    Owner,
}

/// Drop the targets that live outside the service the client named.
///
/// A target is kept when a host's alias scopes its repo as a whole
/// (`(repo, None)`) or its own project (`(repo, owner)`). So in a monorepo a
/// host naming the `users` project keeps `GET /health @services/users` and
/// drops `GET /health @services/admin`, the choice LB.4a's owner-qualified
/// ROUTE ids make possible.
///
/// It acts only on positive evidence and otherwise leaves `hits` alone:
/// - no host, or any host (deployment) that is not a known alias: the client
///   may call something the map does not know;
/// - no target in a named scope: the map is incomplete, and losing an edge
///   to it would be worse than keeping a collision.
///
/// With several hosts the scopes are their union, so a client whose dev and
/// prod bases name different services keeps both services' targets. Set
/// lookups only; `hits` keeps its order.
pub(crate) fn narrow_by_host<T: HostScoped>(
    aliases: &AliasIndex,
    hosts: Option<&[String]>,
    hits: &mut Vec<T>,
) -> Narrowed {
    let Some(hosts) = hosts else {
        return Narrowed::No;
    };
    if hits.len() < 2 {
        return Narrowed::No;
    }
    let mut scopes: HashSet<AliasScope> = HashSet::new();
    for h in hosts {
        let Some(named) = aliases.get(&normalise_alias(host_name(h))) else {
            return Narrowed::No;
        };
        scopes.extend(named.iter().copied());
    }
    let keeps = |t: &T| scopes.contains(&(t.repo(), None)) || scopes.contains(&(t.repo(), t.owner()));
    let kept_repos: HashSet<RepoId> = hits.iter().filter(|t| keeps(t)).map(|t| t.repo()).collect();
    let kept = hits.iter().filter(|t| keeps(t)).count();
    if kept == 0 || kept == hits.len() {
        return Narrowed::No;
    }
    let within_repo = hits.iter().any(|t| !keeps(t) && kept_repos.contains(&t.repo()));
    hits.retain(|t| keeps(t));
    if within_repo { Narrowed::Owner } else { Narrowed::Repo }
}

#[cfg(test)]
mod tests {
    use super::super::tests::channel_graph;
    use super::*;
    use glia_core::{Confidence, Edge};

    fn hit(json: &str) -> Cell {
        Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Json(json.into()) }
    }

    /// A3.3 — `raw` is read only as a KEY, and to the first unescaped quote.
    #[test]
    fn raw_field_matches_the_key_not_a_value() {
        let with = r#"{"method":"GET","path":"/users","confidence":"strong","raw":"https://a/users?x=1"}"#;
        assert_eq!(raw_field(with), Some("https://a/users?x=1"));
        let without = r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#;
        assert_eq!(raw_field(without), None);
        // A path that is literally `raw` is a value, not the key.
        let value = r#"{"method":"GET","path":"raw","file":"a.ts"}"#;
        assert_eq!(raw_field(value), None);
        // An escaped quote inside the value does not end it.
        let esc = r##"{"path":"/q","raw":"/q?s=\"x\"#f"}"##;
        assert_eq!(raw_field(esc), Some(r##"/q?s=\"x\"#f"##));
        // Unterminated → None, never a panic.
        assert_eq!(raw_field(r#"{"raw":"abc"#), None);
    }

    /// The degenerate root-Dockerfile image name aliases nothing.
    #[test]
    fn root_dockerfile_image_is_not_an_alias() {
        let g = channel_graph(
            "root-dockerfile",
            &[
                (node_kind::INFRA_RESOURCE, "infra:image:image"),
                (node_kind::INFRA_RESOURCE, "infra:configmap:users"),
                (node_kind::INFRA_RESOURCE, "infra:statefulset:Users_DB"),
            ],
        );
        let (idx, project_level) = build_service_alias_index(std::slice::from_ref(&g), &Owners::default());
        assert_eq!(idx.keys().collect::<Vec<_>>(), vec!["users-db"]);
        assert_eq!(project_level, 0);
    }

    /// Owners interned in the given order.
    fn owners(names: &[&str]) -> Owners {
        let mut o = Owners::default();
        for n in names {
            o.intern(n);
        }
        o
    }

    /// LB.4b: a nested PROJECT contributes its label (last `/` segment, so an
    /// npm scope drops) and its directory name, scoped to its route owner. A
    /// project with no route is a known name with no scope; the repo root is
    /// no alias at all.
    #[test]
    fn project_alias_from_label_and_dir_name() {
        let mut g = channel_graph(
            "project-alias",
            &[
                (node_kind::PROJECT, "project:services/users"),
                (node_kind::PROJECT, "project:apps/storefront"),
                (node_kind::PROJECT, "project:."),
                (node_kind::PROJECT, "project:go/billing"),
                (node_kind::PROJECT, "project:my app"),
            ],
        );
        let r = g.repo;
        let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        let labels = ["users-service", "@shop/web", "root", "github.com/acme/billing-api", "My App"];
        for (id, label) in ids.iter().zip(labels) {
            g.nav.name_by_id.insert(*id, label.to_string());
        }
        let owners = owners(&["services/users", "go/billing", "my%20app"]);
        let (idx, project_level) = build_service_alias_index(std::slice::from_ref(&g), &owners);
        let scopes = |k: &str| {
            let mut v: Vec<AliasScope> = idx.get(k).map(|s| s.iter().copied().collect()).unwrap_or_default();
            v.sort_by_key(|(repo, owner)| (repo.0, *owner));
            v
        };
        assert_eq!(scopes("users"), vec![(r, Some(0))], "label users-service and dir users meet");
        assert_eq!(scopes("billing"), vec![(r, Some(1))], "Go module path: last segment");
        assert_eq!(scopes("my app"), vec![(r, Some(2))], "a spaced rel finds its escaped owner");
        assert!(idx.contains_key("web") && scopes("web").is_empty(), "npm scope dropped; no route, no scope");
        assert!(idx.contains_key("storefront") && scopes("storefront").is_empty());
        assert!(!idx.contains_key("root") && !idx.contains_key("."), "the repo root is no alias");
        let mut keys: Vec<&String> = idx.keys().collect();
        keys.sort();
        assert_eq!(keys, ["billing", "my app", "storefront", "users", "web"]);
        assert_eq!(project_level, 5);
    }

    /// LB.4b: an IaC alias is scoped to the nested project its declaring file
    /// (the MODULE that DEFINES it) lies in, and to the whole repo from the
    /// repo root, from a project that serves no route, or with no file.
    #[test]
    fn infra_alias_is_scoped_by_its_declaring_file() {
        let mut g = channel_graph(
            "infra-alias",
            &[
                (node_kind::PROJECT, "project:services/users"),
                (node_kind::PROJECT, "project:web"),
                (node_kind::MODULE, "services::users::k8s::deploy"),
                (node_kind::MODULE, "docker-compose"),
                (node_kind::MODULE, "web::Dockerfile"),
                (node_kind::MODULE, "services::usersx::compose"),
                (node_kind::INFRA_RESOURCE, "infra:deployment:users-api"),
                (node_kind::INFRA_RESOURCE, "infra:service:orders"),
                (node_kind::INFRA_RESOURCE, "infra:image:frontend"),
                (node_kind::INFRA_RESOURCE, "infra:service:carts"),
                (node_kind::INFRA_RESOURCE, "infra:service:search"),
            ],
        );
        let r = g.repo;
        let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        let defines = |from: NodeId, to: NodeId| Edge {
            from,
            to,
            category: edge_category::DEFINES,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        };
        g.edges = vec![
            defines(ids[2], ids[6]),
            defines(ids[3], ids[7]),
            defines(ids[4], ids[8]),
            defines(ids[5], ids[9]),
        ];
        let owners = owners(&["services/users"]);
        let (idx, _) = build_service_alias_index(std::slice::from_ref(&g), &owners);
        let scopes = |k: &str| {
            let mut v: Vec<AliasScope> = idx.get(k).map(|s| s.iter().copied().collect()).unwrap_or_default();
            v.sort_by_key(|(repo, owner)| (repo.0, *owner));
            v
        };
        assert_eq!(scopes("users"), vec![(r, Some(0))], "declared inside the users project");
        assert_eq!(scopes("orders"), vec![(r, None)], "root compose: the whole repo");
        assert_eq!(scopes("frontend"), vec![(r, None)], "a project with no route falls back to the repo");
        assert_eq!(scopes("carts"), vec![(r, None)], "usersx is not under users");
        assert_eq!(scopes("search"), vec![(r, None)], "no declaring file");
    }

    /// The same cases as the engine's `OwnerIndex::owner_of` test: the
    /// longest segment-bounded rel, a file or a directory alike.
    #[test]
    fn owner_of_file_takes_the_longest_segment_bounded_rel() {
        let rels = ["services", "services/api", "web"];
        assert_eq!(owner_of_file(&rels, "services/api/main.go"), Some("services/api"));
        assert_eq!(owner_of_file(&rels, "services/other/x.go"), Some("services"));
        assert_eq!(owner_of_file(&rels, "services/api"), Some("services/api"));
        assert_eq!(owner_of_file(&rels, "web"), Some("web"));
        assert_eq!(owner_of_file(&rels, "webx/a.ts"), None, "segment boundary, not a string prefix");
        assert_eq!(owner_of_file(&rels, "main.go"), None);
        assert_eq!(owner_of_file(&[], "services/api/main.go"), None);
    }

    #[test]
    fn normalise_alias_meets_compose_k8s_and_client_spellings() {
        for s in [
            "users",
            "Users",
            "users-service",
            "users_service",
            "users-svc",
            "users_svc",
            "users-api",
            "users-server",
            "users-api-service",
            "users-service.default.svc.cluster.local",
            "users.default.svc",
            "users.local",
        ] {
            assert_eq!(normalise_alias(s), "users", "{s}");
        }
        // A public hostname stays whole, so it cannot alias onto `api`.
        assert_eq!(normalise_alias("api.example.com"), "api.example.com");
        assert_eq!(normalise_alias("api"), "api");
        // A bare suffix is a name, not an empty stem.
        assert_eq!(normalise_alias("service"), "service");
        assert_eq!(normalise_alias("-svc"), "-svc");
        assert_eq!(normalise_alias(""), "");
    }

    #[test]
    fn host_name_drops_port_and_userinfo_only() {
        assert_eq!(host_name("users-service:8080"), "users-service");
        assert_eq!(host_name("users-service"), "users-service");
        assert_eq!(host_name("u:p@users:80"), "users");
        assert_eq!(host_name("[::1]:8080"), "[::1]");
        assert_eq!(host_name("[::1]"), "[::1]");
        assert_eq!(host_name(""), "");
    }

    #[test]
    fn endpoint_hit_host_fields_are_read_as_keys() {
        let h = |json: &str| hit_hosts(&[hit(json)]);
        assert_eq!(h(r#"{"path":"/u","host":"a:1"}"#), Some(vec!["a:1".to_string()]));
        // `hosts` wins and is the whole set; `host` is its first entry.
        assert_eq!(
            h(r#"{"host":"a","hosts":["a", "b" ,"a"]}"#),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(h(r#"{"hosts":[]}"#), None);
        // A value that is literally `host` is not the key.
        assert_eq!(h(r#"{"path":"host","raw":"\"host\":\"x\""}"#), None);
        // Malformed arrays are no evidence, never a panic.
        assert_eq!(h(r#"{"host":"a","hosts":["a""#), None);
        assert_eq!(h(r#"{"host":"a","hosts":[1]}"#), None);
        assert_eq!(hit_hosts(&[]), None);
        assert_eq!(str_field(r#"{"host":"a"}"#, "host"), Some("a"));
        assert_eq!(str_field(r#"{"hosts":["a"]}"#, "host"), None);
    }

    /// A target that is not an HTTP route: one id, its repo and its owner.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Plain {
        id: u32,
        repo: RepoId,
        owner: Option<u32>,
    }

    impl HostScoped for Plain {
        fn repo(&self) -> RepoId {
            self.repo
        }
        fn owner(&self) -> Option<u32> {
            self.owner
        }
    }

    /// CB.12: the narrowing is generic. Two targets of one repo in two nested
    /// projects (the ws handlers of `services/chat` and `services/notify`),
    /// their owners interned by the caller and the alias index built over
    /// them, exactly as the HTTP resolver does for its routes: a host naming
    /// `chat-svc` keeps the chat target and reports an owner-level cut; no
    /// host, or an unknown one, keeps both.
    #[test]
    fn narrow_by_host_over_a_plain_target() {
        let mut g = channel_graph(
            "plain-target",
            &[
                (node_kind::PROJECT, "project:services/chat"),
                (node_kind::PROJECT, "project:services/notify"),
                (node_kind::PROJECT, "project:web"),
            ],
        );
        let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        for (id, label) in ids.iter().zip(["chat-service", "notify", "web"]) {
            g.nav.name_by_id.insert(*id, label.to_string());
        }
        let mut owners = Owners::default();
        let chat = owners.intern("services/chat");
        let notify = owners.intern("services/notify");
        assert_eq!((chat, notify, owners.len()), (Some(0), Some(1), 2));
        let (aliases, project_level) = build_service_alias_index(std::slice::from_ref(&g), &owners);
        assert_eq!(project_level, 3, "chat, notify and web");
        let both = || {
            vec![
                Plain { id: 1, repo: g.repo, owner: chat },
                Plain { id: 2, repo: g.repo, owner: notify },
            ]
        };
        let ids = |hits: &[Plain]| hits.iter().map(|t| t.id).collect::<Vec<_>>();

        let mut hits = both();
        let host = vec!["chat-svc:8080".to_string()];
        assert_eq!(narrow_by_host(&aliases, Some(&host), &mut hits), Narrowed::Owner);
        assert_eq!(ids(&hits), [1]);

        let mut hits = both();
        assert_eq!(narrow_by_host(&aliases, None, &mut hits), Narrowed::No);
        assert_eq!(ids(&hits), [1, 2]);

        for unknown in [vec!["billing:9000".to_string()], vec!["web".to_string()]] {
            let mut hits = both();
            assert_eq!(narrow_by_host(&aliases, Some(&unknown), &mut hits), Narrowed::No, "{unknown:?}");
            assert_eq!(ids(&hits), [1, 2], "{unknown:?}");
        }
    }
}
