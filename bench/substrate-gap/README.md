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

## Fixture shape — the FROZEN key.json vocabulary

Each `fixtures/<name>/` has source file(s) for one framework plus `key.json`.
The ten fields below are the **complete and only** vocabulary:

```json
{
  "framework": "ts-angular-di",
  "language":  "typescript",
  "dirs":      ["."],              // 1 dir => generate(); 2+ => generate_many()
                                   //   (distinct RepoIds so cross-graph resolvers
                                   //   — HttpStack, gRPC, Queue … — fire across
                                   //   the boundary; the documented substrate-eval path)

  // ---- RECALL: what MUST be emitted ----
  "expect_nodes": [ {"kind": "SERVICE", "name": "ApiService", "note": "…"} ],
  "expect_edges": [ {"from": "AppComponent", "to": "ApiService",
                     "category": "INJECTS", "note": "…"} ],
  "expect_cells": [ {"kind": "ENDPOINT", "node": "GET /users",
                     "cell": "POSITION", "contains": "app.ts", "note": "…"} ],

  // ---- PRECISION: what must NOT be emitted ----
  "forbid": [
    {"kind": "ROUTE", "name": "/users"},                 // no such node at all
    {"kind": "ROUTE"},                                    // name omitted = any name
    {"kind": "ROUTE", "name": "/users", "max_nodes": 1},  // at most N such nodes
    {"from": "Nav", "to": "GET /users", "category": "HTTP_CALLS"}  // no such edge
  ],

  // ---- matrix binding: stored and echoed, NOT graded here ----
  "mechanism": "http",
  "cells": ["typescript/http"],

  "note": "free-form commentary, ignored by the grader"
}
```

**`grade_fixture` RAISES `ValueError` on any unrecognised top-level field**, and
on any unrecognised sub-field of `expect_nodes` / `expect_edges` / `expect_cells`
/ `forbid`. That is deliberate. Before this rule, unknown fields were ignored in
silence, so six planned packets each invented their own spelling of the same
assertion (`forbid_edges`, `expect_absent_edges`, `expect_absent_nodes`,
`forbid`; `expect_cells` vs `expect_literals`) — whichever lost the race would
have shipped a precision gate that scored **green while never executing**. If you
need something this vocabulary cannot express, extend `TOP_FIELDS` in `grade.py`
**and this section**; do not invent a fifth spelling.

Semantics:

- **Identity matching** is lenient (case-folded substring over name **or** qname,
  `::`/`.` normalised to `/`) and strict on kind/category id. We measure "did an
  edge of the right category between the right two entities get emitted at all."
- **`forbid` uses the exact same matcher** as `expect_edges` / `expect_nodes`. A
  precision gate that matched more loosely than the recall gate would be
  unfalsifiable. A violation is `matched > max_nodes` (default `0`); edge entries
  have no cap. Violations are reported per fixture as `FORBID VIOLATIONS: n` and
  do **not** move the recall matrix — a fixture can read `1.00` and still be
  violating, which is precisely why the section is printed separately.
- **`expect_cells`** asserts a node of `kind` leniently matching `node` carries a
  cell of type `cell`, optionally containing `contains`. `contains` is a
  case-folded **plain** substring of the cell payload (not the `_norm` identity
  matcher — payloads are literals, not qnames). This is how a fixture gates
  "located" (POSITION), captured env VALUES, message types and identifying
  literals instead of eyeballing `--dump`.
- An **unregistered `cell` name scores a miss, not a raise**, so a `0.00`
  baseline is recordable for a cell type a later packet still has to add. An
  unregistered `kind`/`category` still raises — those registries are locked.
- **`mechanism` / `cells`** are stored and echoed into `results.jsonl`
  (`"<language>/<mechanism>"`); the derived coverage matrix is a later packet's
  job, not this harness's.

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

## Reading the run

Under the matrix, `run.py` prints five totals. Each is a gate:

| section | green | meaning |
|---|---|---|
| `BLIND SPOTS (recall 0.00)` | 0 | no cell is fully blind |
| `PARTIAL (0 < recall < 1)` | *(listed)* | a **half**-fixed cell. Added W0.4: before it, only `recall == 0.0` was flagged, so `php-laravel` CALLS 0.50 was invisible in the matrix **and** in `results.jsonl`, and any packet that half-fixed a cell was unobservable. |
| `FORBID VIOLATIONS` | 0 | every `forbid` assertion held |
| `MISSING CELLS` | 0 | every `expect_cells` assertion held |
| `GRADER ERRORS` | 0 | no fixture raised. A raising fixture **drops out of the matrix**, so it is named on stdout here as well as stderr — otherwise a broken key.json reads as a silently-missing row. |

`results.jsonl` is the append-only history — one record per `run.py`, tagged with
engine version, so the map is diffable across sessions and after each P1 fix.
A fix is proven when its cell flips `0.00 → 1.00` here (the fired_on marker).
