//! Boundary health (0.5.4, FH batch): per mechanism (HTTP, gRPC, RPC,
//! GraphQL, queues, WebSocket), how much of a repo's cross-boundary surface the
//! graph pairs, leaves unpaired or cannot see, reported as an answer. Owner
//! FH.2a fills this slot; FH.2b (`glia health`, `PyGraph.health()`) and FH.9
//! (the `blind` caveat on answers) read it by module path
//! (`glia_engine::health::..`), never flattened.
//!
//! [`health`] reads the built graph, plus one gated walk of each repo root for
//! the files no parser reads; it never builds, writes or refreshes anything.
//! What it measures (the definitions start from the cohort probe's
//! `health.py`, FH.1b):
//!
//! - **Mechanisms** ([`MECHANISMS`]): per mechanism the distinct producer
//!   nodes (the client side), consumer nodes (the server side) and pair edges.
//!   Client reach is producers with an outgoing pair edge over producers;
//!   server reach is consumers with an incoming pair edge over consumers. A
//!   producer stamped ORIGIN `external` (a call to someone else's service,
//!   CJ.3 / DF.10) is counted apart and in neither ratio, and so is one
//!   stamped ORIGIN `test_fixture` (a test file's call into the app's own
//!   router through an inferred wrapper, FH.17, which pairs nothing by
//!   design; FH.26, `test_site_producers`). HTTP's consumers
//!   leave out client-router pages (ORIGIN `nav_route`) and ROUTEs only test
//!   code registers (DT.6a, `passes::test_route_ids`), and a ROUTE a WebSocket
//!   client connects through counts as reached (DT.3c), so server reach is
//!   `gaps`' `unpaired_route` reading.
//! - **Role**: `library` when no ROUTE (pages included), RPC_PROCEDURE or
//!   ENDPOINT sits outside test code (ORIGIN `test_fixture`) and example
//!   trees (a path component `example`, `examples` or `_examples`), else
//!   `app`. A library is never judged on reach: each of its mechanisms is OK,
//!   reason `library`.
//! - **Verdict** per mechanism, the first rule that holds: BLIND `no_clients`
//!   (consumers >= 10 and producers < 10% of them) or `no_servers` (the
//!   mirror); BLIND `clients_unpaired` (client reach < 20% with >= 10
//!   producers) or `servers_unreached` (server reach < 20% with >= 10
//!   consumers); DEGRADED `clients_partly_paired` / `servers_partly_reached`
//!   (that reach < 60%, same floors); else OK, reason `paired`, `few` (both
//!   sides under 10) or `absent` (neither side exists).
//! - **Parse coverage**: the walk's files by extension that no parser takes
//!   (`extract::detect_language` is `None`), split in two. Code-looking files
//!   ([`CODE_LIKE_EXTS`]: single-file components, server templates, `.mjs` /
//!   `.cts` and the like) are `unparsed` and judge the `parse` row: BLIND
//!   `code_unparsed` with >= 10 such files making >= 10% of the code files,
//!   DEGRADED `code_partly_unparsed` with >= 10 making >= 2%, OK
//!   `minor_unparsed` with fewer (a monorepo's `.mjs` / `.cjs` tool configs
//!   are no blind spot), else OK `parsed`. Prose files
//!   ([`DOC_LIKE_EXTS`]: `.mdx`, `.rst`, `.txt`, ..) are `docs_unparsed`,
//!   reported apart and never a flag (FH.1b: doc-heavy repos read blind with
//!   no code component unread).
//! - **Workspace imports**: a package a manifest under a repo root declares
//!   (a non-root PROJECT whose ORIGIN `label_source` is `manifest`, ecosystem
//!   npm / go / cargo) that a MODULE's IMPORTS cell (the library names the
//!   build left external) lists, by its name or a `<name>/..` subpath, with
//!   no IMPORTS edge from that module into the package's directory.
//! - **Suspicious nodes**: ENDPOINT / ROUTE qnames whose path holds a
//!   `data:` / `blob:` / `mailto:` URL, `base64`, or whitespace.
//! - **Dead share**: `gaps`' `dead_symbol` count (computed with no roots,
//!   `surface=health`) over the distinct CLASS / FUNCTION / METHOD nodes.
//!
//! The answer's verdict is the worst row (mechanisms, then `parse`); its
//! reason names that row. Every list is sorted (mechanisms in
//! [`MECHANISMS`] order, extensions and packages by name); a HashMap or
//! HashSet here is only probed, never iterated into an output.
//!
//! **Blind answers (FH.9).** An answer that reads a mechanism this module
//! rates BLIND or DEGRADED carries a FACT [`BlindCaveat`] row naming it
//! ([`blind_caveats`]): contract-breaks (every client mechanism its orphan
//! search reads), review (its impact's mechanisms plus `http` / `graphql`,
//! the model exchanges), serves (the channel's), trace and each entry flow
//! (the pair categories and boundary kinds on its hops), diff-impact (its
//! impact's, [`blast_mechanisms`]) and gaps (every mechanism, or the one its
//! category reads). [`blind_line`] is the table form.
//!
//! fired_on marker, once per [`health_with`] / [`health`] call, on stderr:
//!   `[health] repo=<label> role=<app|library> http=<OK|DEGRADED|BLIND>:<reach>
//!   grpc=.. rpc=.. graphql=.. queue=.. ws=.. unparsed=<n> workspace_unresolved=<n>
//!   suspicious=<n> dead_share=<x> docs_unparsed=<d> parse=<V> verdict=<V> surface=<s>`
//! (one line; `<reach>` is the deciding ratio to three decimals, `-` when no
//! ratio decided; `<label>` joins a merge's labels with `+`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::project_roots::LABEL_MANIFEST;
use glia_code_domain::walk_gating::{Gate, IgnoreStack, gate_dir};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_graph::nav::{is_nav_route, nav_route_path};

