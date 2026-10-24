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
                    Andersen-Chung-Lang, over an Adjacency, DE.5; kept in 0.5.3 for
                    rank_eval's PushPpr), and 0.5.3's `algo::fuse` (EB.2: minmax, blend,
                    mean, lex_order, order, lx: rank_eval's min-max / blends / LX fusion,
                    ties by candidate index; `[algo] fuse ..` under GLIA_ALGO_DEBUG=1);
                    `ppr_index` (0.5.3, EA.1: `PprIndex`, the CSR incidences, out-weights,
                    dangling list and PPR dials built once and run per seed set, `vector` /
                    `vector_weighted`; `ActivationPlan::run_indexed` / `rank_indexed` and
                    `activate_indexed` take it, a config mismatch falls back to the
                    unindexed answer); `plan` (one
                    ActivationPlan over the RankingSignal / FilterPredicate / SynthHook
                    hooks) and `profile` (DomainTables / DomainProfile; 0.5.2 adds
                    IdentityRule / IdentityTier and `DomainTables.identity_rules`, DD.3;
                    the close-out's `NamedEntry.exts` / `accepts_ext`, a named entry row
                    limited to some file extensions, and `EntryRule::is_entry_at`, which
                    reads a node's extension lazily, DT.5l)
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
                client needles (a vendored stub mints no GRPC_CLIENT, DE.8);
                evm/ (0.5.2 tweaks, DT.10c-DT.10k): EVM contract clients, one module
                per reader over a shared code view (scan.rs) and ABI reader (abi.rs:
                human-readable, JSON, TS object-literal, parseAbi): ts.rs (ethers,
                hardhat, TypeChain, artifact imports, viem / wagmi, web3.js) and go.rs
                (go-ethereum: JSON ABI consts, abi.JSON / NewBoundContract bindings,
                abigen metadata, events, same-file method-name wrappers); each mints
                RPC_CALL `evm_call:<scope>.<fn>` / EVENT_HANDLER `evm_event:<scope>.<E>`
                with an evm ENDPOINT_HIT (`[evm-client] lang=ts|go ..`,
                `[evm-wrappers]`); angular_template (DT.5a-DT.5c): the Angular
                template / host-metadata scanner and binder (`[angular-template]`)
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
                    0.5.3 removes the DE.5 PPR side file and its flag (EG.4: exit 8 is
                    retired, a stale side file from a 0.5.2 run is deleted, the
                    glia-activation dependency is gone); every run writes
                    `<out>.hints.jsonl` (DT.12a), each fact's LB.6 `Identity::hint` v1 form (or
                    null), bound to the gmap by `gmap_digest`, the key `glia cell set --hint`
                    takes (DT.12b; `[engram-export] hints side: ..`, exit 9 when unwritten)
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
2. **Every new 0.5.0 / 0.5.1 / 0.5.2 / 0.5.3 primitive is a public module slot**, reached by
   module path — `glia_engine::delta::graph_delta_vs_rev`, `::persist::load_layout`,
   `::profile::CODE_PROFILE`, `::pack::pack`, `::rules::query`, `::watch::WatchSession`,
   `::identity::identity_classes`, `glia_graph::roles::roles_in` — never flattened into
   the root. The slot's owner fills its file; no later packet edits a facade (in 0.5.0
   only LD.11a, the crate rename, touched `engine/src/lib.rs` after wave 0; in 0.5.1 only
   the wave-0 C0.* packets touched the engine, graph and py facades; in 0.5.2 only the
   wave-0 D0.* packets touched a facade, the workspace Cargo.toml or Cargo.lock, until the
   tweaks: DT.1a and DT.10a declared code-domain's `wire` / `evm` slots in its lib.rs, which
   also holds `NavFact` (DT.1a, DT.1q, DT.10g appended variants), and DT.1g, DT.5f and DT.10b
   added their resolvers' `mod` lines; no tweak touched Cargo.toml or Cargo.lock; in 0.5.3
   only the wave-0 E0.* packets touched a facade (E0.1 `algo::fuse`, E0.2 the engine's
   `rank` / `intent` / `calibrate`) beside EA.1's `ppr_index` lines in activation's lib.rs, and
   no packet touched Cargo.toml or the workspace Cargo.lock).
3. **A spec step "FACADE: `pub use x::{..}`" or "`mod x;`" is already done** — skip it
   and call the item by its module path from `cli` / `py`.
