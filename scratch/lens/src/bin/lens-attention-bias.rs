//! lens-attention-bias — C7 driver for the C3 attention-output bias
//! mechanism shipped in `runtime.rs::AttentionBiasSpec`.
//!
//! Variant of lens-inject. Where lens-inject modifies `l_out-N` (residual)
//! at decision layers, lens-attention-bias modifies `attn_out-N` (post-
//! attention projection, BEFORE the FFN in the same layer). This biases
//! the attention OUTPUT in the target direction regardless of which keys
//! the attention attended to — a different lever for the same
//! L25-27 × position-23-25 decision window the cycle 0.4 lens identified.
//!
//! Both biases can be active in the same forward pass; this bin sets only
//! the attention bias (target_embed for B3 is zero-vector so the residual
//! path is a no-op).
//!
//! Output JSONL schema matches lens-inject so downstream tooling consumes
//! it identically.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use glia_lens::jsonl::LensJsonlWriter;
use glia_lens::lens::{compute_lens_steps, AutoregressiveCaptures, ForwardPassCaptures};
use glia_lens::runtime::{AttentionBiasSpec, FakeRuntime, InjectSpec, LensRuntime};

#[cfg(feature = "real")]
use glia_lens::runtime::LlamaCppRuntime;

fn autoregressive_to_forward_pass(cap: &AutoregressiveCaptures) -> ForwardPassCaptures {
    ForwardPassCaptures {
        run: cap.run.clone(),
        positions: cap.steps.clone(),
    }
}

#[derive(Parser, Debug)]
#[command(name = "lens-attention-bias", version,
          about = "C3 attention-output bias at the cycle 0.4 decision band (L25-27)")]
struct Args {
    #[arg(long)]
    weights: PathBuf,

    #[arg(long)]
    tokenizer: PathBuf,

    #[arg(long)]
    prefix: PathBuf,

    #[arg(long)]
    suffix: PathBuf,

    #[arg(long)]
    directive: Option<PathBuf>,

    /// Steering target — first token of this string is used as the bias
    /// vector. Same shape as lens-inject's --target-qname.
    #[arg(long)]
    target_qname: String,

    /// Layers (0-indexed) at whose attn_out-N to bias. Default 25,26,27.
    #[arg(long, default_value = "25,26,27")]
    bias_layers: String,

    /// Generated-token positions to bias at (0-indexed). Default 23,24,25.
    #[arg(long, default_value = "23,24,25")]
    bias_positions: String,

    /// Mix strength. Smaller than lens-inject because attn_out is added
    /// straight into residual; start at 0.1.
    #[arg(long, default_value_t = 0.1)]
    alpha: f32,

    #[arg(long, default_value_t = 64)]
    max_new: u32,

    #[arg(long, default_value_t = 5)]
    top_k: usize,

    #[arg(long)]
    out: PathBuf,

    #[arg(long, default_value_t = false)]
    fake: bool,

    #[arg(long, default_value = "lens-attention-bias")]
    label: String,
}

