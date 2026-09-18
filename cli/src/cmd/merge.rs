//! `glia merge` — build one MergedGraph from N repo paths so cross-graph
//! resolvers fire across the boundary; summary + optional JSON dump.

use std::path::Path;

use repo_graph_engine::{generate_many, generate_many_incremental};

use crate::common::{print_json, print_summary_table, write_json_to};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Repo paths to merge (each becomes its own RepoId).
    repos: Vec<String>,
    /// Write a JSON dump to this path. Pass `-` for stdout.
    #[arg(long)]
    out: Option<String>,
    /// Reuse the per-repo incremental parse caches (WP-D): each repo gets
    /// its own `<repo>/.ai/repo-graph/parse_cache.bin`. Off by default for
    /// merges, so a merge writes nothing into the repos it reads.
    #[arg(long)]
    incremental: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repos = args.repos.as_slice();
    let out = args.out.as_deref();
    let incremental = args.incremental;
    if repos.is_empty() {
        eprintln!("error: at least one repo path required");
        return 1;
    }
    let built = if incremental {
        generate_many_incremental(repos)
    } else {
        generate_many(repos)
    };
    let result = match built {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    print_summary_table(&result);
    if let Some(out_path) = out {
        if out_path == "-" {
            print_json(&result.merged);
        } else {
            let path = Path::new(out_path);
            let mut buffer = Vec::new();
            write_json_to(&result.merged, &mut buffer);
            if let Err(e) = std::fs::write(path, buffer) {
                eprintln!("error writing {out_path}: {e}");
                return 4;
            }
            eprintln!("wrote {} bytes to {}", path.metadata().map(|m| m.len()).unwrap_or(0), out_path);
        }
    }
    0
}
