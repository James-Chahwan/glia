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
                    GraphSource, CategorySet, the CSR Adjacency index and `algo::reach`
                    (bfs / reachable / reachable_by), O(V+E) per walk
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

Parsers live at `parsers/<domain>/<language>/`. When v0.5.0 adds non-code domains, they nest alongside `parsers/code/`.

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
              route.rs      per-file routing (yaml/Dockerfile/manifest/dotenv/.proto), parse cache
              extract.rs    detect_language, parse_one, parse_one_with, cross-cutting extractors
              build/        mod.rs         GenerateResult, generate_one* / generate_many* pipeline;
                                           assemble_many (generate_many before its passes run)
                            assemble.rs    build_graphs_for_repo, build_const_table, SuppressPanicHook
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
  private slots (items pub(crate)):
              http_owner LB.4a         rekey LB.4a (LC.2 edits)   git_rev LE.1b
              adr LF.4b                parallel LG.1a (+LG.1b)
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

code-domain/src/  lib.rs    the id registries (node_kind, edge_category, cell_type) — ids
                            are allocated here and nowhere else; walk_gating, project_roots
  public slots:     data_entity A13.1   evidence LC.3a   external_inputs LF.1a (+LF.4a)
                    glia_config LF.2a (+LG.3d)   profile LD.14a (+LD.14b, LD.6, LE.4d)
                    snapshots LF.5a (+LF.6a)

py/src/       lib.rs        #[pymodule]: every registered ModuleFns sorted by name,
                            then add_class PyGraph — never edited for a new API
              graph.rs      #[pyclass] PyGraph (fields pub(crate)) + its core methods
              registry.rs   the ModuleFns inventory type + registry / version functions
              convert.rs    escape_json (LD.2 adds the JSON -> Python converter)
              build.rs layout.rs      #[pyfunction]s (generate*, load_from_gmap, is_stale ...)
              text.rs records.rs traversal.rs blast.rs trace.rs find.rs docs.rs
              arch.rs contracts.rs    one `#[pymethods] impl PyGraph` block each
  private slots: pages LA.6e   implementors LD.7c   serves LD.8b   delta LE.1c
              diff_impact LE.2   tests_for LE.3b   effects LE.4d   why LE.5   cycles LE.6b
              patterns LE.7b   check LE.8   spec_status LE.9b   cells LF.1b   gaps LF.2c
              snapshots LF.5d, LF.6d   merge LC.10c   feature_flows LG.3c

