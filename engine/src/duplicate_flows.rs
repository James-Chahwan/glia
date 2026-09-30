//! Duplicate entry flows (CD.4e): entry flows whose reached sets are
//! identical (exact, fingerprinted, DERIVED) or overlap at a Jaccard
//! threshold (MinHash / LSH candidates then verified, HEURISTIC), with utility
//! hubs and test entries left out. The sketches are
//! `glia_activation::algo::minhash`. Public slot, reached by module path
//! (`glia_engine::duplicate_flows::<item>`).
//!
//! Two entry points that run the same code are either redundant (an aliased
//! route, a v1 / v2 pair never retired) or copies drifting apart. This answer
//! is about the SAME nodes reached from two entries; the same shape over
//! different nodes is pattern conformance's job (`patterns`, LE.7a).
//!
//! ENTRIES are `glia flows`' entries (`trace::entries`: the entry rule
//! liveness seeds from, in graph then node order), so every entry a group
//! names is one a user can `glia flows` / `glia trace`. [`DupFlowArgs::scope`]
//! (a path or a project label, resolved once) keeps the entries located under
//! it; an unlocatable entry is kept, as `answers::node_in_scope` keeps it.
//! TEST ENTRIES are dropped unless [`DupFlowArgs::include_tests`]: a node with
//! ORIGIN provenance `test_fixture`, or a FUNCTION / METHOD whose name starts
//! with one of the entry rule's named prefixes (`test` / `Test`), the rule
//! `hubs` leaves tests out by. Go table tests (`TestLoad_Spec003a`,
//! `TestLoad_Spec003b`, ...) otherwise fill the exact groups.
//!
//! A FLOW is the entry's forward walk over the code profile's carry edges
//! (`algo::reach::bfs` over `Adjacency::carry`, the walk `entry_flows` runs)
//! within [`DupFlowArgs::depth`]: the reached ids minus the entry itself minus
//! the utility hubs, as an ascending id list. A flow of fewer than
//! [`DupFlowArgs::min_size`] ids (at least 1: an empty flow is no flow) is
//! skipped. The UTILITY HUBS are `hubs`' fan-in set (`hubs::utility_hubs`
//! over `HubArgs::default()` with this answer's scope): a logger every
//! function calls would otherwise pull every flow toward every other.
//! [`DupFlowArgs::keep_hubs`] keeps them.
//!
//! EXACT groups: flows are bucketed by the xxhash64 of their id list and
//! confirmed equal inside a bucket (a collision is never a group). A set two
//! or more entries reach is a group, [`EXACT`] / [`DERIVED`], `jaccard` 1.0:
//! the equality is read off the built edges.
//!
//! NEAR groups: one representative per distinct set is signed with
//! [`SIGNATURE_LEN`] MinHash permutations ([`DupFlowArgs::seed`]) over its
//! nodes' kind and qname (a `NodeId` hashes the repo's path, so signing ids
//! would pick different candidates for the same code checked out elsewhere)
//! and banded [`BANDS`] x [`ROWS`] (`algo::minhash::lsh_candidates`); every
//! candidate pair is verified with the exact Jaccard (`jaccard_sorted`) of
//! its id sets, and a pair at or above [`DupFlowArgs::threshold`] is kept. Kept pairs are joined by
//! union-find (pairs in sorted order, the smaller index the root), so a group
//! is single-linkage: `jaccard` is the group's WEAKEST kept pair, and two
//! members joined only through a third may share less. [`NEAR`] /
//! [`HEURISTIC`]: the banding may miss a pair (at Jaccard 0.5 a pair is a
//! candidate with probability 0.87, at 0.7 above 0.9999), and a band bucket
//! over `MAX_BUCKET` members is skipped and counted in
//! [`DuplicateFlows::oversized_buckets`]. A near group names every entry of
//! its sets, so an exact pair near a third set is named in both groups.
//!
//! A group's `shared` / `union` are the sizes of the intersection / union of
//! its distinct sets; `differing` the ids in some but not every set, located,
//! the first [`DIFFERING_MAX`] by qname; `services` the `glia arch` services
//! of its entries (`arch::default_keying` + `service_of` of the entry's
//! located file, else of its qname's owner segment, `page:/x @frontend` under
//! `frontend/`; an entry with neither names none unless repos are the
//! keying), sorted. Entries and `differing` sort by qname, file, line and
//! kind, the id last. Groups sort exact first, then by `union` descending,
//! then by their first entry. Every row is located through one `Locator`. No
//! group is an absence (`no_match`). Apart from the ids, the answer is the
//! same for the same code built at any path and any thread count.
//!
//! fired_on marker, once per answer:
//! `[dupflows] entries=<E> flows=<F> exact_groups=<X> near_groups=<N> candidates=<C> hubs_ignored=<H> threshold=<t> surface=<engine|cli|py>`
//! — grep `[dupflows] entries=`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hasher;

