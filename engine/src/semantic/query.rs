//! The semantic source of `find --files`' fused order (FB.7a, ruling
//! FB-7-FUSED): one model's fresh recipe-t2 vectors ([`VectorIndex`]) and the
//! surface's embedder, read as one score per candidate FILE.
//!
//! FB.6b's fusion gate (`dev-notes/fusion-gate-0.5.4.md`, VERDICT: PASS)
//! measured exactly one reading, and this is it: the query's subject
//! (`intent::text::subject`) prefixed with BGE's retrieval instruction
//! [`QUERY_PREFIX`] is embedded once ([`SemanticSource::query_vector`]), and
//! each candidate file scores the max cosine (`glia_embed::codec::cosine` on
//! unit vectors) over its nodes that hold a fresh vector
//! ([`SemanticSource::file_scores`], rank_eval's `NodeVecs::file_max`), None
//! for a file with none. The score is only ever BLENDED with the lexical
//! BM25 (`calibrate::fusions`' `FUSE[T1s,SEMp:t2]`, behind typed id routing
//! in `LX[IDR,FUSE[T1s,SEMp:t2]]`): similarity alone orders nothing, no node
//! row comes from it, and presence stays lexical (the find answer's
//! absence reads IDR and the text, never this).
//!
//! The vectors come from `.glia/vectors.jsonl` only (ruling VECTORS-OUT):
//! nothing here enters a built graph. Fired_on, stderr: one `[semantic]
//! query model=<m> recipe=t2 dims=<d> ms=<t>` per [`SemanticSource::query_vector`],
//! after the index's own `[semantic] index ..` line.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glia_embed::{Embedder, codec};
use glia_graph::MergedGraph;

use super::index::{IndexStats, VectorIndex};
use super::text::EmbedRecipe;
use crate::intent::text::{Candidates, subject};
use crate::rank::FileIndex;

/// BGE's documented retrieval instruction, prefixed to the query's subject
/// (rank_eval's `SEM_QUERY_PREFIX`, verbatim: the `SEMp:*` rankers' form).
pub const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// The recipe the fused order reads: FB.6b gated `SEMp:t2`.
pub const FUSED_RECIPE: EmbedRecipe = EmbedRecipe::T2;

/// One model's t2 vectors and the embedder that embeds queries into the same
/// space (module docs).
#[non_exhaustive]
pub struct SemanticSource {
    index: VectorIndex,
    embedder: Arc<dyn Embedder>,
}

impl std::fmt::Debug for SemanticSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticSource")
            .field("stats", self.index.stats())
            .field("embedder", &self.embedder.model())
            .finish()
    }
}

impl SemanticSource {
    /// The [`FUSED_RECIPE`] vectors of `model` under every repo root, with
    /// `embedder` for the query. None when the sidecars hold no row at all
    /// (no `glia embed` ran), or when `embedder` embeds another model than
    /// `model` (its query would sit in another space; printed as
    /// `[semantic] query model mismatch rows=<m> embedder=<e>`).
    pub fn load(
        merged: &MergedGraph,
        repo_roots: &BTreeMap<u64, PathBuf>,
        model: &str,
        embedder: Arc<dyn Embedder>,
    ) -> Option<SemanticSource> {
        if embedder.model() != model {
            eprintln!(
                "[semantic] query model mismatch rows={model} embedder={}",
                embedder.model()
            );
            return None;
        }
        let index = VectorIndex::load(merged, repo_roots, model, FUSED_RECIPE);
        Self::from_index(index, embedder)
    }

    /// A source over an index already loaded (None when it read no row).
    pub fn from_index(index: VectorIndex, embedder: Arc<dyn Embedder>) -> Option<SemanticSource> {
        if index.stats().rows == 0 {
            return None;
        }
        Some(SemanticSource { index, embedder })
    }

