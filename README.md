# glia

**Cross-service code graph engine.** Walk a polyglot codebase — or a fleet of
microservices across many repos — and produce a single graph of every
component, every cross-service call, every shared resource. Designed to be
the substrate other tools (LLM coding assistants, impact analyzers,
service-catalog viewers) read from instead of re-implementing all of this
themselves.

glia is a Rust engine. It exposes a CLI (`glia`) and a Python wheel
(`repo-graph-py`); the MCP server [repo-graph](https://github.com/James-Chahwan/repo-graph)
wraps the wheel so LLM clients can call it directly.

## What you get

```
$ glia merge ./services/api ./services/worker ./services/web

# glia analyze
- nodes: 4,213
- edges (intra-repo): 5,108
- cross-edges: 312
...
| Category               | Count |
| HTTP_CALLS             | 38    |
| GRPC_CALLS             | 17    |
| QUEUE_FLOWS            | 4     |
| SHARES_CONFIG          | 12    |   # same env var read by 2+ services
| SHARES_DATA_ENTITY     | 9     |   # same Postgres table / Mongo collection
| SHARES_INFRA_REF       | 6     |   # same image referenced in 2+ k8s manifests
| SHARES_DEPENDENCY      | 41    |   # same package depended on by 2+ services
```

Every one of those cross-edges is a real, queryable relationship — *"the
`api` service emits to a Kafka topic that the `worker` service subscribes
to"*, *"both `api` and `web` read `JWT_SECRET` from env"*, *"the cron job in
`infra/k8s/cleanup.yaml` runs the image built by `services/worker/Dockerfile`"*.

## Why "substrate"

There are good tools that index single repos (Sourcegraph, ctags). There are
good tools that find vulnerabilities in dependency lists (Snyk, Endor). There
are good tools that browse k8s manifests (Lens, k9s). What was missing is the
layer beneath all of them: **a graph of every service, every cross-service
edge, every shared piece of infrastructure, in one place, queryable by name.**

Once that substrate exists, the consumers get cheap:

- An LLM assistant asks "what calls `/api/users`?" — it's an edge lookup, not
  a grep across 12 repos.
- An impact analyzer asks "if I change column `users.email`, what tests
  break?" — walk the graph from the SQL `users` data-entity to all
  reachable handlers to all tests that cover them.
- A service catalog asks "which services share the `redis` cache?" — query
  by `infra:redis` resource node.

glia ships the substrate. The other things layer on top.

## Coverage

**19 language parsers** (tree-sitter-backed):
Python · Go · TypeScript · JavaScript · React · Vue · Angular · Rust · Java ·
Kotlin · C# · Ruby · PHP · Swift · C/C++ · Scala · Clojure · Dart · Elixir ·
Solidity · Terraform.

**~30 web framework extractors** across those languages:
Flask · FastAPI · Django · Celery · Rails · Sinatra · Laravel · Symfony ·
Slim · Spring · Quarkus · Dropwizard · Javalin · Ktor · WebFlux · Micronaut ·
JAX-RS · ASP.NET (controllers + Minimal API) · Express · Koa · Hono ·
Fastify · NestJS · Next.js (Pages + App Router) · SvelteKit · Hapi.js ·
Bun.serve · Axum · Actix · Rocket · Tide · Poem · Salvo · Gin · Echo · Chi ·
Fiber · Gorilla Mux · stdlib `net/http` · Phoenix · React Router · Angular
Router · Vue Router.

**13 cross-graph resolvers** that pair up entities across repo boundaries:
HTTP (frontend Endpoint ↔ backend Route) · gRPC (client ↔ proto service) ·
Queue (producer ↔ consumer, including raw Redis lists) · GraphQL · WebSocket ·
EventBus · CLI invocation ↔ command · Shared schema imports · Shared data
entities (SQL Tables / NoSQL Collections / Graph-DB Labels) · Cron schedules ·
Config keys (env vars across services) · IaC resources (Dockerfile-built
images ↔ k8s manifest references) · Package dependencies.

**4 non-source file types** flow through bypass extractors: YAML
(`.github/workflows/`, k8s manifests, docker-compose) · Dockerfiles ·
`.env` files · package manifests (`package.json`, `pyproject.toml`,
`requirements.txt`, `Cargo.toml`, `go.mod`, `Gemfile`, `composer.json`).

## Numbers

Eval against 22 framework demos + 3 multi-service demos
(GoogleCloudPlatform/microservices-demo, dockersamples/example-voting-app,
GoogleCloudPlatform/bank-of-anthos), 45 effective repo paths total, ~128MB
of cloned source:

```
Total:       13,371 nodes / 14,105 intra-edges / 2,789 cross-edges
Wall time:   3.1s  (1.5s per-repo + 1.6s merged-resolver pass)

Cross-graph edges (resolvers fired):
  PackageResolver        1,021    (cross-language shared deps)
  DbResolver               691    (shared tables / collections)
  ConfigResolver           370    (env var sharing)
  IacResolver              280    (image / service references)
  GrpcStackResolver        175    (microservices-demo gRPC mesh)
  SharedSchemaResolver     140
  HttpStackResolver         66    (frontend → backend route matches)
  EventBusResolver          25
  WebSocketResolver         16
  GraphQLStackResolver       4
  QueueStackResolver         1    (voting-app vote → worker via Redis BLPOP)
  CronResolver               0    (corpus-sparse — only 2 GHA workflows used schedules)
  CliInvocationResolver      0    (corpus-sparse — needs CLI-heavy projects)
```

Per-framework coverage check: **22 of 23 framework demos pass** the
"extractor emits the expected node kind" assertion (the 1 soft-miss is
react-cra emitting 0 hooks because the demo is the build-tooling repo, not a
component-heavy app — extractor wired correctly, corpus issue).

## Install

```
# Build the CLI from source (Rust 1.95+)
git clone https://github.com/James-Chahwan/glia
cd glia
cargo build --release -p glia-cli
cp target/release/glia ~/.local/bin/

# Or install the Python wheel for use from scripts / MCP
pip install repo-graph-py     # ships pyo3 wheels for Linux / macOS / Windows
```

For the LLM/MCP integration, see [repo-graph](https://github.com/James-Chahwan/repo-graph)
which wraps the wheel as an MCP server with 13 navigation tools.

## CLI

```
glia analyze <repo> [--format summary|mermaid|json]
    Walk one repo. Default format is a Markdown summary table; `mermaid`
    emits a `graph LR` of cross-stack edges; `json` is the full
    nodes+edges dump for downstream tools.

glia impact <repo> <qname> [--direction forward|backward|both] [--depth N]
    Reachability walk over the merged graph from one entity. Forward = what
    this reaches; backward = what reaches this; depth caps the BFS.

glia merge <repo1> <repo2> [...] [--out <file>]
    Build a single MergedGraph across N repos so cross-graph resolvers
    fire across the boundary. `--out -` for stdout JSON, `--out <path>`
    for file.

glia build <repo> [--out <dir>]
    Walk repo and write per-language `.gmap` files (rkyv + mmap) to
    `<repo>/.glia/` (or the given dir). Used by tools that read .gmap
    directly without re-walking the source tree.

glia install-hooks <repo> [--uninstall] [--command "..."]
    Install opt-in git hooks (`post-commit`, `post-merge`, `post-checkout`)
    that re-run `glia build .` on every change to keep the .gmap fresh.
    Refuses to clobber existing non-glia-managed hooks.
```

## Architecture

```
source files
   → per-language parser (tree-sitter → ExtractedItems)
   → cross-cutting extractors (HTTP routes, gRPC, queues, data sources,
     CLI commands, env var reads, package deps, cron schedules, IaC
     resources, config files, …)
   → graph builder (resolves intra-repo references)
   → cross-graph resolvers (HttpStack, GraphQL, gRPC, Queue, WebSocket,
     EventBus, SharedSchema, DB, CLI, Cron, Config, IaC, Package)
   → MergedGraph
   → .gmap binary (rkyv + mmap, sharded)  /  dense text projection  /
     JSON  /  pyo3 → Python
```

Workspace crates:
- `core/` — `Node`, `Edge`, `QName`, `RepoId`, shared primitives.
- `code-domain/` — code-specific registries (40 `NodeKind` IDs, 31
  `EdgeCategory` IDs).
- `parsers/code/<lang>/` — one crate per language; `parsers/code/extractors/`
  for cross-cutting (gRPC, queues, WebSocket, EventBus, GraphQL, CLI,
  data-sources, data-entities, cron, config, IaC, packages, ts-routes,
  React, Angular, Vue).
- `graph/` — per-repo builder, MergedGraph, all 13 cross-graph resolvers,
  PPR activation.
- `engine/` — orchestration glue (file walk → parse → extract → build →
  resolve). Used by `py/` (pyo3 wrapper) and `cli/` (`glia` binary).
- `store/` — `.gmap` container (rkyv + mmap, atomic write-via-`.tmp`).
- `projection-text/` — dense sigil projection for LLM context.
- `activation/` — Personalised PageRank, domain-agnostic.

## Compared to

| Tool | What it does well | What glia adds |
|---|---|---|
| **Sourcegraph / ctags** | intra-repo symbol search at scale | cross-service edges (HTTP/gRPC/queue/shared-DB), declarative resolver layer |
| **CodeQL / Semgrep** | deep semantic per-file analysis, custom rules | substrate is wider (more languages, more frameworks, less depth per query) and ships out of the box |
| **Apiiro / Endor / Snyk** | dependency-graph + vuln matching | cross-language reachability via call edges (not just manifest-list); IaC / config / queue resolvers in one tool |
| **Codebase-Memory MCP** | LLM-targeted graph of one codebase | multi-repo merge + cross-service resolvers; pure-Rust core |
| **SocratiCode** | LLM-driven code Q&A | structural index, not LLM-derived; deterministic, repeatable |
| **Backstage / service catalog** | curated org-level service registry | derived from source + manifests automatically; no curation step |

What glia explicitly does NOT do today:
- No intra-procedural data-flow / taint analysis (CodeQL territory).
- No vulnerability matching against CVE feeds (Snyk territory).
- No source-level fix suggestions (LLM-tier work; we emit substrate).
- No Kustomize template merging, no Helm rendering — IaC resolver reads
  raw manifests only.

## Experimental notes

### Latent-injection SWE-bench arm (parked)

glia v0.4.13 ran an experimental arm injecting graph-derived pooled vectors
into a transformer's input embedding stream — testing whether graph context
supplied as latent vectors (rather than verbose prefix text) could close
composition gaps on SWE-bench-Lite. The arm landed
**marshmallow-1359 SOLVE** on a 7B Q4 model (Qwen 2.5 Coder), with the
gold-aligned auto-driver pipeline reproducing the recipe deterministically.

The arm itself is parked. The `forward_input_embed` hook (forked qwen2 model
+ candle dependency) lives in `scratch/latent/` and is **excluded from the
default workspace build** so default cargo invocations skip the candle
download:

```
cargo build                       # core glia, no candle
cargo build -p repo-graph-latent  # opt in to the parked arm
```

Bench inference itself migrated from candle to **llama.cpp**
(`scratch/latent/out/run_llama_pathB.py`) — ~7× faster on CPU, plus
GBNF-grammar-constrained decoding kills the format-prior failure class
that plagued the candle path. The full embed-injection port to llama.cpp's
`llama_batch.embd` API is feasible but research-tier follow-up, not a
v0.4.x deliverable.

## Roadmap

**v0.4.x (this release):** the substrate above + CLI + pyo3 wheel + GHA
wheel matrix. **v0.4.13a/b/c/d:** SWE-bench latent-injection arm
(marshmallow-1359 SOLVE).

**v0.5.0:** domain registries for non-code (video, chemistry, policy,
climate). Code becomes one of N domains. The activation crate is already
domain-agnostic; the parser+extractor layer is what abstracts.

**v0.5+:** Cross-language taint, contract drift, type propagation;
node dedupe across repos; incremental `.gmap` rebuild; manifest format
for `glia merge`; org-internal-package routing (sibling-repo imports).

## License

See `LICENSE` (TBD — see [open question](https://github.com/James-Chahwan/glia/issues)
on a custom union-conditional commercial-tier license under discussion).
The `Cargo.toml` strings currently say `MIT` as a placeholder.

## Acknowledgments

- [Joern](https://joern.io/) — Code Property Graph schema is reference
  inspiration for glia's node + edge taxonomy.
- [tree-sitter](https://tree-sitter.github.io/) — every language parser
  is built on it.
- [rkyv](https://rkyv.org/) — zero-copy serialisation behind the `.gmap`
  container.
- [PyO3](https://pyo3.rs/) — Python bindings.
- [maturin](https://maturin.rs/) — wheel build.
- [candle](https://github.com/huggingface/candle) — the v0.4.13
  latent-injection experiment forked the qwen2 model from here (Apache-2.0
  / MIT).
