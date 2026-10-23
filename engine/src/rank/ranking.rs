//! The measured structure rankings (EA.2, items 1, 2 and 10): which file
//! ranking an answer applies, named as rank_eval.rs names the ranker it ports.
//!
//! - `P1` ([`RankBase::Ppr`]): undirected PPR at
//!   `CODE_TABLES.activation_config(None)` (damping 0.5), seeded uniformly over
//!   every node of the seed files, max-pooled per file (rank_eval's `base_cfg`,
//!   `Ppr::uniform`, `file_pool(.., false)`).
//! - `PT*` ([`RankBase::PprWide`]): the same at [`WIDE_DAMPING`] with sum
//!   pooling (`r1_tune_indexes`' grid entry `base,d85`, `file_pool(.., true)`).
//! - `B0` ([`RankBase::Bfs`]): carry-edge BFS in both directions from the seed
//!   files' nodes, key `[-depth, file degree]` (`bfs_key`).
//!
//! Each has a co-change-first form (`run_query`, `r3_transfer`): `H10` =
//! `[C0, P1]`, `H10[PT*]` = `[C0, PT*]`, `H10[B0]` = `[C0, -depth, degree]`.

use glia_code_domain::profile::CODE_TABLES;

/// `PT*`'s damping: rank_eval.rs `R1_DAMP`'s picked entry (grid name
/// `base,d85`). It is not CODE_TABLES' default: round 3 measured PT* - P1 at
/// -0.035 on one repo and +0.019 on another, so d0.85 reaches the answers
/// only as a selectable candidate (item 4 / 10's selection), and hotspots /
/// communities, which read CODE_TABLES, must not move with a ranking
/// candidate. The edge weights stay CODE_TABLES' base weights, as PT* used
/// them.
pub const WIDE_DAMPING: f64 = 0.85;

/// The structural score a file ranking starts from.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RankBase {
    /// `P1`: PPR at the code domain's damping, max-pooled per file.
    Ppr,
    /// `PT*`: PPR at [`WIDE_DAMPING`], sum-pooled per file.
    PprWide,
    /// `B0`: carry-edge BFS both ways, key `[-depth, file degree]`.
    Bfs,
}

impl RankBase {
    /// The CLI / pyo3 name: `ppr`, `ppr-wide`, `bfs`.
    pub fn name(self) -> &'static str {
        match self {
            RankBase::Ppr => "ppr",
            RankBase::PprWide => "ppr-wide",
            RankBase::Bfs => "bfs",
        }
    }

    /// The inverse of [`Self::name`].
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "ppr" => Ok(RankBase::Ppr),
            "ppr-wide" => Ok(RankBase::PprWide),
            "bfs" => Ok(RankBase::Bfs),
            _ => Err(format!("unknown ranking `{s}`: expected one of ppr, ppr-wide, bfs")),
        }
    }

    /// The rank_eval ranker this base ports: `P1`, `PT*`, `B0`.
    pub fn harness_name(self) -> &'static str {
        match self {
            RankBase::Ppr => "P1",
            RankBase::PprWide => "PT*",
            RankBase::Bfs => "B0",
        }
    }

    /// The inverse of [`Self::harness_name`] (EC's calibration record stores
    /// harness names).
    pub fn from_harness_name(s: &str) -> Option<Self> {
        match s {
            "P1" => Some(RankBase::Ppr),
            "PT*" => Some(RankBase::PprWide),
            "B0" => Some(RankBase::Bfs),
            _ => None,
        }
    }

    /// The PPR damping, or None for the BFS ranking.
    pub fn damping(self) -> Option<f64> {
        match self {
            RankBase::Ppr => Some(CODE_TABLES.activation_config(None).damping),
            RankBase::PprWide => Some(WIDE_DAMPING),
            RankBase::Bfs => None,
        }
    }

    /// Whether node scores pool per file by sum (else by max).
    pub fn sum_pooled(self) -> bool {
        match self {
            RankBase::Ppr => false,
            RankBase::PprWide => true,
            RankBase::Bfs => false,
        }
    }
}

/// A file ranking: a structural base, optionally with co-change first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ranking {
    pub base: RankBase,
    /// Co-change from the history snapshot ranks first, the base breaks its
    /// ties (a no-op with no snapshot).
    pub cochange_first: bool,
}

impl Default for Ranking {
    /// `H10`, the agreed default (item 2): about +0.10 MRR on every repo in
    /// the first run; round 3's review shape H10 - P1 above 0 on 5 / 4 / 4 of
    /// 12 repos, below on 0 / 0 / 1.
    fn default() -> Self {
        Ranking { base: RankBase::Ppr, cochange_first: true }
    }
}

impl Ranking {
    /// `P1`: the 0.5.2 row order.
    pub const fn head() -> Self {
        Ranking { base: RankBase::Ppr, cochange_first: false }
    }

