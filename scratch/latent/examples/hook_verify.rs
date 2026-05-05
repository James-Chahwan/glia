//! Hook equivalence test.
//!
//! The claim under test: for a fresh model, `forward(tokens, 0)` and
//! `forward_embeds(embed(tokens), 0)` must produce bitwise-close logits —
//! `forward_embeds` only skips the `tok_embeddings.forward` step; the rest of
//! the stack (RoPE, attention, MLP, norm, output head) runs identically.
//!
//! A mismatch means the fork diverged somewhere during the `forward_embeds`
//! rewrite. A match does NOT validate the overall model (the RoPE bug, if
//! present, biases both paths identically).
//!
//! cargo run --example hook_verify --release -- <gguf> <tokenizer.json>

use anyhow::{Context, Result, anyhow};
use candle_core::{Device, Tensor};
use candle_core::quantized::gguf_file;
use repo_graph_latent::{ForwardEmbeds, qwen2_hacked::ModelWeights};
use tokenizers::Tokenizer;

fn load(path: &str, device: &Device) -> Result<ModelWeights> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {path}"))?;
    let ct = gguf_file::Content::read(&mut f).context("gguf content")?;
    ModelWeights::from_gguf(ct, &mut f, device).context("from_gguf")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <gguf> <tokenizer.json>", args[0]);
        std::process::exit(2);
    }
    let gguf_path = &args[1];
    let tok_path = &args[2];

    let device = Device::Cpu;

    let tokenizer = Tokenizer::from_file(tok_path).map_err(|e| anyhow!("tokenizer: {e}"))?;
    let prompt = "fn add(a: i32, b: i32) -> i32 {";
    let enc = tokenizer.encode(prompt, true).map_err(|e| anyhow!("encode: {e}"))?;
    let ids: Vec<u32> = enc.get_ids().to_vec();
    eprintln!("[verify] tokens: {} — {:?}", ids.len(), ids);

    let tokens = Tensor::new(ids.as_slice(), &device)?.unsqueeze(0)?;

    eprintln!("[verify] path A: forward(tokens)");
    let mut model_a = load(gguf_path, &device)?;
    let logits_a = model_a.forward(&tokens, 0)?;

    eprintln!("[verify] path B: forward_embeds(embed(tokens))");
    let mut model_b = load(gguf_path, &device)?;
    let embeds = model_b.embed(&tokens)?;
    let logits_b = model_b.forward_embeds(&embeds, 0)?;

    let shape_a = logits_a.dims().to_vec();
    let shape_b = logits_b.dims().to_vec();
    eprintln!("[verify] shapes: A={shape_a:?} B={shape_b:?}");
    if shape_a != shape_b {
        return Err(anyhow!("shape mismatch: A={shape_a:?} B={shape_b:?}"));
    }

    let diff = (&logits_a - &logits_b)?.abs()?;
    let max_abs: f32 = diff.max(candle_core::D::Minus1)?.max(candle_core::D::Minus1)?.to_scalar::<f32>().or_else(|_| {
        // dequantised logits may be f16/bf16 — convert
        diff.to_dtype(candle_core::DType::F32)?.max(candle_core::D::Minus1)?.max(candle_core::D::Minus1)?.to_scalar::<f32>()
    })?;
    let mean_abs: f32 = {
        let d = diff.to_dtype(candle_core::DType::F32)?;
        let n = d.elem_count() as f64;
        d.sum_all()?.to_scalar::<f32>()? / (n as f32)
    };

    eprintln!("[verify] max |A-B| = {max_abs:.6e}");
    eprintln!("[verify] mean |A-B| = {mean_abs:.6e}");

    // Also check argmax agrees — this is the sampling-relevant test.
    let arg_a: u32 = logits_a.squeeze(0)?.argmax(candle_core::D::Minus1)?.to_scalar::<u32>()?;
    let arg_b: u32 = logits_b.squeeze(0)?.argmax(candle_core::D::Minus1)?.to_scalar::<u32>()?;
    eprintln!("[verify] argmax A = {arg_a}, argmax B = {arg_b}");

    if arg_a != arg_b {
        return Err(anyhow!("argmax disagreement — hook is not equivalent"));
    }
    if max_abs > 1e-2 {
        eprintln!("[verify] WARN: max |A-B| = {max_abs:.6e} > 1e-2 — unexpected given identical compute path");
    }
    eprintln!("[verify] PASS");
    Ok(())
}
