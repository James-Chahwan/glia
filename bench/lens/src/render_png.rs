//! 2D PNG renderer via the `plotters` crate.
//!
//! Two side-by-side panels:
//!
//!   ┌──────────────────────────────┬──────────────────────────────┐
//!   │ BASELINE                     │ WITH INJECTION               │
//!   │                              │                              │
//!   │ top-1 prob ─ red             │ top-1 prob ─ red             │
//!   │ gold prob ─ blue             │ gold prob ─ blue             │
//!   │ KL ─ dashed black            │ KL ─ dashed black            │
//!   │                              │                              │
//!   │ X = layer index (0..N-1)     │ X = layer index (0..N-1)     │
//!   │ Y = probability (0..1)       │ Y = probability (0..1)       │
//!   └──────────────────────────────┴──────────────────────────────┘
//!
//! The KL curve only appears on the right panel (KL is undefined on baseline
//! against itself). The gold-prob curve is dropped if gold_token_id is None.

use std::path::Path;

use anyhow::Result;
use plotters::prelude::*;

use crate::jsonl::LensStep;

const W: u32 = 1600;
const H: u32 = 800;

pub fn render(
    baseline: &[LensStep],
    with_injection: &[LensStep],
    out_path: &Path,
    header_label: &str,
) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let root = BitMapBackend::new(out_path, (W, H)).into_drawing_area();
    root.fill(&WHITE)?;
    let title = format!("glia-lens · {header_label}");
    let root = root.titled(&title, ("sans-serif", 22))?;
    let (left, right) = root.split_horizontally(W / 2);

    // Detect multi-position mode: more than 1 distinct position in either pass.
    let distinct_positions: std::collections::HashSet<u32> = baseline
        .iter()
        .chain(with_injection.iter())
        .map(|s| s.position)
        .collect();
    let multi_pos = distinct_positions.len() > 1;

    if multi_pos {
        draw_heatmap(&left, baseline, "BASELINE · top-1 prob")?;
        draw_heatmap(&right, with_injection, "WITH INJECTION · KL vs baseline")?;
    } else {
        draw_panel(&left, baseline, "BASELINE", false)?;
        draw_panel(&right, with_injection, "WITH INJECTION", true)?;
    }
    root.present()?;
    Ok(())
}

/// Multi-position heatmap: X = position, Y = layer, color intensity = metric.
/// For baseline panel, metric = top-1 prob (higher = more confident).
/// For with-injection panel, metric = KL divergence vs baseline at same
/// (pos, layer) (higher = more diverged from baseline).
fn draw_heatmap<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    steps: &[LensStep],
    title: &str,
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    if steps.is_empty() {
        area.fill(&WHITE).map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(());
    }
    // Determine grid dimensions.
    let mut positions: Vec<u32> = steps.iter().map(|s| s.position).collect();
    positions.sort();
    positions.dedup();
    let mut layers: Vec<u32> = steps.iter().map(|s| s.layer).collect();
    layers.sort();
    layers.dedup();

    let is_kl_panel = title.contains("KL");
    let metric_max = if is_kl_panel {
        steps
            .iter()
            .filter_map(|s| s.kl_vs_baseline)
            .fold(0.0f32, |a, b| a.max(b))
            .max(1.0)
    } else {
        1.0f32
    };

    // Index step → (position, layer) → metric.
    let mut grid: std::collections::HashMap<(u32, u32), f32> = std::collections::HashMap::new();
    for s in steps {
        let v = if is_kl_panel {
            s.kl_vs_baseline.unwrap_or(0.0)
        } else {
            s.top_k.first().map(|t| t.prob).unwrap_or(0.0)
        };
        grid.insert((s.position, s.layer), v);
    }

    let x_max = (positions.last().copied().unwrap_or(0) + 1) as i32;
    let y_max = (layers.last().copied().unwrap_or(0) + 1) as i32;
    let mut chart = ChartBuilder::on(area)
        .caption(title, ("sans-serif", 18))
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(0i32..x_max, 0i32..y_max)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    chart
        .configure_mesh()
        .x_desc("position")
        .y_desc("layer")
        .axis_desc_style(("sans-serif", 14))
        .draw()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let cell_rects: Vec<Rectangle<(i32, i32)>> = positions
        .iter()
        .flat_map(|&p| {
            let positions_ref = &positions;
            let grid_ref = &grid;
            layers.iter().map(move |&l| {
                let v = grid_ref.get(&(p, l)).copied().unwrap_or(0.0);
                let intensity = (v / metric_max).clamp(0.0, 1.0);
                // White (low) → red (high) for KL; white → blue for prob.
                let color = if is_kl_panel {
                    RGBColor(
                        255,
                        ((1.0 - intensity) * 255.0) as u8,
                        ((1.0 - intensity) * 255.0) as u8,
                    )
                } else {
                    RGBColor(
                        ((1.0 - intensity) * 255.0) as u8,
                        ((1.0 - intensity) * 255.0) as u8,
                        255,
                    )
                };
                let _ = positions_ref;
                Rectangle::new([(p as i32, l as i32), (p as i32 + 1, l as i32 + 1)], color.filled())
            })
        })
        .collect();
    chart
        .draw_series(cell_rects)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

