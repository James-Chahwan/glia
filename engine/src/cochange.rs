//! Co-change suggestions, ROSE style (CC.11a, + CC.11b): for a set of changed
//! files, the files that usually change with them, with directional pairwise
//! confidence from CO_CHANGES and module churn, the support, and whether any
//! static link joins them; CC.11b adds multi-antecedent rules mined from the
//! history snapshot's commits and the working tree's changed set against a
//! git rev. Public slot, reached by module path
//! (`glia_engine::cochange::<item>`). Filled by CC.11a.
