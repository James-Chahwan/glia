//! The calibration step (EC.5, item 4): the snapshot step's engine entry,
//! beside `glia history sync`. [`calibrate`] runs git-free over
//! `<root>/.glia/history-snapshot/` (with EB.1's `messages.jsonl` when the
//! sync kept it) against the graph of the layout, replays it leak-free over
//! the registered candidate sets (EC.2-EC.4) and writes
//! `<root>/.glia/calibration-snapshot/` (EC.1). A build never runs it; the
//! ranked answers only read its record through `select`.
//!
//! [`calibrate_snapshot`] is the same step without the reads and the write,
//! so the rank gate (EG.6 / EG.7) can run it in memory over a snapshot built
//! from rank_eval's own commit list.
//!
//! The file space is EB.3's per-repo view ([`Candidates::of`]) over EA.3's
//! `rank::FileIndex` from the same [`RankCache`] the structure bank reads;
//! commit files map through `HistoryFile.p` with no rename fold
//! ([`build_replay`]).
//!
//! The summary reports, per row, the online selection's mean recall@10 minus
//! the fallback's, with a commit-cluster bootstrap 95% CI ([`bootstrap_ci`]:
//! rank_eval's `paired_boot`, test commits the resampled unit). Its seed and
//! resample count only shape the reported CI, never a pick.
//!
//! Fired_on, stderr, once per run: `[calibrate] done surface=<lib|cli|pyo3>
//! sets=<n> test_commits=<t> ms=<ms>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use glia_code_domain::snapshots::{
    HistoryMessage, HistorySnapshot, read_history, read_history_messages,
};
use glia_graph::MergedGraph;
use serde::Serialize;

use super::bank::StructIndex;
use super::candidate::ReplayState;
use super::fusions::IdrState;
use super::record::{
    Calibration, CalibrationMeta, CommitScore, ReplayParamsRecord, Shape, calibration_dir,
    write_calibration,
};
use super::replay::{ReplayParams, build_replay};
use super::select::run_sets;
use super::sets::{log_hooks, register_hooks, specs};
use crate::intent::ids::IdVocab;
use crate::intent::text::Candidates;
use crate::rank::RankCache;

/// The candidate groups a default run replays: every group `sets::specs`
/// registers (`sets::GROUPS`: `structure`, `review`, `find`).
pub const DEFAULT_GROUPS: [&str; 3] = super::sets::GROUPS;

/// rank_eval's bootstrap resamples (`--boot`'s default).
pub const BOOTSTRAP_RESAMPLES: usize = 2000;
/// rank_eval's `paired_boot` generator seed.
pub const BOOTSTRAP_SEED: u64 = 0x5eed_2026_1018;

/// How one calibrate run replays.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CalibrateOptions {
    /// The replay's bounds.
    pub params: ReplayParams,
    /// The candidate groups replayed, in order ([`DEFAULT_GROUPS`]).
    pub groups: Vec<&'static str>,
    /// Who ran it: `lib`, `cli` or `pyo3` (the marker's `surface=`).
    pub surface: &'static str,
    /// The `[ids]` vocabulary the replay's IDR routes with (EC.14), built by
    /// the caller with [`IdVocab`]'s constructors; `None` reads the root's
    /// `.glia/overlay.toml` ([`IdVocab::for_root`]), else the defaults.
    pub vocab: Option<IdVocab>,
}

impl Default for CalibrateOptions {
    fn default() -> Self {
        CalibrateOptions {
            params: ReplayParams::default(),
            groups: DEFAULT_GROUPS.to_vec(),
            surface: "lib",
            vocab: None,
        }
    }
}

/// What a calibrate run measured.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CalibrationSummary {
    /// The record's directory (`<root>/.glia/calibration-snapshot`).
    pub dir: String,
    /// The history snapshot's head.
    pub history_head: String,
    /// Commits replayed.
    pub commits: usize,
    /// Commits whose queries were scored.
    pub test_commits: usize,
    /// Whether the replay read commit messages.
    pub messages: bool,
    /// Wall time of the replay and selection, milliseconds.
    pub ms: u64,
    /// One row per (set, shape), in the record's order.
    pub sets: Vec<SetSummary>,
}

/// One (set, shape) row of a [`CalibrationSummary`].
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SetSummary {
    /// The candidate set.
    pub set: String,
    /// [`Shape::as_str`].
    pub shape: String,
    /// The record's pick (the fallback when refused, ruling EC-3).
    pub pick: String,
    /// The online rule's pick at the newest test commit.
    pub last_online_pick: String,
    /// The set's uncalibrated ranking.
    pub fallback: String,
    /// Test commits with a query of this shape.
    pub test_commits: usize,
    /// Queries of this shape.
    pub queries: u32,
    /// Mean recall@10 of the online selection.
    pub selection_recall10: f64,
    /// Mean recall@10 of the fallback.
    pub fallback_recall10: f64,
    /// `selection_recall10 - fallback_recall10`.
    pub delta_recall10: f64,
    /// The 2.5% order statistic of the commit-cluster bootstrap of the delta.
    pub ci_low: f64,
    /// The 97.5% order statistic.
    pub ci_high: f64,
}

