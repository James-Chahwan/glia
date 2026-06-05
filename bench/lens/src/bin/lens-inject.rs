//! lens-inject — B3, cycle 0.6 spitball bucket.
//!
//! Surgical lever the cycle 0.4 autoregressive lens identified: modify the
//! residual stream at L25-27 × generated-token positions 23-25 to steer the
//! model toward a target token (the first token of a graph-derived qname).
//!
//! Uses `LensRuntime::forward_generate_with_inject` — FakeRuntime adds the
//! target_embed * alpha to the synthetic residuals (testable end-to-end);
//! LlamaCppRuntime modifies the real l_out-N tensor in cb_eval via
//! `ggml_backend_tensor_set` before subsequent layers read from it.
//!
//! The target embed is computed from the model's own token-embedding table:
//! tokenize `--target-qname` (e.g. `_bind_to_schema`) → take the FIRST token's
//! embedding row as the steering vector. Length must equal n_embd.
//!
//! Output: same JSONL schema as `lens --mode generate` so downstream lens-
//! analyze / lens-batch consume it identically. Tagged with run="injected".

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use glia_lens::jsonl::LensJsonlWriter;
use glia_lens::lens::{compute_lens_steps, AutoregressiveCaptures, ForwardPassCaptures};
use glia_lens::runtime::{FakeRuntime, InjectSpec, LensRuntime};

#[cfg(feature = "real")]
use glia_lens::runtime::LlamaCppRuntime;

fn autoregressive_to_forward_pass(cap: &AutoregressiveCaptures) -> ForwardPassCaptures {
    ForwardPassCaptures {
        run: cap.run.clone(),
        positions: cap.steps.clone(),
    }
}

#[derive(Parser, Debug)]
#[command(name = "lens-inject", version,
          about = "K/V residual injection at the cycle 0.4 decision band (L25-27)")]
struct Args {
    #[arg(long)]
    weights: PathBuf,

    #[arg(long)]
    tokenizer: PathBuf,

    #[arg(long)]
    prefix: PathBuf,

    #[arg(long)]
    suffix: PathBuf,

    /// Optional graph-derived directive prepended to suffix (same semantics
    /// as `lens --directive`).
    #[arg(long)]
    directive: Option<PathBuf>,

    /// Steering target — the first token of this string is encoded and used
    /// as the residual offset. Typical values: `_bind_to_schema`,
    /// `file_permissions_mode`, `format_cursor_data` — qname tails
    /// surfaced by the synth bins.
    #[arg(long)]
    target_qname: String,

    /// Layer indices (0-indexed) to inject at. Default 25,26,27 per cycle
    /// 0.4 lens evidence. Comma-separated.
    #[arg(long, default_value = "25,26,27")]
    inject_layers: String,

    /// Generated-token positions (0-indexed) to inject at. Default 23,24,25
    /// per cycle 0.4 marshmallow divergence window. Comma-separated.
    #[arg(long, default_value = "23,24,25")]
    inject_positions: String,

    /// Mix strength. 0.0 = no-op; experiment with 0.1/0.3/0.5/1.0.
    #[arg(long, default_value_t = 0.3)]
    alpha: f32,

    /// Max tokens to generate. Generated tokens beyond inject_positions max
    /// won't be modified but still get captured.
    #[arg(long, default_value_t = 64)]
    max_new: u32,

    /// Top-K for lens math.
    #[arg(long, default_value_t = 5)]
    top_k: usize,

    /// JSONL output path.
    #[arg(long)]
    out: PathBuf,

    /// Fake runtime — no llama.cpp. Verifies the injection plumbing end-to-end
    /// against synthetic residuals.
    #[arg(long, default_value_t = false)]
    fake: bool,

    /// Label embedded in JSONL.
    #[arg(long, default_value = "lens-inject")]
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

    // Encode target qname → first token id → embed-table row as steering vec.
    let target_tokens = rt.tokenize(&args.target_qname)?;
    if target_tokens.is_empty() {
        anyhow::bail!("target_qname {:?} tokenizes to empty", args.target_qname);
    }
    let target_token_id = target_tokens[0];
    let target_embed = read_token_embed(&mut rt, target_token_id, n_embd)?;
    tracing::info!(
        "target: qname={:?} first_token_id={} embed_norm={:.3}",
        args.target_qname,
        target_token_id,
        target_embed.iter().map(|x| x * x).sum::<f32>().sqrt()
    );

    let spec = InjectSpec {
        target_embed,
        inject_layers: parse_u32_csv(&args.inject_layers)?,
        inject_positions: parse_u32_csv(&args.inject_positions)?,
        alpha: args.alpha,
    };
    tracing::info!(
        "inject spec: layers={:?} positions={:?} alpha={}",
        spec.inject_layers,
        spec.inject_positions,
        spec.alpha
    );

