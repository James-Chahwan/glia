//! `glia plugin-families <repo>` (lua-support) — create the repo's plugin
//! family declaration `.glia/plugin_families.toml`, the file the build's
//! runtime discovery reads FIRST (a declared family beats an inferred one).
//!
//! The written file is the seed architectures rendered as `[[plugin_family]]`
//! tables, so a person edits rather than writes from nothing. The only
//! writer of the file: a build never creates it (an undeclared repo is
//! inferred from the tree), and `--force` is the explicit overwrite.

/// The `glia plugin-families` arguments.
#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// The repo to write `.glia/plugin_families.toml` in.
    pub(crate) repo: String,
    /// Overwrite an existing declaration.
    #[arg(long)]
    pub(crate) force: bool,
}

pub(crate) fn run(a: Args) -> i32 {
    let root = std::path::Path::new(&a.repo);
    if !root.is_dir() {
        eprintln!("error: not a directory: {}", a.repo);
        return 2;
    }
    let path = root.join(".glia/plugin_families.toml");
    if path.exists() && !a.force {
        eprintln!(
            "error: {} already exists; pass --force to overwrite",
            path.display()
        );
        return 2;
    }
    let text = glia_code_domain::plugin_families::declaration_template();
    if let Err(e) = std::fs::write(&path, text) {
        eprintln!("error: write {}: {e}", path.display());
        return 2;
    }
    println!("{}", path.display());
    println!(
        "note: edit the [[plugin_family]] tables to this repo's plugin architecture; \
         a family declared here is read by the next build, an undeclared one is inferred"
    );
    0
}
