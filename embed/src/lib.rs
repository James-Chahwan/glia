//! glia-embed: query-time text embeddings (0.5.4, owner FB.1; devices FB.14).
//!
//! The crate turns text into vectors for the semantic tiers: fastembed's
//! bge-small behind the `fastembed` feature, and a deterministic `test-hash`
//! model without it, which every offline test uses. It is never a build input:
//! no build pass calls it, it is not a stamp HASHED_ROOTS entry, and an
//! embedding change never moves PARSER_STAMP. Vectors are written only by the
//! `glia embed` snapshot step, never concurrently with a build's rayon pool.
//!
//! Engram's measured learnings are rules here, not suggestions:
//! - the model is `EmbeddingModel::BGESmallENV15` = [`MODEL_BGE_SMALL`]
//!   (`onnx/model.onnx`, fp32, CLS pooling, [`DIMS_BGE_SMALL`] dims), never the
//!   quantised BGESmallENV15Q;
//! - output is L2-normalised and ranked by raw cosine ([`codec::cosine`]);
//!   similarity ranks rows, it never decides whether something is present;
//! - fastembed caches the model under the current directory unless told
//!   otherwise, so the cache is a user cache dir ([`cache_dir`]), never inside
//!   a repository ([`refuse_repo_cache`], checked before a model loads);
//! - two ONNX Runtime sessions at once thrash the CPU and fastembed sets its
//!   intra-op threads to every core with no knob, so one embedder holds ONE
//!   session and feeds it sequential chunks.
//!
//! DEVICES (FB.14, [`device`], [`worker`]): bge-small runs on the GPU by
//! default. [`bge_small`] picks the device from `GLIA_EMBED_DEVICE` (unset =
//! WebGPU when built, else the CPU); a GPU session always runs in a
//! crash-isolated worker process, and any GPU failure falls back to the CPU
//! in-process, saying why on the `[embed] device=.. backend=.. driver=..
//! fallback=.. batch=..` line. Every vector carries the device it came from
//! ([`Embedder::embed_tagged`]); the devices agree to 4e-7, so a row's device
//! never decides its staleness.
//!
//! The ORT stack does not exist on x86_64 macOS (no ONNX Runtime >= 1.24 build
//! for x86_64-apple-darwin, ruling INTEL-MAC-NO-EMBED): there `bge-small`
//! refuses ([`INTEL_MAC_REFUSAL`]) and only `test-hash` embeds.
//!
//! The engine depends on this crate without features: it needs only the
//! [`Embedder`] trait, since a query is embedded by the caller's embedder.

use std::path::{Component, Path, PathBuf};

pub mod codec;
pub mod device;
#[cfg(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
))]
mod fast;
pub mod worker;

pub use device::Device;
#[cfg(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
))]
pub use fast::FastEmbedder;

/// Whether this binary carries ONNX Runtime (feature `fastembed` on a target
/// other than x86_64 macOS), and so can run bge-small on the CPU.
pub const ORT_BUILT: bool = cfg!(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
));

/// What `bge-small` answers on x86_64 macOS, feature or not.
pub const INTEL_MAC_REFUSAL: &str = "embeddings are not available on Intel macOS (no ONNX \
     Runtime >= 1.24 build exists for x86_64-apple-darwin)";

/// One vector and the device that computed it (None: a model with no device,
/// the test model).
pub type Tagged = (Vec<f32>, Option<Device>);

/// The bge-small model's name, as fastembed and Engram record it.
pub const MODEL_BGE_SMALL: &str = "Xenova/bge-small-en-v1.5";
/// bge-small's vector length.
pub const DIMS_BGE_SMALL: u32 = 384;
/// The deterministic test model's name, recorded on every row it writes.
pub const MODEL_TEST_HASH: &str = "glia-test-hash-v1/64";
/// The test model's vector length.
pub const DIMS_TEST_HASH: u32 = 64;
/// Texts per model call on the CPU: one session, sequential chunks of this
/// size (the GPU's sub-batch is [`device::batch`]).
pub const CHUNK: usize = 256;
/// The model spellings the CLI (`--model`) and pyo3 accept, sorted.
pub const MODEL_NAMES: [&str; 2] = ["bge-small", "test-hash"];

/// A text embedder: one L2-normalised vector per text.
pub trait Embedder: Send + Sync {
    /// The model name recorded on every vector this embedder writes.
    fn model(&self) -> &str;
    /// The length of every vector [`Embedder::embed`] returns.
    fn dims(&self) -> u32;
    /// One unit vector per text, in input order, each [`Embedder::dims`]
    /// long; Err names the chunk that failed.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;

