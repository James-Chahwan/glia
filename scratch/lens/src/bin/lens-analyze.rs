//! Slice 1.7 — analyzer + cross-instance pattern report.
//!
//! Reads:
//!   - `<batch-dir>/all-instances.jsonl` (aggregated lens output, with
//!     `instance_id` injected per row by `lens-batch`)
//!   - `<batch-dir>/summary.json` (per-instance PASS/FAIL from f2p.json)
//!
//! Computes per-instance:
//!   - effect_onset_layer: first layer where with-injection top-1 != baseline
//!   - effect_peak_layer / effect_peak_pos / effect_peak_kl: argmax KL
//!   - effect_persistence: count of consecutive layers KL > median * 2
//!   - final_layer_kl_max: max KL at the last layer across positions
//!   - gold_first_layer / gold_first_pos: first (layer, position) where
//!     gold token enters top-3 (None if no gold).
//!
//! Emits:
//!   - `<out>/cross-instance-report.md` (markdown digest + ASCII histograms)
//!   - `<out>/cross-instance-summary.png` (2x2 panels)
//!   - `<out>/per-instance.jsonl` (one line per instance with stats)

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use plotters::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(name = "lens-analyze", version)]
struct Args {
    /// Directory containing all-instances.jsonl + summary.json (output of lens-batch).
    #[arg(long)]
    batch_dir: PathBuf,

    /// Output directory for the report + PNG + per-instance JSONL.
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct BatchSummary {
    instances: Vec<InstanceSummary>,
}

#[derive(Debug, Deserialize, Clone)]
struct InstanceSummary {
    instance_id: String,
    project: String,
    pass: bool,
    jsonl_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
struct PerInstanceStats {
    instance_id: String,
    project: String,
    pass: bool,
    n_layers: u32,
    n_positions: u32,
    effect_onset_layer: Option<u32>,
    effect_peak_layer: Option<u32>,
    effect_peak_pos: Option<u32>,
    effect_peak_kl: Option<f32>,
    effect_persistence_layers: u32,
    final_layer_kl_max: Option<f32>,
    gold_first_layer: Option<u32>,
    gold_first_pos: Option<u32>,
    /// D3 (cycle 0.6) — for each tracked position p, the smallest layer L
    /// such that inj-run top-1 at every layer L..L_max equals the final-layer
    /// top-1. Captures "when did the model commit to this token." Computed
    /// only when the inj run is in generate mode (positions = generated
    /// tokens); otherwise empty.
    layer_of_decision_per_token: Vec<u32>,
    /// Mean of `layer_of_decision_per_token`, or None when empty. Lower
    /// means the model decides earlier in the stack. Cycle 0.4 lens results
    /// expect ~25 for marshmallow PASS (decision band).
    mean_decision_layer: Option<f32>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    std::fs::create_dir_all(&args.out_dir)?;

    let summary: BatchSummary = {
        let s = std::fs::read_to_string(args.batch_dir.join("summary.json"))
            .context("read summary.json")?;
        serde_json::from_str(&s)?
    };
    let pass_map: HashMap<String, bool> = summary
        .instances
        .iter()
        .map(|i| (i.instance_id.clone(), i.pass))
        .collect();
    let project_map: HashMap<String, String> = summary
        .instances
        .iter()
        .map(|i| (i.instance_id.clone(), i.project.clone()))
        .collect();

    let agg_path = args.batch_dir.join("all-instances.jsonl");
    let by_instance = load_agg(&agg_path)?;
    tracing::info!("loaded {} instances from aggregated JSONL", by_instance.len());

    let mut per_instance: Vec<PerInstanceStats> = Vec::new();
    for (instance_id, rows) in by_instance {
        let pass = pass_map.get(&instance_id).copied().unwrap_or(false);
        let project = project_map
            .get(&instance_id)
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let stats = compute_per_instance(&instance_id, &project, pass, &rows);
        per_instance.push(stats);
    }
    per_instance.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));

    // Per-instance JSONL.
    let pi_path = args.out_dir.join("per-instance.jsonl");
    {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&pi_path)?);
        for s in &per_instance {
            f.write_all(serde_json::to_string(s)?.as_bytes())?;
            f.write_all(b"\n")?;
        }
    }

    // Markdown report.
    let report = build_markdown_report(&per_instance);
    let report_path = args.out_dir.join("cross-instance-report.md");
    std::fs::write(&report_path, report.as_bytes())?;

    // 2×2 PNG.
    let png_path = args.out_dir.join("cross-instance-summary.png");
    render_summary_png(&per_instance, &png_path)?;

    tracing::info!(
        "analyzer DONE: per-instance={}, report={}, png={}",
        pi_path.display(),
        report_path.display(),
        png_path.display(),
    );
    Ok(())
}

