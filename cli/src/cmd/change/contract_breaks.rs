//! `glia contract-breaks` (CC.8b) — "did my change break my clients?", as a
//! CI gate: the human surface over the engine's
//! `contract_breaks::contract_breaks_vs_rev` (CC.8a), the git rev `--base`
//! (default `HEAD`) against the working tree. Every contract (an OpenAPI /
//! AsyncAPI op, a proto / Avro / JSON Schema message type) is paired old ->
//! new and judged by its format's evolution rules, and every client the change
//! left without a provider is listed.
//!
//! `--with <repo>` (repeatable, CC.8c) adds client repos: both sides are then
//! built as multi-repo merges (`contract_breaks_vs_rev_with`), the provider
//! at the rev and at its working tree, each beside the same clients at their
//! own trees, so a client in another repo that lost its provider is an
//! orphaned client too. Every client is built twice, once per side.
//!
//! Inferred models (DT.1p): every client <-> server model exchange the graph
//! pairs (an `http_model` call site against its handler, a
//! `graphql_selection` operation against its schema type; the engine's
//! `contract_breaks::model_breaks`, DT.1o) is judged across the same rev
//! pair, `--with` clients included. A breaking model row fails the gate only
//! when its pairing tier is at or above `--models-min-tier` (`fact`, the
//! default, or `derived`: check's vocabulary; a heuristic pairing never
//! blocks). Every pairing glia 0.5.2 infers is derived, so by default a model
//! row is ADVISORY: listed, counted in `advisory`, never in `breaking`.
//!
//! Names (FH.7b): every client site, in the provider or a `--with` repo,
//! still using a name the change removed or renamed on the server side of a
//! paired exchange (the engine's `contract_breaks::name_breaks`, FH.7a: the
//! style-agnostic fallback for the exchanges no declared or inferred model
//! covers). A name row is HEURISTIC (DERIVED for a rename over a fact
//! pairing) and fails the gate only under `--models-min-tier heuristic`;
//! under any other tier it is advisory (counted in `advisory`). Under
//! `heuristic` the model rows are judged as under `derived`.
//!
//! Shared-schema fold (FH.7b): one change to a schema many operations
//! reference is one row, listing every operation it folded (JSON
//! `operations`, the table's key ` +<n>`); `breaking` still counts every
//! breaking contract.
//!
//! Table mode prints `# glia contract-breaks <repo> vs <rev>`, a summary line
//! (`- breaking: <B> (<n> contract changes, <o> orphaned clients[, <k>
//! blocking model breaks][, <k> blocking name breaks]) | ..`; with a folded
//! row `<n> contract changes in <c> contracts`), followed, when a model row
//! exists, by `- models: <B> breaking (<K> blocking, <A> advisory) | <C>
//! compatible | min tier <t>` and, when a name row exists, by `- names: <N>
//! client sites (<K> blocking, <A> advisory) | min tier <t>`, then
//! `## breaking` and, unless `--breaking-only` left them out,
//! `## unknown` and `## compatible` (a section with no row is omitted, bar
//! `## breaking`), each `| kind | key | change | tier | at | rule | field |
//! before | after |` with one row per field change (`before` is the rev's
//! declaration, `after` the working tree's; a breaking one's rule in bold)
//! and one row for a contract removed, added or without a comparable field
//! (its `change` carries the engine's note); a folded row's key is its first
//! operation's then ` +<n>` for the others. Then `## models` when a model
//! row exists: `| pairing | via | status | gate | tier | changed | rule |
//! field | before | after | client | server |`, one line per field change
//! (`via` the first exchange's route or operation, ` +<n>` for the rest;
//! `gate` `blocking` / `advisory` on a breaking row, `—` on a compatible
//! one; a breaking rule in bold; a site the side's qname then the field's
//! `file:line` there, else the side's, ` (at <rev>)` on one read from the
//! rev). Then `## names` when a name row exists: `| name | gate | tier | at |
//! position | client | server |`, one line per client site (`name` the old
//! name, `` `old` -> `new` `` for a rename; `gate` `blocking` / `advisory`;
//! `at` the client file's `file:line` in its repo, then ` (<repo>)`; FH.8d:
//! a folded row's `<N> sites in <M> files (e.g. a, b, c)`, the engine's
//! `review::name_break_sites`). Then
//! `## orphaned clients`, `| client | at | category | was calling
//! | reason | tier |`, when a client lost its provider. `at` is the working
//! tree's 1-based `file:line` (LD.1), a removed contract's the rev's, marked
//! `(at <rev>)`. An answer with no row in any section prints `_(no contract
//! change vs <rev>)_` and the engine's absence. `--json` prints the engine's
//! `ContractBreaks` (`{base, schemas, models, orphaned_clients, names,
//! breaking, advisory, absence}`; `names` and a row's `operations` only when
//! not empty).
//!
//! Exit 1 when `breaking` (the breaking contracts plus the orphaned clients
//! plus the BLOCKING model and name rows) is above 0, else 0, in both modes:
//! a compatible or unknown change, and an advisory model or name row, never
//! fail the gate. Exit 2 on a usage error (an `--avro` mode outside
//! `contract_breaks::AVRO_MODES`, a `--models-min-tier` outside
//! `contract_breaks::NAME_MIN_TIERS`, `--no-overlay`) or a git / build
//! failure, with the engine's message.
//!
//! Writes: the working tree's parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored), as `glia delta`
//! does, and each `--with` client's, as `glia merge --incremental` does,
//! under `GLIA_NO_PERSIST=1` too; never a `.gmap` layout.
//!
//! Fired-on markers: the engine's
//! `[contract-breaks] base=<rev> pairs=<P> breaking=<B> compatible=<C> unknown=<U> removed=<R> added=<A> orphaned_clients=<O>`
//! (schema rows only), then its
//! `[model-breaks] base=<rev> min_tier=<t> exchanges=<E> http=<H> graphql=<G> rows=<R> breaking=<B> blocking=<K> advisory=<A> compatible=<C> new_pairings=<N> lost=<L> unjudged=<U>`,
//! once per run each, then FH.7b's
//! `[name-breaks] answer rows=<k> blocking=<b> advisory=<a> min_tier=<t> folded_schema_rows=<s>-><t> surface=contract_breaks`
//! (after FH.7a's `[name-breaks] base=<rev> ..`, which the name pass prints
//! first), then with `--with` `[contract-breaks] clients=<N> built_twice=<N>`.

