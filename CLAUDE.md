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
                    src/ is MODULES, not one lib.rs (see "Module layout" below)
engine/             Orchestration: walk → parse → extract → build → merge → resolve.
                    Owns every parser dependency. src/ is MODULES; lib.rs is a facade
cli/                The `glia` binary
doc-sources/        Tier-4 doc ingestion (Confluence REST + local snapshot)
snapshots/          External-input snapshot writers (git history, test reports) - the only
                    crates that shell out; the build reads their output through
                    code-domain::snapshots
store/              .gmap binary format — rkyv + mmap, sharded layout
projection-text/    Dense sigil text output (scopes, defaults, module dedup)
activation/         Spreading activation — domain-agnostic PPR with configurable direction/weights;
                    also the domain-free build-pass registry (`passes`: Stage, PassSpec,
                    PassRegistry) — build-time, so PARSER_STAMP hashes it — and `algo`:
                    GraphSource, CategorySet, the CSR Adjacency index, `algo::reach`
                    (bfs / reachable / reachable_by, O(V+E) per walk), `algo::delta`
                    (graph delta), `algo::cycles` (Tarjan SCC), and the 0.5.1 slots
                    `algo::community` (label propagation, seeded Leiden, CD.1a/b),
                    `algo::cut` (Stoer-Wagner / Dinic, CD.2a), `algo::linkpred`
                    (CD.3a), `algo::hubs` (degree / HITS, CD.4a), `algo::minhash`
                    (MinHash + LSH, CD.4d), `algo::timeline` (validity intervals,
                    CD.5a); `plan` (one
                    ActivationPlan over the RankingSignal / FilterPredicate / SynthHook
                    hooks) and `profile` (DomainTables / DomainProfile)
parsers/code/
  python/  go/  typescript/  rust/  java/  csharp/  ruby/  php/  swift/
  c_cpp/   scala/  clojure/  dart/  elixir/  solidity/  terraform/
  kotlin/  — own parser (tree-sitter-kotlin-ng), but ONE graph with java/:
             the JVM family joins in engine/src/build/lang_build.rs
  react/   angular/  vue/    — framework parsers stacked on typescript
  extractors/ — cross-cutting: data_sources, cli, grpc, queues, websocket,
                eventbus, graphql, ts_routes, angular/react/vue route extractors
stamp/              Build identity: RELEASE + PARSER_STAMP (content hash of every
                    graph-shaping source). Keys the parse cache, so a parser fix
                    invalidates caches without a version bump.
py/                 pyo3 bindings — the only Rust crate published to PyPI (as glia-py, module glia_py)
toy-domain/         test-only second domain (publish = false) proving the domain seam; never a dependency of a shipped crate
engram-export/      (excluded) glia -> engram_core::Gmap exporter; needs ../Engram; build/test only via scripts/check-engram-export.sh
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
2. **Every new 0.5.0 primitive is a public module slot**, reached by module path —
   `glia_engine::delta::graph_delta_vs_rev`, `::persist::load_layout`,
   `::profile::CODE_PROFILE`, `glia_graph::roles::roles_in` — never flattened into
   the root. The slot's owner fills its file; no later packet edits a facade (only
   LD.11a, the crate rename, touches `engine/src/lib.rs`).
3. **A spec step "FACADE: `pub use x::{..}`" or "`mod x;`" is already done** — skip it
   and call the item by its module path from `cli` / `py`.
4. Two globbed modules exporting one name raise `ambiguous_glob_reexports`: rename,
   never `#[allow]`. A slot still doc-only at release is dead weight — LG.4b's docs
   refresh fails on any slot file whose only lines are `//!`.

