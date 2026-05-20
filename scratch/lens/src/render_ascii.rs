//! Terminal ASCII summary table.
//!
//! Two purposes:
//!   1. Cheapest diagnostic — runs anywhere, no graphics stack.
//!   2. Cross-check against JSONL + PNG. If all three disagree, one renderer
//!      has a bug.
//!
//! Output shape:
//!
//!   LENS: glia-lens · target=marshmallow-1359 · pos=0 · k=5
//!   ─────┬────────────────────┬──────┬────────────────────┬──────┬──────────
//!    lyr │   baseline top-1    │ rank │  inj top-1          │ rank │  KL
//!   ─────┼────────────────────┼──────┼────────────────────┼──────┼──────────
//!     0  │ "tok13049" 0.04 ▁  │ 387  │ "tok13049" 0.18 ▃  │  42  │ 0.84
//!     1  │ "tok13049" 0.09 ▂  │ 102  │ "tok13049" 0.31 ▅  │   7  │ 0.92
//!     …

use std::collections::BTreeMap;

use crate::jsonl::LensStep;

/// Render the per-layer comparison between a baseline pass and a
/// with-injection pass. Auto-detects multi-position mode: if either side has
/// more than one distinct `position`, switches to per-layer summary stats
/// (one row per layer aggregating across positions). Single-position mode
/// preserves the slice-1 table verbatim.
pub fn render(
    baseline: &[LensStep],
    with_injection: &[LensStep],
    header_label: &str,
    output_position: u32,
    top_k: usize,
) -> String {
    let distinct_positions: std::collections::HashSet<u32> = baseline
        .iter()
        .chain(with_injection.iter())
        .map(|s| s.position)
        .collect();

    if distinct_positions.len() > 1 {
        return render_multi_position(baseline, with_injection, header_label, &distinct_positions, top_k);
    }

    let mut out = String::new();
    out.push_str(&format!(
        "LENS: glia-lens · target={header_label} · pos={output_position} · k={top_k}\n"
    ));
    out.push_str(&horizontal_rule('┬'));
    out.push_str(&format!(
        " {:>3} │ {:^22} │ {:^4} │ {:^22} │ {:^4} │ {:^8}\n",
        "lyr", "baseline top-1", "rank", "inj top-1", "rank", "KL"
    ));
    out.push_str(&horizontal_rule('┼'));

    let n = baseline.len().max(with_injection.len());
    for i in 0..n {
        let b_opt = baseline.get(i);
        let w_opt = with_injection.get(i);
        if b_opt.is_none() && w_opt.is_none() {
            continue;
        }
        let empty = LensStep {
            layer: i as u32,
            run: "—".into(),
            position: output_position,
            top_k: vec![],
            gold_token_id: None,
            gold_rank: None,
            gold_prob: None,
            kl_vs_baseline: None,
            instance_id: None,
        };
        let b = b_opt.unwrap_or(&empty);
        let w = w_opt.unwrap_or(&empty);
        let b_top = b
            .top_k
            .first()
            .map(|t| (t.token_str.as_str(), t.prob))
            .unwrap_or(("∅", 0.0));
        let w_top = w
            .top_k
            .first()
            .map(|t| (t.token_str.as_str(), t.prob))
            .unwrap_or(("∅", 0.0));
        let b_bar = prob_bar(b_top.1);
        let w_bar = prob_bar(w_top.1);
        let b_rank = b
            .gold_rank
            .map(|r| format!("{r}"))
            .unwrap_or_else(|| "—".into());
        let w_rank = w
            .gold_rank
            .map(|r| format!("{r}"))
            .unwrap_or_else(|| "—".into());
        let kl = w
            .kl_vs_baseline
            .map(|k| format!("{k:.3}"))
            .unwrap_or_else(|| "—".into());

        out.push_str(&format!(
            " {:>3} │ {:>14} {:.2} {} │ {:>4} │ {:>14} {:.2} {} │ {:>4} │ {:>8}\n",
            b.layer,
            truncate_quoted(b_top.0, 14),
            b_top.1,
            b_bar,
            b_rank,
            truncate_quoted(w_top.0, 14),
            w_top.1,
            w_bar,
            w_rank,
            kl,
        ));
    }
    out.push_str(&horizontal_rule('┴'));
    out
}

