//! Review's text selection (EC.11, item 4; EC.12, item 8): the seeded fusion
//! bank of rank_eval `r3_bank_seed` / `round1_rankers` over text (T1s),
//! structure (P1, PT*), co-change (C0), the work signals (W5, M) and typed id
//! routing (IDR), the candidates of set `SELt2` (rank_eval `R1_SEL`), the
//! replay's text corpus and its id router.
//!
//! One code path: the calibration replay and the review answer both rank
//! through [`seeded_keys`], so the record measures exactly what the answer
//! applies. Every min-max (`mm`) is over the query's non-excluded files (the
//! seeds; the changed files in an answer), a flat vector 0
//! (`glia_activation::algo::fuse::minmax`, rank_eval's `mm_ex`):
//!
//! - `T1s`, `P1`, `PT*`, `C0`: the signal itself, key `[s, 0, 0]`;
//! - `H9s50` = `blend(0.5, mm(T1s), mm(P1))`; `H9s25[PT*]` =
//!   `blend(0.25, mm(T1s), mm(PT*))`; `H18s[PT*]` = `mean(mm(T1s), mm(C0),
//!   mm(PT*))`; `H10[PT*]` = `[C0, PT*, 0]`;
//! - `HTs<tag>[W5]` / `HTs<tag>[M]` = `a mm(T1s) + b mm(W) + c mm(P1)`, the
//!   sum in that order, `(a, b, c)` from [`HT_GRID`];
//! - `ALL5` = `mean(mm(T1s), mm(W5), mm(M), mm(C0), mm(P1))`;
//! - `LX[IDR,P1]` = `[IDR, P1, 0]` (EB.7's shipped order, the set's
//!   scored-only fallback) and `LX[IDR,H9s50]` = `[IDR, H9s50, 0]`, raw IDR
//!   first;
//! - EC.12's id-routed fusions (SELt2's last four): `LX[IDR,H18s[PT*]]` =
//!   `[IDR, H18s[PT*], 0]`, `LX[IDR,H9s25[PT*]]` = `[IDR, H9s25[PT*], 0]`,
//!   `H18si[PT*]` = `mean(mm(IDR), mm(T1s), mm(C0), mm(PT*))` and
//!   `LX[IDR,C0,PT*]` = `[IDR, C0, PT*]`. A subject with no id routes nothing:
//!   IDR is all zero, so each `LX[IDR, X]` orders exactly as X and
//!   `H18si[PT*]` (its `mm(IDR)` flat 0) as `H18s[PT*]`.
//!
//! [`IdrState`] is the replay's router (EC.12): each test commit's subject is
//! routed by EB.6's [`route`] over the tree of the commit's PARENT (rank_eval
//! `r2_build`), since the rows an id names are edited by that very commit
//! (pending -> done); the recent scopes are [`recent_scopes`] over the
//! strictly older subjects. The answer's IDR stays EB.7's (the review base).
//!
//! [`TextState`] is the replay's BM25 corpus: at each test commit one EB.3
//! [`TextIndex`] over the full messages of the strictly older commits (the
//! index's own 0 / > 30 candidate-file skip is rank_eval's `train_ok`),
//! scoring the commit's own subject (rank_eval's `tokens(subject(&c.msg))`
//! over `bm`). The index is built once per test commit and read by every
//! query and candidate of it through memo key [`MEMO_T1S`]; it is also the
//! find shape's [`StateHook::subject_scores`].
//!
//! Fired_on, stderr: `[calibrate] text builds=<b> docs_max=<d>
//! messages=<yes|no> ms=<ms>` once per calibrate run that replays the
//! `review` group ([`TextState::log_summary`]); `[calibrate] idr unavailable
//! commits=<n>` after it when n test commits had no readable parent (no git,
//! an unknown sha, a root commit; [`IdrState::log_summary`]); and EB.6's
//! `[idr] repo=<label> surface=calibrate rev=<parent12> ..` per routed subject.

use std::any::Any;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use glia_activation::algo::fuse;
use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot, read_history};
use glia_core::NodeId;
use glia_graph::MergedGraph;

use super::bank::structure_keys;
use super::candidate::{Candidate, QueryCtx, RankKey, StateHook};
use super::replay::{Replay, ReplayCommit};
use super::work::{m_scores, w5_scores, work_at_head};
use crate::answers::project_roots;
use crate::git_rev::{Tree, resolve_rev};
use crate::intent::defs::read_defs;
use crate::intent::idr::{IdrSide, conventional_scopes, recent_scopes, route};
use crate::intent::ids::IdVocab;
use crate::intent::paths::{PathResolver, rename_pairs};
use crate::intent::text::{Candidates, TextIndex, subject};
use crate::rank::{RankBase, RankCache, Ranking, c0, rank_files};

/// The memo key of a query's dense T1s vector.
pub const MEMO_T1S: &str = "t1s";
/// The memo key of a query's dense IDR vector (EC.12).
pub const MEMO_IDR: &str = "idr";

/// rank_eval's `HT_GRID` rows SELt reads: (tag, a T1s, b W, c P1).
pub const HT_GRID: [(&str, f64, f64, f64); 4] = [
    ("a", 0.5, 0.25, 0.25),
    ("b", 0.25, 0.5, 0.25),
    ("c", 0.25, 0.25, 0.5),
    ("d", 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0),
];

