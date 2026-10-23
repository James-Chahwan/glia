//! The leak-free replay's inputs (EC.2, item 4): the history snapshot oldest
//! first over one repo's candidate files, each commit's message from EB.1's
//! sidecar, rank_eval's eligibility and per-shape queries, and the online
//! co-change state whose C0 every query reads BEFORE its own commit is
//! observed. rank_eval.rs's main replay loop and `r3_tqueries` are the
//! specification; this module reproduces their arithmetic (hash seed order,
//! accumulation order, tie rules) exactly.
//!
//! - File space: EB.3's [`Candidates`] (one repo's slice of EA.3's
//!   `rank::FileIndex`). A commit's files map through `HistoryFile.p` with NO
//!   rename fold (`from` is never read: the harness's `git log
//!   --no-renames`); a path the candidates lack drops. Files are candidate
//!   positions, sorted and deduped ("file order").
//! - Eligibility: a commit trains iff it maps 1..=`max_files` files; it is
//!   test-eligible iff it maps 2..=`max_files`; the tests are the newest
//!   `max_commits` test-eligible commits.
//! - Shapes: K1 (each of the first `max_seeds` hash-ordered files alone,
//!   targets the other files in file order), K2 / K3 / Half (the first 2 / 3
//!   / n/2 hash-ordered files, needing 3 / 4 / 4 files, targets the rest in
//!   hash order) and Find (no seed here: its seeds need the text ranking,
//!   which EC.3's driver supplies through [`find_top`] / [`CoState::c0_weighted`];
//!   recorded as a placeholder with every file a target, in file order).
//!
//! Fired_on, stderr: `[calibrate] replay commits=<c> train=<t>
//! test_eligible=<e> test=<n> messages=<yes|no> queries k1=<a> k2=<b> k3=<c>
//! half=<d> find=<f>`.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot};

use super::record::Shape;
use crate::intent::text::Candidates;
use crate::rank::{CochangeHistory, c0};

/// rank_eval's `R3_FIND_TOP`: the text ranking's top files a find query seeds.
pub const FIND_TOP: usize = 10;

/// The replay's bounds: rank_eval's `--max-commits`, `--max-seeds` and
/// `MAX_FILES`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayParams {
    /// The newest test-eligible commits that are tests (300).
    pub max_commits: usize,
    /// K1 queries per test commit (5).
    pub max_seeds: usize,
    /// The most candidate files a commit may map and still train or test (30).
    pub max_files: usize,
}

impl Default for ReplayParams {
    fn default() -> Self {
        ReplayParams {
            max_commits: 300,
            max_seeds: 5,
            max_files: 30,
        }
    }
}

/// One commit of the replay, oldest first.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayCommit {
    /// Full sha.
    pub sha: String,
    /// Committer time, unix seconds.
    pub t: i64,
    /// Author time from the messages sidecar; 0 without one.
    pub at: i64,
    /// The branch label from the messages sidecar, when it named one.
    pub br: Option<String>,
    /// The subject (EB.1's `s`); empty without the sidecar.
    pub subject: String,
    /// The full message, subject `\n` body (EB.3's document text); empty
    /// without the sidecar.
    pub message: String,
    /// Candidate positions, ascending, deduped.
    pub files: Vec<u32>,
}

/// One query of a test commit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayQuery {
    /// The test ordinal (0 = the oldest test commit).
    pub tc: usize,
    /// The commit's index in [`Replay::commits`].
    pub commit: usize,
    /// The query shape.
    pub shape: Shape,
    /// Seed files (empty for a Find placeholder).
    pub seeds: Vec<u32>,
    /// One weight per seed (1.0 for a file seed).
    pub weights: Vec<f64>,
    /// The files the ranking should find.
    pub targets: Vec<u32>,
    /// `(file, C0)` for every file with a non-zero C0, ascending by file,
    /// read from strictly older training commits.
    pub c0: Vec<(u32, f64)>,
}

/// The replay of one repo's history.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Replay {
    /// Every snapshot commit, oldest first.
    pub commits: Vec<ReplayCommit>,
    /// Per commit: it trains (1..=max_files files).
    pub train: Vec<bool>,
    /// The test commits' indices, oldest first.
    pub tests: Vec<usize>,
    /// Every query, in (test ordinal, shape order, seed order).
    pub queries: Vec<ReplayQuery>,
    /// Whether the messages sidecar was read.
    pub messages: bool,
    /// Test-eligible commits (2..=max_files files).
    pub test_eligible: usize,
}

