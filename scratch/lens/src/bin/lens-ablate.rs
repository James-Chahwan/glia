//! lens-ablate — A3, cycle 0.6 spitball bucket.
//!
//! For a single fixture, runs forward_generate N+1 times:
//!   - 1 baseline pass: record the generated token sequence
//!   - For each input-token index i in --ablation-range, mask tokens[i] with
//!     --mask-token-id (default 151643 = Qwen's <|endoftext|>) and re-run,
//!     record the resulting generated sequence.
//!
//! Importance score per masked token: number of generated positions where
//! the ablated sequence differs from baseline. High = the masked token
//! mattered for greedy decode trajectory. Low = the model ignored it.
//!
//! Output JSONL (one line per ablation):
//!   {"token_idx": i, "token_id": u32, "token_str": "...",
//!    "hamming_count": N, "first_divergence_step": Option<u32>,
//!    "ablated_tokens": [u32]}
//!
//! Performance budget: 7B Q4 forward_generate ~5-10s per call on CPU.
//! 50 ablations × 8s = ~7min wall-clock. Scope to directive block tokens
//! (~50-100) via --ablation-range to stay reasonable.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;

use glia_lens::runtime::{FakeRuntime, LensRuntime};

#[cfg(feature = "real")]
use glia_lens::runtime::LlamaCppRuntime;

#[derive(Parser, Debug)]
#[command(name = "lens-ablate", version,
          about = "Token-importance ablation via forward_generate Hamming diff")]
struct Args {
    #[arg(long)]
    weights: PathBuf,

    #[arg(long)]
    tokenizer: PathBuf,

    #[arg(long)]
    prefix: PathBuf,

    #[arg(long)]
    suffix: PathBuf,

    /// Optional directive prepended to suffix (same semantics as
    /// `lens --directive`).
    #[arg(long)]
    directive: Option<PathBuf>,

    /// Max tokens to generate per pass. Smaller = faster per ablation;
    /// larger = more sensitive to subtle steering. 16 is the cycle 0.4
    /// decision-band window; 32 catches late commitment.
    #[arg(long, default_value_t = 16)]
    max_new: u32,

    /// Token range to ablate (inclusive..exclusive). Format:
    /// "<lo>..<hi>" or "<lo>..=<hi>". When omitted, defaults to the last
    /// 80 prompt tokens (typically captures the directive block).
    #[arg(long)]
    ablation_range: Option<String>,

    /// Token ID to substitute when masking. Default = 151643
    /// (Qwen `<|endoftext|>`). Use 0 for a more conservative mask.
    #[arg(long, default_value_t = 151643)]
    mask_token_id: u32,

    /// Output JSONL path.
    #[arg(long)]
    out: PathBuf,

    /// Use FakeRuntime for end-to-end pipeline testing.
    #[arg(long, default_value_t = false)]
    fake: bool,

    /// Label embedded in the output JSONL header line.
    #[arg(long, default_value = "lens-ablate")]
    label: String,
}

#[derive(Debug, Serialize)]
struct AblationRecord<'a> {
    token_idx: u32,
    token_id: u32,
    token_str: String,
    hamming_count: u32,
    first_divergence_step: Option<u32>,
    baseline_tokens: &'a [u32],
    ablated_tokens: Vec<u32>,
}

fn parse_range(spec: &str, n_tokens: usize) -> Result<(u32, u32)> {
    let inclusive = spec.contains("..=");
    let (lo, hi) = if inclusive {
        spec.split_once("..=").unwrap()
    } else if spec.contains("..") {
        spec.split_once("..").unwrap()
    } else {
        anyhow::bail!("bad range: {spec}");
    };
    let lo: u32 = lo.trim().parse().context("parse range lo")?;
    let mut hi: u32 = hi.trim().parse().context("parse range hi")?;
    if inclusive {
        hi += 1;
    }
    if hi as usize > n_tokens {
        hi = n_tokens as u32;
    }
    if lo >= hi {
        anyhow::bail!("empty range: {spec}");
    }
    Ok((lo, hi))
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
    let tokens = rt.tokenize(&prompt)?;
    let n_tokens = tokens.len();
    tracing::info!("prompt tokens: {n_tokens}");

    let (lo, hi) = match &args.ablation_range {
        Some(s) => parse_range(s, n_tokens)?,
        None => {
            let lo = n_tokens.saturating_sub(80) as u32;
            (lo, n_tokens as u32)
        }
    };
    tracing::info!("ablation range: {lo}..{hi} ({} tokens)", hi - lo);

    let baseline = rt.forward_generate(&tokens, args.max_new, None, "baseline")?;
    let baseline_tokens: Vec<u32> = baseline.generated_tokens.clone();
    tracing::info!(
        "baseline generated {} tokens (stopped_on_eos={})",
        baseline_tokens.len(),
        baseline.stopped_on_eos
    );

    use std::io::Write;
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(&args.out)?);
    writeln!(
        f,
        "{}",
        serde_json::json!({
            "kind": "Header",
            "label": args.label,
            "n_prompt_tokens": n_tokens,
            "max_new": args.max_new,
            "ablation_range": [lo, hi],
            "mask_token_id": args.mask_token_id,
            "baseline_tokens": baseline_tokens,
        })
    )?;

    let total = (hi - lo) as usize;
    for (idx, i) in (lo..hi).enumerate() {
        let mut masked = tokens.clone();
        let original_id = masked[i as usize];
        masked[i as usize] = args.mask_token_id;
        let ablated = rt.forward_generate(&masked, args.max_new, None, "ablated")?;
        let ablated_tokens = ablated.generated_tokens;

        let max_len = baseline_tokens.len().max(ablated_tokens.len());
        let mut hamming = 0u32;
        let mut first_div: Option<u32> = None;
        for step in 0..max_len {
            let b = baseline_tokens.get(step).copied();
            let a = ablated_tokens.get(step).copied();
            if a != b {
                hamming += 1;
                if first_div.is_none() {
                    first_div = Some(step as u32);
                }
            }
        }

        let record = AblationRecord {
            token_idx: i,
            token_id: original_id,
            token_str: rt.detokenize(original_id),
            hamming_count: hamming,
            first_divergence_step: first_div,
            baseline_tokens: &baseline_tokens,
            ablated_tokens,
        };
        writeln!(f, "{}", serde_json::to_string(&record)?)?;
        f.flush()?;

        if (idx + 1) % 10 == 0 || idx + 1 == total {
            tracing::info!(
                "ablation progress: {}/{} (last token_idx={}, hamming={}, first_div={:?})",
                idx + 1,
                total,
                i,
                hamming,
                first_div,
            );
        }
    }

    tracing::info!("DONE: wrote {} ablation records to {}", total, args.out.display());
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
