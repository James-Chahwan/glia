//! Smoke test: load Qwen GGUF via forked quantized_qwen2, generate a short completion.
//! Runs on CPU by default. Pass `--release` for usable speed on 1.5B.
//!
//! cargo run --example smoke --release -- \
//!   ~/Models/qwen2.5-coder-1.5b-gguf/qwen2.5-coder-1.5b-instruct-q4_k_m.gguf \
//!   ~/Models/qwen2.5-coder-tokenizer/tokenizer.json \
//!   "fn fibonacci(n: u32) -> u32 {"

use anyhow::{Context, Result, anyhow};
use candle_core::{Device, Tensor};
use candle_core::quantized::gguf_file;
use repo_graph_latent::qwen2_hacked::ModelWeights;
use std::io::Write as _;
use tokenizers::Tokenizer;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: {} <gguf> <tokenizer.json> <prompt>", args[0]);
        std::process::exit(2);
    }
    let gguf_path = &args[1];
    let tok_path = &args[2];
    let prompt = &args[3];
    let max_new_tokens: usize = std::env::var("MAX_NEW").ok().and_then(|s| s.parse().ok()).unwrap_or(48);

    let device = Device::Cpu;

    let tokenizer = Tokenizer::from_file(tok_path).map_err(|e| anyhow!("tokenizer: {e}"))?;

    eprintln!("[smoke] loading {gguf_path}");
    let mut file = std::fs::File::open(gguf_path).with_context(|| format!("open {gguf_path}"))?;
    let content = gguf_file::Content::read(&mut file).context("gguf content")?;
    let mut model = ModelWeights::from_gguf(content, &mut file, &device).context("from_gguf")?;

    let enc = tokenizer.encode(prompt.as_str(), true).map_err(|e| anyhow!("encode: {e}"))?;
    let ids: Vec<u32> = enc.get_ids().to_vec();
    eprintln!("[smoke] prompt tokens: {}", ids.len());

    print!("{prompt}");
    std::io::stdout().flush().ok();

    let mut index_pos: usize = 0;
    let mut input = Tensor::new(ids.as_slice(), &device)?.unsqueeze(0)?;

    for step in 0..max_new_tokens {
        let logits = model.forward(&input, index_pos)?;
        let logits = logits.squeeze(0)?;
        let next_id: u32 = logits.argmax(candle_core::D::Minus1)?.to_scalar::<u32>()?;

        index_pos += input.dim(1)?;
        let piece = tokenizer.decode(&[next_id], false).map_err(|e| anyhow!("decode: {e}"))?;
        print!("{piece}");
        std::io::stdout().flush().ok();

        // EOS guards for Qwen2.5 (<|im_end|>=151645, <|endoftext|>=151643)
        if next_id == 151645 || next_id == 151643 {
            eprintln!("\n[smoke] eos at step {step}");
            break;
        }
        input = Tensor::new(&[next_id], &device)?.unsqueeze(0)?;
    }

    eprintln!("\n[smoke] done");
    Ok(())
}
