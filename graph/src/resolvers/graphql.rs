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
//! fired_on marker, once per build with GraphQL to resolve:
//!   `[graphql-resolve] 2 pairs; dropped: type-level=2, unkeyable=0, no-match=0; narrowed-by-host=1`
//! `narrowed-by-host` counts operations whose resolvers a host narrowed.

use std::collections::{HashMap, HashSet};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, NodeId};

use super::host::SideNarrowing;
use super::{CrossGraphResolver, RuleTally, build_kind_index, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// GraphQLStackResolver — matches operation → resolver by field key
// ============================================================================

pub struct GraphQLStackResolver;

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

        let (mut pairs, mut unkeyable, mut no_match) = (0usize, 0usize, 0usize);
        // CB.24: an operation's hosts narrow its resolvers; the operations
        // narrowed (distinct ids) and the `host` rule's tally.
        let mut narrowing = SideNarrowing::new(
            &merged.graphs,
            node_kind::GRAPHQL_OPERATION,
            node_kind::GRAPHQL_RESOLVER,
        );
        let mut narrowed: HashSet<NodeId> = HashSet::new();
        let mut rules = RuleTally::new("graphql", &["host"]);
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
                let Some(key) = gql_key(op_name) else {
                    unkeyable += 1;
                    continue;
                };
                let Some(targets) = by_key.get(&key) else {
                    no_match += 1;
                    continue;
                };
                let mut targets = targets.clone();
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
                    }
                    merged.cross_edges.push(edge);
                    pairs += 1;
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