/// Every name [`seeded_keys`] ranks, in this order.
pub const SEEDED_NAMES: [&str; 22] = [
    "T1s",
    "P1",
    "PT*",
    "C0",
    "H9s50",
    "H9s25[PT*]",
    "H18s[PT*]",
    "H10[PT*]",
    "HTsa[W5]",
    "HTsb[W5]",
    "HTsc[W5]",
    "HTsd[W5]",
    "HTsa[M]",
    "HTsb[M]",
    "HTsc[M]",
    "HTsd[M]",
    "ALL5",
    "LX[IDR,P1]",
    "LX[IDR,H18s[PT*]]",
    "LX[IDR,H9s25[PT*]]",
    "H18si[PT*]",
    "LX[IDR,C0,PT*]",
];

/// The one more name [`seeded_keys`] ranks: EB.7's measured alternative.
pub const LX_IDR_H9S50: &str = "LX[IDR,H9s50]";

/// The per-candidate signals one seeded query (or one review) fuses, dense
/// over the repo's candidate files; a signal a fusion does not read may be
/// left empty (it then reads 0).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Signals {
    /// rank_eval's P1: PPR d0.5 from the seed files' nodes, max-pooled.
    pub p1: Vec<f64>,
    /// PT*: PPR d0.85, sum-pooled.
    pub pt: Vec<f64>,
    /// C0: co-change from the seed files.
    pub c0: Vec<f64>,
    /// T1s: BM25 of the subject (title) over past commit messages.
    pub t1s: Vec<f64>,
    /// W5: the last five training commits' files, decayed.
    pub w5: Vec<f64>,
    /// M: the milestone's files.
    pub m: Vec<f64>,
    /// Typed id routing: EB.7's at the review base in an answer, the
    /// parent revision's ([`IdrState`]) in the replay; None reads all zeros.
    pub idr: Option<Vec<f64>>,
}

impl Signals {
    /// All-zero signals over `n` candidates.
    pub fn zeros(n: usize) -> Signals {
        Signals {
            p1: vec![0.0; n],
            pt: vec![0.0; n],
            c0: vec![0.0; n],
            t1s: vec![0.0; n],
            w5: vec![0.0; n],
            m: vec![0.0; n],
            idr: None,
        }
    }
}

/// The signals a fusion reads beyond P1 / T1s (which an answer always has):
/// PT*, C0, W5, M, and IDR (an answer always has it too; the replay routes a
/// subject only for a fusion that reads it).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs {
    pub pt: bool,
    pub c0: bool,
    pub w5: bool,
    pub m: bool,
    pub idr: bool,
}

/// What `name` reads; None for a name [`seeded_keys`] does not rank.
pub fn needs(name: &str) -> Option<Needs> {
    let n = |pt, c0, w5, m| Some(Needs { pt, c0, w5, m, idr: false });
    let i = |pt, c0| Some(Needs { pt, c0, w5: false, m: false, idr: true });
    match name {
        "T1s" | "P1" | "H9s50" => n(false, false, false, false),
        "LX[IDR,P1]" | LX_IDR_H9S50 => i(false, false),
        "LX[IDR,H9s25[PT*]]" => i(true, false),
        "LX[IDR,H18s[PT*]]" | "H18si[PT*]" | "LX[IDR,C0,PT*]" => i(true, true),
        "PT*" | "H9s25[PT*]" => n(true, false, false, false),
        "C0" => n(false, true, false, false),
        "H18s[PT*]" | "H10[PT*]" => n(true, true, false, false),
        "HTsa[W5]" | "HTsb[W5]" | "HTsc[W5]" | "HTsd[W5]" => n(false, false, true, false),
        "HTsa[M]" | "HTsb[M]" | "HTsc[M]" | "HTsd[M]" => n(false, false, false, true),
        "ALL5" => n(false, true, true, true),
        _ => None,
    }
}

