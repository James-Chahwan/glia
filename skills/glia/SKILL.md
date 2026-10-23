---
name: glia
description: Answer structural questions about a codebase with the glia CLI, no MCP server needed. Use it for where X is defined, what changing X (or a diff) affects, how feature Y flows across services, what calls, serves or implements X, what X writes or sends, which tests cover a change, what a change did to the graph, a PR review of a change, whether a change breaks an API client, whether a frontend's model of an API matches what the backend sends, what usually changes with a change, which files a task, ticket or PR title is about, the context for a task packed to a token budget, where the churning, load-bearing or tightly coupled code is, what is one thing across repos, and any structural question a Datalog or GQL query can ask. Reach for it before grep when the question is about structure, not text.
---

# glia from the command line

glia parses a repo into one cross-language graph and answers questions from it. The graph holds calls, imports, the HTTP / gRPC / GraphQL / queue / event / WebSocket links between services, data access, tests and docs. Each answer is one `glia <command> <repo> ...` call. Add `--json` for structured output (progress lines go to stderr). Open the `file:line` an answer gives and read it there rather than grepping for it.

## Which command

| Question | Command |
|---|---|
| What services are there, and how do they talk? | `glia arch <repo> --json` |
| Where is X? (a name, qname or fragment) | `glia find <repo> X --json` |
| Which files is this task, ticket or PR title about? | `glia find <repo> "<text>" --files --json` |
| What should I read for task T, within a token budget? | `glia pack <repo> "<T>" --budget 4000 --json` |
| What does this stacktrace / failing test id / diff point at? | `glia resolve <repo> "<signal>" --json` |
| What does changing X affect? | `glia blast-radius <repo> X --json` |
| What does my uncommitted change affect? A pasted diff? | `glia diff-impact <repo> --json`, `glia diff-impact <repo> --diff <file> --json` |
| How does feature Y flow, end to end? | `glia trace <repo> Y --json` |
| What are the entry points, and where does each go? | `glia flows <repo> --json` |
| Why are A and B connected? | `glia why <repo> A B --json` |
| Who serves `GET /path`, or consumes topic T? | `glia serves <repo> "GET /path" --json` |
| Who implements, extends or overrides X? | `glia implementors <repo> X --json` |
| What does X write, send or call outside the process? | `glia effects <repo> X --json` |
| Which tests cover X, or my change? | `glia tests-for <repo> X --json`, `glia tests-for <repo> --base main --json` |
| What did my change do to the graph? | `glia delta <repo> --json` |
| My change as one PR report: impact, tests, edges, new rule violations? | `glia review <repo> --base main --json` |
| Did my change break a client of an API or message contract? | `glia contract-breaks <repo> --base main --json` |
| Does the frontend's model of an API response or request match the backend's (or the OpenAPI op, or the GraphQL schema)? | `glia contracts <repo> --with <backend> --fields --json` |
| What usually changes along with my change? | `glia cochange <repo> --base main --json` |
| Are there loops between services, or import cycles? | `glia cycles <repo> --json` |
| Does the code break the repo's declared rules or architecture? | `glia check <repo> --json` |
| Which feature flags are dead, undefined, read in one place, or quiet? | `glia flags <repo> --json` |
| What changes often and has much depending on it? | `glia hotspots <repo> --json` |
| What are the tightly coupled clusters? | `glia communities <repo> --json` |
| What carries the most load: utilities, orchestrators, cross-service connectors? | `glia hubs <repo> --json` |
| A structural question no command answers (every route with no handler, every call from `web/` into `api/`)? | `glia query <repo> -e '<datalog>' --json`, `glia query <repo> --gql -e "MATCH ..." --json` |
| Which packages, messages, contract ops or client / server models are one thing across these repos, and which copies diverged? | `glia identity <repo> --with <other> --json` |
| Where is glia blind in this repo? | `glia coverage <repo> --json` |
| What could glia not pair? What looks dead? | `glia gaps <repo> --json` |
| Which docs govern X? | `glia docs-for <repo> X --json` |

`hotspots`, `cochange` and the `quiet` finding of `flags` read git history from a snapshot, never from git itself: run `glia history sync <repo> --blame` once first (it writes `<repo>/.glia/history-snapshot/`). Without one, `hotspots` and `cochange` come back empty with the absence reason `no_history` (exit 1), and `flags` reports `quiet_evaluated: false`.

The ranked answers read the same snapshot. With one, `blast-radius`, `diff-impact`, `review` and `pack` put the files that usually change with the seeds' files first (co-change first, then PageRank), and `review` / `find --files` rank by past commit messages too. Then `glia history calibrate <repo> --json` replays the repo's own history and records, per answer and seed shape, which ranking predicted its past commits best (`<repo>/.glia/calibration-snapshot/`); each ranked answer then uses that pick and says so in `selection` (`source: calibrated`, or `fallback` with the reason, e.g. `absent` before any calibration, `build_stamp` after a glia upgrade: calibrate again). So, once per repo and again after an upgrade: `glia history sync <repo>`, then `glia history calibrate <repo>`. Neither runs inside a build, and without them every answer keeps its structural order. `--ranking ppr|ppr-wide|bfs` forces one ranking instead of the calibrated pick, `--no-cochange` drops the co-change tier, and `--top-files <N>` (blast-radius, diff-impact) sizes the ranked `files` list.

