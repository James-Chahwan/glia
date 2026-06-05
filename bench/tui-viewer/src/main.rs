//! glia-tui — TUI tester for the glia engine.
//!
//! Three modes (`--mode`):
//!   ppr    Personalised PageRank pulse animation (default).
//!   flow   Replay a flow trace (entry → handler → data).
//!   build  Replay parse → extract → resolve as the graph materialises.
//!
//! Watches the repo with notify-rs and rebuilds on change.

mod state;
mod watcher;
mod layout;
mod panels;
mod tree;
mod style;
mod modes;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::state::{AppState, Mode};

#[derive(Parser, Debug)]
#[command(name = "glia-tui", version, about = "TUI tester for the glia engine")]
struct Cli {
    /// Repo root to analyse + watch.
    repo: PathBuf,

    /// Animation mode.
    #[arg(long, value_enum, default_value_t = ModeArg::Ppr)]
    mode: ModeArg,

    /// Optional seed qname for PPR mode (e.g. `app::routes::groups`).
    #[arg(long)]
    seed: Option<String>,

    /// Frame rate (frames per second). Default 12.
    #[arg(long, default_value_t = 12)]
    fps: u32,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ModeArg {
    Ppr,
    Flow,
    Build,
}

impl From<ModeArg> for Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Ppr => Mode::Ppr,
            ModeArg::Flow => Mode::Flow,
            ModeArg::Build => Mode::Build,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = cli.repo.canonicalize().context("repo path")?;

    let (reload_tx, reload_rx) = mpsc::channel();
    let _watcher = watcher::spawn(&repo, reload_tx).context("file watcher")?;

    let mut state = AppState::new(repo.clone(), cli.mode.into(), cli.seed)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let frame_dur = Duration::from_millis((1000 / cli.fps.max(1)).into());
    let mut last_frame = Instant::now();
    let result = run_loop(&mut terminal, &mut state, &reload_rx, frame_dur, &mut last_frame);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    reload_rx: &mpsc::Receiver<()>,
    frame_dur: Duration,
    last_frame: &mut Instant,
) -> Result<()> {
    loop {
        // Drain pending reload events; if any, rebuild graph.
        while reload_rx.try_recv().is_ok() {
            state.reload()?;
        }

        let elapsed = last_frame.elapsed();
        if elapsed >= frame_dur {
            state.tick();
            *last_frame = Instant::now();
        }

        terminal.draw(|f| state.render(f))?;

        // Poll for input up to the next frame deadline so we stay smooth.
        let timeout = frame_dur.saturating_sub(last_frame.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if state.query_focused {
                    // Search-mode key handling.
                    match key.code {
                        KeyCode::Esc => {
                            state.query_clear();
                            state.query_focus(false);
                        }
                        KeyCode::Enter => state.query_focus(false),
                        KeyCode::Backspace => state.query_pop(),
                        KeyCode::Char(c) => state.query_push(c),
                        _ => {}
                    }
                } else {
                    // Normal key handling.
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('/') => state.query_focus(true),
                        KeyCode::Char('r') => state.reload()?,
                        KeyCode::Char('1') => state.set_mode(Mode::Ppr),
                        KeyCode::Char('2') => state.set_mode(Mode::Flow),
                        KeyCode::Char('3') => state.set_mode(Mode::Build),
                        KeyCode::Char('p') => state.toggle_pause(),
                        KeyCode::Char(' ') => state.toggle_collapsed_at_cursor(),
                        KeyCode::Char('n') => state.next_seed(),
                        KeyCode::Char('c') => state.query_clear(),
                        KeyCode::Up | KeyCode::Char('k') => state.move_cursor(-1),
                        KeyCode::Down | KeyCode::Char('j') => state.move_cursor(1),
                        KeyCode::PageUp => state.move_cursor(-10),
                        KeyCode::PageDown => state.move_cursor(10),
                        KeyCode::Home => state.move_cursor(-1_000_000),
                        KeyCode::End => state.move_cursor(1_000_000),
                        KeyCode::Enter => state.use_selected_as_seed(),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}