fn at(v: &[f64], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

/// `v` padded or cut to `n` (a missing signal reads 0).
fn dense(v: &[f64], n: usize) -> Vec<f64> {
    (0..n).map(|i| at(v, i)).collect()
}

fn scalar(v: &[f64]) -> Vec<RankKey> {
    v.iter().map(|&s| [s, 0.0, 0.0]).collect()
}

fn lex2(a: &[f64], b: &[f64]) -> Vec<RankKey> {
    a.iter().zip(b).map(|(&x, &y)| [x, y, 0.0]).collect()
}

/// The keys of the seeded ranker `name` over `s` (one per candidate,
/// `excluded.len()` of them), rank_eval's arithmetic (module docs); None for
/// a name it does not rank.
pub fn seeded_keys(name: &str, s: &Signals, excluded: &[bool]) -> Option<Vec<RankKey>> {
    needs(name)?;
    let n = excluded.len();
    let p1 = dense(&s.p1, n);
    let t1s = dense(&s.t1s, n);
    let mm = |v: &[f64]| fuse::minmax(&dense(v, n), excluded);
    let ht = |w: &[f64], tag: &str| -> Option<Vec<RankKey>> {
        let &(_, a, b, c) = HT_GRID.iter().find(|g| g.0 == tag)?;
        let (mm_t1s, mm_w, mm_p1) = (mm(&t1s), mm(w), mm(&p1));
        let v: Vec<f64> = (0..n)
            .map(|f| a * mm_t1s[f] + b * mm_w[f] + c * mm_p1[f])
            .collect();
        Some(scalar(&v))
    };
    let idr = || s.idr.as_deref().map_or_else(|| vec![0.0; n], |v| dense(v, n));
    let h9s50 = || fuse::blend(0.5, &mm(&t1s), &mm(&p1));
    let h9s25 = || fuse::blend(0.25, &mm(&t1s), &mm(&s.pt));
    let h18 = || fuse::mean(&[&mm(&t1s), &mm(&s.c0), &mm(&s.pt)]);
    let keys = match name {
        "T1s" => scalar(&t1s),
        "P1" => scalar(&p1),
        "PT*" => scalar(&dense(&s.pt, n)),
        "C0" => scalar(&dense(&s.c0, n)),
        "H9s50" => scalar(&h9s50()),
        "H9s25[PT*]" => scalar(&h9s25()),
        "H18s[PT*]" => scalar(&h18()),
        "H10[PT*]" => lex2(&dense(&s.c0, n), &dense(&s.pt, n)),
        "ALL5" => scalar(&fuse::mean(&[
            &mm(&t1s),
            &mm(&s.w5),
            &mm(&s.m),
            &mm(&s.c0),
            &mm(&p1),
        ])),
        "LX[IDR,P1]" => lex2(&idr(), &p1),
        LX_IDR_H9S50 => lex2(&idr(), &h9s50()),
        "LX[IDR,H18s[PT*]]" => lex2(&idr(), &h18()),
        "LX[IDR,H9s25[PT*]]" => lex2(&idr(), &h9s25()),
        "H18si[PT*]" => scalar(&fuse::mean(&[&mm(&idr()), &mm(&t1s), &mm(&s.c0), &mm(&s.pt)])),
        "LX[IDR,C0,PT*]" => {
            let (i, c0, pt) = (idr(), dense(&s.c0, n), dense(&s.pt, n));
            (0..n).map(|f| [i[f], c0[f], pt[f]]).collect()
        }
        other => {
            let rest = other.strip_prefix("HTs")?;
            let (tag, w) = (rest.get(..1)?, rest.get(1..)?);
            match w {
                "[W5]" => ht(&s.w5, tag)?,
                "[M]" => ht(&s.m, tag)?,
                _ => return None,
            }
        }
    };
    Some(keys)
}

/// The non-excluded candidates of `keys`, best first: lexicographic,
/// descending, then `tie` ascending (`fuse::lex_order`, rank_eval's
/// `better`).
pub fn seeded_order(keys: &[RankKey], tie: &[u32], excluded: &[bool]) -> Vec<usize> {
    let cols: [Vec<f64>; 3] = [0, 1, 2].map(|j| keys.iter().map(|k| k[j]).collect());
    fuse::lex_order(&[&cols[0], &cols[1], &cols[2]], tie, excluded)
}

/// A seeded fusion of the review set, ranked from the replay's prefix state.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct Seeded(pub &'static str);

/// The replay's signals for `name`: P1 / PT* from the structure bank
/// (memo `p1` / `pt`), C0 from the query (memo `c0`, the bank's), T1s from
/// [`TextState`] (memo [`MEMO_T1S`]), W5 / M from the work state, IDR from
/// [`IdrState`] (memo [`MEMO_IDR`]; all zeros with no router registered).
/// None with no structure bank registered.
pub fn replay_signals(name: &str, q: &QueryCtx<'_>) -> Option<Signals> {
    let need = needs(name)?;
    let m = q.cands.len();
    let col0 = |n: &str| -> Option<Vec<f64>> {
        Some(structure_keys(n, q)?.into_iter().map(|k| k[0]).collect())
    };
    let mut s = Signals::zeros(m);
    s.p1 = col0("P1")?;
    if need.pt {
        s.pt = col0("PT*")?;
    }
    if need.c0 {
        s.c0 = q
            .memo
            .get_or("c0", || {
                let mut v = vec![0.0; m];
                for &(f, x) in &q.query.c0 {
                    if let Some(slot) = v.get_mut(f as usize) {
                        *slot = x;
                    }
                }
                v
            })
            .to_vec();
    }
    s.t1s = q
        .memo
        .get_or(MEMO_T1S, || match q.state.hook::<TextState>() {
            Some(t) if t.cur_sha == q.commit.sha => t.cur.to_vec(),
            _ => vec![0.0; m],
        })
        .to_vec();
    if need.w5 {
        s.w5 = w5_scores(q).to_vec();
    }
    if need.m {
        s.m = m_scores(q).to_vec();
    }
    if need.idr {
        s.idr = Some(
            q.memo
                .get_or(MEMO_IDR, || match q.state.hook::<IdrState>() {
                    Some(r) => r.scores(q),
                    None => vec![0.0; m],
                })
                .to_vec(),
        );
    }
    Some(s)
}

impl Candidate for Seeded {
    fn name(&self) -> &'static str {
        self.0
    }

    /// The fusion's keys over the replay's signals; an empty vector (the
    /// driver then drops the set) with no structure bank or an unknown name.
    fn rank(&self, q: &QueryCtx<'_>) -> Vec<RankKey> {
        replay_signals(self.0, q)
            .and_then(|s| seeded_keys(self.0, &s, q.excluded))
            .unwrap_or_default()
    }
}

