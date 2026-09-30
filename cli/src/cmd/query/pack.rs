//! `glia pack` (CC.4c) — the CLI surface over the engine's `pack::pack`
//! (CC.4b): the context for a query packed to a token budget, ready to paste.
//! stdout carries exactly the pack's `text` (so `glia pack . checkout
//! --budget 6000 > ctx.md` writes a clean file); everything else goes to
//! stderr — the engine's `[pack] query=...` fired_on line, then one summary
//! line `packed <P> nodes (<F> full, <V> preview, <O> outline, <Q> qname) in
//! <U>/<B> tokens (est. <x.y> bytes/token); <D> dropped` and, for an empty
//! pack, the absence (`> FACT: ...`, as `glia resolve` words it).
//!
//! `--json` prints the whole `Pack` instead — `{query, text, budget_tokens,
//! used_tokens, bytes, bytes_per_token, candidates, nodes, dropped,
//! rerenders, absence}` — with the same stderr lines. `--bytes-per-token`
//! takes `N` or `N.D` (one decimal) between 1.0 and 20.0, held in tenths as
//! the engine counts it; `--preset` takes a preset name of the code domain's
//! `CODE_TABLES` (read from the table, so a new preset needs no edit here).
//! Exit 0 with nodes; 1 when the pack is empty (no match, or a budget too
//! small for the title and one qname); 2 on a usage or build error.

use glia_code_domain::profile::CODE_TABLES;
use glia_engine::absence::Absence;
use glia_engine::pack::{
    DEFAULT_BUDGET_TOKENS, DEFAULT_CANDIDATES, DEFAULT_SEEDS, Pack, PackArgs, pack,
};

use crate::common::generate_for;

// `--bytes-per-token`'s default is the literal "3.7" (clap's `default_value`
// is a `&'static str`); this keeps it the engine's default.
const _: () = assert!(glia_engine::pack::DEFAULT_BYTES_PER_TOKEN_X10 == 37);

/// The `--bytes-per-token` range, in tenths: 1.0..=20.0.
const MIN_X10: u32 = 10;
const MAX_X10: u32 = 200;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// What to pack the context for: a symbol, qname or fragment, matched as
    /// `glia find` matches it.
    query: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// The most tokens the pack may take (estimated from its bytes).
    #[arg(long, default_value_t = DEFAULT_BUDGET_TOKENS)]
    budget: usize,
    /// Bytes per token for the estimate: `N` or `N.D`, 1.0 to 20.0.
    #[arg(long, default_value = "3.7", value_parser = parse_bytes_per_token)]
    bytes_per_token: u32,
    /// The most `find` rows the pack seeds from.
    #[arg(long, default_value_t = DEFAULT_SEEDS)]
    seeds: usize,
    /// The most nodes the pack chooses from, seeds included.
    #[arg(long, default_value_t = DEFAULT_CANDIDATES)]
    candidates: usize,
    /// An activation preset of the code domain; default the base weights.
    #[arg(
        long,
        value_parser = clap::builder::PossibleValuesParser::new(
            CODE_TABLES.activation_presets.iter().map(|p| p.name)
        ),
    )]
    preset: Option<String>,
    /// Keep seeds and neighbours under this repo-relative path or project
    /// label (see `glia projects`); a node with no file is kept.
    #[arg(long)]
    scope: Option<String>,
    /// Emit the whole pack (text and manifest) as JSON.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = PackArgs::default();
    query.budget_tokens = args.budget;
    query.bytes_per_token_x10 = args.bytes_per_token;
    query.seeds = args.seeds;
    query.candidates = args.candidates;
    query.preset = args.preset;
    query.scope = args.scope;
    let mut packed = pack(&result.merged, &args.query, &query);
    if let Some(a) = packed.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }

    if args.json {
        println!("{}", serde_json::to_string(&packed).unwrap_or_default());
    } else {
        // The text ends in a newline: stdout is exactly `text`.
        print!("{}", packed.text);
    }
    eprintln!("{}", summary(&packed));
    match &packed.absence {
        Some(a) => {
            for line in absence_lines(a) {
                eprintln!("{line}");
            }
            1
        }
        None => 0,
    }
}

/// `N` or `N.D` (one decimal) as tenths, within [`MIN_X10`]..=[`MAX_X10`].
fn parse_bytes_per_token(s: &str) -> Result<u32, String> {
    let bad = || format!("`{s}` is not N or N.D (one decimal) between 1.0 and 20.0");
    let (whole, tenth) = match s.split_once('.') {
        Some((w, d)) if d.len() == 1 => (w, d),
        Some(_) => return Err(bad()),
        None => (s, "0"),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(tenth) {
        return Err(bad());
    }
    let x10 = whole
        .parse::<u32>()
        .ok()
        .and_then(|w| w.checked_mul(10))
        .and_then(|w| tenth.parse::<u32>().ok().and_then(|d| w.checked_add(d)))
        .ok_or_else(bad)?;
    if (MIN_X10..=MAX_X10).contains(&x10) {
        Ok(x10)
    } else {
        Err(bad())
    }
}

/// The one stderr summary line.
fn summary(p: &Pack) -> String {
    let at = |rung: &str| p.nodes.iter().filter(|n| n.fidelity == rung).count();
    format!(
        "packed {} nodes ({} full, {} preview, {} outline, {} qname) in {}/{} tokens (est. {} bytes/token); {} dropped",
        p.nodes.len(),
        at("full"),
        at("preview"),
        at("outline"),
        at("qname"),
        p.used_tokens,
        p.budget_tokens,
        p.bytes_per_token,
        p.dropped,
    )
}

/// Why the pack is empty, worded as `glia resolve`'s absence (stderr here:
/// stdout carries the pack only).
fn absence_lines(a: &Absence) -> Vec<String> {
    let mut lines = vec![format!("> FACT: {}", a.note)];
    for c in &a.caveats {
        lines.push(format!(
            "> caveat ({}, {}): {} - verify: {}",
            c.language, c.edge_category, c.note, c.verify
        ));
    }
    if a.unparsed_files > 0 {
        lines.push(format!(
            "> {} file(s) failed to parse, so the graph may be missing what they hold",
            a.unparsed_files
        ));
    }
    if !a.suggestions.is_empty() {
        lines.push(format!("> did you mean: {}", a.suggestions.join(", ")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::parse_bytes_per_token;

    #[test]
    fn bytes_per_token_is_tenths_in_range() {
        assert_eq!(parse_bytes_per_token("3.7"), Ok(37));
        assert_eq!(parse_bytes_per_token("4"), Ok(40));
        assert_eq!(parse_bytes_per_token("1.0"), Ok(10));
        assert_eq!(parse_bytes_per_token("20"), Ok(200));
        assert_eq!(parse_bytes_per_token("20.0"), Ok(200));
        for bad in [
            "0.5",
            "0.9",
            "20.1",
            "21",
            "3.75",
            "3.",
            ".7",
            "",
            "-3.7",
            "+3.7",
            "3,7",
            "x",
            "99999999999",
        ] {
            assert!(parse_bytes_per_token(bad).is_err(), "{bad:?} accepted");
        }
    }
}