use glia_activation::profile::Tier;

use crate::absence::BlindCaveat;
use crate::arch::node_file;
use crate::extract::detect_language;
use crate::gaps::{DEAD_SYMBOL, DEAD_SYMBOL_KINDS, GapsOptions, gaps_report};
use crate::profile::CodeBuildCtx;

/// A side of a mechanism needs at least this many nodes before a ratio can
/// make it BLIND or DEGRADED.
pub const MIN_SIDE: usize = 10;
/// Under this reach a side is BLIND; also the lopsided ratio's cut for the
/// other side (`LOPSIDED`).
pub const BLIND_REACH: f64 = 0.2;
/// Under this reach (and at or over [`BLIND_REACH`]) a side is DEGRADED.
pub const DEGRADED_REACH: f64 = 0.6;
/// One side counts as missing when it holds under this share of the other.
pub const LOPSIDED: f64 = 0.1;
/// The `parse` row is BLIND when the unparsed code files make at least this
/// share of the code files (and number at least [`MIN_SIDE`]).
pub const UNPARSED_BLIND_SHARE: f64 = 0.1;
/// The `parse` row is DEGRADED from this share (with at least [`MIN_SIDE`]
/// files) up to [`UNPARSED_BLIND_SHARE`].
pub const UNPARSED_DEGRADED_SHARE: f64 = 0.02;
/// How many nodes [`Health::role_basis`] keeps.
pub const ROLE_BASIS: usize = 5;
/// How many suspicious qnames [`Suspicious::examples`] keeps.
pub const SUSPICIOUS_EXAMPLES: usize = 20;

/// Code-looking extensions no parser reads: single-file components, server
/// templates, module-flavoured JS / TS and a few languages glia does not
/// parse. A file of one of these with no parser is an `unparsed` file.
pub const CODE_LIKE_EXTS: [&str; 37] = [
    "cjs",
    "coffee",
    "cr",
    "cshtml",
    "cts",
    "eex",
    "ejs",
    "elm",
    "erb",
    "erl",
    "fs",
    "fsx",
    "groovy",
    "haml",
    "handlebars",
    "hbs",
    "heex",
    "hrl",
    "hs",
    "j2",
    "jinja",
    "jinja2",
    "jl",
    "kts",
    "leex",
    "liquid",
    "lua",
    "marko",
    "mjs",
    "ml",
    "mts",
    "mustache",
    "njk",
    "pug",
    "razor",
    "slim",
    "twig",
];

/// Prose extensions no parser reads (`.md` is ingested by the docs graph, so
/// it is not here). Counted as `docs_unparsed`, never a flag.
pub const DOC_LIKE_EXTS: [&str; 10] = [
    "adoc", "asciidoc", "markdown", "mdx", "org", "pod", "rdoc", "rst", "tex", "txt",
];

/// The ecosystems whose sub-project manifests name a workspace package.
const WORKSPACE_ECOSYSTEMS: [&str; 3] = ["npm", "go", "cargo"];

/// A path component that marks an example tree (never the repo's own app).
const EXAMPLE_DIRS: [&str; 3] = ["example", "examples", "_examples"];

/// One mechanism's node kinds and pair category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MechanismSpec {
    /// The name the marker and the answer use (`http`, `grpc`, ..).
    pub name: &'static str,
    /// The client side: the node kind that calls.
    pub producer: NodeKindId,
    /// The server side: the node kind that is called.
    pub consumer: NodeKindId,
    /// The cross edge that pairs a producer with a consumer.
    pub pair: EdgeCategoryId,
}

/// Every mechanism [`health`] judges, in answer and marker order.
pub const MECHANISMS: [MechanismSpec; 6] = [
    MechanismSpec {
        name: "http",
        producer: node_kind::ENDPOINT,
        consumer: node_kind::ROUTE,
        pair: edge_category::HTTP_CALLS,
    },
    MechanismSpec {
        name: "grpc",
        producer: node_kind::GRPC_CLIENT,
        consumer: node_kind::GRPC_SERVICE,
        pair: edge_category::GRPC_CALLS,
    },
    MechanismSpec {
        name: "rpc",
        producer: node_kind::RPC_CALL,
        consumer: node_kind::RPC_PROCEDURE,
        pair: edge_category::RPC_CALLS,
    },
    MechanismSpec {
        name: "graphql",
        producer: node_kind::GRAPHQL_OPERATION,
        consumer: node_kind::GRAPHQL_RESOLVER,
        pair: edge_category::GRAPHQL_CALLS,
    },
    MechanismSpec {
        name: "queue",
        producer: node_kind::QUEUE_PRODUCER,
        consumer: node_kind::QUEUE_CONSUMER,
        pair: edge_category::QUEUE_FLOWS,
    },
    MechanismSpec {
        name: "ws",
        producer: node_kind::WS_CLIENT,
        consumer: node_kind::WS_HANDLER,
        pair: edge_category::WS_CONNECTS,
    },
];

/// How far an answer that reads a mechanism can be trusted.
#[derive(Serialize, Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Verdict {
    /// The graph pairs most of what it sees.
    #[default]
    #[serde(rename = "OK")]
    Ok,
    /// The graph pairs a minority (20% .. 60%) of one side.
    #[serde(rename = "DEGRADED")]
    Degraded,
    /// The graph pairs almost nothing, or sees one side only.
    #[serde(rename = "BLIND")]
    Blind,
}

impl Verdict {
    /// `OK`, `DEGRADED` or `BLIND`.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Ok => "OK",
            Verdict::Degraded => "DEGRADED",
            Verdict::Blind => "BLIND",
        }
    }
}

/// What the repo is, by its boundary nodes.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Role {
    /// Serves or calls something outside tests and examples.
    App,
    /// No ROUTE, page, RPC_PROCEDURE or ENDPOINT outside tests and examples.
    Library,
}

