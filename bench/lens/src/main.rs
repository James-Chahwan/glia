//! glia-lens — slice 1 logit lens MVP.
//!
//! Orchestration:
//!   1. Parse CLI args.
//!   2. Load runtime (real or fake).
//!   3. Tokenize prefix + suffix; baseline run.
//!   4. Tokenize prefix + injection + suffix; with-injection run.
//!   5. For each pass, compute lens steps (residual → norm → unembed →
//!      softmax → top-K + gold-rank).
//!   6. Emit JSONL + render PNG + render terminal ASCII.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use glia_lens::jsonl::LensJsonlWriter;
use glia_lens::lens::{compute_lens_steps, AutoregressiveCaptures, ForwardPassCaptures};
use glia_lens::runtime::{FakeRuntime, LensRuntime};
use glia_lens::{render_ascii, render_png};

/// Coerce an `AutoregressiveCaptures` (cycle 0.4 generate mode) into a
/// `ForwardPassCaptures` so the downstream lens-math + JSONL + renderer
/// pipeline doesn't need to know about the mode. `position` in the resulting
/// `LensStep` then means "generated-token index" instead of "prompt offset
/// from end" — interpretation lives with the renderer / reader.
fn autoregressive_to_forward_pass(cap: &AutoregressiveCaptures) -> ForwardPassCaptures {
    ForwardPassCaptures {
        run: cap.run.clone(),
        positions: cap.steps.clone(),
    }
}

#[cfg(feature = "real")]
use glia_lens::runtime::LlamaCppRuntime;

/// Parsed `--output-positions` argument: a finite list of u32 positions plus
/// the source spec string for diagnostics. Resolved against the actual prompt
/// length at runtime (out-of-range positions silently dropped).
#[derive(Debug, Clone)]
struct PositionSpec {
    raw: String,
    items: Vec<u32>,
}

fn parse_positions_spec(s: &str) -> Result<PositionSpec, String> {
    let raw = s.to_string();
    let trimmed = s.trim();
    // Single int.
    if let Ok(n) = trimmed.parse::<u32>() {
        return Ok(PositionSpec { raw, items: vec![n] });
    }
    // Range a..b (exclusive) or a..=b (inclusive).
    if let Some(idx) = trimmed.find("..") {
        let (lhs, rhs) = trimmed.split_at(idx);
        let (rhs_start, inclusive) = if let Some(r) = rhs.strip_prefix("..=") {
            (r, true)
        } else if let Some(r) = rhs.strip_prefix("..") {
            (r, false)
        } else {
            return Err(format!("malformed range: {s}"));
        };
        let lo: u32 = lhs.parse().map_err(|e| format!("bad range lo {lhs:?}: {e}"))?;
        let hi: u32 = rhs_start
            .parse()
            .map_err(|e| format!("bad range hi {rhs_start:?}: {e}"))?;
        let items: Vec<u32> = if inclusive {
            (lo..=hi).collect()
        } else {
            (lo..hi).collect()
        };
        if items.is_empty() {
            return Err(format!("empty range: {s}"));
        }
        return Ok(PositionSpec { raw, items });
    }
    // Comma list.
    let mut items: Vec<u32> = Vec::new();
    for part in trimmed.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        items.push(p.parse().map_err(|e| format!("bad list entry {p:?}: {e}"))?);
    }
    if items.is_empty() {
        return Err(format!("could not parse positions spec: {s:?}"));
    }
    Ok(PositionSpec { raw, items })
}

/// Slice 1 logit-lens MVP. Run a Qwen forward pass twice (baseline + with
/// injection), capture per-layer residual streams, project to vocab logits,
/// emit JSONL + 2D PNG + terminal ASCII summary.
#[derive(Parser, Debug)]
#[command(name = "glia-lens", version)]
struct Args {
    /// Path to the GGUF weights file.
    #[arg(long)]
    weights: PathBuf,

