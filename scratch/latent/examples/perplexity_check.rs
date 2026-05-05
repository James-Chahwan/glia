//! Perplexity sanity check on injection stability.
//!
//! Loads v8 SOLVE artifacts (prefix, suffix, summaries) and computes the
//! perplexity of the SUFFIX tokens under three conditions:
//!   - text_only:      prefix || suffix
//!   - real_vectors:   prefix || pooled vecs from summaries || suffix
//!   - random_vectors: prefix || random Gaussians (matched mean L2 norm) || suffix
//!
//! Outputs JSON to <out_json>: {text_only, real_vectors, random_vectors, ...}
//!
//! Interpretation:
//!   real << random  → vectors carry signal (not just noise)
//!   real ≈ text     → vectors don't degrade prediction
//!   real >> text    → vectors corrupt context
//!
//! cargo run --release --example perplexity_check -- \
//!     <gguf> <tok> <prefix_file> <suffix_file> <summaries_json> <out_json>

use anyhow::{Context, Result, anyhow};
use candle_core::quantized::gguf_file;
use candle_core::{DType, Device, Tensor};
use candle_nn::ops::log_softmax;
use repo_graph_latent::ForwardEmbeds;
use repo_graph_latent::qwen2_hacked::ModelWeights;
use serde::Deserialize;
use std::io::Write as _;
use tokenizers::Tokenizer;

#[derive(Debug, Deserialize)]
struct NodeSummary {
    #[allow(dead_code)]
    id: u64,
    #[allow(dead_code)]
    qname: String,
    #[allow(dead_code)]
    score: f32,
    summary: String,
}

fn tokenize(tok: &Tokenizer, text: &str, device: &Device) -> Result<Tensor> {
    let enc = tok.encode(text, true).map_err(|e| anyhow!("encode: {e}"))?;
    let ids: Vec<u32> = enc.get_ids().to_vec();
    Ok(Tensor::new(ids.as_slice(), device)?.unsqueeze(0)?)
}

