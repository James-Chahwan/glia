//! `glia ids ls|next` (HT.2), hidden: the id blocks (`glia_core::ids`) and
//! the ids of every domain this binary links, read-only.
//!
//! `ls` prints the blocks, then each linked domain's three registries (node
//! kinds, edge categories, cell types) with each id's block and the next free
//! id per table. `next --domain <graph_type> --table <table>` prints the next
//! free id of that table in the domain's own block (or, with `--private`, in
//! the first private block `0x8000`) as a ready Rust const line, the name left
//! for the author: the allocator writes no source, so rustfmt and review see
//! an ordinary hand-written constant, and HT.1's `core/ids.lock` test still
//! guards the allocation. The rule mirrors that test's `next_free`: one past
//! the highest position held in the block, a code id in decimal (it is its own
//! position), any other block's through `glia_core::ids::id_in`.
//!
//! Exits 0; 2 on an unknown domain or table, or a full block.
//! fired_on (stderr, once per call): `[ids] ls domains=<d> blocks=<b>
//! surface=cli` / `[ids] next domain=<g> table=<t> id=0x<hex> block=<name>`.

use glia_activation::profile::DomainTables;
use glia_code_domain::profile::CODE_TABLES;
use glia_core::ids::{self, Block};

/// Every domain this binary links, by graph type. A domain crate the CLI
/// links later (the build-target domain) adds its `DomainTables` here; the
/// list is the CLI's own so the engine facade stays untouched.
const LINKED: &[&DomainTables] = &[&CODE_TABLES];

/// The three registries: the table key (`--table`, the lock's key) and the
/// Rust id type a const of that table has.
const TABLES: [(&str, &str); 3] = [
    ("node_kind", "NodeKindId"),
    ("edge_category", "EdgeCategoryId"),
    ("cell_type", "CellTypeId"),
];

/// The first private block, where `next --private` numbers.
const PRIVATE_FIRST: u16 = 0x8000;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: IdsCmd,
}

#[derive(clap::Subcommand, Debug)]
enum IdsCmd {
    /// The id blocks, then each linked domain's ids per table with their
    /// block and the next free id.
    Ls(LsArgs),
    /// The next free id of one (domain, table), as a Rust const line.
    Next(NextArgs),
}

#[derive(clap::Args, Debug)]
struct LsArgs {
    /// Only this domain (its graph type, e.g. `code`).
    #[arg(long)]
    domain: Option<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug)]
struct NextArgs {
    /// The domain's graph type (e.g. `code`).
    #[arg(long)]
    domain: String,
    /// `node_kind`, `edge_category` or `cell_type`.
    #[arg(long)]
    table: String,
    /// The next id in the private range (block 0x8000) instead of the
    /// domain's own block: for a branch or prototype, never a shipped table.
    #[arg(long)]
    private: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        IdsCmd::Ls(a) => ls(a),
        IdsCmd::Next(a) => next(a),
    }
}

/// `(id, name)` of one table of `d`, sorted by id.
fn table_ids(d: &DomainTables, table: &str) -> Vec<(u32, &'static str)> {
    let r = &d.registries;
    let mut out: Vec<(u32, &'static str)> = match table {
        "node_kind" => r.node_kinds.iter().map(|(i, n)| (i.0, *n)).collect(),
        "edge_category" => r.edge_categories.iter().map(|(i, n)| (i.0, *n)).collect(),
        _ => r.cell_types.iter().map(|(i, n)| (i.0, *n)).collect(),
    };
    out.sort_unstable();
    out
}

fn domain(graph_type: &str) -> Option<&'static DomainTables> {
    LINKED.iter().copied().find(|d| d.graph_type == graph_type)
}

fn known_domains() -> String {
    LINKED
        .iter()
        .map(|d| d.graph_type)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The registered block `d` allocates in, if any.
fn own_block(d: &DomainTables) -> Option<&'static Block> {
    ids::BLOCKS
        .iter()
        .find(|b| b.status == ids::BlockStatus::Registered && b.graph_types.contains(&d.graph_type))
}

/// The next free id of `table` in `block`: one past the highest position any
/// linked domain holds there (blocks are owned, so in practice the owner's),
/// or `Err` when the block is full.
fn next_in(block: u16, table: &str) -> Result<u32, String> {
    let top = LINKED
        .iter()
        .flat_map(|d| table_ids(d, table))
        .filter(|(id, _)| ids::block_of(*id) == block)
        .map(|(id, _)| ids::position_of(id))
        .max()
        .unwrap_or(0);
    match top.checked_add(1) {
        Some(n) => Ok(ids::id_in(block, n)),
        None => Err(format!("block 0x{block:04X} is full for {table}")),
    }
}

/// How the next id reads in a table: decimal in the code block, hex
/// elsewhere (HT.1's `next=` rule).
fn shown(id: u32) -> String {
    if ids::block_of(id) == ids::CODE {
        id.to_string()
    } else {
        format!("0x{id:08x}")
    }
}

/// The `id_in` argument naming `block`: its `glia_core::ids` constant
/// (`BUILD`) when a [`ids::BLOCKS`] row names it, else the hex number.
fn block_expr(block: u16) -> String {
    match ids::BLOCKS.iter().find(|b| b.number == block) {
        Some(b) => b.name.to_uppercase(),
        None => format!("0x{block:04X}"),
    }
}

