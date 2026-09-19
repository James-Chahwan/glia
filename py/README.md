# repo-graph-py

Native Rust engine for [`mcp-repo-graph`](https://pypi.org/project/mcp-repo-graph/) — parses source, builds a unified cross-language graph, stores it in a zero-copy `.gmap` file.

This package is the Rust engine (via pyo3 + maturin). Users install it transitively through `mcp-repo-graph`; there is usually no reason to install it directly.

## Install

```bash
pip install mcp-repo-graph
```

This pulls `repo-graph-py` as a dependency and gives you the `repo-graph` CLI.

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

- **v0.4.12** — Linux x86_64 only (prebuilt wheel). Other platforms will need Rust + maturin at install time until v0.4.13 adds the full wheel matrix.
- **v0.4.13 (planned)** — Linux x86_64/aarch64 (manylinux), macOS x86_64/arm64, Windows x86_64 × Python 3.11–3.14 via maturin GitHub Actions.

## License

MIT
