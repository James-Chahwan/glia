//! DB resolver — services touching the same Table / Collection / NodeLabel,
//! and (A13.3) services reaching the same external data provider.

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_code_domain::data_entity::table_of;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Cell, Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::{CrossGraphResolver, RuleTally, emit_cross_repo_pairs, rule_evidence};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// The five provider kinds `extractors::data_sources` emits, one per
/// `DataSourceKind` variant. Nodes of these kinds carry the global qname
/// `data_source:<provider>`, so two services reaching the same Redis converge
/// on the same key in different repos.
const DATA_SOURCE_KINDS: [NodeKindId; 5] = [
    node_kind::DATABASE,
    node_kind::CACHE,
    node_kind::BLOB_STORE,
    node_kind::SEARCH_INDEX,
    node_kind::EMAIL_SERVICE,
];

/// Distinct repos a single provider may join before the pairing is dropped.
/// Provider nodes are the "framework-tag fallback is pairable" shape: every
/// service in a 40-service stack imports `redis`, and an unbounded pairing
/// would emit ~800 all-to-all edges of near-zero information. Above the cap we
/// emit NOTHING for that provider rather than a fan-out nobody can use.
const MAX_DATA_SOURCE_FANOUT: usize = 8;

/// Distinct repos a canonical entity bucket may span before its FOLD pairs
/// (joins between two different spellings of one entity) are skipped. Exact
/// same-qname pairs are never capped — they are what the resolver emitted
/// before the fold existed. SHARES_DATA_ENTITY is a blast carry edge, so an
/// uncapped fold over a `user` bucket in a 30-service stack would widen every
/// blast radius by hundreds of naming-convention guesses. Do not lower this
/// without re-checking blast-radius width.
const MAX_ENTITY_FANOUT: usize = 12;

/// Every emitter but Java's `@Entity` projection prefixes DATA_ENTITY qnames
/// with `data_entity:<flavor>:`.
const ENTITY_PREFIX: &str = "data_entity:";

/// The flavor a prefix-less DATA_ENTITY qname is read as. Java's bare
/// `@Entity` / `@Document` simple-name projection is the only prefix-less
/// emitter, and `@Entity` (JPA) is a relational table. A Java `@Document`
/// (Mongo) is misread as `sql` here until A13.2 gives Java a flavored prefix.
const PREFIXLESS_FLAVOR: &str = "sql";

/// LC.3c: the DB evidence rules, in `[evidence-rules]` order. The rule is a
/// function of the pass that paired the edge, never of HashMap order:
/// `entity` — the entity pass's exact half (one verbatim DATA_ENTITY qname in
/// two repos); `entity_fold` — its fold half (two spellings, one
/// `(flavor, canonical name)` key, forced Weak); `provider` — the A13.3
/// provider pass (one `data_source:<provider>` qname, forced Weak).
const DB_RULES: [&str; 3] = ["entity", "entity_fold", "provider"];

// ============================================================================
// DbResolver — joins services that touch the same Table / Collection /
// NodeLabel. Mirrors SharedSchemaResolver's pairwise-pair shape.
//
// Two passes over DATA_ENTITY nodes:
//   * EXACT: nodes sharing a verbatim qname (`data_entity:<flavor>:<name>`)
//     in different repos pair uncapped, at `weakest(a, b)`.
//   * FOLD (A13.1): nodes whose qnames differ but whose join key
//     `(flavor, canonical name)` agrees pair at `Weak`, bucket fan-out capped
//     at MAX_ENTITY_FANOUT. This is what joins Java's `User`, Ruby's
//     `data_entity:sql:User` and Python's `data_entity:sql:users`.
// The flavor half of the key keeps a SQL `users` table and a Mongo `User`
// collection apart in both passes.
// ============================================================================

pub struct DbResolver;