/// The replay's text corpus (module docs).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TextState {
    n_files: usize,
    messages: bool,
    /// Every observed commit, oldest first: (sha, full message, files).
    docs: Vec<(String, String, Vec<u32>)>,
    /// The current test commit and its subject's scores.
    cur_sha: String,
    cur: Arc<Vec<f64>>,
    builds: usize,
    docs_max: usize,
    ms: f64,
}

impl TextState {
    /// The corpus of `replay` over `n_files` candidates; with no messages
    /// sidecar it scores nothing.
    pub fn for_replay(replay: &Replay, n_files: usize) -> TextState {
        TextState {
            n_files,
            messages: replay.messages,
            docs: Vec::new(),
            cur_sha: String::new(),
            cur: Arc::new(vec![0.0; n_files]),
            builds: 0,
            docs_max: 0,
            ms: 0.0,
        }
    }

    /// Indexes built (one per test commit with messages).
    pub fn builds(&self) -> usize {
        self.builds
    }

    /// The `[calibrate] text ..` marker, once after the replay.
    pub fn log_summary(&self) {
        eprintln!(
            "[calibrate] text builds={} docs_max={} messages={} ms={:.1}",
            self.builds,
            self.docs_max,
            if self.messages { "yes" } else { "no" },
            (self.ms * 10.0).trunc() / 10.0,
        );
    }
}

impl StateHook for TextState {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn begin_test_commit(&mut self, replay: &Replay, k: usize) {
        let Some(c) = replay.commits.get(k) else {
            return;
        };
        self.cur_sha = c.sha.clone();
        if !self.messages {
            self.cur = Arc::new(vec![0.0; self.n_files]);
            return;
        }
        let t0 = Instant::now();
        let idx = TextIndex::from_docs(
            self.n_files,
            self.docs
                .iter()
                .map(|(s, m, f)| (s.as_str(), m.as_str(), f.clone())),
        );
        self.cur = Arc::new(idx.scores(subject(&c.message)));
        self.builds += 1;
        self.docs_max = self.docs_max.max(idx.stats().docs);
        self.ms += t0.elapsed().as_secs_f64() * 1000.0;
    }

    fn observe(&mut self, c: &ReplayCommit, _train: bool) {
        if self.messages {
            self.docs
                .push((c.sha.clone(), c.message.clone(), c.files.clone()));
        }
    }

    fn subject_scores(&self, c: &ReplayCommit) -> Option<Vec<f64>> {
        (self.messages && c.sha == self.cur_sha).then(|| self.cur.to_vec())
    }
}

/// A subject's recent-scope list: at most this many files per older commit
/// (rank_eval `MAX_FILES`, EB.6's `recent_subjects` shape: 2..=30 files).
const RECENT_MAX_FILES: usize = 30;

/// What every routed test commit shares: the working tree's `[ids]`
/// vocabulary (EB.4, as EB.7 reads it), the graph's PROJECT dirs, and the
/// path resolver over the candidates and the history snapshot's renames.
struct IdrCtx {
    label: String,
    vocab: IdVocab,
    dirs: Vec<String>,
    paths: PathResolver,
}

impl IdrCtx {
    /// The context of `root`'s repo over `cands`. The renames are the
    /// on-disk history snapshot's ([`rename_pairs`]), none without one.
    fn new(root: &Path, merged: &MergedGraph, cands: &Candidates) -> IdrCtx {
        let renames = read_history(root)
            .as_ref()
            .map(rename_pairs)
            .unwrap_or_default();
        IdrCtx {
            label: root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            vocab: IdVocab::for_root(root),
            dirs: project_roots(merged)
                .into_iter()
                .map(|p| p.path)
                .filter(|p| p != ".")
                .collect(),
            paths: PathResolver::new(cands, &renames),
        }
    }

    /// The IDR of `subj` read at commit `sha`'s parent: Err when the parent
    /// cannot be resolved (no git, an unknown sha, a root commit). A subject
    /// with no id mention and no conventional-commit scope routes nothing
    /// (all zeros) without reading the tree, exactly as [`route`] would.
    fn route_at_parent(
        &self,
        root: &Path,
        cands: &Candidates,
        sha: &str,
        subj: &str,
        older_newest_first: &[&str],
    ) -> Result<Vec<f64>, String> {
        let parent = resolve_rev(root, &format!("{sha}^"))?;
        if self.vocab.mentions(subj).is_empty() && conventional_scopes(subj).is_empty() {
            return Ok(vec![0.0; cands.len()]);
        }
        let tree = Tree::Rev { repo: root, sha: parent.sha };
        let dirs: Vec<&str> = self.dirs.iter().map(String::as_str).collect();
        let defs = read_defs(&tree, &dirs, &self.vocab, None);
        let ns = defs.namespaces(&self.vocab.local_words);
        let side = IdrSide::new(cands, &self.paths, &defs, &ns, &self.vocab, &tree)
            .labelled(&self.label, "calibrate");
        let recent = recent_scopes(older_newest_first, &ns, &self.vocab);
        Ok(route(subj, &side, &recent).idr)
    }
}