use glia_activation::algo::minhash::{MinHasher, jaccard_sorted, lsh_candidates};
use glia_activation::algo::{Adjacency, Walk, reach};
use glia_code_domain::{cell_type, edge_category, endpoint};
use glia_core::{Cell, CellPayload, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use twox_hash::XxHash64;

use crate::absence::{self, Absence};
use crate::answers::{Located, Locator, in_scope, resolve_scope};
use crate::arch::{ServiceKeying, default_keying, service_of};
use crate::hubs::{HubArgs, HubIndex, utility_hubs};
use crate::profile::CODE_PROFILE;
use crate::trace::{self, Entry};

/// [`DupFlowArgs::depth`] by default: `glia flows`' depth.
pub const DEFAULT_DEPTH: usize = 6;
/// [`DupFlowArgs::threshold`] by default.
pub const DEFAULT_THRESHOLD: f64 = 0.8;
/// [`DupFlowArgs::min_size`] by default.
pub const DEFAULT_MIN_SIZE: usize = 3;
/// [`DupFlowArgs::seed`] by default: a fixed seed, so the answer is
/// reproducible (the bytes of `dupflows`).
pub const DEFAULT_SEED: u64 = 0x6475_7066_6c6f_7773;
/// MinHash permutations per signature.
pub const SIGNATURE_LEN: usize = 128;
/// LSH bands per signature.
pub const BANDS: usize = 32;
/// Signature values per LSH band (`BANDS * ROWS == SIGNATURE_LEN`).
pub const ROWS: usize = 4;
/// Located `differing` ids kept per group.
pub const DIFFERING_MAX: usize = 10;

/// [`DupFlowGroup::kind`]: two or more entries reach one set.
pub const EXACT: &str = "exact";
/// [`DupFlowGroup::kind`]: sets joined by verified Jaccard pairs.
pub const NEAR: &str = "near";
/// [`DupFlowGroup::tier`] of an exact group.
pub const DERIVED: &str = "derived";
/// [`DupFlowGroup::tier`] of a near group.
pub const HEURISTIC: &str = "heuristic";
/// [`DupFlowArgs::surface`] when the engine is called directly.
pub const SURFACE_ENGINE: &str = "engine";

const PRIMITIVE: &str = "duplicate_flows";

/// What [`duplicate_flows`] compares. Start from `default()` and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct DupFlowArgs {
    /// Entries under this path or project label only.
    pub scope: Option<String>,
    /// Hops per flow ([`DEFAULT_DEPTH`]).
    pub depth: usize,
    /// The least Jaccard a near pair keeps ([`DEFAULT_THRESHOLD`]); the
    /// surfaces accept `(0, 1]`.
    pub threshold: f64,
    /// Flows with fewer ids are skipped ([`DEFAULT_MIN_SIZE`]); 0 acts as 1.
    pub min_size: usize,
    /// Keep test entries.
    pub include_tests: bool,
    /// Keep the utility hubs in every flow.
    pub keep_hubs: bool,
    /// The MinHash seed ([`DEFAULT_SEED`]).
    pub seed: u64,
    /// Who asked, for the marker: [`SURFACE_ENGINE`], `cli` or `py`.
    pub surface: &'static str,
}

