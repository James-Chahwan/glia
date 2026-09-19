//! `glia spec-status` (LE.9b) — the human surface over the engine's
//! `spec_status::spec_status`: per feature, which declared ops are implemented
//! and which are missing, then the routes of governed services that nobody
//! declared, and a totals line. A report, not a gate: it exits 0 whatever it
//! finds. `--json` is the `SpecStatus` object
//! `{rows, by_feature, governed_services, ungoverned_routes}`; `--status`
//! narrows `rows` only.

use glia_engine::Located;
use glia_engine::spec_status::{
    DECLARED_MISSING, IMPLEMENTED, SpecStatusRow, UNDECLARED, spec_status,
};

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Only this feature's ops (drops the feature-less undeclared rows).
    #[arg(long)]
    feature: Option<String>,
    /// Only rows with this status.
    #[arg(long, value_parser = [IMPLEMENTED, DECLARED_MISSING, UNDECLARED])]
    status: Option<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// `file:line`, `file`, or `—`.
fn at(l: Option<&Located>) -> String {
    match l.map(|l| (l.file.as_deref(), l.line)) {
        Some((Some(f), Some(n))) => format!("{f}:{n}"),
        Some((Some(f), None)) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// The `route / handler` cell: where the route is registered, then its
/// handler by name and place.
fn route_cell(r: &SpecStatusRow) -> String {
    let route = r.route.as_ref().map(|l| at(Some(l)));
    let handler = r
        .handler
        .as_ref()
        .map(|h| format!("`{}` {}", h.name, at(Some(h))));
    match (route, handler) {
        (Some(route), Some(handler)) => format!("{route} / {handler}"),
        (Some(route), None) => route,
        (None, Some(handler)) => handler,
        (None, None) => "—".to_string(),
    }
}

fn status_cell(r: &SpecStatusRow) -> String {
    match (r.pairing, r.confidence) {
        (Some(p), Some(c)) => format!("{} ({p}, {c})", r.status),
        (None, Some(c)) => format!("{} ({c})", r.status),
        _ => r.status.to_string(),
    }
}

fn print_table(rows: &[&SpecStatusRow]) {
    println!("| status | method | path | route / handler | declared at |");
    println!("|---|---|---|---|---|");
    for r in rows {
        println!(
            "| {} | {} | `{}` | {} | {} |",
            status_cell(r),
            r.method,
            r.path,
            route_cell(r),
            at(r.decl.as_ref())
        );
    }
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    // Declarations, pairing, governance and the sort order all live in the
    // engine (which prints the `[sdd] spec_status` marker); this is transport
    // + rendering only.
    let mut status = spec_status(&result.merged, &result.repo_labels, args.feature.as_deref());
    let totals = status.summary();
    if let Some(only) = args.status.as_deref() {
        status.rows.retain(|r| r.status == only);
    }
    if args.json {
        println!("{}", serde_json::to_string(&status).unwrap_or_default());
        return 0;
    }

    println!("# glia spec-status `{}`", args.repo);
    let mut features: Vec<&str> = Vec::new();
    for r in &status.rows {
        if let Some(f) = r.feature.as_deref()
            && features.last() != Some(&f)
        {
            features.push(f);
        }
    }
    for f in features {
        let rows: Vec<&SpecStatusRow> = status
            .rows
            .iter()
            .filter(|r| r.feature.as_deref() == Some(f))
            .collect();
        println!();
        match status.by_feature.get(f) {
            Some(t) => println!(
                "## feature `{f}` — {} declared, {} implemented, {} missing",
                t.declared, t.implemented, t.declared_missing
            ),
            None => println!("## feature `{f}`"),
        }
        println!();
        print_table(&rows);
    }
    let undeclared: Vec<&SpecStatusRow> =
        status.rows.iter().filter(|r| r.feature.is_none()).collect();
    if !undeclared.is_empty() {
        println!();
        println!(
            "## undeclared — routes of governed services ({}) no declared op documents",
            status.governed_services.join(", ")
        );
        println!();
        print_table(&undeclared);
    }
    if let (true, Some(f)) = (status.by_feature.is_empty(), args.feature.as_deref()) {
        println!();
        println!("_no declared op names feature `{f}`._");
    } else if status.by_feature.is_empty() {
        println!();
        println!(
            "_no declared contract ops (OpenAPI / feature.yaml): nothing is governed, so no \
             route is reported undeclared._"
        );
    }
    println!();
    println!("totals: {totals}");
    if status.ungoverned_routes > 0 {
        println!(
            "_{} route(s) sit in services that implement no declared op; counted, not listed._",
            status.ungoverned_routes
        );
    }
    0
}
