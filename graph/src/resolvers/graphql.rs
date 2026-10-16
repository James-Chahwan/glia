//! GraphQL stack resolver — operation → resolver by exact field key.
//!
//! Both sides are reduced to the FIELD they name (`gql_key`) and pair only
//! when those keys are equal. Until A5.5 the rule paired whenever either
//! lowercased name CONTAINED the other, so an un-named `useQuery` hook (whose
//! op name falls back to the needle text) paired with the decorator-noun
//! `Query` resolver node — all-to-all fan-out on the busiest node in a GraphQL
//! repo.
//!
//! HOST NARROWING (CB.24). The resolver index is owner-free (LB.8b), so an
//! operation pairs every same-field resolver whichever project serves it. A
//! project that builds its GraphQL client on a literal base URL (`new
//! ApolloClient({ uri: "http://users-svc/graphql" })`) has that authority
//! stamped on its operations as an ENDPOINT_HIT `hosts` by the engine's
//! client-host graft; an operation matching two or more resolvers keeps only
//! those of the project or repo the host names (`host::SideNarrowing`, A11.4 /
//! LB.4b's rule: no host, an unknown one or a named scope with no resolver
//! keeps every pair). Such a pair's evidence rule is `host`; every other pair
//! keeps the engine's emitter-only stamp.
//!
//! THIRD-PARTY OPERATIONS (DF.9). An operation whose every site the engine
//! marked external (DF.8: the client's base names a literal public host the
//! repo does not configure, `uri: "https://api.github.com/graphql"`) and none
//! of whose hosts names a service / project alias of the build
//! (`host::third_party_hosts`, CG.4b's HTTP rule) queries someone else's
//! schema: it pairs with nothing, even when a resolver of the build serves
//! the same field. An unkeyable operation is dropped first, as before.
//! [`GraphQLStackResolver::third_party_clients`] lists such operations by the
//! same predicate.
//!
//! ROOT FIELDS (DT.11a). An operation read from a document carries it as its
//! CODE cell (the extractor, `glia_code_extractors::graphql`), and pairs by
//! the ROOT FIELDS that document selects
//! (`glia_code_extractors::graphql::operation_root_fields`), not by its
//! operation name: `query StockList { products { .. } }` pairs
//! `graphql_resolver:products`, and `query Products { me { .. } }` pairs `me`
//! and never `products`. The fields replace the name key whenever any is
//! read; an operation with no document (a codegen hook `useGetUserQuery()`,
//! a document built at run time, one selecting only fragment spreads, a CODE
//! span a loaded layout could not read back) keeps HEAD's name key. Every
//! field key reaching a resolver gives one pair per (operation, resolver),
//! and such a pair's evidence rule is `root_field` unless a host narrowed
//! it. The root TYPE is not checked: the resolver index is field-only, so a
//! field served under both `Query` and `Subscription` pairs both.
//!
//! fired_on marker, once per build with GraphQL to resolve:
//!   `[graphql-resolve] 2 pairs; dropped: type-level=2, unkeyable=0, no-match=0; narrowed-by-host=1`
//! `narrowed-by-host` counts operations whose resolvers a host narrowed.
//! Then, once per build with a keyable operation (DT.11a):
//!   `[graphql-root-fields] ops=2 fields=2 pairs=2 name_key_ops=0`
//! `ops` / `fields` count the operations keyed by root fields and their
//! field keys, `pairs` the pairs those operations made, and `name_key_ops`
//! the operations keyed by their name. Then, when a third-party operation was
//! left unpaired (DF.9):
//!   `[channel-external] via=graphql clients=<n> hosts=<k>`

use std::collections::{HashMap, HashSet};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_code_extractors::graphql::operation_root_fields;
use glia_core::{Cell, CellPayload, Edge, NodeId};