impl Default for DupFlowArgs {
    fn default() -> Self {
        DupFlowArgs {
            scope: None,
            depth: DEFAULT_DEPTH,
            threshold: DEFAULT_THRESHOLD,
            min_size: DEFAULT_MIN_SIZE,
            include_tests: false,
            keep_hubs: false,
            seed: DEFAULT_SEED,
            surface: SURFACE_ENGINE,
        }
    }
}

/// Entries whose flows are one set ([`EXACT`]) or overlap ([`NEAR`]).
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct DupFlowGroup {
    /// [`EXACT`] or [`NEAR`].
    pub kind: &'static str,
    /// [`DERIVED`] (exact) or [`HEURISTIC`] (near).
    pub tier: &'static str,
    /// The entries, located, by qname then id.
    pub entries: Vec<Located>,
    /// 1.0 for an exact group; a near group's weakest kept pair.
    pub jaccard: f64,
    /// Ids every set holds.
    pub shared: usize,
    /// Ids some set holds.
    pub union: usize,
    /// Ids some but not every set holds, located, the first
    /// [`DIFFERING_MAX`] by qname.
    pub differing: Vec<Located>,
    /// The `glia arch` services of the entries, sorted.
    pub services: Vec<String>,
}

/// The duplicate-flow answer.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct DuplicateFlows {
    /// Entries compared (after the scope and the test rule).
    pub entries: usize,
    /// Of those, the flows of at least `min_size` ids.
    pub flows: usize,
    /// Utility hubs left out of every flow (0 with `keep_hubs`).
    pub hubs_ignored: usize,
    /// LSH candidate pairs among the distinct sets, before verification.
    pub candidates: usize,
    /// LSH band buckets too big to pair (`algo::minhash::MAX_BUCKET`),
    /// skipped: a near pair only they would have found is missed.
    pub oversized_buckets: usize,
    pub groups: Vec<DupFlowGroup>,
    /// `Some` exactly when `groups` is empty.
    pub absence: Option<Absence>,
}

/// One compared entry and its flow.
struct Flow {
    entry: NodeId,
    /// Ascending, distinct.
    set: Vec<u64>,
}