The rest of the CLI:
- `glia pages <repo>`: frontend router pages and dead links.
- `glia contracts <repo>`: producer vs consumer message types per queue topic; `--fields` diffs fields (schema copies, OpenAPI vs Pact, and the client / server models in the table above).
- `glia spec-status <repo>`: OpenAPI / feature.yaml operations implemented or missing.
- `glia projects <repo>`: the labels `--scope` takes.
- `glia impact <repo> X`: plain reachability, unranked.
- `glia patterns <repo>`: each service's route-handler convention and the handlers off it.
- `glia splits <repo>`: where a scope splits into services at the least coupling (a suggestion, tier heuristic).
- `glia duplicate-flows <repo>`: entry points whose flows reach the same nodes (an aliased route, a v1 / v2 pair, copies drifting apart).
- `glia timeline build <repo>`, then `glia timeline history <repo> <qname>`: when each edge of a node appeared and went, over the last 20 commits.
- `glia scip import <repo> <index.scip>`: load a compiler-grade SCIP index; the next answer reads its references as FACT-tier CALLS / USES / IMPLEMENTS / INHERITS_FROM.
- `glia watch <repo>`: keep the build open in the foreground and print one NDJSON line per change saying what it did to the graph (a ready line first); it rewrites `<repo>/.glia/graph/` on exit and every 60 s. For a long session, not for one question.

`glia <command> --help` lists every flag.

Flags most commands share:
- `--with <repo>` (repeatable) merges more repos in, so links pair across them. With one service per repo, pass the others this way. Each row's file stays relative to its own repo's root.
- `--scope <path-or-label>` narrows the answer within a repo.
- The ranked walks also take `--top-k`, `--depth` and `--direction forward|backward|both`.
- `--no-collapse` (on `find`, `blast-radius`, `trace`, `flows`, `tests-for`, `serves` and `arch`) shows one row per copy instead of one per identity class.

## Reading an answer

- **Location.** Node rows carry `file` and `line`: 1-based, relative to the repo. Some nodes have no single definition: `endpoint:GET:/users` (an outbound call), `data_entity:sql:prefs` (a table) and queue topics can have `file: null`. For those, the edge's `site_file` / `site_line` (in `path`, `witness` and `site`) locates the code.
- **Liveness.** `live: false` (`⊘` in tables) means no entry point reaches the node. It is likely dead, or it is reached from an entry point glia does not know.
- **Tier.** `tier` is `fact` (declared in the source), `derived` (inferred by a resolver, e.g. a client URL paired to a route) or `heuristic` (paired by name). Case varies by command. Weigh the answer by it.
- **Empty answers.** An empty answer is not "nothing exists". It carries `absence: {tier, reason, note, caveats, suggestions}`. Each caveat names an edge kind glia is known to miss (`edge_category`, `language`), how many it did find (`edges_found`) and what to grep instead (`verify`).
- **Blind spots.** `glia coverage <repo> --json` lists the same caveats for the whole repo. Run it once before trusting "not found".
- **Identity.** With `--with`, one package, message, contract op or SCIP symbol that several repos hold is one row, its best copy, with `identity: {tier, rules, diverged, also_in}`. `also_in` lists the other copies with their `repo`, `qname`, `file` and `line`, and `diverged: true` means the copies differ (a message's fields, say). Vendored copies are `heuristic` candidates in `glia identity` and are never folded. A frontend model and the server struct of the HTTP exchange it reads (rule `http_model`), and a GraphQL client's result model with its schema type (`graphql_model`), are `derived` classes too, inside one repo as well: in a monorepo `find` shows a TS interface and its Go struct as one row, the other one under `also_in`.
- **External sinks.** A call whose every site dials a public host outside the build (`fetch("https://nominatim.openstreetmap.org/search?…")`) is an ENDPOINT marked external, never paired with a route. `effects` rows name the host in `external_hosts`; the table's downstream cell reads `external: <host>`. `serves "GET /search" --json` lists it after any route, `"kind":"ENDPOINT","match":"external"`, so the channel is answered, not absent. `external_hosts` is `[]` on every other row. Such a call is real traffic, not a gap.
- **Docs.** `docs-for` and the `DOCUMENTS` edges read only the markdown the build ingests. That is the well-known files (`README*.md`, `CLAUDE.md`, ...) and the `docs/` tree at the repo root and at every project root, a top-level `.ai/` tree, ADR directories, SDD feature docs (`features/<feature>/*.md`, spec-kit `specs/<NNN-slug>/`) and synced docs (`glia docs sync`). A section documents a symbol only when it names it in single backticks. A README inside a source directory is not read. `glia coverage` states this as a `DOCUMENTS` row.
- **Dead symbols.** A `gaps` `dead_symbol` row is a function, method or class with no entry point reaching it and no incoming call or use. Never a row: a method that IMPLEMENTS a called method (an interface method, an abstract member: dispatch reaches it); the members a framework or the runtime calls on a class something uses (Angular lifecycle hooks, form-control and validator members, pipe `transform`, interceptors, guards, Flutter `build` / `createState` / `dispose`, ...) and its constructor; generated code (ORIGIN `generated` / `generated_proto`). stderr counts what it held back: `[dead-framework] dead_symbol rows withheld: invoked=<i> generated=<g>`. A method an Angular template or `host:` binding calls (inline or through `templateUrl`), a member a decorator binds (`@HostListener`, an `@Input` setter), what a Flutter widget's closures call and a TypeScript function passed by value (a route guard, a provider, a comparator) are reached through real edges, so they are not dead either. So are a Dart getter read, tear-off or member named in any value position, a call on a Riverpod provider read (`ref.read(p).m()`), a Go method called on a range variable's element or a package-qualified field, a Go func or method passed by value and a struct a composite literal builds, a `pkg.F` route handler, and a TypeScript call through an Angular signal query (`viewChild(T)`), an inline `inject(T)`, a `const x = f()` local or a getter on another typed object. Entry points in their own right: a Go `init`, an iOS `@main` type, a Flutter Android activity, and a Python script with an `if __name__ == "__main__":` guard (the guard's calls are the module's). Live when their class is used: a Go method the runtime calls through an interface, checked by signature (`Error`, `String`, `MarshalJSON`, `Scan`, `Value`, `ServeHTTP`, `Len` / `Less` / `Swap` together); an `@main` app delegate's callbacks; the members a library base calls on a class extending it (a Transloco loader's `getTranslation`, an Angular `ErrorHandler`'s `handleError` or `Resolve`'s `resolve`, a dio interceptor's `onRequest`). stderr counts the invoked members by framework: `[invoked] rows=<n> angular=.. constructor=.. flutter=.. go_runtime=.. apple_app=.. transloco=.. dio=.. sig_rejected=<r>` (`sig_rejected`: methods named like a runtime method whose own signature or owner's sibling does not match, `Scan() bool`; they are not withheld). On two audited full-stack repos, every remaining row was truly dead.
- **Unpaired routes and endpoints.** `unpaired_route` leaves out a route a WebSocket client connects through (`[gaps] ws-served routes=<n>`) and a route only test code registers (`[gaps] unpaired_route rows withheld: <n> (test-file routes)`). A GraphQL client's HTTP transport (`endpoint:POST:/graphql`) is no `unpaired_endpoint` when its project's operations pair a resolver on a server with no route at that path; any other unpaired transport's `detail` starts `graphql transport; ` (`[gaps] graphql transports implied=<n> unpaired=<m>`).
- **Ranking.** `blast-radius` and `diff-impact` list `files` after the rows: the most relevant files ranked (`rank`, `file`, `repo`, `cochange`, `ppr`, `depth` under bfs, `rows`: how many result rows sit in it, 0 for a file the walk never reached, which co-change alone put there). `selection` (`set`, `shape`, `pick`, `source`, `reason`, `test_commits`) names the ranking an answer used; the table form prints it as one `ranking:` line. `review` with a title (`--title`, or by default the newest commit subject of `<base>..HEAD`) adds `intent`: the ids the title names (a ticket `LANG-12`, a spec-kit `FR-001` / `T012`, or the shapes `.glia/overlay.toml [ids]` declares) routed to the task or requirement rows defining them and the files those rows name, past commits ranked by the title's words, and `intent.files` ranked by the repo's pick (`selection_intent`); impact rows in files an id routes to are listed first. `find --files` answers the same question for a query: files with `idr` (an id in the text routes there) or `text` (past commit messages match) above 0, ranked; with neither it is an absence whose note says which input was missing (no id resolved, no history messages, no past message matches), exit 0.
- **Exit codes.** 0 for any answer, empty included, except: `pack`, `hotspots`, `hubs`, `cochange`, `splits`, `query` and `identity` exit 1 when the answer is empty. `why` exits 1 when no edge joins the two nodes (it still prints a witness path). `check` exits 1 on a BLOCKING violation: one whose tier is at least its rule's `min_tier` (default `derived`). A `heuristic` violation never blocks; it is listed as advisory. A declared rule that checks nothing (a stanza anchored nowhere or rejected, one the graph does not hold, a scope no node sits in) is listed under `unanchored` ("Rules that check nothing") with its reason and a fix, and changes the exit code only with `--strict` (then 2). `review` exits 1 when the change adds a blocking violation or a blocking model break, and `contract-breaks` when a change is breaking, leaves a client with no provider, or is a blocking model break. A model break (a client / server model drifted across the change) blocks only at or above `--models-min-tier` (default `fact`, so the resolver-paired `derived` rows are advisory: counted in `advisory`, not `breaking`). `blast-radius` exits 3 when no seed resolved. 2 means a usage, git or build error, a query that does not parse or analyse, or a rule `check` could not evaluate.