use super::host::{ExternalSides, SideNarrowing, third_party_sides};
use super::{CrossGraphResolver, RuleTally, ServiceTarget, build_kind_index, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// GraphQLStackResolver — matches operation → resolver by field key
// ============================================================================

pub struct GraphQLStackResolver;

impl GraphQLStackResolver {
    /// DF.9: the GRAPHQL_OPERATION ids this resolver leaves out of pairing as
    /// third-party operations (see the module doc), by the one predicate the
    /// resolver applies, so a post-pass can label them without a second copy
    /// of it (the engine's `tag_synthetic_provenance`). An unkeyable
    /// operation, which the resolver drops first, is listed too when its sites
    /// qualify: it is third-party all the same. Only probed: no order is
    /// promised.
    ///
    /// The alias index is built with no resolver owners: owners scope an
    /// alias, they never add or remove one, and only the alias NAMES are read
    /// here.
    pub fn third_party_clients(graphs: &[RepoGraph]) -> HashSet<NodeId> {
        third_party_sides(graphs, node_kind::GRAPHQL_OPERATION)
    }
}

impl CrossGraphResolver for GraphQLStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let by_name = build_kind_index(&merged.graphs, node_kind::GRAPHQL_RESOLVER, "graphql_resolver:");

        // Re-key the resolver index by field. Equality of keys is the whole
        // predicate, so a keyed lookup is the old ops × resolvers scan without
        // the scan. Names are sorted first so that when two of them fold to
        // one key (`getUser` / `GetUser`) their targets append in a stable
        // order rather than HashMap order.
        let mut names: Vec<_> = by_name.into_iter().collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let mut by_key: HashMap<String, Vec<_>> = HashMap::new();
        let mut type_level = 0usize;
        for (name, targets) in names {
            match gql_key(&name) {
                Some(key) => by_key.entry(key).or_default().extend(targets),
                // A resolver that names no field (`Query`, `Resolver`,
                // `strawberry.type`) is left in the graph but never pairs.
                None => type_level += targets.len(),
            }
        }

        // DT.11a: every operation's CODE cells, over every graph entry of it.
        let mut documents: HashMap<NodeId, Vec<&Cell>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRAPHQL_OPERATION) {
                    documents.entry(n.id).or_default().extend(n.cells.iter().filter(|c| c.kind == cell_type::CODE));
                }
            }
        }

        let (mut pairs, mut unkeyable, mut no_match) = (0usize, 0usize, 0usize);
        let mut roots = RootFieldTally::default();
        // CB.24: an operation's hosts narrow its resolvers; the operations
        // narrowed (distinct ids) and the `host` rule's tally.
        let mut narrowing = SideNarrowing::new(
            &merged.graphs,
            node_kind::GRAPHQL_OPERATION,
            node_kind::GRAPHQL_RESOLVER,
        );
        let mut narrowed: HashSet<NodeId> = HashSet::new();
        let mut rules = RuleTally::new("graphql", &["host", "root_field"]);
        let mut external = ExternalSides::default();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRAPHQL_OPERATION) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                // LB.8: the owner segment names the calling project, not the
                // field. The resolver index is owner-free (`build_kind_index`).
                let Some(op_name) = split_owner(qname).0.strip_prefix("graphql_op:") else {
                    continue;
                };
                let docs = documents.get(&n.id).map_or(&[][..], Vec::as_slice);
                let (keys, source) = op_keys(docs, op_name);
                if keys.is_empty() {
                    unkeyable += 1;
                    continue;
                }
                roots.keyed(source, keys.len());
                // DF.9 (CG.4b's rule): an operation that queries only public
                // hosts outside the build pairs with nothing, whatever
                // resolver of the build serves its field.
                if let Some(hosts) = narrowing.third_party_hosts(n.id) {
                    external.add(n.id, hosts);
                    continue;
                }
                let mut targets = key_targets(&by_key, &keys);
                if targets.is_empty() {
                    no_match += 1;
                    continue;
                }
                let by_host = narrowing.narrow(n.id, &mut targets, |t| t.id);
                if by_host {
                    narrowed.insert(n.id);
                }
                for t in targets {
                    let mut edge = Edge {
                        from: n.id,
                        to: t.id,
                        category: edge_category::GRAPHQL_CALLS,
                        confidence: weakest(n.confidence, t.confidence),
                        cells: Vec::new(),
                    };
                    if by_host {
                        edge.cells.push(rules.cell("host"));
                    } else if source == KeySource::RootField {
                        edge.cells.push(rules.cell("root_field"));
                    }
                    merged.cross_edges.push(edge);
                    pairs += 1;
                    if source == KeySource::RootField {
                        roots.pairs += 1;
                    }
                }
            }
        }
        rules.report();

        // One line per build, and only when there was GraphQL to resolve —
        // the `[ws-resolve]` house style.
        if pairs + type_level + unkeyable + no_match > 0 {
            eprintln!(
                "[graphql-resolve] {pairs} pairs; dropped: type-level={type_level}, \
                 unkeyable={unkeyable}, no-match={no_match}; narrowed-by-host={}",
                narrowed.len()
            );
        }
        if let Some(line) = roots.marker() {
            eprintln!("{line}");
        }
        external.report("graphql");
    }
}