/// The duplicate flows of `merged` — see the module doc. `repo_labels` name
/// the services as `glia arch` names them (`GenerateResult::repo_labels`).
pub fn duplicate_flows(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &DupFlowArgs,
) -> DuplicateFlows {
    let loc = Locator::new(merged);
    let all = trace::entries(merged);
    let facts = entry_facts(merged, &all);
    let scope = args.scope.as_deref().map(|s| resolve_scope(merged, s));
    let kept: Vec<&Entry<'_>> = all
        .iter()
        .filter(|e| match (scope.as_deref(), loc.file_of(e.id)) {
            (Some(s), Some(f)) => in_scope(&f, s),
            _ => true,
        })
        .filter(|e| {
            args.include_tests
                || !facts
                    .get(&e.id)
                    .is_some_and(|f| is_test_entry(f.cells, f.kind, e.name))
        })
        .collect();

    let hubs: BTreeSet<u64> = if args.keep_hubs {
        BTreeSet::new()
    } else {
        let h = HubArgs {
            scope: args.scope.clone(),
            ..HubArgs::default()
        };
        HubIndex::build(merged, &loc, &h)
            .map(|ix| utility_hubs(&ix, h.min_degree).0)
            .unwrap_or_default()
    };

    let adj = Adjacency::carry(merged, &CODE_PROFILE.tables);
    let min_size = args.min_size.max(1);
    let mut flows: Vec<Flow> = Vec::new();
    for e in &kept {
        let mut set: Vec<u64> = reach::bfs(&adj, &[e.id], Walk::Forward, args.depth)
            .reached
            .iter()
            .filter(|r| r.id != e.id && !hubs.contains(&r.id.0))
            .map(|r| r.id.0)
            .collect();
        set.sort_unstable();
        set.dedup();
        if set.len() >= min_size {
            flows.push(Flow { entry: e.id, set });
        }
    }

    let classes = exact_classes(&flows);
    let set_of = |c: usize| flows[classes[c][0]].set.as_slice();

    // Fewer than two distinct sets have no pair to sign.
    let sigs: Vec<Vec<u64>> = if classes.len() < 2 {
        Vec::new()
    } else {
        let hasher = MinHasher::new(SIGNATURE_LEN, args.seed);
        let mut keys = PortableKeys::new(merged);
        (0..classes.len())
            .map(|c| hasher.signature(&keys.of(set_of(c))))
            .collect()
    };
    let lsh = lsh_candidates(&sigs, BANDS, ROWS);
    let mut kept_pairs: Vec<(usize, usize, f64)> = Vec::new();
    for &(i, j) in &lsh.pairs {
        let (i, j) = (i as usize, j as usize);
        let (inter, union) = jaccard_sorted(set_of(i), set_of(j));
        if union == 0 {
            continue;
        }
        let jac = f64::from(inter) / f64::from(union);
        if jac >= args.threshold {
            kept_pairs.push((i, j, jac));
        }
    }

    let groups = Groups {
        loc: &loc,
        labels: repo_labels,
        keying: default_keying(merged),
        repo: facts.iter().map(|(id, f)| (*id, f.repo)).collect(),
    };
    let mut out: Vec<DupFlowGroup> = Vec::new();
    for class in classes.iter().filter(|c| c.len() >= 2) {
        let set = &flows[class[0]].set;
        let ids: Vec<NodeId> = class.iter().map(|&f| flows[f].entry).collect();
        out.push(groups.group(EXACT, DERIVED, &ids, 1.0, &[set.as_slice()]));
    }
    for (members, jac) in near_components(classes.len(), &kept_pairs) {
        let ids: Vec<NodeId> = members
            .iter()
            .flat_map(|&c| classes[c].iter().map(|&f| flows[f].entry))
            .collect();
        let sets: Vec<&[u64]> = members.iter().map(|&c| set_of(c)).collect();
        out.push(groups.group(NEAR, HEURISTIC, &ids, jac, &sets));
    }
    out.sort_by(|a, b| {
        (a.kind != EXACT)
            .cmp(&(b.kind != EXACT))
            .then(b.union.cmp(&a.union))
            .then_with(|| by_first_entry(a, b))
    });

    let absence = out.is_empty().then(|| {
        let mechanisms: Vec<&'static str> = CODE_PROFILE
            .tables
            .carry_edges
            .iter()
            .map(|&c| edge_category::name(c))
            .collect();
        let note = format!(
            "no two of the {} entry flows ({} entries compared, flows of >= {min_size} nodes within {} hops) are identical or share >= {} of their nodes (Jaccard)",
            flows.len(),
            kept.len(),
            args.depth,
            args.threshold
        );
        absence::empty(
            merged,
            PRIMITIVE,
            &query(args),
            "no_match",
            note,
            &mechanisms,
            None,
        )
    });
    let answer = DuplicateFlows {
        entries: kept.len(),
        flows: flows.len(),
        hubs_ignored: hubs.len(),
        candidates: lsh.pairs.len(),
        oversized_buckets: lsh.oversized_buckets,
        groups: out,
        absence,
    };
    marker(&answer, args);
    answer
}

/// What the test rule and the service keying read of an entry: its kind,
/// cells and repo, from the first graph holding it.
struct EntryFacts<'g> {
    kind: Option<NodeKindId>,
    cells: &'g [Cell],
    repo: u64,
}

