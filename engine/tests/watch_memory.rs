//! GA.12: the release RSS gate of a held [`WatchSession`] under the one memory
//! budget (`glia_engine::memory::MemoryBudget`). Linux only (it reads
//! `/proc/self/status`), `#[ignore]`d, its own `[[test]]` (build.rs `KEEP`):
//! it measures the process's resident set, which a neighbour test in the same
//! binary would move.
//!
//! Run it on a `git archive` copy (it edits files there, and restores them):
//! `GLIA_WATCH_MEM_REPO=<archive> cargo test -p glia-engine --release --test watch_memory -- --ignored --nocapture`.
//!
//! Three runs on the repo, one session each, 100 updates each: every update
//! appends one comment line to, or removes it from, one source file of a fixed
//! sorted list ([`TOUCHED`] files spread over the repo's sorted source files
//! of its most common [`COMMENTS`] extension, taken in turn), applied through
//! `WatchSession::apply`; the process's RSS is read after each update (after
//! its trim and the budget's evictions). One extension, so every update
//! rebuilds the same language group: post-trim RSS depends on the group an
//! update rebuilt (a large group's transient heap leaves its arenas more
//! fragmented), so a mix of groups makes the series swing with the touch order
//! instead of with what the session retains (the first run on glia, below).
//!
//! 1. Unbudgeted (`MemoryBudget::none`). Gate 1, no leak: the max over updates
//!    51-100 is at most 1.15 x the mean over updates 11-25. Its plateau is the
//!    mean over updates 51-100.
//! 2. Floor (`MemoryBudget::mb(1)`: every cached group and the build memos are
//!    evicted at every update). Its plateau is the floor; a floor at or above
//!    0.95 x the unbudgeted plateau means the eviction buys nothing and the
//!    test fails saying so.
//! 3. Budgeted at `max(0.85 x plateau, 1.05 x floor)` MB. Gate 2, the cap
//!    holds: the max over updates 51-100 is at most [`GATE2_MARGIN`] x the
//!    budget, and its plateau is below the unbudgeted one.
//!
//! Every update of runs 2 and 3 must carry the unbudgeted run's graph: equal
//! `total_nodes` / `total_edges` and the same delta rows (an eviction is a
//! cache drop, never a change of bytes).
//!
//! fired_on, on stdout, one line per run once all three ran:
//! `[watch-mem] repo=<r> run=<unbudgeted|floor|budgeted> updates=100 first_mb=<f> plateau_mb=<p> floor_mb=<l> max_mb=<m> budget_mb=<b|none> evictions=<e> ms_median=<t>`
//! (`max_mb` over updates 51-100, `evictions` the cached groups the budget
//! dropped in that run).
//!
//! Measured when this landed (GA.12; release build, 16 cores, default
//! threads, glibc trim on, other agents building alongside):
//! - glia at the landing base (`git archive`, 1,004 `.rs` of 2,434 source
//!   files; 27 language groups; 818 s for the three runs):
//!   `[watch-mem] repo=glia run=unbudgeted updates=100 first_mb=835 plateau_mb=940 floor_mb=779 max_mb=1032 budget_mb=none evictions=0 ms_median=2232.3`
//!   `[watch-mem] repo=glia run=floor updates=100 first_mb=702 plateau_mb=779 floor_mb=779 max_mb=834 budget_mb=1 evictions=2727 ms_median=2392.3`
//!   `[watch-mem] repo=glia run=budgeted updates=100 first_mb=800 plateau_mb=796 floor_mb=779 max_mb=854 budget_mb=819 evictions=2535 ms_median=2361.8`
//!   Gate 1: 1031 MB <= 1.15 x 913 MB (1,049). The floor is 0.83 x the
//!   plateau; gate 2: 854 MB <= 1.10 x 819 MB (901), plateau 796 < 940.
//!   Every evicting update costs about 7 % more time (median 2.39 s vs 2.23 s).
//!   A first run that touched ten files of every extension (five `.rs`, the
//!   rest small substrate-gap fixtures in four other groups) failed gate 1,
//!   999 MB > 1.15 x 843 MB: its RSS swung by the group each update rebuilt,
//!   ten-update cycles whose maxima peaked at 998 in the sixth and fell to 929
//!   by the ninth (no leak), and its gate 2 read 849 MB over a 766 MB budget
//!   (1.108). Hence one extension.
//! - twenty (the cohort's large case, ~2.3-2.5 GB held): a release-only
//!   measurement (about 15 min a run), left to the close-out.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};

use glia_engine::BuildOptions;
use glia_engine::memory::{MemoryBudget, release_freed_memory, rss_kb};
use glia_engine::watch::WatchSession;

