//! The post-cache graft sequence of `build_graphs_for_repo`. Every pass here
//! reads something that is not a function of one file's content (the repo's
//! const table, the build's proto services, the whole repo's declarations), so
//! it runs on the router's output — cached parses included — and never inside
//! the per-file extractors. That keeps incremental == clean.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::glia_config::LoadedConfig;
use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::{
    CodeNav, FileParse, ImportTarget, LocalModuleIndex, NavFact, attach_imports_cell_filtered,
    cell_type, node_kind,
};
use glia_code_extractors::constants::ConstTable;
use glia_code_extractors::next_pages::{self, NextRoots, PageRouter};
use glia_code_extractors::{anchor, cron, eventbus, openapi_annot, queue_topic, queues, websocket};
use glia_core::{Cell, CellPayload, NodeId, NodeKindId, RepoId};
use glia_graph::rust_paths::RustCrate;

use super::angular_templates;
use super::assemble::FileConstTables;
use super::evm_abi;
use super::lang_build::TsAliasSet;
use super::rpc_needles::{RpcContext, RpcNeedleCounts, apply_rpc_needles};
use crate::arch::node_file;
use crate::endpoint_fold;
use crate::external::{WrapperPass, WrapperPhase, infer_entity_wrappers};
use crate::extract::{detect_language, merge_nav};
use crate::http_owner;
use crate::route::ModuleQnames;

/// Run every post-cache graft over one repo's parses, in order: the LF.2e
/// overlay wrapper scan and its http half, the DT.10g cross-file EVM ABI join
/// (`evm_abi`), the A11.2 endpoint fold, the LA.6d
/// Next.js page graft, the LA.4 queue-topic const fold, the CB.3b event-name
/// const fold, the DT.3b WebSocket client base fold, the DF.13 Python signal
/// binding, the DG.7b akka-quartz conf
/// schedules, the LF.2e wrappers' queue half, the A5.2 / A5.3 / LA.17 RPC
/// needles with their `[grpc-client]` /
/// `[grpc-server-impl]` / `[proto-rpc]` markers, the A5.8 `[marker-anchor]`
/// census and the LE.4a `[data-access]` census, the LB.4a / LB.8 owner
/// segment, the CB.24 client-host stamp, the DF.19 annotated OpenAPI base
/// stamp, the DF.8 external client-site mark, the DT.5b Angular `templateUrl`
/// template bindings, then the A16.4 IMPORTS-cell filter. `const_table` is
/// the repo's A11.1 table and `file_tables` the per-file scans it was merged from (DC.3: the
/// queue / event const folds read a file's own bindings there instead of
/// re-scanning it); `roots` are the walk's project roots (A8.4);
/// `rust_crates` their Cargo packages, whose names the IMPORTS filter treats
/// as intra-repo (LA.1b), as it does every specifier a `ts_aliases` key
/// matches (A6.8).
/// `wrappers` is the repo's
/// `.glia/overlay.toml` when the build applies the overlay (`None` under
/// `--no-overlay`): its `[[wrapper]]` stanzas feed the LF.2e stage
/// (`external::WrapperPass`), after the CA.4 inferred Go collection wrappers
/// (`external::infer_entity_wrappers`), which run either way. Returns the
/// RPC needle pass's counts (`apply_rpc_needles`), whose `files` the
/// caller's `[parallel]` line reads (LG.1c).
///
/// ORDERING RULE (LB.8). The owner pass (`http_owner::qualify_repo`) is the
/// LAST step that may mint or re-key an owned kind: ROUTE / ENDPOINT / page
/// nodes and the channel sides (QUEUE_PRODUCER / QUEUE_CONSUMER, WS_HANDLER /
/// WS_CLIENT, GRAPHQL_RESOLVER / GRAPHQL_OPERATION, GRPC_CLIENT /
/// GRPC_SERVER). Any graft that mints one of those goes ABOVE it: a node
/// minted after it stays owner-free and pairs all-to-all with the qualified
/// ones. The `[http-owner]` / `[channel-owner]` counts make a miss visible.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_post_cache(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    rpc: &RpcContext,
    const_table: &ConstTable,
    file_tables: &FileConstTables,
    roots: &[ProjectRoot],
    rust_crates: &[RustCrate],
    ts_aliases: &TsAliasSet,
    wrappers: Option<&LoadedConfig>,
    parse_errors: &mut Vec<String>,
    repo_label: &str,
) -> RpcNeedleCounts {
    // LB.9b: the router's MODULE plan, recomputed from the same walked list
    // (a pure function of it), so every graft below finds a file's parse by
    // the MODULE id the router gave it.
    let modules = ModuleQnames::plan(files);
    // LF.2e: the overlay `[[wrapper]]` call sites, plus (CA.4) the Go
    // collection wrappers the code itself declares, inferred with or without
    // the overlay (an overlay stanza naming the same call shadows one). The
    // http half mints its ENDPOINTs BEFORE the endpoint fold, so a `${X}`
    // wrapper path folds through the const table like any other client call.
    let inferred = infer_entity_wrappers(parses_by_lang, files, &modules, repo, parse_errors);
    let mut wrappers = WrapperPass::scan(wrappers, inferred, files, parse_errors);
    if let Some(w) = wrappers.as_mut() {
        w.mint(WrapperPhase::Http, parses_by_lang, files, repo, &modules, parse_errors);
    }
    // DT.10g: an EVM client whose ABI another file declares (a TS import, a Go
    // constant of another file of the package) takes that file's entries, so
    // `resolver:evm` identifies it by ABI. Here, while the nav facts it reads
    // are present, and above every stamp that adds a key to a client hit.
    evm_abi::join_evm_abis(parses_by_lang).report(repo_label);
    // A11.2: re-key client ENDPOINTs whose base the table resolves and record
    // their authority. Post-cache, so cached parses are folded too and the
    // cache keeps the pre-fold parse. CH.5b: a call site read through a URL
    // builder that reads a configured API prefix moves under it; `roots` key
    // the builders by owner. Before nav is dropped (the builder facts live
    // only there) and above the owner pass (it re-keys ENDPOINTs).
    endpoint_fold::fold_repo(parses_by_lang.values_mut().flatten(), const_table, repo, roots)
        .report(repo_label);
    // LA.6d: Next.js file-system pages. Before the owner pass, so a grafted
    // page is owner-qualified like every other nav ROUTE.
    graft_next_pages(parses_by_lang, files, repo, &modules, parse_errors).report(repo_label);
    // LA.4 (A11.7): queue topics named by a constant. Same seam and the same
    // cache rule as the endpoint fold; runs before the A16.4 filter, which then
    // rewrites the folded nodes' IMPORTS cell like every other node's.
    let folds = FoldInputs { files, repo, modules: &modules, consts: const_table, file_tables };
    let (queue_stats, queue_pool) = apply_queue_const_topics(parses_by_lang, &folds, parse_errors);
    queue_stats.report(repo_label);
    // CB.3b: event sites keyed by a constant fold to its literal through the
    // same tables and the same cache rule. Above the owner pass, which
    // qualifies EVENT_EMITTER / EVENT_HANDLER like every owned kind, and
    // before the A16.4 filter, which rewrites the folded nodes' IMPORTS cell.
    let (event_stats, event_pool) = apply_event_const_names(parses_by_lang, &folds, parse_errors);
    event_stats.report(repo_label);
    report_const_fold_pool(queue_pool, event_pool, repo_label);
    // DT.3b: WebSocket clients whose URL base sits in the configuration fold
    // through the same tables and the same cache rule, as the A11.2 endpoint
    // fold does for HTTP clients. Above the owner pass (WS_CLIENT is an owned
    // kind), and above the CB.24 stamp and the DF.8 external mark, which read
    // the hosts it records.
    apply_ws_client_bases(parses_by_lang, &folds, parse_errors).report(repo_label);
    // DF.13: blinker / Django signal sites, named through the repo's signal
    // table. After the CB.3b fold, whose re-emit it extends (one helper,
    // `event_fold_one`, so the two cannot drift), and in the same seam: above
    // the owner pass and the A16.4 filter, so the swapped nodes are
    // owner-qualified and IMPORTS-filtered like every event node.
    apply_python_signals(parses_by_lang, &folds, roots, parse_errors).report(repo_label);
    // DG.7b: akka-quartz jobs started by a schedule name that a HOCON conf
    // declares, re-keyed to the conf's expression. Post-cache over `files`, so
    // a conf edit re-keys a cache-served job. CRON_JOB is no owned kind and no
    // needle input (CLASS / STRUCT / METHOD / FUNCTION), so neither the LB.8
    // rule nor the DC.4 NeedleKey is touched by where it runs.
    graft_quartz_conf_schedules(parses_by_lang, files, repo).report(repo_label);
    // LF.2e: the wrappers' queue half, AFTER the const fold (which rebuilds a
    // folded file's queue nodes from the queue scan and would drop a wrapper
    // node) and above the owner pass, like every graft that mints an owned kind.
    if let Some(w) = wrappers.as_mut() {
        w.mint(WrapperPhase::Queue, parses_by_lang, files, repo, &modules, parse_errors);
        w.report(repo_label);
    }

    let rpc_added = apply_rpc_needles(parses_by_lang, files, repo, &modules, rpc, parse_errors);
    // A5.2 fired_on marker, once per repo. Printed whenever the build knows a
    // proto service or this repo holds a client stub.
    let grpc_clients = parses_by_lang
        .values()
        .flatten()
        .flat_map(|fp| fp.nav.kind_by_id.values())
        .filter(|k| **k == node_kind::GRPC_CLIENT)
        .count();
    if grpc_clients > 0 || !rpc.services.is_empty() {
        eprintln!(
            "[grpc-client] {grpc_clients} stubs from {} known services (proto-needle +{}) repo={repo_label}",
            rpc.services.len(),
            rpc_added.clients
        );
    }
    // A5.3 per-repo extraction marker; the resolver's `[grpc-server]` line
    // reports the pairing.
    if rpc_added.servers > 0 {
        eprintln!(
            "[grpc-server-impl] {} server markers from {} known services repo={repo_label}",
            rpc_added.servers,
            rpc.services.len()
        );
    }
    // LA.17 fired_on marker, once per repo where the Connect / Twirp pass read
    // a registration or a client; `[trpc-link]` then reports the pairing.
    let p = rpc_added.proto_rpc;
    if p.any() {
        eprintln!(
            "[proto-rpc] connect procedures={} calls={} twirp procedures={} calls={} (ambiguous={} unowned={}) repo={repo_label}",
            p.connect_procedures,
            p.connect_calls,
            p.twirp_procedures,
            p.twirp_calls,
            p.ambiguous,
            p.unowned
        );
    }
    // A5.8 fired_on marker, once per repo that holds an RPC-family marker node:
    //   `[marker-anchor] {a} anchored to methods, {m} to module, {u} unanchored repo=<label>`
    // Counted off the finished parses, so cache-served files count too.
    let mut anchored = anchor::AnchorStats::default();
    for fp in parses_by_lang.values().flatten() {
        anchored.add(anchor::census(fp));
    }
    anchor::report(anchored, repo_label);
    // LE.4a fired_on marker, once per repo that holds a data-access edge:
    //   `[data-access] rehomed fn={F} module_kept={M} modes read={R} write={W} read_write={X} unknown={U} repo=<label>`
    // Counted off the finished parses (the re-home runs in the per-file
    // extractors and is cached with the parse), so cache hits count too.
    let mut access = anchor::AccessStats::default();
    for fp in parses_by_lang.values().flatten() {
        access.add(anchor::access_census(fp));
    }
    anchor::report_access(access, repo_label);
    // LE.4b fired_on marker, from the same census, once per repo that holds a
    // config-extractor env-read edge:
    //   `[config-read] rehomed fn={F} module_kept={M} repo=<label>`
    anchor::report_config_read(access, repo_label);
    // CC.7a fired_on marker, from the same census, once per repo that holds
    // a feature-flag check edge:
    //   `[flag-read] rehomed fn={F} module_kept={M} repo=<label>`
    anchor::report_flag_read(access, repo_label);

    // LB.4a / LB.8: qualify every owned node under a nested project root with
    // ` @<project path>` (see the ordering rule above). After the endpoint
    // fold, which keys endpoints by their owner-free path, the queue const
    // fold and the RPC needles, which mint queue nodes and GRPC_CLIENT /
    // GRPC_SERVER markers by owner-free name; per parse, so HashMap order
    // cannot reach the ids.
    let owners = http_owner::OwnerIndex::from_roots(roots);
    http_owner::qualify_repo(parses_by_lang.values_mut().flatten(), &owners, repo)
        .report(repo_label);

    // CB.24: spread each project's GraphQL / tRPC client base URL to its
    // GRAPHQL_OPERATION / RPC_CALL sides as an ENDPOINT_HIT `hosts`. After the
    // owner pass, whose owners group the facts and the sides; it mints and
    // re-keys nothing, so the ordering rule above holds.
    stamp_client_hosts(parses_by_lang, &owners).report(repo_label);

    // DF.19: a base-less OpenAPI op of a project whose Go code declares one
    // swaggo general-info `@BasePath` records that base, which the
    // contract-link pass then pairs it through (rule `annotated_base`).
    // Beside the CB.24 stamp and for the same reason: the owners group the
    // bases and the ops, and it rewrites ORIGIN payloads only, so the
    // ordering rule above holds.
    stamp_annotated_bases(parses_by_lang, files, &owners).report(repo_label);

    // DF.8: mark each WS_CLIENT / GRPC_CLIENT / GRAPHQL_OPERATION site hit
    // that dials a public host outside the repo's configuration, CG.4a's rule
    // for the channel clients. After the CB.24 stamp, which writes the GraphQL
    // `hosts` it reads; it rewrites payloads only, so the ordering rule holds.
    endpoint_fold::mark_external_clients(parses_by_lang.values_mut().flatten(), const_table)
        .report(repo_label);

    // DT.5b: a component's `templateUrl` template binds its METHODs (CALLS /
    // USES from the CLASS, EVIDENCE at the template's file and row). The
    // template is a second walked file, so the read is post-cache: an edit
    // to the template alone moves the edges of a cache-served component. It
    // mints no node, so the ordering rule holds, and it runs after the RPC
    // needles, so no DC.4 NeedleKey input moves.
    angular_templates::graft_template_urls(parses_by_lang, files, repo, &modules, parse_errors)
        .report(repo_label);

    // A16.4: drop intra-repo names from every IMPORTS cell. It needs the whole
    // repo's declarations, so like `apply_rpc_needles` it runs after the parse
    // cache (cached parses are filtered too) and after the RPC grafts (whose
    // markers carry the same cell).
    filter_imports_cells(parses_by_lang, rust_crates, ts_aliases, repo_label);

    rpc_added
}