/// DT.11a: where an operation's pairing keys came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeySource {
    /// The root fields its document selects.
    RootField,
    /// Its operation name (no document, or one with no root field read).
    OpName,
}

/// DT.11a: the field keys an operation pairs by, and where they came from:
/// the [`gql_key`] of every root field that
/// [`operation_root_fields`] reads from the operation's CODE text cells
/// (`docs`, the union over every graph entry of it, first seen first), else
/// `gql_key(op_name)`. Root fields REPLACE the name key once any is read, so
/// `query Users { me }` never keys `users`; when none of them is keyable the
/// keys are empty (the operation is unkeyable). A CODE payload that is not
/// text (an unresolved CD.7c span) reads no field.
fn op_keys(docs: &[&Cell], op_name: &str) -> (Vec<String>, KeySource) {
    let mut fields: Vec<String> = Vec::new();
    for c in docs {
        let (true, CellPayload::Text(doc)) = (c.kind == cell_type::CODE, &c.payload) else {
            continue;
        };
        for f in operation_root_fields(doc) {
            if !fields.contains(&f) {
                fields.push(f);
            }
        }
    }
    if fields.is_empty() {
        return (gql_key(op_name).into_iter().collect(), KeySource::OpName);
    }
    let mut keys: Vec<String> = Vec::new();
    for key in fields.iter().filter_map(|f| gql_key(f)) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    (keys, KeySource::RootField)
}

/// DT.11a: the resolvers `keys` reach, in key order, each key's list as the
/// index holds it (one key's list is HEAD's target list). There is one pair
/// per (operation, resolver) however many root fields name it: [`op_keys`]
/// dedups the keys, and a resolver sits under one name, so under one key.
fn key_targets(by_key: &HashMap<String, Vec<ServiceTarget>>, keys: &[String]) -> Vec<ServiceTarget> {
    keys.iter().filter_map(|k| by_key.get(k)).flatten().copied().collect()
}

/// DT.11a: the `[graphql-root-fields]` counts.
#[derive(Default)]
struct RootFieldTally {
    ops: usize,
    fields: usize,
    pairs: usize,
    name_key_ops: usize,
}

impl RootFieldTally {
    /// Count one keyable operation keyed by `source` with `keys` keys.
    fn keyed(&mut self, source: KeySource, keys: usize) {
        match source {
            KeySource::RootField => {
                self.ops += 1;
                self.fields += keys;
            }
            KeySource::OpName => self.name_key_ops += 1,
        }
    }

    /// The marker line, or `None` when no operation was keyable.
    fn marker(&self) -> Option<String> {
        (self.ops + self.name_key_ops > 0).then(|| {
            format!(
                "[graphql-root-fields] ops={} fields={} pairs={} name_key_ops={}",
                self.ops, self.fields, self.pairs, self.name_key_ops
            )
        })
    }
}

