//! The persisted calibration record (EC.1): `<root>/.glia/calibration-snapshot/`
//! holds `calibration.json` (the replay's numbers per candidate set and query
//! shape) and `meta.json`, written LAST, so a write stopped part-way leaves a
//! record the reader rejects. The discipline copies the history snapshot's
//! (`glia_code_domain::snapshots::write_history`), but the record lives in the
//! engine: code-domain is hashed into PARSER_STAMP, and a ranking file format
//! must never move the parse cache.
//!
//! A record is valid only for its [`CALIBRATION_VERSION`], the
//! `glia_stamp::BUILD_STAMP` that wrote it (strict, James 2026-10-23: a glia
//! upgrade or parser change sends answers to the fallback until `glia history
//! calibrate` runs again), [`RANKER_REVISION`] and, per set, the digest of its
//! candidate list. [`select`] is the one reader every ranked answer calls: it
//! turns a record into that answer's [`Selection`], or the caller's fallback
//! with a named reason, and [`log_selection`] prints its `[select]` line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glia_code_domain::snapshots::data_hash;
use glia_code_domain::walk_gating::CONTROL_DIR;
use serde::{Deserialize, Serialize};

/// The record's directory under `.glia/`.
pub const CALIBRATION_DIR: &str = "calibration-snapshot";
/// The record's data file.
pub const CALIBRATION_FILE: &str = "calibration.json";
/// The record's meta file, written last.
pub const META_FILE: &str = "meta.json";
/// The record format; any other value reads as `Rejected("version")`.
pub const CALIBRATION_VERSION: u32 = 1;
/// Who writes a record.
pub const CALIBRATION_GENERATOR: &str = "glia history calibrate";
/// The candidates' arithmetic. Hand-bumped by any packet that changes how a
/// candidate ranking scores, so a record replayed over the old arithmetic is
/// rejected (`ranker_revision`) and every set digest moves.
pub const RANKER_REVISION: u32 = 1;

/// `<root>/.glia/calibration-snapshot`.
pub fn calibration_dir(root: &Path) -> PathBuf {
    root.join(CONTROL_DIR).join(CALIBRATION_DIR)
}

/// `meta.json`: what the record was replayed from and by which glia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CalibrationMeta {
    /// [`CALIBRATION_VERSION`].
    pub version: u32,
    /// [`CALIBRATION_GENERATOR`].
    pub generator: String,
    /// `glia_stamp::BUILD_STAMP` at write time.
    pub build_stamp: String,
    /// [`RANKER_REVISION`] at write time.
    pub ranker_revision: u32,
    /// `HistoryMeta.head` of the history snapshot replayed.
    pub history_head: String,
    /// `HistoryMeta.data_hash` of that snapshot.
    pub history_data_hash: String,
    /// The snapshot's commit-message hash; None when it held no messages.
    pub messages_hash: Option<String>,
    /// Number of rows in `calibration.json`'s `sets`.
    pub sets: usize,
    /// `data_hash` over `calibration.json`'s bytes.
    pub data_hash: String,
}

impl CalibrationMeta {
    /// A meta for a replay of the history snapshot at `history_head`. The
    /// derived fields (`sets`, `data_hash`) are left empty:
    /// [`write_calibration`] fills them from the bytes it writes.
    pub fn new(
        history_head: String,
        history_data_hash: String,
        messages_hash: Option<String>,
    ) -> Self {
        Self {
            version: CALIBRATION_VERSION,
            generator: CALIBRATION_GENERATOR.to_string(),
            build_stamp: glia_stamp::BUILD_STAMP.to_string(),
            ranker_revision: RANKER_REVISION,
            history_head,
            history_data_hash,
            messages_hash,
            sets: 0,
            data_hash: String::new(),
        }
    }
}

/// `calibration.json`: the replay's parameters and one row per (set, shape),
/// sorted by (set, [`Shape::order`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Calibration {
    /// The bounds the replay ran under.
    pub replay: ReplayParamsRecord,
    /// One row per (candidate set, query shape).
    pub sets: Vec<CalibratedSet>,
}

/// The replay's bounds and its commit split.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReplayParamsRecord {
    /// Most commits read from the history snapshot.
    pub max_commits: usize,
    /// Most seed files one query takes.
    pub max_seeds: usize,
    /// Largest change (in files) a commit may touch to be replayed.
    pub max_files: usize,
    /// Commits replayed.
    pub commits: usize,
    /// Of them, the earlier commits only trained (no query scored).
    pub train_commits: usize,
    /// Of them, the commits whose queries were scored.
    pub test_commits: usize,
}