impl Role {
    /// `app` or `library`.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::App => "app",
            Role::Library => "library",
        }
    }
}

/// One mechanism's counts and verdict.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MechanismHealth {
    /// [`MechanismSpec::name`].
    pub mechanism: &'static str,
    /// Distinct producer nodes, external and test-site ones left out.
    pub producers: usize,
    /// Distinct producer nodes stamped ORIGIN `external`.
    pub external_producers: usize,
    /// Distinct producer nodes stamped ORIGIN `test_fixture` (test-file
    /// sites, FH.17), in neither ratio (FH.26).
    pub test_site_producers: usize,
    /// Distinct consumer nodes (for HTTP: no pages, no test-only ROUTEs).
    pub consumers: usize,
    /// Pair edges of the mechanism's category.
    pub pairs: usize,
    /// Producers with an outgoing pair edge.
    pub paired_producers: usize,
    /// Consumers with an incoming pair edge (HTTP: or a WS client).
    pub reached_consumers: usize,
    /// `paired_producers / producers`; `None` with no producer.
    pub client_reach: Option<f64>,
    /// `reached_consumers / consumers`; `None` with no consumer.
    pub server_reach: Option<f64>,
    pub verdict: Verdict,
    /// The rule that decided (`no_clients`, `clients_unpaired`, `paired`, ..).
    pub reason: &'static str,
    /// The deciding ratio, when a ratio decided.
    pub reach: Option<f64>,
    /// The measure in words: `clients paired 7% of 14`.
    pub measure: String,
}

/// The `parse` row: code-looking files no parser read, and prose apart.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ParseHealth {
    /// Walked files a parser takes.
    pub parsed_files: usize,
    /// Code-looking files no parser takes, by extension (no dot).
    pub unparsed: BTreeMap<String, usize>,
    /// Prose files no parser takes, by extension; never a flag.
    pub docs_unparsed: BTreeMap<String, usize>,
    /// The sum of `unparsed`.
    pub unparsed_files: usize,
    /// The sum of `docs_unparsed`.
    pub docs_unparsed_files: usize,
    pub verdict: Verdict,
    /// `code_unparsed`, `code_partly_unparsed`, `minor_unparsed` or `parsed`.
    pub reason: &'static str,
    /// `415 code files unparsed (.svelte 415) of 2913`.
    pub measure: String,
}

/// One workspace package imported by name and bound to no in-repo module.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceImport {
    /// The manifest's package name.
    pub package: String,
    /// `npm`, `go` or `cargo`.
    pub ecosystem: String,
    /// The package's repo-relative directory.
    pub path: String,
    /// Importing MODULE files, sorted (repo-relative, as POSITION holds them).
    pub importers: Vec<String>,
}

/// Suspicious ENDPOINT / ROUTE qnames.
#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Suspicious {
    /// Distinct suspicious nodes.
    pub count: usize,
    /// The first [`SUSPICIOUS_EXAMPLES`] qnames, sorted.
    pub examples: Vec<String>,
}

/// `gaps`' dead_symbol count over the symbols it can flag.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct DeadShare {
    pub dead_symbols: usize,
    /// Distinct CLASS / FUNCTION / METHOD nodes.
    pub symbols: usize,
    /// `dead_symbols / symbols`; `None` with no symbol.
    pub share: Option<f64>,
}

/// The mechanisms and the role, from the graph alone ([`boundary_health`]).
#[derive(Serialize, Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Boundary {
    pub role: Role,
    /// [`Health::role_basis`].
    pub role_basis: Vec<String>,
    /// One row per [`MECHANISMS`] entry, in its order.
    pub mechanisms: Vec<MechanismHealth>,
}

/// [`health`]'s answer.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Health {
    /// The repo labels the answer covers, in root order.
    pub repos: Vec<String>,
    pub role: Role,
    /// What made the role `app`: the first [`ROLE_BASIS`] boundary nodes
    /// outside tests and examples, `<qname> (<file>)`, sorted; empty for a
    /// library.
    pub role_basis: Vec<String>,
    /// The worst row's verdict (mechanisms, then `parse`).
    pub verdict: Verdict,
    /// `<row>: <reason> (<measure>)` of the worst row; `ok` when all are OK.
    pub reason: String,
    pub mechanisms: Vec<MechanismHealth>,
    pub parse: ParseHealth,
    pub workspace_unresolved: Vec<WorkspaceImport>,
    pub suspicious: Suspicious,
    pub dead: DeadShare,
}

impl Health {
    /// The row of `mechanism` (`http`, `grpc`, ..), if [`MECHANISMS`] has it.
    pub fn mechanism(&self, mechanism: &str) -> Option<&MechanismHealth> {
        self.mechanisms.iter().find(|m| m.mechanism == mechanism)
    }

    /// The `[health]` marker line for `surface` (empty = `engine`).
    pub fn marker(&self, surface: &str) -> String {
        let label = if self.repos.is_empty() {
            "-".to_string()
        } else {
            self.repos.join("+")
        };
        let mechs: Vec<String> = self
            .mechanisms
            .iter()
            .map(|m| format!("{}={}:{}", m.mechanism, m.verdict.as_str(), ratio(m.reach)))
            .collect();
        let surface = if surface.is_empty() {
            "engine"
        } else {
            surface
        };
        format!(
            "[health] repo={label} role={} {} unparsed={} workspace_unresolved={} suspicious={} dead_share={} docs_unparsed={} parse={} verdict={} surface={surface}",
            self.role.as_str(),
            mechs.join(" "),
            self.parse.unparsed_files,
            self.workspace_unresolved.len(),
            self.suspicious.count,
            ratio(self.dead.share),
            self.parse.docs_unparsed_files,
            self.parse.verdict.as_str(),
            self.verdict.as_str(),
        )
    }
}