impl CrossGraphResolver for DbResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut rules = RuleTally::new("db", &DB_RULES);
        let entities = entity_pass(&merged.graphs);
        rules.add("entity", entities.stats.exact_paired);
        rules.add("entity_fold", entities.stats.folded_paired);
        if entities.stats.entities > 0 {
            let s = &entities.stats;
            eprintln!(
                "[db-entity] buckets={} exact_paired={} folded_paired={} \
                 skipped_fanout={} prefixless={} table_cells={}",
                s.buckets, s.exact_paired, s.folded_paired, s.skipped_fanout, s.prefixless,
                s.table_cells,
            );
        }
        merged.cross_edges.extend(entities.edges);

        // ---- A13.3: provider pass ------------------------------------------
        // Keyed on the VERBATIM `data_source:<provider>` qname — providers are
        // a closed vocabulary from the extractor's PATTERNS table, so there is
        // nothing to canonicalise and any normalisation would only collide
        // distinct providers.
        let mut source_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> =
            HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                let Some(kind) = g.nav.kind_by_id.get(&n.id) else {
                    continue;
                };
                if !DATA_SOURCE_KINDS.contains(kind) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                source_index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        let mut providers = 0usize;
        let mut paired = 0usize;
        let mut skipped_fanout = 0usize;
        let mut emitted: Vec<Edge> = Vec::new();
        let provider_ev = rules.evidence("provider");
        for refs in source_index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            if repos.len() > MAX_DATA_SOURCE_FANOUT {
                skipped_fanout += 1;
                continue;
            }
            providers += 1;
            // Forced `Weak`, NOT `weakest(a, b)`: the provider nodes are
            // Medium by construction, and `weakest` would hand a substring
            // match on `"Redis"` the same confidence as a resolved call.
            paired += emit_cross_repo_pairs(
                refs,
                edge_category::SHARES_DATA_SOURCE,
                Some(Confidence::Weak),
                Some(&provider_ev),
                &mut emitted,
            );
        }
        // HashMap iteration order is per-process random; sort so the emitted
        // block is byte-stable across runs.
        emitted.sort_by_key(|e| (e.from.0, e.to.0));
        if providers > 0 || skipped_fanout > 0 {
            eprintln!(
                "[db-source] providers={providers} paired={paired} \
                 skipped_fanout={skipped_fanout}"
            );
        }
        merged.cross_edges.extend(emitted);
        rules.add("provider", paired);
        rules.report();
    }
}

type EntityRef = (NodeId, RepoId, Confidence);

/// Counters behind the `[db-entity]` marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct EntityStats {
    /// DATA_ENTITY nodes seen across every graph.
    entities: usize,
    /// Distinct `(flavor, canonical)` join keys.
    buckets: usize,
    /// Edges from the EXACT pass (same qname, different repos).
    exact_paired: usize,
    /// Edges from the FOLD pass (different qnames, same join key).
    folded_paired: usize,
    /// Buckets whose fold pairs were skipped for spanning more than
    /// MAX_ENTITY_FANOUT repos.
    skipped_fanout: usize,
    /// Buckets holding at least one prefix-less qname (Java `@Entity`).
    /// Reads 0 once A13.2 prefixes every emitter.
    prefixless: usize,
    /// DATA_ENTITY nodes whose key came from a table cell, not the qname.
    table_cells: usize,
}

struct EntityPass {
    /// Sorted by `(from, to)` so the block is byte-stable across processes.
    edges: Vec<Edge>,
    stats: EntityStats,
}

/// The join key of one DATA_ENTITY node, plus where it came from.
struct JoinKey {
    flavor: String,
    canonical: String,
    prefixless: bool,
    from_table_cell: bool,
}

/// Split a DATA_ENTITY qname into its join key `(flavor, canonical name)`.
///
/// The flavor is the `data_entity:<flavor>:` segment, or PREFIXLESS_FLAVOR for
/// a prefix-less qname. The name is the `table` of an ORM table cell on the
/// node when one is present (a model mapped by `@Table` / `$table` / `@@map`
/// joins the table other services name), else the qname tail.
fn join_key(qname: &str, cells: &[Cell]) -> JoinKey {
    let (flavor, tail, prefixless) = match qname.strip_prefix(ENTITY_PREFIX) {
        Some(rest) => match rest.split_once(':') {
            Some((flavor, name)) => (flavor, name, false),
            None => (PREFIXLESS_FLAVOR, rest, false),
        },
        None => (PREFIXLESS_FLAVOR, qname, true),
    };
    let table = table_of(cells);
    let from_table_cell = table.is_some();
    let canonical = canonical_entity_name(table.as_deref().unwrap_or(tail));
    JoinKey {
        flavor: flavor.to_owned(),
        canonical,
        prefixless,
        from_table_cell,
    }
}

