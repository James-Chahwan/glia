//! Mode 1 — PPR pulse / multi-seed neurons.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use repo_graph_activation::{activate, ActivationConfig};
use repo_graph_core::NodeId;

use crate::layout;
use crate::panels;
use crate::state::AppState;
use crate::tree;

pub fn render(f: &mut Frame, app: &mut AppState) {
    let frame = layout::split(f.area());

    let q_matches = app.query_matches();
    let seeds: Vec<NodeId> = if !q_matches.is_empty() {
        q_matches.clone()
    } else if let Some(s) = app.current_seed() {
        vec![s]
    } else {
        Vec::new()
    };

    let scores: HashMap<NodeId, f64> = if !seeds.is_empty() {
        let cfg = ActivationConfig {
            top_k: app.all_nodes.len(),
            max_iterations: 50,
            ..Default::default()
        };
        let res = activate(&app.all_nodes, &app.all_edges, &seeds, &cfg);
        let max = res.scores.first().map(|(_, s)| *s).unwrap_or(1.0).max(1e-12);
        res.scores
            .into_iter()
            .map(|(id, s)| (id, s / max))
            .collect()
    } else {
        HashMap::new()
    };

    let rows = app.build_tree(&scores);
    app.last_tree = rows.clone();

    panels::search(f, frame.search, app);

    let header_line = if !q_matches.is_empty() {
        Line::from(vec![
            Span::styled(
                "[1] PPR neurons  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("query: "),
            Span::styled(
                format!("\"{}\"", app.query),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "    {} seeds · {} reachable    frame {}    reload#{}",
                seeds.len(),
                scores.len(),
                app.frame,
                app.reload_count
            )),
        ])
    } else {
        let label: String = app
            .current_seed()
            .and_then(|id| app.meta.get(&id))
            .map(|m| m.qname.clone())
            .unwrap_or_else(|| "(no seeds — press / to query)".into());
        Line::from(vec![
            Span::styled(
                "[1] PPR pulse  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("seed: "),
            Span::styled(
                label,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("    frame {}    reload#{}", app.frame, app.reload_count)),
        ])
    };
    let header = Paragraph::new(header_line).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, frame.header);

    let seed_set: HashSet<NodeId> = seeds.iter().copied().collect();
    let tags: HashMap<NodeId, String> = HashMap::new();
    tree::render(f, frame.tree, app, &rows, &tags, &seed_set);

    let selected = app.selected_node_in_tree();
    tree::code_pane(f, frame.code, app, selected);
    panels::info(f, frame.info, app, selected, &scores);
    panels::footer(f, frame.footer, app);
}
