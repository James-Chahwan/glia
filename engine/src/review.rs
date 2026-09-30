//! The review report (CC.6a, + CC.6b): one `RevDelta` against a git rev ->
//! the changed nodes, their ranked impact, the tests to run, every added /
//! removed edge with its `why` tier, and the working tree's rules checked on
//! both sides (new vs resolved violations); CC.6b adds the markdown PR-report
//! renderer the CLI command and pyo3 `review_vs_rev` share. Public slot,
//! reached by module path (`glia_engine::review::<item>`). Filled by CC.6a.
