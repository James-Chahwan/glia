//! The vector index (FB.4): one model's `.glia/vectors.jsonl` rows, bound to
//! the graph's nodes per repo, checked fresh against the embed text, and read
//! by cosine top-k.
//!
//! A row binds through the cell API's own binder
//! (`glia_graph::cells::QnameIndex`, scoped to its repo), and is kept only
//! when its `text_hash` equals [`text_hash`] of the node's current
//! [`embed_text`](super::embed_text) under the index's recipe: any other row embeds other text
//! (`stale`) and the node is listed in [`VectorIndex::missing_ids`], the work
//! list of the embed run. A reader skips rows of another model (seed §B).
//! Vectors sit in one dense array sorted by (qname, NodeId), so every walk of
//! the index is in a fixed order; a HashMap here is looked up, never iterated.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use glia_code_domain::external_inputs::{VectorRow, b64_decode, read_rows};
use glia_code_domain::profile::EMBED_KINDS;
use glia_core::{NodeId, RepoId};
use glia_embed::codec;
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex};

use super::text::{EmbedRecipe, node_text, text_hash};

/// The sidecar a repo's vectors live in, under its root.
pub const VECTORS_FILE: &str = ".glia/vectors.jsonl";

/// What one [`VectorIndex::load`] read. Every sidecar row lands in exactly
/// one of `fresh`, `stale`, `other_model`, `unbound` and `bad`, so they sum to
/// `rows`; `missing` counts embeddable nodes with no fresh vector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexStats {
    /// The model the index reads.
    pub model: String,
    /// The recipe freshness is judged by.
    pub recipe: EmbedRecipe,
    /// The repos read (each repo root given).
    pub repos: usize,
    /// The vectors' length; 0 when none was kept.
    pub dims: u32,
    /// Sidecar rows (and unparsable lines) read.
    pub rows: usize,
    /// Rows kept: bound, of the model, embedding the node's current text.
    pub fresh: usize,
    /// Rows bound to a node whose text has changed (or that recorded none).
    pub stale: usize,
    /// Embeddable nodes ([`EMBED_KINDS`]) with no fresh vector.
    pub missing: usize,
    /// Rows of another model (or none named).
    pub other_model: usize,
    /// Rows that bound no single embeddable node (ambiguous, orphaned, an
    /// unknown kind name, a node of a kind glia does not embed).
    pub unbound: usize,
    /// Unparsable lines, undecodable payloads, rows of other dims and second
    /// rows for a node already holding a fresh vector.
    pub bad: usize,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SemanticHit {
    pub id: NodeId,
    pub qname: String,
    /// Cosine similarity, unrounded (compared as is).
    pub similarity: f64,
}

impl SemanticHit {
    /// The similarity rounded to 4 decimals, for output only.
    pub fn similarity_rounded(&self) -> f64 {
        (self.similarity * 10_000.0).round() / 10_000.0
    }
}

/// One model's fresh vectors over a merged graph (module doc).
#[derive(Debug, Clone, Default)]
pub struct VectorIndex {
    stats: IndexStats,
    /// `ids.len() * dims` unit vectors, row i belonging to `ids[i]`.
    data: Vec<f32>,
    ids: Vec<NodeId>,
    qnames: Vec<String>,
    /// Embeddable nodes with no fresh vector, sorted by (qname, NodeId).
    missing: Vec<(String, NodeId)>,
    /// NodeId -> its row in `ids` (looked up, never iterated).
    by_id: HashMap<NodeId, usize>,
}