/// The env var naming the repo copy the gate runs on.
const REPO_ENV: &str = "GLIA_WATCH_MEM_REPO";
/// Updates per run.
const UPDATES: usize = 100;
/// Source files the updates touch in turn.
const TOUCHED: usize = 10;
/// Gate 2's margin over the budget: the spec's 1.10, which a later run may
/// set from its measured noise, never looser than 1.20 (GA.12's risks). The
/// landing run held it (glia: max 854 MB over a 819 MB budget, 1.043).
const GATE2_MARGIN: f64 = 1.10;
/// Extensions of the files an update may touch, with their line comment.
const COMMENTS: &[(&str, &str)] = &[
    ("rs", "//"),
    ("go", "//"),
    ("ts", "//"),
    ("tsx", "//"),
    ("js", "//"),
    ("java", "//"),
    ("kt", "//"),
    ("c", "//"),
    ("cpp", "//"),
    ("py", "#"),
    ("rb", "#"),
];
/// Directories never descended into.
const SKIP_DIRS: &[&str] = &[".git", ".glia", "target", "node_modules", "vendor", "dist"];
/// The line an update appends (and the next touch of that file removes).
const MARK: &str = "glia watch-mem gate edit";

/// What one update left.
struct Step {
    rss_kb: u64,
    nodes: usize,
    edges: usize,
    /// A hash of the update's delta rows (nodes, then edges, as JSON).
    rows: u64,
    ms: f64,
}

/// One run: its updates and the cached groups its budget evicted.
struct Run {
    steps: Vec<Step>,
    evictions: u64,
}

impl Run {
    fn mb(&self, i: usize) -> f64 {
        self.steps.get(i).map_or(0.0, |s| s.rss_kb as f64 / 1024.0)
    }

    /// Mean RSS (MB) over updates `from..=to` (1-based).
    fn mean_mb(&self, from: usize, to: usize) -> f64 {
        let n = (to - from + 1) as f64;
        (from - 1..to).map(|i| self.mb(i)).sum::<f64>() / n
    }

    /// Max RSS (MB) over updates `from..=to` (1-based).
    fn max_mb(&self, from: usize, to: usize) -> f64 {
        (from - 1..to).map(|i| self.mb(i)).fold(0.0, f64::max)
    }

    fn ms_median(&self) -> f64 {
        let mut ms: Vec<f64> = self.steps.iter().map(|s| s.ms).collect();
        ms.sort_by(f64::total_cmp);
        ms.get(ms.len() / 2).copied().unwrap_or(0.0)
    }
}

/// Every source file under `root` with a [`COMMENTS`] extension, repo-relative
/// and sorted.
fn source_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(p);
                }
            } else if ft.is_file() && comment_of(&p).is_some() {
                let rel = p.strip_prefix(root).expect("under root");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

fn comment_of(p: &Path) -> Option<&'static str> {
    let ext = p.extension()?.to_str()?;
    COMMENTS.iter().find(|(e, _)| *e == ext).map(|(_, c)| *c)
}

/// [`TOUCHED`] files spread evenly over the sorted `all`, of the extension
/// most of them have (ties by [`COMMENTS`] order).
fn touched(all: &[String]) -> Vec<String> {
    let ext_of = |f: &String| comment_ext(Path::new(f));
    let count = |ext: &str| all.iter().filter(|f| ext_of(f) == Some(ext)).count();
    let best = COMMENTS
        .iter()
        .map(|(ext, _)| *ext)
        .fold(None::<(&str, usize)>, |best, ext| match best {
            Some((_, n)) if n >= count(ext) => best,
            _ => Some((ext, count(ext))),
        })
        .map(|(ext, _)| ext)
        .expect("COMMENTS is not empty");
    let of: Vec<&String> = all.iter().filter(|f| ext_of(f) == Some(best)).collect();
    assert!(of.len() >= TOUCHED, "{} .{best} files: too few", of.len());
    (0..TOUCHED)
        .map(|k| of[k * of.len() / TOUCHED].clone())
        .collect()
}

fn comment_ext(p: &Path) -> Option<&'static str> {
    let ext = p.extension()?.to_str()?;
    COMMENTS.iter().find(|(e, _)| *e == ext).map(|(e, _)| *e)
}

/// Append [`MARK`] as a comment line to `path`, or drop the one appended.
fn toggle(path: &Path) {
    let text = std::fs::read_to_string(path).expect("read a touched file");
    let comment = comment_of(path).expect("a touched file has a comment syntax");
    let line = format!("\n{comment} {MARK}\n");
    let next = match text.strip_suffix(&line) {
        Some(base) => base.to_string(),
        None => format!("{text}{line}"),
    };
    std::fs::write(path, next).expect("write a touched file");
}

fn rows_hash(nodes: &impl serde::Serialize, edges: &impl serde::Serialize) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_string(nodes).expect("rows").hash(&mut h);
    serde_json::to_string(edges).expect("rows").hash(&mut h);
    h.finish()
}

