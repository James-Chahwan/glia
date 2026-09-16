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

Requires the `repo_graph_py` wheel importable. **`grade.py` imports the INSTALLED
wheel, never the working tree** — a Rust change is invisible to grading until you
rebuild. The working recipe on this machine — there is no venv, so the `develop` flow
does not apply, and `maturin build` alone can repackage a stale `.so`, which is what
the `clean` is for:

```bash
cargo clean -p repo-graph-engine -p repo-graph-py
maturin build
pip install --force-reinstall target/wheels/<wheel>
```

Grading is hermetic (`GLIA_NO_PERSIST=1`, non-incremental).

## Fixture shape — the FROZEN key.json vocabulary

Each `fixtures/<name>/` has source file(s) for one framework plus `key.json`.
The eleven fields below are the **complete and only** vocabulary:

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

  // ---- files git refuses to track, copied in for the grade only ----
  "materialize": {"libs/sdk/.git": "libs/sdk/_dotgit"},  // dest: source

  // ---- matrix binding: stored and echoed, NOT graded here ----
  "mechanism": "http",
  "cells": ["typescript/http"],

  "note": "free-form commentary, ignored by the grader"
}
```

### `materialize` — shipping a file git will not track

git silently refuses to track any path component named `.git`: in a tree holding
`libs/sdk/.git` (a `gitdir:` file) plus `libs/sdk/a.py`, `git add -A` exits 0 and
`git ls-files` lists **only** `libs/sdk/a.py`. So a fixture that must prove
"a nested `.git` makes a REGION anchor instead of being walked into" cannot ship
the file it needs. It ships a trackable stand-in (`libs/sdk/_dotgit`) and maps
`destination -> source` here; `build_graph` copies each one into place before
`generate()` sees the tree, prints `[materialize] <n> paths in <fixture>` on
stderr, and removes them again in a `finally` — including when `generate()`
raises, because a leaked `.git` under `fixtures/` would make the glia tree
itself look like it contains a submodule. Both paths are relative to the fixture
dir and both must stay inside it (`../escape` raises
`ValueError: … materialize path escapes fixture dir`); an existing destination
raises rather than being overwritten. `git status --porcelain
bench/substrate-gap/fixtures` is empty after a grade.

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

## Precision gates match exactly (wave 2 correction)

`forbid` uses **strict** identity — the normalised pattern must EQUAL the node's
name or its qname — while `expect_nodes` / `expect_edges` keep the lenient
substring matcher.

This asymmetry is deliberate. Leniency in a *recall* gate can only turn a miss
into a hit, so it is safe. Leniency in a *precision* gate turns it into a false
accusation: `forbid {to: "UserController"}` also matched the method whose qname
is `UserController::UserController::getUser`, and the harness reported a
violation against a correct graph (wave 2, `java-spring-composed`).

`test_grade.py` pins both directions — the strict matcher must not match an
ancestor qname segment, and must still catch a real violation.
