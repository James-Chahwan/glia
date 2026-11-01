//! `glia watch` (DC.8): a foreground loop over `engine::watch::WatchSession`
//! printing one NDJSON object per update on stdout. No daemon, socket or
//! pidfile: the process dies with its terminal or its parent. It watches one
//! repo, or (GA.11) the repo and every `--with` repo as one joint build
//! (`WatchSession::open_many`, in argument order, as `generate_many` builds
//! them).
//!
//! stdout, one JSON object per line: first the ready line
//! `{"ready":true,"seq":0,"repo":<path>,"total_nodes":N,"total_edges":E}`
//! (with `--with`, `"repos":[<path>,..]` after `repo`: every root as given,
//! in build order) once the session is open (the signal handlers are in place
//! by then), then
//! one `WatchUpdate` per update that changed the graph (`seq` moved: a
//! touched file or a re-plan that found nothing new prints nothing), its
//! `nodes` / `edges` rows in the delta's order (`delta::located_delta_*`). A
//! full rebuild is one line too, however many rows it carries. Under `--with`
//! the update names each file `<root>:<rel>` and each rebuilt group
//! `<root>:<key>` (the engine's multi-repo naming).
//!
//! The loop. Every `--poll-ms` (at least 10; slept in 10 ms slices that check
//! the stop flags) it runs the session's stat scan (`changed_paths`) and adds
//! what it reports to a pending set, stat'ing each pending path itself. The
//! scan keeps reporting a path until an update reads it, so a non-empty scan
//! is not activity: a tick is active when the pending set grew or a pending
//! path's (len, mtime) moved since the tick before. Once the set is non-empty
//! and no tick was active for `--debounce-ms`, it is applied as one update
//! and cleared. GA.11, the max-latency cap: a set that has been non-empty for
//! `--max-latency-ms` (default 2000) is applied at that tick even when it is
//! still moving, so a path rewritten every tick (a log, a build output under
//! the tree) can no longer hold every other change back for ever; a path that
//! moves again after the update is simply pending again.
//!
//! Persisting. The layout (`<repo>/.glia/graph`, writer `watch`; the
//! `--out <dir>` when given) and the parse cache are written on exit, always
//! (the store skips unchanged shards), and every `--persist-every` seconds (0: only on exit) while the
//! graph holds something the last write did not: the opened build, or an
//! update since. An interval write waits for an empty pending set and runs
//! right after that tick's scan and update, so the graph it writes matches
//! the disk (CD.7c stores a CODE cell as a span only when the file still
//! matches its text). `GLIA_NO_PERSIST` is not read: like `glia build`,
//! writing the layout is what the command is for.
//!
//! `--with` needs `--out` (exit 2 before anything is read: `error: --with
//! needs --out; a joint layout never overwrites a repo's own .glia/graph`):
//! a repo's own `.glia/graph` is its single-repo layout, which the MCP and the
//! hooks read, so the joint layout goes only to the dir named. Each repo's
//! parse cache is still saved beside its own root (`WatchSession::save_cache`,
//! the sidecar a single-repo build of that repo reuses).
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
//! - `[watch] listening repo=<path> poll_ms=<p> debounce_ms=<d>
//!   max_latency_ms=<m> persist_every=<s>` (with `--with`, ` repos=<n>`
//!   after it, the first repo included) once, right before the ready line;
//! - `[watch] emitted seq=<s> nodes=<n> edges=<e> bytes=<b> rss_mb=<m>` per
//!   update line printed: its node and edge rows, the line's bytes and the
//!   process's resident set after it (`VmRSS`; `?` where `/proc/self/status`
//!   does not give it), the series a long watch's memory is read from;
//! - `[watch] persisted seq=<s> reason=interval|exit` (with `--out`,
//!   ` out=<dir>` after it, the dir as given);
//! - the engine's `[watch] rss ..` line (GA.12) at open, per update and once
//!   more when `--memory-budget-mb` replaces the budget the session read from
//!   `GLIA_MEMORY_BUDGET_MB` (`0`: no budget);
//! - `[watch] exit reason=sigint|sigterm|stdin_eof|stdout_closed`, once the
//!   loop stops, before the exit persist;
//! - GA.15, under `--embed`: `[watch] embed seq=<s> embedded=<e> cached=<c>
//!   ms=<t>` once per update line printed, after it (below).
//!
//! Embedding (GA.15). `--embed` keeps `<repo>/.glia/vectors.jsonl` (FB.5a's
//! sidecar, the one `glia embed` writes) up to date as the watch runs: after
//! each update line is printed, and before the next scan, the loop thread runs
//! `semantic::embed_repo` over the session's current graph for every watched
//! repo in build order, at most 2000 nodes a run (the rest wait for the next
//! update): the nodes the update changed and any still without a fresh
//! vector. One embed per update line, never per file, so the debounce that
//! batches an update batches its embed; the stdout stream is unchanged (the
//! embed prints only its stderr line, `embedded` / `cached` / `ms` summed over
//! the repos). It runs on the loop thread after `apply` has returned, so the
//! model never shares the cores with the build's rayon pool, and its sidecar
//! write is a query-time input the scan drops (`watch::durability`, FB.3), so
//! it starts no update. The model is `--embed-model`, else
//! `GLIA_EMBED_MODEL` (`semantic::surface_model`, bge-small by default),
//! loaded once after the session opens; before the listening line, a model
//! cache directory inside a watched repo (bge-small) or a sidecar holding
//! another model's vectors exits 2 (`glia embed --replace` rewrites one). An
//! embed that fails later prints `warning: watch: embed seq=<s>: <reason>`
//! and the watch goes on. Vectors are not in the layout, so the persist
//! cadence is unchanged. The model session lives for one run: it is dropped
//! after it and the freed heap handed back (`[memory] malloc_trim
//! site=watch_embed ..`), the next run loading the model again (a disk read,
//! no download), because a resident session keeps its working memory:
//! measured on glia's own tree, a CPU bge-small session held after its first
//! 2000-node run left the watch at 16 GB RSS against 0.9 GB before, which no
//! `--memory-budget-mb` can evict. While a run lasts its memory is the
//! process's (a bge-small GPU session lives in its `embed-worker` child
//! instead). A stop signal that arrives during a run is honoured once the run
//! returns: an embed is never cut short.
//!
//! Grep them from a run: `glia watch <repo> 2>&1 >/dev/null | grep '^\[watch\] '`.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use clap::builder::PossibleValuesParser;
use glia_code_domain::external_inputs::{VectorRow, read_rows};
use glia_core::RepoId;
use glia_embed::Embedder;
use glia_engine::memory::{MemoryBudget, release_freed_memory, rss_kb};
use glia_engine::persist::default_layout_dir;
use glia_engine::semantic::{EmbedOptions, VECTORS_FILE, embed_repo, surface_model};
use glia_engine::watch::{WatchSession, WatchUpdate};