/// One candidate set at one query shape: what each candidate scored over the
/// replay and what the online rule picked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CalibratedSet {
    /// The set's name (the answer surface's ranking family).
    pub set: String,
    /// [`Shape::as_str`].
    pub shape: String,
    /// The candidate rankings, in selection order (ties go to the earlier).
    pub candidates: Vec<String>,
    /// [`set_digest`] of `candidates`.
    pub digest: String,
    /// The surface's uncalibrated ranking.
    pub fallback: String,
    /// Test commits with at least one query of this shape.
    pub test_commits: usize,
    /// Queries of this shape over the test commits.
    pub queries: u32,
    /// Per candidate, recall@10 summed over the queries.
    pub recall10: Vec<f64>,
    /// Per candidate, MRR summed over the queries.
    pub mrr: Vec<f64>,
    /// The fallback's summed recall@10.
    pub fallback_recall10: f64,
    /// The fallback's summed MRR.
    pub fallback_mrr: f64,
    /// The online rule's next step: argmax of the summed recall@10 over every
    /// replayed query, ties to the earlier candidate.
    pub pick: String,
    /// The pick the rule made at the newest test commit, before that commit's
    /// queries were added (rank_eval's `pick_at_the_last_test_commit`).
    pub last_online_pick: String,
    /// The online selection against the fallback.
    pub online: OnlineReplay,
}

/// The leak-free online replay of one set: each test commit ranked by the
/// candidate picked from strictly earlier test commits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OnlineReplay {
    /// Mean recall@10 of the online selection over the queries.
    pub selection_recall10: f64,
    /// Mean recall@10 of the fallback over the queries.
    pub fallback_recall10: f64,
    /// Mean MRR of the online selection.
    pub selection_mrr: f64,
    /// Mean MRR of the fallback.
    pub fallback_mrr: f64,
    /// Test commits per online pick (rank_eval's `picks_per_test_commit`).
    pub picks: BTreeMap<String, usize>,
    /// Per test commit, oldest first: the unit a commit-cluster bootstrap
    /// resamples.
    pub per_commit: Vec<CommitScore>,
}

/// One test commit's sums over its queries of one shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CommitScore {
    /// The test commit's sha.
    pub sha: String,
    /// The online pick at this commit, made from strictly earlier test commits
    /// only, so a caller can replay the selection commit by commit.
    pub pick: String,
    /// Queries of this shape at this commit.
    pub queries: u32,
    /// Summed recall@10 of the online pick.
    pub sel_r10: f64,
    /// Summed recall@10 of the fallback.
    pub fb_r10: f64,
    /// Summed MRR of the online pick.
    pub sel_mrr: f64,
    /// Summed MRR of the fallback.
    pub fb_mrr: f64,
}

/// A query's shape: how many seed files it starts from (the replay's shapes:
/// one seed file; two; three; half of a 4+ file change), or a `find` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Shape {
    /// One seed file.
    K1,
    /// Two seed files.
    K2,
    /// Three seed files.
    K3,
    /// Half of a change of four or more files.
    Half,
    /// A text query (`find --files`), set explicitly.
    Find,
}

impl Shape {
    /// The shape of a query seeded by `n` files: 0 | 1 -> K1, 2 -> K2, 3 -> K3,
    /// else Half.
    pub fn from_seed_files(n: usize) -> Shape {
        match n {
            0 | 1 => Shape::K1,
            2 => Shape::K2,
            3 => Shape::K3,
            _ => Shape::Half,
        }
    }

    /// `k1` `k2` `k3` `half` `find`.
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::K1 => "k1",
            Shape::K2 => "k2",
            Shape::K3 => "k3",
            Shape::Half => "half",
            Shape::Find => "find",
        }
    }

    /// The record's row order: k1 < k2 < k3 < half < find.
    pub fn order(self) -> u8 {
        match self {
            Shape::K1 => 0,
            Shape::K2 => 1,
            Shape::K3 => 2,
            Shape::Half => 3,
            Shape::Find => 4,
        }
    }

    /// The shape [`Shape::as_str`] names.
    pub fn parse(s: &str) -> Option<Shape> {
        match s {
            "k1" => Some(Shape::K1),
            "k2" => Some(Shape::K2),
            "k3" => Some(Shape::K3),
            "half" => Some(Shape::Half),
            "find" => Some(Shape::Find),
            _ => None,
        }
    }
}

