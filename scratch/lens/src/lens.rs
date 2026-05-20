//! Per-layer top-K + gold-rank + KL math.
//!
//! Pure logic, no llama.cpp dependency. Takes captured residual streams from
//! the runtime + the unembed weights and produces the `LensStep` records the
//! JSONL / PNG / ASCII renderers consume.
//!
//! Logit lens math:
//!   1. For each captured residual stream `r_l` (shape [n_embd]) at layer l:
//!      `normed = rms_norm(r_l, output_norm_weight, eps)`
//!      `logits = output_weight @ normed`     // shape [n_vocab]
//!      `probs  = softmax(logits)`
//!   2. Take top-K by logit. Look up gold token rank/prob if known.
//!   3. KL(baseline || with_injection) at same (layer, position).

use serde::{Deserialize, Serialize};

use crate::jsonl::{LensStep, TopKEntry};

/// One captured layer's data at one output position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerCapture {
    pub layer: u32,
    /// Residual stream after the full transformer block at this position.
    /// Length = n_embd.
    pub residual: Vec<f32>,
}

/// Static model weights needed to project a residual stream to vocab logits.
///
/// `Clone` is implemented because the head extracts during runtime load and
/// we take an owned copy into the lens-math step. The head is large (n_embd +
/// n_vocab*n_embd f32s) — for Qwen 7B Q4 that's ~152k * 3584 ≈ 2 GB. Cloning
/// is expensive but happens once per binary run; the alternative (borrowing
/// across run() with a mutable runtime) bumps lifetimes everywhere.
#[derive(Clone)]
pub struct UnembedHead {
    /// RMSNorm weight applied after the residual stream. Length = n_embd.
    pub output_norm_weight: Vec<f32>,
    /// Output projection matrix: vocab_size rows × n_embd cols, row-major.
    /// `logits[v] = sum_{e} output_weight[v*n_embd + e] * normed[e]`
    pub output_weight: Vec<f32>,
    pub n_embd: usize,
    pub n_vocab: usize,
    /// `output_norm_eps` from model hparams. Qwen2 uses 1e-6.
    pub eps: f32,
}

/// Apply RMSNorm: `x * weight / sqrt(mean(x*x) + eps)`. In place.
fn rms_norm_inplace(x: &mut [f32], weight: &[f32], eps: f32) {
    debug_assert_eq!(x.len(), weight.len());
    let n = x.len() as f32;
    let mean_sq: f32 = x.iter().map(|v| v * v).sum::<f32>() / n;
    let scale = (mean_sq + eps).sqrt().recip();
    for (xi, wi) in x.iter_mut().zip(weight.iter()) {
        *xi = (*xi * scale) * wi;
    }
}

/// Project one residual stream to vocab logits.
pub fn residual_to_logits(residual: &[f32], head: &UnembedHead) -> Vec<f32> {
    debug_assert_eq!(residual.len(), head.n_embd);
    let mut normed = residual.to_vec();
    rms_norm_inplace(&mut normed, &head.output_norm_weight, head.eps);

    let mut logits = vec![0.0f32; head.n_vocab];
    let stride = head.n_embd;
    for v in 0..head.n_vocab {
        let row = &head.output_weight[v * stride..(v + 1) * stride];
        let mut acc = 0.0f32;
        for e in 0..head.n_embd {
            acc += row[e] * normed[e];
        }
        logits[v] = acc;
    }
    logits
}

/// Numerically-stable softmax.
pub fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let exp: Vec<f32> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f32 = exp.iter().sum();
    exp.into_iter().map(|x| x / sum).collect()
}

