//! Graph traversal over the merged graph (LD.3c): one-hop `neighbours` and
//! the walks `bfs`, `predecessors`, `reachable_by`, `shortest_path`. Each is
//! one call into `MergedGraph`'s LD.3a method plus conversion, so every walk
//! sees every repo's edges AND the cross-repo edges.
//!
//! Shared parameters:
//!
//! - `direction` is `"out"` (follow edges from -> to), `"in"` (to -> from) or
//!   `"both"`; anything else raises ValueError naming the three.
//! - `categories` is a list of edge-category ids (see `category_names()`), or
//!   `None` for every category: DEFINES / CONTAINS / IMPORTS included, the
//!   repo-graph wrapper's all-edge semantics. An id the registry does not hold
//!   raises ValueError.
//! - `depth` bounds the hops. Any depth is accepted: a walk builds one index
//!   over the kept edges and visits each node at most once, so it costs
//!   O(V + E) whatever the depth, and a large depth walks to the fixed point.
//! - A node id the graph does not hold is not an error: it has no edges.
//!
//! Rows are tuples of ints (the pair-shaped convention in `convert.rs`), never
//! dicts: a node id is a Python `int`, exact above `2**63`.
//!
//! `bfs`, `predecessors`, `reachable_by` and `shortest_path` each print LD.3a's
//! one `[traverse] op=<op> walk=<Forward|Backward|Both> seeds=<n> reached=<n>
//! index_nodes=<n>` line to stderr; `neighbours` is one edge scan and prints
//! nothing (the wrapper calls it per node).
//!
//! The bodies live in pyo3-free helpers so `cargo test -p glia-py`
//! covers them (see the crate doc).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_code_domain::edge_category;
use glia_core::{EdgeCategoryId, NodeId};
use glia_graph::{MergedGraph, Reach};

use crate::graph::PyGraph;

/// A one-hop neighbour: (other id, category id, `"out"` | `"in"`).
type NeighbourRow = (u64, u32, &'static str);
/// A reached node: (id, depth, category of the edge that reached it, parent id).
type BfsRow = (u64, usize, u32, u64);
/// A path step: (id, category id of the edge that entered it — `None` for the
/// start). Named because `PyResult<Option<Vec<(u64, Option<u32>)>>>` trips
/// clippy's `type_complexity`; the surface snapshot shows it as `PathStep`.
type PathStep = (u64, Option<u32>);

/// `"out"` / `"in"` / `"both"` -> the walk direction; anything else is an
/// error naming the three.
fn parse_direction(direction: &str) -> Result<Reach, String> {
    match direction {
        "out" => Ok(Reach::Forward),
        "in" => Ok(Reach::Backward),
        "both" => Ok(Reach::Both),
        other => Err(format!(
            "unknown direction '{other}'; valid directions: \"out\", \"in\", \"both\""
        )),
    }
}

/// The inverse of [`parse_direction`]: how a `neighbours` row names the way
/// its edge was walked. `Reach` is `#[non_exhaustive]`, so a variant added
/// later is an error here until it is given a name, never a silent `"out"`.
fn direction_label(reach: Reach) -> Result<&'static str, String> {
    match reach {
        Reach::Forward => Ok("out"),
        Reach::Backward => Ok("in"),
        Reach::Both => Ok("both"),
        other => Err(format!("walk direction {other:?} has no name")),
    }
}