/// What reading a calibration gave.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub enum CalibrationRead {
    /// Nobody read one (no repo root to read from).
    #[default]
    NotLoaded,
    /// The repo has no calibration record.
    Absent,
    /// A record exists but may not be applied, for the named reason:
    /// `incomplete`, `version`, `build_stamp`, `ranker_revision`, `multi_repo`.
    Rejected(&'static str),
    /// A valid record.
    Loaded(Box<Calibration>),
}

/// A shared, cheaply cloned [`CalibrationRead`], read once per graph and
/// handed to every answer.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct CalibrationHandle(Arc<CalibrationRead>);

impl CalibrationHandle {
    /// Wrap a read.
    pub fn new(read: CalibrationRead) -> Self {
        Self(Arc::new(read))
    }

    /// The read.
    pub fn read(&self) -> &CalibrationRead {
        &self.0
    }
}

/// One answer's choice of ranking.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Selection {
    /// The candidate set.
    pub set: String,
    /// [`Shape::as_str`].
    pub shape: String,
    /// The ranking the answer uses.
    pub pick: String,
    /// `calibrated`, `fallback` or `explicit`.
    pub source: &'static str,
    /// Why the fallback was used; None otherwise.
    pub reason: Option<&'static str>,
    /// The test commits the calibrated row measured (0 for a fallback or an
    /// explicit pick).
    pub test_commits: usize,
}

/// The digest a set's row records for its candidate list: bound to
/// [`RANKER_REVISION`] and to the list's order.
pub fn set_digest(candidates: &[&str]) -> String {
    data_hash(&[
        format!("rev={RANKER_REVISION}\n").as_bytes(),
        candidates.join("\n").as_bytes(),
    ])
}

/// The first 12 characters of a sha (all of it when shorter).
fn sha12(s: &str) -> &str {
    s.get(..12).unwrap_or(s)
}

/// Write `path` through a temp file in its directory and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))
}

/// The row order key of a set: (set, shape order; an unknown shape last).
fn row_key(s: &CalibratedSet) -> (&str, u8) {
    (
        s.set.as_str(),
        Shape::parse(&s.shape).map_or(u8::MAX, Shape::order),
    )
}

/// Write `cal` as `<root>/.glia/calibration-snapshot/`: an existing meta.json
/// is removed first, then calibration.json (rows sorted by (set, shape order)),
/// then meta.json with `sets` and `data_hash` filled. Returns the meta written.
pub fn write_calibration(
    root: &Path,
    mut meta: CalibrationMeta,
    cal: &Calibration,
) -> Result<CalibrationMeta, String> {
    let dir = calibration_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let meta_path = dir.join(META_FILE);
    match std::fs::remove_file(&meta_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove {}: {e}", meta_path.display())),
    }
    let mut sorted = cal.clone();
    sorted.sets.sort_by(|a, b| row_key(a).cmp(&row_key(b)));
    let mut bytes =
        serde_json::to_vec_pretty(&sorted).map_err(|e| format!("serialise calibration: {e}"))?;
    bytes.push(b'\n');
    write_atomic(&dir.join(CALIBRATION_FILE), &bytes)?;
    meta.sets = sorted.sets.len();
    meta.data_hash = data_hash(&[&bytes]);
    let mut mbytes =
        serde_json::to_vec_pretty(&meta).map_err(|e| format!("serialise calibration meta: {e}"))?;
    mbytes.push(b'\n');
    write_atomic(&meta_path, &mbytes)?;
    eprintln!(
        "[calibration] wrote dir={} sets={} build_stamp={} history_head={}",
        dir.display(),
        meta.sets,
        meta.build_stamp,
        sha12(&meta.history_head)
    );
    Ok(meta)
}