    pub const fn new(base: RankBase, cochange_first: bool) -> Self {
        Ranking { base, cochange_first }
    }

    /// `ppr`, `ppr-wide`, `bfs`, or `cochange+<base>`.
    pub fn name(&self) -> String {
        if self.cochange_first { format!("cochange+{}", self.base.name()) } else { self.base.name().to_string() }
    }

    /// The rank_eval ranker this ranking ports: `P1`, `PT*`, `B0`, `H10`,
    /// `H10[PT*]`, `H10[B0]`.
    pub fn harness_name(&self) -> &'static str {
        match (self.base, self.cochange_first) {
            (RankBase::Ppr, false) => "P1",
            (RankBase::PprWide, false) => "PT*",
            (RankBase::Bfs, false) => "B0",
            (RankBase::Ppr, true) => "H10",
            (RankBase::PprWide, true) => "H10[PT*]",
            (RankBase::Bfs, true) => "H10[B0]",
        }
    }

    /// The inverse of [`Self::harness_name`]: exactly the six names, anything
    /// else None (EC.7, folded in here).
    pub fn from_harness_name(name: &str) -> Option<Self> {
        match name {
            "H10" => Some(Ranking::new(RankBase::Ppr, true)),
            "H10[PT*]" => Some(Ranking::new(RankBase::PprWide, true)),
            "H10[B0]" => Some(Ranking::new(RankBase::Bfs, true)),
            other => RankBase::from_harness_name(other).map(|b| Ranking::new(b, false)),
        }
    }

    /// A base name ([`RankBase::parse`]) and whether co-change ranks first.
    pub fn parse(base: &str, cochange: bool) -> Result<Self, String> {
        RankBase::parse(base).map(|b| Ranking::new(b, cochange))
    }
}

/// Item 10's structure-only candidate set, in this order: SELs0 (rank_eval.rs
/// `r3_sets`: P1, PT*, B0). With co-change first on top it is SELh0's set
/// (H10, H10[PT*], H10[B0]).
pub const STRUCTURE_CANDIDATES: [RankBase; 3] = [RankBase::Ppr, RankBase::PprWide, RankBase::Bfs];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_h10() {
        assert_eq!(Ranking::default().harness_name(), "H10");
        assert_eq!(Ranking::default().name(), "cochange+ppr");
        assert_eq!(Ranking::head().harness_name(), "P1");
    }

    #[test]
    fn base_names_round_trip() {
        for b in STRUCTURE_CANDIDATES {
            assert_eq!(RankBase::parse(b.name()), Ok(b));
            assert_eq!(RankBase::from_harness_name(b.harness_name()), Some(b));
        }
        assert_eq!(
            RankBase::parse("pagerank"),
            Err("unknown ranking `pagerank`: expected one of ppr, ppr-wide, bfs".to_string())
        );
        assert_eq!(RankBase::from_harness_name("H10"), None);
    }

    #[test]
    fn structure_candidates_are_sels0() {
        let names: Vec<&str> = STRUCTURE_CANDIDATES.iter().map(|b| b.harness_name()).collect();
        assert_eq!(names, ["P1", "PT*", "B0"]);
        let h: Vec<&str> = STRUCTURE_CANDIDATES.iter().map(|&b| Ranking::new(b, true).harness_name()).collect();
        assert_eq!(h, ["H10", "H10[PT*]", "H10[B0]"]);
    }

    #[test]
    fn six_harness_names_round_trip() {
        let six = ["P1", "PT*", "B0", "H10", "H10[PT*]", "H10[B0]"];
        for n in six {
            let r = Ranking::from_harness_name(n).expect(n);
            assert_eq!(r.harness_name(), n);
            assert_eq!(Ranking::parse(r.base.name(), r.cochange_first), Ok(r));
        }
        for bad in ["", "p1", "H10[P1]", "H9s50", "LX[C0,W5,P1]"] {
            assert_eq!(Ranking::from_harness_name(bad), None, "{bad}");
        }
        let names: Vec<String> = six.iter().filter_map(|n| Ranking::from_harness_name(n)).map(|r| r.name()).collect();
        assert_eq!(names, ["ppr", "ppr-wide", "bfs", "cochange+ppr", "cochange+ppr-wide", "cochange+bfs"]);
    }

    #[test]
    fn damping_and_pooling() {
        assert_eq!(RankBase::Ppr.damping(), Some(0.5));
        assert_eq!(RankBase::PprWide.damping(), Some(WIDE_DAMPING));
        assert_eq!(RankBase::Bfs.damping(), None);
        assert!(RankBase::PprWide.sum_pooled());
        assert!(!RankBase::Ppr.sum_pooled() && !RankBase::Bfs.sum_pooled());
    }
}
