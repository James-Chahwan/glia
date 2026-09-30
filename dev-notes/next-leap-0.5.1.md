# glia 0.5.1 — the catch-up leap, and the train after it

Scoped 2026-09-30 at `2170ff8` (v0.5.0, tagged and on PyPI 2026-09-20). This file is the agreed scope for the
0.5.1 spec run; `dev-notes/next-leap-0.5.0.md` §6 / §7.6 and the memories `project_post_050_thinking_list` /
`project_research_horizons_2026_09_19` are its sources. Every "exists" / "missing" below was verified against code
by a read-only scope pass (6 agents, 2026-09-30); line numbers are at `2170ff8` and drift, so find symbols by name.

## 0. Decisions (verbatim)

- 2026-09-19: *"Let's only take these as 0.5.1 and all of five can be breaking changes if need be this will be our
  catch up leap"* / *"I'd say tier 1 and 2 can be 5.1 / Tier 3 should be broken up into most useful batches / Like
  5.2 / Top 3 bets then / 5.3 / Everything but dbt data linage"* / *"1,2,3 all have to be 0.5.4 then like a
  refinement and here's proof again people lol"* / *"A for sure is also for calibration"*.
- 2026-09-20: TS family built inside the engine pool — *"It can join 0.5.1"*.
- 2026-09-30: *"hey lets scope these specs we planned"* / *"engram side started now it should finish in time while
  we scope and speckit the work and research parts we need to"*. Answers:
  - format = leap packets (this doc + packets / waves JSON on the wave-runner), not spec-kit files;
  - **version: keep the name 0.5.1, consumers pin exactly** (breaks allowed; each handoff states the exact pin);
  - **0.5.2 = four bets**: `glia watch`, the Datalog rule layer, cross-repo identity, Engram PPR memory;
  - spec run: *"Yes, run it"*;
  - `decide/` (Laya in Rust, a–e): *"can we do a-e in 0.5.4"*;
  - hosted Jev: *"i thought we would make it from our own work"* → no third-party API; `glia judge` runs on our own
    `decide/` model and moves to 0.5.4 with it;
  - GQL / Cypher read: folded into 0.5.2's Datalog layer (one query surface);
  - compression: *"Rescope + CODE spans"* — EVIDENCE interning + CODE cells as spans + parse-cache lz4, one
    FORMAT_VERSION 3 bump; WebGraph adjacency dropped (topology is ~5% of a shard).

## 1. The train

| release | contents | packets |
|---|---|---|
| **0.5.1** catch-up | §2: dogfood fixes, language / HTTP backlog, matrix probes, new answers, algorithms, external inputs, format 3 | specced now |
| **0.5.2** bets | `glia watch`, Datalog rule layer (+ GQL front-end), cross-repo identity, Engram PPR memory — §4 | research now, packets after 0.5.1 lands |
| **0.5.3** scale | Stack Graphs-style incremental resolution, build-target graphs (first real domain candidate) — §5 | later |
| **0.5.4** proof | precision rate on real repos, agent before/after, big-repo per-phase timings, calibrated confidence, `decide/` a–e, `glia judge` on our own model — §6 | later |

## 2. 0.5.1 scope (≈ 29–42k LOC; 0.5.0 was ≈ 59k over 251 packets)

Packet ids: `C<group>.<n>[letter]`; `C0.*` is wave 0 (id allocation, module / CLI / py slots). Breaks are allowed
when declared (kinds as in 0.5.0: none, qname_shape, node_id, edge_removal, cell_value, api_signature, format,
cli_output, out_of_repo).

### CA — dogfood fixes and coverage concretes (≈ 2.3–3.5k)