/// The IDR of `subj` for test commit `sha`, read leak-free at the commit's
/// PARENT revision (rank_eval `r2_build`): EB.6's [`route`] over that tree's
/// definitions (no Tier-4 snapshot), the working tree's `[ids]` vocabulary,
/// `merged`'s PROJECT dirs and the path resolver over `cands` (renames from
/// `root`'s history snapshot), with [`recent_scopes`] over
/// `older_subjects_newest_first`. Err when the parent cannot be resolved.
/// The replay's [`IdrState`] reads exactly this, once per test commit.
pub fn idr_at_parent(
    root: &Path,
    merged: &MergedGraph,
    cands: &Candidates,
    sha: &str,
    subj: &str,
    older_subjects_newest_first: &[&str],
) -> Result<Vec<f64>, String> {
    IdrCtx::new(root, merged, cands).route_at_parent(root, cands, sha, subj, older_subjects_newest_first)
}

/// The replay's id router (EC.12, module docs): per test commit, the subject
/// routed at the parent revision ([`idr_at_parent`]), computed by the first
/// query that reads IDR and shared by the commit's every query and candidate
/// (memo [`MEMO_IDR`]).
#[non_exhaustive]
pub struct IdrState {
    n_files: usize,
    /// The observed commits' subjects shaped like a test commit (2..=30
    /// files, a subject), oldest first.
    subjects: Vec<String>,
    cur_sha: String,
    cur_subject: String,
    /// The subjects strictly older than the current test commit, newest first.
    cur_older: Vec<String>,
    /// The current test commit's IDR, once computed.
    cur: Mutex<Option<Arc<Vec<f64>>>>,
    /// Built by the first routed commit.
    ctx: Mutex<Option<IdrCtx>>,
    /// (routed, unavailable) test commits.
    counts: Mutex<(usize, usize)>,
}

impl std::fmt::Debug for IdrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (routed, unavailable) = self.counts();
        f.debug_struct("IdrState")
            .field("n_files", &self.n_files)
            .field("subjects", &self.subjects.len())
            .field("routed", &routed)
            .field("unavailable", &unavailable)
            .finish()
    }
}

impl IdrState {
    /// The router of `replay` over `n_files` candidates.
    pub fn for_replay(_replay: &Replay, n_files: usize) -> IdrState {
        IdrState {
            n_files,
            subjects: Vec::new(),
            cur_sha: String::new(),
            cur_subject: String::new(),
            cur_older: Vec::new(),
            cur: Mutex::new(None),
            ctx: Mutex::new(None),
            counts: Mutex::new((0, 0)),
        }
    }

