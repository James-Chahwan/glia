//! MinHash (CD.4d): seeded MinHash signatures over id sets with
//! integer-only hashing, LSH banding of those signatures into candidate
//! pairs, and exact Jaccard similarity to confirm a candidate. Domain-free:
//! the sets are plain ids the caller chose, never a named code kind.
//!
//! - [`MinHasher`] draws `k` seeds from one `SplitMix64` stream (the one
//!   [`community`](crate::algo::community) draws from); permutation `i`
//!   hashes an id `x` to `mix64(x ^ seed_i)`, where `mix64` is SplitMix64's
//!   integer-only output finaliser. A signature is the per-permutation
//!   minimum over the set, so it depends on the set only (not its order or
//!   repeats), and two sets agree at a position with probability equal to
//!   their Jaccard similarity.
//! - [`lsh_candidates`] cuts each signature into `bands` bands of `rows`
//!   values; two sets sharing every value of some band are a candidate pair.
//!   Sets at Jaccard `J` pair with probability `1 - (1 - J^rows)^bands`, so
//!   near-duplicates are found without comparing every pair.
//! - [`jaccard_sorted`] is the exact check: every reported pair is verified
//!   with it, so a candidate is never an answer by itself.
//!
//! Everything is integer arithmetic (wrapping multiplies, shifts, xors) and
//! every grouping is a sort, never a `HashMap` walk, so a signature and a
//! candidate list are identical on every platform and at any thread count.

use super::community::{SplitMix64, mix64};

/// A band bucket with more members than this is skipped (and counted in
/// [`LshCandidates::oversized_buckets`]) instead of emitting its pairs: a
/// degenerate band, such as every flow sharing one hub, would otherwise emit
/// millions of pairs that say nothing.
pub const MAX_BUCKET: usize = 2_000;

/// `k` seeded MinHash permutations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinHasher {
    /// Permutation `i` hashes `x` to `mix64(x ^ seeds[i])`.
    seeds: Vec<u64>,
}

impl MinHasher {
    /// `k` permutations whose seeds are the first `k` draws of
    /// `SplitMix64(seed)`: the same `(k, seed)` gives the same permutations
    /// everywhere, and a longer signature extends a shorter one.
    pub fn new(k: usize, seed: u64) -> Self {
        let mut rng = SplitMix64::new(seed);
        Self { seeds: (0..k).map(|_| rng.next_u64()).collect() }
    }

    /// The signature length.
    pub fn k(&self) -> usize {
        self.seeds.len()
    }

    /// The signature of `set`: value `i` is the minimum of
    /// `mix64(x ^ seed_i)` over the ids `x` in `set`. The order of `set` and
    /// a repeated id change nothing; an empty set's signature is `k` copies
    /// of `u64::MAX`.
    pub fn signature(&self, set: &[u64]) -> Vec<u64> {
        let mut sig = vec![u64::MAX; self.seeds.len()];
        for &x in set {
            for (m, &s) in sig.iter_mut().zip(&self.seeds) {
                let h = mix64(x ^ s);
                if h < *m {
                    *m = h;
                }
            }
        }
        sig
    }
}

/// The candidate pairs of an LSH banding pass.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LshCandidates {
    /// `(i, j)` indices into the signatures, `i < j`, sorted and
    /// deduplicated: the pairs that share every value of at least one band
    /// (or, with chance about `n^2 / 2^64` per band, whose band keys collide;
    /// the exact check drops those).
    pub pairs: Vec<(u32, u32)>,
    /// Band buckets over [`MAX_BUCKET`] members, skipped: their pairs are
    /// candidates only through another band.
    pub oversized_buckets: usize,
    /// Signatures whose length is not `bands * rows`: they join no bucket.
    /// Non-zero only when the caller's shape disagrees with its
    /// [`MinHasher::k`].
    pub misshapen: usize,
}

