//! Mode 3 — build replay (parse → resolve → cross-resolve, animated counters).
//!
//! In tree-mode, "reveal" is reflected by score: nodes that haven't "arrived"
//! yet get score 0 (they show but dimly). Header carries the live counts.

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

const STAGE_FRAMES: u64 = 30;
const TOTAL_FRAMES: u64 = STAGE_FRAMES * 3;

pub fn render(f: &mut Frame, app: &mut AppState) {
    let frame = layout::split(f.area());

    let cycle = app.frame % TOTAL_FRAMES;
    let stage = (cycle / STAGE_FRAMES) as usize;
    let progress_in_stage = (cycle % STAGE_FRAMES) as f64 / STAGE_FRAMES as f64;

    let total_nodes = app.all_nodes.len();
    let total_intra = app.all_edges.len() - app.graph.cross_edges.len();
    let total_cross = app.graph.cross_edges.len();

    let visible_nodes = if stage == 0 {
        ((total_nodes as f64) * progress_in_stage).floor() as usize
    } else {
        total_nodes
    };
    let visible_intra = if stage < 1 {
        0
    } else if stage == 1 {
        ((total_intra as f64) * progress_in_stage).floor() as usize
    } else {
        total_intra
    };
    let visible_cross = if stage < 2 {
        0
    } else {
        ((total_cross as f64) * progress_in_stage.min(1.0)).floor() as usize
    };

    // Score = stable hash → arrival order. Visible nodes get higher score.
    let scores: HashMap<NodeId, f64> = app
        .all_nodes
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let arrived = i < visible_nodes;
            let h = (id.0 as f64).sin().abs();
            (*id, if arrived { 0.5 + 0.5 * h } else { 0.05 })
        })
        .collect();
    let q_matches: HashSet<NodeId> = app.query_matches().into_iter().collect();
    let tags: HashMap<NodeId, String> = HashMap::new();

    let rows = app.build_tree(&scores);
    app.last_tree = rows.clone();

    panels::search(f, frame.search, app);

    let stage_label = match stage {
        0 => "[3] build ‖ parse",
        1 => "[3] build ‖ resolve",
        _ => "[3] build ‖ cross-resolve",
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled(format!("{}  ", stage_label), Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("nodes "),
        Span::styled(format!("{:>5}", visible_nodes), Style::default().fg(Color::Yellow)),
        Span::raw(format!("/{}    edges ", total_nodes)),
        Span::styled(format!("{:>5}", visible_intra), Style::default().fg(Color::Yellow)),
        Span::raw(format!("/{}    cross ", total_intra)),
        Span::styled(format!("{:>3}", visible_cross), Style::default().fg(Color::Magenta)),
        Span::raw(format!("/{}    reload#{}", total_cross, app.reload_count)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, frame.header);

    tree::render(f, frame.tree, app, &rows, &tags, &q_matches);

    let selected = app.selected_node_in_tree();
    tree::code_pane(f, frame.code, app, selected);
    panels::info(f, frame.info, app, selected, &scores);
    panels::footer(f, frame.footer, app);
}