/// Idempotent fold of a surface entity name to its canonical form.
///
/// In order: (1) drop a schema / owner qualifier — keep the text after the
/// last `.` — and any surrounding quote or bracket; (2) split camel / Pascal
/// humps with `_` (`UserAccount` → `User_Account`), an acronym run being one
/// hump (`APIKey` → `API_Key`), and read `-` as `_`; (3) ASCII-lowercase;
/// (4) singularise the last `_` segment.
///
/// The fold is lossy by design and only has to be IDEMPOTENT and applied to
/// both sides: a wrong-but-consistent fold (`series` → `sery`) still joins.
/// It accepts `user`/`users`, `Order`/`orders`, `Category`/`categories` as one
/// entity — all desirable. The false-join risk is two genuinely different
/// names folding together; the flavor half of the key bounds it (a Mongo
/// `Post` never joins a SQL `posts`), and fold pairs are `Weak`.
fn canonical_entity_name(name: &str) -> String {
    let tail = name
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .trim_matches(|c| matches!(c, '"' | '`' | '\'' | '[' | ']'));
    if tail.is_empty() {
        return name.to_owned();
    }
    let chars: Vec<char> = tail.chars().collect();
    let mut split = String::with_capacity(tail.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c == '-' {
            split.push('_');
            continue;
        }
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let hump = prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_lower);
            if hump {
                split.push('_');
            }
        }
        split.push(c);
    }
    let lower = split.to_ascii_lowercase();
    match lower.rsplit_once('_') {
        Some((head, last)) => format!("{head}_{}", singularise(last)),
        None => singularise(&lower),
    }
}

/// Plural → singular for one lowercase segment: `ies` → `y`;
/// `sses|shes|ches|xes|zes` → drop `es`; a trailing `s` after a letter other
/// than `s`, `u` or `i` → drop it; otherwise unchanged. Leaves `address`,
/// `status`, `analysis`, `data` alone. Idempotent: every output ends in a
/// letter no rule strips again (`y`, `ss`, `sh`, `ch`, `x`, `z`, or a non-`s`
/// letter), or is the unchanged input. The "after a letter" guard keeps
/// `ab's` from folding to `ab'`, which step (1) of the fold would then trim.
fn singularise(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies") {
        if !stem.is_empty() {
            return format!("{stem}y");
        }
    }
    if ["sses", "shes", "ches", "xes", "zes"].iter().any(|s| word.ends_with(s)) {
        if let Some(stem) = word.strip_suffix("es") {
            return stem.to_owned();
        }
    }
    if let Some(stem) = word.strip_suffix('s') {
        if let Some(prev) = stem.chars().last() {
            if prev.is_ascii_alphabetic() && !matches!(prev, 's' | 'u' | 'i') {
                return stem.to_owned();
            }
        }
    }
    word.to_owned()
}

