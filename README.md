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

**Queues, jobs, events and other channels** across those languages: Kafka, RabbitMQ, SQS / SNS, GCP Pub/Sub, Azure Service Bus, NATS, MQTT, Redis lists and pub/sub, JMS, Celery, Dramatiq, Sidekiq, Oban and BullMQ, including Go broker clients (streadway / amqp091-go, azservicebus, cloud.google.com/go/pubsub, go-redis, aws-sdk-go SQS / SNS, confluent-kafka-go, kafka-go `Writer` literals), JVM and .NET broker builders (Azure Service Bus Java builders, AWS SDK v2 request builders, MQTTnet, Spring Cloud GCP `PubSubTemplate`, Spring Data Redis lists, NATS.Net v2, Google.Cloud.PubSub.V1), and the JobRunr, Hangfire and asynq task queues, whose job is named by the method or task type it runs. In-process event buses, Go's asaskevich/EventBus included. GraphQL servers and clients, HotChocolate code-first resolvers and Go machinebox/graphql / C# GraphQL.Client requests included. Cron schedules, NestJS `@Cron` / `@Interval` included. CLI invocations, JVM `ProcessBuilder` / `Runtime.exec` launches included. HTTP clients, Dart Dio generic calls (`dio.get<T>(..)`) and Dio base URLs included.

**Inside a language** the graph builder resolves calls, imports and heritage per language, beyond plain names. Two examples: in the TypeScript family a `this.m()` / `super.m()` binds through superclasses, and an override of an abstract member IMPLEMENTS it. In Go, struct embeds are INHERITS_FROM and their methods are promoted to calls and interface satisfaction; in-repo type aliases are resolved before signatures are compared; package-var initialisers call; repository-root package imports are recorded. A call on a freshly constructed object (`new Calc().add(..)`) binds through the constructed type. Every language gets function-level TESTS edges from a test module's root functions.

**15 cross-graph resolvers** that pair entities across repo boundaries:
HTTP (frontend Endpoint ↔ backend Route), gRPC (client ↔ proto service), RPC (tRPC / Connect / Twirp call ↔ procedure), Queue (producer ↔ consumer, including raw Redis lists), GraphQL, WebSocket, EventBus, CLI invocation ↔ command, shared message schemas (proto / Avro / JSON Schema types), shared schema imports, shared data entities (SQL Tables / NoSQL Collections / Graph-DB Labels), Cron schedules, Config keys (env vars across services), IaC resources (Dockerfile-built images ↔ k8s manifest references), Package dependencies.

**Non-source files** flow through bypass extractors: YAML (`.github/workflows/`, k8s manifests, docker-compose, OpenAPI / AsyncAPI), Dockerfiles, `.env` files, package manifests (`package.json`, `pyproject.toml`, `requirements.txt`, `Cargo.toml`, `go.mod`, `Gemfile`, `composer.json`), migration `.sql`, Prisma schemas, `.proto`, `.graphql`, Avro `.avsc`, JSON Schema and contract `.json`, .NET `appsettings*.json` (every key a `config:setting:<Section:Key>` that C# `IConfiguration` reads pair with, and an env define `Section__Key` defines it too), and markdown docs. Markdown is read from the well-known files (`README*.md`, `ARCHITECTURE.md`, `CLAUDE.md`, ...) and the `docs/` tree at the repo root and at every project root, a top-level `.ai/` tree, ADR directories, SDD feature docs (`features/<feature>/*.md`, spec-kit `specs/<NNN-slug>/`) and synced external docs (`glia docs sync`). A section documents a symbol it names in backticks; `glia coverage` states that scope.

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

**Use without MCP.** An agent can also call the CLI directly, one `glia <command> <repo> --json` per question. [`skills/glia/SKILL.md`](./skills/glia/SKILL.md) is a Claude Code skill that teaches this. It maps each question to its command and shows how to read an answer (`file:line` rows, absences, blind spots), with one worked example per command. To install it, copy it to `~/.claude/skills/glia/` or `<repo>/.claude/skills/glia/`. A second skill, [`skills/glia-overlay/SKILL.md`](./skills/glia-overlay/SKILL.md), is the model step of the overlay loop: it reads the gaps `glia overlay propose` lists, writes candidate `.glia/overlay.toml` stanzas from the code, measures them with `glia overlay try` and accepts only what the verdict keeps. Install it the same way, to `~/.claude/skills/glia-overlay/SKILL.md`, or to `<repo>/.claude/skills/glia-overlay/` for one repo. No server stays resident: each call builds the graph in memory and exits. `cli/tests/skill_surface.rs` checks every command and flag both skills name against `cli/surface/`.

## CLI

Every subcommand and flag of `glia`, written from the committed CLI surface snapshots in `cli/surface/` (LG.6a) and held to them by `cli/tests/readme_surface.rs`: every command has a usage line, every flag a line shows is declared and every flag a command declares is shown, and every `a|b` value set is the snapshot's. `glia <command> --help` has the full text. `--with <repo>` (repeatable) merges more repos in first, so the resolvers pair across them; `--json` prints JSON instead of tables; `--scope` takes a repo-relative path or a project label from `glia projects`. The global `--no-overlay` builds without `.glia/overlay.toml`'s `[[edge]]` stanzas, the extraction-only graph ([docs/overlay.md](./docs/overlay.md)).

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
glia cache push <REPO> <STORE> [--json] [--key-file <FILE>] [--layout] [--unsigned]
    Upload a built checkout's parse cache to an object store, one signed object per
    content address, skipping what the store holds. STORE is a directory or an
    https:// URL (PUT, `Authorization: Bearer $GLIA_CACHE_TOKEN` when set; plain
    http:// to a loopback host only). The MAC key is GLIA_CACHE_KEY or --key-file;
    --unsigned trusts the store as-is. --layout first uploads the whole .glia/graph/
    layout of a clean checkout whose layout is fresh. See docs/cache.md.
glia cache pull <REPO> <STORE> [--jobs <JOBS>] [--json] [--key-file <FILE>] [--layout] [--unsigned] [--verify <N|all>]
    Fetch the parses this checkout's cache lacks, check each object (size, key,
    MAC), re-parse a sample (--verify; 32 by default when --unsigned) and write
    <repo>/.glia/graph/parse_cache.bin, which the next build reuses. Exits 1 and
    writes nothing when a sample differs from a local parse. --layout first
    installs the finished layout the store holds for this clean checkout, keyed by
    build stamp, repo identity, HEAD tree, .glia inputs, target and walk digest.
    The build itself never touches the network.