    /// Path to the HuggingFace tokenizer.json.
    #[arg(long)]
    tokenizer: PathBuf,

    /// File whose contents become the prefix of the prompt (chat header +
    /// system + user up to the injectable middle).
    #[arg(long)]
    prefix: PathBuf,

    /// File whose contents become the suffix (closing prompt).
    #[arg(long)]
    suffix: PathBuf,

    /// JSON file with the injection content. When present, a second pass
    /// runs with `prefix + serialized(injection) + suffix`. When absent,
    /// only the baseline pass runs.
    #[arg(long)]
    injection: Option<PathBuf>,

    /// Optional graph-derived DIRECTIVE markdown file. When set, the file's
    /// raw text is prepended to the suffix (mirrors run_instance.py's
    /// directive-prepend pattern at run_instance.py:750-761). Use this to
    /// make lens runs real-pipeline-comparable. Compatible with --injection:
    /// the JSON pool goes between prefix and suffix, the directive sits at
    /// the front of the suffix (real-pipeline layout).
    #[arg(long)]
    directive: Option<PathBuf>,

    /// Positions in the output stream to track. Each position p means
    /// "predict the next token after the p-th-from-LAST prompt token"; so
    /// p=0 is the first generated token (predicted by the last prompt token),
    /// p=1 is what the second-to-last prompt token predicts, etc.
    ///
    /// Accepts:
    ///   - single int: `--output-positions 0`
    ///   - comma list: `--output-positions 0,2,5,10`
    ///   - inclusive-end range with `..=`: `--output-positions 0..=15`
    ///   - exclusive-end range with `..`:  `--output-positions 0..16`
    ///
    /// Default: 0..min(8, seq_len).
    #[arg(long, value_parser = parse_positions_spec, default_value = "0..8")]
    output_positions: PositionSpec,

    /// Deprecated single-position alias, kept for backward-compat with slice-1
    /// invocations. If set, overrides `--output-positions`.
    #[arg(long)]
    output_position: Option<u32>,