/// DG.7b: per-repo tallies of the akka-quartz conf join.
#[derive(Debug, Default, PartialEq, Eq)]
struct QuartzConfStats {
    /// HOCON confs walked (`cron::is_hocon_conf_path`).
    confs: usize,
    /// Distinct schedule names the confs declare with a cron expression.
    schedules: usize,
    /// Conf REF jobs re-keyed to their schedule's expression.
    rekeyed: usize,
    /// ... dropped, no conf naming their schedule.
    unresolved: usize,
    /// Later conf definitions of a name that differ from the first one's.
    conflicts: usize,
}

impl QuartzConfStats {
    /// fired_on marker, once per repo with a HOCON conf or a conf REF:
    ///   `[cron] akka-quartz confs={f} schedules={s} rekeyed={r} unresolved={u} conflicts={c} repo=<label>`
    fn report(&self, repo_label: &str) {
        if self.confs + self.rekeyed + self.unresolved > 0 {
            eprintln!(
                "[cron] akka-quartz confs={} schedules={} rekeyed={} unresolved={} conflicts={} repo={repo_label}",
                self.confs, self.schedules, self.rekeyed, self.unresolved, self.conflicts
            );
        }
    }
}

/// DG.7b: akka-quartz-scheduler's config form. `.schedule("Nightly", ..)`
/// names a schedule whose expression lives in a HOCON file
/// (`akka.quartz.schedules.Nightly.expression`), so the per-file pass mints a
/// REF (`cron:@quartz-conf:Nightly:Nightly`) and this graft resolves it.
///
/// The table, schedule name -> expression, is read from every walked conf
/// (`cron::hocon_quartz_schedules`; the walk admits `cron::is_hocon_conf_path`
/// files and the router skips them) in walk (sorted path) order: the first
/// definition of a name wins and a differing later one counts as a conflict.
/// It is a BTreeMap and parses are visited in sorted language order, then walk
/// order, so no HashMap order reaches an id. Then `cron::rekey_quartz_refs`
/// re-keys each hit to `cron:<expression>:<name>` and drops each miss. Like
/// the const folds it runs on the router's output, cached parses included,
/// and the cache keeps the REF: a conf edit re-keys a cache-served job, so
/// incremental == clean.
fn graft_quartz_conf_schedules(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
) -> QuartzConfStats {
    let mut stats = QuartzConfStats::default();
    let mut table: BTreeMap<String, String> = BTreeMap::new();
    for (path, text) in files {
        if !cron::is_hocon_conf_path(path) {
            continue;
        }
        stats.confs += 1;
        for (name, expr, _line) in cron::hocon_quartz_schedules(text) {
            match table.get(&name) {
                None => {
                    table.insert(name, expr);
                }
                Some(first) if *first != expr => stats.conflicts += 1,
                Some(_) => {}
            }
        }
    }
    stats.schedules = table.len();
    let mut langs: Vec<&'static str> = parses_by_lang.keys().copied().collect();
    langs.sort_unstable();
    for lang in langs {
        for fp in parses_by_lang.get_mut(lang).into_iter().flatten() {
            let r = cron::rekey_quartz_refs(fp, &table, repo);
            stats.rekeyed += r.rekeyed;
            stats.unresolved += r.unresolved;
        }
    }
    stats
}

/// The client-host `via` (`NavFact::ClientHost`) and the side kind it
/// narrows (CB.24).
const CLIENT_HOST_SIDES: &[(&str, NodeKindId)] = &[
    ("graphql", node_kind::GRAPHQL_OPERATION),
    ("rpc", node_kind::RPC_CALL),
];

/// CB.24: per-repo tallies of the client-host graft.
#[derive(Debug, Default, PartialEq, Eq)]
struct ClientHostStats {
    /// `ClientHost` facts naming a GraphQL client's base URL.
    graphql: usize,
    /// ... naming a tRPC client's.
    rpc: usize,
    /// GRAPHQL_OPERATION / RPC_CALL ids stamped (once each).
    sides: usize,
}

impl ClientHostStats {
    /// fired_on marker, once per repo that holds a client-host fact:
    ///   `[client-hosts] graphql={g} rpc={r} sides={s} repo=<label>`
    /// Printed at `sides=0` too, so a client whose project holds no side is
    /// visible rather than silent.
    fn report(&self, repo_label: &str) {
        if self.graphql + self.rpc > 0 {
            eprintln!(
                "[client-hosts] graphql={} rpc={} sides={} repo={repo_label}",
                self.graphql, self.rpc, self.sides
            );
        }
    }
}

