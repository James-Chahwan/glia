//! Scrollable tree renderer + cell/code panel.
//!
//! Replaces the 4-column layout with a single tree that groups nodes by
//! tier → namespace prefix → node, collapsible at any group level.

use std::collections::HashMap;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use repo_graph_code_domain::cell_type;
use repo_graph_core::{CellPayload, NodeId};

use crate::state::{AppState, TreeRow};
use crate::style::{glyph, heat_glyph, kind_label};

/// Render the tree pane. Mode-specific extras come via `tags` (e.g. layer
/// tag for flow mode like "[2]" prepended to the line) and `seed_set`
/// (highlight as ★).
pub fn render(
    f: &mut Frame,
    area: Rect,
    app: &mut AppState,
    rows: &[TreeRow],
    tags: &HashMap<NodeId, String>,
    seed_set: &std::collections::HashSet<NodeId>,
) {
    let viewport = area.height.saturating_sub(2) as usize;
    app.clamp_tree(rows.len(), viewport);

    let mut lines: Vec<Line> = Vec::with_capacity(viewport);
    let scroll = app.tree_scroll;
    let cursor = app.tree_cursor;

    for (visible_idx, abs_idx) in (scroll..rows.len()).take(viewport).enumerate() {
        let _ = visible_idx;
        let row = &rows[abs_idx];
        let is_cursor = abs_idx == cursor;
        lines.push(render_row(row, app, tags, seed_set, is_cursor, area.width));
    }

    let title = format!(
        " tree  ({}/{} rows · ↑↓ scroll · space collapse) ",
        cursor + 1,
        rows.len()
    );
    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(widget, area);
}

fn render_row<'a>(
    row: &'a TreeRow,
    app: &'a AppState,
    tags: &HashMap<NodeId, String>,
    seed_set: &std::collections::HashSet<NodeId>,
    is_cursor: bool,
    width: u16,
) -> Line<'a> {
    let cursor_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let prefix = if is_cursor { "▶ " } else { "  " };

    match row {
        TreeRow::Tier { label, count, expanded } => {
            let chevron = if *expanded { "▾" } else { "▸" };
            let style = if is_cursor {
                cursor_style
            } else {
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
            };
            Line::from(vec![Span::styled(
                format!("{}{} {} ({})", prefix, chevron, label, count),
                style,
            )])
        }
        TreeRow::Group { label, count, expanded, .. } => {
            let chevron = if *expanded { "▾" } else { "▸" };
            let style = if is_cursor {
                cursor_style
            } else {
                Style::default().fg(Color::Yellow)
            };
            Line::from(vec![Span::styled(
                format!("{}  {} {} ({})", prefix, chevron, label, count),
                style,
            )])
        }
        TreeRow::Node { id, score, .. } => {
            let meta = app.meta.get(id);
            let kind = meta.map(|m| m.kind).unwrap_or(repo_graph_core::NodeKindId(0));
            let name = meta.map(|m| m.name.as_str()).unwrap_or("");
            let heat = heat_glyph(*score);
            let kg = glyph(kind);
            let tag = tags.get(id).cloned().unwrap_or_default();
            let is_seed = seed_set.contains(id);

            let star = if is_seed { "★ " } else { "" };
            let trimmed = truncate(
                name,
                width.saturating_sub(16 + tag.len() as u16) as usize,
            );
            let body = format!(
                "{}    {}{} {}{} {:.2}  {}",
                prefix,
                star,
                heat,
                kg,
                if tag.is_empty() { String::new() } else { format!(" {}", tag) },
                score,
                trimmed
            );
            let style = if is_cursor {
                cursor_style
            } else if is_seed {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if *score > 0.4 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![Span::styled(body, style)])
        }
    }
}

/// Render the cell/code pane — the actual content of the selected node.
pub fn code_pane(f: &mut Frame, area: Rect, app: &AppState, selected: Option<NodeId>) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(id) = selected {
        // Find the live Node by id across all repo graphs.
        let mut node_ref = None;
        for g in &app.graph.graphs {
            if let Some(n) = g.nodes.iter().find(|n| n.id == id) {
                node_ref = Some(n);
                break;
            }
        }
        let Some(node) = node_ref else {
            lines.push(Line::raw("  (node not found in graph)"));
            render_pane(f, area, lines);
            return;
        };

        if let Some(meta) = app.meta.get(&id) {
            lines.push(Line::from(vec![Span::styled(
                format!("► {} {}", glyph(meta.kind), kind_label(meta.kind)),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )]));
        }

        if node.cells.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::raw("  (no cells — graph node carries no payload)"));
        } else {
            for cell in &node.cells {
                let label = cell_label(cell.kind);
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![Span::styled(
                    format!("  {} ──", label),
                    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                )]));
                match &cell.payload {
                    CellPayload::Text(t) => {
                        for ln in t.lines().take(40) {
                            lines.push(Line::from(Span::raw(format!("    {}", ln))));
                        }
                        if t.lines().count() > 40 {
                            lines.push(Line::from(Span::styled(
                                format!("    … ({} more lines)", t.lines().count() - 40),
                                Style::default().add_modifier(Modifier::DIM),
                            )));
                        }
                    }
                    CellPayload::Json(j) => {
                        for ln in j.lines().take(20) {
                            lines.push(Line::from(Span::styled(
                                format!("    {}", ln),
                                Style::default().fg(Color::Green),
                            )));
                        }
                    }
                    CellPayload::Bytes(b) => {
                        lines.push(Line::from(Span::styled(
                            format!("    <{} bytes>", b.len()),
                            Style::default().add_modifier(Modifier::DIM),
                        )));
                    }
                }
            }
        }
    } else {
        lines.push(Line::raw("  (move cursor onto a node to see its cells)"));
    }
    render_pane(f, area, lines);
}

fn render_pane(f: &mut Frame, area: Rect, lines: Vec<Line>) {
    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" cells / code "))
        .wrap(Wrap { trim: false });
    f.render_widget(widget, area);
}

fn cell_label(k: repo_graph_core::CellTypeId) -> &'static str {
    match k {
        x if x == cell_type::CODE => "CODE",
        x if x == cell_type::DOC => "DOC",
        x if x == cell_type::POSITION => "POSITION",
        x if x == cell_type::INTENT => "INTENT",
        x if x == cell_type::ROUTE_METHOD => "ROUTE_METHOD",
        x if x == cell_type::ENDPOINT_HIT => "ENDPOINT_HIT",
        x if x == cell_type::TEST => "TEST",
        x if x == cell_type::ATTN => "ATTN",
        x if x == cell_type::FAIL => "FAIL",
        x if x == cell_type::CONSTRAINT => "CONSTRAINT",
        x if x == cell_type::DECISION => "DECISION",
        x if x == cell_type::ENV => "ENV",
        x if x == cell_type::CONV => "CONV",
        x if x == cell_type::VECTOR => "VECTOR",
        _ => "CELL",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
