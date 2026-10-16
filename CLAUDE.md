# CLAUDE.md (glia engine)

This file provides guidance to Claude Code when working in the Rust workspace. The split is **done** — this is its own repo (`James-Chahwan/glia`, mirrored to GitLab), not a subdirectory of repo-graph, and these docs travelled with it.

**Companion docs:** `WORKFLOW.md` is the session rhythm (orient→work→record, knowledge routing across the four memory systems, ≥100-instance pre-flight, house style) — **run `/orient` at the start of every session**. `CODE_RULES.md` is the operational code conventions. This file is architecture.

## What This Is

**glia** is the Rust engine behind repo-graph. It parses source, builds a unified cross-language graph, stores it in a zero-copy `.gmap` file (rkyv + mmap), runs Personalised PageRank activation over it, and projects it to dense text or structured forms.

Designed to be domain-agnostic: code is the first primitive, but other domains (video, molecules, policy, climate) slot in via the same registry model. See `dev-notes/glia-memory/project_050_domain_agnostic.md` for the direction.

## Workspace Layout

```
core/               Node, Edge, QName, shared primitives (no domain assumptions)
code-domain/        Code-specific registries: NodeKind, EdgeCategory, CellType (u32 IDs)
                    + the code DomainTables (profile.rs)
graph/              Per-repo graph builder, universal resolver, cross-graph resolvers
                    src/ is MODULES, not one lib.rs (see "Module layout" below). Its parser
                    crates (python, go, typescript) are [dev-dependencies] only (CD.6a):
                    glia-graph's normal dependency tree holds no tree-sitter
engine/             Orchestration: walk → parse → extract → build → merge → resolve.
                    Owns every parser dependency. src/ is MODULES; lib.rs is a facade
cli/                The `glia` binary
doc-sources/        Tier-4 doc ingestion: Confluence REST, Notion, MediaWiki, a local wiki
                    checkout (`glia docs sync --source confluence|notion|mediawiki|dir`)
                    + the local snapshot
snapshots/          External-input snapshot writers (git history, test reports, SCIP
                    indexes); a build never runs them and reads their output through
                    code-domain::snapshots
store/              .gmap binary format — rkyv + mmap, sharded layout (format 3), and the
                    timeline.gmap sidecar
projection-text/    Dense sigil text output (scopes, defaults, module dedup)
activation/         Spreading activation — domain-agnostic PPR with configurable direction/weights;
                    also the domain-free build-pass registry (`passes`: Stage, PassSpec,
                    PassRegistry) — build-time, so PARSER_STAMP hashes it — and `algo`:
                    GraphSource, CategorySet, the CSR Adjacency index, `algo::reach`
                    (bfs / reachable / reachable_by, O(V+E) per walk; `bfs_levels`, the
                    levelled multi-source BFS with mask, rank and plus modes the rules
                    closures and transitive forbid walk, DA.1), `algo::delta` (graph
                    delta), `algo::cycles` (Tarjan SCC), the 0.5.1 slots
                    `algo::community` (label propagation, seeded Leiden, CD.1a/b),
                    `algo::cut` (Stoer-Wagner / Dinic, CD.2a), `algo::linkpred`
                    (CD.3a), `algo::hubs` (degree / HITS, CD.4a), `algo::minhash`
                    (MinHash + LSH, CD.4d), `algo::timeline` (validity intervals,
                    CD.5a), and 0.5.2's `algo::classes` (identity classes: UnionFind
                    with min-id roots, Classes, and the Quotient GraphSource a walk
                    over a class runs on, DD.1) and `algo::ppr` (push PPR,
                    Andersen-Chung-Lang, over an Adjacency, DE.5); `plan` (one
                    ActivationPlan over the RankingSignal / FilterPredicate / SynthHook
                    hooks) and `profile` (DomainTables / DomainProfile; 0.5.2 adds
                    IdentityRule / IdentityTier and `DomainTables.identity_rules`, DD.3)
rules/              glia-rules (0.5.2, DA): the Datalog evaluator. Depends on glia-core and
                    glia-activation ONLY (the `dependency_guard` test in rules/tests/toy.rs
                    pins it), and is query-time: not a HASHED_ROOTS entry, so a rules change
                    never moves PARSER_STAMP. A string-named IR (ir), one lexer (lexer)
                    shared by the Datalog parser (parser) and the GQL front end (gql/),
                    analysis + strata, a planner, a levelled semi-naive evaluator (eval /
                    exec over relation), the reaches / scc closures over
                    activation::algo (closure), provenance + explain, aggregates, magic
                    sets. A domain plugs in by implementing `source::FactSource`
                    (a GraphSource); the code domain's is engine::rules::CodeFacts, and
                    the toy proof is `cargo test -p glia-rules --test toy` (Q10)
parsers/code/
  python/  go/  typescript/  rust/  java/  csharp/  ruby/  php/  swift/
  c_cpp/   scala/  clojure/  dart/  elixir/  solidity/  terraform/
  kotlin/  — own parser (tree-sitter-kotlin-ng), but ONE graph with java/:
             the JVM family joins in engine/src/build/lang_build.rs
  react/   angular/  vue/    — framework parsers stacked on typescript
  extractors/ — cross-cutting: data_sources, cli, grpc, queues, websocket,
                eventbus, graphql, ts_routes, angular/react/vue route extractors;
                code_guard (private): the one .rs / .py string-literal and comment
                guard (LazyGuard) every needle scanner reads through, so glia's own
                needle tables and test literals mint nothing (CJ.1a-c, `[code-guard]`);
                generated (0.5.2, DE.7): the one generated-file classifier
                (`is_generated`: protoc / codegen names, or a generator banner in the
                first 40 comment lines), read by ORIGIN `generated` and the gRPC
                client needles (a vendored stub mints no GRPC_CLIENT, DE.8)
stamp/              Build identity: RELEASE + PARSER_STAMP (content hash of every
                    graph-shaping source). Keys the parse cache, so a parser fix
                    invalidates caches without a version bump.
py/                 pyo3 bindings — the only Rust crate published to PyPI (as glia-py, module glia_py)
toy-domain/         test-only second domain (publish = false) proving the domain seam; never a dependency of a shipped crate
                    (the rules layer's own toy proof is inline in rules/tests/toy.rs, so this list is unchanged)
engram-export/      (excluded) glia -> engram_core::Gmap exporter; needs ../Engram; build/test only via scripts/check-engram-export.sh.
                    Writes <out> plus <out>.files.json and <out>.meta.json (glia version, BUILD_STAMP,
                    gmap_digest; CK.3), and <out>.glia/, the history layout `--since` reads;
                    `-V` prints glia's version line. 0.5.2: every clone of one commit exports the
                    same bytes (DE.2); ENDPOINTs and `page:` ROUTEs anchor identity hints and
                    `--since` pairs re-keyed endpoints and renamed symbols (DE.3, DE.4);
                    `--ppr-candidates <K>` writes `<out>.ppr.jsonl`, each fact's top-K push-PPR
                    neighbours, bound to the gmap by its digest (DE.5)
```

Parsers live at `parsers/<domain>/<language>/`. When a non-code domain lands (0.5.0 ships the seam, not a domain), its parsers nest alongside `parsers/code/`.

### Module layout — every hot crate is a facade over modules

`lib.rs` (`main.rs` for `cli`) in `engine`, `graph`, `code-domain`, `py` and `cli` is a
**facade**: module declarations plus re-exports, no logic. The 0.5.0 wave 0 (L0.1–L0.6)
split them so the leap's packets edit disjoint files. **Never trust a line number in
older prose — `grep -rn` the symbol name.**

The facade rules:

1. **The pre-0.5.0 API stays flat.** `engine` glob re-exports `answers`, `build`,
   `coverage`, `extract`; `graph` globs `blast`, `build`, `merged`,
   `resolvers`, `types`. A `pub` item in a globbed module IS public API, so a helper
   another module needs is `pub(crate)`, never `pub`. Modules that declare no free `pub`
   items (engine `walk`/`route`/`passes`/`docs`/`endpoint_fold`; graph `calls`/`imports`/
   `signal`/`traversal`) are not globbed. `engine::arch` and `engine::cache` are
   `pub mod` with an explicit, partial flat list.
2. **Every new 0.5.0 / 0.5.1 / 0.5.2 primitive is a public module slot**, reached by
   module path — `glia_engine::delta::graph_delta_vs_rev`, `::persist::load_layout`,
   `::profile::CODE_PROFILE`, `::pack::pack`, `::rules::query`, `::watch::WatchSession`,
   `::identity::identity_classes`, `glia_graph::roles::roles_in` — never flattened into
   the root. The slot's owner fills its file; no later packet edits a facade (in 0.5.0
   only LD.11a, the crate rename, touched `engine/src/lib.rs` after wave 0; in 0.5.1 only
   the wave-0 C0.* packets touched the engine, graph and py facades; in 0.5.2 only the
   wave-0 D0.* packets touched a facade, the workspace Cargo.toml or Cargo.lock).
3. **A spec step "FACADE: `pub use x::{..}`" or "`mod x;`" is already done** — skip it
   and call the item by its module path from `cli` / `py`.
4. Two globbed modules exporting one name raise `ambiguous_glob_reexports`: rename,
   never `#[allow]`. A slot still doc-only at release is dead weight: the release docs
   packet (LG.4b in 0.5.0, CZ.2 in 0.5.1, DZ.1 in 0.5.2) fails on any slot file whose
   only lines are `//!` (`grep -v '^//!' <file> | grep -c .` is 0), and a dropped owner's
   slot is deleted with its `mod` line. At 0.5.2 every tracked `.rs` file has code, the
   34 D0 slot files included (none was dropped; D0.8's `cmd/query/query.rs` became
   `rules_query.rs` in DB.5).

