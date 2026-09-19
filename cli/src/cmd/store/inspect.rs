//! `glia inspect` (LC.4) — the human surface over the store's `inspect_path`:
//! what a `.gmap` file or a layout directory holds, every id named from the
//! file's own header registries (no code-domain table is consulted, so a file
//! from a newer build or another domain is still labelled). `--json` is the
//! `Inspection` object `{path, manifest_schema, build_stamp, shards, totals}`.
//! Exits 1 with the store's error (an old-format file prints its "rebuild the
//! graph" reason) when a file cannot be read.

use std::path::Path;

use glia_store::{Inspection, NamedCount, inspect_path};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// A layout directory (holding manifest.json) or a single .gmap file.
    path: String,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let ins = match inspect_path(Path::new(&args.path)) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {}: {e}", args.path);
            return 1;
        }
    };
    eprintln!("{}", ins.marker());
    if args.json {
        return match serde_json::to_string(&ins) {
            Ok(s) => {
                println!("{s}");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        };
    }
    print_tables(&ins);
    0
}

fn print_tables(ins: &Inspection) {
    println!("# glia inspect `{}`", ins.path);
    println!();
    match (ins.manifest_schema, &ins.build_stamp) {
        (Some(schema), Some(stamp)) => println!("manifest schema {schema}, build `{stamp}`"),
        (Some(schema), None) => println!("manifest schema {schema}, no build stamp"),
        (None, _) => println!("single file"),
    }
    println!();
    println!("## Shards");
    println!();
    println!("| shard | graph_type | format | nodes | edges | sections |");
    println!("|---|---|---|---|---|---|");
    for s in &ins.shards {
        let sections = if s.sections.is_empty() {
            "—".to_string()
        } else {
            s.sections.iter().map(|(n, len)| format!("{n}:{len}B")).collect::<Vec<_>>().join(", ")
        };
        println!(
            "| {} | {} | {} | {} | {} | {sections} |",
            s.name, s.graph_type, s.format, s.nodes, s.edges
        );
    }
    let t = &ins.totals;
    println!();
    println!("{} nodes, {} edges in {} shard(s)", t.nodes, t.edges, ins.shards.len());
    table("Node kinds", &t.kinds);
    table("Edge categories", &t.categories);
    table("Node cells", &t.node_cells);
    table("Edge cells", &t.edge_cells);
    if t.unregistered > 0 {
        println!();
        println!(
            "{} id(s) no shard's header names (shown as `#<id>`): written by a domain or \
             build that did not register them.",
            t.unregistered
        );
    }
}

fn table(title: &str, rows: &[NamedCount]) {
    println!();
    println!("## {title}");
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        return;
    }
    println!("| id | name | count |");
    println!("|---|---|---|");
    for r in rows {
        println!("| {} | {} | {} |", r.id, r.name, r.count);
    }
}