/// Both DATA_ENTITY passes over `graphs`, returning the SHARES_DATA_ENTITY
/// edges and the marker counters.
///
/// Additive over the pre-A13.1 resolver by construction: the EXACT pass is
/// that resolver's raw-qname pairing unchanged (uncapped, `weakest(a, b)`),
/// so its edge set is a subset of this one. The FOLD pass only adds pairs
/// between DIFFERENT qnames, forced `Weak` because a naming-convention match
/// is an inference, not a resolved reference. The skip decision is per bucket
/// and order-independent, so HashMap iteration order cannot change the set.
fn entity_pass(graphs: &[RepoGraph]) -> EntityPass {
    let mut stats = EntityStats::default();
    let mut exact_index: HashMap<&str, Vec<EntityRef>> = HashMap::new();
    // join key -> raw qname -> nodes. BTreeMap so the sub-groups (and thus the
    // direction of every fold edge) come out in a fixed order.
    let mut buckets: HashMap<(String, String), BTreeMap<&str, Vec<EntityRef>>> =
        HashMap::new();
    let mut prefixless_keys: HashSet<(String, String)> = HashSet::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::DATA_ENTITY) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            stats.entities += 1;
            let entity = (n.id, g.repo, n.confidence);
            exact_index.entry(qname.as_str()).or_default().push(entity);
            let key = join_key(qname, &n.cells);
            if key.from_table_cell {
                stats.table_cells += 1;
            }
            let bucket = (key.flavor, key.canonical);
            if key.prefixless {
                prefixless_keys.insert(bucket.clone());
            }
            buckets
                .entry(bucket)
                .or_default()
                .entry(qname.as_str())
                .or_default()
                .push(entity);
        }
    }
    stats.buckets = buckets.len();
    stats.prefixless = prefixless_keys.len();

    let mut edges: Vec<Edge> = Vec::new();
    let exact_ev = rule_evidence("db", "entity");
    for refs in exact_index.values() {
        if refs.len() < 2 {
            continue;
        }
        let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
        if repos.len() < 2 {
            continue;
        }
        stats.exact_paired += emit_cross_repo_pairs(
            refs,
            edge_category::SHARES_DATA_ENTITY,
            None,
            Some(&exact_ev),
            &mut edges,
        );
    }

    let fold_cell = rule_evidence("db", "entity_fold").to_cell();
    for groups in buckets.values() {
        if groups.len() < 2 {
            continue;
        }
        let repos: HashSet<RepoId> = groups.values().flatten().map(|(_, r, _)| *r).collect();
        if repos.len() < 2 {
            continue;
        }
        if repos.len() > MAX_ENTITY_FANOUT {
            stats.skipped_fanout += 1;
            continue;
        }
        let groups: Vec<&Vec<EntityRef>> = groups.values().collect();
        for (i, left) in groups.iter().enumerate() {
            for right in &groups[i + 1..] {
                for a in left.iter() {
                    for b in right.iter() {
                        if a.1 == b.1 {
                            continue;
                        }
                        edges.push(
                            Edge::new(
                                a.0,
                                b.0,
                                edge_category::SHARES_DATA_ENTITY,
                                Confidence::Weak,
                            )
                            .with_cell(fold_cell.clone()),
                        );
                        stats.folded_paired += 1;
                    }
                }
            }
        }
    }
    // HashMap iteration order is per-process random; sort so the emitted
    // block is byte-stable across runs.
    edges.sort_by_key(|e| (e.from.0, e.to.0));
    EntityPass { edges, stats }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
    use repo_graph_core::Node;
    use crate::types::{RepoGraph, SymbolTable};

    /// Build a single-node RepoGraph holding one DATA_ENTITY for `qname` in
    /// `repo_id`. Used to assemble cross-repo fixtures for DbResolver tests.
    fn graph_with_entity(repo_id: RepoId, qname: &str) -> RepoGraph {
        graph_with_entity_cells(repo_id, qname, vec![])
    }

    /// `graph_with_entity`, with `cells` on the DATA_ENTITY node (an ORM
    /// declaration site carrying a table cell).
    fn graph_with_entity_cells(repo_id: RepoId, qname: &str, cells: Vec<Cell>) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::DATA_ENTITY, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::DATA_ENTITY,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells,
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    /// Build a single-node RepoGraph holding one provider node of `kind` for
    /// `qname` in `repo_id` — the shape `extractors::data_sources` emits.
    fn graph_with_data_source(repo_id: RepoId, kind: NodeKindId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, kind, qname);
        let mut nav = CodeNav::default();
        nav.record(id, qname.rsplit(':').next().unwrap_or(qname), qname, kind, None);
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn db_resolver_pairs_same_data_source_across_repos() {
        // A13.3 — two services reaching the same Redis. Exactly one edge, and
        // it is WEAK: the provider nodes are Medium, so `weakest(a, b)` would
        // have said Medium and overstated a substring guess.
        let g_a = graph_with_data_source(RepoId(11), node_kind::CACHE, "data_source:redis");
        let g_b = graph_with_data_source(RepoId(12), node_kind::CACHE, "data_source:redis");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DATA_SOURCE)
            .collect();
        assert_eq!(edges.len(), 1, "expected one cross-repo SHARES_DATA_SOURCE edge");
        assert_eq!(
            edges[0].confidence,
            Confidence::Weak,
            "provider pairing is a substring guess and must stay Weak"
        );
    }

    #[test]
    fn db_resolver_keeps_providers_separate() {
        // postgres and redis are distinct keys even across repos.
        let g_a = graph_with_data_source(RepoId(11), node_kind::CACHE, "data_source:redis");
        let g_b =
            graph_with_data_source(RepoId(12), node_kind::DATABASE, "data_source:postgres");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_SOURCE),
            "different providers must not pair"
        );
    }

    #[test]
    fn db_resolver_drops_data_source_fanout_above_cap() {
        // 9 distinct repos on one provider is the all-to-all noise shape the
        // cap exists to refuse: nothing at all, not 36 edges.
        let graphs: Vec<RepoGraph> = (0..(MAX_DATA_SOURCE_FANOUT as u64 + 1))
            .map(|i| {
                graph_with_data_source(RepoId(100 + i), node_kind::CACHE, "data_source:redis")
            })
            .collect();
        let mut merged = MergedGraph::new(graphs);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_SOURCE),
            "a provider above MAX_DATA_SOURCE_FANOUT must emit nothing"
        );
    }

    #[test]
    fn shares_data_source_is_not_a_blast_carry_edge() {
        // REGRESSION GUARD. A shared Postgres is an operational fact, not a
        // code dependency: carrying it would fan every blast radius across
        // every service in the stack. Do not "fix" this by adding the row.
        assert!(
            !crate::blast::blast_carry_edges().contains(&edge_category::SHARES_DATA_SOURCE),
            "SHARES_DATA_SOURCE must stay OUT of blast_carry_edges()"
        );
    }

    #[test]
    fn db_resolver_pairs_same_entity_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_entity(repo_a, "data_entity:sql:users");
        let g_b = graph_with_entity(repo_b, "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DATA_ENTITY)
            .collect();
        assert_eq!(edges.len(), 1, "expected one cross-repo SHARES_DATA_ENTITY edge");
    }

    #[test]
    fn db_resolver_does_not_pair_within_a_single_repo() {
        // Two DATA_ENTITY nodes with the same qname inside one repo would
        // already collapse via NodeId; even if duplicated, no cross-edge.
        let repo_a = RepoId(11);
        let g1 = graph_with_entity(repo_a, "data_entity:sql:users");
        let g2 = graph_with_entity(repo_a, "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g1, g2]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_ENTITY),
            "must not emit SHARES_DATA_ENTITY when all matches share the same repo"
        );
    }

    #[test]
    fn db_resolver_keeps_flavors_separate() {
        // A SQL `users` table and a NoSQL `users` collection have different
        // qname flavor segments and must not be joined.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_entity(repo_a, "data_entity:sql:users");
        let g_b = graph_with_entity(repo_b, "data_entity:nosql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_ENTITY),
            "flavor mismatch must not emit a SHARES_DATA_ENTITY edge"
        );
    }

    fn entity_edges(merged: &MergedGraph) -> Vec<&Edge> {
        merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DATA_ENTITY)
            .collect()
    }

    fn entity_id(repo_id: RepoId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::DATA_ENTITY, qname)
    }

    #[test]
    fn db_resolver_joins_bare_java_name_to_prefixed_table() {
        // A13.1 — Java's @Entity projects the bare simple name `User`; Python's
        // `__tablename__ = "users"` mints `data_entity:sql:users`. One entity.
        let g_a = graph_with_entity(RepoId(11), "User");
        let g_b = graph_with_entity(RepoId(12), "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges = entity_edges(&merged);
        assert_eq!(edges.len(), 1, "expected one folded SHARES_DATA_ENTITY edge");
        assert_eq!(edges[0].from, entity_id(RepoId(11), "User"));
        assert_eq!(edges[0].to, entity_id(RepoId(12), "data_entity:sql:users"));
        assert_eq!(
            edges[0].confidence,
            Confidence::Weak,
            "a naming-convention fold is an inference and must stay Weak"
        );
        let stats = entity_pass(&merged.graphs).stats;
        assert_eq!(
            (stats.buckets, stats.exact_paired, stats.folded_paired, stats.prefixless),
            (1, 0, 1, 1)
        );
    }

    #[test]
    fn db_resolver_joins_table_cell_to_sql_table() {
        // An ORM model keyed on its MODEL name, carrying the table it maps to,
        // joins the service that names the table directly.
        let cell = repo_graph_code_domain::data_entity::table_cell(
            "app_users",
            repo_graph_code_domain::data_entity::orm::JPA,
        );
        let g_a = graph_with_entity_cells(RepoId(11), "data_entity:sql:User", vec![cell]);
        let g_b = graph_with_entity(RepoId(12), "data_entity:sql:app_users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges = entity_edges(&merged);
        assert_eq!(edges.len(), 1, "table cell must key the model onto app_users");
        assert_eq!(edges[0].confidence, Confidence::Weak);
        let stats = entity_pass(&merged.graphs).stats;
        assert_eq!((stats.table_cells, stats.folded_paired), (1, 1));
    }

    #[test]
    fn db_resolver_table_cell_keeps_model_off_its_default_plural() {
        // `User` mapped to `app_users` is NOT the `users` table another
        // service reads: the cell replaces the qname tail in the key.
        let cell = repo_graph_code_domain::data_entity::table_cell(
            "app_users",
            repo_graph_code_domain::data_entity::orm::ELOQUENT,
        );
        let g_a = graph_with_entity_cells(RepoId(11), "data_entity:sql:User", vec![cell]);
        let g_b = graph_with_entity(RepoId(12), "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(entity_edges(&merged).is_empty());
    }

    #[test]
    fn db_resolver_exact_pair_survives_divergent_table_cell() {
        // Additivity guard: two repos naming the SAME qname pair exactly as
        // before A13.1, even when only one of them carries a table cell and
        // their fold keys therefore differ.
        let cell = repo_graph_code_domain::data_entity::table_cell(
            "app_users",
            repo_graph_code_domain::data_entity::orm::JPA,
        );
        let g_a = graph_with_entity_cells(RepoId(11), "data_entity:sql:User", vec![cell]);
        let g_b = graph_with_entity(RepoId(12), "data_entity:sql:User");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges = entity_edges(&merged);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].confidence, Confidence::Medium, "exact pairs keep weakest(a, b)");
    }

    #[test]
    fn db_resolver_does_not_fold_across_flavors() {
        // The spec's `db_resolver_does_not_join_across_flavors`: a SQL `users`
        // table and a Mongo `User` collection fold to the same name but live
        // under different flavors, so they stay apart.
        let g_a = graph_with_entity(RepoId(11), "data_entity:sql:users");
        let g_b = graph_with_entity(RepoId(12), "data_entity:nosql:User");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(entity_edges(&merged).is_empty(), "flavor mismatch must not fold");
    }

    #[test]
    fn db_resolver_does_not_fold_within_a_single_repo() {
        let g1 = graph_with_entity(RepoId(11), "User");
        let g2 = graph_with_entity(RepoId(11), "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g1, g2]);
        merged.run(&DbResolver);
        assert!(entity_edges(&merged).is_empty());
    }

    #[test]
    fn db_resolver_exact_pairs_uncapped_fold_pairs_capped() {
        // 13 repos on one verbatim qname: C(13, 2) = 78 exact pairs, exactly
        // as before A13.1 — MAX_ENTITY_FANOUT never removes an exact pair.
        let thirteen = || -> Vec<RepoGraph> {
            (0..13u64)
                .map(|i| graph_with_entity(RepoId(100 + i), "data_entity:sql:users"))
                .collect()
        };
        let mut merged = MergedGraph::new(thirteen());
        merged.run(&DbResolver);
        let edges = entity_edges(&merged);
        assert_eq!(edges.len(), 78);
        assert!(edges.iter().all(|e| e.confidence == Confidence::Medium));
        assert_eq!(entity_pass(&merged.graphs).stats.skipped_fanout, 0);

        // A 14th repo spelling it `data_entity:sql:User` folds into a bucket
        // spanning 14 > 12 repos: its fold pairs are skipped and counted, the
        // 78 exact pairs are untouched.
        let mut graphs = thirteen();
        graphs.push(graph_with_entity(RepoId(200), "data_entity:sql:User"));
        let mut merged = MergedGraph::new(graphs);
        merged.run(&DbResolver);
        assert_eq!(entity_edges(&merged).len(), 78);
        let stats = entity_pass(&merged.graphs).stats;
        assert_eq!(
            (stats.exact_paired, stats.folded_paired, stats.skipped_fanout),
            (78, 0, 1)
        );
    }

    #[test]
    fn db_resolver_fold_pairs_every_cross_repo_spelling_under_the_cap() {
        // Three spellings across three repos: every cross-repo pair between
        // DIFFERENT spellings folds once, in raw-qname order.
        let graphs = vec![
            graph_with_entity(RepoId(1), "User"),
            graph_with_entity(RepoId(2), "data_entity:sql:User"),
            graph_with_entity(RepoId(3), "data_entity:sql:users"),
            graph_with_entity(RepoId(4), "data_entity:sql:users"),
        ];
        let pass = entity_pass(&graphs);
        // exact: repos 3-4 on `users`. fold: User×sql:User, User×users(2),
        // sql:User×users(2) = 5.
        assert_eq!((pass.stats.exact_paired, pass.stats.folded_paired), (1, 5));
        assert_eq!(pass.edges.len(), 6);
        let mut sorted = pass.edges.clone();
        sorted.sort_by_key(|e| (e.from.0, e.to.0));
        assert_eq!(pass.edges, sorted, "emitted block must be sorted");
    }

    #[test]
    fn canonical_entity_name_folds_spellings() {
        for (raw, want) in [
            ("users", "user"),
            ("User", "user"),
            ("public.users", "user"),
            ("[dbo].[Users]", "user"),
            ("UserAccount", "user_account"),
            ("user_accounts", "user_account"),
            ("user-accounts", "user_account"),
            ("APIKey", "api_key"),
            ("categories", "category"),
            ("boxes", "box"),
            ("classes", "class"),
            ("dishes", "dish"),
            ("matches", "match"),
            ("address", "address"),
            ("status", "status"),
            ("analysis", "analysis"),
            ("data", "data"),
            ("series", "sery"),
        ] {
            assert_eq!(canonical_entity_name(raw), want, "fold of {raw:?}");
        }
    }

    #[test]
    fn canonical_entity_name_is_idempotent() {
        for raw in [
            "users", "User", "public.users", "[dbo].[Users]", "UserAccount", "user_accounts",
            "APIKey", "HTTPRequests", "categories", "boxes", "classes", "statuses", "series",
            "ab's", "user2Accounts", "_Users", "users_", "a.b.", "\"\"", "s", "ies", "Ünits",
            "",
        ] {
            let once = canonical_entity_name(raw);
            assert_eq!(canonical_entity_name(&once), once, "fold of {raw:?} is not idempotent");
        }
    }

    #[test]
    fn join_key_reads_flavor_prefix_and_table_cell() {
        let k = join_key("data_entity:nosql:Posts", &[]);
        assert_eq!((k.flavor.as_str(), k.canonical.as_str()), ("nosql", "post"));
        assert!(!k.prefixless && !k.from_table_cell);

        let k = join_key("Order", &[]);
        assert_eq!((k.flavor.as_str(), k.canonical.as_str()), ("sql", "order"));
        assert!(k.prefixless);

        let cell = repo_graph_code_domain::data_entity::table_cell(
            "shop.order_lines",
            repo_graph_code_domain::data_entity::orm::DJANGO,
        );
        let k = join_key("data_entity:sql:Order", &[cell]);
        assert_eq!((k.flavor.as_str(), k.canonical.as_str()), ("sql", "order_line"));
        assert!(k.from_table_cell);
    }
}
