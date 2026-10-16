//! `glia watch` (DC.8): a foreground loop over `engine::watch::WatchSession`
//! printing one NDJSON object per update on stdout. No daemon, socket or
//! pidfile: the process dies with its terminal or its parent, and watches one
//! repo.
//!
//! stdout, one JSON object per line: first the ready line
//! `{"ready":true,"seq":0,"repo":<path>,"total_nodes":N,"total_edges":E}`
//! once the session is open (the signal handlers are in place by then), then
//! one `WatchUpdate` per update that changed the graph (`seq` moved: a
//! touched file or a re-plan that found nothing new prints nothing), its
//! `nodes` / `edges` rows in the delta's order (`delta::located_delta_*`). A
//! full rebuild is one line too, however many rows it carries.
//!
//! The loop. Every `--poll-ms` (at least 10; slept in 10 ms slices that check
//! the stop flags) it runs the session's stat scan (`changed_paths`) and adds
//! what it reports to a pending set, stat'ing each pending path itself. The
//! scan keeps reporting a path until an update reads it, so a non-empty scan
//! is not activity: a tick is active when the pending set grew or a pending
//! path's (len, mtime) moved since the tick before. Once the set is non-empty
//! and no tick was active for `--debounce-ms`, it is applied as one update
//! and cleared.
//!
//! Persisting. The layout (`<repo>/.glia/graph`, writer `watch`) and the
//! parse cache are written on exit, always (the store skips unchanged
//! shards), and every `--persist-every` seconds (0: only on exit) while the
//! graph holds something the last write did not: the opened build, or an
//! update since. An interval write waits for an empty pending set and runs
//! right after that tick's scan and update, so the graph it writes matches
//! the disk (CD.7c stores a CODE cell as a span only when the file still
//! matches its text). `GLIA_NO_PERSIST` is not read: like `glia build`,
//! writing the layout is what the command is for.
//!
//! Stopping. SIGINT and SIGTERM (unix: `signal(2)` through the libc std
//! links, the handler only stores the signal number into an atomic), stdin
//! reaching EOF (a detached thread reads it to the end: a parent that exits
//! or closes the pipe ends the watch, and one that runs it with stdin =
//! /dev/null gets an immediate clean exit, by design), or stdout closing
//! under a write. Each one ends the loop the same way: the pending set and a
//! last scan's paths applied, the layout and cache persisted, exit 0. On
//! other platforms no handler is installed: Ctrl-C ends the process without
//! the exit persist; stdin EOF still persists.
//!
//! `--no-overlay` is refused (exit 2) before anything is read or written:
//! the default layout dir holds the overlay-applied graph, the rule `glia
//! build` applies without `--out`. Exits 0 on a stop, 2 when the repo is not a
//! directory, cannot be opened or stops being one, 5 when a persist fails
//! (`glia build`'s code).
//!
//! fired_on markers, on stderr (besides the engine's `[watch] open` /
//! `update` / `noop` / `poll` lines and the stages an update runs):
//! - `[watch] listening repo=<path> poll_ms=<p> debounce_ms=<d> persist_every=<s>`
//!   once, right before the ready line;
//! - `[watch] emitted seq=<s> nodes=<n> edges=<e> bytes=<b> rss_mb=<m>` per
//!   update line printed: its node and edge rows, the line's bytes and the
//!   process's resident set after it (`VmRSS`; `?` where `/proc/self/status`
//!   does not give it), the series a long watch's memory is read from;
//! - `[watch] persisted seq=<s> reason=interval|exit`;
//! - `[watch] exit reason=sigint|sigterm|stdin_eof|stdout_closed`, once the
//!   loop stops, before the exit persist.
//!
//! Grep them from a run: `glia watch <repo> 2>&1 >/dev/null | grep '^\[watch\] '`.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use glia_engine::persist::default_layout_dir;
use glia_engine::watch::{WatchSession, WatchUpdate};

use crate::common::build_options;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Persist the layout and parse cache every this many seconds while the
    /// graph holds unsaved updates (0: only on exit).
    #[arg(long, value_name = "S", default_value_t = 60)]
    persist_every: u64,
    /// Milliseconds between two stat scans of the tree (at least 10).
    #[arg(long, value_name = "N", default_value_t = 500)]
    poll_ms: u64,
    /// Milliseconds the pending changes must stay still before they are
    /// applied as one update.
    #[arg(long, value_name = "N", default_value_t = 200)]
    debounce_ms: u64,
}

/// The shortest `--poll-ms` honoured, and the slice every wait is cut into
/// so a stop flag is seen within it.
const SLICE_MS: u64 = 10;

