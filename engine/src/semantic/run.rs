//! The embed run (FB.5a): [`embed_repo`] brings one repo's
//! `.glia/vectors.jsonl` up to date with the graph, the work behind `glia
//! embed`, `watch --embed` and the install-hooks embed line. It is a snapshot
//! step: no build pass calls it, and a build never reads what it writes.
//!
//! THE RUN, in order:
//! 1. Refuse to start inside a rayon worker (Engram's finding: an ONNX
//!    session beside a busy pool thrashes every core). The caller runs it
//!    after its build has returned.
//! 2. Read the sidecar. A row of another model (or one naming none) is an
//!    Err naming `--replace` unless [`EmbedOptions::replace`]: one model per
//!    file (FB.2).
//! 3. [`VectorIndex::load`] for this repo, model and recipe: its
//!    [`VectorIndex::missing_ids`] is the work list.
//! 4. Every sidecar row of this model that records a `text_hash` (and has the
//!    embedder's dims) seeds a cache keyed by that hash. A missing node whose
//!    embed text hashes to a cached vector reuses its bytes (a moved or
//!    renamed node whose text did not change); the rest are queued.
//! 5. The queue (at most [`EmbedOptions::limit`] nodes) is embedded in
//!    sequential [`glia_embed::CHUNK`]-text chunks, one model call each.
//! 6. The file becomes: the fresh rows (bytes unchanged), the new rows, and
//!    the foreign rows (this model, no `text_hash`: written by something
//!    other than glia, kept untouched unless a glia row now holds their
//!    `vector_key`). Every other row of this model is stale and dropped; under
//!    `replace`, every row of another model too. It is written once, sorted by
//!    `vector_key`, through `glia_store::write_vector_rows` under
//!    `.glia/cells.lock`. No row carries a time, so one tree, model and file
//!    give byte-identical output.
//!
//! A missing node sharing its (qname, kind) with another embeddable node of
//! the repo is skipped and counted `ambiguous`: the sidecar keys a row by
//! (qname, kind), so no row could hold one vector per twin.
//!
//! The model cache rule (`FastEmbedder::refuse_repo_dir`) is the caller's:
//! the engine holds the model only as `&dyn Embedder`, so the surface that
//! loads bge-small checks its cache directory before the model loads.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use glia_code_domain::external_inputs::{VectorRow, b64_encode, read_rows, vector_key};
use glia_code_domain::node_kind;
use glia_code_domain::profile::EMBED_KINDS;
use glia_core::{NodeId, RepoId};
use glia_embed::{CHUNK, Embedder, codec};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex};
use glia_graph::identity::identity_of;
use glia_store::{VectorWriteMode, write_vector_rows};
use serde::Serialize;

use super::index::{VECTORS_FILE, VectorIndex};
use super::text::{DEFAULT_RECIPE, EmbedRecipe, node_text, text_hash};

/// The environment variable [`surface_model`] reads.
pub const EMBED_MODEL_ENV: &str = "GLIA_EMBED_MODEL";

/// The `glia_embed::named` model a surface uses: [`EMBED_MODEL_ENV`] when it
/// names one of `glia_embed::MODEL_NAMES` (`bge-small` | `test-hash`), else
/// `bge-small`. An unset or empty variable is the default silently; any other
/// value prints `[embed] GLIA_EMBED_MODEL=<v> unknown, using bge-small`. The
/// one reader of the variable, so `glia embed`, `watch --embed` and the
/// semantic tiers agree, and offline tests run on `test-hash`.
pub fn surface_model() -> &'static str {
    let v = std::env::var(EMBED_MODEL_ENV).unwrap_or_default();
    model_for(&v)
}

fn model_for(v: &str) -> &'static str {
    if let Some(m) = glia_embed::MODEL_NAMES.iter().find(|m| **m == v) {
        return m;
    }
    if !v.is_empty() {
        eprintln!("[embed] {EMBED_MODEL_ENV}={v} unknown, using bge-small");
    }
    "bge-small"
}

/// What one [`embed_repo`] run does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EmbedOptions {
    /// The text recipe the vectors embed.
    pub recipe: EmbedRecipe,
    /// Rewrite a sidecar holding another model's rows (dropping them).
    pub replace: bool,
    /// Embed at most this many missing nodes (reused vectors do not count);
    /// None embeds all.
    pub limit: Option<usize>,
    /// Count the work and write nothing; the model is never called.
    pub check_only: bool,
}

