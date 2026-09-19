//! `glia projects` (A8.6) — the manifest-rooted sub-projects in the repo;
//! the vocabulary for `--scope`.

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in. Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let with = args.with.as_slice();
    let json = args.json;
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let roots = glia_engine::project_roots(&result.merged);
    eprintln!("[projects] surface=cli roots={}", roots.len());
    if json {
        println!("{}", serde_json::to_string(&roots).unwrap_or_default());
        return 0;
    }
    println!("# glia projects `{repo}`");
    println!();
    if roots.is_empty() {
        println!("_(no manifest-rooted projects)_");
        return 0;
    }
    println!("_Pass a label or a path as `--scope` — both give the same answer._");
    println!();
    println!("| label | ecosystem | path | manifest |");
    println!("|---|---|---|---|");
    for p in &roots {
        println!(
            "| `{}` | {} | `{}` | `{}` |",
            p.label, p.ecosystem, p.path, p.manifest
        );
    }
    0
}