/// The unix signal numbers the watch stops on (the same on every unix).
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// The first stop signal caught (0: none yet). Written by the signal handler,
/// read by the loop.
static SIGNAL: AtomicI32 = AtomicI32::new(0);
/// Set by the stdin reader once stdin reached EOF (or failed).
static STDIN_EOF: AtomicBool = AtomicBool::new(false);

/// The FFI half: `signal(2)` from the libc std already links on unix, so no
/// signal crate (and no Cargo.lock change). This block is the CLI's only
/// `unsafe`.
#[cfg(unix)]
mod sig {
    use std::sync::atomic::Ordering;

    unsafe extern "C" {
        /// `sighandler_t signal(int, sighandler_t)`; the return is taken as
        /// an integer, since `SIG_DFL` (0) and `SIG_ERR` (-1) are no function.
        fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
    }

    /// `SIG_ERR`, `(sighandler_t) -1`.
    const SIG_ERR: usize = usize::MAX;

    /// Async-signal-safe: one lock-free atomic store, nothing else.
    extern "C" fn on_signal(signum: i32) {
        super::SIGNAL.store(signum, Ordering::SeqCst);
    }

    /// Install [`on_signal`] for SIGINT and SIGTERM; false when either
    /// install failed.
    pub(super) fn install() -> bool {
        [super::SIGINT, super::SIGTERM].iter().all(|&s| {
            // SAFETY: `signal` is the C library's, with its C signature; the
            // handler is an `extern "C" fn(i32)` that only stores into an
            // atomic, which is async-signal-safe.
            unsafe { signal(s, on_signal) != SIG_ERR }
        })
    }
}

/// No handler off unix (module doc): Ctrl-C ends the process unpersisted.
#[cfg(not(unix))]
mod sig {
    pub(super) fn install() -> bool {
        true
    }
}

/// Why the loop stopped, if it should: a signal first, then stdin's EOF.
fn stop_reason() -> Option<&'static str> {
    match SIGNAL.load(Ordering::SeqCst) {
        SIGINT => Some("sigint"),
        SIGTERM => Some("sigterm"),
        _ if STDIN_EOF.load(Ordering::SeqCst) => Some("stdin_eof"),
        _ => None,
    }
}

/// Read stdin to its end on a detached thread, then set [`STDIN_EOF`].
fn watch_stdin() -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("glia-watch-stdin".into())
        .spawn(|| {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            STDIN_EOF.store(true, Ordering::SeqCst);
        })
        .map(|_| ())
}

/// Sleep `total` in [`SLICE_MS`] slices; the stop reason as soon as one is
/// set.
fn nap(total: Duration) -> Option<&'static str> {
    let started = Instant::now();
    loop {
        if let Some(r) = stop_reason() {
            return Some(r);
        }
        let left = total.saturating_sub(started.elapsed());
        if left.is_zero() {
            return None;
        }
        std::thread::sleep(left.min(Duration::from_millis(SLICE_MS)));
    }
}

/// A pending path's stat as the CLI last saw it: `(len, mtime)`, `None` when
/// it is gone.
type Stat = Option<(u64, Option<SystemTime>)>;

fn stat(p: &Path) -> Stat {
    std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()))
}

/// The process's resident set in MB (`VmRSS` of `/proc/self/status`), `?`
/// where that file does not give it.
fn rss_mb() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .map_or_else(|| "?".to_string(), |kb| (kb / 1024).to_string())
}

/// How a step of the loop failed: the exit code and its message.
struct Fail(i32, String);

/// The watch's state between ticks.
struct Loop {
    session: WatchSession,
    /// Paths the scan reported that no update has read yet.
    pending: BTreeMap<PathBuf, Stat>,
    /// When a tick last saw activity (the pending set grew or moved).
    last_activity: Instant,
    /// The graph holds something the last persist did not write.
    dirty: bool,
    last_persist: Instant,
    layout: PathBuf,
}

impl Loop {
    /// One scan: merge its paths into the pending set; whether the tick was
    /// active.
    fn scan(&mut self) -> bool {
        let mut active = false;
        for (p, s) in self.pending.iter_mut() {
            let now = stat(p);
            if now != *s {
                *s = now;
                active = true;
            }
        }
        for p in self.session.changed_paths() {
            if let Entry::Vacant(e) = self.pending.entry(p) {
                let s = stat(e.key());
                e.insert(s);
                active = true;
            }
        }
        active
    }

    /// Apply the pending set as one update and print it when `seq` moved.
    /// `Ok(false)` when stdout is closed.
    fn apply(&mut self) -> Result<bool, Fail> {
        if self.pending.is_empty() {
            return Ok(true);
        }
        let paths: Vec<PathBuf> = std::mem::take(&mut self.pending).into_keys().collect();
        let seq = self.session.seq();
        let update = self.session.apply(&paths).map_err(|e| Fail(2, e))?;
        if update.seq == seq {
            return Ok(true);
        }
        self.dirty = true;
        emit(&update)
    }