impl Default for EmbedOptions {
    fn default() -> Self {
        EmbedOptions {
            recipe: DEFAULT_RECIPE,
            replace: false,
            limit: None,
            check_only: false,
        }
    }
}

impl EmbedOptions {
    /// These options under `recipe`.
    pub fn with_recipe(mut self, recipe: EmbedRecipe) -> Self {
        self.recipe = recipe;
        self
    }

    /// These options with `replace` set.
    pub fn with_replace(mut self, replace: bool) -> Self {
        self.replace = replace;
        self
    }

    /// These options embedding at most `limit` nodes.
    pub fn with_limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
    }

    /// These options counting only.
    pub fn with_check_only(mut self, check_only: bool) -> Self {
        self.check_only = check_only;
        self
    }
}

/// What one [`embed_repo`] run did. Under `check_only`, `embedded` and
/// `reused` count what a run would do and `rows_written` is 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct EmbedReport {
    /// The repo's label (its directory name).
    pub repo: String,
    /// The model name recorded on the rows.
    pub model: String,
    /// The recipe embedded.
    #[serde(serialize_with = "recipe_str")]
    pub recipe: EmbedRecipe,
    /// The vectors' length.
    pub dims: u32,
    /// The repo's embeddable ([`EMBED_KINDS`]) nodes.
    pub nodes: usize,
    /// Fresh rows kept as they were.
    pub cached: usize,
    /// Missing nodes given a vector already in the file under another key
    /// (same text hash: a moved or renamed node).
    pub reused: usize,
    /// Missing nodes the model embedded.
    pub embedded: usize,
    /// Missing nodes skipped: they share (qname, kind) with another node.
    pub ambiguous: usize,
    /// Missing nodes left for a later run by [`EmbedOptions::limit`].
    pub deferred: usize,
    /// Rows dropped: bound to no node any more, embedding changed text,
    /// superseded, or (under `replace`) of another model.
    pub stale_dropped: usize,
    /// Rows of this model glia did not write (no `text_hash`), kept.
    pub foreign_kept: usize,
    /// Rows in the file written (0 under `check_only`).
    pub rows_written: usize,
    /// Wall time of the run.
    pub ms: u64,
}

fn recipe_str<S: serde::Serializer>(r: &EmbedRecipe, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(r.as_str())
}

/// One embeddable node of the repo: what a row for it holds.
struct NodeInfo {
    qname: String,
    kind: &'static str,
    text: String,
}