impl VectorIndex {
    /// Read `<root>/.glia/vectors.jsonl` of every repo in `repo_roots` (key
    /// order) and keep the fresh rows of `model` under `recipe`. A repo with
    /// no sidecar contributes missing nodes only. Prints one `[semantic]
    /// index ..` line.
    pub fn load(
        merged: &MergedGraph,
        repo_roots: &BTreeMap<u64, PathBuf>,
        model: &str,
        recipe: EmbedRecipe,
    ) -> VectorIndex {
        let t0 = Instant::now();
        let mut stats = IndexStats {
            model: model.to_string(),
            recipe,
            repos: repo_roots.len(),
            ..IndexStats::default()
        };
        let mut dims: Option<u32> = None;
        let mut kept: Vec<(String, NodeId, Vec<f32>)> = Vec::new();
        let mut missing: Vec<(String, NodeId)> = Vec::new();

        for (&repo, root) in repo_roots {
            let texts = repo_texts(merged, RepoId(repo), recipe);
            let (rows, errors) = read_rows::<VectorRow>(&root.join(VECTORS_FILE));
            stats.rows += rows.len() + errors.len();
            stats.bad += errors.len();
            let mut fresh_ids: HashSet<NodeId> = HashSet::new();
            let binder = (!rows.is_empty()).then(|| QnameIndex::build(merged, Some(RepoId(repo))));
            for row in &rows {
                if row.model.as_deref() != Some(model) {
                    stats.other_model += 1;
                    continue;
                }
                let Some(binder) = binder.as_ref() else {
                    continue;
                };
                let id = match binder.resolve(&row.qname, row.kind.as_deref(), row.hint.as_deref())
                {
                    CellTarget::Bound(id) | CellTarget::Rekeyed { id, .. } => id,
                    _ => {
                        stats.unbound += 1;
                        continue;
                    }
                };
                let Some((qname, text)) = texts.get(&id) else {
                    stats.unbound += 1;
                    continue;
                };
                if row.text_hash.as_deref() != Some(text_hash(recipe, text).as_str()) {
                    stats.stale += 1;
                    continue;
                }
                let want = row.dims.or(dims);
                let vector = match (want, b64_decode(&row.b64)) {
                    (Some(d), Ok(bytes)) => codec::from_le_bytes(&bytes, d).ok(),
                    (None, Ok(bytes)) if bytes.len() % 4 == 0 && !bytes.is_empty() => {
                        codec::from_le_bytes(&bytes, (bytes.len() / 4) as u32).ok()
                    }
                    _ => None,
                };
                let Some(vector) = vector else {
                    stats.bad += 1;
                    continue;
                };
                let d = vector.len() as u32;
                if dims.is_some_and(|have| have != d) || fresh_ids.contains(&id) {
                    stats.bad += 1;
                    continue;
                }
                dims = Some(d);
                fresh_ids.insert(id);
                stats.fresh += 1;
                kept.push((qname.clone(), id, vector));
            }
            let mut repo_missing: Vec<(String, NodeId)> = texts
                .order
                .iter()
                .filter(|id| !fresh_ids.contains(id))
                .filter_map(|id| texts.get(id).map(|(q, _)| (q.clone(), *id)))
                .collect();
            missing.append(&mut repo_missing);
        }

        stats.dims = dims.unwrap_or(0);
        Self::assemble(stats, kept, missing, t0, "")
    }