/// The candidate pairs among `sigs` when each signature is cut into `bands`
/// consecutive bands of `rows` values: every pair whose signatures agree on
/// a whole band. Each band's `rows` values are chained through `mix64` into a
/// `u64` key, and the `(key, index)` pairs are sorted so a bucket is a run of
/// one key with its members in index order. A bucket of 2 to
/// [`MAX_BUCKET`] members emits all its pairs; a bigger one is skipped and
/// counted.
///
/// `bands` and `rows` must be positive and `bands * rows` must equal each
/// signature's length; a signature of any other length (every one of them,
/// when the shape is wrong) is counted in [`LshCandidates::misshapen`] and
/// joins no bucket. Only the first `2^32` signatures are banded, since an
/// index is a `u32`.
pub fn lsh_candidates(sigs: &[Vec<u64>], bands: usize, rows: usize) -> LshCandidates {
    let width = bands.checked_mul(rows).filter(|&w| w > 0);
    let mut shaped: Vec<(&[u64], u32)> = Vec::with_capacity(sigs.len());
    let mut misshapen = 0usize;
    for (sig, ix) in sigs.iter().zip(0..=u32::MAX) {
        if width == Some(sig.len()) {
            shaped.push((sig.as_slice(), ix));
        } else {
            misshapen += 1;
        }
    }

    let mut pairs: Vec<(u32, u32)> = Vec::new();
    if shaped.len() < 2 {
        // Nothing to pair; also keeps a bad shape (bands * rows past
        // usize) from walking its bands.
        return LshCandidates { pairs, oversized_buckets: 0, misshapen };
    }
    let mut compacted = 0usize;
    let mut oversized_buckets = 0usize;
    let mut keyed: Vec<(u64, u32)> = Vec::with_capacity(shaped.len());
    for band in 0..bands {
        let span = band * rows..(band + 1) * rows;
        keyed.clear();
        keyed.extend(shaped.iter().map(|&(sig, ix)| (band_key(&sig[span.clone()]), ix)));
        keyed.sort_unstable();
        for bucket in keyed.chunk_by(|a, b| a.0 == b.0) {
            if bucket.len() > MAX_BUCKET {
                oversized_buckets += 1;
                continue;
            }
            for (n, &(_, i)) in bucket.iter().enumerate() {
                pairs.extend(bucket[n + 1..].iter().map(|&(_, j)| (i, j)));
            }
        }
        // Near-duplicates share many bands, so the same pair recurs; compact
        // whenever the list doubles to keep it near its distinct size.
        if pairs.len() > 2 * compacted.max(1 << 16) {
            pairs.sort_unstable();
            pairs.dedup();
            compacted = pairs.len();
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    LshCandidates { pairs, oversized_buckets, misshapen }
}

/// A band's bucket key: its values chained through `mix64`.
fn band_key(rows: &[u64]) -> u64 {
    rows.iter().fold(0u64, |h, &v| mix64(h ^ v))
}

/// The exact Jaccard similarity of two sets given as ascending id lists:
/// `(intersection, union)` sizes, so the similarity is
/// `intersection / union` (two empty sets give `(0, 0)`, for the caller to
/// read). A repeated id counts once. Lists that are not ascending give
/// meaningless counts, never a panic. A count past `u32::MAX` saturates.
pub fn jaccard_sorted(a: &[u64], b: &[u64]) -> (u32, u32) {
    let (mut i, mut j) = (0usize, 0usize);
    let (mut inter, mut union) = (0u64, 0u64);
    let mut last: Option<u64> = None;
    loop {
        // The smallest id not yet consumed; both lists are at it when both
        // hold it, since everything smaller is behind them.
        let x = match (a.get(i), b.get(j)) {
            (Some(&x), Some(&y)) if x == y => {
                i += 1;
                j += 1;
                if last != Some(x) {
                    inter += 1;
                }
                x
            }
            (Some(&x), Some(&y)) if x < y => {
                i += 1;
                x
            }
            (_, Some(&y)) => {
                j += 1;
                y
            }
            (Some(&x), None) => {
                i += 1;
                x
            }
            (None, None) => break,
        };
        if last != Some(x) {
            union += 1;
            last = Some(x);
        }
    }
    let sat = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    (sat(inter), sat(union))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Knuth's MMIX LCG: full period mod 2^64, so one stream never repeats
    /// a value and sets drawn from it are disjoint unless built to share.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            (self.next() >> 33) % n
        }

        fn take(&mut self, n: usize) -> Vec<u64> {
            (0..n).map(|_| self.next()).collect()
        }
    }

    fn sorted(mut v: Vec<u64>) -> Vec<u64> {
        v.sort_unstable();
        v.dedup();
        v
    }

    fn estimate(a: &[u64], b: &[u64]) -> f64 {
        let agree = a.iter().zip(b).filter(|(x, y)| x == y).count();
        agree as f64 / a.len() as f64
    }

    fn exact(a: &[u64], b: &[u64]) -> f64 {
        let (i, u) = jaccard_sorted(a, b);
        f64::from(i) / f64::from(u)
    }

    /// Two sets over one fresh LCG stream with `shared` ids in common and
    /// `only_a` / `only_b` ids of their own.
    fn pair(rng: &mut Lcg, shared: usize, only_a: usize, only_b: usize) -> (Vec<u64>, Vec<u64>) {
        let common = rng.take(shared);
        let mut a = common.clone();
        a.extend(rng.take(only_a));
        let mut b = common;
        b.extend(rng.take(only_b));
        (sorted(a), sorted(b))
    }

    #[test]
    fn estimate_tracks_jaccard() {
        let mh = MinHasher::new(256, 0x5eed);
        let mut rng = Lcg(2026);
        let mut total_err = 0.0;
        for t in 0..50u32 {
            // Targets spread over [0.1, 0.9], unions of 60 to 400 ids.
            let target = 0.1 + 0.8 * f64::from(t) / 49.0;
            let union = 60 + rng.below(341) as usize;
            let shared = ((target * union as f64).round() as usize).max(1);
            let rest = union - shared;
            let only_a = rng.below(rest as u64 + 1) as usize;
            let (a, b) = pair(&mut rng, shared, only_a, rest - only_a);
            let truth = exact(&a, &b);
            assert!((0.09..=0.91).contains(&truth), "pair {t}: true Jaccard {truth}");
            let err = (estimate(&mh.signature(&a), &mh.signature(&b)) - truth).abs();
            assert!(err <= 0.125, "pair {t}: |estimate - {truth}| = {err}");
            total_err += err;
        }
        let mean = total_err / 50.0;
        assert!(mean <= 0.03, "mean absolute error {mean}");
    }

    #[test]
    fn identical_sets_identical_signatures() {
        let mh = MinHasher::new(64, 9);
        let a: Vec<u64> = (0..100).map(|x| x * 7 + 3).collect();
        let b = a.clone();
        assert_eq!(mh.signature(&a), mh.signature(&b));
        assert_eq!(estimate(&mh.signature(&a), &mh.signature(&b)), 1.0);
        // Each permutation is a bijection, so a disjoint set's minimum is a
        // different value at every position (one extra id would move each
        // position with chance 1/101 only, so it proves nothing).
        let c: Vec<u64> = a.iter().map(|x| x + 1).collect();
        let (sa, sc) = (mh.signature(&a), mh.signature(&c));
        assert!(sa.iter().zip(&sc).all(|(x, y)| x != y));
    }

    #[test]
    fn empty_set_all_max() {
        let mh = MinHasher::new(32, 1);
        assert_eq!(mh.signature(&[]), vec![u64::MAX; 32]);
        let none = MinHasher::new(0, 1);
        assert_eq!(none.k(), 0);
        assert!(none.signature(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn lsh_finds_near_pairs() {
        let mh = MinHasher::new(128, 77);
        let mut rng = Lcg(51);
        let mut sets: Vec<Vec<u64>> = (0..180)
            .map(|_| {
                let n = 20 + rng.below(40) as usize;
                sorted(rng.take(n))
            })
            .collect();
        // 10 planted pairs: 40 to 57 shared ids, 1 to 3 of each side's own.
        let mut planted = BTreeSet::new();
        for p in 0..10u64 {
            let (a, b) = pair(&mut rng, 40 + 2 * p as usize, 1 + (p % 3) as usize, 3 - (p % 3) as usize);
            assert!(exact(&a, &b) >= 0.85, "planted pair {p}: {}", exact(&a, &b));
            let (i, j) = (rng.below(sets.len() as u64 + 1) as usize, rng.below(sets.len() as u64 + 2) as usize);
            sets.insert(i, a);
            sets.insert(j, b);
        }
        assert_eq!(sets.len(), 200);
        // Recover where each planted pair landed: its twin is the one set it
        // shares an id with.
        for x in 0..sets.len() {
            for y in x + 1..sets.len() {
                if jaccard_sorted(&sets[x], &sets[y]).0 > 0 {
                    planted.insert((x as u32, y as u32));
                }
            }
        }
        assert_eq!(planted.len(), 10);

        let sigs: Vec<Vec<u64>> = sets.iter().map(|s| mh.signature(s)).collect();
        let got = lsh_candidates(&sigs, 32, 4);
        assert_eq!((got.oversized_buckets, got.misshapen), (0, 0));
        assert!(got.pairs.windows(2).all(|w| w[0] < w[1]), "sorted, deduplicated");
        assert!(got.pairs.iter().all(|&(i, j)| i < j));
        for pair in &planted {
            assert!(got.pairs.contains(pair), "planted pair {pair:?} is not a candidate");
        }
        // Each permutation is a bijection, so sets with no id in common
        // never agree at any position: the unplanted pairs are no candidates.
        assert_eq!(got.pairs, planted.into_iter().collect::<Vec<_>>());
    }

    #[test]
    fn deterministic_and_order_free() {
        let mh = MinHasher::new(48, 3);
        let set: Vec<u64> = (0..60).map(|x| x * x + 11).collect();
        let mut shuffled = set.clone();
        shuffled.reverse();
        shuffled.swap(3, 40);
        let mut repeated = set.clone();
        repeated.extend_from_slice(&set[..10]);
        assert_eq!(mh.signature(&set), mh.signature(&shuffled));
        assert_eq!(mh.signature(&set), mh.signature(&repeated));

        assert_eq!(MinHasher::new(48, 3).seeds, mh.seeds);
        assert_ne!(MinHasher::new(48, 4).seeds, mh.seeds);
        // A longer signature extends a shorter one.
        assert_eq!(MinHasher::new(64, 3).seeds[..48], mh.seeds[..]);
        // SplitMix64(0)'s reference first outputs, and a signature pinned
        // against an independent (Python, arbitrary-precision) evaluation:
        // the same bits on every platform.
        assert_eq!(MinHasher::new(4, 0).seeds, [0xe220a8397b1dcdaf, 0x6e789e6aa1b965f4, 0x06c45d188009454f, 0xf88bb8a8724c81ec]);
        assert_eq!(MinHasher::new(3, 7).signature(&[40, 2, 1, 3]), [0x0524257c04fcf117, 0x2ac41d15edbb29d9, 0x24ed189de445e5d4]);
        assert_eq!(band_key(&[1, 2]), 0xef30b01c2974aeeb);

        let sigs: Vec<Vec<u64>> = (0..30u64).map(|s| mh.signature(&[s % 5, 100, 200 + s % 3])).collect();
        assert_eq!(lsh_candidates(&sigs, 12, 4), lsh_candidates(&sigs, 12, 4));
    }

    #[test]
    fn jaccard_sorted_exact() {
        assert_eq!(jaccard_sorted(&[1, 2, 3, 4], &[3, 4, 5]), (2, 5));
        assert_eq!(jaccard_sorted(&[1, 3, 5], &[2, 4, 6]), (0, 6));
        assert_eq!(jaccard_sorted(&[7, 8, 9], &[7, 8, 9]), (3, 3));
        assert_eq!(jaccard_sorted(&[5], &[1, 2, 5, 9]), (1, 4));
        assert_eq!(jaccard_sorted(&[1], &[]), (0, 1));
        assert_eq!(jaccard_sorted(&[], &[]), (0, 0));
        // A repeated id counts once, on either side or both.
        assert_eq!(jaccard_sorted(&[1, 1, 2], &[1, 2, 2]), (2, 2));
        assert_eq!(jaccard_sorted(&[0, 1, 1], &[1, 1, 3]), (1, 3));
        assert_eq!(jaccard_sorted(&[u64::MAX], &[0, u64::MAX]), (1, 2));
    }

    #[test]
    fn oversized_bucket_skipped_and_counted() {
        // One band of every value: identical signatures share its bucket.
        let same = |n: usize| vec![vec![5u64, 6]; n];
        let at_cap = lsh_candidates(&same(MAX_BUCKET), 1, 2);
        assert_eq!(at_cap.pairs.len(), MAX_BUCKET * (MAX_BUCKET - 1) / 2);
        assert_eq!(at_cap.oversized_buckets, 0);
        let over = lsh_candidates(&same(MAX_BUCKET + 1), 1, 2);
        assert!(over.pairs.is_empty());
        assert_eq!(over.oversized_buckets, 1);
        // Band 0 is one oversized bucket; band 1 splits off {0, 1}, whose
        // pair it still emits, from another oversized bucket.
        let mut sigs = vec![vec![5u64, 6, 1, 1]; MAX_BUCKET + 3];
        sigs[0][2] = 9;
        sigs[1][2] = 9;
        let partly = lsh_candidates(&sigs, 2, 2);
        assert_eq!((partly.oversized_buckets, partly.misshapen), (2, 0));
        assert_eq!(partly.pairs, [(0, 1)]);
    }

    #[test]
    fn misshapen_signatures_join_no_bucket() {
        let sigs = vec![vec![1u64, 2, 3, 4], vec![1, 2, 3, 4], vec![1, 2, 3], vec![1, 2, 3, 4]];
        let got = lsh_candidates(&sigs, 2, 2);
        assert_eq!(got.misshapen, 1);
        assert_eq!(got.pairs, [(0, 1), (0, 3), (1, 3)]);
        // A shape that fits no signature: nothing is banded, nothing panics.
        let wrong = lsh_candidates(&sigs, 3, 2);
        assert_eq!((wrong.pairs.len(), wrong.misshapen), (0, 4));
        let empty = lsh_candidates(&[vec![], vec![]], 0, 0);
        assert_eq!((empty.pairs.len(), empty.misshapen), (0, 2));
        assert!(lsh_candidates(&[], 32, 4).pairs.is_empty());
    }
}