use glia_engine::contract_breaks::{
    AVRO_MODES, ContractBreakArgs, ContractBreaks, ModelBreak, ModelSite, NAME_MIN_TIERS,
    NameBreak, OrphanedClient, SchemaChange, contract_breaks_vs_rev_with,
};
use glia_engine::contract_fields::{FieldChange, FieldSide};

use glia_engine::review::name_break_sites;

use crate::cmd::resolve::print_absence;
use crate::common::build_options;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo (a git work tree).
    repo: String,
    /// The git rev to compare the working tree against: a branch, tag, sha
    /// or `HEAD~N`.
    #[arg(long, value_name = "REV", default_value = "HEAD")]
    base: String,
    /// Avro's compatibility direction: `backward` (the new schema reads what
    /// the old one wrote), `forward` (the old schema reads what the new one
    /// writes) or `full` (both).
    #[arg(
        long,
        value_name = "MODE",
        default_value = AVRO_MODES[0],
        value_parser = clap::builder::PossibleValuesParser::new(AVRO_MODES),
    )]
    avro: String,
    /// List only the breaking contract changes and model breaks (the exit
    /// code and the orphaned clients are the same either way).
    #[arg(long)]
    breaking_only: bool,
    /// The weakest pairing tier at which an inferred client <-> server model
    /// break fails the gate: fact (the default; every pairing glia 0.5.2
    /// infers is derived, so its rows are advisory), derived, or heuristic
    /// (a stale client name site fails it too; model rows are judged as
    /// under derived); a heuristic model pairing never fails it.
    #[arg(
        long,
        value_name = "TIER",
        default_value = NAME_MIN_TIERS[0],
        value_parser = clap::builder::PossibleValuesParser::new(NAME_MIN_TIERS),
    )]
    models_min_tier: String,
    /// A client repo to build beside the provider on both sides, so its
    /// calls that lose their provider are orphaned clients. Repeatable. Each
    /// client is built twice (once at the rev's side, once at the working
    /// tree's), always at its own working tree.
    #[arg(long, value_name = "REPO")]
    with: Vec<String>,
    /// Emit JSON (`{base, schemas, models, orphaned_clients, names,
    /// breaking, advisory, absence}`) instead of tables.
    #[arg(long)]
    json: bool,
}

