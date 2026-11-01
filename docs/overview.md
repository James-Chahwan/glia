# glia in depth

What the graph covers, how it is built and measured, and how it compares. The README keeps install, the quickstart and the CLI reference; the release history is [history.md](./history.md); the model underneath, a DPG (Domain-Parameterised Graph), is [dpg.md](./dpg.md).

## Across services

Point it at several repos (or a monorepo) and the cross-graph resolvers pair what each side declares: an HTTP client call to the route it hits (through base URLs, API prefixes, router mounts and OpenAPI server bases), a gRPC stub to its service, a producer to its consumer on a topic, a tRPC / Connect / Twirp call to its procedure, a GraphQL operation to the resolvers of the root fields it selects (and its client's HTTP transport to the server's `/graphql` route), a WebSocket client to its handler (also through the HTTP route that upgrades it), an ethers / viem / web3.js / go-ethereum contract call to the Solidity function of the contract it names, a CLI invocation to its command, and the shared resources: one table, one env var, one image. A client that dials a third-party host (an HTTP, WebSocket, gRPC or GraphQL client) is marked `external` and never paired.

**Identity across repos.** One package, one proto / Avro / JSON Schema message, one contract op or one SCIP symbol in several repos is one identity class: the last resolver joins its copies with SAME_AS edges, one star per class, FACT or DERIVED by rule. So is a frontend model and the backend struct of the HTTP exchange it reads (a TS interface or Dart class and a Go struct whose JSON names confirm the pair), and a GraphQL client's result model with the schema type it returns, joined by the models pass, DERIVED; this happens inside one monorepo too. Answers show such a class once, at its best member, with the other repos under `also_in` (`find`, `blast-radius`, `trace`, `flows`, `tests-for`, `serves`, `arch`), and walks treat a message, a contract op or a SCIP symbol as one node, so a blast radius crosses into every repo that shares it. Copies that differ are kept and marked `diverged`, and vendored copies (a declaration copied into another repo under a vendoring path, byte-identical or diverged) are HEURISTIC candidates, never collapsed. `--no-collapse` shows the raw rows; `glia identity` lists the classes.

`glia merge` builds several repos as one graph (or merges pre-built layouts); `glia arch` lists the services and the links between them.

## Coverage

**18 languages** (tree-sitter): Python, Go, TypeScript / JavaScript, Rust, Java, Kotlin, C#, Ruby, PHP, Swift, C / C++, Scala, Clojure, Dart, Elixir, Solidity, Terraform, Lua (0.5.5, from glia PR #3); plus React, Vue and Angular parsers stacked on TypeScript.

