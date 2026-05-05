# glia

Cross-service code graph engine. Builds a graph of every component, every cross-service call, every shared resource across one repo or many. Other tools (LLM assistants, impact analyzers, service catalogs) read from this instead of reimplementing.

Rust engine. CLI (`glia`) and Python wheel (`repo-graph-py`). MCP server [repo-graph](https://github.com/James-Chahwan/repo-graph) wraps the wheel.

> **Licensed [Glia Software License v0.1](./LICENSE).** PolyForm Noncommercial 1.0.0 + worker-protection overlay. Free for individuals, students, researchers, nonprofits, OSS projects, orgs <500 STEM workers, worker-owned coops, B Corps, unionized workplaces. Commercial license required otherwise. Contact `j.r.chahwan@gmail.com`. Not OSI-approved by design. See [License](#license).

## What you get

```
$ glia merge ./services/api ./services/worker ./services/web

# glia analyze
- nodes: 4,213
- edges (intra-repo): 5,108
- cross-edges: 312

| Category               | Count |
| HTTP_CALLS             | 38    |
| GRPC_CALLS             | 17    |
| QUEUE_FLOWS            | 4     |
| SHARES_CONFIG          | 12    |   # same env var read by 2+ services
| SHARES_DATA_ENTITY     | 9     |   # same Postgres table / Mongo collection
| SHARES_INFRA_REF       | 6     |   # same image referenced in 2+ k8s manifests
| SHARES_DEPENDENCY      | 41    |   # same package depended on by 2+ services
```

Each cross-edge is a real queryable relationship. `api` emits to a Kafka topic that `worker` subscribes to. Both `api` and `web` read `JWT_SECRET` from env. The cron job in `infra/k8s/cleanup.yaml` runs the image built by `services/worker/Dockerfile`.

## Why "substrate"

Sourcegraph and ctags index single repos. Snyk and Endor scan dependency lists. Lens and k9s browse k8s manifests. None of them give you a graph of every service, every cross-service edge, every shared infra piece, in one place.

That's the layer glia ships. With it, downstream queries get cheap:

- LLM assistant: "what calls `/api/users`?" is an edge lookup, not a 12-repo grep.
- Impact analyzer: "if I change column `users.email`, what tests break?" walks the graph from the SQL `users` entity to handlers to tests.
- Service catalog: "which services share the `redis` cache?" filters on `infra:redis`.

Substrate ships. Other things layer on.

## Coverage

**19 language parsers** (tree-sitter):
Python, Go, TypeScript, JavaScript, React, Vue, Angular, Rust, Java, Kotlin, C#, Ruby, PHP, Swift, C/C++, Scala, Clojure, Dart, Elixir, Solidity, Terraform.

**~30 web framework extractors** across those languages:
Flask, FastAPI, Django, Celery, Rails, Sinatra, Laravel, Symfony, Slim, Spring, Quarkus, Dropwizard, Javalin, Ktor, WebFlux, Micronaut, JAX-RS, ASP.NET (controllers + Minimal API), Express, Koa, Hono, Fastify, NestJS, Next.js (Pages + App Router), SvelteKit, Hapi.js, Bun.serve, Axum, Actix, Rocket, Tide, Poem, Salvo, Gin, Echo, Chi, Fiber, Gorilla Mux, stdlib `net/http`, Phoenix, React Router, Angular Router, Vue Router.

**13 cross-graph resolvers** that pair entities across repo boundaries:
HTTP (frontend Endpoint ↔ backend Route), gRPC (client ↔ proto service), Queue (producer ↔ consumer, including raw Redis lists), GraphQL, WebSocket, EventBus, CLI invocation ↔ command, shared schema imports, shared data entities (SQL Tables / NoSQL Collections / Graph-DB Labels), Cron schedules, Config keys (env vars across services), IaC resources (Dockerfile-built images ↔ k8s manifest references), Package dependencies.

**4 non-source file types** flow through bypass extractors: YAML (`.github/workflows/`, k8s manifests, docker-compose), Dockerfiles, `.env` files, package manifests (`package.json`, `pyproject.toml`, `requirements.txt`, `Cargo.toml`, `go.mod`, `Gemfile`, `composer.json`).

## Numbers

22 framework demos plus 3 multi-service demos (microservices-demo, voting-app, bank-of-anthos). 45 effective repo paths, ~128MB of cloned source.

```
Total:       13,371 nodes / 14,105 intra-edges / 2,789 cross-edges
Wall time:   3.1s  (1.5s per-repo + 1.6s merged-resolver pass)

Cross-graph edges (resolvers fired):
  PackageResolver        1,021    cross-language shared deps
  DbResolver               691    shared tables / collections
  ConfigResolver           370    env var sharing
  IacResolver              280    image / service references
  GrpcStackResolver        175    microservices-demo gRPC mesh
  SharedSchemaResolver     140
  HttpStackResolver         66    frontend → backend route matches
  EventBusResolver          25
  WebSocketResolver         16
  GraphQLStackResolver       4
  QueueStackResolver         1    voting-app vote → worker via Redis BLPOP
  CronResolver               0    corpus-sparse, only 2 GHA workflows used schedules
  CliInvocationResolver      0    corpus-sparse, needs CLI-heavy projects
```

22 of 23 framework demos pass the per-framework coverage check. The 1 soft-miss is react-cra (corpus is the build-tooling repo, not a component-heavy app, so HOOK count is 0; extractor wired correctly).

## Install

```
# CLI from source (Rust 1.95+)
git clone https://github.com/James-Chahwan/glia
cd glia
cargo build --release -p glia-cli
cp target/release/glia ~/.local/bin/

# Python wheel (works for scripts and the MCP server)
pip install repo-graph-py     # ships pyo3 wheels for Linux / macOS / Windows
```

For LLM/MCP usage see [repo-graph](https://github.com/James-Chahwan/repo-graph), which wraps the wheel as an MCP server with 13 navigation tools.

## CLI

```
glia analyze <repo> [--format summary|mermaid|json]
    Walk one repo. Default is a Markdown summary. `mermaid` renders a
    `graph LR` of cross-stack edges. `json` is full nodes+edges.

glia impact <repo> <qname> [--direction forward|backward|both] [--depth N]
    Reachability walk over the merged graph from one entity. Forward is
    what this reaches, backward is what reaches this, depth caps the BFS.

glia merge <repo1> <repo2> [...] [--out <file>]
    Build a single MergedGraph across N repos so cross-graph resolvers
    fire across repo boundaries. `--out -` for stdout JSON, `--out <path>`
    for file.

glia build <repo> [--out <dir>]
    Walk repo and write per-language `.gmap` files (rkyv + mmap) to
    `<repo>/.glia/` (or the given dir). For tools that read .gmap directly.

glia install-hooks <repo> [--uninstall] [--command "..."]
    Install opt-in git hooks (post-commit, post-merge, post-checkout) that
    re-run `glia build .` on every change. Refuses to clobber existing
    non-glia hooks.
```

## Architecture

```
source files
   → per-language parser (tree-sitter → ExtractedItems)
   → cross-cutting extractors (HTTP routes, gRPC, queues, data sources,
     CLI commands, env var reads, package deps, cron schedules, IaC
     resources, config files, ...)
   → graph builder (resolves intra-repo references)
   → cross-graph resolvers (HttpStack, GraphQL, gRPC, Queue, WebSocket,
     EventBus, SharedSchema, DB, CLI, Cron, Config, IaC, Package)
   → MergedGraph
   → .gmap binary (rkyv + mmap, sharded), dense text projection,
     JSON, or pyo3 → Python
```

Workspace crates:
- `core/`: `Node`, `Edge`, `QName`, `RepoId`, shared primitives.
- `code-domain/`: code-specific registries (40 NodeKind IDs, 31 EdgeCategory IDs).
- `parsers/code/<lang>/`: one crate per language. `parsers/code/extractors/` for cross-cutting (gRPC, queues, WebSocket, EventBus, GraphQL, CLI, data-sources, data-entities, cron, config, IaC, packages, ts-routes, React, Angular, Vue).
- `graph/`: per-repo builder, MergedGraph, all 13 cross-graph resolvers, PPR activation.
- `engine/`: orchestration glue. Used by `py/` and `cli/`.
- `store/`: `.gmap` container (rkyv + mmap, atomic write).
- `projection-text/`: dense sigil projection for LLM context.
- `activation/`: Personalised PageRank, domain-agnostic.

## Compared to

| Tool | What it does well | What glia adds |
|---|---|---|
| Sourcegraph / ctags | intra-repo symbol search at scale | cross-service edges (HTTP/gRPC/queue/shared-DB), declarative resolver layer |
| CodeQL / Semgrep | deep semantic per-file analysis, custom rules | wider substrate (more languages, more frameworks, less depth per query), works out of the box |
| Apiiro / Endor / Snyk | dependency-graph + vuln matching | cross-language reachability via call edges, not just manifest lists; IaC, config, and queue resolvers in one tool |
| Codebase-Memory MCP | LLM-targeted graph of one codebase | multi-repo merge + cross-service resolvers, pure-Rust core |
| SocratiCode | LLM-driven code Q&A | structural index, not LLM-derived; deterministic, repeatable |
| Backstage / service catalog | curated org-level service registry | derived from source + manifests automatically, no curation step |

What glia does NOT do today:
- No intra-procedural data-flow / taint analysis (CodeQL territory).
- No vulnerability matching against CVE feeds (Snyk territory).
- No source-level fix suggestions (LLM-tier work; we emit substrate).
- No Kustomize template merging, no Helm rendering. IaC resolver reads raw manifests only.

## Experimental notes

### LLM debugging: 2.5x fewer tokens, 9x faster than grep-and-read

End-to-end test on a 566-node / 620-edge Go + Angular monorepo via the [repo-graph](https://github.com/James-Chahwan/repo-graph) MCP wrapper. Same bug, same model (Claude Opus, 100% no Haiku routing), same prompt: *"Groups that were created recently are showing as closed, and old groups show as open. This is backwards. New groups should be open for members to join. Find and fix the bug."* Fresh `/clear` for both runs.

|  | Without graph (grep + read loop) | With glia substrate |
|---|---|---|
| Tokens used | 75,308 | 29,838 |
| Time to fix | 4m 36s | ~30s |
| Files explored | ~15 (grep, read, grep, read...) | 2 (flow lookup + handler) |
| Outcome | Found and fixed | Found and fixed |

2.5x fewer tokens, 9x faster, same correct fix. Without the graph Claude greps for keywords, reads candidates, greps again, narrows down. With the graph Claude calls `flow("groups")`, gets the handler function and file, reads it, fixes it.

### Substrate scale + speed

| Metric | Value |
|---|---|
| 99-repo sweep, median repo (5,746 nodes, 4,979 edges) | 1.4s parse+resolve |
| 99-repo sweep, p90 (60,500 nodes, 65,667 edges) | 10.4s |
| 99-repo sweep, max (elasticsearch: 342,804 nodes / 336,081 edges) | 73.1s for 1.3GB of source |
| Aggregate across 99 repos | 2,083,755 nodes / 2,243,664 edges |
| 45-repo cross-service eval (this release) | 13,371 nodes / 14,105 edges / 2,789 cross-edges in 3.1s |
| Substrate failures across 99 repos | 0 generate failures, 0 timeouts |

A single laptop CPU walks the median real-world repo in under 2 seconds. The full microservices-demo + voting-app + bank-of-anthos + 22 framework demos cross-merge in 3.1 seconds with all 13 cross-graph resolvers running.

### SWE-bench latent-injection arm (parked, single-instance proof-of-concept)

glia v0.4.13 ran an arm that injected graph-derived pooled vectors into a transformer's input embedding stream. The hypothesis: graph context as latent vectors (instead of verbose prefix text) could close composition gaps on SWE-bench-Lite.

**What landed:** marshmallow-1359 SOLVE on a 7B Q4 model (Qwen 2.5 Coder). The gold-aligned auto-driver pipeline reproduces the recipe deterministically. Single-instance proof-of-concept, not a generalizable benchmark result. A follow-up N=50 bench surfaced that ~80% of apply-then-test failures were infra (pytest collection, import errors, wheel mismatches), not model output quality. Clean cross-instance results need apply/test-runner hardening.

**Why parked:** the conceptual win (graph context substituting for prose context at the embedding layer) is demonstrated on one instance. Generalising it needs per-instance plumbing, but the deeper reason is that latent injection isn't necessarily the right shape. The substrate ships independently. The open research question is bigger than "make the latent arm work":

> Given a graph + a problem + a query, what's the correct distillation over composition / sage-filtering / synthesised cells / pooled vectors that lets a 7B model do what a 70B model can do? There's a shape out there connecting static reasoning, query-specific context selection, and capability lifting. It hasn't fully connected yet.

The substrate is the precondition for trying any of those shapes cleanly. v0.4.x ships substrate; the reasoning layer above it is under design. v0.5+ will probably look very different from v0.4.13's latent-injection arm. The right answer isn't "more vectors", it's "smarter selection of what to feed where".

**Engineering wins from this arm that did ship to v0.4.x core:**
1. Graph substrate hardened to feed cross-language reachability into ranked composition cells.
2. Bench inference moved from candle to llama.cpp (`scratch/latent/out/run_llama_pathB.py`). ~7x faster CPU decode plus GBNF-grammar-constrained decoding kills the format-prior failure class plaguing the candle path.

The latent arm itself lives in `scratch/latent/`, excluded from the default workspace build so default cargo invocations skip the candle download:

```
cargo build                       # core glia, no candle
cargo build -p repo-graph-latent  # opt in to the parked arm
```

Embed-injection port to llama.cpp's `llama_batch.embd` API is feasible (API verified) but research follow-up, not a v0.4.x deliverable.

## Roadmap

**v0.4.x (this release):** substrate + CLI + pyo3 wheel + GHA wheel matrix. v0.4.13a/b/c/d shipped the SWE-bench latent-injection arm (marshmallow-1359 SOLVE).

**v0.4.14 (perf + cleanup):**
- **Per-graph-area incremental rebuild.** Re-walk and re-resolve only the regions that changed. `glia build` rewalks everything every invocation today; on a 100k-LOC monorepo the post-commit hook is the bottleneck. Want per-file content hashing, dirty-set propagation, partial `.gmap` patching instead of full rewrite.
- **Iterator parallelisation.** Per-language parser pipeline, cross-cutting extractor pass, and per-resolver index builds are embarrassingly parallel today and run sequentially. Rayon over walk + parse + extract; sharded resolver index construction.
- **Cleanup.** Single `--features research` toggle (replaces `driver`); "non-tree-sitter source dispatcher" trait consolidating the 5 bypass branches in `engine/src/lib.rs`; promote `looks_like_url_path` and the framework-presence-signals helper into a shared extractor-utils module (currently duplicated across queues/ts_routes/react); pull `engine`'s repeated language-dispatch arms into a small registry table.

**v0.5.0:** domain registries for non-code (video, chemistry, policy, climate). Code becomes one of N domains. The activation crate is already domain-agnostic; the parser+extractor layer is what abstracts.

**v0.5+:** Cross-language taint, contract drift, type propagation; node dedupe across repos; manifest format for `glia merge`; org-internal-package routing (sibling-repo imports); query-specific distillation over composition / sage / synth-cells / vectors (the reasoning-layer search direction noted in Experimental notes).

## License

See [`LICENSE`](./LICENSE). Glia Software License v0.1, an overlay on PolyForm Noncommercial 1.0.0 with Additional Permissions for worker-protective commercial use.

### Tier check

| If you are... | Cost |
|---|---|
| Individual, student, academic, researcher, hobbyist, OSS contributor | Free |
| Nonprofit | Free |
| For-profit org with **fewer than 500 STEM workers** | Free |
| **Worker-owned** org (workers hold ≥50% equity) | Free at any size |
| **Certified B Corporation** in good standing | Free at any size |
| Org where **≥50% of STEM workers are covered by a recognized union** under an active CBA | Free at any size |
| Any other for-profit org | Commercial license required |

Commercial license inquiries: `j.r.chahwan@gmail.com` or open an issue on [the repo](https://github.com/James-Chahwan/glia). Author retains discretion to grant free Commercial Licenses case-by-case. When in doubt, ask. Past compliant use is never retroactively revoked (LICENSE §5.3).

### Why not OSI-approved

OSI's Open Source Definition was authored in 1998 to make free software palatable to enterprises. Two clauses (§5 No Discrimination Against Persons or Groups, §6 No Discrimination Against Fields of Endeavor) exist for that reason. They forbid any license condition based on who you are or what you do. Including conditions like "treat workers fairly".

That choice has wins (the ecosystem we have) and costs (no license can encode worker, environmental, or human-rights conditions). Every ethical-source license (Hippocratic, ACSL, CSL, PolyForm Noncommercial, this one) is non-OSI for that reason.

A 1998 corporate-adoption strategy is not a 2026 verdict on what good licensing looks like. We're picking the modern take.

### What the license actually selects for

The qualifying conditions are baseline 21st-century governance hygiene:

- **<500 STEM workers.** Almost every startup, every small consultancy, every research lab. The threshold sits well above the size where you can claim resource constraints prevent intentional governance.
- **B-Corp certification.** ~9,000 companies and growing, including Anthropic, Patagonia, Kickstarter. ~6 months of work, manageable annual fees.
- **Recognized union.** Mostly labor-law compliance with a side of dignity. The bar (≥50% STEM coverage under an active CBA) is real but well under what unionized European tech companies have.
- **Worker-owned.** Every cooperative, every founder-led startup before dilution, Mondragon. Bar is collective worker stake ≥50%.

An org failing all four:
- Is large enough to have resources for governance
- Has chosen not to certify worker-protective governance
- Has actively suppressed (or simply opposed) collective representation
- Has opted for an extractive, no-equity employment model

That's a specific shape of company. It's the failure mode where scale is achieved by externalizing cost onto workers. The license declines to subsidize that mode.

Practical effects: GitHub marks the repo as "Other / non-standard". PyPI won't show the "OSI Approved" classifier. Some corporate legal teams auto-block. Fine. The orgs running those auto-blocks are the ones the license is asking to either qualify or pay.

## Acknowledgments

**Graph schema and traversal lineage:**
- [Joern](https://joern.io/). Code Property Graph schema is reference inspiration for glia's node + edge taxonomy. Joern's pass-composition model (parser → CFG → type-recovery → dataflow → OSS) shaped how glia layers per-language parsers, cross-cutting extractors, and cross-graph resolvers as independent passes that can be ablated.
- **Personalized PageRank** (Jeh & Widom, 2002). The activation algorithm underneath `activation/`. Domain-agnostic; glia's `ActivationConfig` exposes direction, edge weights, and node specificity as the three dials the code domain sets.
- [HippoRAG](https://github.com/OSU-NLP-Group/HippoRAG) (Jiménez Gutiérrez et al., 2024). Prior art for PPR-driven retrieval over an open knowledge graph, hippocampal-indexing-inspired. glia's activation pass borrows the *seed nodes → PPR → top-K reachable* shape. Difference: glia's graph is a structural code substrate, not entity-and-relation triples extracted from prose, and the consumer is downstream tooling (CLIs, MCP), not RAG context-stuffing.
- **Spreading activation** (Quillian 1967, Anderson 1983, Collins & Loftus 1975). Cognitive-science antecedent to all PPR-style retrieval. glia's PPR implementation is a modern, mathematically-grounded version of the same intuition: relevance propagates from seeds along weighted edges with decay.
- [GraphRAG](https://github.com/microsoft/graphrag) (Microsoft, 2024). Parallel work on graph-structured retrieval. Informs the broader space of "use a graph instead of/alongside vector search" approaches.

**Tooling:**
- [tree-sitter](https://tree-sitter.github.io/). Every language parser is built on it.
- [rkyv](https://rkyv.org/). Zero-copy serialisation behind the `.gmap` container.
- [PyO3](https://pyo3.rs/). Python bindings.
- [maturin](https://maturin.rs/). Wheel build.
- [PolyForm Project](https://polyformproject.org). The noncommercial license that glia's worker-protective overlay sits on top of.

**Parked experiment:**
- [candle](https://github.com/huggingface/candle). The v0.4.13 latent-injection arm forked the qwen2 model from here (Apache-2.0 / MIT). Bench inference subsequently moved to llama.cpp for ~7x CPU speedup. The candle fork lives in `scratch/latent/` for replay.

**Thinking partners:**
- [Anthropic's Claude](https://claude.com). Sustained design partner through the project. The graph-substrate framing, the resolver decomposition, the worker-protective licensing direction, and most of glia's actual implementation were distilled in long collaborative sessions. Thanks for being a tool that lets a single person turn a core thinking advantage into shippable substrate.