/// How [`health_with`] runs. Made with `Default` plus field assignment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HealthOptions {
    /// `RepoId.0` -> label (a build's `GenerateResult::repo_labels`). A root
    /// with no label is named by its directory's base name.
    pub labels: BTreeMap<u64, String>,
    /// The marker's `surface=` (`cli`, `py`); empty = `engine`.
    pub surface: &'static str,
}

/// The boundary health of `merged`. `roots` are `(RepoId.0, repo root)`
/// pairs (a build's `GenerateResult::repo_roots`); empty skips the walk, so
/// `parse` reads no file. Prints the `[health]` marker.
pub fn health(merged: &MergedGraph, roots: &[(u64, PathBuf)]) -> Health {
    health_with(merged, roots, &HealthOptions::default())
}

/// [`health`] with labels and a marker surface.
pub fn health_with(merged: &MergedGraph, roots: &[(u64, PathBuf)], opts: &HealthOptions) -> Health {
    let Boundary {
        role,
        role_basis,
        mechanisms,
    } = boundary_health(merged);
    let mut parse = ParseHealth::default();
    for (_, root) in roots {
        walk_root(root, &mut parse);
    }
    judge_parse(&mut parse);
    let repos = roots
        .iter()
        .map(|(id, root)| {
            opts.labels.get(id).cloned().unwrap_or_else(|| {
                root.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| root.to_string_lossy().to_string())
            })
        })
        .collect();
    let (verdict, reason) = overall(&mechanisms, &parse);
    let out = Health {
        repos,
        role,
        role_basis,
        verdict,
        reason,
        mechanisms,
        parse,
        workspace_unresolved: workspace_unresolved(merged),
        suspicious: suspicious(merged),
        dead: dead_share(merged),
    };
    eprintln!("{}", out.marker(opts.surface));
    out
}

/// The role and the per-mechanism rows of `merged`, from the graph alone
/// (no walk, no gaps report, no marker): what an answer that only needs a
/// mechanism's verdict reads.
pub fn boundary_health(merged: &MergedGraph) -> Boundary {
    let test_routes = crate::passes::test_route_ids(merged, &CodeBuildCtx::empty());
    let ws_served = glia_graph::WebSocketStackResolver::served_routes(&merged.graphs);
    let pair_cats: HashSet<EdgeCategoryId> = MECHANISMS.iter().map(|m| m.pair).collect();
    let mut from: HashSet<(NodeId, EdgeCategoryId)> = HashSet::new();
    let mut to: HashSet<(NodeId, EdgeCategoryId)> = HashSet::new();
    let mut pairs: HashMap<EdgeCategoryId, usize> = HashMap::new();
    for e in merged.all_edges() {
        if pair_cats.contains(&e.category) {
            from.insert((e.from, e.category));
            to.insert((e.to, e.category));
            *pairs.entry(e.category).or_default() += 1;
        }
    }
    // A ROUTE a WS client reaches through WS_CONNECTS counts as reached.
    let ws_into: HashSet<NodeId> = to
        .iter()
        .filter(|(_, c)| *c == edge_category::WS_CONNECTS)
        .map(|(id, _)| *id)
        .collect();

    #[derive(Default)]
    struct Tally {
        producers: usize,
        external: usize,
        test_sites: usize,
        consumers: usize,
        paired: usize,
        reached: usize,
    }
    let mut tallies: Vec<Tally> = MECHANISMS.iter().map(|_| Tally::default()).collect();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut basis: BTreeSet<String> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if !seen.insert(n.id) {
                continue;
            }
            let provenance = origin_provenance(&n.cells);
            if (kind == node_kind::ROUTE
                || kind == node_kind::RPC_PROCEDURE
                || kind == node_kind::ENDPOINT)
                && !(kind == node_kind::ROUTE && test_routes.contains(&n.id))
                && provenance.as_deref() != Some("test_fixture")
            {
                let file = node_file(n);
                if !file.as_deref().is_some_and(in_example_tree) {
                    let qname = g.nav.qname_by_id.get(&n.id).map_or("?", String::as_str);
                    basis.insert(match file {
                        Some(f) => format!("{qname} ({f})"),
                        None => qname.to_string(),
                    });
                }
            }
            for (m, t) in MECHANISMS.iter().zip(tallies.iter_mut()) {
                if kind == m.producer {
                    match provenance.as_deref() {
                        Some("external") => t.external += 1,
                        Some("test_fixture") => t.test_sites += 1,
                        _ => {
                            t.producers += 1;
                            t.paired += usize::from(from.contains(&(n.id, m.pair)));
                        }
                    }
                }
                if kind == m.consumer {
                    if m.consumer == node_kind::ROUTE
                        && (is_nav_route(&n.cells) || test_routes.contains(&n.id))
                    {
                        continue;
                    }
                    t.consumers += 1;
                    let reached = to.contains(&(n.id, m.pair))
                        || (m.consumer == node_kind::ROUTE
                            && (ws_served.contains(&n.id) || ws_into.contains(&n.id)));
                    t.reached += usize::from(reached);
                }
            }
        }
    }
    let role = if basis.is_empty() {
        Role::Library
    } else {
        Role::App
    };
    let role_basis = basis.into_iter().take(ROLE_BASIS).collect();
    let mechanisms = MECHANISMS
        .iter()
        .zip(tallies)
        .map(|(m, t)| {
            let client_reach = share(t.paired, t.producers);
            let server_reach = share(t.reached, t.consumers);
            let mut row = MechanismHealth {
                mechanism: m.name,
                producers: t.producers,
                external_producers: t.external,
                test_site_producers: t.test_sites,
                consumers: t.consumers,
                pairs: pairs.get(&m.pair).copied().unwrap_or(0),
                paired_producers: t.paired,
                reached_consumers: t.reached,
                client_reach,
                server_reach,
                verdict: Verdict::Ok,
                reason: "",
                reach: None,
                measure: String::new(),
            };
            judge_mechanism(&mut row, role);
            row
        })
        .collect();
    Boundary {
        role,
        role_basis,
        mechanisms,
    }
}

