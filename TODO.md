# glia v0.4.x — 2-day ship plan

Locked 2026-05-05. Ship gate: substrate complete + CLI/MCP + G2 refactor + SWE-bench footnote. Reasoning trial optional at end.

## Day 1 — substrate

### Frameworks (parsers/code/&lt;lang&gt;/src/lib.rs)

- [ ] **Go HTTP framework extractor** — Gin / Echo / Chi / Fiber / Gorilla Mux / stdlib `http.HandleFunc`. Biggest single gap (grep: zero hits in current Go parser). Pattern: copy shape of `scan_axum_routes`.
- [ ] **Sinatra (Ruby)** — `get '/path' do ... end`, `post`, `namespace`. Sibling to existing `scan_rails_routes`.
- [ ] **Symfony + Slim (PHP)** — Symfony: `#[Route('/path')]` attribute or `@Route` annotation. Slim: `$app->get/post('/path', ...)`. Sibling to `scan_laravel_routes`.
- [ ] **Rust web frameworks** — Warp (`warp::path!(...).and(warp::get())`), Tide (`app.at('/path').get()`), Poem (`#[handler]` + `Route::new()`), Salvo (`Router::with_path`). Extend existing axum/actix/rocket scanner.
- [ ] **Java verification** — grep for Quarkus / Dropwizard / Javalin. Quarkus/Dropwizard likely covered by existing JAX-RS `@Path` patterns. Javalin uses `app.get('/path', handler)` — different shape, probably a real gap.

### Cross-graph resolvers (graph/src/lib.rs, CrossGraphResolver impls)

- [ ] **DB resolver** — three flavor tracks under one `DataStore { flavor: sql | nosql | graph }` umbrella:
    - **SQL:** `Table` / `Column` / `View` nodes. ORM model classes → table edges (SQLAlchemy / Django ORM / ActiveRecord / Eloquent / Sequelize / TypeORM / Prisma / GORM / sqlx / EF Core). Raw SQL strings parsed for table refs. Migrations create/alter tables.
    - **NoSQL:** `Collection` nodes (MongoDB / DynamoDB / Firestore / CosmosDB). Schema/model classes (Mongoose / Beanie / Pydantic+Motor) → collection edges. Repos / DAOs that wrap collections.
    - **Graph DB:** `NodeLabel` + `RelationshipType` nodes (Neo4j / ArangoDB / Dgraph). v1 recognizes Cypher MATCH/MERGE label refs; full query parsing punted.
    - **Cross-service edge:** two services touch same Table/Collection/NodeLabel name → join.
- [ ] **Cron resolver** — crontab files, k8s CronJob manifests, GH Actions `schedule:`, GitLab CI pipeline schedules, systemd `.timer`, GCP Cloud Scheduler / AWS EventBridge / Azure Logic Apps, framework schedulers (Celery beat, Sidekiq-cron, node-cron, Quartz, Hangfire, Oban, Rails `whenever`, Laravel Scheduler, apscheduler).
- [ ] **Config resolver** — three flavor tracks emitting `ConfigKey` nodes with `defined_in: <source>` and `read_at: <code-site>` edges:
    - **Env vars:** `os.environ` / `process.env` / `std::env::var` etc. in code ↔ Dockerfile `ENV`, k8s `env`/`envFrom`, `.env` files, CI variables, docker-compose `environment`.
    - **Config files:** `application.yml` (Spring), `config/database.yml` (Rails), `appsettings.json` (.NET), `pyproject.toml`, `next.config.js`, etc. ↔ where keys are read in code.
    - **Secrets / vault refs:** Vault paths, AWS Secrets Manager ARNs, k8s `Secret` envFrom refs. Same key-lookup pattern, different source tag.
    - **Cross-service edge:** same key consumed in N services.