fn horizontal_rule(j: char) -> String {
    let line = "─".repeat(70);
    format!("─────{j}{line}\n")
}

/// Multi-position render: one row per layer, columns aggregate stats across
/// positions. Cols: layer | baseline-top1-mode | inj-top1-mode | KL-peak-pos |
/// KL-peak-val | gold-first-rank≤3-(pos).
///
/// "mode" = the token that's top-1 at the most positions in this layer (the
/// modal/dominant prediction across positions).
fn render_multi_position(
    baseline: &[LensStep],
    with_injection: &[LensStep],
    header_label: &str,
    distinct_positions: &std::collections::HashSet<u32>,
    top_k: usize,
) -> String {
    let mut out = String::new();
    let mut positions: Vec<u32> = distinct_positions.iter().copied().collect();
    positions.sort();
    let pos_summary = if positions.len() <= 6 {
        positions.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
    } else {
        format!("{}..={} (n={})", positions.first().unwrap(), positions.last().unwrap(), positions.len())
    };
    out.push_str(&format!(
        "LENS: glia-lens · target={header_label} · positions=[{pos_summary}] · k={top_k}\n"
    ));
    out.push_str(&horizontal_rule('┬'));
    out.push_str(&format!(
        " {:>3} │ {:^18} │ {:^18} │ {:^4} │ {:^6} │ {:^10}\n",
        "lyr", "baseline-mode", "inj-mode", "klPos", "klVal", "goldFirst≤3"
    ));
    out.push_str(&horizontal_rule('┼'));

    // Group both passes by layer.
    let mut by_layer_b: BTreeMap<u32, Vec<&LensStep>> = BTreeMap::new();
    for s in baseline {
        by_layer_b.entry(s.layer).or_default().push(s);
    }
    let mut by_layer_w: BTreeMap<u32, Vec<&LensStep>> = BTreeMap::new();
    for s in with_injection {
        by_layer_w.entry(s.layer).or_default().push(s);
    }
    let mut all_layers: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    all_layers.extend(by_layer_b.keys().copied());
    all_layers.extend(by_layer_w.keys().copied());

    for l in all_layers {
        let b_steps = by_layer_b.get(&l).cloned().unwrap_or_default();
        let w_steps = by_layer_w.get(&l).cloned().unwrap_or_default();
        let b_mode = modal_top1(&b_steps);
        let w_mode = modal_top1(&w_steps);
        let (kl_pos, kl_val) = peak_kl(&w_steps);
        let gold = first_pos_with_gold_rank_at_most_3(&w_steps).or_else(|| first_pos_with_gold_rank_at_most_3(&b_steps));

        out.push_str(&format!(
            " {:>3} │ {:>16} │ {:>16} │ {:>4} │ {:>6} │ {:>10}\n",
            l,
            truncate_quoted(&b_mode, 16),
            truncate_quoted(&w_mode, 16),
            kl_pos.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
            kl_val.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()),
            gold.map(|p| format!("@p={p}")).unwrap_or_else(|| "—".into()),
        ));
    }
    out.push_str(&horizontal_rule('┴'));
    out
}

