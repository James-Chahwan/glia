//! The replay's evaluation and the online pick (EC.3, item 4): every
//! candidate of every set ranks every replayed query from the prefix state,
//! is scored by rank_eval.rs's `eval_ex` (recall@10 and MRR, the seeds left
//! out), and the online rule of `r3_select` picks, per (set, shape), at each
//! test commit the candidate with the best recall@10 summed over the earlier
//! test commits' queries of that shape.
//!
//! The rule, verbatim from the harness:
//! - only a test commit WITH a query of the shape takes a pick (`if
//!   qs.is_empty() { continue }`);
//! - candidate 0 before any query was seen (`seen == 0`);
//! - else the argmax of the summed recall@10, ties to the earlier candidate
//!   (`max_by(.. .then(b.cmp(&a)))`);
//! - then that commit's queries add their values.
//!
//! The row's `pick` is the rule's next step (the argmax over every replayed
//! query) and `last_online_pick` the pick made at the newest test commit
//! (rank_eval's `pick_at_the_last_test_commit`). Ruling EC-3 (James
//! 2026-10-23): a pick whose online selection scores below the fallback on the
//! repo's own replay (mean recall@10, strictly below) is refused, and the
//! row's `pick` is the fallback.
//!
//! Every f64 sum folds in query order on one thread; only the ranking runs on
//! the pool (`parallel::par_map_ordered`), so a record is byte-identical at
//! any `GLIA_THREADS`.
//!
//! Fired_on, stderr, one line per row: `[calibrate] set=<SET> shape=<k>
//! candidates=<n> test_commits=<t> queries=<q> pick=<name>
//! last_online_pick=<name> selection_r10=<x.xxx> fallback_r10=<y.yyy>
//! refused=<no|below_fallback>`; a candidate whose ranking has the wrong
//! length: `[calibrate] candidate <name> returned <n> keys of <m>` (its set's
//! rows are not written).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use glia_graph::MergedGraph;

use super::candidate::{Candidate, Memo, QueryCtx, RankKey, ReplayState};
use super::record::{CalibratedSet, CommitScore, OnlineReplay, Shape, set_digest};
use super::replay::{Replay, ReplayQuery, find_top, find_top_scores};
use crate::intent::text::Candidates;

/// rank_eval's `better`: `a` (at position `ia`) ranks before `b` (at `ib`):
/// lexicographically greater, an exact tie to the smaller position.
pub fn better(a: &RankKey, ia: usize, b: &RankKey, ib: usize) -> bool {
    for i in 0..3 {
        if a[i] > b[i] {
            return true;
        }
        if a[i] < b[i] {
            return false;
        }
    }
    ia < ib
}

/// rank_eval's `eval_ex`, recall@10 and MRR: a target's rank is 1 + the
/// non-excluded files other than it that are [`better`]; recall@10 is the
/// share of targets ranked <= 10, MRR 1 / the best rank. No target gives
/// (0, 0).
pub fn eval_query(keys: &[RankKey], excluded: &[bool], targets: &[u32]) -> (f64, f64) {
    let mut ranks: Vec<usize> = targets
        .iter()
        .map(|&t| {
            let t = t as usize;
            let Some(kt) = keys.get(t) else {
                return usize::MAX;
            };
            let mut r = 1;
            for (f, kf) in keys.iter().enumerate() {
                if !excluded.get(f).copied().unwrap_or(false) && f != t && better(kf, f, kt, t) {
                    r += 1;
                }
            }
            r
        })
        .collect();
    ranks.sort_unstable();
    let Some(&best) = ranks.first() else {
        return (0.0, 0.0);
    };
    let nt = targets.len() as f64;
    let r10 = ranks.iter().filter(|&&r| r <= 10).count() as f64 / nt;
    (r10, 1.0 / best as f64)
}