    /// An index over vectors already in memory (FB.7c: the rank gate's
    /// harness hands the engine the node vectors it embedded, so the shipped
    /// fused `find --files` is measured on the gate's own vectors): `ids[i]`'s
    /// vector is `vecs[i]`, stored as given (the embedder's unit vectors). A
    /// pair is kept when `ids[i]` is an embeddable node ([`EMBED_KINDS`], with
    /// a qname) of `merged`, its vector is non-empty and of the first kept
    /// vector's length, and the node holds no vector yet; else it counts in
    /// `unbound` (no such node) or `bad`. Every kept pair is `fresh` (the
    /// caller vouches for the text: no hash is read), `rows` is `ids.len()`,
    /// `missing` the embeddable nodes left without one, `repos` the merge's
    /// distinct repos. `vecs` shorter than `ids` leaves the tail `bad`. Prints
    /// the `[semantic] index ..` line with ` source=memory`.
    pub fn from_vectors(
        merged: &MergedGraph,
        ids: &[NodeId],
        vecs: &[&[f32]],
        model: &str,
        recipe: EmbedRecipe,
    ) -> VectorIndex {
        let t0 = Instant::now();
        let repos: BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
        let mut stats = IndexStats {
            model: model.to_string(),
            recipe,
            repos: repos.len(),
            rows: ids.len(),
            ..IndexStats::default()
        };
        // Every embeddable node, first instance in graph then node order.
        let mut qname_of: HashMap<NodeId, String> = HashMap::new();
        let mut order: Vec<NodeId> = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if qname_of.contains_key(&n.id)
                    || !g
                        .nav
                        .kind_by_id
                        .get(&n.id)
                        .is_some_and(|k| EMBED_KINDS.contains(k))
                {
                    continue;
                }
                if let Some(q) = g.nav.qname_by_id.get(&n.id) {
                    qname_of.insert(n.id, q.clone());
                    order.push(n.id);
                }
            }
        }
        let mut dims: Option<usize> = None;
        let mut held: HashSet<NodeId> = HashSet::new();
        let mut kept: Vec<(String, NodeId, Vec<f32>)> = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let Some(qname) = qname_of.get(id) else {
                stats.unbound += 1;
                continue;
            };
            let Some(v) = vecs.get(i) else {
                stats.bad += 1;
                continue;
            };
            if v.is_empty() || dims.is_some_and(|d| d != v.len()) || held.contains(id) {
                stats.bad += 1;
                continue;
            }
            dims = Some(v.len());
            held.insert(*id);
            kept.push((qname.clone(), *id, v.to_vec()));
        }
        stats.fresh = kept.len();
        stats.dims = dims.map_or(0, |d| d as u32);
        let missing: Vec<(String, NodeId)> = order
            .iter()
            .filter(|id| !held.contains(id))
            .filter_map(|id| qname_of.get(id).map(|q| (q.clone(), *id)))
            .collect();
        Self::assemble(stats, kept, missing, t0, " source=memory")
    }

    /// The index from its kept vectors and missing nodes (each sorted here by
    /// (qname, NodeId)), with `stats.dims` set; prints the `[semantic] index`
    /// line, `suffix` appended.
    fn assemble(
        mut stats: IndexStats,
        mut kept: Vec<(String, NodeId, Vec<f32>)>,
        mut missing: Vec<(String, NodeId)>,
        t0: Instant,
        suffix: &str,
    ) -> VectorIndex {
        kept.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        missing.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        stats.missing = missing.len();
        let mut index = VectorIndex {
            data: Vec::with_capacity(kept.len() * stats.dims as usize),
            ids: Vec::with_capacity(kept.len()),
            qnames: Vec::with_capacity(kept.len()),
            missing,
            by_id: HashMap::with_capacity(kept.len()),
            stats,
        };
        for (q, id, v) in kept {
            index.data.extend_from_slice(&v);
            index.by_id.insert(id, index.ids.len());
            index.ids.push(id);
            index.qnames.push(q);
        }
        let s = &index.stats;
        eprintln!(
            "[semantic] index repos={} model={} recipe={} dims={} rows={} fresh={} stale={} \
             missing={} other_model={} unbound={} bad={} ms={}{suffix}",
            s.repos,
            s.model,
            s.recipe,
            s.dims,
            s.rows,
            s.fresh,
            s.stale,
            s.missing,
            s.other_model,
            s.unbound,
            s.bad,
            t0.elapsed().as_millis()
        );
        index
    }

    /// The `top_k` vectors most similar to `query` by cosine, skipping
    /// `exclude`: similarity descending, then qname, then NodeId ascending.
    /// The query is renormalised; one of other dims (or zero) finds nothing.
    pub fn search(
        &self,
        query: &[f32],
        top_k: usize,
        exclude: &HashSet<NodeId>,
    ) -> Vec<SemanticHit> {
        let dims = self.stats.dims as usize;
        if top_k == 0 || dims == 0 || query.len() != dims {
            return Vec::new();
        }
        let mut q = query.to_vec();
        if codec::norm(&q) == 0.0 {
            return Vec::new();
        }
        codec::l2_normalise(&mut q);
        // (similarity, row): rows are in (qname, NodeId) order, so a lower row
        // wins a similarity tie.
        let mut best: Vec<(f32, usize)> = Vec::with_capacity(top_k + 1);
        for (row, id) in self.ids.iter().enumerate() {
            if exclude.contains(id) {
                continue;
            }
            let v = &self.data[row * dims..(row + 1) * dims];
            let sim = codec::cosine(&q, v);
            if best.len() == top_k && best.last().is_some_and(|&(s, _)| sim <= s) {
                continue;
            }
            let at = best.partition_point(|&(s, r)| s > sim || (s == sim && r < row));
            best.insert(at, (sim, row));
            best.truncate(top_k);
        }
        best.into_iter()
            .map(|(sim, row)| SemanticHit {
                id: self.ids[row],
                qname: self.qnames[row].clone(),
                similarity: f64::from(sim),
            })
            .collect()
    }

    /// Node `id`'s fresh unit vector, else None (FB.7a: the fused find's
    /// per-file max cosine reads it).
    pub fn vector(&self, id: NodeId) -> Option<&[f32]> {
        let dims = self.stats.dims as usize;
        let row = *self.by_id.get(&id)?;
        self.data.get(row * dims..(row + 1) * dims)
    }

    /// What the load read.
    pub fn stats(&self) -> &IndexStats {
        &self.stats
    }

    /// Embeddable nodes with no fresh vector, sorted by (qname, NodeId): the
    /// embed run's work list.
    pub fn missing_ids(&self) -> Vec<NodeId> {
        self.missing.iter().map(|(_, id)| *id).collect()
    }

    /// The fresh vectors held.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// No fresh vector held.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// One repo's embeddable nodes: id -> (qname, embed text), first instance of
