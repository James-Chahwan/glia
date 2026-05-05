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

> **Licensed under the [Glia Software License v0.1](./LICENSE)** — a worker-
> protective overlay on PolyForm Noncommercial 1.0.0. Free for individuals,
> students, researchers, nonprofits, open-source projects, organizations
> with <500 STEM workers, worker-owned coops, B Corporations, and unionized
> workplaces. Commercial license required for all other for-profit use —
> contact `j.r.chahwan@gmail.com`. **Not OSI-approved by design** — see
> [Licensing](#license) for the why.

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

### LLM debugging: 2.5× fewer tokens, ~9× faster than grep-and-read

End-to-end test on a 566-node / 620-edge Go + Angular monorepo via the
[repo-graph](https://github.com/James-Chahwan/repo-graph) MCP wrapper. Same
bug, same model (Claude Opus, 100% — no Haiku routing), same prompt
(*"Groups that were created recently are showing as closed, and old groups
show as open. This is backwards — new groups should be open for members to
join. Find and fix the bug."*), fresh `/clear` context for both runs.

|  | Without graph (grep + read loop) | With glia substrate |
|---|---|---|
| Tokens used | 75,308 | 29,838 |
| Time to fix | 4m 36s | ~30s |
| Files explored | ~15 (grep, read, grep, read…) | 2 (flow lookup + handler) |
| Outcome | Found and fixed | Found and fixed |

**Net: 2.5× fewer tokens, ~9× faster, same correct fix.** The mechanism:
without the graph, Claude greps for keywords, reads candidates, greps
again, narrows down. With the graph, Claude calls `flow("groups")`, gets
the exact handler function and file, reads it, fixes it. Structural
context displaces lexical search.

### Substrate scale + speed

| Metric | Value |
|---|---|
| 99-repo sweep (median repo: 5,746 nodes, 4,979 edges) | **1.4s** parse+resolve per repo |
| 99-repo p90 (60,500 nodes, 65,667 edges) | **10.4s** |
| 99-repo max (elasticsearch: 342,804 nodes / 336,081 edges) | **73.1s** for 1.3GB of source |
| Aggregate across 99 repos | **2,083,755 nodes / 2,243,664 edges** |
| 45-repo cross-service eval (this release) | **13,371 nodes / 14,105 edges / 2,789 cross-edges in 3.1s** |
| Substrate failures across 99 repos | **0 generate failures, 0 timeouts** |

A single laptop CPU walks the median real-world repo in well under 2
seconds. The full microservices-demo + voting-app + bank-of-anthos +
22 framework demos cross-merge in 3.1 seconds with all 13 cross-graph
resolvers firing.

### SWE-bench latent-injection arm (parked, single-instance proof-of-concept)

glia v0.4.13 ran an experimental arm injecting graph-derived pooled vectors
into a transformer's input embedding stream — testing whether graph context
supplied as latent vectors (rather than verbose prefix text) could close
composition gaps on SWE-bench-Lite.

**What landed:** **marshmallow-1359 SOLVE** on a 7B Q4 model (Qwen 2.5
Coder). The gold-aligned auto-driver pipeline reproduces the recipe
deterministically. Single-instance proof-of-concept — *not* a
generalizable benchmark result. A subsequent N=50 bench surfaced that
~80% of the apply-then-test pipeline failures were infra (pytest
collection / import errors / wheel mismatches), not model output quality;
clean cross-instance results need apply/test-runner hardening.

**Why parked:** the conceptual win (graph context can substitute for prose
context at the embedding layer) is demonstrated on one instance.
Generalising it requires per-instance plumbing work, sure — but the
deeper reason is that the *latent injection* shape isn't necessarily the
right shape. The substrate ships independently regardless. The open
research question on top of it is bigger than just "make the latent arm
work":

> Given a graph + a problem + a query, what is the correct distillation
> over composition / sage-filtering / synthesised cells / pooled vectors
> that lets a 7B model do what a 70B model can do? There's a shape out
> there that connects static reasoning, query-specific context selection,
> and capability lifting. It hasn't fully connected yet.

The substrate is the precondition for trying any of those shapes
cleanly. v0.4.x ships the substrate; the reasoning layer above it is
under active design. v0.5+ will probably look very different from
v0.4.13's latent-injection arm — the right answer isn't "more vectors"
but "smarter selection of what to feed where".

**Engineering wins that did ship to v0.4.x core from this arm:**
1. The graph substrate hardened to feed cross-language reachability into
   ranked composition cells.
2. Bench inference moved from **candle to llama.cpp**
   (`scratch/latent/out/run_llama_pathB.py`) — **~7× faster CPU decode**
   plus GBNF-grammar-constrained decoding that kills the format-prior
   failure class plaguing the candle path.

**The latent arm itself** lives in `scratch/latent/` and is **excluded
from the default workspace build** so default cargo invocations skip
the candle download:

```
cargo build                       # core glia, no candle
cargo build -p repo-graph-latent  # opt in to the parked arm
```

Embed-injection port to llama.cpp's `llama_batch.embd` API is feasible
(API verified) but research-tier follow-up, not a v0.4.x deliverable.

## Roadmap

**v0.4.x (this release):** the substrate above + CLI + pyo3 wheel + GHA
wheel matrix. **v0.4.13a/b/c/d:** SWE-bench latent-injection arm
(marshmallow-1359 SOLVE).

**v0.4.14 (perf + cleanup, between this and v0.5):**
- **Per-graph-area incremental rebuild** — only re-walk and re-resolve
  the source regions that actually changed since last build. Currently
  `glia build` rewalks the whole repo every invocation; on a 100k-LOC
  monorepo the post-commit hook latency is the bottleneck. Wants:
  per-file content hashing, dirty-set propagation, partial `.gmap`
  patching instead of whole-file rewrite.
- **Iterator parallelisation** — the per-language parser pipeline,
  cross-cutting extractor pass, and per-resolver index builds are all
  embarrassingly parallel today and run sequentially. Rayon-ify the
  walk + parse + extract steps; sharded resolver index construction.
- **Cleanup** — feature-gate the synth bins as a single `--features
  research` toggle (currently `driver`); consolidate the 5 bypass
  branches in `engine/src/lib.rs` behind a uniform "non-tree-sitter
  source dispatcher" trait; promote the `looks_like_url_path` validator
  + framework-presence-signals helper into a shared extractor-utils
  module (currently duplicated across queues/ts_routes/react); pull
  `engine`'s repeated language-dispatch match arms out into a small
  registry table.

**v0.5.0:** domain registries for non-code (video, chemistry, policy,
climate). Code becomes one of N domains. The activation crate is already
domain-agnostic; the parser+extractor layer is what abstracts.

**v0.5+:** Cross-language taint, contract drift, type propagation;
node dedupe across repos; manifest format for `glia merge`;
org-internal-package routing (sibling-repo imports); query-specific
distillation over composition / sage / synth-cells / vectors (the
reasoning-layer search direction noted in Experimental notes above).

## License

See [`LICENSE`](./LICENSE) — **Glia Software License v0.1**, an overlay on
PolyForm Noncommercial 1.0.0 with Additional Permissions for worker-
protective commercial use.

### Tier check

| If you are… | Cost |
|---|---|
| An individual, student, academic, researcher, hobbyist, or open-source contributor | **Free** |
| A nonprofit | **Free** |
| A for-profit organization with **fewer than 500 STEM workers** | **Free** |
| A **worker-owned** organization (workers hold ≥50% equity) | **Free** at any size |
| A **Certified B Corporation** in good standing | **Free** at any size |
| An organization where **≥50% of STEM workers are covered by a recognized union** under an active CBA | **Free** at any size |
| Any other for-profit organization | Commercial license required |

Commercial license inquiries: `j.r.chahwan@gmail.com` or open an issue on
[the repo](https://github.com/James-Chahwan/glia). Author retains
discretion to grant free Commercial Licenses case-by-case; when in doubt,
ask. Past compliant use is never retroactively revoked (LICENSE §5.3).

### Why not OSI-approved

OSI's Open Source Definition was authored in 1998 explicitly to make free
software palatable to enterprises. Two of its clauses — §5 "No
Discrimination Against Persons or Groups" and §6 "No Discrimination Against
Fields of Endeavor" — exist for that reason. They forbid any license
clause that would condition use on who you are or what you do. Including
clauses that would, say, require treating workers fairly.

That was a real strategic choice with real wins: it gave us the ecosystem
we have. It also has real costs: it structurally rules out any license
that wants to encode worker, environmental, or human-rights conditions —
which is why every ethical-source license (Hippocratic, ACSL, CSL,
PolyForm Noncommercial, this one) is "non-OSI."

A 1998 corporate-adoption strategy is not a 2026 verdict on what good
licensing looks like. The structural problem with OSI-approved licensing
is that it prevents the field from evolving. We're picking the modern
take.

### What the license actually selects for

This isn't friction on commercial adoption — it's friction on a specific
failure mode of corporate scaling. Look at the qualifying conditions:

- **<500 STEM workers** — almost every startup, every small consultancy,
  every research lab. The threshold sits well above the size where you
  could reasonably claim resource constraints prevent intentional
  governance.
- **B-Corp certification** — ~9,000 companies and growing, including
  Anthropic, Patagonia, Kickstarter. Cheap to get (~6 months of work,
  manageable annual fees). Standard ESG hygiene at this point.
- **Recognized union** — in most jurisdictions, just labor-law compliance
  with a side of dignity. The bar (≥50% STEM coverage under active CBA)
  is real but well under what unionized European tech companies have.
- **Worker-owned** — every cooperative, every founder-led startup before
  dilution, Mondragon, etc. The bar is collective worker stake ≥50%.

An organization that fails ALL FOUR is by construction:
- Large enough to have resources for governance
- Has chosen not to certify worker-protective governance
- Has actively suppressed (or simply opposed) collective representation
- Has opted for an extractive, no-equity employment model

That's a *specific* shape of company. It's not "competitive enterprise" —
it's the failure mode where scale is achieved by externalizing cost onto
workers. The license declines to subsidize that mode.

Practical effects: GitHub will mark the repo as "Other / non-standard."
PyPI won't show the "OSI Approved" classifier. Some corporate legal teams
will auto-block. All of that is fine — the orgs running those auto-blocks
are exactly the ones the license is asking to either qualify or pay.

## Acknowledgments

**Graph schema + traversal lineage:**
- [Joern](https://joern.io/) — Code Property Graph schema is reference
  inspiration for glia's node + edge taxonomy. Joern's pass-composition
  model (parser → CFG → type-recovery → dataflow → OSS) shaped how glia
  layers per-language parsers, cross-cutting extractors, and cross-graph
  resolvers as independent passes that can be ablated.
- **Personalized PageRank** (Jeh & Widom, 2002) — the activation algorithm
  underneath `activation/`. Domain-agnostic; glia's `ActivationConfig`
  exposes direction, edge weights, and node specificity as the three dials
  the code domain sets.
- [HippoRAG](https://github.com/OSU-NLP-Group/HippoRAG) (Jiménez Gutiérrez
  et al., 2024) — prior art for PPR-driven retrieval over an open knowledge
  graph, hippocampal-indexing-inspired. glia's activation pass borrows the
  *"seed nodes → PPR → top-K reachable"* shape; difference is that glia's
  graph is a structural code substrate rather than entity-and-relation
  triples extracted from prose, and the consumer is downstream tooling
  (CLIs, MCP) rather than RAG context-stuffing.
- **Spreading activation** (Quillian 1967, Anderson 1983, Collins & Loftus
  1975) — the cognitive-science antecedent to all PPR-style retrieval.
  glia's PPR implementation is a modern, mathematically-grounded version
  of the same intuition: relevance propagates from seeds along weighted
  edges with decay.
- [GraphRAG](https://github.com/microsoft/graphrag) (Microsoft, 2024) —
  parallel work on graph-structured retrieval; informs the broader space
  of "use a graph instead of/alongside vector search" approaches.

**Tooling:**
- [tree-sitter](https://tree-sitter.github.io/) — every language parser
  is built on it.
- [rkyv](https://rkyv.org/) — zero-copy serialisation behind the `.gmap`
  container.
- [PyO3](https://pyo3.rs/) — Python bindings.
- [maturin](https://maturin.rs/) — wheel build.
- [PolyForm Project](https://polyformproject.org) — the noncommercial
  license that glia's worker-protective overlay sits on top of.

**Parked experiment:**
- [candle](https://github.com/huggingface/candle) — the v0.4.13
  latent-injection arm forked the qwen2 model from here (Apache-2.0 /
  MIT). Bench inference subsequently moved to llama.cpp for ~7× CPU
  speedup; the candle fork lives in `scratch/latent/` for replay.

**Thinking partners:**
- [Anthropic's Claude](https://claude.com) — sustained design partner
  through the project. The graph-substrate framing, the resolver
  decomposition, the worker-protective licensing direction, and most
  of glia's actual implementation were distilled in long collaborative
  sessions. Thanks for being a tool that lets a single person turn a
  core thinking advantage into shippable substrate.