/// One candidate set: the rankings an answer surface may pick among, at the
/// query shapes it is calibrated on.
#[non_exhaustive]
pub struct SetSpec {
    /// The set's name (`SELh0`, `SELs0`, ..).
    pub set: &'static str,
    /// The shapes a row is written for.
    pub shapes: &'static [Shape],
    /// The surface's uncalibrated ranking.
    pub fallback: &'static str,
    /// The candidates, in selection order (ties go to the earlier).
    pub candidates: Vec<Box<dyn Candidate>>,
    /// The fallback's ranking when it is not one of `candidates`: scored,
    /// never selectable.
    pub fallback_candidate: Option<Box<dyn Candidate>>,
}

impl SetSpec {
    /// A set over `candidates` whose fallback is one of them.
    pub fn new(
        set: &'static str,
        shapes: &'static [Shape],
        fallback: &'static str,
        candidates: Vec<Box<dyn Candidate>>,
    ) -> SetSpec {
        SetSpec {
            set,
            shapes,
            fallback,
            candidates,
            fallback_candidate: None,
        }
    }

    /// The same set with a scored-only fallback ranking.
    pub fn with_fallback_candidate(mut self, fallback: Box<dyn Candidate>) -> SetSpec {
        self.fallback_candidate = Some(fallback);
        self
    }

    fn names(&self) -> Vec<&'static str> {
        self.candidates.iter().map(|c| c.name()).collect()
    }

    /// Where the fallback's values come from: a candidate's index, the
    /// scored-only ranking, or nowhere (the set is skipped).
    fn fallback_source(&self) -> Option<FallbackSource> {
        if let Some(i) = self
            .candidates
            .iter()
            .position(|c| c.name() == self.fallback)
        {
            return Some(FallbackSource::Candidate(i));
        }
        self.fallback_candidate
            .as_ref()
            .filter(|c| c.name() == self.fallback)
            .map(|_| FallbackSource::Extra)
    }
}

#[derive(Clone, Copy)]
enum FallbackSource {
    Candidate(usize),
    Extra,
}

/// One query's (recall@10, MRR) per candidate of one set, plus the fallback's.
struct SetVals {
    cand: Vec<(f64, f64)>,
    fb: (f64, f64),
}

/// One (set, shape) row's running state.
#[derive(Default)]
struct Acc {
    sum_r10: Vec<f64>,
    sum_mrr: Vec<f64>,
    fb_r10: f64,
    fb_mrr: f64,
    sel_r10: f64,
    sel_mrr: f64,
    queries: u32,
    test_commits: usize,
    per_commit: Vec<CommitScore>,
    picks: BTreeMap<String, usize>,
    last: Option<usize>,
}

/// The argmax of `sum`, ties to the smaller index (rank_eval's `max_by(..
/// .then(b.cmp(&a)))`); 0 for an empty slice.
fn argmax(sum: &[f64]) -> usize {
    (0..sum.len())
        .max_by(|&a, &b| sum[a].total_cmp(&sum[b]).then(b.cmp(&a)))
        .unwrap_or(0)
}

/// Rank and score one query for every spec covering its shape. `Err` names
/// the first candidate whose ranking had the wrong length, per spec.
fn score_query(
    ctx: &QueryCtx<'_>,
    specs: &[SetSpec],
    live: &[Option<FallbackSource>],
) -> Vec<Option<Result<SetVals, (&'static str, usize)>>> {
    let m = ctx.cands.len();
    let eval = |c: &dyn Candidate| -> Result<(f64, f64), (&'static str, usize)> {
        let keys = c.rank(ctx);
        if keys.len() != m {
            return Err((c.name(), keys.len()));
        }
        Ok(eval_query(&keys, ctx.excluded, &ctx.query.targets))
    };
    specs
        .iter()
        .zip(live)
        .map(|(spec, src)| {
            let src = (*src)?;
            if !spec.shapes.contains(&ctx.query.shape) {
                return None;
            }
            let run = || -> Result<SetVals, (&'static str, usize)> {
                let mut cand = Vec::with_capacity(spec.candidates.len());
                for c in &spec.candidates {
                    cand.push(eval(c.as_ref())?);
                }
                let fb = match src {
                    FallbackSource::Candidate(i) => cand[i],
                    FallbackSource::Extra => match &spec.fallback_candidate {
                        Some(c) => eval(c.as_ref())?,
                        None => (0.0, 0.0),
                    },
                };
                Ok(SetVals { cand, fb })
            };
            Some(run())
        })
        .collect()
}