/// One JSONL row read from the aggregated batch file. Mirrors the structure
/// emitted by `lens-batch::aggregate` (LensStep + instance_id injected).
#[derive(Debug, Clone, Deserialize)]
struct AggRow {
    event: AggEvent,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", content = "data")]
enum AggEvent {
    LensStep(AggLensStep),
}

#[derive(Debug, Clone, Deserialize)]
struct AggLensStep {
    layer: u32,
    run: String,
    position: u32,
    #[serde(default)]
    top_k: Vec<AggTopK>,
    #[serde(default)]
    gold_rank: Option<u32>,
    #[serde(default)]
    kl_vs_baseline: Option<f32>,
    instance_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AggTopK {
    #[serde(rename = "token_id")]
    _token_id: u32,
    token_str: String,
}

fn load_agg(path: &Path) -> Result<BTreeMap<String, Vec<AggLensStep>>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path:?}"))?;
    let mut by_instance: BTreeMap<String, Vec<AggLensStep>> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: AggRow = serde_json::from_str(line)
            .with_context(|| format!("parse line {} of {path:?}", i + 1))?;
        let AggEvent::LensStep(step) = row.event;
        if let Some(inst) = step.instance_id.clone() {
            by_instance.entry(inst).or_default().push(step);
        }
    }
    Ok(by_instance)
}

