//! `glia analyze` — walk a repo and print a summary of node-kinds +
//! cross-graph edges, the service-graph Mermaid, or the full JSON dump.

use clap::ValueEnum;
use repo_graph_engine::generate_one;

use crate::cmd::arch::{drop_non_flow_links, print_service_mermaid};
use crate::common::{print_json, print_summary_table};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Output format.
    #[arg(long, value_enum, default_value_t = AnalyzeFormat::Summary)]
    format: AnalyzeFormat,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum AnalyzeFormat {
    /// Markdown-style summary table to stdout (default).
    Summary,
    /// Mermaid `graph LR` of cross-stack edges (HTTP, gRPC, queue, etc.).
    Mermaid,
    /// Full JSON dump (nodes + edges).
    Json,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let format = args.format;
    let result = match generate_one(repo) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    // Per-file panic isolation swallows the message; name every failed file
    // on stderr so stdout (summary / json / mermaid) stays unchanged. Same
    // `[parse]` prefix pyo3's `generate` uses.
    if !result.parse_errors.is_empty() {
        eprintln!(
            "[parse] {} file(s) failed to parse",
            result.parse_errors.len()
        );
        for e in &result.parse_errors {
            eprintln!("[parse] error {e}");
        }
    }
    match format {
        AnalyzeFormat::Summary => print_summary_table(&result),
        // A9.4: routed at the A9.2 service map. The old `print_mermaid`
        // partitioned by RepoId and labelled each node `repo <u64 hash>`, so on
        // a single repo — which is all `analyze` ever builds — it rendered ONE
        // hash node and ZERO arrows even with 96 cross-edges in the graph.
        // Retired here rather than left beside `print_service_mermaid`: two
        // mermaid paths is how the dead one survived this long.
        AnalyzeFormat::Mermaid => {
            let mut map = repo_graph_engine::service_map(&result.merged, &result.repo_labels);
            // Same view, same default as `glia arch --mermaid`: flows only.
            // `glia arch --include-shared` is where the rest lives.
            drop_non_flow_links(&mut map);
            print_service_mermaid(&map);
        }
        AnalyzeFormat::Json => print_json(&result.merged),
    }
    0
}
