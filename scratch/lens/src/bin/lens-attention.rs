//! lens-attention — D2, cycle 0.6 spitball bucket.
//!
//! Attention-bloat-ratio metric. Runs forward_generate with attention
//! capture enabled; emits per-(layer, token-position) attention output
//! L2 norms to JSONL. The downstream analyzer computes the bloat ratio:
//!
//!   bloat = (sum of attention magnitudes at non-pool prompt positions) /
//!           (sum of attention magnitudes at pool prompt positions)
//!
//! High bloat = the model attended diffusely (pool injected but not
//! attended); low bloat = the pool tokens received concentrated attention.
//!
//! Output JSONL includes a header (kind=Header) + one record per (layer,
//! position) for both the BASELINE and GENERATE passes. Tagged with
//! generated_token_count + n_prompt_tokens + tensor_names_seen (diagnostic
//! for which cb_eval names actually fired in Qwen2's graph).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;

use glia_lens::jsonl::LensJsonlWriter;
use glia_lens::runtime::{FakeRuntime, LensRuntime};

#[cfg(feature = "real")]
use glia_lens::runtime::LlamaCppRuntime;

#[derive(Parser, Debug)]
#[command(name = "lens-attention", version,
          about = "Per-layer attention output norms (D2 attention-bloat-ratio).")]
struct Args {
    #[arg(long)]
    weights: PathBuf,

    #[arg(long)]
    tokenizer: PathBuf,

    #[arg(long)]
    prefix: PathBuf,

    #[arg(long)]
    suffix: PathBuf,

    /// Optional graph-derived directive prepended to suffix.
    #[arg(long)]
    directive: Option<PathBuf>,

    /// Max tokens to generate. Default 16 — keeps wall-clock manageable.
    #[arg(long, default_value_t = 16)]
    max_new: u32,

    /// Output JSONL path.
    #[arg(long)]
    out: PathBuf,

    #[arg(long, default_value_t = false)]
    fake: bool,

    #[arg(long, default_value = "lens-attention")]
    label: String,
}

#[derive(Debug, Serialize)]
struct AttentionRecord {
    layer: u32,
    position_idx: u32,
    norm: f32,
    run: String,
}

fn run<R: LensRuntime>(args: &Args) -> Result<()> {
    let mut rt = R::load(&args.weights, &args.tokenizer)?;

    let prefix = std::fs::read_to_string(&args.prefix)?;
    let suffix_raw = std::fs::read_to_string(&args.suffix)?;
    let suffix = if let Some(dir_path) = &args.directive {
        let dir_text = std::fs::read_to_string(dir_path)?;
        format!("{dir_text}{suffix_raw}")
    } else {
        suffix_raw
    };

    let prompt = format!("{prefix}{suffix}");
    let prompt_tokens = rt.tokenize(&prompt)?;
    let n_prompt = prompt_tokens.len() as u32;
    tracing::info!("prompt tokens: {n_prompt}");

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut writer = LensJsonlWriter::create(&args.out)?;

    // Enable attention capture; run forward_generate.
    rt.set_capture_attention(true);
    let gen_caps = rt.forward_generate(&prompt_tokens, args.max_new, None, "generate")?;
    let attn = rt.attention_norms();
    rt.set_capture_attention(false);

    tracing::info!(
        "generated {} tokens; attn tensor names seen: {:?}",
        gen_caps.generated_tokens.len(),
        attn.tensor_names_seen
    );
    tracing::info!("attn captures: {} layer entries", attn.norms_per_layer.len());

    let header = serde_json::json!({
        "kind": "Header",
        "label": args.label,
        "n_prompt_tokens": n_prompt,
        "max_new": args.max_new,
        "generated_token_count": gen_caps.generated_tokens.len() as u32,
        "attn_tensor_names_seen": attn.tensor_names_seen.iter().collect::<Vec<_>>(),
    });
    writer.record_raw(&header)?;

    // Emit one record per (layer, position_idx) for the generate run.
    // position_idx = absolute row index in the captured buffer. The first
    // n_prompt rows correspond to the prompt prefill ubatches; subsequent
    // rows correspond to each generation step's single-token decode.
    let mut layers: Vec<u32> = attn.norms_per_layer.keys().copied().collect();
    layers.sort();
    for l in &layers {
        let buf = attn.norms_per_layer.get(l).unwrap();
        for (i, &norm) in buf.iter().enumerate() {
            let rec = AttentionRecord {
                layer: *l,
                position_idx: i as u32,
                norm,
                run: "generate".into(),
            };
            writer.record_raw(&serde_json::to_value(&rec)?)?;
        }
    }

    writer.finish()?;
    tracing::info!("DONE: wrote {} attention records to {}",
                   attn.norms_per_layer.values().map(|v| v.len()).sum::<usize>(),
                   args.out.display());
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    if args.fake {
        run::<FakeRuntime>(&args)?;
    } else {
        #[cfg(feature = "real")]
        {
            run::<LlamaCppRuntime>(&args)?;
        }
        #[cfg(not(feature = "real"))]
        anyhow::bail!("real runtime not compiled in; rebuild with --features real, or pass --fake");
    }
    Ok(())
}
