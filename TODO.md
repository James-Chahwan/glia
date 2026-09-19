# glia: open items outside the 0.5.0 leap

The v0.4.x "2-day ship plan" (locked 2026-05-05) that used to live here is retired. Each of its boxes was
checked against the code on 2026-09-19. The ones that shipped were removed, and the commit that retired
the plan records the evidence for each (`git log -- TODO.md`). The open items that remain are below.

- 0.5.0 scope and packets: `dev-notes/next-leap-0.5.0.md`, `dev-notes/leap-packets.json`
- State of the finished programme: `dev-notes/handoff-2026-09-18-next-session.md`
- What glia will not build (taint, CVE joins, bulk scanning): `SECURITY.md`

## Closed by a leap packet

These stay here so each packet can find its line. The packet ticks or removes the box when it lands.

- [ ] **Synth bins gating.** The four synth bins sit behind the `driver` feature. The hook-trait refactor (`SynthHook` / `FilterPredicate` / `RankingSignal`, one `ActivatedView`) and the rename `driver` → `research` are LD.12a–e. LD.12b marks this box done.
- [ ] **`glia merge` of pre-built `.gmap`s.** `glia merge` takes repo paths and rebuilds each one. Merging pre-built layouts under a workspace manifest is LC.10b (engine) plus LC.10c (CLI and pyo3).
- [ ] **`glia impact` from a file or diff.** `impact` takes a qname. Seeding it with changed files or a pasted diff is LE.2 (diff_impact).
- [ ] **Effect classification.** Reshaped as effects(A): data, config and queue sites anchored to their functions (LE.4a–c), then the effect sinks downstream of a node (LE.4d).
- [ ] **Config: secrets and feature flags as `CONFIG_KEY` flavours.** Vault paths, AWS Secrets Manager ARNs and k8s `Secret` refs are A13.8 (Batch C).
- [ ] **DB: ORM breadth and migrations.** JPA/Hibernate, EF Core, GORM, ActiveRecord, Eloquent, TypeORM, Prisma and Django implicit tables are A13.10–A13.17. Migration files and DDL are A13.9 (Batch C).

## Outside the leap

- [ ] **Warp routes (Rust).** `warp::path!` builds its segments in a macro DSL, so the scanner skips it (see the comment above `scan_at_path_chains` in `parsers/code/rust/src/lib.rs`). Tide, Poem and Salvo are covered.
- [ ] **`glia analyze` on a pre-built `.gmap`.** `analyze` builds from a repo path. It cannot read a layout that is already on disk.
- [ ] **Cron sources outside code.** Committed crontab files, systemd `.timer` units, and cloud schedulers declared in IaC (GCP Cloud Scheduler, EventBridge Scheduler). Schedules configured in a UI, such as GitLab pipeline schedules, never appear in the repo.
- [ ] **Config files as `CONFIG_KEY` sources.** Spring `application.yml` / `.properties`, Rails `config/database.yml`, .NET `appsettings.json`, and CI variable definitions (GitHub Actions `env:`, GitLab CI variables).
- [ ] **Kustomize overlays and Helm rendering.** The IaC resolver reads raw manifests only.
- [ ] **More package manifests, and lockfiles.** Maven `pom.xml`, Gradle, .NET `*.csproj`, plus lockfiles (`package-lock.json`, `Cargo.lock`, `go.sum`). PACKAGE_DEP stays dependency substrate (`SECURITY.md`).
- [ ] **Org-internal package routing.** A shared-lib import should bind to the sibling repo's real definitions, not a phantom external node. This needs the workspace manifest (LC.10b) first.
- [ ] **DB depth.** Sequelize models, CosmosDB containers, column and view level entities, and graph-DB relationship types. Today DATA_ENTITY resolves only to table, collection or label.
- [ ] **Cross-repo node dedupe.** MergedGraph joins each repo's nodes with cross-edges, and some duplicate external-package nodes are accepted.

## Decided, not open

- `glia merge` does no git fetching and no private-repo auth. Users clone with their own tooling.
- There is no central daemon (`glia serve`). The shape is the CLI plus a post-commit hook.