    /// Lens mode. Default is "prompt-position": inspect residuals at the
    /// specified prompt positions (slice 1.5). "generate" mode does
    /// autoregressive greedy generation and captures per-generated-token
    /// residuals (cycle 0.4 — required to see the cycle 0.3 directive's
    /// actual steering effect during decoding).
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["prompt-position", "generate"]), default_value = "prompt-position")]
    mode: String,

    /// In `generate` mode: max tokens to generate before stopping. Stops
    /// earlier on EOS. Default 64 — keeps wall-clock manageable; bump for
    /// long-diff inspection. Ignored in prompt-position mode.
    #[arg(long, default_value_t = 64)]
    max_new: u32,

    /// Top-K predictions per layer.
    #[arg(long, default_value_t = 5)]
    top_k: usize,

    /// JSONL output path. Schema mirrors neuropil's `RecordedFlow`.
    #[arg(long)]
    out: PathBuf,

    /// Optional PNG output path. Two side-by-side panels.
    #[arg(long)]
    render_png: Option<PathBuf>,

    /// When set, prints the terminal ASCII summary to stdout.
    #[arg(long, default_value_t = false)]
    render_ascii: bool,

    /// Optional gold token id from the SWE-bench gold patch. When set, the
    /// lens tracks rank/prob of this token at every layer. Pre-tokenize on
    /// the consumer side; we don't read parquet here.
    #[arg(long)]
    gold_token_id: Option<u32>,

    /// Label embedded in the JSONL header + PNG title. Defaults to the
    /// prefix file's stem.
    #[arg(long)]
    label: Option<String>,

    /// Use the synthetic fake runtime instead of llama.cpp. Lets the rest of
    /// the pipeline be exercised without a 10-min llama-cpp-sys-2 compile.
    #[arg(long, default_value_t = false)]
    fake: bool,

    /// Cap the injection to the top-N entries (by score, descending) before
    /// splicing between prefix and suffix. Default 30 — keeps total token
    /// count under ~16k for typical SWE-bench fixtures so they fit a 32k ctx
    /// with comfortable KV-cache margin. Pass 0 to disable.
    #[arg(long, default_value_t = 30)]
    max_injection_entries: usize,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let label = args
        .label
        .clone()
        .unwrap_or_else(|| {
            args.prefix
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("lens")
                .to_string()
        });

    let prefix = std::fs::read_to_string(&args.prefix)
        .with_context(|| format!("read prefix {:?}", args.prefix))?;
    let suffix_raw = std::fs::read_to_string(&args.suffix)
        .with_context(|| format!("read suffix {:?}", args.suffix))?;

    // Optional graph-derived directive prepended to suffix (A3, real-pipeline
    // layout — matches run_instance.py:750-761). The directive contributes to
    // BOTH baseline and with-injection passes since real-pipeline always
    // includes it; the JSON pool is what varies between conditions.
    let suffix = if let Some(dir_path) = &args.directive {
        let dir_text = std::fs::read_to_string(dir_path)
            .with_context(|| format!("read directive {:?}", dir_path))?;
        tracing::info!(
            "directive: {} ({} bytes prepended to suffix)",
            dir_path.display(),
            dir_text.len()
        );
        format!("{dir_text}{suffix_raw}")
    } else {
        suffix_raw
    };

    let injection_text = if let Some(path) = &args.injection {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read injection {:?}", path))?;
        Some(serialize_injection(&raw, args.max_injection_entries)?)
    } else {
        None
    };

    if args.fake {
        run::<FakeRuntime>(&args, &label, &prefix, &suffix, injection_text.as_deref())?;
    } else {
        #[cfg(feature = "real")]
        {
            run::<LlamaCppRuntime>(&args, &label, &prefix, &suffix, injection_text.as_deref())?;
        }
        #[cfg(not(feature = "real"))]
        anyhow::bail!("real runtime not compiled in; rebuild with --features real, or pass --fake to use the synthetic pipeline");
    }

    Ok(())
}