/// The engine's arguments; `Err` names a mode outside [`AVRO_MODES`] or a
/// tier outside [`NAME_MIN_TIERS`] (clap has already refused either, so
/// this only guards the `&'static str` lookups).
fn engine_args(
    avro: &str,
    breaking_only: bool,
    models_min_tier: &str,
) -> Result<ContractBreakArgs, String> {
    let Some(mode) = AVRO_MODES.iter().copied().find(|m| *m == avro) else {
        return Err(format!(
            "unknown avro mode `{avro}`: expected one of {}",
            AVRO_MODES.join(", ")
        ));
    };
    let Some(tier) = NAME_MIN_TIERS
        .iter()
        .copied()
        .find(|t| *t == models_min_tier)
    else {
        return Err(format!(
            "unknown models min tier `{models_min_tier}`: expected one of {}",
            NAME_MIN_TIERS.join(", ")
        ));
    };
    let mut args = ContractBreakArgs::default();
    args.avro_mode = mode;
    args.breaking_only = breaking_only;
    args.models_min_tier = tier;
    Ok(args)
}

/// A markdown table cell: a `|` in a type, key or path must not split the row.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `file:line`, `file`, or `None`.
fn at(file: Option<&str>, line: Option<i64>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    }
}

/// A side's location, falling back to its qname.
fn side_at(s: &FieldSide) -> String {
    at(s.file.as_deref(), s.line).unwrap_or_else(|| format!("`{}`", s.qname))
}

/// Where a contract row points: the working tree's declaration, else (a
/// removed contract) the rev's, marked with the rev.
fn row_at(r: &SchemaChange, base: &str) -> String {
    match (&r.after, &r.before) {
        (Some(a), _) => side_at(a),
        (None, Some(b)) => format!("{} (at {base})", side_at(b)),
        (None, None) => "—".to_string(),
    }
}

/// The `change` cell: the contract's change, and the engine's note on it.
fn row_change(r: &SchemaChange) -> String {
    match r.note {
        Some(n) => format!("{} — _{n}_", r.change),
        None => r.change.to_string(),
    }
}

/// The `field` cell: `total`, a non-`fields` section (`request`,
/// `response:<code>`, `payload[:<name>]`) in front as `[response:200] total`.
fn field_name(c: &FieldChange) -> String {
    if c.section == "fields" {
        format!("`{}`", c.field)
    } else {
        format!("[{}] `{}`", c.section, c.field)
    }
}

/// The table rows of one contract: one per field change, else one for the
/// whole contract (removed, added, or not comparable).
fn contract_rows(r: &SchemaChange, base: &str) -> Vec<String> {
    let key = match r.operations.len() {
        0 | 1 => format!("`{}`", r.key),
        n => format!("`{}` +{}", r.key, n - 1),
    };
    let lead = format!(
        "| {} | {} | {} | {} | {} |",
        r.kind,
        cell(&key),
        cell(&row_change(r)),
        r.tier,
        cell(&row_at(r, base)),
    );
    if r.changes.is_empty() {
        return vec![format!("{lead} — | — | — | — |")];
    }
    let side = |s: &Option<String>| s.as_deref().map(cell).unwrap_or_else(|| "—".to_string());
    r.changes
        .iter()
        .map(|c| {
            let rule = if c.breaking {
                format!("**{}**", c.rule)
            } else {
                c.rule.to_string()
            };
            format!(
                "{lead} {rule} | {} | {} | {} |",
                cell(&field_name(c)),
                side(&c.producer),
                side(&c.consumer),
            )
        })
        .collect()
}

fn print_contracts(title: &str, rows: &[&SchemaChange], base: &str) {
    println!("## {title}");
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| kind | key | change | tier | at | rule | field | before | after |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        for line in contract_rows(r, base) {
            println!("{line}");
        }
    }
    println!();
}

/// A model row's side (module docs): its qname, then the change's field
/// site there, else the side's own location; ` (at <rev>)` on a site read
/// from the rev, `—` for none.
fn model_site(side: &FieldSide, field: Option<&ModelSite>, base: &str) -> String {
    let loc = match field.and_then(|s| at(s.file.as_deref(), s.line).map(|a| (a, s.at_base))) {
        Some((a, true)) => format!("{a} (at {base})"),
        Some((a, false)) => a,
        None => at(side.file.as_deref(), side.line).unwrap_or_else(|| "—".to_string()),
    };
    format!("`{}` {loc}", side.qname)
}