fn entry_facts<'g>(
    merged: &'g MergedGraph,
    entries: &[Entry<'_>],
) -> HashMap<NodeId, EntryFacts<'g>> {
    let wanted: HashSet<NodeId> = entries.iter().map(|e| e.id).collect();
    let mut facts: HashMap<NodeId, EntryFacts<'g>> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if wanted.contains(&n.id) && !facts.contains_key(&n.id) {
                facts.insert(
                    n.id,
                    EntryFacts {
                        kind: g.nav.kind_by_id.get(&n.id).copied(),
                        cells: &n.cells,
                        repo: g.repo.0,
                    },
                );
            }
        }
    }
    facts
}

/// A test entry: ORIGIN provenance `test_fixture`, or a FUNCTION / METHOD the
/// entry rule's named prefixes (`test` / `Test`) match. The rule `hubs` leaves
/// test nodes out by (its `is_test_node`).
fn is_test_entry(cells: &[Cell], kind: Option<NodeKindId>, name: &str) -> bool {
    let needle = "\"provenance\":\"test_fixture\"";
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j) if j.contains(needle))
    }) || kind.is_some_and(|k| {
        CODE_PROFILE.tables.entry.named.iter().any(|rule| {
            rule.kinds.contains(&k) && rule.prefixes.iter().any(|p| name.starts_with(p))
        })
    })
}

/// The MinHash elements of a flow: each id's kind and qname, xxhashed, so
/// the candidate pairs are the same for the same code checked out at another
/// path (a `NodeId` hashes its repo's path). Two nodes with one kind and
/// qname share an element, which only moves a signature, never a verified
/// Jaccard: verification reads the ids.
struct PortableKeys<'g> {
    kind: HashMap<NodeId, u32>,
    qname: HashMap<NodeId, &'g str>,
    memo: HashMap<u64, u64>,
}

impl<'g> PortableKeys<'g> {
    /// The kind and qname of each node, from the first graph holding it.
    fn new(merged: &'g MergedGraph) -> Self {
        let mut kind: HashMap<NodeId, u32> = HashMap::new();
        let mut qname: HashMap<NodeId, &'g str> = HashMap::new();
        for g in &merged.graphs {
            for (id, k) in &g.nav.kind_by_id {
                kind.entry(*id).or_insert(k.0);
            }
            for (id, q) in &g.nav.qname_by_id {
                qname.entry(*id).or_insert(q.as_str());
            }
        }
        PortableKeys {
            kind,
            qname,
            memo: HashMap::new(),
        }
    }

    /// The elements of `set`, one per id.
    fn of(&mut self, set: &[u64]) -> Vec<u64> {
        set.iter()
            .map(|&id| {
                if let Some(&k) = self.memo.get(&id) {
                    return k;
                }
                let node = NodeId(id);
                let mut h = XxHash64::with_seed(0);
                h.write(
                    &self
                        .kind
                        .get(&node)
                        .copied()
                        .unwrap_or(u32::MAX)
                        .to_le_bytes(),
                );
                match self.qname.get(&node) {
                    Some(q) => h.write(q.as_bytes()),
                    // A node no graph names keeps its id.
                    None => h.write(&id.to_le_bytes()),
                }
                let k = h.finish();
                self.memo.insert(id, k);
                k
            })
            .collect()
    }
}

/// xxhash64 of an id list.
fn fingerprint(set: &[u64]) -> u64 {
    let mut h = XxHash64::with_seed(0);
    for id in set {
        h.write(&id.to_le_bytes());
    }
    h.finish()
}

/// The flows partitioned by equal sets: each class lists its flows in order,
/// the classes in order of their first flow. Bucketed by [`fingerprint`] and
/// compared inside a bucket, so a hash collision never merges two sets.
fn exact_classes(flows: &[Flow]) -> Vec<Vec<usize>> {
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut classes: Vec<Vec<usize>> = Vec::new();
    for (f, flow) in flows.iter().enumerate() {
        let bucket = buckets.entry(fingerprint(&flow.set)).or_default();
        match bucket
            .iter()
            .copied()
            .find(|&c| flows[classes[c][0]].set == flow.set)
        {
            Some(c) => classes[c].push(f),
            None => {
                bucket.push(classes.len());
                classes.push(vec![f]);
            }
        }
    }
    classes
}

