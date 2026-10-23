//! The work signals (EC.10, item 4's W5 and M): what the repo was just
//! working on, read from the replay's training commits only.
//!
//! rank_eval.rs's round-1 `R1State` is the specification; this module
//! reproduces its arithmetic (accumulation order, the segment rule, the
//! source preference) for the parts the engine ports:
//!
//! - **W5** (`r1_window` with k = 5): over the training commits so far, the
//!   j-th newest (j = 1) weighs [`W_DECAY`]^(j-1); a file's weight is the sum
//!   over the last [`W_K`] commits that touched it.
//! - **M** (`R1State::test`): Mbr, the files every training commit merged in
//!   on the test commit's branch label touched (1.0 per commit), else Mgap,
//!   the current segment's files when the test commit's time is within
//!   [`GAP_S`] of the newest training commit, else nothing. A training commit
//!   continues the current segment when it is within [`GAP_S`] of the
//!   previous training commit AND shares a file with the segment's last
//!   [`OVERLAP_LAST`] commits; otherwise it opens a new one. The harness's
//!   Mid (labels from a repo's own plan ids) is repo-specific and not ported.
//!
//! Times are the author time (EB.1's messages sidecar, `HistoryMessage.at`,
//! 0 when the message log did not list a commit, as the harness reads it);
//! without a sidecar (a version-1 snapshot or `--no-messages`) the committer
//! time stands in and no commit has a branch label, an unmeasured variant the
//! markers name `work=committer_time`.
//!
//! Every map is a BTreeMap, so no output depends on hash order.
//!
//! Fired_on, stderr: `[calibrate] work trained=<t> segments=<s> labels=<l>
//! m_src mbr=<a> mgap=<b> none=<c> work=<author_time|committer_time>` once
//! per calibrate run ([`WorkState::log_summary`]), and `[work] head
//! w5_files=<n> m_files=<m> m_src=<Mbr|Mgap|none> surface=<review|find>`
//! once per answer that reads them ([`work_at_head`]).

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot};

use super::candidate::{QueryCtx, StateHook};
use super::replay::{Replay, ReplayCommit, ReplayParams};
use crate::intent::text::Candidates;

/// rank_eval's `R1_DECAY`: the j-th newest training commit weighs
/// `W_DECAY^(j-1)`.
pub const W_DECAY: f64 = 0.75;
/// The window W5 reads: the last this-many training commits.
pub const W_K: usize = 5;
/// rank_eval's `R1_GAP_S`: a commit continues the current milestone only
/// within this gap (seconds).
pub const GAP_S: i64 = 24 * 3600;
/// rank_eval's `R1_OVERLAP_LAST`: ... and only when it shares a file with the
/// milestone's last this-many commits.
pub const OVERLAP_LAST: usize = 3;

/// The memo key of a query's dense W5 vector.
pub const MEMO_W5: &str = "w5";
/// The memo key of a query's dense M vector.
pub const MEMO_M: &str = "m";

/// The replay's work state: the training commits so far, the Mgap segments
/// and the branch-label milestones.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct WorkState {
    /// Candidate files (the dense vectors' length).
    n_files: usize,
    /// The widest commit that trains.
    max_files: usize,
    /// Author time (true) or committer time (false).
    author_time: bool,
    /// Files of every training commit so far, oldest first.
    recent: Vec<Vec<u32>>,
    /// Each training commit's time, aligned with `recent`.
    recent_at: Vec<i64>,
    /// Per segment, its positions in `recent`.
    seg_pos: Vec<Vec<usize>>,
    /// Per segment, 1.0 per (commit, file).
    seg_files: Vec<BTreeMap<u32, f64>>,
    /// Per label (`br:<label>`), 1.0 per (commit, file).
    lab_files: BTreeMap<String, BTreeMap<u32, f64>>,
    /// The current test commit's dense W5 / M and M's source, set by
    /// `begin_test_commit`.
    cur_w5: Arc<Vec<f64>>,
    cur_m: Arc<Vec<f64>>,
    cur_src: &'static str,
    /// Test commits per M source: Mbr, Mgap, none.
    src_counts: [usize; 3],
}

