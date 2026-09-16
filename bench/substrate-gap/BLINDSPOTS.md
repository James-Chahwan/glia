# Substrate blind-spot map — v1 (engine 0.4.16, 2026-07-08)

Full `(framework × edge-category)` extraction-recall audit — 24 analyzers, 69
fixtures, graded by `run.py` against hand-enumerated ground truth. Raw matrix +
history in `results.jsonl`. Verdicts: **extracted** (1.0), **partial** (edge
emitted but wrong granularity/target), **blind** (0.0, no edge).

The gaps are not isolated cells — they cluster into **9 systemic patterns**.
Independently re-verified: Go IMPORTS, Scala CALLS, Java IMPORTS all confirmed
blind by direct `--dump` (only DEFINES edges emitted).

## What WORKS (the positive space — don't regress these)
CALLS: Python, Go, TS, Ruby, Rust, Swift, C++, Clojure · IMPORTS: Python, Ruby ·
HANDLED_BY: flask, chi, Spring, ASP.NET · HTTP_CALLS: TS, Angular, Vue ·
INHERITS_FROM: Python, Solidity · STATE_VAR: Solidity · Cross-stack resolvers:
gRPC, WebSocket, EventBus all pair correctly; GraphQL pairs at node level.

## The 9 patterns (ranked by cross-boundary value per handoff thesis)

### Pattern A — Client HTTP-call extraction is TypeScript-ONLY  ★ highest value
Client calls become `ENDPOINT` nodes (→ `HttpStackResolver` pairs to server
`ROUTE` → `HTTP_CALLS`) **only in the TS parser**. Every other language's client
is blind:
| stack | HTTP_CALLS | why |
|---|---|---|
| TS fetch/axios, Angular HttpClient, Vue axios | ✓ 1.0 | `try_detect_endpoint` in typescript/src/lib.rs |
| **Python `requests`** | ⊘ 0.0 | no ENDPOINT emitted |
| **Go `net/http`** | ⊘ 0.0 | no ENDPOINT emitted |
| **Java `RestTemplate`/`WebClient`** | ⊘ 0.0 | no ENDPOINT emitted |
| **Swift `URLSession`** | ⊘ 0.0 | no ENDPOINT emitted |
| **Dart `dio`/`http`** | ⊘ 0.0 | mis-emits phantom **ROUTE** from the URL literal |
| React fetch in `useEffect` | ⊘ 0.0 | TS extractor doesn't descend into nested arrow callbacks |
The resolver is fine; extraction is the hole. HTTP is the dominant cross-service
link → fixing this flips 5 languages. **On quokka (Go+Angular+Flutter) this is
exactly the Flutter blindness the handoff named.**

### Pattern B — IMPORTS edges wire for Python + Ruby only  ★ huge blast radius
Two sub-causes:
- **B1 — TS-family None stub:** `engine/src/lib.rs:412` passes
  `build_typescript(.., |_,_| None)`. Blind: TS/JS/Angular/React/Vue + Dart/Swift/
  C++ (all on the `_` arm). Imports survive only as `Symbol.imports` *cells*.
- **B2 — dotted/Go format mismatch:** the resolver *runs* but qname lookups miss.
  Blind: Java, C#, Scala, Elixir, Clojure, PHP (dotted resolver) + Go
  (`resolve_imports_go` exists but import-path→module-qname mapping misses) + Rust
  (emits module-level, not symbol-level).
Imports are how `impact`/`activate` traverse — but handoff P1-bullet-4 warns import
edges also *over-connect* impact. **Fix B must pair with edge-category-aware
traversal (down-weight IMPORTS in blast radius).**

### Pattern C — CALLS wiring weak outside the primary languages
Blind: Java (intra-class method call unresolved), C# (zero CALLS), Scala (only
DEFINES), Elixir (only DEFINES). Partial: PHP (self/free calls work; instance
dispatch blind). INHERITS_FROM/IMPLEMENTS in Java/C# emit but target renders `'?'`
(superclass/interface unresolved). The generic call/ref resolver isn't fed
correctly by these parsers (callsites not emitted, or qname mismatch).

### Pattern D — Route→handler HANDLED_BY missing for half the frameworks
Works: flask, chi, Spring (buggy paths — see G), ASP.NET. Blind: **Rails** (no
routes.rb extractor → no ROUTE node at all), **Laravel** (ROUTE node but no
HANDLED_BY edge), **Phoenix** (ROUTE node but `emit_phoenix_route` never wires the
action), **axum** (ROUTE + handler nodes present, no HANDLED_BY edge).

### Pattern E — DI / INJECTS universally blind
Angular constructor DI, Java Spring `@Autowired`, C# ctor DI — all ⊘. `INJECTS`
(cat 8) is defined + weighted (`graph/src/lib.rs:2061`) but never emitted anywhere.
Directly causes liveness to false-flag `@Injectable`/`@Service` beans dead.

