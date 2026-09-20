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

## What's new in 0.5.0

The first release under the `glia-py` name, and one breaking release that moves
every contract at once.

- **Answers, not just a graph.** `blast_radius` (multi-seed), `diff_impact`,
  `graph_delta`, `tests_for`, `effects`, `why`, `cycles`, `check`, `spec_status`,
  `serves`, `implementors`, `entry_flows`, `feature_flows`, `patterns_experimental`,
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
- **One lookup.** `find(query, top_k=20, kinds=None, scope=None)` is the name /
  qname search. When `q` is a node's exact qname or name,
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
