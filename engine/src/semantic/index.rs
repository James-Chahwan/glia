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

use std::collections::{BTreeMap, HashMap, HashSet};
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

        kept.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        missing.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        stats.dims = dims.unwrap_or(0);
        stats.missing = missing.len();
        let mut index = VectorIndex {
            data: Vec::with_capacity(kept.len() * stats.dims as usize),
            ids: Vec::with_capacity(kept.len()),
            qnames: Vec::with_capacity(kept.len()),
            missing,
            stats,
        };
        for (q, id, v) in kept {
            index.data.extend_from_slice(&v);
            index.ids.push(id);
            index.qnames.push(q);
        }
        let s = &index.stats;
        eprintln!(
            "[semantic] index repos={} model={} recipe={} dims={} rows={} fresh={} stale={} \
             missing={} other_model={} unbound={} bad={} ms={}",
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