/// `num / den`, `None` for an empty denominator.
fn share(num: usize, den: usize) -> Option<f64> {
    (den > 0).then(|| num as f64 / den as f64)
}

/// A ratio to three decimals, `-` for none.
fn ratio(r: Option<f64>) -> String {
    r.map_or_else(|| "-".to_string(), |x| format!("{x:.3}"))
}

/// A ratio as a whole percent.
fn pct(r: f64) -> String {
    format!("{}%", (r * 100.0).round() as i64)
}

/// The verdict rules of the module doc, first match wins.
fn judge_mechanism(row: &mut MechanismHealth, role: Role) {
    let (p, c) = (row.producers, row.consumers);
    let set = |row: &mut MechanismHealth, v, reason, reach, measure: String| {
        row.verdict = v;
        row.reason = reason;
        row.reach = reach;
        row.measure = measure;
    };
    if role == Role::Library {
        let m = format!("library: {p} clients, {c} servers");
        return set(row, Verdict::Ok, "library", None, m);
    }
    if p == 0 && c == 0 {
        return set(
            row,
            Verdict::Ok,
            "absent",
            None,
            "no clients or servers".into(),
        );
    }
    if c >= MIN_SIDE && (p as f64) < LOPSIDED * c as f64 {
        let m = format!("{p} clients for {c} servers");
        return set(row, Verdict::Blind, "no_clients", share(p, c), m);
    }
    if p >= MIN_SIDE && (c as f64) < LOPSIDED * p as f64 {
        let m = format!("{c} servers for {p} clients");
        return set(row, Verdict::Blind, "no_servers", share(c, p), m);
    }
    let clients = |r: f64| format!("clients paired {} of {p}", pct(r));
    let servers = |r: f64| format!("servers reached {} of {c}", pct(r));
    let cr = row.client_reach.filter(|_| p >= MIN_SIDE);
    let sr = row.server_reach.filter(|_| c >= MIN_SIDE);
    if let Some(r) = cr.filter(|r| *r < BLIND_REACH) {
        return set(row, Verdict::Blind, "clients_unpaired", Some(r), clients(r));
    }
    if let Some(r) = sr.filter(|r| *r < BLIND_REACH) {
        return set(
            row,
            Verdict::Blind,
            "servers_unreached",
            Some(r),
            servers(r),
        );
    }
    if let Some(r) = cr.filter(|r| *r < DEGRADED_REACH) {
        let m = clients(r);
        return set(row, Verdict::Degraded, "clients_partly_paired", Some(r), m);
    }
    if let Some(r) = sr.filter(|r| *r < DEGRADED_REACH) {
        let m = servers(r);
        return set(row, Verdict::Degraded, "servers_partly_reached", Some(r), m);
    }
    if cr.is_none() && sr.is_none() {
        let m = format!("{p} clients, {c} servers (under {MIN_SIDE} each)");
        return set(row, Verdict::Ok, "few", None, m);
    }
    let reach = match (cr, sr) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let mut parts = Vec::new();
    if let Some(r) = row.client_reach {
        parts.push(clients(r));
    }
    if let Some(r) = row.server_reach {
        parts.push(servers(r));
    }
    set(row, Verdict::Ok, "paired", reach, parts.join(", "));
}

/// The `parse` row's verdict from its counts.
fn judge_parse(parse: &mut ParseHealth) {
    parse.unparsed_files = parse.unparsed.values().sum();
    parse.docs_unparsed_files = parse.docs_unparsed.values().sum();
    let n = parse.unparsed_files;
    let total = n + parse.parsed_files;
    let by_ext: Vec<String> = parse
        .unparsed
        .iter()
        .map(|(e, k)| format!(".{e} {k}"))
        .collect();
    parse.measure = if n == 0 {
        format!("0 code files unparsed of {total}")
    } else {
        format!("{n} code files unparsed ({}) of {total}", by_ext.join(", "))
    };
    (parse.verdict, parse.reason) =
        if n >= MIN_SIDE && n as f64 >= UNPARSED_BLIND_SHARE * total as f64 {
            (Verdict::Blind, "code_unparsed")
        } else if n >= MIN_SIDE && n as f64 >= UNPARSED_DEGRADED_SHARE * total as f64 {
            (Verdict::Degraded, "code_partly_unparsed")
        } else if n > 0 {
            (Verdict::Ok, "minor_unparsed")
        } else {
            (Verdict::Ok, "parsed")
        };
}

/// The worst row: mechanisms in order, then `parse`; the first of the worst
/// verdict names the reason.
fn overall(mechanisms: &[MechanismHealth], parse: &ParseHealth) -> (Verdict, String) {
    let rows = mechanisms
        .iter()
        .map(|m| (m.verdict, m.mechanism, m.reason, m.measure.as_str()))
        .chain([(parse.verdict, "parse", parse.reason, parse.measure.as_str())]);
    let mut worst: Option<(Verdict, String)> = None;
    for (v, name, reason, measure) in rows {
        if v != Verdict::Ok && worst.as_ref().is_none_or(|(w, _)| v > *w) {
            worst = Some((v, format!("{name}: {reason} ({measure})")));
        }
    }
    worst.unwrap_or((Verdict::Ok, "ok".to_string()))
}

/// The `provenance` of a node's first ORIGIN cell that has one.
fn origin_provenance(cells: &[Cell]) -> Option<String> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .find_map(|c| json_str(c, "provenance"))
}