/// Type-level resolver names, lowercased. Since LA.38 the extractor mints only
/// the root types (`ROOT_TYPES` / `ROOT_DECORATORS` in
/// parsers/code/extractors/src/graphql.rs: query / mutation / subscription);
/// the other entries are the pre-LA.38 decorator nouns, kept as a guard. They
/// name a root TYPE, never a field, so nothing may key off them.
const GQL_TYPE_LEVEL: &[&str] = &[
    "query",
    "mutation",
    "subscription",
    "resolver",
    "resolvefield",
    "objecttype",
    "type",
    "strawberry",
    "strawberry.type",
    "strawberry.mutation",
    "graphene",
    "graphene.objecttype",
];

/// Operation names the extractor falls back to when no gql template names the
/// operation (`OPERATION_PATTERNS` in the same file, minus the `(`),
/// lowercased. They are needle text, not an operation. Only `request` and
/// `uselazyquery` would survive the other rules, but the list is kept whole so
/// it reads against its source.
const GQL_FALLBACK_OPS: &[&str] = &[
    "usequery",
    "usemutation",
    "usesubscription",
    "uselazyquery",
    "client.query",
    "client.mutate",
    "client.subscribe",
    "graphql-request",
    "request",
];

/// Shortest field key that may pair. Matching is exact, so a short key can
/// only ever meet an identical one; the floor exists to discard the one-letter
/// remainders a suffix strip can leave, while keeping the common `me` root
/// field pairable.
const GQL_MIN_KEY: usize = 2;

/// Reduce an operation or resolver name to the FIELD it names, or `None` when
/// it names no field at all.
fn gql_key(raw: &str) -> Option<String> {
    if GQL_FALLBACK_OPS.contains(&raw.to_lowercase().as_str()) {
        return None;
    }
    // `useGetUserQuery` → `GetUserQuery`: strip the hook prefix only when the
    // remainder is capitalised, so a real field named `users` survives intact.
    let stripped = raw
        .strip_prefix("use")
        .filter(|r| r.chars().next().is_some_and(char::is_uppercase))
        .unwrap_or(raw);
    let mut key = stripped.to_lowercase();
    if GQL_TYPE_LEVEL.contains(&key.as_str()) {
        return None;
    }
    // A GraphQL name is `[_A-Za-z][_0-9A-Za-z]*`; anything else (`client.query`,
    // `strawberry.type`, a `${...}` fragment) is not a field.
    let mut chars = key.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return None;
    }
    // `CreateUserMutation` → `createuser`; `MeQuery` → `me`.
    for suffix in ["query", "mutation", "subscription"] {
        if let Some(rest) = key.strip_suffix(suffix)
            && rest.trim_matches('_').len() >= GQL_MIN_KEY
        {
            key = rest.to_string();
            break;
        }
    }
    let key = key.trim_matches('_');
    if key.len() < GQL_MIN_KEY || GQL_TYPE_LEVEL.contains(&key) {
        return None;
    }
    Some(key.to_string())
}

#[cfg(test)]
mod host_tests {
    use super::super::tests::{channel_graph, cross_pairs};
    use super::*;
    use glia_code_domain::cell_type;
    use glia_code_domain::evidence::Evidence;
    use glia_core::{Cell, CellPayload};

