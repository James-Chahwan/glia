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
//! [`specs`] returns the named groups' sets in registration order; EC.11 /
//! EC.13 append the `review` / `find` groups. A candidate here is [`Named`]:
//! its keys come from the structure bank ([`super::bank::structure_keys`]).

use super::bank::structure_keys;
use super::candidate::{Candidate, QueryCtx, RankKey};
use super::record::Shape;
use super::select::SetSpec;

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

/// The candidate groups [`specs`] knows, in registration order.
const GROUPS: [&str; 1] = ["structure"];

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
        _ => None,
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
    }

    #[test]
    fn unknown_group_names_the_known() {
        assert_eq!(
            specs(&["structure", "nope"]).err(),
            Some("unknown candidate group `nope`: expected one of structure".to_string())
        );
        assert!(specs(&[]).expect("empty").is_empty());
    }
}
