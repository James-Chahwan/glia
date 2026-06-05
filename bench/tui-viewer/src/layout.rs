//! TUI layout: search + header + (tree | code) + info + footer.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct Frame {
    pub search: Rect,
    pub header: Rect,
    pub tree: Rect,
    pub code: Rect,
    pub info: Rect,
    pub footer: Rect,
}

pub fn split(area: Rect) -> Frame {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // search
            Constraint::Length(2),  // header
            Constraint::Min(8),     // main: tree + code side-by-side
            Constraint::Length(6),  // info (graph metadata)
            Constraint::Length(2),  // footer
        ])
        .split(area);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(outer[2]);

    Frame {
        search: outer[0],
        header: outer[1],
        tree: main[0],
        code: main[1],
        info: outer[3],
        footer: outer[4],
    }
}