/// The online co-change state: rank_eval's `cnt` / `co`, counted over the
/// commits observed so far.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoState {
    /// Training commits per file.
    pub cnt: Vec<u32>,
    /// Per file, `(other file, commits touching both)` ascending by other file.
    pub co: Vec<Vec<(u32, u32)>>,
}

impl CochangeHistory for CoState {
    fn count(&self, f: u32) -> u32 {
        self.cnt.get(f as usize).copied().unwrap_or(0)
    }

    fn cochanged(&self, f: u32) -> &[(u32, u32)] {
        self.co.get(f as usize).map_or(&[], Vec::as_slice)
    }
}

impl CoState {
    /// An empty state over `n` files.
    pub fn new(n: usize) -> CoState {
        CoState {
            cnt: vec![0; n],
            co: vec![Vec::new(); n],
        }
    }

    /// Fold one commit's files in when it trains (1..=`max_files` files):
    /// each file's count gains one, each ordered pair a != b one.
    pub fn observe(&mut self, files: &[u32], max_files: usize) {
        if files.is_empty() || files.len() > max_files {
            return;
        }
        for &a in files {
            let Some(c) = self.cnt.get_mut(a as usize) else {
                continue;
            };
            *c += 1;
            let row = &mut self.co[a as usize];
            for &b in files {
                if a == b {
                    continue;
                }
                match row.binary_search_by_key(&b, |x| x.0) {
                    Ok(i) => row[i].1 += 1,
                    Err(i) => row.insert(i, (b, 1)),
                }
            }
        }
    }

    /// C0 of `seeds`: the sparse form of `rank::c0(self, seeds, n)` (per
    /// seed s in the given order, `co(s, f) / (cnt(s) + 1)` added to f), the
    /// non-zero entries ascending by file. One seed is rank_eval's
    /// `run_query` C0; several are `r3_tqueries`' sum.
    pub fn c0_seeds(&self, seeds: &[u32]) -> Vec<(u32, f64)> {
        c0(self, seeds, self.cnt.len())
            .into_iter()
            .enumerate()
            .filter(|&(_, v)| v != 0.0)
            .map(|(f, v)| (f as u32, v))
            .collect()
    }

    /// The find shape's C0x over `top`, the text ranking's top files with
    /// their RAW scores ([`find_top_scores`]): `tot` = the scores' sum, then
    /// per (g, w) in `top` order and f ascending, `w / tot * co(g, f) /
    /// (cnt(g) + 1)` added to f (rank_eval's `r3_tqueries`, operand order
    /// kept).
    pub fn c0_weighted(&self, top: &[(u32, f64)]) -> Vec<(u32, f64)> {
        let tot: f64 = top.iter().map(|x| x.1).sum();
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        for &(g, wg) in top {
            let d = f64::from(self.count(g)) + 1.0;
            for &(f, x) in self.cochanged(g) {
                *acc.entry(f).or_default() += wg / tot * f64::from(x) / d;
            }
        }
        acc.into_iter().collect()
    }
}

/// rank_eval's `mix`: the splitmix64 finaliser.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// rank_eval's `h64`: FNV-1a 64 then [`mix`].
fn h64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    mix(h)
}

/// The seed order key of `path` in commit `sha`: rank_eval's
/// `h64(&format!("{sha}:{path}"))`.
pub fn seed_order_hash(sha: &str, path: &str) -> u64 {
    h64(&format!("{sha}:{path}"))
}

/// The find shape's top files by raw score: entries > 0.0 of the dense
/// text ranking `t1s`, by (score descending, file ascending), at most
/// [`FIND_TOP`].
pub fn find_top_scores(t1s: &[f64]) -> Vec<(u32, f64)> {
    let mut top: Vec<(u32, f64)> = t1s
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v > 0.0)
        .map(|(f, &v)| (f as u32, v))
        .collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(FIND_TOP);
    top
}

/// The find shape's seeds and weights: [`find_top_scores`], each score
/// divided by their total (rank_eval's `TQuery.w`).
pub fn find_top(t1s: &[f64]) -> Vec<(u32, f64)> {
    let top = find_top_scores(t1s);
    let tot: f64 = top.iter().map(|x| x.1).sum();
    top.into_iter().map(|(f, v)| (f, v / tot)).collect()
}

