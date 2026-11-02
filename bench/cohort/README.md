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
  `run` / `report` / `check` are the no-model runner (below); FH.12's gate reads its results
  (`## The cohort gate`).
- `tasks/<label>.jsonl` - the task rows, in `bench/agent-ab/tasks.py`'s schema plus `shape`.
- `results/<release>/` - one runner pass with one glia: `meta.json` (the glia version line,
  the corpus digest), `health.jsonl` (one row per repo), `tasks.jsonl` (one probe row per
  task), `misses.jsonl`, `summary.json` (no times: what a gate compares) and `report.md`; `results/baseline/` is the gate's
  baseline, the same files plus the gate's own.
- `test_cohort.py` - hermetic tests (two scripted git repos and a fake glia printing canned
  JSON; no network, `gh` or real glia).

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

## The runner (no model)

    python3 bench/cohort/cohort.py run --release <r> [--glia <bin>] [--jobs 4] [--only <label>,..]
                                       [--ids <regex>] [--skip-health]
    python3 bench/cohort/cohort.py report --release <r>
    python3 bench/cohort/cohort.py check --release <r> [--json]

`run` runs every probe of `bench/agent-ab/probe/` that needs no model, with their measured
definitions, through one glia binary (default the tree's `target/release/glia`; build it with
`cargo build --release -p glia-cli`), on a thread pool of `--jobs` workers interleaved over the
repos. Each (task, state) gets its own checkout under `$GLIA_COHORT_DIR/work/<release>/` (a
`git clone --shared` of the bare clone, so no history is copied, with the remote removed as the
probes' `git init` + fetch had none), deleted when its probes are done; glia runs with
`GLIA_THREADS=4`. Snapshot steps (`history sync`) run only in those checkouts.

| probe | tasks | at | what is measured (from) |
|---|---|---|---|
| health | each repo | its pinned sha | `build`, `gaps` counts, `coverage` rows, ENDPOINT / ROUTE / HTTP_CALLS reads, client reach (paired endpoints / endpoints), route reach (1 - unpaired routes / routes), dead symbols, suspicious endpoints, unparsed `.svelte` / `.astro` / `.mdx` files, imported workspace packages, plus the boundary node and pair counts per kind (`health.py`) |
| locate | every task | its start | `find --files <prompt>` (a drift task: its subject) recall@10 and the first truth rank, beside a scripted `git grep` of the prompt's words (`oracle.py` L) |
| rest | every task | its start | given the core of the change (a drift task's server half, already HEAD; else the truth file with the most changed lines, applied to the work tree), `diff-impact` recall@10 over the other truth files, beside a grep of the diff's identifiers (`oracle.py` R) |
| gate | drift | S | `contract-breaks --base HEAD~1` names a client truth file; breaking / rows / distinct changes; `review --base HEAD~1` the same (`gate.py`) |
| clear | drift, S breaking | C | fewer breaking rows once the authors' client half is in (`gate.py`) |
| names | drift | S and C | a file `diff-impact --base HEAD~1 --depth 6` reaches still holds a name the server change removed (case-normalised), and it is a client truth file; the no-graph control beside it (`names.py`) |
| evidence | every task | its start | per truth file: exists, parsed (a MODULE is located in it), its boundary nodes (ENDPOINT, GRPC_CLIENT, GRAPHQL_OPERATION, WS_CLIENT, RPC_CALL, QUEUE_PRODUCER, EVENT_EMITTER) and their pair edges, one `glia query` program |

At a drift task's S the build, the gate, the evidence and names run before `history sync`, then
locate and rest (each probe sees the state it saw in `bench/agent-ab/probe/`).

A check is `pass`, `miss`, `n/a` (it does not apply: a library has no drift task, a gate that
does not break at S has nothing to clear, a truth file the change adds cannot be named before it
exists) or `error`. A miss is classified:

- `glia_blind` - the mechanism's node or edge is absent: the truth file is unparsed, or no answer
  reaches it (the `why` says per file: unparsed, no boundary node, unpaired, no path). A repo's
  health flag (routes unreached, clients unpaired, no client endpoints, unparsed single-file
  components) is a `glia_blind` miss of scope `repo`; a library is never flagged on client reach.
- `noisy` - the truth is reached, in an answer of more than 10x as many rows as distinct changes
  (contract-breaks rows / distinct (section, field, change) triples; names flagged files / truth
  files; diff-impact rows / rest files).
- `glia_wrong` - reached, and still wrong: a 0-breaking gate on a breaking change, a gate that does
  not clear after the fix, a truth file ranked out of the top 10, a reached client file holding no
  removed name.

`run` replaces only the rows it ran (`--only` / `--ids` re-run a part), refuses a results dir
another glia or another corpus wrote, and prints, once per run,
`[cohort] run release=<r> repos=<n> tasks=<t> misses=<m> (blind=<b> wrong=<w> noisy=<z>) ms=<ms>`.
`report` prints `report.md`: boundary health per repo, the probes per repo (passed / judged), the
drift gate per task and every miss by class. `check` re-derives every check, the miss list, the
summary and the report from the stored probe rows, and exits 1 on a difference, a task with no
probe row or a repo with no health row (`[cohort] check release=<r> ok - ..`).

## The cohort gate (FH.12)

    bash scripts/cohort-gate.sh check [--glia <bin>] [--only <label>,..] [--jobs 4]
    bash scripts/cohort-gate.sh baseline | ratchet | run | compare <base> <new> | selftest
    bash scripts/cohort-gate.sh recheck [--only <label>,..]

The release check beside the rank gate (`scripts/rank-gate.sh`): where that one blocks a
ranking regression, this one blocks glia going blind on a real repo or on a client / server
combination it read before. `check` builds the tree's glia (`cargo build --release -p
glia-cli`, or takes `--glia`), runs `build-tasks --check` (the corpus re-derives offline), the
runner over the whole cohort (`run --release baseline` into the scratch dir, then `check` on
it), `glia health --json` on each repo at its pinned sha (FH.2, reduced to `boundary.jsonl`)
and the Sibyl cover (`bench/sibyl/run.py`, FH.11), then compares the lot with
`results/baseline/`. Clones are reused from `$GLIA_COHORT_DIR`; scratch is
`${GLIA_COHORT_GATE_DIR:-~/.cache/glia-cohort-gate}` (never this tree). Every run is one
`--jobs` pool per stage (cohort, health builds, Sibyl cases), glia at `GLIA_THREADS=4`.

`results/baseline/` is a runner results dir (`cohort.py check --release baseline` re-derives
it) plus `boundary.jsonl` (one row per repo: verdict, role, the parse row, per mechanism the
verdict, reason, producers / consumers / pairs and client / server reach, the unresolved
workspace packages with their importer counts), `sibyl/results.json` / `results.md` and
`gate.json` (the glia's version line, this checkout's head at the run, `glia_source` `built`
or `given` by `--glia`, the tolerance). Base -> new, a drop is:

| what | a drop |
|---|---|
| cohort health, per repo | `client_reach` / `route_reach` down more than 0.05, or lost; a new blind flag; a new probe error |
| boundary health, per repo | the verdict or the parse verdict worse (OK < DEGRADED < BLIND); an app turned library; a new error |
| boundary health, per mechanism | its verdict worse; `client_reach` / `server_reach` down more than 0.05, or lost |
| cohort tasks | any check (locate, rest, gate = the client named, clear, names) `pass` before and not now |
| Sibyl | a matrix cell or chain cell level worse (full > partial > none); a case check (pair, drift_s, clear_c, client) passing before and failing now |

A reach down by 0.05 or less is a `decline`: listed, not a drop, and it holds the ratchet (a
ratchet would let reach creep down 0.05 a wave). Gains are listed. A changed corpus (the tasks'
digest, the repo set, the Sibyl seed / transports / case ids) is drift, exit 2: re-baseline.
The last line, which `dev-notes/wave-runner/closeout.py` gates on:

    [cohort-gate] check: ok - <repos> repos, <tasks> tasks, <cells> cells, <gains> gains, 0 drops
    [cohort-gate] check: FAILED (<n> drops: <key> <old>-><new>, ..)          exit 1
    [cohort-gate] check: FAILED (<reason>)                                    exit 2
    [cohort-gate] check: skipped - <reason>                                   exit 3

Drop keys name what moved: `<label>/<mechanism>/client_reach 0.712->0.1`,
`<label>/health/verdict OK->BLIND`, `<label>/cohort/blind +clients_unpaired`,
`<task>/<check> pass->miss`, `sibyl/cell/<lang>/<mech> full->partial`,
`sibyl/case/<id>/<check> pass->fail`. `--only` judges those repos alone (the Sibyl cover
still runs whole) and its marker ends `(partial: only <labels>)`; a partial check is never
ratcheted. `GLIA_COHORT_GATE_SKIP=<reason>` prints the `skipped` line (a close-out that
cannot afford the run records why; it fails the release's last close-out).

The close-out runs `check` after the rank gate whenever the wave's commits touch `engine/`,
`graph/`, `parsers/`, `code-domain/` or the gate's own inputs (`bench/cohort/`,
`bench/sibyl/`, the script), and always at the release's last close-out; with every gate
green it runs `ratchet`, which copies the check's results over the baseline only when a
compared value moved (wall times excluded), the check was full and made at HEAD and no decline is pending. Only
`baseline` (James's or a close-out's explicit call) and `ratchet` write `results/baseline/`.
`selftest` checks the compare rules on seeded results with no glia and no network.

`recheck` (HP.2) judges the last run's results again against `results/baseline/` as it is now, with no
build and no run: for a close-out whose only moved input is the baseline (a ratchet commit, a ruled
re-record). Every clean run writes `run/run-meta.json` last (the head that measured, its `only`, the sha256
of every file under `run/results/baseline/`; outside the results, so no baseline ever copies it). `recheck`
refuses (`[cohort-gate] check: FAILED (no run to recheck: <why>)`, exit 2) unless run-meta.json and the
results' `gate.json` agree on the head, every listed file is unchanged and none was added, and the run covers
the request (a full run covers any `--only`; a partial run only a subset of its own labels). Its `check:`
line ends ` (rejudged run of <head12>)`; `last-check.json` records the measured head and `"rejudged": true`,
and `ratchet` refuses it (`the last check was a re-judge`). The close-out runs it as `[closeout] rejudge
cohort from <commit12> (judge inputs moved: ..)` when the cohort's measure inputs and env are those of its
last run. `selftest`'s `recheck` case re-judges the seeded results against an edited base (one task check
and one reach value moved), then checks the ratchet's refusal and a refused re-judge of a changed run.

The first baseline (a glia built at `5727c57d`, 21 repos, 200 tasks, `--jobs 4`) took 6136 s
with other builds loading the machine; a full `check` of the same binary against it read `ok
- 21 repos, 200 tasks, 48 cells, 0 gains, 0 drops` in 3382 s, and its ratchet (head `0f6ce15d`)
rewrote only wall times, which the ratchet now ignores. Against it, a glia whose FH.4a call-shape rule was switched
off (a private seed build, never committed) fails `check --only immich` with 7 drops, among
them `immich/http/client_reach 0.973->0.333` and `immich/health/verdict OK->BLIND`. At
that baseline the unbound workspace importers FH.2a measured before FH.5 (twenty-shared
10,247, twenty-ui 3,052, @immich/sdk 187) read 0 / 68 / 0 (`boundary.jsonl`
`workspace_unresolved`; twenty-sdk, not in FH.2a's list, reads 1,165).

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