use crate::common::build_options;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Another repo to watch with this one as one joint build (repeatable,
    /// in build order); needs --out.
    #[arg(long = "with", value_name = "REPO")]
    with: Vec<String>,
    /// Layout directory to persist to (default `<repo>/.glia/graph`);
    /// required with --with, so a joint layout never overwrites a repo's own.
    #[arg(long, value_name = "DIR")]
    out: Option<String>,
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
    /// Milliseconds a pending change may wait: once the pending set is this
    /// old it is applied even while a path keeps moving.
    #[arg(long, value_name = "N", default_value_t = 2000)]
    max_latency_ms: u64,
    /// Process RSS budget in MB for the session's caches (cached language
    /// group builds, build memos), evicted past it; overrides
    /// GLIA_MEMORY_BUDGET_MB (0: no budget).
    #[arg(long, value_name = "MB")]
    memory_budget_mb: Option<u64>,
    /// After each update, embed the changed and un-embedded nodes (at most
    /// 2000 a run) into `<repo>/.glia/vectors.jsonl`, as `glia embed` does.
    #[arg(long)]
    embed: bool,
    /// The embedding model --embed uses (default bge-small, or
    /// GLIA_EMBED_MODEL).
    #[arg(long, value_name = "MODEL", requires = "embed", value_parser = PossibleValuesParser::new(glia_embed::MODEL_NAMES))]
    embed_model: Option<String>,
}

/// GA.15: the most nodes one post-update embed run embeds.
const EMBED_LIMIT: usize = 2000;

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
    std::fs::metadata(p)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

/// The process's resident set in MB (`VmRSS` of `/proc/self/status`,
/// `memory::rss_kb`), `?` where that file does not give it.
fn rss_mb() -> String {
    rss_kb().map_or_else(|| "?".to_string(), |kb| (kb / 1024).to_string())
}

/// How a step of the loop failed: the exit code and its message.
struct Fail(i32, String);