/// Build the replay of `snapshot` (commits newest first, as the snapshot
/// stores them) over `cands`, with `messages` row i belonging to commit i
/// when given. Builds only `shapes`, each test commit's queries in shape
/// order; every query's C0 is read before its commit is observed.
pub fn build_replay(
    snapshot: &HistorySnapshot,
    messages: Option<&[HistoryMessage]>,
    cands: &Candidates,
    params: &ReplayParams,
    shapes: &[Shape],
) -> Replay {
    let msgs = messages.filter(|m| m.len() == snapshot.commits.len());
    let mut commits: Vec<ReplayCommit> = snapshot
        .commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut files: Vec<u32> = c
                .files
                .iter()
                .filter_map(|f| cands.position(&f.p))
                .map(|p| p as u32)
                .collect();
            files.sort_unstable();
            files.dedup();
            let mut rc = ReplayCommit {
                sha: c.c.clone(),
                t: c.t,
                files,
                ..ReplayCommit::default()
            };
            if let Some(m) = msgs.map(|m| &m[i]) {
                rc.at = m.at;
                rc.br = m.br.clone();
                rc.subject = m.s.clone();
                rc.message = format!("{}\n{}", m.s, m.b);
            }
            rc
        })
        .collect();
    commits.reverse();

    let maxf = params.max_files;
    let train: Vec<bool> = commits
        .iter()
        .map(|c| !c.files.is_empty() && c.files.len() <= maxf)
        .collect();
    let eligible: Vec<usize> = (0..commits.len())
        .filter(|&k| (2..=maxf).contains(&commits[k].files.len()))
        .collect();
    let tests: Vec<usize> = eligible[eligible.len().saturating_sub(params.max_commits)..].to_vec();
    let test_ord: BTreeMap<usize, usize> = tests.iter().enumerate().map(|(o, &k)| (k, o)).collect();
    let wanted: BTreeSet<Shape> = shapes.iter().copied().collect();
    let mut wanted: Vec<Shape> = wanted.into_iter().collect();
    wanted.sort_by_key(|s| s.order());

    let mut co = CoState::new(cands.len());
    let mut queries: Vec<ReplayQuery> = Vec::new();
    for (k, c) in commits.iter().enumerate() {
        if let Some(&tc) = test_ord.get(&k) {
            let mut ord = c.files.clone();
            ord.sort_by_key(|&f| seed_order_hash(&c.sha, &cands.paths[f as usize]));
            let n = ord.len();
            for &shape in &wanted {
                let q = |seeds: Vec<u32>, targets: Vec<u32>, c0: Vec<(u32, f64)>| ReplayQuery {
                    tc,
                    commit: k,
                    shape,
                    weights: vec![1.0; seeds.len()],
                    seeds,
                    targets,
                    c0,
                };
                match shape {
                    Shape::K1 => {
                        for &s in &ord[..params.max_seeds.min(n)] {
                            let targets: Vec<u32> =
                                c.files.iter().copied().filter(|&f| f != s).collect();
                            queries.push(q(vec![s], targets, co.c0_seeds(&[s])));
                        }
                    }
                    Shape::Find => {
                        queries.push(q(Vec::new(), c.files.clone(), Vec::new()));
                    }
                    _ => {
                        let (kk, need) = match shape {
                            Shape::K2 => (2, 3),
                            Shape::K3 => (3, 4),
                            _ => (n / 2, 4),
                        };
                        if n >= need {
                            let seeds = ord[..kk].to_vec();
                            let c0v = co.c0_seeds(&seeds);
                            queries.push(q(seeds, ord[kk..].to_vec(), c0v));
                        }
                    }
                }
            }
        }
        if train[k] {
            co.observe(&c.files, maxf);
        }
    }

    let count = |s: Shape| queries.iter().filter(|q| q.shape == s).count();
    eprintln!(
        "[calibrate] replay commits={} train={} test_eligible={} test={} messages={} queries k1={} k2={} k3={} half={} find={}",
        commits.len(),
        train.iter().filter(|&&t| t).count(),
        eligible.len(),
        tests.len(),
        if msgs.is_some() { "yes" } else { "no" },
        count(Shape::K1),
        count(Shape::K2),
        count(Shape::K3),
        count(Shape::Half),
        count(Shape::Find),
    );
    Replay {
        commits,
        train,
        tests,
        queries,
        messages: msgs.is_some(),
        test_eligible: eligible.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta};

    const ALL: [Shape; 5] = [Shape::K1, Shape::K2, Shape::K3, Shape::Half, Shape::Find];

    fn cands(paths: &[&str]) -> Candidates {
        let mut c = Candidates::default();
        let mut p: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        p.sort();
        c.deg = vec![1.0; p.len()];
        c.files = (0..p.len() as u32).collect();
        c.paths = p;
        c
    }

    fn file(p: &str) -> HistoryFile {
        HistoryFile {
            p: p.to_string(),
            a: Some(1),
            d: Some(0),
            from: None,
        }
    }

    /// `rows` oldest first; the snapshot stores them newest first.
    fn snap(rows: &[(&str, &[&str])]) -> HistorySnapshot {
        let mut commits: Vec<HistoryCommit> = rows
            .iter()
            .enumerate()
            .map(|(i, (sha, fs))| HistoryCommit {
                c: sha.to_string(),
                t: 1_000 + i as i64,
                files: fs.iter().map(|p| file(p)).collect(),
            })
            .collect();
        commits.reverse();
        HistorySnapshot {
            meta: HistoryMeta::new("head".into(), 2000, None, String::new()),
            commits,
            blame: Vec::new(),
        }
    }

    fn msgs(s: &HistorySnapshot) -> Vec<HistoryMessage> {
        s.commits
            .iter()
            .map(|c| HistoryMessage {
                c: c.c.clone(),
                at: c.t - 1,
                s: format!("subject {}", c.c),
                b: "body".into(),
                br: None,
            })
            .collect()
    }

    fn paths(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("src/f{i:02}.rs")).collect()
    }

    #[test]
    fn h64_literal_matches_rank_eval() {
        // rank_eval.rs h64("abc:src/a.rs"), computed once.
        assert_eq!(seed_order_hash("abc", "src/a.rs"), 0x2124_d5d3_d4bc_c69d);
    }

    #[test]
    fn eligibility_train_and_test() {
        let ps = paths(31);
        let pr: Vec<&str> = ps.iter().map(String::as_str).collect();
        let cs = cands(&pr);
        let s = snap(&[
            ("c1", &pr[..1]),
            ("c31", &pr[..31]),
            ("c2", &pr[..2]),
            ("c0", &["docs/x.md"]),
        ]);
        let r = build_replay(&s, None, &cs, &ReplayParams::default(), &ALL);
        let shas: Vec<&str> = r.commits.iter().map(|c| c.sha.as_str()).collect();
        assert_eq!(shas, ["c1", "c31", "c2", "c0"]);
        assert_eq!(r.train, [true, false, true, false]);
        assert_eq!(r.tests, [2]);
        assert_eq!(r.test_eligible, 1);
    }

    #[test]
    fn max_commits_keeps_newest() {
        let ps = paths(4);
        let pr: Vec<&str> = ps.iter().map(String::as_str).collect();
        let cs = cands(&pr);
        let s = snap(&[
            ("a", &pr[..2]),
            ("b", &pr[1..3]),
            ("c", &pr[2..4]),
            ("d", &pr[..1]),
        ]);
        let p = ReplayParams {
            max_commits: 2,
            ..ReplayParams::default()
        };
        let r = build_replay(&s, None, &cs, &p, &[Shape::K1]);
        assert_eq!(r.tests, [1, 2]);
        assert_eq!(r.test_eligible, 3);
        assert!(r.queries.iter().all(|q| q.commit == 1 || q.commit == 2));
        assert_eq!(r.queries.first().map(|q| q.tc), Some(0));
    }

    #[test]
    fn k1_follows_hash_order_and_stops_at_max_seeds() {
        let ps = paths(8);
        let pr: Vec<&str> = ps.iter().map(String::as_str).collect();
        let cs = cands(&pr);
        let s = snap(&[("deadbeef", &pr[..])]);
        let r = build_replay(&s, None, &cs, &ReplayParams::default(), &[Shape::K1]);
        let mut ord: Vec<u32> = (0..8).collect();
        ord.sort_by_key(|&f| seed_order_hash("deadbeef", &ps[f as usize]));
        let seeds: Vec<u32> = r.queries.iter().map(|q| q.seeds[0]).collect();
        assert_eq!(seeds, ord[..5]);
        for q in &r.queries {
            let want: Vec<u32> = (0..8).filter(|&f| f != q.seeds[0]).collect();
            assert_eq!(q.targets, want, "k1 targets in file order");
            assert_eq!(q.weights, [1.0]);
            assert!(q.c0.is_empty(), "no older commit");
        }
    }

    #[test]
    fn multi_seed_shapes_need_files_and_keep_hash_order() {
        let ps = paths(6);
        let pr: Vec<&str> = ps.iter().map(String::as_str).collect();
        let cs = cands(&pr);
        let s = snap(&[
            ("t2", &pr[..2]),
            ("t3", &pr[..3]),
            ("t4", &pr[..4]),
            ("t6", &pr[..6]),
        ]);
        let r = build_replay(
            &s,
            None,
            &cs,
            &ReplayParams::default(),
            &[Shape::K2, Shape::K3, Shape::Half],
        );
        let shapes = |k: usize| -> Vec<Shape> {
            r.queries
                .iter()
                .filter(|q| q.commit == k)
                .map(|q| q.shape)
                .collect()
        };
        assert!(shapes(0).is_empty());
        assert_eq!(shapes(1), [Shape::K2]);
        assert_eq!(shapes(2), [Shape::K2, Shape::K3, Shape::Half]);
        assert_eq!(shapes(3), [Shape::K2, Shape::K3, Shape::Half]);
        let mut ord: Vec<u32> = (0..6).collect();
        ord.sort_by_key(|&f| seed_order_hash("t6", &ps[f as usize]));
        let half = r
            .queries
            .iter()
            .find(|q| q.commit == 3 && q.shape == (Shape::Half))
            .map(|q| (q.seeds.clone(), q.targets.clone()));
        assert_eq!(half, Some((ord[..3].to_vec(), ord[3..].to_vec())));
        let k3 = r
            .queries
            .iter()
            .find(|q| q.commit == 3 && q.shape == (Shape::K3))
            .map(|q| q.targets.clone());
        assert_eq!(k3, Some(ord[3..].to_vec()));
    }

    #[test]
    fn pre_rename_commit_old_path_drops() {
        let cs = cands(&["src/new.rs", "src/b.rs"]);
        let mut renamed = file("src/new.rs");
        renamed.from = Some("src/old.rs".into());
        let mut s = snap(&[("x", &["src/old.rs", "src/b.rs"])]);
        s.commits.insert(
            0,
            HistoryCommit {
                c: "y".into(),
                t: 9,
                files: vec![renamed, file("src/b.rs")],
            },
        );
        let r = build_replay(&s, None, &cs, &ReplayParams::default(), &[Shape::K1]);
        let b = cs.position("src/b.rs").map(|p| p as u32);
        let n = cs.position("src/new.rs").map(|p| p as u32);
        assert_eq!(
            r.commits[0].files,
            b.into_iter().collect::<Vec<_>>(),
            "old path drops"
        );
        let mut both: Vec<u32> = b.into_iter().chain(n).collect();
        both.sort_unstable();
        assert_eq!(
            r.commits[1].files, both,
            "the rename commit maps its new path"
        );
    }

    #[test]
    fn c0_is_leak_free_and_equals_hand_counts() {
        let cs = cands(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        // a=0 b=1 c=2 d=3
        let s = snap(&[
            ("1", &["a.rs", "b.rs"]),
            ("2", &["a.rs", "b.rs", "c.rs"]),
            ("3", &["a.rs"]),
            ("4", &["a.rs", "d.rs"]),
        ]);
        let p = ReplayParams {
            max_seeds: 2,
            ..ReplayParams::default()
        };
        let r = build_replay(&s, None, &cs, &p, &[Shape::K1]);
        assert_eq!(r.tests, [0, 1, 3]);
        // Test "4": older training commits 1, 2, 3 -> cnt[a]=3, co[a][b]=2, co[a][c]=1;
        // cnt[d]=0. Its own pair (a, d) must not leak.
        let qa = r
            .queries
            .iter()
            .find(|q| q.commit == 3 && q.seeds == [0])
            .map(|q| q.c0.clone());
        assert_eq!(qa, Some(vec![(1, 2.0 / 4.0), (2, 1.0 / 4.0)]));
        let qd = r
            .queries
            .iter()
            .find(|q| q.commit == 3 && q.seeds == [3])
            .map(|q| q.c0.clone());
        assert_eq!(qd, Some(Vec::new()));
        // Test "1": nothing older.
        assert!(
            r.queries
                .iter()
                .filter(|q| q.commit == 0)
                .all(|q| q.c0.is_empty())
        );
    }

    #[test]
    fn c0_seeds_is_rank_c0() {
        let mut co = CoState::new(5);
        co.observe(&[0, 1, 2], 30);
        co.observe(&[0, 3], 30);
        co.observe(&[1, 2, 4], 30);
        co.observe(&(0..5).collect::<Vec<_>>(), 4); // too wide: ignored
        let seeds = [2u32, 0];
        let dense = c0(&co, &seeds, 5);
        let sparse = co.c0_seeds(&seeds);
        let from_dense: Vec<(u32, f64)> = dense
            .iter()
            .enumerate()
            .filter(|x| *x.1 != 0.0)
            .map(|(f, &v)| (f as u32, v))
            .collect();
        assert_eq!(sparse, from_dense);
        // r3_tqueries' hand sum: seed c (cnt 2): a 1/3, b 2/3, e 1/3; seed a (cnt 2): b 1/3, c 1/3, d 1/3.
        assert_eq!(
            sparse,
            vec![
                (0, 1.0 / 3.0),
                (1, 2.0 / 3.0 + 1.0 / 3.0),
                (2, 1.0 / 3.0),
                (3, 1.0 / 3.0),
                (4, 1.0 / 3.0)
            ]
        );
        assert_eq!(co.cochanged(1), &[(0, 1), (2, 2), (4, 1)]);
    }

    #[test]
    fn find_top_and_c0_weighted_hand_case() {
        let mut co = CoState::new(4);
        co.observe(&[0, 1], 30);
        co.observe(&[0, 2], 30);
        co.observe(&[3, 2], 30);
        let t1s = [2.0, 0.0, -1.0, 2.0];
        let raw = find_top_scores(&t1s);
        assert_eq!(raw, vec![(0, 2.0), (3, 2.0)], "score desc, file asc");
        assert_eq!(find_top(&t1s), vec![(0, 0.5), (3, 0.5)]);
        // g=0 (cnt 2, d 3): f1 += 2/4*1/3, f2 += 2/4*1/3; g=3 (cnt 1, d 2): f2 += 2/4*1/2.
        let w = co.c0_weighted(&raw);
        assert_eq!(
            w,
            vec![
                (1, 2.0 / 4.0 * 1.0 / 3.0),
                (2, 2.0 / 4.0 * 1.0 / 3.0 + 2.0 / 4.0 * 1.0 / 2.0)
            ]
        );
        let many: Vec<f64> = (0..15).map(f64::from).collect();
        let top = find_top_scores(&many);
        assert_eq!(top.len(), FIND_TOP);
        assert_eq!(top[0], (14, 14.0));
    }

    #[test]
    fn messages_fill_subjects_and_absence_leaves_them_empty() {
        let cs = cands(&["a.rs", "b.rs"]);
        let s = snap(&[("m1", &["a.rs", "b.rs"])]);
        let m = msgs(&s);
        let r = build_replay(&s, Some(&m), &cs, &ReplayParams::default(), &ALL);
        assert!(r.messages);
        assert_eq!(r.commits[0].subject, "subject m1");
        assert_eq!(r.commits[0].message, "subject m1\nbody");
        assert_eq!(r.commits[0].at, r.commits[0].t - 1);
        let none = build_replay(&s, None, &cs, &ReplayParams::default(), &ALL);
        assert!(!none.messages);
        assert!(
            none.commits
                .iter()
                .all(|c| c.subject.is_empty() && c.message.is_empty())
        );
        let find = none
            .queries
            .iter()
            .find(|q| q.shape == (Shape::Find))
            .map(|q| (q.seeds.clone(), q.targets.clone()));
        assert_eq!(find, Some((Vec::new(), vec![0, 1])));
    }

    #[test]
    fn replay_is_deterministic() {
        let ps = paths(12);
        let pr: Vec<&str> = ps.iter().map(String::as_str).collect();
        let cs = cands(&pr);
        let rows: Vec<(String, Vec<&str>)> = (0..40)
            .map(|i| {
                (
                    format!("{i:040x}"),
                    pr.iter()
                        .copied()
                        .filter(|p| (p.len() + i) % 3 != 0)
                        .take(2 + i % 7)
                        .collect(),
                )
            })
            .collect();
        let rows_ref: Vec<(&str, &[&str])> = rows
            .iter()
            .map(|(s, f)| (s.as_str(), f.as_slice()))
            .collect();
        let s = snap(&rows_ref);
        let m = msgs(&s);
        let a = build_replay(&s, Some(&m), &cs, &ReplayParams::default(), &ALL);
        let b = build_replay(
            &s,
            Some(&m),
            &cs,
            &ReplayParams::default(),
            &[Shape::Find, Shape::Half, Shape::K3, Shape::K2, Shape::K1],
        );
        assert_eq!(a, b);
        assert!(!a.queries.is_empty());
        let order: Vec<(usize, u8)> = a.queries.iter().map(|q| (q.tc, q.shape.order())).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "(tc, shape order)");
    }
}