    /// Write the layout and the parse cache.
    fn persist(&mut self, reason: &str) -> Result<(), Fail> {
        self.session.persist(&self.layout).map_err(|e| Fail(5, e))?;
        self.session
            .save_cache()
            .map_err(|e| Fail(5, format!("watch: save the parse cache: {e}")))?;
        self.dirty = false;
        self.last_persist = Instant::now();
        eprintln!("[watch] persisted seq={} reason={reason}", self.session.seq());
        Ok(())
    }
}

/// Print one update as one NDJSON line, flushed. `Ok(false)` when stdout is
/// closed (the reader went away).
fn emit(update: &WatchUpdate) -> Result<bool, Fail> {
    let line = serde_json::to_string(update)
        .map_err(|e| Fail(2, format!("watch: encode update {}: {e}", update.seq)))?;
    if !print_line(&line) {
        return Ok(false);
    }
    eprintln!(
        "[watch] emitted seq={} nodes={} edges={} bytes={} rss_mb={}",
        update.seq,
        update.nodes.len(),
        update.edges.len(),
        line.len() + 1,
        rss_mb()
    );
    Ok(true)
}

/// `line` + `\n` on a locked stdout, flushed; false when the write failed.
fn print_line(line: &str) -> bool {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").and_then(|()| out.flush()).is_ok()
}

pub(crate) fn run(a: Args) -> i32 {
    let opts = build_options();
    if !opts.overlay {
        eprintln!(
            "error: --no-overlay: watch persists into the default gmap dir, which holds the overlay-applied graph"
        );
        return 2;
    }
    let session = match WatchSession::open(&a.repo, &opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if !sig::install() {
        eprintln!(
            "warning: watch: SIGINT / SIGTERM handlers not installed; a signal ends the watch without the exit persist"
        );
    }
    if let Err(e) = watch_stdin() {
        eprintln!("error: watch: start the stdin reader: {e}");
        return 2;
    }
    let poll = Duration::from_millis(a.poll_ms.max(SLICE_MS));
    let debounce = Duration::from_millis(a.debounce_ms);
    let interval = Duration::from_secs(a.persist_every);
    let mut w = Loop {
        layout: default_layout_dir(Path::new(&a.repo)),
        pending: BTreeMap::new(),
        last_activity: Instant::now(),
        // The opened build is not on disk as far as this process knows.
        dirty: true,
        last_persist: Instant::now(),
        session,
    };
    eprintln!(
        "[watch] listening repo={} poll_ms={} debounce_ms={} persist_every={}",
        a.repo,
        poll.as_millis(),
        a.debounce_ms,
        a.persist_every
    );
    let result = w.session.result();
    let ready = format!(
        "{{\"ready\":true,\"seq\":{},\"repo\":{},\"total_nodes\":{},\"total_edges\":{}}}",
        w.session.seq(),
        serde_json::Value::String(a.repo.clone()),
        result.total_nodes,
        result.total_edges
    );
    let reason = if print_line(&ready) {
        match watch_loop(&mut w, poll, debounce, interval) {
            Ok(r) => r,
            Err(Fail(code, e)) => {
                eprintln!("error: {e}");
                return code;
            }
        }
    } else {
        "stdout_closed"
    };
    eprintln!("[watch] exit reason={reason}");
    match finish(&mut w) {
        Ok(()) => 0,
        Err(Fail(code, e)) => {
            eprintln!("error: {e}");
            code
        }
    }
}

/// Tick until a stop: the stop's reason.
fn watch_loop(
    w: &mut Loop,
    poll: Duration,
    debounce: Duration,
    interval: Duration,
) -> Result<&'static str, Fail> {
    loop {
        if let Some(r) = nap(poll) {
            return Ok(r);
        }
        if w.scan() {
            w.last_activity = Instant::now();
        }
        if !w.pending.is_empty() && w.last_activity.elapsed() >= debounce && !w.apply()? {
            return Ok("stdout_closed");
        }
        if !interval.is_zero()
            && w.dirty
            && w.pending.is_empty()
            && w.last_persist.elapsed() >= interval
        {
            w.persist("interval")?;
        }
    }
}

/// The exit: what is pending and what a last scan finds applied (an update
/// still printed while stdout is open), then the persist.
fn finish(w: &mut Loop) -> Result<(), Fail> {
    w.scan();
    w.apply()?;
    w.persist("exit")
}
