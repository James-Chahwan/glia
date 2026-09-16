# Review 2026-09-15 — open tasks, four reported issues, coverage matrix, SDD direction

Produced by two read-only workflows (17 agents, 0 errors): a 6-topic investigation +
adversarial critic, and a 5-family × 16-language coverage inventory with a grep
verify pass on every `none` cell. All claims below carry file:line; the critic
refuted or corrected 9 investigator claims (noted inline). LOC not time.

## 0. State

- `main` = v0.4.18, released to PyPI (6 artifacts), nothing unpushed. Only untracked
  file: `dev-notes/repo-graph-handoff-v0.4.18.md` (commit it).
- **Everything the planning docs list as open is DONE** except the items in §1:
  substrate-gap Fix 1/2/3 (4e68254), handoff-v6 P1–P3, WP-0..WP-J, audit-2026-06-10
  Tier 1, py_smoke (1e2de00), emoji-slug collisions (d8c392a), v0.4.17 PyPI gap (6071d3b).
  Phase 2/3 per-file delta is superseded by WP-D; nothing is blocked on neuropil.
- Stale prose to fix in one docs commit: `glia-build-plan.md:46/129`, `BLINDSPOTS.md`
  FINAL ("3 cosmetics remain" — cleared), `CLAUDE.md:82` (rust/py/Cargo.toml),
  `issues_surfacing_now.md` (bench cycle 2.0 file, not a glia task list).
- `bench/substrate-gap/results.jsonl` is gitignored (.gitignore:36): the 48→0 proof
  is machine-local. Commit a `results-latest.json`.

## 1. Genuinely open engine items (pre-existing)

