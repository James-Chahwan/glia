//! `glia build` — walk a repo and write one `.gmap` per per-language
//! sub-graph to `<repo>/.glia/` (or `--out`).

use std::path::Path;

use repo_graph_engine::{generate_one, generate_one_incremental};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Output directory. Defaults to `<repo>/.glia`.
    #[arg(long)]
    out: Option<String>,
    /// Force a full reparse, ignoring the incremental parse cache (WP-D).
    #[arg(long)]
    no_incremental: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let out = args.out.as_deref();
    let incremental = !args.no_incremental;
    let built = if incremental {
        generate_one_incremental(repo)
    } else {
        // An explicit clean build also discards the sidecar — otherwise the
        // next default-on build would reuse the cache the user was escaping.
        if let Err(e) = repo_graph_engine::ParseCache::purge(repo) {
            eprintln!("warning: could not remove parse cache: {e}");
        }
        generate_one(repo)
    };
    let result = match built {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let out_dir = match out {
        Some(p) => Path::new(p).to_path_buf(),
        None => Path::new(repo).join(".glia"),
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error creating {}: {e}", out_dir.display());
        return 4;
    }
    let mut total_bytes: u64 = 0;
    let mut written = 0;
    // A merged graph for a single repo has one RepoGraph per detected
    // language — they all share `g.repo.0`. Number them so they don't
    // collide in the output dir.
    for (i, g) in result.merged.graphs.iter().enumerate() {
        let filename = if result.merged.graphs.len() == 1 {
            format!("repo-{}.gmap", g.repo.0)
        } else {
            format!("repo-{}-{:02}.gmap", g.repo.0, i)
        };
        let path = out_dir.join(filename);
        if let Err(e) = repo_graph_store::write_repo_graph(g, &path) {
            eprintln!("error writing {}: {e}", path.display());
            return 5;
        }
        total_bytes += path.metadata().map(|m| m.len()).unwrap_or(0);
        written += 1;
    }
    eprintln!(
        "wrote {} .gmap file{} ({:.1} KiB) to {}",
        written,
        if written == 1 { "" } else { "s" },
        total_bytes as f64 / 1024.0,
        out_dir.display()
    );
    if !result.parse_errors.is_empty() {
        eprintln!("(plus {} parse errors)", result.parse_errors.len());
    }
    0
}
