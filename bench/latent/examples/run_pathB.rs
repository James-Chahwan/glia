//! Path B — latent injection. Mean-pool each graph node's summary to one
//! 4096-dim vector and splice that vector-sequence between a ChatML prefix
//! and suffix, then generate greedily.
//!
//! Prompt layout (conceptual):
//!   <prefix text>                       ← tokenized + embed()
//!   <pooled vec for node 1>             ← embed(summary).mean(1)
//!   <pooled vec for node 2>
//!   ... one vec per activated node ...
//!   <suffix text including issue body>  ← tokenized + embed()
//!   <assistant start>                   ← already in suffix
//!
//! Everything gets concat'd along the T axis, fed through forward_embeds
//! for prefill (index_pos = 0), then greedy-decoded with forward(new_id, pos).
//!
//! cargo run --release --example run_pathB -- <gguf> <tok> <prefix_file> <suffix_file> <summaries_json> <out_file>
//!
//! env:
//!   MAX_NEW  — max new tokens (default 400)
//!   DRY_RUN  — "1" to print shapes/counts and exit before prefill

use anyhow::{Context, Result, anyhow};
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use repo_graph_latent::ForwardEmbeds;
use repo_graph_latent::qwen2_hacked::ModelWeights;
use serde::Deserialize;
use std::io::Write as _;
use tokenizers::Tokenizer;

#[derive(Debug, Deserialize)]
struct NodeSummary {
    #[allow(dead_code)]
    id: u64,
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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 {
        eprintln!(
            "usage: {} <gguf> <tok> <prefix_file> <suffix_file> <summaries_json> <out_file>",
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
    let max_new: usize = std::env::var("MAX_NEW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let dry_run = std::env::var("DRY_RUN").ok().as_deref() == Some("1");

    let prefix = std::fs::read_to_string(prefix_path).with_context(|| format!("read {prefix_path}"))?;
    let suffix = std::fs::read_to_string(suffix_path).with_context(|| format!("read {suffix_path}"))?;
    let summaries_raw = std::fs::read_to_string(summaries_path)
        .with_context(|| format!("read {summaries_path}"))?;
    let summaries: Vec<NodeSummary> =
        serde_json::from_str(&summaries_raw).context("parse summaries json")?;
    eprintln!(
        "[pathB] {} node summaries, prefix {} chars, suffix {} chars",
        summaries.len(),
        prefix.len(),
        suffix.len()
    );

    let tokenizer = Tokenizer::from_file(tok_path).map_err(|e| anyhow!("tokenizer: {e}"))?;

    eprintln!("[pathB] loading {gguf_path}");
    let device = Device::Cpu;
    let mut f = std::fs::File::open(gguf_path)?;
    let ct = gguf_file::Content::read(&mut f)?;
    let mut model = ModelWeights::from_gguf(ct, &mut f, &device)?;

    // Tokenize + embed the prefix and suffix blocks.
    let prefix_ids = tokenize(&tokenizer, &prefix, &device)?;
    let suffix_ids = tokenize(&tokenizer, &suffix, &device)?;
    let prefix_embeds = model.embed(&prefix_ids)?; // [1, Tp, D]
    let suffix_embeds = model.embed(&suffix_ids)?; // [1, Ts, D]
    let prefix_t = prefix_embeds.dim(1)?;
    let suffix_t = suffix_embeds.dim(1)?;
    eprintln!("[pathB] prefix {prefix_t} tok, suffix {suffix_t} tok");

    // Per-node: tokenize summary → embed → mean-pool axis T → unsqueeze to [1, 1, D].
    let mut node_vecs: Vec<Tensor> = Vec::with_capacity(summaries.len());
    for (i, s) in summaries.iter().enumerate() {
        if s.summary.trim().is_empty() {
            eprintln!("[pathB] skip empty summary for {}", s.qname);
            continue;
        }
        let ids = tokenize(&tokenizer, &s.summary, &device)?;
        let embed = model.embed(&ids)?; // [1, t, D]
        let pooled = embed.mean(1)?.unsqueeze(1)?; // [1, 1, D]
        if i < 3 {
            eprintln!("[pathB] node[{i}] {} ({} tok → 1 pooled vec)", s.qname, ids.dim(1)?);
        }
        node_vecs.push(pooled);
    }
    eprintln!("[pathB] pooled {} node vectors", node_vecs.len());

    // Build the full embed sequence.
    let mut pieces: Vec<&Tensor> = Vec::with_capacity(2 + node_vecs.len() + 1);
    pieces.push(&prefix_embeds);
    for v in &node_vecs {
        pieces.push(v);
    }
    pieces.push(&suffix_embeds);
    let full_embeds = Tensor::cat(&pieces, 1)?; // [1, T_total, D]
    let t_total = full_embeds.dim(1)?;
    let d = full_embeds.dim(2)?;
    eprintln!(
        "[pathB] full embed: [{}, {}, {}] (text {} + nodes {} + text {} = {})",
        full_embeds.dim(0)?,
        t_total,
        d,
        prefix_t,
        node_vecs.len(),
        suffix_t,
        prefix_t + node_vecs.len() + suffix_t
    );

    if dry_run {
        eprintln!("[pathB] DRY_RUN=1 — stopping before prefill");
        return Ok(());
    }

    let mut out_file = std::fs::File::create(out_path)?;
    let t0 = std::time::Instant::now();

    // Prefill over the concatenated embed sequence.
    let logits = model.forward_embeds(&full_embeds, 0)?;
    let mut index_pos = t_total;

    // First sampled token from the prefill logits.
    let logits_s = logits.squeeze(0)?;
    let mut next_id: u32 = logits_s.argmax(candle_core::D::Minus1)?.to_scalar::<u32>()?;
    let mut collected_ids: Vec<u32> = vec![next_id];
    let mut generated = 1usize;
    eprintln!("[pathB] prefill done in {:.1}s ({} tokens)", t0.elapsed().as_secs_f32(), t_total);

    // Greedy loop — regular forward() after prefill.
    for step in 1..max_new {
        if next_id == 151645 || next_id == 151643 {
            eprintln!("[pathB] eos at step {step}");
            break;
        }
        let input = Tensor::new(&[next_id], &device)?.unsqueeze(0)?;
        let logits = model.forward(&input, index_pos)?;
        let logits = logits.squeeze(0)?;
        next_id = logits.argmax(candle_core::D::Minus1)?.to_scalar::<u32>()?;
        index_pos += 1;
        collected_ids.push(next_id);
        generated += 1;

        if generated % 16 == 0 {
            let elapsed = t0.elapsed().as_secs_f32();
            eprintln!("[pathB] {generated} tok @ {:.2} tok/s", generated as f32 / elapsed);
        }
    }

    let text = tokenizer
        .decode(&collected_ids, false)
        .map_err(|e| anyhow!("decode: {e}"))?;
    out_file.write_all(text.as_bytes())?;
    let elapsed = t0.elapsed().as_secs_f32();
    eprintln!(
        "[pathB] done: {generated} tokens in {elapsed:.1}s ({:.2} tok/s) → {out_path}",
        generated as f32 / elapsed
    );
    Ok(())
}