/// rank_eval's `mix` (splitmix64's finaliser).
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// rank_eval's `Rng`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0)
    }
}

/// The commit-cluster bootstrap 95% CI of the selection's recall@10 gain over
/// the fallback (rank_eval's `paired_boot`): [`BOOTSTRAP_RESAMPLES`]
/// resamples of the test commits with replacement, each one's delta
/// `sum(sel_r10 - fb_r10) / sum(queries)` (0 with no query), then the 2.5% /
/// 97.5% order statistics. No commit gives (0, 0).
pub fn bootstrap_ci(per_commit: &[CommitScore]) -> (f64, f64) {
    let n = per_commit.len();
    if n == 0 {
        return (0.0, 0.0);
    }
    let boot = BOOTSTRAP_RESAMPLES;
    let csum: Vec<f64> = per_commit.iter().map(|c| c.sel_r10 - c.fb_r10).collect();
    let ccnt: Vec<usize> = per_commit.iter().map(|c| c.queries as usize).collect();
    let mut rng = Rng(BOOTSTRAP_SEED);
    let mut means = Vec::with_capacity(boot);
    for _ in 0..boot {
        let (mut s, mut c) = (0.0, 0usize);
        for _ in 0..n {
            let k = (rng.next() % n as u64) as usize;
            s += csum[k];
            c += ccnt[k];
        }
        means.push(if c > 0 { s / c as f64 } else { 0.0 });
    }
    means.sort_by(f64::total_cmp);
    let lo = means[((boot as f64) * 0.025) as usize];
    let hi = means[(((boot as f64) * 0.975) as usize).min(boot - 1)];
    (lo, hi)
}

/// The one repo `merged` holds. A repo builds several graphs (its language
/// groups, region, project and docs graphs), so this counts distinct repo
/// ids, never graphs; zero or several repos is an error (a calibration is
/// per repo).
fn one_repo(merged: &MergedGraph) -> Result<u64, String> {
    let repos: BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    match (repos.len(), repos.first()) {
        (1, Some(&r)) => Ok(r),
        (n, _) => Err(format!("calibration is per repo: got {n} repos")),
    }
}