/// Top-K (token_id, logit, prob) sorted by descending logit.
pub fn top_k(logits: &[f32], probs: &[f32], k: usize) -> Vec<(u32, f32, f32)> {
    let n = logits.len().min(probs.len());
    let mut idx: Vec<usize> = (0..n).collect();
    let k_eff = k.min(n);
    idx.select_nth_unstable_by(k_eff.saturating_sub(1).min(n - 1), |a, b| {
        logits[*b]
            .partial_cmp(&logits[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut head: Vec<usize> = idx.into_iter().take(k_eff).collect();
    head.sort_by(|a, b| {
        logits[*b]
            .partial_cmp(&logits[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    head.into_iter()
        .map(|i| (i as u32, logits[i], probs[i]))
        .collect()
}

/// Rank of `gold_id` in `logits` (0 = top-1). O(n) — no full sort needed.
pub fn rank_of(gold_id: u32, logits: &[f32]) -> u32 {
    let g = gold_id as usize;
    if g >= logits.len() {
        return logits.len() as u32;
    }
    let g_logit = logits[g];
    let mut rank = 0u32;
    for (i, &l) in logits.iter().enumerate() {
        if i == g {
            continue;
        }
        if l > g_logit {
            rank += 1;
        }
    }
    rank
}

/// KL(p || q) where p and q are full probability distributions. Numerically
/// guards against log(0).
pub fn kl_divergence(p: &[f32], q: &[f32]) -> f32 {
    debug_assert_eq!(p.len(), q.len());
    const EPS: f32 = 1e-12;
    let mut acc = 0.0f32;
    for i in 0..p.len() {
        let pi = p[i];
        if pi < EPS {
            continue;
        }
        let qi = q[i].max(EPS);
        acc += pi * (pi / qi).ln();
    }
    acc
}

/// One forward pass's captures across multiple output positions × layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardPassCaptures {
    pub run: String,
    /// One slice per position in the output stream we tracked.
    pub positions: Vec<PositionCapture>,
}

/// Per-layer captures at one output position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionCapture {
    pub position: u32,
    pub layers: Vec<LayerCapture>,
}

/// Output from an autoregressive generation run (cycle 0.4 lens). Records the
/// per-layer residual at each generated token position. This is what the
/// slice-1.5 prompt-position lens couldn't see: the cycle 0.3 directive
/// steers DURING DECODING, so the differentiation between conditions is at
/// generated tokens 1, 5, 20, 50 — not at the first generated token alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoregressiveCaptures {
    pub run: String,
    /// The sequence of token ids the model actually generated (post-prompt).
    /// `generated_tokens[i]` corresponds to `steps[i]`.
    pub generated_tokens: Vec<u32>,
    /// Per-generated-token, per-layer residual stream.
    pub steps: Vec<PositionCapture>,
    /// True if generation stopped on EOS rather than hitting max_new.
    pub stopped_on_eos: bool,
}

/// Baseline probability lookup indexed by (position, layer). Built from the
/// baseline pass's `compute_lens_steps` result so the with-injection pass can
/// look up `(p, l) -> Vec<f32>` for KL divergence at matching coordinates.
pub type BaselineProbsByPosLayer = std::collections::HashMap<(u32, u32), Vec<f32>>;

/// Compute lens results for one pass. Optionally pass a baseline-probs map
/// to populate `kl_vs_baseline` on each emitted step.
///
/// Returns `(steps, baseline_probs)` so the baseline pass's call can hand its
/// per-(pos,layer) probability distributions into the with-injection pass.
pub fn compute_lens_steps(
    pass: &ForwardPassCaptures,
    head: &UnembedHead,
    k: usize,
    gold_token_id: Option<u32>,
    baseline_probs: Option<&BaselineProbsByPosLayer>,
    token_str_of: impl Fn(u32) -> String,
) -> (Vec<LensStep>, BaselineProbsByPosLayer) {
    let total: usize = pass.positions.iter().map(|p| p.layers.len()).sum();
    let mut steps = Vec::with_capacity(total);
    let mut probs_map: BaselineProbsByPosLayer =
        std::collections::HashMap::with_capacity(total);

    for pc in &pass.positions {
        for capture in &pc.layers {
            let logits = residual_to_logits(&capture.residual, head);
            let probs = softmax(&logits);
            let top = top_k(&logits, &probs, k);
            let (gold_rank, gold_prob) = match gold_token_id {
                Some(g) => (Some(rank_of(g, &logits)), Some(probs[g as usize])),
                None => (None, None),
            };
            let kl = baseline_probs
                .and_then(|m| m.get(&(pc.position, capture.layer)))
                .map(|baseline_p| kl_divergence(baseline_p, &probs));
            let step = LensStep {
                layer: capture.layer,
                run: pass.run.clone(),
                position: pc.position,
                top_k: top
                    .into_iter()
                    .map(|(id, logit, prob)| TopKEntry {
                        token_id: id,
                        token_str: token_str_of(id),
                        logit,
                        prob,
                    })
                    .collect(),
                gold_token_id,
                gold_rank,
                gold_prob,
                kl_vs_baseline: kl,
                instance_id: None,
            };
            steps.push(step);
            probs_map.insert((pc.position, capture.layer), probs);
        }
    }
    (steps, probs_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one() {
        let logits = vec![1.0, 2.0, 3.0, -1.0];
        let probs = softmax(&logits);
        let s: f32 = probs.iter().sum();
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn top_k_returns_largest_descending() {
        let logits = vec![0.1, 5.0, 3.2, -1.0, 5.5, 2.0];
        let probs = softmax(&logits);
        let top = top_k(&logits, &probs, 3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].0, 4); // 5.5
        assert_eq!(top[1].0, 1); // 5.0
        assert_eq!(top[2].0, 2); // 3.2
        // Logits descending.
        assert!(top[0].1 >= top[1].1);
        assert!(top[1].1 >= top[2].1);
    }

    #[test]
    fn rank_of_gold_matches_naive_sort() {
        let logits = vec![3.0, 1.0, 5.0, 2.0, 4.0];
        // Sorted desc: [5.0(idx2), 4.0(idx4), 3.0(idx0), 2.0(idx3), 1.0(idx1)]
        assert_eq!(rank_of(2, &logits), 0);
        assert_eq!(rank_of(4, &logits), 1);
        assert_eq!(rank_of(0, &logits), 2);
        assert_eq!(rank_of(3, &logits), 3);
        assert_eq!(rank_of(1, &logits), 4);
    }

    #[test]
    fn kl_divergence_zero_for_identical_distributions() {
        let p = vec![0.1, 0.2, 0.3, 0.4];
        let kl = kl_divergence(&p, &p);
        assert!(kl.abs() < 1e-5);
    }

    #[test]
    fn kl_divergence_positive_for_different_distributions() {
        let p = vec![0.7, 0.1, 0.1, 0.1];
        let q = vec![0.1, 0.7, 0.1, 0.1];
        let kl = kl_divergence(&p, &q);
        assert!(kl > 0.0);
    }

    #[test]
    fn rms_norm_zero_input_yields_zero() {
        let mut x = vec![0.0f32; 4];
        let w = vec![1.0f32; 4];
        rms_norm_inplace(&mut x, &w, 1e-6);
        for v in &x {
            // sqrt(0+eps) is tiny, but 0 * weight / tiny = 0.
            assert!(v.abs() < 1e-3);
        }
    }

    #[test]
    fn rms_norm_unit_input_with_unit_weight_is_identity_ish() {
        // x of all 1.0, mean_sq = 1.0, scale = 1/sqrt(1+eps) ≈ 1.
        let mut x = vec![1.0f32; 8];
        let w = vec![1.0f32; 8];
        rms_norm_inplace(&mut x, &w, 1e-6);
        for v in &x {
            assert!((v - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn residual_to_logits_dimensions() {
        let head = UnembedHead {
            output_norm_weight: vec![1.0; 4],
            output_weight: vec![
                1.0, 0.0, 0.0, 0.0, // v=0
                0.0, 1.0, 0.0, 0.0, // v=1
                0.0, 0.0, 1.0, 0.0, // v=2
            ],
            n_embd: 4,
            n_vocab: 3,
            eps: 1e-6,
        };
        let residual = vec![3.0, 2.0, 1.0, 0.0];
        let logits = residual_to_logits(&residual, &head);
        assert_eq!(logits.len(), 3);
        // After RMSNorm, residual is scaled by 1/sqrt(mean_sq). The relative
        // order is preserved with this identity-ish weight.
        assert!(logits[0] > logits[1]);
        assert!(logits[1] > logits[2]);
    }
}
