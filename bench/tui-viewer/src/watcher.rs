//! File watcher — debounced reload signal on source-tree changes.
//!
//! We watch the repo root recursively. On any modify/create/remove, we send
//! a unit to the channel. The main loop coalesces multiple signals per frame.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(500);

pub fn spawn(root: &Path, tx: Sender<()>) -> Result<RecommendedWatcher> {
    let mut last = Instant::now() - DEBOUNCE;
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if !matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        // Filter out target/ + .git/ + .glia/ noise so the editor save loop
        // doesn't trigger a rebuild for cargo / git / build artefacts.
        if ev.paths.iter().any(|p| {
            p.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                matches!(s.as_ref(), "target" | ".git" | ".glia" | "node_modules")
            })
        }) {
            return;
        }
        let now = Instant::now();
        if now.duration_since(last) < DEBOUNCE {
            return;
        }
        last = now;
        let _ = tx.send(());
    })?;
    watcher.configure(Config::default().with_poll_interval(Duration::from_secs(1)))?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}
