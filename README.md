# glia

Cross-service code graph engine. Builds a graph of every component, every cross-service call, every shared resource across one repo or many. Other tools (LLM assistants, impact analyzers, service catalogs) read from this instead of reimplementing.

Rust engine. CLI (`glia`) and Python wheel (`glia-py`, `import glia_py`; formerly `repo-graph-py`, up to 0.4.18). MCP server [repo-graph](https://github.com/James-Chahwan/repo-graph) wraps the wheel.

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

**20 language parsers** (tree-sitter):
Python, Go, TypeScript, JavaScript, React, Vue, Angular, Rust, Java, Kotlin, C#, Ruby, PHP, Swift, C/C++, Scala, Clojure, Dart, Elixir, Solidity, Terraform.

**~30 web framework extractors** across those languages:
Flask, FastAPI, Django, Celery, Rails, Sinatra, Laravel, Symfony, Slim, Spring, Quarkus, Dropwizard, Javalin, Ktor, WebFlux, Micronaut, JAX-RS, ASP.NET (controllers + Minimal API), Express, Koa, Hono, Fastify, NestJS, Next.js (Pages + App Router), SvelteKit, Hapi.js, Bun.serve, Axum, Actix, Rocket, Tide, Poem, Salvo, Gin, Echo, Chi, Fiber, Gorilla Mux, stdlib `net/http`, Phoenix, React Router, Angular Router, Vue Router.

**15 cross-graph resolvers** that pair entities across repo boundaries:
HTTP (frontend Endpoint ↔ backend Route), gRPC (client ↔ proto service), RPC (tRPC / Connect / Twirp call ↔ procedure), Queue (producer ↔ consumer, including raw Redis lists), GraphQL, WebSocket, EventBus, CLI invocation ↔ command, shared message schemas (proto / Avro / JSON Schema types), shared schema imports, shared data entities (SQL Tables / NoSQL Collections / Graph-DB Labels), Cron schedules, Config keys (env vars across services), IaC resources (Dockerfile-built images ↔ k8s manifest references), Package dependencies.

**Non-source files** flow through bypass extractors: YAML (`.github/workflows/`, k8s manifests, docker-compose, OpenAPI / AsyncAPI), Dockerfiles, `.env` files, package manifests (`package.json`, `pyproject.toml`, `requirements.txt`, `Cargo.toml`, `go.mod`, `Gemfile`, `composer.json`), migration `.sql`, Prisma schemas, `.proto`, `.graphql`, Avro `.avsc`, JSON Schema and contract `.json`, and markdown docs.

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
pip install glia-py          # 0.5.0+; abi3 pyo3 wheels for Linux / macOS / Windows, import glia_py
```

A build runs its walk reads, per-file parse / extract, const-table scan, RPC needle pass and per-language graph builds (all but the TypeScript family's) on the engine's own thread pool, one thread per core; `GLIA_THREADS=N` sets the size (unset or `0` = every core, clamped to 1..=256, `1` = the single-threaded path), the graph is byte-identical at any size, and stderr carries `[parallel]` lines (one per walk, two per repo) naming the thread count.

For LLM/MCP usage see [repo-graph](https://github.com/James-Chahwan/repo-graph), which wraps the wheel as an MCP server with 13 navigation tools.

**Use without MCP.** An agent can also call the CLI directly, one `glia <command> <repo> --json` per question. [`skills/glia/SKILL.md`](./skills/glia/SKILL.md) is a Claude Code skill that teaches this. It maps each question to its command and shows how to read an answer (`file:line` rows, absences, blind spots), with one worked example per command. To install it, copy it to `~/.claude/skills/glia/` or `<repo>/.claude/skills/glia/`. No server stays resident: each call builds the graph in memory and exits. `cli/tests/skill_surface.rs` checks every command and flag the skill names against `cli/surface/`.

## CLI

Every subcommand and flag of `glia`, one usage line each, rendered from the committed CLI surface snapshots in `cli/surface/` (LG.6a); `glia <command> --help` has the full text. `--with <repo>` (repeatable) merges more repos in first, so the resolvers pair across them; `--json` prints JSON instead of tables; `--scope` takes a repo-relative path or a project label from `glia projects`. The global `--no-overlay` builds without `.glia/overlay.toml`'s `[[edge]]` stanzas, the extraction-only graph ([docs/overlay.md](./docs/overlay.md)).

```
# Build, merge, store
glia build <REPO> [--no-incremental] [--out <OUT>]
    Walk a repo and write its graph layout (manifest.json + per-language .gmap
    shards + cross_stack.gmap) to <repo>/.glia/graph/, the directory the MCP
    server and load_from_gmap read. Incremental through the parse cache beside it.
glia analyze <REPO> [--format summary|mermaid|json]
    Walk one repo and print node kinds + cross-graph edges. `mermaid` is the
    service graph (as `glia arch --mermaid`), `json` the full nodes + edges.
glia merge [REPOS]... [--gmap <DIR>]... [--incremental] [--layout <DIR>] [--out <OUT>] [--workspace <FILE>]
    One MergedGraph across N repos, or across pre-built layouts (--gmap, or a
    glia.workspace.json via --workspace) without their sources; the resolvers
    fire across repo boundaries. --layout writes the merged layout, --out JSON.
glia inspect <PATH> [--json]
    What a .gmap file or layout directory holds, every id named from the file's
    own header. Exits 1 on an unreadable or old-format file ("rebuild the graph").
glia install-hooks [REPO] [--command <COMMAND>] [--pair <PAIR>] [--uninstall]
    Opt-in post-commit / post-merge / post-checkout hooks that rerun `glia build .`
    (or --command), written where git reads hooks: core.hooksPath, else the common
    dir, so one install serves every worktree of the repo. --pair <sibling> adds a
    branch-pair lock: pre-commit blocks unless the sibling has the same branch
    checked out, commit-msg pins `Glia-Pinned-At: <sibling HEAD>`. Bypass once with
    `git commit --no-verify` (both hooks) or GLIA_BRANCH_PAIR=skip (the check
    only). Needs git >= 2.13. Never overwrites hooks glia did not write.

# The whole stack
glia arch <REPO> [--include-shared] [--json] [--mermaid] [--with <WITH>]...
    The services and the cross-service links between them, each with its mechanism
    and channel. One repo keys services by top-level directory; SHARES_* and
    DOCUMENTS links show only with --include-shared.
glia projects <REPO> [--json] [--with <WITH>]...
    The manifest-rooted sub-projects (label, ecosystem, path): the --scope vocabulary.
glia coverage <REPO> [--json] [--with <WITH>]...
    Per-language extraction caveats and edges found, plus the co-change-without-edge
    audit, so a grep fallback is a deliberate choice.
glia gaps <REPO> [--category <CATEGORY>] [--json] [--overlay-delta] [--top-k <TOP_K>] [--with <WITH>]...
    Ranked blind spots (unpaired / ambiguous / unresolved endpoints, uncalled routes,
    tag-only queues, dead symbols, co-change without an edge, overlay rot), each with
    the overlay section that could repair it. --overlay-delta: what an overlay edit did.
glia contracts <REPO> [--breaking-only] [--fields] [--json] [--mismatch-only] [--with <WITH>]...
    Per queue topic, producer vs consumer message type. --fields diffs declared fields
    (schema copies, topics, AsyncAPI channels, OpenAPI vs Pact).
glia pages <REPO> [--dead-only] [--json] [--with <WITH>]...
    Frontend router pages, the links between them, dead deep links, unlinked pages.
glia flows <REPO> [--depth <DEPTH>] [--feature <FEATURE>] [--features] [--group-by feature|entry] [--json] [--out <OUT>] [--scope <SCOPE>] [--with <WITH>]...
    Every entry point's forward flow: reach, mechanisms, whether it crosses a service,
    keyed by the feature word `glia trace` takes. --features / --out group them into
    per-feature records (<feature>.yaml + index.json).
glia spec-status <REPO> [--feature <FEATURE>] [--json] [--status implemented|declared_missing|undeclared] [--with <WITH>]...
    Per feature, the declared API ops (OpenAPI, feature.yaml) a route implements, the
    ones still missing, and routes no op declares.
glia cycles <REPO> [--json] [--kind event|import|all] [--scope <SCOPE>] [--with <WITH>]...
    Cross-service event loops with a located witness, call loops, service-level
    possible loops, then module import cycles.

# One symbol, channel or signal
glia find <REPO> <QUERY> [--json] [--kind <KIND>]... [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    The ranked, located nodes a symbol, qname or fragment names, with the match tier.
glia resolve <REPO> <SIGNAL> [--json] [--kind <KIND>] [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    A stacktrace, diff, test id or free text -> the ranked, located nodes it names.
glia impact <REPO> <QNAME> [--depth <DEPTH>] [--direction forward|backward|both]
    Reachability walk from one entity: forward, backward or both.
glia blast-radius <REPO> <QNAME>... [--depth <DEPTH>] [--direction forward|backward|both] [--json] [--live-only] [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    Edge-category-aware, PPR-ranked, located closure around one or more seeds, each
    row naming its seed, edge reason and `live` flag; import / contain edges excluded.
glia trace <REPO> <FEATURE> [--depth <DEPTH>] [--json] [--max-paths <MAX_PATHS>] [--to <TO>] [--with <WITH>]...
    Ranked distinct paths from a feature across service boundaries, each hop with its
    mechanism; --to gives the paths between two nodes.
glia why <REPO> <FROM> <TO> [--category <CATEGORY>] [--json] [--with <WITH>]...
    Every edge between two nodes with its emitter, rule, call site, confidence and
    FACT / DERIVED / HEURISTIC tier; with none, a witness path. Exits 1 when not found.
glia implementors <REPO> <QNAME> [--direct] [--json] [--up] [--with <WITH>]...
    Who implements or extends a type, or overrides a method (--up: its supertypes),
    each row tiered by the weakest heritage edge on its path.
glia serves <REPO> <CHANNEL> [--json] [--mechanism auto|http|queue] [--with <WITH>]...
    Who serves `METHOD /path` or a queue topic, through the resolver's own matcher. An
    empty answer is a FACT with the mechanism's caveats and near misses.
glia effects <REPO> <QNAMES>... [--class <CLASS>]... [--cross-service] [--depth <DEPTH>] [--json] [--scope <SCOPE>] [--with <WITH>]... [--writes-only]
    The effect sinks downstream of the seeds (DB read / write, queue produce, outbound
    HTTP / RPC / WS / GraphQL call, event emit), each with its witness path.
glia docs-for <REPO> <QNAME> [--json] [--scope <SCOPE>] [--with <WITH>]...
    The doc sections that DOCUMENT a symbol.

# A change (git rev or pasted diff)
glia delta <REPO> [--base <BASE>] [--category <CATEGORY>]... [--edges-only] [--json]
    What the working tree's change did to the graph against a git rev (default HEAD):
    nodes and edges added, removed, modified or moved, located.
glia diff-impact <REPO> [--base <REV>] [--depth <DEPTH>] [--diff <FILE>] [--direction forward|backward|both] [--json] [--live-only] [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    The changed nodes (--base rev or --diff file) and one ranked blast radius around
    them, each row naming the change that reached it.
glia tests-for <REPO> [QNAMES]... [--base <REV>] [--depth <DEPTH>] [--diff <FILE>] [--files-only] [--json] [--no-module-level] [--scope <SCOPE>] [--with <WITH>]...
    The tests to run for a change (seed qnames, --diff or --base), tiered FACT /
    DERIVED / HEURISTIC; --files-only prints test files for a runner.
glia patterns <REPO> [--base <REV>] [--experimental] [--json] [--min-share <MIN_SHARE>] [--min-support <MIN_SUPPORT>] [--scope <SCOPE>] [--with <WITH>]...
    Experimental (refuses without --experimental): route handlers whose role chain to
    their first effect sink diverges from their service's convention. Counts, not rules.

# Rules
glia check <REPO> [--json] [--with <WITH>]...
    Evaluate the [[constraint]] rules of .glia/overlay.toml (forbid_edge, no_cycle) to
    located VIOLATIONs. Exits 0 clean, 1 on violations, 2 on an error: CI-ready.

# Inputs from outside the source: each writes a .glia/ snapshot or sidecar that the
# next build reads. The build itself never fetches, syncs or ingests.
glia docs sync <REPO> --space <SPACE> [--email <EMAIL>] [--exclude <PATTERN>]... [--include <PATTERN>]... [--site <SITE>] [--token <TOKEN>]
    Pull a Confluence space into <repo>/.glia/docs-snapshot/ (network step).
glia docs push --file <FILE> --space <SPACE> --title <TITLE> [--email <EMAIL>] [--markdown] [--page-id <PAGE_ID>] [--site <SITE>] [--token <TOKEN>]
    Create or update (--page-id) a Confluence page; --markdown converts the file first.
glia history sync <REPO> [--blame] [--blame-max-files <BLAME_MAX_FILES>] [--max-commits <MAX_COMMITS>] [--since <SINCE>]
    Read the local git history into <repo>/.glia/history-snapshot/: churn and blame
    ATTN cells, CO_CHANGES edges. No author identities are stored.
glia tests ingest <REPO> [--junit <PATH>...] [--lcov <PATH>...] [--log <PATH>...] [--run <LABEL>]
    Read one CI run's JUnit XML, CI logs and lcov into <repo>/.glia/test-snapshot/:
    FAIL and COVERAGE cells. Messages are redacted for secrets before they are stored.
glia cell set <REPO> <QNAME> <CELL> [--dims <DIMS>] [--file <PATH>] [--json <ENTRY>] [--kind <KIND>] [--model <MODEL>] [--text <TEXT>]
    Write a CONSTRAINT / DECISION / CONV entry (one of --json, --text, --file) or a
    VECTOR (--file, --model, --dims) on a node; kept in .glia/cells.jsonl /
    vectors.jsonl across rebuilds.
glia cell rm <REPO> <QNAME> <CELL> [--id <ID>] [--source <SOURCE>]
    Remove one entry (--id, --source) or a node's VECTOR.
glia cell ls <REPO> [--check] [--json] [--qname <QNAME>] [--rekey]
    List the sidecar rows. --check binds each against a fresh build and exits 1 on an
    orphaned, ambiguous or rejected row; --rekey rewrites rows a moved node re-bound.
```

## Architecture

```
source files
   → per-language parser (tree-sitter → ExtractedItems)
   → cross-cutting extractors (HTTP routes, gRPC, queues, data sources,
     CLI commands, env var reads, package deps, cron schedules, IaC
     resources, config files, ...)
   → graph builder (resolves intra-repo references)
   → cross-graph resolvers (HttpStack, gRPC, RPC, Queue, GraphQL,
     WebSocket, EventBus, SharedSchema, MessageSchema, CLI, DB, Cron,
     Config, IaC, Package), then the post-passes (overlay edges, TESTS,
     doc links, evidence), all one ordered pass registry
   → MergedGraph
   → .gmap layout (rkyv + mmap, sharded, self-describing header),
     dense text projection, JSON, or pyo3 → Python
```

Workspace crates:
- `core/`: `Node`, `Edge`, `QName`, `RepoId`, shared primitives.
- `code-domain/`: code-specific registries (49 NodeKind, 36 EdgeCategory, 25 CellType IDs) and the code domain's profile tables.
- `parsers/code/<lang>/`: one crate per language. `parsers/code/extractors/` for cross-cutting (gRPC, queues, WebSocket, EventBus, GraphQL, CLI, data-sources, data-entities, cron, config, IaC, packages, ts-routes, React, Angular, Vue).
- `graph/`: per-repo builder, MergedGraph, all 15 cross-graph resolvers, blast radius.
- `engine/`: orchestration (walk, parse, build, passes, persist) and every answer primitive. Used by `py/` and `cli/`.
- `store/`: `.gmap` container (rkyv + mmap, atomic write).
- `projection-text/`: dense sigil projection for LLM context.
- `activation/`: Personalised PageRank, the pass registry, the domain profile and the generic algorithms (reachability, graph delta, SCC), all domain-agnostic.
- `doc-sources/`: Confluence ingestion behind `glia docs`. `snapshots/`: the git-history and test-report snapshot writers. `stamp/`: the build identity that keys the parse cache.
- `toy-domain/`: a test-only second domain proving the domain seam; never a dependency of a shipped crate.

## Compared to

| Tool | What it does well | What glia adds |
|---|---|---|
| Sourcegraph / ctags | intra-repo symbol search at scale | cross-service edges (HTTP/gRPC/queue/shared-DB), declarative resolver layer |
| CodeQL / Semgrep | deep semantic per-file analysis, custom rules | wider substrate (more languages, more frameworks, less depth per query), works out of the box |
| Apiiro / Endor / Snyk | dependency-graph + vuln matching | the dependency graph as one layer of a cross-service graph (shared packages across services, next to IaC, config and queue links); no vulnerability matching by design ([SECURITY.md](./SECURITY.md)) |
| Codebase-Memory MCP | LLM-targeted graph of one codebase | multi-repo merge + cross-service resolvers, pure-Rust core |
| SocratiCode | LLM-driven code Q&A | structural index, not LLM-derived; deterministic, repeatable |
| Backstage / service catalog | curated org-level service registry | derived from source + manifests automatically, no curation step |

What glia does NOT do:
- No data-flow / taint analysis. A deliberate non-goal, not a roadmap gap: glia maps structure, not exploitability ([SECURITY.md](./SECURITY.md)).
- No vulnerability matching against CVE feeds, and no joining package or call reachability to them. Also deliberate ([SECURITY.md](./SECURITY.md)).
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
| 45-repo cross-service eval (v0.4.x) | 13,371 nodes / 14,105 edges / 2,789 cross-edges in 3.1s |
| Substrate failures across 99 repos | 0 generate failures, 0 timeouts |
| Parallel build, 0.5.0 (LG.1a–c; 16 cores, release `glia analyze --format json`, min of 5 runs, `GLIA_THREADS=1` → default) | grpc-go 3.06s → 0.79s; glia 4.35s → 1.23s; quokka-stack 0.45s → 0.11s |

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

## Roadmap

**v0.4.x (released; the last is 0.4.18, on PyPI as `repo-graph-py`):** substrate, CLI, pyo3 wheel, GHA wheel matrix, and the answer primitives `blast-radius`, `trace`, `resolve`, `coverage` and `docs-for`. v0.4.13a/b/c/d ran the parked SWE-bench latent-injection arm (marshmallow-1359 SOLVE).

**0.5.0, the leap (landed on `main`; the version bump, tag and publish are the release gate):** the one breaking release. Every id, qname, `.gmap` format and API break ships at once, and glia, repo-graph and Engram move to it together. It prepares glia for a second domain; it does not ship one. Scope and decisions: [`dev-notes/next-leap-0.5.0.md`](./dev-notes/next-leap-0.5.0.md); every packet: [`dev-notes/leap-packets.json`](./dev-notes/leap-packets.json).
- **The 2026-09 programme** (there is no 0.4.19, so it ships here): walk gating, `.gitignore`, project roots and `--scope`; `glia arch`, `glia projects` and `glia contracts`; C# Refit, `.proto` and Kafka; the coverage matrix derived from graded fixtures. Record: [`dev-notes/wave-plan-2026-09-16.md`](./dev-notes/wave-plan-2026-09-16.md).
- **A. Extraction depth.** Rust path calls, `use` trees and typed receivers (LA.1, LA.35); calls inside macro arguments (LA.2); inline `mod {}` blocks and enum variants as nodes (LA.3, LA.30); receiver-type inference through field, constructor and local types in TS, Java, C#, Python, Ruby, Go and Kotlin (A6.2, A14.3, LA.23), and self / lexical-scope calls in Dart and Swift (LA.34, LA.36); heritage through `UnresolvedRef`, interface method tables with method-level IMPLEMENTS, Go implicit interfaces (A6.3–A6.6, LD.7); DI in TS, Java, C#, FastAPI and PHP (A7.1–A7.5, A7.8); eight ORMs, migrations and DDL, secrets and feature flags (A13.1, A13.2, A13.8–A13.17); Kotlin as its own parser with Spring, Ktor and Retrofit (A14.1–A14.6); frontend NAVIGATES_TO links and page flow (LA.6); Connect and Twirp RPC (LA.17); WebSocket, cron and CLI breadth (LA.18–LA.20, A13.4); client ENDPOINTs from Java HTTP clients, Feign / Retrofit interfaces and Clojure (LA.22); queue topics through constants and consumer callbacks (LA.4, LA.33); OpenAPI annotations and JSON Schema as contracts (LA.15, LA.16); docstrings, Elixir `@doc` and NatSpec tags as doc cells (LA.7, LA.8); service stereotypes as ROLE cells (LA.21); single-directory builds pair stack edges (LA.10); tsconfig `paths` (A6.8); Go packages and go.mod roots (LA.13); needle precision for GraphQL, events, Cypher and SQL literals (LA.26–LA.29, LA.38, LA.39, LA.41, LA.42, LG.3b).
- **B. Identity.** A path-independent RepoId, so node ids survive a clone, a worktree or a relative path (LB.1); no doubled file-stem segment in Java, Scala, PHP, Swift and C# qnames (LB.2, LB.7, LB.14); same-qname framework overlays folded into their declaration as a ROLE cell (LB.3); an owner segment on route and channel qnames under nested project roots, with project-level host narrowing (LB.4, LB.8); one leading slash on every ROUTE / ENDPOINT (LB.5); move-stable identity and move detection (LB.6); file-named MODULEs for same-stem files and C/C++ (LB.9, LB.10, LB.13); per-(method, path) ROUTE identity (LB.11); contract ops and doc sections scoped by directory + stem (LB.12).
- **C. Store format.** `.gmap` FORMAT_VERSION 2 and manifest schema 2 behind a `GLIAGMAP` preamble, so an old file says "rebuild the graph" (LC.1); cells on edges (LC.2) and an EVIDENCE cell on every edge: emitter, rule, call-site line (LC.3); a self-describing header and `glia inspect` (LC.4); domain-owned container sections (LC.5); interface tables, repo labels, parse errors and graph properties persisted (LC.6, LC.7); `load_from_gmap` rebuilds a stale or old-format layout instead of raising (LC.8); one on-disk layout at `<repo>/.glia/graph/` (LC.9); `glia merge` of pre-built layouts and workspace manifests (LC.10); the parse cache written only when it changed (LC.11).
- **D. API contract and cross-domain prep.** A 1-based `line` in every answer (LD.1); native Python returns and one `find` (LD.2); ranked `find` and traversal over the merged graph (LD.3); ranked trace paths and entry flows (LD.4); multi-seed blast radius (LD.5); one entrypoint set and `entry_kinds()` (LD.6); `implementors` (LD.7); absence answers and `serves` (LD.8); `#[non_exhaustive]` public result types (LD.9); a domain-free `core` (LD.10); the rename to `glia-*` crates / `glia_*` paths and the `glia-py` wheel (LD.11); activation hooks in one `ActivationPlan`, the `driver` feature renamed `research` (LD.12); pass composition (LD.13); the domain profile (LD.14); generic algorithms in `activation::algo` (LD.15); a test-only second domain proving the seam (LD.16).
- **E. What a change did, with evidence.** `glia delta` (LE.1), `diff-impact` (LE.2), `tests-for` and the TEST cell (LE.3), `effects` with function-level data access and config reads (LE.4), `why` (LE.5), `cycles` (LE.6), `patterns --experimental` (LE.7), `check` (LE.8), `spec-status` (LE.9), `contracts --fields` (LE.10).
- **F. Inputs from outside the source.** The cell write API and `glia cell` (LF.1); `.glia/overlay.toml` edges, wrappers, constants and route prefixes, `glia gaps` and `--no-overlay` (LF.2); walk config and declared entrypoints (LF.3); declared constraints, decisions and notes, and ADRs (LF.4); `glia history sync`: churn, blame, CO_CHANGES and the co-change-without-edge audit (LF.5); `glia tests ingest`: FAIL and COVERAGE cells (LF.6). Each input is a snapshot or sidecar written by its own step, so the build stays deterministic.
- **G. Perf, hygiene and handoffs.** The engine's thread pool and `GLIA_THREADS` (LG.1); `install-hooks --pair` (LG.2); `glia flows --features` and the quokka / lapse dogfood (LG.3); [SECURITY.md](./SECURITY.md) and this refresh (LG.4); pyo3 and CLI surface snapshots, the pre-leap `.gmap` compat fixtures, and the neuropil and engram-export compile checks (LG.6, LG.13); end-inclusive markdown section lines (LG.10a); Engram contract v6 with line-anchored spans, deterministic bytes, an incremental `--since` diff, move-stable identity, NatSpec facts and Documents edges (LG.7-LG.12); the repo-graph, neuropil and Engram handoff docs (LG.5a, LG.5b, LG.14); and `skills/glia/SKILL.md`, the non-MCP way in (LG.15).

**Later (not in 0.5.0):** `.graphqls` routing; Go routers held on a struct; WebSocket / GraphQL / gRPC client host narrowing; communities, duplicate-flow detection and hubs; dominators (once middleware is extracted and the security gate is ruled on); a RuntimeZone resolver; cross-repo node dedupe; Notion / wiki doc adapters; LSP; per-graph-area rebuilds, if a big repo is slow after LG.1; org-internal-package routing (sibling-repo imports); one dispatcher for the non-tree-sitter file branches and one shared home for the duplicated extractor helpers (`looks_like_url_path`); query-specific distillation over composition / synth cells / vectors (the research direction in Experimental notes). The per-language gaps the spec runs found are in §7.6 of the leap doc. A non-code domain builds on the 0.5.0 seam (header registries, container sections, the domain profile, pass composition); none is scheduled.

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
