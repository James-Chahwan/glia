//! Stale feature-flag report (CC.7b), report only, never a rewrite: every
//! flag with its definitions and function-level readers, and its dead /
//! undefined / single-site / quiet findings, each tiered. Public slot,
//! reached by module path (`glia_engine::flags::<item>`). An inventory of
//! flag checks: no flag value, no taint, no auth semantics.
//!
//! A FLAG is every CONFIG_KEY whose qname is `config:flag:<key>` (A13.8),
//! grouped by `<key>` across repos. `providers` are the distinct `source`
//! values of its ENV cells (`launchdarkly`, `flipt`, ...). `definitions` are
//! the DEFINES_CONFIG edges into its nodes (a Flipt `flags:` file), `reads`
//! the READS_CONFIG edges (an SDK check, on the reading function since
//! CC.7a); SHARES_CONFIG is ignored. Each site is its source node, located at
//! the edge's EVIDENCE file and line when the evidence names a file (a Flipt
//! definition names the file only), else at the node through one
//! [`Locator`]; sites sort by (file, line, qname). `readers` counts distinct
//! reading nodes; `definitions_in_graph` counts the distinct files holding a
//! flag definition anywhere in the graph (plus each definer no evidence or
//! position places). `0` means the repo keeps its definitions outside its
//! files, and the undefined rule is then not evaluated.
//!
//! FINDINGS, in this order on a row:
//! - `dead` (derived): defined, never read. The scanner captures literal
//!   keys only, so a key built at runtime reads as unused.
//! - `undefined` (derived): read, defined nowhere, while the graph holds at
//!   least one flag definition file.
//! - `single_site` (fact): exactly one reading node.
//! - `quiet` (heuristic): every reader carries history recency (symbol blame
//!   `last` for a FUNCTION / METHOD / CLASS, module churn `last` for a
//!   MODULE, read through `external::signals`), and the newest of them,
//!   `last_read_change`, is at least `quiet_days` older than
//!   `signals::history_now`: the graph's own newest history change, never
//!   the wall clock, so the answer is a function of the graph. It says when
//!   the reading code last changed, not when the flag was last used or
//!   flipped. `last_read_change` is `None` while any reader is undated.
//!
//! `scope` (a path or a project label, resolved once) keeps a row when any
//! of its sites is under it or unlocated (`answers::node_in_scope`'s rules);
//! a kept row stays whole. Rows sort with findings first, then by key. An
//! empty report carries an [`Absence`]: `no_match` when no flag node exists,
//! or when the scope dropped every row.
//!
//! fired_on marker, once per report:
//! `[flags] keys=<K> defined_files=<D> dead=<a> undefined=<b> single_site=<c> quiet=<d> quiet_evaluated=<true|false>`,
//! plus `[flags] undefined not evaluated: ...` when flags are read and the
//! graph holds no definition file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::answers::{Locator, in_scope, resolve_scope};
use crate::external::signals;

/// Days a flag's reading code must be unchanged to be `quiet`.
pub const DEFAULT_QUIET_DAYS: u32 = 90;
/// The qname prefix of a feature-flag CONFIG_KEY.
pub const FLAG_PREFIX: &str = "config:flag:";
pub const STATUS_DEAD: &str = "dead";
pub const STATUS_UNDEFINED: &str = "undefined";
pub const STATUS_SINGLE_SITE: &str = "single_site";
pub const STATUS_QUIET: &str = "quiet";
/// Every finding status, in the order a row lists them.
pub const STATUSES: [&str; 4] = [
    STATUS_DEAD,
    STATUS_UNDEFINED,
    STATUS_SINGLE_SITE,
    STATUS_QUIET,
];
pub const TIER_FACT: &str = "fact";
pub const TIER_DERIVED: &str = "derived";
pub const TIER_HEURISTIC: &str = "heuristic";

const PRIMITIVE: &str = "flags";
const DAY: i64 = 86_400;
const SYMBOL_KINDS: [glia_core::NodeKindId; 3] =
    [node_kind::FUNCTION, node_kind::METHOD, node_kind::CLASS];

/// The status constant spelled `s`, if it is one.
pub fn parse_status(s: &str) -> Option<&'static str> {
    STATUSES.into_iter().find(|x| *x == s)
}

/// What [`flags`] reports. Start from `default()` and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct FlagArgs {
    /// Days of unchanged reading code that make a flag `quiet`.
    pub quiet_days: u32,
    /// Keep only rows with a site under this path or project label.
    pub scope: Option<String>,
}