```
engine/src/   lib.rs        facade (rules above)
              walk.rs       repo walk, gitignore, region graph
              route.rs      per-file routing (yaml/Dockerfile/manifest/dotenv/.proto), parse cache;
                            ModuleQnames::plan names each file's MODULE (same-stem code files,
                            LB.9b / LB.13, and every C/C++ file, LB.10a, are named by file
                            name; graph/src/imports.rs `SameStem` picks the importer's
                            sibling, `[imports] same-stem picks`)
              extract.rs    detect_language, parse_one, parse_one_with, cross-cutting extractors
              build/        mod.rs         GenerateResult, generate_one* / generate_many* pipeline;
                                           assemble_many (generate_many before its passes run)
                            assemble.rs    build_graphs_for_repo, build_const_table (panics go
                                           quiet through parallel::quiet; SuppressPanicHook is gone)
                            grafts.rs      apply_post_cache — every post-cache graft goes here
                            rpc_needles.rs RpcContext, apply_rpc_needles, graft_rpc_markers
                            lang_build.rs  build_language_graphs — per-language build + markers;
                                           rust_crates (the Cargo packages build_rust resolves against)
              docs.rs       markdown ingest          passes.rs    doc-linker, TESTS edge, the post-pass fns
              profile.rs    CODE_PASSES — every build pass in run order (15 resolvers, 6 post-passes,
                            evidence fill + determinism sort); run_code_passes is the whole build tail
              coverage.rs   coverage_report          answers.rs   the P3 primitives
              cache.rs      incremental parse cache  arch.rs      service_map (glia arch)
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
              overlay_loop/ CE.3b (+CE.3c, CE.3d) — directory module; writer.rs, trial.rs,
                        propose.rs, accept.rs, each declared in its mod.rs
  private slots (items pub(crate)):
              http_owner LB.4a         rekey LB.4a (LC.2 edits)   git_rev LE.1b
              adr LF.4b                parallel LG.1a (+LG.1b, LG.1c)
              suspected CD.3b (0.5.1, C0.2)
              external/ LF.1a — directory module; LF.2b, LF.2e, LF.3b, LF.4a, LF.5b, LF.6b
                        add their stage files and declare them in external/mod.rs

graph/src/    lib.rs        facade (rules above)
              types.rs build.rs imports.rs calls.rs merged.rs traversal.rs
              blast.rs signal.rs
              resolvers/    one module per mechanism (http, grpc, queue, graphql,
                            websocket, eventbus, shared_schema, db, cron, config,
                            iac, package, cli) + mod.rs
  public slots (glia_graph::<slot>::<item>):
              rust_paths LA.1a (+LA.1b, LA.3)   roles LB.3a (+LA.21a)
              identity LB.6                     cells LF.1a
              nav LA.6a   is_nav_route / nav_route_path, and the NAVIGATES_TO resolver
                          + page-component lift that calls::resolve_refs runs last
  private slots (items pub(crate)):
              go_mounts CB.20   swift_scope CB.18   cpp_scope CB.25

code-domain/src/  lib.rs    the id registries (node_kind, edge_category, cell_type) — ids
                            are allocated here and nowhere else; walk_gating, project_roots
  public slots:     data_entity A13.1   evidence LC.3a   external_inputs LF.1a (+LF.4a)
                    glia_config LF.2a (+LG.3d)   profile LD.14a (+LD.14b, LD.6, LE.4d)
                    snapshots LF.5a (+LF.6a)   code_span CD.7c

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
              timeline CD.5d

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
                          duplicate-flows CD.4f
                  change  delta LE.1c, diff-impact LE.2, tests-for LE.3b, patterns LE.7b;
                          0.5.1: review CC.6b, contract-breaks CC.8b (+CC.8c),
                          cochange CC.11c, timeline CD.5d
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
  (C0.5) are declared as doc-only slots (`mod <name>;` plus one `//!` paragraph, no
  items); the owner adds `Args`, `run`, its variant `<Name>(<module>::Args)` (the doc
  comment is clap's about) and its arm, and regenerates `cli/surface/<cmd>.txt`.
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
     Queue, GraphQL, WebSocket, EventBus, SharedSchema, MessageSchema, CLI, DB, Cron, Config,
     IaC, Package), then the post-passes, evidence fill and determinism sort
   → merged graph
   → .gmap layout at <repo>/.glia/graph/ (rkyv + mmap, sharded)
   → [optional] activation (PPR) / projection-text / pyo3 → Python
