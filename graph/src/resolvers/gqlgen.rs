//! gqlgen resolver (DT.5f): a schema-first GraphQL field, declared in SDL,
//! HANDLED_BY the Go method gqlgen's generated executable schema calls for it.
//!
//! gqlgen (github.com/99designs/gqlgen) generates `generated.go`, often left
//! uncommitted, which calls `ec.resolvers.Query().Products(ctx, ..)` through
//! the generated `QueryResolver` interface. What the repo keeps is gqlgen's
//! resolver template:
//!
//! ```go
//! type Resolver struct{ .. }                                  // resolver.go
//! func (r *Resolver) Query() QueryResolver { return &queryResolver{r} }
//! type queryResolver struct{ *Resolver }                      // schema.resolvers.go
//! func (r *queryResolver) Products(ctx context.Context, ..) ([]*model.Product, error)
//! ```
//!
//! The SDL (`schema.graphqls`) mints one GRAPHQL_RESOLVER per root field
//! (`graphql_resolver:products @api`) and one per root type
//! (`graphql_resolver:Query @api`), entry nodes with no code behind them.
//! The SDL sits in the graphql group's graph and the Go code in the go
//! group's graph of the same repo, so the join is a cross-graph resolver.
//!
//! THE GATE is the template shape, nothing looser:
//! - a ROOT STRUCT is a Go STRUCT named exactly `queryResolver`,
//!   `mutationResolver` or `subscriptionResolver` with an INHERITS_FROM edge
//!   (the Go builder's embed bind, CI.2a) to a STRUCT named `Resolver` of the
//!   same repo. A `cacheResolver`, or a `queryResolver` without the embed,
//!   pairs nothing.
//! - a FIELD (`graphql_resolver:<f>`) is HANDLED_BY every root-struct METHOD
//!   (nav `parent_of`) whose name folds equal to `f`: ASCII lower-case with
//!   `_` removed, whole names only, so gqlgen's `productById` ->
//!   `ProductByID` pairs and `products` never pairs `ProductByID`. A field
//!   carrying a `<Root>.` prefix (`Query.products`, an SDL re-key) pairs only
//!   that root's struct. One SDL node per distinct field name means a
//!   `products` in both Query and Mutation pairs both structs' `Products`,
//!   each a real implementation. Two methods of ONE struct folding to the
//!   same name (`UserID` / `UserId`) pair neither.
//! - a ROOT TYPE (`graphql_resolver:Query`) is HANDLED_BY the root getter:
//!   the METHOD `Query` of the `Resolver` STRUCT its `queryResolver` embeds
//!   (`bind_split_go_receivers` parents it under the struct even when it
//!   sits in another file of the package).
//! - only SDL resolvers pair: a GRAPHQL_RESOLVER whose POSITION file ends
//!   `.graphql` / `.graphqls` / `.gql`. Code-first families (NestJS,
//!   strawberry, HotChocolate, ...) bind their own HANDLED_BY at the anchor.
//! - OWNER SCOPING: a resolver qualified with an owner (`@api`, LB.8) pairs
//!   only methods whose POSITION file lies under `<owner>/`; an owner-free one
//!   pairs within its repo.
//!
//! Each edge is `GRAPHQL_RESOLVER -HANDLED_BY-> METHOD` with EVIDENCE emitter
//! `resolver:gqlgen` (tier DERIVED) and rule `gqlgen_field` or `gqlgen_root`,
//! pushed in (resolver id, method id) order. Liveness follows: the resolver
//! is an entry kind and HANDLED_BY a carry edge, so the methods and what
//! they call are reached.
//!
//! Not paired: gqlgen's object resolvers (`orderResolver.Customer` for
//! `type Order { customer: Customer }`), since the SDL extractor mints
//! GRAPHQL_RESOLVERs for the root types only. They are counted as
//! `object_structs` (a lower-case-initial STRUCT named `<x>Resolver`
//! embedding `Resolver`, other than the three roots).
//!
//! fired_on marker, once per build that found a root struct:
//!   `[gqlgen] root structs=2 object_structs=0 paired fields=3 roots=2 unpaired=0 ambiguous=0`
//! counting SDL resolvers of the repos holding a root struct: `paired
//! fields` / `roots` paired at least one method, `ambiguous` paired none
//! because of a fold collision, `unpaired` paired none otherwise (the four
//! partition them). grep: `... 2>&1 | grep '^\[gqlgen\] root structs='`.
//! The rule tally follows as `[evidence-rules] resolver=gqlgen ...`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::evidence::locate;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Cell, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};