impl Default for FlagArgs {
    fn default() -> Self {
        FlagArgs {
            quiet_days: DEFAULT_QUIET_DAYS,
            scope: None,
        }
    }
}

/// One definition or read of a flag: the node that makes it, and where.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct FlagSite {
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based: the EVIDENCE line when the evidence names a file, else the node's.
    pub line: Option<i64>,
}

/// One finding on a flag.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct FlagFinding {
    /// One of [`STATUSES`].
    pub status: &'static str,
    /// [`TIER_FACT`], [`TIER_DERIVED`] or [`TIER_HEURISTIC`].
    pub tier: &'static str,
    pub note: String,
}

/// One flag key, across every repo of the graph.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct FlagRow {
    pub key: String,
    /// Distinct ENV-cell `source` values, sorted.
    pub providers: Vec<String>,
    pub definitions: Vec<FlagSite>,
    pub reads: Vec<FlagSite>,
    /// Distinct reading nodes.
    pub readers: usize,
    /// Newest history time (unix seconds) of any reader; `None` unless every
    /// reader carries one.
    pub last_read_change: Option<i64>,
    pub findings: Vec<FlagFinding>,
}

/// The stale-flag report.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct FlagsReport {
    pub flags: Vec<FlagRow>,
    /// Distinct files holding a flag definition; `0` turns the undefined rule off.
    pub definitions_in_graph: usize,
    /// History was present and some reader of a reported flag was dated.
    pub quiet_evaluated: bool,
    /// The reference time of `quiet` (`signals::history_now`).
    pub history_now: Option<i64>,
    pub quiet_days: u32,
    /// Findings per status, every status present.
    pub counts: BTreeMap<&'static str, usize>,
    /// `Some` iff `flags` is empty.
    pub absence: Option<Absence>,
}

/// A located site and the node it belongs to.
struct Site {
    from: NodeId,
    at: FlagSite,
}

#[derive(Default)]
struct Acc {
    providers: BTreeSet<String>,
    definitions: Vec<Site>,
    reads: Vec<Site>,
}

