//! Rank fusion over per-candidate score vectors (0.5.3, slot E0.1, filled by
//! EB.2): the min-max normalisation over a candidate set, the weighted blends,
//! the equal means and the lexicographic order (LX) that every 0.5.3 text
//! answer and the per-repo calibration bank share. Domain-free: a caller hands
//! in one score per signal per candidate, indexed by candidate position, and
//! gets back an order over those positions; nothing here names a node kind,
//! an edge category or a file. Ties go by a caller-given total order (`tie`),
//! then candidate index, never a NodeId and never map order.
//!
//! Each item reproduces the arithmetic of `engine/examples/rank_eval.rs`, the
//! harness that measured it:
//!
//! - [`minmax`] is rank_eval's `minmax` / `mm_ex`: lo / hi over the candidates
//!   (every index not `excluded`, the harness's seed files); when `!(hi > lo)`
//!   (flat, empty, or every entry excluded) the whole vector is 0.0, else
//!   `(v[i] - lo) / (hi - lo)` for every index, excluded ones included.
//! - [`blend`] is `blend`: `a * x[i] + (1.0 - a) * y[i]`. The ranker H9s50 is
//!   `blend(0.5, minmax(T1s), minmax(P1))`.
//! - [`mean`] is `r3_eq`: the per-index sum over the parts divided by the part
//!   count. The ranker H18s is `mean([minmax(T1s), minmax(C0), minmax(PT*)])`.
//! - [`lex_order`] is `better` / `order` over a `Key`: the keys compared in
//!   turn, descending, with plain `>` / `<` (so -0.0 and 0.0 are equal and
//!   fall through to the next key, as in the harness), then `tie` ascending.
//!   [`order`] is one key; [`lx`] puts the typed-link key first (item 7's LX:
//!   `LX[IDR,H9s50]` = order by IDR, then H9s50).
//!
//! A fusion never adds a candidate: the order holds exactly the non-excluded
//! indices, and a key of all zeros leaves the rest of the order to the next
//! key. Under `GLIA_ALGO_DEBUG=1` every [`lex_order`] call prints
//! `[algo] fuse keys=<k> candidates=<n> excluded=<e> zero_first_key=<z>`
//! (z = candidates whose first key is 0, i.e. ordered by the rest).

use std::cmp::Ordering;

use super::algo_debug;

/// Min-max over the candidates (the indices with `!excluded[i]`); a flat,
/// empty or all-excluded vector maps to all zeros. Every index gets a value,
/// an excluded one included (callers never rank it). A non-finite entry is a
/// caller bug (debug assertion); in release it takes no part in lo / hi and
/// maps to 0.0.
pub fn minmax(v: &[f64], excluded: &[bool]) -> Vec<f64> {
    debug_assert_eq!(v.len(), excluded.len(), "minmax: score and exclusion lengths differ");
    debug_assert!(v.iter().all(|s| s.is_finite()), "minmax: non-finite score");
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, &s) in v.iter().enumerate() {
        if !excluded.get(i).copied().unwrap_or(false) && s.is_finite() {
            lo = lo.min(s);
            hi = hi.max(s);
        }
    }
    if hi.partial_cmp(&lo) != Some(Ordering::Greater) {
        return vec![0.0; v.len()];
    }
    v.iter().map(|&s| if s.is_finite() { (s - lo) / (hi - lo) } else { 0.0 }).collect()
}

/// The weighted blend `a * x[i] + (1.0 - a) * y[i]`.
pub fn blend(a: f64, x: &[f64], y: &[f64]) -> Vec<f64> {
    debug_assert_eq!(x.len(), y.len(), "blend: lengths differ");
    x.iter().zip(y).map(|(p, q)| a * p + (1.0 - a) * q).collect()
}

/// The equal mean of the parts: per index, the sum over the parts (in part
/// order) divided by the part count. No parts gives an empty vector.
pub fn mean(parts: &[&[f64]]) -> Vec<f64> {
    let Some(first) = parts.first() else {
        return Vec::new();
    };
    let n = first.len();
    debug_assert!(parts.iter().all(|p| p.len() == n), "mean: lengths differ");
    (0..n)
        .map(|i| parts.iter().map(|p| p.get(i).copied().unwrap_or(0.0)).sum::<f64>() / parts.len() as f64)
        .collect()
}

/// The non-excluded indices, best first: `keys[0][i]` .. `keys[k-1][i]`
/// compared descending with plain `>` / `<` (equal values, -0.0 against 0.0
/// included, fall through to the next key), then `tie[i]` ascending, then the
/// index itself.
pub fn lex_order(keys: &[&[f64]], tie: &[u32], excluded: &[bool]) -> Vec<usize> {
    let n = tie.len();
    debug_assert_eq!(excluded.len(), n, "lex_order: tie and exclusion lengths differ");
    debug_assert!(keys.iter().all(|k| k.len() == n), "lex_order: key and tie lengths differ");
    let key = |k: &[f64], i: usize| k.get(i).copied().unwrap_or(0.0);
    let mut ix: Vec<usize> = (0..n).filter(|&i| !excluded.get(i).copied().unwrap_or(false)).collect();
    ix.sort_by(|&a, &b| {
        for k in keys {
            let (x, y) = (key(k, a), key(k, b));
            if x > y {
                return Ordering::Less;
            }
            if x < y {
                return Ordering::Greater;
            }
        }
        tie[a].cmp(&tie[b]).then(a.cmp(&b))
    });
    if algo_debug() {
        let zero = keys.first().map_or(0, |k| ix.iter().filter(|&&i| key(k, i) == 0.0).count());
        eprintln!(
            "[algo] fuse keys={} candidates={} excluded={} zero_first_key={zero}",
            keys.len(),
            ix.len(),
            n - ix.len()
        );
    }
    ix
}