| item | today (verified) | LOC | break |
|---|---|---|---|
| CA-1 Go calls inside closures | `collect_calls_in` skips `func_literal` (parsers/code/go/src/lib.rs ~1268); quokka `repository_provider.go:49` `UserRepository()` has 0 out-edges. Route-handler literals keep LA.18d's HANDLED_BY; `func_literal_handlers` is filled by `collect_routes_in`, which runs after calls — reorder or pre-mark | 140–270 | contents |
| CA-2 Go receivers from a call's return type | chained call → `ComplexReceiver` (go lib.rs ~1291), dropped by `resolve_calls` (graph/src/calls.rs ~217) and `receiver_field` (~741); Go records field types only — no params, locals or return types (CodeNav has `field_types` / `local_types`). Add parser return types (package qualifier kept) + locals + params, a build-time `CodeNav.return_types` (serialised in engine/src/cache.rs), a GoPackages arm resolving the inner call then the type in the callee's module; `unique_global_function` (calls.rs ~666) has no kind filter (quokka has FUNCTION and STRUCT `UserRepository`). quokka: 64 chained + 63 `x := Services.XRepository()` sites | 400–600 | contents |
| CA-3 implementors over-match on one-method Go interfaces | root cause `emit_go_implicit_implements` (graph/src/build.rs ~1071) matches method names only; grpc-go `Closable` (internal/xds/bootstrap/tlscreds/bundle_ext_test.go:53). For one-method sets: same package or an importing package (`GoPackages.import_path`); test-file interfaces match test files only; optional signature text from CODE cells. Also: method-level pairs from a Medium Go edge are pushed Strong (calls.rs ~973, ~994). Keep the pinned `[iface]` marker (build.rs ~2555); add a new line | 150–300 | contents |
| CA-4 Go Mongo collections through generic wrappers | `.Collection(` is ALREADY a needle (extractors/src/data_entities.rs ~2300); quokka's only call is inside `NewCollection[T]` (turps/Services/Repositories/collection.go:19-20). Infer wrappers (a function passing a param straight into `.Collection(param)`) and mint through engine/src/external/wrappers.rs (today it returns None without an overlay file, ~298-306; `Stanza` borrows `WrapperDecl`). New ORIGIN provenance value, no registry id | 300–450 | contents |
| CA-5 patterns judges something on real repos | `handler>(no effect)` counts toward population size against the 75% share (engine/src/patterns.rs ~74-80, ~396-407). Needs CA-1 + CA-4 for quokka; receiver-method route handlers (Kina `h.List` → `Attribute{h,List}`, go lib.rs ~2448) 30–60; sighted-only verdict with blind reported + per-package keying 60–140; stale doc patterns.rs ~94 (Go cross-package calls, fixed by LA.13b Go package resolution) | 90–200 | api (additive) |
| CA-6 Kotlin parameter / `val` receiver types | the last blind fixture `kotlin-ktor: CALLS` is a parameter-typed receiver (`store: ItemStore` → `store.all()`), not a Ktor client call; record via `record_local_type` (generic `receiver_type` already reads locals, calls.rs ~831-850). Plus Ktor-client ENDPOINTs (documented caveat engine/src/coverage.rs ~230, pinned ~625) + a fixture 150–250 | 210–370 | contents |
| CA-7 TS family inside the engine pool | built last on the calling thread (engine/src/build/lang_build.rs ~130-158) for ordering only; `par_map_owned` (engine/src/parallel.rs) preserves order with TS as the last item; no blocker found. `[parallel] … <g> language graphs` +1 when TS present | 20–50 | none |
| CA-8 grade.py exact match | substring `_node_matches` for recall (grade.py ~153, ~346); strict `_node_matches_exact` exists (~224). Add `exact` to EDGE_FIELDS / NODE_FIELDS, README, test_grade.py. Fix README ~115 vs ~250 contradiction on forbid matching | 30–60 | none |
| CA-9 per-phase build timers | `[parallel]` markers print counts, not times; no timing code outside tests. Pulled forward from 0.5.4 so 0.5.2's `glia watch` design rests on measured phase costs: walk / parse / language-build / resolvers / post-passes / persist | 150–350 | none |

### CB — language and HTTP / RPC / event backlog (≈ 6.5k; all 34 items still open at 2170ff8)

