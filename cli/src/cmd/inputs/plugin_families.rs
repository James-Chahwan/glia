//! `glia plugin-families <repo>` (lua-support) — dump a repo's default
//! plugin family declaration, or write it.
//!
//! The declaration lives at `.glia/plugin_families.toml`; the build's
//! runtime discovery reads that file FIRST (a declared family beats an
//! inferred one). A build never creates it (an undeclared repo is inferred
//! from the tree). The default declaration is the seed architectures
//! rendered as `[[plugin_family]]` tables, so a person edits rather than
//! writes from nothing.
//!
//! Default: dump the declaration to stdout. `--write` writes it to
//! `<repo>/.glia/plugin_families.toml`; `--write --force` overwrites an
//! existing declaration.

/// The `glia plugin-families` arguments.
#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// The repo whose default plugin-family declaration to dump or write.
    pub(crate) repo: String,
    /// Write the declaration to `<repo>/.glia/plugin_families.toml`.
    #[arg(long)]
    pub(crate) write: bool,
    /// Overwrite `<repo>/.glia/plugin_families.toml` when writing.
    #[arg(long)]
    pub(crate) force: bool,
}

pub(crate) fn run(a: Args) -> i32 {
    let root = std::path::Path::new(&a.repo);
    if !root.is_dir() {
        eprintln!("error: not a directory: {}", a.repo);
        return 2;
    }
    let text = glia_code_domain::plugin_families::declaration_template();
    if a.force && !a.write {
        eprintln!("error: --force requires --write");
        return 2;
    }
    if !a.write {
        // Dump the default declaration to screen.
        println!("{text}");
        return 0;
    }
    let path = root.join(".glia/plugin_families.toml");
    if path.exists() && !a.force {
        eprintln!(
            "error: {} already exists; pass --write --force to overwrite",
            path.display()
        );
        return 2;
    }
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