/// The `## models` lines of one row: one per field change.
fn model_rows(m: &ModelBreak, base: &str) -> Vec<String> {
    let via = match m.exchanges.split_first() {
        Some((first, [])) => format!("`{}`", first.key),
        Some((first, rest)) => format!("`{}` +{}", first.key, rest.len()),
        None => "—".to_string(),
    };
    let gate = match (m.status, m.blocking) {
        ("breaking", true) => "blocking",
        ("breaking", false) => "advisory",
        _ => "—",
    };
    let side = |s: &Option<String>| s.as_deref().map(cell).unwrap_or_else(|| "—".to_string());
    m.changes
        .iter()
        .map(|c| {
            let rule = if c.breaking {
                format!("**{}**", c.rule)
            } else {
                c.rule.to_string()
            };
            format!(
                "| {} | {} | {} | {gate} | {} | {} | {rule} | {} | {} | {} | {} | {} |",
                m.pairing,
                cell(&via),
                m.status,
                m.tier,
                m.changed,
                cell(&format!("`{}`", c.field)),
                side(&c.before),
                side(&c.after),
                cell(&model_site(&m.client, c.client.as_ref(), base)),
                cell(&model_site(&m.server, c.server.as_ref(), base)),
            )
        })
        .collect()
}

fn print_models(models: &[ModelBreak], base: &str, min_tier: &str) {
    println!("## models");
    println!();
    println!(
        "_Inferred client <-> server models, judged against what the client reads or sends; a breaking row fails the gate only at or above `--models-min-tier` ({min_tier})._"
    );
    println!();
    println!(
        "| pairing | via | status | gate | tier | changed | rule | field | before | after | client | server |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for m in models {
        for line in model_rows(m, base) {
            println!("{line}");
        }
    }
    println!();
}

/// One `## names` line (module docs).
fn name_row(n: &NameBreak) -> String {
    let name = match &n.renamed_to {
        Some(to) => format!("`{}` -> `{to}`", n.name),
        None => format!("`{}`", n.name),
    };
    let gate = if n.blocking { "blocking" } else { "advisory" };
    // FH.8d: a folded row (FH.7d) stands for its sites, not its first one.
    let place =
        name_break_sites(n).unwrap_or_else(|| format!("{}:{} ({})", n.file, n.line, n.repo));
    format!(
        "| {} | {gate} | {} | {} | {} | {} | {} |",
        cell(&name),
        n.tier,
        cell(&place),
        n.position,
        cell(&format!("`{}`", n.client)),
        cell(&format!("`{}`", n.server)),
    )
}

fn print_names(names: &[NameBreak], min_tier: &str) {
    println!("## names");
    println!();
    println!(
        "_Client sites still using a name the change removed or renamed on the server side of a paired exchange; a row fails the gate only under `--models-min-tier heuristic` ({min_tier})._"
    );
    println!();
    println!("| name | gate | tier | at | position | client | server |");
    println!("|---|---|---|---|---|---|---|");
    for n in names {
        println!("{}", name_row(n));
    }
    println!();
}

fn print_orphans(orphans: &[OrphanedClient]) {
    println!("## orphaned clients");
    println!();
    println!("| client | at | category | was calling | reason | tier |");
    println!("|---|---|---|---|---|---|");
    for o in orphans {
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            cell(&format!("`{}`", o.client_qname)),
            cell(&at(o.file.as_deref(), o.line).unwrap_or_else(|| "—".to_string())),
            o.category,
            cell(&format!("`{}`", o.target_qname)),
            o.reason,
            o.tier,
        );
    }
    println!();
}