**How much of it is measured.** `bench/substrate-gap/` holds 507 fixture repos (0.5.6), each a small project with a graded key of the nodes, edges and cells it must produce and the ones it must not; none is blind, and a close-out fails on any new blind, forbid, missing-node or missing-cell row. They fill an 18-language x 30-mechanism coverage matrix ([`COVERAGE.md`](../bench/substrate-gap/COVERAGE.md)): 354 cells full, 0 partial, 0 none, 158 not yet probed and 28 that cannot apply. A second axis grades **use patterns**, how code is used rather than how services talk (host commands, host events, host callbacks, config tables, embedded scripts, script entries and, since 0.5.6, process entries): 18 languages x 7 patterns, 21 cells full (Lua's six, Python's five, TypeScript's six and the process entries of C#, Go, Java and Rust), 0 partial, 0 none, 105 not yet probed. In 0.5.1 it read 243 full, 34 partial and 69 none: 0.5.2's DF, DG and DH groups measured and closed every none and all but one partial cell, and the tweaks closed the last (Dart's function-level TESTS, through closure calls). Since 0.5.4 a second, real-repo side grades the same cells on the outlier cohort (`bench/cohort/`, 21 public repos) and a synthetic cross-service matrix (`bench/sibyl/`), and both run in every release's close-out beside the fixtures. A partial cell would be a known gap, and `glia coverage` prints the caveats for the repo you point it at.

**~30 web framework extractors** across those languages:
Flask, FastAPI, Django, Celery, Rails, Sinatra, Laravel, Symfony, Slim, Spring, Quarkus, Dropwizard, Javalin, Ktor, WebFlux, Micronaut, JAX-RS, ASP.NET (controllers + Minimal API), Express, Koa, Hono, Fastify, NestJS, Next.js (Pages + App Router), SvelteKit, Hapi.js, Bun.serve, Axum, Actix, Rocket, Tide, Poem, Salvo, Gin, Echo, Chi, Fiber, Gorilla Mux, stdlib `net/http`, Phoenix, http4s, Akka / Pekko HTTP, Vapor, Compojure, reitit, shelf_router, React Router, Angular Router, Vue Router.

**Queues, jobs, events and other channels** across those languages: Kafka, RabbitMQ, SQS / SNS, GCP Pub/Sub, Azure Service Bus, NATS, MQTT, Redis lists and pub/sub, JMS, Celery, Dramatiq, Sidekiq, Oban and BullMQ, including Go broker clients (streadway / amqp091-go, azservicebus, cloud.google.com/go/pubsub, go-redis, aws-sdk-go SQS / SNS, confluent-kafka-go, kafka-go `Writer` literals), JVM and .NET broker builders (Azure Service Bus Java builders, AWS SDK v2 request builders, MQTTnet, Spring Cloud GCP `PubSubTemplate`, Spring Data Redis lists, NATS.Net v2, Google.Cloud.PubSub.V1), and the JobRunr, Hangfire and asynq task queues, whose job is named by the method or task type it runs. In-process event buses, Go's asaskevich/EventBus included. GraphQL servers and clients, HotChocolate code-first resolvers and Go machinebox/graphql / C# GraphQL.Client requests included. Cron schedules, NestJS `@Cron` / `@Interval` included. CLI invocations, JVM `ProcessBuilder` / `Runtime.exec` launches included. HTTP clients, Dart Dio generic calls (`dio.get<T>(..)`) and Dio base URLs included.


The 0.5.2 additions, language by language, and the tweaks' contract models, NATS, WebSocket, GraphQL, smart-contract and dead-symbol work are listed in [history.md](./history.md#052s-coverage-additions-in-detail).

**Inside a language** the graph builder resolves calls, imports and heritage per language, beyond plain names. Two examples: in the TypeScript family, parameters and locals type their receivers (annotated, `new T()`, `inject(T)`, `x as T`, `this.<field>` aliases); a `this.m()`, `super.m()` or typed-receiver call binds through superclasses; and an override of an abstract or a concrete member IMPLEMENTS it, so liveness and dead-symbol dispatch reach it. In Go, struct embeds are INHERITS_FROM and their methods are promoted to calls and interface satisfaction; in-repo type aliases are resolved before signatures are compared; package-var initialisers call; repository-root package imports are recorded and an import binds its package's `package` clause name. A call on a freshly constructed object (`new Calc().add(..)`, Ruby's `Calc.new.add(..)`) binds through the constructed type. Every language gets function-level TESTS edges from a test module's root functions.

**17 cross-graph resolvers, the models pass and the identity pass** that pair entities across repo boundaries:
HTTP (frontend Endpoint ↔ backend Route), gRPC (client ↔ proto service), RPC (tRPC / Connect / Twirp call ↔ procedure), EVM (contract client call or event subscription ↔ Solidity procedure or event), Queue (producer ↔ consumer, including raw Redis lists, NATS wildcard subjects and a Terraform-bound Pub/Sub subscription), GraphQL (operation ↔ the resolvers of its root fields), gqlgen (SDL resolver ↔ its Go method), WebSocket, EventBus, CLI invocation ↔ command, shared schema imports, shared data entities (SQL Tables / NoSQL Collections / Graph-DB Labels), Cron schedules, Config keys (env vars across services), the host graph (0.5.6: a start a Dockerfile, compose service, k8s container, Procfile type, systemd unit or supervisord program declares, or a manifest's binary ↔ the code it runs, through package scripts, entry shell scripts and `dist/` back to source; an nginx / OpenResty location ↔ the Lua file it runs; a script another language loads ↔ its module), IaC resources (Dockerfile-built images ↔ k8s manifest references); then the models pass (SAME_AS: a client model and the server model of its HTTP exchange, a GraphQL result model and its schema type) and identity (SAME_AS: one package across repos, a package and the project its manifest names, one proto / Avro / JSON Schema message, one contract op, one SCIP symbol, and vendored copies as candidates). Last, org-internal package routing (0.5.4): an npm, Go, cargo or pypi import of a package another repo of the build declares (the `manifest_name` identity class) binds to that project's module, and so do the calls and references through it, Maven / Gradle coordinates included since 0.5.6. The 0.5.1 Package and MessageSchema resolvers are gone: their pairs are identity classes now.

**Hosts (0.5.6).** Much code runs inside a host that loads it and calls into it. One table, `code_domain::hosts`, names what each host registers; a row never mints an edge by itself: a parser reads the registration in the AST (the call, its name and its handler) only where the host shows in that file or tree. Read so: Neovim user commands, autocmds, options and plugin units; Maki tools; WoW events; AwesomeWM signals; LÖVE callbacks; WezTerm settings; VS Code commands, events, settings and `activate`; Obsidian commands, events and lifecycle methods; webpack / tapable hooks; Flask app and blueprint hooks; pytest hooks and fixtures; `#!` and `__main__` / `require.main === module` script entries. Each becomes a `host_cmd:` CLI_COMMAND, a `host_event:` EVENT_HANDLER or a `config:<host>:` CONFIG_KEY with a HANDLED_BY to its handler, or an ENTRYPOINT cell on the function the host calls, so liveness, flows and blast radius start where the host starts the code. A Dockerfile stage is read as a launch only when a compose / k8s file targets it or it is the final stage: a stage only a CI script builds with `--target` gets no start edge (no guessing).

**Non-source files** flow through bypass extractors: YAML (`.github/workflows/`, k8s manifests, docker-compose, skaffold, OpenAPI / AsyncAPI), Dockerfiles (per stage: base images, `ENTRYPOINT` / `CMD`, `WORKDIR`, the `COPY` map), Procfiles, systemd units, supervisord configs, nginx confs that run Lua, `.env` files, package manifests (`package.json`, `pyproject.toml`, `requirements.txt`, `Cargo.toml`, `go.mod`, `Gemfile`, `composer.json`, and since 0.5.4 Maven `pom.xml`, Gradle `build.gradle(.kts)`, `pubspec.yaml`, `mix.exs` and .NET project files), migration `.sql`, Prisma schemas, `.proto`, `.graphql`, Avro `.avsc`, JSON Schema and contract `.json`, .NET `appsettings*.json` (every key a `config:setting:<Section:Key>` that C# `IConfiguration` reads pair with, and an env define `Section__Key` defines it too), and markdown docs. A `requirements.txt` or `Pipfile` beside Python code roots a project, and an sbt `build.sbt`'s sub-projects are project roots. Generated code (protoc and codegen file names, or a generator banner in the first 40 comment lines) is marked ORIGIN `generated`, and a vendored generated gRPC stub is no client. Markdown is read from the well-known files (`README*.md`, `ARCHITECTURE.md`, `CLAUDE.md`, ...) and the `docs/` tree at the repo root and at every project root, a top-level `.ai/` tree, ADR directories, SDD feature docs (`features/<feature>/*.md`, spec-kit `specs/<NNN-slug>/`) and synced external docs (`glia docs sync`). A section documents a symbol it names in backticks; `glia coverage` states that scope.

## Numbers

Measured on 2026-10-18 at v0.5.2, release build, 16 cores, best of 3 (`glia analyze`, nothing persisted):

| repo | files | nodes | intra-repo edges | cross-edges | build (16 threads) | build (`GLIA_THREADS=1`) |
|---|--:|--:|--:|--:|--:|--:|
| [glia](https://github.com/James-Chahwan/glia) itself | 4,136 | 36,721 | 122,591 | 1,742 | 2.16s | 10.24s |
| [flutter/samples](https://github.com/flutter/samples) | 4,845 | 19,292 | 33,245 | 121 | 2.27s | |
| [grpc-go](https://github.com/grpc/grpc-go) | 1,150 | 14,296 | 51,755 | 1,460 | 1.11s | 4.22s |
| [microservices-demo](https://github.com/GoogleCloudPlatform/microservices-demo) (11 services, 5 languages) | 363 | 3,060 | 4,908 | 78 | 0.09s | |

The graph is byte-identical at any thread count. A rebuild reuses a per-file parse cache, so only changed files are parsed again.

**`glia watch` (0.5.2).** A held session re-reads only the edited file, rebuilds only the language group whose inputs moved and computes the delta over the rebuilt graphs. Measured 2026-10-16 after 0.5.2's code waves W0-W13 (`9de9114b`, before the tweaks), release build, 16 cores, default threads, on an otherwise idle machine, with `bench_one_line_edit`: three one-line edits of one file, each `[watch] update took=` beside a disk-incremental `glia build` of the same tree run right after it (`generate_one_incremental`: cache load, build, save):

| repo, edited file | groups rebuilt | watch update | incremental build |
|---|--:|--:|--:|
| glia, a Python file (`bench/substrate-gap/grade.py`) | 1 of 26 | 529 / 478 / 460 ms | 1.40 / 1.38 / 1.35 s |
| glia, a Rust file (`engine/src/delta.rs`) | 1 of 26 | 1245 / 1148 / 1146 ms | 1.42 / 1.40 / 1.39 s |
| grpc-go, a Go file (`server.go`) | 1 of 5 | 903 / 875 / 866 ms | 0.93 / 0.94 / 0.91 s |

Opening the session (one full build) took 1.22-1.27 s on glia and 0.81 s on grpc-go. A first run minutes earlier read within 8% of these, and its phase times show where an update goes: the Rust and Go updates are mostly that one group's rebuild (531-560 ms for glia's Rust group, 601-614 ms for grpc-go's Go group), while the Python update rebuilds a small group (25-27 ms), so the passes (resolve + post + finalize, about 175 ms), the grafts (67-74 ms) and cloning the reused groups out of the cache (47-129 ms) dominate it. Earlier numbers under sibling-build load, in the DC.7a / DC.7b commit bodies, read higher (glia Python 601-731 ms, Rust 1685-3101 ms, grpc-go 1035-1337 ms).

**Memory (0.5.2).** A Python process that calls `generate()` repeatedly no longer grows: a PyGraph drop returns freed heap to the OS (glibc `malloc_trim`). `bench/rss/rss_loop.py`, 15 rebuilds in one process (Linux glibc, three runs agree): glia 410 -> 1,017 MB before, 404 -> 265 MB after; grpc-go 223 -> 520 MB before, 219 -> 170 MB after. A held pyo3 `WatchSession` over 12 updates: glia 668 -> 748 MB, grpc-go 290 -> 327 MB.

**Watch's body-only fast path (0.5.6).** An edit that keeps every declaration of its files is re-stitched against the cached group instead of rebuilding it (Python, Go, the TypeScript family; Rust measured below its bar and has none). Release build, default threads, `git archive` copies, median `[watch] update took=` with the fast path on / off (`GLIA_WATCH_FAST_PATH=0`), from the GA.4 / GA.7 / GA.6 commits: prefect, 20 one-file Python body edits, 1534 / 2846 ms (18 of 20 took it); photoprism `internal/entity`, 10 Go edits, 1177 / 3305 ms (7 took it); twenty's server auth module, 20 TypeScript edits, 6902 / 9037 ms (13 took it). A refused edit (a file declaring a node another file also declares, a route, a mount or a wire cell) rebuilds the group as before, and every update's graph equals a clean build.

**One memory budget (0.5.6).** `GLIA_MEMORY_BUDGET_MB` (or `glia watch --memory-budget-mb`, `query --max-mb`) is a process RSS target: a watch session past it evicts its cached group builds, then its build memos, trading time for bytes. On glia: plateau 940 MB unbudgeted, 779 MB with every cache evicted, 796 MB at an 819 MB budget, about 7% more time per evicting update.

## Architecture

```
source files
   → per-language parser (tree-sitter → ExtractedItems)
   → cross-cutting extractors (HTTP routes, gRPC, queues, data sources,
     CLI commands, env var reads, package deps, cron schedules, IaC
     resources, config files, ...)
   → graph builder (resolves intra-repo references)
   → cross-graph resolvers (HttpStack, gRPC, RPC, EVM, Queue, GraphQL,
     gqlgen, WebSocket, EventBus, SharedSchema, CLI, DB, Cron, Config,
     Host, IaC), then the models pass, identity (SAME_AS) and org-internal
     package routing, then the
     post-passes (overlay edges, TESTS, doc links, evidence), all one
     ordered pass registry
   → MergedGraph
   → .gmap layout (rkyv + mmap, sharded, self-describing header,
     format 3: interned evidence strings, CODE cells as spans),
     dense text projection, JSON, or pyo3 → Python
   → at query time: Datalog rules and GQL (glia-rules), identity
     collapse on the answers, the rankings (co-change, PageRank,
     BM25 over commit messages, the repo's calibrated pick) and the
     embedding blend in find --files and concept search; glia watch
     keeps a build open, re-stitching body-only edits
```

Workspace crates:
- `core/`: `Node`, `Edge`, `QName`, `RepoId`, shared primitives.
- `code-domain/`: code-specific registries (50 NodeKind, 38 EdgeCategory, 27 CellType IDs; 0.5.2 added SAME_AS, 0.5.4 IMPORT_BINDINGS and three ids reserved for 0.5.7's goal layer; 0.5.6 none) and the code domain's profile tables, identity rules included, and the host table (`hosts`: plugin hosts, launchers).
- `parsers/code/<lang>/`: one crate per language. `parsers/code/extractors/` for cross-cutting (gRPC, queues, WebSocket, EventBus, GraphQL, CLI, data-sources, data-entities, cron, config, IaC, packages, ts-routes, React, Angular and Angular templates, Vue, and the EVM contract clients for TypeScript and Go).
- `graph/`: per-repo builder (with the per-file stitching seam that re-stitches body-only edits for Python, Go and TypeScript), MergedGraph, the 17 cross-graph resolvers (the host resolver and org-internal package routing included), the models pass (client / server contract models: wire shapes, pairs, drift) and the identity resolver, blast radius.
- `engine/`: orchestration (walk, parse, build, passes, persist), every answer primitive, the rules layer over the code graph, identity collapse, the watch session, the rankings, boundary health and the query-time embedding tier. Used by `py/` and `cli/`.
- `embed/`: `glia-embed`, the local embedding model (fastembed bge-small, a test-hash model) and its crash-isolated GPU worker.
- `rules/`: `glia-rules`, the Datalog evaluator: parser, GQL front end, stratified semi-naive evaluation with tiers, magic sets, aggregates, explain. Depends on `core/` and `activation/` only.
- `cli/`: the `glia` binary, one file per command. `py/`: the pyo3 bindings published as `glia-py`.
- `store/`: `.gmap` container (rkyv + mmap, atomic write).
- `projection-text/`: dense sigil projection for LLM context.
- `activation/`: Personalised PageRank (power iteration and push), the pass registry, the domain profile and the generic algorithms (reachability, levelled BFS, graph delta, SCC, communities, cuts, hubs, MinHash, identity classes and their quotient), all domain-agnostic.
- `doc-sources/`: Confluence ingestion behind `glia docs`. `snapshots/`: the git-history, test-report and identifier-vocabulary snapshot writers. `stamp/`: the build identity that keys the parse cache.
- `toy-domain/`: a test-only second domain proving the domain seam; never a dependency of a shipped crate. `build-domain/` and `parsers/build/bazel/` are empty slots reserved for 0.5.7's build-target graphs.

## Compared to

| Tool | What it does well | What glia adds |
|---|---|---|
| Sourcegraph / ctags | intra-repo symbol search at scale | cross-service edges (HTTP/gRPC/queue/shared-DB), declarative resolver layer |
| CodeQL / Semgrep | deep semantic per-file analysis, custom rules | wider substrate (more languages, more frameworks, less depth per query), works out of the box; structural Datalog rules over the cross-service graph, no data flow |
| Apiiro / Endor / Snyk | dependency-graph + vuln matching | the dependency graph as one layer of a cross-service graph (shared packages across services, next to IaC, config and queue links); no vulnerability matching by design ([SECURITY.md](../SECURITY.md)) |
| Codebase-Memory MCP | LLM-targeted graph of one codebase | multi-repo merge + cross-service resolvers, pure-Rust core |
| SocratiCode | LLM-driven code Q&A | structural index, not LLM-derived; deterministic, repeatable |
| Backstage / service catalog | curated org-level service registry | derived from source + manifests automatically, no curation step |

What glia does NOT do:
- No data-flow / taint analysis. A deliberate non-goal, not a roadmap gap: glia maps structure, not exploitability ([SECURITY.md](../SECURITY.md)).
- No vulnerability matching against CVE feeds, and no joining package or call reachability to them. Also deliberate ([SECURITY.md](../SECURITY.md)).
- No source-level fix suggestions (LLM-tier work; we emit substrate).
- No Kustomize template merging, no Helm rendering. IaC resolver reads raw manifests only.

## Experimental notes

These are earlier measurements, kept for the record: the first two predate 0.5.0's answer surface, and the SWE-bench arm is parked.

### LLM debugging: 2.5x fewer tokens, 9x faster than grep-and-read (v0.4.x)

End-to-end test on a 566-node / 620-edge Go + Angular monorepo via the [repo-graph](https://github.com/James-Chahwan/repo-graph) MCP wrapper. Same bug, same model (Claude Opus, 100% no Haiku routing), same prompt: *"Groups that were created recently are showing as closed, and old groups show as open. This is backwards. New groups should be open for members to join. Find and fix the bug."* Fresh `/clear` for both runs.

|  | Without graph (grep + read loop) | With glia substrate |
|---|---|---|
| Tokens used | 75,308 | 29,838 |
| Time to fix | 4m 36s | ~30s |
| Files explored | ~15 (grep, read, grep, read...) | 2 (flow lookup + handler) |
| Outcome | Found and fixed | Found and fixed |

2.5x fewer tokens, 9x faster, same correct fix. Without the graph Claude greps for keywords, reads candidates, greps again, narrows down. With the graph Claude calls `flow("groups")`, gets the handler function and file, reads it, fixes it.

### Substrate scale + speed (v0.4.x, plus the 0.5.0 parallel build)

| Metric | Value |
|---|---|
| 99-repo sweep, median repo (5,746 nodes, 4,979 edges) | 1.4s parse+resolve |
| 99-repo sweep, p90 (60,500 nodes, 65,667 edges) | 10.4s |
| 99-repo sweep, max (elasticsearch: 342,804 nodes / 336,081 edges) | 73.1s for 1.3GB of source |
| Aggregate across 99 repos | 2,083,755 nodes / 2,243,664 edges |
| 45-repo cross-service eval (v0.4.x) | 13,371 nodes / 14,105 edges / 2,789 cross-edges in 3.1s |
| Substrate failures across 99 repos | 0 generate failures, 0 timeouts |
| Parallel build, 0.5.0 (LG.1a–c; 16 cores, release `glia analyze --format json`, min of 5 runs, `GLIA_THREADS=1` → default) | grpc-go 3.06s → 0.79s; glia 4.35s → 1.23s; a private full-stack app 0.45s → 0.11s |

A single laptop CPU walks the median real-world repo in under 2 seconds single-threaded (v0.4.x). The full microservices-demo + voting-app + bank-of-anthos + 22 framework demos cross-merge in 3.1 seconds with all 13 of v0.4.x's cross-graph resolvers running.

### SWE-bench latent-injection arm (parked, single-instance proof-of-concept)

glia v0.4.13 ran an arm that injected graph-derived pooled vectors into a transformer's input embedding stream. The hypothesis: graph context as latent vectors (instead of verbose prefix text) could close composition gaps on SWE-bench-Lite.

**What landed:** marshmallow-1359 SOLVE on a 7B Q4 model (Qwen 2.5 Coder). The gold-aligned auto-driver pipeline reproduces the recipe deterministically. Single-instance proof-of-concept, not a generalizable benchmark result. A follow-up N=50 bench surfaced that ~80% of apply-then-test failures were infra (pytest collection, import errors, wheel mismatches), not model output quality. Clean cross-instance results need apply/test-runner hardening.

**Why parked:** the conceptual win (graph context substituting for prose context at the embedding layer) is demonstrated on one instance. Generalising it needs per-instance plumbing, but the deeper reason is that latent injection isn't necessarily the right shape. The substrate ships independently. The open research question is bigger than "make the latent arm work":

> Given a graph + a problem + a query, what's the correct distillation over composition / sage-filtering / synthesised cells / pooled vectors that lets a 7B model do what a 70B model can do? There's a shape out there connecting static reasoning, query-specific context selection, and capability lifting. It hasn't fully connected yet.

The substrate is the precondition for trying any of those shapes cleanly. v0.4.x ships substrate; the reasoning layer above it is under design. v0.5+ will probably look very different from v0.4.13's latent-injection arm. The right answer isn't "more vectors", it's "smarter selection of what to feed where".

**Engineering wins from this arm that did ship to v0.4.x core:**
1. Graph substrate hardened to feed cross-language reachability into ranked composition cells.
2. Bench inference moved from candle to llama.cpp (`bench/latent/out/run_llama_pathB.py`). ~7x faster CPU decode plus GBNF-grammar-constrained decoding kills the format-prior failure class plaguing the candle path.

The latent arm itself lives in `bench/latent/`, excluded from the default workspace build so default cargo invocations skip the candle download:

```
cargo build                       # core glia, no candle
cargo build -p glia-latent  # opt in to the parked arm
```

Embed-injection port to llama.cpp's `llama_batch.embd` API is feasible (API verified) but research follow-up, not a v0.4.x deliverable.

## Acknowledgments

**Graph schema and traversal lineage:**
- [Joern](https://joern.io/). Code Property Graph schema is reference inspiration for glia's node + edge taxonomy. Joern's pass-composition model (parser → CFG → type-recovery → dataflow → OSS) shaped how glia layers per-language parsers, cross-cutting extractors, and cross-graph resolvers as independent passes that can be ablated.
- **Personalized PageRank** (Jeh & Widom, 2002). The activation algorithm underneath `activation/`. Domain-agnostic; glia's `ActivationConfig` exposes direction, edge weights, and node specificity as the three dials the code domain sets.
- [HippoRAG](https://github.com/OSU-NLP-Group/HippoRAG) (Jiménez Gutiérrez et al., 2024). Prior art for PPR-driven retrieval over an open knowledge graph, hippocampal-indexing-inspired. glia's activation pass borrows the *seed nodes → PPR → top-K reachable* shape. Difference: glia's graph is a structural code substrate, not entity-and-relation triples extracted from prose, and the consumer is downstream tooling (CLIs, MCP), not RAG context-stuffing.
- **Spreading activation** (Quillian 1967, Anderson 1983, Collins & Loftus 1975). Cognitive-science antecedent to all PPR-style retrieval. glia's PPR implementation is a modern, mathematically-grounded version of the same intuition: relevance propagates from seeds along weighted edges with decay.
- [GraphRAG](https://github.com/microsoft/graphrag) (Microsoft, 2024). Parallel work on graph-structured retrieval. Informs the broader space of "use a graph instead of/alongside vector search" approaches.

**Graph theory background:**
- *Introduction to Algorithms*, 4th edition (Cormen, Leiserson, Rivest, Stein). Reference for the graph algorithms underneath glia's traversal primitives.
- [DanielKeogh/com.danielkeogh.graph](https://github.com/DanielKeogh/com.danielkeogh.graph). Friend's graph library; helped along the way.

**Tooling:**
- [tree-sitter](https://tree-sitter.github.io/). Every language parser is built on it.
- [rkyv](https://rkyv.org/). Zero-copy serialisation behind the `.gmap` container.
- [PyO3](https://pyo3.rs/). Python bindings.
- [maturin](https://maturin.rs/). Wheel build.
- [PolyForm Project](https://polyformproject.org). The noncommercial license that glia's worker-protective overlay sits on top of.

**Parked experiment:**
- [candle](https://github.com/huggingface/candle). The v0.4.13 latent-injection arm forked the qwen2 model from here (Apache-2.0 / MIT). Bench inference subsequently moved to llama.cpp for ~7x CPU speedup. The candle fork lives in `bench/latent/` for replay.

**Thinking partners:**
- [Anthropic's Claude](https://claude.com). Sustained design partner through the project. The graph-substrate framing, the resolver decomposition, the worker-protective licensing direction, and most of glia's actual implementation were distilled in long collaborative sessions. Thanks for being a tool that lets a single person turn a core thinking advantage into shippable substrate.