    /// The device this embedder's vectors come from now; None for a model
    /// with no device (the test model).
    fn device(&self) -> Option<Device> {
        None
    }

    /// [`Embedder::embed`] with the device each vector came from: one device
    /// per call by default, per text for an embedder that can fall back
    /// mid-call (the [`worker::WorkerEmbedder`]).
    fn embed_tagged(&self, texts: &[String]) -> Result<Vec<Tagged>, String> {
        let device = self.device();
        Ok(self
            .embed(texts)?
            .into_iter()
            .map(|v| (v, device))
            .collect())
    }
}

/// The embedder a CLI / pyo3 model name selects: `test-hash` always,
/// `bge-small` through [`bge_small`] with the worker found beside the current
/// executable ([`worker::WorkerProgram::beside_current_exe`]; none: the CPU
/// with fallback `no_worker`).
pub fn named(name: &str) -> Result<Box<dyn Embedder>, String> {
    match name {
        "test-hash" => Ok(Box::new(HashEmbedder)),
        "bge-small" => bge_small(worker::WorkerProgram::beside_current_exe()),
        other => Err(format!(
            "unknown embedding model `{other}`: expected one of {}",
            MODEL_NAMES.join(", ")
        )),
    }
}

/// bge-small on the device `GLIA_EMBED_DEVICE` selects ([`device`] module
/// doc), its GPU session in a worker started from `worker`:
/// - `cpu`: an in-process CPU `FastEmbedder`, fallback `override`;
/// - `webgpu` / `cuda`: a strict [`worker::WorkerEmbedder`] (a start failure
///   is an Err naming the device);
/// - unset: a [`worker::WorkerEmbedder`] on WebGPU that falls back to the CPU
///   (`not_built`, `no_worker`, `no_adapter:..`, `gpu_crash:..`, `oom`).
///
/// Err when this binary has no ONNX Runtime (feature `fastembed` off, or
/// x86_64 macOS) or `GLIA_EMBED_DEVICE` is misspelt. The model loads lazily,
/// on the first embed, in whichever process runs it.
pub fn bge_small(worker: Option<worker::WorkerProgram>) -> Result<Box<dyn Embedder>, String> {
    if !ORT_BUILT {
        return Err(no_ort());
    }
    match device::requested()? {
        Some(Device::Cpu) => {
            let e = cpu_bge_small()?;
            eprintln!(
                "{}",
                device::marker(
                    &device::cpu_adapter(),
                    Device::Cpu,
                    Some("override"),
                    device::batch(Device::Cpu)
                )
            );
            Ok(e)
        }
        Some(d) => Ok(Box::new(worker::WorkerEmbedder::new(
            "bge-small",
            d,
            true,
            worker,
            cpu_bge_small,
        )?)),
        None => Ok(Box::new(worker::WorkerEmbedder::new(
            "bge-small",
            Device::WebGpu,
            false,
            worker,
            cpu_bge_small,
        )?)),
    }
}

/// An in-process CPU bge-small session (the fallback every worker embedder
/// holds).
pub fn cpu_bge_small() -> Result<Box<dyn Embedder>, String> {
    #[cfg(all(
        feature = "fastembed",
        not(all(target_os = "macos", target_arch = "x86_64"))
    ))]
    {
        Ok(Box::new(FastEmbedder::new(Device::Cpu)?))
    }
    #[cfg(not(all(
        feature = "fastembed",
        not(all(target_os = "macos", target_arch = "x86_64"))
    )))]
    {
        Err(no_ort())
    }
}

fn no_ort() -> String {
    if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        INTEL_MAC_REFUSAL.to_string()
    } else {
        "glia was built without the embedding model (feature fastembed): use --model \
         test-hash or a build with embeddings"
            .to_string()
    }
}