4. Two globbed modules exporting one name raise `ambiguous_glob_reexports`: rename,
   never `#[allow]`. A slot still doc-only at release is dead weight: the release docs
   packet (LG.4b in 0.5.0, CZ.2 in 0.5.1, DZ.1 in 0.5.2, EZ.1 in 0.5.3) fails on any slot file whose
   only lines are `//!` (`grep -v '^//!' <file> | grep -c .` is 0), and a dropped owner's
   slot is deleted with its `mod` line. At 0.5.2 every tracked `.rs` file has code, the
   34 D0 slot files included (none was dropped; D0.8's `cmd/query/query.rs` became
   `rules_query.rs` in DB.5). At 0.5.3 too: the 8 E0 slot files with a body
   (`activation/src/algo/fuse.rs`, `engine/src/{rank,calibrate}/mod.rs`,
   `engine/src/intent/{text,ids,defs,paths,idr}.rs`) and every other tracked `.rs` (EZ.1's scan).

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
                                           `[const-fold] pooled`); the 0.5.2 tweaks add
                                           apply_ws_client_bases (a WS client's config base
                                           folded through the const table, DT.3b, `[ws-base]`)
                                           and the two child-module grafts below
                            angular_templates.rs  graft_template_urls (DT.5b): a component's
                                           `templateUrl` .html (the walk admits an .html beside a
                                           same-stem .ts) bound to its CLASS's members and receivers
                                           (`[angular-template] external ..`)
                            evm_abi.rs     join_evm_abis (DT.10g): an exported TS ABI const imported
                                           by relative path (NavFact::EvmAbi), or a Go string const
                                           of another file of the package (NavFact::GoStringConst),
                                           fills its clients' evm cell (`[evm-abi-join] ..`)
                            rpc_needles.rs RpcContext, apply_rpc_needles, graft_rpc_markers
                            lang_build.rs  plan_groups / build_groups / fold_groups — the
                                           per-language build groups (DC.2; build_groups takes a
                                           reuse slot a watch fills with cached builds) + markers;
                                           GroupCtx holds every repo-level builder input, and
                                           watch.rs hidden_fp destructures it exhaustively, so a
                                           new input fails to compile until it joins a row (DC.7b);
                                           rust_crates (the Cargo packages build_rust resolves
                                           against), DartPackages (pubspec names, DF.4a); the
                                           dart arm calls glia_graph::build_dart (DT.5h)
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
                            changed (`[evidence-split] graphs=..`); test_route_ids (DT.6a), the
                            one test-only-ROUTE predicate the `http` pass, the provenance stamp
                            and (DT.6b) gaps / suspected read
              profile.rs    CODE_PASSES — every build pass in run order (17 Resolve passes: 15
                            cross-graph resolvers, then models and identity; 9 post-passes,
                            evidence fill + determinism sort); run_code_passes_with(merged,
                            &CodeBuildCtx) is the whole build tail (run_code_passes is gone, DD.7);
                            CodeBuildCtx::for_merge passes a layout merge's recorded SCIP roots, so
                            merge == joint build
              coverage.rs   coverage_report; the caveat rows every absence reads. The reflexion
                            golden engine/tests/data/reflexion/allow_absence.json embeds the
                            universal rows (RPC_CALLS' EVM reading and the `*` EVENT_FLOWS row
                            since DT.10h; QUEUE_FLOWS / WS_CONNECTS / GRAPHQL_CALLS, DT.11z), so
                            editing one regenerates it: `GLIA_UPDATE_GOLDEN=1 cargo test -p
                            glia-engine --lib rules::reflexion_parity`, declared as a cli_output break
              answers.rs    the P3 primitives
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
              contract_breaks CC.8a (+CC.8c; 0.5.2 tweaks: contract_breaks/models.rs, the
                        inferred-model judge model_breaks, DT.1o, wired into contract-breaks and
                        review with a min-tier gate, DT.1p)
              hotspots CC.10a   cochange CC.11a (+CC.11b)
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
  0.5.3 public slots (E0.2), owner (+ extenders); all query-time, read by the answers:
              rank/    EA.2 (+EA.3, EA.4) — directory module: ranking.rs (RankBase Ppr /
                       PprWide / Bfs = rank_eval's P1 / PT* / B0; Ranking { base,
                       cochange_first }, default H10 = co-change then P1, head() = P1, the 0.5.2
                       order; the harness names P1, PT*, B0, H10, H10[PT*], H10[B0];
                       WIDE_DAMPING 0.85, kept out of CODE_TABLES; STRUCTURE_CANDIDATES),
                       cache.rs (RankCache: one holder's repo roots / labels, at most 8
                       PprIndexes by SourceKey Merged | Quotient + PprIndex::matches, oldest
                       evicted, the carry Adjacency, the FileIndex, the CochangeIndex and the
                       calibration record, all reset by guard() on a (nodes, edges) change),
                       files.rs (FileIndex: rank_eval's index_graph candidate rule, located
                       MODULE files, no ORIGIN generated, NOT_CODE_EXTS out, sorted by (repo
                       label, path): the ONE file space co-change, text, IDR and the replay
                       index through), cochange.rs (CochangeIndex from each root's
                       .glia/history-snapshot/ through HistoryFile.p with NO rename fold, a
                       commit training iff it maps 1..=30 files; c0), order.rs (rank_files /
                       rank_files_with_c0 / base_keys -> FileRanking, rank_eval's keys and
                       `better` order verbatim; order_rows, the row permutation)
              intent/  EB.3 (+EB.4, EB.5a-c, EB.6) — text.rs (TextIndex, BM25 over the history
                       snapshot's messages, rank_eval's T1s: k1 1.2, b 0.75, digit tokens kept,
                       Candidates over the FileIndex, exclude_range for a PR's own commits),
                       ids.rs (IdVocab from `.glia/overlay.toml [ids]`, DEFAULT_PATTERNS
                       `{U}-{N}` / `T{N3}`; the shape grammar is code-domain
                       glia_config::IdPattern), defs.rs (read_defs at a Tree::Work | Rev: list
                       items, table rows, headings of the admitted docs, `[ids] markdown`
                       globs, `[[ids.tracker]]` JSON, Tier-4 sections; Namespaces, item 9),
                       paths.rs (PathResolver, rank_eval's KPaths; rename_pairs re-exported
                       from external::history), idr.rs (route: typed id routing, NAMED 1.0
                       pending / 0.5 done, CITED 0.5 per comment, the conventional-commit scope
                       route, recent scopes; IdrSide::quiet for the replay)
              calibrate/ EC.1 (+EC.2-EC.6, EC.10-EC.14) — record.rs (the
                       .glia/calibration-snapshot/ record: CALIBRATION_VERSION 1, the writing
                       BUILD_STAMP strictly, RANKER_REVISION 1, candidate-set digests; select /
                       explicit -> Selection), replay.rs (the leak-free replay: commits oldest
                       first, train 1..=30 files, test the newest 300 of 2..=30, the k1 / k2 /
                       k3 / half / find queries), candidate.rs, select.rs (run_sets:
                       rank_eval's r3_select online pick; ruling EC-3, a pick below the
                       fallback is refused), bank.rs (the structure bank), sets.rs (SELh0,
                       SELs0, SELt2, SELf2t; GROUPS structure / review / find), fusions.rs
                       (the seeded and find banks over algo::fuse), work.rs (W5 / M),
                       step.rs (calibrate / calibrate_snapshot, CalibrateOptions { params,
                       groups, surface, vocab }, the bootstrap CI)
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
              build/go_wire.rs  (DT.1f) a child of build.rs: types each Go handler wire cell
                            (DT.1e) to its in-repo STRUCT through GoPackages and writes
                            handler -USES-> STRUCT (emitter graph:go_wire)
              build/dart.rs  (DT.5h, DT.5i) a child of build.rs, the passes of the flat
                            `build_dart`: resolve_member_reads (a Dart NavFact::MemberRead
                            bound in the reader's own scope, emitter graph:reads),
                            provider_type / provider_call (Riverpod provider receivers,
                            emitter graph:dart)
              imports/reexports.rs  (DT.1q) a child of imports.rs: TS / JS re-export
                            barrels resolved as ECMA-262 ResolveExport (Barrels::read /
                            materialize), IMPORTS rule `reexport`
              resolvers/    one module per mechanism (http, grpc, rpc, evm, queue, graphql,
                            gqlgen, websocket, eventbus, shared_schema, db, cron, config, iac,
                            cli) + models.rs (the contract-model pass: client_wires, wire_pairs,
                            graphql_pairs, wire_fields and ModelResolver, the http_model /
                            graphql_model SAME_AS stars, DT.1g-DT.1o) + identity.rs
                            (IdentityResolver, the SAME_AS stars, D0.5 slot, DD.5 + DD.6 +
                            DD.7) + mod.rs; message_schema and package
                            are gone (DD.16: their SHARES_SCHEMA message pairs and
                            SHARES_DEPENDENCY clique are SAME_AS stars now); host.rs is the
                            shared host narrowing (service alias index, narrow_by_host)
                            the http / websocket / grpc / graphql / rpc resolvers use (CB.12);
                            external WS / gRPC / GraphQL clients stay out of pairing (DF.9);
                            evm.rs (EvmContractResolver, DT.10b) pairs EVM client calls and
                            subscriptions to the Solidity procedure / event of the contract
                            they identify; gqlgen.rs (GqlgenResolver, DT.5f) pairs SDL
                            GRAPHQL_RESOLVERs with gqlgen's Go resolver methods
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
              counted once, arch's reading, DD.14). The 0.5.2 tweaks add (each struct /
              enum #[non_exhaustive], the three resolvers unit structs): client_wires and
              ClientWire, ClientWires, ClientWireStats,
              ResolvedShape (+ judged_against, DT.1p), ResolvedField, RouteHandler, RouteSide,
              ServerSite, WireDirection (DT.1g); wire_pairs and WirePairs, WirePairStats,
              WirePair, WireChange, ModelPair, ModelJoin (DT.1m); graphql_pairs and
              GraphqlPairs, GraphqlPair, GraphqlChange, GraphqlPairStats (DT.1n);
              wire_fields, graphql_fields, graphql_fields_all, WireField, WireFields (DT.1o);
              the resolvers ModelResolver (DT.1h), EvmContractResolver (DT.10b) and
              GqlgenResolver (DT.5f); WebSocketStackResolver::served_routes (DT.3c);
              MountedHttpResolver::without_routes (DT.6a); and on the identity slot
              identity::{identities, HINT_VERSION} (DT.12a). The close-out adds
              build_dart (DT.5h, through the build glob)

code-domain/src/  lib.rs    the id registries (node_kind, edge_category, cell_type) — ids
                            are allocated here and nowhere else; walk_gating, project_roots
  public slots:     data_entity A13.1   evidence LC.3a   external_inputs LF.1a (+LF.4a)
                    glia_config LF.2a (+LG.3d)   profile LD.14a (+LD.14b, LD.6, LE.4d)
                    snapshots LF.5a (+LF.6a)   code_span CD.7c
                    same_as D0.1 + DD.2 (the eight identity rule names, the shared
                    contract-op key, package normalisation, the SCIP key, content
                    digests; the 0.5.2 tweaks add RULE_HTTP_MODEL / RULE_GRAPHQL_MODEL,
                    MODEL_RULES [&str; 2], ALL_RULES [&str; 10] (RULES then MODEL_RULES),
                    IDENTITY_EMITTERS / is_identity_emitter, DT.1h, DT.1k); project_roots
                    gains ProjectRoot.label_source (manifest | dir | root | declared, DD.4)
                    0.5.2 tweaks: wire DT.1a (the contract-model JSON vocabulary: ModelSchema
                    SCHEMA_FIELDS format ts / go / dart / graphql, wire Shape, ServerWire,
                    append_hit_fields, json_kind)   evm DT.10a (+DT.10g: the EVM qname
                    prefixes `evm:` / `evm_call:` / `evm_event:`, Scope, AbiEntry, the
                    procedure and client cells, AbiRef / ClientHit); profile gains
                    INVOKED_MEMBERS / invoked_member (DT.5d, + DT.5e's flutter row), and
                    the close-out's `node_ext` (the one extension reader), ExternalBase,
                    InvokedMembers.{sigs, siblings, owner_bases} with matches_at / gates /
                    checks_signature, and invoked_member_at (DT.5l: rows 12-32);
                    glia_config gains Rejection / LoadedConfig.rejections (DT.7c)

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
   → CODE_PASSES (engine/src/profile.rs): the 15 cross-graph resolvers (HttpStack, gRPC, RPC,
     EVM, Queue, GraphQL, gqlgen, WebSocket, EventBus, SharedSchema, CLI, DB, Cron, Config, IaC),
     then models (the http_model / graphql_model SAME_AS stars, after http and graphql, DT.1h)
     and Identity (SAME_AS stars, the last Resolve pass), then the 9 post-passes, the evidence
     fill (group fills taken at language build, re-filled only where something changed, DC.5)
     and the determinism sort — `[passes] domain=code resolve=17 post=9 finalize=2`
   → merged graph
   → .gmap layout at <repo>/.glia/graph/ (rkyv + mmap, sharded)
   → [optional] activation (PPR) / projection-text / pyo3 → Python
   → [query time] rules (glia-rules over engine::rules::CodeFacts): `glia query`, check's rule
     invariants and reflexion model; identity collapse (engine::identity) on the answers
   → [query time] ranking (0.5.3, engine::rank / intent / calibrate over one holder's RankCache):
     the answer's seed files -> FileIndex -> co-change (the history snapshot) + PPR / BFS
     (a cached PprIndex) -> the calibrated pick, else H10 (pack: its own ranking, never the
     record, ruling EC-9b) -> row order and `files`; review's
     title and find --files' query -> IDR (ids routed at a revision) + BM25 over the stored
     messages -> LX[IDR, ..] or the calibrated pick

snapshot steps (CLI / pyo3, never a build, never a .gmap):
   git log -> `glia history sync` -> .glia/history-snapshot/ (commits, blame, CO_CHANGES;
     0.5.3: version 2 + messages.jsonl, redacted, trailers dropped)
   history snapshot + a built graph -> `glia history calibrate` (0.5.3) -> the leak-free
     online replay -> .glia/calibration-snapshot/ (meta.json written last, bound to BUILD_STAMP)
```

Graph content does not depend on where a tree is checked out: the doc linker binds an ambiguous
mention to the least (qname, kind, graph) candidate and history binds a file's MODULE by least
qname, never by the RepoId-derived NodeId, so two clones of one commit (another remote or
directory name) build the same qname-level graph (DE.1, `engine/tests/repo_id_invariance.rs`;
`[doclink] tie-break by qname: qualified=<q> bare=<b>`). A node id still hashes the RepoId, and
RepoId-ordered tie-breaks remain in pick_primary, cochange, overlay bind, declared and cells
(the 0.5.4 seed, §D).

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

**The 0.5.2 tweaks' markers (DT, waves W15-W30)**, each on stderr, grouped by what they watch:
- *Contract models (T1).* Per file: `[model-schemas] lang=typescript interfaces=<i> classes=<c>
  fields=<f> open=<o> file=<p>` / `[model-schemas] lang=go structs=<s> fields=<f> embeds=<e>
  skipped=<k> file=<p>` (DT.1c); `[ts-wire] responses=<r> requests=<q> (typed= inline= literal=
  annotated= fetch=) request_unknown_client=<u> awaited_generic=<a> file=<f>` (DT.1d) and
  `[ts-wire] graphql_results=<n> file=<f>` (DT.1k); `[go-wire] responses=<r> binds=<b>
  skipped_status=<s> unknown_status=<u> shapes(type= call= chain= object= open=) slice_results=<n>
  in <file>` (DT.1e); `[dart-wire] models=<m> (fromjson= serializable=) skipped_generated=<g>
  responses=<r> requests=<q> skipped_multi_call=<k> file=<p>` (DT.1l). Per Go graph: `[go-wire]
  resolved sites=<r>/<t> models=<m> (type= call= chain=) unresolved=<u> uses=<n> nested=<k>`
  (DT.1f). Per resolve with an HTTP-paired shaped hit: `[models] client_wires hits=<h> routed=<r>
  response=<s> request=<q> client_models=<c> unresolved_client=<u> server_sites=<v>
  server_unknown_status=<k> open=<o> weak_pairing=<w>` (DT.1g), `[models] pairs=<p> response=<r>
  request=<q> model_pairs=<m> changes=<c> breaking=<b> ambiguous_server=<a> open=<o>` (DT.1m),
  `[models] classes=<c> edges=<e> diverged=<d> ambiguous_client=<a> ambiguous_project=<p>
  embed_reduced=<r>` (DT.1h), `[models] graphql results=<r> documents_resolved=<d>
  generated_models=<g> classes=<c>` (DT.1k) and `[evidence-rules] resolver=models http_model=<h>
  graphql_model=<g>`; `[models] graphql pairs=<g> undefined_fields=<u> open=<o> refused=<r>`
  (DT.1n). The answers: `[contract-fields] pairs=.. unknown=<n> http_model=<n> client_contract=<n>
  graphql_selection=<n>` (DT.1i, DT.1n); `[model-breaks] base=<rev> min_tier=<t> exchanges=<E>
  http=<h> graphql=<g> rows=<n> breaking=<b> blocking=<k> advisory=<a> compatible=<c>
  new_pairings=<p> lost=<l> unjudged=<U>` once per contract-breaks / review run (DT.1o, DT.1p);
  `[review] caveats rules=<r> lines=<l> folded=<f>` (DT.1p).
- *Imports.* `[imports] dir-index binds=<n> (relative=<r> alias=<a>)` (DT.1b) and `[imports]
  reexport stars=<s> named=<n> namespace=<p> local=<l> edges=<e> external=<x> entries=<t>
  aliased=<r> bound=<b> unbound=<u> ambiguous=<a> cycles=<c> capped=<k>` (DT.1q), once per
  TS-like graph build that has one.
- *Queues (T2).* `[queues] tag framework=<F> side=<s> sites=<n> (mixed_rows=<m>) path=<f>`
  (DT.2a); `[queues] go-nats side=<s> sites=<n> (typed=<t> untyped=<u>) dropped=<d> (foreign=<f>
  multi_broker=<m>) path=<f>` (DT.2b); `[queues] subject-fold side=<s> folded=<n> (concat=
  template= sprintf= func=) expr=<e> dynamic=<d> path=<f>` and `[queues] resolver placeholder
  producers=<p> paired=<q>` (DT.2c).
- *WebSocket (T3) and HTTP (T4, T6).* `[ws] client urls read=<n> (local= field= builder=
  url_struct=) unread=<x> in <path>` (DT.3a); `[ws-base] folded <n> client sites in <f> files
  (unresolved=<u>) repo=<label>` (DT.3b); `[gaps] ws-served routes=<n>` (DT.3c);
  `[ts-endpoints] query suffixes=<q> file=<p>` (DT.4); `[http] test routes excluded from route
  index: <n>` (DT.6a; `[http] routes=`, `[http-qname] routes=` and `[http-owner] index routes=`
  drop by it); `[gaps] unpaired_route rows withheld: <n> (test-file routes)` (DT.6b).
- *Liveness and UI code (T5).* `[angular-template] inline file=<p> components=<c> calls=<k>
  uses=<u> unbound=<n> shadowed=<s> unread=<r> no_class=<m> receivers=<v> decorated=<d>` (DT.5a,
  DT.5c) and `[angular-template] external repo=<label> components=<c> templates=<t> missing=<m>
  calls=<k> uses=<u> unbound=<n> shadowed=<s> no_class=<x> receivers=<v>` (DT.5b, DT.5c);
  `[recv] class-scope receivers bound=<b> unbound=<u>` (DT.5c); `[live] profile=.. owners=<o>
  implementers=<i> invoked=<v> reached=<r>/<t>` and `[dead-framework] dead_symbol rows withheld:
  invoked=<i> generated=<g>` (DT.5d); `[dart-calls] self= bare= local_skip= closures= creations=
  method_refs= file=` and `[dart-flutter] components=<n> (widgets=<w> states=<s>) file=<p>`
  (DT.5e); `[gqlgen] root structs=<s> object_structs=<o> paired fields=<f> roots=<r> unpaired=<u>
  ambiguous=<a>` and `[evidence-rules] resolver=gqlgen gqlgen_field=<n> gqlgen_root=<n>`
  (DT.5f); `[ts-value-refs] refs=<r> in_file=<i> imported=<m> const_inits=<c> decorators=<d>
  accessor_calls=<a> file=<p>` (DT.5g).
- *The close-out's dead_symbol false positives (DT.5h-DT.5l, W27-W30)*, one marker per line:
  - Dart, per file (DT.5h adds the last five fields; it fires on more files, any initialised field):
    `[dart-calls] self=<s> bare=<b> local_skip=<l> closures=<c> creations=<o> method_refs=<m> accessor_refs=<a> tear_offs=<t> field_inits=<f> member_reads=<r> elements=<e> file=<p>`
  - Dart, per file importing Riverpod, after `[dart-flutter]` (DT.5i):
    `[dart-riverpod] providers=<p> locals=<l> holders=<h> file=<p>`
  - Dart, per graph: `[dart-scope] provider receivers bound=<n>` (DT.5i, when the hook bound
    one), then `[reads] member reads accessor_read=<a> tear_off=<t> static_member=<s> unbound=<u>` (DT.5h)
  - Dart, per file holding a Dio interceptor (DT.5i):
    `[services] dart classified=<n> rules=dio_interceptor:<k> module=<qname>`
  - Go, once per graph after the existing `[go-receivers] bound=..` line (DT.5j):
    `[go-receivers] typed elem=<e> qualified_field=<q> func_value=<f> alternatives=<a> conversion=<c>` and
    `[go-values] uses=<u> (method=<m> func=<f> composite_literal=<c>) dropped=<d> refs=<r> pkg_handlers=<h>`
  - TypeScript (DT.5k): `[ts-locals]` gains `call=<c>` inside its parenthesis; per TS-family file
    `[ts-receivers] query_fields=<q> query_receivers=<r> inject_receivers=<i> inject_refs=<j> static_reads=<s> call_locals=<l> member_reads=<m> accessors=<a> file=<p>`;
    per TS-like graph `[recv] ts receivers bound: expression=<e> returned=<r> accessor_read=<a> accessor_write=<w> member_reads=<m>`
    and `[resolve] static member uses bound: <n>`
  - Liveness, once per `entrypoint_reachable`, right after `[live]` (DT.5l; every INVOKED_MEMBERS
    tag in first-appearance order, zeros included):
    `[invoked] rows=<n> angular=<a> constructor=<c> flutter=<f> go_runtime=<g> apple_app=<p> transloco=<t> dio=<d> sig_rejected=<r>`
  - Python / Swift, per file (DT.5l): `[py-main] guard module=<qname> calls=<n> file=<p>`,
    `[py-nested] nested-def calls bound=<n> file=<p>` and `[swift-app] entry types=<n> file=<p>`;
    the Kotlin `[kotlin/retrofit] .. components=<c>` counts Flutter activities. A Go `init` entry
    counts in `[live] kind=`, a `__main__` script MODULE in `[live] declared=`.
- *Rules and check (T7, T8).* `[declared] .. anchor_enclosing=<e>` (DT.7a); `[check] rules=<R> ..
  errors=<E> unanchored=<n>` and `[check] declared not checked: orphaned=<o> rejected=<r>
  absent=<a> empty_scope=<e> blocking=<b>` (DT.7b); `[check] overlay rejected root=<r>
  whole_file=<b> constraint=<c> component=<m> layer=<l>` and the try-rules suffix
  ` checks_nothing=<n>` (DT.7c); `[check] caveats rules=<r> lines=<l> folded=<f>` (DT.8a);
  `[gql] anchor column=<var> rows=<n> merged=<m>` (DT.8b).
- *Smart contracts (T10).* `[sol-abi] procedures=<n> (functions=<f> getters=<g> abstract=<a>)
  contracts=<c> path=<f>` (DT.10a); `[evm-link] calls= events= paired= edges= (contract= abi=
  abi_hint= name_hint=) unmatched= ambiguous= conflict= not_declared= arity_mismatch=
  not_in_abi= procedures= contracts=` and `[eventbus-evm] solidity_events=<s>
  evm_subscriptions=<h> left to resolver:evm` (DT.10b); `[evm-client] lang=ts|go calls=
  events= sites= (ethers= hardhat= typechain= viem= web3= geth=) abis= not_in_abi= dynamic=
  path=` per file (DT.10c-DT.10k; `lang=go` from DT.10e); `[evm-wrappers] wrappers=<w> (direct=
  forwarded= ambiguous=) sites=<s> dynamic=<y> path=<f>` (DT.10f); `[evm-abi-join] joined=<j>
  unresolved=<u> ambiguous=<a> declarations=<d> repo=<label>` (DT.10g).
- *GraphQL (T11).* `[graphql-sdl] files=<f> resolvers=<r> types=<t>` and `[graphql-sdl] code
  types=<n> path=<p>` (DT.1j); `[graphql-root-fields] ops=<o> fields=<f> pairs=<p>
  name_key_ops=<k>` (DT.11a); `[graphql-transport] endpoints=<n> (absolute= relative= template=)
  skipped_ws=<w> path=<f>` (DT.11b); `[gaps] graphql transports implied=<n> unpaired=<m>` (DT.11c).
- *Hints (T12).* `[cells] write .. target=hint_differs .. differs=<fields>` (DT.12b) and
  `[engram-export] hints side: <facts> facts, <hinted> v1 hints, <ms> ms -> <path>` (DT.12a).

**The 0.5.3 markers (E0-EG, waves W0-W15)**, each on stderr unless noted, grouped by what they watch:
- *The PPR index and its cache (EA.1, EA.2, EA.7, EA.8).* GLIA_ACTIVATION_DEBUG=1's `[activation] plan
  mode=<m> .. index=<prebuilt|built|none>` (the field is new), and `[activation] index mismatch mode=<m>`
  unconditionally when a caller's index was built for another config; `[rank-cache] holder=<pygraph|watch|
  cli|..> source=<merged|quotient> damping=<d> direction=<dir> index=hit|built nodes=<n> incidences=<i>
  build_ms=<ms> entries=<e>` per lookup, `[rank-cache] reset holder=<h> reason=graph_changed nodes=<a>-><b>
  edges=<c>-><d>`, and for the calibration's own cache one `[rank-cache] holder=calibrate summary
  lookups=<l> built=<b> hits=<h> build_ms=<ms>` on drop (EG.7).
- *The file space and co-change (EA.3, EA.4).* `[rank-files] holder=<h> repos=<r> files=<f> generated=<g>
  not_code=<n> ms=<ms>`; `[rank-cochange] holder=<h> source=<snapshot|given> repos= snapshots= commits=
  train_commits= files= pairs= ms=` or `[rank-cochange] holder=<h> none reason=<no_roots|no_snapshot>`;
  `[rank-files] rank ranking=<harness name> seed_files=<s> extra_seeds=<e> candidates=<n>
  cochange_files=<c> index=<hit|built|bfs> ms=<ms>` once per rank_files.
- *The ranked answers (EA.5, EA.6, EA.6b).* After `[blast] seeds=`: `[rank] surface=<blast_radius|
  diff_impact|review> ranking=<P1|PT*|B0|H10|H10[PT*]|H10[B0]> seed_files=<s> cochange_files=<c> files=<f>
  lifted=<l> closure_index=<hit|built>` (`files=0 reason=no_seed_file` with no seed file); before
  `[pack] query=`: `[rank] surface=pack ranking=.. seed_files= cochange_files= files=0 lifted=<l>
  closure_index= lifted_files=<f> max_file_share=<m>/<k>` (lifted = one best row per co-changed file).
- *History messages (EB.1).* `[history] sync repo=<l> head=<12> commits=<n> .. window=.. messages=<n>
  redacted=<r> branch_labels=<b> trailers_dropped=<t> surface=<s>` (`messages=off` with --no-messages);
  `[history] messages incomplete dir=<d> reason=<r>` on a bad sidecar.
- *Intent (EB.2-EB.8).* `[algo] fuse keys=<k> candidates=<n> excluded=<e> zero_first_key=<z>` under
  GLIA_ALGO_DEBUG=1; `[intent] text repo=<l> surface=<s> docs=<d> skipped_wide=<w> excluded=<e>
  tokens=<q> matched_docs=<m> files=<f>` (or `history=absent|messages=absent`); `[ids] vocab repo=<l>
  surface=<s> patterns=<n> source=<config|default|off|override> trackers=<t> rejected=<r> markdown=<m>`
  (`override`: CalibrateOptions.vocab, EC.14); `[id-defs] repo=<l> rev=<sha12|worktree> ..`; `[id-ns]
  repo=<l> series=<s> local=<l> merged=<m> multi_series_ids=<x> namespaces=<n> lineage_merges=<g>
  lineage_redirects=<r> lineage_capped=<c>`; `[id-paths] repo=<l> surface=<s> candidates=<c>
  renamed=<h> resolved=<n> routes=<route:n,..> ambiguous=<a> over_cap=<o> unresolved=<u>`; `[idr]
  repo=<l> surface=<s> rev=<r> ids= resolved= unresolved= routes= lineage_keys= scopes= named_files=
  cited_files= files= recent=`, and the replay's one `[idr] replay surface=calibrate sides=<s>
  routed=<r> resolved=<x> unavailable=<u> ms=<t>` (EC.13); `[review-intent] base=<rev>
  title_source=<arg|commit|none> fusion=<pick> ids=<n> idr_files=<i> text=<present|absent>
  text_files=<t> candidates=<c> reordered=<r>` after `[review] base=`; `[find-files] repo=<l>
  surface=<cli|py|lib> fusion=<pick> ids=<n> idr_files=<i> text=<present|absent> text_files=<t>
  rows=<r>`.
- *Calibration and selection (EC.1-EC.14).* `[calibration] read root=<r> status=<s> reason=<r>
  sets=<n> history_head=<h>`, `[calibration] wrote dir=<d> sets=<n> build_stamp=<b>
  history_head=<h>`; `[calibrate] replay commits=<c> train=<t> test_eligible=<e> test=<n>
  messages=<yes|no> queries k1= k2= k3= half= find=`, `[calibrate] bank structure files=<f> nodes=<n>
  ppr_indexes=2 carry_edges=<e>`, `[calibrate] text builds=<b> docs_max=<d> messages=<yes|no> ms=<ms>`,
  `[calibrate] work trained= segments= labels= m_src mbr= mgap= none= work=<author_time|committer_time>`,
  `[calibrate] idr unavailable commits=<n>`, per row `[calibrate] set=<SET> shape=<k> candidates=<n>
  test_commits=<t> queries=<q> pick=<name> last_online_pick=<name> selection_r10=<x> fallback_r10=<y>
  refused=<no|below_fallback>`, `[calibrate] candidate <name> returned <n> keys of <m>` (drops that
  set's rows), and `[calibrate] done surface=<cli|pyo3|..> sets=<n> test_commits=<t> ms=<ms>`;
  `[work] head w5_files= m_files= m_src= surface= work=`; per answer `[select] surface=<blast_radius|
  diff_impact|review|pack|review_intent|find_files> set=<SELh0|SELs0|SELt2|SELf2t> shape=<k1|k2|k3|half|
  find> pick=<name> source=<calibrated|fallback|explicit> reason=<r|-> test_commits=<n>`.
- *The harness and the gate (EA.9, EB.9, EG.1-EG.7; engine/examples, not hashed).* `[rank_eval] ans
  rankers=<n> single=<q> transfer=<t> find=<f> ms=<ms>`, `[rank_eval] ans calibration sets=<n>
  test_commits=<t> ms=<ms>`, `[rank_eval] ans intent review=<q> find=<f> idr_parent_reads=<p>
  idr_commits=<i> ms=<ms>`; `[rank-parity] files ..` and `[rank-parity] label=<l> shape=<s>
  ranking=<r> queries=<q> key_mismatch=<k> order_mismatch=<o>` (`--engine-parity on`); `[rank-eval]
  engine parity repo=<l> t1s_raw_mismatch=<q>/<n> ..` (`--engine on`); `[rank-gate] verdict label=<l>
  rows=<n> drops=<d> gains=<g> new=<w> drift=<k> missing=<m> changed=<c> correction=holm alpha=0.05`
  per check, `[rank-gate] drift ..` / `missing ranker=..` / `new ..` / `refused <name>: not a public
  https snapshot`, and scripts/rank-gate.sh's last lines `[rank-gate] check: ok - <n> rows, <g> gains,
  <d> drops on <s> snapshots` (or `check: FAILED (<k> drops: ..)`) and `[rank-gate] coverage: ok -
  <n> items measured, <e> exempt`.
- *Hygiene (EG.4, EG.5).* engram-export prints no PPR side line any more; scripts/check-engram-export.sh
  ends `[engram-export] check: ok - build + <N> tests passed against engram-core
  GMAP_FORMAT_VERSION=<V>`. The wheels workflow's `[wheels-ci]` check is a PR run's log: no
  `Node.js 20 is deprecated` annotation, runner lines `Image: ubuntu-24.04` / `windows-2025-vs2026` /
  `macos-14-arm64`. The release-docs packets' own: `[readme-surface] commands=<n> usage_lines=<u>
  flags_checked=<f>` and `[skill] <name>: <n> invocations checked against cli/surface (<k> stale)`
  (EZ.1), `[check-neuropil]` (scripts/check-neuropil.sh, EZ.2).

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
  0.5.2 tweaks, TS family:
  - **Directory imports (DT.1b).** `ts_target` (graph/src/imports.rs) binds a specifier that
    names no file extension from a TS / JS / Vue importer to `<dir>::index` (exactly, or through
    the same-stem pick of an index.ts + index.js pair) once the exact module and same-stem
    lookups fail; a file always wins over the directory of its stem. IMPORTS rule `dir_index`,
    `[imports] dir-index binds=<n> (relative=<r> alias=<a>)`.
  - **Re-export barrels (DT.1q).** The TS parser records `NavFact::ReExportStar` /
    `ReExportNamespace` / `ReExportName` / `ExportLocal` and one `Exports` (every name the file's
    own `export` declarations export, node or not). `resolve_imports_ts` resolves them as
    ECMA-262 ResolveExport in `graph/src/imports/reexports.rs`, with one IMPORTS edge from each
    barrel to each module it re-exports from (rule `reexport`). The barrel's `module_symbols`
    gains its same-name re-exported declarations; renamed, namespace and default entries bind
    only through a named import (a build-time side table). A local or indirect export shadows
    the stars even when it mints no node, two star sources exporting distinct bindings give
    nothing, `export *` never re-exports `default`, a path set ends cycles and depth 32 ends
    chains. A ref a name-only fallback bound to the same target is re-emitted `import_binding`
    (heuristic -> fact).
  - **Value references and accessors (DT.5g).** A module-level `const X = <value>` initialiser
    and a class decorator's arguments are walked (credited to X's STATE_VAR, the MODULE or the
    class); an identifier passed by value (a whole call / `new` argument, an array element, a
    pair value, a shorthand property, `export default <id>`) is a USES of the in-file top-level
    FUNCTION / CLASS / STATE_VAR / ENUM it names (rule `value_ref`) or a Bare USES ref the graph
    binds through a named import; a `this.<accessor>` read or write is a CALLS of the accessor
    (rules `accessor_read` / `accessor_write`). `[ts-value-refs] ..`.
  - **Receivers the graph could not type (DT.5k).** The TS parser states what the AST says about
    a receiver, and `graph/src/calls.rs` binds through the existing receiver chain, never through
    a repo-unique name. New NavFacts on the scope: `ReceiverType { receiver, ty }` for an Angular
    signal query field (`viewChild` / `contentChild` and `.required`, import-gated, alias
    honoured; a `read` option wins over the type argument and the locator; list queries and
    string locators type nothing) and an inline `inject(T)` receiver; `MemberRead` (DT.5h's
    variant, TS its second user) / `MemberWrite { receiver, name, line }` for a member access on
    a typed this-field, local or fact-typed expression; `Getter` / `Setter` on a `get` / `set`
    METHOD. `receiver_type` reads a ReceiverType for a ComplexReceiver naming no field (CALLS rule
    `receiver_type`); `receiver_type_node` types a call-result local (`const store = makeStore()`,
    the parser recording the call form `f()` unless shadowed, FUNCTION return annotations filling
    `CodeNav::return_types`) through the callee's declared return type (rule `receiver_return`);
    `resolve_member_reads` (TS-family scopes only) binds a getter read / setter write on another
    typed object as CALLS rules `receiver_accessor_read` / `receiver_accessor_write`. An inline
    `inject(T)` that is no field's whole initialiser is an INJECTS from the scope that runs it
    (functional guards and interceptors, InjectionToken factories, call-initialised fields; a
    Bare ref for an import, parser rule `inject_call` in-file, first argument only: a later one
    is a DT.5g `value_ref` USES); an in-file `T.X` read outside T, or a cross-file one whose
    member binds nothing, is a USES of the CLASS T (rule `static_member`); Pattern E's
    repo-unique fallback is skipped for a TS INJECTS ref from a non-CLASS. `[ts-receivers] ..`,
    `[recv] ts receivers bound: ..`, `[resolve] static member uses bound: <n>`.
  - **Angular templates (DT.5a-DT.5c).** An inline `template:` / `host:` binding, and a
    `templateUrl` .html (the walk admits one beside a same-stem .ts; a post-cache graft binds
    it, `engine/src/build/angular_templates.rs`), that calls or reads a METHOD of its component
    is a CALLS / USES from the CLASS (EVIDENCE `extractor:angular_template`, rules template_call
    / template_ref / host_call / host_ref, at the binding's row). A one-hop `member.m()` in a
    template is a CallSite from the CLASS with qualifier `this.<member>`, bound by
    `calls::receiver_type`, which now reads a CLASS / STRUCT / ENUM caller's OWN field type
    (`[recv] class-scope receivers bound= unbound=`); `@HostListener` / `@HostBinding` /
    `@Input` and query setters are USES from their class (rules host_listener / host_binding /
    input_setter / query_setter).
  - **Client wire shapes (DT.1d, DT.4).** A typed or bodied HTTP call site's `response` /
    `request` shape is spliced last onto its ENDPOINT_HIT (wire::append_hit_fields), and an
    awaited generic call (`await axios.get<T>(..)`) mints its ENDPOINT; a URL whose last
    substitution is a query string (`${base}/smr${q}`) drops it from the path and records
    `"query":"<ident>"|"inline"` on the hit (`[ts-endpoints] query suffixes=`).
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
  constant folds (DF.7). 0.5.2 tweaks: the Go parser writes one wire::ServerWire MESSAGE_TYPE
  cell (form `json_response` / `json_bind`, window `site`) per known-2xx / unknown-status JSON
  write and per JSON bind in a gin / echo / fiber / net/http / chi handler, skipping a non-2xx
  status (DT.1e), plus `NavFact::SliceResult` on `[]T` results; `graph/src/build/go_wire.rs`
  then types each cell's value to its in-repo STRUCT (a written type through the imports, a
  call through its callee's first result, a chain through SCHEMA_FIELDS) and writes one
  handler -USES-> STRUCT per (handler, struct, form), emitter `graph:go_wire`, rules
  `json_response` / `json_bind` (Strong from a written type, Medium through call / chain
  typing); the cell's shapes gain `model` and a STRUCT's SCHEMA_FIELDS fields their nested
  `model` (DT.1f). The close-out (DT.5j): `go_type_name` keeps a field's package qualifier
  (`pkg.T`, read through the declaring struct file's imports by `field_text`, so the qualified
  text never binds back onto a same-named wrapper) and records a slice / array / map of a named
  type as `[]E`; a variadic is `[]T`, a range value variable `<chain>[]`, a func literal or
  func-typed local `func() R`, and a local several declarations of one fn scope type keeps
  agree-or-unknown alternatives `a|b` (at most 4); a composite literal `T{..}` / `&pkg.T{..}`
  reads as the conversion `T()`. GoPackages' `type_of_text` resolves them (`chain_hop`,
  `elem_type`), so `for _, c := range s.checks { c.Check(..) }` binds the interface METHOD
  CH.1c's dispatch reads; a qualified-field call the generic pass bound by bare name
  (graph:calls `receiver_type`) is now bound by the hook through the import (graph:go_packages
  `receiver_field_chain`, same target). The parser queues USES value refs (arguments, literal
  elements, `=`, `:=`, `var`, `return`, a package var's initialiser; dropped for a route's
  handler argument, inside a DI registration, rooted at an external package, or shadowed by a
  local, parameter, named result, body const or fn-local type), and `bind_value_refs` binds a
  METHOD (rule `method_value`), a FUNCTION (`func_value`) or a STRUCT a composite literal builds
  (`composite_literal`; file, package or the import's package, never repo-wide), emitter
  graph:go_packages; so Go STRUCTs carry incoming USES from their literals. CI.4's
  `package_handler` binds a `pkg.F` route handler to a FUNCTION in another file of the imported
  package (HANDLED_BY rule `package_function`). `[go-receivers] typed ..`, `[go-values] ..`.
- Rust: `impl Trait for Type` with an imported or path-qualified trait is IMPLEMENTS, and axum
  `State` / `Extension`, actix-web `web::Data` and Rocket `&State` handler parameters are INJECTS
  to the held type, both bound by Rust path rules through `RustIndex::resolve_type_refs`, never by
  bare name (DG.13a, DG.13b, `[rust-types] implements=<b>/<r> injects=<b>/<r> ..`).
- Ruby `Calc.new.add(..)` binds through the constructed type (CL.5a's rule, extended to Ruby's
  postfix `.new` by DH.13), so Minitest methods get function-level TESTS; `class X < Base` is
  INHERITS_FROM. PHP `extends` / `implements` are heritage edges (DH.14), and a `use` of a
  declared namespace binds that namespace's member (DF.2). Dart resolves own-package
  `package:<pubspec name>/..` URIs (DF.4a) and binds a plain import's public names (DF.4b).
  0.5.2 tweaks (DT.5e): a call inside a Dart closure is its enclosing member's, `const T(..)` /
  `new T(..)` is a call, a METHOD / FUNCTION passed by name is a USES (rule `method_ref`), and a
  class extending one of 13 Flutter / Riverpod / flutter_hooks bases gets ROLE COMPONENT (an
  entry); `INVOKED_MEMBERS`' flutter row makes `build` / `createState` / `initState` /
  `dispose` .. of a used one live. DT.1l: a `factory C.fromJson` / `@JsonSerializable` class
  writes SCHEMA_FIELDS format dart, and a Dio call's response / request shape joins its
  ENDPOINT_HIT. The close-out: Dart builds through `glia_graph::build_dart` (DT.5h:
  build_typescript's passes, then `build::dart::resolve_member_reads`, then method-level
  IMPLEMENTS). Every identifier the body walk visits that is a name reference
  (`is_name_reference`: not a label, break / continue target, member or cascade selector,
  annotation, symbol, `name` / `constructor` field or callee; `$name` interpolations included)
  is a NameRef, in any value position (conditional branches and conditions, `??` operands,
  initialisers, collection and record elements, returns, asserts, case constants): a getter /
  setter target is CALLS rule `accessor_read` (`accessor_write` for an assignment's whole left
  side), another METHOD / FUNCTION USES `method_ref`. `T.new` and a `constructor_tearoff` are a
  CallSite of T; a class field's initialiser is walked from its CLASS (a self-loop when a class
  constructs itself in a static field); a one-parameter closure over a `List<T>` / `Set<T>` /
  `Iterable<T>` field types its parameter T. A `Type.member` / `field.member` / typed-local read
  no call follows is `NavFact::MemberRead`, bound in the reader's own scope (`calls::scope_type`,
  factored out of CL.5a's constructed_type_id): a getter is CALLS `accessor_read`, a method USES
  `tear_off`, a static field USES its CLASS `static_member`, emitter graph:reads. Dart getters /
  setters fill `RepoGraph.properties`. DT.5i: a top-level provider STATE_VAR of a file importing
  riverpod / flutter_riverpod / hooks_riverpod / state_notifier records
  `NavFact::RiverpodProvider { read, notifier }` (`Provider<T>` / `StateProvider` /
  `ChangeNotifierProvider` read T, `StateNotifierProvider<N, S>` / `NotifierProvider` read S and
  notifier N, `.autoDispose` / `.family` and a generic-less factory read through; FutureProvider
  / StreamProvider record nothing); `final svc = ref.read(p);` types svc by the read's text (a
  name bound twice in its scope is never typed); `provider_call`, resolve_calls' extra hook,
  binds `ref.read(p).m()`, `ref.read(p.notifier).m()` and `svc.m()` only to a METHOD of that type
  (CALLS emitter graph:dart rule `provider_receiver`), and `provider_type`, the member reads'
  hook, makes `ref.watch(p).isReady` a getter read; a StateNotifier<T> / Notifier<T> /
  ValueNotifier<T> subclass's `state` / `value` field is a T. A class extending a Dio
  Interceptor / QueuedInterceptor / InterceptorsWrapper / QueuedInterceptorsWrapper in a file
  importing package:dio/ gets ROLE SERVICE (the services extractor's rule `dio_interceptor`,
  HEURISTIC, folded into the CLASS by LB.3a). Solidity (DT.10a): one RPC_PROCEDURE `evm:<Contract>.<fn>` per external /
  public function, interface member and public getter, HANDLED_BY its METHOD / STATE_VAR.
- Runtime-invoked entries (DT.5l, the close-out). Python: an `if __name__ == "__main__":` guard's
  consequence is walked from the MODULE (its calls are MODULE CALLS, and its client calls run
  the call-site detectors from the MODULE), which gets one ENTRYPOINT cell
  `{"decl":"<file>:<1-based row>","source":"main_guard"}`, so `[live] declared=` counts it; a bare
  call binds the caller's own nested def first, then outwards through enclosing FUNCTIONs, then a
  module-level def (CALLS rule `nested_def`; the edge that bound a shadowed module-level def
  moves). Swift: a `@main` / `@UIApplicationMain` / `@NSApplicationMain` type gets ROLE COMPONENT.
  Kotlin: a class extending FlutterActivity / FlutterFragmentActivity / FlutterFragment /
  FlutterApplication under an io.flutter import is a COMPONENT. A Go `func init()` is an entry
  through the entry table's `.go`-limited named row (Activation below); the Go parser's
  `method_signature(code)` reads CA.3a's normalised signature back from a METHOD's CODE text for
  the go_runtime INVOKED_MEMBERS rows.
- An `extra_hook` seam on `resolve_calls` lets a language contribute resolution the generic
  walker misses; it is consulted only after every generic lookup failed. Six builders use it
  (`build_dart` hands it `build::dart::provider_call`, DT.5i; the other five follow):
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
GraphQL client ORIGIN `external` (DF.10). Rules are query-time and store nothing (Q8). The 0.5.2
tweaks keep the format too, and move content again: SCHEMA_FIELDS (code-domain `wire::ModelSchema`,
format ts / go / dart / graphql) on TS interfaces and field-only classes, Go structs, Dart models
and GraphQL SDL types (DT.1c, DT.1l, DT.1j); MESSAGE_TYPE wire cells on Go handlers (DT.1e,
`model` keys from DT.1f), GraphQL root fields (`graphql_returns`), operations (`graphql_selection`)
and TS callables issuing a typed GraphQL call (`graphql_result`, DT.1k); trailing `response` /
`request` keys on TS and Dart HTTP ENDPOINT_HITs (DT.1d, DT.1l), `query` on a query-suffixed one
(DT.4), `host` on a folded WS client's (DT.3b) and `"via":"graphql"` on a GraphQL transport
ENDPOINT's (DT.11b); the evm cells (a JSON CODE on each RPC_PROCEDURE, an evm ENDPOINT_HIT on each
client, DT.10a / DT.10c, `abi_ref` / `abi_from` since DT.10g); SAME_AS rules `http_model` /
`graphql_model` (emitter `resolver:models`); and a CODE text cell on every GRAPHQL_OPERATION read
from a document (DT.11a).

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
center, written by IdentityResolver (DD.5) and, for client <-> server models, ModelResolver
(DT.1h). The 0.5.2 tweaks (DT) allocate none: EVM procedures and clients reuse RPC_PROCEDURE /
RPC_CALL / EVENT_HANDLER and RPC_CALLS / EVENT_FLOWS, GraphQL schema types MESSAGE_TYPE, model
fields SCHEMA_FIELDS. The next free ids are node 50 / edge 38 / cell 26.
SHARES_DEPENDENCY (31) stays registered with no emitter since DD.16 (an id is never reused), so an
old layout still decodes it.

IDs ref: `dev-notes/glia-memory/reference_kind_category_ids.md` and `reference_code_domain_registries.md`.

## Activation (PPR)

Personalised PageRank with damping = 0.5 (not custom spreading activation). `ActivationConfig` is domain-agnostic: direction, edge weights, and node specificity are all provided by the domain, not hardcoded. Code-graph adaptations: edge weights, direction, node specificity — three dials the domain sets.

The domain sets them in its **profile** (LD.14a/b), and nowhere else. The code domain's
`DomainTables` is `CODE_TABLES` in `code-domain/src/profile.rs`: the entry rule (the
kinds, roles and `main` / `test*` names liveness seeds from, and Go's `init`, a row limited to
`.go` files, DT.5l), `carry_edges` (what blast
radius, cross-stack trace and liveness follow), `effect_sinks`, the base
`activation_weights` and the named `activation_presets` (`repair` / `review` /
`onboard`). The engine wraps it with the build passes as `CODE_PROFILE`
(`engine/src/profile.rs`). A PPR config is `CODE_TABLES.activation_config(None |
Some(preset))`; `MergedGraph::blast_radius` takes `&DomainTables`. The graph, store and
projection crates read `CODE_TABLES` directly (they cannot depend on the engine); a new
entry kind, carry edge or weight is one table row there, never a list in a consumer.

0.5.3 keeps every table and default above: damping stays 0.5 in `CODE_TABLES`, so hotspots,
communities and every unranked PPR read are unchanged. What moves is how a computed PPR is kept
and read. `glia_activation::PprIndex` (EA.1) is the index `ppr_vector` used to rebuild per call
(`ppr_vector` is now `PprIndex::build(..).vector(..)`, pinned bit for bit by an oracle copy of the
0.5.2 code); the engine keeps up to 8 of them per holder in `rank::RankCache` (one per `PyGraph`,
one per `WatchSession`, replaced after an update that changed the graph, one per CLI call; found by
`SourceKey` and `PprIndex::matches` on the dials), so a long-lived holder builds each once. The
ranked answers read three structural bases (`rank::RankBase`): `P1` (undirected PPR at the code
domain's damping over CODE_TABLES' weights, max-pooled per file), `PT*` (the same at
`rank::WIDE_DAMPING` = 0.85, sum-pooled: rank_eval's picked `base,d85`, kept out of CODE_TABLES
because round 3 measured it -0.035 on one repo and +0.019 on another, so it reaches an answer only as
a calibrated candidate) and `B0` (carry-edge BFS both ways, `[-depth, file degree]`), each optionally
behind co-change first (`H10..`). The text and fusion math is domain-free too: `algo::fuse` (EB.2);
the code-domain readings (FileIndex, co-change, IDR, the calibration replay) live in the engine.

Identity is a table too (DD.3): `CODE_TABLES.identity_rules` holds the ten ways two nodes are
one thing, one `IdentityRule` per `same_as::ALL_RULES` name (the kinds it pairs, the key, the tier
and whether a class walks): `package_name` (FACT, never walks: a registry package would join every
dependant), `manifest_name` (FACT, a dependency and the project its manifest names), `message_fqname`
(FACT), `contract_op` / `contract_route` (DERIVED), `scip_symbol` (FACT, DD.7), `body_hash` /
`path_suffix` (HEURISTIC vendored copies, DD.6: same-as candidates, never collapsed), and the two
`MODEL_RULES` the models pass writes (emitter `resolver:models`, so `same_as::is_identity_emitter`
names both emitters): `http_model` (DERIVED, walks; a client INTERFACE / CLASS -> the server STRUCT
of one HTTP exchange whose JSON names confirm it, DT.1h) and `graphql_model` (DERIVED, walks; a
GraphQL client call's result model or gqlgen's generated STRUCT -> the SDL MESSAGE_TYPE
`message:graphql:<T>` its root field returns, DT.1k); either is `/diverged` when the fields
differ. Liveness reads one more table (DT.5d): `INVOKED_MEMBERS`, the members a framework or the
runtime calls (Angular lifecycle hooks, CVA / Validator members, pipe `transform`, interceptors,
guards, Flutter `build` / `createState` / .., and constructors per language), which a used owner's
walk-local hop reaches (`invoked_member`, the one matcher). The close-out (DT.5l) widens both
tables. The entry rule gains `FUNCTION init` limited to `.go` (`NamedEntry.exts`, empty = any
file, compared ignoring ASCII case; `validate` rejects an empty or dotted ext), so a TS or Python
`init` stays an ordinary name: the entry readers (liveness, trace, communities, splits,
projection-text and pyo3 records) call `EntryRule::is_entry_at(kind, name, roles, ext)`, which
reads `code_domain::profile::node_ext(cells)` (the POSITION file's extension) lazily, only when a
limited row matches kind and name, at most once; `is_entry` stays extension-blind, so a limited
row never matches there. hubs' and duplicate-flows' test-node checks read only the named rows'
`prefixes`, which the `init` row has none of (a limited row with prefixes would need them to call
`accepts_ext`: the 0.5.4 seed). `INVOKED_MEMBERS` gains rows 12-32 and
three gates on a row, each unchecked when empty and read only by `InvokedMembers::matches_at` /
`invoked_member_at` (the 4-argument `invoked_member` never matches a gated row): `sigs`, the
member's CA.3a signature re-read from its CODE (`go_runtime`: `Error` / `String` `()(string)`,
`Unwrap` beside an `Error`, the JSON / text / BSON marshalers and unmarshalers, `Scan`, `Value`,
`ServeHTTP`, and `Len` / `Less` / `Swap` only together); `siblings`, the (name, signature) pairs
the owner must also declare; and `owner_bases`, an `ExternalBase { library, name }` the owner
extends or implements, read as its heritage ref left unbound, no in-repo type of that name, and
its file's IMPORTS cell listing the library (`transloco` loaders / handlers / interceptors /
transpilers / fallback strategies; Angular ErrorHandler `handleError`, Resolve `resolve`,
TitleStrategy `updateTitle`, RouteReuseStrategy; `dio` interceptors' `onRequest` / `onResponse` /
`onError`). The `apple_app` row makes an `@main` COMPONENT's app-delegate callbacks and SwiftUI
`body` live. `[invoked] rows= .. sig_rejected=` follows `[live]`. SAME_AS
weighs 0 in PPR and is in no carry or community list: identity reaches a walk or a ranking only
through the quotient (`algo::classes::Quotient`), never as an edge. Answers collapse FACT /
DERIVED classes to one row carrying `identity {tier, rules, diverged, also_in}` (Q17), and walk
classes (messages, contract ops, SCIP symbols, package <-> project, client <-> server models) are
one node in blast radius, trace / flows and tests-for walks and their PPR (DD.10-DD.12). A model
class forms inside one repo too (a monorepo's TS interface and Go struct), so a single-repo answer
can fold a client model into its server model.

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
`resolvers/http.rs` is the canonical example. Shipped: 15 cross-graph resolvers, in pass order
http (a written-out `PassSpec`: it takes the overlay's route mounts and, since DT.6a, leaves the
test-only ROUTEs out of its index), then 14 `resolver!` rows: grpc, rpc, evm (EvmContractResolver,
DT.10b), queue, graphql, gqlgen (GqlgenResolver, DT.5f), websocket, eventbus, shared_schema, cli,
db, cron, config, iac; then `models` (a `PassSpec` with `after: ["http", "graphql"]`, the
http_model / graphql_model SAME_AS stars, DT.1h / DT.1k) and Identity, the last Resolve pass
(SAME_AS, DD.5): 17 Resolve passes in all. MessageSchema and Package were removed in 0.5.2
(DD.16), their pairs now identity classes. `resolver:evm`'s `abi_hint` / `name_hint` pairs are
HEURISTIC (Weak; `why::HEURISTIC_PAIRING_RULES`), so a check violation through one is advisory;
its `contract` / `abi` pairs and gqlgen's are DERIVED. EventBusResolver leaves Solidity events and
`evm_event:` handlers to resolver:evm (James's ruling 2, `[eventbus-evm]`). See
`dev-notes/glia-memory/project_040_stack_resolvers_backlog.md`.

## Key Design Decisions

- **Tree-sitter, not regex.** 0.4.x moved to AST extraction; 0.2.0 regex is not a ceiling.
- **Zero-copy store.** rkyv + mmap; writes rebuild the whole file.
- **Domain-agnostic core.** `core` and `activation` know nothing about code. Code lives in `code-domain` and the parsers.
- **Generic graph algorithms live in `activation::algo`, over `GraphSource`, never in engine.** Reachability (`algo::reach`, LD.15a; `bfs_levels`, DA.1), graph delta (`algo::delta`, LE.1a) and Tarjan SCC (`algo::cycles`, LE.6a); 0.5.2 adds identity classes and the quotient (`algo::classes`, DD.1) and push PPR (`algo::ppr`, DE.5); 0.5.1 adds label propagation and seeded Leiden with `modularity` (`algo::community`, CD.1a/b), Stoer-Wagner and the minimum s-t cut (`algo::cut`, CD.2a), neighbourhood pair scoring (`algo::linkpred::score_pairs`, CD.3a), degree tables and HITS (`algo::hubs`, CD.4a), MinHash + LSH (`algo::minhash`, CD.4d) and validity spans over consecutive snapshots (`algo::timeline::TimelineBuilder`, CD.5a); 0.5.3 adds rank fusion (`algo::fuse`, EB.2) and the kept PPR index (`PprIndex`, EA.1, beside `plan`). A graph type opts in by implementing `GraphSource`, and a walk runs over a per-query CSR `Adjacency`, never a scan of the edge list per visited node; the engine answers that use them (`communities`, `splits`, `hubs`, `duplicate_flows`, `timeline`, the `suspected_edge` gap) add the code-domain reading on top: labels, services, located rows, tiers.
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
  `[rules] prelude relations=<n> (..)`. The 0.5.2 tweaks: a GQL `RETURN` of properties only is
  located at the first property's node, as its Datalog twin is (`gql::Compiled.anchor`, a hidden
  trailing column folded away, `[gql] anchor ..`, DT.8b; an aggregate row stays unlocated); the
  prelude's `unhandled_route` skips a ROUTE stamped ORIGIN `test_fixture` (`origin/2` reads no
  edge, so the caveats stay HANDLED_BY's, DT.6b); a row through a `resolver:evm` `abi_hint` /
  `name_hint` edge tiers heuristic.
- **Rankings are query-time (0.5.3).** Whatever a ranking reads from history is a snapshot step
  (`glia history sync`, `glia history calibrate`), never a build pass and never a `.gmap`: the
  build's inputs are byte-identical with or without messages or a calibration record. Each port
  reproduces `engine/examples/rank_eval.rs`'s arithmetic (float accumulation order, tie rules) and
  is pinned by a parity test against it; ties go by (repo label, path) or candidate index, never a
  NodeId or HashMap order. Commit messages and branch labels are untrusted text, redacted
  (`redact_untrusted`) before they are stored, author identities never read. A calibration record
  is read only by the BUILD_STAMP that wrote it, at RANKER_REVISION 1 and with its candidate-set
  digests; anything else falls back with a reason. The rank gate (`bash scripts/rank-gate.sh check`,
  six pinned public snapshots, judged against `bench/rank-gate/baseline/0.5.2/` and `current/`,
  each its own Holm family, one-sided, alpha 0.05) is part of every close-out that touches a ranked
  path; a drop fails it, and only `baseline` / `baseline-add` / `ratchet` write the baselines.
- **Publish gate.** Only `py/` publishes to PyPI (as `glia-py`, imported as `glia_py`). Everything else is internal workspace.
- **No Python fallback.** After v0.4.10c, Python is a thin pyo3 wrapper; there is no parallel Python implementation to keep in sync.

## Query & Answer Surface (v6 P2/P3, 0.5.0, 0.5.1, 0.5.2, 0.5.3)

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
`[identity] collapse surface=<s> ..` fires on stderr once per collapsing answer. Since the 0.5.2
tweaks the classes include client <-> server models (`http_model`, DT.1h) and GraphQL result /
generated models with their schema type (`graphql_model`, DT.1k), both DERIVED and walking; `why`
reads a SAME_AS edge of either emitter as an identity edge.

Ranking (0.5.3, EA.5-EA.8, EB.7, EB.8, EC.8-EC.13). The text-free ranked answers (`blast-radius`,
`diff-impact`, `review`'s impact, `pack`) rank every candidate file from the seeds' distinct files
(`rank::rank_files`) and reorder their rows by it (`rank::order_rows`, before the identity fold,
scope and top_k; pack lifts each co-changed file's best row only, EA.6b, ruling EA-2b): with a
history snapshot under the holder's repo roots, co-change first (H10), else the 0.5.2 order byte
for byte. Blast-radius answers gain `files` (the `--top-files` / `top_files` first: rank, file,
repo, cochange, ppr, depth, rows; seed files never), every ranked answer `selection` {set, shape,
pick, source calibrated | fallback | explicit, reason, test_commits}. The set is SELh0 (H10,
H10[PT*], H10[B0]) when co-change ranks first, else SELs0 (P1, PT*, B0); the shape is the seeds'
distinct files (k1 one, k2 two, k3 three, half more); the repo's calibration record picks within
the set (one repo only: a merge reads `multi_repo`), and an explicit `--ranking` / `ranking=` is
`source=explicit`. pack never reads the record (ruling EC-9b, EC.9b: the impact replay's pick
lowered pack's recall@10 on every gate snapshot): it ranks by `PackArgs.ranking` alone, its
selection `fallback` with reason `pack_uncalibrated` for the default H10, else `explicit`. Text answers: `review` with a title (`--title`, else the newest non-merge
subject of `<base>..HEAD`, redacted) gains `intent` {title, title_source, fusion, ids, files,
text} and `selection_intent` (SELt2, the fallback LX[IDR,P1]); IDR is routed at the BASE tree,
BM25 excludes the PR's own commits, and impact rows in id-routed files move first (text never
moves a row). `find --files` / `PyGraph.find_files` rank one repo's files for a free-text query
(IDR over the working tree, BM25 with nothing excluded; SELf2t, fallback LX[IDR,T1s]); its
empty answer is a FACT `no_match` absence naming the missing inputs, exit 0, and the `Answer`
carries `selection` (serialized for this answer only). Not ranked by this: `find`'s node answer,
`trace`, `tests-for`, `hotspots` and every other answer.

CLI: every subcommand and flag is in README.md `## CLI`, rendered from `cli/surface/`
(regenerate: `GLIA_UPDATE_SURFACE=1 cargo test --manifest-path cli/Cargo.toml cli_surface`). Most take
`--with <repo>` (repeatable, merge first) and `--json`; `--scope` takes a path or a
`glia projects` label. By `cli/src/cmd/` area:
- pre-0.5.0 (`cmd/<command>.rs`): `analyze`, `arch` (A9.2 `service_map`; a single
  repo keys services by top-level directory; `SHARES_*`, `SAME_AS` and `DOCUMENTS` only with
  `--include-shared`; a SAME_AS class is one link per (service pair, class) at its weakest
  edge and other links count class-mapped endpoint pairs once, `--no-collapse` the raw star,
  DD.14), `projects`, `contracts` (`--fields [--breaking-only]`, LE.10d; the 0.5.2 tweaks add
  three `fields` pairings: `http_model`, each client call site's model against the server model
  of its HTTP exchange, response and request (DT.1i, the wire compare of DT.1m: rules
  json_field_missing, json_field_optional_absent, json_field_unread, json_field_may_be_absent,
  json_field_dropped, json_required_missing, json_field_unset, json_type_mismatch and, DT.1n,
  json_required_may_be_unset); `client_contract`, a client against the OpenAPI op that
  DOCUMENTS its route (contract_field_undeclared, contract_field_optional_undeclared,
  contract_field_unread, contract_request_field_undeclared, contract_required_missing,
  contract_field_unset, contract_type_mismatch); `graphql_selection`, each GRAPHQL_CALLS op's
  selection against the SDL type its root field returns (graphql_field_undefined, DT.1n). Notes
  `open` and `union` ride on a verdict; a row that compared nothing is `unknown` with
  `ambiguous_server`, `client_type_unresolved` / `server_type_unresolved` / `op_type_unresolved`,
  or a GraphQL refusal reason; `[contract-fields] .. http_model=<n> client_contract=<n>
  graphql_selection=<n>`),
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
  heuristic=<h> diverged=<x>`; `--rule` takes the ten `same_as::ALL_RULES` names and `by_rule`
  lists them in that order, `http_model` and `graphql_model` last, DT.1h / DT.1k). A `--gql` row
  that returns properties only is located at the first property's node (DT.8b). 0.5.1:
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
  change, plus the clients left with no provider; exits 1 on either). The 0.5.2 tweaks put the
  inferred contract models in both PR gates (DT.1o, DT.1p): every http_model / graphql_selection
  exchange judged rev -> working tree (`contract_breaks::model_breaks`), one row per client <->
  server model pair with located field lines (rules json_name_changed / request_json_name_changed,
  response_field_removed / _added / _type_changed / _now_optional, request_field_removed /
  _added, new_required_request_field, request_type_changed, schema_field_removed /
  _type_changed / _now_nullable, and the client-side wire rules; notes `open`, `new_pairing`),
  tiered by why's reading of the joining edges and blocking only at or above
  `--models-min-tier fact|derived` (default `fact`, so the resolver-paired, derived rows of
  0.5.2 are advisory). contract-breaks gains `models` and `advisory` (`breaking` adds only the
  blocking model rows) and a `## models` section; review gains `model_breaks`,
  `counts.model_breaks`, `### Model breaks (blocking)` / `(advisory: ..)` tables and blocks on a
  blocking model row; review also checks rules through check's gate (an empty-scope rule goes to
  `unanchored`, `### Rules that check nothing`, never blocking) and prints a rule's caveats once
  per section (`[review] caveats rules= lines= folded=`); `[model-breaks] ..` on both. `cochange
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
  derived; a heuristic violation is advisory under any setting and never exits 1, Q2); a declared
  rule that checks nothing is listed under 'Rules that check nothing' with a reason and a fix,
  non-blocking unless `--strict` (exit 2) (DT.7b, DT.7c; `[check] declared not checked: ..`,
  `[check] overlay rejected root=<r> whole_file=<b> constraint=<c> component=<m> layer=<l>`).
  Such a rule is one of: a stanza the build anchored nowhere (orphaned); one the binder or the
  overlay loader rejected (a version-2 key in a version-1 file, a path two components claim, a
  layer / allow naming a dropped component; a TOML / schema / version error ignores the whole
  file and is one `overlay` row); one the graph does not hold (absent); a rule whose scope no
  located node sits in (empty_scope). A sub-project `[[constraint]]` with no `anchor` anchors on
  the deepest PROJECT enclosing its scope, else the smallest-id MODULE under it (DT.7a,
  `[declared] .. anchor_enclosing=`). The markdown prints a rule's coverage caveats once, under
  its heading (DT.8a, `[check] caveats rules=<r> lines=<l> folded=<f>`; --json keeps them per
  violation). Exits 0 when nothing blocks, 1 on a blocking violation, 2 on a build error or a
  rule it cannot evaluate (and, under `--strict`, on a rule that checks nothing);
  `[check] gate blocking=<b> advisory=<a>`, DB.3d), `spec-status` (LE.9b; a row's
  `pairing` is `exact`, `server_base` (another server base the OpenAPI document lists),
  `annotated_base` (the project's swaggo `@BasePath` for a base-less document, at most medium)
  or `prefix`, DF.19). 0.5.1: `flags [--status dead|undefined|single_site|quiet]` (CC.7c; the
  stale feature-flag report, exits 0).
- store: `inspect <path>` (LC.4), `cell set|rm|ls` (LF.1c; `cell set --hint <v1>` passes a
  caller's LB.6 identity hint, the form engram-export's `<out>.hints.jsonl` lists: a non-v1,
  unregistered-kind or kind-contradicting hint exits 2 and writes nothing, and a fresh layout
  whose binding differs from it reports `target=hint_differs` (applied, flagged) with a `note:`
  line and the marker's ` differs=<fields>`, DT.12b). 0.5.1: `cache push|pull|gc`
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
  suggest `remove`, DB.4; the 0.5.2 tweaks: `unpaired_route` withholds a ROUTE a WebSocket
  client connects through, DT.3c, `glia_graph::WebSocketStackResolver::served_routes`, `[gaps]
  ws-served routes=<n>`, and a ROUTE only test code registers, ORIGIN `test_fixture`, DT.6b,
  `[gaps] unpaired_route rows withheld: <n> (test-file routes)`, which unpaired_endpoint's
  `suffix_of` / `route_prefix|edge` candidates and `suspected_edge` skip too; an unpaired
  `"via":"graphql"` transport whose project's operations pair a resolver on a server holding
  no route at its last segment is no unpaired_endpoint, and any other unpaired transport's
  detail starts `graphql transport; ` (with `route_at_server=<qname> (<repo>)` when the server
  holds such a route), DT.11c, `[gaps] graphql transports implied=<n> unpaired=<m>`;
  `dead_symbol` withholds a used owner's framework- and runtime-invoked members and
  constructors (`INVOKED_MEMBERS`: liveness takes a walk-local owner -> member hop) and every
  ORIGIN `generated` / `generated_proto` node, DT.5d, `[dead-framework] dead_symbol rows
  withheld: invoked=<i> generated=<g>`, the `[live]` line gaining `invoked=`; the close-out:
  Dart member reads, tear-offs and names in any value position, Riverpod provider receivers,
  Go range-element / qualified-field / func-value / composite-literal receivers and
  `package_function` handlers, TS signal-query / inline-inject / call-result / accessor receivers
  and static reads are edges (DT.5h-DT.5k), and DT.5l's entries (Go `init`, `__main__` script
  MODULEs, `@main` types, Flutter activities) and gated INVOKED_MEMBERS rows are live, `[invoked]
  ..`; the re-demo's audit of Kina's 52 and quokka-stack's 124 rows found 103 false: at the close-out
  (`1212a3be`) Kina reads 32 and quokka-stack 41, exactly the 73 rows it judged truly dead), `history sync`
  (LF.5d), `tests ingest`
  (LF.6d; a rolling window of runs, `--window`, `--reset`, CC.9b). 0.5.1: `overlay
  propose|try|accept` (CE.3e; the overlay loop, `skills/glia-overlay/SKILL.md`; 0.5.2,
  DB.9a-c, Q9: a candidate may also hold `[[constraint]]` rules, handles `constraint#<n>`;
  try judges a rule by the check delta on the build with every stanza and demotes a graph
  stanza that adds or hides a declared rule's violation to review; a rule stanza's try status
  gains `checks_nothing` (its from / to / via / scope holds no node: verdict review, `reason` /
  `fix`, check's own reading since DT.7b, through check::check_rules_gated, DT.7c); accept
  refuses only a rule that does not parse or defines no `violation`, and moves the file to
  `version = 2` in place only when a written rule needs it; `[overlay] try-rules ..` (gaining
  ` checks_nothing=<n>` when one does); `--remove` takes orphaned_rule / redundant_rule /
  rotted_rule ids), `scip import <repo> <index.scip>` (CE.1c; writes `.glia/scip-snapshot/`, which the
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
- 0.5.3 (ranking): `blast-radius`, `diff-impact` and `review` take `--ranking
  ppr|ppr-wide|bfs` (no default: absent = the calibrated pick, else ppr) and `--no-cochange`
  (common::CalibratedRankingArgs); `pack` takes the same two flags as its own `PackRankingArgs`
  (`--ranking` default ppr; pack never reads the calibration record, ruling EC-9b, EC.9b); `blast-radius` / `diff-impact` `--top-files <N>` (10) with a
  `## files` table after the rows; each table gains one `ranking: <pick> (<source>[: <reason>],
  shape <k>[, <t> commits])` line, and each command holds one RankCache (holder `cli`) built from
  the build's repo roots and labels, `--with` repos included (EA.8, EC.8, EC.9). `review --title
  <TEXT>` adds `### Intent` after the impact table with a `_ranked by <pick> (..)_` line (EB.7,
  EC.11, EC.12). `find --files` (EB.8, EC.13; conflicts with --kind / --no-collapse / --scope,
  `--with` exits 2): the routed-id table, then `| # | file | idr | text | ids |`, ending `ranked by
  <pick> (..)`. inputs: `history sync --no-messages` (EB.1); `history calibrate <repo>
  [--max-commits <n>] [--max-seeds <n>] [--sets <group,..>] [--json]` (EC.6; groups structure,
  review, find, every one by default; one line per (set, shape), then `wrote <dir>`; exits 0
  written, 1 on no history snapshot or an unknown group (checked before the build), 2 on a
  build error).

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
every category, DEFINES included; LD.3c; `categories` takes ids or registry names, mixed,
case-insensitive, `["CALLS", "imports", 9]`, an unknown name a `ValueError` naming it, DT.9;
rows still carry ids) — `set_cell` / `remove_cell` (+ `activate`,
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
dicts `rule` / `rules`; `overlay_accept` raises `ValueError` on a rule that does not parse or
defines no `violation`). The 0.5.2 tweaks: `PyGraph.check(strict=False)` reads each repo root's
overlay and gains `unanchored` (only when a row exists; an empty-scope rule leaves `errors` for
it unless `strict=True`, DT.7b, DT.7c); `contract_breaks_vs_rev(.., models_min_tier="fact")` and
`review_vs_rev(.., models_min_tier="fact")` (the dicts gain `models` / `advisory` and
`model_breaks` / `unanchored`, a bad tier a `ValueError` before any build, DT.1p);
`PyGraph.set_cell(.., dims=None, hint=None)` / `write_cell(.., dims=None, hint=None)` with the
outcome's `node_hint` / `hint_differs` (DT.12b); `contract_fields` returns the `http_model`,
`client_contract` and `graphql_selection` rows (DT.1i, DT.1n); `query_gql` property rows carry
`file` / `line` (DT.8b); `overlay_try`'s rule status may read `checks_nothing` with `reason` /
`fix` (DT.7c). 0.5.3: `blast_radius(.., collapse=True, ranking=None, cochange=True,
top_files=10)`, `diff_impact` / `diff_impact_vs_rev(.., scope=None, ranking=None, cochange=True,
top_files=10)`, `pack` / `pack_ids(.., scope=None, ranking=None, cochange=True)` and
`review_vs_rev(.., models_min_tier="fact", ranking=None, cochange=True, title=None)` (EA.7, EC.8,
EC.9, EB.7; `ranking` None = calibrated, else ppr, a bad name a `ValueError` before any work);
their dicts gain `files` / `selection` (and review's `intent` / `selection_intent`);
`PyGraph.find_files(query, top_k=20)` (EB.8); `history_sync(.., messages=True)` (EB.1) and
`history_calibrate(repo_path, max_commits=300, max_seeds=5, sets=None)` (EC.6, the GIL released).
A PyGraph holds one `Arc<RankCache>` (holder `pygraph`) for its lifetime and reads
`.glia/history-snapshot/` / `.glia/calibration-snapshot/` under its repo roots; a WatchSession's
`snapshot()` shares the session's.
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
`glia_graph::{blast_radius_over, cross_links_with}` (DD.10, DD.14). The 0.5.2 tweaks:
`check::{check_with_roots, CheckOptions, Unanchored, ORPHANED, REJECTED, ABSENT, EMPTY_SCOPE}`
and `CheckReport.unanchored` (DT.7b), `contract_breaks::{model_breaks, ModelBreak, ModelBreaks,
ModelBreakArgs, ModelChange, ModelExchange, ModelSite, MODEL_MIN_TIERS}` (DT.1o) with
`ContractBreakArgs` / `ReviewArgs.models_min_tier`, `ContractBreaks.{models, advisory}`,
`Review.{model_breaks, unanchored}` and `review::{caveat_fold, CaveatFold}` (DT.1p),
`overlay_loop::RULE_CHECKS_NOTHING` and `RuleTrial.{reason, fix}` (DT.7c), and the graph
facade's contract-model items (Module layout above).
0.5.3, by module path: `rank::{RankCache, SourceKey, FileIndex, CochangeIndex, c0, rank_files,
rank_files_with_c0, base_keys, order_rows, FileRanking, Ranking, RankBase, WIDE_DAMPING}`;
`intent::{text::TextIndex, ids::IdVocab, defs::read_defs, paths::PathResolver, idr::route}`;
`calibrate::{calibrate, calibrate_snapshot, CalibrateOptions, select, explicit, Selection,
load_for_roots, specs, run_sets}`; `find::{find_files, find_files_with_ids, FindFilesOptions,
FoundFile, FIND_FILES_FUSION}`; `review::{ReviewIntent, IntentFile, INTENT_FUSION_REVIEW}` with
`ReviewArgs.title`, `Review.{intent, selection_intent}`; `BlastOptions.{ranking, rank_cache,
top_files, select}`, `BlastRadius.{files, selection}`, `PackArgs.{ranking, rank_cache, select}`,
`Pack.selection`, `Answer.selection`, `watch::WatchSession::rank_cache()`; on the graph facade
`blast_radius_over_with` (a prebuilt PprIndex) and `closure_config`; the #[doc(hidden)] seams
`review::intent_files_with` / `IntentHistory` and `find::find_files_with` that rank_eval's gate
calls with leak-free history (EG.7).
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
- **0.5.2 — the four bets + the tweaks: released 2026-10-18 (tag `v0.5.2`, `glia-py` 0.5.2 on
  PyPI).** James, 2026-10-18: *"maybe we just do the 0.5.2 and slowly work on 0.5.3 over the next
  week"*, then "I prep, you push" and "repo-graph upgrades to 0.5.2 now": 0.5.2 is released
  2026-10-18 as tag `v0.5.2` and `glia-py` 0.5.2 on PyPI (his 2026-10-17 ruling, *"can't we just
  go 0.5.1-> 0.5.3 release"*, is reversed), and 0.5.3 is paced over the week as a normal 0.5.2 ->
  0.5.3 release. The bump was done: the Cargo
  workspace, `py/pyproject.toml` and the excluded engram-export read 0.5.2, so every crate can be
  pinned `=0.5.2`. The code is the code at `196728e3`, where Engram ran its registered replays
  (results in the 0.5.4 seed §E): only plan commits and the bump follow it. Scope in
  `dev-notes/next-leap-0.5.2.md` (§0.1 the binding decisions Q1–Q25, §4.2 the defaults the
  packets took), research `dev-notes/research-0.5.2/`, packets in
  `dev-notes/leap-052-packets.json` (153 packets in W0–W14, the gap run's DB.9a-c and DF.22a-c
  among them, then the 57 tweak packets DT.1a–DT.13 in W15–W26, then the close-out's
  dead_symbol packets DT.5h–DT.5l in W27–W30 and DT.14 in W31), overrides in
  `dev-notes/leap-052-corrections.json`, schedule and LANDED waves in
  `dev-notes/wave-runner/schedule_052.py` (32 waves, W0–W31, runner `--release 052`). It may
  break contracts, each break declared by its packet; the handoffs are
  `dev-notes/{repo-graph,neuropil,engram}-handoff-0.5.2.md` (DZ.2, then DT.13 for the tweaks and
  DT.14 for the close-out), each with the exact pin (`glia-py==0.5.2`, `=0.5.2`) and every
  declared break; repo-graph's is active, no longer held (James 2026-10-18). The tweaks
  (`dev-notes/tweaks-0.5.2.md`, the gaps the Kina and quokka-stack demos showed): T1 contract
  models (DT.1a-DT.1q: SCHEMA_FIELDS on TS / Go / Dart / GraphQL models, client wire shapes, Go
  server wire cells, the `models` pass with http_model / graphql_model identity, contract_fields
  drift rows, inferred-model breaks in contract-breaks / review, TS directory imports and
  re-export barrels); T2 NATS queues (DT.2a-c); T3 WebSockets inside HTTP routes (DT.3a-c); T4
  query-string suffixes (DT.4); T5 dead_symbol precision in UI code (DT.5a-g: Angular templates,
  framework-invoked members, Flutter widget trees, gqlgen, TS value references); T6 test-file
  routes (DT.6a-b); T7 sub-project constraints and rules that check nothing (DT.7a-c); T8 check
  caveats once and located GQL rows (DT.8a-b); T9 traversal category names (DT.9); T10 smart
  contracts across the stack (DT.10a-k: Solidity procedures, resolver:evm, ethers / hardhat /
  TypeChain / viem / wagmi / web3.js and go-ethereum clients); T11 GraphQL root fields,
  transports and implied transports (DT.11a-c, DT.11z); T12 one hint format with Engram
  (DT.12a-b); DT.13 these docs. The close-out (tweaks ruling 7, James 2026-10-17: *"lets fix the
  false positives that counts as closeout i think"*): the re-demo's remaining dead_symbol rows on
  Kina (52) and quokka-stack (124) were audited, 103 false and 73 truly dead, and each false
  mechanism became a packet: DT.5h Dart names and member reads, DT.5i Riverpod provider
  receivers and the Dio interceptor ROLE, DT.5j Go receiver typing and func / method /
  composite-literal values, DT.5k TS signal-query / inline-inject / call-result / accessor
  receivers, DT.5l runtime-invoked entries and members; at `1212a3be` Kina reads 32 rows and
  quokka-stack 41, the audit's truly dead set exactly; DT.14 these docs' second pass. The
  version bump is the release step after the docs, never inside a packet. Groups: D0 the
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
  DZ these docs and the handoffs. The matrix stands at 346 full, 0 partial (DT.5e closed
  dart/tests), 0 none, 136 unknown, 28 n/a, over 419 graded fixtures. No FORMAT_VERSION change
  and no registry id beyond SAME_AS.
- **0.5.3 — ranking: complete on local `main`, version bumped, awaiting the tag.** James, 2026-10-18:
  the ranking experiments' table becomes 0.5.3, *"it's a small lay up step and has the
  biggest generalised wins"*; items 6-11 and the hygiene confirmed 2026-10-23 (*"yes to all"*). Scope in
  `dev-notes/next-leap-0.5.3.md` (items 1-11, the gate, the hygiene), evidence
  `dev-notes/ranking-scoreboard.md` and the `dev-notes/ranking-eval-*.md` round reports, all from
  `engine/examples/rank_eval.rs`; packets in `dev-notes/leap-053-packets.json` (its `rulings`:
  James's 23), overrides in `dev-notes/leap-053-corrections.json`, schedule and LANDED waves in
  `dev-notes/wave-runner/schedule_053.py` (runner `--release 053`, ids E*, waves W0-W17). Groups:
  E0 the module slots (`algo::fuse`; the engine's `rank`, `intent`, `calibrate`); EA structural
  ranking (`PprIndex` and its `RankCache`, the FileIndex candidate space, co-change first from the
  history snapshot, PPR d0.85 + sum pooling and BFS as candidates, in blast-radius / diff-impact /
  review / pack, `files`, the CLI and pyo3 kwargs); EB intent (messages in the history snapshot,
  BM25, the `[ids]` vocabulary, id definitions / namespaces / paths, IDR, `review --title`, `find
  --files`); EC per-repo calibrated selection (the `.glia/calibration-snapshot/` record, the
  leak-free online replay, `glia history calibrate`, SELh0 / SELs0 / SELt2 / SELf2t, `selection` on
  every ranked answer, `--ranking` absent = calibrated, except pack: ruling EC-9b, EC.9b in W17,
  pack ignores the record); EG the rank gate (rank_eval's ANS rankers
  over the shipped answers, `--gate-check` with Holm, `bench/rank-gate/` and `scripts/rank-gate.sh`
  on six pinned public snapshots, against 0.5.2 and current, plus EC.9b's calibrated-vs-default
  contrasts, gate.json `contrasts`, `[rank-gate] contrasts: ok - <n> pairs, 0 below default`;
  the close-out check reads `[rank-gate]
  check: ok - 1176 rows, 47 gains, 0 drops on 6 snapshots` and `coverage: ok - 9 items measured, 2
  exempt`), the DE.5 side-file removal (EG.4) and the wheels workflow on Node 24 actions pinned by
  commit with pinned runner images (EG.5; macos-14 is deprecated upstream, a later move to macos-15
  is deliberate); EZ these docs and the handoffs (`dev-notes/{repo-graph,neuropil,engram}-handoff-0.5.3.md`;
  neuropil skipped 0.5.2, so its handoff runs 0.5.1 -> 0.5.3). No registry id, no
  FORMAT_VERSION / MANIFEST_VERSION change, no crate added and the workspace Cargo.lock unmoved;
  the on-disk changes are the history snapshot's version 2 (`messages.jsonl`) and the calibration
  record. A calibrated answer depends on the glia build that calibrated (BUILD_STAMP strict), so
  every repo recalibrates after the upgrade. The version bump is the release step after EZ, never
  inside a packet.
- **After 0.5.3:** **0.5.4** scale (seed `dev-notes/next-leap-0.5.4.md`, the old 0.5.3 seed) —
  Stack Graphs-style incremental resolution with watch's A2 body-only fast path,
  build-target graphs (the likely first real non-code domain), multi-repo `glia watch --with`,
  org-internal package routing on SAME_AS; plus James's 2026-10-16 additions there (local
  embeddings and `glia embed`, a clarification queue, one tier vocabulary, a watch / rules
  memory budget, concept search over flows, the self-graph hygiene cluster), the goal-layer connector
  (§G) and the 0.5.2 waves' leftovers (§D, §E). **0.5.5** proof — precision rate on real repos, agent
  before/after, big-repo per-phase timings, calibrated confidence, `decide/` and `glia judge`
  on our own model. Still later from the 0.5.0 list: dominators (once middleware is extracted
  and the security gate is ruled on), RuntimeZone, LSP, per-graph-area rebuilds. The §6
  "Gated" items stay out (`SECURITY.md`).

## Memory

Relevant architecture/spec memories from the repo-graph project memory system are copied under `dev-notes/glia-memory/`. They seeded this repo's Claude memory directory when the split landed. **Files under `dev-notes/glia-memory/` are point-in-time snapshots and are not updated in place** — several still cite the pre-split `rust/` layout and pre-wave-0 `lib.rs` line numbers. Read them as history; this file is the current architecture.