/// Every feature flag of `merged`, its sites and its findings.
pub fn flags(merged: &MergedGraph, args: &FlagArgs) -> FlagsReport {
    let loc = Locator::new(merged);
    let scope = args.scope.as_deref().map(|s| resolve_scope(merged, s));
    let accs = collect(merged, &loc);

    let mut files: BTreeSet<&str> = BTreeSet::new();
    let mut unfiled: HashSet<NodeId> = HashSet::new();
    for s in accs.values().flat_map(|a| &a.definitions) {
        match s.at.file.as_deref() {
            Some(f) => {
                files.insert(f);
            }
            None => {
                unfiled.insert(s.from);
            }
        }
    }
    let definitions_in_graph = files.len() + unfiled.len();
    let recency = recency(merged, &accs);
    let now = signals::history_now(merged);
    let quiet_before = now.map(|t| t.saturating_sub(i64::from(args.quiet_days) * DAY));

    let (mut rows, mut dropped, mut quiet_evaluated) = (Vec::new(), 0usize, false);
    for (key, acc) in accs {
        let in_view = |s: &Site| {
            s.at.file
                .as_deref()
                .is_none_or(|f| scope.as_deref().is_none_or(|sc| in_scope(f, sc)))
        };
        if !acc.definitions.iter().chain(&acc.reads).any(in_view) {
            dropped += 1;
            continue;
        }
        let readers: BTreeSet<u64> = acc.reads.iter().map(|s| s.from.0).collect();
        let dates: Vec<Option<(i64, bool)>> =
            readers.iter().map(|id| recency.get(id).copied()).collect();
        quiet_evaluated |= now.is_some() && dates.iter().any(Option::is_some);
        let last_read_change = if dates.is_empty() {
            None
        } else {
            dates
                .iter()
                .try_fold(i64::MIN, |m, d| d.map(|(t, _)| m.max(t)))
        };

        let mut findings = Vec::new();
        if acc.reads.is_empty()
            && let Some(first) = acc.definitions.first()
        {
            let more = acc.definitions.len() - 1;
            let more = if more > 0 {
                format!(" (+{more} more)")
            } else {
                String::new()
            };
            findings.push(finding(STATUS_DEAD, TIER_DERIVED, format!(
                "defined in {}{more}; no literal read of the key is extracted (a key built at runtime is not captured)",
                site_ref(&first.at)
            )));
        }
        if !acc.reads.is_empty() && acc.definitions.is_empty() && definitions_in_graph > 0 {
            findings.push(finding(STATUS_UNDEFINED, TIER_DERIVED, format!(
                "read, but none of the {definitions_in_graph} flag definition file(s) in the graph defines it; a provider console keeps definitions outside the repo"
            )));
        }
        if readers.len() == 1
            && let Some(first) = acc.reads.first()
        {
            let place = if first.at.file.is_some() {
                format!(" ({})", site_ref(&first.at))
            } else {
                String::new()
            };
            findings.push(finding(
                STATUS_SINGLE_SITE,
                TIER_FACT,
                format!("read only in {}{place}", first.at.qname),
            ));
        }
        if let (Some(now), Some(before), Some(last)) = (now, quiet_before, last_read_change)
            && last <= before
        {
            let source = if dates.iter().flatten().any(|(_, module)| *module) {
                "git history of the reading code"
            } else {
                "git blame"
            };
            findings.push(finding(STATUS_QUIET, TIER_HEURISTIC, format!(
                "no reading line changed in {} days before the snapshot's newest change ({source}, not runtime use)",
                (now - last) / DAY
            )));
        }
        rows.push(FlagRow {
            key,
            providers: acc.providers.into_iter().collect(),
            definitions: acc.definitions.into_iter().map(|s| s.at).collect(),
            reads: acc.reads.into_iter().map(|s| s.at).collect(),
            readers: readers.len(),
            last_read_change,
            findings,
        });
    }
    rows.sort_by(|a, b| {
        a.findings
            .is_empty()
            .cmp(&b.findings.is_empty())
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut counts: BTreeMap<&'static str, usize> = STATUSES.iter().map(|s| (*s, 0)).collect();
    for f in rows.iter().flat_map(|r| &r.findings) {
        *counts.entry(f.status).or_default() += 1;
    }
    let n = |s: &str| counts.get(s).copied().unwrap_or(0);
    eprintln!(
        "[flags] keys={} defined_files={definitions_in_graph} dead={} undefined={} single_site={} quiet={} quiet_evaluated={quiet_evaluated}",
        rows.len(),
        n(STATUS_DEAD),
        n(STATUS_UNDEFINED),
        n(STATUS_SINGLE_SITE),
        n(STATUS_QUIET),
    );
    let read_keys = rows.iter().filter(|r| r.readers > 0).count();
    if definitions_in_graph == 0 && read_keys > 0 {
        eprintln!(
            "[flags] undefined not evaluated: no flag definition file in the graph (read keys={read_keys})"
        );
    }

    let absence = rows.is_empty().then(|| {
        let query = format!(
            "quiet_days={}{}",
            args.quiet_days,
            args.scope.as_deref().map(|s| format!(" scope={s}")).unwrap_or_default()
        );
        match args.scope.as_deref() {
            Some(s) if dropped > 0 => absence::scope_emptied(merged, PRIMITIVE, &query, dropped, s),
            _ => absence::empty(
                merged,
                PRIMITIVE,
                &query,
                "no_match",
                "no feature-flag read or definition was extracted (SDKs: LaunchDarkly, OpenFeature, Unleash, Flagsmith, Split; Flipt flags: yaml)".to_string(),
                &[edge_category::name(edge_category::READS_CONFIG), edge_category::name(edge_category::DEFINES_CONFIG)],
                None,
            ),
        }
    });
    FlagsReport {
        flags: rows,
        definitions_in_graph,
        quiet_evaluated,
        history_now: now,
        quiet_days: args.quiet_days,
        counts,
        absence,
    }
}

/// Every flag key with its providers and its sorted sites, from one pass
/// over the nodes and one over the edges (intra and cross, each
/// `(from, to, category)` once).
fn collect(merged: &MergedGraph, loc: &Locator<'_>) -> BTreeMap<String, Acc> {
    let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
    let mut key_of: HashMap<NodeId, String> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CONFIG_KEY) {
                continue;
            }
            let Some(key) = g
                .nav
                .qname_by_id
                .get(&n.id)
                .and_then(|q| q.strip_prefix(FLAG_PREFIX))
            else {
                continue;
            };
            if key.is_empty() {
                continue;
            }
            key_of.entry(n.id).or_insert_with(|| key.to_string());
            accs.entry(key.to_string())
                .or_default()
                .providers
                .extend(providers(&n.cells));
        }
    }
    let mut seen = HashSet::new();
    let edges = merged
        .graphs
        .iter()
        .flat_map(|g| &g.edges)
        .chain(&merged.cross_edges);
    for e in edges {
        let defines = e.category == edge_category::DEFINES_CONFIG;
        if !defines && e.category != edge_category::READS_CONFIG {
            continue;
        }
        let Some(key) = key_of.get(&e.to) else {
            continue;
        };
        if !seen.insert((e.from, e.to, e.category)) {
            continue;
        }
        let at = loc.locate(e.from);
        let (file, line) = match Evidence::of(e) {
            Some(Evidence {
                file: Some(f),
                line,
                ..
            }) => (Some(f), line.map(|l| i64::from(l) + 1)),
            _ => (at.file, at.line),
        };
        let site = Site {
            from: e.from,
            at: FlagSite {
                qname: at.qname,
                kind: at.kind,
                file,
                line,
            },
        };
        let acc = accs.entry(key.clone()).or_default();
        if defines {
            acc.definitions.push(site)
        } else {
            acc.reads.push(site)
        }
    }
    for acc in accs.values_mut() {
        for sites in [&mut acc.definitions, &mut acc.reads] {
            sites.sort_by(|a, b| {
                (&a.at.file, a.at.line, &a.at.qname, a.from.0).cmp(&(
                    &b.at.file,
                    b.at.line,
                    &b.at.qname,
                    b.from.0,
                ))
            });
        }
    }
    accs
}

