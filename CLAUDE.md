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
graph/              Per-repo graph builder, universal resolver, cross-graph resolvers
                    src/ is MODULES, not one lib.rs (see "Module layout" below)
engine/             Orchestration: walk → parse → extract → build → merge → resolve.
                    Owns every parser dependency. src/ is MODULES; lib.rs is a facade
cli/                The `glia` binary
doc-sources/        Tier-4 doc ingestion (Confluence REST + local snapshot)
store/              .gmap binary format — rkyv + mmap, sharded layout
projection-text/    Dense sigil text output (scopes, defaults, module dedup)
activation/         Spreading activation — domain-agnostic PPR with configurable direction/weights
parsers/code/
  python/  go/  typescript/  rust/  java/  csharp/  ruby/  php/  swift/
  c_cpp/   scala/  clojure/  dart/  elixir/  solidity/  terraform/
  react/   angular/  vue/    — framework parsers stacked on typescript
  extractors/ — cross-cutting: data_sources, cli, grpc, queues, websocket,
                eventbus, graphql, ts_routes, angular/react/vue route extractors
stamp/              Build identity: RELEASE + PARSER_STAMP (content hash of every
                    graph-shaping source). Keys the parse cache, so a parser fix
                    invalidates caches without a version bump.
py/                 pyo3 bindings — the only Rust crate published to PyPI (as repo-graph-py)
```

Parsers live at `parsers/<domain>/<language>/`. When v0.5.0 adds non-code domains, they nest alongside `parsers/code/`.

### Module layout — `engine` and `graph` are split, not monolithic

Both crates' `lib.rs` is a **facade**: `mod` declarations plus the `pub use` list that
fixes the crate's public surface. No logic lives there. **Never trust a line number in
older prose — `grep -rn` the symbol name.**

```
engine/src/   walk.rs     repo walk, gitignore, region graph
              route.rs    per-file routing (yaml/Dockerfile/manifest/dotenv/.proto), parse cache
              extract.rs  detect_language, parse_one, parse_one_with, cross-cutting extractors
              build.rs    generate_one*, generate_many, build_graphs_for_repo, run_all_resolvers
              docs.rs     markdown ingest          passes.rs   doc-linker, TESTS edge
              coverage.rs coverage_report          answers.rs  the P3 primitives
              cache.rs    incremental parse cache
graph/src/    types.rs build.rs imports.rs calls.rs merged.rs traversal.rs
              blast.rs activation.rs signal.rs
              resolvers/  one module per mechanism (http, grpc, queue, graphql,
                          websocket, eventbus, shared_schema, db, cron, config,
                          iac, package, cli) + mod.rs
```

To widen something across a new module boundary use `pub(crate)`, never `pub` — the
public API is exactly what the facade re-exports.

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
- Parsers must not short-circuit this: extract what the AST makes available; don't cap at what old regex heuristics happened to capture.

## Format Spec — `.gmap`

Zero-copy rkyv serialisation with memory-mapped read. Sharded by kind to keep hot paths local. Write-once, rebuild-whole-file — no in-place mutation. Owned vs Archived types are the mental model: loaded views are `Archived<T>`, writes go through `Owned<T>` then serialise.

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
its `pub use resolvers::{…}` list only if it must be public. Register it so `MergedGraph`
calls it during cross-repo resolution (`run_all_resolvers` in `engine/src/build.rs`).
`resolvers/http.rs` is the canonical example. Shipped: HTTP, gRPC, Queue, GraphQL,
WebSocket, EventBus, SharedSchema, DB, Cron, Config, IaC, Package, CLI. See
`dev-notes/glia-memory/project_040_stack_resolvers_backlog.md`.

## Key Design Decisions

- **Tree-sitter, not regex.** 0.4.x moved to AST extraction; 0.2.0 regex is not a ceiling.
- **Zero-copy store.** rkyv + mmap; writes rebuild the whole file.
- **Domain-agnostic core.** `core` and `activation` know nothing about code. Code lives in `code-domain` and the parsers.
- **Publish gate.** Only `rust/py/` publishes to PyPI (as `repo-graph-py`). Everything else is internal workspace.
- **No Python fallback.** After v0.4.10c, Python is a thin pyo3 wrapper; there is no parallel Python implementation to keep in sync.

## Query & Answer Surface (v6 P2/P3)

Answer-shaped primitives live in the **engine** (shared by CLI + pyo3/MCP + future
TUI), not composed by the consumer. Each is one call: complete, ranked, located.

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

pyo3 (`PyGraph`): `blast_radius`, `cross_stack_trace`, `resolve`, `coverage`,
`governing_docs` (+ `activate`, `find_node`, `node_cells`, `dense_text*`). Engine
entry points: `blast_radius_by_qname`, `cross_stack_trace`, `resolve_signal_located`,
`coverage_report`, `governing_docs`, `entrypoint_reachable`, `locate_node`,
`service_map` / `service_map_with` (A9.2, behind `glia arch`).

## Roadmap

- **v6 (done, glia-side):** P1 substrate completeness (blind 48→0, eval-gated by
  `bench/substrate-gap`), P2 coverage signaling, P3 answer-shaped primitives
  (above). P4 (collapse ~13 MCP tools → ~4) is repo-graph's job; these primitives
  are its enabler.
- **0.5.0** — finish the **glia** rename; domain registries for non-code (video, chemistry, policy, climate); code stays the reference domain.
  Already renamed: the repo and the `glia` binary (`cli/Cargo.toml`). Still `repo-graph-*`:
  every library crate name and the PyPI package (`repo-graph-py`) — renaming those breaks
  downstream pins, so it is a 0.5.0 gate, not a drive-by.

## Memory

Relevant architecture/spec memories from the repo-graph project memory system are copied under `dev-notes/glia-memory/`. They seeded this repo's Claude memory directory when the split landed. **Files under `dev-notes/glia-memory/` are point-in-time snapshots and are not updated in place** — several still cite the pre-split `rust/` layout and pre-wave-0 `lib.rs` line numbers. Read them as history; this file is the current architecture.