```

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
`GLIA_THREADS=1` keeps them in sequential order.
Per-phase timers (CA.9, `engine/src/build/timing.rs`; wall time on the orchestrating thread,
milliseconds truncated to one decimal, stderr only — never a cache, `.gmap`, manifest, stdout
or answer): `[timing] repo=<label> walk=<ms> parse=<ms> const_scan=<ms> grafts=<ms> language_build=<ms>`
once per built repo, `[timing] build repos=<n> resolve=<ms> post=<ms> finalize=<ms> external_cells=<ms> total=<ms> slowest_pass=<name>:<ms>`
once per build (a layout merge prints no `external_cells=`; the stage and per-pass times are
`PassReport::elapsed` / `pass_elapsed`), and `[timing] persist writer=<w> <ms> dir=<dir>` once
per layout write.

## Parser-vs-Graph Split (locked at v0.4.3b)

Parsers **extract**; graph crate **resolves**. Parsers emit raw `ExtractedItems` with unresolved references (`UnresolvedRef`). The graph builder walks the tree to turn those into concrete edges uniformly across languages.

- `SelfMethod` walks to the enclosing `CLASS` / `STRUCT` / equivalent.
- An `extra_hook` seam on `resolve_calls` lets a language contribute resolution the generic
  walker misses; it is consulted only after every generic lookup failed. Three builders use it:
  `build_rust` hands it the path resolver in `graph/src/rust_paths.rs` (`crate::` / `self::` /
  `super::` / `Self::` / workspace-crate paths), fed the walk's Cargo packages as `RustCrate`s by
  `lang_build::rust_crates` (LA.1a); `build_go` hands it `GoPackages` (a package is its
  directory, LA.13b); `build_c_cpp` hands it `CppCallScope` (out-of-line members and directly
  `#include`d headers, LB.10c).
- Parsers must not short-circuit this: extract what the AST makes available; don't cap at what old regex heuristics happened to capture.

## Format Spec — `.gmap`

Zero-copy rkyv serialisation with memory-mapped read. Sharded by kind to keep hot paths local. Write-once, rebuild-whole-file — no in-place mutation. Owned vs Archived types are the mental model: loaded views are `Archived<T>`, writes go through `Owned<T>` then serialise.

Lives at `<repo>/.glia/graph/` (manifest + shards + cross_stack + parse cache), the one layout
`glia build`, the install-hooks hooks and pyo3 all write (LC.9). `FORMAT_VERSION` 2 and
`MANIFEST_VERSION` 2 (LC.1): every file opens with a `GLIAGMAP` preamble, so an old, future or
foreign file reports OldFormat / FutureFormat / Corrupt ("rebuild the graph"), and
`load_from_gmap` rebuilds such a layout from its repo root instead of raising (LC.8). The header
names its own node-kind / edge-category / cell-type registries (LC.4, read by `glia inspect`);
the container is a domain-free core plus named domain sections (`code`: nav, symbols,
interface methods; LC.5b, LC.6); edges carry cells (LC.2), every edge one EVIDENCE cell with its
emitter, rule, file and 0-based line (LC.3).

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
`resolvers/http.rs` is the canonical example. Shipped (15): HTTP, gRPC, RPC, Queue, GraphQL,
WebSocket, EventBus, SharedSchema, MessageSchema, CLI, DB, Cron, Config, IaC, Package. See
`dev-notes/glia-memory/project_040_stack_resolvers_backlog.md`.

## Key Design Decisions

- **Tree-sitter, not regex.** 0.4.x moved to AST extraction; 0.2.0 regex is not a ceiling.
- **Zero-copy store.** rkyv + mmap; writes rebuild the whole file.
- **Domain-agnostic core.** `core` and `activation` know nothing about code. Code lives in `code-domain` and the parsers.
- **Generic graph algorithms live in `activation::algo`, over `GraphSource`, never in engine.** Reachability (`algo::reach`, LD.15a), graph delta (`algo::delta`, LE.1a) and Tarjan SCC (`algo::cycles`, LE.6a); a graph type opts in by implementing `GraphSource`, and a walk runs over a per-query CSR `Adjacency`, never a scan of the edge list per visited node.
- **Publish gate.** Only `py/` publishes to PyPI (as `glia-py`, imported as `glia_py`). Everything else is internal workspace.
- **No Python fallback.** After v0.4.10c, Python is a thin pyo3 wrapper; there is no parallel Python implementation to keep in sync.

## Query & Answer Surface (v6 P2/P3, 0.5.0)

