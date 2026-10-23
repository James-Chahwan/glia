//! What a calibration candidate is (EC.3, item 4): a ranking of one repo's
//! candidate files for one replayed query, scored from the replay's PREFIX
//! state only. The driver ([`super::select::run_sets`]) ranks every query of a
//! test commit from the state of the strictly older commits, and only then
//! lets [`ReplayState::observe`] fold that commit in: a candidate can never
//! read its own commit's files.
//!
//! - [`RankKey`] is rank_eval.rs's `Key`: compared lexicographically,
//!   descending, an exact tie to the smaller file position. A scalar score s
//!   is `[s, 0, 0]`; a lexicographic fusion LX[a, b, c] is `[a, b, c]`.
//! - [`ReplayState`] holds the online co-change state ([`CoState`]) and the
//!   parts later packets register through [`StateHook`] (EC.4's structure
//!   bank, EC.10's work state, EC.11's text corpus); a candidate reads a part
//!   with [`ReplayState::hook`].
//! - [`Memo`] is one query's cache of shared signals (one PPR per damping,
//!   one BM25 per commit), so several candidates of several sets pay once.

use std::any::Any;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use glia_graph::MergedGraph;

use super::replay::{CoState, Replay, ReplayCommit, ReplayQuery};
use crate::intent::text::Candidates;

/// rank_eval's `Key`: one candidate file's sort key, descending
/// lexicographic; an exact tie goes to the smaller file position.
pub type RankKey = [f64; 3];

/// Everything one candidate may read for one query.
#[non_exhaustive]
pub struct QueryCtx<'a> {
    /// The built graph of the repo being calibrated.
    pub merged: &'a MergedGraph,
    /// The repo's candidate files; keys are indexed by their positions.
    pub cands: &'a Candidates,
    /// The query's test commit.
    pub commit: &'a ReplayCommit,
    /// The query (a find query's seeds, weights and C0 already filled).
    pub query: &'a ReplayQuery,
    /// Per candidate position: left out of the ranking (the seeds; nothing
    /// for a find query).
    pub excluded: &'a [bool],
    /// The prefix state: every commit strictly older than `commit`.
    pub state: &'a ReplayState,
    /// This query's signal cache, shared by every candidate of every set.
    pub memo: &'a Memo,
    /// The repo root.
    pub repo_root: &'a Path,
}

/// One ranking the calibration can pick.
pub trait Candidate: Send + Sync {
    /// The ranking's name (the record's candidate name).
    fn name(&self) -> &'static str;
    /// One key per candidate file (exactly `q.cands.len()`); a vector of
    /// another length makes the driver drop the set's rows.
    fn rank(&self, q: &QueryCtx<'_>) -> Vec<RankKey>;
}

/// A part of the replay's prefix state a later packet registers (EC.4's
/// structure bank, EC.10's work state, EC.11's text corpus).
pub trait StateHook: Send + Sync + Any {
    /// The part as `Any`, for [`ReplayState::hook`].
    fn as_any(&self) -> &dyn Any;
    /// Called once per test commit, before any of its queries is scored;
    /// `k` is the commit's index in `replay.commits`. The state still holds
    /// only the commits older than `k`.
    fn begin_test_commit(&mut self, _replay: &Replay, _k: usize) {}
    /// Fold one commit in, after its queries (if any) were scored. Called for
    /// every commit, oldest first; `train` is `Replay::train` of it.
    fn observe(&mut self, _c: &ReplayCommit, _train: bool) {}
    /// The text corpus's scores of `c`'s subject over the candidate files
    /// (rank_eval's T1s), read by the find shape's seeds. None when this part
    /// holds no text corpus.
    fn subject_scores(&self, _c: &ReplayCommit) -> Option<Vec<f64>> {
        None
    }
}

/// The replay's prefix state.
#[non_exhaustive]
pub struct ReplayState {
    /// Online co-change over the observed training commits.
    pub co: CoState,
    /// `ReplayParams::max_files`: the widest commit that trains.
    pub max_files: usize,
    observed: usize,
    hooks: Vec<Box<dyn StateHook>>,
}

impl std::fmt::Debug for ReplayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayState")
            .field("files", &self.co.cnt.len())
            .field("max_files", &self.max_files)
            .field("observed", &self.observed)
            .field("hooks", &self.hooks.len())
            .finish()
    }
}

