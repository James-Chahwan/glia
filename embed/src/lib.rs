//! glia-embed: query-time text embeddings (0.5.4, owner FB.1).
//!
//! The crate turns text into vectors for the semantic tiers: fastembed's
//! bge-small behind the `fastembed` feature, and a deterministic `test-hash`
//! model without it, which every test uses. It is never a build input: no
//! build pass calls it, it is not a stamp HASHED_ROOTS entry, and an embedding
//! change never moves PARSER_STAMP. Vectors are written only by the `glia embed`
//! snapshot step, CPU only, never concurrently with a build's rayon pool.
//!
//! Engram's measured learnings are rules here, not suggestions:
//! - the model is `EmbeddingModel::BGESmallENV15` = [`MODEL_BGE_SMALL`]
//!   (`onnx/model.onnx`, fp32, CLS pooling, [`DIMS_BGE_SMALL`] dims), never the
//!   quantised BGESmallENV15Q;
//! - output is L2-normalised and ranked by raw cosine ([`codec::cosine`]);
//!   similarity ranks rows, it never decides whether something is present;
//! - fastembed caches the model under the current directory unless told
//!   otherwise, so the cache is a user cache dir, never inside a repository
//!   (`FastEmbedder::refuse_repo_dir`);
//! - two ONNX Runtime sessions at once thrash the CPU and fastembed sets its
//!   intra-op threads to every core with no knob, so one embedder holds ONE
//!   session and feeds it sequential [`CHUNK`]-text chunks.
//!
//! The engine depends on this crate without features: it needs only the
//! [`Embedder`] trait, since a query is embedded by the caller's embedder.

pub mod codec;
#[cfg(feature = "fastembed")]
mod fast;

#[cfg(feature = "fastembed")]
pub use fast::FastEmbedder;

/// The bge-small model's name, as fastembed and Engram record it.
pub const MODEL_BGE_SMALL: &str = "Xenova/bge-small-en-v1.5";
/// bge-small's vector length.
pub const DIMS_BGE_SMALL: u32 = 384;
/// The deterministic test model's name, recorded on every row it writes.
pub const MODEL_TEST_HASH: &str = "glia-test-hash-v1/64";
/// The test model's vector length.
pub const DIMS_TEST_HASH: u32 = 64;
/// Texts per model call: one session, sequential chunks of this size.
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
}

/// The embedder a CLI / pyo3 model name selects: `test-hash` always,
/// `bge-small` only in a build with the `fastembed` feature.
pub fn named(name: &str) -> Result<Box<dyn Embedder>, String> {
    match name {
        "test-hash" => Ok(Box::new(HashEmbedder)),
        "bge-small" => bge_small(),
        other => Err(format!(
            "unknown embedding model `{other}`: expected one of {}",
            MODEL_NAMES.join(", ")
        )),
    }
}

#[cfg(feature = "fastembed")]
fn bge_small() -> Result<Box<dyn Embedder>, String> {
    Ok(Box::new(FastEmbedder::new()?))
}

#[cfg(not(feature = "fastembed"))]
fn bge_small() -> Result<Box<dyn Embedder>, String> {
    Err(
        "glia was built without the embedding model (feature fastembed): use --model \
         test-hash or a build with embeddings"
            .to_string(),
    )
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
