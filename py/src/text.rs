//! Text projections of the graph (dense sigil text, prose) and the PPR
//! `activate` that picks the node set a scoped projection renders.

use pyo3::prelude::*;

use glia_core::NodeId;

use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    fn dense_text(&self) -> String {
        glia_projection_text::render_merged(&self.merged)
    }

    /// Same as `dense_text()` but preserves full cell bodies (source code is
    /// not truncated to a one-line preview). Use for LLM context construction
    /// where the model needs the actual function body, not a signature stub.
    fn dense_text_full(&self) -> String {
        glia_projection_text::render_merged_full(&self.merged)
    }

    /// Scoped dense sigil text for just `node_ids` (+ structural glue), not the
    /// whole graph (WP-C / GR-3). `full` keeps untruncated cell bodies. Pass the
    /// top-K from `activate` for a scoped view / `mode=prose` precursor.
    #[pyo3(signature = (node_ids, full=false))]
    fn dense_text_subset(&self, node_ids: Vec<u64>, full: bool) -> String {
        let ids: Vec<NodeId> = node_ids.into_iter().map(NodeId).collect();
        let sub = self.merged.subset(&ids);
        if full {
            glia_projection_text::render_merged_full(&sub)
        } else {
            glia_projection_text::render_merged(&sub)
        }
    }

    /// Prose projection (WP-C / GR-3) of just `node_ids`: one readable block per
    /// node (kind, qname, location, doc/code preview). Backs `mode=prose`.
    fn prose(&self, node_ids: Vec<u64>) -> String {
        let ids: Vec<NodeId> = node_ids.into_iter().map(NodeId).collect();
        let sub = self.merged.subset(&ids);
        glia_projection_text::render_prose(&sub)
    }

    /// Spreading activation (PPR) from `seed_ids`. `profile` (WP-F / GR-5)
    /// selects an edge-weight preset — "default", "repair", "review", or
    /// "onboard" — so the same engine serves different agent tasks. Returns
    /// `(id, score)` pairs, score-sorted, capped at `top_k`.
    #[pyo3(signature = (seed_ids, top_k=None, profile=None))]
    fn activate(
        &self,
        seed_ids: Vec<u64>,
        top_k: Option<usize>,
        profile: Option<String>,
    ) -> Vec<(u64, f64)> {
        let seeds: Vec<NodeId> = seed_ids.into_iter().map(NodeId).collect();
        let mut config =
            glia_code_domain::profile::CODE_TABLES.activation_config(profile.as_deref());
        if let Some(k) = top_k {
            config.top_k = k;
        }
        let result = self.merged.activate(&seeds, &config);
        result
            .scores
            .iter()
            .map(|(id, score)| (id.0, *score))
            .collect()
    }
}