/// The distinct ENV-cell `source` values of a flag node.
fn providers(cells: &[Cell]) -> Vec<String> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ENV)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => {
                serde_json::from_str::<serde_json::Value>(s).ok()
            }
            CellPayload::Bytes(_) => None,
        })
        .filter_map(|v| {
            v.get("source")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Reader id -> (newest history time, dated by module churn): the first
/// dated copy of each reader in graph order.
fn recency(merged: &MergedGraph, accs: &BTreeMap<String, Acc>) -> HashMap<u64, (i64, bool)> {
    let readers: HashSet<NodeId> = accs
        .values()
        .flat_map(|a| a.reads.iter().map(|s| s.from))
        .collect();
    let mut out = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if !readers.contains(&n.id) || out.contains_key(&n.id.0) {
                continue;
            }
            let dated = match g.nav.kind_by_id.get(&n.id) {
                Some(&k) if k == node_kind::MODULE => {
                    signals::module_churn(&n.cells).map(|m| (m.last, true))
                }
                Some(k) if SYMBOL_KINDS.contains(k) => {
                    signals::symbol_blame(&n.cells).map(|b| (b.last, false))
                }
                _ => None,
            };
            if let Some(d) = dated {
                out.insert(n.id.0, d);
            }
        }
    }
    out
}

fn finding(status: &'static str, tier: &'static str, note: String) -> FlagFinding {
    FlagFinding { status, tier, note }
}

/// `file:line`, `file`, or the qname of an unlocated site.
fn site_ref(s: &FlagSite) -> String {
    match (&s.file, s.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.clone(),
        (None, _) => s.qname.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_parse() {
        assert_eq!(parse_status("single_site"), Some(STATUS_SINGLE_SITE));
        assert_eq!(parse_status("stale"), None);
    }

    #[test]
    fn providers_read_the_source_of_every_env_cell() {
        let env = |s: &str| Cell {
            kind: cell_type::ENV,
            payload: CellPayload::Json(s.into()),
        };
        let cells = [
            env(r#"{"source":"launchdarkly","redacted":true}"#),
            env("not json"),
            env(r#"{"redacted":true}"#),
            Cell {
                kind: cell_type::ATTN,
                payload: CellPayload::Json(r#"{"source":"git"}"#.into()),
            },
        ];
        assert_eq!(providers(&cells), ["launchdarkly"]);
    }

    #[test]
    fn site_ref_names_what_is_known() {
        let site = |file: Option<&str>, line| FlagSite {
            qname: "m::f".into(),
            kind: "FUNCTION",
            file: file.map(Into::into),
            line,
        };
        assert_eq!(site_ref(&site(Some("a.py"), Some(3))), "a.py:3");
        assert_eq!(site_ref(&site(Some("a.yaml"), None)), "a.yaml");
        assert_eq!(site_ref(&site(None, None)), "m::f");
    }
}
