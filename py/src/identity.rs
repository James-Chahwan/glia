//! **identity** (DD.15): pyo3 surface for `glia_engine::identity` (DD.8) —
//! `PyGraph.identity_classes`, the `glia identity` report as a native dict
//! (LD.2): every identity class of the graph by rule and tier, each member
//! located and labelled by repo, diverged copies with their variants.
//! Private module slot (declared by D0.7); a `#[pymethods]` block only, no
//! `#[pyfunction]`, so no inventory registration. The `collapse` keyword of
//! the answers that fold through the identity view (`find`, `blast_radius`,
//! `cross_stack_trace`, `tests_for`, `serves`, `service_map`) lives in each
//! answer's own module.
//!
//! The body is the pyo3-free [`identity_query`] and [`identity_report`], so
//! `cargo test -p glia-py` covers it; the engine prints its
//! `[identity] report classes=` fired_on line on every call that answers.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::identity::{IdentityQuery, IdentityReport, identity_classes};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The engine filters of a [`PyGraph::identity_classes`] call. `limit`
/// `None` (or `0`) is every class; the labels are the build's.
fn identity_query(
    tier: Option<String>,
    rule: Option<String>,
    diverged_only: bool,
    limit: Option<usize>,
    repo_labels: &BTreeMap<u64, String>,
) -> IdentityQuery {
    let mut q = IdentityQuery::default();
    q.tier = tier;
    q.rule = rule;
    q.diverged_only = diverged_only;
    q.limit = limit;
    q.repo_labels = repo_labels.clone();
    q
}

/// The whole body of [`PyGraph::identity_classes`] after [`identity_query`],
/// minus pyo3: the engine report, its absence counting the build's unparsed
/// files. `Err` (an unknown tier or rule, naming the valid ones) becomes a
/// `ValueError`. Returns the engine report itself, so `to_py` keeps its
/// field order (`convert.rs`).
fn identity_report(
    merged: &MergedGraph,
    q: &IdentityQuery,
    unparsed_files: usize,
) -> Result<IdentityReport, String> {
    let mut report = identity_classes(merged, q)?;
    if let Some(a) = report.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(report)
}

#[pymethods]
impl PyGraph {
    /// **identity_classes** (DD.8 / DD.15): the identity classes of this
    /// graph — nodes the build says are one thing (a package two repos
    /// depend on, one message, one contract op, one SCIP symbol: tier
    /// `fact` / `derived`, folded to one row by every answer) and same-as
    /// candidates (vendored copies, overlay guesses: tier `heuristic`, never
    /// folded). The collapse classes come first, then the candidates, each
    /// group by (tier, rules, key).
    ///
    /// `tier` keeps one tier (`"fact"`, `"derived"` or `"heuristic"`), `rule`
    /// one identity rule (`package_name`, `manifest_name`, `message_fqname`,
    /// `contract_op`, `contract_route`, `scip_symbol`, `body_hash`,
    /// `path_suffix`); either unknown raises `ValueError` naming the valid
    /// values. `diverged_only` keeps classes whose copies differ. `limit`
    /// cuts the listed classes (`None` or `0` = every class), never the
    /// counts.
    ///
    /// Returns a dict `{counts, classes, omitted, absence}`: `counts` is
    /// `{classes, members, fact, derived, heuristic, diverged, by_rule}`
    /// (`by_rule` a list of `[rule, classes]`, every rule in table order),
    /// each class `{key, tier, rules, collapses, walks, diverged, variants,
    /// members}` with `key` the display member's qname, and each member
    /// `{repo, qname, kind, file, line, variant}`: `repo` the build's label,
    /// `line` 1-based, `variant` the index of its content among the class's
    /// distinct contents. The display member (the row answers show) is
    /// first. `omitted` counts the classes `limit` cut; `absence` is `None`
    /// when any class is listed, else a dict saying why.
    #[pyo3(signature = (tier=None, rule=None, diverged_only=false, limit=None))]
    fn identity_classes(
        &self,
        py: Python<'_>,
        tier: Option<String>,
        rule: Option<String>,
        diverged_only: bool,
        limit: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let q = identity_query(tier, rule, diverged_only, limit, &self.repo_labels);
        let report = identity_report(&self.merged, &q, self.parse_errors.len())
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use glia_engine::{GenerateResult, generate_many};

    fn fixture(name: &str, dirs: &[&str]) -> GenerateResult {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../bench/substrate-gap/fixtures")
            .join(name);
        let paths: Vec<String> = dirs
            .iter()
            .map(|d| root.join(d).to_string_lossy().into_owned())
            .collect();
        generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
    }

    /// DD.15: pyo3 `identity_classes` is the engine report under the same
    /// filters, field for field, labelled by the build.
    #[test]
    fn report_matches_engine() {
        let r = fixture("identity-npm-package", &["web", "admin"]);
        let q = identity_query(None, None, false, None, &r.repo_labels);
        let got = identity_report(&r.merged, &q, 0).expect("valid filters");
        let want = identity_classes(&r.merged, &q).expect("valid filters");
        assert_eq!(
            serde_json::to_string(&got).ok(),
            serde_json::to_string(&want).ok()
        );
        assert_eq!((got.counts.classes, got.counts.fact), (2, 2), "{got:?}");
        assert!(got.absence.is_none());
        let repos: Vec<&str> = got.classes[0]
            .members
            .iter()
            .map(|m| m.repo.as_str())
            .collect();
        assert_eq!(repos, ["web", "admin"], "{:?}", got.classes[0]);
        assert!(
            got.classes
                .iter()
                .all(|c| c.tier == "fact" && c.rules == ["package_name"])
        );
    }

    /// `limit` cuts the classes, not the counts; `0` is every class.
    #[test]
    fn limit_cuts_classes_only() {
        let r = fixture("identity-npm-package", &["web", "admin"]);
        let one = identity_report(
            &r.merged,
            &identity_query(None, None, false, Some(1), &r.repo_labels),
            0,
        )
        .expect("valid filters");
        assert_eq!(
            (one.classes.len(), one.omitted, one.counts.classes),
            (1, 1, 2)
        );
        let all = identity_report(
            &r.merged,
            &identity_query(None, None, false, Some(0), &r.repo_labels),
            0,
        )
        .expect("valid filters");
        assert_eq!((all.classes.len(), all.omitted), (2, 0));
    }

    /// An unknown rule or tier is the engine's `Err`, naming the valid
    /// values (`ValueError` in Python); an empty answer's absence counts the
    /// unparsed files.
    #[test]
    fn errors_and_absence() {
        let r = fixture("identity-npm-package", &["web"]);
        let bad_rule = identity_report(
            &r.merged,
            &identity_query(None, Some("nope".into()), false, None, &r.repo_labels),
            0,
        )
        .expect_err("unknown rule");
        assert!(
            glia_code_domain::same_as::RULES
                .iter()
                .all(|rule| bad_rule.contains(rule)),
            "{bad_rule}"
        );
        let bad_tier = identity_report(
            &r.merged,
            &identity_query(Some("maybe".into()), None, false, None, &r.repo_labels),
            0,
        )
        .expect_err("unknown tier");
        assert!(
            bad_tier.contains("fact, derived or heuristic"),
            "{bad_tier}"
        );

        let empty = identity_report(
            &r.merged,
            &identity_query(None, None, false, None, &r.repo_labels),
            3,
        )
        .expect("valid filters");
        assert!(empty.classes.is_empty());
        let a = empty.absence.expect("an empty report says why");
        assert_eq!(a.unparsed_files, 3);
        assert!(a.note.contains("no SAME_AS edge"), "{}", a.note);
    }
}