/// The step over an in-memory snapshot: replay `snapshot` (and `messages`,
/// row i belonging to commit i) over `merged`'s one repo and the groups of
/// `opts`, and return the record and its summary. Writes nothing; `root` is
/// read only by candidates that read the tree (IDR's parent revisions,
/// EC.12). A merge of several repos is refused: a calibration is per repo.
pub fn calibrate_snapshot(
    merged: &MergedGraph,
    root: &Path,
    snapshot: &HistorySnapshot,
    messages: Option<&[HistoryMessage]>,
    opts: &CalibrateOptions,
) -> Result<(Calibration, CalibrationSummary), String> {
    let started = Instant::now();
    let repo = one_repo(merged)?;
    let set_specs = specs(&opts.groups)?;
    let label = root.file_name().map_or_else(
        || root.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let roots: BTreeMap<u64, String> = BTreeMap::from([(repo, root.display().to_string())]);
    let labels: BTreeMap<u64, String> = BTreeMap::from([(repo, label)]);
    let cache = Arc::new(RankCache::new("calibrate", &roots, &labels));
    let cands = Candidates::of(&cache.files(merged), repo);

    let mut shapes: Vec<Shape> = Vec::new();
    for s in &set_specs {
        for &sh in s.shapes {
            if !shapes.contains(&sh) {
                shapes.push(sh);
            }
        }
    }
    let replay = build_replay(snapshot, messages, &cands, &opts.params, &shapes);
    let mut state = ReplayState::new(cands.len(), opts.params.max_files);
    state.register(Box::new(StructIndex::build(
        merged,
        &cands,
        Arc::clone(&cache),
    )));
    register_hooks(
        &mut state,
        &replay,
        cands.len(),
        opts.params.max_files,
        &opts.groups,
    );
    if let (Some(v), Some(idr)) = (&opts.vocab, state.hook::<IdrState>()) {
        idr.set_vocab(v.clone());
    }
    let rows = run_sets(merged, &cands, root, &replay, &mut state, &set_specs);
    log_hooks(&state);

    let train = replay.train.iter().filter(|&&t| t).count();
    let mut cal = Calibration {
        replay: ReplayParamsRecord {
            max_commits: opts.params.max_commits,
            max_seeds: opts.params.max_seeds,
            max_files: opts.params.max_files,
            commits: replay.commits.len(),
            train_commits: train,
            test_commits: replay.tests.len(),
        },
        sets: rows,
    };
    // The record's row order (write_calibration sorts the same way).
    let order = |s: &str| Shape::parse(s).map_or(u8::MAX, Shape::order);
    cal.sets
        .sort_by(|a, b| (a.set.as_str(), order(&a.shape)).cmp(&(b.set.as_str(), order(&b.shape))));

    let sets: Vec<SetSummary> = cal
        .sets
        .iter()
        .map(|r| {
            let (ci_low, ci_high) = bootstrap_ci(&r.online.per_commit);
            SetSummary {
                set: r.set.clone(),
                shape: r.shape.clone(),
                pick: r.pick.clone(),
                last_online_pick: r.last_online_pick.clone(),
                fallback: r.fallback.clone(),
                test_commits: r.test_commits,
                queries: r.queries,
                selection_recall10: r.online.selection_recall10,
                fallback_recall10: r.online.fallback_recall10,
                delta_recall10: r.online.selection_recall10 - r.online.fallback_recall10,
                ci_low,
                ci_high,
            }
        })
        .collect();
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let summary = CalibrationSummary {
        dir: calibration_dir(root).display().to_string(),
        history_head: snapshot.meta.head.clone(),
        commits: replay.commits.len(),
        test_commits: replay.tests.len(),
        messages: replay.messages,
        ms,
        sets,
    };
    Ok((cal, summary))
}

/// The calibration step: read `<root>/.glia/history-snapshot/` and its
/// messages, replay them over `merged` ([`calibrate_snapshot`]) and write
/// `<root>/.glia/calibration-snapshot/`. No snapshot is an error naming
/// `glia history sync`; no test commit writes zero-query rows (an answer's
/// select then reads `no_queries`).
pub fn calibrate(
    root: &Path,
    merged: &MergedGraph,
    opts: &CalibrateOptions,
) -> Result<CalibrationSummary, String> {
    let started = Instant::now();
    one_repo(merged)?;
    let snapshot = read_history(root).ok_or_else(|| {
        format!(
            "no history snapshot under {}: run `glia history sync <repo>` first",
            root.display()
        )
    })?;
    let messages = read_history_messages(root, &snapshot);
    let (cal, mut summary) =
        calibrate_snapshot(merged, root, &snapshot, messages.as_deref(), opts)?;
    let meta = CalibrationMeta::new(
        snapshot.meta.head.clone(),
        snapshot.meta.data_hash.clone(),
        snapshot.meta.messages_hash.clone(),
    );
    write_calibration(root, meta, &cal)?;
    summary.ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    eprintln!(
        "[calibrate] done surface={} sets={} test_commits={} ms={}",
        opts.surface,
        summary.sets.len(),
        summary.test_commits,
        summary.ms
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(queries: u32, sel: f64, fb: f64) -> CommitScore {
        CommitScore {
            queries,
            sel_r10: sel,
            fb_r10: fb,
            ..CommitScore::default()
        }
    }

    #[test]
    fn bootstrap_of_nothing_is_zero() {
        assert_eq!(bootstrap_ci(&[]), (0.0, 0.0));
    }

    #[test]
    fn bootstrap_of_a_constant_gain_is_that_gain() {
        let rows: Vec<CommitScore> = (0..7).map(|_| cs(2, 1.5, 0.5)).collect();
        assert_eq!(bootstrap_ci(&rows), (0.5, 0.5));
    }

    #[test]
    fn bootstrap_is_deterministic_and_brackets_the_mean() {
        let rows: Vec<CommitScore> = (0..40)
            .map(|i| cs(1 + i % 3, f64::from(i % 5) * 0.2, 0.3))
            .collect();
        let a = bootstrap_ci(&rows);
        assert_eq!(a, bootstrap_ci(&rows));
        let q: f64 = rows.iter().map(|r| f64::from(r.queries)).sum();
        let mean = rows.iter().map(|r| r.sel_r10 - r.fb_r10).sum::<f64>() / q;
        assert!(a.0 <= mean && mean <= a.1, "{a:?} {mean}");
        assert!(a.0 < a.1);
    }

    /// rank_eval's generator, first draws from its seed.
    #[test]
    fn rng_is_rank_evals() {
        let mut r = Rng(BOOTSTRAP_SEED);
        let a = r.next();
        let b = r.next();
        let mut z = BOOTSTRAP_SEED.wrapping_add(0x9e37_79b9_7f4a_7c15);
        assert_eq!(a, mix(z));
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        assert_eq!(b, mix(z));
    }
}
