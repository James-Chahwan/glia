//! GraphQL stack resolver — operation → resolver by exact field key.
//!
//! Both sides are reduced to the FIELD they name (`gql_key`) and pair only
//! when those keys are equal. Until A5.5 the rule paired whenever either
//! lowercased name CONTAINED the other, so an un-named `useQuery` hook (whose
//! op name falls back to the needle text) paired with the decorator-noun
//! `Query` resolver node — all-to-all fan-out on the busiest node in a GraphQL
//! repo.

use std::collections::HashMap;

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::Edge;

use super::{CrossGraphResolver, build_kind_index, weakest};
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
                for t in targets {
                    merged.cross_edges.push(Edge {
                        from: n.id,
                        to: t.id,
                        category: edge_category::GRAPHQL_CALLS,
                        confidence: weakest(n.confidence, t.confidence),
                    });
                    pairs += 1;
                }
            }
        }

        // One line per build, and only when there was GraphQL to resolve —
        // the `[ws-resolve]` house style.
        if pairs + type_level + unkeyable + no_match > 0 {
            eprintln!(
                "[graphql-resolve] {pairs} pairs; dropped: type-level={type_level}, \
                 unkeyable={unkeyable}, no-match={no_match}"
            );
        }
    }
}

/// Resolver names the extractor mints from a decorator or type-level needle
/// (`RESOLVER_PATTERNS` in parsers/code/extractors/src/graphql.rs), lowercased.
/// They name a root TYPE, never a field, so nothing may key off them.
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