    /// (test commits routed at a readable parent, test commits whose parent
    /// could not be read).
    pub fn counts(&self) -> (usize, usize) {
        *self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The `[calibrate] idr unavailable commits=<n>` marker, once after the
    /// replay, only when n > 0.
    pub fn log_summary(&self) {
        let (_, unavailable) = self.counts();
        if unavailable > 0 {
            eprintln!("[calibrate] idr unavailable commits={unavailable}");
        }
    }

    /// The current test commit's IDR over `q.cands` (zeros when its parent
    /// cannot be read, or for a query of another commit).
    pub fn scores(&self, q: &QueryCtx<'_>) -> Vec<f64> {
        let m = q.cands.len();
        if q.commit.sha != self.cur_sha {
            return vec![0.0; m];
        }
        let mut cur = self.cur.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(v) = cur.as_ref() {
            return v.to_vec();
        }
        let got = {
            let mut ctx = self.ctx.lock().unwrap_or_else(PoisonError::into_inner);
            let ctx = ctx.get_or_insert_with(|| IdrCtx::new(q.repo_root, q.merged, q.cands));
            let older: Vec<&str> = self.cur_older.iter().map(String::as_str).collect();
            ctx.route_at_parent(q.repo_root, q.cands, &self.cur_sha, &self.cur_subject, &older)
        };
        let v = {
            let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
            match got {
                Ok(v) if v.len() == m => {
                    counts.0 += 1;
                    v
                }
                _ => {
                    counts.1 += 1;
                    vec![0.0; m]
                }
            }
        };
        let v = Arc::new(v);
        *cur = Some(Arc::clone(&v));
        v.to_vec()
    }
}

impl StateHook for IdrState {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn begin_test_commit(&mut self, replay: &Replay, k: usize) {
        let Some(c) = replay.commits.get(k) else {
            return;
        };
        self.cur_sha = c.sha.clone();
        self.cur_subject = c.subject.clone();
        self.cur_older = self.subjects.iter().rev().cloned().collect();
        *self.cur.get_mut().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn observe(&mut self, c: &ReplayCommit, _train: bool) {
        if !c.subject.is_empty() && (2..=RECENT_MAX_FILES).contains(&c.files.len()) {
            self.subjects.push(c.subject.clone());
        }
    }
}

/// What a review's intent reads for the calibrated selection, beyond the
/// signals EB.7 computes itself (P1, T1s, IDR).
#[non_exhaustive]
pub struct AnswerInputs<'a> {
    pub merged: &'a MergedGraph,
    pub cache: &'a RankCache,
    /// The repo's candidate files.
    pub cands: &'a Candidates,
    /// The changed files, in the rank cache's file space, seed order.
    pub seed_files: &'a [u32],
    /// Seed nodes whose file is no candidate.
    pub extra: &'a [NodeId],
    /// The PR's own commits, never training commits.
    pub exclude: &'a BTreeSet<String>,
}

/// The answer's signals for the fusion `name` beyond the ones the review
/// computes itself (EB.7's P1, T1s and IDR, left empty here): only when
/// `name` reads them, PT* (`rank_files` under `PT*`, the bank's arithmetic),
/// C0 (the rank cache's co-change of the changed files) and W5 / M
/// ([`work_at_head`] with the PR's commits left out, `now` the newest PR
/// commit's author time, else the newest other snapshot commit's). One code
/// path with the replay: the caller ranks the result through
/// [`seeded_keys`].
pub fn answer_signals(
    name: &str,
    x: &AnswerInputs<'_>,
    now_at: Option<i64>,
    history: Option<(&HistorySnapshot, &[HistoryMessage])>,
) -> Signals {
    let n = x.cands.len();
    let need = needs(name).unwrap_or_default();
    let mut s = Signals::zeros(n);
    let to_cands = |v: &dyn Fn(u32) -> f64| -> Vec<f64> { x.cands.files.iter().map(|&f| v(f)).collect() };
    if need.pt && !(x.seed_files.is_empty() && x.extra.is_empty()) {
        let fr = rank_files(
            x.merged,
            x.cache,
            x.seed_files,
            x.extra,
            Ranking::new(RankBase::PprWide, false),
        );
        s.pt = to_cands(&|f| fr.keys.get(f as usize).map_or(0.0, |k| k[0]));
    }
    if need.c0 {
        let files = x.cache.files(x.merged);
        let v = c0(&*x.cache.cochange(x.merged), x.seed_files, files.len());
        s.c0 = to_cands(&|f| at(&v, f as usize));
    }
    if (need.w5 || need.m)
        && let Some((snap, msgs)) = history
    {
        let now = now_at.unwrap_or_else(|| {
            let aligned = msgs.len() == snap.commits.len() && !msgs.is_empty();
            snap.commits
                .iter()
                .enumerate()
                .find(|(_, c)| !x.exclude.contains(&c.c))
                .map_or(0, |(i, c)| if aligned { msgs[i].at } else { c.t })
        });
        let (w5, m) = work_at_head(snap, msgs, x.cands, x.exclude, now, None, "review");
        s.w5 = w5;
        s.m = m;
    }
    s.p1 = Vec::new();
    s.t1s = Vec::new();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- rank_eval.rs's own functions, verbatim, for the parity test ----
    fn mm_ex(v: &[f64], excl: &[bool]) -> Vec<f64> {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for (f, &s) in v.iter().enumerate() {
            if !excl[f] {
                lo = lo.min(s);
                hi = hi.max(s);
            }
        }
        if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
            return vec![0.0; v.len()];
        }
        v.iter().map(|&s| (s - lo) / (hi - lo)).collect()
    }
    fn blend(a: f64, x: &[f64], y: &[f64]) -> Vec<f64> {
        x.iter().zip(y).map(|(p, q)| a * p + (1.0 - a) * q).collect()
    }
    fn r3_eq(parts: &[&[f64]]) -> Vec<f64> {
        let nf = parts[0].len();
        (0..nf)
            .map(|f| parts.iter().map(|p| p[f]).sum::<f64>() / parts.len() as f64)
            .collect()
    }

    fn sig() -> (Signals, Vec<bool>) {
        let s = Signals {
            p1: vec![0.30, 0.02, 0.11, 0.0, 0.07, 0.11],
            pt: vec![1.10, 0.05, 0.40, 0.003, 0.21, 0.33],
            c0: vec![0.5, 0.0, 1.25, 0.0, 0.0, 0.75],
            t1s: vec![0.0, 3.2, 0.0, 7.9, 1.1, 0.0],
            w5: vec![1.0, 0.0, 0.75, 0.0, 1.3125, 0.0],
            m: vec![0.0, 0.0, 2.0, 1.0, 0.0, 1.0],
            idr: Some(vec![0.0, 0.0, 1.0, 0.0, 0.5, 0.0]),
        };
        (s, vec![true, false, false, false, false, false])
    }

