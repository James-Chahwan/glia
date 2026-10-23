//! The calibration's candidate sets by group (EC.4, items 10 and 2; ruling
//! EA-5 + EC-7, James 2026-10-23: co-change first by default).
//!
//! Group `structure`, the text-free answers' two sets, on the seeded shapes
//! k1 / k2 / k3 / half (rank_eval `r3_sets`):
//! - `SELh0` = {H10, H10[PT*], H10[B0]}, fallback H10: the set an answer
//!   reads when co-change ranks first (the default);
//! - `SELs0` = {P1, PT*, B0}, fallback P1: read only under `--no-cochange` /
//!   `cochange=False`.
//!
//! Group `review` (EC.11, item 4; EC.12, item 8), review's text selection on
//! the same seeded shapes:
//! - `SELt2` = rank_eval `R1_SEL`'s SELt2, same order ([`REVIEW_CANDIDATES`]):
//!   SELt's ten [`super::fusions::Seeded`] fusions of T1s, P1, PT*, C0, W5 and
//!   M, then the four id-routed ones (IDR read at each test commit's parent,
//!   [`super::fusions::IdrState`]); its fallback, EB.7's `LX[IDR,P1]`, is
//!   scored only (not selectable). SELt itself is no longer replayed: no
//!   answer reads it, so a record written before EC.12 answers review `no_set`
//!   until recalibrated.
//!
//! Group `find` (EC.13, item 11), `find --files`' selection on the find
//! shape (a query with no seed file; the replay's stand-in is each test
//! commit's subject):
//! - `SELf2t` = rank_eval `R3_SELF2` + `["T1s", "PXT"]` (main's `f2t`), same
//!   order ([`FIND_CANDIDATES`]), ranked by [`super::fusions::find_keys`]
//!   (rank_eval `r3_bank_find`); its fallback, EB.8's `LX[IDR,T1s]`, is
//!   scored only (not selectable).
//!
//! [`specs`] returns the named groups' sets in registration order. A
//! structure candidate is [`Named`]: its keys come from the structure bank
//! ([`super::bank::structure_keys`]). [`register_hooks`] registers the parts
//! of the replay state a group reads beyond the bank (the text corpus, the
//! work state and the id router, once, for `review` or `find`).

use super::bank::structure_keys;
use super::candidate::{Candidate, QueryCtx, RankKey, ReplayState};
use super::fusions::{FindSeeded, IdrState, Seeded, TextState};
use super::record::Shape;
use super::replay::Replay;
use super::select::SetSpec;
use super::work::WorkState;

/// The structure-only set (item 10).
pub const STRUCTURE_SET: &str = "SELs0";
/// SELs0's candidates, in selection order.
pub const STRUCTURE_CANDIDATES: [&str; 3] = ["P1", "PT*", "B0"];
/// SELs0's fallback: the 0.5.2 order.
pub const STRUCTURE_FALLBACK: &str = "P1";
/// The co-change-first set (items 10 with 2).
pub const COCHANGE_SET: &str = "SELh0";
/// SELh0's candidates, in selection order.
pub const COCHANGE_CANDIDATES: [&str; 3] = ["H10", "H10[PT*]", "H10[B0]"];
/// SELh0's fallback: the answers' default ranking (`Ranking::default()`).
pub const COCHANGE_FALLBACK: &str = "H10";
/// The seeded query shapes both sets are calibrated on.
pub const SEEDED_SHAPES: [Shape; 4] = [Shape::K1, Shape::K2, Shape::K3, Shape::Half];

/// Review's text selection set (items 4 and 8).
pub const REVIEW_SET: &str = "SELt2";
/// SELt2's candidates, rank_eval `R1_SEL` order.
pub const REVIEW_CANDIDATES: [&str; 14] = [
    "H18s[PT*]",
    "H9s25[PT*]",
    "H9s50",
    "H10[PT*]",
    "HTsa[W5]",
    "HTsc[W5]",
    "HTsd[W5]",
    "HTsc[M]",
    "HTsd[M]",
    "ALL5",
    "LX[IDR,H18s[PT*]]",
    "LX[IDR,H9s25[PT*]]",
    "H18si[PT*]",
    "LX[IDR,C0,PT*]",
];
/// SELt2's fallback: EB.7's shipped review intent order (scored only).
pub const REVIEW_FALLBACK: &str = crate::review::INTENT_FUSION_REVIEW;

/// `find --files`' selection set (item 11).
pub const FIND_SET: &str = "SELf2t";
/// SELf2t's candidates: rank_eval `R3_SELF2`, then `T1s` and `PXT`.
pub const FIND_CANDIDATES: [&str; 11] = [
    "H18x",
    "H9x25T",
    "H9x",
    "H10x",
    "ALL5x",
    "LX[IDR,H18x]",
    "LX[IDR,H9x25T]",
    "H18xi",
    "LX[IDR,C0x,PXT]",
    "T1s",
    "PXT",
];
/// SELf2t's fallback: EB.8's shipped `find --files` order (scored only).
pub const FIND_FALLBACK: &str = crate::find::FIND_FILES_FUSION;
/// The shape SELf2t is calibrated on.
pub const FIND_SHAPES: [Shape; 1] = [Shape::Find];