/// GA.15: the `--embed` half of the loop: the model, loaded once.
struct Embed {
    /// The `glia_embed::named` model name, reloaded for each run.
    model: &'static str,
    /// The embedder [`Embed::open`] loaded, used by the first run; `None`
    /// after it (each later run loads its own).
    loaded: Option<Box<dyn Embedder>>,
    opts: EmbedOptions,
}

impl Embed {
    /// Load `model` after checking every watched root (module doc,
    /// "Embedding"): the bge-small cache must sit outside each repo (checked
    /// before the model loads), and no sidecar may hold another model's
    /// vectors.
    fn open(model: &'static str, roots: &[PathBuf]) -> Result<Embed, String> {
        if model == "bge-small" {
            for root in roots {
                glia_embed::refuse_repo_cache(root)?;
            }
        }
        let embedder = crate::common::load_embedder(model)?;
        for root in roots {
            refuse_foreign_sidecar(root, embedder.model())?;
        }
        Ok(Embed {
            model,
            loaded: Some(embedder),
            opts: EmbedOptions::default().with_limit(Some(EMBED_LIMIT)),
        })
    }

    /// Embed every watched repo of `session`'s current graph, in build order;
    /// one `[watch] embed` line for update `seq`, or a warning when a run
    /// failed (the watch goes on). The model session lives for this run
    /// only: dropped after it, and the freed heap handed back
    /// (`memory::release_freed_memory("watch_embed")`), so the watch does not
    /// hold the model's working memory between updates (module doc).
    fn run(&mut self, session: &WatchSession, seq: u64) {
        let embedder = match self.loaded.take() {
            Some(e) => e,
            None => match crate::common::load_embedder(self.model) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("warning: watch: embed seq={seq}: {e}");
                    return;
                }
            },
        };
        self.embed_with(embedder.as_ref(), session, seq);
        drop(embedder);
        release_freed_memory("watch_embed");
    }

    fn embed_with(&self, embedder: &dyn Embedder, session: &WatchSession, seq: u64) {
        let started = Instant::now();
        let result = session.result();
        let (mut embedded, mut cached) = (0usize, 0usize);
        for root in session.roots() {
            let Some(id) = result
                .repo_roots
                .iter()
                .find_map(|(id, r)| (r == root).then_some(*id))
            else {
                eprintln!("warning: watch: embed seq={seq}: no repo id for {root}");
                continue;
            };
            match embed_repo(
                Path::new(root),
                &result.merged,
                RepoId(id),
                embedder,
                &self.opts,
            ) {
                Ok(r) => {
                    embedded += r.embedded;
                    cached += r.cached;
                }
                Err(e) => {
                    eprintln!("warning: watch: embed seq={seq}: {e}");
                    return;
                }
            }
        }
        eprintln!(
            "[watch] embed seq={seq} embedded={embedded} cached={cached} ms={}",
            started.elapsed().as_millis()
        );
    }
}

/// Err when `<root>/.glia/vectors.jsonl` holds a row of a model other than
/// `model` (or one naming none): the refusal `embed_repo` would give at the
/// first update, given before the watch starts listening.
fn refuse_foreign_sidecar(root: &Path, model: &str) -> Result<(), String> {
    let path = root.join(VECTORS_FILE);
    let (rows, errors) = read_rows::<VectorRow>(&path);
    if let Some(first) = errors.first() {
        return Err(format!(
            "{} has {} unreadable line(s) (first: {first}); fix or remove them before embedding",
            path.display(),
            errors.len()
        ));
    }
    match rows.iter().find(|r| r.model.as_deref() != Some(model)) {
        Some(other) => Err(format!(
            "{} holds vectors of model {}; run `glia embed --replace` to rewrite it with {model}",
            path.display(),
            other.model.as_deref().unwrap_or("<none>")
        )),
        None => Ok(()),
    }
}