HTTP / RPC / events (≈ 2.9k): a `.graphqls` routing (extract.rs ~51, ~25); b contract yaml sniff reads 64 lines
(extractors/src/contracts.rs `SNIFF_LINES` ~179, ~40); c Go routers on a struct field (c1 ~120) and prefix propagation
through fields / params (c2 ~320, **identity break**: those ROUTE qnames gain their prefix); d tRPC `createCaller`
(~180); e host narrowing for ws / graphql / grpc / RPC (share http.rs `build_service_alias_index` / `narrow_by_host`,
~950, edge removal; reuse ENDPOINT_HIT `host` or one cell id); f in-process events between nested projects that are
one process (project dependency closure from DEPENDS_ON, ~400); g verb-fallback event nodes (extractors/eventbus.rs
`verb_name`; read a real name or mint nothing, ~400, node removal); h TS decorator-line markers (widen the method
span, ~90, cell_value break — or ~150 in anchor.rs with no break); i strawberry / graphene field resolvers (~270,
camelCase decision). C / C++ (≈ 1.65k): C1 extern "C", anonymous namespaces, templates, nested types, unions (~300,
narrow identity break); C2 `.hh/.hxx/.inl/.ipp` routing (~60, narrow identity break); C3 `collect_include` LA.40 class
(~45); C4 include search paths incl. `compile_commands.json` (~450); C5 implicit-this / static / object calls (~410);
C6 free functions through a header prototype (~250); C7 `using namespace` / `using ns::f` (~140). Swift (≈ 0.8k): S1
implicit-self via a new `build_swift` hook (~160); S2 `a + f(x)` (~55); S3 init / deinit / subscript / computed
bodies (~230); S4 field types (~160); S5 static calls on same-module types (~140); S6 `@testable import` (~40). Dart
(≈ 0.67k): D1 constructors / factories / operators (~180, qname decision); D2 abstract members (~80); D3 enum
constants (~80); D4 unnamed extensions (~140, qname decision); D5 top-level initialisers (~110); D6 `as/show/hide`
imports (~80). Namespaces (≈ 0.43k): N1 shared namespace PACKAGE takes the last-merged file as parent AND resolves
members against the wrong file's `using` / `use` (graph/src/build.rs `merge_nav` ~327, calls.rs `enclosing_module`
~686; ~220); N2 PHP `use` forms (php/src/lib.rs `collect_use` ~1309; an AST alias table exists ~1661; ~150); N3 Ruby
`collect_require` from the class qname (~55). The scope agent's file-disjoint batching (19 packets, 3 waves + close)
is the starting point.

### CC — new answers on the 0.5.0 primitives (≈ 4.3–6.8k)

Shared prerequisites first: (A) `diff_impact_vs_rev` and `tests_for_rev` each call `graph_delta_vs_rev` (a full
before + after build each) — add entry points taking one `&RevDelta`, like `pattern_conformance_delta` (~40–60);
(B) one `pub(crate)` ATTN / FAIL cell reader (only gaps.rs `pair_counts` parses ATTN today, ~50); (C) `check` labels
every forbid_edge violation FACT (check.rs ~442) while `why` tiers the same edges DERIVED / HEURISTIC — expose
`why::tier_of` as `pub(crate)` and use it (a correctness fix). Items:

