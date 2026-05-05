//! `NodeSummariesPass` — build per-activated-node text summaries for pooling.
//!
//! Mirror of `bin/node_summaries.rs`: same logic, wrapped as a `Pass` so it
//! can compose with seeds + downstream synth passes through `PassContext`
//! instead of intermediate JSON files on disk.
//!
//! - Reads `"seeds"` artifact: `{ activated: Vec<(qname, score)> }`
//! - Writes `"summaries"` artifact: `Vec<SummaryEntry>`

use anyhow::Result;
use repo_graph_code_domain::node_kind;
use repo_graph_core::Node;
use serde::{Deserialize, Serialize};

use crate::driver_utils::{extract_code_cell, node_by_qname_index};
use crate::passes::context::PassContext;
use crate::passes::traits::Pass;

#[derive(Deserialize)]
struct SeedsArtifact {
    activated: Vec<(String, f64)>,
}

#[derive(Serialize, Deserialize)]
pub struct SummaryEntry {
    pub id: u64,
    pub qname: String,
    pub score: f64,
    pub summary: String,
}

pub struct NodeSummariesPass {
    pub max_chars: usize,
}

impl NodeSummariesPass {
    pub fn new(max_chars: usize) -> Self {
        Self { max_chars }
    }
}

impl Default for NodeSummariesPass {
    fn default() -> Self {
        Self { max_chars: 2000 }
    }
}

impl Pass for NodeSummariesPass {
    fn name(&self) -> &str {
        "node_summaries"
    }

    fn inputs(&self) -> &[&'static str] {
        &["seeds"]
    }

    fn outputs(&self) -> &[&'static str] {
        &["summaries"]
    }

    fn run(&self, ctx: &mut PassContext) -> Result<()> {
        let seeds: SeedsArtifact = ctx.get("seeds")?;
        if seeds.activated.is_empty() {
            anyhow::bail!("node_summaries: seeds artifact has no activated entries");
        }

        let qname_to_node = node_by_qname_index(&ctx.graph);
        let mut out: Vec<SummaryEntry> = Vec::new();
        let mut with_source = 0usize;
        let mut without_source = 0usize;
        let mut skipped = 0usize;

        for (qname, score) in &seeds.activated {
            let node = match qname_to_node.get(qname.as_str()) {
                Some(n) => *n,
                None => {
                    eprintln!("[summaries] skip (no node match): {qname}");
                    skipped += 1;
                    continue;
                }
            };
            let summary = build_summary(
                node,
                ctx.graph.nav.kind_by_id.get(&node.id).copied(),
                qname,
                self.max_chars,
            );
            if summary.has_source {
                with_source += 1;
            } else {
                without_source += 1;
            }
            out.push(SummaryEntry {
                id: node.id.0,
                qname: qname.clone(),
                score: *score,
                summary: summary.text,
            });
        }

        eprintln!(
            "[summaries] {} entries ({} with CODE source, {} without, {} skipped)",
            out.len(),
            with_source,
            without_source,
            skipped,
        );

        ctx.put("summaries", &out)?;
        Ok(())
    }
}

struct Summary {
    text: String,
    has_source: bool,
}

fn build_summary(
    node: &Node,
    kind: Option<repo_graph_core::NodeKindId>,
    qname: &str,
    max_chars: usize,
) -> Summary {
    if let Some(source) = extract_code_cell(node) {
        let capped = if max_chars > 0 && source.len() > max_chars {
            format!("{}\n# ... (truncated)", &source[..max_chars])
        } else {
            source.to_string()
        };
        return Summary {
            text: format!("# {qname}\n{capped}"),
            has_source: true,
        };
    }

    let name = qname.rsplit("::").next().unwrap_or("?");
    let fallback = match kind {
        Some(k) if k == node_kind::METHOD => {
            format!("def {name}(self, *args, **kwargs): ...  # {qname}")
        }
        Some(k) if k == node_kind::FUNCTION => {
            format!("def {name}(*args, **kwargs): ...  # {qname}")
        }
        Some(k) if k == node_kind::CLASS => format!("class {name}: ...  # {qname}"),
        Some(k) if k == node_kind::MODULE => format!("import {}", qname.replace("::", ".")),
        _ => format!("# {qname}"),
    };
    Summary {
        text: fallback,
        has_source: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::RepoId;
    use repo_graph_graph::build_python;
    use repo_graph_parser_python::parse_file;

    use crate::passes::Pipeline;

    fn graph_for(source: &str) -> repo_graph_graph::RepoGraph {
        let repo = RepoId::from_canonical("test");
        let fp = parse_file(source, "m.py", "m", repo).unwrap();
        build_python(repo, vec![fp]).unwrap()
    }

    #[test]
    fn pass_emits_summary_per_activated_qname_with_source() {
        let g = graph_for("class Foo:\n    def bar(self):\n        return 1\n");
        let mut ctx = PassContext::new(g, "issue".into());
        ctx.put(
            "seeds",
            &serde_json::json!({
                "activated": [["m::Foo::bar", 0.5_f64], ["m::Foo", 0.3_f64]],
            }),
        )
        .unwrap();

        NodeSummariesPass::default().run(&mut ctx).unwrap();

        let entries: Vec<SummaryEntry> = ctx.get("summaries").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].qname, "m::Foo::bar");
        assert!(entries[0].summary.starts_with("# m::Foo::bar"));
        assert!(entries[0].summary.contains("def bar"));
        assert_eq!(entries[1].qname, "m::Foo");
        assert!(entries[1].summary.contains("class Foo"));
    }

    #[test]
    fn pass_caps_summary_at_max_chars() {
        let body: String = "    x = 1\n".repeat(500);
        let src = format!("def big():\n{body}");
        let g = graph_for(&src);
        let mut ctx = PassContext::new(g, "issue".into());
        ctx.put("seeds", &serde_json::json!({"activated": [["m::big", 0.9_f64]]}))
            .unwrap();

        NodeSummariesPass::new(200).run(&mut ctx).unwrap();
        let entries: Vec<SummaryEntry> = ctx.get("summaries").unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].summary.contains("# ... (truncated)"));
    }

    #[test]
    fn pass_skips_qnames_with_no_node() {
        let g = graph_for("def real(): pass\n");
        let mut ctx = PassContext::new(g, "issue".into());
        ctx.put(
            "seeds",
            &serde_json::json!({
                "activated": [["m::real", 0.5_f64], ["m::ghost", 0.4_f64]],
            }),
        )
        .unwrap();

        NodeSummariesPass::default().run(&mut ctx).unwrap();
        let entries: Vec<SummaryEntry> = ctx.get("summaries").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].qname, "m::real");
    }

    #[test]
    fn pass_errors_when_seeds_artifact_missing() {
        let g = graph_for("pass\n");
        let mut ctx = PassContext::new(g, "issue".into());
        let err = NodeSummariesPass::default().run(&mut ctx).unwrap_err();
        assert!(err.to_string().contains("seeds"));
    }

    #[test]
    fn pipeline_runs_node_summaries_against_pre_staged_seeds() {
        let g = graph_for("def f(): pass\n");
        let mut ctx = PassContext::new(g, "issue".into());
        ctx.put("seeds", &serde_json::json!({"activated": [["m::f", 1.0_f64]]}))
            .unwrap();

        let pipeline = Pipeline::new().add(NodeSummariesPass::default());
        pipeline.run_only(&mut ctx, &["node_summaries"]).unwrap();
        assert!(ctx.has("summaries"));
        let entries: Vec<SummaryEntry> = ctx.get("summaries").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].qname, "m::f");
    }
}