impl WorkState {
    /// An empty state over `n_files` candidate files; `author_time` says
    /// whether commit times are author times (a messages sidecar was read).
    pub fn new(n_files: usize, max_files: usize, author_time: bool) -> WorkState {
        WorkState {
            n_files,
            max_files,
            author_time,
            recent: Vec::new(),
            recent_at: Vec::new(),
            seg_pos: Vec::new(),
            seg_files: Vec::new(),
            lab_files: BTreeMap::new(),
            cur_w5: Arc::new(vec![0.0; n_files]),
            cur_m: Arc::new(vec![0.0; n_files]),
            cur_src: "none",
            src_counts: [0; 3],
        }
    }

    /// The state for `replay` over `n_files` candidates: author time when
    /// the replay read the messages sidecar, committer time otherwise.
    pub fn for_replay(replay: &Replay, n_files: usize, max_files: usize) -> WorkState {
        WorkState::new(n_files, max_files, replay.messages)
    }

    /// The time this state reads for `c`.
    pub fn time_of(&self, c: &ReplayCommit) -> i64 {
        if self.author_time { c.at } else { c.t }
    }

    /// Fold one training commit in (rank_eval's `R1State::train`, the parts
    /// W and M read). A commit with no file or more than `max_files` files
    /// trains nothing.
    pub fn train(&mut self, files: &[u32], at: i64, br: Option<&str>) {
        if files.is_empty() || files.len() > self.max_files {
            return;
        }
        let pos = self.recent.len();
        let cont = match self.recent_at.last() {
            None => false,
            Some(&prev) => {
                let s = self.seg_pos.len() - 1;
                let last: BTreeSet<u32> = self.seg_pos[s]
                    .iter()
                    .rev()
                    .take(OVERLAP_LAST)
                    .flat_map(|&p| self.recent[p].iter().copied())
                    .collect();
                at - prev <= GAP_S && files.iter().any(|f| last.contains(f))
            }
        };
        if !cont {
            self.seg_pos.push(Vec::new());
            self.seg_files.push(BTreeMap::new());
        }
        let s = self.seg_pos.len() - 1;
        self.seg_pos[s].push(pos);
        for &f in files {
            *self.seg_files[s].entry(f).or_default() += 1.0;
        }
        if let Some(b) = br {
            let e = self.lab_files.entry(format!("br:{b}")).or_default();
            for &f in files {
                *e.entry(f).or_default() += 1.0;
            }
        }
        self.recent.push(files.to_vec());
        self.recent_at.push(at);
    }

    /// W5 (rank_eval's `r1_window(recent, n - 5, n)`): sorted (file, weight).
    pub fn w5(&self) -> Vec<(u32, f64)> {
        let n = self.recent.len();
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        for (j, p) in (n.saturating_sub(W_K)..n).rev().enumerate() {
            let w = W_DECAY.powi(j as i32);
            for &f in &self.recent[p] {
                *acc.entry(f).or_default() += w;
            }
        }
        acc.into_iter().collect()
    }

    /// M for a commit at `at` merged in on `br` (rank_eval's
    /// `R1State::test`, Mid not ported): sorted (file, weight) and the
    /// source, `Mbr`, `Mgap` or `none`.
    pub fn m(&self, at: i64, br: Option<&str>) -> (Vec<(u32, f64)>, &'static str) {
        if let Some(files) = br.and_then(|b| self.lab_files.get(&format!("br:{b}")))
            && !files.is_empty()
        {
            return (files.iter().map(|(&f, &v)| (f, v)).collect(), "Mbr");
        }
        let joined = self.recent_at.last().is_some_and(|&prev| at - prev <= GAP_S);
        if joined && let Some(seg) = self.seg_files.last() && !seg.is_empty() {
            return (seg.iter().map(|(&f, &v)| (f, v)).collect(), "Mgap");
        }
        (Vec::new(), "none")
    }

    /// Training commits folded in.
    pub fn trained(&self) -> usize {
        self.recent.len()
    }

    /// Segments opened.
    pub fn segments(&self) -> usize {
        self.seg_pos.len()
    }