| item | LOC | tier | break |
|---|---|---|---|
| CC-1 context packing to N tokens: per-node fidelity ladder in projection-text (full / preview / stub / qname), greedy score-per-token loop with re-render, a manifest; seeds via `find_nodes`. No tokenizer crate in the lock — decide counting (bytes/4 vs a crate) | 550–900 | seeds FACT/HEURISTIC, neighbours DERIVED | none |
| CC-2 reflexion models as `check` v2: `[[component]] name, paths`, optional `[[layer]]`, `kind="allow"`; component matrix → convergences / divergences / absences / unmapped (Datalog in 0.5.2 can compile these later) | 650–950 | per-edge `tier_of`; absence FACT + caveats | contents (new constraint kinds; old readers skip) |
| CC-3 `glia review`: one RevDelta → changed nodes, impact, tests, a tier per added / removed edge, `check` both sides (new vs resolved violations), markdown renderer; exit 1 on a new violation | 600–900 | passes through | none |
| CC-4 stale feature flags (report only, no rewriting): flag reads move from module to function (secrets_flags.rs `scan` keeps no offsets; extract.rs passes no sites; anchor.rs ~1696 pins it; 80–150); dead / undefined / quiet (blame older than T) / single-site | 450–700 | FACT / DERIVED / HEURISTIC per rule | contents |
| CC-5 contract-break check vs a git rev: pair old vs new schema copies, OpenAPI-vs-OpenAPI evolution rules, orphaned clients; (+150–250 for `--with` clients). Language-level public API (cargo-semver-checks parity) is out: no visibility / signature cell | 500–1,150 | FACT same qname, DERIVED via moves | none |
| CC-6 predictive test selection: 6a re-rank `tests_for` by failed-last-run / seed-on-failing-trace / co-change, co-change-only rows HEURISTIC, `--limit` (200–350); 6b a rolling window of runs (snapshot version + FAIL payload, 300–500, format break of the snapshot) | 500–850 | HEURISTIC for new signals | none / snapshot format |
| CC-7 hotspots: churn rank × centrality rank (global PageRank via `activate`, one adjacency index), module and symbol rows, no bare score | 450–650 | HEURISTIC | none |
| CC-8 co-change suggestions (ROSE): directional confidence from CO_CHANGES + ATTN, static-link flag; pairwise, plus multi-antecedent rules from commits.jsonl (+150–250) | 350–800 | HEURISTIC | none |
| CC-9 patterns out of experimental: written promotion criterion, blind count per population, rename with aliases for one release; needs CA-1 / CA-4 / CA-5 measured on quokka / lapse / Kina first | 100–200 | HEURISTIC | api (aliased) |

### CD — graph algorithms and the store (≈ 11–18k)

| item | LOC | notes |
|---|---|---|
| CD-1 communities + structured summaries | 1,900–3,000 | seeded Leiden (label-propagation fallback) in `activation::algo::community`, integer-scaled weights, inline SplitMix64; per-community kinds / top members / entries / services / sinks / inter-community links; prose stays with the consumer; computed at query time (no cell) |
| CD-2 service-split suggestions | 1,800–2,700 | on CD-1: Stoer–Wagner on the module / community quotient + Dinic s-t; cut edges located, shared-write entities, cycles between proposed services, diff vs `arch` |
| CD-3 suspected edges (link prediction) | 1,000–1,750 | new gaps category `suspected_edge`: learned (kind, category, kind) triples, Adamic–Adar / resource-allocation over the CSR, channel-token similarity, co-change boost, ready-to-paste overlay stanza; always HEURISTIC; the overlay loop (CE-3) verifies. No TransE / RotatE |
| CD-4 hubs + duplicate flows | 1,750–2,850 | O(V+E) per-category degree, fan-in / fan-out / cross-service labels, fixed-iteration HITS; duplicate flows exact by fingerprint (DERIVED) + near by seeded MinHash / LSH (HEURISTIC) |
| CD-5 time-travel graph | 1,500–2,500 | walk N revs chaining `algo::delta` through the move map into per-edge `[valid_from, invalid_at)` intervals in a sidecar + an `as_of` view; manifest `rev` field with `#[serde(default)]`; one incremental build per rev |
| CD-6 WASM gmap reader | 950–1,700 | owned-bytes backing for `MmapContainer` (store/src/container.rs; 3 callers), filesystem-free layout read, store + traversal + PPR only (answers live in the engine). First: `graph/Cargo.toml` lists 3 tree-sitter crates under `[dependencies]` that graph/src never uses — move to dev-deps (~3 lines). Verify rkyv layout cross-target |
| CD-7 FORMAT_VERSION 3: size | 1,300–2,400 | EVIDENCE string interning (5.46 MB of a 37.6 MB shard, 400–800), CODE cells as spans into source instead of copies (~23 MB, 800–1,500, cell_value break; consumers that read CODE text must fetch the span), lz4_flex on `parse_cache.bin` (38 MB, ~100). One format bump; old layouts rebuild through LC.8's path. zstd is C and would block wasm — use pure-Rust codecs |

