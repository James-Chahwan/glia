# glia-py

Python bindings for [glia](https://github.com/James-Chahwan/glia), the Rust
engine that parses source, builds one cross-language code graph (every
component, cross-service call and shared resource across one repo or many) and
stores it in a zero-copy `.gmap` file. Built with pyo3 + maturin.

The [repo-graph](https://github.com/James-Chahwan/repo-graph) MCP server
(`mcp-repo-graph` on PyPI) depends on this package. Install it directly to call
the engine from your own Python code.

## Install

```bash
pip install glia-py
```

```python
import glia_py

g = glia_py.generate("path/to/repo")
print(glia_py.version(), glia_py.build_stamp())
```

**Names and versions.** The distribution is `glia-py` and the module is
`glia_py`, from 0.5.0 on. The 0.4.x releases (up to 0.4.18) shipped under the
old repo-graph package name. 0.5.0 is also the release that changes the API
conventions below, so an existing caller moves both in one step. `version()`
is the release; `build_stamp()` is `<release>+p<16 hex>`, the hex half a
content hash of every graph-shaping source file, so two builds of one release
that parse differently report different stamps.

## What's new in 0.5.2

**Pin exactly: `glia-py==0.5.2`.**

```bash
pip install glia-py==0.5.2
```

0.5.2 keeps the API conventions below, but it adds answers, changes graph contents and removes
two deprecated names, and each glia release is pinned exactly. The store format is unchanged
(format 3); `load_from_gmap` rebuilds a layout another glia build wrote, so a 0.5.1 layout is
rebuilt once, with the new graph contents.

- **Rules and queries.** `PyGraph.query(program, outputs=None, limit=None, explain=False,
  max_rows=None)` evaluates a Datalog program over the graph, with the predicate prelude
  merged in. `PyGraph.query_gql(query, limit=None, explain=False, max_rows=None)` answers a
  GQL read query (`MATCH ... WHERE ... RETURN`). Both return the QueryAnswer as a dict
  (`language`, `outputs`, `results`, `absence`). Each row's values are located, each row
  carries a `tier`, a row derived through a negation carries its coverage `caveats`, and
  `explain=True` adds witnesses. A parse, analysis or row-budget error raises `ValueError`
  with the rendered diagnostics. The dialect and the GQL subset are documented in the repo's
  `docs/rules.md`.
- **`glia_py.WatchSession(repo, overlay=True)`.** One repo's incremental build, held open in
  the process. `apply(paths)` updates it from the files you saw change, and `poll()` from its
  own stat scan (it returns `None` when nothing changed). Each update is a dict: `seq`,
  `files`, `groups_rebuilt`, `full_rebuild`, the totals, the located `nodes` / `edges` delta
  rows and `phase_ms`. Only the language group whose inputs moved is rebuilt. `snapshot()` is
  an independent `PyGraph` of the current build, and `persist()` writes `<repo>/.glia/graph/`
  plus the parse cache. The GIL is released while the engine works. Drive one session from
  one thread.
- **Cross-repo identity.** One package, message, contract op or SCIP symbol in several repos
  is one identity class (new edge category `SAME_AS`). `find`, `blast_radius`,
  `cross_stack_trace`, `entry_flows`, `tests_for`, `tests_for_diff`, `serves` and
  `service_map`, plus the module function `tests_for_rev`, collapse such a class to one row
  carrying `identity` (`tier`, `rules`, `diverged`, `also_in`: the other repos' copies,
  located). Each gains a trailing `collapse=True` keyword; `collapse=False` returns the raw
  rows. `serves` rows also gain `handler_identity`. `PyGraph.identity_classes(tier=None,
  rule=None, diverged_only=False, limit=None)` lists the classes.
- **Memory.** Dropping a `PyGraph` returns freed heap to the OS (glibc `malloc_trim`). A
  process that calls `generate()` in a loop now levels off instead of growing: 15 rebuilds of
  glia went from 410 -> 1,017 MB to 404 -> 265 MB. Set `GLIA_MALLOC_TRIM=0` to turn the trim
  off.
- **Removed.** The 0.5.0 `_experimental` spellings of `PyGraph.patterns` and
  `glia_py.patterns_vs_rev`, deprecated in 0.5.1. Call `patterns` / `patterns_vs_rev`.
- **Answer and overlay changes.** `check` evaluates `[[constraint]]` rules with a Datalog `rule`
  and transitive forbids (overlay `version = 2`). Each violation carries `blocking`, true when
  its tier is at least its rule's `min_tier` (a heuristic one never blocks). `review_vs_rev`'s
  `blocking` is set only by a new blocking violation. `gaps` gains the `rotted_rule` category. `overlay_try`'s dict gains
  `rules` when a build checked a rule or the candidate holds a `[[constraint]]`, and stanza
  dicts gain `rule` / `rules`. `overlay_accept` raises `ValueError` on a rule that `glia check`
  cannot evaluate. The cell API (`set_cell`, `write_cell`) refuses a CONSTRAINT entry with a
  `rule`, since rule text is declared only in `.glia/overlay.toml`.
- **Graph content a caller sees.** Rebuild any stored graph.
  - SAME_AS stars replace the SHARES_DEPENDENCY edges between repos sharing a package, and the
    SHARES_SCHEMA edges between copies of one message. SHARES_SCHEMA now pairs DTOs only.
  - Nodes in generated files carry ORIGIN `generated`, and a vendored generated gRPC stub no
    longer makes a service "call" every service. A third-party WebSocket, gRPC or GraphQL
    client is ORIGIN `external` and never paired. `effects` and `serves` name its host in
    `external_hosts`.
  - `.glia/overlay.toml` `[walk] tests` now adds TESTS edges, not only provenance.
  - TypeScript parameters and locals type their receivers. Inherited typed-receiver calls bind
    to the superclass method, and concrete overrides IMPLEMENTS the member they override.
    Go imports bind the package's `package` clause name. Rust trait impls across files are
    IMPLEMENTS. Ruby, PHP and Elixir gain heritage edges, and Dart own-package imports and
    plain imports resolve.
  - The coverage matrix's remaining none / partial cells for Rust, Scala, Ruby, PHP, Dart and
    Elixir (brokers, buses, GraphQL, WebSockets, cron, CLIs, ORMs, config) now extract, as do
    Python blinker / Django signals and a GCP Pub/Sub subscription bound to its topic by
    Terraform.
  - An ambiguous doc mention links the same symbol in every clone of a repo.
- **The tweaks** (the gaps two real full-stack repos showed):
  - **Contract models.** TypeScript interfaces and field-only classes, Go structs, Dart
    models and GraphQL schema types carry their fields (a SCHEMA_FIELDS cell); a typed HTTP
    call site records the response and request shape it reads and sends (`response` /
    `request` on its ENDPOINT_HIT), and a Go handler what it writes and binds. A client model
    and the server struct of its HTTP exchange form an identity class (rule `http_model`), as
    does a GraphQL client's result model with its schema type (`graphql_model`): `find`,
    `blast_radius` and the other collapsing answers fold them, inside one repo too.
    `identity_classes(rule=...)` accepts both rules.
  - **`contract_fields`** returns three new pairings: `http_model` (client model against
    server model, response and request), `client_contract` (client against the OpenAPI op
    documenting its route) and `graphql_selection` (an operation's selection against its
    schema type).
  - **`contract_breaks_vs_rev(.., models_min_tier="fact")` and `review_vs_rev(..,
    models_min_tier="fact")`.** Both judge the inferred models against the rev: the
    contract-breaks dict gains `models` and `advisory`, the review dict `model_breaks`,
    `counts["model_breaks"]` and, when non-empty, `unanchored`. A model row blocks only at or
    above `models_min_tier` (`"fact"` by default, so the resolver-paired rows are advisory);
    an unknown tier raises `ValueError` before any build.
  - **`PyGraph.check(strict=False)`.** It reads each repo root's `.glia/overlay.toml`, and a
    declared rule that checks nothing (anchored nowhere, rejected by the loader, absent from
    the graph, or scoped where no node sits) is listed under `unanchored` with a reason and a
    fix instead of passing silently; an empty-scope rule leaves `errors` for it unless
    `strict=True`. `overlay_try`'s rule status may read `checks_nothing` (verdict review, with
    `reason` / `fix`), and `overlay_accept` refuses only a rule that does not parse or defines
    no `violation`.
  - **Traversal takes category names.** `neighbours`, `bfs`, `predecessors`, `reachable_by` and
    `shortest_path` accept `categories=["CALLS", "imports", 9]`: ids or registry names, mixed,
    case-insensitive. An unknown name raises `ValueError` naming it; rows still carry ids.
  - **`set_cell(.., hint=None)` / `write_cell(.., hint=None)`** take a node's v1 identity hint
    (`v1|<kind id>|<file name>|<body hash>|<local name>`), validated (`ValueError` on a
    malformed or kind-contradicting one). The outcome dict gains `node_hint` and
    `hint_differs`, and `target` reads `hint_differs` when a fresh build binds the qname to a
    node whose own hint differs.
  - **`query_gql`** rows that return properties only carry `file` / `line`, the first
    property's node, as the Datalog form does.
  - **Graph content.** TS directory imports and re-export barrels bind; NATS calls on any
    receiver mint queue nodes, subjects folded on the NATS grammar (`orders.*.placed`); a
    WebSocket client's URL is read through the local, field, builder or config base that holds
    it; a trailing query-string interpolation leaves an endpoint's path; a GraphQL operation
    pairs by the root fields it selects, and its client's transport is an HTTP call; Solidity
    functions are EVM procedures that ethers, hardhat, TypeChain, viem, wagmi, web3.js and
    go-ethereum client calls pair with; gqlgen resolver methods are handlers; Angular template
    and host bindings, Flutter widget trees and TypeScript values passed by reference are
    edges; a used class's framework-invoked members and constructors are live, and generated
    code and test-only routes leave `gaps`. Rebuild any stored graph.
- **The close-out: dead-symbol false positives.** No signature or return shape changes; the
  graph and every liveness reader move. Dart getter reads, tear-offs and names in any value
  position, calls on Riverpod provider reads, Go range-element, qualified-field and func-value
  receivers, func / method values and composite literals (`USES`), `pkg.F` route handlers,
  TypeScript signal-query, inline-`inject(T)`, call-result and accessor receivers, inline
  `inject(T)` (`INJECTS`) and static `T.X` reads (`USES`) are edges. A Go `init`, a Swift
  `@main` type and a Kotlin Flutter activity (ROLE COMPONENT) are entries, so a node record's
  `entry` flips; a Python module with an `if __name__ == "__main__":` guard is a declared entry
  (its guard's calls are the module's, `node_cells` shows an ENTRYPOINT cell
  `{"decl":"<file>:<line>","source":"main_guard"}`, and its record's `live` flips while `entry`
  stays false); on a used class, Go methods the runtime calls by signature,
  app-delegate callbacks and the members a library base calls (Transloco, Angular `ErrorHandler` /
  `Resolve`, dio interceptors) are live; a Dio interceptor class gains ROLE SERVICE. `gaps`
  drops those `dead_symbol` rows: on the two demo repos 52 -> 32 and 124 -> 41, exactly the
  rows a hand audit judged truly dead. Rebuild any stored graph.

## What's new in 0.5.1

**Pin exactly: `glia-py==0.5.1`.** 0.5.1 keeps 0.5.0's API conventions, but it changes
graph contents and the store format, and each glia release is pinned exactly.

- **New `PyGraph` answers.** `pack` / `pack_ids` (the context for a query, packed to
  a token budget), `hotspots`, `cochange`, `flags` (stale feature flags),
  `communities`, `splits`, `hubs`, `duplicate_flows`, and `patterns`, out of
  experimental (the `_experimental` name warned in 0.5.1 and was removed in 0.5.2). `tests_for` /
  `tests_for_diff` gain `limit=` and `signals=` (rank by recent failures and
  co-change).
- **New module functions.** `review_vs_rev` is the PR report for a change against
  a git rev (`format="markdown"` for a PR comment). Also `contract_breaks_vs_rev`
  and `cochange_vs_rev`. `timeline_build` / `timeline_history` / `timeline_as_of`
  say when each edge appeared and went over recent commits. `overlay_propose` /
  `overlay_try` / `overlay_accept` run the overlay loop in-process. `patterns_vs_rev`
  is new; its deprecated `_experimental` alias was kept through 0.5.1 and removed in
  0.5.2. `tests_ingest` gains `window=` / `reset=`: a re-ingest keeps a rolling
  window of runs.
- **Answer shapes.** `effects` and `serves` rows gain `external_hosts`, the
  third-party hosts a sink calls (`[]` for in-repo rows). `serves` lists such an
  endpoint for an HTTP channel. `gaps` rows gain a stable `id` and the
  `suspected_edge` category, and `gaps` no longer lists an implementation of a
  called method as a dead symbol.
- **Store.** `.gmap` format 3 is smaller: interned evidence, code cells stored as
  spans into the source, and an lz4 parse cache. `load_from_gmap` rebuilds a 0.5.0
  layout once.
- **Graph content a caller sees.** Rebuild any stored graph; a node whose qname
  moved gets a new id.
  - Client endpoints whose URL comes from a URL builder, a URL method, a `const`
    local or a `readonly` field get real paths instead of
    `endpoint:<METHOD>:<unresolved>`, and a configured API prefix (`/api`) folds
    into their qname.
  - Go routes mounted on a group passed through a parameter or a struct field gain
    their prefix.
  - .NET `appsettings*.json` keys are a new `config:setting:<Section:Key>` track,
    read by C# `IConfiguration` and defined by `Section__Key` env vars too.
  - Every language gets function-level TESTS edges, and TEST cells on functions.
  - TypeScript: `this.m()` / `super.m()` calls bind through superclasses, and
    overrides IMPLEMENTS abstract members. Signal and call-initialised fields are
    STATE_VARs, and a method passed by value is USES.
  - Go: struct embeds are INHERITS_FROM, with promoted calls and IMPLEMENTS.
    Type aliases count in interface satisfaction, so streaming gRPC servers
    implement their service. Package-var initialisers call, and imports of the
    repository-root package resolve.
  - `.glia/overlay.toml` `[walk] tests` adds `test_fixture` provenance.
  - More broker, job, GraphQL and cron coverage, and fewer phantom nodes from
    string literals in Rust and Python.

## What's new in 0.5.0

The first release under the `glia-py` name, and one breaking release that moves
every contract at once.

- **Answers, not just a graph.** `blast_radius` (multi-seed), `diff_impact`,
  `graph_delta`, `tests_for`, `effects`, `why`, `cycles`, `check`, `spec_status`,
  `serves`, `implementors`, `entry_flows`, `feature_flows`, `patterns` (shipped as
  an `_experimental` name, deprecated in 0.5.1 and removed in 0.5.2),
  `gaps` and `page_flow`. Each returns located rows with a confidence tier, a
  `live` flag, and an `absence` that says what the graph could not see.
- **Evidence on every edge.** Each edge records the extractor, parser, resolver
  or pass that made it, the rule it matched, and the exact call-site line.
- **Identity that holds still.** Repo ids come from the git remote rather than
  the path; same-stem files in one directory no longer collide; symbols keep
  their identity across file moves.
- **Broader extraction.** Kotlin as its own parser; dependency injection in five
  languages; eight ORMs; WebSocket, tRPC / Connect / Twirp, cron and CLI
  breadth; frontend route and page flows; typed receivers in eight languages.
- **Your own knowledge in the graph.** `.glia/overlay.toml` (edges, wrappers,
  constants, route prefixes, constraints, decisions, notes), the cell write API,
  git-history ingest (churn, co-change) and test-report ingest (failures,
  coverage).
- **Store.** `.gmap` format 2 behind a `GLIAGMAP` preamble: an old or foreign
  file reports "rebuild" instead of failing validation. One on-disk layout at
  `<repo>/.glia/graph/`, which repairs itself when stale.
- **Faster.** Parsing runs on a thread pool (`GLIA_THREADS`), graph traversal is
  about 25x quicker, and the parse cache is written only when it changed. On
  real repos, builds are 1.5-3.6x faster than 0.4.18 with 11-61% more edges.
- **Breaking.** The module is `glia_py`, not `repo_graph_py`; answers are native
  Python objects (see below); `find_node`, `find_nodes_by_qname` and
  `resolve_signal` are replaced by `find` and `resolve`; node ids, qnames and the
  `.gmap` format all change once, so rebuild any stored graph.

## API conventions

Every function and `PyGraph` method follows these rules (since 0.5.0, LD.2):

- **Answers are native Python objects.** `find`, `resolve`, `governing_docs`,
  `blast_radius` and `service_map` return a `dict`; `cross_stack_trace`,
  `coverage`, `project_roots` and `contracts` return a `list` of `dict`;
  `page_flow` returns a `dict`. The keys are exactly the engine struct's
  fields, in field order. An envelope answer (`find`, `resolve`,
  `governing_docs`) is `{"results": [...], "absence": None | {...}}`: an empty
  answer says why in `absence`, including `unparsed_files`. `blast_radius`
  (LD.5) takes one qname or a list of them and answers
  `{"seeds", "unresolved", "results", "absence"}`: one walk and one ranking
  over every seed, each row naming its `seed`.
- **A name ending in `_json` returns a JSON string.** Only the bulk dumps:
  `nodes_json`, `edges_json`, `parse_file_to_json`. Call `json.loads` on them.
- **Pair-shaped data is a list of tuples.** `activate`, `node_cells`,
  `neighbours`, `kind_names`, `category_names`, `cell_type_names`.
- **Node ids are Python `int`s.** They are `u64` and often above `2**63`, so
  never round-trip them through `float`.
- **Lines are 1-based** in every answer record and in `nodes_json`.
- **One lookup.** `find(query, top_k=20, kinds=None, scope=None, collapse=True)` is
  the name / qname search. When `q` is a node's exact qname or name,
  `find(q, top_k=1)["results"][0]` is the node `blast_radius(q)` starts from.
  `kinds` takes node-kind names in any case (`["FUNCTION", "CLASS"]`).
  `scope` is a filter: nodes whose file lies outside it are dropped, and
  nodes with no file are kept.

### Building and persisting

`generate(repo_path, incremental=False)` and
`generate_many(repo_paths, incremental=False)` share one contract:

- **They only build.** Neither writes the `.gmap` layout. Call
  `g.save_to_default(repo_path)` (or `g.save_to(dir)`) to persist, then
  `load_from_gmap(dir)` in a later session.
- **`incremental=False` (the default) is pure.** It reads, writes and deletes
  nothing under the repo.
- **`incremental=True` uses a per-file parse cache.** It reuses and refreshes
  `<repo>/.glia/graph/parse_cache.bin`, and the graph is identical to a clean
  build. `purge_parse_cache(repo_path)` deletes that cache.
- **Both raise `ValueError`** when no node was produced and files failed to
  parse. Otherwise parse failures are listed in `g.parse_errors`.

`GLIA_NO_PERSIST=1` no longer affects `generate`. It still stops
`load_from_gmap` from writing back a layout it had to rebuild.

## Platform support

abi3 wheels for CPython 3.11 and newer: Linux x86_64 and aarch64 (manylinux
2_28), macOS x86_64 and arm64, Windows x86_64, plus an sdist. Building the
sdist needs a Rust toolchain.

## License

[Glia Software License v0.1](https://github.com/James-Chahwan/glia/blob/main/LICENSE):
PolyForm Noncommercial 1.0.0 plus a worker-protection overlay. See `LICENSE`.
