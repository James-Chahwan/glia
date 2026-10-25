# The outlier cohort

The standing corpus a glia release is judged on (FH.1, James 2026-10-26, option 1 of the Sibyl
plan). Every fixture and gate before it was shaped by our own repos; the agent before / after
work (`dev-notes/agent-ab-probe-0.5.4.md`) showed glia blind on real full-stack repos it had
never been tuned on. The cohort is real public repos chosen for the shape they bring, each
pinned by sha, and tasks whose answers are the authors' own commits. No model runs here.

- `repos.json` - one row per repo: `label`, `url` (public GitHub https), `sha` (the pinned
  head, verified with `git ls-remote`), `lang`, `role`, `source` (who added it), `shape` (what
  it brings, stated before glia was run on it), `sides` and, for FE.4's drift tasks, `picks`.
- `cohort.py` - `build-tasks` mines `tasks/<label>.jsonl` from each repo's history;
  `build-tasks --check` re-derives every row offline; `starts` rebuilds the synthetic drift
  starts (`--compare-agent-ab` compares them with `bench/agent-ab/probe/drift.py`'s refs).
  FH.1b adds the no-model runner (`run`, `report`) and FH.12 the gate that reads its results.
- `tasks/<label>.jsonl` - the task rows, in `bench/agent-ab/tasks.py`'s schema plus `shape`.
- `test_cohort.py` - hermetic tests (two scripted git repos; no network, `gh` or glia).

## Roles and sides

`role` is `app` (judged on both sides of its boundaries), `service-set` (services calling each
other: judged like an app, the callers being the client side) or `library` (no boundary: never
judged on client reach, and it gets no drift task). An app's `sides` name the path prefixes of
its `server` and `client` halves, the words the drift prompt uses for the clients
(`client_names`), and optionally `generated`, a regex added to the default generated-path rule
(`bench/agent-ab/probe/drift.py`'s, verbatim): a generated path goes with the server, as a
regenerated SDK follows the server.

## Task kinds

Mined newest first along the first-parent history of the pinned sha (at most `--max-commits`
3000 commits), up to `--per-kind` (4) rows per kind; a repo's `picks` are taken first whatever
the filters say, so a kind holds at most 6. A commit is one task: drift, then symptom, then
ripple. Bot authors, release / bump / merge / docs subjects and subjects under four words are
skipped (`tasks.py`'s `candidate`).

- **drift** - a commit changing 2-10 non-test code files with at least one on a client prefix
  and one on a server prefix (generated paths counted as server). The task starts at a
  synthetic commit S: the real parent P plus every non-client change, so the checkout holds the
  server half and the clients are stale; the truth is the authors' client half (S..C). S exists
  nowhere upstream: the row holds its recipe (`start`: the commit, P, the client prefixes, the
  generated-path rule) and its `parent` is the ref `refs/cohort/drift/<id>`, which `build-tasks`
  and `starts` write into the bare clone (`cohort.checkout_rev` rebuilds it on demand). The
  rebuild is byte-identical in any clone (fixed identity and dates). The prompt names the
  server change and the stale clients.
- **ripple** - a definition whose name or signature moves in one file and is edited in at least
  two other code files (its callers); 2-12 code files, 5-600 changed lines. The prompt is the
  commit subject; `probe` names the symbol, its file and the callers.
- **symptom** - a commit (same size rule) that closes a GitHub issue; the prompt is the issue
  alone (title and body through `redact`, untrusted text), never the subject that states the
  fix; an issue whose title names a changed file is skipped. Issue text is fetched once with
  `gh api` and carried over on every later build (`--refetch` to fetch again).

`[cohort] tasks repo=<label> drift=<d> ripple=<r> symptom=<s> starts=<n>` is printed per repo,
and `[cohort] short repo=<label> kinds=..` when the history holds fewer than 3 of a kind.

## Clones

Bare clones live under `${GLIA_COHORT_DIR:-~/.cache/glia-cohort}/<label>.git`, never in the
tree. A missing clone is made with `git clone --bare --local` from
`${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}/<label>.git` when that exists (the FE.1 and FE.4
repos; twenty is 2.3 GB), else from the url. `GLIA_COHORT_OFFLINE=1`: no clone, fetch or `gh`.

## The repos

| label | role | lang | source | the shape it brings |
|---|---|---|---|---|
| `shell` | library | typescript | FE.1 | a GNOME Shell extension in TypeScript (window tiling): GJS imports, signal callbacks and GObject classes, no network boundary |
| `websocket` | library | go | FE.1 | a Go protocol library (gorilla/websocket): client and server halves of one protocol in one package, exported API used only by examples and tests |
| `samples` | app | dart | FE.1 | a monorepo of ~50 Flutter sample apps; compass_app pairs a Dart shelf server with a Flutter client over HTTP / JSON models |
| `commons-text` | library | java | FE.1 | a Java utility library (Apache Commons Text): deep class hierarchies, overload-heavy public API, no boundary |
| `microservices-demo` | service-set | polyglot | FE.1 | eleven services in five languages (Go, C#, Node, Python, Java) joined by gRPC from one protos/ dir, a Go web frontend and a Python load generator |
| `click` | library | python | FE.1 | a Python CLI library (pallets/click): decorators building commands, a src/ layout, no boundary |
| `immich` | app | polyglot | FE.4 drift probe | a NestJS server (enum-member controller prefixes), an OpenAPI-generated fetch SDK as a pnpm workspace package, a SvelteKit web app (.svelte) and a Flutter mobile app, a Python ML service |
| `twenty` | app | typescript | FE.4 drift probe | a large Nx TypeScript monorepo: a NestJS server exposing GraphQL and REST, a React front end through Apollo hooks and generated GraphQL types, shared workspace packages |
| `photoprism` | app | polyglot | FE.4 drift probe | a Go (gin) API with route groups and swaggo annotations, and a Vue 3 front end with hand-written REST model classes |
| `gin-vue-admin` | app | polyglot | FE.4 health probe | a Go (gin) admin backend with router-group registration per module and a Vue 3 admin front end calling it through a `service({ url, method })` wrapper |
| `scaffold-eth-2` | app | polyglot | FE.4 health probe | a Solidity (Hardhat) dapp starter with a Next.js client that reads contracts through wagmi hooks and a generated deployedContracts.ts, no HTTP |
| `documenso` | app | typescript | FH.1a | tRPC: a document-signing app whose React Router (formerly Next.js) client calls a tRPC router in a workspace package, typed end to end with no URL strings, plus a ts-rest public API |
| `medusa` | app | typescript | FH.1a | Zod + Express: a commerce engine whose Express-based API routes validate with Zod schemas (validators.ts), called by a React admin dashboard through a hand-written JS SDK workspace package |
| `mealie` | app | polyglot | FH.1a | Pydantic / FastAPI + TS client: a recipe manager with a FastAPI backend of Pydantic schemas and a Nuxt (Vue) client whose API classes wrap route strings and whose types are generated from the Pydantic models |
| `hoppscotch` | app | typescript | FH.1a | GraphQL codegen: an API client app whose NestJS backend is code-first GraphQL and whose Vue front ends issue .graphql documents compiled by graphql-codegen (urql) |
| `tolgee` | app | polyglot | FH.1a | Kotlin + Spring: a localization platform with a Kotlin Spring Boot backend (annotated controllers, JPA) and a React web app typed by an openapi-typescript generated schema |
| `prefect` | app | polyglot | FH.1a | a Python monorepo: one src/ tree holding a FastAPI server, its Python client, 19 integration packages under src/integrations/, and two Vue / React UIs (ui-v2 typed by generated OpenAPI types) |
| `zora-protocol` | app | polyglot | FH.1a | a Solidity protocol with its clients: Foundry contract packages, wagmi-generated ABI packages, and the TypeScript SDK and CLI that call the contracts through viem |
| `chatwoot` | app | polyglot | FH.1a | Rails + JS: a support desk with Rails controllers, jbuilder views and an enterprise overlay, and Vue dashboards / widget calling it through ApiClient subclasses that build URLs from a resource name |
| `squidex` | app | polyglot | FH.1a | .NET + TS: a headless CMS with an ASP.NET Core backend (attribute-routed controllers, DTO classes) and an Angular front end with hand-written services and a generated model file |
| `plane` | app | polyglot | FH.1a | Django REST + Next.js: a project tracker whose Django REST Framework API (serializers, viewsets, url patterns) is called by several Next.js apps through TS service classes in a workspace package |

The ten `FH.1a` rows were picked for shapes glia's fixtures never chained, one per shape, before
glia was run on any of them. Every repo is public; a private project never joins the cohort.