### CE — external inputs and caches (≈ 4.8–7.8k)

| item | LOC | notes |
|---|---|---|
| CE-1 SCIP ingest (FACT) | 1,250–2,000 | `scip` 0.10.0 crate (protobuf =3.7.2 exact pin, pure Rust) decoded CLI-side only; `glia scip import` writes `.glia/scip-snapshot/*.jsonl` + meta with per-document source hash; new stage `engine/src/external/scip.rs`; bind by position (Definition occurrence start line → node, checked by name), never by parsing symbol strings; add a `scip` EVIDENCE stage (evidence.rs ~51 is a closed list); one EVIDENCE cell per edge, so "confirming" needs a rule; SCIP has no call role → CALLS vs USES is HEURISTIC; `py` depends on `snapshots`, so protobuf must not land there |
| CE-2 shared / remote cache | 900–1,950 | per-entry content address H(BUILD_STAMP, repo identity, lang, path, module form, go context, content) with blake3 / sha2; `ParseCache` import / export; `glia cache pull|push <dir|https>` (ureq is in the lock) so the build stays offline; trust model (untrusted bincode is in SECURITY.md scope); GC; optional whole-layout cache keyed by git tree + stamp + `.glia` fingerprint (+300–500). Keep the store read-mostly (shared-gmap writes are on the Not-doing list) |
| CE-3 the overlay loop as a command | 1,100–1,800 | `glia overlay propose|try|accept` + an agent skill for the LLM step; `BuildOptions` gains a candidate-overlay path (today `overlay: bool`, fixed path); per-category deltas (only 3 orphan categories measured today); per-stanza leave-one-out on the incremental cache; a TOML writer; stable `GapRow` ids; run on a worktree copy of quokka, never quokka itself |
| CE-4 doc adapters | 1,560–2,520 | seams first: `Page` / `record_from_page` are Confluence-shaped, and `write_snapshot` overwrites the one `manifest.jsonl` (a second source wipes the first; merge by kind, 60–120). Notion (750–1,100), MediaWiki API (650–1,100, html5ever), local git-wiki dir (100–200). An HTML sitemap crawler stays gated |

### CF — the coverage matrix probes (≈ 2.5k tier 1; ≈ 11k all)

Unknown = no key.json claims the cell (`matrix.py`); there is deliberately no not-applicable list, so terraform (30)
and solidity (26) stay unknown forever, and Kotlin is not a matrix row. A probe is scaffold.py → ≤2 files per dir →
`grade.py --dump` → key → `matrix.py --emit`; the 102 existing probes average 66 lines / 3.4 files. Tier 1: python 2,
go 8, ts 7, java 9, csharp 12 (38 cells, ~2.5k). Then ruby / php / rust / scala / dart / elixir (130 cells, ~8.6k).
Every probe that grades none / partial becomes an extractor item (it goes to the backlog, not into its probe packet).
Open for the integrator: whether to add an explicit not-applicable list and a Kotlin row (a matrix vocabulary change).

### Cross-cutting findings (every group reads these)