/// `payload[key]` of a JSON cell, as a string.
fn json_str(c: &Cell, key: &str) -> Option<String> {
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    v.get(key)?.as_str().map(str::to_string)
}

/// A repo-relative path under an example tree.
fn in_example_tree(file: &str) -> bool {
    let mut parts: Vec<&str> = file.split(['/', '\\']).collect();
    parts.pop();
    parts
        .iter()
        .any(|seg| EXAMPLE_DIRS.iter().any(|d| seg.eq_ignore_ascii_case(d)))
}

/// A file name's extension, lower-cased, without the dot.
fn ext_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Count `root`'s files into `parse`, gated as the build's walk gates them
/// (hard skips, collapsed regions, `.gitignore` layers, `[walk] skip`). A
/// symlink is not followed.
fn walk_root(root: &Path, parse: &mut ParseHealth) {
    let mut ignores = IgnoreStack::for_repo(root);
    let pushed = ignores.push_dir(root);
    walk_dir(root, root, &mut ignores, parse);
    if pushed {
        ignores.pop();
    }
}

fn walk_dir(root: &Path, dir: &Path, ignores: &mut IgnoreStack, parse: &mut ParseHealth) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_unstable_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            let gate = gate_dir(
                &path,
                &name,
                ignores.is_ignored(&path, true),
                ignores.is_excluded(&path, true),
            );
            if gate != Gate::Descend {
                continue;
            }
            let pushed = ignores.push_dir(&path);
            walk_dir(root, &path, ignores, parse);
            if pushed {
                ignores.pop();
            }
        } else if ft.is_file() {
            if ignores.exclude_file(&path) || ignores.skip_file(&path) {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
            if detect_language(&rel).is_some() {
                parse.parsed_files += 1;
                continue;
            }
            let Some(ext) = ext_of(&name) else {
                continue;
            };
            if CODE_LIKE_EXTS.contains(&ext.as_str()) {
                *parse.unparsed.entry(ext).or_default() += 1;
            } else if DOC_LIKE_EXTS.contains(&ext.as_str()) {
                *parse.docs_unparsed.entry(ext).or_default() += 1;
            }
        }
    }
}

/// The module doc's workspace-import rule.
fn workspace_unresolved(merged: &MergedGraph) -> Vec<WorkspaceImport> {
    // (name, ecosystem, path) of each non-root manifest-named sub-project.
    let mut packages: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut files: HashMap<NodeId, String> = HashMap::new();
    let mut importers: Vec<(NodeId, String, Vec<String>)> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if kind == node_kind::PROJECT {
                for c in n.cells.iter().filter(|c| c.kind == cell_type::ORIGIN) {
                    let get = |k| json_str(c, k).unwrap_or_default();
                    let (label, eco, path) = (get("label"), get("ecosystem"), get("path"));
                    if get("label_source") == LABEL_MANIFEST
                        && !path.is_empty()
                        && !label.is_empty()
                        && WORKSPACE_ECOSYSTEMS.contains(&eco.as_str())
                    {
                        packages.insert((label, eco, path));
                    }
                }
                continue;
            }
            if kind != node_kind::MODULE || !seen.insert(n.id) {
                continue;
            }
            let Some(file) = node_file(n) else {
                continue;
            };
            let libs: Vec<String> = n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::IMPORTS)
                .filter_map(|c| match &c.payload {
                    CellPayload::Json(s) | CellPayload::Text(s) => {
                        serde_json::from_str::<Vec<String>>(s).ok()
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            if !libs.is_empty() {
                importers.push((n.id, file.clone(), libs));
            }
            files.insert(n.id, file);
        }
    }
    if packages.is_empty() || importers.is_empty() {
        return Vec::new();
    }
    let mut imports_to: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for e in merged.all_edges() {
        if e.category == edge_category::IMPORTS {
            imports_to.entry(e.from).or_default().push(e.to);
        }
    }
    let mut out = Vec::new();
    for (name, eco, path) in packages {
        let dir = format!("{path}/");
        let mut hit: BTreeSet<String> = BTreeSet::new();
        for (id, file, libs) in &importers {
            if file.starts_with(&dir) {
                continue;
            }
            let names_it = libs.iter().any(|l| {
                l == &name
                    || l.strip_prefix(name.as_str())
                        .is_some_and(|r| r.starts_with('/'))
            });
            if !names_it {
                continue;
            }
            let bound = imports_to.get(id).is_some_and(|ts| {
                ts.iter()
                    .any(|t| files.get(t).is_some_and(|f| f.starts_with(&dir)))
            });
            if !bound {
                hit.insert(file.clone());
            }
        }
        if !hit.is_empty() {
            out.push(WorkspaceImport {
                package: name,
                ecosystem: eco,
                path,
                importers: hit.into_iter().collect(),
            });
        }
    }
    out
}

/// The path part of an ENDPOINT (`endpoint:<METHOD>:<path>`) or ROUTE qname.
fn http_path(kind: NodeKindId, qname: &str) -> Option<&str> {
    let (q, _owner) = split_owner(qname);
    if kind == node_kind::ENDPOINT {
        let rest = q.strip_prefix("endpoint:")?;
        return rest.split_once(':').map(|(_, p)| p);
    }
    nav_route_path(qname).or_else(|| q.split_once(' ').map(|(_, p)| p))
}

/// A path holding a `data:` / `blob:` / `mailto:` URL, `base64` or whitespace.
fn suspicious_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["data:", "blob:", "mailto:", "base64"]
        .iter()
        .any(|s| lower.contains(s))
        || path.contains(char::is_whitespace)
}