fn modal_top1(steps: &[&LensStep]) -> String {
    let mut counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for s in steps {
        if let Some(t) = s.top_k.first() {
            *counts.entry(t.token_str.as_str()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(k, _)| k.to_string())
        .unwrap_or_else(|| "—".into())
}

fn peak_kl(steps: &[&LensStep]) -> (Option<u32>, Option<f32>) {
    let mut best: Option<(u32, f32)> = None;
    for s in steps {
        if let Some(k) = s.kl_vs_baseline {
            best = Some(match best {
                Some((_, b)) if b >= k => best.unwrap(),
                _ => (s.position, k),
            });
        }
    }
    match best {
        Some((p, k)) => (Some(p), Some(k)),
        None => (None, None),
    }
}

fn first_pos_with_gold_rank_at_most_3(steps: &[&LensStep]) -> Option<u32> {
    let mut sorted: Vec<&&LensStep> = steps.iter().collect();
    sorted.sort_by_key(|s| s.position);
    for s in sorted {
        if let Some(r) = s.gold_rank {
            if r <= 3 {
                return Some(s.position);
            }
        }
    }
    None
}

fn truncate_quoted(s: &str, max: usize) -> String {
    let mut q = String::with_capacity(max + 2);
    q.push('"');
    for c in s.chars().take(max) {
        if c == '"' || c == '\n' || c == '\t' {
            q.push('·');
        } else {
            q.push(c);
        }
    }
    q.push('"');
    q
}

/// 8 Unicode blocks: ▁▂▃▄▅▆▇█. prob in [0,1].
fn prob_bar(p: f32) -> char {
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let idx = (p.clamp(0.0, 1.0) * (bars.len() as f32 - 1.0)).round() as usize;
    bars[idx.min(bars.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::TopKEntry;

    fn mk(layer: u32, run: &str, tok: &str, p: f32, rank: Option<u32>, kl: Option<f32>) -> LensStep {
        mk_pos(layer, 0, run, tok, p, rank, kl)
    }

    fn mk_pos(
        layer: u32,
        position: u32,
        run: &str,
        tok: &str,
        p: f32,
        rank: Option<u32>,
        kl: Option<f32>,
    ) -> LensStep {
        LensStep {
            layer,
            run: run.into(),
            position,
            top_k: vec![TopKEntry {
                token_id: 0,
                token_str: tok.into(),
                logit: 0.0,
                prob: p,
            }],
            gold_token_id: rank.map(|_| 0),
            gold_rank: rank,
            gold_prob: rank.map(|_| p),
            kl_vs_baseline: kl,
            instance_id: None,
        }
    }

    #[test]
    fn renders_without_panic() {
        let b = vec![mk(0, "baseline", "x", 0.1, Some(99), None)];
        let w = vec![mk(0, "with_injection", "y", 0.5, Some(3), Some(0.4))];
        let out = render(&b, &w, "marshmallow-1359", 0, 5);
        assert!(out.contains("LENS"));
        assert!(out.contains("\"x\""));
        assert!(out.contains("\"y\""));
    }

    #[test]
    fn prob_bar_clamps_extremes() {
        assert_eq!(prob_bar(0.0), '▁');
        assert_eq!(prob_bar(1.0), '█');
        assert_eq!(prob_bar(-5.0), '▁');
        assert_eq!(prob_bar(99.0), '█');
    }

    #[test]
    fn multi_position_renders_summary_table() {
        // 2 layers × 3 positions, with-injection has varying KL across positions.
        let mut b = Vec::new();
        let mut w = Vec::new();
        for layer in 0..2u32 {
            for pos in 0..3u32 {
                b.push(mk_pos(layer, pos, "baseline", "alpha", 0.2, Some(50), None));
                let kl = (layer * 10 + pos) as f32 * 0.1;
                w.push(mk_pos(layer, pos, "with_injection", "beta", 0.5, Some(3 - pos as u32).filter(|_| pos <= 3), Some(kl)));
            }
        }
        let out = render(&b, &w, "test", 0, 5);
        // Multi-position mode header.
        assert!(out.contains("positions=[0,1,2]"), "got: {out}");
        assert!(out.contains("baseline-mode"), "got: {out}");
        assert!(out.contains("klPos"), "got: {out}");
        assert!(out.contains("\"alpha\""), "got: {out}");
        assert!(out.contains("\"beta\""), "got: {out}");
    }

    #[test]
    fn single_position_keeps_old_table() {
        let b = vec![mk(0, "baseline", "x", 0.1, Some(99), None)];
        let w = vec![mk(0, "with_injection", "y", 0.5, Some(3), Some(0.4))];
        let out = render(&b, &w, "test", 0, 5);
        // Single-position mode uses the old header layout.
        assert!(out.contains("baseline top-1"), "got: {out}");
        assert!(!out.contains("baseline-mode"), "got: {out}");
    }
}
