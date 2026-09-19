//! The post-cache graft sequence of `build_graphs_for_repo`. Every pass here
//! reads something that is not a function of one file's content (the repo's
//! const table, the build's proto services, the whole repo's declarations), so
//! it runs on the router's output — cached parses included — and never inside
//! the per-file extractors. That keeps incremental == clean.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::project_roots::ProjectRoot;
use repo_graph_code_domain::{
    FileParse, GRAPH_TYPE, LocalModuleIndex, attach_imports_cell_filtered, cell_type, node_kind,
};
use repo_graph_code_extractors::constants::ConstTable;
use repo_graph_code_extractors::next_pages::{self, NextRoots, PageRouter};
use repo_graph_code_extractors::{anchor, queue_topic, queues};
use repo_graph_core::{CellPayload, NodeId, RepoId};
use repo_graph_graph::rust_paths::RustCrate;

use super::rpc_needles::{RpcContext, apply_rpc_needles};
use crate::endpoint_fold;
use crate::extract::{detect_language, merge_nav, path_to_qname};
use crate::http_owner;

/// Run every post-cache graft over one repo's parses, in order: the A11.2
/// endpoint fold, the LA.6d Next.js page graft, the LA.4 queue-topic const
/// fold, the A5.2 / A5.3 / LA.17 RPC needles with their `[grpc-client]` /
/// `[grpc-server-impl]` / `[proto-rpc]` markers, the A5.8 `[marker-anchor]`
/// census, the LB.4a / LB.8 owner segment, then the A16.4 IMPORTS-cell
/// filter. `const_table` is the repo's A11.1 table; `roots` are the walk's
/// project roots (A8.4); `rust_crates` their Cargo packages, whose names the
/// IMPORTS filter treats as intra-repo (LA.1b).
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
    parse_errors: &mut Vec<String>,
    repo_label: &str,
) {
    // A11.2: re-key client ENDPOINTs whose base the table resolves and record
    // their authority. Post-cache, so cached parses are folded too and the
    // cache keeps the pre-fold parse.
    endpoint_fold::fold_repo(parses_by_lang.values_mut().flatten(), const_table, repo)
        .report(repo_label);
    // LA.6d: Next.js file-system pages. Before the owner pass, so a grafted
    // page is owner-qualified like every other nav ROUTE.
    graft_next_pages(parses_by_lang, files, repo, parse_errors).report(repo_label);
    // LA.4 (A11.7): queue topics named by a constant. Same seam and the same
    // cache rule as the endpoint fold; runs before the A16.4 filter, which then
    // rewrites the folded nodes' IMPORTS cell like every other node's.
    apply_queue_const_topics(parses_by_lang, files, repo, const_table, parse_errors)
        .report(repo_label);

    let rpc_added = apply_rpc_needles(parses_by_lang, files, repo, rpc, parse_errors);
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

    // LB.4a / LB.8: qualify every owned node under a nested project root with
    // ` @<project path>` (see the ordering rule above). After the endpoint
    // fold, which keys endpoints by their owner-free path, the queue const
    // fold and the RPC needles, which mint queue nodes and GRPC_CLIENT /
    // GRPC_SERVER markers by owner-free name; per parse, so HashMap order
    // cannot reach the ids.
    let owners = http_owner::OwnerIndex::from_roots(roots);
    http_owner::qualify_repo(parses_by_lang.values_mut().flatten(), &owners, repo)
        .report(repo_label);

    // A16.4: drop intra-repo names from every IMPORTS cell. It needs the whole
    // repo's declarations, so like `apply_rpc_needles` it runs after the parse
    // cache (cached parses are filtered too) and after the RPC grafts (whose
    // markers carry the same cell).
    filter_imports_cells(parses_by_lang, rust_crates, repo_label);
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
/// `detect_language` key, the one whose first node is the file's MODULE. No
/// match (the file failed to parse) means no module to hang the page on, and
/// the page is skipped. Deterministic: roots are sorted, `files` is
/// walk-sorted, one ROUTE per page file, no HashMap order reaches a parse.
fn graft_next_pages(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
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
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, &path_to_qname(path));
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
    consts: &ConstTable,
    parse_errors: &mut Vec<String>,
) -> QueueConstStats {
    let mut stats = QueueConstStats::default();
    let is_queue = |k: &repo_graph_core::NodeKindId| {
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
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, &path_to_qname(path));
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
            Ok(fold) => {
                stats.folded += fold.counts.folded;
                stats.unresolved += fold.counts.unresolved;
                if fold.counts.folded > 0 {
                    stats.files += 1;
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
/// parser emits raw `use` paths, so `use repo_graph_engine::..` in a sibling
/// crate reaches the filter as `repo_graph_engine`, which is not a dependency.
/// Only Rust crate names are seeded (`rust_crates` is the Cargo projects).
///
/// fired_on marker, once per repo that holds a language-parser parse:
///   `[imports] local-filter: kept {k}, dropped {d} intra-repo name(s) across {n} language group(s) repo=<label>`
/// `kept` / `dropped` sum the per-file library names; `n` counts lang tags.
fn filter_imports_cells(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    rust_crates: &[RustCrate],
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

    use repo_graph_code_domain::edge_category;
    use repo_graph_graph::MergedGraph;

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
        repo_graph_store::write_merged_sharded(m, dir).unwrap();
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
