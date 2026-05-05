//! Run a prompt from a file through the forked Qwen, save output to a file.
//! Greedy decode, CPU, stops on Qwen chat-template EOS tokens.
//!
//! cargo run --release --example run_prompt -- <gguf> <tok> <prompt_file> <out_file>

use anyhow::{Context, Result, anyhow};
use candle_core::{Device, Tensor};
use candle_core::quantized::gguf_file;
use repo_graph_latent::qwen2_hacked::ModelWeights;
use std::io::Write as _;
use tokenizers::Tokenizer;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: {} <gguf> <tok> <prompt_file> <out_file>", args[0]);
        std::process::exit(2);
    }
    let gguf_path = &args[1];
    let tok_path = &args[2];
    let prompt_path = &args[3];
    let out_path = &args[4];
    let max_new: usize = std::env::var("MAX_NEW").ok().and_then(|s| s.parse().ok()).unwrap_or(512);

    let prompt = std::fs::read_to_string(prompt_path).with_context(|| format!("read {prompt_path}"))?;
    let tokenizer = Tokenizer::from_file(tok_path).map_err(|e| anyhow!("tokenizer: {e}"))?;

    eprintln!("[run] loading {gguf_path}");
    let device = Device::Cpu;
    let mut f = std::fs::File::open(gguf_path)?;
    let ct = gguf_file::Content::read(&mut f)?;
    let mut model = ModelWeights::from_gguf(ct, &mut f, &device)?;

    let enc = tokenizer.encode(prompt.as_str(), true).map_err(|e| anyhow!("encode: {e}"))?;
    let ids: Vec<u32> = enc.get_ids().to_vec();
    eprintln!("[run] prompt tokens: {}", ids.len());

    let mut out_file = std::fs::File::create(out_path)?;
    let mut index_pos: usize = 0;
    let mut input = Tensor::new(ids.as_slice(), &device)?.unsqueeze(0)?;

    let t0 = std::time::Instant::now();
    let mut generated = 0usize;
    let mut collected_ids: Vec<u32> = Vec::new();

    for step in 0..max_new {
        let logits = model.forward(&input, index_pos)?;
        let logits = logits.squeeze(0)?;
        let next_id: u32 = logits.argmax(candle_core::D::Minus1)?.to_scalar::<u32>()?;

        index_pos += input.dim(1)?;
        collected_ids.push(next_id);
        generated += 1;

        // EOS for Qwen2.5-Coder-Instruct: <|im_end|>=151645, <|endoftext|>=151643
        if next_id == 151645 || next_id == 151643 {
            eprintln!("[run] eos at step {step}");
            break;
        }
        input = Tensor::new(&[next_id], &device)?.unsqueeze(0)?;

        if generated % 16 == 0 {
            let elapsed = t0.elapsed().as_secs_f32();
            eprintln!("[run] {generated} tok @ {:.2} tok/s", generated as f32 / elapsed);
        }
    }

    let text = tokenizer.decode(&collected_ids, false).map_err(|e| anyhow!("decode: {e}"))?;
    out_file.write_all(text.as_bytes())?;
    let elapsed = t0.elapsed().as_secs_f32();
    eprintln!("[run] done: {generated} tokens in {elapsed:.1}s ({:.2} tok/s) → {out_path}", generated as f32 / elapsed);
    Ok(())
}
