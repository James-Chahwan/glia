//! The bge-small embedder over fastembed (feature `fastembed`, FB.1).
//!
//! One [`FastEmbedder`] holds one ONNX Runtime session behind a mutex and
//! feeds it sequential [`CHUNK`]-text chunks, one `embed(chunk, Some(CHUNK))`
//! call each, so fastembed's own batch split yields one batch and no second
//! session ever runs beside it (fastembed 4.9.1 sets intra-op threads to every
//! core with no knob; two sessions at once thrash).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::{CHUNK, DIMS_BGE_SMALL, Embedder, MODEL_BGE_SMALL, codec};

/// fastembed's `BGESmallENV15` ([`MODEL_BGE_SMALL`], fp32, [`DIMS_BGE_SMALL`]
/// dims), CPU only, cached under a user cache directory.
pub struct FastEmbedder {
    model: Mutex<TextEmbedding>,
    cache_dir: PathBuf,
}

impl std::fmt::Debug for FastEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastEmbedder")
            .field("model", &MODEL_BGE_SMALL)
            .field("cache_dir", &self.cache_dir)
            .finish()
    }
}

/// The model cache: `FASTEMBED_CACHE_DIR` when set and non-empty, else
/// `$XDG_CACHE_HOME/glia/fastembed`, else `$HOME/.cache/glia/fastembed`.
/// Never fastembed's own default, `.fastembed_cache` under the current
/// directory, which would put the model inside whatever repo glia runs in.
fn cache_dir() -> Result<PathBuf, String> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if let Some(dir) = env("FASTEMBED_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = env("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(xdg).join("glia").join("fastembed"));
    }
    if let Some(home) = env("HOME") {
        return Ok(PathBuf::from(home)
            .join(".cache")
            .join("glia")
            .join("fastembed"));
    }
    Err("no embedding model cache: set FASTEMBED_CACHE_DIR, XDG_CACHE_HOME or HOME".to_string())
}

impl FastEmbedder {
    /// Loads (downloading once into the cache) the bge-small model. Prints
    /// `[embed] model load model=<m> dims=<d> cache=<dir> ms=<t>` on stderr.
    pub fn new() -> Result<Self, String> {
        let dir = cache_dir()?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create model cache {}: {e}", dir.display()))?;
        let start = Instant::now();
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_cache_dir(dir.clone())
                .with_show_download_progress(false),
        )
        .map_err(|e| format!("cannot load {MODEL_BGE_SMALL}: {e}"))?;
        eprintln!(
            "[embed] model load model={MODEL_BGE_SMALL} dims={DIMS_BGE_SMALL} cache={} ms={:.1}",
            dir.display(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(Self {
            model: Mutex::new(model),
            cache_dir: dir,
        })
    }

    /// The directory the model is cached in.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Err when the model cache sits inside `repo_root` (both canonicalised):
    /// a snapshot step must never write the model into the tree it embeds.
    pub fn refuse_repo_dir(&self, repo_root: &Path) -> Result<(), String> {
        let cache = self
            .cache_dir
            .canonicalize()
            .map_err(|e| format!("model cache {}: {e}", self.cache_dir.display()))?;
        let root = repo_root
            .canonicalize()
            .map_err(|e| format!("repo root {}: {e}", repo_root.display()))?;
        if cache.starts_with(&root) {
            return Err(format!(
                "the embedding model cache {} is inside the repository {}: set \
                 FASTEMBED_CACHE_DIR to a user cache directory",
                cache.display(),
                root.display()
            ));
        }
        Ok(())
    }
}

impl Embedder for FastEmbedder {
    fn model(&self) -> &str {
        MODEL_BGE_SMALL
    }

    fn dims(&self) -> u32 {
        DIMS_BGE_SMALL
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let model = self
            .model
            .lock()
            .map_err(|_| "the embedding session was poisoned by an earlier panic".to_string())?;
        let mut out = Vec::with_capacity(texts.len());
        for (i, chunk) in texts.chunks(CHUNK).enumerate() {
            let first = i * CHUNK;
            let range = format!("chunk {i} (texts {first}..{})", first + chunk.len());
            let vectors = model
                .embed(chunk.to_vec(), Some(CHUNK))
                .map_err(|e| format!("{MODEL_BGE_SMALL} {range}: {e}"))?;
            if vectors.len() != chunk.len() {
                return Err(format!(
                    "{MODEL_BGE_SMALL} {range}: {} vectors for {} texts",
                    vectors.len(),
                    chunk.len()
                ));
            }
            for mut v in vectors {
                if v.len() != DIMS_BGE_SMALL as usize {
                    return Err(format!(
                        "{MODEL_BGE_SMALL} {range}: a vector of {} dims, expected {DIMS_BGE_SMALL}",
                        v.len()
                    ));
                }
                // fastembed already normalises; renormalising only off-unit
                // vectors keeps the raw bits Engram's rows hold.
                if (codec::norm(&v) - 1.0).abs() > 1e-6 {
                    codec::l2_normalise(&mut v);
                }
                out.push(v);
            }
        }
        Ok(out)
    }
}