/// CB.24: a GraphQL operation or a tRPC call names only a field or a
/// procedure path; the service it reaches is decided by the client the
/// project builds once, usually in another file (`new ApolloClient({ uri:
/// "http://users-svc/graphql" })`, `httpBatchLink({ url })`). The extractors
/// record that base URL's authority as a `ClientHost` fact on the file's
/// MODULE (engine/src/extract.rs); this graft groups the facts by `(via,
/// owner)`, the owner being the longest nested project root enclosing the
/// MODULE's file (`None` outside every one, [`http_owner::OwnerIndex`]), and
/// gives every GRAPHQL_OPERATION (`via` graphql) / RPC_CALL (`via` rpc) of the
/// same owner ONE ENDPOINT_HIT `{"via":"graphql","hosts":["users-svc"]}`: the
/// A11.4 array shape `host::hit_hosts` reads, a single host included. The
/// graphql / rpc resolvers then narrow the side's same-key targets to the
/// project the host names, on positive evidence only.
///
/// One cell per node id per repo, on the first parse (language tags sorted,
/// then walk order) that holds it: parses of one id merge their cells in the
/// build, and a second identical cell would only bloat it. A side that
/// already carries an ENDPOINT_HIT is left alone, and so is every side of a
/// project with no fact (no narrowing: the pre-CB.24 behaviour). The side's
/// owner is read the owner pass's way, from its own file (POSITION) or its
/// parse's MODULE file, so it is the ` @owner` its qname carries.
///
/// Post-cache, so a cached parse is stamped too and the cache keeps the
/// unstamped one. Deterministic: hosts are a sorted set per `(via, owner)`, and
/// the stamp order does not depend on HashMap order.
fn stamp_client_hosts(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    owners: &http_owner::OwnerIndex,
) -> ClientHostStats {
    let mut stats = ClientHostStats::default();
    let mut hosts: BTreeMap<(&'static str, Option<String>), BTreeSet<String>> = BTreeMap::new();
    for fp in parses_by_lang.values().flatten() {
        if fp.nav.nav_facts.is_empty() {
            continue;
        }
        let mut read: HashSet<NodeId> = HashSet::new();
        for node in &fp.nodes {
            let Some(facts) = fp.nav.nav_facts.get(&node.id) else {
                continue;
            };
            if !read.insert(node.id) {
                continue;
            }
            let mut owner: Option<Option<String>> = None;
            for fact in facts {
                let NavFact::ClientHost { via, host, .. } = fact else {
                    continue;
                };
                let Some(&(via, kind)) = CLIENT_HOST_SIDES.iter().find(|(v, _)| v == via) else {
                    continue;
                };
                if kind == node_kind::GRAPHQL_OPERATION {
                    stats.graphql += 1;
                } else {
                    stats.rpc += 1;
                }
                let owner = owner
                    .get_or_insert_with(|| {
                        node_file(node)
                            .or_else(|| http_owner::module_file(fp))
                            .and_then(|f| owners.owner_of(&f).map(str::to_string))
                    })
                    .clone();
                hosts.entry((via, owner)).or_default().insert(host.clone());
            }
        }
    }
    if hosts.is_empty() {
        return stats;
    }
    let mut langs: Vec<&'static str> = parses_by_lang.keys().copied().collect();
    langs.sort_unstable();
    let mut done: HashSet<NodeId> = HashSet::new();
    for lang in langs {
        let Some(parses) = parses_by_lang.get_mut(lang) else {
            continue;
        };
        for fp in parses.iter_mut() {
            let mut parse_file: Option<Option<String>> = None;
            for i in 0..fp.nodes.len() {
                let node = &fp.nodes[i];
                let Some(kind) = fp.nav.kind_by_id.get(&node.id) else {
                    continue;
                };
                let Some(&(via, _)) = CLIENT_HOST_SIDES.iter().find(|(_, k)| k == kind) else {
                    continue;
                };
                if done.contains(&node.id) {
                    continue;
                }
                if node.cells.iter().any(|c| c.kind == cell_type::ENDPOINT_HIT) {
                    done.insert(node.id);
                    continue;
                }
                let file = node_file(node)
                    .or_else(|| parse_file.get_or_insert_with(|| http_owner::module_file(fp)).clone());
                let owner = file.and_then(|f| owners.owner_of(&f).map(str::to_string));
                let Some(set) = hosts.get(&(via, owner)) else {
                    continue;
                };
                let id = node.id;
                fp.nodes[i].cells.push(client_hosts_cell(via, set));
                done.insert(id);
                stats.sides += 1;
            }
        }
    }
    stats
}

/// DF.19: per-repo tallies of the annotated-base stamp.
#[derive(Debug, Default, PartialEq, Eq)]
struct AnnotatedBaseStats {
    /// Distinct OpenAPI op ids stamped with an `annotated_base`.
    ops: usize,
    /// Owners whose one general-info base stamped at least one op.
    bases: usize,
}

impl AnnotatedBaseStats {
    /// fired_on marker, once per repo where an op was stamped:
    ///   `[contract-base] annotated ops=<n> bases=<k> repo=<label>`
    fn report(&self, repo_label: &str) {
        if self.ops > 0 {
            eprintln!(
                "[contract-base] annotated ops={} bases={} repo={repo_label}",
                self.ops, self.bases
            );
        }
    }
}

/// DF.19: swaggo writes a Go service's base in code (`// @BasePath /api` in
/// the general-info block above `main`) and generates `docs/swagger.json`
/// from it with no `basePath` of its own, so every op path of that document
/// lacks the prefix the routes are mounted under. This graft gives such an
/// op the base its own project declares.
///
/// Bases: every walked `.go` file holding `@BasePath` is read with
/// `openapi_annot::swaggo_general_base` (the first comment run with
/// `@BasePath` and no `@Router`), grouped by owner (`owners`, the longest
/// nested project root enclosing the file; `None` outside every one). An
/// owner qualifies when its files name exactly ONE distinct base and that
/// base is not the root (`@BasePath /` folds to "" and counts as a base, so
/// a root block beside an `/api` block is two bases and stamps nothing):
/// two general-info blocks in one project are two APIs, and which one the
/// document came from is unknown. Several files repeating one base keep the
/// first `file:line` in walk order.
///
/// Ops: a DOC_SECTION whose ORIGIN says `provenance: contract`, `source:
/// openapi`, `path` == `raw_path` (the document joined no base) and carries
/// no `bases` (DF.18: a document listing two or more servers chose its own)
/// gains `"annotated_base":"/api","annotated_at":"<file>:<1-based line>"`,
/// appended to the payload so its key order is kept, when its POSITION file
/// is in a qualifying owner. Every parse holding the op id is stamped alike
/// (an op whose json and yaml twins share a qname is one node).
///
/// Post-cache, over `files`, so a `@BasePath` edit re-stamps a cached op
/// and incremental == clean. Order-free: each rewrite is a function of the
/// node and the owner table alone, both tables are BTreeMaps, and the
/// counts are set sizes.
fn stamp_annotated_bases(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    owners: &http_owner::OwnerIndex,
) -> AnnotatedBaseStats {
    let mut stats = AnnotatedBaseStats::default();
    // owner -> base -> the first `file:line` naming it, in walk order.
    let mut declared: BTreeMap<Option<String>, BTreeMap<String, String>> = BTreeMap::new();
    for (path, text) in files {
        if !path.ends_with(".go") || !text.contains("@BasePath") {
            continue;
        }
        let Some((base, line)) = openapi_annot::swaggo_general_base(text) else {
            continue;
        };
        declared
            .entry(owners.owner_of(path).map(str::to_string))
            .or_default()
            .entry(base)
            .or_insert_with(|| format!("{path}:{}", line + 1));
    }
    let annotated: BTreeMap<Option<String>, (String, String)> = declared
        .into_iter()
        .filter_map(|(owner, bases)| {
            let mut it = bases.into_iter();
            let one = it.next()?;
            (it.next().is_none() && !one.0.is_empty()).then_some((owner, one))
        })
        .collect();
    if annotated.is_empty() {
        return stats;
    }
    let mut stamped: HashSet<NodeId> = HashSet::new();
    let mut used: BTreeSet<Option<String>> = BTreeSet::new();
    for fp in parses_by_lang.values_mut().flatten() {
        let FileParse { nodes, nav, .. } = fp;
        for node in nodes.iter_mut() {
            if nav.kind_by_id.get(&node.id) != Some(&node_kind::DOC_SECTION) {
                continue;
            }
            let Some(owner) = node_file(node).map(|f| owners.owner_of(&f).map(str::to_string))
            else {
                continue;
            };
            let Some((base, at)) = annotated.get(&owner) else {
                continue;
            };
            let Some(json) = node.cells.iter_mut().find_map(|c| match &mut c.payload {
                CellPayload::Json(j) if c.kind == cell_type::ORIGIN => Some(j),
                _ => None,
            }) else {
                continue;
            };
            if !is_base_less_openapi_op(json) {
                continue;
            }
            let Some(with_base) = with_annotated_base(json, base, at) else {
                continue;
            };
            *json = with_base;
            stamped.insert(node.id);
            used.insert(owner);
        }
    }
    stats.ops = stamped.len();
    stats.bases = used.len();
    stats
}

/// DF.19: an OpenAPI op ORIGIN whose document joined no base onto its path
/// (`path` == `raw_path`), listed no second server (`bases`) and is not
/// stamped yet.
fn is_base_less_openapi_op(json: &str) -> bool {
    // Cheap reject: every non-OpenAPI ORIGIN (nav routes, annotations, ...).
    if !json.contains(r#""openapi""#) {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
    s("provenance") == Some("contract")
        && s("source") == Some("openapi")
        && s("path").is_some()
        && s("path") == s("raw_path")
        && v.get("bases").is_none()
        && v.get("annotated_base").is_none()
}

/// `{..}` -> `{..,"annotated_base":"<base>","annotated_at":"<at>"}`: the two
/// fields appended before the object's closing brace, so every existing key
/// keeps its place. `None` on a payload that is not a JSON object's text.
fn with_annotated_base(json: &str, base: &str, at: &str) -> Option<String> {
    let body = json.trim_end().strip_suffix('}')?;
    let quote = |s: &str| serde_json::to_string(s).ok();
    Some(format!(
        r#"{body},"annotated_base":{},"annotated_at":{}}}"#,
        quote(base)?,
        quote(at)?
    ))
}

/// The ENDPOINT_HIT CB.24 stamps on a side: `{"via":"<via>","hosts":[..]}`,
/// hosts in sorted order, each a JSON string.
fn client_hosts_cell(via: &str, hosts: &BTreeSet<String>) -> Cell {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""));
    let list: Vec<String> = hosts.iter().map(|h| quote(h)).collect();
    Cell {
        kind: cell_type::ENDPOINT_HIT,
        payload: CellPayload::Json(format!(r#"{{"via":{},"hosts":[{}]}}"#, quote(via), list.join(","))),
    }
}

/// LA.6d: per-repo tallies of the Next.js page graft.
#[derive(Debug, Default, PartialEq, Eq)]
struct NextPageStats {
    /// Project roots whose `package.json` declares `next`.
    roots: usize,
    /// Page files grafted (one nav ROUTE each).
    pages: usize,
    /// ... of which under `pages/` (or `src/pages/`).
    pages_dir: usize,
    /// ... of which under `app/` (or `src/app/`).
    app_dir: usize,
    /// ... of which end in a `[...x]` / `[[...x]]` catch-all.
    catchall: usize,
}

impl NextPageStats {
    /// fired_on marker, once per repo with a Next root:
    ///   `[nav-pages] next roots={n} pages={p} (pages_dir={a} app_dir={b} catchall={c}) repo=<label>`
    /// Printed at `pages=0` too, so a Next repo whose pages all missed is
    /// visible rather than silent.
    fn report(&self, repo_label: &str) {
        if self.roots > 0 {
            eprintln!(
                "[nav-pages] next roots={} pages={} (pages_dir={} app_dir={} catchall={}) repo={repo_label}",
                self.roots, self.pages, self.pages_dir, self.app_dir, self.catchall
            );
        }
    }
}

/// LA.6d: Next.js file-system routing. The file tree IS the route table, so
/// every page file under a Next root (`pages/**`, `app/**/page.*`, with or
/// without `src/`) gets one nav ROUTE `page:<path>` with a HANDLED_BY ref to
/// its default-exported component (`next_pages::graft_page`, built on LA.6b's
/// `emit_nav_routes`), grafted onto the file's own parse.
///
/// The gate is repo-level: the file's OWNING root (the longest enclosing
/// `package.json`, `next_pages::NextRoots`) must declare `next`. So a Vite /
/// CRA app's `src/pages/` folder is never a page table, even nested in a Next
/// monorepo. That fact depends on another file, so like
/// `apply_queue_const_topics` this runs post-cache over `files` (cache-served
/// parses included) and the cache keeps the page-free parse: adding or
/// dropping the `next` dependency can never leave a stale page behind.
///
/// The parse is found the `apply_rpc_needles` way: under the file's
/// `detect_language` key, the one whose first node is the file's MODULE (the
/// LB.9b plan's id, `modules`). No
/// match (the file failed to parse) means no module to hang the page on, and
/// the page is skipped. Deterministic: roots are sorted, `files` is
/// walk-sorted, one ROUTE per page file, no HashMap order reaches a parse.
fn graft_next_pages(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    modules: &ModuleQnames,
    parse_errors: &mut Vec<String>,
) -> NextPageStats {
    let mut stats = NextPageStats::default();
    let roots = NextRoots::from_files(files.iter().map(|(p, s)| (p.as_str(), s.as_str())));
    stats.roots = roots.next_count();
    if stats.roots == 0 {
        return stats;
    }
    for (path, source) in files {
        let Some(rel) = roots.rel_under_next_root(path) else {
            continue;
        };
        // Cheap path test first: only a page file is worth a parse lookup.
        if next_pages::page_route(rel).is_none() {
            continue;
        }
        let Some(lang) = detect_language(path) else {
            continue;
        };
        let Some(parses) = parses_by_lang.get_mut(lang) else {
            continue;
        };
        let module_id = modules.module_id(path, repo);
        let Some(fp) = parses
            .iter_mut()
            .find(|fp| fp.nodes.first().is_some_and(|n| n.id == module_id))
        else {
            continue;
        };
        let graft = catch_unwind(AssertUnwindSafe(|| {
            next_pages::graft_page(rel, source, module_id, repo)
        }));
        let page = match graft {
            Ok(Some(page)) => page,
            Ok(None) => continue,
            Err(_) => {
                parse_errors.push(format!("{path}: PANIC (next pages)"));
                continue;
            }
        };
        stats.pages += 1;
        match page.route.router {
            PageRouter::Pages => stats.pages_dir += 1,
            PageRouter::App => stats.app_dir += 1,
        }
        stats.catchall += usize::from(page.route.catchall);
        // No IMPORTS cell here: `filter_imports_cells` (last in
        // `apply_post_cache`) gives every node of this parse the file's
        // filtered cell, after ROUTE_METHOD + ORIGIN, the same cell order a
        // table-declared nav ROUTE ends with.
        fp.nodes.extend(page.out.nodes);
        merge_nav(&mut fp.nav, page.out.nav);
        fp.refs.extend(page.out.refs);
    }
    stats
}

/// LA.4 (A11.7): per-repo tallies of the queue-topic const fold.
#[derive(Debug, Default, PartialEq, Eq)]
struct QueueConstStats {
    /// Files whose queue nodes were swapped (at least one site folded).
    files: usize,
    /// Topic sites an identifier named and the const tables resolved.
    folded: usize,
    /// Topic sites an identifier named and nothing resolved.
    unresolved: usize,
}

impl QueueConstStats {
    /// fired_on marker, once per repo where any queue topic slot held an
    /// identifier:
    ///   `[queue-const] folded {n} topic sites in {f} files (unresolved={u}) repo=<label>`
    fn report(&self, repo_label: &str) {
        if self.folded + self.unresolved > 0 {
            eprintln!(
                "[queue-const] folded {} topic sites in {} files (unresolved={}) repo={repo_label}",
                self.folded, self.files, self.unresolved
            );
        }
    }
}

/// LA.4 (A11.7): fold queue topics named by a constant through the const
/// tables, before the unresolved-sentinel fallback has the last word.
///
/// The per-file queue extractor records the identifier in a topic slot
/// (`queue_topic::TopicHit::expr`) but cannot resolve it: a lookup reads other
/// files, and the extractor's output is cached by the file's own hash (the
/// constants.rs cache rule). So this pass re-runs the text-only queue scan —
/// no tree-sitter — with a resolver, for the files whose parse already holds a
/// queue node, cached parses included; the cache keeps the pre-fold parse.
///
/// Resolution is strict. A SAME-FILE binding resolves at any shape (Go
/// `const SubjectOrders`, a TS class field read as `this.topic`); a binding
/// in another file only when the expression is constant-shaped
/// (`ConstTable::resolve_identity`); an ambiguous key never. A wrong topic
/// manufactures a false cross-service QUEUE_FLOWS edge; no topic does not.
///
/// DC.3: the same-file bindings are the file's own A11.1 scan
/// ([`FileConstTables`]), never a second scan; the targets are chosen in
/// file order ([`fold_targets`]), each fold is computed on the engine pool
/// ([`crate::parallel::par_map_ordered`], read-only on the parses), then the
/// stats, the evidence stamp, the swap and any error line are applied in
/// target order, which is file order: the inline loop's results, byte for
/// byte. Returns the stats and the pool the folds ran on.
///
/// A file's queue nodes are swapped (`queues::replace_queue_nodes`) only when
/// at least one site folded, so every other file is untouched.
fn apply_queue_const_topics(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    inputs: &FoldInputs<'_>,
    parse_errors: &mut Vec<String>,
) -> (QueueConstStats, FoldPool) {
    let mut stats = QueueConstStats::default();
    let targets = fold_targets(parses_by_lang, inputs, is_queue_node, queue_position);
    if targets.is_empty() {
        return (stats, FoldPool::default());
    }
    let (outs, threads) = crate::parallel::par_map_ordered(&targets, |t| {
        fold_one(inputs, t, |source, path, resolve| {
            let (module, repo) = (t.module_id, inputs.repo);
            queues::extract_queue_nodes_with_consts(source, path, module, repo, resolve)
        })
    });
    for (t, out) in targets.iter().zip(outs) {
        let Some(mut fold) = out else {
            parse_errors.push(format!("{}: PANIC (queue const fold)", inputs.path(t)));
            continue;
        };
        stats.folded += fold.counts.folded;
        stats.unresolved += fold.counts.unresolved;
        if fold.counts.folded == 0 {
            continue;
        }
        stats.files += 1;
        // LC.3a: the re-emitted `module -> queue node` CONTAINS edges replace
        // ones the queue extractor stamped in the parse closure; these land
        // post-cache, unstamped.
        let ev = Evidence::emitter("extractor:queues").rule("const_fold");
        evidence::stamp_missing_with(&mut fold.consumers.edges, &ev);
        evidence::stamp_missing_with(&mut fold.producers.edges, &ev);
        if let Some(fp) = parses_by_lang.get_mut(t.lang).and_then(|p| p.get_mut(t.slot)) {
            queues::replace_queue_nodes(fp, t.module_id, t.lang, fold);
        }
    }
    (stats, FoldPool { files: targets.len(), threads })
}

/// DC.3: what both const folds read besides the parses they swap.
struct FoldInputs<'a> {
    /// The walked files, walk-sorted.
    files: &'a [(String, String)],
    repo: RepoId,
    /// The router's LB.9b MODULE plan.
    modules: &'a ModuleQnames,
    /// The repo's A11.1 table, LF.2d pins included.
    consts: &'a ConstTable,
    /// The per-file scans `consts` was merged from, aligned with `files`.
    file_tables: &'a FileConstTables,
}

impl FoldInputs<'_> {
    /// The path of `t`'s file, for its error line.
    fn path(&self, t: &FoldTarget) -> &str {
        self.files.get(t.file).map_or("", |(path, _)| path.as_str())
    }
}

/// DC.3: one file a const fold runs on: `files[file]`, its parse language,
/// its MODULE id (the folded nodes' parent) and `slot`, the index in
/// `parses_by_lang[lang]` of the parse the fold swaps.
struct FoldTarget {
    file: usize,
    lang: &'static str,
    module_id: NodeId,
    slot: usize,
}

/// DC.3: the pool one const fold ran on: the files it folded (its targets)
/// and the pool's thread count.
#[derive(Clone, Copy, Default)]
struct FoldPool {
    files: usize,
    threads: usize,
}

/// DC.3 fired_on marker, once per repo where either const fold had a target:
///   `[const-fold] pooled queue_files=<q> event_files=<e> threads=<t> repo=<label>`
/// `q` / `e` count the files the LA.4 queue / CB.3b event fold ran on, each
/// fold over the file's own A11.1 scan, `t` the engine pool's thread count.
/// After the `[queue-const]` / `[event-const]` lines, which are unchanged.
fn report_const_fold_pool(queue: FoldPool, event: FoldPool, repo_label: &str) {
    if queue.files + event.files > 0 {
        eprintln!(
            "[const-fold] pooled queue_files={} event_files={} threads={} repo={repo_label}",
            queue.files,
            event.files,
            queue.threads.max(event.threads)
        );
    }
}