| LOC | item | where |
|---|---|---|
| 50 | `glia docs sync --include/--exclude` | cli/src/main.rs:199 |
| 150 | `glia docs push --markdown` (markdown→storage inverse) | doc-sources/src/confluence.rs:49 |
| 100 | doc-linker precision tiers (qualified names → Strong) | engine/src/lib.rs:531 |
| 120 | intra-workspace import leak (audit #12) | code-domain/src/lib.rs:504 |
| 70 | ParseCache pub iter/diff, BTreeMap, per-pid tmp | engine/src/cache.rs:73,175 |
| 40 | generate_many threads a per-repo ParseCache (audit #14) | engine/src/lib.rs:125 |
| 40 | share walker gating with is_gmap_stale (audit #17) | store/src/lib.rs:957 |
| 25 | escape_json control chars + PyGraph.parse_errors (audit #15/16) | py/src/lib.rs:335,366 |
| 60 | php instance-var dispatch (only sub-1.0 eval cell, CALLS 0.5) | parsers/code/php |

**Hazard nobody had listed:** `CACHE_VERSION = CARGO_PKG_VERSION` (engine/src/cache.rs:22,153)
and `is_gmap_stale` (store/src/lib.rs:951) key on the release version only. Every
parser/extractor change merged without a workspace bump is invisible to incremental
builds — which are the pyo3/MCP default (py/src/lib.rs:353). The bench uses
`generate(dir, False)` so eval cells flip while real repos keep pre-fix nodes.
Fix: fold a `PARSER_STAMP` into CACHE_VERSION (or bump per parser batch). Land first.

## 2. The four reported issues — verdicts

### #2 Scoping (walker) — confirmed, ~430 LOC total
- always_region (engine/src/lib.rs:1003) has no bin/obj/.vs/TestResults; obj/ generated
  `*.AssemblyInfo.cs`/`*.g.cs` are parsed as authored C#.
- `is_hard_skip` (engine:995) drops a `.git` FILE by name before the is_dir check, so a
  submodule or linked worktree is walked INTO the parent RepoId: duplicate symbols,
  pick_primary flapping, Go imports misclassified (root go.mod only, engine:164-175).
  The `.git`-file `gitdir:` parser already exists at cli/src/main.rs:1160 (install-hooks only).
- `load_gitignore_dirs` (engine:1019): root file only, no globs/negation/anchoring, final
  component matched at ANY depth (a root `/build` collapses every `build` dir).
- No sub-project notion anywhere (grep = 0). `resolve_file` (graph:2452) matches by
  basename → `utils.py`/`index.ts` collide across sub-projects (a precision bug, not UX).
- Fix v1 (200 LOC): shared `code_domain::walk_gating` used by engine walker AND
  store::is_gmap_stale; bin/obj collapse gated on sibling `*.csproj|*.sln|*.fsproj|Directory.Build.props`
  or `obj/project.assets.json`; nested `.git` → REGION anchor provenance submodule|worktree|nested_repo,
  never descend; marker `[walk] collapsed N regions (always=A gitignore=G bundle=B dotnet=K nested=M)`.
- Then scope filter (120 LOC): `scope: Option<&str>` on blast_radius_by_qname /
  resolve_signal_located (filter SEEDS pre-PPR) / governing_docs + `find_located`; must run
  BEFORE `truncate(top_k)` (engine:1803, 1932); CLI `--scope`, pyo3 `scope=None`.
- Full gitignore via the `ignore` crate MATCHER (110 LOC) is the only item needing network
  (crate absent from Cargo.lock and the offline registry).
- Then `project_roots` pre-pass (manifest roots: package.json/go.mod/Cargo.toml/pyproject/
  pom/gradle/*.csproj/pubspec/mix.exs/…), ~300-600 LOC — the vocabulary #3 needs for monorepos.

### #3 Architecture summary — raw material confirmed, nothing groups it, ~330 LOC
- cross_edges + mechanism labelling exist (engine cross_stack_trace :1828); the only
  aggregator, cli `print_mermaid` (:367-408), is dead: `analyze` is single-repo so every
  cross edge is from==to and skipped (:390-392), and it labels repos by u64 hash.
- Repo human label is lost at `RepoId::from_canonical`; needs `repo_labels` on GenerateResult.
- **Critic correction:** keying on RepoId alone renders a generate_one monorepo (quokka-stack)
  as ONE service with ZERO links. v1 must key on the top-level path segment when repos==1;
  ENDPOINT/ROUTE nav parents are None (typescript:1243, go:1358) so their dir comes from the
  ENDPOINT_HIT/ROUTE_METHOD cell `file`.
- Design: `graph::cross_links()` (BTreeMap grouping by from/to/category/channel, 55 LOC) →
  engine `service_map()` with per-service summary (150) → pyo3 `service_map()` (25) →
  CLI `glia arch <repo> --with .. [--json|--mermaid]` (80) → tests (75). Marker
  `[arch] S services, L links`. orient() embedding is repo-graph's P4 job.

### #4 C# Refit / .proto / Kafka — confirmed with corrections, ~430 LOC + fixtures
- **Correction to the issue text:** `.proto` IS walked (engine:783) and `.cs` IS routed to
  the queue + grpc extractors (engine:890-900). The blindness is needle/emitter-side.
- Kafka: the resolver-path node DOES carry the topic literal (`queue_producer:<topic>`,
  queues.rs:177); but `extract_topic_near` is first-occurrence per file (:197) and
  `extract_first_string_literal` reads only an inline quoted first arg after one `(` (:202-222).
  So `ProduceAsync("orders"`, kafkajs `send({topic:'x'})`, Java `new ProducerRecord<>("x"`,
  Go `kafka.Message{Topic:"x"}` ALL collapse to the topic-agnostic tag node
  `queue_producer:kafka`, which the resolver pairs all-to-all (graph:1420). C# is
  "accidentally partial" (producer-only, gate-dependent). Java Spring Kafka has zero needles.
- Refit: zero ENDPOINT emission in C# (Refit or HttpClient). ASP.NET ROUTEs are relative
  templates without a leading `/` so `index_route_node` (graph:1228) drops them — the
  whole C# server side is unreachable from every client language. Also class-level
  `check_route_attrs(type_node)` (csharp:218) text-scans the whole class body, so every
  action route is emitted twice (HANDLED_BY method AND controller class).
- Proto: the proto branch (engine:318-334) bypasses `stash_synthetic_parse` (:2016), so no
  MODULE node, GRPC_SERVICE parent dangles, only an INTENT cell (no POSITION). gRPC client
  needles require the service name to end in `Service`/`Svc` (grpc.rs:98-106): `Greeter`,
  `Auth`, `Users` are blind in every language. No server-impl detection in any language.
- Order: (1) ASP.NET route composition + leading `/` + minimal-API top-level + dedupe
  class scan, ~90 LOC; (2) Refit + HttpClient ENDPOINT via `push_client_endpoint`, ~130;
  (3) queue extractor correctness for ALL languages + C#/Java needles, ~140;
  (4) proto through stash_synthetic_parse + package/options + rpc METHOD nodes + gated
  generic client needles, ~75. Fixtures: csharp-aspnet-composed, csharp-refit-http,
  csharp-httpclient-http, xcut-queue-csharp-kafka, xcut-queue-multi-topic,
  xcut-queue-java-spring, xcut-grpc-csharp. Record their 0.00 baselines BEFORE fixing.

### #6 Producer/consumer schema match — design only, after #4, ~260 LOC report-only v1
- Queue nodes carry `cells: []` (queues.rs:152/:183): no message TYPE, no POSITION.
- Append `cell_type::MESSAGE_TYPE = 17` (+ `ALL` entry or pyo3 can't decode; no store
  FORMAT_VERSION bump — store is v1, GMAP_FORMAT_VERSION=5 is engram-core's).
- `extract_message_type_near`: last type arg of `Message<K,V>`/`ConsumerRecord<K,V>`/
  `KafkaTemplate<K,V>`/`IProducer<K,V>`/`IConsumer<K,V>`, Go `&pb.X{`.
- engine `message_contracts()` → rows {topic, producer repo/qname/type, consumer …, status
  match|mismatch|unknown}; CLI `glia contracts`, pyo3 `contracts()`. SHARES_SCHEMA edge
  deferred: `SharedSchemaResolver` walks MODULE children only (graph:1586) and every C# type
  sits under a namespace PACKAGE node (csharp:86-128) → structurally blind to C#.

## 3. Coverage matrix (what glia extracts today, 16 languages × 30 mechanisms)

Status: ● full = extraction + identifying literal captured + routed; ◐ partial = pattern
exists but literal not captured / one library / phantom-only; · none.

```
LEGEND ● full  ◐ partial (pattern exists but literal not captured / one lib / not routed)  · none
           http_cl http_sr   kafka    amqp sqs/sns  pubsub azure_s    nats   redis    mqtt   taskq    grpc graphql      ws eventbu      db    migr  config secrets   flags    cron cli_def cli_inv   calls imports injects    impl   tests service subproj
python           ●       ●       ◐       ◐       ·       ·       ·       ◐       ◐       ◐       ◐       ◐       ◐       ·       ◐       ◐       ·       ●       ·       ·       ◐       ◐       ◐       ●       ●       ·       ●       ●       ◐       ·
go               ●       ●       ◐       ·       ·       ·       ·       ◐       ·       ·       ·       ◐       ◐       ◐       ·       ◐       ·       ◐       ·       ·       ·       ◐       ◐       ●       ●       ·       ·       ◐       ●       ·
typescript       ●       ◐       ◐       ◐       ◐       ·       ·       ◐       ◐       ◐       ◐       ◐       ●       ◐       ◐       ◐       ·       ●       ·       ·       ◐       ◐       ◐       ◐       ●       ◐       ◐       ◐       ●       ·
java             ·       ◐       ◐       ·       ·       ·       ·       ◐       ◐       ◐       ·       ◐       ·       ◐       ◐       ◐       ·       ◐       ·       ·       ◐       ·       ·       ·       ·       ·       ·       ·       ·       ·
csharp           ·       ◐       ◐       ·       ·       ·       ·       ◐       ◐       ·       ·       ◐       ·       ◐       ·       ◐       ·       ·       ·       ·       ·       ·       ◐       ◐       ●       ◐       ●       ·       ·       ·
ruby             ·       ◐       ◐       ·       ◐       ·       ·       ◐       ◐       ◐       ◐       ·       ·       ◐       ◐       ◐       ·       ◐       ·       ·       ·       ·       ◐       ◐       ◐       ·       ·       ◐       ·       ·
php              ·       ●       ·       ·       ·       ·       ·       ◐       ◐       ◐       ·       ◐       ◐       ·       ·       ◐       ·       ◐       ·       ·       ·       ·       ◐       ◐       ●       ·       ·       ·       ·       ·
swift            ●       ◐       ·       ·       ·       ·       ·       ·       ·       ◐       ·       ·       ·       ·       ·       ◐       ·       ◐       ·       ·       ·       ·       ·       ◐       ·       ·       ·       ·       ·       ·
c_cpp            ·       ·       ◐       ·       ·       ·       ·       ·       ◐       ◐       ·       ·       ·       ◐       ·       ◐       ·       ●       ·       ·       ·       ·       ◐       ◐       ◐       ·       ·       ◐       ·       ·
scala            ·       ◐       ◐       ·       ·       ·       ·       ◐       ◐       ·       ·       ◐       ·       ·       ·       ◐       ·       ◐       ·       ·       ◐       ·       ·       ◐       ●       ·       ●       ·       ·       ·
clojure          ·       ◐       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ◐       ·       ·       ·       ·       ·       ·       ·       ◐       ●       ·       ·       ◐       ·       ·
dart             ●       ◐       ·       ·       ·       ·       ·       ·       ·       ◐       ·       ◐       ◐       ◐       ·       ◐       ·       ·       ·       ·       ◐       ·       ·       ◐       ◐       ·       ·       ◐       ·       ·
elixir           ·       ●       ·       ·       ·       ·       ·       ·       ·       ·       ◐       ·       ◐       ◐       ·       ◐       ·       ·       ·       ·       ·       ·       ·       ◐       ●       ·       ·       ◐       ·       ·
rust             ·       ◐       ◐       ◐       ·       ·       ·       ◐       ◐       ◐       ·       ·       ·       ·       ·       ◐       ·       ●       ·       ·       ·       ◐       ●       ●       ●       ·       ◐       ·       ◐       ·
solidity         ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·                       ●       ·       ·       ·       ·       ·       ·       ·       ·       ◐       ◐       ·       ◐       ·       ·       ·
terraform        ·       ·       ◐       ◐       ◐       ◐       ◐       ·       ·       ·       ·       ·                       ·       ·       ·       ·       ·       ·       ·       ·       ·       ·       ◐       ·       ·       ·       ·       ◐

PER-MECHANISM  full/partial/none across 16 langs:
  http_cli  ● 5 ◐ 0 ·11
  http_srv  ● 4 ◐ 9 · 3
  kafka     ● 0 ◐10 · 6
  amqp      ● 0 ◐ 4 ·12
  sqs/sns   ● 0 ◐ 3 ·13
  pubsub    ● 0 ◐ 1 ·15
  azure_sb  ● 0 ◐ 1 ·15
  nats      ● 0 ◐ 9 · 7
  redis     ● 0 ◐ 9 · 7
  mqtt      ● 0 ◐ 9 · 7
  taskq     ● 0 ◐ 4 ·12
  grpc      ● 0 ◐ 8 · 8
  graphql   ● 1 ◐ 5 · 8
  ws        ● 0 ◐ 8 · 6
  eventbus  ● 1 ◐ 4 ·11
  db        ● 0 ◐14 · 2
  migr      ● 0 ◐ 0 ·16
  config    ● 4 ◐ 6 · 6
  secrets   ● 0 ◐ 0 ·16
  flags     ● 0 ◐ 0 ·16
  cron      ● 0 ◐ 5 ·11
  cli_def   ● 0 ◐ 4 ·12
  cli_inv   ● 1 ◐ 7 · 8
  calls     ● 3 ◐11 · 2
  imports   ● 9 ◐ 5 · 2
  injects   ● 0 ◐ 2 ·14
  impl      ● 3 ◐ 3 ·10
  tests     ● 1 ◐ 7 · 8
  service   ● 2 ◐ 2 ·12
  subproj   ● 0 ◐ 1 ·15

PER-LANGUAGE  full/partial/none across 30 mechanisms:
  python      ● 7 ◐14 · 9
  go          ● 5 ◐10 ·15
  typescript  ● 5 ◐19 · 6
  java        ● 0 ◐11 ·19
  csharp      ● 2 ◐10 ·18
  ruby        ● 0 ◐15 ·15
  php         ● 2 ◐ 9 ·19
  swift       ● 1 ◐ 5 ·24
```

Headline: **0 of 30 mechanisms is full across languages**; only http_client (5),
http_server (4), imports (9), config (4), calls (3), impl (3) have any ● at all.
Six mechanisms are at zero everywhere: migrations, secrets, feature flags,
pubsub/azure (◐1), sub-project. Kotlin is routed through the Java grammar (engine:771)
with no kotlin crate — files "parse" but emit almost nothing (false confidence).

### Contract-file and project-shape rows (single cells)

| row | status | note |
|---|---|---|
| OpenAPI/Swagger file | none | yaml reaches the yaml branch (engine:199-235) with no `openapi:` sniff; `.json` dropped |
| OpenAPI annotations in code | none (py partial via FastAPI) | Swashbuckle/springdoc/NestJS swagger/rswag/swaggo unseen |
| AsyncAPI | none | pairs naturally with QueueStackResolver's `queue_*:<channel>` index |
| GraphQL SDL (.graphql/.gql) | none | not in detect_language; smallest fix in the family |
| Avro/.avsc, protobuf `message` | none | not walked; generated classes become plain CLASS nodes |
| JSON Schema / Pact | none | `.json` dropped at walker; Pact is the most ROUTE-pairable contract after OpenAPI |
| .proto | partial | service pairable; no rpc/message nodes, no package, no POSITION |
| tRPC / Connect / Twirp | none / go partial | tRPC = the largest full-stack blind spot for Next.js/T3 repos |
| service-hostname pairing (compose/k8s/env values) | none | the missing join that disambiguates `/users` across services |
| base-URL / proxy awareness | none | `${environment.apiUrl}/users` → `/{}/users` never pairs; the DEFAULT client shape |
| topic/URL constants and env folding | none | `const TOPIC = "x"` → tag node; caps real-world queue recall near zero |
| git submodule / nested repo | none | see #2 |
| git worktree | partial | detector exists only in install-hooks |
| .gitignore semantics | partial | root, dir-name, no globs |
| interface→impl traversal | partial | class-level IMPLEMENTS only; no method-level edge; interface-typed receiver CALLS never resolve |

### Resolver-level bugs (each one is multi-language)

- **HTTP:** no `ANY` method wildcard (Django, Rails resources, Spring @RequestMapping,
  C# [Route], Go HandleFunc all dead weight, 15 LOC); prefix strip endpoint-side only and
  hardcoded; `${base}` prefix unpairable (60 LOC); `<int:id>`, `*path` placeholders not
  folded (30); TS/Dart skip `url_to_path` (host/query kept); client-side NAV routes
  (react-router/Angular Router/vue-router/GoRoute) enter the ROUTE index and pair with
  same-repo fetches; ts_routes mints phantom server ROUTEs from `this.http.get('/x')`;
  NO ROUTE or ENDPOINT in any language carries POSITION so locate_node returns
  file=None for every HTTP node (120 LOC fallback to ENDPOINT_HIT/ROUTE_METHOD json).
- **Queues:** first-occurrence per file; inline-literal-only arg parsing; framework-tag
  fallback is pairable (all-to-all fan-out); task-queue identity is the receiver/class not
  the string arg (`.delay('x')` emits a phantom topic); no annotation consumers
  (@KafkaListener/@RabbitListener/@SqsListener/@Processor); cloud brokers zero in all 16;
  Redis/MQTT/NATS land in eventbus.rs as Weak EVENT_* nodes; same-repo NodeId collision
  for two files on one topic; no wildcard/normalisation; kind-blind key ('events','jobs').
- **RPC:** GraphQL resolver matches by bidirectional substring (`usequery`⊃`query`);
  WebSocket wildcard pairs any unparsed client to any server; gRPC index keyed on bare
  service name (package collisions); EventBusResolver string-exact so type-keyed buses
  (Spring events, MediatR, Solidity events) never pair; marker nodes parented to MODULE
  with no edge to the enclosing method (trace enters/exits at file boundary).
- **Data/infra:** DATA_ENTITY key inconsistent per language (Java bare `User`, Ruby
  `data_entity:sql:User`, Python table name) so polyglot stacks share zero entities;
  DbResolver ignores DATA_SOURCE kinds; CLI declarations are subcommands but invocations
  are binaries (never pair); Terraform qnames disjoint from iac.rs; k8s multi-doc CronJob
  keeps only the last schedule; ConfigResolver discards env VALUES; only 6 eval cells guard
  the whole family.
- **Structural:** no receiver-type inference anywhere (`this.svc.m()` — the dominant call
  shape in every DI codebase — dropped, 400 LOC); Attribute resolution ignores PACKAGE bases
  (all Elixir cross-module calls unresolved, 25 LOC); TS/Solidity heritage edges target
  fabricated NodeIds; Dart heritage downgraded to none by the verifier (bare-name NodeIds);
  TESTS is snake_case-only (zero for Java/Kotlin/C#/PHP/Swift/Scala CamelCase); tsconfig
  `paths` aliases unresolved; `extra_hook` seam never used.

## 4. SDD / spec-kit assist — plan (~335 LOC first slice)

Substrate: hidden dirs are walked, so `.specify/`, `specs/`, `.kiro/` files are read then
dropped by `include_doc` (engine:1217-1225). Same-stem spec files across features collide
on qname `docs::spec::overview` (FileDocSource sets no container). chunk_markdown splits on
`#`/`##` only and `cap_prose(500)` truncates tasks.md before the linker. The linker rejects
`/` and `.` (no file-path or route mentions) and drops unresolved mentions silently.

Slice 1 (accepted, in this order): 1a detect_sdd (speckit iff `.specify/` or
`specs/\d{3}-slug/spec.md`; kiro; openspec) + per-feature DOC_SPACE container (65 LOC) →
1b OpenAPI ops from `contracts/*.yaml|json` → one DOC_SECTION per op with ORIGIN
`contract:{method,path}` via a zero-dep indentation scanner (65) → 1c `link_contract_routes`
post-pass (make `build_route_index` + `lookup_route_with_prefix_strip` pub; DOCUMENTS
contract→ROUTE Strong exact / Medium prefix-stripped, 40) → 1d `spec_status` primitive
{feature, method, path, status implemented|declared_missing|undeclared, route, file, line}
+ CLI `glia spec-status` + pyo3 (95) → 1e bench/sdd-speckit fixture + check.sh (70).
Marker `[sdd] framework=<f> features=<n> contracts=<m> routes_linked=<k>`.

Slice 2: tasks.md items as DOC_SECTIONs (parsed before cap_prose, 150) + `sdd_report()`
(53). Slice 3: repo-scoped governing docs (constitution/steering) as an additive `scope`
field on LocatedNode — NOT a fan-in edge (25). Slice 4: `spec_for_diff` (110; needs the
resolve(diff) basename→suffix change — owner call, alters repo-graph find(diff)).
Slice 5: `plan_context(feature, budget_chars)` (90). The same OpenAPI parser serves
AsyncAPI→queue pairing and the repo's own `openapi.yaml`/Swashbuckle output.

## 5. LLM-assisted overlay mode (proposal)

Keep the build deterministic; let a model feed it.
1. `glia gaps <repo>`: unpaired ENDPOINTs/ROUTEs, tag-only queue nodes, `<unresolved>`
   endpoint sinks, dead-flagged exports, `${base}` paths — the prompt material.
2. The agent writes `.glia/overlay.toml` (checked in, reviewable): client receivers,
   wrapper-call mappings, topic/base-URL constant aliases, route-prefix map per
   sub-project, explicit edges. Locked IDs only; no new kinds.
3. Engine applies it deterministically at build with ORIGIN `overlay:llm` + Weak
   confidence; primitives can hide them (`--no-overlay`). Generalises the existing
   `# @service` opt-in (services.rs).
4. Accept only if pairings rise: rebuild, re-run coverage, marker
   `[overlay] N rules, +M edges, orphans K→J`.
Glia owns gaps report + overlay schema + applier (pyo3/CLI); repo-graph owns handing gaps
to the agent and writing the file back (the MCP server cannot call a model).

## 6. Merged ranked plan (next sessions, LOC)

Batch A — substrate correctness that lifts many cells at once
1. PARSER_STAMP in CACHE_VERSION + ParseCache determinism + per-repo cache in generate_many (110)
2. Queue extractor correctness for ALL languages: multi-occurrence, object/kwarg/struct
   topic forms, case-insensitive gates, annotation consumers, tag fallback unpairable+Weak,
   C#/Java/PHP/Go casing rows, POSITION on queue nodes (~250) + ~10 fixtures asserting topic qnames
3. HTTP resolver: ANY wildcard, `${base}` fold + suffix fallback, `<…>`/`*` placeholders,
   TS/Dart url_to_path, NAV routes out of the index, ROUTE/ENDPOINT locate fallback (~275)
4. ASP.NET route composition + dedupe class scan (90), then Spring/Symfony/FastAPI/Flask/
   Rails/Laravel prefix composition through a shared `code_domain::endpoint::join_path` (~260)
5. C# client ENDPOINT Refit+HttpClient (130), then ruby/php/rust/elixir/scala clients (~80 each)
6. Proto via stash_synthetic_parse + package/options + rpc METHOD nodes + data-driven client
   needles from the repo's own proto service set (75+140), gRPC server-impl detection (260)
7. Attribute resolution for PACKAGE bases (25) — unlocks every Elixir/C# static call

Batch B — project shape and the answer surfaces
8. Walker scoping v1 (200) → scope filter on primitives (120) → project_roots pre-pass (300-600)
9. service_map + `glia arch` (330), keyed on top-level dir when repos==1
10. Contract files: OpenAPI/Swagger + AsyncAPI + GraphQL SDL/.avsc routing (320+180) = SDD slice 1
11. service-hostname pairing from compose/k8s/env values (200) + constant/env folding (80)
12. message_contracts v1 report (260)

Batch C — structural depth
13. Receiver-type inference (400), INJECTS `inject()`/Lombok/[FromServices] (350),
    TS/Solidity/Rust heritage as UnresolvedRef + Dart fix (220), TESTS CamelCase (150),
    tsconfig paths (120), method-level interface→impl edge (150)
14. Kotlin crate on tree-sitter-kotlin (600-900); until then a universal Kotlin caveat row (40)
15. Secrets/flags as CONFIG_KEY flavors (220), migrations routing (120), DATA_ENTITY key
    normalisation (80), ORM coverage (600), CLI key normalisation (140)
16. LLM overlay mode (§5)

Hazards: record every new fixture's 0.00 baseline before fixing; `cargo clean -p
repo-graph-engine -p repo-graph-py` before `maturin build` (stale .so); rank 4 changes
every C# ROUTE qname (update csharp lib.rs:878 test + csharp-aspnet key.json in the same
commit); rank 2 may DROP raw QUEUE_FLOWS counts before they rise (grade the multi-topic
fixture, not the count); scope filter before truncate; never filter trace hops; new
DOCUMENTS edges are blast carry edges (result change in SDD repos); MESSAGE_TYPE needs its
`ALL` entry; the `ignore` crate is the only network-requiring item.
