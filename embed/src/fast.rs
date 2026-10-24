//! The bge-small embedder over fastembed (feature `fastembed`, FB.1; the
//! device, FB.14).
//!
//! One [`FastEmbedder`] holds one ONNX Runtime session behind a mutex on one
//! [`Device`]. It feeds the session sub-batches of [`batch`]`(device)` texts,
//! one `embed(sub_batch, Some(len))` call each, so fastembed's own batch split
//! yields one batch, no second batch ever runs beside it on the GPU, and no
//! second session runs beside it on the CPU (fastembed sets intra-op threads
//! to every core; two sessions at once thrash).
//!
//! A GPU [`FastEmbedder`] is built only inside the worker process
//! ([`crate::worker`]); the calling process embeds on the CPU alone. Its
//! provider is registered with `error_on_failure`, so a provider that cannot
//! start is an Err here, never a silent CPU run.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use fastembed::{EmbeddingModel, ExecutionProviderDispatch, InitOptions, TextEmbedding};

use crate::device::{Device, batch};
use crate::{DIMS_BGE_SMALL, Embedder, MODEL_BGE_SMALL, codec};

/// fastembed's `BGESmallENV15` ([`MODEL_BGE_SMALL`], fp32, [`DIMS_BGE_SMALL`]
/// dims) on one [`Device`], cached under a user cache directory
/// ([`crate::cache_dir`]).
pub struct FastEmbedder {
    model: Mutex<Session>,
    cache_dir: PathBuf,
    device: Device,
}

/// The session and the sub-batch it is fed (halved once on a CUDA
/// out-of-memory).
struct Session {
    model: TextEmbedding,
    batch: usize,
}

impl std::fmt::Debug for FastEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastEmbedder")
            .field("model", &MODEL_BGE_SMALL)
            .field("cache_dir", &self.cache_dir)
            .field("device", &self.device)
            .finish()
    }
}

impl FastEmbedder {
    /// Loads (downloading once into the cache) the bge-small model on
    /// `device`. Prints `[embed] model load model=<m> dims=<d> device=<dev>
    /// cache=<dir> ms=<t>` on stderr. Err when `device` is not built into this
    /// binary, or (GPU) when its provider fails to start: no adapter, no
    /// Vulkan ICD, no CUDA runtime.
    pub fn new(device: Device) -> Result<Self, String> {
        if !device.built() {
            return Err(format!(
                "the {device} embedding device is not built into this binary"
            ));
        }
        let dir = crate::cache_dir()?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create model cache {}: {e}", dir.display()))?;
        let start = Instant::now();
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_cache_dir(dir.clone())
                .with_show_download_progress(false)
                .with_execution_providers(providers(device)?),
        )
        .map_err(|e| format!("cannot load {MODEL_BGE_SMALL} on {device}: {e}"))?;
        eprintln!(
            "[embed] model load model={MODEL_BGE_SMALL} dims={DIMS_BGE_SMALL} device={device} \
             cache={} ms={:.1}",
            dir.display(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(Self {
            model: Mutex::new(Session {
                model,
                batch: batch(device),
            }),
            cache_dir: dir,
            device,
        })
    }

    /// The directory the model is cached in.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Err when the model cache sits inside `repo_root` (both canonicalised):
    /// a snapshot step must never write the model into the tree it embeds.
    pub fn refuse_repo_dir(&self, repo_root: &Path) -> Result<(), String> {
        crate::refuse_cache_in(&self.cache_dir, repo_root)
    }

    /// The sub-batch the session is fed now.
    pub fn batch(&self) -> usize {
        self.model.lock().map(|s| s.batch).unwrap_or(1)
    }
}

/// The execution providers a `device` session registers: none for the CPU
/// (ORT's default), else exactly that provider with `error_on_failure`.
fn providers(device: Device) -> Result<Vec<ExecutionProviderDispatch>, String> {
    match device {
        Device::Cpu => Ok(Vec::new()),
        Device::WebGpu => webgpu(),
        Device::Cuda => cuda(),
    }
}

#[cfg(all(
    feature = "gpu-webgpu",
    any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )
))]
fn webgpu() -> Result<Vec<ExecutionProviderDispatch>, String> {
    Ok(vec![
        ort_gpu::ep::WebGPU::default().build().error_on_failure(),
    ])
}

#[cfg(not(all(
    feature = "gpu-webgpu",
    any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )
)))]
fn webgpu() -> Result<Vec<ExecutionProviderDispatch>, String> {
    Err("the webgpu embedding device is not built into this binary".to_string())
}

#[cfg(all(
    feature = "gpu-cuda",
    any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )
))]
fn cuda() -> Result<Vec<ExecutionProviderDispatch>, String> {
    Ok(vec![
        ort_gpu::ep::CUDA::default().build().error_on_failure(),
    ])
}

#[cfg(not(all(
    feature = "gpu-cuda",
    any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )
)))]
fn cuda() -> Result<Vec<ExecutionProviderDispatch>, String> {
    Err("the cuda embedding device is not built into this binary".to_string())
}

/// Whether an ORT run error is an allocation failure (CUDA's out-of-memory
/// is a clean Err naming one of these).
fn is_oom(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    l.contains("out of memory") || l.contains("failed to allocate") || l.contains("bfc_arena")
}

impl Embedder for FastEmbedder {
    fn model(&self) -> &str {
        MODEL_BGE_SMALL
    }

    fn dims(&self) -> u32 {
        DIMS_BGE_SMALL
    }

    fn device(&self) -> Option<Device> {
        Some(self.device)
    }

    /// One vector per text, sub-batch by sub-batch. On CUDA an out-of-memory
    /// halves the sub-batch once and retries; a second one is an Err starting
    /// `oom:` (the worker's fallback reason).
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let mut session = self
            .model
            .lock()
            .map_err(|_| "the embedding session was poisoned by an earlier panic".to_string())?;
        let mut out = Vec::with_capacity(texts.len());
        let mut halved = false;
        let mut first = 0usize;
        while first < texts.len() {
            let end = (first + session.batch).min(texts.len());
            let sub = &texts[first..end];
            let range = format!("texts {first}..{end} on {}", self.device);
            let vectors = match session.model.embed(sub, Some(sub.len())) {
                Ok(v) => v,
                Err(e) => {
                    let msg = e.to_string();
                    if self.device == Device::Cuda && is_oom(&msg) {
                        if !halved && session.batch > 1 {
                            halved = true;
                            session.batch = (session.batch / 2).max(1);
                            eprintln!(
                                "[embed] oom device={} batch halved to {}",
                                self.device, session.batch
                            );
                            continue;
                        }
                        return Err(format!("oom: {MODEL_BGE_SMALL} {range}: {msg}"));
                    }
                    return Err(format!("{MODEL_BGE_SMALL} {range}: {msg}"));
                }
            };
            if vectors.len() != sub.len() {
                return Err(format!(
                    "{MODEL_BGE_SMALL} {range}: {} vectors for {} texts",
                    vectors.len(),
                    sub.len()
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
            first = end;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oom_messages() {
        assert!(is_oom(
            "Failed to allocate memory for requested buffer of size 805306368"
        ));
        assert!(is_oom("CUDA failure 2: out of memory"));
        assert!(!is_oom("Invalid input name: input_ids"));
    }
}
