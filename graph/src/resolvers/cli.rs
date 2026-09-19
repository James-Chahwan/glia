//! CLI resolver — CLI invocations → CLI commands, by binary and by subcommand.

use std::collections::HashSet;

use glia_code_domain::{edge_category, node_kind};
use glia_code_extractors::cli::parse_argv_cell;
use glia_core::{Confidence, Edge, NodeId};

use super::{CrossGraphResolver, build_kind_index, weakest};
use crate::merged::MergedGraph;

/// Most argv vectors per invocation node the subcommand pass reads. A binary
/// called with dozens of argument shapes is a wrapper script, and every extra
/// bare-word lookup is another chance to mis-pair.
const MAX_SUBCOMMAND_ARGVS: usize = 8;

// ============================================================================
// CliInvocationResolver — matches CLI invocations → CLI commands
// ============================================================================

/// A13.4 — two lookups per `cli_invoke:<bin>` node against the `cli:<name>`
/// declarations:
///
/// 1. **bin** — `<bin>` itself, exact (`mytool` -> `cli:mytool`, a cobra root).
///    Confidence `weakest(invocation, command)`.
/// 2. **sub** — the subcommand word of each argv vector on the node's argv
///    cells (`mytool migrate --yes` -> `cli:migrate`, `php bin/console
///    app:sync-orders` -> `cli:app:sync-orders`). A bare word like
///    `migrate` is common, so this match is `Weak`, capped at
///    [`MAX_SUBCOMMAND_ARGVS`] vectors, and never promoted.
///
/// Edges are deduped on `(from, to)` — a pair both lookups find keeps the bin
/// confidence — and emitted sorted, so the output does not depend on graph order.
pub struct CliInvocationResolver;

impl CrossGraphResolver for CliInvocationResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let command_index = build_kind_index(&merged.graphs, node_kind::CLI_COMMAND, "cli:");
        let commands: usize = command_index.values().map(Vec::len).sum();
        let mut invocations = 0usize;
        let (mut bin, mut sub) = (0usize, 0usize);
        let mut paired: HashSet<(NodeId, NodeId)> = HashSet::new();
        let mut edges: Vec<Edge> = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CLI_INVOCATION) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(tool) = qname.strip_prefix("cli_invoke:") else { continue };
                invocations += 1;
                for t in command_index.get(tool).into_iter().flatten() {
                    if paired.insert((n.id, t.id)) {
                        edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::CLI_INVOKES,
                            confidence: weakest(n.confidence, t.confidence),
                            cells: Vec::new(),
                        });
                        bin += 1;
                    }
                }
                let argvs: Vec<Vec<String>> = n
                    .cells
                    .iter()
                    .flat_map(parse_argv_cell)
                    .take(MAX_SUBCOMMAND_ARGVS)
                    .collect();
                for word in argvs.iter().filter_map(|argv| subcommand_word(argv)) {
                    for t in command_index.get(word).into_iter().flatten() {
                        if paired.insert((n.id, t.id)) {
                            edges.push(Edge {
                                from: n.id,
                                to: t.id,
                                category: edge_category::CLI_INVOKES,
                                confidence: Confidence::Weak,
                                cells: Vec::new(),
                            });
                            sub += 1;
                        }
                    }
                }
            }
        }
        edges.sort_by_key(|e| (e.from.0, e.to.0));
        let pairs = edges.len();
        merged.cross_edges.extend(edges);
        // One line per BUILD, and only when there was anything to pair — the
        // `[queues]` house style, so builds with no CLI surface stay silent.
        if commands > 0 || invocations > 0 {
            eprintln!(
                "[cli] commands={commands} invocations={invocations} paired={pairs} (bin={bin} sub={sub})"
            );
        }
    }
}

/// The subcommand of one argument vector: the first token that is not a flag
/// and is shaped like a command word (`[a-z0-9][a-z0-9_:-]*`), so `-f`, `.`,
/// `x.yaml` and `/path` are passed over. LA.20b: `:` belongs to the word, since
/// Symfony and Laravel name their commands `app:sync-orders` / `emails:send`
/// (`php bin/console app:sync-orders` passes over `bin/console` and pairs with
/// `cli:app:sync-orders`).
fn subcommand_word(argv: &[String]) -> Option<&str> {
    argv.iter().map(String::as_str).find(|t| {
        let mut chars = t.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && chars.all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | ':')
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn subcommand_word_skips_flags_and_paths() {
        assert_eq!(subcommand_word(&argv(&["migrate", "--yes"])), Some("migrate"));
        assert_eq!(subcommand_word(&argv(&["--config", "cfg.yaml", "db-sync"])), Some("db-sync"));
        assert_eq!(subcommand_word(&argv(&["-f", "/tmp/x", "."])), None);
        assert_eq!(subcommand_word(&argv(&["Build"])), None);
        assert_eq!(subcommand_word(&[]), None);
    }

    /// LA.20b: `php bin/console app:sync-orders` passes over the script path
    /// and pairs, Weak, with the Symfony command `cli:app:sync-orders`.
    #[test]
    fn subcommand_token_may_contain_colon() {
        use glia_code_domain::{CodeNav, GRAPH_TYPE};
        use glia_code_extractors::cli::argv_cell;
        use glia_core::{Node, RepoId};

        use crate::types::{RepoGraph, SymbolTable};

        let repo = RepoId::from_canonical("test://cli/colon");
        let mut nav = CodeNav::default();
        let inv = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLI_INVOCATION, "cli_invoke:php");
        nav.record(inv, "php", "cli_invoke:php", node_kind::CLI_INVOCATION, None);
        let cmd = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLI_COMMAND, "cli:app:sync-orders");
        nav.record(cmd, "app:sync-orders", "cli:app:sync-orders", node_kind::CLI_COMMAND, None);
        let argvs = [argv(&["bin/console", "app:sync-orders"])];
        let graph = RepoGraph {
            repo,
            nodes: vec![
                Node { id: inv, repo, confidence: Confidence::Medium, cells: vec![argv_cell("php", &argvs)] },
                Node { id: cmd, repo, confidence: Confidence::Strong, cells: vec![] },
            ],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        let mut merged = MergedGraph::new(vec![graph]);
        CliInvocationResolver.resolve(&mut merged);
        let edges: Vec<(NodeId, NodeId, Confidence)> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::CLI_INVOKES)
            .map(|e| (e.from, e.to, e.confidence))
            .collect();
        assert_eq!(edges, vec![(inv, cmd, Confidence::Weak)]);
        assert_eq!(subcommand_word(&argv(&["bin/console", "app:sync-orders"])), Some("app:sync-orders"));
        assert_eq!(subcommand_word(&argv(&["emails:send", "--queue"])), Some("emails:send"));
    }
}