- **Any `Cargo.lock` change moves PARSER_STAMP** (`stamp/build.rs` hashes the lock; optional deps are written to it),
  so a new dependency invalidates every parse cache. That is fine for 0.5.1 (breaks allowed) but must be declared;
  heavy optional tools belong in an excluded workspace with its own lock (engram-export's pattern).
- `py` depends on `glia-snapshots` — anything added there ships in the wheel.
- Next free registry ids: node_kind 50, edge_category 37, cell_type 26. Only `C0.1` allocates.
- Each new engine / py / CLI module needs its facade slot; `C0.*` declares them all first so later packets edit only
  their own files (0.5.0's wave-0 lesson).
- Stale docs to fix in the packet that touches them: engine/src/patterns.rs ~94; bench/substrate-gap/README.md ~115
  vs ~250; results.jsonl ends at 0.4.18 (legacy-latest.json is the file of record).
- Not glia work, flagged: the bench-era SWE-bench driver `run_instance.py` (3,286 LOC) sits untracked in gitignored
  `bench/latent/out` — 0.5.4's agent before/after needs it; back it up somewhere tracked.

## 3. What 0.5.1 does not take

- `decide/` a–e and `glia judge` → 0.5.4 (§6). No hosted Jev.
- GQL / Cypher read → 0.5.2 Datalog (§4).
- WebGraph adjacency compression — dropped.
- Precision audit, agent before/after, big-repo proof, calibrated confidence → 0.5.4.
- The gated list is unchanged (`SECURITY.md`): taint / value flow, CVE joins, security zones, sitemap crawling.

## 4. 0.5.2 — the four bets (research in this run; packets after 0.5.1 lands)

| bet | 0.5.0 gives | glia LOC | biggest unknown |
|---|---|---|---|
| Engram PPR memory | engram-export v6 (5,353 LOC); domain-free PPR (`activate`, ~117-line `ppr_vector`) | 400–900 (+1.2–2k in Engram) | whether PPR beats Engram's 2-hop spreading on its shootout / quokka_recall tests; gated on Engram applying v6 (running now in its own session). engram-core may share only the gmap, so Engram copies ~120 LOC of PPR |
| `glia watch` | ParseCache get / iter / diff, LC.11 write-if-changed, unchanged-shard skip on persist, `graph_delta` / `located_delta`, `detect_moves` | 800–1,500 pragmatic; 3–6k Salsa-style | post-cache phase costs on big repos (CA-9 measures them); incremental must stay byte_identical; TODO.md: no central daemon → foreground CLI or embeddable library |
| Datalog rule layer (+ GQL front-end) | `check` (forbid_edge direct only; `invariant` unchecked), `ConstraintKind`, overlay `[[edge]]` / `[[constraint]]`, `CODE_PASSES` | 2–3.5k (+ GQL front-end) | a runtime semi-naive evaluator (ascent / crepe compile rules at build time; overlay.toml rules need runtime) — datafrog-style or hand-rolled; derived predicates on demand (additive) vs stored (content) |
| Cross-repo identity | `merge_layouts` ("no cross-repo node dedupe"), `PackageResolver` links same-qname PACKAGE_DEP, `IdentityIndex` | 1–2.5k (union-find) | which equivalences beyond packages (proto messages, contract ops, vendored copies); merge nodes vs `SAME_AS` edges; alias table persisted = format |

## 5. 0.5.3

Stack Graphs-style incremental resolution (3–6k; github/stack-graphs archived 2025-09-09; one partial-path model for
20 languages is the unknown); build-target graphs (2–3k; Bazel macros / `select()` defeat a static parse, so
`bazel query` through a `snapshots/` crate; likely the first real non-code domain).

## 6. 0.5.4 — refinement and proof

Precision rate (real-repo edge sampler + labeller, 500–900; label budget ~20–40 strata × ~100); agent before/after
(repo-graph/bench/run_bench.py A/B harness, revive the SWE-bench driver 1–2k; small AND large model); big-repo
per-phase timings (CA-9's timers); calibrated confidence keyed by emitter:rule (400–800, needs the precision labels);
`decide/` a–e (candle ModernBERT is present in candle-transformers 0.11.0; Laya's checkpoint loading unverified;
excluded workspace with its own lock; `tokenizers` with `default-features=false, features=["fancy-regex"]`);
`glia judge` on our own model (HEURISTIC cells, payload-hash cache, never in a build).

## 7. Spec run

One workflow, like 0.5.0's runs: per group a spec agent writes draft packets and an adversarial verifier writes the
final ones (CA, CB, CC, CD, CE, CF); two research agents write the 0.5.2 research notes (§4); one integrator
allocates ids, writes the `C0.*` slot packets, wires cross-group dependencies and checks completeness. Packets are
validated by `dev-notes/wave-runner/validate_packets.py`. Outputs merge into `dev-notes/leap-051-packets.json`; the
0.5.1 implementing brief is derived from `shared_brief_leap.md` once the packets exist.

### 7.1 Result (workflow wf_9c9de08b-7ee, 2026-09-30: 15 agents, 0 errors, ~7.3M subagent tokens)

**149 packets, 42,701 LOC, 12 waves** after the rulings in 7.2 (W0 = 7 serial slot / dependency packets, then 11
parallel waves of 26 / 26 / 26 / 20 / 12 / 11 / 7 / 6 / 4 / 2 / 2 at the runner's cap of 26 packets per wave).
`dev-notes/leap-051-packets.json` holds the packets, deps_patch, split decisions and the integration report;
`python3 dev-notes/wave-runner/schedule_051.py` reproduces the schedule. Critical path, after W0 (10 packets): CA.1 Go
closure calls → CA.2a / CA.2b Go return-type receivers → CA.3a / CA.3b implementors filter → CA.5a / CA.5b patterns on
real repos → CC.12a / CC.12b patterns promotion → CZ.2 docs close-out.

| group | packets | LOC |
|---|---|---|
| C0 wave 0: activation / engine / graph / py / cli slots + one dependency commit (Cargo.lock once) | 7 | 290 |
| CA dogfood | 13 | 3,085 |
| CB language / HTTP backlog | 26 | 7,380 |
| CC answers | 26 | 6,840 |
| CD algorithms + FORMAT_VERSION 3 (WASM reader dropped) | 26 | 8,302 |
| CE inputs | 22 | 7,780 |
| CF matrix probes (tiers 1 + 2) | 27 | 8,464 |
| CZ release docs close-out (README `## CLI`, CLAUDE.md; took 23 README claimants off the waves) | 2 | 560 |

- **No registry ids allocated.** Every group reused existing kinds, categories and cells; the next free ids stay
  node 50 / edge 37 / cell 26.
- **Everything in section 2 is covered.** 137 GAP (no owner) rows are in `integration_report.gaps_no_owner`, 111 of
  them CF cells that a probe predicts will grade none / partial (extractor backlog). The ones most worth an owner:
  (a) Go imports of the repository-root package are never recorded; (b) `why::tier_of` marks Medium Go implicit
  IMPLEMENTS as FACT; (c) glia's own queues.rs / eventbus.rs mint phantom QUEUE_* / EVENT_* nodes from their needle
  tables; (d) PHP symbol imports sharing a short name bind nothing; (e) pyo3 `overlay_propose / try / accept` for
  repo-graph.
- **Runner (done 2026-09-30):** `gen_wave.py --release 051 N out.js` and `closeout.py --release 051 N <run-id>` load
  `leap-051-packets.json` through `schedule_051.py` (at most 26 packets per wave; still 12 waves), fold follow-ups
  naming `C*.*` ids into `leap-051-corrections.json`, record LANDED in `schedule_051.py` (`--verify`), and brief
  agents with `shared_brief_051.md`. closeout now reads the matrix grid from the counts (CF.13a / CF.13b change it).
- **Release step with no packet:** the 0.5.1 handoff docs (repo-graph, neuropil, Engram — each with the exact pin and
  every declared break), collected from the packets' `breaking` blocks and the agents' results, as 0.5.0's LG.5 /
  LG.14 did.
- **0.5.2 research** is in `dev-notes/research-0.5.2/` (R1-datalog, R2-watch, R2-identity, R2-engram).

### 7.2 Decisions (James, 2026-09-30)

Answered "sure" / "yeap good reccomendation" / "yes" to the recommendations:
1. CF.13a: a not-applicable list of the 28 structural cells (solidity / terraform); the 4 "no client" cells stay unknown.
2. CF.13b: a Kotlin matrix row (480 -> 510).
3. Ratify every qname / NodeId break listed below. Change to CB.3: an event keyed by a constant is keyed by its
   resolved literal through the const table, falling back to the constant path, so there is no second identity
   change later.
4. CB.4: widen a decorated TS method's POSITION / CODE to its first decorator (cell_value break).
5. CB.21 / CB.24: reuse ENDPOINT_HIT for client hosts; no CLIENT_HOST cell.
6. CE-1: a hand-written SCIP decoder (no `scip` crate, no protobuf pin, no lock change); SCIP calls tiered FACT.
7. CE-2: CLI-only transport, keep the whole-layout cache CE.2d, the trust model as specced.
8. CE.4a: redact secrets in Confluence CODE cells.
9. CC.12a: promotion thresholds C1 (at least 2 of 3 repos judged) and C2 (blind handlers at most 25% of a
   population).
10. CC.3 takes the `why::tier_of` confidence fix (+~30 LOC).
11. WASM gmap reader dropped: *"oh don't bother on that small one thing then"*. CD.6b / CD.6c are gone (no consumer;
    wasm64 is tier 3 in Rust, with no prebuilt std); CD.6a stays as graph dependency hygiene. Existing equivalents
    for reading a graph outside Rust: the glia-py wheel, CLI `--json`, `glia analyze --format json`, the repo-graph MCP.

Applied to leap-051-packets.json the same day: CB.3 split into CB.3a (extractor, constant-path fallback) + CB.3b (a
post-build fold to the resolved literal through the const table, reusing the queue-topic rule); CB.4 widen-only;
CE.1c hand-written SCIP decoder only; CE.2 split so all transport is CLI-only (CE.2c signed store + dir store, CE.2e
HTTPS store) with a gate of zero ureq in the engine / glia-py trees; CC.3 takes the `tier_of` fix (a graph-stage edge
below Strong confidence tiers DERIVED); CF.13a trimmed to 28 cells, CF.13b's counts follow.

The questions as asked:

CF.13a not-applicable matrix cells; CF.13b a Kotlin matrix row; the qname / NodeId breaks to ratify; CB.4 decorator
span; CB.21 / CB.24 client-host cell; CE-1 SCIP decoder + tier; CE-2 cache placement, whole-layout cache, trust
model; CE.4a Confluence CODE redaction; CC.12a promotion thresholds; the `tier_of` fix in CC.3; the wasm32 target
install for CD.6c.

### 7.3 Late addition: group CG, the Engram session's findings (2026-10-01)

The Engram session ran a 480-query recall battery on glia 0.5.0 exports of quokka-stack and Kina.
- Three of its gaps were already packets. Their exact cases became corrections:
  - calls inside returned Go closures → CA.1;
  - locals typed from a call's return → CA.2b;
  - name-only Go IMPLEMENTS → CA.3b.
- The rest became group CG (wf_506df58f-7ab spec → verify; 6 packets, 1,190 LOC; still 12 waves; W1 unchanged):
  - CG.1: TS arrow-function class fields as METHOD nodes.
  - CG.2a: path-based test_fixture provenance (e2e / cypress / fixtures / testdata / __mocks__ / test_*.py ...; glia +2,305 nodes, Kina +50).
  - CG.2b: `engram-export --exclude-path`.
  - CG.3: doc CODE text no longer capped at 500 bytes, plus a fenced-span guard.
  - CG.4a / CG.4b: HTTP endpoints whose literal host matches no service are marked `external` and kept out of pairing.
- CG ids sort after W1's capped 26 packets, so adding them could not reorder a landed wave.
- The three doc mentions Engram named are not a linker gap: those files are outside doc ingestion by design (`include_doc`). Widening ingestion is a question for James.
- New gaps with no owner:
  - TS `abstract class` declarations mint no CLASS / METHOD nodes;
  - TS URL-builder wrappers collapse to `<unresolved>` endpoints.