### Pattern F — Edge granularity / anchoring
Edge of the right category emitted but at the wrong node: ACCESSES_DATA anchored to
MODULE/`main` not the query-issuing function (Python, Go); TESTS anchored file→file
not test-fn→target (Python, TS); Rust IMPORTS to module not the imported symbol.

### Pattern G — Node-kind misclassification
Solidity `event` → FUNCTION (not EVENT_EMITTER, blocks EVENT_FLOWS) · Java
`@Entity` → CLASS (not DATA_ENTITY) · Terraform resource → STRUCT (not
INFRA_RESOURCE) · Elixir `def` → FUNCTION, `defmodule` → PACKAGE · React
null-returning component → FUNCTION (not COMPONENT) · custom hook → both HOOK and
COMPOSABLE (duplicate).

### Pattern H — Cross-cutting extractor casing / granularity
QUEUE: `queues.rs` needles are lowercase JS/Py/Ruby (`nc.publish`); Go's exported
`nc.Publish` never matches → Go queues blind (resolver is fine — proven by a
lowercase probe pairing 1.0). Topic parse also breaks on `pattern("topic",..)`.
GRAPHQL: resolver names are type-level (`@Query(`→"Query"), operation names are
field-level (`getUser`) → never string-match. gRPC/WS/EventBus all work.

### Pattern I — Resolver path normalization
Angular absolute URL `http://host/users` extracts ENDPOINT but HTTP_CALLS pairing
drops to 0 (host prefix not stripped before route match) — affects ALL stacks, not
just Angular.

## Recommended fix order (value = cross-boundary-ness × language frequency)

