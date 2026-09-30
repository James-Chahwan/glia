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
    FileParse, LocalModuleIndex, NavFact, attach_imports_cell_filtered, cell_type, node_kind,
};
use glia_code_extractors::constants::ConstTable;
use glia_code_extractors::next_pages::{self, NextRoots, PageRouter};
use glia_code_extractors::{anchor, queue_topic, queues};
use glia_core::{Cell, CellPayload, NodeId, NodeKindId, RepoId};
use glia_graph::rust_paths::RustCrate;

use super::lang_build::TsAliasSet;
use super::rpc_needles::{RpcContext, RpcNeedleCounts, apply_rpc_needles};
use crate::arch::node_file;
use crate::endpoint_fold;
use crate::external::{WrapperPass, WrapperPhase, infer_entity_wrappers};
use crate::extract::{detect_language, merge_nav};
use crate::http_owner;
use crate::route::ModuleQnames;

/// Run every post-cache graft over one repo's parses, in order: the LF.2e
/// overlay wrapper scan and its http half, the A11.2 endpoint fold, the LA.6d
/// Next.js page graft, the LA.4 queue-topic const fold, the LF.2e wrappers'
/// queue half, the A5.2 / A5.3 / LA.17 RPC needles with their `[grpc-client]` /
/// `[grpc-server-impl]` / `[proto-rpc]` markers, the A5.8 `[marker-anchor]`
/// census and the LE.4a `[data-access]` census, the LB.4a / LB.8 owner
/// segment, the CB.24 client-host stamp, then the A16.4 IMPORTS-cell
/// filter. `const_table` is the repo's A11.1 table; `roots` are the walk's
/// project roots (A8.4); `rust_crates` their Cargo packages, whose names the
/// IMPORTS filter treats as intra-repo (LA.1b), as it does every specifier a
/// `ts_aliases` key matches (A6.8). `wrappers` is the repo's
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
    // A11.2: re-key client ENDPOINTs whose base the table resolves and record
    // their authority. Post-cache, so cached parses are folded too and the
    // cache keeps the pre-fold parse.
    endpoint_fold::fold_repo(parses_by_lang.values_mut().flatten(), const_table, repo)
        .report(repo_label);
    // LA.6d: Next.js file-system pages. Before the owner pass, so a grafted
    // page is owner-qualified like every other nav ROUTE.
    graft_next_pages(parses_by_lang, files, repo, &modules, parse_errors).report(repo_label);
    // LA.4 (A11.7): queue topics named by a constant. Same seam and the same
    // cache rule as the endpoint fold; runs before the A16.4 filter, which then
    // rewrites the folded nodes' IMPORTS cell like every other node's.
    apply_queue_const_topics(parses_by_lang, files, repo, &modules, const_table, parse_errors)
        .report(repo_label);
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

    // A16.4: drop intra-repo names from every IMPORTS cell. It needs the whole
    // repo's declarations, so like `apply_rpc_needles` it runs after the parse
    // cache (cached parses are filtered too) and after the RPC grafts (whose
    // markers carry the same cell).
    filter_imports_cells(parses_by_lang, rust_crates, ts_aliases, repo_label);

    rpc_added
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
#[derive(Debug, Default)]
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
/// A file's queue nodes are swapped (`queues::replace_queue_nodes`) only when
/// at least one site folded, so every other file is untouched. `files` is
/// walk-sorted and the parse index is keyed by (language, module), so the
/// result does not depend on HashMap order.
fn apply_queue_const_topics(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    modules: &ModuleQnames,
    consts: &ConstTable,
    parse_errors: &mut Vec<String>,
) -> QueueConstStats {
    let mut stats = QueueConstStats::default();
    let is_queue = |k: &glia_core::NodeKindId| {
        *k == node_kind::QUEUE_PRODUCER || *k == node_kind::QUEUE_CONSUMER
    };
    // (language, module) -> the parses holding a queue node under that module,
    // in walk order. The module is the queue nodes' parent: the id the router
    // handed the extractors, whatever the language parser emitted first.
    let mut holders: HashMap<(&'static str, NodeId), Vec<usize>> = HashMap::new();
    for (lang, parses) in parses_by_lang.iter() {
        for (i, fp) in parses.iter().enumerate() {
            let module = fp
                .nav
                .kind_by_id
                .iter()
                .find(|(_, k)| is_queue(k))
                .and_then(|(id, _)| fp.nav.parent_of.get(id));
            if let Some(module) = module {
                holders.entry((*lang, *module)).or_default().push(i);
            }
        }
    }
    if holders.is_empty() {
        return stats;
    }
    for (path, source) in files {
        let Some(lang) = detect_language(path) else {
            continue;
        };
        let module_id = modules.module_id(path, repo);
        let Some(candidates) = holders.get(&(lang, module_id)) else {
            continue;
        };
        let Some(parses) = parses_by_lang.get_mut(lang) else {
            continue;
        };
        // `a.ts` and `a.js` share a module id; the queue nodes' POSITION names
        // the file they were read from, so a fold never lands on a sibling.
        let position = format!(r#"{{"file":"{}","#, queue_topic::escape_json(path));
        let Some(&at) = candidates.iter().find(|&&i| {
            parses
                .get(i)
                .is_some_and(|fp| queue_nodes_read_from(fp, &position))
        }) else {
            continue;
        };
        let Some(fp) = parses.get_mut(at) else {
            continue;
        };
        let fold = catch_unwind(AssertUnwindSafe(|| {
            let local = ConstTable::scan_file(source, lang);
            let resolve = |expr: &str| {
                local
                    .resolve_expr_strict(expr)
                    .or_else(|| consts.resolve_identity(expr))
                    .map(str::to_string)
            };
            queues::extract_queue_nodes_with_consts(source, path, module_id, repo, &resolve)
        }));
        match fold {
            Ok(mut fold) => {
                stats.folded += fold.counts.folded;
                stats.unresolved += fold.counts.unresolved;
                if fold.counts.folded > 0 {
                    stats.files += 1;
                    // LC.3a: the re-emitted `module -> queue node` CONTAINS
                    // edges replace ones the queue extractor stamped in the
                    // parse closure; these land post-cache, unstamped.
                    let ev = Evidence::emitter("extractor:queues").rule("const_fold");
                    evidence::stamp_missing_with(&mut fold.consumers.edges, &ev);
                    evidence::stamp_missing_with(&mut fold.producers.edges, &ev);
                    queues::replace_queue_nodes(fp, module_id, lang, fold);
                }
            }
            Err(_) => parse_errors.push(format!("{path}: PANIC (queue const fold)")),
        }
    }
    stats
}

/// Does one of `fp`'s queue nodes carry a POSITION cell opening with
/// `position` (`{"file":"<escaped path>",`)?
fn queue_nodes_read_from(fp: &FileParse, position: &str) -> bool {
    fp.nodes.iter().any(|n| {
        fp.nav
            .kind_by_id
            .get(&n.id)
            .is_some_and(|k| *k == node_kind::QUEUE_PRODUCER || *k == node_kind::QUEUE_CONSUMER)
            && n.cells.iter().any(|c| {
                c.kind == cell_type::POSITION
                    && matches!(&c.payload, CellPayload::Json(p) if p.starts_with(position))
            })
    })
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