/// The model cache: `FASTEMBED_CACHE_DIR` when set and non-empty, else
/// `$XDG_CACHE_HOME/glia/fastembed`, else `$HOME/.cache/glia/fastembed`.
/// Never fastembed's own default, `.fastembed_cache` under the current
/// directory, which would put the model inside whatever repo glia runs in.
pub fn cache_dir() -> Result<PathBuf, String> {
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

/// Err when the model cache ([`cache_dir`]) would sit inside `repo_root`:
/// the pre-load check every surface runs before a model loads (or
/// downloads), so a refused run fetches nothing. The cache need not exist
/// yet: its longest existing prefix is canonicalised and the rest joined.
pub fn refuse_repo_cache(repo_root: &Path) -> Result<(), String> {
    refuse_cache_in(&cache_dir()?, repo_root)
}

/// Err when `cache` is inside `repo_root` (module doc's cache rule).
pub(crate) fn refuse_cache_in(cache: &Path, repo_root: &Path) -> Result<(), String> {
    let cache = absolute(cache)?;
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

/// `p` made absolute with its longest existing prefix canonicalised and the
/// rest joined lexically (`..` pops, `.` drops).
fn absolute(p: &Path) -> Result<PathBuf, String> {
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("current directory: {e}"))?
            .join(p)
    };
    let mut existing = p.as_path();
    let mut rest: Vec<Component> = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            for comp in rest.iter().rev() {
                match comp {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::CurDir => {}
                    other => out.push(other.as_os_str()),
                }
            }
            return Ok(out);
        }
        let Some(parent) = existing.parent() else {
            return Ok(p.clone());
        };
        if let Some(last) = existing.components().next_back() {
            rest.push(last);
        }
        existing = parent;
    }
}

/// The deterministic test-only embedder ([`MODEL_TEST_HASH`],
/// [`DIMS_TEST_HASH`] dims).
///
/// It is a feature hash, not a language model: tokens are the text split on
/// non-alphanumerics and at camel-case steps (lower -> Upper, letter -> digit),
/// lowercased, two or more characters long. Each token adds +-1 to one slot
/// (FNV-1a 64 of its bytes: slot `h % 64`, sign + when bit 63 is set) and each
/// adjacent token pair `"t1 t2"` adds +-0.5 the same way, so word order counts
/// a little; the sum is L2-normalised, and a text with no token is the unit
/// vector e0. Integer hashing and f32 adds in token order make it bit-identical
/// on every machine. Rows it writes carry its own model name, so no reader of
/// bge-small rows ever reads them; it exists so tests need no model download.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashEmbedder;

impl HashEmbedder {
    /// One text's vector.
    pub fn embed_one(text: &str) -> Vec<f32> {
        let dims = DIMS_TEST_HASH as usize;
        let mut v = vec![0.0f32; dims];
        let tokens = tokens(text);
        if tokens.is_empty() {
            v[0] = 1.0;
            return v;
        }
        for t in &tokens {
            add(&mut v, t.as_bytes(), 1.0);
        }
        for pair in tokens.windows(2) {
            let joined = format!("{} {}", pair[0], pair[1]);
            add(&mut v, joined.as_bytes(), 0.5);
        }
        codec::l2_normalise(&mut v);
        if v.iter().all(|&x| x == 0.0) {
            // Every hash cancelled out: fall back to e0, still a unit vector.
            v[0] = 1.0;
        }
        v
    }
}

impl Embedder for HashEmbedder {
    fn model(&self) -> &str {
        MODEL_TEST_HASH
    }

    fn dims(&self) -> u32 {
        DIMS_TEST_HASH
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| Self::embed_one(t)).collect())
    }
}

fn add(v: &mut [f32], bytes: &[u8], weight: f32) {
    let h = fnv1a64(bytes);
    let slot = (h % v.len() as u64) as usize;
    let sign = if h >> 63 == 1 { 1.0 } else { -1.0 };
    v[slot] += sign * weight;
}

/// FNV-1a, 64-bit.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The test model's tokens: alphanumeric runs split at camel steps,
/// lowercased, two characters or more.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        let mut cur = String::new();
        let mut prev: Option<char> = None;
        for c in word.chars() {
            if let Some(p) = prev {
                let camel = p.is_lowercase() && c.is_uppercase();
                let digit = p.is_alphabetic() && c.is_numeric();
                if camel || digit {
                    push_token(&mut out, &mut cur);
                }
            }
            cur.extend(c.to_lowercase());
            prev = Some(c);
        }
        push_token(&mut out, &mut cur);
    }
    out
}

fn push_token(out: &mut Vec<String>, cur: &mut String) {
    if cur.chars().count() >= 2 {
        out.push(std::mem::take(cur));
    } else {
        cur.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_split_on_camel_and_digits() {
        assert_eq!(
            tokens("UserService.getUser"),
            vec!["user", "service", "get", "user"]
        );
        assert_eq!(tokens("parseV2 a_b x"), vec!["parse"]);
        assert_eq!(tokens("http2Server"), vec!["http", "2server"]);
        assert!(tokens(" .. ").is_empty());
    }

    #[test]
    fn fnv_known_values() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