    /// Branch labels seen.
    pub fn labels(&self) -> usize {
        self.lab_files.len()
    }

    /// Test commits per M source so far: (Mbr, Mgap, none).
    pub fn source_counts(&self) -> (usize, usize, usize) {
        (self.src_counts[0], self.src_counts[1], self.src_counts[2])
    }

    /// The current test commit's M source (set by `begin_test_commit`).
    pub fn current_source(&self) -> &'static str {
        self.cur_src
    }

    /// The `[calibrate] work ..` marker; the calibrate run that registered
    /// this state calls it once, after the replay.
    pub fn log_summary(&self) {
        let (a, b, c) = self.source_counts();
        eprintln!(
            "[calibrate] work trained={} segments={} labels={} m_src mbr={a} mgap={b} none={c} work={}",
            self.trained(),
            self.segments(),
            self.labels(),
            work_clock(self.author_time),
        );
    }

    fn dense(&self, sparse: &[(u32, f64)]) -> Vec<f64> {
        dense(self.n_files, sparse)
    }
}

impl StateHook for WorkState {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn begin_test_commit(&mut self, replay: &Replay, k: usize) {
        let Some(c) = replay.commits.get(k) else {
            return;
        };
        let (m, src) = self.m(self.time_of(c), c.br.as_deref());
        self.cur_w5 = Arc::new(self.dense(&self.w5()));
        self.cur_m = Arc::new(self.dense(&m));
        self.cur_src = src;
        self.src_counts[match src {
            "Mbr" => 0,
            "Mgap" => 1,
            _ => 2,
        }] += 1;
    }

    fn observe(&mut self, c: &ReplayCommit, train: bool) {
        if train {
            let at = self.time_of(c);
            self.train(&c.files, at, c.br.as_deref());
        }
    }
}

fn work_clock(author_time: bool) -> &'static str {
    if author_time { "author_time" } else { "committer_time" }
}

fn dense(n: usize, sparse: &[(u32, f64)]) -> Vec<f64> {
    let mut v = vec![0.0; n];
    for &(f, w) in sparse {
        if let Some(x) = v.get_mut(f as usize) {
            *x = w;
        }
    }
    v
}

/// The query's dense W5 over its candidates, through memo key [`MEMO_W5`]
/// (the same for every query of one test commit); zeros when no
/// [`WorkState`] is registered.
pub fn w5_scores(q: &QueryCtx<'_>) -> Arc<Vec<f64>> {
    q.memo.get_or(MEMO_W5, || match q.state.hook::<WorkState>() {
        Some(w) => w.cur_w5.to_vec(),
        None => vec![0.0; q.cands.len()],
    })
}

/// The query's dense M over its candidates, through memo key [`MEMO_M`];
/// zeros when no [`WorkState`] is registered.
pub fn m_scores(q: &QueryCtx<'_>) -> Arc<Vec<f64>> {
    q.memo.get_or(MEMO_M, || match q.state.hook::<WorkState>() {
        Some(w) => w.cur_m.to_vec(),
        None => vec![0.0; q.cands.len()],
    })
}