/// The union-find components of `pairs` over `0..n` with two or more
/// members: each component's members ascending and its weakest pair's
/// Jaccard, in order of the smallest member. Pairs are joined in sorted
/// order and a root is always the smaller index.
fn near_components(n: usize, pairs: &[(usize, usize, f64)]) -> Vec<(Vec<usize>, f64)> {
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut parent: Vec<usize> = (0..n).collect();
    let mut sorted: Vec<(usize, usize, f64)> = pairs.to_vec();
    sorted.sort_by_key(|&(i, j, _)| (i, j));
    for &(i, j, _) in &sorted {
        let (a, b) = (root(&mut parent, i), root(&mut parent, j));
        if a != b {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            parent[hi] = lo;
        }
    }
    let mut weakest: BTreeMap<usize, f64> = BTreeMap::new();
    for &(i, _, jac) in &sorted {
        let r = root(&mut parent, i);
        let w = weakest.entry(r).or_insert(jac);
        if jac < *w {
            *w = jac;
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for x in 0..n {
        let r = root(&mut parent, x);
        if weakest.contains_key(&r) {
            members.entry(r).or_default().push(x);
        }
    }
    members
        .into_iter()
        .map(|(r, m)| (m, weakest.get(&r).copied().unwrap_or(0.0)))
        .collect()
}

/// Builds located groups.
struct Groups<'a> {
    loc: &'a Locator<'a>,
    labels: &'a BTreeMap<u64, String>,
    keying: ServiceKeying,
    repo: HashMap<NodeId, u64>,
}

impl Groups<'_> {
    /// One group of `entries` over its distinct `sets`.
    fn group(
        &self,
        kind: &'static str,
        tier: &'static str,
        entries: &[NodeId],
        jaccard: f64,
        sets: &[&[u64]],
    ) -> DupFlowGroup {
        let mut located: Vec<Located> = entries.iter().map(|&id| self.loc.locate(id)).collect();
        located.sort_by(by_place);
        located.dedup_by(|a, b| a.id == b.id);

        let mut count: BTreeMap<u64, usize> = BTreeMap::new();
        for set in sets {
            for &id in *set {
                *count.entry(id).or_default() += 1;
            }
        }
        let shared = count.values().filter(|&&n| n == sets.len()).count();
        let mut differing: Vec<Located> = count
            .iter()
            .filter(|&(_, &n)| n < sets.len())
            .map(|(&id, _)| self.loc.locate(NodeId(id)))
            .collect();
        differing.sort_by(by_place);
        differing.truncate(DIFFERING_MAX);

        let services: BTreeSet<String> = located.iter().filter_map(|e| self.service(e)).collect();
        DupFlowGroup {
            kind,
            tier,
            entries: located,
            jaccard,
            shared,
            union: count.len(),
            differing,
            services: services.into_iter().collect(),
        }
    }

    /// The `glia arch` service of an entry: its repo's label when repos are
    /// the services, else keyed by its located file or, unlocated, by its
    /// qname's owner segment (`page:/x @frontend` sits under `frontend/`);
    /// `None` with neither.
    fn service(&self, e: &Located) -> Option<String> {
        let repo = *self.repo.get(&NodeId(e.id))?;
        if matches!(self.keying, ServiceKeying::PerRepo) {
            return Some(service_of("", repo, &self.keying, self.labels));
        }
        let file = e.file.clone().or_else(|| {
            endpoint::split_owner(&e.qname)
                .1
                .map(|owner| format!("{owner}/"))
        })?;
        Some(service_of(&file, repo, &self.keying, self.labels))
    }
}