- [ ] **IaC resolver** — Dockerfile (`FROM`/`ENV`/`EXPOSE`/`CMD`), k8s (Deployment/Service/Ingress/ConfigMap/Secret), Kustomize overlays, docker-compose.yml. Emit Resource nodes joined to services. Concept from Codebase-Memory MCP.
- [ ] **Package-deps resolver** — manifests (`package.json`, `go.mod`, `Cargo.toml`, `requirements.txt`/`Pipfile`/`pyproject.toml`, `Gemfile`, `pom.xml`/`build.gradle`, `composer.json`, `*.csproj`) + lockfiles. Join to call edges for per-symbol reachability. Cross-language reachability is the differentiator vs Endor / Snyk / Socket.dev. **Org-internal-package detection (route imports to sibling repo's real defs instead of phantom external nodes) deferred to v0.5+ — depends on workflow/manifest story.**

## Day 2 — surface, refactor, ship

- [ ] **Maturin packaging for `repo-graph-mcp`** — already built; wire wheel matrix alongside existing `repo-graph-py` GHA.
- [ ] **`glia analyze` CLI** — `glia analyze <repo|.gmap>` → Mermaid service graph + JSON + cross-language call list. Formatter on top of MergedGraph + resolvers.
- [ ] **`glia impact` CLI** — `glia impact <file|qname>` → reachable nodes across resolver edges; cross-service blast-radius.
- [ ] **`glia merge` (minimal primitive only)** — `glia merge a.gmap b.gmap c.gmap -o org.gmap` with positional args. No manifest format. No opinion on workflow. MergedGraph is structurally already multi-repo; this is the thinnest possible CLI wrapper.
    - **Decoupled from fetching:** users clone with their own tooling, run `glia build` per-repo, then `glia merge` to join. No auth/credentials work.
    - **Dedupe:** don't try. MergedGraph keeps per-repo nodes joined via cross-edges; some external-pkg duplication accepted.
    - **Manifest format / org-meta-repo pattern / per-repo discovery / shared-internal-lib routing / daemon = v0.5+ or later.** Ship the primitive, observe real usage, design workflow from signal not speculation.
- [ ] **Synth bins gating (defer real refactor to v0.5)** — leave `synth_composition` / `synth_key_symbols` / `synth_callsite_argflow` / `synth_derived_notes` in place. Run `cargo check`; if any break after Day 1 substrate changes, gate them behind `--features sweben` so default builds skip them. Real hook-trait refactor (`SynthHook` / `FilterPredicate` / `RankingSignal` traits, single `ActivatedView` pass) happens in v0.5.0 when cross-domain work actually needs the seam.
- [ ] **SWE-bench footnote + candle decision** — README paragraph (3–5 sentences) parking the latent-injection arm with marshmallow-1359 v8 SOLVE as artifact. Candle dep: strip from tree (recommend) or leave dormant.
- [ ] **README rewrite (do last)** — substrate-as-moat framing. Sections: lead with cross-service code graph claim + counts (verified actuals: 16 base langs + 3 framework parsers, ~13 IPC mechanism resolvers, ~30 frameworks); examples (`glia analyze` / `impact` / `merge` / MCP); **Compared To** (Codebase-Memory MCP / SocratiCode / Sourcegraph / CodeQL / Apiiro with concrete deltas, concede where they win); architecture diagram; experimental notes (effect classification status if shipped, SWE-bench footnote).

## Optional — only if Day 2 has runway

- [ ] **`glia install-hooks` (opt-in git hooks)** — install `.git/hooks/post-commit`, `post-merge`, `post-checkout` that re-run `glia build` to keep `.gmap` fresh. **Opt-in, not default-on** (rebuild latency on big repos would frustrate users). Document the CI pattern in README rather than building a CI integration. Incremental rebuild = v0.5+ (touches `.gmap` format).
- [ ] **Effect classification reasoning trial** — single static-reasoning pass tagging every node with effect class (pure / read-state / write-state / network / fs / db / time / nondeterministic). Compose along call edges. Visible win: `glia analyze` diagrams gain edge annotations. Ship in v0.4.x only if diagrams visibly improve; otherwise defer to v0.5+.

## Notes

- **G8 release engineering is done** — repo split happened post-v0.4.12; we're rust-only.
- **Verified framework coverage already shipped:** Flask / FastAPI / Django (Python); Spring / Ktor / WebFlux / Micronaut / JAX-RS (Java); Express / Koa / Hono / Fastify / Next.js / SvelteKit / NestJS (TS); Rails (Ruby); Laravel (PHP); ASP.NET / Minimal API (C#); Axum / Actix / Rocket (Rust); Phoenix (Elixir); React / Angular / Vue. Queues: Celery / BullMQ / Sidekiq / Oban / NATS / RabbitMQ / Kafka. Don't re-build these.
- **v0.5.0 = cross-domain pivot** (video / chemistry / policy / climate). Code becomes one of N domains. Out of scope for these 2 days.
- **Cross-language taint, contract drift, type propagation** = v0.5+ refinements. Out of scope.
- **Node dedupe across repos** = not attempted in v0.4.x. MergedGraph stitches via cross-edges; some external-pkg duplication accepted.
- **Incremental .gmap rebuild** = v0.5+. v0.4.x rebuilds whole file each time.
- **Git fetch / private-repo auth in `glia merge`** = explicitly out of scope. Users clone with their own tooling.
- **Multi-repo manifest format** = deferred. v0.4.x ships positional-args `glia merge` only.
- **Org-meta-repo workflow pattern** = documented later, not v0.4.x.
- **Per-repo declarations / `glia discover` (Pattern 2)** = v0.5+ if real demand.
- **Central daemon / `glia serve` (Pattern 3)** = killed; too much for the project's current shape.
- **Org-internal-package routing** (shared-lib imports → sibling repo's real defs) = v0.5+ once workflow story lands.
