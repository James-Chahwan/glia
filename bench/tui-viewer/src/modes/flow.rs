//! Mode 2 — flow trace replay (BFS forward from seed, layer per beat).

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use repo_graph_core::NodeId;

use crate::layout;
use crate::panels;
use crate::state::AppState;
use crate::tree;

const FRAMES_PER_LAYER: u64 = 8;
const MAX_LAYER: usize = 6;

pub fn render(f: &mut Frame, app: &mut AppState) {
    let frame = layout::split(f.area());

    let q_matches = app.query_matches();
    let seed = q_matches.first().copied().or(app.current_seed());
    let seed_label: String = seed
        .and_then(|id| app.meta.get(&id))
        .map(|m| m.qname.clone())
        .unwrap_or_else(|| "(no seed — press / or n)".into());

    let mut dist: HashMap<NodeId, usize> = HashMap::new();
    if let Some(s) = seed {
        let mut frontier = vec![s];
        dist.insert(s, 0);
        for layer in 1..=MAX_LAYER {
            let mut next = Vec::new();
            for nid in frontier.drain(..) {
                if let Some(neigh) = app.adjacency_out.get(&nid) {
                    for (to, _cat) in neigh {
                        if !dist.contains_key(to) {
                            dist.insert(*to, layer);
                            next.push(*to);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
    }

    let current_layer = ((app.frame / FRAMES_PER_LAYER) as usize).min(MAX_LAYER);
    let scores: HashMap<NodeId, f64> = dist
        .iter()
        .filter(|(_, d)| **d <= current_layer)
        .map(|(id, d)| (*id, 1.0 / (1.0 + *d as f64)))
        .collect();
    let tags: HashMap<NodeId, String> = dist
        .iter()
        .map(|(id, d)| (*id, format!("[L{}]", d)))
        .collect();

    let rows = app.build_tree(&scores);
    app.last_tree = rows.clone();

    panels::search(f, frame.search, app);

    let header_line = Line::from(vec![
        Span::styled("[2] flow trace  ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("seed: "),
        Span::styled(
            seed_label,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "    layer {} / {}    {} reachable    reload#{}",
            current_layer, MAX_LAYER, dist.len(), app.reload_count
        )),
    ]);
    let header = Paragraph::new(header_line).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, frame.header);

    let seed_set: HashSet<NodeId> = seed.into_iter().collect();
    tree::render(f, frame.tree, app, &rows, &tags, &seed_set);

    let selected = app.selected_node_in_tree();
    tree::code_pane(f, frame.code, app, selected);
    panels::info(f, frame.info, app, selected, &scores);
    panels::footer(f, frame.footer, app);
}