cli/src/      main.rs       Cli (global options), enum Cmd, main() dispatch
              common.rs     shared helpers (generate_for, print_json, node lookup,
                            ImpactDirection)
              hooks.rs      install-hooks + HooksCmd (LG.2's hidden `hook`)
              surface.rs    #[cfg(test)] CLI surface test (LG.6a)
              cmd/mod.rs    pub(crate) mod lines — never edited again
              cmd/<command>.rs   one per pre-0.5.0 command: clap Args + run(a) -> i32
              cmd/<area>/mod.rs  flattened Subcommand enum per area of leap commands:
                  query   pages LA.6e, find LD.3b, flows LD.4b, implementors LD.7c,
                          serves LD.8b, why LE.5
                  change  delta LE.1c, diff-impact LE.2, tests-for LE.3b, patterns LE.7b
                  rules   effects LE.4d, cycles LE.6b, check LE.8, spec-status LE.9b
                  store   inspect LC.4, cell LF.1c
                  inputs  gaps LF.2c, history LF.5d, tests LF.6d
```

- **py:** pyo3 `multiple-pymethods` lets each module carry its own
  `#[pymethods] impl PyGraph`; a module that owns `#[pyfunction]`s ends with a
  `register()` plus `inventory::submit! { ModuleFns { .. } }`. A new API goes in its
  primitive's module. Helpers that tests exercise stay pyo3-free: the test harness never
  initialises Python (py/src/lib.rs "Link note"). `extension-module` is deliberately not a
  Cargo feature of py/ (maturin turns it on for wheels via pyproject.toml); re-adding it
  there stops the unit tests linking.
- **cli:** a new command adds one variant and one match arm to its area's `mod.rs` and
  its own `cli/src/cmd/<area>/<name>.rs` (`Args` + `run`).
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
   → cross-graph resolvers (HttpStack, GraphQL, gRPC, Queue, WebSocket, EventBus, SharedSchema, DB, CLI)
   → merged graph
   → .gmap (rkyv + mmap, sharded)
   → [optional] activation (PPR) / projection-text / pyo3 → Python
```

## Parser-vs-Graph Split (locked at v0.4.3b)

Parsers **extract**; graph crate **resolves**. Parsers emit raw `ExtractedItems` with unresolved references (`UnresolvedRef`). The graph builder walks the tree to turn those into concrete edges uniformly across languages.

- `SelfMethod` walks to the enclosing `CLASS` / `STRUCT` / equivalent.
- A reserved `extra_hook` seam lets a parser contribute language-specific resolution when the generic walker isn't enough.
  Rust is the first language to use it: `build_rust` hands `resolve_calls` the path resolver in
  `graph/src/rust_paths.rs` (`crate::` / `self::` / `super::` / `Self::` / workspace-crate paths),
  fed the walk's Cargo packages as `RustCrate`s by `lang_build::rust_crates` (LA.1a).
- Parsers must not short-circuit this: extract what the AST makes available; don't cap at what old regex heuristics happened to capture.

## Format Spec — `.gmap`

Zero-copy rkyv serialisation with memory-mapped read. Sharded by kind to keep hot paths local. Write-once, rebuild-whole-file — no in-place mutation. Owned vs Archived types are the mental model: loaded views are `Archived<T>`, writes go through `Owned<T>` then serialise.

Lives at `<repo>/.glia/graph/` (manifest + shards + cross_stack + parse cache).

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
`resolvers/http.rs` is the canonical example. Shipped: HTTP, gRPC, Queue, GraphQL,
WebSocket, EventBus, SharedSchema, DB, Cron, Config, IaC, Package, CLI. See
`dev-notes/glia-memory/project_040_stack_resolvers_backlog.md`.

## Key Design Decisions

- **Tree-sitter, not regex.** 0.4.x moved to AST extraction; 0.2.0 regex is not a ceiling.
- **Zero-copy store.** rkyv + mmap; writes rebuild the whole file.
- **Domain-agnostic core.** `core` and `activation` know nothing about code. Code lives in `code-domain` and the parsers.
- **Generic graph algorithms live in `activation::algo`, over `GraphSource`, never in engine.** Reachability now (LD.15a), graph delta (LE.1) and SCC / cycles (LE.6) next; a graph type opts in by implementing `GraphSource`, and a walk runs over a per-query CSR `Adjacency`, never a scan of the edge list per visited node.
- **Publish gate.** Only `py/` publishes to PyPI (as `glia-py`, imported as `glia_py`). Everything else is internal workspace.
- **No Python fallback.** After v0.4.10c, Python is a thin pyo3 wrapper; there is no parallel Python implementation to keep in sync.

## Query & Answer Surface (v6 P2/P3)

Answer-shaped primitives live in the **engine** (shared by CLI + pyo3/MCP + future
TUI), not composed by the consumer. Each is one call: complete, ranked, located.
Every record's `line` is 1-based (an editor's line), located through one
`Locator` per answer; POSITION cells store 0-based rows — `Locator::locate` is
the only place that converts.

CLI (all accept `--with <repo>` repeatable for cross-service merge; `--json`):
- `glia arch <repo> [--mermaid] [--include-shared]` — the whole-stack view: the
  services present and the cross-service links between them, each with mechanism
  + channel + count (A9.2 `service_map`). A single repo keys services by
  top-level directory, so a monorepo does not collapse to one node. Non-flow
  links (`SHARES_*`, `DOCUMENTS`) are hidden unless `--include-shared`.
  `glia analyze --format mermaid` renders the same service graph.
- `glia blast-radius <repo> <qname>` — edge-category-aware, PPR-ranked, located
  closure with per-node edge-reason + `live` flag (`--live-only`, `--direction`,
  `--depth`, `--top-k`). Excludes structural import/contain edges (no fan-out noise).
- `glia trace <repo> <feature>` — ordered cross-service path with mechanism labels
  (http/queue/grpc/call) + `cross_service` flags.
- `glia resolve <repo> <signal> [--kind stacktrace|test|diff|auto]` — signal →
  ranked located nodes.
- `glia coverage <repo>` — P2 blind-spot signaling: per-language known extraction
  caveats + edges-found, so graph+grep fallback is deliberate.
- `glia docs-for <repo> <qname>` — the DOC_SECTIONs that DOCUMENTS a symbol
  (governing_docs). Tier-4 doc ingestion: `glia docs sync --space <KEY>` /
  `glia docs push` (network; feeds the deterministic build via a local snapshot).
- `glia pages <repo> [--dead-only]` — frontend page flow (LA.6e `pages::page_flow`):
  client-router pages with their handlers, the links between them, dead deep links
  (a router link no route serves, with the catch-all that absorbs it) and unlinked
  pages (no in-repo link reaches them: a fact, never "dead"). Exits 0 either way.

pyo3 (`PyGraph`): `blast_radius`, `cross_stack_trace`, `resolve`, `coverage`,
`governing_docs`, `page_flow` (+ `activate`, `find_node`, `node_cells`,
`dense_text*`). Engine entry points: `blast_radius_by_qname`, `cross_stack_trace`,
`resolve_signal_located`, `coverage_report`, `governing_docs`,
`entrypoint_reachable`, `locate_node`, `service_map` / `service_map_with` (A9.2,
behind `glia arch`), `pages::page_flow` (behind `glia pages`).

## Roadmap

- **v6 (done, glia-side):** P1 substrate completeness (blind 48→0, eval-gated by
  `bench/substrate-gap`), P2 coverage signaling, P3 answer-shaped primitives
  (above). P4 (collapse ~13 MCP tools → ~4) is repo-graph's job; these primitives
  are its enabler.
- **0.5.0** — finish the **glia** rename; domain registries for non-code (video, chemistry, policy, climate); code stays the reference domain.
  Renamed, all table-driven by `dev-notes/rename-0.5.0.py` (`--check` lists anything
  left on the old names): the repo, the `glia` binary (`cli/Cargo.toml`), every library
  crate — packages `glia-*`, Rust paths `glia_*` (LD.11a) — and the Python package: PyPI
  dist `glia-py`, module `glia_py`, wheel `glia_py-<ver>-cp311-abi3-*.whl` (LD.11b,
  `--python`). The repo-graph MCP wrapper still imports the old module until its own
  session moves to `glia-py` (LG.5), so both wheels stay installed side by side.

## Memory

Relevant architecture/spec memories from the repo-graph project memory system are copied under `dev-notes/glia-memory/`. They seeded this repo's Claude memory directory when the split landed. **Files under `dev-notes/glia-memory/` are point-in-time snapshots and are not updated in place** — several still cite the pre-split `rust/` layout and pre-wave-0 `lib.rs` line numbers. Read them as history; this file is the current architecture.