/// The watch's state between ticks.
struct Loop {
    session: WatchSession,
    /// Paths the scan reported that no update has read yet.
    pending: BTreeMap<PathBuf, Stat>,
    /// When a tick last saw activity (the pending set grew or moved).
    last_activity: Instant,
    /// GA.11: when the pending set last went from empty to non-empty (`None`
    /// while it is empty): the max-latency cap's clock.
    pending_since: Option<Instant>,
    /// The graph holds something the last persist did not write.
    dirty: bool,
    last_persist: Instant,
    layout: PathBuf,
    /// The `--out` dir as given, for the persisted marker (`None`: the
    /// repo's default layout).
    out: Option<String>,
    /// GA.15: `--embed`'s model, run after each update line printed.
    embed: Option<Embed>,
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
        if !self.pending.is_empty() && self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
        active
    }

    /// GA.11: whether the pending set is due: non-empty, and either still
    /// for `debounce` or pending for `max_latency` (the cap).
    fn due(&self, debounce: Duration, max_latency: Duration) -> bool {
        !self.pending.is_empty()
            && (self.last_activity.elapsed() >= debounce
                || self
                    .pending_since
                    .is_some_and(|t| t.elapsed() >= max_latency))
    }

    /// Apply the pending set as one update and print it when `seq` moved.
    /// `Ok(false)` when stdout is closed.
    fn apply(&mut self) -> Result<bool, Fail> {
        if self.pending.is_empty() {
            return Ok(true);
        }
        let paths: Vec<PathBuf> = std::mem::take(&mut self.pending).into_keys().collect();
        self.pending_since = None;
        let seq = self.session.seq();
        let update = self.session.apply(&paths).map_err(|e| Fail(2, e))?;
        if update.seq == seq {
            return Ok(true);
        }
        self.dirty = true;
        let open = emit(&update)?;
        if open && let Some(embed) = &mut self.embed {
            embed.run(&self.session, update.seq);
        }
        Ok(open)
    }

    /// Write the layout and the parse cache.
    fn persist(&mut self, reason: &str) -> Result<(), Fail> {
        self.session.persist(&self.layout).map_err(|e| Fail(5, e))?;
        self.session
            .save_cache()
            .map_err(|e| Fail(5, format!("watch: save the parse cache: {e}")))?;
        self.dirty = false;
        self.last_persist = Instant::now();
        let out = self
            .out
            .as_deref()
            .map_or_else(String::new, |o| format!(" out={o}"));
        eprintln!(
            "[watch] persisted seq={} reason={reason}{out}",
            self.session.seq()
        );
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
    if !a.with.is_empty() && a.out.is_none() {
        eprintln!(
            "error: --with needs --out; a joint layout never overwrites a repo's own .glia/graph"
        );
        return 2;
    }
    let roots: Vec<String> = std::iter::once(a.repo.clone())
        .chain(a.with.iter().cloned())
        .collect();
    let mut session = match WatchSession::open_many(&roots, &opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Some(mb) = a.memory_budget_mb {
        session.set_memory_budget(MemoryBudget::mb(mb));
    }
    let embed = if a.embed {
        // The parser admits only `MODEL_NAMES`, so the name is found there.
        let model = a
            .embed_model
            .as_deref()
            .and_then(|m| glia_embed::MODEL_NAMES.iter().copied().find(|n| *n == m))
            .unwrap_or_else(surface_model);
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        match Embed::open(model, &roots) {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    } else {
        None
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
    let max_latency = Duration::from_millis(a.max_latency_ms);
    let interval = Duration::from_secs(a.persist_every);
    let mut w = Loop {
        layout: a
            .out
            .as_ref()
            .map_or_else(|| default_layout_dir(Path::new(&a.repo)), PathBuf::from),
        out: a.out.clone(),
        pending: BTreeMap::new(),
        last_activity: Instant::now(),
        pending_since: None,
        // The opened build is not on disk as far as this process knows.
        dirty: true,
        last_persist: Instant::now(),
        session,
        embed,
    };
    let multi = !a.with.is_empty();
    let repos_field = if multi {
        format!(" repos={}", roots.len())
    } else {
        String::new()
    };
    eprintln!(
        "[watch] listening repo={} poll_ms={} debounce_ms={} max_latency_ms={} persist_every={}{repos_field}",
        a.repo,
        poll.as_millis(),
        a.debounce_ms,
        a.max_latency_ms,
        a.persist_every
    );
    let result = w.session.result();
    let repos_json = if multi {
        let listed = serde_json::Value::from(w.session.roots());
        format!(",\"repos\":{listed}")
    } else {
        String::new()
    };
    let ready = format!(
        "{{\"ready\":true,\"seq\":{},\"repo\":{}{repos_json},\"total_nodes\":{},\"total_edges\":{}}}",
        w.session.seq(),
        serde_json::Value::String(a.repo.clone()),
        result.total_nodes,
        result.total_edges
    );
    let reason = if print_line(&ready) {
        match watch_loop(&mut w, poll, debounce, max_latency, interval) {
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
    max_latency: Duration,
    interval: Duration,
) -> Result<&'static str, Fail> {
    loop {
        if let Some(r) = nap(poll) {
            return Ok(r);
        }
        if w.scan() {
            w.last_activity = Instant::now();
        }
        if w.due(debounce, max_latency) && !w.apply()? {
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