/// Category ids -> the follow filter: `None` follows every category, a list
/// exactly the listed ones. An id the registry does not hold is an error.
fn parse_categories(categories: Option<&[u32]>) -> Result<Option<Vec<EdgeCategoryId>>, String> {
    let Some(ids) = categories else {
        return Ok(None);
    };
    ids.iter()
        .map(|&id| {
            edge_category::ALL
                .iter()
                .find(|(known, _)| known.0 == id)
                .map(|(known, _)| *known)
                .ok_or_else(|| {
                    format!("unknown edge category id {id}; valid ids are those category_names() lists")
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The whole body of [`PyGraph::neighbours`], minus pyo3.
fn neighbour_rows(
    merged: &MergedGraph,
    node_id: u64,
    direction: &str,
    categories: Option<&[u32]>,
) -> Result<Vec<NeighbourRow>, String> {
    let reach = parse_direction(direction)?;
    let follow = parse_categories(categories)?;
    merged
        .neighbours(NodeId(node_id), reach, follow.as_deref())
        .into_iter()
        .map(|(other, category, walked)| Ok((other.0, category.0, direction_label(walked)?)))
        .collect()
}

/// The whole body of [`PyGraph::bfs`], minus pyo3.
fn bfs_rows(
    merged: &MergedGraph,
    node_id: u64,
    direction: &str,
    categories: Option<&[u32]>,
    depth: usize,
) -> Result<Vec<BfsRow>, String> {
    let reach = parse_direction(direction)?;
    let follow = parse_categories(categories)?;
    Ok(merged
        .bfs(&[NodeId(node_id)], reach, follow.as_deref(), depth)
        .into_iter()
        .map(|r| (r.id.0, r.depth, r.via.0, r.parent.0))
        .collect())
}

/// The whole body of [`PyGraph::predecessors`], minus pyo3.
fn predecessor_ids(
    merged: &MergedGraph,
    node_id: u64,
    categories: Option<&[u32]>,
    depth: usize,
) -> Result<Vec<u64>, String> {
    let follow = parse_categories(categories)?;
    Ok(merged
        .predecessors(NodeId(node_id), follow.as_deref(), depth)
        .into_iter()
        .map(|id| id.0)
        .collect())
}

/// The whole body of [`PyGraph::reachable_by`], minus pyo3.
fn reachable_ids(
    merged: &MergedGraph,
    sink_id: u64,
    source_ids: &[u64],
    categories: Option<&[u32]>,
    depth: usize,
) -> Result<Vec<u64>, String> {
    let follow = parse_categories(categories)?;
    let sources: Vec<NodeId> = source_ids.iter().map(|&id| NodeId(id)).collect();
    Ok(merged
        .reachable_by(NodeId(sink_id), &sources, follow.as_deref(), depth)
        .into_iter()
        .map(|id| id.0)
        .collect())
}

/// The whole body of [`PyGraph::shortest_path`], minus pyo3.
fn path_steps(
    merged: &MergedGraph,
    from_id: u64,
    to_id: u64,
    direction: &str,
    categories: Option<&[u32]>,
    depth: usize,
) -> Result<Option<Vec<PathStep>>, String> {
    let reach = parse_direction(direction)?;
    let follow = parse_categories(categories)?;
    Ok(merged
        .shortest_path(NodeId(from_id), NodeId(to_id), reach, follow.as_deref(), depth)
        .map(|path| path.into_iter().map(|(id, via)| (id.0, via.map(|c| c.0))).collect()))
}

#[pymethods]
impl PyGraph {
    /// One-hop neighbours of `node_id` over intra-repo and cross-repo edges, in
    /// edge order: a list of `(other id, category id, "out" | "in")`. `"out"`
    /// is an edge leaving `node_id`, `"in"` one entering it; `direction="both"`
    /// lists both (a self-loop once, as `"out"`). `categories=None` keeps every
    /// category. Prints nothing.
    #[pyo3(signature = (node_id, direction="out", categories=None))]
    fn neighbours(
        &self,
        node_id: u64,
        direction: &str,
        categories: Option<Vec<u32>>,
    ) -> PyResult<Vec<(u64, u32, &'static str)>> {
        neighbour_rows(&self.merged, node_id, direction, categories.as_deref())
            .map_err(PyValueError::new_err)
    }

    /// Breadth-first walk from `node_id` along `direction`, up to `depth`
    /// hops: every reached node in discovery order as `(id, depth, category
    /// id of the edge that first reached it, parent id)`. `node_id` itself is
    /// not in it; `depth=0` reaches nothing.
    #[pyo3(signature = (node_id, direction="out", categories=None, depth=3))]
    fn bfs(
        &self,
        node_id: u64,
        direction: &str,
        categories: Option<Vec<u32>>,
        depth: usize,
    ) -> PyResult<Vec<(u64, usize, u32, u64)>> {
        bfs_rows(&self.merged, node_id, direction, categories.as_deref(), depth)
            .map_err(PyValueError::new_err)
    }

    /// The ids that reach `node_id` within `depth` hops (a backward walk), in
    /// discovery order; `node_id` itself excluded.
    #[pyo3(signature = (node_id, categories=None, depth=3))]
    fn predecessors(
        &self,
        node_id: u64,
        categories: Option<Vec<u32>>,
        depth: usize,
    ) -> PyResult<Vec<u64>> {
        predecessor_ids(&self.merged, node_id, categories.as_deref(), depth)
            .map_err(PyValueError::new_err)
    }

    /// The ids of `source_ids` that reach `sink_id` within `depth` hops, in
    /// `source_ids` order. The sink itself is never a hit; the walk stops as
    /// soon as every source is found.
    #[pyo3(signature = (sink_id, source_ids, categories=None, depth=6))]
    fn reachable_by(
        &self,
        sink_id: u64,
        source_ids: Vec<u64>,
        categories: Option<Vec<u32>>,
        depth: usize,
    ) -> PyResult<Vec<u64>> {
        reachable_ids(&self.merged, sink_id, &source_ids, categories.as_deref(), depth)
            .map_err(PyValueError::new_err)
    }

    /// A shortest path by hop count from `from_id` to `to_id` along
    /// `direction`, as `[(id, category id of the edge that entered it), ...]`
    /// with `None` for `from_id`'s step, or `None` when `to_id` is not reached
    /// within `depth` hops. `from_id == to_id` is `[(from_id, None)]`. Among
    /// equally short paths it is the one edge order meets first.
    #[pyo3(signature = (from_id, to_id, direction="both", categories=None, depth=12))]
    fn shortest_path(
        &self,
        from_id: u64,
        to_id: u64,
        direction: &str,
        categories: Option<Vec<u32>>,
        depth: usize,
    ) -> PyResult<Option<Vec<PathStep>>> {
        path_steps(&self.merged, from_id, to_id, direction, categories.as_deref(), depth)
            .map_err(PyValueError::new_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALLS: u32 = edge_category::CALLS.0;
    const DEFINES: u32 = edge_category::DEFINES.0;

    /// LD.1's 9-line app: `app` DEFINES `helper` and `main`; `main` CALLS
    /// `helper`. Returns the graph and the (app, helper, main) ids.
    fn app() -> (MergedGraph, u64, u64, u64) {
        let root = std::env::temp_dir().join(format!("glia-ld3c-traversal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(
            root.join("app.py"),
            "import os\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n",
        )
        .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;
        let id = |qname: &str| -> u64 {
            merged
                .graphs
                .iter()
                .flat_map(|g| g.nav.qname_by_id.iter())
                .find(|(_, q)| q.as_str() == qname)
                .map(|(id, _)| id.0)
                .unwrap_or_else(|| panic!("{qname} is a node"))
        };
        let (a, h, m) = (id("app"), id("app::helper"), id("app::main"));
        (merged, a, h, m)
    }

    #[test]
    fn directions_round_trip_and_reject_anything_else() {
        for d in ["out", "in", "both"] {
            let reach = parse_direction(d).expect("a valid direction");
            assert_eq!(direction_label(reach), Ok(d));
        }
        let err = parse_direction("sideways").expect_err("not a direction");
        assert!(err.contains("'sideways'") && err.contains("\"out\", \"in\", \"both\""), "{err}");
        assert!(parse_direction("OUT").is_err(), "directions are exact");
    }

    #[test]
    fn categories_are_registry_ids_or_none() {
        assert_eq!(parse_categories(None), Ok(None));
        assert_eq!(parse_categories(Some(&[])), Ok(Some(Vec::new())));
        assert_eq!(
            parse_categories(Some(&[CALLS, DEFINES])),
            Ok(Some(vec![edge_category::CALLS, edge_category::DEFINES]))
        );
        let err = parse_categories(Some(&[CALLS, 999])).expect_err("999 is no category");
        assert!(err.contains("999") && err.contains("category_names()"), "{err}");
    }

    /// LD.3c acceptance, measured on the Rust side of the binding: incoming
    /// edges are visible (HEAD listed outgoing only), every category is kept
    /// by default (so `app`'s DEFINES shows beside `main`'s CALLS), and each
    /// walk is LD.3a's.
    #[test]
    fn traversal_over_the_nine_line_app() {
        let (g, app, helper, main) = app();

        assert_eq!(
            neighbour_rows(&g, helper, "in", None),
            Ok(vec![(app, DEFINES, "in"), (main, CALLS, "in")])
        );
        assert_eq!(neighbour_rows(&g, helper, "in", Some(&[CALLS])), Ok(vec![(main, CALLS, "in")]));
        assert_eq!(neighbour_rows(&g, helper, "out", None), Ok(vec![]));
        assert_eq!(neighbour_rows(&g, main, "out", None), Ok(vec![(helper, CALLS, "out")]));
        assert_eq!(
            neighbour_rows(&g, main, "both", None),
            Ok(vec![(app, DEFINES, "in"), (helper, CALLS, "out")])
        );
        assert!(neighbour_rows(&g, main, "sideways", None).is_err());
        assert!(neighbour_rows(&g, main, "out", Some(&[999])).is_err());

        let walked = bfs_rows(&g, main, "out", None, 3).expect("bfs");
        assert_eq!(walked, vec![(helper, 1, CALLS, main)]);
        assert_eq!(bfs_rows(&g, main, "out", None, 0), Ok(vec![]));
        let up = bfs_rows(&g, helper, "in", None, 3).expect("bfs");
        assert_eq!(up, vec![(app, 1, DEFINES, helper), (main, 1, CALLS, helper)]);

        assert_eq!(predecessor_ids(&g, helper, Some(&[CALLS]), 3), Ok(vec![main]));
        assert_eq!(predecessor_ids(&g, helper, None, 3), Ok(vec![app, main]));

        assert_eq!(reachable_ids(&g, helper, &[main, app], Some(&[CALLS]), 6), Ok(vec![main]));
        assert_eq!(reachable_ids(&g, helper, &[main, app], None, 6), Ok(vec![main, app]));
        assert_eq!(reachable_ids(&g, helper, &[], None, 6), Ok(vec![]));

        assert_eq!(
            path_steps(&g, helper, main, "both", None, 12),
            Ok(Some(vec![(helper, None), (main, Some(CALLS))]))
        );
        assert_eq!(path_steps(&g, helper, main, "out", None, 12), Ok(None));
        assert_eq!(
            path_steps(&g, main, helper, "out", None, 12),
            Ok(Some(vec![(main, None), (helper, Some(CALLS))]))
        );
        assert_eq!(path_steps(&g, main, main, "in", None, 12), Ok(Some(vec![(main, None)])));
        assert!(path_steps(&g, helper, main, "up", None, 12).is_err());
        assert!(predecessor_ids(&g, helper, Some(&[999]), 3).is_err());
    }
}