/// each id in graph then node order, and that order.
struct RepoTexts {
    by_id: HashMap<NodeId, (String, String)>,
    order: Vec<NodeId>,
}

impl RepoTexts {
    fn get(&self, id: &NodeId) -> Option<&(String, String)> {
        self.by_id.get(id)
    }
}

fn repo_texts(merged: &MergedGraph, repo: RepoId, recipe: EmbedRecipe) -> RepoTexts {
    let mut out = RepoTexts {
        by_id: HashMap::new(),
        order: Vec::new(),
    };
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in merged.graphs.iter().filter(|g| g.repo == repo) {
        for n in &g.nodes {
            if !seen.insert(n.id) {
                continue;
            }
            if !g
                .nav
                .kind_by_id
                .get(&n.id)
                .is_some_and(|k| EMBED_KINDS.contains(k))
            {
                continue;
            }
            let (Some(q), Some(text)) = (g.nav.qname_by_id.get(&n.id), node_text(g, n, recipe))
            else {
                continue;
            };
            out.by_id.insert(n.id, (q.clone(), text));
            out.order.push(n.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::external_inputs::{VectorRow, b64_encode};
    use glia_embed::{HashEmbedder, MODEL_TEST_HASH};

    use crate::semantic::embed_text;

    const APP: &str = "def fetch_invoice(invoice_id):\n    \"\"\"Fetch one invoice.\"\"\"\n    return invoice_id\n\n\n\
                       def render_chart(points):\n    \"\"\"Draw the chart.\"\"\"\n    return points\n\n\n\
                       class Ledger:\n    def total(self):\n        return 0\n";

    /// FB.7c: an index built in memory from the vectors a sidecar holds is the
    /// loaded index: the same rows in the same (qname, NodeId) order, the same
    /// bits, the same missing list and the same search order; an unknown id is
    /// `unbound`, a second vector for a node and one of other dims are `bad`.
    #[test]
    fn from_vectors_is_the_loaded_index() {
        let td = tempfile::tempdir().expect("tempdir");
        std::fs::write(td.path().join("app.py"), APP).expect("write app.py");
        let r = crate::generate_one(td.path().to_str().expect("utf-8")).expect("generate_one");
        let m = &r.merged;
        let recipe = EmbedRecipe::T2;
        // Every embeddable node but the last gets a fresh test-hash row.
        let mut nodes: Vec<(String, NodeId)> = Vec::new();
        for g in &m.graphs {
            for n in &g.nodes {
                if embed_text(m, n.id, recipe).is_some()
                    && let Some(q) = g.nav.qname_by_id.get(&n.id)
                    && !nodes.iter().any(|(_, id)| *id == n.id)
                {
                    nodes.push((q.clone(), n.id));
                }
            }
        }
        assert!(nodes.len() >= 4, "{nodes:?}");
        let left_out = nodes.pop().expect("a node");
        let mut body = String::new();
        let kind_of = |id: NodeId| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.kind_by_id.get(&id).copied())
                .map(|k| glia_code_domain::node_kind::name(k).to_string())
        };
        for (q, id) in &nodes {
            let text = embed_text(m, *id, recipe).expect("text");
            let v = HashEmbedder::embed_one(&text);
            let row = VectorRow {
                qname: q.clone(),
                kind: kind_of(*id),
                hint: None,
                model: Some(MODEL_TEST_HASH.to_string()),
                dims: Some(v.len() as u32),
                text_hash: Some(text_hash(recipe, &text)),
                device: None,
                b64: b64_encode(&codec::to_le_bytes(&v)),
            };
            body.push_str(&serde_json::to_string(&row).expect("json"));
            body.push('\n');
        }
        let path = td.path().join(VECTORS_FILE);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, body).expect("write vectors");
        let roots: BTreeMap<u64, PathBuf> = r
            .repo_roots
            .iter()
            .map(|(k, v)| (*k, PathBuf::from(v)))
            .collect();
        let loaded = VectorIndex::load(m, &roots, MODEL_TEST_HASH, recipe);
        assert_eq!(loaded.len(), nodes.len(), "{:?}", loaded.stats());

        // The same vectors, newest node first, then an unknown id, a second
        // vector for the first node and one of other dims.
        let mut ids: Vec<NodeId> = loaded.ids.iter().rev().copied().collect();
        let mut vecs: Vec<Vec<f32>> = ids
            .iter()
            .map(|id| loaded.vector(*id).expect("vector").to_vec())
            .collect();
        ids.push(NodeId(u64::MAX));
        vecs.push(vecs[0].clone());
        ids.push(ids[0]);
        vecs.push(vecs[0].clone());
        ids.push(left_out.1);
        vecs.push(vec![1.0, 0.0]);
        let refs: Vec<&[f32]> = vecs.iter().map(Vec::as_slice).collect();
        let mem = VectorIndex::from_vectors(m, &ids, &refs, MODEL_TEST_HASH, recipe);

        assert_eq!(mem.ids, loaded.ids);
        assert_eq!(mem.qnames, loaded.qnames);
        let bits = |x: &[f32]| x.iter().map(|f| f.to_bits()).collect::<Vec<u32>>();
        assert_eq!(bits(&mem.data), bits(&loaded.data));
        assert_eq!(mem.missing_ids(), loaded.missing_ids());
        assert_eq!(mem.missing_ids(), vec![left_out.1]);
        let s = mem.stats();
        assert_eq!(
            (
                s.rows, s.fresh, s.unbound, s.bad, s.stale, s.missing, s.repos
            ),
            (ids.len(), nodes.len(), 1, 2, 0, 1, 1)
        );
        assert_eq!(s.dims, loaded.stats().dims);
        assert_eq!((s.model.as_str(), s.recipe), (MODEL_TEST_HASH, recipe));
        let q = HashEmbedder::embed_one("chart of the points");
        let none = HashSet::new();
        assert_eq!(mem.search(&q, 10, &none), loaded.search(&q, 10, &none));
        // An index of no vector at all holds nothing (a source over it is None).
        let empty = VectorIndex::from_vectors(m, &[], &[], MODEL_TEST_HASH, recipe);
        assert!(empty.is_empty());
        assert_eq!(empty.stats().missing, nodes.len() + 1);
        let emb: std::sync::Arc<dyn glia_embed::Embedder> = std::sync::Arc::new(HashEmbedder);
        assert!(crate::semantic::SemanticSource::from_index(empty, emb.clone()).is_none());
        assert!(crate::semantic::SemanticSource::from_index(mem, emb).is_some());
    }
}
