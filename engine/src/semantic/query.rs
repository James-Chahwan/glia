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
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use glia_embed::{Embedder, codec};
use glia_graph::MergedGraph;

use glia_core::NodeId;

use super::index::{IndexStats, SemanticHit, VectorIndex};
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
    /// The model the rows record: a query embedder of another model is refused.
    model: String,
    /// The query embedder, built on the first query ([`Self::lazy`]) so an
    /// answer that never queries (a pack whose find matched by name) never
    /// starts a model; its load error, once, for every later query.
    embedder: OnceLock<Result<Arc<dyn Embedder>, String>>,
    make: Mutex<Option<MakeEmbedder>>,
    /// The last query and its vector: an answer that ranks files and then
    /// seeds from them (pack's fallback) embeds the query once.
    last: Mutex<Option<(String, Vec<f32>)>>,
}

/// What builds a [`SemanticSource`]'s query embedder on first use.
type MakeEmbedder = Box<dyn FnOnce() -> Result<Arc<dyn Embedder>, String> + Send>;

impl std::fmt::Debug for SemanticSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticSource")
            .field("stats", self.index.stats())
            .field("model", &self.model)
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
        let model = embedder.model().to_string();
        Some(SemanticSource {
            index,
            model,
            embedder: OnceLock::from(Ok(embedder)),
            make: Mutex::new(None),
            last: Mutex::new(None),
        })
    }

    /// A source over an index already loaded whose query embedder `make`
    /// builds on the first query (None when the index read no row). An
    /// embedder that fails to load, or embeds another model than `model`,
    /// prints one `warning:` line and every query is then an Err, so the
    /// answer ranks without vectors (`find --files`' `sem=error`).
    pub fn lazy(
        index: VectorIndex,
        model: &str,
        make: impl FnOnce() -> Result<Arc<dyn Embedder>, String> + Send + 'static,
    ) -> Option<SemanticSource> {
        if index.stats().rows == 0 {
            return None;
        }
        Some(SemanticSource {
            index,
            model: model.to_string(),
            embedder: OnceLock::new(),
            make: Mutex::new(Some(Box::new(make))),
            last: Mutex::new(None),
        })
    }

    /// The query embedder, built now if [`Self::lazy`] deferred it.
    fn embedder(&self) -> Result<&Arc<dyn Embedder>, String> {
        self.embedder
            .get_or_init(|| {
                let make = self.make.lock().ok().and_then(|mut m| m.take());
                let built = match make {
                    Some(make) => make(),
                    None => Err("no query embedder".to_string()),
                };
                let checked = built.and_then(|e| {
                    if e.model() == self.model {
                        Ok(e)
                    } else {
                        Err(format!(
                            "the query embedder embeds {}, the vectors are {}",
                            e.model(),
                            self.model
                        ))
                    }
                });
                if let Err(e) = &checked {
                    eprintln!(
                        "warning: the query embedder did not load, ranking without vectors: {e}"
                    );
                }
                checked
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The query's vector: [`QUERY_PREFIX`] + `subject(query)`, embedded in
    /// one call. Err on an embedder error, or a vector whose length is not
    /// the index's (when the index kept any).
    pub fn query_vector(&self, query: &str) -> Result<Vec<f32>, String> {
        let t0 = Instant::now();
        let text = format!("{QUERY_PREFIX}{}", subject(query));
        if let Some((t, v)) = self.last.lock().ok().as_deref().and_then(Option::as_ref)
            && *t == text
        {
            return Ok(v.clone());
        }
        let embedder = self.embedder()?;
        let mut out = embedder.embed(std::slice::from_ref(&text))?;
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
            embedder.model(),
            FUSED_RECIPE,
            v.len(),
            t0.elapsed().as_millis()
        );
        if let Ok(mut last) = self.last.lock() {
            *last = Some((text, v.clone()));
        }
        Ok(v)
    }

    /// [`Self::query_vector`] of every query in `queries`, in input order, in
    /// ONE embedder call (the embedder batches it) and with no per-query
    /// marker: the calibration replay's find queries (FB.7d), whose caller
    /// prints one `[calibrate] semantic ..` line instead. Err on an embedder
    /// error, a count other than the queries', or a vector whose length is
    /// not the index's.
    pub fn query_vectors(&self, queries: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        let texts: Vec<String> = queries
            .iter()
            .map(|q| format!("{QUERY_PREFIX}{}", subject(q)))
            .collect();
        let out = self.embedder()?.embed(&texts)?;
        if out.len() != texts.len() {
            return Err(format!(
                "the embedder returned {} vectors for {} queries",
                out.len(),
                texts.len()
            ));
        }
        let dims = self.index.stats().dims as usize;
        if let Some(v) = out.iter().find(|v| dims > 0 && v.len() != dims) {
            return Err(format!(
                "query vector has {} dims, the index {dims}",
                v.len()
            ));
        }
        Ok(out)
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

    /// The `top_k` nodes whose vectors are most similar to `qvec`
    /// ([`VectorIndex::search`]'s order: similarity desc, qname, NodeId),
    /// skipping `exclude`; `top_k` 0 keeps none.
    pub fn search(
        &self,
        qvec: &[f32],
        top_k: usize,
        exclude: &HashSet<NodeId>,
    ) -> Vec<SemanticHit> {
        self.index.search(qvec, top_k, exclude)
    }

    /// How many fresh vectors the source holds.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// True when the source holds no fresh vector.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
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
/// The embedder is built on the first query ([`SemanticSource::lazy`]), so a
/// repo without vectors, or an answer that never queries, never starts a
/// model; a load error or a model mismatch is then a `warning:` and the
/// answer ranks without vectors.
pub fn open_for_surface(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    surface: &str,
    embedder: impl FnOnce() -> Result<Arc<dyn Embedder>, String> + Send + 'static,
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
    if surface == "bge-small" {
        for root in roots.values() {
            if let Err(e) = glia_embed::refuse_repo_cache(root) {
                eprintln!(
                    "warning: the query embedder ({surface}) did not load, ranking without vectors: {e}"
                );
                return None;
            }
        }
    }
    SemanticSource::lazy(index, model, embedder).map(Arc::new)
}