    /// The query's vector: [`QUERY_PREFIX`] + `subject(query)`, embedded in
    /// one call. Err on an embedder error, or a vector whose length is not
    /// the index's (when the index kept any).
    pub fn query_vector(&self, query: &str) -> Result<Vec<f32>, String> {
        let t0 = Instant::now();
        let text = format!("{QUERY_PREFIX}{}", subject(query));
        let mut out = self.embedder.embed(&[text])?;
        let v = match out.pop() {
            Some(v) if out.is_empty() => v,
            _ => return Err("the embedder returned no single vector for the query".into()),
        };
        let dims = self.index.stats().dims as usize;
        if dims > 0 && v.len() != dims {
            return Err(format!(
                "query vector has {} dims, the index {dims}",
                v.len()
            ));
        }
        eprintln!(
            "[semantic] query model={} recipe={} dims={} ms={}",
            self.embedder.model(),
            FUSED_RECIPE,
            v.len(),
            t0.elapsed().as_millis()
        );
        Ok(v)
    }

    /// Per candidate (in `cands` order), the max cosine of `qvec` over the
    /// candidate file's nodes ([`FileIndex::nodes`], rank_eval's
    /// `file_nodes`) that hold a fresh vector; None for a file with none.
    pub fn file_scores(
        &self,
        files: &FileIndex,
        cands: &Candidates,
        qvec: &[f32],
    ) -> Vec<Option<f64>> {
        cands
            .files
            .iter()
            .map(|&f| {
                if f as usize >= files.len() {
                    return None;
                }
                let mut best: Option<f64> = None;
                for id in files.nodes(f) {
                    let Some(v) = self.index.vector(*id) else {
                        continue;
                    };
                    let c = f64::from(codec::cosine(qvec, v));
                    best = Some(best.map_or(c, |b| b.max(c)));
                }
                best
            })
            .collect()
    }

    /// What the index read.
    pub fn stats(&self) -> &IndexStats {
        self.index.stats()
    }
}

/// The vector model name a surface model name ([`super::surface_model`]:
/// `bge-small` | `test-hash`) records on its rows (`Embedder::model`): what
/// the sidecar's rows are matched by.
pub fn recorded_model(surface: &str) -> &'static str {
    if surface == "test-hash" {
        glia_embed::MODEL_TEST_HASH
    } else {
        glia_embed::MODEL_BGE_SMALL
    }
}

/// A surface's [`SemanticSource`] (FB.7b; the CLI's and pyo3's one loader):
/// the fused-recipe vectors of `surface` under `repo_roots` and the query
/// embedder `embedder` builds, or None: not one repo, no
/// `.glia/vectors.jsonl` under the root (nothing read, nothing printed), no
/// fresh vector of the model, or an embedder that fails to load or embeds
/// another model (a `warning:` stderr line; the answer is the unfused one).
/// The embedder is built only once a fresh vector exists, so a repo without
/// vectors never starts a model.
pub fn open_for_surface(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    surface: &str,
    embedder: impl FnOnce() -> Result<Arc<dyn Embedder>, String>,
) -> Option<Arc<SemanticSource>> {
    if repo_roots.len() != 1 {
        return None;
    }
    let roots: BTreeMap<u64, PathBuf> = repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    if !roots
        .values()
        .any(|r| r.join(super::VECTORS_FILE).is_file())
    {
        return None;
    }
    let model = recorded_model(surface);
    let index = VectorIndex::load(merged, &roots, model, FUSED_RECIPE);
    if index.is_empty() {
        return None;
    }
    let loaded = (|| {
        if surface == "bge-small" {
            for root in roots.values() {
                glia_embed::refuse_repo_cache(root)?;
            }
        }
        embedder()
    })();
    let embedder = match loaded {
        Ok(e) if e.model() == model => e,
        Ok(e) => {
            eprintln!(
                "warning: the query embedder embeds {}, the vectors are {model}; ranking without vectors",
                e.model()
            );
            return None;
        }
        Err(e) => {
            eprintln!(
                "warning: the query embedder ({surface}) did not load, ranking without vectors: {e}"
            );
            return None;
        }
    };
    SemanticSource::from_index(index, embedder).map(Arc::new)
}