```
engine/src/   lib.rs        facade (rules above)
              walk.rs       repo walk, gitignore, region graph; plan_walk / read_walk (DC.1:
                            a held walk table reuses each file whose stat stamp did not
                            move, `[walk] reuse root=<r> reused=<n> read=<m> of <t>`); the
                            HOCON `.conf` files akka-quartz names schedules in (DG.7b); sbt
                            sub-projects as PROJECT roots (DG.12); requirements.txt /
                            Pipfile python roots (DE.9); a `[[project]]` label names a
                            detected root whose manifest names nothing (`relabelled=`)
              route.rs      per-file routing (yaml/Dockerfile/manifest/dotenv/.proto), parse cache;
                            ModuleQnames::plan names each file's MODULE (same-stem code files,
                            LB.9b / LB.13, and every C/C++ file, LB.10a, are named by file
                            name; graph/src/imports.rs `SameStem` picks the importer's
                            sibling, `[imports] same-stem picks`)
              extract.rs    detect_language, parse_one, parse_one_with, cross-cutting extractors
              build/        mod.rs         GenerateResult, generate_one* / generate_many* pipeline;
                                           assemble_many (generate_many before its passes run);
                                           DC.2's callable stages: aux_graphs (the region /
                                           project / docs graphs) and finish_build (the
                                           CODE_PASSES tail), so WatchSession runs the same fns
                            assemble.rs    build_graphs_for_repo, build_const_table (panics go
                                           quiet through parallel::quiet; SuppressPanicHook is gone);
                                           parse_and_graft (route / parse / extract + grafts) and
                                           repo_markers, the per-repo stages (DC.2)
                            grafts.rs      apply_post_cache — every post-cache graft goes here;
                                           0.5.2 adds the external WS / gRPC / GraphQL client
                                           marking (endpoint_fold::mark_external_clients, DF.8),
                                           apply_python_signals (DF.13), stamp_annotated_bases
                                           (DF.19) and graft_quartz_conf_schedules (DG.7b); the
                                           queue / event const folds run on the pool (DC.3,
                                           `[const-fold] pooled`)
                            rpc_needles.rs RpcContext, apply_rpc_needles, graft_rpc_markers
                            lang_build.rs  plan_groups / build_groups / fold_groups — the
                                           per-language build groups (DC.2; build_groups takes a
                                           reuse slot a watch fills with cached builds) + markers;
                                           GroupCtx holds every repo-level builder input, and
                                           watch.rs hidden_fp destructures it exhaustively, so a
                                           new input fails to compile until it joins a row (DC.7b);
                                           rust_crates (the Cargo packages build_rust resolves
                                           against), DartPackages (pubspec names, DF.4a)
                            memo.rs        BuildMemo on ParseCache (DC.4): per-file const-table
                                           scans, RPC needle halves and pure non-code routing
                                           outcomes replayed for unchanged files by a caller that
                                           holds a cache across builds (watch, in-process rev
                                           builds), `[graft-memo] const hit=..`; in memory only
                            c_includes.rs  IncludeRoots / IncludeResolver — C/C++ include search
                                           roots (compile_commands.json, CMake, include/; CB.22)
                            timing.rs      the per-phase `[timing]` stderr timers (CA.9)
              docs.rs       markdown ingest; include_doc / DocRule: the well-known files and docs/
                            at the repo root and every PROJECT root, .ai/, ADR dirs, SDD feature
                            docs (features/<f>/*.md, spec-kit specs/<NNN-slug>/; CJ.2, `[docs] scope:`)
              passes.rs     doc-linker (an ambiguous bare mention binds the least-qname
                            candidate, DE.1), TESTS edges (module pairs, a `[walk] tests`
                            module by stem, rule declared_name, DF.11, then fn-level from a
                            test module's root functions, CL.5b), the post-pass fns; the
                            evidence fill split (DC.5): each language group's intra edges are
                            filled at group build (GroupFill) and Finalize re-fills only what
                            changed (`[evidence-split] graphs=..`)
              profile.rs    CODE_PASSES — every build pass in run order (14 Resolve passes: 13
                            resolvers then identity, 9 post-passes, evidence fill + determinism
                            sort); run_code_passes_with(merged, &CodeBuildCtx) is the whole build
                            tail (run_code_passes is gone, DD.7); CodeBuildCtx::for_merge passes a
                            layout merge's recorded SCIP roots, so merge == joint build
              coverage.rs   coverage_report          answers.rs   the P3 primitives
              cache.rs      incremental parse cache (the GLIAPCZ1 lz4 frame, CD.7a)
              arch.rs       service_map (glia arch)
              endpoint_fold.rs  client base-URL fold onto the ENDPOINT path
  public slots (glia_engine::<slot>::<item>), owner (+ extenders):
              pages LA.6e              persist LC.7 (+LC.8, LC.9, LC.10a)   merge LC.10b
              find LD.3b (+LD.6, LD.8a) absence LD.8a        trace LD.4a (+LD.4b)
              implementors LD.7c       serves LD.8b
              profile LD.13, LD.14a (+LD.14b, LD.6, LE.3a, LE.4d)   contract_fields LE.10c
              delta LE.1b              diff_impact LE.2     tests_for LE.3b   effects LE.4d
              why LE.5                 cycles LE.6b         patterns LE.7a    check LE.8
              spec_status LE.9b        gaps LF.2c (+LF.2e, LF.5c)   feature_flows LG.3a
  0.5.1 public slots (C0.2), owner (+ extenders):
              pack CC.4b               review CC.6a (+CC.6b)   flags CC.7b
              contract_breaks CC.8a (+CC.8c)   hotspots CC.10a   cochange CC.11a (+CC.11b)
              communities CD.1d        splits CD.2b (+CD.2c)   hubs CD.4b
              duplicate_flows CD.4e    timeline CD.5c
              shared_cache/ CE.2a (+CE.2b, CE.2d) — directory module, engine half only
                        (the transport is CLI-only); key.rs, export.rs, import.rs, layout.rs
              overlay_loop/ CE.3b (+CE.3c, CE.3d; 0.5.2: DB.9a-c, [[constraint]] candidates
                        judged by the check delta) — directory module; writer.rs, trial.rs,
                        propose.rs, accept.rs, each declared in its mod.rs
  0.5.2 public slots (D0.6), owner (+ extenders):
              rules/   DB.2a (+DB.2b, DB.2c, DB.7c, DB.8a, DB.8b) — directory module: facts.rs
                       (CodeFacts, the code FactSource, CODE_ATTRIBUTES), query.rs (query /
                       validate / query_gql / evaluate_rule), prelude.rs + prelude.dl (the
                       predicate prelude, PRELUDE), reflexion.rs + reflexion.dl (check's
                       reflexion model as rules; the goldens in engine/tests/data/reflexion/
                       are its only oracle: a change to reflexion.dl's semantics regenerates
                       them with GLIA_UPDATE_GOLDEN=1 and declares it, DB.8b)
              watch    DC.7a (+DC.7b, DC.7c) — WatchSession, WatchUpdate, Durability
              identity DD.8 (+DD.9-DD.15) — IdentityView, Collapse, RowIdentity,
                       identity_classes: SAME_AS read into the collapse and walk classes
              memory   DE.10 — release_freed_memory (glibc malloc_trim(0) after a big drop;
                       GLIA_MALLOC_TRIM=0 turns it off, `[memory] malloc_trim site=<s>
                       rss_mb=<before>-><after>`)
  private slots (items pub(crate)):
              http_owner LB.4a         rekey LB.4a (LC.2 edits)   git_rev LE.1b
              adr LF.4b                parallel LG.1a (+LG.1b, LG.1c)
              suspected CD.3b (0.5.1, C0.2)
              external/ LF.1a — directory module; LF.2b, LF.2e, LF.3b, LF.4a, LF.5b, LF.6b
                        add their stage files and declare them in external/mod.rs; 0.5.1
                        adds signals.rs (CC.2: the one typed reader of ATTN / FAIL cells),
                        infer_wrappers.rs (CA.4) and scip.rs (CE.1d, the SCIP build stage)

graph/src/    lib.rs        facade (rules above)
              types.rs build.rs imports.rs calls.rs merged.rs traversal.rs
              blast.rs signal.rs
              resolvers/    one module per mechanism (http, grpc, rpc, queue, graphql,
                            websocket, eventbus, shared_schema, db, cron, config, iac,
                            cli) + identity.rs (IdentityResolver, the SAME_AS stars, D0.5
                            slot, DD.5 + DD.6 + DD.7) + mod.rs; message_schema and package
                            are gone (DD.16: their SHARES_SCHEMA message pairs and
                            SHARES_DEPENDENCY clique are SAME_AS stars now); host.rs is the
                            shared host narrowing (service alias index, narrow_by_host)
                            the http / websocket / grpc / graphql / rpc resolvers use (CB.12);
                            external WS / gRPC / GraphQL clients stay out of pairing (DF.9)
  public slots (glia_graph::<slot>::<item>):
              rust_paths LA.1a (+LA.1b, LA.3)   roles LB.3a (+LA.21a)
              identity LB.6                     cells LF.1a
              nav LA.6a   is_nav_route / nav_route_path, and the NAVIGATES_TO resolver
                          + page-component lift that calls::resolve_refs runs last
  private slots (items pub(crate)):
              go_mounts CB.20 (Go group mounts)   swift_scope CB.18 (SwiftModuleTypes, the
              Swift call hook)   cpp_scope CB.25 (CppScope, C++ name lookup)
  flat 0.5.2 additions: blast_radius_over (blast radius over any GraphSource, so an
              answer walks the identity Quotient, DD.10) and cross_links_with (the service
              links with SAME_AS classes one link per service pair and collapse classes
              counted once, arch's reading, DD.14)

code-domain/src/  lib.rs    the id registries (node_kind, edge_category, cell_type) — ids
                            are allocated here and nowhere else; walk_gating, project_roots
  public slots:     data_entity A13.1   evidence LC.3a   external_inputs LF.1a (+LF.4a)
                    glia_config LF.2a (+LG.3d)   profile LD.14a (+LD.14b, LD.6, LE.4d)
                    snapshots LF.5a (+LF.6a)   code_span CD.7c
                    same_as D0.1 + DD.2 (the eight identity rule names, the shared
                    contract-op key, package normalisation, the SCIP key, content
                    digests); project_roots gains ProjectRoot.label_source
                    (manifest | dir | root | declared, DD.4)

py/src/       lib.rs        #[pymodule]: every registered ModuleFns sorted by name,
                            then add_class PyGraph — never edited for a new API
              graph.rs      #[pyclass] PyGraph (fields pub(crate)) + its core methods
              registry.rs   the ModuleFns inventory type + registry / version functions
              convert.rs    escape_json + to_py: LD.2's return convention (an answer is a
                            native dict / list; only a `*_json` method returns JSON text)
              build.rs layout.rs      #[pyfunction]s (generate*, load_from_gmap, is_stale ...)
              text.rs records.rs traversal.rs blast.rs trace.rs find.rs docs.rs
              arch.rs contracts.rs    one `#[pymethods] impl PyGraph` block each
  private slots: pages LA.6e   implementors LD.7c   serves LD.8b   delta LE.1c
              diff_impact LE.2   tests_for LE.3b   effects LE.4d   why LE.5   cycles LE.6b
              patterns LE.7b   check LE.8   spec_status LE.9b   cells LF.1b   gaps LF.2c
              snapshots LF.5d, LF.6d   merge LC.10c   feature_flows LG.3c
  0.5.1 private slots (C0.4): pack CC.4c   review CC.6b   flags CC.7c
              contract_breaks CC.8b (+CC.8c)   hotspots CC.10b   cochange CC.11c
              communities CD.1e   splits CD.2d   hubs CD.4c   duplicate_flows CD.4f
              timeline CD.5d   overlay_loop CK.1 (C0.9; DB.9b's rules in the try dict)
  0.5.2 private slots (D0.7): query DB.6 (PyGraph.query / query_gql)
              identity DD.15 (PyGraph.identity_classes)
              watch DC.9 (the WatchSession class: apply / poll / snapshot / persist,
              the GIL released while the engine works)
              graph.rs: a PyGraph drop calls memory::release_freed_memory (DE.10)

cli/src/      main.rs       Cli (global options), enum Cmd, main() dispatch
              common.rs     shared helpers (generate_for, print_json, node lookup,
                            ImpactDirection)
              hooks.rs      install-hooks + HooksCmd (LG.2's hidden `hook`)
              surface.rs    #[cfg(test)] CLI surface test (LG.6a)
              cmd/mod.rs    pub(crate) mod lines — never edited again
              cmd/<command>.rs   one per pre-0.5.0 command: clap Args + run(a) -> i32
              cmd/<area>/mod.rs  flattened Subcommand enum per area of leap commands:
                  query   pages LA.6e, find LD.3b, flows LD.4b, implementors LD.7c,
                          serves LD.8b, why LE.5; 0.5.1: pack CC.4c, hotspots CC.10b,
                          communities CD.1e, splits CD.2d, hubs CD.4c,
                          duplicate-flows CD.4f; 0.5.2: query DB.5 (rules_query.rs),
                          identity DD.15
                  change  delta LE.1c, diff-impact LE.2, tests-for LE.3b, patterns LE.7b;
                          0.5.1: review CC.6b, contract-breaks CC.8b (+CC.8c),
                          cochange CC.11c, timeline CD.5d; 0.5.2: watch DC.8
                  rules   effects LE.4d, cycles LE.6b, check LE.8, spec-status LE.9b;
                          0.5.1: flags CC.7c
                  store   inspect LC.4, cell LF.1c; 0.5.1: cache/ CE.2c (+CE.2d, CE.2e)
                  inputs  gaps LF.2c, history LF.5d, tests LF.6d; 0.5.1: overlay CE.3e,
                          scip/ CE.1c
```

- **py:** pyo3 `multiple-pymethods` lets each module carry its own
  `#[pymethods] impl PyGraph`; a module that owns `#[pyfunction]`s ends with a
  `register()` plus `inventory::submit! { ModuleFns { .. } }`. A new API goes in its
  primitive's module. Helpers that tests exercise stay pyo3-free: the test harness never
  initialises Python (py/src/lib.rs "Link note"). `extension-module` is deliberately not a
  Cargo feature of py/ (maturin turns it on for wheels via pyproject.toml); re-adding it
  there stops the unit tests linking.
- **cli:** a new command adds one variant and one match arm to its area's `mod.rs` and
  its own `cli/src/cmd/<area>/<name>.rs` (`Args` + `run`). The 0.5.1 command files
  (C0.5) were declared as doc-only slots (`mod <name>;` plus one `//!` paragraph, no
  items); each owner added `Args`, `run`, its variant `<Name>(<module>::Args)` (the doc
  comment is clap's about) and its arm, and regenerated `cli/surface/<cmd>.txt`. All 14
  are filled, and so are 0.5.2's three (D0.8: identity, query, watch).
- **Snapshot ownership:** the packet that claims `py/src/<m>.rs` owns
  `py/api_surface/<m>.txt` and `py/tests/surface/test_<m>.py`; the packet that claims
  `cli/src/cmd/<..>/<c>.rs` owns `cli/surface/<c>.txt` (global options:
  `cli/surface/_global.txt`). The owner regenerates them in the same commit.

## Data Flow

```
source files
   → per-language parser (tree-sitter → ExtractedItems)
   → extractors (cross-cutting: HTTP, gRPC, queues, data_sources, CLI)
   → graph builder (resolves intra-repo references)
   → CODE_PASSES (engine/src/profile.rs): the 13 cross-graph resolvers (HttpStack, gRPC, RPC,
     Queue, GraphQL, WebSocket, EventBus, SharedSchema, CLI, DB, Cron, Config, IaC), then
     Identity (SAME_AS stars, the last Resolve pass), then the 9 post-passes, the evidence fill
     (group fills taken at language build, re-filled only where something changed, DC.5) and
     the determinism sort — `[passes] domain=code resolve=14 post=9 finalize=2`
   → merged graph
   → .gmap layout at <repo>/.glia/graph/ (rkyv + mmap, sharded)
   → [optional] activation (PPR) / projection-text / pyo3 → Python
   → [query time] rules (glia-rules over engine::rules::CodeFacts): `glia query`, check's rule
     invariants and reflexion model; identity collapse (engine::identity) on the answers
```

Graph content does not depend on where a tree is checked out: the doc linker binds an ambiguous
mention to the least (qname, kind, graph) candidate and history binds a file's MODULE by least
qname, never by the RepoId-derived NodeId, so two clones of one commit (another remote or
directory name) build the same qname-level graph (DE.1, `engine/tests/repo_id_invariance.rs`;
`[doclink] tie-break by qname: qualified=<q> bare=<b>`). A node id still hashes the RepoId, and
RepoId-ordered tie-breaks remain in pick_primary, cochange, overlay bind, declared and cells
(the 0.5.3 seed, §D).

**Watch (DC.1-DC.10b).** `engine::watch::WatchSession` keeps one repo's build open: the walk
table with stat stamps (DC.1), one ParseCache with its in-memory build memos (DC.4) and the last
GenerateResult. `apply(paths)` reruns the same DC.2 stage fns a clean build calls, rebuilding only
the language groups whose fingerprint moved (DC.7b: the post-graft parses, repo id, overlay, proto
service set and each builder's hidden inputs) and taking the scoped located delta when it can
(DC.6); every update's graph is a clean build of the tree as the session read it, which
`engine/tests/watch_parity.rs` (scripted, DC.10a) and its seeded random sequences (DC.10b) check
byte for byte. A path is Low (a source file) or High (`.glia/`, a `.gitignore`, a manifest, a
`go.mod`, a tsconfig, include sources, a migration: re-plans the walk), `watch::durability`; an
overlay, identity, Go-module or proto-service change is a `full_rebuild` with its reason. `poll()`
is a stat scan (no notify crate) with a racy-mtime rule (DC.7c). Single repo; persists only when
asked (`persist`, `save_cache`). After each update the replaced graph is dropped and freed heap
returned (`memory::release_freed_memory("watch_update")`, DE.10).

**Parallel build (LG.1a–c).** The walk's reads, the per-file route / parse / extract, the
const-table scan, the RPC needle pass, a multi-repo build's per-repo walks and the per-language
graph builds (every build group; the TS family's one graph is the last pooled item, CA.7)
run on the engine's own rayon pool (`engine/src/parallel.rs`, 16 MiB worker stacks), each
through an order-preserving map folded in input order, so a build is byte-identical at any
pool size (`--test byte_identical`, `--test parallel_build`). `GLIA_THREADS` sets the size:
unset / `0` = every core, clamped 1..=256, `1` = no pool. Parser panics are silenced by a
thread-local flag one process-wide hook reads (`parallel::quiet`), so other threads' panics
still reach the embedding app's hook. fired_on, on stderr:
`[parallel] walk <root>: read <n> files on <t> threads (...)` once per walk,
`[parallel] <repo>: routed <n> files on <t> threads (...)` and
`[parallel] <repo>: const-scan <c> files, rpc-needles <r> files, <g> language graphs on <t> threads`
once per repo. Per-language builder markers interleave at default threads;
`GLIA_THREADS=1` keeps them in sequential order. 0.5.2 adds, once per repo,
`[groups] repo=<label> built=<b> reused=<r> keys=<k1,k2,..>` (the build groups, DC.2; a clean
build prints `reused=0`), `[const-fold] pooled queue_files=<q> event_files=<e> threads=<t>
repo=<label>` (DC.3) and `[walk] reuse root=<r> reused=<n> read=<m> of <t>` (DC.1); for a caller
that holds a cache, `[graft-memo] const hit=<h>/<n> needles hit=<h>/<n> noncode hit=<h>/<n>
repo=<label>` (DC.4); once per build `[evidence-split] graphs=<g> group_filled=<n> refilled=<r>
fallback_graphs=<f> cross=<c>` (DC.5); per scoped delta `[delta] scoped graphs before=<b>
after=<a> ids=<i> keys=<k> moves=<on|off>` (DC.6); the session's `[watch] open` / `update` /
`noop` / `poll` lines (DC.7a, DC.7c); `[memory] malloc_trim site=<site> rss_mb=<before>-><after>`
per trim (DE.10); `[imports] dart plain-import names bound=<b> ambiguous=<a>` (DF.4b); and the
identity pass's `[identity] classes=<c> edges=<e> diverged=<d> <rule>=<n>... offsets=<o>/<t>`
(DD.5) and the answers' `[identity] collapse surface=<s> classes=<c> walk_classes=<w> folded=<f>
rows=<r>` (DD.8).
Per-phase timers (CA.9, `engine/src/build/timing.rs`; wall time on the orchestrating thread,
milliseconds truncated to one decimal, stderr only — never a cache, `.gmap`, manifest, stdout
or answer): `[timing] repo=<label> walk=<ms> parse=<ms> const_scan=<ms> grafts=<ms> language_build=<ms>`
once per built repo, `[timing] build repos=<n> resolve=<ms> post=<ms> finalize=<ms> external_cells=<ms> total=<ms> slowest_pass=<name>:<ms>`
once per build (a layout merge prints no `external_cells=`; the stage and per-pass times are
`PassReport::elapsed` / `pass_elapsed`), and `[timing] persist writer=<w> <ms> dir=<dir>` once
per layout write.

## Parser-vs-Graph Split (locked at v0.4.3b)

Parsers **extract**; graph crate **resolves**. Parsers emit raw `ExtractedItems` with unresolved references (`UnresolvedRef`). The graph builder walks the tree to turn those into concrete edges uniformly across languages.

- `SelfMethod` walks to the enclosing `CLASS` / `STRUCT` / equivalent. In the TS family
  (`build_typescript_family`, which lang_build's TS group calls) an unbound `this.m()` /
  `super.m()` then follows INHERITS_FROM to the nearest ancestor defining m: the TS parser
  emits `CallQualifier::SuperMethod` for `super.m()`, which starts one level up.
  `resolve_inherited_calls` binds `this.m()` (CALLS rule `inherited_method`), `super.m()`
  (rule `super_method`, DF.22c) and a typed-receiver call whose class inherits m (rule
  `inherited_receiver`, DF.22b). `emit_override_implements` (renamed from
  emit_abstract_implements) pairs an override with the member it overrides: abstract
  (`NavFact::AbstractMethod`, method-level IMPLEMENTS rule `abstract_override`, CH.1b) or
  concrete (rule `concrete_override`, DF.22c). Constructors never pair (`[ts-inherit] calls
  bound through superclasses: <n> (self= super= receiver= ambiguous=) abstract implements=<a>
  concrete overrides=<c>`). The TS parser fills `CodeNav::local_types` for parameters and
  locals (annotated, `new T()`, `inject(T)`, `x as T`, `this.<field>` aliases; locals in
  top-level describe / it callbacks share the file's scope; DF.22a, `[ts-locals]`), and a typed
  receiver reads another class's STATE_VAR (rule `receiver_state`, DF.17).
- Go resolves more than calls through its package model (graph/src/build.rs). Struct
  embeds bind as STRUCT INHERITS_FROM edges (`bind_go_embeds`), and `GoPromoted` /
  `GoPackages::method_on` promote embedded methods and fields to calls and to implicit
  IMPLEMENTS (CI.2a, CI.2b). `resolve_imports_go` (graph/src/imports.rs) binds imports of
  the repository-root package (CI.3), and CA.3b's reach rule pairs a `_test.go` side only
  through its own file's direct import (`DirImportGraph::imports_directly`).
  `GoPackages::bind_type_handlers` binds receiver-method route handlers whose type sits in
  another file of the package (CI.4). `build_go` also resolves in-repo
  type aliases (`NavFact::TypeAlias`, `resolve_go_alias_sigs` over `alias_scope`) in method
  signatures before the implicit-IMPLEMENTS compare (CI.6, `[go-alias]`). A call in a
  package-var initialiser is a CALLS of the var's STATE_VAR (CI.1, parser-side). 0.5.2: an
  un-aliased import binds the imported package's `package` clause name (`NavFact::GoPackageName`,
  read by resolve_imports_go / GoPackages / bind_go_embeds: `store` for `internal/v2store`,
  DF.15); a method-value route handler on a typed parameter or local (`h *TokensHandler`,
  `h := NewTokensHandler()`) binds through its type text, rule `local_type_method` (DF.16,
  `[go-routes] receiver-method handlers=<n> local-method=<m>`); a route-group prefix held in a
  constant folds (DF.7).
- Rust: `impl Trait for Type` with an imported or path-qualified trait is IMPLEMENTS, and axum
  `State` / `Extension`, actix-web `web::Data` and Rocket `&State` handler parameters are INJECTS
  to the held type, both bound by Rust path rules through `RustIndex::resolve_type_refs`, never by
  bare name (DG.13a, DG.13b, `[rust-types] implements=<b>/<r> injects=<b>/<r> ..`).
- Ruby `Calc.new.add(..)` binds through the constructed type (CL.5a's rule, extended to Ruby's
  postfix `.new` by DH.13), so Minitest methods get function-level TESTS; `class X < Base` is
  INHERITS_FROM. PHP `extends` / `implements` are heritage edges (DH.14), and a `use` of a
  declared namespace binds that namespace's member (DF.2). Dart resolves own-package
  `package:<pubspec name>/..` URIs (DF.4a) and binds a plain import's public names (DF.4b).
- An `extra_hook` seam on `resolve_calls` lets a language contribute resolution the generic
  walker misses; it is consulted only after every generic lookup failed. Five builders use it:
  `build_rust` hands it the path resolver in `graph/src/rust_paths.rs` (`crate::` / `self::` /
  `super::` / `Self::` / workspace-crate paths), fed the walk's Cargo packages as `RustCrate`s by
  `lang_build::rust_crates` (LA.1a); `build_go` hands it `GoPackages` (a package is its
  directory, LA.13b; typed receivers, CA.2b); `build_c_cpp` hands it `CppCallScope` (out-of-line
  members and directly `#include`d headers, LB.10c), then `CppScope` (`graph/src/cpp_scope.rs`:
  C++ name lookup, CB.25); `build_swift` hands it `SwiftModuleTypes`
  (`graph/src/swift_scope.rs`: implicit self across extensions, cross-file static calls, CB.18;
  0.5.2: parameters and locals type their receivers, DF.3); `build_elixir` hands it
  `ElixirModules` (remote calls by full module name, un-aliased top-level modules,
  `__MODULE__.f(..)`, nested-module automatic aliases and alias paths, DH.17a,
  `[elixir-calls] full-name remote calls bound=<b> ambiguous=<a>`). Every other builder passes
  `|_, _| None`.
- Parsers must not short-circuit this: extract what the AST makes available; don't cap at what old regex heuristics happened to capture.

## Format Spec — `.gmap`

Zero-copy rkyv serialisation with memory-mapped read. Sharded by kind to keep hot paths local. Write-once, rebuild-whole-file — no in-place mutation. Owned vs Archived types are the mental model: loaded views are `Archived<T>`, writes go through `Owned<T>` then serialise.

Lives at `<repo>/.glia/graph/` (manifest + shards + cross_stack + parse cache, and the
`timeline.gmap` sidecar once `glia timeline build` ran), the one layout `glia build`, the
install-hooks hooks and pyo3 all write (LC.9). `FORMAT_VERSION` 3 (0.5.1; format 2 was 0.5.0,
LC.1) and `MANIFEST_VERSION` 2: every file opens with a `GLIAGMAP` preamble whose first 12 bytes
(magic + version) are frozen, so an old, future or foreign file reports OldFormat / FutureFormat
/ Corrupt ("rebuild the graph"), and `load_from_gmap` rebuilds such a layout from its repo root
instead of raising (LC.8). The header names its own node-kind / edge-category / cell-type
registries (LC.4, read by `glia inspect`); the container is a domain-free core plus named domain
sections (`code`: nav, symbols, interface methods; LC.5b, LC.6); edges carry cells (LC.2), every
edge one EVIDENCE cell with its emitter, rule, file and 0-based line (LC.3).

Format 3 (0.5.1) is compression, no new ids:
- **EVIDENCE interning (CD.7b).** Every canonical EVIDENCE edge cell is stored as a compact
  `Bytes` payload (tag `0x01`) whose strings live in the file's `"strings"` section
  (`intern_evidence` / `expand_evidence` in `store/src/code_section.rs`), expanded back to the exact
  JSON on read.
- **CODE cells as spans (CD.7c).** A CODE cell that is a verbatim slice of its POSITION file is
  stored as a `CodeSpan` (`code-domain/src/code_span.rs`: tag `0x02`, file index into the same
  strings table, byte range, xxh64), and re-read from the source under the recorded repo root on
  load. A span whose file moved, changed or is gone reads back as `{"code_span":{file, start,
  end, xxh64}}` JSON and is counted; `persist::load_or_rebuild` treats any as the rebuild reason
  `source changed since the layout was written: <n> code spans unresolved`.
- **Parse cache frame (CD.7a).** `parse_cache.bin` is `GLIAPCZ1` + the build stamp, uncompressed,
  then an `lz4_flex` block of the bincode (`engine/src/cache.rs`), so a stale cache is rejected
  from its first bytes without decompressing; a pre-frame cache is discarded.
- **Timeline sidecar (CD.5b).** `timeline.gmap` is a preamble'd container with one `timeline`
  section (revs, strings, node and edge validity spans; `glia_store::write_timeline`), written
  by `glia timeline build` and read by `timeline history` / `as-of`; `RepoMeta.rev` records the
  commit a layout was built at, read from `.git` with no git process.

0.5.2 changes no on-disk layout: `FORMAT_VERSION` stays 3 and `MANIFEST_VERSION` 2. What a
layout holds moves: SAME_AS (edge 37) is an ordinary cross edge, one star per identity class
(every member -> the class's center, the least (qname, kind, graph index) member), its EVIDENCE
rule the identity rule (`message_fqname`, or `message_fqname/diverged` when that copy's fields
differ from the center's; `code_domain::same_as::evidence_rule`); no SHARES_DEPENDENCY edge and
no message SHARES_SCHEMA pair is written (DD.16); a PROJECT's ORIGIN cell gains `label_source`
(DD.4), a generated file's nodes ORIGIN `generated` (DE.7) and an unpaired third-party WS / gRPC /
GraphQL client ORIGIN `external` (DF.10). Rules are query-time and store nothing (Q8).

Projections on top of the store:
- **Binary** — the `.gmap` itself, consumed by activation and the pyo3 layer
- **Dense text** — sigil-based projection with prefix/default/module dedup and scope collapse
- **JSON** — legacy enriched nodes/edges for compatibility

See `dev-notes/glia-memory/reference_format_spec.md` and `reference_rkyv_design.md`.

## Code-Domain Registries

Locked `u32` IDs for `NodeKind`, `EdgeCategory`, `CellType`. Qualified names use `::` as the separator. Extraction vs. resolution split is enforced across all code parsers so the graph builder sees a uniform shape.

**IDs are allocated in `code-domain/src/lib.rs` and nowhere else.** Each registry has a
`RESERVED ids` block; take the next free value there, in the same commit as the emitter,
and never pick one inline in a parser. An id is **locked once allocated** — it is baked
into every `.gmap` on disk, so reusing or renumbering one silently corrupts old stores.

0.5.2 allocates one id, edge category 37 SAME_AS (D0.1): an identity class's member -> its
center, written by IdentityResolver (DD.5). The next free ids are node 50 / edge 38 / cell 26.
SHARES_DEPENDENCY (31) stays registered with no emitter since DD.16 (an id is never reused), so an
old layout still decodes it.

IDs ref: `dev-notes/glia-memory/reference_kind_category_ids.md` and `reference_code_domain_registries.md`.

## Activation (PPR)

Personalised PageRank with damping = 0.5 (not custom spreading activation). `ActivationConfig` is domain-agnostic: direction, edge weights, and node specificity are all provided by the domain, not hardcoded. Code-graph adaptations: edge weights, direction, node specificity — three dials the domain sets.

The domain sets them in its **profile** (LD.14a/b), and nowhere else. The code domain's
`DomainTables` is `CODE_TABLES` in `code-domain/src/profile.rs`: the entry rule (the
kinds, roles and `main` / `test*` names liveness seeds from), `carry_edges` (what blast
radius, cross-stack trace and liveness follow), `effect_sinks`, the base
`activation_weights` and the named `activation_presets` (`repair` / `review` /
`onboard`). The engine wraps it with the build passes as `CODE_PROFILE`
(`engine/src/profile.rs`). A PPR config is `CODE_TABLES.activation_config(None |
Some(preset))`; `MergedGraph::blast_radius` takes `&DomainTables`. The graph, store and
projection crates read `CODE_TABLES` directly (they cannot depend on the engine); a new
entry kind, carry edge or weight is one table row there, never a list in a consumer.

Identity is a table too (DD.3): `CODE_TABLES.identity_rules` holds the eight ways two nodes are
one thing, one `IdentityRule` per `same_as::RULES` name (the kinds it pairs, the key, the tier and
whether a class walks): `package_name` (FACT, never walks: a registry package would join every
dependant), `manifest_name` (FACT, a dependency and the project its manifest names), `message_fqname`
(FACT), `contract_op` / `contract_route` (DERIVED), `scip_symbol` (FACT, DD.7), `body_hash` /
`path_suffix` (HEURISTIC vendored copies, DD.6: same-as candidates, never collapsed). SAME_AS
weighs 0 in PPR and is in no carry or community list: identity reaches a walk or a ranking only
through the quotient (`algo::classes::Quotient`), never as an edge. Answers collapse FACT /
DERIVED classes to one row carrying `identity {tier, rules, diverged, also_in}` (Q17), and walk
classes (messages, contract ops, SCIP symbols, package <-> project) are one node in blast radius,
trace / flows and tests-for walks and their PPR (DD.10-DD.12).

## Adding a New Language Parser

1. Create `parsers/code/<language>/` with a `Cargo.toml`
2. Use the `tree-sitter-<lang>` grammar; beware grammar quirks (see `dev-notes/glia-memory/reference_treesitter_quirks.md`)
3. Implement `parse()` → `ExtractedItems` with raw nodes + `UnresolvedRef`s
4. Emit qnames with `::` separator; use the locked `NodeKind` IDs from `code-domain`
5. Add the crate to `Cargo.toml` workspace members
6. Add it to `engine/Cargo.toml` and wire it into `detect_language` + `parse_one_with`
   (`engine/src/extract.rs`) — **the engine owns every parser dependency**. `py/` lists
   no parser crates at all, so it only changes if the language needs a new pyo3 function.

If the language needs routes (HTTP, gRPC, queues, etc.), the extractor belongs in `parsers/code/extractors/` as a cross-cutting module, not inside the language parser.

## Adding a New Cross-Graph Resolver

Create **`graph/src/resolvers/<name>.rs`** and implement `CrossGraphResolver` there, then
add exactly two lines to `graph/src/resolvers/mod.rs` — `mod <name>;` and
`pub use <name>::<Name>Resolver;`. Nothing goes in `graph/src/lib.rs`; add the resolver to
its `pub use resolvers::{…}` list only if it must be public. Register it by adding one
`resolver!("<name>", <Name>Resolver)` `Resolve`-stage `PassSpec` to `CODE_PASSES` in
`engine/src/profile.rs` (LD.13); its cross edges are stamped `resolver:<name>`. Any new build
pass (resolver, post-pass, external stage) is a `PassSpec` there with its `Stage`, `after` and
`populates`, never a new call in `engine/src/build/`; the `code_passes_order_is_head_order`
and `populates_is_exact` tests pin the order and the declared cell types.
`resolvers/http.rs` is the canonical example. Shipped: 13 resolvers (HTTP, gRPC, RPC, Queue,
GraphQL, WebSocket, EventBus, SharedSchema, CLI, DB, Cron, Config, IaC) and Identity, the last
Resolve pass (SAME_AS, DD.5); MessageSchema and Package were removed in 0.5.2 (DD.16), their
pairs now identity classes. See `dev-notes/glia-memory/project_040_stack_resolvers_backlog.md`.

## Key Design Decisions

- **Tree-sitter, not regex.** 0.4.x moved to AST extraction; 0.2.0 regex is not a ceiling.
- **Zero-copy store.** rkyv + mmap; writes rebuild the whole file.
- **Domain-agnostic core.** `core` and `activation` know nothing about code. Code lives in `code-domain` and the parsers.
- **Generic graph algorithms live in `activation::algo`, over `GraphSource`, never in engine.** Reachability (`algo::reach`, LD.15a; `bfs_levels`, DA.1), graph delta (`algo::delta`, LE.1a) and Tarjan SCC (`algo::cycles`, LE.6a); 0.5.2 adds identity classes and the quotient (`algo::classes`, DD.1) and push PPR (`algo::ppr`, DE.5); 0.5.1 adds label propagation and seeded Leiden with `modularity` (`algo::community`, CD.1a/b), Stoer-Wagner and the minimum s-t cut (`algo::cut`, CD.2a), neighbourhood pair scoring (`algo::linkpred::score_pairs`, CD.3a), degree tables and HITS (`algo::hubs`, CD.4a), MinHash + LSH (`algo::minhash`, CD.4d) and validity spans over consecutive snapshots (`algo::timeline::TimelineBuilder`, CD.5a). A graph type opts in by implementing `GraphSource`, and a walk runs over a per-query CSR `Adjacency`, never a scan of the edge list per visited node; the engine answers that use them (`communities`, `splits`, `hubs`, `duplicate_flows`, `timeline`, the `suspected_edge` gap) add the code-domain reading on top: labels, services, located rows, tiers.
- **Rules are query-time (0.5.2, Q8).** Datalog runs over the built graph when asked (`glia
  query`, check's rule invariants and reflexion model); it stores no derived edge, and glia-rules
  is not a HASHED_ROOTS entry, so a rules change never invalidates a parse cache. Every row carries
  a tier from the Q5 lattice: the min over derivations of the max over inputs, evaluated exactly
  (each stratum to fixpoint with FACT inputs, then +DERIVED, then +HEURISTIC); a negated row adds
  the coverage caveats of what the negation reads, an aggregate takes its weakest contributing
  row. Rows sort by rendered values (never NodeIds); the crate's one hash map is
  `value::Lookup`, which cannot be iterated, so map order never reaches an output; evaluation is
  single-threaded; `eval::DEFAULT_MAX_ROWS` caps the rows one evaluation derives at 5,000,000
  (`--max-rows` / `max_rows=` override it; past it, a `Budget` error naming the head); and the
  `__a_` / `__m_` prefixes (any `__` name) are reserved for the magic-sets rewrite. The query
  language reads structure only: no prelude predicate names an auth / security property, and
  there is no value-flow relation (SECURITY.md). Reference: `docs/rules.md` (the Datalog dialect, the prelude, the GQL subset).
  Markers: `[rules] eval strata=.. rules=.. rounds=.. ms=..`, `[rules] closure cats=<c>
  mode=<m> bfs=<n> rows=<r>`, `[rules] budget pred=<p> rows=<n> cap=<c>`, `[rules] magic
  pred=<p> adornment=<a> sites=<n>`, `[rules] explain pred=<p> tier=<t> height=<h> ..`,
  `[rules] prelude relations=<n> (..)`.
- **Publish gate.** Only `py/` publishes to PyPI (as `glia-py`, imported as `glia_py`). Everything else is internal workspace.
- **No Python fallback.** After v0.4.10c, Python is a thin pyo3 wrapper; there is no parallel Python implementation to keep in sync.

## Query & Answer Surface (v6 P2/P3, 0.5.0, 0.5.1, 0.5.2)

Answer-shaped primitives live in the **engine** (shared by CLI + pyo3/MCP + future
TUI), not composed by the consumer. Each is one call: complete, ranked, located.
Every record's `line` is 1-based (an editor's line), located through one
`Locator` per answer; POSITION cells store 0-based rows — `Locator::locate` is
the only place that converts (LD.1). A list answer that can come back empty is
`Answer { results, absence }`: the absence is a FACT-tier reason plus the coverage
caveats of the mechanisms it depended on (LD.8a, `engine::absence`). Rows carry an
evidence tier — FACT (read at a site), DERIVED (paired by a resolver or pass) or
HEURISTIC (a name guess, an overlay, git history, a clustering or a model) — never an
unexplained score; where an answer ranks by two signals (hotspots) it shows both ranks,
never a blend.

Identity (0.5.2, DD.8-DD.15, Q17). `find`, `blast-radius`, `trace`, `flows`, `tests-for`,
`serves` and `arch` collapse every FACT / DERIVED identity class to one row, the class's best
member, carrying `identity {tier, rules, diverged, also_in[{repo, qname, file, line}]}`, and walk
the identity quotient (a walk class is one node in the BFS and the PPR). HEURISTIC same-as
candidates (vendored copies) stay separate rows (Q18). `--no-collapse` (pyo3: `collapse=False`, the
last keyword) is the raw pre-identity answer: no quotient walk, no identity tag. Not collapsed:
pack, hubs, hotspots, communities, duplicate-flows, effects, implementors, why and gaps rows.
`[identity] collapse surface=<s> ..` fires on stderr once per collapsing answer.

CLI: every subcommand and flag is in README.md `## CLI`, rendered from `cli/surface/`
(regenerate: `GLIA_UPDATE_SURFACE=1 cargo test --manifest-path cli/Cargo.toml cli_surface`). Most take
`--with <repo>` (repeatable, merge first) and `--json`; `--scope` takes a path or a
`glia projects` label. By `cli/src/cmd/` area:
- pre-0.5.0 (`cmd/<command>.rs`): `analyze`, `arch` (A9.2 `service_map`; a single
  repo keys services by top-level directory; `SHARES_*`, `SAME_AS` and `DOCUMENTS` only with
  `--include-shared`; a SAME_AS class is one link per (service pair, class) at its weakest
  edge and other links count class-mapped endpoint pairs once, `--no-collapse` the raw star,
  DD.14), `projects`, `contracts` (`--fields [--breaking-only]`, LE.10d),
  `impact`, `blast-radius` (many seeds are one walk and one ranking, LD.5; `--no-collapse`,
  DD.10), `trace` (ranked distinct paths, `--to`, `--max-paths`, LD.4a; a class hop shows its
  display member with an `(also in: ..)` cell, `--no-collapse`, DD.11), `resolve`, `coverage` (+ the
  co-change audit, LF.5c), `docs-for`, `docs sync|push` (`sync --source
  confluence|notion|mediawiki|dir`, CE.4b-CE.4f), `merge` (`--gmap`, `--workspace`,
  `--layout`, LC.10c), `build`, `install-hooks` (`--pair`, LG.2).
- query: `pages` (LA.6e), `find` (LD.3b; `--no-collapse`, DD.9), `flows` (LD.4b;
  `--features` / `--out`, LG.3c; `--no-collapse`, which conflicts with `--features` /
  `--out`, DD.11), `implementors` (LD.7c), `serves <repo> <channel> [--mechanism
  auto|http|queue]` (LD.8b; an HTTP channel also lists each ENDPOINT stamped ORIGIN `external`
  for that verb + path, match `external`, `Server.external_hosts`, CJ.3; a server row in a
  class folds into the first with `identity`, a server's handlers of one class keep the first
  and `handler_identity` names each, `--no-collapse`, DD.13), `why <repo> <from> <to>`
  (LE.5; exits 1 when not found). 0.5.2: `query <repo> [<file>] [-e <text>] [--gql]
  [--output <pred>]... [--limit <n>] [--max-rows <n>] [--explain]` (DB.5,
  `cmd/query/rules_query.rs`; Datalog from a file, `-e` or stdin, or with `--gql` a GQL read
  query, over the code graph plus the prelude; one table per output, the program's sinks or each
  `--output`; `--explain` adds each row's located witness; exits 0 with rows, 1 with none (the
  absence), 2 on a usage, parse, analysis, row-budget or build error, diagnostics as
  `<source>:<line>:<col>: error: <message>`; `[query] lang=<l> outputs=<o> rows=<r> ..`), `identity
  <repo> [--tier fact|derived|heuristic] [--rule <rule>] [--diverged-only] [--limit <n>]` (DD.15;
  the identity classes by rule and tier, each member located, diverged copies with their
  variants: FACT / DERIVED classes collapse in answers, HEURISTIC same-as candidates never; exits
  0 with classes, 1 with none, 2 on an error; `[identity] report classes=<c> fact=<f> derived=<d>
  heuristic=<h> diverged=<x>`). 0.5.1:
  `pack <repo> <query> [--budget <tokens>] [--preset repair|review|onboard|centrality]`
  (CC.4c; the context for a query packed to a token budget, each node at the most detail
  the budget buys; stdout is the pack text alone), `hotspots [--level module|symbol|both]`
  (CC.10b; git churn rank beside PageRank centrality rank), `communities [--method
  leiden|lpa]` (CD.1e), `splits [--parts <n> | --from <side> --to <side>] [--quotient
  module|community]` (CD.2d; the least-coupling cut, a suggestion), `hubs [--category
  <NAME>]` (CD.4c; fan-in, fan-out and cross-service lists), `duplicate-flows
  [--threshold <jaccard>]` (CD.4f; exact and near groups of entry flows). `pack`,
  `hotspots`, `hubs` and `splits` exit 1 on an empty answer.
- change: `delta [--base <rev>] [--edges-only] [--category <NAME>]...` (LE.1c),
  `diff-impact` (LE.2; impact rows name their `also_in` repos, DD.15), `tests-for` (LE.3b;
  ranked by failure count, failing trace and co-change, `--limit`, `--no-signals`, CC.9a /
  CC.9b; test rows of one class fold, `--no-collapse`, DD.12), `patterns` (LE.7b; out of
  experimental since CC.12b, the `--experimental` flag removed in 0.5.2, DE.11; `--group-by
  service|package`, CA.5b). 0.5.1: `review [--base <rev>]` (CC.6b; the PR report in one
  call: changed nodes, ranked impact, tests to run, edges by tier, rule violations new and
  resolved, rule and forbid_path violations by subject, DB.3c; markdown for a PR comment, exits
  1 only when the change adds a BLOCKING violation, an advisory one under its own heading),
  `contract-breaks [--base <rev>] [--avro backward|forward|full] [--breaking-only]
  [--with <repo>]` (CC.8b / CC.8c; every contract judged against itself across the
  change, plus the clients left with no provider; exits 1 on either), `cochange
  [<file>...] [--base <rev>]` (CC.11c; what usually changes with the change, a
  directional confidence and whether a static link joins them; exits 1 with no rows),
  `timeline build|history|as-of` (CD.5d; the last `--revs` commits as edge validity
  spans in the `timeline.gmap` sidecar). A git or build error exits 2. 0.5.2: `watch <repo>
  [--poll-ms <n>] [--debounce-ms <n>] [--persist-every <s>]` (DC.8; a foreground loop over
  `WatchSession`, no daemon, socket or pidfile, one repo: a ready line, then one NDJSON
  WatchUpdate per update that changed the graph; stat-polls every `--poll-ms` (500), applies a
  batch once still for `--debounce-ms` (200), persists the layout and parse cache every
  `--persist-every` seconds (60; 0 = exit only) and on exit; stops on SIGINT / SIGTERM / stdin EOF
  with exit 0, refuses `--no-overlay` (2), 2 on a bad repo or a build error, 5 when a persist
  fails; `[watch] listening` / `emitted` / `persisted` / `exit` lines).
- rules: `effects` (LE.4d; an external sink names its host, downstream `external: <host>`,
  `EffectRow.external_hosts`: an ENDPOINT (CJ.3, `[effects-external]`) and, since DF.8-DF.10,
  the WS / gRPC / GraphQL clients ORIGIN-stamped `external`, sinks `ws_send` / `rpc_call` /
  `graphql_op`), `cycles` (LE.6b), `check` (LE.8; renders the reflexion model —
  `[[component]]`, `[[layer]]`, `kind = "allow"` — as a `## reflexion model` section, CC.5b /
  CC.5c, evaluated through `engine/src/rules/reflexion.dl` since DB.8b; 0.5.2 evaluates an `invariant` with
  a Datalog `rule` (DB.3a) and a `transitive` forbid_edge as `forbid_path` violations, `via`
  facades never walked through (DB.3b, Q3), and gates on `min_tier = fact|derived` (default
  derived; a heuristic violation is advisory under any setting and never exits 1, Q2): exits 0
  when nothing blocks, 1 on a blocking violation, 2 on a build error or a rule it cannot
  evaluate; `[check] gate blocking=<b> advisory=<a>`, DB.3d), `spec-status` (LE.9b; a row's
  `pairing` is `exact`, `server_base` (another server base the OpenAPI document lists),
  `annotated_base` (the project's swaggo `@BasePath` for a base-less document, at most medium)
  or `prefix`, DF.19). 0.5.1: `flags [--status dead|undefined|single_site|quiet]` (CC.7c; the
  stale feature-flag report, exits 0).
- store: `inspect <path>` (LC.4), `cell set|rm|ls` (LF.1c). 0.5.1: `cache push|pull|gc`
  (CE.2c; the shared parse cache over a directory or HTTPS object store, objects signed
  with a blake3 keyed MAC; `--layout` moves a whole `.glia/graph/` layout, CE.2d; HTTPS
  through `ureq`, CE.2e). The transport lives in the glia binary only: neither
  glia-engine nor glia-py has `ureq` in its normal dependency tree.
- inputs: `gaps [--overlay-delta]` (LF.2c; `cochange_no_edge` rows carry no repo, so a
  merge with one relative path in two repos gives two alike rows; `suspected_edge` rows
  carry a paste-ready `[[edge]]` draft, CD.3b; `dead_symbol` withholds a METHOD that
  IMPLEMENTS a METHOD with an incoming CALLS / USES from a third node, CH.1c,
  `[dead-dispatch]`; a `super.m()` call (rule super_method) vouches for nothing there, and TS
  concrete overrides join the dispatch rule through rule concrete_override, DF.22c;
  `rotted_rule`, a FACT row per `[[constraint]]` whose `rule` this glia can no longer read,
  suggest `remove`, DB.4), `history sync` (LF.5d), `tests ingest`
  (LF.6d; a rolling window of runs, `--window`, `--reset`, CC.9b). 0.5.1: `overlay
  propose|try|accept` (CE.3e; the overlay loop, `skills/glia-overlay/SKILL.md`; 0.5.2,
  DB.9a-c, Q9: a candidate may also hold `[[constraint]]` rules, handles `constraint#<n>`;
  try judges a rule by the check delta on the build with every stanza and demotes a graph
  stanza that adds or hides a declared rule's violation to review; accept refuses a rule check
  cannot evaluate and moves the file to `version = 2` in place only when a written rule needs
  it; `[overlay] try-rules ..`; `--remove` takes orphaned_rule / redundant_rule / rotted_rule
  ids), `scip import <repo> <index.scip>` (CE.1c; writes `.glia/scip-snapshot/`, which the
  CE.1d build stage reads as FACT-tier CALLS / USES / IMPLEMENTS / INHERITS_FROM, and the
  identity pass, through `CodeBuildCtx.scip_roots`, as `scip_symbol` classes, DD.7). A snapshot
  step never runs inside a build. `.glia/overlay.toml` `version = 2` (DB.1, Q7) adds the
  `[[constraint]]` fields `rule`, `transitive`, `via` and `min_tier`; a version-1 file keeps
  loading, and rule text is declared only there: the cell API refuses a CONSTRAINT entry with a
  `rule` (Q9). `[walk] tests = [..]` (CJ.4, `WalkConfig::tests_matcher`: gitignore syntax,
  repo-root anchored, additive) gives matching nodes ORIGIN `test_fixture` (`[provenance]
  declared tests`), and the TESTS pass reads it (DF.11): a declared test module pairs the app
  module of its stem (affix stripped when present), rule `declared_name`, and CL.5b derives its
  function-level TESTS; a layout merge still pairs declared tests by name only. It is user
  config like `[walk] skip`: applied under `--no-overlay` and never a candidate section, so
  neither `overlay try` nor `--overlay-delta` measures it.
- hidden: `hook pre-commit|commit-msg`, the runners `install-hooks --pair` writes.

pyo3 (`PyGraph`): `blast_radius`, `cross_stack_trace`, `entry_flows`,
`feature_flows` / `write_feature_flows`, `resolve`, `find`, `coverage`,
`governing_docs`, `page_flow`, `service_map`, `contracts`, `contract_fields`,
`implementors`, `serves`, `why`, `diff_impact`, `tests_for` / `tests_for_diff`
(`limit=`, `signals=`), `effects` (and `serves`: each row carries `external_hosts`, CJ.3),
`cycles`, `check`, `spec_status`, `gaps`, `patterns`
(CC.12b; its 0.5.0 `_experimental` alias was removed in 0.5.2, DE.11), 0.5.1's `pack` /
`pack_ids`, `flags`, `hotspots`, `cochange`, `communities`, `splits`, `hubs`,
`duplicate_flows`, and 0.5.2's `query(program, outputs=None, limit=None, explain=False,
max_rows=None)` / `query_gql(query, limit=None, explain=False, max_rows=None)` (DB.6; the
QueryAnswer as a dict, a parse / analysis / budget error a `ValueError` with the rendered
diagnostics) and `identity_classes(tier=None, rule=None, diverged_only=False, limit=None)`
(DD.15); `find`, `blast_radius`, `cross_stack_trace`, `entry_flows`, `tests_for`,
`tests_for_diff`, `serves` and `service_map` take a trailing `collapse=True` (and the module
function `tests_for_rev`); traversal —
`neighbours(node_id, direction="out", categories=None)` -> `(id, category,
"out"|"in")`, `bfs`, `predecessors`, `reachable_by`, `shortest_path` (categories=None is
every category, DEFINES included; LD.3c) — `set_cell` / `remove_cell` (+ `activate`,
`node_cells`, `dense_text*`, `nodes_json` / `edges_json`, `save_to*`). Module functions:
`generate` / `generate_many`, `load_from_gmap` (rebuilds a stale or old layout, LC.8),
`is_stale`, `merge_gmaps`, `graph_delta`, `diff_impact_vs_rev`, `tests_for_rev`,
`patterns_vs_rev` (its deprecated `_experimental` alias removed in 0.5.2, DE.11),
`overlay_delta`, `history_sync`, `tests_ingest`, `write_cell` / `remove_cell`,
`kind_names` / `category_names` / `cell_type_names` / `entry_kinds`, and 0.5.1's
`review_vs_rev` (`format="dict"|"markdown"`), `contract_breaks_vs_rev` (`with_repos=`),
`cochange_vs_rev`, `timeline_build` / `timeline_history` / `timeline_as_of`, and
`overlay_propose` / `overlay_try` / `overlay_accept` (CK.1, `py/src/overlay_loop.rs`:
transport over `overlay_loop::{propose, try_candidate, accept}`; `candidate` is TOML
text, a refusal is a `ValueError`, markers end `surface=py`; since DB.9b the try dict gains
`rules` when a build checked a rule or the candidate holds a `[[constraint]]`, and stanza
dicts `rule` / `rules`; `overlay_accept` raises `ValueError` on a rule check cannot evaluate).
Classes: `PyGraph`, and 0.5.2's `WatchSession(repo, overlay=True)` (DC.9: `apply(paths)` /
`poll()` return a WatchUpdate dict (`poll` None when nothing changed), `snapshot()` a PyGraph of
the current build, `persist()` writes the layout and cache; the GIL is released while the engine
works). A PyGraph drop returns freed heap to the OS (`memory::release_freed_memory
("pygraph_drop")`, glibc `malloc_trim(0)`; `GLIA_MALLOC_TRIM=0` turns it off, DE.10). The cache
transport, `scip import` and `glia watch`'s loop are CLI-only. An answer is a native dict / list;
only `*_json` returns JSON text (LD.2). The committed surface is `py/api_surface/<module>.txt`.

Engine entry points — flat: `blast_radius`, `resolve_signal_located`,
`coverage_report`, `governing_docs`, `entrypoint_reachable`, `locate_node`,
`service_map` / `service_map_with`; by module path: `trace::{cross_stack_trace,
entry_flows}`, `find::find_nodes`, `pages::page_flow`, `implementors::implementors`,
`serves::serves`, `why::why_edge`, `delta::graph_delta_vs_rev`,
`diff_impact::{diff_impact_vs_rev, diff_impact_from_diff, diff_impact_from_delta}`,
`tests_for::{tests_for, tests_for_diff, tests_for_rev, tests_for_delta}`,
`effects::effects`, `cycles::cycles`, `check::check`, `spec_status::spec_status`,
`patterns::{pattern_conformance, pattern_conformance_delta}`,
`gaps::{gaps_report, overlay_delta}`, `contract_fields::contract_fields`,
`feature_flows::{feature_flows, write_feature_flows}`,
`merge::{merge_layouts, read_workspace}`, `persist::{load_layout, load_or_rebuild}`.
0.5.1, by module path: `pack::{pack, pack_ids}`, `review::{review_vs_rev, review,
render_markdown}`, `flags::flags`, `contract_breaks::{contract_breaks,
contract_breaks_vs_rev, contract_breaks_vs_rev_with, AVRO_MODES}` (the delta's content
cells are CODE plus SCHEMA_FIELDS), `hotspots::hotspots`, `cochange::{cochange,
cochange_multi, cochange_vs_rev}`, `communities::communities`, `splits::splits`,
`hubs::hubs`, `duplicate_flows::duplicate_flows`, `timeline::{build_timeline,
load_timeline, edge_history, as_of}`, `shared_cache::{file_key, export_entries,
cache_rows, wanted, import_entries, layout_key, export_layout, install_layout}` (the
engine half of the shared cache: keys, export, verified import; no transport) and
`overlay_loop::{propose, try_candidate, accept, parse_candidate, merge, without}` (0.5.2
adds the rule trial types `RuleHit`, `RulesDelta`, `RuleTrial`, `RulesReport`, DB.9b).
0.5.2, by module path: `rules::{query, query_gql, validate, evaluate_rule, CodeFacts,
CODE_ATTRIBUTES, PRELUDE, prelude_predicates}` (QueryOptions / QueryAnswer; DB.2a-c, DB.7c),
`watch::{WatchSession, durability}` (DC.7a-c), `identity::{identity_classes, IdentityView,
Collapse}` (DD.8, DD.15), `memory::release_freed_memory` (DE.10), `serves::serves_with` and
`arch::service_map_collapsed` (the collapse-aware forms, DD.13 / DD.14), and on the graph facade
`glia_graph::{blast_radius_over, cross_links_with}` (DD.10, DD.14).
A `*_with_live` variant takes a precomputed liveness set, so a caller answering several
questions over one graph computes `entrypoint_reachable` once.

## Roadmap

- **v6 (done, glia-side):** P1 substrate completeness (blind 48→0, eval-gated by
  `bench/substrate-gap`), P2 coverage signaling, P3 answer-shaped primitives
  (above). P4 (collapse ~13 MCP tools → ~4) is repo-graph's job; these primitives
  are its enabler.
- **0.5.0 — released 2026-09-20** (tag `v0.5.0` at `2170ff8`, `glia-py` 0.5.0 on PyPI).
  The 2026-09 programme (unreleased since v0.4.18; there is no 0.4.19) plus the leap, waves
  A–G of `dev-notes/next-leap-0.5.0.md`, packets in `dev-notes/leap-packets.json` (251
  packets, 41 waves): every id / qname / format / API break at once. It is cross-domain
  *prep* — header registries, domain container sections, the domain profile, pass
  composition, `activation::algo`, the test-only `toy-domain/` — and ships no second
  domain. The rename, table-driven by `dev-notes/rename-0.5.0.py` (`--check` lists
  anything left on the old names): the repo, the `glia` binary (`cli/Cargo.toml`), every
  library crate — packages `glia-*`, Rust paths `glia_*` (LD.11a) — and the Python
  package: PyPI dist `glia-py`, module `glia_py`, wheel
  `glia_py-<ver>-cp311-abi3-*.whl` (LD.11b, `--python`).
- **0.5.1 — the catch-up leap: released 2026-10-02 (tag `v0.5.1`, `glia-py` 0.5.1 on PyPI).** Scope in
  `dev-notes/next-leap-0.5.1.md` (§7.2 is James's rulings), packets in
  `dev-notes/leap-051-packets.json`, overrides in `dev-notes/leap-051-corrections.json`,
  schedule and LANDED waves in `dev-notes/wave-runner/schedule_051.py`: 198 packets, 155 in
  waves W0–W11 and the 43-packet finishing batch (leap doc §7.4) in W12–W19. The name
  stays 0.5.1 and consumers pin it exactly, so it may break
  contracts, each break declared by its packet. Groups: C0 wave-0 slots and the one
  dependency commit; CA dogfood fixes (Go closure calls, typed receivers, signature-checked
  implicit IMPLEMENTS, patterns on real repos, Kotlin receivers, the pooled TS build,
  `[timing]` phase timers); CB the language / HTTP / RPC / event backlog (Swift and C++
  call scope, Go route mounts, C/C++ include roots, client host narrowing); CC the new
  answers (`pack`, `review`, `flags`, `contract-breaks`, `hotspots`, `cochange`, the
  reflexion model in `check`, tests-for ranking, `patterns` out of experimental); CD
  graph algorithms (`communities`, `splits`, `hubs`, `duplicate-flows`, the
  `suspected_edge` gap, `timeline`) and `FORMAT_VERSION` 3; CE external inputs and caches
  (SCIP import, the shared parse cache, the overlay loop, Notion / MediaWiki / wiki-dir
  doc sources); CF the coverage-matrix probes (a Kotlin row, 28 not-applicable cells);
  CG the Engram session's findings (TS field methods, test-fixture provenance,
  `--exclude-path`, full doc CODE text, external endpoints); CZ the release docs. The
  finishing batch: C0.9 the pyo3 `overlay_loop` slot; CH TypeScript / Angular / Dart
  (abstract classes, inherited `this` / `super` calls and abstract-override IMPLEMENTS,
  dead_symbol dispatch, call-initialised STATE_VARs, URL-builder endpoints, the non-HTTP
  receiver gate, method-by-value USES, the `/api` prefix fold for TS builders and Dio
  bases); CI Go (package-var initialiser calls, embedded-struct promotion for calls and
  IMPLEMENTS, root-package imports, split-file handlers, foreign and parameter-rooted
  mounts, type aliases); CJ the `.rs` / `.py` literal guard, project-root and SDD doc
  ingestion, external sinks in `effects` / `serves`, `[walk] tests`; CK the pyo3 overlay
  loop and the engram-export route pairing and build sidecar; CL the matrix slice (queue
  row semantics, Go / JVM / .NET brokers, JobRunr / Hangfire / asynq, constructed
  receivers, fn-level TESTS, GraphQL clients and HotChocolate, .NET env and appsettings,
  Go EventBus, NestJS cron, JVM process launches); CZ.3 these docs and the handoffs. No
  registry id is allocated (edge 37 stayed open until 0.5.2's SAME_AS took it); the
  WASM reader was dropped (CD.6b / CD.6c). It is
  released: `main` and the `v0.5.1` tag pushed, `glia-py` 0.5.1 published by `wheels-py.yml`. The bump: the Cargo workspace and
  `py/pyproject.toml` read 0.5.1, every glia crate takes the workspace version
  (`version.workspace = true`; leap doc §7.4) and the excluded engram-export is 0.5.1, so
  every crate can be pinned `=0.5.1`. The handoff docs
  (`dev-notes/{repo-graph,neuropil,engram}-handoff-0.5.1.md`, each with the exact pin and
  every declared break) cover W0–W19.
- **0.5.2 — the four bets: landed (W0–W14), release pending James.** Scope in
  `dev-notes/next-leap-0.5.2.md` (§0.1 the binding decisions Q1–Q25, §4.2 the defaults the
  packets took), research `dev-notes/research-0.5.2/`, packets in
  `dev-notes/leap-052-packets.json` (153 packets, the gap run's DB.9a-c and DF.22a-c among
  them), overrides in `dev-notes/leap-052-corrections.json`, schedule and LANDED waves in
  `dev-notes/wave-runner/schedule_052.py` (15 waves, runner `--release 052`). The name stays
  0.5.2 and consumers pin it exactly, so it may break contracts, each break declared by its
  packet; the handoffs are `dev-notes/{repo-graph,neuropil,engram}-handoff-0.5.2.md` (DZ.2;
  repo-graph's is held to be sent with 0.5.3's as one combined handover, James 2026-10-16).
  The version bump is the release step after the docs, never inside a packet. Groups: D0 the
  wave-0 slots, SAME_AS = edge 37 (the release's only id) and the one dependency commit (the
  glia-rules crate, no external crate); DA the glia-rules Datalog crate; DB rules in the engine
  (`glia query` + GQL, check's rule invariants / transitive forbid / min_tier, review's
  blocking gate, overlay version 2, `rotted_rule`, the prelude, the reflexion model as rules,
  rules through the overlay loop); DC `glia watch` (walk reuse, stage fns, build memos, the
  evidence-fill split, the scoped delta, WatchSession with group early cutoff and the stat
  poll, the CLI loop, pyo3, the parity gates); DD cross-repo identity (classes and the
  quotient, identity rules, IdentityResolver with SAME_AS stars for packages, manifests,
  messages, contract ops, SCIP symbols and vendored copies, collapse in seven answers, `glia
  identity`, the Q16 replacement of SHARES_DEPENDENCY and the message SHARES_SCHEMA); DE
  Engram anchors and engine hygiene (RepoId-independent content, engram-export determinism and
  re-key pairing, push PPR side file, generated-code provenance, python requirements roots,
  `malloc_trim` after a PyGraph drop, the 0.5.0 pattern aliases removed); DF the 0.5.1
  deferrals (PHP imports, CB.26's residuals, external WS / gRPC / GraphQL clients, `[walk]
  tests` feeding TESTS, Python signals, the Pub/Sub Terraform binding, the Go package clause,
  OpenAPI bases, TS typed receivers / inherited calls / concrete overrides); DG / DH the rest
  of the coverage matrix (Rust and Scala, then Ruby, PHP, Dart, Elixir, Swift, Clojure, C);
  DZ these docs and the handoffs. The matrix stands at 345 full, 1 partial (dart/tests), 0
  none, 136 unknown, 28 n/a. No FORMAT_VERSION change.
- **After 0.5.2** (the leap doc's §1 train; seed `dev-notes/next-leap-0.5.3.md`): **0.5.3**
  scale — Stack Graphs-style incremental resolution with watch's A2 body-only fast path,
  build-target graphs (the likely first real non-code domain), multi-repo `glia watch --with`,
  org-internal package routing on SAME_AS; plus James's 2026-10-16 additions there (local
  embeddings and `glia embed`, a clarification queue, one tier vocabulary, a watch / rules
  memory budget, concept search over flows, the self-graph hygiene cluster) and the 0.5.2
  waves' leftovers (§D, §E). **0.5.4** proof — precision rate on real repos, agent
  before/after, big-repo per-phase timings, calibrated confidence, `decide/` and `glia judge`
  on our own model. Still later from the 0.5.0 list: dominators (once middleware is extracted
  and the security gate is ruled on), RuntimeZone, LSP, per-graph-area rebuilds. The §6
  "Gated" items stay out (`SECURITY.md`).

## Memory

Relevant architecture/spec memories from the repo-graph project memory system are copied under `dev-notes/glia-memory/`. They seeded this repo's Claude memory directory when the split landed. **Files under `dev-notes/glia-memory/` are point-in-time snapshots and are not updated in place** — several still cite the pre-split `rust/` layout and pre-wave-0 `lib.rs` line numbers. Read them as history; this file is the current architecture.
