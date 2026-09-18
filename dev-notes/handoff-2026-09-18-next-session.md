# glia — handoff for the next session (2026-09-18)

You are picking up the **glia** Rust engine (`/home/ivy/Code/glia`): it parses source in 16 languages, builds one
cross-language graph, stores it as a zero-copy `.gmap`, and answers questions over it (blast radius, cross-service
trace, signal resolution, architecture). It is the engine behind the `repo-graph` MCP wrapper, which lives in a
**separate repo and a separate session**.

## 0. Read first

1. `CLAUDE.md` (architecture — current, includes the post-split module layout), `WORKFLOW.md` (session rhythm; run
   `/orient`), `CODE_RULES.md`. The native memory index loads automatically; its "glia — live state" section is current.
2. **State right now**

| | |
|---|---|
| HEAD | `5280cee` on `main`, tree clean |
| version | `0.4.18` — **unchanged**; last release on PyPI is 0.4.18 |
| unpushed | **142 commits** since `origin/main` (= v0.4.18) — nothing pushed, nothing published |
| release gate | **James.** Never push, tag, bump or publish without an explicit go. Walk him through files + validation first. |
| tests | `cargo test --workspace` **1,056 passing / 0 failing** |
| extraction eval | `bench/substrate-gap`: **140 fixtures**, blind spots / missing nodes / partial / forbid / missing cells / grader errors **all 0** |
| coverage matrix | derived + committed: **146 / 480 cells measured — 97 full, 29 partial, 20 none** |

3. **Separate repos, separate sessions.** glia / repo-graph / neuropil / Engram are four repos. Act on glia only; hand
   anything else over as a note. repo-graph's side of this work is in `dev-notes/repo-graph-handoff-programme-2026-09.md`.

## 1. What just happened

A 19-wave, 105-packet programme implemented sections 1–3 of `dev-notes/review-2026-09-15-coverage-and-issues.md`
(the 9 open engine items, the four reported issues, and the coverage matrix). All 105 packets landed; every
end-of-wave gate passed. Plan and record: `dev-notes/wave-plan-2026-09-16.md` (addenda 1–3 at the end).

| | start (`6071d3b`, v0.4.18) | now (`5280cee`) |
|---|---|---|
| tests | 481 / 0 | **1,056 / 0** |
| substrate-gap fixtures | 71 | **140** |
| blind · missing · partial · forbid · missing cells · grader errors | 0 · 0 · 1 · — · — · 0 | **0 · 0 · 0 · 0 · 0 · 0** |
| coverage matrix | hand-written prose (proved systematically pessimistic) | **derived from graded assertions, committed, drift-gated** |

Reported issues: **#2 scoping** (walk gating, gitignore, project roots, `--scope`) done · **#3 architecture summary**
(`glia arch`) done · **#4 C# Refit / .proto / Kafka** done · **#6 message contracts** (`glia contracts`, report-only v1) done.

Real corpus check (`/home/ivy/Code/quokka-stack`): `[http] routes=61 endpoints=49 paired=49` (every endpoint pairs),
a 40-operation OpenAPI spec ingested and linked, 6 project roots, a real Android → turps gRPC link, 1 gRPC server.

## 2. Feature tables — what glia does now

### 2.1 CLI (`glia <cmd>`; most accept `--with <repo>` repeatable and `--json`)

| command | what it answers | status |
|---|---|---|
| `arch` | the services in a stack and the cross-service links between them — mechanism, channel, count; keyed on **project roots** (manifest-rooted), files outside any project fold into `(outside projects)`; `--mermaid`, `--include-shared` | **new** |
| `contracts` | per queue topic: producer's vs consumer's declared message type, `match / mismatch / unknown` (report-only) | **new** |
| `projects` | manifest-rooted sub-projects: label, ecosystem, path, manifest — the vocabulary for `--scope` | **new** |
| `blast-radius` | edge-category-aware, PPR-ranked, located closure with per-node edge reason + `live` flag; **`--scope` path or label** | scope new |
| `resolve` | stacktrace / diff / test id → ranked located nodes; **`--scope`** | scope new |
| `trace` | ordered cross-service path, each hop labelled with its mechanism + `cross_service` | — |
| `docs-for` | doc sections that DOCUMENT a symbol (qualified mentions Strong, ambiguous bare names Weak) | precision tiers new |
| `coverage` | per-language known extraction caveats + edges found | — |
| `analyze` | kinds + cross-edge summary; `--format mermaid` now renders the service graph (no more `repo_<u64>` labels) | mermaid rewired |
| `merge` | N repos into one graph; **`--incremental`** (per-repo parse cache, opt-in) | incremental new |
| `build` | writes `.gmap` shards; cache + manifest keyed on the **build stamp** | stamp new |
| `docs sync` / `docs push` | Confluence ↔ local snapshot; **`--include/--exclude`**, **`push --markdown`** | new flags (live halves unverified — site returns 404) |
| `--version` | `glia 0.4.18 (build 0.4.18+p<parser-hash>)` — was a stale `0.4.13` | fixed |