fn parse_u32_csv(spec: &str) -> Result<Vec<u32>> {
    spec.split(',')
        .map(|s| s.trim().parse::<u32>().context(format!("parse {s:?}")))
        .collect()
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
    let n_embd = rt.n_embd();

    let target_tokens = rt.tokenize(&args.target_qname)?;
    if target_tokens.is_empty() {
        anyhow::bail!("target_qname {:?} tokenizes to empty", args.target_qname);
    }
    let target_token_id = target_tokens[0];
    let target_embed = read_token_embed(&mut rt, target_token_id, n_embd)?;
    tracing::info!(
        "target: qname={:?} first_token_id={} embed_norm={:.3}",
        args.target_qname, target_token_id,
        target_embed.iter().map(|x| x * x).sum::<f32>().sqrt()
    );

    let attn_spec = AttentionBiasSpec {
        target_embed: target_embed.clone(),
        bias_layers: parse_u32_csv(&args.bias_layers)?,
        bias_positions: parse_u32_csv(&args.bias_positions)?,
        alpha: args.alpha,
    };
    tracing::info!(
        "attn-bias spec: layers={:?} positions={:?} alpha={}",
        attn_spec.bias_layers, attn_spec.bias_positions, attn_spec.alpha
    );

    // No-op InjectSpec — B3 residual path stays inactive; only C3 attn-bias
    // fires. forward_generate_with_inject is reused so inject_active_position
    // gets set per step (C3 trampoline reads it).
    let zero_inject = InjectSpec {
        target_embed: vec![0.0; n_embd],
        inject_layers: vec![],
        inject_positions: vec![],
        alpha: 0.0,
    };

    // Baseline (no bias).
    rt.set_attn_bias(None);
    let baseline = rt.forward_generate(&prompt_tokens, args.max_new, None, "baseline")?;
    tracing::info!("baseline: {} tokens (eos={})",
                   baseline.generated_tokens.len(), baseline.stopped_on_eos);

    // Biased run.
    rt.set_attn_bias(Some(attn_spec.clone()));
    let biased = rt.forward_generate_with_inject(
        &prompt_tokens, args.max_new, None, "attn-biased", &zero_inject,
    )?;
    rt.set_attn_bias(None);
    tracing::info!("biased: {} tokens (eos={})",
                   biased.generated_tokens.len(), biased.stopped_on_eos);

    // Hamming diff.
    let max_len = baseline.generated_tokens.len().max(biased.generated_tokens.len());
    let mut hamming = 0u32;
    let mut first_div: Option<u32> = None;
    for step in 0..max_len {
        let b = baseline.generated_tokens.get(step).copied();
        let a = biased.generated_tokens.get(step).copied();
        if a != b {
            hamming += 1;
            if first_div.is_none() {
                first_div = Some(step as u32);
            }
        }
    }
    tracing::info!("diff: hamming={} first_divergence={:?} of {} steps",
                   hamming, first_div, max_len);

    // JSONL.
    let baseline_fpc = autoregressive_to_forward_pass(&baseline);
    let biased_fpc = autoregressive_to_forward_pass(&biased);
    let head = rt.unembed_head().clone();

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut writer = LensJsonlWriter::create(&args.out)?;

    let token_str = |id: u32| format!("tok{id}");

    let (baseline_steps, baseline_probs) = compute_lens_steps(
        &baseline_fpc, &head, args.top_k, None, None, &token_str,
    );
    for step in baseline_steps {
        writer.record(step)?;
    }
    let (biased_steps, _) = compute_lens_steps(
        &biased_fpc, &head, args.top_k, None, Some(&baseline_probs), &token_str,
    );
    for step in biased_steps {
        writer.record(step)?;
    }

    // Detokenize generated sequences so callers (e.g. run_instance.py's
    // attn-injection beam channel) get the actual diff text, not token ids.
    let baseline_output: String = baseline.generated_tokens.iter()
        .map(|&t| rt.detokenize(t))
        .collect();
    let injected_output: String = biased.generated_tokens.iter()
        .map(|&t| rt.detokenize(t))
        .collect();

    let summary = serde_json::json!({
        "kind": "AttentionBiasSummary",
        "data": {
            "label": args.label,
            "target_qname": args.target_qname,
            "target_token_id": target_token_id,
            "bias_layers": attn_spec.bias_layers,
            "bias_positions": attn_spec.bias_positions,
            "alpha": attn_spec.alpha,
            "baseline_tokens": baseline.generated_tokens,
            "biased_tokens": biased.generated_tokens,
            "baseline_output": baseline_output,
            "injected_output": injected_output,
            "hamming": hamming,
            "first_divergence_step": first_div,
        }
    });
    writer.record_raw(&summary)?;
    writer.finish()?;
    tracing::info!("DONE: wrote {}", args.out.display());
    Ok(())
}

fn read_token_embed<R: LensRuntime>(rt: &mut R, tid: u32, n_embd: usize) -> Result<Vec<f32>> {
    let _ = rt.forward_capture(&[tid], &[0], "embed-probe");
    let head = rt.unembed_head();
    if head.output_weight.is_empty() {
        anyhow::bail!("unembed weights not captured — re-run after a real forward pass");
    }
    let row_start = (tid as usize) * n_embd;
    let row_end = row_start + n_embd;
    if row_end > head.output_weight.len() {
        anyhow::bail!("token_id {} out of range (n_embd={})", tid, n_embd);
    }
    Ok(head.output_weight[row_start..row_end].to_vec())
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