/// Mean cross-entropy over `suffix_ids`, given full-sequence logits `[1, T_total, V]`.
/// Uses logits at positions `[suffix_start - 1 .. suffix_start - 1 + Ts]` (causal: position p
/// predicts the token at position p+1, so position S-1 predicts the first suffix token).
fn suffix_cross_entropy(logits: &Tensor, suffix_ids: &[u32], suffix_start: usize) -> Result<f32> {
    let ts = suffix_ids.len();
    let pred = logits.narrow(1, suffix_start - 1, ts)?; // [1, Ts, V]
    let log_probs = log_softmax(&pred, candle_core::D::Minus1)?
        .squeeze(0)?
        .to_dtype(DType::F32)?; // [Ts, V]
    let v = log_probs.dim(1)?;
    let lp_vec: Vec<f32> = log_probs.flatten_all()?.to_vec1()?;
    let mut total = 0f32;
    for (i, &tok) in suffix_ids.iter().enumerate() {
        total += -lp_vec[i * v + tok as usize];
    }
    Ok(total / ts as f32)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 {
        eprintln!(
            "usage: {} <gguf> <tok> <prefix_file> <suffix_file> <summaries_json> <out_json>",
            args[0]
        );
        std::process::exit(2);
    }
    let gguf_path = &args[1];
    let tok_path = &args[2];
    let prefix_path = &args[3];
    let suffix_path = &args[4];
    let summaries_path = &args[5];
    let out_path = &args[6];

    let prefix =
        std::fs::read_to_string(prefix_path).with_context(|| format!("read {prefix_path}"))?;
    let suffix =
        std::fs::read_to_string(suffix_path).with_context(|| format!("read {suffix_path}"))?;
    let summaries_raw = std::fs::read_to_string(summaries_path)
        .with_context(|| format!("read {summaries_path}"))?;
    let summaries: Vec<NodeSummary> =
        serde_json::from_str(&summaries_raw).context("parse summaries json")?;
    eprintln!(
        "[ppl] {} node summaries, prefix {} chars, suffix {} chars",
        summaries.len(),
        prefix.len(),
        suffix.len()
    );

    let tokenizer = Tokenizer::from_file(tok_path).map_err(|e| anyhow!("tokenizer: {e}"))?;

    let device = Device::Cpu;
    eprintln!("[ppl] loading {gguf_path}");
    let mut f = std::fs::File::open(gguf_path)?;
    let ct = gguf_file::Content::read(&mut f)?;
    let mut model = ModelWeights::from_gguf(ct, &mut f, &device)?;

    let prefix_ids = tokenize(&tokenizer, &prefix, &device)?;
    let suffix_ids_t = tokenize(&tokenizer, &suffix, &device)?;
    let suffix_ids: Vec<u32> = suffix_ids_t.squeeze(0)?.to_vec1()?;
    let prefix_embeds = model.embed(&prefix_ids)?; // [1, Tp, D]
    let suffix_embeds = model.embed(&suffix_ids_t)?; // [1, Ts, D]
    let prefix_t = prefix_embeds.dim(1)?;
    let suffix_t = suffix_embeds.dim(1)?;
    let d = prefix_embeds.dim(2)?;
    eprintln!("[ppl] prefix {prefix_t} tok, suffix {suffix_t} tok, dim {d}");

    // Pool real vectors from summaries.
    let mut node_vecs: Vec<Tensor> = Vec::with_capacity(summaries.len());
    for s in summaries.iter() {
        if s.summary.trim().is_empty() {
            continue;
        }
        let ids = tokenize(&tokenizer, &s.summary, &device)?;
        let embed = model.embed(&ids)?;
        let pooled = embed.mean(1)?.unsqueeze(1)?; // [1, 1, D]
        node_vecs.push(pooled);
    }
    let n_vecs = node_vecs.len();
    eprintln!("[ppl] pooled {n_vecs} real vectors");

    // Mean L2 norm across real vectors — match this for the random control.
    let mut norms: Vec<f32> = Vec::with_capacity(n_vecs);
    for v in &node_vecs {
        let v_flat = v.squeeze(0)?.squeeze(0)?.to_dtype(DType::F32)?; // [D]
        let n = v_flat.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>()?;
        norms.push(n);
    }
    let mean_norm: f32 = norms.iter().sum::<f32>() / norms.len() as f32;
    eprintln!("[ppl] real vec mean L2 = {:.4}", mean_norm);

    // Random Gaussian unit-direction vectors scaled to match real mean L2.
    let mut random_vecs: Vec<Tensor> = Vec::with_capacity(n_vecs);
    for _ in 0..n_vecs {
        let r = Tensor::randn(0f32, 1f32, (1, 1, d), &device)?; // [1, 1, D]
        let r_flat = r.squeeze(0)?.squeeze(0)?; // [D]
        let r_norm = r_flat.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>()?;
        let scale = (mean_norm / r_norm.max(1e-8)) as f64;
        let scaled = (r * scale)?;
        random_vecs.push(scaled);
    }
    eprintln!("[ppl] generated {} random vectors at norm {:.4}", random_vecs.len(), mean_norm);

    // ── Condition 1: text-only ──────────────────────────────────────────────
    eprintln!("[ppl] cond 1 / 3 — text_only");
    let t0 = std::time::Instant::now();
    let pieces: Vec<&Tensor> = vec![&prefix_embeds, &suffix_embeds];
    let full = Tensor::cat(&pieces, 1)?;
    let logits = model.forward_embeds_full(&full, 0)?;
    let suffix_start = prefix_t;
    let ce_text = suffix_cross_entropy(&logits, &suffix_ids, suffix_start)?;
    eprintln!(
        "[ppl] text_only CE={:.4} PPL={:.3} ({:.1}s)",
        ce_text,
        ce_text.exp(),
        t0.elapsed().as_secs_f32()
    );
    drop(logits);
    drop(full);

    // ── Condition 2: real vectors ───────────────────────────────────────────
    eprintln!("[ppl] cond 2 / 3 — real_vectors");
    let t0 = std::time::Instant::now();
    let mut pieces: Vec<&Tensor> = Vec::with_capacity(2 + n_vecs);
    pieces.push(&prefix_embeds);
    for v in &node_vecs {
        pieces.push(v);
    }
    pieces.push(&suffix_embeds);
    let full = Tensor::cat(&pieces, 1)?;
    let logits = model.forward_embeds_full(&full, 0)?;
    let suffix_start = prefix_t + n_vecs;
    let ce_real = suffix_cross_entropy(&logits, &suffix_ids, suffix_start)?;
    eprintln!(
        "[ppl] real_vectors CE={:.4} PPL={:.3} ({:.1}s)",
        ce_real,
        ce_real.exp(),
        t0.elapsed().as_secs_f32()
    );
    drop(logits);
    drop(full);

    // ── Condition 3: random vectors ─────────────────────────────────────────
    eprintln!("[ppl] cond 3 / 3 — random_vectors");
    let t0 = std::time::Instant::now();
    let mut pieces: Vec<&Tensor> = Vec::with_capacity(2 + n_vecs);
    pieces.push(&prefix_embeds);
    for v in &random_vecs {
        pieces.push(v);
    }
    pieces.push(&suffix_embeds);
    let full = Tensor::cat(&pieces, 1)?;
    let logits = model.forward_embeds_full(&full, 0)?;
    let suffix_start = prefix_t + n_vecs;
    let ce_rand = suffix_cross_entropy(&logits, &suffix_ids, suffix_start)?;
    eprintln!(
        "[ppl] random_vectors CE={:.4} PPL={:.3} ({:.1}s)",
        ce_rand,
        ce_rand.exp(),
        t0.elapsed().as_secs_f32()
    );

    let result = serde_json::json!({
        "n_suffix_tok": suffix_t,
        "n_vecs": n_vecs,
        "mean_real_l2": mean_norm,
        "text_only":      { "mean_ce": ce_text, "ppl": ce_text.exp() },
        "real_vectors":   { "mean_ce": ce_real, "ppl": ce_real.exp() },
        "random_vectors": { "mean_ce": ce_rand, "ppl": ce_rand.exp() },
    });
    let mut out_file = std::fs::File::create(out_path)?;
    out_file.write_all(serde_json::to_string_pretty(&result)?.as_bytes())?;
    out_file.write_all(b"\n")?;
    eprintln!("[ppl] wrote {out_path}");
    Ok(())
}