### 2.2 pyo3 (`repo_graph_py`, published as `repo-graph-py`) — all additive

| surface | members |
|---|---|
| module functions | `parse_file_to_json` `load_from_gmap` `default_gmap_dir` `is_stale` `kind_names` `category_names` `cell_type_names` `version` **`build_stamp`** |
| `generate_many` | gains **`incremental=False`** |
| `PyGraph` | `activate` `blast_radius(scope=None)` **`contracts`** `coverage` `cross_edge_count` `cross_stack_trace` `dense_text` `dense_text_full` `dense_text_subset` `edge_count` `edges_json` `find_node` `find_nodes_by_qname` `governing_docs(scope=None)` `neighbours` `node_cells` `node_count` `nodes_json` **`parse_errors`** **`project_roots`** `prose` `resolve(scope=None)` `resolve_signal` `save_to` `save_to_default` **`service_map`** |

`build_stamp()` tells you which code a wheel contains — the cure for the stale-`.so` failure below.

### 2.3 Capability by area

| area | works now | gaps (see §5) |
|---|---|---|
| **HTTP** | client ENDPOINTs in 12 languages (incl. C# Refit + HttpClient, Ruby, PHP Guzzle/Symfony, Rust reqwest, Scala, Elixir, Dart); server ROUTEs composed with class/router prefixes (ASP.NET attr + minimal API, Spring, Symfony, Laravel, FastAPI/Flask, Rails, Phoenix); tiered matching exact → endpoint-prefix strip → ANY → route prefix → base fold → suffix; placeholder folding (`<int:id>` `:id` `*path` `{id}` `[slug]`); `${environment.apiUrl}/x` folded via the constant table; host capture + host-based narrowing; client-side NAV routes excluded; `ts_routes` phantom server routes removed; Express named handlers bound; ROUTE/ENDPOINT always locatable | c_cpp + clojure clients measure `none`; tsconfig `paths` (Batch C) |
| **Queues / messaging** | topic scanner at every occurrence with real argument forms (kwarg/object/struct/named); topic-agnostic framework tag is unpairable + Weak; broker-family-gated join + wildcard subscriptions; task-queue identity by receiver (Celery/Sidekiq); annotation consumers (`@KafkaListener`, `@RabbitListener`, `@SqsListener`, `@Processor`, `[ServiceBusTrigger]`…); SQS/SNS, Pub/Sub, Azure Service Bus with URL/ARN topic folding; POSITION + per-site provenance; `MESSAGE_TYPE` cell (generic + Go struct-literal forms); eventbus `publish(`/`.subscribe(` gated | `const TOPIC = "x"` not folded into topics (constant table exposes it; queue scanner does not consume it); pubsub partial |
| **RPC** | `.proto` → MODULE / GRPC_SERVICE / one METHOD per rpc / MESSAGE_TYPE, with POSITION + `RPC_PACKAGE`; data-driven gRPC client needles from the build's own proto services; package-qualified service index; **gRPC server-impl detection** (Go/Java/Python/C#); ASP.NET `AddGrpcClient<T>`; RPC markers anchored to their enclosing method; tRPC procedures + `RPC_CALLS`; GraphQL exact field-key pairing + SDL files; WebSocket segment matching (no wildcard); type-keyed EventBus (Spring/MediatR/CQRS) | — |
| **Contract files** | OpenAPI/Swagger (yaml + sniffed json), AsyncAPI, Pact, GraphQL SDL, protobuf messages, Avro `.avsc`; `DOCUMENTS` contract op → implementing ROUTE; `SHARES_SCHEMA` for one message declared in two repos | OpenAPI *annotations in code* (Swashbuckle/springdoc/NestJS), JSON Schema, Connect/Twirp — uncovered |
| **Project shape** | one directory gate shared by the builder and `is_gmap_stale` (dotnet bin/obj, nested `.git` → REGION, gated-dir mtimes watched, engine output dir skipped); full `.gitignore` via the `ignore` crate matcher; project roots (npm/go/cargo/gradle/…) → `PROJECT` nodes; `--scope` by path or label | a near-empty nested Gradle root still becomes its own `arch` service |
| **Build identity / incremental** | `PARSER_STAMP` (hash of every graph-shaping source) keys the parse cache and the `.gmap` manifest, so a parser fix invalidates caches without a version bump; per-pid staging file; deterministic sidecar; opt-in per-repo cache for multi-repo builds | — |
| **Structural** | PACKAGE-base attribute resolution (Elixir cross-module calls); `resolve_file` prefers a path-suffix match over a bare basename; SharedSchema sees namespace packages; doc-linker precision tiers; intra-workspace names no longer listed as external imports; PHP `$x = new Cls()` receiver binding | receiver-type inference, heritage, CamelCase TESTS, method-level IMPLEMENTS, most DI — Batch C |
| **DI** | shared `di_stats` counters; Go wire/fx/dig and Scala DI emit `INJECTS` | TS `inject()`, Lombok/Dagger, C# primary ctor, FastAPI `Depends`, PHP — Batch C |

### 2.4 Cross-graph resolvers (`graph/src/resolvers/`, one file each)

`http` `grpc` `queue` `graphql` `websocket` `eventbus` `shared_schema` `db` `cron` `config` `iac` `package` `cli`
+ **new** `rpc` (tRPC `RPC_CALLS`) and `message_schema` (`SHARES_SCHEMA` across repos). Add one by creating
`resolvers/<name>.rs` + two lines in `resolvers/mod.rs`; register in `run_all_resolvers` (`engine/src/build.rs`).

### 2.5 Registry ids added (allocated only in `code-domain/src/lib.rs` → `RESERVED` blocks)

node_kind **45 PROJECT · 46 MESSAGE_TYPE · 47 GRPC_SERVER · 48 RPC_PROCEDURE · 49 RPC_CALL**; edge_category
**33 SHARES_DATA_SOURCE · 34 RPC_CALLS**; cell_type **17 MESSAGE_TYPE · 18 RPC_PACKAGE**. Next free: 50 / 35 / 19.
Never renumber — ids are baked into every `.gmap` on disk.

### 2.6 fired_on markers (grep stderr to prove a feature ran)

| marker prefix | proves |
|---|---|
| `[walk] collapsed N regions (always= gitignore= bundle= dotnet= nested=)` | directory gating |
| `[roots] N project roots (...)` / `[roots] emitted N PROJECT nodes` | project roots |
| `[incremental] <repo>: reused R, reparsed P, evicted E (parse cache, stamp=...)` | incremental build |
| `[http] routes=R endpoints=E paired=P (exact= any= eprefix= rprefix= base= suffix=)` | HTTP matching tiers |
| `[http] nav-routes excluded from route index: N` · `[http] placeholder folds:` · `[endpoint-fold] folded N ...` | HTTP precision |
| `[queues] ...` · `[msgtype] queue_nodes= typed= tag_topics=` · `[contracts] topics=` | messaging |
| `[proto] files= services= messages=` · `[grpc-server-impl] ...` · `[grpc-server] N impls matched` · `[avro] files= records=` · `[schema-link]` | RPC / schemas |
| `[contract] json sniffed= admitted=` · `[contract] files= ops=` · `[contract-link] ops= exact= prefix=` · `[graphql-sdl]` | contract files |
| `[const] repo table: N bindings from F files (C conflicts)` | constant table |
| `[di] injects refs: python= go= typescript= java= csharp= php= scala=` | DI |
| `[arch] S services, L links (keying= buckets= unplaced= self= unlocated=)` · `[cross-links] buckets=` | architecture |
| `[imports] local-filter: kept K, dropped D` · `[extract] ts-routes client-calls skipped: N` | precision filters |

## 3. Coverage matrix (derived — do not hand-edit)

Source of truth: `bench/substrate-gap/COVERAGE.md` + `results-latest.json`, regenerated by `matrix.py --emit`, drift-gated
by `matrix.py --check`. A cell's level is DERIVED from four graded assertions (extract / literal / route / forbid).
**`·` = a fixture exists and nothing of the right kind was emitted (a measured blind spot). `?` = no fixture claims the
cell — no claim either way.** Only 30.4% of the grid is measured; `?` is not evidence of absence.

```
LEGEND ● full  ◐ partial  · none (fixture exists, nothing emitted)  ? unknown (no fixture)  ! error
           http_cl http_sr   kafka    amqp sqs/sns  pubsub azure_s    nats   redis    mqtt   taskq    grpc graphql      ws eventbu      db    migr  config secrets   flags    cron cli_def cli_inv   calls imports injects    impl   tests service subproj
python           ●       ●       ●       ◐       ●       ◐       ●       ●       ●       ●       ·       ●       ◐       ·       ?       ●       ·       ●       ●       ·       ?       ◐       ·       ●       ●       ?       ●       ●       ?       ?
go               ●       ●       ◐       ·       ·       ?       ?       ●       ·       ●       ?       ●       ?       ●       ?       ●       ?       ?       ?       ·       ◐       ●       ●       ●       ●       ●       ?       ◐       ?       ·
typescript       ●       ●       ●       ◐       ?       ◐       ?       ●       ●       ●       ◐       ●       ●       ◐       ●       ?       ?       ?       ●       ·       ?       ?       ?       ●       ●       ●       ◐       ●       ?       ·
java             ●       ●       ●       ●       ◐       ?       ?       ?       ·       ?       ?       ●       ·       ?       ●       ●       ·       ?       ?       ?       ?       ?       ?       ●       ●       ●       ●       ◐       ?       ·
csharp           ●       ●       ●       ◐       ?       ?       ●       ?       ●       ?       ?       ●       ?       ?       ?       ?       ?       ?       ·       ?       ?       ?       ?       ●       ●       ●       ●       ◐       ?       ?
ruby             ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ●       ·       ?       ?       ?       ?       ?       ?       ●       ●       ◐       ?       ◐       ?       ?
php              ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ◐       ?       ◐       ?       ?
swift            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?
c_cpp            ·       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
scala            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ●       ●       ?       ?       ?
clojure          ·       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
dart             ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ◐       ●       ◐       ◐       ?       ?       ?
elixir           ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
rust             ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ◐       ◐       ?       ?
solidity         ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ●       ?       ?       ?
terraform        ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?

PER-MECHANISM  across 16 languages:
  http_client  ● 12  ◐ 0   · 2   ? 2   ! 0
  http_server  ● 9   ◐ 4   · 0   ? 3   ! 0
  kafka        ● 4   ◐ 1   · 0   ? 11  ! 0
  amqp         ● 1   ◐ 3   · 1   ? 11  ! 0
  sqs_sns      ● 1   ◐ 1   · 1   ? 13  ! 0
  pubsub       ● 0   ◐ 2   · 0   ? 14  ! 0
  azure_sb     ● 2   ◐ 0   · 0   ? 14  ! 0
  nats         ● 3   ◐ 0   · 0   ? 13  ! 0
  redis        ● 3   ◐ 0   · 2   ? 11  ! 0
  mqtt         ● 3   ◐ 0   · 0   ? 13  ! 0
  taskq        ● 1   ◐ 1   · 1   ? 13  ! 0
  grpc         ● 5   ◐ 0   · 0   ? 11  ! 0
  graphql      ● 1   ◐ 1   · 1   ? 13  ! 0
  ws           ● 1   ◐ 1   · 1   ? 13  ! 0
  eventbus     ● 3   ◐ 0   · 0   ? 13  ! 0
  db           ● 4   ◐ 0   · 0   ? 12  ! 0
  migrations   ● 0   ◐ 0   · 3   ? 13  ! 0
  config       ● 1   ◐ 0   · 0   ? 15  ! 0
  secrets      ● 2   ◐ 0   · 1   ? 13  ! 0
  flags        ● 0   ◐ 0   · 3   ? 13  ! 0
  cron         ● 0   ◐ 1   · 0   ? 15  ! 0
  cli_def      ● 1   ◐ 1   · 0   ? 14  ! 0
  cli_inv      ● 1   ◐ 0   · 1   ? 14  ! 0
  calls        ● 14  ◐ 1   · 0   ? 1   ! 0
  imports      ● 13  ◐ 0   · 0   ? 3   ! 0
  injects      ● 5   ◐ 3   · 0   ? 8   ! 0
  impl         ● 5   ◐ 3   · 0   ? 8   ! 0
  tests        ● 2   ◐ 6   · 0   ? 8   ! 0
  service      ● 0   ◐ 0   · 0   ? 16  ! 0
  subproject   ● 0   ◐ 0   · 3   ? 13  ! 0

PER-LANGUAGE  across 30 mechanisms:
  python       ● 16  ◐ 4   · 5   ? 5   ! 0
  go           ● 12  ◐ 3   · 5   ? 10  ! 0
  typescript   ● 14  ◐ 5   · 2   ? 9   ! 0
  java         ● 11  ◐ 2   · 4   ? 13  ! 0
  csharp       ● 10  ◐ 2   · 1   ? 17  ! 0
  ruby         ● 6   ◐ 2   · 1   ? 21  ! 0
  php          ● 3   ◐ 2   · 0   ? 25  ! 0
  swift        ● 2   ◐ 1   · 0   ? 27  ! 0
  c_cpp        ● 2   ◐ 0   · 1   ? 27  ! 0
  scala        ● 5   ◐ 1   · 0   ? 24  ! 0
  clojure      ● 2   ◐ 1   · 1   ? 26  ! 0
  dart         ● 2   ◐ 4   · 0   ? 24  ! 0
  elixir       ● 4   ◐ 0   · 0   ? 26  ! 0
  rust         ● 4   ◐ 2   · 0   ? 24  ! 0
  solidity     ● 4   ◐ 0   · 0   ? 26  ! 0
  terraform    ● 0   ◐ 0   · 0   ? 30  ! 0
```

**How to read it honestly**

- **The measured shape is good where it has been probed**: calls 14/16 full, imports 13, http_client 12, http_server 9.
- **The review's hand-made table was pessimistic**: secrets, taskq, cli_def and several messaging cells it recorded as
  zero measured `full` once probed. It was right about flags and migrations (`·` everywhere probed).
- **`subproject` is STALE, not blind**: `matrix_vocab.py` defines `subproject` as `REGION/MODULE + CONTAINS`, and its probes
  (`matrix/{go,typescript,java}/subproject`) predate the `PROJECT` kind (45) that waves 12–13 shipped. Retarget the
  vocabulary to `PROJECT` and re-author the three probes — likely three cells flip. **Cheapest open item in the matrix.**
- **`service` has no probes at all** (16 `?`); **terraform has none** (30 `?`); swift, c_cpp, clojure, elixir, php,
  solidity each have ≤ 5 measured cells.
- Adding probes: `python3 bench/substrate-gap/scaffold.py <lang> <mech>` + `AUTHORING.md`. Then the end-of-wave rebuild,
  then `matrix.py --emit`.

## 4. How to work here — rules that each cost something to learn

| rule | why |
|---|---|
| **`grade.py` reads the INSTALLED wheel, never the working tree.** Rebuild: `cargo clean -p repo-graph-engine -p repo-graph-py` → `maturin build -m py/Cargo.toml --release` → `pip install --force-reinstall --no-deps target/wheels/repo_graph_py-*.whl`, then `find <crates> -name '*.rs' -newer <installed .so>` must print nothing | a Rust fix is invisible to grading until rebuilt; `maturin` has silently repackaged stale `.so`s |
| **Gate on a captured before-file and a diff**, never a literal count | counts, artefacts and anchors all move when a sibling lands |
| **`key.json` vocabulary is frozen**; unknown fields RAISE; `forbid` matches EXACTLY; `mechanism`/`cells` validated against `matrix_vocab.py` | a lenient precision gate once accused a correct parser; invented column names passed silently |
| **Registry ids only in `code-domain/src/lib.rs`**, in the same commit as the emitter | three packets once claimed the same id |
| **API additive; graph content correctable; real contract breaks parked** for a joint repo-graph + glia version | James: "for all the contract changes we will break them next version in another session and mvoe repo-graph and glia together" |
| **Parsers extract, the graph crate resolves**; `pub(crate)` across module boundaries, never `pub`; `engine`/`graph` `lib.rs` are facades | locked at v0.4.3b |
| **Every feature ships a grep-able `fired_on` marker** | several shipped features were silent dead code for cycles |
| **Use the repo-graph MCP (`find`/`impact`/`trace`) before grepping** — and put that in every delegated brief | standing James rule; verify the graph is fresh first |
| **Committed artefacts** (`results-latest.json`, `COVERAGE.md`, `legacy-latest.json`) are regenerated only after a wheel rebuild, never by a delegated packet | emitting before the rebuild froze cells at the wrong level |
| **Delegated agents verify prerequisites**; if a brief is wrong they stopgap inside their own files and say so | a correction once asserted a module existed that did not |

**Wave tooling (reuse it for Batch C):** `dev-notes/wave-runner/` — `schedule.py --verify` (deterministic file-disjoint
scheduler; must reproduce landed waves), `gen_wave.py N out.js` (renders a Workflow script from `shared_brief.md`,
`baseline.json`, the packet specs and `packet-corrections.json`), `closeout.py N <workflow-run-id>` (the whole end-of-wave
sequence; commits only if every gate is clean). `/tmp` does not survive a session — regenerate scripts from the repo.
Resume-by-run-id across sessions is unreliable: after a break, check commits since the last close-out, the dirty tree and
`cargo check`, then regenerate and relaunch.

## 5. Open work, ranked

### 5.1 Release (James's decision, not yours)
Review 142 commits → push → version bump (0.4.18 → next) → tag → PyPI (`repo-graph-py` wheel builds on `v*` tags)
→ repo-graph session consumes `dev-notes/repo-graph-handoff-programme-2026-09.md`. **Before the repo-graph pin moves:**
confirm the wrapper decodes node/edge/cell types via `kind_names()` / `category_names()` / `cell_type_names()` (new ids
45–49 / 33–34 / 17–18), and that **neuropil** (in-process Rust consumer, not checked) builds — `GenerateResult` gained a
public field `repo_labels`, which breaks struct-literal construction.

### 5.2 Batch C — the deferred second programme (34 packets, ~4470 LOC, fully specced)
Specs in `dev-notes/wave-packets.json`, corrections (incl. handoffs from waves 1–19) in `dev-notes/packet-corrections.json`.
To run it: add `C` to `KEEP_BATCHES` in `schedule.py`, re-verify, and use the same wave loop.

| packet | LOC | what |
|---|---|---|
| A6.2a | 185 | Receiver-type inference — CodeNav field-type carrier, the resolution pass, and C# enablement |
| A6.2b | 110 | TypeScript/Angular field types — constructor parameter properties and typed class fields |
| A6.2c | 70 | Java field types — reuse the existing @Autowired field walker for every declared field |
| A6.3 | 80 | TypeScript heritage through UnresolvedRef (stop fabricating cross-file NodeIds) + the [heritage] marker |
| A6.4 | 60 | Solidity heritage through UnresolvedRef |
| A6.5 | 70 | Dart heritage through UnresolvedRef (bare-name NodeIds are dangling even same-file) |
| A6.6 | 160 | Interface method table + method-level IMPLEMENTS edge (makes interface-typed receivers resolvable) |
| A6.7 | 130 | TESTS edges for CamelCase test modules (Java, Kotlin, C#, PHP, Swift, Scala) |
| A6.8 | 125 | tsconfig `paths` aliases in TS/JS import resolution |
| A7.1 | 150 | TypeScript family: Angular `inject()` field form + NestJS/`@Inject()` constructor DI |
| A7.2 | 175 | Java: Lombok generated constructors, records, JSR-330/Dagger/Guice `@Inject`, and DI-wrapper generics |
| A7.3 | 125 | C#: primary-constructor injection and `[FromServices]` parameter injection |
| A7.4 | 130 | Python: FastAPI `Depends()` dependency injection |
| A7.5 | 135 | PHP: Symfony/Laravel constructor injection (including promoted properties) |
| A7.8 | 120 | Liveness: entrypoint qname-twins and enclosing-type propagation, so INJECTS edges actually flip `live` |
| A13.1 | 95 | DATA_ENTITY canonical join key inside DbResolver (polyglot entity sharing) |
| A13.10 | 75 | ORM: Hibernate/JPA @Table(name=) and @Document(collection=) |
| A13.11 | 85 | ORM: EF Core DbSet<T> / ToTable() (first C# DATA_ENTITY) |
| A13.12 | 85 | ORM: GORM TableName() / db.Model() / db.Table() |
| A13.13 | 80 | ORM: ActiveRecord model declaration + self.table_name |
| A13.14 | 80 | ORM: Eloquent $table / Model subclass (first PHP DATA_ENTITY) |
| A13.15 | 75 | ORM: TypeORM @Entity('table') decorator |
| A13.16 | 95 | ORM: Prisma schema.prisma model / @@map |
| A13.17 | 80 | ORM: Django models.Model implicit table name |
| A13.2 | 50 | Java DATA_ENTITY gets the explicit flavor prefix (BREAKING qname) |
| A13.4 | 150 | CLI invocation key normalisation — argv tokenisation + binary/subcommand pairing |
| A13.8 | 230 | Secrets + feature-flag references as CONFIG_KEY flavors |
| A13.9 | 175 | Migration file routing + SQL DDL and ORM-DSL table capture |
| A14.1 | 40 | Kotlin blind-spot caveat rows + "kt" language detection in coverage_report, plus the four failing Kotlin fixtu |
| A14.2 | 400 | parsers/code/kotlin crate on tree-sitter-kotlin-ng: entities + imports + Ktor scanner port, wired into the eng |
| A14.3 | 230 | Kotlin calls + heritage: CallSite/UnresolvedRef emission and the build_kotlin extra_hook for bare self-calls |
| A14.4 | 260 | Kotlin Spring needles: annotation routes + HANDLED_BY, stereotype beans, and primary-constructor INJECTS |
| A14.5 | 170 | Ktor AST route extraction with HANDLED_BY to the enclosing declared function, replacing the ported text scanne |
| A14.6 | 190 | Kotlin/Android needles: Retrofit interface ENDPOINTs and Android component classification |

### 5.3 Parked contract break
**A6.6 persisted variant** — `interface_methods` on `SymbolTable`/`SymbolTableStore` is a `.gmap` layout change read through
validated rkyv, needing a `MANIFEST_VERSION` bump. Build-time-only is in Batch C; the persisted form waits for the joint version.

### 5.4 Unowned findings (verified during the programme; no packet owns them)

| finding | evidence / where |
|---|---|
| Java parser doubles the class segment in every qname | `UserController::UserController::getUser`, `App::App::compute` — pre-existing, visible in legacy fixtures |
| Rust calls written with a full crate path produce no `CALLS` edge | `repo_graph_engine::service_map(..)` from cli/py → liveness flags the engine's public API dead |
| Calls inside macro arguments are not extracted | `impact` under-reports callers of every extractor wired via `run!` / `run_counted!` in `engine/src/extract.rs` |
| Enum variants and inline `mod x {}` blocks are not graph nodes | e.g. `MatchTier::BaseFold`, `code_domain::endpoint` |
| `AUTHORING.md` claims single-repo `generate()` can never pair across services | measured false for HTTP / WS / GraphQL / gRPC / EVENT flows |
| Live Confluence untested | the site returns `404 Site temporarily unavailable`; `docs sync`/`push` transport halves unverified; `Config::base()` hardcodes `https://` (no local stub seam) |
| `glia arch` splits a near-empty nested Gradle root into its own service | `quokka_android/android/app` (1 file, 1 node) |
| Constant table not consumed by the queue scanner | `const TOPIC = "orders"; send(TOPIC)` still falls back to the framework tag |
| Matrix `subproject` vocabulary predates `PROJECT` | §3 — three stale `·` cells |

### 5.5 Out of scope this programme
Review §4 (SDD / spec-kit assist — `detect_sdd`, `spec_status`) and §5 (LLM overlay mode — `glia gaps`,
`.glia/overlay.toml`). Their substrate (contract files, `DOCUMENTS` links, `spec`-shaped docs) now exists.

### 5.6 Memory hygiene
The native memory index was rewritten index-only on 2026-09-17 (backup in `~/.claude/projects/-home-ivy-Code-glia/memory-backups/`).
Mining the finished bench-era archive (`archive_bench.md`, 361 memories) into mempalace was decided and deferred.

## 6. File map

| path | what |
|---|---|
| `dev-notes/review-2026-09-15-coverage-and-issues.md` | the review the programme implemented |
| `dev-notes/wave-plan-2026-09-16.md` | plan + addenda 1–3 (the record) |
| `dev-notes/wave-packets.json` / `packet-corrections.json` | all 146 packet specs / corrections that override them |
| `dev-notes/wave-runner/` | scheduler, wave generator, close-out, shared agent brief, measured baseline |
| `dev-notes/repo-graph-handoff-programme-2026-09.md` | graph-content changes by packet, for the repo-graph session |
| `bench/substrate-gap/` | `run.py` (legacy view, `legacy-latest.json`), `grade.py`, `matrix.py` + `matrix_vocab.py` (`COVERAGE.md`, `results-latest.json`), `scaffold.py`, `AUTHORING.md`, `fixtures/`, `matrix/` |
