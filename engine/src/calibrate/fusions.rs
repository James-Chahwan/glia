//! Review's text selection (EC.11, item 4): the seeded fusion bank of rank_eval
//! `r3_bank_seed` / `round1_rankers` over text (T1s), structure (P1, PT*),
//! co-change (C0) and the work signals (W5, M), the candidates of set `SELt`
//! (rank_eval `R1_SEL`), and the replay's text corpus.
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
//!   first. The replay has no router yet (EC.12): its IDR is all zeros.
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
//! `review` group ([`TextState::log_summary`]).

use std::any::Any;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use glia_activation::algo::fuse;
use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot};
use glia_core::NodeId;
use glia_graph::MergedGraph;

use super::bank::structure_keys;
use super::candidate::{Candidate, QueryCtx, RankKey, StateHook};
use super::replay::{Replay, ReplayCommit};
use super::work::{m_scores, w5_scores, work_at_head};
use crate::intent::text::{Candidates, TextIndex, subject};
use crate::rank::{RankBase, RankCache, Ranking, c0, rank_files};

/// The memo key of a query's dense T1s vector.
pub const MEMO_T1S: &str = "t1s";

/// rank_eval's `HT_GRID` rows SELt reads: (tag, a T1s, b W, c P1).
pub const HT_GRID: [(&str, f64, f64, f64); 4] = [
    ("a", 0.5, 0.25, 0.25),
    ("b", 0.25, 0.5, 0.25),
    ("c", 0.25, 0.25, 0.5),
    ("d", 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0),
];

/// Every name [`seeded_keys`] ranks, in this order.
pub const SEEDED_NAMES: [&str; 18] = [
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
    /// Typed id routing; None (all zeros) in the replay until EC.12.
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

/// The signals a fusion reads beyond P1 / T1s / IDR (which an answer always
/// has): PT*, C0, W5, M.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs {
    pub pt: bool,
    pub c0: bool,
    pub w5: bool,
    pub m: bool,
}

/// What `name` reads; None for a name [`seeded_keys`] does not rank.
pub fn needs(name: &str) -> Option<Needs> {
    let n = |pt, c0, w5, m| Some(Needs { pt, c0, w5, m });
    match name {
        "T1s" | "P1" | "H9s50" | "LX[IDR,P1]" | LX_IDR_H9S50 => n(false, false, false, false),
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
    let keys = match name {
        "T1s" => scalar(&t1s),
        "P1" => scalar(&p1),
        "PT*" => scalar(&dense(&s.pt, n)),
        "C0" => scalar(&dense(&s.c0, n)),
        "H9s50" => scalar(&h9s50()),
        "H9s25[PT*]" => scalar(&fuse::blend(0.25, &mm(&t1s), &mm(&s.pt))),
        "H18s[PT*]" => scalar(&fuse::mean(&[&mm(&t1s), &mm(&s.c0), &mm(&s.pt)])),
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
/// [`TextState`] (memo [`MEMO_T1S`]), W5 / M from the work state; IDR none.
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

    /// The harness arithmetic of every SELt name, the fallback and
    /// LX[IDR,H9s50] (r3_bank_seed's `full` bank and round1_rankers').
    fn harness(s: &Signals, ex: &[bool]) -> Vec<(&'static str, Vec<RankKey>)> {
        let nf = s.p1.len();
        let mm = |v: &[f64]| mm_ex(v, ex);
        let (mm_p1, mm_pt, mm_c0, mm_t1s, mm_w5, mm_m) =
            (mm(&s.p1), mm(&s.pt), mm(&s.c0), mm(&s.t1s), mm(&s.w5), mm(&s.m));
        let h9s50 = blend(0.5, &mm_t1s, &mm_p1);
        let idr = s.idr.clone().unwrap_or_default();
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
