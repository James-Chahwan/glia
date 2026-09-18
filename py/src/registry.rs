//! The registry tables (kind / category / cell-type names), the build
//! identity (`version`, `build_stamp`), and the self-registration type every
//! module-function module submits to.

use pyo3::prelude::*;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};

/// One module's contribution to the Python module: `add` puts its
/// `#[pyfunction]`s on `m`. Each module that owns `#[pyfunction]`s ends with
/// `inventory::submit! { ModuleFns { name: "<module>", add: register } }`;
/// `lib.rs` runs every submission sorted by `name`, so a new primitive never
/// edits `lib.rs`.
pub(crate) struct ModuleFns {
    pub(crate) name: &'static str,
    pub(crate) add: fn(&Bound<'_, PyModule>) -> PyResult<()>,
}

inventory::collect!(ModuleFns);

/// Canonical node-kind `id → name` table (WP-I / #3). Lets the wrapper decode
/// `nodes_json` kinds without a hardcoded Python table that goes stale when a
/// kind is added. Returns `[(id, name)]`.
#[pyfunction]
fn kind_names() -> Vec<(u32, String)> {
    node_kind::ALL.iter().map(|(id, n)| (id.0, (*n).to_string())).collect()
}

/// Canonical edge-category `id → name` table (WP-I / #3). Pairs with
/// `edges_json` category ids.
#[pyfunction]
fn category_names() -> Vec<(u32, String)> {
    edge_category::ALL.iter().map(|(id, n)| (id.0, (*n).to_string())).collect()
}

/// Canonical cell-type `id → name` table — labels the structured cells exposed
/// by `node_cells` (WP-J).
#[pyfunction]
fn cell_type_names() -> Vec<(u32, String)> {
    cell_type::ALL.iter().map(|(id, n)| (id.0, (*n).to_string())).collect()
}

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Build identity of THIS wheel: `<release>+p<16 hex>`, where the hex half is a
/// content hash of every graph-shaping source file (repo_graph_stamp). Two
/// wheels with the same `version()` but different `build_stamp()` contain
/// different parsers — which is how you catch a stale `.so` that maturin
/// repackaged without rebuilding. `version()` above stays the bare release:
/// the repo-graph wrapper and bench/substrate-gap/run.py read it.
#[pyfunction]
fn build_stamp() -> &'static str {
    repo_graph_engine::BUILD_STAMP
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(kind_names, m)?)?;
    m.add_function(wrap_pyfunction!(category_names, m)?)?;
    m.add_function(wrap_pyfunction!(cell_type_names, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(build_stamp, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "registry", add: register } }

#[cfg(test)]
mod tests {
    use super::*;

    /// A1.6: `build_stamp()` extends `version()` — same release, plus the
    /// parser hash — so a wrapper that reads `version()` keeps working and a
    /// stale `.so` shows up as a stamp that did not move across a rebuild.
    #[test]
    fn build_stamp_is_the_release_plus_the_parser_stamp() {
        let stamp = build_stamp();
        assert_eq!(stamp, repo_graph_engine::BUILD_STAMP);
        let hex = stamp
            .strip_prefix(version())
            .and_then(|rest| rest.strip_prefix("+p"))
            .unwrap_or_default();
        assert_eq!(hex, repo_graph_engine::PARSER_STAMP, "build_stamp() = {stamp:?}");
        assert_eq!(hex.len(), 16, "build_stamp() = {stamp:?}");
    }
}