fn draw_panel<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    steps: &[LensStep],
    title: &str,
    show_kl: bool,
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    if steps.is_empty() {
        area.fill(&WHITE).map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(());
    }

    let n = steps.len();
    let x_range = 0u32..(n as u32);
    let mut chart = ChartBuilder::on(area)
        .caption(title, ("sans-serif", 18))
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(x_range.clone(), 0.0f32..1.05f32)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    chart
        .configure_mesh()
        .x_desc("layer")
        .y_desc("prob")
        .axis_desc_style(("sans-serif", 14))
        .draw()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // top-1 prob in red.
    chart
        .draw_series(LineSeries::new(
            steps
                .iter()
                .enumerate()
                .map(|(i, s)| (i as u32, s.top_k.first().map(|t| t.prob).unwrap_or(0.0))),
            RED.stroke_width(2),
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .label("top-1 prob")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], RED));

    // gold prob in blue, when present.
    if steps.iter().any(|s| s.gold_prob.is_some()) {
        chart
            .draw_series(LineSeries::new(
                steps
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| s.gold_prob.map(|p| (i as u32, p))),
                BLUE.stroke_width(2),
            ))
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .label("gold prob")
            .legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], BLUE));
    }

    // KL on the right panel only. KL can exceed 1.0, but rescale to fit the
    // same Y axis: take min(KL, 1.0) and draw dashed.
    if show_kl && steps.iter().any(|s| s.kl_vs_baseline.is_some()) {
        chart
            .draw_series(LineSeries::new(
                steps.iter().enumerate().filter_map(|(i, s)| {
                    s.kl_vs_baseline.map(|k| (i as u32, k.min(1.0)))
                }),
                BLACK.stroke_width(1),
            ))
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .label("KL (min 1.0)")
            .legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], BLACK));
    }

    chart
        .configure_series_labels()
        .border_style(BLACK)
        .background_style(WHITE.mix(0.8))
        .draw()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::TopKEntry;

    fn mk(layer: u32, p: f32, gold: Option<f32>) -> LensStep {
        mk_pos(layer, 0, p, gold, None)
    }

    fn mk_pos(layer: u32, position: u32, p: f32, gold: Option<f32>, kl: Option<f32>) -> LensStep {
        LensStep {
            layer,
            run: "x".into(),
            position,
            top_k: vec![TopKEntry {
                token_id: 0,
                token_str: "t".into(),
                logit: 0.0,
                prob: p,
            }],
            gold_token_id: gold.map(|_| 0),
            gold_rank: gold.map(|_| 0),
            gold_prob: gold,
            kl_vs_baseline: kl,
            instance_id: None,
        }
    }

    #[test]
    fn renders_to_temp_file_without_panic() {
        let dir = std::env::temp_dir().join("glia_lens_test_png");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("smoke.png");
        let b: Vec<_> = (0..8).map(|l| mk(l, (l as f32) / 10.0, Some((l as f32) / 12.0))).collect();
        let w: Vec<_> = (0..8).map(|l| mk(l, (l as f32) / 8.0, Some((l as f32) / 9.0))).collect();
        render(&b, &w, &path, "test").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 0);
    }

    #[test]
    fn renders_multi_position_heatmap_without_panic() {
        let dir = std::env::temp_dir().join("glia_lens_test_png_heatmap");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("smoke-heatmap.png");
        // 6 layers × 4 positions.
        let mut b = Vec::new();
        let mut w = Vec::new();
        for l in 0..6u32 {
            for p in 0..4u32 {
                let prob = 0.1 + (l as f32) * 0.1 + (p as f32) * 0.02;
                let kl = (l as f32) * 0.5 + (p as f32) * 0.3;
                b.push(mk_pos(l, p, prob.min(1.0), Some(0.2), None));
                w.push(mk_pos(l, p, prob.min(1.0), Some(0.4), Some(kl)));
            }
        }
        render(&b, &w, &path, "test-heatmap").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 200);
    }
}