Answer-shaped primitives live in the **engine** (shared by CLI + pyo3/MCP + future
TUI), not composed by the consumer. Each is one call: complete, ranked, located.
Every record's `line` is 1-based (an editor's line), located through one
`Locator` per answer; POSITION cells store 0-based rows — `Locator::locate` is
the only place that converts (LD.1). A list answer that can come back empty is
`Answer { results, absence }`: the absence is a FACT-tier reason plus the coverage
caveats of the mechanisms it depended on (LD.8a, `engine::absence`). Rows carry an
evidence tier — FACT (read at a site), DERIVED (paired by a resolver or pass) or
HEURISTIC (a name guess, an overlay, git history) — never an unexplained score.

CLI: every subcommand and flag is in README.md `## CLI`, rendered from `cli/surface/`
(regenerate: `GLIA_UPDATE_SURFACE=1 cargo test --manifest-path cli/Cargo.toml cli_surface`). Most take
`--with <repo>` (repeatable, merge first) and `--json`; `--scope` takes a path or a
`glia projects` label. By `cli/src/cmd/` area:
- pre-0.5.0 (`cmd/<command>.rs`): `analyze`, `arch` (A9.2 `service_map`; a single
  repo keys services by top-level directory; `SHARES_*` / `DOCUMENTS` only with
  `--include-shared`), `projects`, `contracts` (`--fields [--breaking-only]`, LE.10d),
  `impact`, `blast-radius` (many seeds are one walk and one ranking, LD.5), `trace`
  (ranked distinct paths, `--to`, `--max-paths`, LD.4a), `resolve`, `coverage` (+ the
  co-change audit, LF.5c), `docs-for`, `docs sync|push`, `merge` (`--gmap`,
  `--workspace`, `--layout`, LC.10c), `build`, `install-hooks` (`--pair`, LG.2).
- query: `pages` (LA.6e), `find` (LD.3b), `flows` (LD.4b; `--features` / `--out`, LG.3c),
  `implementors` (LD.7c), `serves <repo> <channel> [--mechanism auto|http|queue]`
  (LD.8b), `why <repo> <from> <to>` (LE.5; exits 1 when not found).
- change: `delta [--base <rev>] [--edges-only] [--category <NAME>]...` (LE.1c),
  `diff-impact` (LE.2), `tests-for` (LE.3b), `patterns --experimental` (LE.7b). A git
  or build error exits 2.
- rules: `effects` (LE.4d), `cycles` (LE.6b), `check` (LE.8; exits 0 clean, 1 on
  violations, 2 on an error), `spec-status` (LE.9b).
- store: `inspect <path>` (LC.4), `cell set|rm|ls` (LF.1c).
- inputs: `gaps [--overlay-delta]` (LF.2c; `cochange_no_edge` rows carry no repo, so a
  merge with one relative path in two repos gives two alike rows), `history sync`
  (LF.5d), `tests ingest` (LF.6d). A snapshot step never runs inside a build.
- hidden: `hook pre-commit|commit-msg`, the runners `install-hooks --pair` writes.

pyo3 (`PyGraph`): `blast_radius`, `cross_stack_trace`, `entry_flows`,
`feature_flows` / `write_feature_flows`, `resolve`, `find`, `coverage`,
`governing_docs`, `page_flow`, `service_map`, `contracts`, `contract_fields`,
`implementors`, `serves`, `why`, `diff_impact`, `tests_for` / `tests_for_diff`,
`effects`, `cycles`, `check`, `spec_status`, `gaps`, `patterns_experimental`,
traversal — `neighbours(node_id, direction="out", categories=None)` -> `(id, category,
"out"|"in")`, `bfs`, `predecessors`, `reachable_by`, `shortest_path` (categories=None is
every category, DEFINES included; LD.3c) — `set_cell` / `remove_cell` (+ `activate`,
`node_cells`, `dense_text*`, `nodes_json` / `edges_json`, `save_to*`). Module functions:
`generate` / `generate_many`, `load_from_gmap` (rebuilds a stale or old layout, LC.8),
`is_stale`, `merge_gmaps`, `graph_delta`, `diff_impact_vs_rev`, `tests_for_rev`,
`patterns_vs_rev_experimental`, `overlay_delta`, `history_sync`, `tests_ingest`,
`write_cell` / `remove_cell`, `kind_names` / `category_names` / `cell_type_names` /
`entry_kinds`. An answer is a native dict / list; only `*_json` returns JSON text (LD.2).
The committed surface is `py/api_surface/<module>.txt`.