    /// The harness arithmetic of every SELt2 name, the fallback and
    /// LX[IDR,H9s50] (r3_bank_seed's `full` bank and round1_rankers').
    fn harness(s: &Signals, ex: &[bool]) -> Vec<(&'static str, Vec<RankKey>)> {
        let nf = s.p1.len();
        let mm = |v: &[f64]| mm_ex(v, ex);
        let (mm_p1, mm_pt, mm_c0, mm_t1s, mm_w5, mm_m) =
            (mm(&s.p1), mm(&s.pt), mm(&s.c0), mm(&s.t1s), mm(&s.w5), mm(&s.m));
        let h9s50 = blend(0.5, &mm_t1s, &mm_p1);
        let idr = s.idr.clone().unwrap_or_default();
        let mm_idr = mm(&idr);
        let h9s25 = blend(0.25, &mm_t1s, &mm_pt);
        let h18 = r3_eq(&[&mm_t1s, &mm_c0, &mm_pt]);
        let lx3 = |a: &[f64], b: &[f64], c: &[f64]| -> Vec<RankKey> { (0..nf).map(|f| [a[f], b[f], c[f]]).collect() };
        let ht = |w: &[f64], i: usize| {
            let (_, a, b, c) = HT_GRID[i];
            scalar(&(0..nf).map(|f| a * mm_t1s[f] + b * w[f] + c * mm_p1[f]).collect::<Vec<_>>())
        };
        vec![
            ("H18s[PT*]", scalar(&r3_eq(&[&mm_t1s, &mm_c0, &mm_pt]))),
            ("H9s25[PT*]", scalar(&blend(0.25, &mm_t1s, &mm_pt))),
            ("H9s50", scalar(&h9s50)),
            ("H10[PT*]", lex2(&s.c0, &s.pt)),
            ("HTsa[W5]", ht(&mm_w5, 0)),
            ("HTsc[W5]", ht(&mm_w5, 2)),
            ("HTsd[W5]", ht(&mm_w5, 3)),
            ("HTsc[M]", ht(&mm_m, 2)),
            ("HTsd[M]", ht(&mm_m, 3)),
            ("ALL5", scalar(&r3_eq(&[&mm_t1s, &mm_w5, &mm_m, &mm_c0, &mm_p1]))),
            ("LX[IDR,P1]", lex2(&idr, &s.p1)),
            ("LX[IDR,H9s50]", lex2(&idr, &h9s50)),
            // EC.12: r3_bank_seed's id-routed fusions, SELt2's last four
            ("LX[IDR,H18s[PT*]]", lex2(&idr, &h18)),
            ("LX[IDR,H9s25[PT*]]", lex2(&idr, &h9s25)),
            ("H18si[PT*]", scalar(&r3_eq(&[&mm_idr, &mm_t1s, &mm_c0, &mm_pt]))),
            ("LX[IDR,C0,PT*]", lx3(&idr, &s.c0, &s.pt)),
        ]
    }

    fn bits(k: &[RankKey]) -> Vec<[u64; 3]> {
        k.iter().map(|x| x.map(f64::to_bits)).collect()
    }

    #[test]
    fn seeded_keys_are_the_harness_arithmetic() {
        let (s, ex) = sig();
        for (name, want) in harness(&s, &ex) {
            let got = seeded_keys(name, &s, &ex).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(bits(&got), bits(&want), "{name}");
        }
        // values computed once with the harness functions
        let h9 = seeded_keys("H9s50", &s, &ex).expect("h9");
        assert_eq!(h9[3][0], 0.5);
        assert_eq!(h9[2][0], 0.5 * 0.0 + 0.5 * 1.0);
        let all5 = seeded_keys("ALL5", &s, &ex).expect("all5");
        assert!((all5[2][0] - (0.0 + 0.75 / 1.3125 + 1.0 + 1.0 + 1.0) / 5.0).abs() < 1e-15);
        assert_eq!(seeded_keys("HTsa[W5]", &s, &ex).map(|k| k.len()), Some(6));
        assert!(seeded_keys("B0", &s, &ex).is_none());
        assert!(seeded_keys("HTse[W5]", &s, &ex).is_none());
    }

    #[test]
    fn every_seeded_name_ranks_and_flat_signals_tie_to_position() {
        let (s, ex) = sig();
        for name in SEEDED_NAMES.iter().chain([&LX_IDR_H9S50]) {
            assert_eq!(seeded_keys(name, &s, &ex).map(|k| k.len()), Some(6), "{name}");
        }
        let z = Signals::zeros(4);
        let none = [false; 4];
        let tie = [0, 1, 2, 3];
        for name in SEEDED_NAMES {
            let k = seeded_keys(name, &z, &none).expect(name);
            assert_eq!(seeded_order(&k, &tie, &none), [0, 1, 2, 3], "{name}");
        }
    }

    /// LX[IDR,P1] through the bank orders exactly as EB.7's `fuse::lx`.
    #[test]
    fn the_fallback_order_is_eb7s() {
        let (s, ex) = sig();
        let tie: Vec<u32> = (0..6).collect();
        let k = seeded_keys("LX[IDR,P1]", &s, &ex).expect("lx");
        let idr = s.idr.clone().expect("idr");
        assert_eq!(
            seeded_order(&k, &tie, &ex),
            fuse::lx(&idr, &[&s.p1], &tie, &ex)
        );
        assert_eq!(seeded_order(&k, &tie, &ex), [2, 4, 5, 1, 3]);
    }