**Tier 1 — cross-boundary (the graph's whole reason to exist):**
1. Pattern A: shared client-HTTP-call → ENDPOINT extraction for Python/Go/Java/
   Swift/Dart (start Dart per handoff; build as the reusable pattern). +
   Pattern I host-normalization.
2. Pattern E: INJECTS for Angular/Java/C# (unblocks liveness).

**Tier 2 — traversal completeness:**
3. Pattern B: IMPORTS (B1 TS stub + B2 dotted/Go) — paired with edge-category-aware
   `impact` (down-weight IMPORTS) so it doesn't over-connect.
4. Pattern C: CALLS for Java/C#/Scala/Elixir.
5. Pattern D: HANDLED_BY for axum/Laravel/Phoenix/Rails.

**Tier 3 — correctness/granularity:** Patterns F, G, H, I remainder.

Each fix is proven when its eval cell flips 0→1 in `results.jsonl` (fired_on
marker); the full matrix is the regression guard. Rebuild the wheel — `cargo clean -p
repo-graph-engine -p repo-graph-py && maturin build && pip install --force-reinstall
target/wheels/<wheel>`. There is no venv on this machine, so the `develop` flow does
not apply, and the `clean` is load-bearing — see the stale-`.so` warning in the
PROGRESS LOG below. Then `run.py` + `cargo test` +
byte-identical determinism gate after each.

---

## FINAL — 2026-07-08: ALL edge-category blind spots eliminated (48 → 0)

Every `(framework × edge-category)` cell now passes (recall > 0). Full workspace
tests pass except the pre-existing `py_smoke` (fails identically on clean HEAD);
byte-identical determinism gate green throughout.

**Re-verified 2026-09-16 on engine 0.4.18** (`python3 run.py --no-log`, 79 fixtures at
the wave-1 mark — the count grows as the 2026-09-16 wave programme lands):
`BLIND SPOTS (recall 0.00): 0` · `PARTIAL (0 < recall < 1): 0` · `FORBID VIOLATIONS: 0`
· `MISSING CELLS: 0` · `GRADER ERRORS: 0`, and **zero `expect_nodes` misses** — so the
three node-kind label residues recorded here in July are cleared. The last cell that
sat below 1.00 (`php-laravel · CALLS` — `$greeter->greet()` after
`$greeter = new Greeter()`) reached 1.00 in `0dcb452` "feat(php): bind local
`$x = new Cls()` / typed-param receivers to their class"; `grade.py
fixtures/php-laravel-calls` now reports `CALLS 2/2 (1.00)`.

Two notes for anyone reading the v1 body above:
- Its `file:line` anchors (e.g. `engine/src/lib.rs:412`) **predate the module split** —
  `engine/src/lib.rs` and `graph/src/lib.rs` are now thin facades. Grep the symbol name.
- `key.json`'s field vocabulary is **frozen**, and `grade.py` raises on any unknown
  field. The single authority is `README.md` → "Fixture shape — the FROZEN key.json
  vocabulary"; it is deliberately not restated here.

Waves (all verified, determinism-green, regression-guarded by the full matrix):
- **A client HTTP ×5** (dart/go/py/java/swift), **E INJECTS ×3** (angular/java/csharp),
  **B imports ×13** (TS-family merge + resolvers; dotted/go/dart/c_cpp tail + relative
  resolvers; rust module-level; swift N/A), **C CALLS ×5** (java/csharp/scala/solidity
  /elixir), **D HANDLED_BY ×4** (rust/php/elixir/ruby + Rails routes.rb extractor),
  **Tier-3**: terraform INFRA_RESOURCE + INFRA_REFERENCES + DEPENDS_ON, solidity
  EVENT_EMITTER + EVENT_FLOWS, java @Entity→DATA_ENTITY + ACCESSES_DATA, java/scala/
  csharp heritage-as-ref, xcut-graphql field-name, xcut-queue Go casing + topic-parse,
  react nested-useEffect fetch, py/go/ruby ACCESSES_DATA fn-anchored, py fn-level TESTS.

### PROGRESS LOG — 2026-07-08 (mid-run snapshots below, superseded by the FINAL above)
Blind 48 → 21 → 12 → 4 → 1 → 0.

Rebuild note: `maturin build` can reuse a stale `.so` — run `cargo clean -p
repo-graph-engine -p repo-graph-py` before it, or mtime-check the installed `.so`.

**FIXED (27 cells), all verified (determinism byte-gate green; only pre-existing
py_smoke fails; TS-family cells regression-checked):**
- **A · client HTTP → ENDPOINT (5):** dart, go, python, java, swift HTTP_CALLS. Shared
  `code_domain::endpoint::push_client_endpoint` + `url_to_path`; per-parser
  `try_detect_<lang>_endpoint`. (React stays blind — fetch nested in useEffect.)
- **E · INJECTS / DI (3):** angular, java, csharp. `UnresolvedRef{category:INJECTS}` +
  resolve_refs global-by-name fallback.
- **B · imports (10):** TS-family (ts/react/vue/angular) via the TS-family graph merge
  + un-stubbed `resolve_ts_source`; dotted (java/csharp/scala/php) + go via import
  tail-name / module-tail fallbacks.
- **C · CALLS (4):** java, csharp, scala, solidity (bare intra-type call → SelfMethod).
- **D · route→handler HANDLED_BY (4):** rust, php, elixir, ruby (+ a Rails routes.rb
  ROUTE extractor). `UnresolvedRef{category:HANDLED_BY}`.

**Reusable substrate built (beyond the cells):** shared client-endpoint helper +
`url_to_path` (Pattern I host-strip); resolve_refs global-by-name fallback now covers
INJECTS/INHERITS_FROM/IMPLEMENTS (+HANDLED_BY); `build_symbol_table` indexes PACKAGE
(namespace) members; `unique_global_function` id-dedup + new `unique_global_module`;
**engine builds TS-family (ts/angular/react/vue) as ONE graph** (cross-tag DI+imports);
`resolve_ts_source` un-stubs engine:412; dotted/Go import tail fallbacks.

**REMAINING (21) — Tier-3 / tail, lower cross-boundary value:**
- **Node-kind (G):** terraform resource→INFRA_RESOURCE (2); solidity event→EVENT_EMITTER
  (1); java @Entity→DATA_ENTITY (1).
- **Heritage target (3):** java INHERITS_FROM+IMPLEMENTS (2), scala (1) — parser emits a
  name-derived phantom edge; needs emit-as-`UnresolvedRef` (graph fallback for
  INHERITS_FROM/IMPLEMENTS is ALREADY landed, awaiting the parser half — currently inert).
- **Extractor casing (H):** xcut-queue Go capitalized needles (1); xcut-graphql
  field-level name match (1).
- **Granularity (F):** ACCESSES_DATA anchored to module not fn (go/py/java/ruby, 4);
  py TESTS module-level (1).
- **Hard IMPORTS tails:** dart/swift/c_cpp (`_`-arm stubbed resolver, 3); clojure/elixir
  (`from_module` ns-vs-file-stem qname mismatch, parser-side, 2); rust module-vs-symbol
  (fixture-key, 1).
- **elixir CALLS (1):** functions under a `defmodule` PACKAGE; resolve_calls' Bare walks
  to the file MODULE not the PACKAGE (same class as C# INJECTS was).
- **react HTTP_CALLS (1):** TS call-extractor doesn't descend into nested arrow callbacks.

The Tier-3 node-kind/extractor/heritage batch was scoped for workflow `wp91wixpu` but the
agent pool hit its session limit (resets ~3:50am Australia/Sydney); resume it next —
parser-side + additive.