/// The candidate groups [`specs`] knows, in registration order.
pub const GROUPS: [&str; 3] = ["structure", "review", "find"];

/// A structure-bank ranker by its harness name.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct Named(pub &'static str);

impl Candidate for Named {
    fn name(&self) -> &'static str {
        self.0
    }

    /// The bank's keys; an empty vector (the driver then drops the set) when
    /// no bank is registered or the name is no bank ranker.
    fn rank(&self, q: &QueryCtx<'_>) -> Vec<RankKey> {
        structure_keys(self.0, q).unwrap_or_default()
    }
}

fn seeded(names: &[&'static str]) -> Vec<Box<dyn Candidate>> {
    names
        .iter()
        .map(|&n| Box::new(Seeded(n)) as Box<dyn Candidate>)
        .collect()
}

fn find_seeded(names: &[&'static str]) -> Vec<Box<dyn Candidate>> {
    names
        .iter()
        .map(|&n| Box::new(FindSeeded(n)) as Box<dyn Candidate>)
        .collect()
}

fn named(names: &[&'static str]) -> Vec<Box<dyn Candidate>> {
    names
        .iter()
        .map(|&n| Box::new(Named(n)) as Box<dyn Candidate>)
        .collect()
}

/// The sets of `group`, or None for an unknown group.
fn group(name: &str) -> Option<Vec<SetSpec>> {
    match name {
        "structure" => Some(vec![
            SetSpec::new(
                COCHANGE_SET,
                &SEEDED_SHAPES,
                COCHANGE_FALLBACK,
                named(&COCHANGE_CANDIDATES),
            ),
            SetSpec::new(
                STRUCTURE_SET,
                &SEEDED_SHAPES,
                STRUCTURE_FALLBACK,
                named(&STRUCTURE_CANDIDATES),
            ),
        ]),
        "review" => Some(vec![
            SetSpec::new(
                REVIEW_SET,
                &SEEDED_SHAPES,
                REVIEW_FALLBACK,
                seeded(&REVIEW_CANDIDATES),
            )
            .with_fallback_candidate(Box::new(Seeded(REVIEW_FALLBACK))),
        ]),
        "find" => Some(vec![
            SetSpec::new(
                FIND_SET,
                &FIND_SHAPES,
                FIND_FALLBACK,
                find_seeded(&FIND_CANDIDATES),
            )
            .with_fallback_candidate(Box::new(FindSeeded(FIND_FALLBACK))),
        ]),
        _ => None,
    }
}

/// Register the replay-state parts `groups` read beyond the structure bank:
/// for `review` or `find` (once for both), the text corpus ([`TextState`],
/// also the find queries' seeds through `subject_scores`), the work state
/// ([`WorkState`]) and the id router ([`IdrState`]), in that order.
/// `n_files` is the candidate count.
pub fn register_hooks(
    state: &mut ReplayState,
    replay: &Replay,
    n_files: usize,
    max_files: usize,
    groups: &[&str],
) {
    if groups.contains(&"review") || groups.contains(&"find") {
        state.register(Box::new(TextState::for_replay(replay, n_files)));
        state.register(Box::new(WorkState::for_replay(replay, n_files, max_files)));
        state.register(Box::new(IdrState::for_replay(replay, n_files)));
    }
}

/// The registered parts' once-per-run markers (`[calibrate] text ..`,
/// `[calibrate] work ..`, `[calibrate] idr unavailable ..` when any), after
/// the replay.
pub fn log_hooks(state: &ReplayState) {
    if let Some(t) = state.hook::<TextState>() {
        t.log_summary();
    }
    if let Some(w) = state.hook::<WorkState>() {
        w.log_summary();
    }
    if let Some(r) = state.hook::<IdrState>() {
        r.log_summary();
    }
}

/// The sets of the named groups, in the order the groups are given and each
/// group's registration order; an unknown group is an error naming the known
/// ones.
pub fn specs(groups: &[&str]) -> Result<Vec<SetSpec>, String> {
    let mut out = Vec::new();
    for g in groups {
        match group(g) {
            Some(sets) => out.extend(sets),
            None => {
                return Err(format!(
                    "unknown candidate group `{g}`: expected one of {}",
                    GROUPS.join(", ")
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibrate::record::set_digest;

    #[test]
    fn structure_group_is_selh0_then_sels0() {
        let s = specs(&["structure"]).expect("structure");
        let rows: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = s
            .iter()
            .map(|x| {
                (
                    x.set,
                    x.fallback,
                    x.candidates.iter().map(|c| c.name()).collect(),
                    x.shapes.iter().map(|h| h.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("SELh0", "H10", vec!["H10", "H10[PT*]", "H10[B0]"], vec!["k1", "k2", "k3", "half"]),
                ("SELs0", "P1", vec!["P1", "PT*", "B0"], vec!["k1", "k2", "k3", "half"]),
            ]
        );
        assert!(s.iter().all(|x| x.fallback_candidate.is_none()));
    }

    /// A rename or reorder moves a digest: bump RANKER_REVISION with these.
    #[test]
    fn set_digests_are_pinned() {
        assert_eq!(set_digest(&COCHANGE_CANDIDATES), "3017fcbd3b38fc59");
        assert_eq!(set_digest(&STRUCTURE_CANDIDATES), "b8d25cb5c9150854");
        assert_eq!(set_digest(&REVIEW_CANDIDATES), "ca79a93f8e2d0576");
        assert_eq!(set_digest(&FIND_CANDIDATES), "e682add57de8c8cd");
    }

    /// EC.13: SELf2t is rank_eval's `R3_SELF2` then `T1s`, `PXT` (main's
    /// `f2t`), literally and in order, on the find shape; its fallback EB.8's
    /// scored-only LX[IDR,T1s].
    #[test]
    fn find_group_is_self2t() {
        assert_eq!(
            FIND_CANDIDATES,
            [
                "H18x",
                "H9x25T",
                "H9x",
                "H10x",
                "ALL5x",
                "LX[IDR,H18x]",
                "LX[IDR,H9x25T]",
                "H18xi",
                "LX[IDR,C0x,PXT]",
                "T1s",
                "PXT"
            ]
        );
        assert_eq!((FIND_SET, FIND_FALLBACK), ("SELf2t", "LX[IDR,T1s]"));
        let s = specs(&["find"]).expect("find");
        assert_eq!(s.len(), 1);
        let x = &s[0];
        assert_eq!((x.set, x.fallback), ("SELf2t", "LX[IDR,T1s]"));
        assert_eq!(x.candidates.iter().map(|c| c.name()).collect::<Vec<_>>(), FIND_CANDIDATES);
        assert_eq!(x.shapes, [Shape::Find]);
        assert_eq!(x.fallback_candidate.as_ref().map(|c| c.name()), Some("LX[IDR,T1s]"));
        assert!(!FIND_CANDIDATES.contains(&FIND_FALLBACK));
        for n in FIND_CANDIDATES.iter().chain([&FIND_FALLBACK]) {
            assert!(crate::calibrate::fusions::find_needs(n).is_some(), "{n}");
        }
        assert_eq!(GROUPS, ["structure", "review", "find"]);
    }

    /// EC.12: SELt2 is rank_eval's R1_SEL `SELt2`, literally and in order
    /// (SELt's ten, then the four id-routed fusions), its fallback EB.7's
    /// scored-only LX[IDR,P1].
    #[test]
    fn review_group_is_selt2() {
        assert_eq!(
            REVIEW_CANDIDATES,
            [
                "H18s[PT*]",
                "H9s25[PT*]",
                "H9s50",
                "H10[PT*]",
                "HTsa[W5]",
                "HTsc[W5]",
                "HTsd[W5]",
                "HTsc[M]",
                "HTsd[M]",
                "ALL5",
                "LX[IDR,H18s[PT*]]",
                "LX[IDR,H9s25[PT*]]",
                "H18si[PT*]",
                "LX[IDR,C0,PT*]"
            ]
        );
        assert_eq!(REVIEW_FALLBACK, "LX[IDR,P1]");
        let s = specs(&["review"]).expect("review");
        assert_eq!(s.len(), 1);
        let x = &s[0];
        assert_eq!((x.set, x.fallback), ("SELt2", "LX[IDR,P1]"));
        assert_eq!(x.candidates.iter().map(|c| c.name()).collect::<Vec<_>>(), REVIEW_CANDIDATES);
        assert_eq!(x.shapes, SEEDED_SHAPES);
        assert_eq!(x.fallback_candidate.as_ref().map(|c| c.name()), Some("LX[IDR,P1]"));
        assert!(!REVIEW_CANDIDATES.contains(&REVIEW_FALLBACK));
        for n in REVIEW_CANDIDATES {
            assert!(crate::calibrate::fusions::needs(n).is_some(), "{n}");
        }
        assert_eq!(GROUPS, crate::calibrate::DEFAULT_GROUPS);
    }

    #[test]
    fn unknown_group_names_the_known() {
        assert_eq!(
            specs(&["structure", "nope"]).err(),
            Some("unknown candidate group `nope`: expected one of structure, review, find".to_string())
        );
        assert!(specs(&[]).expect("empty").is_empty());
    }
}
