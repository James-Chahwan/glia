//! CLI resolver — CLI invocations → CLI commands by tool name.

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::Edge;

use super::{CrossGraphResolver, build_kind_index, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// CliInvocationResolver — matches CLI invocations → CLI commands
// ============================================================================

pub struct CliInvocationResolver;

impl CrossGraphResolver for CliInvocationResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let command_index = build_kind_index(&merged.graphs, node_kind::CLI_COMMAND, "cli:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CLI_INVOCATION) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(tool) = qname.strip_prefix("cli_invoke:") else { continue };
                if let Some(targets) = command_index.get(tool) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::CLI_INVOKES,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}