use super::{CrossGraphResolver, RuleTally, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// The pass name and evidence emitter suffix (`resolver:gqlgen`).
const NAME: &str = "gqlgen";
const RULE_FIELD: &str = "gqlgen_field";
const RULE_ROOT: &str = "gqlgen_root";
/// gqlgen's root types and the struct its template implements each with.
const ROOTS: [(&str, &str); 3] = [
    ("Query", "queryResolver"),
    ("Mutation", "mutationResolver"),
    ("Subscription", "subscriptionResolver"),
];
/// The root resolver struct every template struct embeds.
const RESOLVER: &str = "Resolver";
const QNAME_PREFIX: &str = "graphql_resolver:";
const SDL_EXTS: [&str; 3] = [".graphql", ".graphqls", ".gql"];

/// The `gqlgen` Resolve pass: SDL GRAPHQL_RESOLVER -HANDLED_BY-> the gqlgen
/// template's Go METHOD (module doc).
pub struct GqlgenResolver;

/// A STRUCT `<root>Resolver` embedding one or more `Resolver` STRUCTs.
struct RootStruct {
    /// The root type (`Query`).
    root: &'static str,
    id: NodeId,
    /// The `Resolver` STRUCTs it embeds, in edge order.
    embeds: Vec<NodeId>,
}

/// A METHOD of a root struct or of an embedded `Resolver`.
struct Method<'a> {
    id: NodeId,
    parent: NodeId,
    name: &'a str,
    /// [`fold`] of `name`.
    key: String,
    /// The POSITION file, owner-escaped ([`owner_path`]).
    file: Option<String>,
    confidence: Confidence,
}

/// One SDL GRAPHQL_RESOLVER.
struct SdlResolver<'a> {
    id: NodeId,
    /// The qname minus owner and `graphql_resolver:` (`products`, `Query`).
    name: &'a str,
    owner: Option<&'a str>,
    confidence: Confidence,
}

#[derive(Default)]
struct Stats {
    root_structs: usize,
    object_structs: usize,
    fields: usize,
    roots: usize,
    unpaired: usize,
    ambiguous: usize,
}

impl Stats {
    fn line(&self) -> Option<String> {
        (self.root_structs > 0).then(|| {
            format!(
                "[gqlgen] root structs={} object_structs={} paired fields={} roots={} unpaired={} ambiguous={}",
                self.root_structs,
                self.object_structs,
                self.fields,
                self.roots,
                self.unpaired,
                self.ambiguous
            )
        })
    }
}

impl CrossGraphResolver for GqlgenResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut stats = Stats::default();
        let mut pairs: Vec<(NodeId, NodeId, &'static str, Confidence)> = Vec::new();
        for repo in repos_in_order(&merged.graphs) {
            let graphs: Vec<&RepoGraph> = merged.graphs.iter().filter(|g| g.repo == repo).collect();
            pair_repo(&graphs, &mut pairs, &mut stats);
        }
        pairs.sort_by_key(|&(from, to, ..)| (from.0, to.0));
        pairs.dedup_by_key(|&mut (from, to, ..)| (from, to));
        let mut rules = RuleTally::new(NAME, &[RULE_FIELD, RULE_ROOT]);
        for (from, to, rule, confidence) in pairs {
            merged.cross_edges.push(
                Edge::new(from, to, edge_category::HANDLED_BY, confidence)
                    .with_cell(rules.cell(rule)),
            );
        }
        if let Some(line) = stats.line() {
            eprintln!("{line}");
        }
        rules.report();
    }
}

/// Each repo of `graphs` once, in first-graph order.
fn repos_in_order(graphs: &[RepoGraph]) -> Vec<RepoId> {
    let mut out: Vec<RepoId> = Vec::new();
    for g in graphs {
        if !out.contains(&g.repo) {
            out.push(g.repo);
        }
    }
    out
}

/// Every node of `kind` with the graph holding it, each id once, in graph
/// then node order.
fn nodes_of<'a>(graphs: &[&'a RepoGraph], kind: NodeKindId) -> Vec<(&'a RepoGraph, &'a Node)> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for &g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) == Some(&kind) && seen.insert(n.id) {
                out.push((g, n));
            }
        }
    }
    out
}