fn compute_per_instance(
    instance_id: &str,
    project: &str,
    pass: bool,
    rows: &[AggLensStep],
) -> PerInstanceStats {
    let mut layers: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut positions: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for r in rows {
        layers.insert(r.layer);
        positions.insert(r.position);
    }
    let n_layers = layers.len() as u32;
    let n_positions = positions.len() as u32;

    // baseline vs with_injection split.
    let base: Vec<&AggLensStep> = rows.iter().filter(|r| r.run == "baseline").collect();
    let inj: Vec<&AggLensStep> = rows
        .iter()
        .filter(|r| r.run == "with_injection")
        .collect();

    // Top-1 maps for onset comparison.
    let mut base_top1: HashMap<(u32, u32), &str> = HashMap::new();
    for r in &base {
        if let Some(t) = r.top_k.first() {
            base_top1.insert((r.layer, r.position), t.token_str.as_str());
        }
    }
    let mut inj_top1: HashMap<(u32, u32), &str> = HashMap::new();
    for r in &inj {
        if let Some(t) = r.top_k.first() {
            inj_top1.insert((r.layer, r.position), t.token_str.as_str());
        }
    }

    // Effect-onset: smallest layer where top-1 differs at any position.
    let mut effect_onset_layer: Option<u32> = None;
    for &l in layers.iter() {
        let differs = positions.iter().any(|p| {
            base_top1.get(&(l, *p)) != inj_top1.get(&(l, *p))
        });
        if differs {
            effect_onset_layer = Some(l);
            break;
        }
    }

    // Effect-peak (argmax KL across all inj rows).
    let mut best: Option<(u32, u32, f32)> = None;
    for r in &inj {
        if let Some(k) = r.kl_vs_baseline {
            best = Some(match best {
                Some((_, _, prev)) if prev >= k => best.unwrap(),
                _ => (r.layer, r.position, k),
            });
        }
    }
    let (effect_peak_layer, effect_peak_pos, effect_peak_kl) = match best {
        Some((l, p, k)) => (Some(l), Some(p), Some(k)),
        None => (None, None, None),
    };

    // Persistence: count consecutive layers where any position's KL > threshold.
    let mut kl_by_layer: BTreeMap<u32, Vec<f32>> = BTreeMap::new();
    for r in &inj {
        if let Some(k) = r.kl_vs_baseline {
            kl_by_layer.entry(r.layer).or_default().push(k);
        }
    }
    let all_kls: Vec<f32> = kl_by_layer.values().flatten().copied().collect();
    let median = median_f32(&all_kls);
    let threshold = (median * 2.0).max(0.05);
    let mut max_run = 0u32;
    let mut cur_run = 0u32;
    for (_, ks) in &kl_by_layer {
        let any_above = ks.iter().any(|&k| k > threshold);
        if any_above {
            cur_run += 1;
            max_run = max_run.max(cur_run);
        } else {
            cur_run = 0;
        }
    }
    let effect_persistence_layers = max_run;

    // Final-layer max KL.
    let last_layer = layers.iter().last().copied().unwrap_or(0);
    let final_layer_kl_max = inj
        .iter()
        .filter(|r| r.layer == last_layer)
        .filter_map(|r| r.kl_vs_baseline)
        .fold(None, |acc, k| match acc {
            None => Some(k),
            Some(p) => Some(p.max(k)),
        });

    // D3 — layer-of-decision per position. For each position p, find the
    // smallest layer L_d such that inj_top1[(L_d..L_max), p] is constant and
    // equals the final-layer prediction. This is the layer at which the
    // model commits to its output token. Lower = earlier commitment; the
    // cycle 0.4 lens identified L25-27 as the marshmallow decision band.
    let layers_vec: Vec<u32> = layers.iter().copied().collect();
    let mut layer_of_decision_per_token: Vec<u32> = Vec::new();
    if !layers_vec.is_empty() {
        for &p in positions.iter() {
            let last_l = *layers_vec.last().unwrap();
            let Some(&final_top1) = inj_top1.get(&(last_l, p)) else {
                continue;
            };
            let mut decision: Option<u32> = None;
            for (idx, &l) in layers_vec.iter().enumerate() {
                let constant_from_here = layers_vec[idx..].iter().all(|&ll| {
                    inj_top1.get(&(ll, p)).is_some_and(|t| *t == final_top1)
                });
                if constant_from_here {
                    decision = Some(l);
                    break;
                }
            }
            if let Some(d) = decision {
                layer_of_decision_per_token.push(d);
            }
        }
    }
    let mean_decision_layer = if layer_of_decision_per_token.is_empty() {
        None
    } else {
        let sum: u32 = layer_of_decision_per_token.iter().sum();
        Some(sum as f32 / layer_of_decision_per_token.len() as f32)
    };

    // Gold first-surface (rank ≤ 3) in inj.
    let mut sorted_inj: Vec<&&AggLensStep> = inj.iter().collect();
    sorted_inj.sort_by_key(|r| (r.layer, r.position));
    let mut gold_first_layer = None;
    let mut gold_first_pos = None;
    for r in sorted_inj {
        if let Some(rank) = r.gold_rank {
            if rank <= 3 {
                gold_first_layer = Some(r.layer);
                gold_first_pos = Some(r.position);
                break;
            }
        }
    }

    PerInstanceStats {
        instance_id: instance_id.to_string(),
        project: project.to_string(),
        pass,
        n_layers,
        n_positions,
        effect_onset_layer,
        effect_peak_layer,
        effect_peak_pos,
        effect_peak_kl,
        effect_persistence_layers,
        final_layer_kl_max,
        gold_first_layer,
        gold_first_pos,
        layer_of_decision_per_token,
        mean_decision_layer,
    }
}