impl ReplayState {
    /// An empty state over `n_files` candidate files.
    pub fn new(n_files: usize, max_files: usize) -> ReplayState {
        ReplayState {
            co: CoState::new(n_files),
            max_files,
            observed: 0,
            hooks: Vec::new(),
        }
    }

    /// Register a part; hooks are called in registration order.
    pub fn register(&mut self, hook: Box<dyn StateHook>) {
        self.hooks.push(hook);
    }

    /// The first registered part of type `T`.
    pub fn hook<T: StateHook>(&self) -> Option<&T> {
        self.hooks
            .iter()
            .find_map(|h| h.as_any().downcast_ref::<T>())
    }

    /// Commits observed so far (training or not).
    pub fn observed(&self) -> usize {
        self.observed
    }

    /// Tell every part a test commit's queries are about to be scored.
    pub fn begin_test_commit(&mut self, replay: &Replay, k: usize) {
        for h in &mut self.hooks {
            h.begin_test_commit(replay, k);
        }
    }

    /// Fold one commit in (oldest first): co-change when it trains, then
    /// every part.
    pub fn observe(&mut self, c: &ReplayCommit, train: bool) {
        if train {
            self.co.observe(&c.files, self.max_files);
        }
        for h in &mut self.hooks {
            h.observe(c, train);
        }
        self.observed += 1;
    }

    /// The first part's text scores of `c`'s subject; None with no corpus.
    pub fn subject_scores(&self, c: &ReplayCommit) -> Option<Vec<f64>> {
        self.hooks.iter().find_map(|h| h.subject_scores(c))
    }
}

/// One query's cache of shared signal vectors, by key.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct Memo {
    slots: Mutex<BTreeMap<&'static str, Arc<Vec<f64>>>>,
}

impl Memo {
    /// An empty memo.
    pub fn new() -> Memo {
        Memo::default()
    }

    /// The vector under `key`, computing it with `f` the first time. The lock
    /// is not held while `f` runs, so `f` may read other keys.
    pub fn get_or(&self, key: &'static str, f: impl FnOnce() -> Vec<f64>) -> Arc<Vec<f64>> {
        if let Some(v) = self
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Arc::clone(v);
        }
        let v = Arc::new(f());
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(slots.entry(key).or_insert(v))
    }

    /// Keys computed so far, ascending.
    pub fn keys(&self) -> Vec<&'static str> {
        self.slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_computes_once_and_nests() {
        let m = Memo::new();
        let mut calls = 0;
        let a = m.get_or("a", || {
            calls += 1;
            vec![1.0]
        });
        let b = m.get_or("a", || {
            calls += 1;
            vec![2.0]
        });
        assert_eq!(calls, 1);
        assert_eq!(*a, *b);
        let c = m.get_or("c", || m.get_or("b", || vec![3.0]).to_vec());
        assert_eq!(*c, vec![3.0]);
        assert_eq!(m.keys(), ["a", "b", "c"]);
    }

    struct Count(usize);
    impl StateHook for Count {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn observe(&mut self, _c: &ReplayCommit, train: bool) {
            if train {
                self.0 += 1;
            }
        }
    }

    #[test]
    fn state_observes_co_and_hooks() {
        let mut s = ReplayState::new(3, 30);
        s.register(Box::new(Count(0)));
        let c = ReplayCommit {
            files: vec![0, 2],
            ..ReplayCommit::default()
        };
        s.observe(&c, true);
        s.observe(&c, false);
        assert_eq!(s.observed(), 2);
        assert_eq!(s.co.cnt, [1, 0, 1]);
        assert_eq!(s.hook::<Count>().map(|h| h.0), Some(1));
        assert!(s.subject_scores(&c).is_none());
    }
}