/// DC.3: the files a const fold runs on, in file (walk) order — the selection
/// the LA.4 / CB.3b inline loops made, read-only.
///
/// A parse holds a fold's markers when one of its `is_marker` nodes has a
/// parent: the MODULE id the router handed the extractors, whatever the
/// language parser emitted first. Holders are indexed by (language, module),
/// in parse order. A walked file is a target when its (`detect_language`,
/// LB.9b module id) has holders and one of them carries a marker node whose
/// POSITION cell opens with `position(path)`: `a.ts` and `a.js` share a module
/// id, and a node's POSITION names the file it was read from, so a fold never
/// lands on a sibling. A parse holds one file's markers and a swap re-reads
/// that same file, so choosing every target before any swap is the inline
/// loop's choice. The index is only looked up, never iterated into the
/// output: no HashMap order reaches a target.
fn fold_targets(
    parses_by_lang: &HashMap<&'static str, Vec<FileParse>>,
    inputs: &FoldInputs<'_>,
    is_marker: fn(&CodeNav, NodeId) -> bool,
    position: fn(&str) -> Option<String>,
) -> Vec<FoldTarget> {
    let mut holders: HashMap<(&'static str, NodeId), Vec<usize>> = HashMap::new();
    for (lang, parses) in parses_by_lang {
        for (i, fp) in parses.iter().enumerate() {
            let module = fp
                .nav
                .kind_by_id
                .keys()
                .find(|id| is_marker(&fp.nav, **id))
                .and_then(|id| fp.nav.parent_of.get(id));
            if let Some(module) = module {
                holders.entry((*lang, *module)).or_default().push(i);
            }
        }
    }
    if holders.is_empty() {
        return Vec::new();
    }
    let mut targets = Vec::new();
    for (file, (path, _)) in inputs.files.iter().enumerate() {
        let Some(lang) = detect_language(path) else {
            continue;
        };
        let module_id = inputs.modules.module_id(path, inputs.repo);
        let Some(candidates) = holders.get(&(lang, module_id)) else {
            continue;
        };
        let (Some(parses), Some(position)) = (parses_by_lang.get(lang), position(path)) else {
            continue;
        };
        let Some(&slot) = candidates.iter().find(|&&i| {
            parses
                .get(i)
                .is_some_and(|fp| nodes_read_from(fp, &position, is_marker))
        }) else {
            continue;
        };
        targets.push(FoldTarget { file, lang, module_id, slot });
    }
    targets
}

/// DC.3: one const fold's pure half, run on a pool worker: `extract` re-reads
/// `files[t.file]` through the LA.4 resolver — the file's own scan
/// ([`FileConstTables`]) at any shape, then the repo table's
/// `ConstTable::resolve_identity`. `None` is the fold's panic: the file's
/// const scan panicked (its table is `None`, so the re-scan the fold used to
/// run would have panicked too) or `extract` did.
fn fold_one<R>(
    inputs: &FoldInputs<'_>,
    t: &FoldTarget,
    extract: impl FnOnce(&str, &str, &dyn Fn(&str) -> Option<String>) -> R,
) -> Option<R> {
    let (path, source) = inputs.files.get(t.file)?;
    let local = inputs.file_tables.get(t.file)?;
    let consts = inputs.consts;
    crate::parallel::quiet(|| {
        let resolve = |expr: &str| {
            local
                .resolve_expr_strict(expr)
                .or_else(|| consts.resolve_identity(expr))
                .map(str::to_string)
        };
        extract(source, path, &resolve)
    })
    .ok()
}

/// LA.4: the POSITION prefix of a queue node read from `path`.
fn queue_position(path: &str) -> Option<String> {
    Some(format!(r#"{{"file":"{}","#, queue_topic::escape_json(path)))
}

/// CB.3b: the POSITION prefix of an event node read from `path`
/// (`anchor::position_cell` JSON-encodes the file).
fn event_position(path: &str) -> Option<String> {
    let file = serde_json::to_string(path).ok()?;
    Some(format!(r#"{{"file":{file},"#))
}

/// Does one of `fp`'s marker nodes (those `is_marker` selects) carry a
/// POSITION cell opening with `position` (`{"file":"<escaped path>",`)?
fn nodes_read_from(
    fp: &FileParse,
    position: &str,
    is_marker: impl Fn(&CodeNav, NodeId) -> bool,
) -> bool {
    fp.nodes.iter().any(|n| {
        is_marker(&fp.nav, n.id)
            && n.cells.iter().any(|c| {
                c.kind == cell_type::POSITION
                    && matches!(&c.payload, CellPayload::Json(p) if p.starts_with(position))
            })
    })
}

/// LA.4: a QUEUE_PRODUCER / QUEUE_CONSUMER node.
fn is_queue_node(nav: &CodeNav, id: NodeId) -> bool {
    nav.kind_by_id
        .get(&id)
        .is_some_and(|k| *k == node_kind::QUEUE_PRODUCER || *k == node_kind::QUEUE_CONSUMER)
}

/// CB.3b: an event site the eventbus extractor mints (`event_emit:` /
/// `event_handle:`), never a Solidity `event` declaration under its code qname.
fn is_event_site_node(nav: &CodeNav, id: NodeId) -> bool {
    match (nav.kind_by_id.get(&id), nav.qname_by_id.get(&id)) {
        (Some(k), Some(q)) => eventbus::is_event_site(*k, q),
        _ => false,
    }
}

/// CB.3b: per-repo tallies of the event-name const fold.
#[derive(Debug, Default, PartialEq, Eq)]
struct EventConstStats {
    /// Files whose event nodes were swapped (at least one site folded).
    files: usize,
    /// Event sites a constant keyed and the const tables resolved to a name.
    folded: usize,
    /// Event sites a constant keyed that kept the constant path.
    unresolved: usize,
}

impl EventConstStats {
    /// fired_on marker, once per repo where any event site read a constant:
    ///   `[event-const] folded {n} event sites in {f} files (unresolved={u}) repo=<label>`
    fn report(&self, repo_label: &str) {
        if self.folded + self.unresolved > 0 {
            eprintln!(
                "[event-const] folded {} event sites in {} files (unresolved={}) repo={repo_label}",
                self.folded, self.files, self.unresolved
            );
        }
    }
}

/// CB.3b (James 2026-09-30): an event keyed by a constant is keyed by the
/// constant's resolved literal, falling back to the constant path, so
/// `emit(OrderEvents.Created, ..)` pairs with `@OnEvent("order.created")`.
///
/// The per-file eventbus extractor names a constant-keyed site by its path
/// (CB.3a) but cannot resolve it: a lookup reads other files, and the
/// extractor's output is cached by the file's own hash (the constants.rs cache
/// rule). So this pass re-runs the text-only event scan — no tree-sitter —
/// with a resolver, for the files whose parse already holds an event site
/// (every constant-keyed site minted one), cached parses included; the cache
/// keeps CB.3a's pre-fold parse and the fold re-runs every build, so an edit
/// to the constant's file re-keys its users without re-parsing them.
///
/// The resolver is LA.4's, verbatim: a SAME-FILE binding at any shape, a
/// binding in another file only when the path is constant-shaped (dotted or
/// SCREAMING_CASE, `ConstTable::resolve_identity`), a `.glia/overlay.toml`
/// `[constants]` pin (already in `consts`), an ambiguous key never. A value
/// the LA.41 name rule rejects keeps the path. A wrong name manufactures a
/// false EVENT_FLOWS edge; the path does not.
///
/// DC.3: the same split as LA.4's fold — targets in file order
/// ([`fold_targets`]), each fold on the engine pool over the file's own
/// A11.1 scan ([`FileConstTables`]), then stats, swap and error lines in
/// target (= file) order. Returns the stats and the pool the folds ran on.
///
/// A file's event nodes are swapped (`eventbus::replace_event_nodes`) only
/// when at least one site folded.
fn apply_event_const_names(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    inputs: &FoldInputs<'_>,
    parse_errors: &mut Vec<String>,
) -> (EventConstStats, FoldPool) {
    let mut stats = EventConstStats::default();
    let targets = fold_targets(parses_by_lang, inputs, is_event_site_node, event_position);
    if targets.is_empty() {
        return (stats, FoldPool::default());
    }
    let (outs, threads) = crate::parallel::par_map_ordered(&targets, |t| {
        event_fold_one(inputs, t, |_, _, fold| fold)
    });
    for (t, out) in targets.iter().zip(outs) {
        let Some(fold) = out else {
            parse_errors.push(format!("{}: PANIC (event const fold)", inputs.path(t)));
            continue;
        };
        stats.folded += fold.counts.folded;
        stats.unresolved += fold.counts.unresolved;
        if fold.counts.folded == 0 {
            continue;
        }
        stats.files += 1;
        // Event nodes carry no edges of their own (their module and owner
        // edges are the anchor pass's), so there is no extractor edge to
        // stamp: the swap stamps what it re-anchors `extractor:anchor` rule
        // `const_fold`.
        if let Some(fp) = parses_by_lang.get_mut(t.lang).and_then(|p| p.get_mut(t.slot)) {
            eventbus::replace_event_nodes(fp, t.module_id, t.lang, fold);
        }
    }
    (stats, FoldPool { files: targets.len(), threads })
}

/// DT.3b: a WS_CLIENT node.
fn is_ws_client_node(nav: &CodeNav, id: NodeId) -> bool {
    nav.kind_by_id.get(&id) == Some(&node_kind::WS_CLIENT)
}

/// DT.3b: per-repo tallies of the WebSocket client base fold.
#[derive(Debug, Default, PartialEq, Eq)]
struct WsBaseStats {
    /// Files whose WS_CLIENT nodes were swapped (at least one site folded).
    files: usize,
    /// Client sites whose URL text a configured literal changed.
    folded: usize,
    /// Client sites whose member chain nothing bound (or whose bound value
    /// read no path), left on their per-file reading.
    unresolved: usize,
}

impl WsBaseStats {
    /// fired_on marker, once per repo where any client URL offered the fold a
    /// member chain:
    ///   `[ws-base] folded {n} client sites in {f} files (unresolved={u}) repo=<label>`
    fn report(&self, repo_label: &str) {
        if self.folded + self.unresolved > 0 {
            eprintln!(
                "[ws-base] folded {} client sites in {} files (unresolved={}) repo={repo_label}",
                self.folded, self.files, self.unresolved
            );
        }
    }
}

/// DT.3b (tweaks T3): a WebSocket client whose URL base sits in the
/// configuration is keyed by the configured URL's path, and records its host,
/// as the A11.2 endpoint fold keys an HTTP client: `new
/// WebSocket(environment.feedUrl)` with `feedUrl: 'ws://feed-svc:8080/rt/feed'`
/// mints `ws_client:/rt/feed` with ENDPOINT_HIT `"host":"feed-svc:8080"`, and
/// `` `${environment.wsBase}/prices` `` with a base `ws://feed-svc:8080/rt`
/// mints `ws_client:/rt/prices`, so the tail alone never pairs another route.
///
/// The per-file client scan reads such a URL's chain as one dynamic piece: a
/// lookup reads other files, and the scan's output is cached by the file's own
/// hash (the constants.rs cache rule). So this pass re-runs the text-only
/// client scan — no tree-sitter — with a resolver, for the files whose parse
/// already holds a WS_CLIENT (every client site minted one), cached parses
/// included; the cache keeps the pre-fold parse and the fold re-runs every
/// build, so a base edited in environment.ts re-keys a cache-served client.
///
/// The resolver is LA.4's ([`fold_one`]): a SAME-FILE binding at any shape,
/// a binding in another file only when the chain is constant-shaped (dotted or
/// SCREAMING_CASE, `ConstTable::resolve_identity`), a `.glia/overlay.toml`
/// `[constants]` pin (already in `consts`), an ambiguous key never: a base
/// bound per deployment (environment.ts and environment.prod.ts) keeps the
/// per-file reading. An env read never resolves.
///
/// DC.3's split: targets in file order ([`fold_targets`], WS anchors carry
/// `anchor::position_cell`'s POSITION, the CB.3b encoding), each fold on the
/// engine pool, then stats, swap and error lines in target (= file) order.
/// A file's client nodes are swapped (`websocket::replace_ws_client_nodes`)
/// only when at least one site folded, so every other file is untouched.
fn apply_ws_client_bases(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    inputs: &FoldInputs<'_>,
    parse_errors: &mut Vec<String>,
) -> WsBaseStats {
    let mut stats = WsBaseStats::default();
    let targets = fold_targets(parses_by_lang, inputs, is_ws_client_node, event_position);
    if targets.is_empty() {
        return stats;
    }
    let (outs, _threads) = crate::parallel::par_map_ordered(&targets, |t| {
        fold_one(inputs, t, |source, path, resolve| {
            let (module, repo) = (t.module_id, inputs.repo);
            websocket::extract_ws_client_nodes_with_consts(source, path, module, repo, resolve)
        })
    });
    for (t, out) in targets.iter().zip(outs) {
        let Some(fold) = out else {
            parse_errors.push(format!("{}: PANIC (ws base fold)", inputs.path(t)));
            continue;
        };
        stats.folded += fold.counts.folded;
        stats.unresolved += fold.counts.unresolved;
        if fold.counts.folded == 0 {
            continue;
        }
        stats.files += 1;
        // WS client nodes carry no edges of their own (their module and owner
        // edges are the anchor pass's), so there is no extractor edge to stamp:
        // the swap stamps what it re-anchors `extractor:anchor` rule
        // `const_fold`.
        if let Some(fp) = parses_by_lang.get_mut(t.lang).and_then(|p| p.get_mut(t.slot)) {
            websocket::replace_ws_client_nodes(fp, t.module_id, t.lang, fold);
        }
    }
    stats
}

/// CB.3b / DF.13: one file's event nodes re-emitted through the CB.3b
/// resolver ([`fold_one`]: the file's own A11.1 scan, then the repo table's
/// `ConstTable::resolve_identity`), handed to `then` with the file's source
/// and path. The one re-emit both event grafts run, so a file DF.13 swaps
/// gets exactly the nodes CB.3b would give it. `None` is a panic.
fn event_fold_one<R>(
    inputs: &FoldInputs<'_>,
    t: &FoldTarget,
    then: impl FnOnce(&str, &str, eventbus::EventFold) -> R,
) -> Option<R> {
    fold_one(inputs, t, |source, path, resolve| {
        let (module, repo) = (t.module_id, inputs.repo);
        let fold = eventbus::extract_event_nodes_with_consts(source, path, module, repo, resolve);
        then(source, path, fold)
    })
}

// ============================================================================
// DF.13: Python signals, the cross-file binding
// ============================================================================

/// DF.13: the emitter and rule the owner edges of a bound signal site carry,
/// in place of the `const_fold` rule the shared swap stamps.
const PY_SIGNAL_EMITTER: &str = "extractor:anchor";
const PY_SIGNAL_RULE: &str = "python_signal";

/// DF.13: per-repo tallies of the Python signal binding.
#[derive(Debug, Default, PartialEq, Eq)]
struct PySignalStats {
    /// Bindable declarations: distinct (declaring module, variable) pairs
    /// with one identity.
    decls: usize,
    /// Files whose event nodes were swapped (at least one bound site).
    files: usize,
    /// Bound sites that named an event and emit (`.send(` ...) ...
    emit: usize,
    /// ... and that handle (`.connect`, `@receiver(..)` ...).
    handle: usize,
    /// Names refused as ambiguous: a module declaring one variable with two
    /// identities, or a file binding one name to two signals.
    dropped: usize,
}

impl PySignalStats {
    /// fired_on marker, once per repo that declares a Python signal (or
    /// refused one):
    ///   `[event-py-signals] decls=<d> files=<f> emit=<e> handle=<h> dropped=<x> repo=<label>`
    /// Printed at `files=0` too, so declared signals no file binds are
    /// visible rather than silent.
    fn report(&self, repo_label: &str) {
        if self.decls + self.dropped > 0 {
            eprintln!(
                "[event-py-signals] decls={} files={} emit={} handle={} dropped={} repo={repo_label}",
                self.decls, self.files, self.emit, self.handle, self.dropped
            );
        }
    }
}

/// DF.13: a Python file's IMPORT qname, the name `import` / `from` statements
/// give it: [`crate::extract::path_to_qname`] (`shop/signals.py` ->
/// `shop::signals`), a package's `__init__.py` named by its package
/// (`shop/__init__.py` -> `shop`). The stem form, never LB.9b's file-name
/// form: an import names the stem.
fn py_import_qname(path: &str) -> String {
    let q = crate::extract::path_to_qname(path);
    if q == "__init__" {
        return String::new();
    }
    match q.strip_suffix("::__init__") {
        Some(pkg) => pkg.to_string(),
        None => q,
    }
}

/// DF.13: bind `name` to `identity` in `map`; a second, different identity
/// makes the name ambiguous (`None`). Returns true when this call made it so.
fn bind_once(map: &mut BTreeMap<String, Option<String>>, name: &str, identity: &str) -> bool {
    match map.get_mut(name) {
        None => {
            map.insert(name.to_string(), Some(identity.to_string()));
            false
        }
        Some(slot) => match slot {
            Some(had) if had != identity => {
                *slot = None;
                true
            }
            _ => false,
        },
    }
}

/// DF.13: the repo's Python signal declarations and the module names its
/// imports resolve against.
struct PySignalTable {
    /// Declaring module (import qname, [`py_import_qname`]) -> variable ->
    /// event identity (`PySignalDecl::event_identity` of the module's dotted
    /// path); `None` for a variable declared with two identities.
    by_module: BTreeMap<String, BTreeMap<String, Option<String>>>,
    /// Every Python file's import qname, so an import resolves to the module
    /// that EXISTS first on its search path, declaring or not.
    modules: BTreeSet<String>,
    /// The repo-relative dirs of the walk's python project roots (A8.4),
    /// the sys.path entries an absolute import is read against.
    roots: Vec<String>,
}

impl PySignalTable {
    /// Scan every walked `.py` file whose text can hold a declaration
    /// (`eventbus::python_signal_decls`), in walk order. A file whose scan
    /// panics declares nothing and gets a `PANIC (python signals)` line.
    fn build(
        files: &[(String, String)],
        roots: &[ProjectRoot],
        parse_errors: &mut Vec<String>,
    ) -> Self {
        let mut by_module: BTreeMap<String, BTreeMap<String, Option<String>>> = BTreeMap::new();
        let mut modules = BTreeSet::new();
        for (path, source) in files {
            if detect_language(path) != Some("python") {
                continue;
            }
            let key = py_import_qname(path);
            modules.insert(key.clone());
            if key.is_empty() || !(source.contains("signal(") || source.contains("Signal(")) {
                continue;
            }
            let Ok(decls) = crate::parallel::quiet(|| eventbus::python_signal_decls(source, path))
            else {
                parse_errors.push(format!("{path}: PANIC (python signals)"));
                continue;
            };
            let dotted = key.replace("::", ".");
            for decl in decls {
                let identity = decl.event_identity(&dotted);
                bind_once(by_module.entry(key.clone()).or_default(), &decl.name, &identity);
            }
        }
        let roots = roots
            .iter()
            .filter(|r| r.ecosystem == "python" && !r.rel_path.is_empty())
            .map(|r| r.rel_path.clone())
            .collect();
        PySignalTable { by_module, modules, roots }
    }

    /// (bindable declarations, refused ones).
    fn counts(&self) -> (usize, usize) {
        let all = self.by_module.values().flat_map(BTreeMap::values);
        all.fold((0, 0), |(ok, bad), v| match v {
            Some(_) => (ok + 1, bad),
            None => (ok, bad + 1),
        })
    }

    /// Does a module or a package named `q` exist (a file `q`, or a file
    /// under `q::`)?
    fn exists(&self, q: &str) -> bool {
        if self.modules.contains(q) {
            return true;
        }
        let prefix = format!("{q}::");
        self.modules
            .range(prefix.clone()..)
            .next()
            .is_some_and(|m| m.starts_with(&prefix))
    }

    /// The import qname `module` names from the file at `path` (import qname
    /// `importer`), by Python's rules. A relative import (`level` > 0) starts
    /// at the importer's package, `level` 1 (`shop/orders.py` -> `shop`; a
    /// package's own `__init__.py`, whose import qname is already the
    /// package, starts at itself), climbs one package per further dot, then
    /// appends `module`. An absolute one (`level` 0) is read against each
    /// python project root enclosing `path`, innermost first, then the repo
    /// root, and names the first base where the module exists. `None` when
    /// a relative import climbs past the repo root or an absolute one exists
    /// under no base.
    fn resolve(&self, path: &str, importer: &str, module: &str, level: u32) -> Option<String> {
        let dotted = module.replace('.', "::");
        if level > 0 {
            let mut parts: Vec<&str> = importer.split("::").filter(|p| !p.is_empty()).collect();
            let is_init = path.rsplit(['/', '\\']).next() == Some("__init__.py");
            let pop = level as usize - usize::from(is_init);
            if pop > parts.len() {
                return None;
            }
            parts.truncate(parts.len() - pop);
            if !dotted.is_empty() {
                parts.push(&dotted);
            }
            return Some(parts.join("::"));
        }
        let mut bases: Vec<&str> = self
            .roots
            .iter()
            .filter(|r| path.starts_with(&format!("{r}/")))
            .map(String::as_str)
            .collect();
        bases.sort_by_key(|r| std::cmp::Reverse(r.len()));
        bases.push("");
        bases.into_iter().find_map(|base| {
            let q = if base.is_empty() {
                dotted.clone()
            } else {
                format!("{}::{dotted}", base.replace('/', "::"))
            };
            self.exists(&q).then_some(q)
        })
    }

    /// The signals module `q` declares, when it declares any.
    fn declared(&self, q: &str) -> Option<&BTreeMap<String, Option<String>>> {
        self.by_module.get(q)
    }

    /// The names one Python file binds to a signal, with each one's identity
    /// (`None`: refused, two identities):
    /// - the names its own module declares;
    /// - `from <m> import <n> [as <a>]` where module `<m>` declares `<n>`
    ///   (bound as `<a>` or `<n>`), else where `<m>::<n>` is a module that
    ///   declares signals (`from shop import signals` binds
    ///   `signals.<name>`; Python's order: the attribute first, then the
    ///   submodule);
    /// - `import <m> [as <a>]` where `<m>` declares signals: `<m>.<name>`
    ///   (`<a>.<name>`).
    ///
    /// A name a module refused, or a module no search base holds, binds
    /// nothing. `dropped` counts the names this file made ambiguous.
    fn bound(
        &self,
        path: &str,
        fp: &FileParse,
        dropped: &mut usize,
    ) -> BTreeMap<String, Option<String>> {
        let importer = py_import_qname(path);
        let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut bind = |out: &mut BTreeMap<String, Option<String>>, name: &str, identity: &str| {
            *dropped += usize::from(bind_once(out, name, identity));
        };
        if let Some(own) = self.declared(&importer) {
            for (name, identity) in own {
                if let Some(identity) = identity {
                    bind(&mut out, name, identity);
                }
            }
        }
        for stmt in &fp.imports {
            match &stmt.target {
                ImportTarget::Symbol { module, name, alias, level } => {
                    let Some(target) = self.resolve(path, &importer, module, *level) else {
                        continue;
                    };
                    let local = alias.as_deref().unwrap_or(name);
                    if let Some(identity) = self.declared(&target).and_then(|d| d.get(name)) {
                        if let Some(identity) = identity {
                            bind(&mut out, local, identity);
                        }
                        continue;
                    }
                    let sub = if target.is_empty() {
                        name.clone()
                    } else {
                        format!("{target}::{name}")
                    };
                    for (sig, identity) in self.declared(&sub).into_iter().flatten() {
                        if let Some(identity) = identity {
                            bind(&mut out, &format!("{local}.{sig}"), identity);
                        }
                    }
                }
                ImportTarget::Module { path: dotted, alias } => {
                    let Some(target) = self.resolve(path, &importer, dotted, 0) else {
                        continue;
                    };
                    let local = alias.as_deref().unwrap_or(dotted);
                    for (sig, identity) in self.declared(&target).into_iter().flatten() {
                        if let Some(identity) = identity {
                            bind(&mut out, &format!("{local}.{sig}"), identity);
                        }
                    }
                }
            }
        }
        out
    }
}

/// DF.13: the fresh signal nodes of one file merged into its CB.3b re-emit:
/// each EVENT_EMITTER into `fold.emitters`, each EVENT_HANDLER into
/// `fold.handlers`, a node whose id the fold already holds (a literal
/// blinker site naming the same event) kept once, and every site's anchor
/// kept (`eventbus::replace_event_nodes` concatenates both sides' anchors,
/// and `anchor::attach` sorts them).
fn merge_signal_nodes(fold: &mut eventbus::EventFold, sig: eventbus::EventNodes) -> SignalMerge {
    let held: HashSet<NodeId> = fold
        .emitters
        .nodes
        .iter()
        .chain(&fold.handlers.nodes)
        .map(|n| n.id)
        .collect();
    let eventbus::EventNodes { nodes, nav, anchors } = sig;
    let mut minted = HashSet::new();
    for node in nodes {
        let id = node.id;
        if held.contains(&id) || !minted.insert(id) {
            continue;
        }
        let Some(&kind) = nav.kind_by_id.get(&id) else {
            continue;
        };
        let side = if kind == node_kind::EVENT_EMITTER {
            &mut fold.emitters
        } else {
            &mut fold.handlers
        };
        side.nodes.push(node);
        side.nav.kind_by_id.insert(id, kind);
        if let Some(name) = nav.name_by_id.get(&id) {
            side.nav.name_by_id.insert(id, name.clone());
        }
        if let Some(qname) = nav.qname_by_id.get(&id) {
            side.nav.qname_by_id.insert(id, qname.clone());
        }
        if let Some(&parent) = nav.parent_of.get(&id) {
            side.nav.parent_of.insert(id, parent);
            side.nav.children_of.entry(parent).or_default().push(id);
        }
    }
    let mut merge = SignalMerge { minted, emit: 0, handle: 0 };
    for a in &anchors {
        match nav.kind_by_id.get(&a.node) {
            Some(&k) if k == node_kind::EVENT_EMITTER => merge.emit += 1,
            Some(_) => merge.handle += 1,
            None => {}
        }
    }
    fold.handlers.anchors.extend(anchors);
    merge
}

/// DF.13: what [`merge_signal_nodes`] added to one file's re-emit.
struct SignalMerge {
    /// The ids only the binding minted (a membership set: nothing iterates
    /// it into the output).
    minted: HashSet<NodeId>,
    /// Bound sites that named an event, per side: `python_signal_nodes`
    /// anchors each one, and mints nothing for an identity the LA.41 name
    /// rule refuses.
    emit: usize,
    handle: usize,
}

/// DF.13 (blinker and Django signals pair across files). A signal's identity
/// is declared in one file (`order_placed = signal("order-placed")`,
/// `order_placed = Signal()`) and used in another (`from signals import
/// order_placed`, then `order_placed.send(..)`, `@order_placed.connect`,
/// `@receiver(order_placed)`). The per-file extractors are cached by the
/// file's own hash and cannot read the declaring file, so, like CB.3b, this
/// runs after the parse cache, text only:
///
/// 1. the repo's signal table ([`PySignalTable::build`]): DF.12's
///    `python_signal_decls` per `.py` file, each identity from
///    `PySignalDecl::event_identity` of the declaring module's dotted path
///    (`order-placed`; Django `shop.signals.order_placed`);
/// 2. per Python parse, in walk order, the names it binds
///    ([`PySignalTable::bound`]) and their sites (`python_signal_sites`; the
///    bound set is the only gate, so a `.send(` on anything else mints
///    nothing);
/// 3. a file with a bound site gets its CB.3b re-emit ([`event_fold_one`]:
///    its literal, constant-keyed and DF.12 literal blinker nodes exactly as
///    CB.3b would emit them) plus `python_signal_nodes` of its sites
///    ([`merge_signal_nodes`]), swapped in through
///    `eventbus::replace_event_nodes`.
///
/// The swap re-anchors every event node of the file and stamps the owner
/// edges it adds `const_fold`. An owner edge the file held before keeps the
/// evidence it had; a new one, a bound site's, is stamped
/// `extractor:anchor` rule `python_signal`. A bound node gets the file's
/// IMPORTS cell when the swap did not give it one (a file with no event
/// node before has none to inherit), after its POSITION, so the A16.4
/// filter rewrites it like every other node's. No ORIGIN: both libraries
/// are in-process buses (DF.12).
///
/// Determinism: the table is BTree-keyed and built in walk order, files are
/// visited in walk order and each swap is per parse; `parses_by_lang` is only
/// looked up. A re-export (`from app import signals` where
/// `app/__init__.py` re-exports another module's signal) binds only when the
/// import names the declaring module.
fn apply_python_signals(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    inputs: &FoldInputs<'_>,
    roots: &[ProjectRoot],
    parse_errors: &mut Vec<String>,
) -> PySignalStats {
    let mut stats = PySignalStats::default();
    let table = PySignalTable::build(inputs.files, roots, parse_errors);
    (stats.decls, stats.dropped) = table.counts();
    if stats.decls == 0 {
        return stats;
    }
    let slots: HashMap<NodeId, usize> = match parses_by_lang.get("python") {
        Some(parses) => {
            let mut slots = HashMap::new();
            for (slot, fp) in parses.iter().enumerate() {
                if let Some(first) = fp.nodes.first() {
                    slots.entry(first.id).or_insert(slot);
                }
            }
            slots
        }
        None => return stats,
    };
    for (file, (path, source)) in inputs.files.iter().enumerate() {
        if detect_language(path) != Some("python") {
            continue;
        }
        let module_id = inputs.modules.module_id(path, inputs.repo);
        let Some(&slot) = slots.get(&module_id) else {
            continue;
        };
        let Some(fp) = parses_by_lang.get("python").and_then(|p| p.get(slot)) else {
            continue;
        };
        let bound = table.bound(path, fp, &mut stats.dropped);
        let names: BTreeSet<String> = bound
            .iter()
            .filter_map(|(name, identity)| identity.as_ref().map(|_| name.clone()))
            .collect();
        if names.is_empty() {
            continue;
        }
        let Ok(found) = crate::parallel::quiet(|| eventbus::python_signal_sites(source, path, &names))
        else {
            parse_errors.push(format!("{path}: PANIC (python signals)"));
            continue;
        };
        let sites: Vec<(eventbus::PySignalSide, String, usize)> = found
            .into_iter()
            .filter_map(|s| {
                let identity = bound.get(&s.bound)?.clone()?;
                Some((s.side, identity, s.at))
            })
            .collect();
        if sites.is_empty() {
            continue;
        }
        let t = FoldTarget { file, lang: "python", module_id, slot };
        let out = event_fold_one(inputs, &t, |source, path, mut fold| {
            let sig = eventbus::python_signal_nodes(source, path, module_id, inputs.repo, &sites);
            let merge = merge_signal_nodes(&mut fold, sig);
            (fold, merge)
        });
        let Some((fold, merge)) = out else {
            parse_errors.push(format!("{path}: PANIC (python signals)"));
            continue;
        };
        if merge.emit + merge.handle == 0 {
            continue;
        }
        let Some(fp) = parses_by_lang.get_mut("python").and_then(|p| p.get_mut(slot)) else {
            continue;
        };
        stats.files += 1;
        stats.emit += merge.emit;
        stats.handle += merge.handle;
        swap_signal_nodes(fp, module_id, fold, &merge.minted);
    }
    stats
}

/// DF.13: [`eventbus::replace_event_nodes`] for one file, then the two
/// repairs a bound site needs (see [`apply_python_signals`]). The swap stamps
/// every owner edge it adds `extractor:anchor` rule `const_fold`: an owner
/// edge of an event site the file held before gets back the evidence it
/// had, and a new one (a bound site's) is stamped `python_signal`. Each node
/// in `minted` without an IMPORTS cell gets the file's raw cell, the one the
/// router gave every node of the parse.
fn swap_signal_nodes(
    fp: &mut FileParse,
    module_id: NodeId,
    fold: eventbus::EventFold,
    minted: &HashSet<NodeId>,
) {
    type EdgeKey = (NodeId, NodeId, glia_core::EdgeCategoryId);
    let sites_of = |fp: &FileParse| -> HashSet<NodeId> {
        fp.nav
            .kind_by_id
            .keys()
            .filter(|id| is_event_site_node(&fp.nav, **id))
            .copied()
            .collect()
    };
    let old_sites = sites_of(fp);
    let before: HashMap<EdgeKey, Evidence> = fp
        .edges
        .iter()
        .filter(|e| old_sites.contains(&e.from) || old_sites.contains(&e.to))
        .filter_map(|e| Some(((e.from, e.to, e.category), Evidence::of(e)?)))
        .collect();
    let imports = fp
        .nodes
        .iter()
        .flat_map(|n| n.cells.iter())
        .find(|c| c.kind == cell_type::IMPORTS)
        .cloned();
    eventbus::replace_event_nodes(fp, module_id, "python", fold);
    let sites = sites_of(fp);
    let bound = Evidence::emitter(PY_SIGNAL_EMITTER).rule(PY_SIGNAL_RULE);
    for e in fp.edges.iter_mut() {
        if !(sites.contains(&e.from) || sites.contains(&e.to)) {
            continue;
        }
        let Some(ev) = Evidence::of(e) else {
            continue;
        };
        if ev.emitter != PY_SIGNAL_EMITTER || ev.rule.as_deref() != Some("const_fold") {
            continue;
        }
        match before.get(&(e.from, e.to, e.category)) {
            Some(old) if *old != ev => evidence::attach(e, old.clone()),
            Some(_) => {}
            None => evidence::attach(e, bound.clone()),
        }
    }
    if let Some(cell) = imports {
        for n in fp.nodes.iter_mut().filter(|n| minted.contains(&n.id)) {
            if !n.cells.iter().any(|c| c.kind == cell_type::IMPORTS) {
                n.cells.push(cell.clone());
            }
        }
    }
}

/// A16.4 (audit 2026-06-10 #12): rewrite each file's IMPORTS cell without the
/// names that resolve inside this repo — a sibling module, the repo's own
/// package or namespace — which are not dependencies.
///
/// The router attaches the raw cell to every node of a language-parser parse,
/// and never to a synthetic one (yaml / proto / graphql / avro / …), so
/// "carries an IMPORTS cell" selects exactly the parses that take part. That
/// keeps synthetic parses out on both sides: their file-derived modules
/// (`config::logging`) must not shadow a real dependency, and their nodes must
/// not gain a cell they never had. Rewriting in place keeps the cell's slot, so
/// every node's cell order is unchanged.
///
/// The repo's own Cargo packages count as declared too (LA.1b): the Rust
/// parser emits raw `use` paths, so `use glia_engine::..` in a sibling
/// crate reaches the filter as `glia_engine`, which is not a dependency.
/// Only Rust crate names are seeded (`rust_crates` is the Cargo projects).
///
/// A TS-family specifier a tsconfig `paths` key matches (`@core/auth.service`
/// under `@core/*`) names an in-repo module too (A6.8): every key of
/// `ts_aliases` is declared (`LocalModuleIndex::add_alias_prefix`), and the
/// build's TS resolver turns the same specifier into an IMPORTS edge.
///
/// fired_on marker, once per repo that holds a language-parser parse:
///   `[imports] local-filter: kept {k}, dropped {d} intra-repo name(s) across {n} language group(s) repo=<label>`
/// `kept` / `dropped` sum the per-file library names; `n` counts lang tags.
fn filter_imports_cells(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    rust_crates: &[RustCrate],
    ts_aliases: &TsAliasSet,
    repo_label: &str,
) {
    let carries_imports = |fp: &FileParse| {
        fp.nodes
            .iter()
            .any(|n| n.cells.iter().any(|c| c.kind == cell_type::IMPORTS))
    };
    let mut local = LocalModuleIndex::default();
    for fp in parses_by_lang.values().flatten().filter(|fp| carries_imports(fp)) {
        local.add_parse(fp);
    }
    for c in rust_crates {
        local.add_local_crate(&c.name);
    }
    for key in ts_aliases.keys() {
        local.add_alias_prefix(key);
    }
    let (mut kept, mut dropped, mut groups) = (0usize, 0usize, 0usize);
    for (lang, parses) in parses_by_lang.iter_mut() {
        let mut filtered_any = false;
        for fp in parses.iter_mut().filter(|fp| carries_imports(fp)) {
            let (k, d) = attach_imports_cell_filtered(fp, lang, &local);
            kept += k;
            dropped += d;
            filtered_any = true;
        }
        groups += usize::from(filtered_any);
    }
    if groups > 0 {
        eprintln!(
            "[imports] local-filter: kept {kept}, dropped {dropped} intra-repo name(s) across {groups} language group(s) repo={repo_label}"
        );
    }
}

#[cfg(test)]
mod next_page_tests {
    use std::path::Path;

    use glia_code_domain::edge_category;
    use glia_graph::MergedGraph;

    use crate::build::{generate_one, generate_one_with_cache};
    use crate::cache::ParseCache;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Every `page:` ROUTE qname in the merged graph, sorted.
    fn pages(m: &MergedGraph) -> Vec<String> {
        let mut out: Vec<String> = m
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes.iter().filter_map(move |n| {
                    let q = g.nav.qname_by_id.get(&n.id)?;
                    q.starts_with("page:").then(|| q.clone())
                })
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// `(route qname, handler qname)` of every HANDLED_BY out of a `page:` ROUTE.
    fn handled_by(m: &MergedGraph) -> Vec<(String, String)> {
        let qname = |id| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
                .unwrap_or_default()
        };
        let mut out: Vec<(String, String)> = m
            .all_edges()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| (qname(e.from), qname(e.to)))
            .filter(|(f, _)| f.starts_with("page:"))
            .collect();
        out.sort();
        out
    }

    fn write_store(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
        glia_store::write_merged_sharded(m, dir).unwrap();
        let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().to_string(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    }

    const NEXT: &str = r#"{"name":"shop","dependencies":{"next":"14.2.0","react":"18.2.0"}}"#;
    const VITE: &str = r#"{"name":"spa","dependencies":{"react":"18.2.0","vite":"5.0.0"}}"#;
    const USER_PAGE: &str = "export default function UserPage() {\n  return <div />;\n}\n";

    #[test]
    fn next_pages_follow_package_json_under_a_warm_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(&repo, "package.json", NEXT);
        write(&repo, "pages/users/[id].tsx", USER_PAGE);
        let repo_s = repo.to_str().unwrap();

        let mut cache = ParseCache::new();
        let cold = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(pages(&cold.merged), vec!["page:/users/:id".to_string()]);
        assert_eq!(
            handled_by(&cold.merged),
            vec![("page:/users/:id".to_string(), "pages::users::[id]::UserPage".to_string())]
        );

        // Only package.json changes: the page file is served from the cache
        // and must lose the page its dependency no longer declares.
        write(&repo, "package.json", VITE);
        let warm = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1, "the page file must come from the cache");
        assert!(pages(&warm.merged).is_empty(), "stale Next page replayed from cache");
        let clean = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm.merged, &tmp.path().join("warm")),
            write_store(&clean.merged, &tmp.path().join("clean")),
            "incremental vs clean after dropping `next`"
        );

        // And back: the cached parse is a page again.
        write(&repo, "package.json", NEXT);
        let warm2 = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1);
        assert_eq!(pages(&warm2.merged), vec!["page:/users/:id".to_string()]);
        let clean2 = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm2.merged, &tmp.path().join("warm2")),
            write_store(&clean2.merged, &tmp.path().join("clean2")),
            "incremental vs clean with a Next page present"
        );
    }

    #[test]
    fn a_vite_app_nested_in_a_next_repo_keeps_its_pages_folder_out() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(&repo, "package.json", NEXT);
        write(&repo, "pages/index.tsx", "export default function Home() {\n  return <main />;\n}\n");
        write(&repo, "apps/spa/package.json", VITE);
        write(
            &repo,
            "apps/spa/src/pages/Settings.tsx",
            "export default function Settings() {\n  return <div />;\n}\n",
        );
        let built = generate_one(repo.to_str().unwrap()).unwrap();
        assert_eq!(pages(&built.merged), vec!["page:/".to_string()]);
    }

    #[test]
    fn a_nested_next_app_is_owner_qualified() {
        // The owner pass runs after the graft, so a page under a nested
        // project root carries the ` @<project>` owner like every other nav
        // ROUTE.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(&repo, "package.json", r#"{"name":"mono","private":true}"#);
        write(&repo, "apps/web/package.json", NEXT);
        write(&repo, "apps/web/app/orders/[id]/page.tsx", USER_PAGE);
        write(
            &repo,
            "apps/web/pages/index.tsx",
            "export default function Home() {\n  return <main />;\n}\n",
        );
        let built = generate_one(repo.to_str().unwrap()).unwrap();
        assert_eq!(
            pages(&built.merged),
            vec![
                "page:/ @apps/web".to_string(),
                "page:/orders/:id @apps/web".to_string()
            ]
        );
    }
}