fn print_table(repo: &str, b: &ContractBreaks, breaking_only: bool, min_tier: &str) {
    let with = |status: &str| -> Vec<&SchemaChange> {
        b.schemas.iter().filter(|r| r.status == status).collect()
    };
    let (breaking, unknown, compatible) = (with("breaking"), with("unknown"), with("compatible"));
    println!("# glia contract-breaks `{repo}` vs {}", b.base);
    println!();
    let rest = if breaking_only {
        "unknown / compatible: left out (--breaking-only)".to_string()
    } else {
        format!(
            "unknown: {} | compatible: {}",
            unknown.len(),
            compatible.len()
        )
    };
    let model_blocking = b.models.iter().filter(|m| m.blocking).count();
    let mut blocking_models = match model_blocking {
        0 => String::new(),
        1 => ", 1 blocking model break".to_string(),
        k => format!(", {k} blocking model breaks"),
    };
    let name_blocking = b.names.iter().filter(|n| n.blocking).count();
    match name_blocking {
        0 => {}
        1 => blocking_models.push_str(", 1 blocking name break"),
        k => blocking_models.push_str(&format!(", {k} blocking name breaks")),
    }
    // FH.7b: a folded row stands for several contracts.
    let contracts: usize = breaking.iter().map(|r| r.operations.len().max(1)).sum();
    let folded = if contracts > breaking.len() {
        format!(" in {contracts} contracts")
    } else {
        String::new()
    };
    println!(
        "- breaking: {} ({} contract {}{folded}, {} orphaned {}{blocking_models}) | {rest}",
        b.breaking,
        breaking.len(),
        if breaking.len() == 1 {
            "change"
        } else {
            "changes"
        },
        b.orphaned_clients.len(),
        if b.orphaned_clients.len() == 1 {
            "client"
        } else {
            "clients"
        },
    );
    if !b.models.is_empty() {
        let model_breaking = b.models.iter().filter(|m| m.status == "breaking").count();
        let model_compatible = if breaking_only {
            "compatible: left out (--breaking-only)".to_string()
        } else {
            format!("{} compatible", b.models.len() - model_breaking)
        };
        println!(
            "- models: {model_breaking} breaking ({model_blocking} blocking, {} advisory) | {model_compatible} | min tier {min_tier}",
            model_breaking - model_blocking,
        );
    }
    if !b.names.is_empty() {
        println!(
            "- names: {} client {} ({name_blocking} blocking, {} advisory) | min tier {min_tier}",
            b.names.len(),
            if b.names.len() == 1 { "site" } else { "sites" },
            b.names.len() - name_blocking,
        );
    }
    println!();
    if b.schemas.is_empty()
        && b.models.is_empty()
        && b.orphaned_clients.is_empty()
        && b.names.is_empty()
    {
        let what = if breaking_only {
            "breaking contract change"
        } else {
            "contract change"
        };
        println!("_(no {what} vs {})_", b.base);
        if let Some(a) = &b.absence {
            println!();
            print_absence(a);
        }
        return;
    }
    print_contracts("breaking", &breaking, &b.base);
    if !unknown.is_empty() {
        print_contracts("unknown", &unknown, &b.base);
    }
    if !compatible.is_empty() {
        print_contracts("compatible", &compatible, &b.base);
    }
    if !b.models.is_empty() {
        print_models(&b.models, &b.base, min_tier);
    }
    if !b.names.is_empty() {
        print_names(&b.names, min_tier);
    }
    if !b.orphaned_clients.is_empty() {
        print_orphans(&b.orphaned_clients);
    }
}