glia cache gc <STORE> [--json] [--keep-stamps <KEEP_STAMPS>] [--max-bytes <BYTES>]
    Prune a directory store: keep the newest --keep-stamps build stamps, then drop
    the oldest objects over --max-bytes (K / M / G suffixes).

# The whole stack
glia arch <REPO> [--include-shared] [--json] [--mermaid] [--with <WITH>]...
    The services and the cross-service links between them, each with its mechanism
    and channel. One repo keys services by top-level directory; SHARES_* and
    DOCUMENTS links show only with --include-shared.
glia projects <REPO> [--json] [--with <WITH>]...
    The manifest-rooted sub-projects (label, ecosystem, path): the --scope vocabulary.
glia coverage <REPO> [--json] [--with <WITH>]...
    Per-language extraction caveats and edges found, plus the co-change-without-edge
    audit, so a grep fallback is a deliberate choice. Two (*, DOCUMENTS) rows: the
    contract-JSON limits, and which markdown is read at all (the well-known files and
    docs/ trees at the repo and project roots, .ai/, ADR dirs, SDD feature docs) with
    the single-backtick mention rule.
glia gaps <REPO> [--category <CATEGORY>] [--json] [--overlay-delta] [--top-k <TOP_K>] [--with <WITH>]...
    Ranked blind spots (unpaired / ambiguous / unresolved endpoints, uncalled routes,
    tag-only queues, dead symbols (never an implementation of a method something
    calls, which dispatch reaches), co-change without an edge, suspected edges,
    overlay rot), each with a stable gap id and the overlay section that could
    repair it. suspected_edge rows (heuristic) print a paste-ready [[edge]] stanza
    after their table ("paste into <repo>/.glia/overlay.toml ..."). --overlay-delta:
    what an overlay edit did, with a keep / review / drop verdict. Exits 0.
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
glia communities <REPO> [--json] [--members <MEMBERS>] [--method leiden|lpa] [--resolution <RESOLUTION>] [--scope <SCOPE>] [--seed <SEED>] [--top <TOP>] [--with <WITH>]...
    The graph's communities by seeded Leiden (label propagation above its pair cap,
    or exactly --method), largest first: each with its size, cohesion, node kinds,
    `glia arch` services, effect sinks, located entry points, top members and
    heaviest links to other communities. Heuristic. A report: exits 0.
glia splits <REPO> [--from <FROM> --to <TO>] [--json] [--min-share <MIN_SHARE>] [--parts <PARTS>] [--quotient module|community] [--scope <SCOPE>] [--seed <SEED>] [--with <WITH>]...
    Where a scope splits into services at the least coupling: the module (or
    community) quotient cut by Stoer-Wagner into --parts parts (2..=8) above a
    balance floor, or with --from / --to the minimum cut separating two sides. The
    parts, the cut edges located at their evidence sites, the blockers (data
    entities two parts write, cycles between parts) and each part against
    `glia arch`. A suggestion, never a verdict (heuristic); exits 1 with no cut.
glia hubs <REPO> [--category <CATEGORY>] [--include-tests] [--json] [--min-degree <MIN_DEGREE>] [--scope <SCOPE>] [--top <TOP>] [--with <WITH>]...
    The nodes carrying the most structural load, three ranked lists: fan-in
    (utilities), fan-out (orchestrators) and cross-service connectors, each located
    with its degree, HITS authority / hub scores and label. Counts every carry edge
    but tests, docs and manifest dependencies, or one --category; test nodes only
    with --include-tests. Derived. Exits 1 with no rows.
glia duplicate-flows <REPO> [--depth <DEPTH>] [--include-tests] [--json] [--keep-hubs] [--min-size <MIN_SIZE>] [--scope <SCOPE>] [--threshold <THRESHOLD>] [--with <WITH>]...
    Entry points whose forward flows reach the same nodes: exact groups (derived: an
    aliased route, a v1 / v2 pair never retired) and near groups (heuristic:
    MinHash / LSH, verified at Jaccard >= --threshold, default 0.8), each with the
    nodes it differs by. A report: exits 0, 2 on a --threshold outside (0, 1].