/// Embed `repo`'s nodes into `<root>/.glia/vectors.jsonl` (module doc).
/// Prints `[embed] chunk <i>/<n> texts=<k> ms=<t>` per model call and one
/// `[embed] repo=<label> model=<m> recipe=<t> nodes=<n> cached=<c> reused=<r>
/// embedded=<e> stale_dropped=<s> foreign_kept=<f> rows=<w> ms=<t>` line.
pub fn embed_repo(
    root: &Path,
    merged: &MergedGraph,
    repo: RepoId,
    embedder: &dyn Embedder,
    opts: &EmbedOptions,
) -> Result<EmbedReport, String> {
    let t0 = Instant::now();
    if rayon::current_thread_index().is_some() {
        return Err(
            "embed_repo runs inside a rayon worker: embed after the build's pool \
                    has finished (one model session beside a busy pool thrashes every core)"
                .to_string(),
        );
    }
    let model = embedder.model().to_string();
    let dims = embedder.dims();
    let recipe = opts.recipe;
    let path = root.join(VECTORS_FILE);

    // (a) The sidecar, one model per file.
    let (existing, errors) = read_rows::<VectorRow>(&path);
    if let Some(first) = errors.first() {
        return Err(format!(
            "{} has {} unreadable line(s) (first: {first}); fix or remove them before embedding",
            path.display(),
            errors.len()
        ));
    }
    if !opts.replace
        && let Some(other) = existing
            .iter()
            .find(|r| r.model.as_deref() != Some(model.as_str()))
    {
        return Err(format!(
            "{} holds vectors of model {}; pass --replace to rewrite it with {model}",
            path.display(),
            other.model.as_deref().unwrap_or("<none>")
        ));
    }

    // (b) The work list.
    let roots: BTreeMap<u64, PathBuf> = BTreeMap::from([(repo.0, root.to_path_buf())]);
    let index = VectorIndex::load(merged, &roots, &model, recipe);
    let missing = index.missing_ids();
    let missing_set: HashSet<NodeId> = missing.iter().copied().collect();
    let infos = repo_nodes(merged, repo, recipe);
    let mut key_count: HashMap<(&str, &str), usize> = HashMap::new();
    for info in infos.values() {
        *key_count
            .entry((info.qname.as_str(), info.kind))
            .or_default() += 1;
    }

    // (c) Classify the rows and seed the cache.
    let binder = QnameIndex::build(merged, Some(repo));
    let mut cache: HashMap<String, (String, Option<u32>)> = HashMap::new();
    let mut fresh: Vec<VectorRow> = Vec::new();
    let mut fresh_ids: HashSet<NodeId> = HashSet::new();
    let mut foreign: Vec<VectorRow> = Vec::new();
    let mut stale_dropped = 0usize;
    for row in existing {
        if row.model.as_deref() != Some(model.as_str()) {
            // Only reached under `replace`.
            stale_dropped += 1;
            continue;
        }
        let Some(hash) = row.text_hash.clone() else {
            foreign.push(row);
            continue;
        };
        if row_dims_ok(&row, dims) {
            cache
                .entry(hash.clone())
                .or_insert_with(|| (row.b64.clone(), row.dims));
        }
        let id = match binder.resolve(&row.qname, row.kind.as_deref(), row.hint.as_deref()) {
            CellTarget::Bound(id) | CellTarget::Rekeyed { id, .. } => Some(id),
            _ => None,
        };
        let keep = id.filter(|id| {
            !missing_set.contains(id)
                && !fresh_ids.contains(id)
                && row_dims_ok(&row, dims)
                && infos
                    .get(id)
                    .is_some_and(|info| text_hash(recipe, &info.text) == hash)
        });
        match keep {
            Some(id) => {
                fresh_ids.insert(id);
                fresh.push(row);
            }
            None => stale_dropped += 1,
        }
    }

    // (d) Reuse by text hash, else queue.
    let mut new_rows: Vec<VectorRow> = Vec::new();
    let mut queue: Vec<(NodeId, String, String)> = Vec::new();
    let (mut reused, mut ambiguous) = (0usize, 0usize);
    for id in &missing {
        let Some(info) = infos.get(id) else {
            continue;
        };
        if key_count
            .get(&(info.qname.as_str(), info.kind))
            .is_some_and(|n| *n > 1)
        {
            ambiguous += 1;
            continue;
        }
        let hash = text_hash(recipe, &info.text);
        match cache.get(&hash) {
            Some((b64, _)) => {
                reused += 1;
                new_rows.push(new_row(merged, *id, info, &model, dims, hash, b64.clone()));
            }
            None => queue.push((*id, info.text.clone(), hash)),
        }
    }
    let deferred = opts.limit.map_or(0, |n| queue.len().saturating_sub(n));
    queue.truncate(queue.len() - deferred);
    let embedded = queue.len();

    // (e) Sequential chunks, one model call each.
    if !opts.check_only && !queue.is_empty() {
        let chunks = queue.len().div_ceil(CHUNK);
        for (i, chunk) in queue.chunks(CHUNK).enumerate() {
            let tc = Instant::now();
            let texts: Vec<String> = chunk.iter().map(|(_, t, _)| t.clone()).collect();
            let vectors = embedder
                .embed(&texts)
                .map_err(|e| format!("embedding chunk {}/{chunks}: {e}", i + 1))?;
            if vectors.len() != chunk.len() {
                return Err(format!(
                    "embedding chunk {}/{chunks}: {} vectors for {} texts",
                    i + 1,
                    vectors.len(),
                    chunk.len()
                ));
            }
            for ((id, _, hash), v) in chunk.iter().zip(vectors) {
                if v.len() != dims as usize {
                    return Err(format!(
                        "embedding chunk {}/{chunks}: a vector of {} dims, model {model} has {dims}",
                        i + 1,
                        v.len()
                    ));
                }
                let Some(info) = infos.get(id) else {
                    continue;
                };
                let b64 = b64_encode(&codec::to_le_bytes(&v));
                new_rows.push(new_row(merged, *id, info, &model, dims, hash.clone(), b64));
            }
            eprintln!(
                "[embed] chunk {}/{chunks} texts={} ms={}",
                i + 1,
                chunk.len(),
                tc.elapsed().as_millis()
            );
        }
    }

    // (f) The file: fresh + new + foreign not superseded.
    let cached = fresh.len();
    let mut rows = fresh;
    rows.extend(new_rows);
    let taken: HashSet<(String, String)> = rows.iter().map(vector_key).collect();
    let mut foreign_kept = 0usize;
    for row in foreign {
        if taken.contains(&vector_key(&row)) {
            stale_dropped += 1;
        } else {
            foreign_kept += 1;
            rows.push(row);
        }
    }
    rows.sort_by_key(vector_key);

    // (g) One locked write.
    let rows_written = if opts.check_only {
        0
    } else {
        let mode = if opts.replace {
            VectorWriteMode::Replace
        } else {
            VectorWriteMode::MergeSameModel
        };
        write_vector_rows(root, rows, mode).map_err(|e| e.to_string())?
    };

    let report = EmbedReport {
        repo: crate::arch::repo_label_for(&root.to_string_lossy()),
        model,
        recipe,
        dims,
        nodes: infos.len(),
        cached,
        reused,
        embedded,
        ambiguous,
        deferred,
        stale_dropped,
        foreign_kept,
        rows_written,
        ms: u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX),
    };
    eprintln!(
        "[embed] repo={} model={} recipe={} nodes={} cached={} reused={} embedded={} \
         stale_dropped={} foreign_kept={} rows={} ms={}",
        report.repo,
        report.model,
        report.recipe,
        report.nodes,
        report.cached,
        report.reused,
        report.embedded,
        report.stale_dropped,
        report.foreign_kept,
        report.rows_written,
        report.ms
    );
    Ok(report)
}