## Cost, and what gets written

- There is no daemon and no index to keep fresh: each call builds the graph from source in memory, then answers. Rather than looping, pass several seeds to one call (`blast-radius`, `effects` and `tests-for` take many).
- The query commands write nothing to the repo.
- Five commands save a parse cache at `<repo>/.glia/graph/parse_cache.bin` (self-gitignored): `delta`, `diff-impact` against a rev (its default), `tests-for --base`, `review` and `contract-breaks`. The `--diff` forms write nothing, and `cochange --base` persists nothing.
- `flows --features` without `--json` writes `<repo>/.glia/graph/flows/`.
- `glia history sync` writes `<repo>/.glia/history-snapshot/` (with each commit's redacted message; `--no-messages` keeps none), and `glia history calibrate` writes `<repo>/.glia/calibration-snapshot/`. `glia timeline build` writes `<repo>/.glia/graph/timeline.gmap`, and `timeline history` rebuilds and rewrites a stale `.glia/graph/` layout.
- `glia build <repo>` writes the `.glia/graph/` layout that the MCP server and Python bindings load. The CLI does not need it.
- `glia watch <repo>` writes the same layout and the parse cache on exit and every `--persist-every` seconds (60 by default). It does not read `GLIA_NO_PERSIST`: writing the layout is what it is for, so never run it on a repo you must not write to.
- **`GLIA_NO_PERSIST=1`** is for probing a repo you must not write to: set it on every call. It stops glia rewriting a stale layout (`glia merge --gmap`, `timeline history`) and `timeline build` writing its sidecar. It does not stop the parse cache, the flows files or the history snapshot above. On such a repo, use the `--diff` forms and `flows --features --json` instead: `git -C <repo> diff | GLIA_NO_PERSIST=1 glia diff-impact <repo> --diff - --json`.

## Worked examples

Every example below was run from the root of the repo its comment names. Most are glia's test fixtures (`bench/substrate-gap/fixtures/<name>`). The rest are small git repos built to show a change or a rule. Outputs are trimmed: `…` marks dropped fields and rows.

- `xstack-go-http`: `client/client.go` `FetchUsers` does `http.Get(".../users")`, and `server/main.go` serves `GET /users` with chi.
- `loop`: `orders/` emits `order.placed`, `billing/` handles it and emits `payment.settled`, and `orders/` handles that. The change adds `audit()` to `billing/billing.ts` and calls it from `settle()`.
- `orders-proto`: `proto/orders.proto` declares `message OrderCreated { string order_id = 1; int64 total_cents = 2; repeated string sku = 3; }` on `main`. The change makes `total_cents` a `string` and adds `string currency = 4`.
- `shop`: a Python git repo whose `orders/service.py` and `orders/models.py` changed together in all five of their commits, and `util/money.py` together with `billing/charge.py` in three of its four. `glia history sync . --blame` has been run.
- `identity-proto-diverged`: `orders/`, `billing/` and `shipping/` each hold `proto/orders.proto` with `message OrderCreated`; shipping's copy has an extra field.
- `shop-ranked`: a Python git repo of ten commits: `orders/models.py` and `orders/service.py` changed together in six (one subject is `SHOP-12 orders: round totals`), `util/money.py` and `billing/charge.py` in four, and `docs/tasks.md` holds `- [ ] SHOP-12 round order totals in \`orders/models.py\` and \`orders/service.py\``. `glia history sync . --blame` has been run.
- `models-http-contract`: an Angular `portal/` reads `GET /api/plans/:id` as its `Plan` interface (id, name, priceCents, trialDays); the gin handler in `billing/` writes `planBody` (id, name, priceCents), and the `swagger.json` op documenting the route declares the same three.

```bash
# arch-monorepo-flows: api/ gateway/ web/ worker/ in one repo; one service per top-level dir
glia arch . --json
{"keying":"top_level_dir","services":[{"id":"api","languages":["go","typescript"],"files":4,"nodes":20,"routes":1,"inbound":3,"outbound":1,…},…],
 "links":[{"from":"gateway","to":"api","mechanism":"GRPC_CALLS","channel":"UserService","count":1,"confidence":"medium","example_from_qname":"grpc_client:UserService","example_to_qname":"grpc:user.UserService"},…]}

# xstack-go-http; match tiers run exact_qname > exact_name > … > subsequence
glia find . users --kind FUNCTION --json
{"results":[{"qname":"server::main::listUsers","kind":"FUNCTION","live":true,"file":"server/main.go","line":9,"match":"name_word",…},
            {"qname":"client::client::FetchUsers","live":false,"file":"client/client.go","line":8,…}],"absence":null}

# xstack-go-http; panic.txt is a Go panic whose frame is main.listUsers() at /src/server/main.go:9
# resolve takes signals (a stacktrace, test id or diff; --kind forces one), find takes names
glia resolve . "$(cat panic.txt)" --json
{"results":[{"qname":"server::main::listUsers","kind":"FUNCTION","score":0.571,"live":true,"file":"server/main.go","line":9,…}],"absence":null}

# xstack-go-http; "text" is the pack to paste (without --json, stdout is that text alone); fidelity is full, preview, outline or qname
glia pack . users --budget 4000 --json
{"query":"users","text":"# context for users\n\n### server::main::listUsers (FUNCTION server/main.go:9-9)\n…","budget_tokens":4000,"used_tokens":377,…,"candidates":7,
 "nodes":[{"qname":"GET /users","kind":"ROUTE","file":"server/main.go","line":13,"fidelity":"outline","tokens":11,"rank":1,"tier":"heuristic","reason":"seed",…},
          {"qname":"server::main::listUsers","kind":"FUNCTION","file":"server/main.go","line":9,"fidelity":"full","tokens":35,"rank":3,…,"reason":"seed",…},
          {"qname":"server::main","kind":"MODULE","file":"server/main.go","line":1,"fidelity":"full","tokens":77,"rank":5,"tier":"derived","reason":"neighbour",…},…],"dropped":0,"rerenders":0,"absence":null}

# xstack-go-http
glia blast-radius . client::client::FetchUsers --direction forward --json
{"seeds":[{"query":"client::client::FetchUsers","kind":"FUNCTION","file":"client/client.go","line":8,…}],"unresolved":[],
 "results":[{"qname":"endpoint:GET:/users","kind":"ENDPOINT","reason":"CALLS","depth":1,"score":0.267,"live":false,"file":"client/client.go","line":9,"seed":"client::client::FetchUsers"},
            {"qname":"GET /users","kind":"ROUTE","reason":"HTTP_CALLS","depth":2,"file":"server/main.go","line":13,…},
            {"qname":"server::main::listUsers","reason":"HANDLED_BY","depth":3,"file":"server/main.go","line":9,…}],"absence":null}

# shop-ranked, after `glia history sync .`: the calibration step; one row per (set, shape), the record under .glia/calibration-snapshot/
glia history calibrate . --json
{"dir":"./.glia/calibration-snapshot","history_head":"318a924af42f…","commits":10,"test_commits":9,"messages":true,"ms":36,
 "sets":[{"set":"SELf2t","shape":"find","pick":"H18x","last_online_pick":"H18x","fallback":"LX[IDR,T1s]","test_commits":9,"queries":9,"selection_recall10":1.0,"fallback_recall10":1.0,"delta_recall10":0.0,"ci_low":0.0,"ci_high":0.0},
         {"set":"SELh0","shape":"k1","pick":"H10","last_online_pick":"H10","fallback":"H10","test_commits":9,"queries":20,…},…]}
# SELh0 ranks blast-radius / diff-impact / review's impact / pack (SELs0 under --no-cochange), SELt2 review's intent, SELf2t find --files;
# a pick whose online recall@10 falls below its fallback's is refused and recorded as the fallback

# shop-ranked, calibrated: "files" ranks co-changed files first; "selection" names the ranking the answer used
glia blast-radius . orders::models::Order --json
{"seeds":[{"qname":"orders::models::Order","kind":"CLASS","file":"orders/models.py","line":1,…}],"unresolved":[],
 "results":[{"qname":"orders::service::place","kind":"FUNCTION","reason":"CALLS","depth":1,"score":0.201,"file":"orders/service.py","line":4,…}],
 "files":[{"rank":1,"file":"orders/service.py","repo":"shop","cochange":0.857,"ppr":0.116,"depth":null,"rows":1},
          {"rank":2,"file":"billing/charge.py","repo":"shop","cochange":0.143,"ppr":0.0,"depth":null,"rows":0},…],
 "selection":{"set":"SELh0","shape":"k1","pick":"H10","source":"calibrated","reason":null,"test_commits":9},"absence":null}

# shop-ranked: the files a ticket is about; SHOP-12 routes through docs/tasks.md's row (idr), the words through past subjects (text)
glia find . "SHOP-12 round totals" --files --json
{"results":[{"rank":1,"file":"orders/models.py","idr":1.0,"text":8.628,"degree":9.0,"ids":["SHOP-12"]},
            {"rank":2,"file":"orders/service.py","idr":1.0,"text":8.628,"degree":5.0,"ids":["SHOP-12"]},
            {"rank":3,"file":"util/money.py","idr":0.0,"text":1.874,"degree":5.0,"ids":[]}],
 "absence":null,"selection":{"set":"SELf2t","shape":"find","pick":"H18x","source":"calibrated","reason":null,"test_commits":9}}

# shop-ranked with orders/models.py edited: the PR's intent; without --title it is the newest commit subject of <base>..HEAD
glia review . --title "SHOP-12 round order totals" --json
{…,"intent":{"title":"SHOP-12 round order totals","title_source":"arg","fusion":"H18s[PT*]",
   "ids":[{"id":"SHOP-12","ns":"docs/tasks.md","tier":"derived","route":"unique","defined_at":["docs/tasks.md",3],"state":"pending","named_files":2,"cited_files":0}],
   "files":[{"rank":1,"file":"orders/service.py","idr":1.0,"text":11.908,"ppr":0.116,"ids":["SHOP-12"],"in_impact":false},…],"text":"present"},
 "selection_intent":{"set":"SELt2","shape":"k1","pick":"H18s[PT*]","source":"calibrated","reason":null,"test_commits":9}}

# loop, with the audit() change uncommitted
glia diff-impact . --json
{"base":"HEAD","changed":[{"qname":"billing::billing::audit","file":"billing/billing.ts","line":5,"change":"added","seed":true},{"qname":"billing::billing::settle","change":"modified","seed":true,…},…],
 "edges_added":[{"from_qname":"billing::billing::settle","to_qname":"billing::billing::audit","category":"CALLS","site_file":"billing/billing.ts","site_line":4,…},…],"edges_removed":[],
 "impact":{"seeds":[…],"results":[{"qname":"event_emit:payment.settled","reason":"USES","depth":1,"seed":"billing::billing::settle",…},
                                  {"qname":"event_handle:payment.settled","reason":"EVENT_FLOWS","depth":2,"file":"orders/orders.ts","line":4,…},…]},"unresolved_diff_files":[]}
git diff | glia diff-impact . --diff - --json
{"base":null,"changed":[{"qname":"billing::billing::settle","change":"diff_hit","seed":true,…},…],…}

# xstack-go-http with client/ and server/ as two repos; the feature word is a name, a qname or a flows key
glia trace client FetchUsers --with server --json
{"seed":{"qname":"client::FetchUsers","file":"client.go","line":8,…},"resolved_by":"name",
 "paths":[{"rank":1,"hops":[{"depth":1,"mechanism":"CALLS","to_qname":"endpoint:GET:/users","to_file":"client.go","to_line":9,…},
                            {"depth":2,"mechanism":"HTTP_CALLS","cross_service":true,"cross_repo":true,"to_qname":"GET /users","to_file":"main.go","to_line":13,…},
                            {"depth":3,"mechanism":"HANDLED_BY","to_qname":"main::listUsers","to_file":"main.go","to_line":9,…}],
           "cross_service_hops":1,"mechanisms":["CALLS","HTTP_CALLS","HANDLED_BY"],"length":3}],"truncated":false,"absence":null}
glia trace . client::client::FetchUsers --to server::main::listUsers --json

# xstack-go-http; "key" is the word trace takes; --features --json groups flows per feature
glia flows . --json
[{"key":"get_/users","entry":{"qname":"GET /users","kind":"ROUTE","file":"server/main.go","line":13,…},"reach":1,"cross_service":false,"mechanisms":["HANDLED_BY"],"hops":[…],…}]

# xstack-go-http; with no direct edge: "found":false, exit 1, and "path" is a shortest witness
glia why . "GET /users" server::main::listUsers --json
{"found":true,"edges":[{"category":"HANDLED_BY","confidence":"strong","tier":"fact","emitter":"graph:refs","rule":"module_symbol","site":{"file":"server/main.go","line":13},…}],"path":[],"absence":null,…}

# xstack-go-http
glia serves . "GET /users" --json
{"results":[{"qname":"GET /users","kind":"ROUTE","file":"server/main.go","line":13,"match":"exact","confidence":"strong",
             "handlers":[{"qname":"server::main::listUsers","file":"server/main.go","line":9,…}],…}],"absence":null}
# xcut-queue-queue_flows: a topic's QUEUE_CONSUMER with its handlers; an unserved channel is an absence with caveats
glia serves . orders --json

# java-iface-extends: interface Catalog extends Readable, class PgCatalog implements Catalog; --up walks to supertypes
glia implementors . Readable --json
{"results":[{"qname":"Catalog","kind":"INTERFACE","file":"Catalog.java","line":3,"relation":"INHERITS_FROM","depth":1,"via":null,"tier":"FACT",…},
            {"qname":"PgCatalog","kind":"CLASS","file":"PgCatalog.java","line":3,"relation":"IMPLEMENTS","depth":2,"via":"Catalog","tier":"FACT",…}],"absence":null}

# go-sql-call-args: store.Upsert runs INSERT INTO prefs … ON CONFLICT; --cross-service follows a send into its receiver
glia effects . store::Upsert --json
{"seeds":["store::Upsert"],"effects":[{"class":"db","qname":"data_entity:sql:prefs","file":null,"line":null,"mode":"write","depth":1,"tier":"derived",
  "path":[{"from_qname":"store::Upsert","to_qname":"data_entity:sql:prefs","category":"ACCESSES_DATA","site_file":"store.go","site_line":18}],…}],
 "counts":{"db":1,"queue_produce":0,"http_call":0,…},"writes":1,"unresolved":[],"absence":null}

# py-tests: test_calc.py test_add calls calc.add; --files-only prints just the files, for a runner
glia tests-for . calc::add --json
{"seeds":["calc::add"],"tests":[{"qname":"test_calc::test_add","file":"test_calc.py","line":5,"tier":"fact","reason":"tests_edge","covers":["calc::add"],…},
                                {"qname":"test_calc","kind":"MODULE","tier":"heuristic",…}],"test_files":["test_calc.py"],"untested":[],"unresolved":[],"absence":null}
# a git copy of py-tests with calc.py edited: the test files for the working tree's change
glia tests-for . --base main --files-only
test_calc.py

# loop, with the audit() change uncommitted; --base <rev> compares against another rev
glia delta . --json
{"base":"HEAD","counts":{"nodes_added":1,"nodes_modified":2,"edges_added":2,"edges_removed":0,…},
 "nodes":[{"change":"added","qname":"billing::billing::audit","kind":"FUNCTION","file":"billing/billing.ts","line":5,…},…],
 "edges":[{"change":"added","from_qname":"billing::billing::settle","to_qname":"billing::billing::audit","category":"CALLS","site_file":"billing/billing.ts","site_line":4,…},…]}

# loop, with the audit() change uncommitted; without --json it prints markdown for a PR comment; "blocking" is true (exit 1) only when the change adds a blocking rule violation
glia review . --base main --json
{"base":"main","counts":{"nodes_changed":3,"seeds":2,"edges_added":2,"edges_removed":0,"impact":8,"tests":0,"untested_seeds":2,"new_violations":0,"resolved_violations":0,"model_breaks":0,"edges_by_tier":{"fact":2}},
 "changed":[{"qname":"billing::billing::audit","kind":"FUNCTION","file":"billing/billing.ts","line":5,"change":"added","seed":true},…],"impact":{"seeds":[…],…,"results":[…],…},
 "tests":{"seeds":[…],"tests":[],…,"untested":["billing::billing::audit","billing::billing::settle"],…,"absence":{"tier":"FACT","reason":"no_edges",…}},
 "edges":[{"change":"added","from_qname":"billing::billing::settle","to_qname":"billing::billing::audit","category":"CALLS","tier":"fact","site_file":"billing/billing.ts","site_line":4,…},…],
 "new_violations":[],"resolved_violations":[],"model_breaks":[],"check_errors":[],"blocking":false}

# orders-proto, with the change uncommitted; "producer" is the base side, "consumer" the working tree; exit 1: a change is breaking
glia contract-breaks . --base main --json
{"base":"main","schemas":[{"kind":"message","key":"shop.v1.OrderCreated","format":"proto","before":{…},"after":{"qname":"message:proto:shop.v1.OrderCreated","file":"proto/orders.proto","line":5,…},"status":"breaking","change":"modified","tier":"fact",…,
  "changes":[{"section":"fields","field":"total_cents","change":"type","producer":"int64","consumer":"string","rule":"proto_wire_type","breaking":true},
             {"section":"fields","field":"currency","change":"consumer_only","producer":null,"consumer":"string","rule":"proto_unknown_field","breaking":false}],…}],
 "models":[],"orphaned_clients":[],"breaking":1,"advisory":0,"absence":null}
# "models" judges the inferred client / server models the same way (a JSON rename, a field removed or now optional, a new required
# request field), one row per model pair with both field lines; advisory unless --models-min-tier derived, as the 0.5.2 pairs are derived

# models-http-contract; a client field the server never writes is json_field_missing (breaking); "producer" is the server side
glia contracts portal --with billing --fields --json
{"topics":[],"fields":[{"pairing":"client_contract","key":"GET /api/plans/{id} response","producer":{"qname":"contract::docs::swagger::GET:/api/plans/{id}","format":"openapi","file":"docs/swagger.json","line":6,…},
  "consumer":{"qname":"src::app::plan.service::Plan","format":"ts","file":"src/app/plan.service.ts","line":4,…},"status":"breaking","tier":"derived","note":null,
  "changes":[{"section":"response:200","field":"trialDays","change":"consumer_only","producer":null,"consumer":"number","rule":"contract_field_undeclared","breaking":true}]},
 {"pairing":"http_model","key":"GET /api/plans/:id response","producer":{"qname":"handlers::plans::planBody","format":"go","file":"handlers/plans.go","line":9,…},
  "consumer":{"qname":"src::app::plan.service::Plan","format":"ts","file":"src/app/plan.service.ts","line":4,…},"status":"breaking","tier":"derived","note":null,
  "changes":[{"section":"response","field":"trialDays","change":"consumer_only","producer":null,"consumer":"number","rule":"json_field_missing","breaking":true}]}]}
# a GraphQL operation's selection against its schema type is pairing graphql_selection (a selected field the type lacks: graphql_field_undefined)

# shop, with orders/service.py edited; confidence is per mille of the antecedent's own commits; "link":"none" (no static edge) marks a blind spot
glia cochange . --base main --json
{"query_files":["orders/service.py"],"unmapped":[],
 "rows":[{"file":"orders/models.py","module_qname":"orders::models","antecedent":["orders/service.py"],"support":5,"antecedent_commits":5,"confidence_permille":1000,"link":"direct","source":"pairwise","tier":"heuristic",…}],"absence":null}

# loop; kinds are event_loop, call_loop, possible_loop and import_cycle (--kind event|import|all)
glia cycles . --json
[{"kind":"event_loop","tier":"derived","services":["billing","orders"],"channels":["order.placed","payment.settled"],"size":9,"members":[…],
  "witness":[{"from_qname":"event_emit:order.placed","to_qname":"event_handle:order.placed","category":"EVENT_FLOWS","channel":"order.placed","file":"orders/orders.ts","line":3},…]}]

# a repo with web/ and services/api/ (a pyproject.toml each) whose web/app.py imports and calls services/api/internal.py;
# .glia/overlay.toml has a [[constraint]] kind = "forbid_edge" from "web" to "services/api", categories IMPORTS + CALLS; exit 1
glia check . --json
{"rules":1,"checked":1,"unchecked":[],"errors":[],"violations":[{"rule_id":"web-no-api-internals","rule_kind":"forbid_edge","decl":".glia/overlay.toml:3","severity":"VIOLATION","tier":"fact","blocking":true,"count":2,
  "evidence":[{"from_qname":"web::app","to_qname":"services::api::internal","category":"IMPORTS","file":"web/app.py","line":1,"emitter":"graph:imports",…},…]}],"reflexion":null}
# The same file can declare a reflexion model: [[component]] stanzas (a name and its paths), [[layer]] stanzas ranking them top
# first, and [[constraint]] kind = "allow" from one component to another. check then judges every dependency between two
# components: a convergence, a divergence (a violation, exit 1), or an absence (an allow no edge realises, with caveats).
# The table output prints this as a `## reflexion model` section; --json puts it under "reflexion" (matrix, absences, unmapped).
# With version = 2 a [[constraint]] can also take transitive = true (a path through any third scope counts; via = [..] names
# allowed facades), kind = "invariant" with a Datalog rule = '...' whose violation/N rows are its violations, and
# min_tier = "fact" | "derived" (default derived): a violation weaker than min_tier, and every heuristic one, is listed as
# advisory and leaves the exit code at 0. The stderr line `[check] gate blocking=<b> advisory=<a>` counts them.
# A rule that checks nothing (a misspelt scope, a stanza the loader dropped) is an "unanchored" row with "reason" and "fix",
# printed under "## Rules that check nothing"; --strict makes it a rule error (exit 2). A rule scoped to a directory inside a
# project anchors on that project.

# xcut-flag-read-fn: svc/checkout.py reads LaunchDarkly flags, new-checkout inside checkout() and promo-banner at module scope
glia flags . --json
{"flags":[{"key":"new-checkout","providers":["launchdarkly"],"definitions":[],"reads":[{"qname":"svc::checkout::checkout","kind":"FUNCTION","file":"svc/checkout.py","line":8}],"readers":1,"last_read_change":null,
           "findings":[{"status":"single_site","tier":"fact","note":"read only in svc::checkout::checkout (svc/checkout.py:8)"}]},…],
 "definitions_in_graph":0,"quiet_evaluated":false,"history_now":null,"quiet_days":90,"counts":{"dead":0,"quiet":0,"single_site":2,"undefined":0},"absence":null}

# shop; churn_rank and centrality_rank rank the same "ranked" rows (there is no blended score); symbol rows need blame span changes
glia hotspots . --json
{"modules":[{"level":"module","qname":"orders::models","kind":"MODULE","file":"orders/models.py","line":1,"churn":5,"lines_changed":8,"last_change":1788566400,"churn_rank":2,"centrality_rank":1,"ranked":4,"tier":"heuristic"},
            {"level":"module","qname":"orders::service","kind":"MODULE","file":"orders/service.py","line":1,"churn":5,…,"churn_rank":1,"centrality_rank":3,"ranked":4,…},…],
 "symbols":[],"history_head":1789430400,"absence":null}

# arch-monorepo-flows; a label is the top members' shared qname prefix; cohesion is internal weight over internal + boundary
glia communities . --json
{"method":"leiden","seed":42,"resolution":1.0,"modularity":0.7299382716049383,"total":5,"nodes":33,"isolated":4,
 "communities":[{"id":0,"size":8,"label":"api::orders","tier":"heuristic","cohesion":1.0,"files":2,"kinds":[["FUNCTION",3],["MODULE",2],…],
                 "top_members":[{"qname":"worker::notify::registerHandlers","kind":"FUNCTION","file":"worker/notify.ts","line":4,"weight":9},…],
                 "entries":[{"qname":"event_handle:orderPlaced","kind":"EVENT_HANDLER","file":"worker/notify.ts","line":5,…}],"services":[["api",4],["worker",4]],"sinks":[["event_emit",1]],"links":[]},…],"absence":null}

# channel-monorepo-owner: one repo, eleven services; services/orders and services/returns both publish orders.created,
# which services/audit and services/billing consume; a node with callers or callees in two services is cross_service
glia hubs . --json
{"fan_in":[],"fan_out":[],
 "cross_service":[{"qname":"queue_consumer:orders.created @services/audit","kind":"QUEUE_CONSUMER","file":"services/audit/consume.py","line":5,"label":"connector","fan_in":2,"fan_out":1,
                   "by_category":[["QUEUE_FLOWS",2,0],["HANDLED_BY",0,1]],"caller_services":["services/orders","services/returns"],"callee_services":["services/audit"],…,"live":true,"tier":"derived"},…],
 "nodes":63,"edges":23,"p99_in":2,"p99_out":2,"absence":null}

# xstack-go-http
glia coverage . --json
[{"language":"*","edge_category":"CALLS",…,"edges_found":1},
 {"language":"*","edge_category":"HTTP_CALLS","note":"URLs built dynamically (string concat / variables / base-url config) may not pair to a route","verify":"grep the path literal or base URL","edges_found":1},…,
 {"language":"*","edge_category":"DOCUMENTS","note":"repo markdown is ingested only from: the well-known files (README*.md, …) at the repo root and at each PROJECT root, a docs/ tree at the repo root or at a PROJECT root, a top-level .ai/ tree, ADR directories (…), SDD feature docs (…) and synced external docs (`glia docs sync`); … A doc section DOCUMENTS a symbol only when it names it in a single-backtick span …",…,"edges_found":0},…,
 {"language":"go","edge_category":"IMPLEMENTS","note":"implicit interface satisfaction is inferred (Medium, DERIVED) from method names and, where the parser read both, signatures (parameter and result types, with a type alias declared in the repo resolved through the method's package and imports; …); a one-method interface pairs only across an import path (…), and a pair with a side in a _test.go file only when that test file imports the other side's package directly (or both share a package); …; methods promoted through embedded fields count (…); …","verify":"check the method signatures and receivers against the interface","edges_found":0},…]

# xstack-go-http; Datalog from -e (or a file, or stdin): the routes and their handlers. Without --json, one table per output
glia query . -e 'served(r, h) :- handles(r, h), node(r, "ROUTE").' --json
{"language":"datalog","outputs":[{"predicate":"served","arity":2,"columns":["r","h"],"total":1,"shown":1}],
 "results":[{"predicate":"served","values":[{"node":{"qname":"GET /users","kind":"ROUTE","file":"server/main.go","line":13}},
                                            {"node":{"qname":"server::main::listUsers","kind":"FUNCTION","file":"server/main.go","line":9}}],"tier":"fact","file":"server/main.go","line":13}],"absence":null}
# a negation reads an absence, so its rows carry the coverage caveats of what it negated
glia query . -e 'uncalled(f) :- node(f, "FUNCTION"), !calls(_, f).' --json
{…,"results":[{"predicate":"uncalled","values":[{"node":{"qname":"client::client::FetchUsers",…,"line":8}}],"tier":"fact",…,
  "caveats":[{"language":"*","edge_category":"CALLS","note":"calls through reflection, dynamic dispatch, or higher-order indirection are not resolved",…}]},…],"absence":null}
# the same evaluator behind a GQL read query; the client-to-route hop is resolver-paired, so the row is derived
glia query . --gql -e 'MATCH (f:FUNCTION)-[:CALLS]->(e:ENDPOINT)-[:HTTP_CALLS]->(r:ROUTE) RETURN f, r' --json
{"language":"gql","outputs":[{"predicate":"result","arity":2,"columns":["f","r"],"total":1,"shown":1}],
 "results":[{"predicate":"result","values":[{"node":{"qname":"client::client::FetchUsers","kind":"FUNCTION","file":"client/client.go","line":8}},
                                             {"node":{"qname":"GET /users","kind":"ROUTE","file":"server/main.go","line":13}}],"tier":"derived",…}],"absence":null}
# The predicates (node, edge, qname, file, in_scope, reaches, ...), the prelude (handles, calls, unreached, effect, ...) and the GQL subset:
# https://github.com/James-Chahwan/glia/blob/main/docs/rules.md. A query that matches nothing exits 1 with the absence.

# identity-proto-diverged; one message in three repos is one class; variant 1 is shipping's copy with the extra field
glia identity orders --with billing --with shipping --json
{"counts":{"classes":1,"members":3,"fact":1,"derived":0,"heuristic":0,"diverged":1,"by_rule":[["package_name",0],["manifest_name",0],["message_fqname",1],…]},
 "classes":[{"key":"message:proto:shop.v1.OrderCreated","tier":"fact","rules":["message_fqname"],"collapses":true,"walks":true,"diverged":true,"variants":2,
             "members":[{"repo":"orders","qname":"message:proto:shop.v1.OrderCreated","kind":"MESSAGE_TYPE","file":"proto/orders.proto","line":5,"variant":0},
                        {"repo":"billing",…,"variant":0},{"repo":"shipping",…,"variant":1}]}],"omitted":0,"absence":null}

# xstack-go-http; "suggest" names the .glia/overlay.toml section that could repair the row
glia gaps . --json
{"counts":{"dead_symbol":1,"unpaired_endpoint":0,"tag_only_queue":0,…},"skipped":[],
 "rows":[{"category":"dead_symbol","qname":"client::client::FetchUsers","file":"client/client.go","line":8,"detail":"no entrypoint reaches it; no incoming call or use","suggest":"entrypoints","tier":"heuristic"}]}

# docs-adr-decision: the Decision section of doc/adr/0001-use-flask-for-orders.md names list_orders
glia docs-for . list_orders --json
{"results":[{"qname":"docs::doc::adr::0001-use-flask-for-orders::decision","kind":"DOC_SECTION","file":"doc/adr/0001-use-flask-for-orders.md","line":11,…}],"absence":null}
```

Declaring rules, edges glia misses and extra entry points in `.glia/overlay.toml`: https://github.com/James-Chahwan/glia/blob/main/docs/overlay.md. The Datalog a `rule` or `glia query` takes, the prelude of named concepts and the GQL subset: https://github.com/James-Chahwan/glia/blob/main/docs/rules.md. Test code in a directory glia does not know as a test path (`playwright/`, `qa/`) is declared there with `[walk] tests`; the `glia-overlay` skill covers it and the rest of the overlay loop.

## Install

1. Put `glia` on PATH: in a glia checkout, `cargo build --release -p glia-cli`, then `cp target/release/glia ~/.local/bin/`. Check it with `glia --version`.
2. Copy this file to `~/.claude/skills/glia/SKILL.md` (every project) or to `<repo>/.claude/skills/glia/SKILL.md` (one repo).

For an always-on MCP server over the same graph, see repo-graph: https://github.com/James-Chahwan/repo-graph