fn run<R: LensRuntime>(
    args: &Args,
    label: &str,
    prefix: &str,
    suffix: &str,
    injection_text: Option<&str>,
) -> Result<()> {
    let mut rt = R::load(&args.weights, &args.tokenizer)?;
    let mut writer = LensJsonlWriter::create(&args.out)?;

    // Resolve the position spec. --output-position (singular) overrides if set.
    let positions: Vec<u32> = if let Some(p) = args.output_position {
        vec![p]
    } else {
        args.output_positions.items.clone()
    };
    tracing::info!("output positions: {positions:?} (raw spec: {:?})", args.output_positions.raw);

    let baseline_tokens = rt.tokenize(&format!("{prefix}{suffix}"))?;
    tracing::info!(
        "baseline tokens: {} (prefix={} suffix={})",
        baseline_tokens.len(),
        prefix.len(),
        suffix.len()
    );

    // Cycle 0.4 generate mode: autoregressive decode + per-step residuals.
    // Returns ForwardPassCaptures with `position` = generated-token index
    // (0..n_gen), `run` = baseline / with_injection. Skips the prompt-
    // position forward_capture path entirely.
    let (baseline, with_inj) = if args.mode == "generate" {
        tracing::info!("MODE=generate · max_new={}", args.max_new);
        let baseline_gen =
            rt.forward_generate(&baseline_tokens, args.max_new, None, "baseline")?;
        tracing::info!(
            "baseline gen: {} tokens (stopped_on_eos={})",
            baseline_gen.generated_tokens.len(),
            baseline_gen.stopped_on_eos
        );
        let baseline_fpc = autoregressive_to_forward_pass(&baseline_gen);

        let with_inj_fpc = if let Some(inj_text) = injection_text {
            let inj_tokens = rt.tokenize(&format!("{prefix}{inj_text}{suffix}"))?;
            let inj_gen =
                rt.forward_generate(&inj_tokens, args.max_new, None, "with_injection")?;
            tracing::info!(
                "with_injection gen: {} tokens (stopped_on_eos={})",
                inj_gen.generated_tokens.len(),
                inj_gen.stopped_on_eos
            );
            Some(autoregressive_to_forward_pass(&inj_gen))
        } else {
            None
        };
        (baseline_fpc, with_inj_fpc)
    } else {
        let baseline = rt.forward_capture(&baseline_tokens, &positions, "baseline")?;
        let with_inj = if let Some(inj_text) = injection_text {
            let inj_tokens = rt.tokenize(&format!("{prefix}{inj_text}{suffix}"))?;
            tracing::info!("with_injection tokens: {}", inj_tokens.len());
            Some(rt.forward_capture(&inj_tokens, &positions, "with_injection")?)
        } else {
            None
        };
        (baseline, with_inj)
    };

    // Lens math. baseline first so its per-(pos,layer) probs are available
    // for the with-injection pass's KL computation.
    let head = rt.unembed_head().clone();
    if head.output_weight.is_empty() || head.output_norm_weight.is_empty() {
        anyhow::bail!(
            "runtime did not populate unembed head — output_weight: {} elems, output_norm_weight: {} elems",
            head.output_weight.len(),
            head.output_norm_weight.len(),
        );
    }
    let expected = head.n_vocab * head.n_embd;
    if head.output_weight.len() != expected {
        anyhow::bail!(
            "unembed weight has {} elems, expected n_vocab*n_embd = {}*{} = {}",
            head.output_weight.len(),
            head.n_vocab,
            head.n_embd,
            expected,
        );
    }
    let detok = |id: u32| -> String { rt.detokenize(id) };
    let (baseline_steps, baseline_probs_map) =
        compute_lens_steps(&baseline, &head, args.top_k, args.gold_token_id, None, &detok);

    for s in &baseline_steps {
        writer.record(s.clone())?;
    }

    let with_inj_steps: Vec<_> = if let Some(pass) = &with_inj {
        let (steps, _) = compute_lens_steps(
            pass,
            &head,
            args.top_k,
            args.gold_token_id,
            Some(&baseline_probs_map),
            &detok,
        );
        for s in &steps {
            writer.record(s.clone())?;
        }
        steps
    } else {
        Vec::new()
    };

    writer.finish()?;
    tracing::info!("JSONL written to {:?}", args.out);

    if let Some(png) = &args.render_png {
        render_png::render(&baseline_steps, &with_inj_steps, png, label)?;
        tracing::info!("PNG written to {:?}", png);
    }

    if args.render_ascii {
        // For ASCII display, use the first requested position when single-mode,
        // or summary-stats mode when multi-position (the renderer detects via
        // the variety of positions in the slices).
        let display_pos = positions[0];
        let ascii =
            render_ascii::render(&baseline_steps, &with_inj_steps, label, display_pos, args.top_k);
        println!("{ascii}");
    }

    Ok(())
}

/// Flatten an injection JSON file (per `summaries-aplus.json` /
/// `summaries-atomic.json` shape: array of {id, qname, score, summary})
/// into a text block. When `max_entries > 0`, takes the top-N by score
/// descending; otherwise keeps the original order.
fn serialize_injection(raw: &str, max_entries: usize) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .with_context(|| "parse injection JSON")?;
    let arr = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("injection JSON must be an array of summary entries"))?;
    let mut entries: Vec<&serde_json::Value> = arr.iter().collect();
    if max_entries > 0 {
        // Sort descending by score; entries without a numeric score sort last.
        entries.sort_by(|a, b| {
            let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
            let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(max_entries);
    }
    let mut out = String::new();
    out.push_str("\n# Relevant symbols\n");
    for entry in entries {
        let qname = entry.get("qname").and_then(|v| v.as_str()).unwrap_or("?");
        let summary = entry.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("{qname}: {summary}\n"));
    }
    out.push('\n');
    Ok(out)
}

