//! Shared panels: search input, info panel, footer.
//!
//! Modes call these so the chrome looks identical across them.

use std::collections::HashMap;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use repo_graph_code_domain::edge_category;
use repo_graph_core::{EdgeCategoryId, NodeId};

use crate::state::AppState;
use crate::style::{glyph, kind_label, tier_of};

/// Top-of-screen text input. Cursor block when focused.
pub fn search(f: &mut Frame, area: Rect, app: &AppState) {
    let prefix = if app.query_focused { "/ " } else { "  " };
    let display: Vec<Span> = if app.query_focused {
        vec![
            Span::styled(prefix, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(
                app.query.clone(),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow).add_modifier(Modifier::SLOW_BLINK)),
        ]
    } else if app.query.is_empty() {
        vec![
            Span::styled(prefix, Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                "press / to query (e.g. \"auth login\" → multi-seed PPR)",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]
    } else {
        vec![
            Span::styled(prefix, Style::default().add_modifier(Modifier::DIM)),
            Span::raw(app.query.clone()),
            Span::styled(
                format!("    ({} matches)", app.query_matches().len()),
                Style::default().fg(Color::Green),
            ),
        ]
    };
    let title = if app.query_focused {
        " query (typing — Esc cancel · Enter apply) "
    } else {
        " query "
    };
    let widget = Paragraph::new(Line::from(display)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                title,
                Style::default().fg(if app.query_focused {
                    Color::Yellow
                } else {
                    Color::Reset
                }),
            )),
    );
    f.render_widget(widget, area);
}

/// Compact graph-metadata view. Single line for qname; one line each for
/// outgoing and incoming edges. The cells/code live in the right-side pane.
pub fn info(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    selected: Option<NodeId>,
    scores: &HashMap<NodeId, f64>,
) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(id) = selected {
        if let Some(meta) = app.meta.get(&id) {
            let kind_str = kind_label(meta.kind);
            let tier = tier_of(meta.kind);
            let conf = match app
                .graph
                .graphs
                .iter()
                .flat_map(|g| g.nodes.iter())
                .find(|n| n.id == id)
                .map(|n| n.confidence)
            {
                Some(repo_graph_core::Confidence::Strong) => "strong",
                Some(repo_graph_core::Confidence::Medium) => "medium",
                Some(repo_graph_core::Confidence::Weak) => "weak",
                None => "?",
            };
            let score = scores.get(&id).copied().unwrap_or(0.0);

            lines.push(Line::from(vec![
                Span::styled("► ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    meta.qname.clone(),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::raw(format!("{} {}", glyph(meta.kind), kind_str)),
                Span::raw("  "),
                Span::styled(format!("[{}]", tier.label()), Style::default().fg(Color::Magenta)),
                Span::raw(format!("  conf: {}  score: {:.3}", conf, score)),
            ]));

            let outs = app.out_neighbours(id);
            let ins = app.in_neighbours(id);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  out ({}): ", outs.len()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(format_first_n(&outs, 4, app, true)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  in  ({}): ", ins.len()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(format_first_n(&ins, 4, app, false)),
            ]));
        } else {
            lines.push(Line::raw("  (selected row has no node metadata)"));
        }
    } else {
        lines.push(Line::raw("  (cursor is on a group/tier — ↑↓ move, space toggle collapse)"));
    }

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" info "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(widget, area);
}

pub fn footer(f: &mut Frame, area: Rect, app: &AppState) {
    let text = if let Some(msg) = &app.last_message {
        msg.clone()
    } else if app.query_focused {
        "  typing query · Enter apply · Esc cancel".to_string()
    } else {
        "  / query · ↑↓ scroll · space collapse · Enter seed · n cycle · 1/2/3 mode · p pause · r reload · q quit".to_string()
    };
    let widget = Paragraph::new(text)
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(widget, area);
}

fn format_first_n(
    edges: &[(NodeId, EdgeCategoryId)],
    n: usize,
    app: &AppState,
    outgoing: bool,
) -> String {
    if edges.is_empty() {
        return "—".into();
    }
    let mut out = String::new();
    for (i, (other, cat)) in edges.iter().take(n).enumerate() {
        if i > 0 {
            out.push_str("  ·  ");
        }
        let cat_label = edge_category_name(*cat);
        let other_q = app
            .meta
            .get(other)
            .map(|m| m.name.as_str())
            .unwrap_or("?");
        if outgoing {
            out.push_str(&format!("→[{}] {}", cat_label, other_q));
        } else {
            out.push_str(&format!("[{}]← {}", cat_label, other_q));
        }
    }
    if edges.len() > n {
        out.push_str(&format!("  …(+{})", edges.len() - n));
    }
    out
}

fn edge_category_name(c: EdgeCategoryId) -> &'static str {
    use edge_category::*;
    match c {
        x if x == DEFINES => "defines",
        x if x == CONTAINS => "contains",
        x if x == IMPORTS => "imports",
        x if x == CALLS => "calls",
        x if x == USES => "uses",
        x if x == DOCUMENTS => "documents",
        x if x == TESTS => "tests",
        x if x == INJECTS => "injects",
        x if x == HANDLED_BY => "handled_by",
        x if x == HTTP_CALLS => "http_calls",
        x if x == GRPC_CALLS => "grpc_calls",
        x if x == QUEUE_FLOWS => "queue_flows",
        x if x == GRAPHQL_CALLS => "graphql_calls",
        x if x == WS_CONNECTS => "ws_connects",
        x if x == EVENT_FLOWS => "event_flows",
        x if x == SHARES_SCHEMA => "shares_schema",
        x if x == CLI_INVOKES => "cli_invokes",
        x if x == ACCESSES_DATA => "accesses_data",
        x if x == HAS_ATTRIBUTE => "has_attribute",
        x if x == INHERITS_FROM => "inherits_from",
        x if x == RETURNS_TYPE => "returns_type",
        x if x == SHARES_DATA_ENTITY => "shares_data_entity",
        x if x == SCHEDULES => "schedules",
        x if x == SHARES_CRON_SCHEDULE => "shares_cron_schedule",
        x if x == READS_CONFIG => "reads_config",
        x if x == DEFINES_CONFIG => "defines_config",
        x if x == SHARES_CONFIG => "shares_config",
        x if x == INFRA_REFERENCES => "infra_references",
        x if x == SHARES_INFRA_REF => "shares_infra_ref",
        x if x == DEPENDS_ON => "depends_on",
        x if x == SHARES_DEPENDENCY => "shares_dependency",
        _ => "?",
    }
}