/// Pair one repo's SDL resolvers (`graphs` = that repo's graphs, in merged
/// order) with its gqlgen template methods.
fn pair_repo(
    graphs: &[&RepoGraph],
    pairs: &mut Vec<(NodeId, NodeId, &'static str, Confidence)>,
    stats: &mut Stats,
) {
    // STRUCTs named `*Resolver`, declared in Go: the only candidates.
    let mut structs: Vec<(NodeId, &str)> = Vec::new();
    for (g, n) in nodes_of(graphs, node_kind::STRUCT) {
        let Some(name) = g.nav.name_by_id.get(&n.id) else {
            continue;
        };
        if name.ends_with(RESOLVER) && file_of(&n.cells).is_some_and(|f| f.ends_with(".go")) {
            structs.push((n.id, name.as_str()));
        }
    }
    let resolvers: HashSet<NodeId> = structs
        .iter()
        .filter(|(_, name)| *name == RESOLVER)
        .map(|(id, _)| *id)
        .collect();
    if resolvers.is_empty() {
        return;
    }
    let candidates: HashSet<NodeId> = structs.iter().map(|(id, _)| *id).collect();
    // Embeds: INHERITS_FROM candidate -> `Resolver`, in edge order.
    let mut embeds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for &g in graphs {
        for e in &g.edges {
            if e.category == edge_category::INHERITS_FROM
                && candidates.contains(&e.from)
                && resolvers.contains(&e.to)
            {
                let to = embeds.entry(e.from).or_default();
                if !to.contains(&e.to) {
                    to.push(e.to);
                }
            }
        }
    }
    let mut roots: Vec<RootStruct> = Vec::new();
    let mut object_structs = 0usize;
    for &(id, name) in &structs {
        let Some(embedded) = embeds.get(&id) else {
            continue;
        };
        if let Some(&(root, _)) = ROOTS.iter().find(|(_, s)| *s == name) {
            roots.push(RootStruct {
                root,
                id,
                embeds: embedded.clone(),
            });
        } else if name != RESOLVER && name.starts_with(|c: char| c.is_ascii_lowercase()) {
            object_structs += 1;
        }
    }
    // A repo with no root struct is no gqlgen repo: nothing pairs, nothing
    // is counted.
    if roots.is_empty() {
        return;
    }
    stats.root_structs += roots.len();
    stats.object_structs += object_structs;

    // The methods of the root structs and of the `Resolver`s they embed.
    let owners: HashSet<NodeId> = roots
        .iter()
        .flat_map(|r| std::iter::once(r.id).chain(r.embeds.iter().copied()))
        .collect();
    let mut methods: Vec<Method> = Vec::new();
    for (g, n) in nodes_of(graphs, node_kind::METHOD) {
        let Some(&parent) = g.nav.parent_of.get(&n.id).filter(|p| owners.contains(p)) else {
            continue;
        };
        let Some(name) = g.nav.name_by_id.get(&n.id) else {
            continue;
        };
        methods.push(Method {
            id: n.id,
            parent,
            name,
            key: fold(name),
            file: file_of(&n.cells).map(|f| owner_path(&f).into_owned()),
            confidence: n.confidence,
        });
    }

    // The SDL resolvers.
    let mut sdl: Vec<SdlResolver> = Vec::new();
    for (g, n) in nodes_of(graphs, node_kind::GRAPHQL_RESOLVER) {
        if !file_of(&n.cells).is_some_and(|f| SDL_EXTS.iter().any(|x| f.ends_with(x))) {
            continue;
        }
        let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        let (base, owner) = split_owner(qname);
        let Some(name) = base.strip_prefix(QNAME_PREFIX) else {
            continue;
        };
        sdl.push(SdlResolver {
            id: n.id,
            name,
            owner,
            confidence: n.confidence,
        });
    }

    for r in &sdl {
        let under = |m: &Method| match r.owner {
            None => true,
            Some(owner) => m
                .file
                .as_deref()
                .and_then(|f| f.strip_prefix(owner))
                .is_some_and(|rest| rest.starts_with('/')),
        };
        let mut found: Vec<&Method> = Vec::new();
        let mut collided = false;
        let rule = if let Some(&(root, _)) = ROOTS.iter().find(|(t, _)| *t == r.name) {
            // A root type: the getter on each `Resolver` its struct embeds.
            for rs in roots.iter().filter(|rs| rs.root == root) {
                found.extend(
                    methods
                        .iter()
                        .filter(|m| rs.embeds.contains(&m.parent) && m.name == root && under(m)),
                );
            }
            RULE_ROOT
        } else {
            let (only, field) = match r.name.split_once('.') {
                Some((t, f)) if ROOTS.iter().any(|(root, _)| *root == t) => (Some(t), f),
                _ => (None, r.name),
            };
            let key = fold(field);
            for rs in roots.iter().filter(|rs| only.is_none_or(|t| t == rs.root)) {
                let hits: Vec<&Method> = methods
                    .iter()
                    .filter(|m| m.parent == rs.id && m.key == key && under(m))
                    .collect();
                match hits.as_slice() {
                    [one] => found.push(one),
                    [] => {}
                    _ => collided = true,
                }
            }
            RULE_FIELD
        };
        if found.is_empty() {
            if collided {
                stats.ambiguous += 1;
            } else {
                stats.unpaired += 1;
            }
            continue;
        }
        if rule == RULE_ROOT {
            stats.roots += 1;
        } else {
            stats.fields += 1;
        }
        for m in found {
            pairs.push((r.id, m.id, rule, weakest(r.confidence, m.confidence)));
        }
    }
}

/// A node's POSITION file.
fn file_of(cells: &[Cell]) -> Option<String> {
    locate(cells).map(|(file, _)| file)
}

/// gqlgen's name comparison: ASCII lower-case, `_` removed. `productById`,
/// `product_by_id` and `ProductByID` fold to one key.
fn fold(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `file` escaped as the engine escapes a project root into an owner segment
/// (whitespace and `%` as `%XX` per UTF-8 byte), so an owner prefix compares
/// with it byte for byte.
fn owner_path(file: &str) -> Cow<'_, str> {
    if !file.chars().any(|c| c.is_whitespace() || c == '%') {
        return Cow::Borrowed(file);
    }
    let mut out = String::with_capacity(file.len() + 8);
    for c in file.chars() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::evidence::Evidence;
    use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type};
    use glia_core::CellPayload;

    use crate::types::SymbolTable;

    /// A hand-built graph of one repo: `node` records a node with a POSITION
    /// file, `embed` an INHERITS_FROM edge.
    struct G {
        g: RepoGraph,
    }

    impl G {
        fn new(repo: RepoId) -> Self {
            G {
                g: RepoGraph {
                    repo,
                    nodes: vec![],
                    edges: vec![],
                    symbols: SymbolTable::default(),
                    nav: CodeNav::default(),
                    unresolved_calls: vec![],
                    unresolved_refs: vec![],
                    properties: HashSet::new(),
                },
            }
        }

        fn node(
            &mut self,
            kind: NodeKindId,
            name: &str,
            qname: &str,
            file: &str,
            parent: Option<NodeId>,
        ) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.g.repo, kind, qname);
            self.g.nav.record(id, name, qname, kind, parent);
            self.g.nodes.push(Node {
                id,
                repo: self.g.repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(format!(
                        r#"{{"file":"{file}","start_line":0,"end_line":0}}"#
                    )),
                }],
            });
            id
        }

        fn embed(&mut self, from: NodeId, to: NodeId) {
            self.g.edges.push(Edge::new(
                from,
                to,
                edge_category::INHERITS_FROM,
                Confidence::Strong,
            ));
        }
    }

    /// The Go side of the gqlgen template under `root/` (`""` = the repo
    /// root): `Resolver` in resolver.go, `queryResolver` / `mutationResolver`
    /// embedding it (when `embed`), their `methods`, and the getters.
    fn go_side(
        g: &mut G,
        root: &str,
        query: &[&str],
        mutation: &[&str],
        embed: bool,
    ) -> HashMap<String, NodeId> {
        let dir = if root.is_empty() {
            "graph".to_string()
        } else {
            format!("{root}/graph")
        };
        let q = dir.replace('/', "::");
        let mut ids = HashMap::new();
        let resolver_file = format!("{dir}/resolver.go");
        let schema_file = format!("{dir}/schema.resolvers.go");
        let resolver = g.node(
            node_kind::STRUCT,
            RESOLVER,
            &format!("{q}::resolver::Resolver"),
            &resolver_file,
            None,
        );
        ids.insert("Resolver".to_string(), resolver);
        for (strukt, fields) in [("queryResolver", query), ("mutationResolver", mutation)] {
            let sq = format!("{q}::schema.resolvers::{strukt}");
            let s = g.node(node_kind::STRUCT, strukt, &sq, &schema_file, None);
            if embed {
                g.embed(s, resolver);
            }
            ids.insert(strukt.to_string(), s);
            for f in fields {
                let m = g.node(
                    node_kind::METHOD,
                    f,
                    &format!("{sq}::{f}"),
                    &schema_file,
                    Some(s),
                );
                ids.insert(format!("{strukt}.{f}"), m);
            }
        }
        for getter in ["Query", "Mutation"] {
            let m = g.node(
                node_kind::METHOD,
                getter,
                &format!("{q}::schema.resolvers::Resolver::{getter}"),
                &schema_file,
                Some(resolver),
            );
            ids.insert(format!("Resolver.{getter}"), m);
        }
        ids
    }

    /// The SDL side: one GRAPHQL_RESOLVER per name, in `file`, owned by
    /// `owner` (`""` = none).
    fn sdl_side(g: &mut G, names: &[&str], file: &str, owner: &str) -> HashMap<String, NodeId> {
        let mut ids = HashMap::new();
        for name in names {
            let qname = if owner.is_empty() {
                format!("{QNAME_PREFIX}{name}")
            } else {
                format!("{QNAME_PREFIX}{name} @{owner}")
            };
            ids.insert(
                name.to_string(),
                g.node(node_kind::GRAPHQL_RESOLVER, name, &qname, file, None),
            );
        }
        ids
    }

    /// `(from qname, to qname)` of every HANDLED_BY cross edge, sorted.
    fn handled(m: &MergedGraph) -> Vec<(String, String)> {
        super::super::tests::cross_pairs(m, edge_category::HANDLED_BY)
    }

    fn repo(tag: &str) -> RepoId {
        RepoId::from_canonical(&format!("test://{tag}"))
    }

    fn run(graphs: Vec<RepoGraph>) -> MergedGraph {
        let mut m = MergedGraph::new(graphs);
        m.run(&GqlgenResolver);
        m
    }

    /// Two graphs of one repo: the Go group's and the graphql group's.
    fn shop(query: &[&str], mutation: &[&str], embed: bool, sdl: &[&str]) -> MergedGraph {
        let r = repo("shop");
        let mut go = G::new(r);
        go_side(&mut go, "api", query, mutation, embed);
        let mut gql = G::new(r);
        sdl_side(&mut gql, sdl, "api/graph/schema.graphqls", "api");
        run(vec![go.g, gql.g])
    }

    fn pair(from: &str, to: &str) -> (String, String) {
        (from.to_string(), to.to_string())
    }

    #[test]
    fn a_field_pairs_its_root_struct_method_across_graphs() {
        let m = shop(&["Products"], &["Restock"], true, &["products", "restock"]);
        assert_eq!(
            handled(&m),
            [
                pair(
                    "graphql_resolver:products @api",
                    "api::graph::schema.resolvers::queryResolver::Products"
                ),
                pair(
                    "graphql_resolver:restock @api",
                    "api::graph::schema.resolvers::mutationResolver::Restock"
                ),
            ]
        );
    }

    #[test]
    fn a_root_type_pairs_the_root_getter() {
        let m = shop(&[], &[], true, &["Query", "Mutation"]);
        assert_eq!(
            handled(&m),
            [
                pair(
                    "graphql_resolver:Mutation @api",
                    "api::graph::schema.resolvers::Resolver::Mutation"
                ),
                pair(
                    "graphql_resolver:Query @api",
                    "api::graph::schema.resolvers::Resolver::Query"
                ),
            ]
        );
    }

    #[test]
    fn the_id_initialism_folds_and_underscores_drop() {
        let m = shop(
            &["ProductByID", "UserName"],
            &[],
            true,
            &["productById", "user_name"],
        );
        assert_eq!(
            handled(&m),
            [
                pair(
                    "graphql_resolver:productById @api",
                    "api::graph::schema.resolvers::queryResolver::ProductByID"
                ),
                pair(
                    "graphql_resolver:user_name @api",
                    "api::graph::schema.resolvers::queryResolver::UserName"
                ),
            ]
        );
    }

    #[test]
    fn the_fold_compares_whole_names() {
        let m = shop(&["ProductByID"], &[], true, &["products"]);
        assert_eq!(handled(&m), []);
    }

    #[test]
    fn no_embed_pairs_nothing() {
        let m = shop(
            &["Products"],
            &["Restock"],
            false,
            &["products", "restock", "Query", "Mutation"],
        );
        assert_eq!(handled(&m), []);
    }

    #[test]
    fn a_struct_not_named_like_a_root_pairs_nothing() {
        let r = repo("cache");
        let mut go = G::new(r);
        let resolver = go.node(
            node_kind::STRUCT,
            RESOLVER,
            "graph::resolver::Resolver",
            "graph/resolver.go",
            None,
        );
        let cache = go.node(
            node_kind::STRUCT,
            "cacheResolver",
            "cache::cacheResolver",
            "cache/cache.go",
            None,
        );
        go.embed(cache, resolver);
        go.node(
            node_kind::METHOD,
            "Products",
            "cache::cacheResolver::Products",
            "cache/cache.go",
            Some(cache),
        );
        let mut gql = G::new(r);
        sdl_side(&mut gql, &["products"], "graph/schema.graphqls", "");
        let m = run(vec![go.g, gql.g]);
        assert_eq!(handled(&m), []);
        let mut stats = Stats::default();
        let graphs: Vec<&RepoGraph> = m.graphs.iter().collect();
        pair_repo(&graphs, &mut Vec::new(), &mut stats);
        assert_eq!(stats.line(), None, "no root struct: no marker");
        assert_eq!(stats.object_structs, 0, "no gqlgen repo: nothing counted");
    }

    #[test]
    fn a_field_in_both_roots_pairs_both_methods() {
        let m = shop(&["Products"], &["Products"], true, &["products"]);
        assert_eq!(
            handled(&m),
            [
                pair(
                    "graphql_resolver:products @api",
                    "api::graph::schema.resolvers::mutationResolver::Products"
                ),
                pair(
                    "graphql_resolver:products @api",
                    "api::graph::schema.resolvers::queryResolver::Products"
                ),
            ]
        );
    }

    #[test]
    fn a_root_prefixed_field_pairs_only_that_root() {
        let m = shop(&["Products"], &["Products"], true, &["Mutation.products"]);
        assert_eq!(
            handled(&m),
            [pair(
                "graphql_resolver:Mutation.products @api",
                "api::graph::schema.resolvers::mutationResolver::Products"
            )]
        );
    }

    #[test]
    fn a_fold_collision_in_one_struct_pairs_neither() {
        let r = repo("collide");
        let mut go = G::new(r);
        go_side(&mut go, "api", &["UserID", "UserId"], &[], true);
        let mut gql = G::new(r);
        sdl_side(&mut gql, &["userId"], "api/graph/schema.graphqls", "api");
        let graphs = [go.g, gql.g];
        let refs: Vec<&RepoGraph> = graphs.iter().collect();
        let mut stats = Stats::default();
        let mut pairs = Vec::new();
        pair_repo(&refs, &mut pairs, &mut stats);
        assert!(pairs.is_empty(), "{} pairs", pairs.len());
        assert_eq!(
            stats.line().as_deref(),
            Some(
                "[gqlgen] root structs=2 object_structs=0 paired fields=0 roots=0 unpaired=0 ambiguous=1"
            )
        );
    }

    #[test]
    fn owners_scope_each_project_to_its_own_schema() {
        let r = repo("mono");
        let mut go = G::new(r);
        go_side(&mut go, "svc/users", &["Products"], &[], true);
        go_side(&mut go, "svc/shop", &["Products"], &[], true);
        let mut gql = G::new(r);
        sdl_side(
            &mut gql,
            &["products"],
            "svc/users/graph/schema.graphqls",
            "svc/users",
        );
        sdl_side(
            &mut gql,
            &["products", "Query"],
            "svc/shop/graph/schema.graphqls",
            "svc/shop",
        );
        let m = run(vec![go.g, gql.g]);
        assert_eq!(
            handled(&m),
            [
                pair(
                    "graphql_resolver:Query @svc/shop",
                    "svc::shop::graph::schema.resolvers::Resolver::Query"
                ),
                pair(
                    "graphql_resolver:products @svc/shop",
                    "svc::shop::graph::schema.resolvers::queryResolver::Products"
                ),
                pair(
                    "graphql_resolver:products @svc/users",
                    "svc::users::graph::schema.resolvers::queryResolver::Products"
                ),
            ]
        );
    }

    #[test]
    fn an_owner_prefix_needs_a_path_boundary() {
        // `@api` must not reach `api2/`.
        let r = repo("prefix");
        let mut go = G::new(r);
        go_side(&mut go, "api2", &["Products"], &[], true);
        let mut gql = G::new(r);
        sdl_side(&mut gql, &["products"], "api/graph/schema.graphqls", "api");
        assert_eq!(handled(&run(vec![go.g, gql.g])), []);
    }

    #[test]
    fn an_escaped_owner_matches_its_files() {
        let r = repo("space");
        let mut go = G::new(r);
        go_side(&mut go, "my app", &["Products"], &[], true);
        let mut gql = G::new(r);
        sdl_side(
            &mut gql,
            &["products"],
            "my app/graph/schema.graphqls",
            "my%20app",
        );
        assert_eq!(handled(&run(vec![go.g, gql.g])).len(), 1);
    }

    #[test]
    fn a_code_first_resolver_is_left_alone() {
        // A NestJS `@Query() products()` resolver sits in a .ts file: its own
        // anchor binds it, never this resolver.
        let r = repo("nest");
        let mut go = G::new(r);
        go_side(&mut go, "", &["Products"], &[], true);
        sdl_side(&mut go, &["products"], "src/products.resolver.ts", "");
        assert_eq!(handled(&run(vec![go.g])), []);
    }

    #[test]
    fn a_repo_pairs_only_its_own_code() {
        // The schema in one repo, the template in another: no pair.
        let mut go = G::new(repo("go"));
        go_side(&mut go, "", &["Products"], &[], true);
        let mut gql = G::new(repo("schema"));
        sdl_side(&mut gql, &["products"], "graph/schema.graphqls", "");
        assert_eq!(handled(&run(vec![go.g, gql.g])), []);
    }

    #[test]
    fn edges_carry_rule_evidence_and_the_marker_counts() {
        let r = repo("shop");
        let mut go = G::new(r);
        let ids = go_side(
            &mut go,
            "api",
            &["Products", "ProductByID"],
            &["Restock"],
            true,
        );
        // gqlgen's object resolver for `type Order { customer: Customer }`:
        // counted, never paired (the SDL mints no node for `customer`).
        let order = go.node(
            node_kind::STRUCT,
            "orderResolver",
            "api::graph::schema.resolvers::orderResolver",
            "api/graph/schema.resolvers.go",
            None,
        );
        go.embed(order, ids["Resolver"]);
        let mut gql = G::new(r);
        sdl_side(
            &mut gql,
            &[
                "Query",
                "products",
                "productById",
                "Mutation",
                "restock",
                "orphan",
            ],
            "api/graph/schema.graphqls",
            "api",
        );
        let graphs = vec![go.g, gql.g];
        let refs: Vec<&RepoGraph> = graphs.iter().collect();
        let mut stats = Stats::default();
        pair_repo(&refs, &mut Vec::new(), &mut stats);
        assert_eq!(
            stats.line().as_deref(),
            Some(
                "[gqlgen] root structs=2 object_structs=1 paired fields=3 roots=2 unpaired=1 ambiguous=0"
            )
        );
        let m = run(graphs);
        assert_eq!(m.cross_edges.len(), 5);
        for e in &m.cross_edges {
            let to = m.graphs[0].nav.qname_by_id[&e.to].clone();
            let rule = if to.contains("::Resolver::") {
                RULE_ROOT
            } else {
                RULE_FIELD
            };
            assert_eq!(e.category, edge_category::HANDLED_BY);
            assert_eq!(
                e.cells,
                vec![Evidence::emitter("resolver:gqlgen").rule(rule).to_cell()],
                "{to}"
            );
        }
    }

    #[test]
    fn two_runs_push_the_same_edges_in_id_order() {
        let build = || {
            shop(
                &["Products", "ProductByID"],
                &["Restock"],
                true,
                &["restock", "productById", "products", "Query", "Mutation"],
            )
        };
        let a = build();
        let b = build();
        assert_eq!(a.cross_edges, b.cross_edges);
        let keys: Vec<(u64, u64)> = a.cross_edges.iter().map(|e| (e.from.0, e.to.0)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(keys.len(), 5);
    }
}