fn median_f32(xs: &[f32]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 0 {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

fn build_markdown_report(per_instance: &[PerInstanceStats]) -> String {
    let mut s = String::new();
    s.push_str("# Lens cross-instance report\n\n");
    s.push_str(&format!("Instances: {}\n\n", per_instance.len()));
    let n_pass = per_instance.iter().filter(|p| p.pass).count();
    s.push_str(&format!(
        "- PASS: {} ({:.0}%)\n- FAIL: {}\n\n",
        n_pass,
        100.0 * (n_pass as f32) / (per_instance.len() as f32).max(1.0),
        per_instance.len() - n_pass,
    ));

    s.push_str("## Stratified histograms\n\n");
    s.push_str("### effect_peak_layer\n");
    s.push_str("```\n");
    s.push_str(&histogram(per_instance, |p| p.effect_peak_layer.map(|v| v as f32)));
    s.push_str("```\n\n");

    s.push_str("### effect_persistence_layers\n");
    s.push_str("```\n");
    s.push_str(&histogram(per_instance, |p| {
        Some(p.effect_persistence_layers as f32)
    }));
    s.push_str("```\n\n");

    s.push_str("### final_layer_kl_max\n");
    s.push_str("```\n");
    s.push_str(&histogram(per_instance, |p| p.final_layer_kl_max));
    s.push_str("```\n\n");

    s.push_str("### mean_decision_layer (D3 — when the model commits to its token)\n");
    s.push_str("```\n");
    s.push_str(&histogram(per_instance, |p| p.mean_decision_layer));
    s.push_str("```\n\n");

    s.push_str("## Per-project pass rate\n\n");
    let mut by_proj: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for p in per_instance {
        let entry = by_proj.entry(p.project.clone()).or_default();
        entry.1 += 1;
        if p.pass {
            entry.0 += 1;
        }
    }
    for (proj, (n_pass, n_total)) in by_proj {
        s.push_str(&format!(
            "- {proj}: {n_pass}/{n_total} ({:.0}%)\n",
            100.0 * (n_pass as f32) / (n_total as f32).max(1.0),
        ));
    }
    s.push('\n');

    s.push_str("## Per-instance ranked table\n\n");
    s.push_str("| instance | project | pass | peak-lyr | peak-pos | peak-kl | persist | final-kl | mean-dec | gold-1st(lyr,pos) |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    let mut ranked = per_instance.to_vec();
    ranked.sort_by(|a, b| {
        let ak = a.effect_peak_kl.unwrap_or(0.0);
        let bk = b.effect_peak_kl.unwrap_or(0.0);
        bk.partial_cmp(&ak).unwrap_or(std::cmp::Ordering::Equal)
    });
    for p in ranked {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            p.instance_id,
            p.project,
            if p.pass { "PASS" } else { "FAIL" },
            p.effect_peak_layer.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            p.effect_peak_pos.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            p.effect_peak_kl.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()),
            p.effect_persistence_layers,
            p.final_layer_kl_max.map(|v| format!("{v:.3}")).unwrap_or_else(|| "—".into()),
            p.mean_decision_layer.map(|v| format!("{v:.1}")).unwrap_or_else(|| "—".into()),
            match (p.gold_first_layer, p.gold_first_pos) {
                (Some(l), Some(po)) => format!("({l},{po})"),
                _ => "—".into(),
            },
        ));
    }
    s
}

fn histogram<F: Fn(&PerInstanceStats) -> Option<f32>>(
    per_instance: &[PerInstanceStats],
    f: F,
) -> String {
    let pass_vals: Vec<f32> = per_instance
        .iter()
        .filter(|p| p.pass)
        .filter_map(&f)
        .collect();
    let fail_vals: Vec<f32> = per_instance
        .iter()
        .filter(|p| !p.pass)
        .filter_map(&f)
        .collect();
    let lo = pass_vals
        .iter()
        .chain(fail_vals.iter())
        .copied()
        .fold(f32::INFINITY, f32::min);
    let hi = pass_vals
        .iter()
        .chain(fail_vals.iter())
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    if !lo.is_finite() || !hi.is_finite() {
        return "(no data)\n".to_string();
    }
    let bins = 10;
    let span = (hi - lo).max(1e-6);
    let bin_size = span / bins as f32;
    let mut pass_counts = vec![0u32; bins];
    let mut fail_counts = vec![0u32; bins];
    for v in &pass_vals {
        let b = (((v - lo) / bin_size).floor() as usize).min(bins - 1);
        pass_counts[b] += 1;
    }
    for v in &fail_vals {
        let b = (((v - lo) / bin_size).floor() as usize).min(bins - 1);
        fail_counts[b] += 1;
    }
    let max_count = pass_counts.iter().chain(fail_counts.iter()).copied().max().unwrap_or(0);
    let bar_width = 20usize;
    let mut s = String::new();
    s.push_str(&format!("{:>9}  {:^20}  {:^20}\n", "bin", "PASS", "FAIL"));
    for i in 0..bins {
        let edge_lo = lo + (i as f32) * bin_size;
        let edge_hi = edge_lo + bin_size;
        let p_bar = bar_at(pass_counts[i], max_count, bar_width);
        let f_bar = bar_at(fail_counts[i], max_count, bar_width);
        s.push_str(&format!(
            "{:>4.1}-{:<4.1}  {:>3} {:<width$}  {:>3} {:<width$}\n",
            edge_lo,
            edge_hi,
            pass_counts[i],
            p_bar,
            fail_counts[i],
            f_bar,
            width = bar_width,
        ));
    }
    s
}

fn bar_at(n: u32, max: u32, width: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let frac = (n as f32) / (max as f32);
    let len = (frac * width as f32).round() as usize;
    "█".repeat(len.min(width))
}

