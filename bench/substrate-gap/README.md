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
python3 run.py --no-log --check   # exit 1, naming each change, if legacy-latest.json is stale
python3 run.py --no-log --emit    # rewrite the committed legacy-latest.json (clean tree only)
python3 grade.py fixtures/<name>  # grade one fixture (verbose)
python3 grade.py fixtures/<name> --dump   # + dump ALL emitted nodes/edges (author keys against reality)
python3 incremental_check.py      # regrade every fixture through the parse cache; exit 1 if warm != cold
```

Requires the `glia_py` wheel (PyPI `glia-py`, 0.5.0+) importable. **`grade.py`
imports the INSTALLED wheel, never the working tree** — a Rust change is invisible to
grading until you rebuild. The working recipe on this machine — `maturin build` alone
can repackage a stale `.so`, which is what the `clean` is for:

```bash
cargo clean -p glia-engine -p glia-py
maturin build -m py/Cargo.toml --release
pip install --force-reinstall --no-deps target/wheels/glia_py-*.whl
```

During the 0.5.0 leap run the `pip` and every script here with
`~/.venvs/glia-leap/bin/python`, never the user-site `python3`: the user site keeps
the 0.4.x wheel that the repo-graph MCP server imports
(`dev-notes/wave-runner/README.md`).

Grading is hermetic (`GLIA_NO_PERSIST=1`, non-incremental).

## Fixture shape — the FROZEN key.json vocabulary

Each `fixtures/<name>/` has source file(s) for one framework plus `key.json`.
The eleven fields below are the **complete and only** vocabulary:

```json
{
  "framework": "ts-angular-di",
  "language":  "typescript",
  "dirs":      ["."],              // 1 dir => generate(); 2+ => generate_many()
                                   //   (a RepoId per dir; stack resolvers pair
                                   //   either way, only SHARES_* need two repos
                                   //   — AUTHORING.md step 1 has the measured table)

  // ---- RECALL: what MUST be emitted ----
  "expect_nodes": [ {"kind": "SERVICE", "name": "ApiService", "note": "…"} ],
  "expect_edges": [ {"from": "AppComponent", "to": "ApiService",
                     "category": "INJECTS", "note": "…"},
                    {"from": "get_user", "to": "helper", "category": "CALLS",
                     "exact": true} ],   // optional on nodes / edges: strict identity
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

- **Identity matching** in `expect_nodes` / `expect_edges` is lenient
  (case-folded substring over name **or** qname, `::`/`.` normalised to `/`)
  unless the entry sets `"exact": true`, which requires the normalised pattern to
  EQUAL the name or the qname; kind / category are always strict. We measure "did
  an edge of the right category between the right two entities get emitted at
  all." Lenient is the default and stays right where leniency is the point
  (recall across renamed or re-nested qnames). Set `exact` when the key must pin
  WHICH of two same-prefixed nodes carries the edge: lenient `get_user -> helper`
  is satisfied by `get_user_impl -> helper`, exact is not. `exact` must be a JSON
  boolean (`"true"` raises). A fixture that uses it prints `[substrate-gap] exact
  identity rows=<n> in <fixture>` on stderr, and each such row prints `[exact]`
  after its kind / category.
- **`forbid` always matches exactly** (see *Precision gates match exactly*
  below). A violation is `matched > max_nodes` (default `0`); edge entries
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

## Confirmed blind spots (v1, engine 0.4.16 — HISTORICAL, all three fixed)

**This table is kept for the root-cause analysis, not for its verdicts.** All
three cells read 1.00 today (measured 2026-09-16: `dart-http-dio` HTTP_CALLS
2/2, `ts-angular-di` INJECTS 1/1, `ts-imports` IMPORTS 1/1). For what is
actually blind NOW, read `COVERAGE.md` — generated by `matrix.py --emit`,
committed, and re-checked by `matrix.py --check`, so it cannot rot the way this
table did.

| cell | verdict (v1, 0.4.16) | root cause |
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

`results.jsonl` is gitignored and never leaves this machine. **The proof of record
is the committed `legacy-latest.json`**: this runner's view — every fixture's
per-category recall (keyed by fixture directory, so a `0.50` is visible even though
only `0.00` reaches BLIND SPOTS) plus the five summary sections, line for line as
stdout prints them. It is deterministic (no timestamp or build stamp, `sort_keys`)
and written **only** by `--emit`, never by a plain run, so grading for a baseline
never dirties the tree. `run.py --no-log --check` re-grades and exits 1 on any
difference, one line per change (`recall.<fixture>.per_category.<CAT>: <committed>
-> <measured>`; summary lines as `- <line>` only in the committed file, `+ <line>`
only in the fresh run). Markers go to stderr: `[substrate-gap] wrote legacy-latest.json
— …` and `[substrate-gap] check: OK|DRIFT …`. Emit from a tree whose `fixtures/`
matches HEAD — an uncommitted fixture is graded like any other. The 16x30 language x
mechanism grid is a different view with a different schema: `results-latest.json` +
`COVERAGE.md`, owned by `matrix.py --emit/--check`. `test_run.py` pins the drift gate.

## Incremental transparency guard (`incremental_check.py`, A1.7)

Grading builds cold, so a stale parse cache can never move the matrix, and so the
matrix can never catch one either. `engine/tests/byte_identical.rs` proves
"incremental == clean" for one hand-written repo. This script proves it for
**every** fixture: `fixtures/` plus the `matrix/` probes, using `matrix.py`'s
`discover`. It closes no blind spot and flips no cell. It is a regression guard
for parser and extractor changes, and it passes on a healthy tree.

For each fixture it copies the directory into a temp dir and grades the copy
three times:

1. **cold**: grade.py's `build_graph`, untouched.
2. **fill**: through the parse cache (`generate(dir, True)`, or
   `generate_many(dirs, incremental=True)` for multi-dir fixtures).
3. **read**: the same again.

Pass 3 is compared against pass 1 on everything grade.py scores (node/edge
counts, per-category / per-kind / per-cell recall, forbid hits) and on a digest
of every node, edge and cell.

- **`DIVERGENT`**: the warm graph differs from the cold one.
- **`NOT-WARM`**: pass 3 reparsed a file, or printed no `[incremental]` marker.
  The comparison then proved nothing.

Either one, a grader error, or a `parse_cache.bin` left anywhere under this
directory exits 1.

```
[incremental-check] fixtures/py-calls: cold=3n/3e graph=… warm=3n/3e graph=… reused=1 reparsed=0 OK
[incremental-check] 180 fixtures, 0 divergent (cold vs warm)
```

- **Copies, never the fixture dirs.** `GLIA_NO_PERSIST=1` gates only the
  `.gmap` write, not `<repo>/.glia/graph/parse_cache.bin`. A sidecar left in
  a fixture would make later cold grades depend on the previous run. A copy
  grades the same as the in-tree fixture: all 180 fixtures matched at A1.7.
- **grade.py is not modified.** `grade_fixture` runs as-is, with its module
  global `build_graph` swapped for one call. This is the same seam `matrix.py`
  uses, so grade.py's hermetic path has no mode switch that could be set wrong.
- A wheel older than A1.4 has no `generate_many(incremental=)`. On such a wheel,
  multi-dir fixtures print `SKIP` and do not fail the run.
- A fixture made only of files that bypass the cache (yaml, Dockerfile,
  manifests) reads `reused=0 reparsed=0 OK`. It has no cached parse to go stale.
- `--only NAME` (repeatable) checks one fixture. `-v` echoes the engine's
  captured stderr.

**`--selftest [NAME]`** proves the guard can fail. It uses one single-dir
fixture (default `py-calls`):

1. Warm the cache.
2. Edit the file in the sidecar's first entry without changing its length or
   mtime, so the graph changes (one identifier's last letter).
3. **Honest cache:** pass 3 must reparse exactly that file and read `OK`. The
   cache keys on content, not size or mtime.
4. **Forged cache:** write back the pre-edit sidecar with the edited file's
   `content_hash` in that entry. The cache then serves the old parse for the new
   content, which must read `DIVERGENT`. The hash is copied from the honest
   sidecar written in step 3. The offset comes from bincode 1's layout: three
   length-prefixed strings, the entry count, the key, then the u64.

It ends with `[incremental-check] selftest PASS (honest miss OK, forged hit
DIVERGENT; …)`.

## Precision gates match exactly (wave 2 correction)

`forbid` uses **strict** identity — the normalised pattern must EQUAL the node's
name or its qname — while `expect_nodes` / `expect_edges` keep the lenient
substring matcher unless an entry opts into the strict one with `"exact": true`.

This asymmetry is deliberate. Leniency in a *recall* gate can only turn a miss
into a hit, so it is safe. Leniency in a *precision* gate turns it into a false
accusation: `forbid {to: "UserController"}` also matched the method whose qname
is `UserController::UserController::getUser`, and the harness reported a
violation against a correct graph (wave 2, `java-spring-composed`).

`test_grade.py` pins both directions — the strict matcher must not match an
ancestor qname segment, and must still catch a real violation — and the `exact`
recall entries: the lenient over-match, its exact rejection, the true edge found
by name and by qname, a non-boolean `exact` raising, and an entry without `exact`
grading as before.
