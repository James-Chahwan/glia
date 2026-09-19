//! `glia cycles` (LE.6b) — the human surface over the engine's
//! `cycles::cycles`: cross-service event / call loops with a located witness,
//! service-level possible loops, then module import cycles, one table per
//! kind. A report, not a gate (LE.8's `check` is the gate): it exits 0
//! whatever it finds; exit 2 is a build failure. `--json` is the row list.

use repo_graph_engine::cycles::{
    CALL_LOOP, CycleArgs, CycleHop, CycleRow, EVENT_LOOP, IMPORT, IMPORT_CYCLE, POSSIBLE_LOOP,
    cycles, kinds_for,
};

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// `event`: cross-service loops (node-level event / call loops, then
    /// service-level possible loops); `import`: module import cycles.
    #[arg(long, default_value = "all", value_parser = ["event", "import", "all"])]
    kind: String,
    /// Keep only cycles whose members all sit under this path or project label.
    #[arg(long)]
    scope: Option<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// `file:line`, `file`, or `—`.
fn at(h: &CycleHop) -> String {
    match (h.file.as_deref(), h.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// `a -[QUEUE_FLOWS orders.placed]-> b -[CALLS]-> a`: the witness as one chain.
fn chain(w: &[CycleHop]) -> String {
    let Some(first) = w.first() else {
        return "—".to_string();
    };
    let mut out = format!("`{}`", first.from_qname);
    for h in w {
        let label = match &h.channel {
            Some(c) => format!("{} {c}", h.category),
            None => h.category.to_string(),
        };
        out.push_str(&format!(" -[{label}]-> `{}`", h.to_qname));
    }
    out
}

fn print_kind(kind: &str, title: &str, rows: &[&CycleRow]) {
    println!("## {title} ({kind}): {}", rows.len());
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| # | tier | services | mechanisms | channels | size | witness | at |");
    println!("|--:|---|---|---|---|--:|---|---|");
    for (i, r) in rows.iter().enumerate() {
        let located: Vec<String> = r.witness.iter().map(at).collect();
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            i + 1,
            r.tier,
            r.services.join(", "),
            r.mechanisms.join(", "),
            if r.channels.is_empty() {
                "—".to_string()
            } else {
                r.channels.join(", ")
            },
            r.size,
            chain(&r.witness),
            located.join("<br>"),
        );
    }
    if let Some(note) = rows.iter().find_map(|r| r.note) {
        println!();
        println!("_{note}_");
    }
    println!();
}

pub(crate) fn run(args: Args) -> i32 {
    let kinds = match kinds_for(&args.kind) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut opts = CycleArgs::default();
    opts.kinds = kinds.clone();
    opts.scope = args.scope.clone();
    let rows = cycles(&result.merged, &result.repo_labels, &opts);
    if args.json {
        println!("{}", serde_json::to_string(&rows).unwrap_or_default());
        return 0;
    }
    println!("# glia cycles `{}`", args.repo);
    println!();
    let of = |k: &str| rows.iter().filter(|r| r.kind == k).collect::<Vec<_>>();
    if kinds.iter().any(|k| k != IMPORT) {
        print_kind(EVENT_LOOP, "cross-service event loops", &of(EVENT_LOOP));
        print_kind(CALL_LOOP, "cross-service call loops", &of(CALL_LOOP));
        print_kind(
            POSSIBLE_LOOP,
            "service-level possible loops",
            &of(POSSIBLE_LOOP),
        );
    }
    if kinds.iter().any(|k| k == IMPORT) {
        print_kind(IMPORT_CYCLE, "import cycles", &of(IMPORT_CYCLE));
    }
    0
}