/// One score: [`lex_order`] over `[score]`.
pub fn order(score: &[f64], tie: &[u32], excluded: &[bool]) -> Vec<usize> {
    lex_order(&[score], tie, excluded)
}

/// Item 7's LX: the typed-link key first, then `rest` in turn
/// ([`lex_order`] over `[typed, rest..]`).
pub fn lx(typed: &[f64], rest: &[&[f64]], tie: &[u32], excluded: &[bool]) -> Vec<usize> {
    let mut keys: Vec<&[f64]> = Vec::with_capacity(rest.len() + 1);
    keys.push(typed);
    keys.extend_from_slice(rest);
    lex_order(&keys, tie, excluded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuse_minmax_excludes_and_flattens() {
        assert_eq!(minmax(&[3.0, 1.0, 2.0], &[true, false, false]), vec![2.0, 0.0, 1.0]);
        assert_eq!(minmax(&[4.0, 4.0, 4.0], &[false; 3]), vec![0.0; 3]);
        assert_eq!(minmax(&[1.0, 2.0], &[true, true]), vec![0.0; 2]);
        assert!(minmax(&[], &[]).is_empty());
    }

    #[test]
    fn fuse_blend_and_mean() {
        assert_eq!(blend(0.25, &[1.0, 0.0], &[0.0, 1.0]), vec![0.25, 0.75]);
        assert_eq!(blend(0.5, &[0.5, 1.0], &[0.5, 0.0]), vec![0.5, 0.5]);
        assert_eq!(mean(&[&[1.0, 0.0], &[0.0, 0.25], &[0.5, 0.5]]), vec![0.5, 0.25]);
        assert!(mean(&[]).is_empty());
    }

    #[test]
    fn fuse_lex_order_falls_through_to_tie() {
        let a = [1.0, 0.0, 0.0, 1.0];
        let b = [0.2, 0.9, 0.9, 0.1];
        assert_eq!(lex_order(&[&a, &b], &[3, 2, 1, 0], &[false; 4]), vec![0, 3, 2, 1]);
        assert_eq!(lx(&a, &[&b], &[3, 2, 1, 0], &[false; 4]), vec![0, 3, 2, 1]);
    }

    #[test]
    fn fuse_negative_zero_is_a_tie() {
        // Two candidates, one key each: 0.0 and -0.0 compare equal and fall to tie.
        let k = [0.0, -0.0];
        assert_eq!(order(&k, &[1, 0], &[false; 2]), vec![1, 0]);
        assert_eq!(order(&k, &[0, 1], &[false; 2]), vec![0, 1]);
    }

    #[test]
    fn fuse_excluded_never_ordered() {
        let s = [9.0, 1.0, 5.0, 3.0];
        let out = order(&s, &[0, 1, 2, 3], &[true, false, true, false]);
        assert_eq!(out, vec![3, 1]);
        assert!(order(&s, &[0, 1, 2, 3], &[true; 4]).is_empty());
    }

    /// The harness's `better` / `order` over a 2-key `Key`, copied for the
    /// parity check (ties to the lower index).
    fn harness_order(k: &[[f64; 2]], excl: &[bool]) -> Vec<usize> {
        let better = |a: &[f64; 2], ia: usize, b: &[f64; 2], ib: usize| {
            for i in 0..2 {
                if a[i] > b[i] {
                    return true;
                }
                if a[i] < b[i] {
                    return false;
                }
            }
            ia < ib
        };
        let mut ix: Vec<usize> = (0..k.len()).filter(|&f| !excl[f]).collect();
        ix.sort_by(|&a, &b| {
            if better(&k[a], a, &k[b], b) {
                Ordering::Less
            } else if a == b {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        });
        ix
    }

    #[test]
    fn fuse_h9s50_and_lx_idr_match_harness() {
        // Six files, file 0 the seed (excluded), as rank_eval's r3_bank_seed.
        let excl = [true, false, false, false, false, false];
        let t1s = [5.0, 0.0, 2.0, 4.0, 2.0, 1.0];
        let p1 = [0.9, 0.1, 0.3, 0.0, 0.5, 0.3];
        let idr = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let tie: Vec<u32> = (0..6).collect();
        let h9s50 = blend(0.5, &minmax(&t1s, &excl), &minmax(&p1, &excl));
        // mm(T1s) = [1.25, 0, .5, 1, .5, .25], mm(P1) = [1.8, .2, .6, 0, 1, .6].
        let want = [1.525, 0.1, 0.55, 0.5, 0.75, 0.425];
        for (g, w) in h9s50.iter().zip(want) {
            assert!((g - w).abs() < 1e-12, "{h9s50:?}");
        }
        let h9 = order(&h9s50, &tie, &excl);
        assert_eq!(h9, vec![4, 2, 3, 5, 1]);
        let k1: Vec<[f64; 2]> = h9s50.iter().map(|&s| [s, 0.0]).collect();
        assert_eq!(h9, harness_order(&k1, &excl));
        let lxo = lx(&idr, &[&h9s50], &tie, &excl);
        assert_eq!(lxo, vec![2, 5, 4, 3, 1]);
        let k2: Vec<[f64; 2]> = idr.iter().zip(&h9s50).map(|(&a, &b)| [a, b]).collect();
        assert_eq!(lxo, harness_order(&k2, &excl));
        // H18s-style equal mean keeps the harness's r3_eq accumulation.
        let m = mean(&[&t1s, &p1]);
        assert_eq!(m[2], (2.0 + 0.3) / 2.0);
    }
}