    /// EC.12 (item 8's measured no-op): with no id routed (IDR all zero or
    /// absent) each LX[IDR, X] orders exactly as X, and H18si[PT*] as
    /// H18s[PT*] (its mm(IDR) is flat 0, so it is 3/4 of H18s[PT*]).
    #[test]
    fn idr_fusions_without_an_id_order_as_their_base() {
        let (mut s, ex) = sig();
        let tie: Vec<u32> = (0..6).collect();
        for idr in [None, Some(vec![0.0; 6])] {
            s.idr = idr;
            let ord = |n: &str| seeded_order(&seeded_keys(n, &s, &ex).expect(n), &tie, &ex);
            for (a, b) in [
                ("LX[IDR,H18s[PT*]]", "H18s[PT*]"),
                ("LX[IDR,H9s25[PT*]]", "H9s25[PT*]"),
                ("H18si[PT*]", "H18s[PT*]"),
                ("LX[IDR,C0,PT*]", "H10[PT*]"),
                ("LX[IDR,P1]", "P1"),
            ] {
                assert_eq!(ord(a), ord(b), "{a} vs {b}");
            }
        }
        // a routed id leads: file 2 (idr 1.0), then 4 (idr 0.5)
        let (s, ex) = sig();
        for n in ["LX[IDR,H18s[PT*]]", "LX[IDR,H9s25[PT*]]", "LX[IDR,C0,PT*]"] {
            let k = seeded_keys(n, &s, &ex).expect(n);
            assert_eq!(seeded_order(&k, &tie, &ex)[..2], [2, 4], "{n}");
        }
        for n in ["LX[IDR,H18s[PT*]]", "LX[IDR,H9s25[PT*]]", "H18si[PT*]", "LX[IDR,C0,PT*]", "LX[IDR,P1]"] {
            assert!(needs(n).is_some_and(|x| x.idr), "{n}");
        }
        assert!(needs("H18s[PT*]").is_some_and(|x| !x.idr));
    }

    /// The router's prefix bookkeeping: older test-shaped subjects only,
    /// newest first; a tree that is no git repo leaves IDR unavailable.
    #[test]
    fn idr_state_keeps_older_subjects_and_counts_unavailable() {
        let commit = |sha: &str, subj: &str, files: Vec<u32>| ReplayCommit {
            sha: sha.to_string(),
            subject: subj.to_string(),
            message: format!("{subj}\n"),
            files,
            ..ReplayCommit::default()
        };
        let commits = vec![
            commit("a", "T001 first", vec![0, 1]),
            commit("b", "one file", vec![2]),
            commit("c", "", vec![1, 2]),
            commit("d", "T002 second", vec![0, 2]),
            commit("e", "T003 third", vec![1, 3]),
        ];
        let replay = Replay { train: vec![true; 5], tests: vec![4], messages: true, commits, ..Replay::default() };
        let mut r = IdrState::for_replay(&replay, 4);
        for c in &replay.commits[..4] {
            r.observe(c, true);
        }
        r.begin_test_commit(&replay, 4);
        assert_eq!(r.cur_older, ["T002 second", "T001 first"]);
        assert_eq!(r.cur_subject, "T003 third");
        let dir = tempfile::tempdir().expect("tmp");
        let cands = Candidates {
            paths: (0..4).map(|i| format!("f{i}.py")).collect(),
            ..Candidates::default()
        };
        let ctx = IdrCtx {
            label: "t".into(),
            vocab: IdVocab::default(),
            dirs: Vec::new(),
            paths: PathResolver::new(&cands, &[]),
        };
        assert!(ctx.route_at_parent(dir.path(), &cands, "e", "T003 third", &[]).is_err());
    }

    #[test]
    fn text_state_reads_only_older_commits() {
        let commit = |sha: &str, msg: &str, files: Vec<u32>| ReplayCommit {
            sha: sha.to_string(),
            subject: subject(msg).to_string(),
            message: msg.to_string(),
            files,
            ..ReplayCommit::default()
        };
        let commits = vec![
            commit("a", "codec rewrite\n", vec![0, 1]),
            commit("b", "parser fix\n", vec![2]),
            commit("c", "codec parser speedup\n", vec![1, 3]),
        ];
        let replay = Replay {
            train: vec![true; 3],
            tests: vec![2],
            messages: true,
            commits,
            ..Replay::default()
        };
        let mut t = TextState::for_replay(&replay, 4);
        t.observe(&replay.commits[0], true);
        t.observe(&replay.commits[1], true);
        t.begin_test_commit(&replay, 2);
        let s = t.subject_scores(&replay.commits[2]).expect("scores");
        // file 3 is only in the test commit itself: never scored
        assert_eq!(s[3], 0.0);
        assert!(s[0] > 0.0 && s[1] > 0.0 && s[2] > 0.0, "{s:?}");
        assert!(t.subject_scores(&replay.commits[1]).is_none());
        assert_eq!(t.builds(), 1);
        // no messages: nothing scores, nothing is built
        let mut bare = TextState::for_replay(&Replay { messages: false, ..replay.clone() }, 4);
        bare.observe(&replay.commits[0], true);
        bare.begin_test_commit(&replay, 2);
        assert!(bare.subject_scores(&replay.commits[2]).is_none());
        assert_eq!(bare.builds(), 0);
    }
}