    // Baseline run (no inject) for comparison.
    let baseline = rt.forward_generate(&prompt_tokens, args.max_new, None, "baseline")?;
    tracing::info!(
        "baseline: generated {} tokens (stopped_on_eos={})",
        baseline.generated_tokens.len(),
        baseline.stopped_on_eos
    );

    // Injected run.
    let injected = rt.forward_generate_with_inject(
        &prompt_tokens,
        args.max_new,
        None,
        "injected",
        &spec,
    )?;
    tracing::info!(
        "injected: generated {} tokens (stopped_on_eos={})",
        injected.generated_tokens.len(),
        injected.stopped_on_eos
    );

    // Diff baseline vs injected.
    let max_len = baseline.generated_tokens.len().max(injected.generated_tokens.len());
    let mut hamming = 0u32;
    let mut first_div: Option<u32> = None;
    for step in 0..max_len {
        let b = baseline.generated_tokens.get(step).copied();
        let a = injected.generated_tokens.get(step).copied();
        if a != b {
            hamming += 1;
            if first_div.is_none() {
                first_div = Some(step as u32);
            }
        }
    }
    tracing::info!(
        "diff: hamming={} first_divergence={:?} of {} steps",
        hamming,
        first_div,
        max_len
    );

    // Emit JSONL via existing lens math (top-K logits per layer).
    let baseline_fpc = autoregressive_to_forward_pass(&baseline);
    let injected_fpc = autoregressive_to_forward_pass(&injected);
    let head = rt.unembed_head().clone();
    if head.output_weight.is_empty() {
        tracing::warn!("unembed head not populated; lens math will be skipped");
    }

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut writer = LensJsonlWriter::create(&args.out)?;

    let token_str = |id: u32| rt_detokenize(&head, id);

    let (baseline_steps, baseline_probs) = compute_lens_steps(
        &baseline_fpc, &head, args.top_k, None, None, &token_str,
    );
    for step in baseline_steps {
        writer.record(step)?;
    }
    let (injected_steps, _injected_probs) = compute_lens_steps(
        &injected_fpc, &head, args.top_k, None, Some(&baseline_probs), &token_str,
    );
    for step in injected_steps {
        writer.record(step)?;
    }

    let summary = serde_json::json!({
        "kind": "InjectSummary",
        "data": {
            "label": args.label,
            "target_qname": args.target_qname,
            "target_token_id": target_token_id,
            "inject_layers": spec.inject_layers,
            "inject_positions": spec.inject_positions,
            "alpha": spec.alpha,
            "baseline_tokens": baseline.generated_tokens,
            "injected_tokens": injected.generated_tokens,
            "hamming": hamming,
            "first_divergence_step": first_div,
        }
    });
    writer.record_raw(&summary)?;
    writer.finish()?;
    tracing::info!("DONE: wrote {}", args.out.display());
    Ok(())
}

/// Helper: `compute_lens_steps` takes a `token_str_of: Fn(u32) -> String`
/// closure. Inside `run<R>` we can't borrow `rt` mutably AND pass an
/// `rt.detokenize`-using closure simultaneously. So we synthesize the token
/// string from the unembed head's vocab id directly — the lens steps' UI
/// labels are best-effort; full detok happens in render time.
fn rt_detokenize(_head: &glia_lens::lens::UnembedHead, id: u32) -> String {
    format!("tok{id}")
}

/// Pull token `tid`'s embedding row from the model's tok-embeddings table.
/// FakeRuntime: synthesized via UnembedHead.output_weight (the synth one-hot
/// embedding table). LlamaCppRuntime: same path — the cb_eval mechanism
/// captures `token_embd.weight` (or tied with `output.weight`) on first sight.
fn read_token_embed<R: LensRuntime>(rt: &mut R, tid: u32, n_embd: usize) -> Result<Vec<f32>> {
    // Run a tiny forward pass with the target token alone to ensure cb_eval
    // captures the weights (forward_capture populates UnembedHead).
    let _ = rt.forward_capture(&[tid], &[0], "embed-probe");
    let head = rt.unembed_head();
    if head.output_weight.is_empty() {
        anyhow::bail!(
            "unembed weights not captured — re-run after a real forward pass"
        );
    }
    let n_vocab = head.n_vocab;
    let row_start = (tid as usize) * n_embd;
    let row_end = row_start + n_embd;
    if row_end > head.output_weight.len() {
        anyhow::bail!(
            "token_id {} out of range (n_vocab={}, n_embd={})",
            tid, n_vocab, n_embd
        );
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
