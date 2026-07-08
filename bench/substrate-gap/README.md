# Substrate-gap eval (glia P1)

**Handoff v6 thesis:** the graph's value is a function of (size × complexity ×
cross-boundary-ness) exceeding context. Every missing edge is a task grep wins
*today* that would flip to the graph. This eval finds the missing edges.

It is the recall pattern from repo-graph's `bench/grade.py` pushed **down to the
substrate layer**: instead of grading an agent's answer against a hand-enumerated
key, we grade the **graph's extracted edges** against a hand-enumerated key of
edges that *should* exist. A `(framework × edge-category)` cell that reads `0.00`
is a confirmed blind spot.

## Run it

```bash
cd bench/substrate-gap
python3 run.py                    # grade every fixture, print the matrix, append results.jsonl
python3 grade.py fixtures/<name>  # grade one fixture (verbose)
python3 grade.py fixtures/<name> --dump   # + dump ALL emitted nodes/edges (author keys against reality)
```

Requires the `repo_graph_py` wheel importable (`maturin develop` in `py/` for a
local build). Grading is hermetic (`GLIA_NO_PERSIST=1`, non-incremental).

## Fixture shape

Each `fixtures/<name>/` has source file(s) for one framework plus `key.json`:

```json
{
  "framework": "ts-angular-di",
  "language":  "typescript",
  "dirs":      ["."],              // 1 dir => generate(); 2+ => generate_many()
                                   //   (distinct RepoIds so cross-graph resolvers
                                   //   — HttpStack, gRPC, Queue … — fire across
                                   //   the boundary; the documented substrate-eval path)
  "expect_nodes": [ {"kind": "SERVICE",   "name": "ApiService"} ],
  "expect_edges": [ {"from": "AppComponent", "to": "ApiService", "category": "INJECTS"} ]
}
```

Matching is lenient on identity (case-folded substring over name **or** qname,
`::`/`.` normalised to `/`) and strict on kind/category. We measure "did an edge
of the right category between the right two entities get emitted at all."

`expect_nodes` attributes gaps to the right layer: a missing **node** is an
extraction gap in that framework's parser; a present node with a missing **edge**
is a resolver/wiring gap. (E.g. `xstack-ts-go-http` HTTP_CALLS = 1.00 proves the
`HttpStackResolver` works; `dart-http-dio` HTTP_CALLS = 0.00 with a missing
ENDPOINT node isolates the gap to **Dart endpoint extraction**, not the resolver.)

## Confirmed blind spots (v1, engine 0.4.16)

| cell | verdict | root cause |
|---|---|---|
| `dart-http-dio` · HTTP_CALLS | ⊘ 0.00 | Dart `dio.get/post` emits **no ENDPOINT** node. Worse, `scan_dart_routes` mis-emits the client call paths as phantom **ROUTE** nodes. |
| `ts-angular-di` · INJECTS | ⊘ 0.00 | `edge_category::INJECTS` (id 8) is defined + weighted but **never emitted**. Constructor DI produces no edge → liveness false-flags `@Injectable` services dead. |
| `ts-imports` · IMPORTS | ⊘ 0.00 | `engine/src/lib.rs` wires `build_typescript(.., \|_,_\| None)` — the TS import `resolve_source` is **stubbed to None**, so **no TS/JS/Angular/React/Vue import ever becomes a category-3 edge** (imports live only as cells). `impact`/`activate` cannot traverse the entire TS frontend's imports. |

`results.jsonl` is the append-only history — one record per `run.py`, tagged with
engine version, so the map is diffable across sessions and after each P1 fix.
A fix is proven when its cell flips `0.00 → 1.00` here (the fired_on marker).