Engine entry points — flat: `blast_radius`, `resolve_signal_located`,
`coverage_report`, `governing_docs`, `entrypoint_reachable`, `locate_node`,
`service_map` / `service_map_with`; by module path: `trace::{cross_stack_trace,
entry_flows}`, `find::find_nodes`, `pages::page_flow`, `implementors::implementors`,
`serves::serves`, `why::why_edge`, `delta::graph_delta_vs_rev`,
`diff_impact::{diff_impact_vs_rev, diff_impact_from_diff}`,
`tests_for::{tests_for, tests_for_diff, tests_for_rev}`, `effects::effects`,
`cycles::cycles`, `check::check`, `spec_status::spec_status`,
`patterns::{pattern_conformance, pattern_conformance_delta}`,
`gaps::{gaps_report, overlay_delta}`, `contract_fields::contract_fields`,
`feature_flows::{feature_flows, write_feature_flows}`,
`merge::{merge_layouts, read_workspace}`, `persist::{load_layout, load_or_rebuild}`.
A `*_with_live` variant takes a precomputed liveness set, so a caller answering several
questions over one graph computes `entrypoint_reachable` once.

## Roadmap

- **v6 (done, glia-side):** P1 substrate completeness (blind 48→0, eval-gated by
  `bench/substrate-gap`), P2 coverage signaling, P3 answer-shaped primitives
  (above). P4 (collapse ~13 MCP tools → ~4) is repo-graph's job; these primitives
  are its enabler.
- **0.5.0 — complete on local `main`, version bumped, awaiting the tag.** The 2026-09 programme (unreleased
  since v0.4.18; there is no 0.4.19) plus the leap, waves A–G of
  `dev-notes/next-leap-0.5.0.md`, packets in `dev-notes/leap-packets.json`: every id /
  qname / format / API break at once. It is cross-domain *prep* — header registries,
  domain container sections, the domain profile, pass composition, `activation::algo`,
  the test-only `toy-domain/` — and ships no second domain. README.md `## Roadmap` lists
  what landed by packet id. All 41 waves landed (251 packets, 3,185 tests); the
  Cargo workspace and `py/pyproject.toml` are at 0.5.0, so the wheel builds as
  `glia_py-0.5.0-*.whl`. What remains is release mechanics: James walks the commits, a PyPI
  pending trusted publisher for `glia-py`, then push → tag `v0.5.0` → PyPI (the leap doc's
  §1 checklist), and the consumers apply their handoffs.
  The rename is done, table-driven by `dev-notes/rename-0.5.0.py` (`--check` lists anything
  left on the old names): the repo, the `glia` binary (`cli/Cargo.toml`), every library
  crate — packages `glia-*`, Rust paths `glia_*` (LD.11a) — and the Python package: PyPI
  dist `glia-py`, module `glia_py`, wheel `glia_py-<ver>-cp311-abi3-*.whl` (LD.11b,
  `--python`). The repo-graph MCP wrapper still imports the old module until its own
  session moves to `glia-py` (LG.5a), so both wheels stay installed side by side.
- **After 0.5.0:** the leap doc's §6 "Later" list (`.graphqls` routing, struct-held Go
  routers, ws / graphql / grpc client host narrowing, communities, duplicate flows,
  dominators once middleware is extracted and the security gate is ruled on, hubs,
  RuntimeZone, cross-repo node dedupe, Notion / wiki adapters, LSP, per-graph-area
  rebuilds if a big repo is slow after LG.1) and its §7.6 per-language backlog. The
  §6 "Gated" items stay out (`SECURITY.md`).

## Memory

Relevant architecture/spec memories from the repo-graph project memory system are copied under `dev-notes/glia-memory/`. They seeded this repo's Claude memory directory when the split landed. **Files under `dev-notes/glia-memory/` are point-in-time snapshots and are not updated in place** — several still cite the pre-split `rust/` layout and pre-wave-0 `lib.rs` line numbers. Read them as history; this file is the current architecture.