/// Rows by qname, then file, line and kind, then id: the id (which hashes
/// the repo's path) only splits rows alike in everything else.
fn by_place(a: &Located, b: &Located) -> std::cmp::Ordering {
    a.qname
        .cmp(&b.qname)
        .then_with(|| a.file.cmp(&b.file))
        .then(a.line.cmp(&b.line))
        .then(a.kind.cmp(b.kind))
        .then(a.id.cmp(&b.id))
}

/// Groups after kind and union: by their first entry ([`by_place`]).
fn by_first_entry(a: &DupFlowGroup, b: &DupFlowGroup) -> std::cmp::Ordering {
    match (a.entries.first(), b.entries.first()) {
        (Some(x), Some(y)) => by_place(x, y),
        (x, y) => x.is_some().cmp(&y.is_some()),
    }
}

/// The query an absence names.
fn query(args: &DupFlowArgs) -> String {
    let mut q = format!(
        "duplicate flows depth={} threshold={} min_size={}",
        args.depth, args.threshold, args.min_size
    );
    if let Some(s) = &args.scope {
        q.push_str(&format!(" scope={s}"));
    }
    if args.include_tests {
        q.push_str(" include_tests");
    }
    if args.keep_hubs {
        q.push_str(" keep_hubs");
    }
    q
}

/// The CD.4e fired_on line.
fn marker(a: &DuplicateFlows, args: &DupFlowArgs) {
    let exact = a.groups.iter().filter(|g| g.kind == EXACT).count();
    eprintln!(
        "[dupflows] entries={} flows={} exact_groups={exact} near_groups={} candidates={} hubs_ignored={} threshold={} surface={}",
        a.entries,
        a.flows,
        a.groups.len() - exact,
        a.candidates,
        a.hubs_ignored,
        args.threshold,
        args.surface
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banding_matches_the_signature_length() {
        assert_eq!(BANDS * ROWS, SIGNATURE_LEN);
    }

    fn flow(set: &[u64]) -> Flow {
        Flow {
            entry: NodeId(set.first().copied().unwrap_or(0)),
            set: set.to_vec(),
        }
    }

    #[test]
    fn exact_classes_group_equal_sets_in_first_seen_order() {
        let flows = [
            flow(&[1, 2, 3]),
            flow(&[4, 5, 6]),
            flow(&[1, 2, 3]),
            flow(&[7, 8, 9]),
            flow(&[4, 5, 6]),
        ];
        assert_eq!(exact_classes(&flows), vec![vec![0, 2], vec![1, 4], vec![3]]);
    }

    #[test]
    fn fingerprint_is_order_sensitive_but_classes_compare_sets() {
        assert_ne!(fingerprint(&[1, 2]), fingerprint(&[2, 1]));
        assert_eq!(fingerprint(&[1, 2]), fingerprint(&[1, 2]));
    }

    #[test]
    fn near_components_are_single_linkage_with_the_weakest_pair() {
        // 0-1 at 0.9, 1-2 at 0.8, 3-4 at 0.85; 5 alone.
        let pairs = [(1, 2, 0.8), (0, 1, 0.9), (3, 4, 0.85)];
        assert_eq!(
            near_components(6, &pairs),
            vec![(vec![0, 1, 2], 0.8), (vec![3, 4], 0.85)]
        );
        assert!(near_components(3, &[]).is_empty());
    }

    #[test]
    fn test_entries_by_name_and_origin() {
        use glia_code_domain::node_kind;
        assert!(is_test_entry(
            &[],
            Some(node_kind::FUNCTION),
            "TestLoad_Spec003a"
        ));
        assert!(is_test_entry(&[], Some(node_kind::METHOD), "test_list"));
        assert!(!is_test_entry(&[], Some(node_kind::FUNCTION), "main"));
        assert!(!is_test_entry(&[], Some(node_kind::ROUTE), "test"));
        let origin = Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(r#"{"provenance":"test_fixture"}"#.into()),
        };
        assert!(is_test_entry(&[origin], Some(node_kind::ROUTE), "GET /x"));
    }
}