pub(crate) fn run(args: Args) -> i32 {
    if !build_options().overlay {
        eprintln!(
            "error: --no-overlay does not apply to contract-breaks: both sides are built with \
             the repo's overlay"
        );
        return 2;
    }
    let engine = match engine_args(&args.avro, args.breaking_only, &args.models_min_tier) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let answer = match contract_breaks_vs_rev_with(&args.repo, &args.base, &args.with, &engine) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
    } else {
        print_table(
            &args.repo,
            &answer,
            args.breaking_only,
            engine.models_min_tier,
        );
    }
    if answer.breaking > 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_mode_maps_and_others_are_refused() {
        for mode in AVRO_MODES {
            let a = engine_args(mode, true, "fact").expect("an engine mode");
            assert_eq!((a.avro_mode, a.breaking_only), (*mode, true));
        }
        let err = engine_args("sideways", false, "fact").expect_err("no such mode");
        assert_eq!(
            err,
            "unknown avro mode `sideways`: expected one of backward, forward, full"
        );
    }

    /// DT.1p / FH.7b: every min tier maps (`heuristic` too, the name rows'
    /// gate), the default is `fact`, and another spelling is refused in the
    /// engine's words.
    #[test]
    fn every_models_min_tier_maps_and_others_are_refused() {
        for tier in NAME_MIN_TIERS {
            let a = engine_args("backward", false, tier).expect("an engine tier");
            assert_eq!(a.models_min_tier, *tier);
        }
        assert_eq!(NAME_MIN_TIERS, &["fact", "derived", "heuristic"]);
        assert_eq!(ContractBreakArgs::default().models_min_tier, "fact");
        let err = engine_args("backward", false, "certain").expect_err("no such gate");
        assert_eq!(
            err,
            "unknown models min tier `certain`: expected one of fact, derived, heuristic"
        );
    }

    /// A git repo for one test under the temp dir, removed on drop.
    struct Repo(std::path::PathBuf);

    impl Repo {
        fn git(&self, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=glia",
                    "-c",
                    "user.email=glia@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .expect("a git binary");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("dirs");
            std::fs::write(path, text).expect("write");
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const TICKET_SERVER: &str = "package main\n\nimport (\n\t\"net/http\"\n\n\t\"github.com/gin-gonic/gin\"\n)\n\ntype Ticket struct {\n\tID    string `json:\"id\"`\n\tOwner string `json:\"owner\"`\n}\n\nfunc getTicket(c *gin.Context) {\n\tc.JSON(http.StatusOK, Ticket{ID: \"t-1\", Owner: \"ops\"})\n}\n\nfunc main() {\n\tr := gin.Default()\n\tr.GET(\"/api/tickets/:ref\", getTicket)\n\t_ = r.Run(\":8080\")\n}\n";
    const TICKET_CLIENT: &str = "import axios from 'axios';\n\nexport async function loadTicket(ref: string) {\n  const res = await axios.get(`/api/tickets/${ref}`);\n  return { key: res.data.id };\n}\n";
    const TICKET_ROWS: &str = "import { loadTicket } from './ticket';\n\nexport async function rows(ref: string) {\n  const t = await loadTicket(ref);\n  const row = { id: t.key };\n  return [row];\n}\n\nexport function pick(items: { key: string }[]): string {\n  const first = items[0].id;\n  const id = first;\n  return id;\n}\n";

    /// FH.8d: a generic rename's free-text sites fold into one row (FH.7d),
    /// and `## names` prints its counts and sample files where a site row
    /// prints its `file:line (repo)`.
    #[test]
    fn a_folded_name_row_prints_its_counts() {
        let dir = std::env::temp_dir().join(format!("glia-fh8d-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let repo = Repo(dir);
        repo.git(&["init", "-q"]);
        repo.write(
            "go.mod",
            "module example.com/tickets\n\ngo 1.22\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
        );
        repo.write("main.go", TICKET_SERVER);
        repo.write(
            "package.json",
            "{\"name\":\"tickets\",\"dependencies\":{\"axios\":\"^1.6.0\"}}\n",
        );
        repo.write("web/ticket.ts", TICKET_CLIENT);
        repo.write("web/rows.ts", TICKET_ROWS);
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", "base"]);
        repo.write(
            "main.go",
            &TICKET_SERVER.replace("`json:\"id\"`", "`json:\"ticketId\"`"),
        );
        let path = repo.0.to_str().expect("utf-8 path");
        let b = contract_breaks_vs_rev_with(path, "HEAD", &[], &ContractBreakArgs::default())
            .expect("contract breaks");
        let rows: Vec<String> = b.names.iter().map(name_row).collect();
        let folded: Vec<&String> = rows.iter().filter(|r| r.contains(" | folded | ")).collect();
        assert_eq!(folded.len(), 1, "{rows:#?}");
        assert!(
            folded[0].starts_with(
                "| `id` -> `ticketId` | advisory | heuristic | 3 sites in 1 files (e.g. web/rows.ts) | folded | "
            ),
            "{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains(" | web/ticket.ts:5 (") && r.contains(" | property | ")),
            "{rows:#?}"
        );
    }

    #[test]
    fn a_pipe_never_splits_a_row() {
        assert_eq!(cell("string|null"), "string\\|null");
        assert_eq!(at(Some("a.yaml"), Some(7)).as_deref(), Some("a.yaml:7"));
        assert_eq!(at(Some("a.yaml"), None).as_deref(), Some("a.yaml"));
        assert_eq!(at(None, Some(7)), None);
    }
}