fn suspicious(merged: &MergedGraph) -> Suspicious {
    let mut qnames: BTreeSet<String> = BTreeSet::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if (kind != node_kind::ENDPOINT && kind != node_kind::ROUTE) || !seen.insert(n.id) {
                continue;
            }
            let Some(q) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            if http_path(kind, q).is_some_and(suspicious_path) {
                qnames.insert(q.clone());
            }
        }
    }
    Suspicious {
        count: qnames.len(),
        examples: qnames.into_iter().take(SUSPICIOUS_EXAMPLES).collect(),
    }
}

fn dead_share(merged: &MergedGraph) -> DeadShare {
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav
                .kind_by_id
                .get(&n.id)
                .is_some_and(|k| DEAD_SYMBOL_KINDS.contains(k))
            {
                seen.insert(n.id);
            }
        }
    }
    let opts = GapsOptions {
        top_k_per_category: Some(0),
        category: Some(DEAD_SYMBOL.to_string()),
        surface: "health",
    };
    let dead_symbols = gaps_report(merged, &[], &opts)
        .map(|r| r.count(DEAD_SYMBOL))
        .unwrap_or(0);
    DeadShare {
        dead_symbols,
        symbols: seen.len(),
        share: share(dead_symbols, seen.len()),
    }
}

// ---- FH.9: the blind caveat on answers ---------------------------------------

/// The [`MECHANISMS`] entry whose pair category is named `category` (an edge
/// category's registry name: `HTTP_CALLS` -> `http`), if any.
pub fn mechanism_of_category(category: &str) -> Option<&'static str> {
    MECHANISMS
        .iter()
        .find(|m| edge_category::name(m.pair) == category)
        .map(|m| m.name)
}

/// The [`MECHANISMS`] entry whose producer or consumer kind is named `kind`
/// (a node kind's registry name: `ENDPOINT` / `ROUTE` -> `http`), if any.
pub fn mechanism_of_kind(kind: &str) -> Option<&'static str> {
    MECHANISMS
        .iter()
        .find(|m| node_kind::name(m.producer) == kind || node_kind::name(m.consumer) == kind)
        .map(|m| m.name)
}

/// The mechanisms named by any of `categories` (edge category names) or
/// `kinds` (node kind names), each once, in [`MECHANISMS`] order: what an
/// answer that walked those edges and nodes read.
pub fn mechanisms_read<'a>(
    categories: impl IntoIterator<Item = &'a str>,
    kinds: impl IntoIterator<Item = &'a str>,
) -> Vec<&'static str> {
    let named: HashSet<&'static str> = categories
        .into_iter()
        .filter_map(mechanism_of_category)
        .chain(kinds.into_iter().filter_map(mechanism_of_kind))
        .collect();
    MECHANISMS
        .iter()
        .map(|m| m.name)
        .filter(|n| named.contains(n))
        .collect()
}

/// Every [`MECHANISMS`] name, in order: what an answer reading every
/// boundary (gaps, contract-breaks' orphan search) depends on.
pub fn all_mechanisms() -> Vec<&'static str> {
    MECHANISMS.iter().map(|m| m.name).collect()
}

/// The mechanisms a blast radius read (FH.9, also diff-impact's and
/// review's impact): a row put in scope by a mechanism's pair edge, or a
/// seed or row of a mechanism's producer / consumer kind (a radius that
/// stops at an unpaired ENDPOINT read HTTP as much as one that crossed it).
/// Reads the rows the answer holds, so a `top_k` cut row is not counted.
pub fn blast_mechanisms(b: &crate::answers::BlastRadius) -> Vec<&'static str> {
    mechanisms_read(
        b.results.iter().map(|r| r.reason),
        b.results
            .iter()
            .map(|r| r.kind)
            .chain(b.seeds.iter().map(|s| s.kind)),
    )
}