/// Read and validate `<root>/.glia/calibration-snapshot/`, without the marker.
fn read_inner(root: &Path) -> (CalibrationRead, Option<String>) {
    let dir = calibration_dir(root);
    if !dir.is_dir() {
        return (CalibrationRead::Absent, None);
    }
    let incomplete = CalibrationRead::Rejected("incomplete");
    let Some(meta) = std::fs::read(dir.join(META_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<CalibrationMeta>(&b).ok())
    else {
        return (incomplete, None);
    };
    let head = Some(meta.history_head.clone());
    if meta.version != CALIBRATION_VERSION {
        return (CalibrationRead::Rejected("version"), head);
    }
    let Ok(bytes) = std::fs::read(dir.join(CALIBRATION_FILE)) else {
        return (incomplete, head);
    };
    if data_hash(&[&bytes]) != meta.data_hash {
        return (incomplete, head);
    }
    let Ok(cal) = serde_json::from_slice::<Calibration>(&bytes) else {
        return (incomplete, head);
    };
    if cal.sets.len() != meta.sets {
        return (incomplete, head);
    }
    if meta.build_stamp != glia_stamp::BUILD_STAMP {
        return (CalibrationRead::Rejected("build_stamp"), head);
    }
    if meta.ranker_revision != RANKER_REVISION {
        return (CalibrationRead::Rejected("ranker_revision"), head);
    }
    (CalibrationRead::Loaded(Box::new(cal)), head)
}

/// Read `<root>/.glia/calibration-snapshot/`: Absent with no directory,
/// Rejected with the reason a record may not be applied, else Loaded.
pub fn read_calibration(root: &Path) -> CalibrationRead {
    let (read, head) = read_inner(root);
    let (status, reason, sets) = match &read {
        CalibrationRead::Loaded(c) => ("loaded", "-", c.sets.len()),
        CalibrationRead::Rejected(r) => ("rejected", *r, 0),
        CalibrationRead::Absent | CalibrationRead::NotLoaded => ("absent", "-", 0),
    };
    eprintln!(
        "[calibration] read root={} status={status} reason={reason} sets={sets} history_head={}",
        root.display(),
        head.as_deref().map_or("-", sha12)
    );
    read
}

/// The calibration an answer over these repo roots reads: none for no root,
/// `Rejected("multi_repo")` for a merge (a calibration is per repo), else the
/// one root's record.
pub fn load_for_roots(roots: &BTreeMap<u64, String>) -> CalibrationRead {
    let mut it = roots.values();
    match (it.next(), it.next()) {
        (None, _) => CalibrationRead::NotLoaded,
        (Some(root), None) => read_calibration(Path::new(root)),
        (Some(_), Some(_)) => CalibrationRead::Rejected("multi_repo"),
    }
}

/// One answer's ranking: the record's pick for (set, shape) when the record is
/// loaded and its row was measured over exactly `candidates`, else `fallback`
/// with the reason (`not_loaded`, `absent`, the rejection, `no_set`,
/// `candidate_set`, `no_queries`).
pub fn select(
    read: &CalibrationRead,
    set: &str,
    shape: Shape,
    candidates: &[&str],
    fallback: &str,
) -> Selection {
    let fall = |reason: &'static str| Selection {
        set: set.to_string(),
        shape: shape.as_str().to_string(),
        pick: fallback.to_string(),
        source: "fallback",
        reason: Some(reason),
        test_commits: 0,
    };
    let cal = match read {
        CalibrationRead::NotLoaded => return fall("not_loaded"),
        CalibrationRead::Absent => return fall("absent"),
        CalibrationRead::Rejected(r) => return fall(r),
        CalibrationRead::Loaded(c) => c,
    };
    let Some(row) = cal
        .sets
        .iter()
        .find(|r| r.set == set && r.shape == shape.as_str())
    else {
        return fall("no_set");
    };
    if row.digest != set_digest(candidates) || !candidates.contains(&row.pick.as_str()) {
        return fall("candidate_set");
    }
    if row.queries == 0 {
        return fall("no_queries");
    }
    Selection {
        set: set.to_string(),
        shape: shape.as_str().to_string(),
        pick: row.pick.clone(),
        source: "calibrated",
        reason: None,
        test_commits: row.test_commits,
    }
}

/// The Selection of a ranking the caller chose by flag.
pub fn explicit(set: &str, shape: Shape, name: &str) -> Selection {
    Selection {
        set: set.to_string(),
        shape: shape.as_str().to_string(),
        pick: name.to_string(),
        source: "explicit",
        reason: None,
        test_commits: 0,
    }
}

/// The `[select]` line, once per answer.
pub fn log_selection(surface: &str, s: &Selection) {
    eprintln!(
        "[select] surface={surface} set={} shape={} pick={} source={} reason={} test_commits={}",
        s.set,
        s.shape,
        s.pick,
        s.source,
        s.reason.unwrap_or("-"),
        s.test_commits
    );
}