fn render_summary_png(per_instance: &[PerInstanceStats], path: &Path) -> Result<()> {
    let root = BitMapBackend::new(path, (1600, 1200)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow::anyhow!("{e}"))?;
    let root = root
        .titled("lens cross-instance summary", ("sans-serif", 22))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let panels = root.split_evenly((2, 2));

    plot_hist(&panels[0], per_instance, "effect_peak_layer", |p| {
        p.effect_peak_layer.map(|v| v as f32)
    })?;
    plot_hist(&panels[1], per_instance, "effect_persistence_layers", |p| {
        Some(p.effect_persistence_layers as f32)
    })?;
    plot_hist(&panels[2], per_instance, "final_layer_kl_max", |p| {
        p.final_layer_kl_max
    })?;
    plot_project_pass_rate(&panels[3], per_instance)?;

    root.present().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn plot_hist<DB: DrawingBackend, F: Fn(&PerInstanceStats) -> Option<f32>>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    per_instance: &[PerInstanceStats],
    title: &str,
    f: F,
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    let pass_vals: Vec<f32> = per_instance.iter().filter(|p| p.pass).filter_map(&f).collect();
    let fail_vals: Vec<f32> = per_instance.iter().filter(|p| !p.pass).filter_map(&f).collect();
    let lo = pass_vals
        .iter()
        .chain(fail_vals.iter())
        .copied()
        .fold(f32::INFINITY, f32::min);
    let hi = pass_vals
        .iter()
        .chain(fail_vals.iter())
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    if !lo.is_finite() || !hi.is_finite() {
        area.fill(&WHITE).map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(());
    }
    let bins = 10usize;
    let span = (hi - lo).max(1e-6);
    let bin_size = span / bins as f32;
    let mut pass_counts = vec![0u32; bins];
    let mut fail_counts = vec![0u32; bins];
    for v in &pass_vals {
        let b = (((v - lo) / bin_size).floor() as usize).min(bins - 1);
        pass_counts[b] += 1;
    }
    for v in &fail_vals {
        let b = (((v - lo) / bin_size).floor() as usize).min(bins - 1);
        fail_counts[b] += 1;
    }
    let max_count = pass_counts.iter().chain(fail_counts.iter()).copied().max().unwrap_or(1);

    let mut chart = ChartBuilder::on(area)
        .caption(title, ("sans-serif", 18))
        .margin(15)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0i32..(bins as i32), 0i32..(max_count as i32 + 1))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    chart.configure_mesh().draw().map_err(|e| anyhow::anyhow!("{e}"))?;
    chart
        .draw_series((0..bins).map(|i| {
            Rectangle::new(
                [(i as i32, 0i32), (i as i32 + 1, pass_counts[i] as i32)],
                BLUE.mix(0.5).filled(),
            )
        }))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .label("PASS")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], BLUE));
    chart
        .draw_series((0..bins).map(|i| {
            Rectangle::new(
                [(i as i32, 0i32), (i as i32 + 1, fail_counts[i] as i32)],
                RED.mix(0.4).filled(),
            )
        }))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .label("FAIL")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], RED));
    chart
        .configure_series_labels()
        .border_style(BLACK)
        .background_style(WHITE.mix(0.8))
        .draw()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn plot_project_pass_rate<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    per_instance: &[PerInstanceStats],
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    let mut by_proj: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for p in per_instance {
        let entry = by_proj.entry(p.project.clone()).or_default();
        entry.1 += 1;
        if p.pass {
            entry.0 += 1;
        }
    }
    let projects: Vec<String> = by_proj.keys().cloned().collect();
    if projects.is_empty() {
        area.fill(&WHITE).map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(());
    }
    let mut chart = ChartBuilder::on(area)
        .caption("per-project pass rate", ("sans-serif", 18))
        .margin(15)
        .x_label_area_size(80)
        .y_label_area_size(40)
        .build_cartesian_2d(0i32..(projects.len() as i32), 0.0f32..1.0f32)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    chart.configure_mesh().draw().map_err(|e| anyhow::anyhow!("{e}"))?;
    chart
        .draw_series(projects.iter().enumerate().map(|(i, proj)| {
            let (np, nt) = by_proj[proj];
            let rate = np as f32 / nt.max(1) as f32;
            Rectangle::new([(i as i32, 0.0), (i as i32 + 1, rate)], GREEN.mix(0.6).filled())
        }))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