    /// CB.24: a monorepo where `services/users` and `services/catalog` both
    /// serve `getUser`, with the calling projects' operations; `hits` puts an
    /// ENDPOINT_HIT on the `@apps/web` operation.
    fn two_servers(tag: &str, hits: &[&str]) -> crate::types::RepoGraph {
        let mut g = channel_graph(
            tag,
            &[
                (node_kind::PROJECT, "project:services/users"),
                (node_kind::PROJECT, "project:services/catalog"),
                (node_kind::PROJECT, "project:apps/web"),
                (node_kind::PROJECT, "project:apps/admin"),
                (node_kind::GRAPHQL_RESOLVER, "graphql_resolver:getUser @services/users"),
                (node_kind::GRAPHQL_RESOLVER, "graphql_resolver:getUser @services/catalog"),
                (node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/web"),
                (node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/admin"),
            ],
        );
        let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        for (id, label) in ids.iter().zip(["users-svc", "catalog-svc", "web", "admin"]) {
            g.nav.name_by_id.insert(*id, label.to_string());
        }
        g.nodes[6].cells = hits
            .iter()
            .map(|j| Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Json((*j).into()) })
            .collect();
        g
    }

    fn pair(from: &str, to: &str) -> (String, String) {
        (format!("graphql_op:getUser @{from}"), format!("graphql_resolver:getUser @{to}"))
    }

    /// CB.24: the web app's client names users-svc, so its operation keeps
    /// the users resolver only, with rule `host`; the admin app (no client
    /// host) still pairs both. A second graph copy of the web operation with
    /// no stamp of its own is narrowed through the first copy's hit.
    #[test]
    fn graphql_host_narrows_to_the_named_project() {
        let g = two_servers("gql-host", &[r#"{"via":"graphql","hosts":["users-svc"]}"#]);
        let copy = channel_graph("gql-host", &[(node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/web")]);
        let mut merged = MergedGraph::new(vec![g, copy]);
        merged.run(&GraphQLStackResolver);
        assert_eq!(
            cross_pairs(&merged, edge_category::GRAPHQL_CALLS),
            vec![
                pair("apps/admin", "services/catalog"),
                pair("apps/admin", "services/users"),
                pair("apps/web", "services/users"),
                pair("apps/web", "services/users"),
            ]
        );
        let rules: Vec<Option<String>> = merged
            .cross_edges
            .iter()
            .map(|e| Evidence::read(&e.cells).and_then(|ev| ev.rule))
            .collect();
        assert_eq!(rules.iter().filter(|r| r.as_deref() == Some("host")).count(), 2, "{rules:?}");
        assert_eq!(rules.iter().filter(|r| r.is_none()).count(), 2, "unnarrowed pairs stay bare");
    }

    /// DF.9: the web app's Apollo client dials a public host the engine
    /// marked external (`api.github.com`), so its operation pairs with no
    /// resolver, nor does its second graph copy (judged on every copy's
    /// cells, as narrowing reads them); the admin app still pairs both. A
    /// marked host whose label names a project (`users.example-corp.com`)
    /// is the build's own and pairs as before.
    #[test]
    fn external_operation_pairs_nothing() {
        let github = r#"{"via":"graphql","hosts":["api.github.com"],"external":true}"#;
        let g = two_servers("gql-external", &[github]);
        let web = g.nodes[6].id;
        let copy = channel_graph("gql-external", &[(node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/web")]);
        let mut merged = MergedGraph::new(vec![g, copy]);
        merged.run(&GraphQLStackResolver);
        assert_eq!(
            cross_pairs(&merged, edge_category::GRAPHQL_CALLS),
            vec![pair("apps/admin", "services/catalog"), pair("apps/admin", "services/users")]
        );
        let third: Vec<NodeId> = GraphQLStackResolver::third_party_clients(&merged.graphs).into_iter().collect();
        assert_eq!(third, vec![web]);

        let own = r#"{"via":"graphql","hosts":["users.example-corp.com"],"external":true}"#;
        let mut merged = MergedGraph::new(vec![two_servers("gql-own", &[own])]);
        merged.run(&GraphQLStackResolver);
        let web_pairs = cross_pairs(&merged, edge_category::GRAPHQL_CALLS)
            .into_iter()
            .filter(|(f, _)| f.ends_with("@apps/web"))
            .count();
        assert_eq!(web_pairs, 2, "`users` names the users project: not third-party");
        assert!(GraphQLStackResolver::third_party_clients(&merged.graphs).is_empty());
    }

    /// CB.24: without positive evidence nothing narrows: no hit, a hostless
    /// hit, an unknown host, a host naming a project that serves no resolver
    /// (`web`), and hosts naming both servers.
    #[test]
    fn no_host_keeps_every_target() {
        for hits in [
            &[][..],
            &[r#"{"via":"graphql"}"#][..],
            &[r#"{"via":"graphql","hosts":["billing:9000"]}"#][..],
            &[r#"{"via":"graphql","hosts":["web"]}"#][..],
            &[r#"{"via":"graphql","hosts":["catalog-svc","users-svc"]}"#][..],
            &[r#"{"via":"graphql","hosts":["users-svc"]}"#, r#"{"via":"graphql"}"#][..],
        ] {
            let mut merged = MergedGraph::new(vec![two_servers("gql-no-host", hits)]);
            merged.run(&GraphQLStackResolver);
            let web: Vec<(String, String)> = cross_pairs(&merged, edge_category::GRAPHQL_CALLS)
                .into_iter()
                .filter(|(f, _)| f.ends_with("@apps/web"))
                .collect();
            assert_eq!(
                web,
                vec![pair("apps/web", "services/catalog"), pair("apps/web", "services/users")],
                "{hits:?}"
            );
            assert!(merged.cross_edges.iter().all(|e| e.cells.is_empty()), "{hits:?}");
        }
    }
}

/// DT.11a: pairing by the root fields of an operation's document.
#[cfg(test)]
mod root_field_tests {
    use super::super::tests::{channel_graph, cross_pairs};
    use super::*;
    use glia_code_domain::evidence::Evidence;

    /// A graph of `ops` (qname, document or none) and resolvers `fields`.
    fn graph(tag: &str, ops: &[(&str, Option<&str>)], fields: &[&str]) -> RepoGraph {
        let resolvers: Vec<String> = fields.iter().map(|f| format!("graphql_resolver:{f}")).collect();
        let mut spec: Vec<_> = ops.iter().map(|(q, _)| (node_kind::GRAPHQL_OPERATION, *q)).collect();
        spec.extend(resolvers.iter().map(|q| (node_kind::GRAPHQL_RESOLVER, q.as_str())));
        let mut g = channel_graph(tag, &spec);
        for (n, (_, doc)) in g.nodes.iter_mut().zip(ops) {
            n.cells = doc
                .iter()
                .map(|d| Cell { kind: cell_type::CODE, payload: CellPayload::Text((*d).to_string()) })
                .collect();
        }
        g
    }

    fn run(g: RepoGraph) -> MergedGraph {
        let mut merged = MergedGraph::new(vec![g]);
        merged.run(&GraphQLStackResolver);
        merged
    }

    fn pairs(m: &MergedGraph) -> Vec<(String, String)> {
        cross_pairs(m, edge_category::GRAPHQL_CALLS)
    }

    fn p(op: &str, field: &str) -> (String, String) {
        (format!("graphql_op:{op}"), format!("graphql_resolver:{field}"))
    }

    fn rules(m: &MergedGraph) -> Vec<Option<String>> {
        m.cross_edges.iter().map(|e| Evidence::read(&e.cells).and_then(|ev| ev.rule)).collect()
    }

    /// examples-glia's admin console: `query StockList` selects `products`.
    #[test]
    fn root_field_pairs_unlike_name() {
        let doc = "\n  query StockList($first: Int) {\n    products(first: $first) { id sku }\n  }\n";
        let m = run(graph("gql-rf", &[("graphql_op:StockList", Some(doc))], &["products", "stockList"]));
        assert_eq!(pairs(&m), vec![p("StockList", "products")]);
        assert_eq!(rules(&m), vec![Some("root_field".to_string())]);
    }

    /// Every root field pairs, an alias by the field behind it; a field
    /// named twice pairs once.
    #[test]
    fn multi_root_document_pairs_each() {
        let doc = "query Dashboard { me { id } recent: orders(last: 5) { id } again: me { name } }";
        let m = run(graph("gql-multi", &[("graphql_op:Dashboard", Some(doc))], &["me", "orders", "products"]));
        assert_eq!(pairs(&m), vec![p("Dashboard", "me"), p("Dashboard", "orders")]);
        assert_eq!(rules(&m), vec![Some("root_field".to_string()); 2]);
    }

    /// `query Products { __typename me }` reaches `me`, never `products`;
    /// a document whose only root field keys nothing is unkeyable, not its
    /// name.
    #[test]
    fn root_fields_replace_the_name_key() {
        let m = run(graph(
            "gql-replace",
            &[
                ("graphql_op:Products", Some("query Products { __typename me { id } }")),
                ("graphql_op:GetUsers", Some("query GetUsers { u }")),
            ],
            &["products", "me", "getUsers"],
        ));
        assert_eq!(pairs(&m), vec![p("Products", "me")]);
    }

    /// An operation with no document (a codegen hook), only fragment
    /// spreads, or an unresolved CD.7c span keeps the name key and the bare
    /// stamp; two graph entries of one operation pool their documents.
    #[test]
    fn no_document_falls_back_to_name() {
        let span = r#"{"code_span":{"file":"src/q.ts","start":0,"end":9,"xxh64":"0000000000000000"}}"#;
        let mut g = graph(
            "gql-name",
            &[
                ("graphql_op:useGetUserQuery", None),
                ("graphql_op:listUsers", Some("query listUsers { ...UserList }")),
                ("graphql_op:orders", None),
            ],
            &["getUser", "listUsers", "orders", "me"],
        );
        g.nodes[2].cells = vec![Cell { kind: cell_type::CODE, payload: CellPayload::Json(span.into()) }];
        let m = run(g);
        assert_eq!(
            pairs(&m),
            vec![p("listUsers", "listUsers"), p("orders", "orders"), p("useGetUserQuery", "getUser")]
        );
        assert!(rules(&m).iter().all(Option::is_none), "{:?}", rules(&m));

        // The second entry of `graphql_op:orders` holds the document.
        let first = graph("gql-pool", &[("graphql_op:orders", None)], &["me", "orders"]);
        let second = graph("gql-pool", &[("graphql_op:orders", Some("{ me { id } }"))], &[]);
        let mut m = MergedGraph::new(vec![first, second]);
        m.run(&GraphQLStackResolver);
        assert_eq!(pairs(&m), vec![p("orders", "me"), p("orders", "me")]);
    }
}

#[cfg(test)]
mod tests {
    use super::gql_key as key;

    #[test]
    fn field_names_key_to_themselves() {
        assert_eq!(key("getUser"), Some("getuser".into()));
        assert_eq!(key("users"), Some("users".into()), "`use` is not a hook prefix here");
        assert_eq!(key("me"), Some("me".into()));
    }

    #[test]
    fn operation_suffixes_and_hook_prefixes_fold_to_the_field() {
        assert_eq!(key("CreateUserMutation"), Some("createuser".into()));
        assert_eq!(key("createUser"), Some("createuser".into()));
        assert_eq!(key("useGetUserQuery"), Some("getuser".into()));
        assert_eq!(key("MeQuery"), Some("me".into()));
        assert_eq!(key("OnMessageSubscription"), Some("onmessage".into()));
    }

    #[test]
    fn type_level_and_fallback_names_key_to_nothing() {
        for name in [
            "Query", "Mutation", "Subscription", "Resolver", "ResolveField", "ObjectType",
            "strawberry.type", "strawberry.mutation",
            "useQuery", "useMutation", "useSubscription", "useLazyQuery",
            "client.query", "client.mutate", "client.subscribe", "graphql-request", "request",
        ] {
            assert_eq!(key(name), None, "{name} must not be a pairing key");
        }
    }

    #[test]
    fn degenerate_remainders_key_to_nothing() {
        // A strip that would leave one letter keeps the suffix instead, and a
        // bare suffix-of-a-suffix never re-enters as a type-level noun.
        assert_eq!(key("QQuery"), Some("qquery".into()));
        assert_eq!(key("QueryQuery"), None);
        assert_eq!(key("x"), None);
        assert_eq!(key("${op}"), None);
        assert_eq!(key("__"), None);
    }
}