/// A row's payload can stand for `dims`: it names those dims, or none and
/// decodes to that many f32s.
fn row_dims_ok(row: &VectorRow, dims: u32) -> bool {
    match row.dims {
        Some(d) => d == dims,
        None => glia_code_domain::external_inputs::b64_decode(&row.b64)
            .is_ok_and(|b| b.len() == dims as usize * 4),
    }
}

fn new_row(
    merged: &MergedGraph,
    id: NodeId,
    info: &NodeInfo,
    model: &str,
    dims: u32,
    hash: String,
    b64: String,
) -> VectorRow {
    VectorRow {
        qname: info.qname.clone(),
        kind: Some(info.kind.to_string()),
        hint: identity_of(merged, id).map(|i| i.hint()),
        model: Some(model.to_string()),
        dims: Some(dims),
        text_hash: Some(hash),
        b64,
    }
}

/// The repo's embeddable nodes by id (first instance of each id, as the
/// index reads them). Looked up only, never iterated into an output.
fn repo_nodes(
    merged: &MergedGraph,
    repo: RepoId,
    recipe: EmbedRecipe,
) -> HashMap<NodeId, NodeInfo> {
    let mut out: HashMap<NodeId, NodeInfo> = HashMap::new();
    for g in merged.graphs.iter().filter(|g| g.repo == repo) {
        for n in &g.nodes {
            if out.contains_key(&n.id) {
                continue;
            }
            let Some(kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if !EMBED_KINDS.contains(kind) {
                continue;
            }
            let Some(kind_name) = node_kind::ALL
                .iter()
                .find(|(k, _)| k == kind)
                .map(|(_, name)| *name)
            else {
                continue;
            };
            let (Some(qname), Some(text)) = (g.nav.qname_by_id.get(&n.id), node_text(g, n, recipe))
            else {
                continue;
            };
            out.insert(
                n.id,
                NodeInfo {
                    qname: qname.clone(),
                    kind: kind_name,
                    text,
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_model_reads_the_env_spelling() {
        assert_eq!(model_for(""), "bge-small");
        assert_eq!(model_for("test-hash"), "test-hash");
        assert_eq!(model_for("bge-small"), "bge-small");
        assert_eq!(model_for("nope"), "bge-small");
    }
}
