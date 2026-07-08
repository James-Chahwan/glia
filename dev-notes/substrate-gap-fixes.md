# Substrate-gap fixes — P1 (handoff v6)

Eval-driven fix plan for the blind spots `bench/substrate-gap` confirms. Each fix
is "done" when its eval cell flips `0.00 → 1.00` in `results.jsonl` (fired_on
marker). Order = value: Dart endpoint > Angular INJECTS > TS imports.

Do NOT rebuild the wheel while the mapping workflow grades; batch fixes after.

## Fix 1 — Dart client HTTP calls → ENDPOINT (highest value)

**Confirmed:** `dart-http-dio` HTTP_CALLS = 0.00; ENDPOINT node absent.

**Root cause (code-level):** `parsers/code/dart/src/lib.rs::scan_dart_routes`
(lines 544-557) text-scans `.<method>('/path')` for *every* receiver and emits a
**ROUTE**. So a client call `dio.get('/users/$id')` is mis-emitted as a phantom
server ROUTE (`GET /users/$id`), never as a client ENDPOINT. The AST walker
`collect_calls_in` (line 461) already classifies it as `Attribute{base:"dio",
name:"get"}` but does nothing HTTP-aware.

**Fix:** classify by receiver, emit ENDPOINT for client calls mirroring the TS
parser (parsers/code/typescript/src/lib.rs:1099-1130):
- Client receivers (`dio`, `_dio`, `http`, `_http`, `client`, `_client`,
  `httpClient`, `apiClient`, `api`, `_api`, `restClient`) → **ENDPOINT**:
  qname `endpoint:{METHOD}:{path}`, `node_kind::ENDPOINT`, an `ENDPOINT_HIT` cell
  (method, path, file_rel, line, col, confidence) + a `CALLS` edge from the
  enclosing method (available in `collect_calls_in` as `from`). Prefer doing this
  AST-side in `collect_calls_in` so `from` is real (text-scan has no enclosing id).
- Server receivers (`router`, `app`, `_router`, `r`, `handler`, shelf cascade
  `..get`) → **ROUTE** (unchanged).
- go_router `GoRoute(path:)` → ROUTE (unchanged).

**Path normalization subtlety:** dio uses Dart interpolation `'/users/$id'`; the
Go chi route is `/users/{id}`. The working TS control normalizes `/users/${…}` and
pairs it with `/users/{id}` — so `HttpStackResolver` already canonicalises path
params. Dart `$id` (no braces) must normalise to the SAME placeholder. Check
`HttpStackResolver` path-normalisation (graph/src/lib.rs ~1031) and either emit the
Dart ENDPOINT path pre-normalised or extend the normaliser to fold `$ident`.

**Eval key correction (also needed):** `fixtures/dart-http-dio/key.json`
`expect_edges` currently keys `from:"fetchUser"` — wrong. HTTP_CALLS is
ENDPOINT→ROUTE (see xstack dump: `'/users' -> '/users'`). Change to
`{from:"/users", to:"/users", category:"HTTP_CALLS"}`. The `ENDPOINT` node in
`expect_nodes` is the true extraction discriminator.

## Fix 2 — Angular constructor DI → INJECTS

**Confirmed:** `ts-angular-di` INJECTS = 0.00. `edge_category::INJECTS` (id 8) is
defined + weighted (`graph/src/lib.rs:2061`) but never emitted.

**Fix:** when a `@Component`/`@Injectable`/`@Directive` class constructor has typed
params (`constructor(private api: ApiService)`), emit `INJECTS` from the component
node → the service type node. Angular extraction is in
`parsers/code/extractors/src/angular.rs` (+ the base TS class parse). The param
TYPE resolves to a class/SERVICE node via the existing ref resolver. Careful: emit
from the framework COMPONENT/SERVICE node (not just the underlying CLASS) so the
liveness signal counts it. Verify activation isn't re-noised (INJECTS weight 2.0).

**Related:** the COMPONENT/SERVICE framework nodes carry `path=None` (dump showed
it) — same class as the endpoint-no-path P1 item. Consider carrying the class span
onto the framework node so liveness/locate work.

## Fix 3 — TS/JS import resolution → IMPORTS edges (broadest blast radius)

**Confirmed:** `ts-imports` IMPORTS = 0.00 for the entire TS/JS/Angular/React/Vue
(and Dart/Swift/C++/Solidity) family.

**Root cause:** `engine/src/lib.rs:412` — the catch-all arm wires
`build_typescript(repo, parses, |_,_| None)`. The `resolve_source` closure that
maps an import specifier (`./util`, `../a/b`, tsconfig `@app/*`) to an in-repo
module qname is hardcoded to `None`, so every TS import is treated as external →
no `IMPORTS` edge ever. `build_typescript` (graph/src/lib.rs:107) accepts a real
resolver; the engine just never supplies one. Imports currently survive only as
`Symbol.imports` cells (not traversable by impact/activate).

**Fix:** supply a real `resolve_source(from_module, specifier) -> Option<qname>`:
- Relative specifiers (`./x`, `../y/z`): resolve against `from_module`'s dir,
  normalise `.`/`..`, map to the module qname convention the TS builder uses
  (dump showed module qnames are the bare file stem, e.g. `util`, `main`). Try
  `index` resolution for dir imports.
- Bare/scoped specifiers (`@angular/core`, `lodash`): external → `None` (correct).
- tsconfig `paths`/`baseUrl` aliases: later; start with relative (covers most
  intra-repo edges).
This is the highest-blast-radius fix (whole frontend becomes traversable) but also
the one most likely to change graph size / determinism — gate on the byte-identical
determinism test AND re-run the full substrate matrix to catch regressions.

## Verification protocol (all fixes)

1. `maturin develop` in `py/` (rebuild the wheel).
2. `cd bench/substrate-gap && python3 run.py` — target cell flips to 1.00; NO other
   cell regresses (the matrix is the regression guard).
3. `cargo test` in the workspace (parser + graph + engine suites).
4. Determinism: the bytes-on-disk byte-identical gate must stay green (Fix 3 esp.).