#[cfg(test)]
mod client_host_tests {
    use std::path::Path;

    use glia_code_domain::{GRAPH_TYPE, edge_category};
    use glia_core::{Confidence, Node};
    use glia_graph::MergedGraph;

    use super::*;
    use crate::build::generate_one;

    const REPO: RepoId = RepoId(11);

    fn position(file: &str) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(r#"{{"file":"{file}","start_line":0,"end_line":1}}"#)),
        }
    }

    fn push(fp: &mut FileParse, kind: NodeKindId, q: &str, file: &str) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, REPO, kind, q);
        fp.nodes.push(Node { id, repo: REPO, confidence: Confidence::Strong, cells: vec![position(file)] });
        fp.nav.record(id, q, q, kind, None);
        id
    }

    /// A parse of `file`: its MODULE first, as the router hands it over.
    fn parse(file: &str) -> (FileParse, NodeId) {
        let mut fp = FileParse::default();
        let q = file.rsplit_once('.').map_or(file, |(stem, _)| stem).replace('/', "::");
        let m = push(&mut fp, node_kind::MODULE, &q, file);
        (fp, m)
    }

    fn fact(via: &str, host: &str, line: u32) -> NavFact {
        NavFact::ClientHost { via: via.into(), host: host.into(), line }
    }

    /// The ENDPOINT_HIT payloads of every copy of `id`, in parse order.
    fn hits(by_lang: &HashMap<&'static str, Vec<FileParse>>, id: NodeId) -> Vec<String> {
        let mut langs: Vec<&&str> = by_lang.keys().collect();
        langs.sort();
        langs
            .into_iter()
            .flat_map(|l| &by_lang[*l])
            .flat_map(|fp| fp.nodes.iter().filter(move |n| n.id == id))
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
            .collect()
    }

    /// CB.24: a client fact in apps/web/src/apollo.ts stamps apps/web's
    /// operations (once per id, on the first parse holding it) and not
    /// apps/admin's; a graphql fact never stamps an RPC_CALL; a tRPC fact
    /// stamps the project's calls. A second run stamps nothing.
    #[test]
    fn stamp_client_hosts_spreads_within_an_owner() {
        let owners = http_owner::OwnerIndex::from_roots(&[
            ProjectRoot::new(String::new(), "npm", "manifest", None),
            ProjectRoot::new("apps/web".into(), "npm", "manifest", None),
            ProjectRoot::new("apps/admin".into(), "npm", "manifest", None),
        ]);
        let (mut apollo, m) = parse("apps/web/src/apollo.ts");
        apollo.nav.record_fact(m, fact("graphql", "users-svc:4000", 5));
        apollo.nav.record_fact(m, fact("graphql", "users-svc", 2));
        apollo.nav.record_fact(m, fact("rpc", "catalog-svc", 7));
        let (mut profile, _) = parse("apps/web/src/profile.ts");
        let op = push(&mut profile, node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/web", "apps/web/src/profile.ts");
        let call = push(&mut profile, node_kind::RPC_CALL, "rpc_call:item.list @apps/web", "apps/web/src/profile.ts");
        let (mut other, _) = parse("apps/web/src/other.tsx");
        push(&mut other, node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/web", "apps/web/src/other.tsx");
        let (mut admin, _) = parse("apps/admin/src/admin.ts");
        let admin_op = push(&mut admin, node_kind::GRAPHQL_OPERATION, "graphql_op:getUser @apps/admin", "apps/admin/src/admin.ts");
        let mut by_lang: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        by_lang.insert("typescript", vec![apollo, profile, admin]);
        by_lang.insert("vue", vec![other]);

        let stats = stamp_client_hosts(&mut by_lang, &owners);
        assert_eq!(stats, ClientHostStats { graphql: 2, rpc: 1, sides: 2 });
        assert_eq!(hits(&by_lang, op), vec![r#"{"via":"graphql","hosts":["users-svc","users-svc:4000"]}"#]);
        assert_eq!(hits(&by_lang, call), vec![r#"{"via":"rpc","hosts":["catalog-svc"]}"#]);
        assert!(hits(&by_lang, admin_op).is_empty(), "apps/admin builds no client");
        let vue_copy = &by_lang["vue"][0].nodes[1];
        assert!(vue_copy.cells.iter().all(|c| c.kind != cell_type::ENDPOINT_HIT), "one stamp per id");

        let again = stamp_client_hosts(&mut by_lang, &owners);
        assert_eq!(again, ClientHostStats { graphql: 2, rpc: 1, sides: 0 });
    }

    /// DF.19: a contract op DOC_SECTION of `file` whose ORIGIN is `origin`.
    fn op(fp: &mut FileParse, q: &str, file: &str, origin: &str) -> NodeId {
        let id = push(fp, node_kind::DOC_SECTION, q, file);
        let cell = Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(origin.into()) };
        if let Some(n) = fp.nodes.iter_mut().find(|n| n.id == id) {
            n.cells.push(cell);
        }
        id
    }

    /// The ORIGIN payloads of every copy of `id`, in parse order.
    fn origins(by_lang: &HashMap<&'static str, Vec<FileParse>>, id: NodeId) -> Vec<String> {
        let mut langs: Vec<&&str> = by_lang.keys().collect();
        langs.sort();
        langs
            .into_iter()
            .flat_map(|l| &by_lang[*l])
            .flat_map(|fp| fp.nodes.iter().filter(move |n| n.id == id))
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::ORIGIN)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
            .collect()
    }

    /// DF.19: services/turps declares one general-info base, so its
    /// base-less OpenAPI op (both twins of one id) is stamped with it and
    /// the 1-based line; services/billing declares two, and a document with
    /// a base of its own, a document listing several servers and a swaggo
    /// annotation op are never stamped. A second run stamps nothing.
    #[test]
    fn stamp_annotated_bases_per_owner() {
        let owners = http_owner::OwnerIndex::from_roots(&[
            ProjectRoot::new("services/turps".into(), "go", "manifest", None),
            ProjectRoot::new("services/billing".into(), "go", "manifest", None),
        ]);
        let general = |base: &str| {
            format!("package main\n\n// @title API\n// @BasePath {base}\nfunc main() {{}}\n")
        };
        let files: Vec<(String, String)> = vec![
            ("services/turps/main.go".into(), general("/api")),
            ("services/turps/cmd/again.go".into(), general("/api/")),
            ("services/billing/main.go".into(), general("/billing")),
            ("services/billing/cmd/admin.go".into(), general("/admin")),
            ("services/turps/notes.md".into(), "// @BasePath /md".into()),
        ];
        let base_less = r#"{"provenance":"contract","source":"openapi","method":"POST","path":"/auth/login","raw_path":"/auth/login","operation_id":"login"}"#;
        let (mut json, _) = parse("services/turps/docs/swagger.json");
        let login = op(&mut json, "contract::docs::swagger::POST:/auth/login", "services/turps/docs/swagger.json", base_less);
        let based = op(
            &mut json,
            "contract::docs::swagger::GET:/v2/me",
            "services/turps/docs/swagger.json",
            r#"{"provenance":"contract","source":"openapi","method":"GET","path":"/v2/me","raw_path":"/me"}"#,
        );
        let listed = op(
            &mut json,
            "contract::docs::swagger::GET:/stats",
            "services/turps/docs/swagger.json",
            r#"{"provenance":"contract","source":"openapi","method":"GET","path":"/stats","raw_path":"/stats","bases":["","/admin"]}"#,
        );
        let annot = op(
            &mut json,
            "contract::Server::users::GET:/users",
            "services/turps/Server/users.go",
            r#"{"provenance":"contract","source":"swaggo","method":"GET","path":"/users","raw_path":"/users"}"#,
        );
        let billing = op(&mut json, "contract::billing::POST:/auth/login", "services/billing/docs/swagger.json", base_less);
        let (mut yaml, _) = parse("services/turps/docs/swagger.yaml");
        op(&mut yaml, "contract::docs::swagger::POST:/auth/login", "services/turps/docs/swagger.yaml", base_less);
        let mut by_lang: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        by_lang.insert("json", vec![json]);
        by_lang.insert("yaml", vec![yaml]);

        let stats = stamp_annotated_bases(&mut by_lang, &files, &owners);
        assert_eq!(stats, AnnotatedBaseStats { ops: 1, bases: 1 });
        let stamped = r#"{"provenance":"contract","source":"openapi","method":"POST","path":"/auth/login","raw_path":"/auth/login","operation_id":"login","annotated_base":"/api","annotated_at":"services/turps/main.go:4"}"#;
        assert_eq!(origins(&by_lang, login), vec![stamped, stamped], "both twins, key order kept");
        for id in [based, listed, annot, billing] {
            assert!(
                origins(&by_lang, id).iter().all(|o| !o.contains("annotated_base")),
                "{:?}",
                origins(&by_lang, id)
            );
        }

        let again = stamp_annotated_bases(&mut by_lang, &files, &owners);
        assert_eq!(again, AnnotatedBaseStats::default());
        assert_eq!(origins(&by_lang, login), vec![stamped, stamped]);

        // `@BasePath /` is the root: no base to stamp.
        let root = vec![("services/turps/main.go".to_string(), general("/"))];
        let (mut json, _) = parse("services/turps/docs/swagger.json");
        let login = op(&mut json, "contract::docs::swagger::POST:/auth/login", "services/turps/docs/swagger.json", base_less);
        let mut by_lang: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        by_lang.insert("json", vec![json]);
        assert_eq!(stamp_annotated_bases(&mut by_lang, &root, &owners), AnnotatedBaseStats::default());
        assert_eq!(origins(&by_lang, login), vec![base_less]);
    }

    /// CB.24: a repo with no nested root groups every file under the repo
    /// (owner None), so a root client stamps every operation; a repo with
    /// no fact stamps nothing.
    #[test]
    fn a_single_project_repo_stamps_under_the_repo() {
        let owners = http_owner::OwnerIndex::from_roots(&[ProjectRoot::new(String::new(), "npm", "manifest", None)]);
        let (mut client, m) = parse("src/client.ts");
        client.nav.record_fact(m, fact("graphql", "api", 2));
        let op = push(&mut client, node_kind::GRAPHQL_OPERATION, "graphql_op:User", "src/client.ts");
        let mut by_lang: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        by_lang.insert("typescript", vec![client]);
        let stats = stamp_client_hosts(&mut by_lang, &owners);
        assert_eq!(stats, ClientHostStats { graphql: 1, rpc: 0, sides: 1 });
        assert_eq!(hits(&by_lang, op), vec![r#"{"via":"graphql","hosts":["api"]}"#]);

        let (mut bare, _) = parse("src/profile.ts");
        let bare_op = push(&mut bare, node_kind::GRAPHQL_OPERATION, "graphql_op:User", "src/profile.ts");
        let mut by_lang: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        by_lang.insert("typescript", vec![bare]);
        assert_eq!(stamp_client_hosts(&mut by_lang, &owners), ClientHostStats::default());
        assert!(hits(&by_lang, bare_op).is_empty());
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// `(from qname, to qname)` of every `category` cross edge, sorted.
    fn pairs(m: &MergedGraph, category: glia_core::EdgeCategoryId) -> Vec<(String, String)> {
        let q = |id: NodeId| {
            m.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&id).cloned()).unwrap_or_default()
        };
        let mut out: Vec<(String, String)> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == category)
            .map(|e| (q(e.from), q(e.to)))
            .collect();
        out.sort();
        out
    }

    /// CB.24 end to end (the graphql-rpc-host-narrowing fixture): the web
    /// app's Apollo client targets users-svc and its tRPC client catalog-svc,
    /// both built in files other than the operations', so each op / call
    /// keeps the one service its client names.
    #[test]
    fn client_hosts_narrow_graphql_and_rpc_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("shop");
        write(&repo, "package.json", r#"{ "name": "shop", "private": true, "workspaces": ["apps/*", "services/*"] }"#);
        for (dir, name) in [("services/users", "users-svc"), ("services/catalog", "catalog-svc")] {
            write(&repo, &format!("{dir}/package.json"), &format!(r#"{{ "name": "{name}", "dependencies": {{ "@nestjs/graphql": "12.0.0", "@trpc/server": "10.0.0" }} }}"#));
            write(
                &repo,
                &format!("{dir}/src/user.resolver.ts"),
                "import { Resolver, Query } from \"@nestjs/graphql\";\n\n@Resolver()\nexport class UserResolver {\n  @Query(() => String)\n  getUser() {\n    return \"u\";\n  }\n}\n",
            );
            write(
                &repo,
                &format!("{dir}/src/router.ts"),
                "import { createTRPCRouter, publicProcedure } from \"./trpc\";\n\nexport const itemRouter = createTRPCRouter({\n  list: publicProcedure.query(() => []),\n});\n",
            );
        }
        write(&repo, "apps/web/package.json", r#"{ "name": "web", "dependencies": { "@apollo/client": "3.8.0", "@trpc/client": "10.0.0" } }"#);
        write(
            &repo,
            "apps/web/src/apollo.ts",
            "import { ApolloClient, InMemoryCache } from \"@apollo/client\";\n\nexport const client = new ApolloClient({ uri: \"http://users-svc/graphql\", cache: new InMemoryCache() });\n",
        );
        write(
            &repo,
            "apps/web/src/trpc.ts",
            "import { createTRPCProxyClient, httpBatchLink } from \"@trpc/client\";\n\nexport const api = createTRPCProxyClient({ links: [httpBatchLink({ url: \"http://catalog-svc/api/trpc\" })] });\n",
        );
        write(
            &repo,
            "apps/web/src/profile.ts",
            "import { gql, useQuery } from \"@apollo/client\";\nimport { api } from \"./trpc\";\n\nconst GET_USER = gql`\n  query getUser {\n    getUser\n  }\n`;\n\nexport function Profile() {\n  const items = api.item.list.useQuery();\n  return useQuery(GET_USER);\n}\n",
        );
        let built = generate_one(repo.to_str().unwrap()).unwrap();
        assert_eq!(
            pairs(&built.merged, edge_category::GRAPHQL_CALLS),
            vec![("graphql_op:getUser @apps/web".to_string(), "graphql_resolver:getUser @services/users".to_string())]
        );
        assert_eq!(
            pairs(&built.merged, edge_category::RPC_CALLS),
            vec![("rpc_call:item.list @apps/web".to_string(), "rpc:item.list @services/catalog".to_string())]
        );
    }
}

#[cfg(test)]
mod const_fold_tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::super::assemble::{RepoBuildCtx, build_const_table, parse_and_graft};
    use super::super::c_includes::IncludeRoots;
    use super::super::{BuildOptions, go_modules_for};
    use super::*;
    use crate::cache::CacheEntry;
    use crate::external::repo_inputs;
    use crate::route::parse_repo_files;
    use crate::walk::walk_source_files;
    use glia_code_domain::walk_gating::repo_identity;

    /// TS and Go files whose queue topics and event names are constants
    /// declared in ANOTHER file (SCREAMING / dotted, so the repo table's
    /// `resolve_identity` trusts them), plus one Go subscriber whose subject
    /// is a lower-case constant of its OWN file, which only the file's own
    /// scan (`FileConstTables`) resolves.
    fn write_repo(dir: &Path) {
        let write = |rel: &str, body: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        write("go.mod", "module example.com/fx\n");
        write("subjects.go", "package fx\n\nconst SUBJECT_ORDERS = \"orders.created\"\n");
        write(
            "publish.go",
            concat!(
                "package fx\n\n",
                "import \"github.com/nats-io/nats.go\"\n\n",
                "func Publish(nc *nats.Conn, data []byte) {\n",
                "\tnc.Publish(SUBJECT_ORDERS, data)\n",
                "}\n",
            ),
        );
        write(
            "consume.go",
            concat!(
                "package fx\n\n",
                "import \"github.com/nats-io/nats.go\"\n\n",
                "const billingSubject = \"billing.paid\"\n\n",
                "func Consume(nc *nats.Conn) {\n",
                "\tnc.Subscribe(billingSubject, func(m *nats.Msg) {})\n",
                "}\n",
            ),
        );
        write("web/topics.ts", "export const ORDERS_TOPIC = 'orders';\n");
        write(
            "web/publish.ts",
            concat!(
                "import { Kafka } from 'kafkajs';\n",
                "import { ORDERS_TOPIC } from './topics';\n\n",
                "const kafka = new Kafka({ clientId: 'web', brokers: ['b:9092'] });\n",
                "const producer = kafka.producer();\n\n",
                "export async function publishOrder(): Promise<void> {\n",
                "  await producer.send({ topic: ORDERS_TOPIC, messages: [] });\n",
                "}\n",
            ),
        );
        write(
            "web/orders.events.ts",
            concat!(
                "export const OrderEvents = ",
                "{ Created: \"order.created\", Paid: \"order.paid\" } as const;\n",
            ),
        );
        write(
            "web/orders.service.ts",
            concat!(
                "import { Injectable } from \"@nestjs/common\";\n",
                "import { EventEmitter2 } from \"@nestjs/event-emitter\";\n",
                "import { OrderEvents } from \"./orders.events\";\n\n",
                "@Injectable()\n",
                "export class OrdersService {\n",
                "  constructor(private eventEmitter: EventEmitter2) {}\n\n",
                "  create(id: string) {\n",
                "    this.eventEmitter.emit(OrderEvents.Created, { id });\n",
                "  }\n",
                "}\n",
            ),
        );
        write(
            "web/billing.listener.ts",
            concat!(
                "import { Injectable } from \"@nestjs/common\";\n",
                "import { OnEvent } from \"@nestjs/event-emitter\";\n",
                "import { OrderEvents } from \"./orders.events\";\n\n",
                "@Injectable()\n",
                "export class BillingListener {\n",
                "  @OnEvent(OrderEvents.Paid)\n",
                "  onPaid(payload: { id: string }) {}\n",
                "}\n",
            ),
        );
    }

    /// The canonical cache payload (`CacheEntry::payload`) of every parse,
    /// languages sorted, parses in their router order.
    fn payloads(by_lang: &HashMap<&'static str, Vec<FileParse>>) -> Vec<(String, Vec<u8>)> {
        let mut langs: Vec<&&'static str> = by_lang.keys().collect();
        langs.sort();
        langs
            .into_iter()
            .flat_map(|lang| {
                by_lang[*lang].iter().map(move |fp| {
                    let bytes = CacheEntry::new(0, lang, fp.clone()).payload().expect("payload");
                    (lang.to_string(), bytes)
                })
            })
            .collect()
    }

    /// Every queue / event site qname across the parses.
    fn site_qnames(by_lang: &HashMap<&'static str, Vec<FileParse>>) -> BTreeSet<String> {
        by_lang
            .values()
            .flatten()
            .flat_map(|fp| {
                fp.nav
                    .qname_by_id
                    .iter()
                    .filter(|&(id, _)| is_queue_node(&fp.nav, *id) || is_event_site_node(&fp.nav, *id))
                    .map(|(_, q)| q.clone())
            })
            .collect()
    }

    fn in_pool<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool")
            .install(f)
    }

    /// The walk of a repo and the parses of its files, pre-graft.
    struct Parsed {
        files: Vec<(String, String)>,
        repo: RepoId,
        by_lang: HashMap<&'static str, Vec<FileParse>>,
        errors: Vec<String>,
    }

    fn parsed(root: &Path) -> Parsed {
        let label = root.to_str().unwrap();
        let repo = RepoId::from_canonical(&repo_identity(root).key);
        let (files, _regions, _md, roots) = walk_source_files(root);
        let go = go_modules_for(root, &roots, label);
        let (by_lang, errors) = parse_repo_files(&files, repo, &go, None, label);
        Parsed { files, repo, by_lang, errors }
    }

    /// What both folds left: their stats and pools, the parses, the errors.
    struct Folded {
        queue: QueueConstStats,
        event: EventConstStats,
        pools: [(usize, usize); 2],
        by_lang: HashMap<&'static str, Vec<FileParse>>,
        errors: Vec<String>,
    }

    /// Both folds, run directly over a copy of `p`'s parses with `tables` as
    /// the per-file scans.
    fn fold_both(p: &Parsed, consts: &ConstTable, tables: &FileConstTables) -> Folded {
        let mut by_lang = p.by_lang.clone();
        let mut errors = Vec::new();
        let modules = ModuleQnames::plan(&p.files);
        let inputs = FoldInputs {
            files: &p.files,
            repo: p.repo,
            modules: &modules,
            consts,
            file_tables: tables,
        };
        let (queue, qp) = apply_queue_const_topics(&mut by_lang, &inputs, &mut errors);
        let (event, ep) = apply_event_const_names(&mut by_lang, &inputs, &mut errors);
        let pools = [(qp.files, qp.threads), (ep.files, ep.threads)];
        Folded { queue, event, pools, by_lang, errors }
    }

    /// DC.3: the const folds on a pool of 8 equal the folds on a pool of 1
    /// (sequential): the same parses byte for byte (canonical payloads), the
    /// same stats and error lines, through `parse_and_graft` and through the
    /// folds alone; each fold reads `FileConstTables`, and a target whose
    /// scan is `None` reports the fold's PANIC line.
    #[test]
    fn const_folds_pooled_equal_sequential() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        write_repo(&root);
        let label = root.to_str().unwrap().to_string();

        // Through the whole graft stage.
        let graft = |threads: usize| {
            in_pool(threads, || {
                let repo = RepoId::from_canonical(&repo_identity(&root).key);
                let (files, _regions, _md, roots) = walk_source_files(&root);
                let opts = BuildOptions::default();
                let inputs = [repo_inputs(repo, root.clone(), label.clone(), None)];
                let go = go_modules_for(&root, &roots, &label);
                let ts_aliases = TsAliasSet::read(&root, &roots, &label);
                let c_includes = IncludeRoots::read(&root, &files, &roots, &label);
                let mut rpc = RpcContext::default();
                rpc.add_files(&files);
                let ctx = RepoBuildCtx {
                    repo,
                    repo_label: &label,
                    go: &go,
                    ts_aliases: &ts_aliases,
                    c_includes: &c_includes,
                    rpc: &rpc,
                    roots: &roots,
                    config: inputs.first().and_then(|i| i.config.as_ref()),
                    opts: &opts,
                };
                let g = parse_and_graft(&files, None, &ctx);
                (payloads(&g.parses_by_lang), g.parse_errors, site_qnames(&g.parses_by_lang))
            })
        };
        let (seq, seq_errors, seq_sites) = graft(1);
        let (pooled, pooled_errors, pooled_sites) = graft(8);
        assert_eq!(seq_errors, pooled_errors, "parse_and_graft error lines");
        assert_eq!(seq_sites, pooled_sites);
        assert!(seq == pooled, "parse_and_graft parses differ between 1 and 8 threads");
        // Every cross-file constant folded, and the same-file lower-case one.
        let want = [
            "event_emit:order.created",
            "event_handle:order.paid",
            "queue_consumer:billing.paid",
            "queue_producer:orders",
            "queue_producer:orders.created",
        ];
        assert_eq!(seq_sites, want.map(String::from).into_iter().collect::<BTreeSet<_>>());

        // The two folds alone, over a fresh parse and the stage's own scan.
        let p = parsed(&root);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let mut scan_errors = Vec::new();
        let (consts, tables, _, _) = build_const_table(&p.files, &mut scan_errors);
        assert!(scan_errors.is_empty(), "{scan_errors:?}");
        let one = in_pool(1, || fold_both(&p, &consts, &tables));
        let eight = in_pool(8, || fold_both(&p, &consts, &tables));
        let q = &one.queue;
        assert_eq!((q.files, q.folded, q.unresolved), (3, 3, 0), "{q:?}");
        let e = &one.event;
        assert_eq!((e.files, e.folded, e.unresolved), (2, 2, 0), "{e:?}");
        assert_eq!((&one.queue, &one.event), (&eight.queue, &eight.event), "stats by threads");
        assert!(one.errors.is_empty(), "{:?}", one.errors);
        assert_eq!(one.errors, eight.errors);
        assert!(payloads(&one.by_lang) == payloads(&eight.by_lang), "folded parses by threads");
        // (files, threads) per fold: the targets ran on the installed pool.
        assert_eq!(one.pools, [(3, 1), (2, 1)]);
        assert_eq!(eight.pools, [(3, 8), (2, 8)]);

        // The fold reads FileConstTables: with consume.go's scan gone (as
        // when it panicked) its lower-case subject cannot resolve, and the
        // fold reports its own PANIC line instead of re-scanning the file.
        let consume = p.files.iter().position(|(path, _)| path.ends_with("consume.go")).unwrap();
        let mut gone: Vec<Option<ConstTable>> =
            (0..p.files.len()).map(|i| tables.get(i).cloned()).collect();
        gone[consume] = None;
        let gone = in_pool(8, || fold_both(&p, &consts, &FileConstTables(gone)));
        let consume_path = &p.files[consume].0;
        assert_eq!(gone.errors, vec![format!("{consume_path}: PANIC (queue const fold)")]);
        assert_eq!((gone.queue.files, gone.queue.folded), (2, 2), "{:?}", gone.queue);
        assert_eq!(gone.event, one.event, "the event fold does not read consume.go");
        let sites = site_qnames(&gone.by_lang);
        assert!(!sites.contains("queue_consumer:billing.paid"), "{sites:?}");
        assert!(sites.contains("queue_producer:orders.created"), "{sites:?}");
    }
}

#[cfg(test)]
mod ws_base_tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::super::assemble::build_const_table;
    use super::super::go_modules_for;
    use super::*;
    use crate::cache::CacheEntry;
    use crate::route::parse_repo_files;
    use crate::walk::walk_source_files;
    use glia_code_domain::walk_gating::repo_identity;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// The xcut-websocket-env-base client, plus a file whose client URL is a
    /// literal (never a target that folds) and one whose chain nothing binds.
    fn write_repo(dir: &Path) {
        write(dir, "package.json", "{ \"name\": \"feed-client\" }\n");
        write(
            dir,
            "src/environment.ts",
            "export const environment = { feedUrl: 'ws://feed-svc:8080/rt/feed', wsBase: 'ws://feed-svc:8080/rt' };\n",
        );
        write(
            dir,
            "src/live.ts",
            concat!(
                "import { environment } from './environment';\n\n",
                "export function openFeed(): WebSocket {\n",
                "  return new WebSocket(environment.feedUrl);\n",
                "}\n\n",
                "export function openPrices(): WebSocket {\n",
                "  return new WebSocket(`${environment.wsBase}/prices`);\n",
                "}\n",
            ),
        );
        write(
            dir,
            "src/literal.ts",
            "export const open = () => new WebSocket('ws://chat-svc/ws/chat');\n",
        );
        write(
            dir,
            "src/unbound.ts",
            "import { environment } from './environment';\nexport const open = () => new WebSocket(`${environment.missing}/ws/x`);\n",
        );
    }

    struct Parsed {
        files: Vec<(String, String)>,
        repo: RepoId,
        by_lang: HashMap<&'static str, Vec<FileParse>>,
    }

    fn parsed(root: &Path) -> Parsed {
        let label = root.to_str().unwrap();
        let repo = RepoId::from_canonical(&repo_identity(root).key);
        let (files, _regions, _md, roots) = walk_source_files(root);
        let go = go_modules_for(root, &roots, label);
        let (by_lang, errors) = parse_repo_files(&files, repo, &go, None, label);
        assert!(errors.is_empty(), "{errors:?}");
        Parsed { files, repo, by_lang }
    }

    fn client_qnames(by_lang: &HashMap<&'static str, Vec<FileParse>>) -> BTreeSet<String> {
        by_lang
            .values()
            .flatten()
            .flat_map(|fp| {
                fp.nav
                    .qname_by_id
                    .iter()
                    .filter(|(id, _)| is_ws_client_node(&fp.nav, **id))
                    .map(|(_, q)| q.clone())
            })
            .collect()
    }

    /// The canonical cache payload of every parse, languages sorted.
    fn payloads(by_lang: &HashMap<&'static str, Vec<FileParse>>) -> Vec<Vec<u8>> {
        let mut langs: Vec<&&'static str> = by_lang.keys().collect();
        langs.sort();
        langs
            .into_iter()
            .flat_map(|lang| {
                by_lang[*lang]
                    .iter()
                    .map(move |fp| CacheEntry::new(0, lang, fp.clone()).payload().expect("payload"))
            })
            .collect()
    }

    fn fold(
        p: &Parsed,
        threads: usize,
    ) -> (WsBaseStats, HashMap<&'static str, Vec<FileParse>>, Vec<String>) {
        let mut scan_errors = Vec::new();
        let (consts, tables, _, _) = build_const_table(&p.files, &mut scan_errors);
        assert!(scan_errors.is_empty(), "{scan_errors:?}");
        let modules = ModuleQnames::plan(&p.files);
        let inputs = FoldInputs {
            files: &p.files,
            repo: p.repo,
            modules: &modules,
            consts: &consts,
            file_tables: &tables,
        };
        let mut by_lang = p.by_lang.clone();
        let mut errors = Vec::new();
        let stats = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool")
            .install(|| apply_ws_client_bases(&mut by_lang, &inputs, &mut errors));
        (stats, by_lang, errors)
    }

    /// DT.3b: the configured bases re-key their clients in the one file that
    /// holds them; the literal client is untouched and the unbound chain is
    /// counted, not folded; the pool size changes nothing.
    #[test]
    fn ws_client_bases_fold_through_the_repo_table() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("client");
        write_repo(&root);
        let p = parsed(&root);
        assert_eq!(
            client_qnames(&p.by_lang),
            ["ws_client:/prices", "ws_client:/ws/chat", "ws_client:/ws/x", "ws_client:ws"]
                .map(String::from)
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        let (stats, by_lang, errors) = fold(&p, 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(stats, WsBaseStats { files: 1, folded: 2, unresolved: 1 });
        assert_eq!(
            client_qnames(&by_lang),
            ["ws_client:/rt/feed", "ws_client:/rt/prices", "ws_client:/ws/chat", "ws_client:/ws/x"]
                .map(String::from)
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        // Only live.ts changed: every other parse is byte-identical.
        let before = payloads(&p.by_lang);
        let after = payloads(&by_lang);
        assert_eq!(before.len(), after.len());
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        assert_eq!(changed, 1);
        // The pool's order-preserving map: 8 threads == 1.
        let (stats8, by_lang8, errors8) = fold(&p, 8);
        assert_eq!(stats8, stats);
        assert_eq!(errors8, errors);
        assert!(payloads(&by_lang8) == after, "folded parses by threads");
    }
}

#[cfg(test)]
mod quartz_conf_tests {
    use super::*;

    const REPO: RepoId = RepoId(12);

    /// A Scala parse of `path` with its MODULE first and the cron extractor's
    /// output merged in, as the router hands it over.
    fn scala_parse(path: &str, source: &str) -> FileParse {
        let module = NodeId::from_parts(glia_code_domain::GRAPH_TYPE, REPO, node_kind::MODULE, path);
        let mut fp = FileParse::default();
        fp.nodes.push(glia_core::Node {
            id: module,
            repo: REPO,
            confidence: glia_core::Confidence::Strong,
            cells: Vec::new(),
        });
        fp.nav.record(module, path, path, node_kind::MODULE, None);
        let out = cron::extract_cron_nodes(source, path, module, REPO);
        fp.nodes.extend(out.nodes);
        fp.edges.extend(out.edges);
        merge_nav(&mut fp.nav, out.nav);
        fp
    }

    fn start(name: &str) -> String {
        format!(
            "import com.typesafe.akka.extension.quartz.QuartzSchedulerExtension\n\
             object J {{ QuartzSchedulerExtension(sys).schedule(\"{name}\", a, b) }}\n"
        )
    }

    fn cron_qnames(by_lang: &HashMap<&'static str, Vec<FileParse>>) -> Vec<String> {
        let mut out: Vec<String> = by_lang
            .values()
            .flatten()
            .flat_map(|fp| {
                fp.nodes.iter().filter_map(|n| {
                    (fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::CRON_JOB))
                        .then(|| fp.nav.qname_by_id.get(&n.id).cloned())
                        .flatten()
                })
            })
            .collect();
        out.sort();
        out
    }

    /// DG.7b: the conf table is first-wins over walk order, a differing later
    /// definition counts as a conflict, a REF the table names is re-keyed in
    /// whichever language's parse holds it, and one it does not is dropped.
    #[test]
    fn graft_quartz_conf_schedules_first_wins_and_drops_misses() {
        let files: Vec<(String, String)> = vec![
            (
                "a/src/main/resources/application.conf".into(),
                "akka.quartz.schedules { Nightly { expression = \"0 0 3 * * ?\" } }\n".into(),
            ),
            (
                "b/src/main/resources/application.conf".into(),
                "akka.quartz.schedules.Nightly.expression = \"0 0 4 * * ?\"\n\
                 pekko.quartz.schedules.Hourly.expression = \"0 0 * * * ?\"\n"
                    .into(),
            ),
            ("nginx/site.conf".into(), "akka.quartz.schedules.Gone.expression = \"0 0 5 * * ?\"\n".into()),
        ];
        let mut by_lang: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        by_lang.insert(
            "scala",
            vec![scala_parse("a/Jobs.scala", &start("Nightly")), scala_parse("b/Gone.scala", &start("Gone"))],
        );
        by_lang.insert("java", vec![scala_parse("c/Hourly.java", &start("Hourly"))]);
        let stats = graft_quartz_conf_schedules(&mut by_lang, &files, REPO);
        assert_eq!(
            stats,
            QuartzConfStats { confs: 2, schedules: 2, rekeyed: 2, unresolved: 1, conflicts: 1 }
        );
        assert_eq!(cron_qnames(&by_lang), ["cron:0 0 * * * ?:Hourly", "cron:0 0 3 * * ?:Nightly"]);
        // The graft is idempotent over its own output.
        let again = graft_quartz_conf_schedules(&mut by_lang, &files, REPO);
        assert_eq!((again.rekeyed, again.unresolved), (0, 0));

        // No conf and no REF: no marker input at all.
        let mut none: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        none.insert("scala", vec![scala_parse("x.scala", "object X\n")]);
        assert_eq!(graft_quartz_conf_schedules(&mut none, &[], REPO), QuartzConfStats::default());
    }
}