/// The rows of `rows` (a [`Boundary`]'s) whose mechanism is in `mechanisms`
/// and whose verdict is not OK, as caveats, in `rows`' order.
pub(crate) fn caveats_in(
    rows: &[MechanismHealth],
    mechanisms: &[&'static str],
) -> Vec<BlindCaveat> {
    rows.iter()
        .filter(|m| m.verdict != Verdict::Ok && mechanisms.contains(&m.mechanism))
        .map(|m| BlindCaveat {
            tier: Tier::Fact.as_str(),
            mechanism: m.mechanism,
            verdict: m.verdict,
            reason: m.reason,
            measure: m.measure.clone(),
        })
        .collect()
}

/// Prints the `[blind]` marker for one answer.
pub(crate) fn blind_marker(surface: &str, mechanisms: &[&'static str], caveats: &[BlindCaveat]) {
    let count = |v: Verdict| caveats.iter().filter(|c| c.verdict == v).count();
    let named = if mechanisms.is_empty() {
        "-".to_string()
    } else {
        mechanisms.join(",")
    };
    eprintln!(
        "[blind] surface={surface} mechanisms={named} blind={} degraded={}",
        count(Verdict::Blind),
        count(Verdict::Degraded)
    );
}

/// FH.9: the caveats of an answer on `surface` that read `mechanisms` over
/// `merged`: one [`BlindCaveat`] per mechanism [`boundary_health`] rates
/// BLIND or DEGRADED, in [`MECHANISMS`] order; empty when every one it read
/// is OK (and no health pass runs when it read none).
///
/// fired_on marker, once per call, on stderr:
/// `[blind] surface=<s> mechanisms=<m,..|-> blind=<b> degraded=<d>`.
pub fn blind_caveats(
    merged: &MergedGraph,
    surface: &str,
    mechanisms: &[&'static str],
) -> Vec<BlindCaveat> {
    let caveats = if mechanisms.is_empty() {
        Vec::new()
    } else {
        caveats_in(&boundary_health(merged).mechanisms, mechanisms)
    };
    blind_marker(surface, mechanisms, &caveats);
    caveats
}

/// The table form of `caveats`, one line (FH.9):
/// `blind: http (clients paired 7% of 14), queue (degraded: clients paired 40% of 10)`;
/// `None` when there is none.
pub fn blind_line(caveats: &[BlindCaveat]) -> Option<String> {
    if caveats.is_empty() {
        return None;
    }
    let parts: Vec<String> = caveats
        .iter()
        .map(|c| match c.verdict {
            Verdict::Degraded => format!("{} (degraded: {})", c.mechanism, c.measure),
            _ => format!("{} ({})", c.mechanism, c.measure),
        })
        .collect();
    Some(format!("blind: {}", parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(p: usize, c: usize, paired: usize, reached: usize) -> MechanismHealth {
        let mut r = MechanismHealth {
            mechanism: "http",
            producers: p,
            external_producers: 0,
            test_site_producers: 0,
            consumers: c,
            pairs: 0,
            paired_producers: paired,
            reached_consumers: reached,
            client_reach: share(paired, p),
            server_reach: share(reached, c),
            verdict: Verdict::Ok,
            reason: "",
            reach: None,
            measure: String::new(),
        };
        judge_mechanism(&mut r, Role::App);
        r
    }

    #[test]
    fn verdict_rules_in_order() {
        let r = row(14, 436, 5, 5);
        assert_eq!((r.verdict, r.reason), (Verdict::Blind, "no_clients"));
        assert_eq!(r.measure, "14 clients for 436 servers");
        let r = row(40, 2, 0, 0);
        assert_eq!((r.verdict, r.reason), (Verdict::Blind, "no_servers"));
        let r = row(14, 12, 1, 12);
        assert_eq!((r.verdict, r.reason), (Verdict::Blind, "clients_unpaired"));
        assert_eq!(r.measure, "clients paired 7% of 14");
        let r = row(12, 12, 12, 1);
        assert_eq!((r.verdict, r.reason), (Verdict::Blind, "servers_unreached"));
        let r = row(10, 10, 4, 10);
        assert_eq!(
            (r.verdict, r.reason),
            (Verdict::Degraded, "clients_partly_paired")
        );
        let r = row(10, 10, 10, 5);
        assert_eq!(
            (r.verdict, r.reason),
            (Verdict::Degraded, "servers_partly_reached")
        );
        let r = row(10, 12, 9, 12);
        assert_eq!(
            (r.verdict, r.reason, r.reach),
            (Verdict::Ok, "paired", Some(0.9))
        );
        let r = row(3, 9, 0, 0);
        assert_eq!((r.verdict, r.reason), (Verdict::Ok, "few"));
        let r = row(0, 0, 0, 0);
        assert_eq!((r.verdict, r.reason), (Verdict::Ok, "absent"));
        let mut lib = row(14, 436, 0, 0);
        judge_mechanism(&mut lib, Role::Library);
        assert_eq!((lib.verdict, lib.reason), (Verdict::Ok, "library"));
    }

    #[test]
    fn parse_row_judges_code_only() {
        let mut p = ParseHealth {
            parsed_files: 50,
            ..Default::default()
        };
        p.docs_unparsed.insert("mdx".into(), 3000);
        judge_parse(&mut p);
        assert_eq!((p.verdict, p.reason), (Verdict::Ok, "parsed"));
        assert_eq!(p.docs_unparsed_files, 3000);
        p.unparsed.insert("svelte".into(), 9);
        judge_parse(&mut p);
        assert_eq!((p.verdict, p.reason), (Verdict::Ok, "minor_unparsed"));
        p.unparsed.insert("svelte".into(), 10);
        judge_parse(&mut p);
        assert_eq!((p.verdict, p.reason), (Verdict::Blind, "code_unparsed"));
        p.parsed_files = 300;
        judge_parse(&mut p);
        assert_eq!(
            (p.verdict, p.reason),
            (Verdict::Degraded, "code_partly_unparsed")
        );
        p.parsed_files = 5000;
        judge_parse(&mut p);
        assert_eq!((p.verdict, p.reason), (Verdict::Ok, "minor_unparsed"));
        p.parsed_files = 50;
        judge_parse(&mut p);
        assert_eq!((p.verdict, p.reason), (Verdict::Blind, "code_unparsed"));
        assert_eq!(p.measure, "10 code files unparsed (.svelte 10) of 60");
    }

    #[test]
    fn suspicious_paths_and_http_paths() {
        assert_eq!(
            http_path(node_kind::ENDPOINT, "endpoint:GET:/api/users @web"),
            Some("/api/users")
        );
        assert_eq!(http_path(node_kind::ROUTE, "POST /users"), Some("/users"));
        assert!(suspicious_path("data:image/png;base64,AAA"));
        assert!(suspicious_path("/a b"));
        assert!(suspicious_path("mailto:x@y"));
        assert!(!suspicious_path("/api/users/${id}"));
        assert!(in_example_tree("examples/chat/main.go"));
        assert!(!in_example_tree("examples.go"));
        assert_eq!(ext_of("App.Svelte").as_deref(), Some("svelte"));
        assert_eq!(ext_of(".gitignore"), None);
    }

    #[test]
    fn overall_names_the_first_worst_row() {
        let rows = vec![row(10, 10, 4, 10), row(14, 12, 1, 12)];
        let (v, reason) = overall(&rows, &ParseHealth::default());
        assert_eq!(v, Verdict::Blind);
        assert_eq!(reason, "http: clients_unpaired (clients paired 7% of 14)");
    }

    #[test]
    fn extension_tables_are_sorted_and_unparsed() {
        for t in [&CODE_LIKE_EXTS[..], &DOC_LIKE_EXTS[..]] {
            assert!(t.windows(2).all(|w| w[0] < w[1]), "sorted, distinct");
            for e in t {
                assert_eq!(
                    detect_language(&format!("a.{e}")),
                    None,
                    ".{e} has a parser"
                );
            }
        }
    }
}