/// W5 and M for an answer, dense over `cands`: replays [`WorkState::train`]
/// over the snapshot's training commits (1..=`ReplayParams::max_files`
/// candidate files) not in `exclude` (the PR's own commits for review; none
/// for find), oldest first, then reads `w5()` and `m(now_at, br)`. Times are
/// the author times when `messages` aligns with the snapshot (EB.1's
/// sidecar), else the committer times with no branch labels. `surface`
/// names the answer in the `[work] head ..` marker.
pub fn work_at_head(
    snapshot: &HistorySnapshot,
    messages: &[HistoryMessage],
    cands: &Candidates,
    exclude: &BTreeSet<String>,
    now_at: i64,
    br: Option<&str>,
    surface: &str,
) -> (Vec<f64>, Vec<f64>) {
    let max_files = ReplayParams::default().max_files;
    let msgs = (!messages.is_empty() && messages.len() == snapshot.commits.len()).then_some(messages);
    let mut st = WorkState::new(cands.len(), max_files, msgs.is_some());
    for (i, c) in snapshot.commits.iter().enumerate().rev() {
        if exclude.contains(&c.c) {
            continue;
        }
        let mut files: Vec<u32> = c
            .files
            .iter()
            .filter_map(|f| cands.position(&f.p))
            .map(|p| p as u32)
            .collect();
        files.sort_unstable();
        files.dedup();
        let (at, cbr) = match msgs.map(|m| &m[i]) {
            Some(m) => (m.at, m.br.as_deref()),
            None => (c.t, None),
        };
        st.train(&files, at, cbr);
    }
    let w5 = st.w5();
    let (m, src) = st.m(now_at, br);
    eprintln!(
        "[work] head w5_files={} m_files={} m_src={src} surface={surface} work={}",
        w5.len(),
        m.len(),
        work_clock(st.author_time),
    );
    (dense(cands.len(), &w5), dense(cands.len(), &m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta};

    const H: i64 = 3600;

    fn close(a: &[(u32, f64)], b: &[(u32, f64)]) {
        assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x.0, y.0, "{a:?} vs {b:?}");
            assert!((x.1 - y.1).abs() < 1e-12, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn w5_decays_over_the_last_five() {
        let mut s = WorkState::new(10, 30, true);
        // 8 commits, commit i touches file i (and file 9 on the even ones)
        for i in 0..8u32 {
            let files: Vec<u32> = if i % 2 == 0 { vec![i, 9] } else { vec![i] };
            s.train(&files, i64::from(i) * H, None);
        }
        // newest (7) weighs 1, then 6: .75, 5: .5625, 4: .421875, 3: .31640625; 0..=2 out
        let w = s.w5();
        close(
            &w,
            &[
                (3, 0.316_406_25),
                (4, 0.421_875),
                (5, 0.5625),
                (6, 0.75),
                (7, 1.0),
                (9, 0.75 + 0.421_875),
            ],
        );
    }

    #[test]
    fn segments_split_on_gap_and_no_overlap() {
        let mut s = WorkState::new(10, 30, true);
        s.train(&[0, 1], 0, None);
        s.train(&[1, 2], 23 * H, None); // continues: 23 h, shares 1
        assert_eq!(s.segments(), 1);
        s.train(&[2, 3], 23 * H + 25 * H, None); // 25 h gap: new
        assert_eq!(s.segments(), 2);
        s.train(&[5], 49 * H, None); // within gap, no shared file: new
        assert_eq!(s.segments(), 3);
        s.train(&[5, 6], 50 * H, None); // continues
        assert_eq!(s.segments(), 3);
        let (m, src) = s.m(51 * H, None);
        assert_eq!(src, "Mgap");
        close(&m, &[(5, 2.0), (6, 1.0)]);
        // the overlap reads only the segment's last OVERLAP_LAST commits
        let mut t = WorkState::new(10, 30, true);
        t.train(&[0], 0, None);
        t.train(&[0, 1], H, None);
        t.train(&[1, 2], 2 * H, None);
        t.train(&[2, 3], 3 * H, None);
        t.train(&[3, 4], 4 * H, None);
        assert_eq!(t.segments(), 1);
        t.train(&[0], 5 * H, None); // 0 is only in commits 1 and 2 of 5: new segment
        assert_eq!(t.segments(), 2);
    }

    #[test]
    fn m_prefers_branch_then_gap_then_none() {
        let mut s = WorkState::new(10, 30, true);
        s.train(&[0, 1], 0, Some("feat-a"));
        s.train(&[1, 2], H, Some("feat-a"));
        s.train(&[7], 2 * H, None);
        let (m, src) = s.m(3 * H, Some("feat-a"));
        assert_eq!(src, "Mbr");
        close(&m, &[(0, 1.0), (1, 2.0), (2, 1.0)]);
        // an unseen label falls to Mgap
        let (m, src) = s.m(3 * H, Some("feat-b"));
        assert_eq!(src, "Mgap");
        close(&m, &[(7, 1.0)]);
        // 25 h after the newest training commit: none
        let (m, src) = s.m(2 * H + 25 * H, None);
        assert_eq!(src, "none");
        assert!(m.is_empty());
        // a label with history wins even after the gap
        assert_eq!(s.m(2 * H + 25 * H, Some("feat-a")).1, "Mbr");
        // an empty state has nothing
        assert_eq!(WorkState::new(3, 30, true).m(0, None).1, "none");
    }

    #[test]
    fn wide_commit_trains_nothing() {
        let mut s = WorkState::new(40, 30, true);
        let wide: Vec<u32> = (0..31).collect();
        s.train(&wide, 0, Some("x"));
        assert_eq!(s.trained(), 0);
        assert_eq!(s.segments(), 0);
        assert_eq!(s.labels(), 0);
        s.train(&[], 0, None);
        assert_eq!(s.trained(), 0);
        assert!(s.w5().is_empty());
    }

    fn rc(files: Vec<u32>, t: i64, at: i64, br: Option<&str>) -> ReplayCommit {
        ReplayCommit {
            t,
            at,
            br: br.map(str::to_string),
            files,
            ..ReplayCommit::default()
        }
    }

    #[test]
    fn hook_never_leaks_the_test_commit() {
        let commits = vec![
            rc(vec![0, 1], 10, 0, None),
            rc(vec![1, 2], 20, H, Some("b")),
            rc(vec![3, 4], 30, 2 * H, Some("b")),
        ];
        let replay = Replay {
            train: vec![true; 3],
            tests: vec![2],
            queries: Vec::new(),
            messages: true,
            test_eligible: 3,
            commits,
        };
        let mut s = WorkState::for_replay(&replay, 5, 30);
        s.observe(&replay.commits[0], true);
        s.observe(&replay.commits[1], true);
        s.begin_test_commit(&replay, 2);
        assert_eq!(s.current_source(), "Mbr");
        assert_eq!(s.cur_m[3], 0.0);
        assert_eq!(s.cur_m[4], 0.0);
        assert_eq!(s.cur_w5[3], 0.0);
        assert_eq!(s.cur_w5[4], 0.0);
        assert_eq!(*s.cur_w5, vec![0.75, 1.75, 1.0, 0.0, 0.0]);
        assert_eq!(*s.cur_m, vec![0.0, 1.0, 1.0, 0.0, 0.0]);
        s.observe(&replay.commits[2], true);
        assert_eq!(s.trained(), 3);
        assert_eq!(s.source_counts(), (1, 0, 0));
        // committer time without the sidecar
        let cs = WorkState::new(5, 30, false);
        assert_eq!(cs.time_of(&replay.commits[1]), 20);
    }

    fn snap_commit(sha: &str, t: i64, paths: &[&str]) -> HistoryCommit {
        HistoryCommit {
            c: sha.to_string(),
            t,
            files: paths
                .iter()
                .map(|p| HistoryFile {
                    p: (*p).to_string(),
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        }
    }

    #[test]
    fn work_at_head_replays_oldest_first_and_excludes() {
        let cands = Candidates {
            paths: vec!["a".into(), "b".into(), "c".into()],
            deg: vec![1.0; 3],
            files: vec![0, 1, 2],
        };
        let mut snap = HistorySnapshot {
            meta: HistoryMeta::new("h".into(), 100, None, String::new()),
            commits: Vec::new(),
            blame: Vec::new(),
        };
        // newest first, as the snapshot stores them
        snap.commits = vec![
            snap_commit("pr", 3 * H, &["c"]),
            snap_commit("s2", 2 * H, &["b", "zzz"]),
            snap_commit("s1", H, &["a", "b"]),
        ];
        let ex: BTreeSet<String> = ["pr".to_string()].into_iter().collect();
        let (w5, m) = work_at_head(&snap, &[], &cands, &ex, 3 * H, None, "review");
        assert_eq!(w5, vec![0.75, 1.75, 0.0]);
        assert_eq!(m, vec![1.0, 2.0, 0.0]);
        let (w5, _) = work_at_head(&snap, &[], &cands, &BTreeSet::new(), 3 * H, None, "find");
        assert_eq!(w5, vec![0.5625, 1.3125, 1.0]);
    }
}
