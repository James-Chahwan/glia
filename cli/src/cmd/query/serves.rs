//! `glia serves` (LD.8b) — who serves a channel: the human surface over the
//! engine's `serves::serves`. `--json` is the LD.8a envelope
//! `{results, absence}`; an empty table prints the absence block (the FACT,
//! the caveat rows, the near misses) under `_(nothing serves it)_`. A client
//! ENDPOINT whose server is outside the build (CJ.3) is an `external (<confidence>)`
//! row after the routes, its hosts in the handlers column as
//! `external: `<host>``. An absence is an answer, so it exits 0; exit 2 is a
//! build failure.
//!
//! DD.13: identity classes collapse (`serves::serves_with`, the build's repo
//! labels). A handler that is one FACT / DERIVED class with code elsewhere
//! (two repos running one SCIP-identical handler) appends `(also in: <repo>
//! <file>:<line>, ...)` to its entry in the handlers cell (`; diverged` when
//! the members' contents differ), and a server row standing for a class of
//! servers appends the same to its location cell; `--json` carries the
//! row's `identity` and `handler_identity`. `--no-collapse` is the raw rows.

use glia_engine::identity::{Collapse, RowIdentity};
use glia_engine::serves::{Server, serves_with};

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The channel: `METHOD /path` (`POST /orders`), a bare `/path` (every
    /// verb), or a queue topic (`orders.created`).
    channel: String,
    /// How to read the channel: `auto` takes a leading HTTP verb or `/` as
    /// HTTP and anything else as a queue topic.
    #[arg(long, default_value = "auto", value_parser = ["auto", "http", "queue"])]
    mechanism: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// One row per server and handler, as built. By default a handler (or a
    /// server) that is one identity class with code elsewhere (a FACT /
    /// DERIVED SAME_AS class) is shown once, naming the other places
    /// (`also in`).
    #[arg(long)]
    no_collapse: bool,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// `file:line`, `file`, or `None`.
fn place(file: Option<&str>, line: Option<i64>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    }
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    place(file, line).unwrap_or_else(|| "—".to_string())
}

/// `(also in: <repo> <file>:<line>, ...; diverged)`, the `glia find` /
/// `glia trace` form. An unlocated member is its repo, and its qname when
/// that differs from the row's.
fn also_in(id: &RowIdentity, qname: &str) -> String {
    let others: Vec<String> = id
        .also_in
        .iter()
        .map(|m| match place(m.file.as_deref(), m.line) {
            Some(p) => format!("{} {p}", m.repo),
            None if m.qname != qname => format!("{} `{}`", m.repo, m.qname),
            None => m.repo.clone(),
        })
        .collect();
    let diverged = if id.diverged { "; diverged" } else { "" };
    format!("(also in: {}{diverged})", others.join(", "))
}

/// The handlers cell: each handler `` `qname` file:line ``, one per line,
/// a handler standing for an identity class followed by its `(also in: ...)`.
fn handlers_cell(s: &Server) -> String {
    if !s.external_hosts.is_empty() {
        let hosts: Vec<String> = s.external_hosts.iter().map(|h| format!("`{h}`")).collect();
        return format!("external: {}", hosts.join(", "));
    }
    if s.handlers.is_empty() {
        return "—".to_string();
    }
    // `handler_identity` is in handler order, one entry per tagged handler.
    let mut tagged = s.handler_identity.iter().peekable();
    s.handlers
        .iter()
        .map(|h| {
            let entry = format!("`{}` {}", h.qname, at(h.file.as_deref(), h.line));
            match tagged.next_if(|t| t.qname == h.qname) {
                Some(t) => format!("{entry} {}", also_in(&t.identity, &h.qname)),
                None => entry,
            }
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

/// The location cell: the server's place, then `(also in: ...)` for a row
/// standing for a class of servers.
fn location_cell(s: &Server) -> String {
    let here = at(s.file.as_deref(), s.line);
    match &s.identity {
        Some(id) => format!("{here} {}", also_in(id, &s.qname)),
        None => here,
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
    let collapse = if args.no_collapse {
        Collapse::off()
    } else {
        Collapse::with_labels(&result.repo_labels)
    };
    let live = glia_engine::entrypoint_reachable(&result.merged);
    let mut answer = match serves_with(&result.merged, &live, &args.channel, &args.mechanism, &collapse) {
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
    println!("# glia serves `{}`", args.channel.trim());
    println!();
    if let Some(a) = &answer.absence {
        println!("_(nothing serves it)_");
        print_absence(a);
        return 0;
    }
    println!("| match | live | kind | server | location | handlers |");
    println!("|---|:-:|---|---|---|---|");
    for s in &answer.results {
        println!(
            "| {} ({}) | {} | {} | `{}` | {} | {} |",
            s.r#match,
            s.confidence,
            live_glyph(s.live),
            s.kind,
            s.qname,
            location_cell(s),
            handlers_cell(s)
        );
    }
    0
}