/// Replay `replay` over `specs`: one [`CalibratedSet`] per (spec, shape), in
/// spec order then the spec's shape order. `state` must hold no commit yet
/// (it is folded forward through every commit, oldest first). A spec with a
/// candidate whose ranking has the wrong length, or whose fallback is
/// neither a candidate nor its `fallback_candidate`, writes no row.
pub fn run_sets(
    merged: &MergedGraph,
    cands: &Candidates,
    root: &Path,
    replay: &Replay,
    state: &mut ReplayState,
    specs: &[SetSpec],
) -> Vec<CalibratedSet> {
    let live: Vec<Option<FallbackSource>> = specs
        .iter()
        .map(|s| {
            let src = s.fallback_source();
            if src.is_none() {
                eprintln!(
                    "[calibrate] set={} fallback {} is not scored: no candidate or fallback ranking names it",
                    s.set, s.fallback
                );
            }
            src
        })
        .collect();
    let shapes: Vec<Vec<Shape>> = specs
        .iter()
        .map(|s| {
            let mut seen = BTreeSet::new();
            s.shapes
                .iter()
                .copied()
                .filter(|x| seen.insert(*x))
                .collect()
        })
        .collect();
    let mut accs: Vec<Vec<Acc>> = specs
        .iter()
        .zip(&shapes)
        .map(|(s, sh)| {
            sh.iter()
                .map(|_| Acc {
                    sum_r10: vec![0.0; s.candidates.len()],
                    sum_mrr: vec![0.0; s.candidates.len()],
                    ..Acc::default()
                })
                .collect()
        })
        .collect();
    let mut failed: Vec<Option<(&'static str, usize)>> = vec![None; specs.len()];

    let mut by_commit: BTreeMap<usize, Vec<&ReplayQuery>> = BTreeMap::new();
    for q in &replay.queries {
        by_commit.entry(q.commit).or_default().push(q);
    }
    let tests: BTreeSet<usize> = replay.tests.iter().copied().collect();
    let m = cands.len();

    for (k, commit) in replay.commits.iter().enumerate() {
        if tests.contains(&k) {
            state.begin_test_commit(replay, k);
            let qs: Vec<Cow<'_, ReplayQuery>> = by_commit
                .get(&k)
                .map(|v| {
                    v.iter()
                        .map(|&q| {
                            if q.shape != Shape::Find {
                                return Cow::Borrowed(q);
                            }
                            let t1s = state
                                .subject_scores(commit)
                                .filter(|v| v.len() == m)
                                .unwrap_or_else(|| vec![0.0; m]);
                            let top = find_top(&t1s);
                            let mut fq = q.clone();
                            fq.seeds = top.iter().map(|x| x.0).collect();
                            fq.weights = top.iter().map(|x| x.1).collect();
                            fq.c0 = state.co.c0_weighted(&find_top_scores(&t1s));
                            Cow::Owned(fq)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let st: &ReplayState = state;
            let (vals, _threads) = crate::parallel::par_map_ordered(&qs, |q| {
                let q: &ReplayQuery = q;
                let mut excluded = vec![false; m];
                if q.shape != Shape::Find {
                    for &s in &q.seeds {
                        if let Some(e) = excluded.get_mut(s as usize) {
                            *e = true;
                        }
                    }
                }
                let memo = Memo::new();
                let ctx = QueryCtx {
                    merged,
                    cands,
                    commit,
                    query: q,
                    excluded: &excluded,
                    state: st,
                    memo: &memo,
                    repo_root: root,
                };
                score_query(&ctx, specs, &live)
            });
            for (si, spec_accs) in accs.iter_mut().enumerate() {
                for (hi, acc) in spec_accs.iter_mut().enumerate() {
                    let shape = shapes[si][hi];
                    let mut rows: Vec<&SetVals> = Vec::new();
                    for (q, v) in qs.iter().zip(&vals) {
                        if q.shape != shape {
                            continue;
                        }
                        match &v[si] {
                            Some(Ok(sv)) => rows.push(sv),
                            Some(Err(e)) if failed[si].is_none() => failed[si] = Some(*e),
                            Some(Err(_)) | None => {}
                        }
                    }
                    if rows.is_empty() {
                        continue;
                    }
                    let best = if acc.queries == 0 {
                        0
                    } else {
                        argmax(&acc.sum_r10)
                    };
                    let name = specs[si].candidates[best].name();
                    acc.last = Some(best);
                    *acc.picks.entry(name.to_string()).or_default() += 1;
                    acc.test_commits += 1;
                    let mut cs = CommitScore {
                        sha: commit.sha.clone(),
                        pick: name.to_string(),
                        ..CommitScore::default()
                    };
                    for sv in &rows {
                        let (sr, sm) = sv.cand[best];
                        cs.queries += 1;
                        cs.sel_r10 += sr;
                        cs.sel_mrr += sm;
                        cs.fb_r10 += sv.fb.0;
                        cs.fb_mrr += sv.fb.1;
                        acc.sel_r10 += sr;
                        acc.sel_mrr += sm;
                    }
                    acc.per_commit.push(cs);
                    for sv in &rows {
                        for (j, &(r, mm)) in sv.cand.iter().enumerate() {
                            acc.sum_r10[j] += r;
                            acc.sum_mrr[j] += mm;
                        }
                        acc.fb_r10 += sv.fb.0;
                        acc.fb_mrr += sv.fb.1;
                        acc.queries += 1;
                    }
                }
            }
        }
        state.observe(commit, replay.train.get(k).copied().unwrap_or(false));
    }

    let mut out = Vec::new();
    for (si, spec) in specs.iter().enumerate() {
        if live[si].is_none() {
            continue;
        }
        if let Some((name, n)) = failed[si] {
            eprintln!("[calibrate] candidate {name} returned {n} keys of {m}");
            continue;
        }
        let names = spec.names();
        for (hi, acc) in accs[si].iter().enumerate() {
            out.push(row(spec, &names, shapes[si][hi], acc));
        }
    }
    out
}

/// One finished (set, shape) row, with its marker.
fn row(spec: &SetSpec, names: &[&'static str], shape: Shape, acc: &Acc) -> CalibratedSet {
    let first = names.first().copied().unwrap_or("");
    let argmax_name = if acc.queries == 0 {
        first
    } else {
        names[argmax(&acc.sum_r10)]
    };
    let last = acc.last.map_or(first, |i| names[i]);
    let q = f64::from(acc.queries);
    let mean = |s: f64| if acc.queries == 0 { 0.0 } else { s / q };
    let online = OnlineReplay {
        selection_recall10: mean(acc.sel_r10),
        fallback_recall10: mean(acc.fb_r10),
        selection_mrr: mean(acc.sel_mrr),
        fallback_mrr: mean(acc.fb_mrr),
        picks: acc.picks.clone(),
        per_commit: acc.per_commit.clone(),
    };
    let refused = acc.queries > 0 && online.selection_recall10 < online.fallback_recall10;
    let pick = if refused { spec.fallback } else { argmax_name };
    eprintln!(
        "[calibrate] set={} shape={} candidates={} test_commits={} queries={} pick={} last_online_pick={} selection_r10={:.3} fallback_r10={:.3} refused={}",
        spec.set,
        shape.as_str(),
        names.len(),
        acc.test_commits,
        acc.queries,
        pick,
        last,
        online.selection_recall10,
        online.fallback_recall10,
        if refused { "below_fallback" } else { "no" },
    );
    CalibratedSet {
        set: spec.set.to_string(),
        shape: shape.as_str().to_string(),
        candidates: names.iter().map(|s| s.to_string()).collect(),
        digest: set_digest(names),
        fallback: spec.fallback.to_string(),
        test_commits: acc.test_commits,
        queries: acc.queries,
        recall10: acc.sum_r10.clone(),
        mrr: acc.sum_mrr.clone(),
        fallback_recall10: acc.fb_r10,
        fallback_mrr: acc.fb_mrr,
        pick: pick.to_string(),
        last_online_pick: last.to_string(),
        online,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibrate::candidate::StateHook;
    use crate::calibrate::replay::ReplayCommit;
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    const N: usize = 16;

    fn cands() -> Candidates {
        Candidates {
            paths: (0..N).map(|i| format!("src/f{i:02}.rs")).collect(),
            deg: vec![1.0; N],
            files: (0..N as u32).collect(),
        }
    }

    #[test]
    fn better_is_lexicographic_then_smaller_index() {
        assert!(better(&[1.0, 0.0, 0.0], 5, &[0.0, 9.0, 9.0], 1));
        assert!(better(&[1.0, 2.0, 0.0], 5, &[1.0, 1.0, 9.0], 1));
        assert!(better(&[1.0, 1.0, 1.0], 1, &[1.0, 1.0, 1.0], 2));
        assert!(!better(&[1.0, 1.0, 1.0], 2, &[1.0, 1.0, 1.0], 1));
    }

    #[test]
    fn eval_query_hand_case() {
        // Files 0..5; scores 3, 2, 2, 5, 1.
        let keys: Vec<RankKey> = [3.0, 2.0, 2.0, 5.0, 1.0]
            .iter()
            .map(|&s| [s, 0.0, 0.0])
            .collect();
        let none = [false; 5];
        // Order: 3, 0, 1, 2 (tie 1 before 2), 4. Target 2 ranks 4, target 4 ranks 5.
        assert_eq!(eval_query(&keys, &none, &[2, 4]), (1.0, 1.0 / 4.0));
        // Target 1 ties target 2 but is the smaller index: rank 3.
        assert_eq!(eval_query(&keys, &none, &[2, 1]), (1.0, 1.0 / 3.0));
        // Seed 3 excluded: target 2 ranks 3.
        let ex = [false, false, false, true, false];
        assert_eq!(eval_query(&keys, &ex, &[2]), (1.0, 1.0 / 3.0));
        assert_eq!(eval_query(&keys, &none, &[]), (0.0, 0.0));
        // 12 files above the target: recall@10 0, MRR 1/13.
        let mut many: Vec<RankKey> = vec![[1.0, 0.0, 0.0]; 13];
        many[12] = [0.0, 0.0, 0.0];
        assert_eq!(eval_query(&many, &[false; 13], &[12, 0]), (0.5, 1.0));
        assert_eq!(eval_query(&many, &[false; 13], &[12]), (0.0, 1.0 / 13.0));
    }

    /// (commit, commits observed) per call.
    type Seen = Arc<Mutex<Vec<(usize, usize)>>>;

    /// Ranks a query's targets first at the commits in `good`, last elsewhere.
    struct Script {
        name: &'static str,
        good: Vec<usize>,
        seen: Option<Seen>,
    }

    impl Candidate for Script {
        fn name(&self) -> &'static str {
            self.name
        }
        fn rank(&self, q: &QueryCtx<'_>) -> Vec<RankKey> {
            if let Some(seen) = &self.seen {
                seen.lock()
                    .unwrap()
                    .push((q.query.commit, q.state.observed()));
            }
            let g = if self.good.contains(&q.query.commit) {
                1.0
            } else {
                -1.0
            };
            let mut k = vec![[0.0; 3]; q.cands.len()];
            for &t in &q.query.targets {
                k[t as usize] = [g, 0.0, 0.0];
            }
            k
        }
    }

    fn script(name: &'static str, good: &[usize]) -> Box<dyn Candidate> {
        Box::new(Script {
            name,
            good: good.to_vec(),
            seen: None,
        })
    }

    fn query(commit: usize, tc: usize, shape: Shape, files: &[u32], nseeds: usize) -> ReplayQuery {
        ReplayQuery {
            tc,
            commit,
            shape,
            seeds: files[..nseeds].to_vec(),
            weights: vec![1.0; nseeds],
            targets: files[nseeds..].to_vec(),
            c0: Vec::new(),
        }
    }

    /// Commit 0 trains only; commits 1..=5 are tests; commit 3 has only a K2
    /// query (3 files), the rest one K1 query each.
    fn scripted_replay() -> Replay {
        let files: Vec<Vec<u32>> = vec![
            vec![0, 1],
            vec![2, 3],
            vec![4, 5],
            vec![6, 7, 8],
            vec![9, 10],
            vec![11, 12],
        ];
        let commits: Vec<ReplayCommit> = files
            .iter()
            .enumerate()
            .map(|(i, f)| ReplayCommit {
                sha: format!("{i:040x}"),
                files: f.clone(),
                ..ReplayCommit::default()
            })
            .collect();
        let mut queries = Vec::new();
        for (tc, k) in (1..=5).enumerate() {
            if k == 3 {
                queries.push(query(k, tc, Shape::K2, &files[k], 2));
            } else {
                queries.push(query(k, tc, Shape::K1, &files[k], 1));
            }
        }
        Replay {
            train: vec![true; commits.len()],
            commits,
            tests: vec![1, 2, 3, 4, 5],
            queries,
            messages: false,
            test_eligible: 5,
        }
    }

    const SHAPES: &[Shape] = &[Shape::K1, Shape::K2];

    /// r10 per K1 test commit (1, 2, 4, 5): A 0 0 1 0, B 1 1 0 0, C 1 0 1 1.
    fn abc() -> Vec<Box<dyn Candidate>> {
        vec![
            script("A", &[4]),
            script("B", &[1, 2]),
            script("C", &[1, 4, 5]),
        ]
    }

    fn run(specs: &[SetSpec]) -> Vec<CalibratedSet> {
        let replay = scripted_replay();
        let c = cands();
        let mut st = ReplayState::new(N, 30);
        run_sets(
            &MergedGraph::default(),
            &c,
            Path::new("."),
            &replay,
            &mut st,
            specs,
        )
    }

    #[test]
    fn online_rule_matches_hand_sequence() {
        let rows = run(&[SetSpec::new("S", SHAPES, "A", abc())]);
        assert_eq!(rows.len(), 2);
        let k1 = &rows[0];
        assert_eq!(k1.shape, "k1");
        // Commit 1: first -> A. Sums A0 B1 C1.
        // Commit 2: B/C tie at 1 -> B (earlier). Sums A0 B2 C1.
        // Commit 3: no K1 query: skipped.
        // Commit 4: B 2 vs C 1 -> B. Sums A1 B2 C2.
        // Commit 5: B/C tie at 2 -> B. Sums A1 B2 C3.
        let picks: Vec<&str> = k1
            .online
            .per_commit
            .iter()
            .map(|c| c.pick.as_str())
            .collect();
        assert_eq!(picks, ["A", "B", "B", "B"]);
        let shas: Vec<&str> = k1
            .online
            .per_commit
            .iter()
            .map(|c| c.sha.as_str())
            .collect();
        let want: Vec<String> = [1usize, 2, 4, 5]
            .iter()
            .map(|i| format!("{i:040x}"))
            .collect();
        assert_eq!(shas, want);
        assert_eq!(k1.test_commits, 4);
        assert_eq!(k1.queries, 4);
        assert_eq!(k1.recall10, [1.0, 2.0, 3.0]);
        assert_eq!(k1.last_online_pick, "B");
        assert_eq!(k1.pick, "C", "the newest commit flips the argmax");
        assert_eq!(
            k1.online.picks,
            BTreeMap::from([("A".to_string(), 1), ("B".to_string(), 3)])
        );
        // Selection: A0 + B1 + B0 + B0 = 1 of 4; fallback A: 0 0 1 0.
        assert_eq!(k1.online.selection_recall10, 0.25);
        assert_eq!(k1.online.fallback_recall10, 0.25);
        assert_eq!(k1.fallback_recall10, 1.0);
        assert_eq!(k1.candidates, ["A", "B", "C"]);
        assert_eq!(k1.digest, set_digest(&["A", "B", "C"]));
        // MRR: 14 files (neither the seed nor the target) outrank a bad
        // target -> 1/15.
        let bad = 1.0 / 15.0;
        assert_eq!(k1.online.per_commit[0].sel_mrr, bad);
        assert_eq!(k1.online.per_commit[1].sel_mrr, 1.0);
        let k2 = &rows[1];
        assert_eq!(
            (k2.shape.as_str(), k2.test_commits, k2.queries),
            ("k2", 1, 1)
        );
        assert_eq!((k2.pick.as_str(), k2.last_online_pick.as_str()), ("A", "A"));
    }

    #[test]
    fn fallback_candidate_is_scored_never_picked_and_refuses_below() {
        let high = SetSpec::new("H", SHAPES, "D", abc())
            .with_fallback_candidate(script("D", &[1, 2, 3, 4, 5]));
        let low = SetSpec::new("L", SHAPES, "D", abc()).with_fallback_candidate(script("D", &[]));
        let rows = run(&[high, low]);
        assert_eq!(rows.len(), 4);
        for r in &rows {
            assert!(!r.online.picks.contains_key("D"));
            assert_ne!(r.last_online_pick, "D");
            assert_eq!(r.candidates, ["A", "B", "C"]);
        }
        // H: the fallback scores 1.0 against the selection's 0.25: refused.
        assert_eq!(rows[0].online.fallback_recall10, 1.0);
        assert_eq!(rows[0].pick, "D");
        assert_eq!(rows[0].recall10, [1.0, 2.0, 3.0]);
        // L: the fallback scores 0: the argmax stands.
        assert_eq!(rows[2].online.fallback_recall10, 0.0);
        assert_eq!(rows[2].pick, "C");
    }

    #[test]
    fn leak_free_prefix_state() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let spy = Box::new(Script {
            name: "spy",
            good: Vec::new(),
            seen: Some(Arc::clone(&seen)),
        });
        let rows = run(&[SetSpec::new("S", SHAPES, "spy", vec![spy])]);
        assert_eq!(rows.len(), 2);
        let mut got = seen.lock().unwrap().clone();
        got.sort_unstable();
        assert_eq!(got, [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
    }

    struct Short;
    impl Candidate for Short {
        fn name(&self) -> &'static str {
            "short"
        }
        fn rank(&self, _q: &QueryCtx<'_>) -> Vec<RankKey> {
            vec![[0.0; 3]; 3]
        }
    }

    #[test]
    fn short_ranking_drops_its_set_only() {
        let bad = SetSpec::new("bad", SHAPES, "A", vec![script("A", &[]), Box::new(Short)]);
        let unscored = SetSpec::new("nofb", SHAPES, "Z", abc());
        let good = SetSpec::new("good", SHAPES, "A", abc());
        let rows = run(&[bad, unscored, good]);
        let sets: Vec<&str> = rows.iter().map(|r| r.set.as_str()).collect();
        assert_eq!(sets, ["good", "good"]);
    }

    #[test]
    fn no_queries_row_picks_first() {
        let rows = run(&[SetSpec::new("S", &[Shape::Half], "B", abc())]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].queries, 0);
        assert_eq!(rows[0].pick, "A");
        assert_eq!(rows[0].last_online_pick, "A");
        assert!(rows[0].online.per_commit.is_empty());
    }

    struct Text(Vec<f64>);
    impl StateHook for Text {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn subject_scores(&self, _c: &ReplayCommit) -> Option<Vec<f64>> {
            Some(self.0.clone())
        }
    }

    struct FindSpy(Arc<Mutex<Vec<ReplayQuery>>>);
    impl Candidate for FindSpy {
        fn name(&self) -> &'static str {
            "find"
        }
        fn rank(&self, q: &QueryCtx<'_>) -> Vec<RankKey> {
            assert!(q.excluded.iter().all(|&e| !e), "find excludes nothing");
            self.0.lock().unwrap().push(q.query.clone());
            vec![[0.0; 3]; q.cands.len()]
        }
    }

    #[test]
    fn find_queries_take_text_seeds_and_weighted_c0() {
        let c = cands();
        let mut replay = scripted_replay();
        replay.queries = vec![ReplayQuery {
            tc: 1,
            commit: 2,
            shape: Shape::Find,
            seeds: Vec::new(),
            weights: Vec::new(),
            targets: vec![4, 5],
            c0: Vec::new(),
        }];
        let mut t1s = vec![0.0; N];
        t1s[0] = 3.0;
        t1s[2] = 1.0;
        let mut st = ReplayState::new(N, 30);
        st.register(Box::new(Text(t1s.clone())));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let spec = SetSpec::new(
            "F",
            &[Shape::Find],
            "find",
            vec![Box::new(FindSpy(Arc::clone(&seen)))],
        );
        let rows = run_sets(
            &MergedGraph::default(),
            &c,
            Path::new("."),
            &replay,
            &mut st,
            &[spec],
        );
        assert_eq!(rows[0].queries, 1);
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].seeds, [0, 2]);
        assert_eq!(got[0].weights, [0.75, 0.25]);
        // Commits 0 ({0, 1}) and 1 ({2, 3}) observed before commit 2.
        let mut co = crate::calibrate::CoState::new(N);
        co.observe(&[0, 1], 30);
        co.observe(&[2, 3], 30);
        assert_eq!(got[0].c0, co.c0_weighted(&[(0, 3.0), (2, 1.0)]));
        assert_eq!(got[0].c0, vec![(1, 3.0 / 4.0 / 2.0), (3, 1.0 / 4.0 / 2.0)]);
        assert_eq!(got[0].targets, [4, 5]);
    }

    /// Scores from a hash of (commit, file): no candidate is constant, so the
    /// pick sequence and the sums exercise the fold.
    struct Noise(&'static str, u64);
    impl Candidate for Noise {
        fn name(&self) -> &'static str {
            self.0
        }
        fn rank(&self, q: &QueryCtx<'_>) -> Vec<RankKey> {
            (0..q.cands.len())
                .map(|f| {
                    let h = crate::calibrate::seed_order_hash(
                        &q.commit.sha,
                        &format!("{}:{f}:{}", self.1, q.state.observed()),
                    );
                    [(h % 7) as f64, (h % 1000) as f64 / 1000.0, 0.0]
                })
                .collect()
        }
    }

    fn noisy_specs() -> Vec<SetSpec> {
        vec![
            SetSpec::new(
                "N1",
                &[Shape::K1, Shape::K2, Shape::K3, Shape::Half],
                "a",
                vec![
                    Box::new(Noise("a", 1)),
                    Box::new(Noise("b", 2)),
                    Box::new(Noise("c", 3)),
                ],
            ),
            SetSpec::new("N2", &[Shape::K1], "z", vec![Box::new(Noise("d", 4))])
                .with_fallback_candidate(Box::new(Noise("z", 5))),
        ]
    }

    #[test]
    fn rows_are_byte_identical_at_1_and_4_threads() {
        use crate::calibrate::replay::{ReplayParams, build_replay};
        use glia_code_domain::snapshots::{
            HistoryCommit, HistoryFile, HistoryMeta, HistorySnapshot,
        };
        let c = cands();
        let mut commits: Vec<HistoryCommit> = (0..60)
            .map(|i| HistoryCommit {
                c: format!("{i:040x}"),
                t: 1_000 + i as i64,
                files: c
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| (j * 7 + i) % 5 < 2)
                    .take(2 + i % 6)
                    .map(|(_, p)| HistoryFile {
                        p: p.clone(),
                        a: Some(1),
                        d: Some(0),
                        from: None,
                    })
                    .collect(),
            })
            .collect();
        commits.reverse();
        let snap = HistorySnapshot {
            meta: HistoryMeta::new("head".into(), 2000, None, String::new()),
            commits,
            blame: Vec::new(),
        };
        let replay = build_replay(
            &snap,
            None,
            &c,
            &ReplayParams::default(),
            &[Shape::K1, Shape::K2, Shape::K3, Shape::Half],
        );
        assert!(replay.queries.len() > 50);
        let at = |n: usize| -> Vec<u8> {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap();
            let rows = pool.install(|| {
                let mut st = ReplayState::new(N, 30);
                run_sets(
                    &MergedGraph::default(),
                    &c,
                    Path::new("."),
                    &replay,
                    &mut st,
                    &noisy_specs(),
                )
            });
            assert_eq!(rows.len(), 5);
            serde_json::to_vec(&rows).unwrap()
        };
        let one = at(1);
        let four = at(4);
        assert_eq!(one, four);
    }
}
