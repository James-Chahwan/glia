//! `glia effects` (LE.4d) — what the named nodes do to the outside world: the
//! effect sinks downstream of them (DB read / write, queue produce, outbound
//! HTTP / RPC / WS / GraphQL call, event emit), the human surface over the
//! engine's `effects::effects`.
//!
//! One table row per sink: `| class | mode | sink | location | depth | via |
//! downstream |` (`via` is the witness chain from the seed, `downstream` the
//! receivers one flow hop past the sink), plus `| crossed |` with
//! `--cross-service`. A sink whose server is outside the build (CG.4b's ORIGIN
//! `external`, CJ.3) reads `external: `<host>`` in `downstream`. An empty
//! answer prints its LD.8a absence block. `--json` is the engine's whole
//! `Effects` (each row's `external_hosts` included).
//!
//! A report, not a gate: exit 0 on any answer (an empty one included), 2 on a
//! build failure, more than 64 names, or an unknown `--class`.
//!
//! Fired-on marker: the engine's
//! `[effects] seeds=<S> reached=<R> effects=<E> (db=.. queue_produce=.. http_call=.. event_emit=.. other=..) writes=<W> config_seeds=<C>`,
//! and `[effects-external] sinks=<n> hosts=<k>` when a row is external.

use glia_engine::effects::{DEFAULT_MAX_DEPTH, EffectRow, Effects, EffectsArgs, effects};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The nodes whose effects to list: qnames, dotted paths or exact simple
    /// names (at most 64). A config key (`config:env:NAME`) seeds from the
    /// functions that read it.
    #[arg(required = true)]
    qnames: Vec<String>,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Hops of the forward walk.
    #[arg(long, default_value_t = DEFAULT_MAX_DEPTH)]
    depth: usize,
    /// Keep only these effect classes (comma-separated): db, email,
    /// queue_produce, http_call, event_emit, rpc_call, ws_send, graphql_op.
    #[arg(long, value_delimiter = ',')]
    class: Vec<String>,
    /// Keep db reads out: db rows that write, and every send / call.
    #[arg(long)]
    writes_only: bool,
    /// Continue past each queue / HTTP / event / RPC sink into the receiving
    /// handler, and count the services each path crosses.
    #[arg(long)]
    cross_service: bool,
    /// Keep only effects whose sink sits under this path or project label.
    #[arg(long)]
    scope: Option<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// `seed -[CALLS]-> saveOrder -[ACCESSES_DATA]-> sink`, simple names, the
/// config key first when the seed reads one.
fn chain(r: &EffectRow) -> String {
    let tail = |q: &str| q.rsplit("::").next().unwrap_or(q).to_string();
    let mut out = String::new();
    if let Some(key) = &r.via_config {
        out.push_str(&format!("`{key}` -[READS_CONFIG]- "));
    }
    out.push_str(&format!("`{}`", tail(&r.seed)));
    for h in &r.path {
        out.push_str(&format!(" -[{}]-> `{}`", h.category, tail(&h.to_qname)));
    }
    out
}

/// `external: `h1`, `h2`` for a sink outside the build (CJ.3), then the
/// receivers `qname (CATEGORY)`; `—` when there is neither.
fn downstream(r: &EffectRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !r.external_hosts.is_empty() {
        let hosts: Vec<String> = r.external_hosts.iter().map(|h| format!("`{h}`")).collect();
        parts.push(format!("external: {}", hosts.join(", ")));
    }
    parts.extend(
        r.downstream
            .iter()
            .map(|t| format!("`{}` ({})", t.qname, t.category)),
    );
    if parts.is_empty() {
        "—".to_string()
    } else {
        parts.join(", ")
    }
}

fn print_table(a: &Effects, cross_service: bool) {
    let listed: Vec<String> = a.seeds.iter().map(|s| format!("`{s}`")).collect();
    println!(
        "- seeds: {}",
        if listed.is_empty() {
            "none".to_string()
        } else {
            listed.join(", ")
        }
    );
    let counts: Vec<String> = a
        .counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| format!("{c} {n}"))
        .collect();
    println!(
        "- effects: {} ({}), writes {}",
        a.effects.len(),
        if counts.is_empty() {
            "none".to_string()
        } else {
            counts.join(", ")
        },
        a.writes
    );
    if !a.unresolved.is_empty() {
        let names: Vec<String> = a.unresolved.iter().map(|s| format!("`{s}`")).collect();
        println!("- unresolved: {}", names.join(", "));
    }
    println!();
    if a.effects.is_empty() {
        match &a.absence {
            Some(x) => print_absence(x),
            None => println!("_(no effects)_"),
        }
        return;
    }
    if cross_service {
        println!("| class | mode | sink | location | depth | crossed | via | downstream |");
        println!("|---|---|---|---|--:|--:|---|---|");
    } else {
        println!("| class | mode | sink | location | depth | via | downstream |");
        println!("|---|---|---|---|--:|---|---|");
    }
    for r in &a.effects {
        let crossed = if cross_service {
            format!(" {} |", r.services_crossed)
        } else {
            String::new()
        };
        println!(
            "| {} | {} | `{}` | {} | {} |{crossed} {} | {} |",
            r.class,
            r.mode.as_deref().unwrap_or("—"),
            r.qname,
            at(r.file.as_deref(), r.line),
            r.depth,
            chain(r),
            downstream(r)
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
    let mut opts = EffectsArgs::default();
    opts.max_depth = args.depth;
    opts.classes = (!args.class.is_empty()).then(|| args.class.clone());
    opts.writes_only = args.writes_only;
    opts.cross_service = args.cross_service;
    opts.scope = args.scope.clone();
    let names: Vec<&str> = args.qnames.iter().map(String::as_str).collect();
    let mut answer = match effects(&result.merged, &result.repo_labels, &names, &opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia effects `{}`", args.repo);
    println!();
    print_table(&answer, args.cross_service);
    0
}