/// One session on `root` with `budget`, [`UPDATES`] updates over `files`.
fn run(root: &str, files: &[String], budget: MemoryBudget) -> Run {
    let mut s = WatchSession::open(root, &BuildOptions::default()).expect("open");
    s.set_memory_budget(budget);
    let mut steps = Vec::with_capacity(UPDATES);
    for i in 0..UPDATES {
        let rel = &files[i % files.len()];
        toggle(&Path::new(root).join(rel));
        let u = s.apply(&[PathBuf::from(rel)]).expect("apply");
        let m = s.memory();
        steps.push(Step {
            rss_kb: m.rss_kb.expect("Linux: VmRSS is readable"),
            nodes: u.total_nodes,
            edges: u.total_edges,
            rows: rows_hash(&u.nodes, &u.edges),
            ms: u.phase_ms.total,
        });
    }
    let evictions = s.memory().evictions;
    drop(s);
    release_freed_memory("watch_mem_gate");
    Run { steps, evictions }
}

fn line(repo: &str, name: &str, r: &Run, floor: f64, budget: Option<f64>) -> String {
    format!(
        "[watch-mem] repo={repo} run={name} updates={UPDATES} first_mb={:.0} plateau_mb={:.0} \
         floor_mb={floor:.0} max_mb={:.0} budget_mb={} evictions={} ms_median={:.1}",
        r.mb(0),
        r.mean_mb(51, 100),
        r.max_mb(51, 100),
        budget.map_or_else(|| "none".to_string(), |b| format!("{b:.0}")),
        r.evictions,
        r.ms_median()
    )
}

/// Every update of `got` carries `base`'s graph.
fn same_graphs(base: &Run, got: &Run, name: &str) {
    for (i, (a, b)) in base.steps.iter().zip(&got.steps).enumerate() {
        assert_eq!(
            (b.nodes, b.edges, b.rows),
            (a.nodes, a.edges, a.rows),
            "{name}: update {} carries another graph than the unbudgeted run's",
            i + 1
        );
    }
}

#[test]
#[ignore = "release RSS gate: GLIA_WATCH_MEM_REPO=<git archive copy>; edits files there"]
fn watch_memory_gate() {
    let Ok(root) = std::env::var(REPO_ENV) else {
        println!("[watch-mem] skipped: {REPO_ENV} is not set");
        return;
    };
    assert!(
        rss_kb().is_some(),
        "the gate reads /proc/self/status (Linux)"
    );
    let repo = Path::new(&root)
        .file_name()
        .map_or_else(|| root.clone(), |n| n.to_string_lossy().into_owned());
    let files = touched(&source_files(Path::new(&root)));
    let originals: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(Path::new(&root).join(f)).expect("read"))
        .collect();

    let free = run(&root, &files, MemoryBudget::none());
    let floor_run = run(&root, &files, MemoryBudget::mb(1));
    let plateau = free.mean_mb(51, 100);
    let floor = floor_run.mean_mb(51, 100);
    let budget_mb = (0.85 * plateau).max(1.05 * floor);
    let capped = run(&root, &files, MemoryBudget::mb(budget_mb.ceil() as u64));
    for (f, bytes) in files.iter().zip(&originals) {
        std::fs::write(Path::new(&root).join(f), bytes).expect("restore");
    }

    println!("{}", line(&repo, "unbudgeted", &free, floor, None));
    println!("{}", line(&repo, "floor", &floor_run, floor, Some(1.0)));
    println!(
        "{}",
        line(&repo, "budgeted", &capped, floor, Some(budget_mb.ceil()))
    );

    same_graphs(&free, &floor_run, "floor");
    same_graphs(&free, &capped, "budgeted");
    let early = free.mean_mb(11, 25);
    assert!(
        free.max_mb(51, 100) <= 1.15 * early,
        "gate 1 (no leak): max over updates 51-100 {:.0} MB > 1.15 x the mean over 11-25 {early:.0} MB",
        free.max_mb(51, 100)
    );
    assert!(
        floor < 0.95 * plateau,
        "eviction buys nothing: floor {floor:.0} MB >= 0.95 x the unbudgeted plateau {plateau:.0} MB; \
         evict another held structure and re-measure"
    );
    let capped_max = capped.max_mb(51, 100);
    assert!(
        capped_max <= GATE2_MARGIN * budget_mb.ceil(),
        "gate 2 (the cap holds): max over updates 51-100 {capped_max:.0} MB > {GATE2_MARGIN} x the budget {:.0} MB",
        budget_mb.ceil()
    );
    assert!(
        capped.mean_mb(51, 100) < plateau,
        "gate 2: the budgeted plateau {:.0} MB is not below the unbudgeted {plateau:.0} MB",
        capped.mean_mb(51, 100)
    );
}