/// `pub const NAME: <Ty> = <Ty>(<value>);` for `id`, the code block's ids as
/// the plain number they always were.
fn const_line(ty: &str, id: u32) -> String {
    let block = ids::block_of(id);
    let value = if block == ids::CODE {
        id.to_string()
    } else {
        format!(
            "glia_core::ids::id_in({}, {})",
            block_expr(block),
            ids::position_of(id)
        )
    };
    format!("pub const NAME: {ty} = {ty}({value});")
}

fn block_rows() -> Vec<(String, &'static Block)> {
    let mut rows: Vec<(String, &'static Block)> = ids::BLOCKS
        .iter()
        .map(|b| (format!("0x{:04X}", b.number), b))
        .collect();
    if let Some(p) = ids::block(PRIVATE_FIRST) {
        let (lo, hi) = (ids::PRIVATE.start(), ids::PRIVATE.end());
        rows.push((format!("0x{lo:04X}-0x{hi:04X}"), p));
    }
    rows
}

fn ls(a: LsArgs) -> i32 {
    let domains: Vec<&DomainTables> = match &a.domain {
        Some(g) => match domain(g) {
            Some(d) => vec![d],
            None => {
                eprintln!("error: unknown domain `{g}` (linked: {})", known_domains());
                return 2;
            }
        },
        None => LINKED.to_vec(),
    };
    let blocks = block_rows();
    eprintln!(
        "[ids] ls domains={} blocks={} surface=cli",
        domains.len(),
        blocks.len()
    );
    if a.json {
        return print_ls_json(&blocks, &domains);
    }
    println!("# glia id blocks");
    println!();
    println!("| block | name | status | graph types | since |");
    println!("|---|---|---|---|---|");
    for (number, b) in &blocks {
        let owners = if b.graph_types.is_empty() {
            "—".to_string()
        } else {
            b.graph_types.join(", ")
        };
        println!(
            "| {number} | {} | {} | {owners} | {} |",
            b.name,
            b.status.as_str(),
            b.since
        );
    }
    for d in &domains {
        let home = own_block(d).map_or("none", |b| b.name);
        println!();
        println!("## {} (block {home})", d.graph_type);
        for (table, _) in TABLES {
            let rows = table_ids(d, table);
            let next = match own_block(d).map(|b| next_in(b.number, table)) {
                Some(Ok(id)) => shown(id),
                Some(Err(why)) => why,
                None => "—".to_string(),
            };
            println!();
            println!("### {table}: {} ids, next {next}", rows.len());
            println!();
            println!("| id | hex | block | name |");
            println!("|---|---|---|---|");
            for (id, name) in rows {
                println!("| {id} | 0x{id:08x} | {} | {name} |", ids::block_name(id));
            }
        }
    }
    0
}

fn print_ls_json(blocks: &[(String, &'static Block)], domains: &[&DomainTables]) -> i32 {
    let blocks: Vec<serde_json::Value> = blocks
        .iter()
        .map(|(number, b)| {
            serde_json::json!({
                "block": number,
                "name": b.name,
                "status": b.status.as_str(),
                "graph_types": b.graph_types,
                "since": b.since,
            })
        })
        .collect();
    let domains: Vec<serde_json::Value> = domains
        .iter()
        .map(|d| {
            let tables: Vec<serde_json::Value> = TABLES
                .iter()
                .map(|(table, ty)| {
                    let rows = table_ids(d, table);
                    let next = own_block(d).and_then(|b| next_in(b.number, table).ok());
                    let ids: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|(id, name)| {
                            serde_json::json!({
                                "id": id,
                                "hex": format!("0x{id:08x}"),
                                "block": ids::block_name(*id),
                                "name": name,
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "table": table,
                        "type": ty,
                        "count": rows.len(),
                        "next": next,
                        "next_hex": next.map(|n| format!("0x{n:08x}")),
                        "ids": ids,
                    })
                })
                .collect();
            serde_json::json!({
                "graph_type": d.graph_type,
                "block": own_block(d).map(|b| b.name),
                "tables": tables,
            })
        })
        .collect();
    let out = serde_json::json!({ "blocks": blocks, "domains": domains });
    match serde_json::to_string(&out) {
        Ok(s) => {
            println!("{s}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn next(a: NextArgs) -> i32 {
    let Some(d) = domain(&a.domain) else {
        eprintln!(
            "error: unknown domain `{}` (linked: {})",
            a.domain,
            known_domains()
        );
        return 2;
    };
    let Some((table, ty)) = TABLES.iter().find(|(t, _)| *t == a.table) else {
        eprintln!(
            "error: unknown table `{}` (node_kind, edge_category or cell_type)",
            a.table
        );
        return 2;
    };
    let block = if a.private {
        PRIVATE_FIRST
    } else {
        match own_block(d) {
            Some(b) => b.number,
            None => {
                eprintln!(
                    "error: domain `{}` owns no registered id block (core/src/ids.rs BLOCKS); \
                     use --private",
                    d.graph_type
                );
                return 2;
            }
        }
    };
    let id = match next_in(block, table) {
        Ok(id) => id,
        Err(why) => {
            eprintln!("error: {why}");
            return 2;
        }
    };
    eprintln!(
        "[ids] next domain={} table={table} id=0x{id:08x} block={}",
        d.graph_type,
        ids::block_name(id)
    );
    println!(
        "{} // 0x{id:08x}, block {}",
        const_line(ty, id),
        ids::block_name(id)
    );
    0
}
