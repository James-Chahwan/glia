//! Per-file routing: which extractor or language parser sees which file.
//! Holds the non-source branches (yaml / Dockerfile / package manifest /
//! dotenv / migration `.sql` / Prisma `.prisma` / contract and JSON Schema
//! `.json` / `.proto` / `.graphql`), the WP-D incremental parse-cache lookup,
//! and the per-file panic isolation.
//! Split out of `build_graphs_for_repo`.

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::{CodeNav, FileParse, GRAPH_TYPE, evidence, node_kind};
use repo_graph_core::{Cell, Confidence, Edge, Node, NodeId, RepoId};

use crate::cache::{self, ParseCache};
use crate::extract::{
    ExtractStats, apply_cross_cutting_extractors, detect_language, merge_nav, parse_one_with,
    path_to_qname, synthetic_module_qname,
};
use crate::walk::{is_angular_template_path, is_dockerfile_path, is_dotenv_path};

/// The per-file half of `build_graphs_for_repo`: route every walked file to
/// its parser or synthetic extractor and return the parses grouped by
/// language tag and the per-file errors. `.proto` files are no longer a
/// separate bucket — they stash under the `"proto"` lang tag like any other
/// synthetic parse (A5.1), so they get a MODULE node and a file position.
/// `repo_label` only prefixes the `[incremental]` marker (A1.4).
pub(crate) fn parse_repo_files(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    mut cache: Option<&mut ParseCache>,
    repo_label: &str,
) -> (HashMap<&'static str, Vec<FileParse>>, Vec<String>) {
    let mut parses_by_lang: HashMap<&str, Vec<FileParse>> = HashMap::new();
    let mut parse_errors = Vec::new();
    // A5.1 `[proto]` marker counters.
    let mut proto_files = 0usize;
    let mut proto_services = 0usize;
    let mut proto_rpcs = 0usize;
    let mut proto_packages = 0usize;
    // A10.5 `[proto] files=` marker counters.
    let mut proto_messages = 0usize;
    let mut proto_enums = 0usize;
    // A10.4 `[graphql-sdl]` marker counters.
    let mut sdl_files = 0usize;
    let mut sdl_resolvers = 0usize;
    // A10.6 `[avro]` marker counters.
    let mut avro_files = 0usize;
    let mut avro_records = 0usize;
    let mut avro_enums = 0usize;
    let mut avro_fixed = 0usize;
    // LE.10a `[schema-fields]` marker counters: proto messages / Avro records
    // given a SCHEMA_FIELDS cell, and the fields those cells list.
    let mut proto_field_messages = 0usize;
    let mut proto_fields = 0usize;
    let mut avro_field_records = 0usize;
    let mut avro_fields = 0usize;
    // LA.16 (A10.12) `[jsonschema]` marker counters.
    let mut jsonschema_files = 0usize;
    let mut jsonschema_types = 0usize;
    let mut jsonschema_defs = 0usize;
    // A10.1 `[contract]` marker counters: yaml (A10.1 / A10.3) and sniffed
    // JSON (A10.8) contracts both fold in through `ContractCounts::record`.
    let mut contracts = repo_graph_code_extractors::contracts::ContractCounts::default();
    // WP-D incremental: track which main-parser files parsed so deleted (or
    // no-longer-parseable) files get evicted from the sidecar.
    let mut live_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    // LA.12: every main-parser input as (path, content hash), for
    // `ParseCache::diff`; the marker counts are that diff's lengths. Fresh
    // parses wait in `pending` (the one clone the cache keeps) until the diff
    // has been taken against the PRE-build cache — putting them as they come
    // would make every changed file read as reused.
    let mut current: Vec<(String, u64)> = Vec::new();
    let mut pending: Vec<(String, u64, &'static str, FileParse)> = Vec::new();
    // A3.4 `[extract]` marker counter. Only counts files actually reparsed this
    // build — a cache hit replays a FileParse whose ROUTE nodes already carry
    // the mark, so the graph-side `[http]` marker is the complete figure.
    let mut nav_routes_marked = 0usize;
    // LA.6b: the route-table walker's counters, same reparsed-only caveat.
    let mut nav_bound = 0usize;
    let mut nav_redirects = 0usize;
    let mut nav_children = 0usize;
    let mut nav_rejected = 0usize;
    let mut nav_catchalls = 0usize;
    // A3.5 `[extract] ts-routes` marker counter, same reparsed-only caveat.
    let mut ts_client_calls_skipped = 0usize;
    // LA.6c `[nav-links]` marker counters: link sites of the reparsed
    // TS-family files (same reparsed-only caveat) plus every `.component.html`
    // template, which is never cached.
    let mut links_router = 0usize;
    let mut links_href = 0usize;
    let mut links_origin = 0usize;
    let mut links_template = 0usize;
    let mut links_dynamic = 0usize;
    // A13.16: a prismaSchemaFolder schema declares its `datasource` in one
    // `.prisma` file and its models in the others; a model file with no
    // datasource of its own takes the provider every schema file agrees on.
    let prisma_provider = repo_graph_code_extractors::prisma::shared_provider(
        files
            .iter()
            .filter(|(p, _)| repo_graph_code_extractors::prisma::is_prisma_schema(p))
            .map(|(_, s)| s.as_str()),
    );

    for (path, source) in files {
        let yaml_ext = matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str()),
            Some("yml" | "yaml")
        );
        if yaml_ext {
            let module_id = synthetic_module_id(repo, path);
            let cron_out = repo_graph_code_extractors::cron::extract_cron_nodes(
                source, path, module_id, repo,
            );
            let cfg_out = repo_graph_code_extractors::config::extract_yaml_env_defs(
                source, module_id, repo,
            );
            let iac_out =
                repo_graph_code_extractors::iac::extract_yaml(source, module_id, repo);
            // A10.1: an `openapi.yaml` / `swagger.yaml` declares the service's
            // API surface. Non-contract yaml takes a cheap sniff-miss here.
            let contract_out = repo_graph_code_extractors::contracts::extract_yaml_contracts(
                source, path, module_id, repo,
            );
            // A10.3: the same call also covers `asyncapi.yaml`; `record`
            // routes the count to the format the file sniffed as.
            contracts.record(&contract_out);
            if !cron_out.nodes.is_empty()
                || !cfg_out.nodes.is_empty()
                || !iac_out.nodes.is_empty()
                || !contract_out.nodes.is_empty()
            {
                stash_synthetic_parse(
                    "yaml",
                    path,
                    module_id,
                    repo,
                    vec![cron_out.nodes, cfg_out.nodes, iac_out.nodes, contract_out.nodes],
                    vec![cron_out.edges, cfg_out.edges, iac_out.edges, contract_out.edges],
                    vec![cron_out.nav, cfg_out.nav, iac_out.nav, contract_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        if is_dockerfile_path(path) {
            let module_id = synthetic_module_id(repo, path);
            let cfg_out = repo_graph_code_extractors::config::extract_dockerfile_defs(
                source, module_id, repo,
            );
            let iac_out = repo_graph_code_extractors::iac::extract_dockerfile(
                source, path, module_id, repo,
            );
            if !cfg_out.nodes.is_empty() || !iac_out.nodes.is_empty() {
                stash_synthetic_parse(
                    "dockerfile",
                    path,
                    module_id,
                    repo,
                    vec![cfg_out.nodes, iac_out.nodes],
                    vec![cfg_out.edges, iac_out.edges],
                    vec![cfg_out.nav, iac_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        if repo_graph_code_extractors::packages::is_manifest_path(path) {
            let module_id = synthetic_module_id(repo, path);
            let pkg_out = repo_graph_code_extractors::packages::extract_for_path(
                source, path, module_id, repo,
            );
            // LA.20c: the binaries the manifest declares (pyproject scripts,
            // npm `bin`, Cargo `[[bin]]`) as `cli:<bin>` CLI_COMMANDs, so an
            // invocation's argv0 pairs with them. A manifest that declares a
            // binary but no dependency is still stashed.
            let bin_out = repo_graph_code_extractors::cli::extract_manifest_binaries(
                source, path, module_id, repo,
            );
            if let Some(marker) = repo_graph_code_extractors::cli::manifest_marker(path, &bin_out)
            {
                eprintln!("{marker}");
            }
            if !pkg_out.nodes.is_empty() || !bin_out.nodes.is_empty() {
                stash_synthetic_parse(
                    "manifest",
                    path,
                    module_id,
                    repo,
                    vec![pkg_out.nodes, bin_out.nodes],
                    vec![pkg_out.edges],
                    vec![pkg_out.nav, bin_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        if is_dotenv_path(path) {
            let module_id = synthetic_module_id(repo, path);
            let cfg_out = repo_graph_code_extractors::config::extract_dotenv_defs(
                source, module_id, repo,
            );
            if !cfg_out.nodes.is_empty() {
                stash_synthetic_parse(
                    "dotenv",
                    path,
                    module_id,
                    repo,
                    vec![cfg_out.nodes],
                    vec![cfg_out.edges],
                    vec![cfg_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // A13.9: a migration `.sql` (the walk admits one only through
        // `is_migration_path`). Its DDL names the tables it creates, alters or
        // drops, each an ACCESSES_DATA target of the file's MODULE. Before
        // detect_language, which has no sql arm on purpose: the const-table
        // scan would bind `UPDATE t SET name = 'x'` as a constant.
        if repo_graph_code_extractors::migrations::is_migration_path(path) {
            let module_id = synthetic_module_id(repo, path);
            let out = repo_graph_code_extractors::migrations::extract_sql_migration(
                source, path, module_id, repo,
            );
            eprintln!("{}", out.marker(path));
            if !out.entities.nodes.is_empty() {
                stash_synthetic_parse(
                    "migration",
                    path,
                    module_id,
                    repo,
                    vec![out.entities.nodes],
                    vec![out.entities.edges],
                    vec![out.entities.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // A13.16: a Prisma schema (the walk admits `.prisma` only through
        // `is_prisma_schema`). Each `model` is a model-keyed DATA_ENTITY and an
        // ACCESSES_DATA target of the file's MODULE; `@@map` rides a table
        // cell. Before detect_language, which has no prisma arm on purpose:
        // the const-table scan would bind `provider = "postgresql"`.
        if repo_graph_code_extractors::prisma::is_prisma_schema(path) {
            let module_id = synthetic_module_id(repo, path);
            let out = repo_graph_code_extractors::prisma::extract_prisma_models(
                source,
                module_id,
                repo,
                prisma_provider.as_deref(),
            );
            eprintln!("{}", out.marker(path));
            if !out.entities.nodes.is_empty() {
                stash_synthetic_parse(
                    "prisma",
                    path,
                    module_id,
                    repo,
                    vec![out.entities.nodes],
                    vec![out.entities.edges],
                    vec![out.entities.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // LA.6c: an Angular `.component.html` template (admitted by the walk)
        // holds its component's navigation links. It shares the MODULE qname
        // of its `.component.ts` (`path_to_qname` strips only the last
        // extension), so the refs go out from that module under the `angular`
        // key, into the TS-family graph where the nav routes and the
        // component's page live. A bare `FileParse`, never
        // `stash_synthetic_parse`: that mints a MODULE node, and this id is
        // the `.component.ts` parse's. Before detect_language, which has no
        // html arm. The one non-code branch that keeps the CODE form
        // (`path_to_qname`, not `synthetic_module_id`, LB.9a): it borrows the
        // `.component.ts` MODULE id, and the file-name form would orphan
        // every link it reads.
        if is_angular_template_path(path) {
            let module_id =
                NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, &path_to_qname(path));
            match catch_unwind(AssertUnwindSafe(|| {
                repo_graph_code_extractors::nav_links::extract_template_links(source, module_id)
            })) {
                Ok(links) => {
                    links_router += links.router;
                    links_href += links.href;
                    links_origin += links.origin;
                    links_template += links.template;
                    links_dynamic += links.dynamic_skipped;
                    if !links.refs.is_empty() {
                        parses_by_lang
                            .entry("angular")
                            .or_default()
                            .push(FileParse {
                                refs: links.refs,
                                ..Default::default()
                            });
                    }
                }
                Err(payload) => parse_errors.push(format!(
                    "{path}: PANIC (nav template links): {}",
                    panic_payload_str(&payload)
                )),
            }
            continue;
        }

        // A10.8: the walker queues a `.json` only when it sniffed as an API
        // contract or (LA.16) a JSON Schema. After the manifest branch, so
        // package.json / composer.json never land here; before detect_language,
        // which has no json arm.
        let json_ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if json_ext {
            let module_id = synthetic_module_id(repo, path);
            // LA.16 (A10.12): a JSON Schema that is not an API contract
            // declares MESSAGE_TYPEs, the shape a `.proto` message or an
            // `.avsc` record gets, so MessageSchemaResolver can pair them
            // across repos. A contract keeps the A10.8 path below.
            if repo_graph_code_extractors::contracts::sniff_json_contract(source).is_none()
                && repo_graph_code_extractors::schemas::sniff_json_schema(source)
            {
                let recs = repo_graph_code_extractors::schemas::extract_json_schema_types(
                    source, path, module_id, repo,
                );
                jsonschema_files += 1;
                jsonschema_types += recs.nodes.len();
                jsonschema_defs += recs.def_count;
                if !recs.nodes.is_empty() {
                    stash_synthetic_parse(
                        "json",
                        path,
                        module_id,
                        repo,
                        vec![recs.nodes],
                        vec![recs.edges],
                        vec![recs.nav],
                        recs.module_cells,
                        &mut parses_by_lang,
                    );
                }
                continue;
            }
            let out = repo_graph_code_extractors::contracts::extract_json_contract(
                source, path, module_id, repo,
            );
            contracts.record(&out);
            if !out.nodes.is_empty() {
                stash_synthetic_parse(
                    "json",
                    path,
                    module_id,
                    repo,
                    vec![out.nodes],
                    vec![out.edges],
                    vec![out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        let Some(lang) = detect_language(path) else { continue };

        if lang == "proto" {
            let module_id = synthetic_module_id(repo, path);
            let out = repo_graph_code_extractors::grpc::extract_grpc_service_nodes(
                source, path, module_id, repo,
            );
            // A10.5: the file's `message` / `enum` declarations, as
            // MESSAGE_TYPE nodes under the same MODULE.
            let msgs = repo_graph_code_extractors::schemas::extract_proto_messages(
                source, path, module_id, repo,
            );
            proto_files += 1;
            proto_services += out.service_count;
            proto_rpcs += out.rpc_count;
            proto_messages += msgs.message_count;
            proto_enums += msgs.enum_count;
            proto_field_messages += msgs.schema_field_cells;
            proto_fields += msgs.schema_fields;
            if out.package.is_some() {
                proto_packages += 1;
            }
            // A messages-only `.proto` (a shared `common.proto`) is still a
            // parsed file, not a skipped one.
            if !out.nodes.is_empty() || !msgs.nodes.is_empty() {
                // Same synthetic path as yaml / Dockerfile / manifest / dotenv:
                // the file itself becomes a MODULE (with a POSITION cell) that
                // parents the GRPC_SERVICE, so `locate_node` / `docs-for` can
                // answer "where is this service declared".
                stash_synthetic_parse(
                    "proto",
                    path,
                    module_id,
                    repo,
                    vec![out.nodes, msgs.nodes],
                    vec![out.edges, msgs.edges],
                    vec![out.nav, msgs.nav],
                    out.module_cells,
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // A10.4: a `.graphql` / `.gql` schema reaches the SDL field scan, so a
        // schema-first service has resolvers for its clients' operations to
        // pair with. LA.27: the file is read whole as SDL; a code file reads
        // SDL only inside a GraphQL-marked literal. Resolver side only: a
        // schema declares server fields, and the operation needles would mint
        // client ops from its keywords.
        if lang == "graphql" {
            let module_id = synthetic_module_id(repo, path);
            let repo_graph_code_extractors::graphql::GraphqlNodes {
                nodes,
                nav,
                mut anchors,
            } = repo_graph_code_extractors::graphql::extract_graphql_sdl_file_nodes(
                source, module_id, repo,
            );
            sdl_files += 1;
            sdl_resolvers += nodes.len();
            if !nodes.is_empty() {
                stash_synthetic_parse(
                    "graphql",
                    path,
                    module_id,
                    repo,
                    vec![nodes],
                    vec![],
                    vec![nav],
                    vec![],
                    &mut parses_by_lang,
                );
                // A5.8: POSITION on each field, and the MODULE CONTAINS
                // fallback, since no function in a schema owns a field.
                if let Some(fp) = parses_by_lang.get_mut("graphql").and_then(|v| v.last_mut()) {
                    repo_graph_code_extractors::anchor::attach(fp, path, module_id, &mut anchors);
                }
            }
            continue;
        }

        // A10.6: an Avro `.avsc` is the shared contract under a Kafka stack.
        // Its named types (record / enum / fixed) become MESSAGE_TYPE nodes
        // under the file's MODULE, the shape A10.5 gives a `.proto` message.
        if lang == "avro" {
            let module_id = synthetic_module_id(repo, path);
            let recs = repo_graph_code_extractors::schemas::extract_avro_records(
                source, path, module_id, repo,
            );
            avro_files += 1;
            avro_records += recs.message_count;
            avro_enums += recs.enum_count;
            avro_fixed += recs.fixed_count;
            avro_field_records += recs.schema_field_cells;
            avro_fields += recs.schema_fields;
            if !recs.nodes.is_empty() {
                stash_synthetic_parse(
                    "avro",
                    path,
                    module_id,
                    repo,
                    vec![recs.nodes],
                    vec![recs.edges],
                    vec![recs.nav],
                    recs.module_cells,
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // WP-D incremental: reuse the cached parse if the source is unchanged;
        // only changed / new files pay tree-sitter.
        let hash = cache.is_some().then(|| cache::content_hash(source));
        if let Some(h) = hash {
            current.push((path.clone(), h));
        }
        let cached_fp = match hash {
            Some(h) => cache.as_deref().and_then(|c| c.get(path, h, lang)),
            None => None,
        };
        if let Some(fp) = cached_fp {
            live_paths.insert(path.clone());
            parses_by_lang.entry(lang).or_default().push(fp);
            continue;
        }

        // Per-file panic isolation. Parsers occasionally hit slice/regex bugs
        // on adversarial inputs (e.g. parsers/code/rust/src/lib.rs:511 slice
        // OOB on glia's own source as of 2026-05-09). One bad file shouldn't
        // kill an N-file repo build — log it, skip it, keep going.
        let parse_result = catch_unwind(AssertUnwindSafe(|| {
            let mut fp = parse_one_with(source, path, lang, repo, go_module_prefix)?;
            // LC.3a: the edges present now are the parser's own. Stamped here,
            // inside the closure and before the extractors, so the parse cache
            // stores the stamp and a cache hit replays it.
            evidence::stamp_missing(&mut fp.edges, &format!("parser:{lang}"));
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let mut stats = ExtractStats::default();
            apply_cross_cutting_extractors(
                &mut fp, source, path, lang, module_id, repo, &mut stats,
            );
            // G15: denormalize the file's library names onto every node as an
            // IMPORTS cell (one place, all languages). This is the RAW list:
            // telling a dependency from the repo's own module needs the whole
            // repo, so `build::filter_imports_cells` rewrites it in place after
            // the cache (A16.4). The cell also marks a language-parser parse —
            // synthetic parses above never get one.
            repo_graph_code_domain::attach_imports_cell(&mut fp, lang);
            Ok::<_, String>((fp, stats))
        }));
        match parse_result {
            Ok(Ok((fp, stats))) => {
                nav_routes_marked += stats.nav_routes;
                nav_bound += stats.nav_bound;
                nav_redirects += stats.nav_redirects;
                nav_children += stats.nav_children;
                nav_rejected += stats.nav_rejected;
                nav_catchalls += stats.nav_catchalls;
                ts_client_calls_skipped += stats.ts_client_calls_skipped;
                links_router += stats.nav_links_router;
                links_href += stats.nav_links_href;
                links_origin += stats.nav_links_origin;
                links_dynamic += stats.nav_links_dynamic;
                if let Some(h) = hash {
                    pending.push((path.clone(), h, lang, fp.clone()));
                }
                live_paths.insert(path.clone());
                parses_by_lang.entry(lang).or_default().push(fp);
            }
            Ok(Err(e)) => {
                parse_errors.push(format!("{path}: {e}"));
            }
            Err(payload) => {
                parse_errors.push(format!(
                    "{path}: PANIC ({lang} parser/extractors): {}",
                    panic_payload_str(&payload)
                ));
            }
        }
    }

    // WP-D: diff the inputs against the pre-build cache (LA.12), store the
    // fresh parses, evict cached parses for files gone (or no longer
    // parseable) this build, and emit the greppable marker so a cycle can
    // confirm the cache engaged.
    if let Some(c) = cache.as_deref_mut() {
        let diff = c.diff(&current);
        for (path, h, lang, fp) in pending {
            c.put(path, h, lang, fp);
        }
        c.retain_paths(&live_paths);
        c.record_diff(diff);
        // `stamp=` makes the always-on line grep-proof of WHICH code produced
        // these parses — a rebuilt wheel that quietly kept the old .so shows the
        // old stamp here (dev-notes memory: feedback_maturin_stale_wheel). The
        // repo prefix gives a multi-repo build one attributable line per repo.
        // `source=diff` (LA.12 fired_on): the counts are `ParseCache::diff`'s
        // list lengths, the same lists `ParseCache::last_diff` hands out.
        eprintln!(
            "[incremental] {repo_label}: reused {}, reparsed {}, evicted {} (parse cache, stamp={}, source=diff)",
            c.stats.reused,
            c.stats.reparsed,
            c.stats.evicted,
            cache::CACHE_VERSION
        );
    }

    // A3.4 fired_on marker: client-router ROUTE nodes were tagged
    // `provenance: nav_route` so the HTTP route index can skip them.
    // LB.4c: `page-qnamed` counts the nav pages minted in the `page:<path>`
    // qname namespace across EVERY parse — cache-served ones too, and dart's
    // go_router pages, which have no stats channel into `nav_routes_marked`.
    // A sum of per-file counts, so HashMap iteration order cannot leak into it.
    // Kind-gated to ROUTE: a module qnamed `page` (a root `page.tsx`) has
    // children `page::X`, which a bare prefix test would miscount.
    // LA.6b fired_on: the second parenthesis is the shared route-table
    // walker's work on the reparsed files - route -> component refs, redirect
    // refs, composed child routes, `path` objects refused as route records,
    // wildcard routes. Only printed when a build has a page or refused a record.
    let page_qnamed: usize = parses_by_lang
        .values()
        .flatten()
        .map(|fp| {
            fp.nav
                .qname_by_id
                .iter()
                .filter(|(id, q)| {
                    q.starts_with("page:") && fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE)
                })
                .count()
        })
        .sum();
    if nav_routes_marked > 0 || page_qnamed > 0 || nav_rejected > 0 {
        eprintln!(
            "[extract] nav-routes marked: {nav_routes_marked} (page-qnamed {page_qnamed}) \
             (bound={nav_bound} redirects={nav_redirects} children={nav_children} \
             rejected={nav_rejected} catchall={nav_catchalls})"
        );
    }
    // A3.5 fired_on marker: ts_routes declined to mint a server ROUTE from an
    // HTTP-client call (`this.http.get('/users')`). Only printed when it did.
    if ts_client_calls_skipped > 0 {
        eprintln!("[extract] ts-routes client-calls skipped: {ts_client_calls_skipped}");
    }

    // LA.6c fired_on marker: navigation link sites became NAVIGATES_TO refs
    // (`router` includes the `origin` share links; `template` counts the refs
    // read from `.component.html`, which are also in `router` / `href`).
    // Printed once per repo build that emitted a link.
    if links_router + links_href > 0 {
        eprintln!(
            "[nav-links] router={links_router} href={links_href} origin={links_origin} \
             template={links_template} dynamic_skipped={links_dynamic}"
        );
    }

    // A5.1 fired_on marker: a `.proto` is now a first-class parsed file, not a
    // floating service node. Only printed when a build actually saw one.
    if proto_files > 0 {
        eprintln!(
            "[proto] {proto_files} files -> {proto_services} services, {proto_rpcs} rpcs, {proto_packages} packages"
        );
        // A10.5 fired_on marker: the declared message / enum types. Its own
        // line (`files=` shape) so A5.1's line above keeps its format.
        eprintln!(
            "[proto] files={proto_files} services={proto_services} messages={proto_messages} enums={proto_enums}"
        );
    }

    // A10.4 fired_on marker: `.graphql` / `.gql` files are walked and their
    // SDL fields are GRAPHQL_RESOLVER nodes. `files` counts every schema
    // file routed, so `resolvers=0` flags operation-only documents.
    if sdl_files > 0 {
        eprintln!("[graphql-sdl] files={sdl_files} resolvers={sdl_resolvers}");
    }

    // A10.6 fired_on marker: `.avsc` files are walked and their named types
    // are MESSAGE_TYPE nodes. `files` counts every schema routed, so all-zero
    // counts flag a malformed or bare-type-string `.avsc`.
    if avro_files > 0 {
        eprintln!(
            "[avro] files={avro_files} records={avro_records} enums={avro_enums} fixed={avro_fixed}"
        );
    }

    // LE.10a fired_on marker: proto messages and Avro records carry their
    // declared fields as a SCHEMA_FIELDS cell. Only printed when a build gave
    // at least one message / record that cell.
    if proto_field_messages + avro_field_records > 0 {
        eprintln!(
            "[schema-fields] proto_messages={proto_field_messages} proto_fields={proto_fields} avro_records={avro_field_records} avro_fields={avro_fields}"
        );
    }

    // LA.16 (A10.12) fired_on marker: sniffed JSON Schema files are routed and
    // their root / `$defs` object types are MESSAGE_TYPE nodes. `files` counts
    // every file routed, so `types=0` flags an admitted look-alike.
    if jsonschema_files > 0 {
        eprintln!(
            "[jsonschema] files={jsonschema_files} types={jsonschema_types} defs={jsonschema_defs}"
        );
    }

    // LB.9a fired_on marker: the non-code MODULEs this build minted, each
    // named by its full file name (`api::user.proto`). Counted per lang key
    // through a BTreeMap, so no HashMap order reaches the line.
    let synthetic: BTreeMap<&str, usize> = SYNTHETIC_MODULE_KEYS
        .iter()
        .filter_map(|k| parses_by_lang.get(k).map(|v| (*k, v.len())))
        .filter(|(_, n)| *n > 0)
        .collect();
    let synthetic_total: usize = synthetic.values().sum();
    if synthetic_total > 0 {
        let per_key: Vec<String> = synthetic.iter().map(|(k, n)| format!("{k}={n}")).collect();
        eprintln!(
            "[modules] non-code MODULEs named by file: {synthetic_total} ({}) repo={repo_label}",
            per_key.join(" ")
        );
    }

    // A10.1 fired_on marker: the repo's own API contract is now substrate.
    // Only printed when a build actually saw a spec file.
    let contract_ops = contracts.openapi + contracts.asyncapi + contracts.pact;
    if contract_ops > 0 {
        eprintln!(
            "[contract] files={} ops={contract_ops} (openapi={} asyncapi={} pact={})",
            contracts.files, contracts.openapi, contracts.asyncapi, contracts.pact
        );
    }
    // LE.10b fired_on marker: contract ops carry their declared request /
    // response / payload body fields as a SCHEMA_FIELDS cell. Only printed
    // when a build gave at least one op that cell.
    if contracts.ops_with_fields > 0 {
        eprintln!(
            "[contract] fields ops_with_fields={} fields={} refs_resolved={} refs_external={}",
            contracts.ops_with_fields,
            contracts.fields,
            contracts.refs_resolved,
            contracts.refs_external
        );
    }

    // LC.3a: the synthetic parses (`stash_synthetic_parse`: yaml, proto,
    // graphql, ...) and the anchor pass run on the graphql one carry no
    // evidence yet. Only their keys are swept: a language parse was stamped
    // inside the parse closure, so an edge missing there is an unattributed
    // emitter, left missing for the `[evidence]` count (and the corpus test)
    // to name instead of being filed under `extractor:<lang>`. Per FileParse,
    // so HashMap order cannot leak.
    for lang_key in SYNTHETIC_MODULE_KEYS {
        let Some(parses) = parses_by_lang.get_mut(lang_key) else {
            continue;
        };
        let emitter = format!("extractor:{lang_key}");
        for fp in parses.iter_mut() {
            evidence::stamp_missing(&mut fp.edges, &emitter);
        }
    }

    (parses_by_lang, parse_errors)
}

/// Best-effort string extraction from a panic payload returned by
/// `catch_unwind`. The payload is `Box<dyn Any + Send>` and the message is
/// commonly a `&'static str` (from `panic!("literal")`) or `String` (from
/// `panic!("{}", x)` / `unwrap`).
fn panic_payload_str(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic>".to_string()
    }
}

/// The lang keys [`stash_synthetic_parse`] files a non-code MODULE under (the
/// `[modules]` marker counts these). A new non-code branch adds its key here;
/// `synthetic_modules_are_named_by_file_name` fails on a routed key it lacks.
const SYNTHETIC_MODULE_KEYS: [&str; 10] = [
    "avro",
    "dockerfile",
    "dotenv",
    "graphql",
    "json",
    "manifest",
    "migration",
    "prisma",
    "proto",
    "yaml",
];

/// A non-code file's MODULE id (LB.9a): keyed on [`synthetic_module_qname`],
/// the full file name, so `api/user.proto` never shares a NodeId with
/// `api/user.go`, nor `svc/Dockerfile.prod` with `svc/Dockerfile`. Every
/// branch that stashes through [`stash_synthetic_parse`] mints its id here,
/// the id that function records under the same qname.
fn synthetic_module_id(repo: RepoId, path: &str) -> NodeId {
    NodeId::from_parts(
        GRAPH_TYPE,
        repo,
        node_kind::MODULE,
        &synthetic_module_qname(path),
    )
}

fn stash_synthetic_parse(
    lang_key: &'static str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    node_groups: Vec<Vec<Node>>,
    edge_groups: Vec<Vec<Edge>>,
    nav_groups: Vec<CodeNav>,
    // Cells for the synthetic MODULE node itself (e.g. a `.proto`'s whole-file
    // POSITION). Empty for the extractors that have nothing to say about the
    // file as a whole.
    module_cells: Vec<Cell>,
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
) {
    let mut nodes = vec![Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: module_cells,
    }];
    let mut edges = Vec::new();
    let mut merged_nav = CodeNav::default();
    merged_nav.record(
        module_id,
        path.rsplit('/').next().unwrap_or(path),
        &synthetic_module_qname(path),
        node_kind::MODULE,
        None,
    );
    for group in node_groups {
        nodes.extend(group);
    }
    for group in edge_groups {
        edges.extend(group);
    }
    for nav in nav_groups {
        merge_nav(&mut merged_nav, nav);
    }
    let fp = FileParse {
        nodes,
        edges,
        nav: merged_nav,
        ..Default::default()
    };
    parses_by_lang.entry(lang_key).or_default().push(fp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{cell_type, edge_category};
    use repo_graph_core::CellPayload;

    fn route(path: &str, source: &str) -> HashMap<&'static str, Vec<FileParse>> {
        let files = vec![(path.to_string(), source.to_string())];
        let (parses, errors) = parse_repo_files(&files, RepoId(1), "", None, "test");
        assert!(errors.is_empty(), "{errors:?}");
        parses
    }

    /// A13.9: a migration `.sql` becomes a MODULE whose ACCESSES_DATA edges
    /// name the tables its DDL touches; detect_language never sees it.
    #[test]
    fn migration_sql_routes_to_the_ddl_scan() {
        assert_eq!(detect_language("db/migrations/V1__create_users.sql"), None);
        let sql = "-- users\nCREATE TABLE IF NOT EXISTS users (id INT);\n\
                   ALTER TABLE ONLY users ADD COLUMN email TEXT;\n";
        let parses = route("db/migrations/V1__create_users.sql", sql);
        let fps = &parses["migration"];
        assert_eq!(fps.len(), 1);
        let fp = &fps[0];
        let module_id = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::MODULE,
            "db::migrations::V1__create_users.sql",
        );
        let users = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::DATA_ENTITY,
            "data_entity:sql:users",
        );
        assert_eq!(fp.nodes[0].id, module_id, "the migration file is a MODULE");
        assert_eq!(fp.nodes.len(), 2, "MODULE + one DATA_ENTITY: {:?}", fp.nav.qname_by_id);
        assert_eq!(fp.edges.len(), 1);
        assert_eq!((fp.edges[0].from, fp.edges[0].to), (module_id, users));
        assert_eq!(fp.edges[0].category, edge_category::ACCESSES_DATA);

        // Admitted but table-less: no stash, no empty MODULE.
        let parses = route("db/migrations/V2__noop.sql", "-- nothing yet\n");
        assert!(!parses.contains_key("migration"));
    }

    /// A13.16: a `.prisma` file becomes a MODULE (`prisma::schema.prisma`,
    /// named `schema.prisma`) whose models are model-keyed DATA_ENTITYs; a
    /// prismaSchemaFolder model file takes the flavor of the datasource
    /// another file declares.
    #[test]
    fn prisma_schema_routes_to_the_model_scan() {
        assert_eq!(detect_language("prisma/schema.prisma"), None);
        let schema = "datasource db {\n  provider = \"postgresql\"\n}\n\
                      model User {\n  id Int @id\n  @@map(\"app_users\")\n}\n";
        let parses = route("prisma/schema.prisma", schema);
        let fp = &parses["prisma"][0];
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, "prisma::schema.prisma");
        let user = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::DATA_ENTITY,
            "data_entity:sql:User",
        );
        assert_eq!(fp.nodes[0].id, module_id, "the schema file is a MODULE");
        assert_eq!(fp.nav.name_by_id[&module_id], "schema.prisma");
        assert_eq!(fp.nodes[1].id, user);
        assert_eq!(
            repo_graph_code_domain::data_entity::table_of(&fp.nodes[1].cells),
            Some("app_users".to_string())
        );
        assert_eq!((fp.edges[0].from, fp.edges[0].to), (module_id, user));
        assert_eq!(fp.edges[0].category, edge_category::ACCESSES_DATA);

        let files = vec![
            (
                "prisma/schema/main.prisma".to_string(),
                "datasource db {\n  provider = \"mongodb\"\n}\n".to_string(),
            ),
            (
                "prisma/schema/event.prisma".to_string(),
                "model Event {\n  id String @id\n}\n".to_string(),
            ),
        ];
        let (parses, errors) = parse_repo_files(&files, RepoId(1), "", None, "test");
        assert!(errors.is_empty(), "{errors:?}");
        let fps = &parses["prisma"];
        assert_eq!(fps.len(), 1, "the model-less datasource file stashes nothing");
        let event = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::DATA_ENTITY,
            "data_entity:nosql:Event",
        );
        assert_eq!(fps[0].nodes[1].id, event);
    }

    #[test]
    fn graphql_schema_files_route_to_the_sdl_scan() {
        assert_eq!(detect_language("api/schema.graphql"), Some("graphql"));
        assert_eq!(detect_language("api/schema.gql"), Some("graphql"));

        let sdl = "type Query {\n  getUser(id: ID!): User\n}\n\ntype User {\n  id: ID!\n}\n";
        let parses = route("api/schema.graphql", sdl);
        let fps = &parses["graphql"];
        assert_eq!(fps.len(), 1);
        let fp = &fps[0];

        let module_id =
            NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, "api::schema.graphql");
        assert_eq!(fp.nodes[0].id, module_id, "the schema file is a MODULE");

        let mut resolvers: Vec<&str> = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRAPHQL_RESOLVER))
            .filter_map(|n| fp.nav.qname_by_id.get(&n.id).map(String::as_str))
            .collect();
        resolvers.sort_unstable();
        assert_eq!(resolvers, ["graphql_resolver:Query", "graphql_resolver:getUser"]);

        let get_user = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:getUser",
        );
        let node = fp.nodes.iter().find(|n| n.id == get_user).expect("getUser node");
        let position = node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION)
            .map(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            });
        assert_eq!(
            position.as_deref(),
            Some(r#"{"file":"api/schema.graphql","start_line":1,"end_line":1}"#)
        );
        assert!(fp.edges.iter().any(|e| e.from == module_id
            && e.to == get_user
            && e.category == edge_category::CONTAINS));
        assert!(
            !fp.nav.kind_by_id.values().any(|k| *k == node_kind::GRAPHQL_OPERATION),
            "a schema declares no client operations"
        );
    }

    #[test]
    fn avro_schema_files_route_to_the_message_type_scan() {
        assert_eq!(detect_language("schemas/user.avsc"), Some("avro"));

        let avsc = "{\"type\": \"record\", \"name\": \"User\", \"namespace\": \"com.shop\",\n \"fields\": [{\"name\": \"id\", \"type\": \"string\"}]}\n";
        let parses = route("schemas/user.avsc", avsc);
        assert_eq!(parses.keys().copied().collect::<Vec<_>>(), ["avro"]);
        let fp = &parses["avro"][0];

        let module_id =
            NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, "schemas::user.avsc");
        assert_eq!(fp.nodes[0].id, module_id, "the schema file is a MODULE");
        let module_pos = fp.nodes[0].cells.iter().find(|c| c.kind == cell_type::POSITION);
        assert!(
            matches!(module_pos.map(|c| &c.payload), Some(CellPayload::Json(s))
                if s == r#"{"file":"schemas/user.avsc","start_line":0,"end_line":1}"#),
            "{module_pos:?}"
        );
        let user = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::MESSAGE_TYPE,
            "message:avro:com.shop.User",
        );
        assert_eq!(fp.nav.kind_by_id.get(&user), Some(&node_kind::MESSAGE_TYPE));
        assert_eq!(fp.nav.parent_of.get(&user), Some(&module_id));

        assert!(route("schemas/broken.avsc", "{\"type\": ").is_empty(), "malformed: no MODULE");
    }

    /// LA.16 (A10.12): a JSON Schema reaches the MESSAGE_TYPE scan in the
    /// existing "json" group, while an OpenAPI document whose component
    /// schemas also sniff as schema-shaped stays on the contract path.
    #[test]
    fn json_schema_files_route_to_the_message_type_scan() {
        let schema = "{\n  \"title\": \"Refund\",\n  \"type\": \"object\",\n  \"properties\": {}\n}\n";
        let parses = route("schemas/refund.schema.json", schema);
        assert_eq!(parses.keys().copied().collect::<Vec<_>>(), ["json"]);
        let fp = &parses["json"][0];
        let module_id = fp.nodes[0].id;
        assert_eq!(fp.nav.kind_by_id.get(&module_id), Some(&node_kind::MODULE));
        assert!(
            matches!(fp.nodes[0].cells.first().map(|c| &c.payload), Some(CellPayload::Json(s))
                if s == r#"{"file":"schemas/refund.schema.json","start_line":0,"end_line":4}"#),
            "the schema file's MODULE carries its whole-file POSITION"
        );
        let refund =
            NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MESSAGE_TYPE, "message:jsonschema:Refund");
        assert_eq!(fp.nav.kind_by_id.get(&refund), Some(&node_kind::MESSAGE_TYPE));
        assert_eq!(fp.nav.parent_of.get(&refund), Some(&module_id));

        let openapi = r#"{"openapi":"3.0.3","paths":{"/users":{"get":{}}},
            "components":{"schemas":{"User":{"type":"object","properties":{}}}}}"#;
        let parses = route("openapi.json", openapi);
        let fp = &parses["json"][0];
        let kinds: Vec<_> = fp.nav.kind_by_id.values().copied().collect();
        assert!(kinds.contains(&node_kind::DOC_SECTION), "still a contract op");
        assert!(!kinds.contains(&node_kind::MESSAGE_TYPE), "a contract is never a JSON Schema");

        assert!(
            route("testdata/settings.json", r#"{"config":{"type":"object","properties":{}}}"#).is_empty(),
            "a nested look-alike stashes nothing, not even a MODULE"
        );
    }

    /// LA.6c: a `.component.html` template becomes a bare parse under the
    /// `angular` key carrying only its link refs, from the component's MODULE
    /// id; it mints no node (the MODULE is the `.component.ts` parse's).
    #[test]
    fn component_templates_route_to_bare_link_parses() {
        let html = "<a routerLink=\"/home\">h</a>\n<a href=\"/favicon.ico\">i</a>\n";
        let parses = route("src/app/home/home.component.html", html);
        assert_eq!(parses.keys().copied().collect::<Vec<_>>(), ["angular"]);
        let fp = &parses["angular"][0];
        assert!(fp.nodes.is_empty() && fp.edges.is_empty() && fp.nav.qname_by_id.is_empty());
        let module_id = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::MODULE,
            "src::app::home::home.component",
        );
        assert_eq!(fp.refs.len(), 1);
        assert_eq!(
            (fp.refs[0].from, fp.refs[0].from_module),
            (module_id, module_id)
        );
        assert_eq!(fp.refs[0].category, edge_category::NAVIGATES_TO);

        assert!(route("src/app/home/empty.component.html", "<p>no links</p>").is_empty());
    }

    #[test]
    fn graphql_operation_documents_mint_nothing() {
        let doc = "query getUser($id: ID!) {\n  getUser(id: $id) { id }\n}\n";
        assert!(route("client/getUser.graphql", doc).is_empty());
    }

    /// The `cli:<bin>` CLI_COMMANDs a manifest's MODULE parents, by qname.
    fn manifest_cli(fp: &FileParse, module_qname: &str) -> Vec<String> {
        let module_id = NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, module_qname);
        assert_eq!(fp.nodes[0].id, module_id, "the manifest is a MODULE");
        let mut out: Vec<String> = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::CLI_COMMAND))
            .inspect(|n| assert_eq!(fp.nav.parent_of.get(&n.id), Some(&module_id)))
            .filter_map(|n| fp.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        out.sort_unstable();
        out
    }

    /// LA.20c: a manifest's declared binaries ride the synthetic `manifest`
    /// parse beside its PACKAGE_DEPs, and a manifest that declares a binary
    /// but no dependency is stashed all the same.
    #[test]
    fn manifest_binaries_join_the_manifest_parse() {
        let pkg = r#"{"name": "@acme/shipit", "bin": {"shipit": "bin/cli.js"}, "dependencies": {"commander": "12.0.0"}}"#;
        let parses = route("server/package.json", pkg);
        let fp = &parses["manifest"][0];
        assert_eq!(manifest_cli(fp, "server::package.json"), ["cli:shipit"]);
        assert!(
            fp.nav.qname_by_id.values().any(|q| q == "package:npm:commander"),
            "the dependency is still read"
        );

        let scripts_only = "[project.scripts]\nmytool = \"mytool.cli:cli\"\n";
        let parses = route("server/pyproject.toml", scripts_only);
        assert_eq!(manifest_cli(&parses["manifest"][0], "server::pyproject.toml"), ["cli:mytool"]);

        let bin_only = "[package]\nname = \"tools\"\n\n[[bin]]\nname = \"migrate\"\n";
        let parses = route("Cargo.toml", bin_only);
        assert_eq!(manifest_cli(&parses["manifest"][0], "Cargo.toml"), ["cli:migrate"]);

        // Neither a dependency nor a binary: nothing is stashed.
        assert!(route("package.json", r#"{"name": "empty"}"#).is_empty());
    }

    /// LB.9a: every non-code branch names its MODULE by the full file name,
    /// the display name the MODULE already carried, so a `.proto` beside a
    /// same-stem `.go`, a `.json` beside a same-stem `.yaml`, and a
    /// `Dockerfile.prod` beside a `Dockerfile` are separate MODULEs. One case
    /// per branch that stashes through `stash_synthetic_parse`.
    #[test]
    fn synthetic_modules_are_named_by_file_name() {
        assert_eq!(
            synthetic_module_qname("svc/Dockerfile.prod"),
            "svc::Dockerfile.prod"
        );
        assert_eq!(synthetic_module_qname(".env.local"), ".env.local");
        assert_eq!(synthetic_module_qname("openapi.json"), "openapi.json");
        assert_eq!(
            synthetic_module_qname(r"api\v1\user.proto"),
            "api::v1::user.proto"
        );
        assert_eq!(
            path_to_qname("api/user.proto"),
            "api::user",
            "the code form keeps the stem"
        );

        let cases: [(&str, &str, &str, &str); 13] = [
            (
                "api/user.proto",
                "syntax = \"proto3\";\npackage user;\nservice UserService {\n  rpc GetUser (GetUserRequest) returns (User);\n}\nmessage GetUserRequest { string id = 1; }\nmessage User { string id = 1; }\n",
                "proto",
                "api::user.proto",
            ),
            (
                "docs/swagger.json",
                r#"{"openapi":"3.0.0","paths":{"/orders":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
                "json",
                "docs::swagger.json",
            ),
            (
                "docs/swagger.yaml",
                "openapi: 3.0.0\npaths:\n  /users:\n    get:\n      responses:\n        '200':\n          description: ok\n",
                "yaml",
                "docs::swagger.yaml",
            ),
            (
                "svc/Dockerfile.prod",
                "FROM alpine\nENV PORT=9090\n",
                "dockerfile",
                "svc::Dockerfile.prod",
            ),
            (
                "svc/api.dockerfile",
                "FROM alpine\nENV PORT=9090\n",
                "dockerfile",
                "svc::api.dockerfile",
            ),
            (".env.local", "KEY=1\n", "dotenv", ".env.local"),
            (
                "k8s/cron.yml",
                "apiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: nightly\nspec:\n  schedule: \"0 2 * * *\"\n",
                "yaml",
                "k8s::cron.yml",
            ),
            (
                "web/package.json",
                r#"{"name": "web", "dependencies": {"react": "18.0.0"}}"#,
                "manifest",
                "web::package.json",
            ),
            (
                "db/migrations/V1__init.sql",
                "CREATE TABLE users (id INT);\n",
                "migration",
                "db::migrations::V1__init.sql",
            ),
            (
                "prisma/schema.prisma",
                "datasource db {\n  provider = \"postgresql\"\n}\nmodel User {\n  id Int @id\n}\n",
                "prisma",
                "prisma::schema.prisma",
            ),
            (
                "api/schema.graphql",
                "type Query {\n  getUser(id: ID!): String\n}\n",
                "graphql",
                "api::schema.graphql",
            ),
            (
                "schemas/user.avsc",
                "{\"type\": \"record\", \"name\": \"User\", \"fields\": []}\n",
                "avro",
                "schemas::user.avsc",
            ),
            (
                "schemas/refund.schema.json",
                "{\n  \"title\": \"Refund\",\n  \"type\": \"object\",\n  \"properties\": {}\n}\n",
                "json",
                "schemas::refund.schema.json",
            ),
        ];
        for (path, source, key, qname) in cases {
            let parses = route(path, source);
            assert_eq!(parses.keys().copied().collect::<Vec<_>>(), [key], "{path}");
            assert!(
                SYNTHETIC_MODULE_KEYS.contains(&key),
                "{key} feeds the [modules] marker"
            );
            let fp = &parses[key][0];
            let module_id = NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, qname);
            assert_eq!(
                fp.nodes[0].id, module_id,
                "{path}: the MODULE is keyed on the file name"
            );
            assert_eq!(
                fp.nav.qname_by_id.get(&module_id).map(String::as_str),
                Some(qname)
            );
            assert_eq!(
                fp.nav.name_by_id.get(&module_id).map(String::as_str),
                qname.rsplit("::").next(),
                "{path}: the name is the qname's last segment"
            );
        }

        // The one exception: an Angular template mints no MODULE and its refs
        // go out from the `.component.ts` MODULE, which keeps the code form.
        let files = vec![
            (
                "src/app/home/home.component.ts".to_string(),
                "import { Component } from '@angular/core';\n\
                 @Component({ selector: 'app-home', templateUrl: './home.component.html' })\n\
                 export class HomeComponent {}\n"
                    .to_string(),
            ),
            (
                "src/app/home/home.component.html".to_string(),
                "<a routerLink=\"/home\">h</a>\n".to_string(),
            ),
        ];
        let (parses, errors) = parse_repo_files(&files, RepoId(1), "", None, "test");
        assert!(errors.is_empty(), "{errors:?}");
        let ts_module = parses
            .values()
            .flatten()
            .find_map(|fp| {
                let first = fp.nodes.first()?.id;
                (fp.nav.kind_by_id.get(&first) == Some(&node_kind::MODULE)).then_some(first)
            })
            .expect("the .component.ts MODULE");
        assert_eq!(
            ts_module,
            NodeId::from_parts(
                GRAPH_TYPE,
                RepoId(1),
                node_kind::MODULE,
                "src::app::home::home.component"
            )
        );
        let template = parses["angular"]
            .iter()
            .find(|fp| fp.nodes.is_empty())
            .expect("the template's bare parse");
        assert_eq!(template.refs.len(), 1);
        assert_eq!(template.refs[0].from_module, ts_module);
    }
}
