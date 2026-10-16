//! The PPR candidate side file (DE.5): for each exported fact, its top-K
//! approximate personalised PageRank neighbours over the exported edges,
//! written as `<out>.ppr.jsonl` ([`crate::ppr_side_path`]) beside the gmap
//! when `glia-export-engram --ppr-candidates <K>` asks for it.
//!
//! **Data, never a read path.** Engram may take the rows as one optional
//! candidate source (a weight-0 feature in its association pool that stays
//! only if it beats its ablation); it needs no glia code to read them. No
//! glia code reads the file, and writing it changes nothing in the gmap, the
//! `files.json` / `meta.json` sidecars, the `--since` diff or `<out>.glia/`.
//!
//! **The graph.** One node per gmap node (`NodeId(i)` for `gmap.nodes[i]`)
//! and one edge per gmap edge whose two keys are both exported (an edge to an
//! unexported key is skipped and counted). Engram multiplies a hop by
//! `weight.unwrap_or(1.0)`, so that is each edge's weight here: every
//! distinct `(EdgeKind, weight)` pair becomes one synthetic
//! `EdgeCategoryId`, numbered in first-seen gmap edge order and mapped back
//! to its weight. The walk is undirected: a fact's callers neighbour it as
//! much as its callees.
//!
//! **The rows.** For every `Symbol` and `Proposition` fact, in gmap order:
//! [`push_ppr`](glia_activation::algo::ppr::push_ppr) from the fact with
//! alpha [`ALPHA`] (glia's damping 0.5) and epsilon [`EPSILON`], the fact
//! itself dropped, each score rounded to 6 decimals, a score that rounds to
//! 0 dropped, then the top K by (score descending, key ascending). A fact
//! with no neighbour gets `[]`. Rounding comes before the ranking, so a tie
//! at the cut is broken by key, never by float noise or gmap position.
//!
//! **The format.** JSONL, every object's keys sorted (`BTreeMap`). Line 1 is
//! the header that binds the file to its gmap by
//! [`content_digest`](engram_core::content_digest):
//! `{"alpha":0.5,"epsilon":0.001,"gmap_digest":"<16 hex>","k":<K>,
//! "ppr_side":1,"walk":"undirected","weights":"gmap edge weight, 1.0 when
//! none"}`; a reader drops a file whose `gmap_digest` is not its gmap's. Each
//! further line is `{"candidates":[["<key>",<score>],...],"key":"<key>"}`.
//! The bytes are a pure function of the gmap and K: FIFO pushes in edge
//! order, sorted output, rounding at write, and no map is iterated.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use engram_core::{Content, Gmap};
use glia_activation::algo::ppr::PushPpr;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId};

/// The side file's schema version (the header's `ppr_side`): bumped when a
/// field is renamed or removed or changes meaning.
pub const PPR_SIDE_VERSION: u32 = 1;

/// The push's restart probability: glia's PPR damping 0.5.
pub const ALPHA: f64 = 0.5;

/// The push tolerance: a fact's error is at most `EPSILON * d(v)`, and one
/// fact costs at most `1 / (EPSILON * ALPHA)` = 2,000 units of pushed
/// weighted degree.
pub const EPSILON: f64 = 1e-3;

/// The header's `weights` field: where an edge's weight comes from.
const WEIGHTS: &str = "gmap edge weight, 1.0 when none";

/// One fact's candidates: the fact's key and up to K `(key, score)` pairs,
/// best first.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateRow {
    pub key: String,
    pub candidates: Vec<(String, f64)>,
}

/// What [`write_ppr_side`] wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PprSideStats {
    /// Rows written: the `Symbol` and `Proposition` facts.
    pub facts: usize,
    /// The K asked for.
    pub k: usize,
    /// Pushes over every fact's run.
    pub pushes: usize,
    /// Gmap edges walked.
    pub edges: usize,
    /// Gmap edges skipped: an endpoint key that no exported node has.
    pub skipped_edges: usize,
    /// Rows with at least one candidate.
    pub rows_with_candidates: usize,
}

/// The gmap as a [`GraphSource`]: dense node ids, its edges keyed back to
/// node positions, and the weight of each synthetic category.
struct GmapGraph {
    nodes: usize,
    edges: Vec<Edge>,
    /// `weights[c]` is synthetic category `c`'s weight.
    weights: Vec<f64>,
    skipped: usize,
}