glia hotspots <REPO> [--include-tests] [--json] [--level module|symbol|both] [--min-churn <MIN_CHURN>] [--scope <SCOPE>] [--top <TOP>] [--with <WITH>]...
    Modules and symbols that change often AND sit where much depends on them: git
    churn rank beside PageRank centrality rank (the `centrality` preset), no
    composite score. Reads the history snapshot (`glia history sync`, `--blame` for
    symbols). Heuristic. Exits 1 with no rows (no_history / no_match).

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
    HTTP channel also lists, after the routes, each third-party ENDPOINT the code calls
    on that verb and path (match `external`, `external_hosts` in --json). An empty
    answer is a FACT with the mechanism's caveats and near misses.
glia effects <REPO> <QNAMES>... [--class <CLASS>]... [--cross-service] [--depth <DEPTH>] [--json] [--scope <SCOPE>] [--with <WITH>]... [--writes-only]
    The effect sinks downstream of the seeds (DB read / write, queue produce, outbound
    HTTP / RPC / WS / GraphQL call, event emit), each with its witness path. A call to a
    third-party host names it (`external: <host>`, `external_hosts` in --json).
glia docs-for <REPO> <QNAME> [--json] [--scope <SCOPE>] [--with <WITH>]...
    The doc sections that DOCUMENT a symbol, from the markdown the build reads: the
    well-known files and docs/ trees at the repo root and every project root, .ai/,
    ADR dirs, SDD feature docs (features/<f>/*.md, spec-kit specs/<NNN-slug>/) and
    synced docs (`glia docs sync`).
glia pack <REPO> <QUERY> [--budget <BUDGET>] [--bytes-per-token <BYTES_PER_TOKEN>] [--candidates <CANDIDATES>] [--json] [--preset repair|review|onboard|centrality] [--scope <SCOPE>] [--seeds <SEEDS>] [--with <WITH>]...
    The context for a query packed to a token budget (default 8000), ready to paste:
    the seeds `glia find` matches, then their callers and callees by personalised
    PageRank, each at the most detail the budget buys (full source, a preview, an
    outline line or a bare qname), located, with the links between them. stdout is
    the pack only, a summary goes to stderr; --json is the pack with its manifest.
    Exits 1 when the pack is empty (the absence says why).

# A change (git rev or pasted diff)
glia delta <REPO> [--base <BASE>] [--category <CATEGORY>]... [--edges-only] [--json]
    What the working tree's change did to the graph against a git rev (default HEAD):
    nodes and edges added, removed, modified or moved, located.
glia diff-impact <REPO> [--base <REV>] [--depth <DEPTH>] [--diff <FILE>] [--direction forward|backward|both] [--json] [--live-only] [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    The changed nodes (--base rev or --diff file) and one ranked blast radius around
    them, each row naming the change that reached it.
glia tests-for <REPO> [QNAMES]... [--base <REV>] [--depth <DEPTH>] [--diff <FILE>] [--files-only] [--json] [--limit <N>] [--no-module-level] [--no-signals] [--scope <SCOPE>] [--with <WITH>]...
    The tests to run for a change (seed qnames, --diff or --base), tiered FACT /
    DERIVED / HEURISTIC and ranked by the test and history snapshots' signals (failed
    in recent runs, on a failing trace, co-changes with a seed's module; a test found
    only by co-change is a heuristic `cochange` row). --no-signals ranks structurally
    only; --limit keeps the first N; --files-only prints test files for a runner.
glia patterns <REPO> [--base <REV>] [--group-by service|package] [--json] [--min-share <MIN_SHARE>] [--min-support <MIN_SUPPORT>] [--scope <SCOPE>] [--with <WITH>]...
    Pattern conformance: per service (or package), each route handler's role chain
    to its first effect sink; the chain held by --min-share percent of at least
    --min-support sighted handlers is the convention, and every handler off it a
    located DIVERGENCE. Blind handlers are listed, never counted. Counts, not rules;
    --base keeps the divergences the change touched.
glia review <REPO> [--base <REV>] [--depth <N>] [--json] [--markdown-rows <N>] [--max-impact <N>] [--max-tests <N>]
    The PR report for the working tree's change against a rev (default HEAD): the
    changed nodes, their ranked impact, the tests to run, every added / removed edge
    with its tier, and the declared rules checked on both sides. Markdown for a PR
    comment. A CI gate: exits 1 only on a NEW violation.
glia contract-breaks <REPO> [--avro backward|forward|full] [--base <REV>] [--breaking-only] [--json] [--with <REPO>]...
    Did the change against a rev (default HEAD) break a client? Every OpenAPI /
    AsyncAPI op and proto / Avro / JSON Schema type paired old -> new and judged by
    its format's evolution rules, one row per field change; every client whose call
    lost its provider is orphaned (--with adds client repos). A CI gate: exits 1 on
    a breaking change or an orphaned client, 0 otherwise.
glia cochange <REPO> [FILES]... [--base <REV>] [--json] [--min-confidence <SHARE>] [--min-support <N>] [--top <N>] [--unlinked-only] [--with <WITH>]...
    What usually changes with these files (or the working tree's change against
    --base): each with a directional confidence (`0.80 (4/5)`), its support and
    whether a static link joins them (`none`: a blind spot). Needs a `glia history
    sync` snapshot. Heuristic; exits 1 with no rows. --base refuses --with and
    --no-overlay.
glia timeline build <REPO> [--head <HEAD>] [--json] [--revs <REVS>]
    Build the last --revs commits (default 20, at most 200) of --head's first-parent
    chain, one incremental build each on the shared parse cache, into the sidecar
    <repo>/.glia/graph/timeline.gmap.
glia timeline history <REPO> <QNAME> [--category <CATEGORY>] [--json]
    Every edge span that ever touched the node: since which rev, until which rev or
    still present, a file move followed. Before any build it exits 0 with the note
    naming `glia timeline build`.
glia timeline as-of <REPO> <REV> [--json]
    The graph at one rev of the window (an index, or a commit id prefix of 7+ hex
    chars): its counts by category. history and as-of read the sidecar, never rebuild.

# Rules
glia check <REPO> [--json] [--with <WITH>]...
    Evaluate .glia/overlay.toml's [[constraint]] rules (forbid_edge, no_cycle) and its
    reflexion model ([[component]], [[layer]], `allow` rules: the component matrix,
    divergences, absences, unmapped files) to located, tiered VIOLATIONs. Exits 0
    clean, 1 on violations, 2 on an error: CI-ready.
glia flags <REPO> [--json] [--quiet-days <QUIET_DAYS>] [--scope <SCOPE>] [--status dead|undefined|single_site|quiet]... [--with <WITH>]...
    Stale feature flags (LaunchDarkly / OpenFeature / Unleash / Flagsmith / Split
    reads, Flipt definitions), each with its readers and definitions: dead and
    undefined (derived), single_site (fact), quiet (heuristic: every reader unchanged
    for --quiet-days, from `glia history sync --blame`). A report: exits 0.

# Inputs from outside the source: each writes a .glia/ snapshot or sidecar that the
# next build reads. The build itself never fetches, syncs or ingests.
glia docs sync <REPO> [--source confluence] --space <SPACE> [--email <EMAIL>] [--exclude <PATTERN>]... [--include <PATTERN>]... [--site <SITE>] [--token <TOKEN>]
    Pull a Confluence space into <repo>/.glia/docs-snapshot/ (network step);
    credentials from the flags, then CONFLUENCE_SITE / CONFLUENCE_EMAIL /
    CONFLUENCE_TOKEN, then ./.env. --include / --exclude keep or drop pages by
    title, for every source.
glia docs sync <REPO> --source dir --path <DIR> [--container <NAME>] [--url-base <URL>] [--exclude <PATTERN>]... [--include <PATTERN>]...
    A local GitHub / GitLab wiki checkout's Markdown pages (no dot-entries,
    _-prefixed files or symlinks); the container defaults to the directory's name.
glia docs sync <REPO> --source mediawiki --api <URL> --namespace <N> | --category <NAME> [--container <NAME>] [--max-pages <N>] [--token <TOKEN>]
    One namespace or category of a MediaWiki through its Action API (never by
    following links); a bearer token from --token or MEDIAWIKI_TOKEN.
glia docs sync <REPO> --source notion --database <ID> | --page <ID> [--api <URL>] [--max-pages <N>] [--token <TOKEN>]
    A Notion database's pages, or a page and its sub-pages, blocks converted to
    Markdown; the integration token from --token or NOTION_TOKEN.
glia docs push --file <FILE> --space <SPACE> --title <TITLE> [--email <EMAIL>] [--markdown] [--page-id <PAGE_ID>] [--site <SITE>] [--token <TOKEN>]
    Create or update (--page-id) a Confluence page; --markdown converts the file first.
glia history sync <REPO> [--blame] [--blame-max-files <BLAME_MAX_FILES>] [--max-commits <MAX_COMMITS>] [--since <SINCE>]
    Read the local git history into <repo>/.glia/history-snapshot/: churn and blame
    ATTN cells, CO_CHANGES edges. No author identities are stored.
glia tests ingest <REPO> [--junit <PATH>...] [--lcov <PATH>...] [--log <PATH>...] [--reset] [--run <LABEL>] [--window <N>]
    Add one CI run's JUnit XML, CI logs and lcov to <repo>/.glia/test-snapshot/ as
    its newest run; the snapshot keeps the last --window runs (default 10; --reset
    drops the earlier ones): FAIL cells per test (fails in the window, latest run)
    and COVERAGE cells. Messages are redacted for secrets before they are stored.
glia scip import <REPO> <INDEX> [--prefix <PREFIX>]
    Decode a SCIP index (scip-python, scip-typescript, scip-java, scip-go,
    rust-analyzer) into <repo>/.glia/scip-snapshot/; the next build binds it as
    FACT-tier CALLS / USES / IMPLEMENTS / INHERITS_FROM edges and confirms name-only
    ones. --prefix: the directory the index's paths are relative to. Exits 1 when
    the index cannot be decoded (nothing written).
glia overlay propose <REPO> [--category <CATEGORY>]... [--json] [--snippet-lines <SNIPPET_LINES>] [--top-k <TOP_K>] [--with <WITH>]...
    The overlay loop's work list: the gaps an overlay stanza could close, per
    category, each with its gap id, location and source lines, and a suspected
    edge's paste-ready [[edge]]. Writes nothing.
glia overlay try <REPO> --candidate <CANDIDATE> [--json] [--no-leave-one-out] [--with <WITH>]...
    Build a candidate stanza file against the repo (base, with it, and without each
    stanza in turn) and report what each stanza changes and a keep / review / drop
    verdict. --no-leave-one-out: two builds, no per-stanza attribution.
glia overlay accept <REPO> [--candidate <CANDIDATE>] [--dry-run] [--json] [--only <ONLY>]... [--remove <REMOVE>]...
    Merge chosen candidate stanzas (--only `edge#2`) into .glia/overlay.toml, or
    remove orphaned / redundant rules by gap id, validated and written atomically:
    the only writer of that file. Exits 0 written (or --dry-run), 1 when validation
    refuses, 2 on an error. The overlay commands refuse --no-overlay.
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

**0.5.0, the leap (released 2026-09-20: tag `v0.5.0`, `glia-py` 0.5.0 on PyPI):** the one breaking release. Every id, qname, `.gmap` format and API break ships at once, and glia, repo-graph and Engram move to it together. It prepares glia for a second domain; it does not ship one. Scope and decisions: [`dev-notes/next-leap-0.5.0.md`](./dev-notes/next-leap-0.5.0.md); every packet: [`dev-notes/leap-packets.json`](./dev-notes/leap-packets.json).
- **The 2026-09 programme** (there is no 0.4.19, so it ships here): walk gating, `.gitignore`, project roots and `--scope`; `glia arch`, `glia projects` and `glia contracts`; C# Refit, `.proto` and Kafka; the coverage matrix derived from graded fixtures. Record: [`dev-notes/wave-plan-2026-09-16.md`](./dev-notes/wave-plan-2026-09-16.md).
- **A. Extraction depth.** Rust path calls, `use` trees and typed receivers (LA.1, LA.35); calls inside macro arguments (LA.2); inline `mod {}` blocks and enum variants as nodes (LA.3, LA.30); receiver-type inference through field, constructor and local types in TS, Java, C#, Python, Ruby, Go and Kotlin (A6.2, A14.3, LA.23), and self / lexical-scope calls in Dart and Swift (LA.34, LA.36); heritage through `UnresolvedRef`, interface method tables with method-level IMPLEMENTS, Go implicit interfaces (A6.3–A6.6, LD.7); DI in TS, Java, C#, FastAPI and PHP (A7.1–A7.5, A7.8); eight ORMs, migrations and DDL, secrets and feature flags (A13.1, A13.2, A13.8–A13.17); Kotlin as its own parser with Spring, Ktor and Retrofit (A14.1–A14.6); frontend NAVIGATES_TO links and page flow (LA.6); Connect and Twirp RPC (LA.17); WebSocket, cron and CLI breadth (LA.18–LA.20, A13.4); client ENDPOINTs from Java HTTP clients, Feign / Retrofit interfaces and Clojure (LA.22); queue topics through constants and consumer callbacks (LA.4, LA.33); OpenAPI annotations and JSON Schema as contracts (LA.15, LA.16); docstrings, Elixir `@doc` and NatSpec tags as doc cells (LA.7, LA.8); service stereotypes as ROLE cells (LA.21); single-directory builds pair stack edges (LA.10); tsconfig `paths` (A6.8); Go packages and go.mod roots (LA.13); needle precision for GraphQL, events, Cypher and SQL literals (LA.26–LA.29, LA.38, LA.39, LA.41, LA.42, LG.3b).
- **B. Identity.** A path-independent RepoId, so node ids survive a clone, a worktree or a relative path (LB.1); no doubled file-stem segment in Java, Scala, PHP, Swift and C# qnames (LB.2, LB.7, LB.14); same-qname framework overlays folded into their declaration as a ROLE cell (LB.3); an owner segment on route and channel qnames under nested project roots, with project-level host narrowing (LB.4, LB.8); one leading slash on every ROUTE / ENDPOINT (LB.5); move-stable identity and move detection (LB.6); file-named MODULEs for same-stem files and C/C++ (LB.9, LB.10, LB.13); per-(method, path) ROUTE identity (LB.11); contract ops and doc sections scoped by directory + stem (LB.12).
- **C. Store format.** `.gmap` FORMAT_VERSION 2 and manifest schema 2 behind a `GLIAGMAP` preamble, so an old file says "rebuild the graph" (LC.1); cells on edges (LC.2) and an EVIDENCE cell on every edge: emitter, rule, call-site line (LC.3); a self-describing header and `glia inspect` (LC.4); domain-owned container sections (LC.5); interface tables, repo labels, parse errors and graph properties persisted (LC.6, LC.7); `load_from_gmap` rebuilds a stale or old-format layout instead of raising (LC.8); one on-disk layout at `<repo>/.glia/graph/` (LC.9); `glia merge` of pre-built layouts and workspace manifests (LC.10); the parse cache written only when it changed (LC.11).
- **D. API contract and cross-domain prep.** A 1-based `line` in every answer (LD.1); native Python returns and one `find` (LD.2); ranked `find` and traversal over the merged graph (LD.3); ranked trace paths and entry flows (LD.4); multi-seed blast radius (LD.5); one entrypoint set and `entry_kinds()` (LD.6); `implementors` (LD.7); absence answers and `serves` (LD.8); `#[non_exhaustive]` public result types (LD.9); a domain-free `core` (LD.10); the rename to `glia-*` crates / `glia_*` paths and the `glia-py` wheel (LD.11); activation hooks in one `ActivationPlan`, the `driver` feature renamed `research` (LD.12); pass composition (LD.13); the domain profile (LD.14); generic algorithms in `activation::algo` (LD.15); a test-only second domain proving the seam (LD.16).
- **E. What a change did, with evidence.** `glia delta` (LE.1), `diff-impact` (LE.2), `tests-for` and the TEST cell (LE.3), `effects` with function-level data access and config reads (LE.4), `why` (LE.5), `cycles` (LE.6), `patterns` (LE.7; experimental until CC.12b promoted it in 0.5.1), `check` (LE.8), `spec-status` (LE.9), `contracts --fields` (LE.10).
- **F. Inputs from outside the source.** The cell write API and `glia cell` (LF.1); `.glia/overlay.toml` edges, wrappers, constants and route prefixes, `glia gaps` and `--no-overlay` (LF.2); walk config and declared entrypoints (LF.3); declared constraints, decisions and notes, and ADRs (LF.4); `glia history sync`: churn, blame, CO_CHANGES and the co-change-without-edge audit (LF.5); `glia tests ingest`: FAIL and COVERAGE cells (LF.6). Each input is a snapshot or sidecar written by its own step, so the build stays deterministic.
- **G. Perf, hygiene and handoffs.** The engine's thread pool and `GLIA_THREADS` (LG.1); `install-hooks --pair` (LG.2); `glia flows --features` and the quokka / lapse dogfood (LG.3); [SECURITY.md](./SECURITY.md) and this refresh (LG.4); pyo3 and CLI surface snapshots, the pre-leap `.gmap` compat fixtures, and the neuropil and engram-export compile checks (LG.6, LG.13); end-inclusive markdown section lines (LG.10a); Engram contract v6 with line-anchored spans, deterministic bytes, an incremental `--since` diff, move-stable identity, NatSpec facts and Documents edges (LG.7-LG.12); the repo-graph, neuropil and Engram handoff docs (LG.5a, LG.5b, LG.14); and `skills/glia/SKILL.md`, the non-MCP way in (LG.15).

**0.5.1, the catch-up leap (on `main`; the version bump, tag and publish are the release gate):** the 0.5.0 backlog, new answers on the 0.5.0 primitives, graph algorithms, store format 3 and external inputs: 198 packets, 155 in waves W0–W11 and a 43-packet finishing batch in W12–W19. It keeps the name 0.5.1 and consumers pin it exactly, so it may break: each break is declared by its packet and listed in the repo-graph, neuropil and Engram handoff docs. No registry id is allocated; every group reuses existing kinds, categories and cells (C0.1). Scope and James's rulings: [`dev-notes/next-leap-0.5.1.md`](./dev-notes/next-leap-0.5.1.md) §7.2; every packet: [`dev-notes/leap-051-packets.json`](./dev-notes/leap-051-packets.json), with [`dev-notes/leap-051-corrections.json`](./dev-notes/leap-051-corrections.json).
- **0. Wave 0.** The module, CLI and pyo3 slots every later packet fills (C0.2–C0.6) and the one dependency commit: `lz4_flex`, `blake3`, `toml_edit`, `ureq` (C0.7).
- **A. Dogfood fixes.** Go calls inside func literals are CALLS of the enclosing function (CA.1); Go receivers typed from a call's result, parameters, locals and package vars (CA.2a, CA.2b); Go implicit IMPLEMENTS checked by method signature, a one-method interface by package reach (CA.3a, CA.3b); Go collection wrappers inferred without an overlay (CA.4); Go route handlers that are a method value of the receiver (CA.5a); `patterns` counts sighted handlers only and groups by package on request (CA.5b); Kotlin parameter and local receiver types, and Ktor-client ENDPOINTs (CA.6a, CA.6b); the TypeScript family built on the engine pool (CA.7); exact matching in the substrate grader (CA.8); per-phase `[timing]` build timers (CA.9).
- **B. Language and HTTP / RPC / event backlog.** `.graphqls` routed to the SDL scan and C++ header extensions to the C/C++ parser (CB.1); the contract YAML sniff reads every top-level line and needs a version value (CB.2); no verb-named events, and constant-keyed event sites folded to the constant's literal (CB.3a, CB.3b); a decorated TS method anchored at its first decorator (CB.4); in-process events paired across nested projects linked into one process (CB.5); per-scope language facts in CodeNav (CB.6); PHP `use` read from the AST (CB.7); Dart constructors, operators, enum constants, extensions and `as` / `show` / `hide` imports (CB.9, CB.17); Swift infix calls, init / subscript / computed-property bodies and implicit `self` (CB.10, CB.18); Go routers held on a struct field and router mounts resolved at build time (CB.11, CB.20, CB.23); host narrowing as a shared resolver module, then WebSocket, gRPC, GraphQL and tRPC clients narrowed by their dial host or base URL (CB.12, CB.21, CB.24); tRPC server-side callers (CB.13); Python GraphQL field resolvers (CB.14); a namespace PACKAGE resolves each member against its own file's imports (CB.15); Ruby `require` inside a class body (CB.16); C/C++ `extern "C"`, templates, unions and nested types, include search paths and C++ call scope (CB.19, CB.22, CB.25); coverage caveats name what is still partial (CB.26).
- **C. New answers.** One precomputed graph delta behind `diff-impact` and `tests-for` (CC.1); one reader for the history and test signals (CC.2); `check` tiers each violation (CC.3) and evaluates a reflexion model: `[[component]]`, `[[layer]]`, `allow` rules, the component matrix, divergences, absences, unmapped files (CC.5a–c); `glia pack`, context packed to a token budget over a fidelity ladder (CC.4a–c); `glia review`, the PR report that fails CI only on a new violation (CC.6a, CC.6b); `glia flags`, stale feature flags with flag reads homed on the reading function (CC.7a–c); `glia contract-breaks`, format evolution rules against a git rev and orphaned clients, across `--with` client repos (CC.8a–c); predictive test selection, `tests-for` ranked by failure and co-change signals over a rolling window of test runs (CC.9a, CC.9b); `glia hotspots`, churn rank beside centrality rank under the new `centrality` activation preset, which weighs DEFINES / CONTAINS / DOCUMENTS / TESTS at 0 (CC.10a, CC.10b); `glia cochange`, co-change suggestions with directional confidence (CC.11a–c); `patterns` out of experimental against a written promotion criterion, the `_experimental` names kept as DeprecationWarning aliases until 0.5.2 (CC.12a, CC.12b).
- **D. Algorithms and store format 3.** In `activation::algo`: seeded Leiden, label propagation and modularity (CD.1a, CD.1b), Stoer-Wagner and Dinic min cuts (CD.2a), link-prediction scores (CD.3a), degree and HITS (CD.4a), MinHash / LSH (CD.4d), validity intervals over a run of snapshots (CD.5a); the domain profile's `community_weights` (CD.1c). On top of them: `glia communities` (CD.1d, CD.1e), `glia splits` (CD.2b–d), the `suspected_edge` gap with a paste-ready `[[edge]]` stanza (CD.3b, CD.3c), `glia hubs` (CD.4b, CD.4c), `glia duplicate-flows` (CD.4e, CD.4f), and the time-travel graph `glia timeline build|history|as-of` with its `timeline.gmap` sidecar and the commit each layout was built at (CD.5b–d). `glia-graph` depends on no tree-sitter crate outside its tests (CD.6a). The store: an lz4-framed parse cache (CD.7a), `.gmap` FORMAT_VERSION 3 with interned EVIDENCE cells (CD.7b), and CODE cells stored as spans into the source (CD.7c).
- **E. External inputs and caches.** `glia scip import` with a dependency-free SCIP decoder, and a build stage that binds a SCIP index as FACT-tier CALLS / USES / IMPLEMENTS / INHERITS_FROM and confirms name-only edges (CE.1a–e); a shared parse cache of content-addressed, MAC-signed objects over a directory or HTTPS store, `glia cache push|pull|gc` (CE.2a–c, CE.2e), and the whole-layout cache: a clean checkout at a built tree pulls the finished `.glia/graph/` layout (`glia cache push|pull --layout`; key = build stamp, repo identity, HEAD tree, `.glia` inputs, target, walk digest) (CE.2d); the overlay loop: stable gap ids and a keep / review / drop verdict (CE.3a), builds from candidate overlay text and a format-preserving writer (CE.3b), `overlay try` (CE.3c, `glia_engine::overlay_loop::try_candidate`), `propose` and `accept` (CE.3d), `glia overlay propose|try|accept` (CE.3e) and the `glia-overlay` agent skill (CE.3f); doc sources beyond Confluence: source-neutral pages merged by container (CE.4a), a local wiki directory (CE.4b), wikitext to Markdown (CE.4c), MediaWiki's Action API with a bearer token and no cookie login (CE.4d), Notion databases and page trees (CE.4e, CE.4f).
- **F. Matrix probes.** Graded fixtures measure unknown cells of the coverage matrix for Python, Go, TypeScript, Java, C#, Ruby, PHP, Rust, Scala, Dart and Elixir (CF.1–CF.11); an explicit not-applicable list and a Kotlin row (CF.13a, CF.13b).
- **G. From the Engram session.** TS / JS class fields holding an arrow function are METHODs (CG.1); test and fixture provenance by path, and engram-export `--exclude-path` (CG.2a, CG.2b); the doc linker reads a whole markdown section (CG.3); external HTTP endpoints are marked and kept out of pairing (CG.4a, CG.4b).
- **Finishing batch** (after W11, James's 2026-10-01 list, leap doc §7.4; waves W12–W19). One slot: the pyo3 `overlay_loop` module (C0.9).
  - **H. TypeScript, Angular and Dart.** `abstract class` declarations are CLASSes and abstract members METHODs (CH.1). A `this.m()` / `super.m()` the caller's class cannot bind follows INHERITS_FROM to the nearest superclass defining `m`, and an override of an abstract member IMPLEMENTS it (CH.1b, `[ts-inherit]`). `glia gaps` stops listing an implementation of a called method as a dead symbol (CH.1c, `[dead-dispatch]`). Signal and call-initialised class fields (`signal(..)`, `computed(..)`, `input.required<T>()`, `toSignal(..)`) are STATE_VARs that carry their initialiser's CALLS (CH.2). A client call's URL is read through a single-argument URL builder, a same-class URL method, a `const` local or a `readonly` field (CH.3a). A builder that reads one API-prefix member records it (CH.3b). A typed non-HTTP receiver (`this.timers.get(id)` on a `Map`) mints no endpoint (CH.3c). A method passed by value (`addEventListener('resize', this.onResize)`) is USES (CH.4). Dart generic Dio calls are ENDPOINTs, and Dio base URLs are recorded (CH.5a). The endpoint fold prefixes builder-read TypeScript paths (CH.5b) and Dart paths under agreeing Dio base URLs (CH.5c) with the configured `/api`, and a call on a Dio is never a server ROUTE (CH.5c).
  - **I. Go.** Calls in a package-var initialiser are CALLS of the var's STATE_VAR (CI.1). Struct embeds are INHERITS_FROM, and calls bind through promotion (CI.2a). Promoted methods count for implicit IMPLEMENTS, so a gRPC server embedding `pb.UnimplementedXServer` implements `XServer` (CI.2b). Repository-root package imports are recorded, and a `_test.go` side pairs only through a direct import (CI.3). A route handler whose receiver type sits in another file of the package, or whose method is promoted, is HANDLED_BY through the package (CI.4). Router mounts resolve through another package's struct field and through a parameter-rooted field group (CI.5). In-repo type aliases resolve in method signatures before the IMPLEMENTS compare (CI.6, `[go-alias]`).
  - **J. Precision and inputs.** A queue / event (CJ.1a), WebSocket / gRPC / GraphQL-operation (CJ.1b) or cron / data-source / env / secret / flag (CJ.1c) needle inside a Rust or Python string literal or comment is no site (`[code-guard]`). Docs are ingested from every project root's well-known files and `docs/` tree and from SDD feature docs (CJ.2). `effects` and `serves` label third-party sinks (`external_hosts`, CJ.3). `.glia/overlay.toml` `[walk] tests = [..]` declares test paths, which get `test_fixture` provenance (CJ.4).
  - **K. pyo3 and engram-export.** `glia_py.overlay_propose` / `overlay_try` / `overlay_accept` (CK.1). The engram-export `--since` diff pairs moved routes by path and refuses unalike hint pairs (CK.2). `glia-export-engram -V` names the glia build, and every export writes a `<out>.meta.json` sidecar (CK.3).
  - **L. The matrix slice.** A queue framework tag only for an unexplained call, and a RabbitMQ declare is no consumer (CL.1). Go broker rows (CL.2). JVM / .NET broker rows (CL.3). JobRunr / Hangfire / asynq jobs (CL.4). Calls on a constructed object (CL.5a). Function-level TESTS (CL.5b). Go / C# GraphQL client requests (CL.6a) and HotChocolate resolvers (CL.6b). C# environment reads (CL.7a) and .NET `appsettings*.json` with the env cross-define (CL.7b). The Go asaskevich/EventBus (CL.8). NestJS `@Cron` / `@Interval` (CL.9). JVM process launches (CL.10). Two probe fixes (CL.11).
- **Z. This refresh.** README `## CLI` held to `cli/surface/` by `cli/tests/readme_surface.rs` (CZ.1); CLAUDE.md and the `glia` skill (CZ.2); the finishing batch's docs and the three consumer handoffs (CZ.3).

**Next: 0.5.2, four bets** (research in [`dev-notes/research-0.5.2/`](./dev-notes/research-0.5.2/); packets after 0.5.1 ships): `glia watch`, incremental rebuilds on CA.9's measured phase costs; a Datalog rule layer with a GQL / Cypher read front-end; cross-repo identity (node dedupe across merged repos); Engram PPR memory. The finishing batch's deferrals go there too:
- PHP short-name symbol imports and `use \App\{X}`.
- CB.26's measured residuals.
- The rest of the matrix none / partial cells: Ruby, PHP, Rust, Scala, Dart and Elixir broker rows, Python blinker / Django signals, a GCP subscription-to-topic binding.
- WS / gRPC / GraphQL external-endpoint marking.
- An OpenAPI `basePath` for spec-status.
- In TypeScript: typed-receiver inherited calls, concrete-override dispatch, cross-file signal reads.
- The Go `package` clause.
- `[walk] tests` feeding the TESTS pass.

Then 0.5.3 (Stack Graphs-style incremental resolution, build-target graphs) and 0.5.4 (precision on real repos, agent before / after, calibrated confidence). The train is §1 of the 0.5.1 leap doc.

**Later (not in 0.5.1):** dominators (once middleware is extracted and the security gate is ruled on); a RuntimeZone resolver; LSP; per-graph-area rebuilds, if a big repo is slow after LG.1; org-internal-package routing (sibling-repo imports); one dispatcher for the non-tree-sitter file branches and one shared home for the duplicated extractor helpers (`looks_like_url_path`); query-specific distillation over composition / synth cells / vectors (the research direction in Experimental notes). The per-language gaps the 0.5.0 spec runs found are in §7.6 of its leap doc. A non-code domain builds on the 0.5.0 seam (header registries, container sections, the domain profile, pass composition); none is scheduled.

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