impl GmapGraph {
    fn new(gmap: &Gmap) -> Self {
        // Keys are unique in an export; on a repeat the first node keeps it.
        // Only looked up, never iterated.
        let mut at: HashMap<&str, u64> = HashMap::with_capacity(gmap.nodes.len());
        for (i, n) in gmap.nodes.iter().enumerate() {
            at.entry(n.key.as_str()).or_insert(i as u64);
        }
        let mut category: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        let mut weights: Vec<f64> = Vec::new();
        let mut edges = Vec::with_capacity(gmap.edges.len());
        let mut skipped = 0;
        for e in &gmap.edges {
            let (Some(&from), Some(&to)) = (at.get(e.from.as_str()), at.get(e.to.as_str())) else {
                skipped += 1;
                continue;
            };
            let w = e.weight.unwrap_or(1.0);
            let id = *category.entry((e.kind as u32, w.to_bits())).or_insert_with(|| {
                weights.push(f64::from(w));
                (weights.len() - 1) as u32
            });
            edges.push(Edge {
                from: NodeId(from),
                to: NodeId(to),
                category: EdgeCategoryId(id),
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
        }
        Self { nodes: gmap.nodes.len(), edges, weights, skipped }
    }

    fn weight(&self, c: EdgeCategoryId) -> f64 {
        self.weights.get(c.0 as usize).copied().unwrap_or(0.0)
    }
}

impl GraphSource for GmapGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        (0..self.nodes as u64).map(NodeId).collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// A score at the side file's precision.
fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// Every `Symbol` and `Proposition` fact's top-`k` PPR neighbours, in gmap
/// order (the module docs give the rule).
pub fn ppr_candidates(gmap: &Gmap, k: usize) -> Vec<CandidateRow> {
    candidates_with_stats(gmap, k).0
}

fn candidates_with_stats(gmap: &Gmap, k: usize) -> (Vec<CandidateRow>, PprSideStats) {
    let g = GmapGraph::new(gmap);
    let adj = Adjacency::build(&g, &CategorySet::all());
    let weight = |c: EdgeCategoryId| g.weight(c);
    let mut push = PushPpr::new(&adj, &weight);
    let mut stats = PprSideStats { k, edges: g.edges.len(), skipped_edges: g.skipped, ..Default::default() };
    let mut rows = Vec::new();
    for (i, node) in gmap.nodes.iter().enumerate() {
        if !matches!(node.content, Content::Symbol { .. } | Content::Proposition { .. }) {
            continue;
        }
        let seed = NodeId(i as u64);
        let (ranked, run) = push.run(seed, ALPHA, EPSILON);
        stats.pushes += run.pushes;
        let mut candidates: Vec<(&str, f64)> = ranked
            .into_iter()
            .filter(|&(id, _)| id != seed)
            .filter_map(|(id, p)| {
                let score = round6(p);
                let key = gmap.nodes.get(id.0 as usize)?.key.as_str();
                (score > 0.0).then_some((key, score))
            })
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        candidates.truncate(k);
        stats.facts += 1;
        if !candidates.is_empty() {
            stats.rows_with_candidates += 1;
        }
        rows.push(CandidateRow {
            key: node.key.clone(),
            candidates: candidates.into_iter().map(|(key, s)| (key.to_string(), s)).collect(),
        });
    }
    (rows, stats)
}

/// The side file's bytes for `gmap` (whose `content_digest` is `digest`).
fn side_bytes(gmap: &Gmap, digest: u64, k: usize) -> serde_json::Result<(Vec<u8>, PprSideStats)> {
    let (rows, stats) = candidates_with_stats(gmap, k);
    let header: BTreeMap<&str, serde_json::Value> = BTreeMap::from([
        ("alpha", ALPHA.into()),
        ("epsilon", EPSILON.into()),
        ("gmap_digest", format!("{digest:016x}").into()),
        ("k", k.into()),
        ("ppr_side", PPR_SIDE_VERSION.into()),
        ("walk", "undirected".into()),
        ("weights", WEIGHTS.into()),
    ]);
    let mut out = serde_json::to_vec(&header)?;
    out.push(b'\n');
    for row in &rows {
        let candidates: Vec<serde_json::Value> =
            row.candidates.iter().map(|(key, s)| serde_json::json!([key, s])).collect();
        let line: BTreeMap<&str, serde_json::Value> =
            BTreeMap::from([("candidates", candidates.into()), ("key", row.key.as_str().into())]);
        out.extend(serde_json::to_vec(&line)?);
        out.push(b'\n');
    }
    Ok((out, stats))
}

/// Write `gmap`'s side file to `out` (tmp-then-rename): the header naming
/// `digest`, then one row per fact.
pub fn write_ppr_side(gmap: &Gmap, digest: u64, k: usize, out: &Path) -> io::Result<PprSideStats> {
    let (bytes, stats) = side_bytes(gmap, digest, k)?;
    crate::write_atomic(out, &bytes)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::{EdgeKind, GmapEdge, GmapNode, SpanRef};

    fn symbol(key: &str) -> GmapNode {
        GmapNode {
            key: key.to_string(),
            content: Content::Symbol {
                name: key.to_string(),
                span: SpanRef::NONE,
                qname: Some(key.to_string()),
                doc: None,
                imports: None,
            },
            provenance: None,
            concept_hint: None,
            identity_hint: None,
        }
    }

    fn edge(from: &str, kind: EdgeKind, to: &str, weight: Option<f32>) -> GmapEdge {
        GmapEdge { from: from.to_string(), kind, to: to.to_string(), weight }
    }

    /// m contains a, b, c; a calls b, b calls c; d is a lone Proposition; a
    /// Vector fact is a graph node but gets no row.
    fn gmap() -> Gmap {
        let mut nodes: Vec<GmapNode> = ["m", "m::a", "m::b", "m::c"].into_iter().map(symbol).collect();
        nodes.push(GmapNode { content: Content::proposition("lone prose"), ..symbol("doc#1") });
        nodes.push(GmapNode { content: Content::Vector(vec![0.5]), ..symbol("vec") });
        Gmap {
            format_version: engram_core::GMAP_FORMAT_VERSION,
            nodes,
            edges: vec![
                edge("m", EdgeKind::Contains, "m::a", Some(1.0)),
                edge("m", EdgeKind::Contains, "m::b", Some(1.0)),
                edge("m", EdgeKind::Contains, "m::c", Some(1.0)),
                edge("m::a", EdgeKind::Calls, "m::b", Some(0.8)),
                edge("m::b", EdgeKind::Calls, "m::c", Some(0.8)),
                edge("m::a", EdgeKind::Causes, "gone::x", None),
                edge("m::c", EdgeKind::Causes, "vec", None),
            ],
            files: BTreeMap::new(),
        }
    }

    #[test]
    fn rows_follow_gmap_order_and_skip_unexported_keys() {
        let g = gmap();
        let (rows, stats) = candidates_with_stats(&g, 3);
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["m", "m::a", "m::b", "m::c", "doc#1"]);
        assert_eq!((stats.facts, stats.edges, stats.skipped_edges), (5, 6, 1));
        assert_eq!(stats.rows_with_candidates, 4, "the lone Proposition has none");
        assert!(rows[4].candidates.is_empty());
        for r in &rows {
            assert!(r.candidates.len() <= 3);
            assert!(r.candidates.iter().all(|c| c.0 != r.key && c.1 > 0.0));
            assert!(r.candidates.windows(2).all(|w| w[0].1 > w[1].1 || (w[0].1 == w[1].1 && w[0].0 < w[1].0)));
        }
        // From a, the callee b (one hop, and a sibling) beats c (two hops).
        let a: Vec<&str> = rows[1].candidates.iter().map(|c| c.0.as_str()).collect();
        let (b, c) = (a.iter().position(|k| *k == "m::b"), a.iter().position(|k| *k == "m::c"));
        assert!(b.is_some() && (c.is_none() || b < c), "{a:?}");
        // The Vector fact is reachable (a weight-1.0 Causes edge from c).
        assert!(rows[3].candidates.iter().any(|c| c.0 == "vec"), "{:?}", rows[3]);
    }

    #[test]
    fn categories_carry_the_edge_weight() {
        let g = GmapGraph::new(&gmap());
        // (Contains, 1.0), (Calls, 0.8), (Causes, None -> 1.0): three pairs.
        assert_eq!(g.weights, vec![1.0, f64::from(0.8f32), 1.0]);
        let cats: Vec<u32> = g.edges.iter().map(|e| e.category.0).collect();
        assert_eq!(cats, vec![0, 0, 0, 1, 1, 2]);
        assert_eq!(g.weight(EdgeCategoryId(7)), 0.0);
    }

    #[test]
    fn bytes_are_deterministic_and_k_caps_rows() {
        let g = gmap();
        let (one, s1) = side_bytes(&g, 0xabc, 2).unwrap();
        let (two, _) = side_bytes(&g, 0xabc, 2).unwrap();
        assert_eq!(one, two);
        let text = String::from_utf8(one).unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next().unwrap(),
            r#"{"alpha":0.5,"epsilon":0.001,"gmap_digest":"0000000000000abc","k":2,"ppr_side":1,"walk":"undirected","weights":"gmap edge weight, 1.0 when none"}"#
        );
        let rows: Vec<serde_json::Value> = lines.map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.len(), s1.facts);
        assert!(rows.iter().all(|r| r["candidates"].as_array().unwrap().len() <= 2));
        assert_eq!(rows[4], serde_json::json!({"candidates": [], "key": "doc#1"}));
        assert!(ppr_candidates(&g, 0).iter().all(|r| r.candidates.is_empty()));
    }
}
