---
name: glia
description: Answer structural questions about a codebase with the glia CLI, no MCP server needed. Use it for where X is defined, what changing X (or a diff) affects, how feature Y flows across services, what calls, serves or implements X, what X writes or sends, which tests cover a change, and what a change did to the graph. Reach for it before grep when the question is about structure, not text.
---

# glia from the command line

glia parses a repo into one cross-language graph and answers questions from it. The graph holds calls, imports, the HTTP / gRPC / GraphQL / queue / event / WebSocket links between services, data access, tests and docs. Each answer is one `glia <command> <repo> ...` call. Add `--json` for structured output (progress lines go to stderr). Open the `file:line` an answer gives and read it there rather than grepping for it.

## Which command

| Question | Command |
|---|---|
| What services are there, and how do they talk? | `glia arch <repo> --json` |
| Where is X? (a name, qname or fragment) | `glia find <repo> X --json` |
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
| Are there loops between services, or import cycles? | `glia cycles <repo> --json` |
| Does the code break the repo's declared rules? | `glia check <repo> --json` |
| Where is glia blind in this repo? | `glia coverage <repo> --json` |
| What could glia not pair? What looks dead? | `glia gaps <repo> --json` |
| Which docs govern X? | `glia docs-for <repo> X --json` |

The rest of the CLI:
- `glia pages <repo>`: frontend router pages and dead links.
- `glia contracts <repo>`: producer vs consumer message types per queue topic.
- `glia spec-status <repo>`: OpenAPI / feature.yaml operations implemented or missing.
- `glia projects <repo>`: the labels `--scope` takes.
- `glia impact <repo> X`: plain reachability, unranked.

`glia <command> --help` lists every flag.

Flags most commands share:
- `--with <repo>` (repeatable) merges more repos in, so links pair across them. With one service per repo, pass the others this way. Each row's file stays relative to its own repo's root.
- `--scope <path-or-label>` narrows the answer within a repo.
- The ranked walks also take `--top-k`, `--depth` and `--direction forward|backward|both`.

## Reading an answer

- **Location.** Node rows carry `file` and `line`: 1-based, relative to the repo. Some nodes have no single definition: `endpoint:GET:/users` (an outbound call), `data_entity:sql:prefs` (a table) and queue topics can have `file: null`. For those, the edge's `site_file` / `site_line` (in `path`, `witness` and `site`) locates the code.
- **Liveness.** `live: false` (`⊘` in tables) means no entry point reaches the node. It is likely dead, or it is reached from an entry point glia does not know.
- **Tier.** `tier` is `fact` (declared in the source), `derived` (inferred by a resolver, e.g. a client URL paired to a route) or `heuristic` (paired by name). Case varies by command. Weigh the answer by it.
- **Empty answers.** An empty answer is not "nothing exists". It carries `absence: {tier, reason, note, caveats, suggestions}`. Each caveat names an edge kind glia is known to miss (`edge_category`, `language`), how many it did find (`edges_found`) and what to grep instead (`verify`).
- **Blind spots.** `glia coverage <repo> --json` lists the same caveats for the whole repo. Run it once before trusting "not found".
- **Exit codes.** 0 for any answer, empty included. `why` exits 1 when no edge joins the two nodes (it still prints a witness path). `check` exits 1 on a violation. `blast-radius` exits 3 when no seed resolved. 2 means a usage or build error, or a rule `check` could not evaluate.

## Cost, and what gets written

- There is no daemon and no index to keep fresh: each call builds the graph from source in memory, then answers. Rather than looping, pass several seeds to one call (`blast-radius`, `effects` and `tests-for` take many).
- The query commands write nothing to the repo.
- Three commands save a parse cache at `<repo>/.glia/graph/parse_cache.bin` (self-gitignored): `delta`, `diff-impact` against a rev (its default), and `tests-for --base`. The `--diff` forms write nothing.
- `flows --features` without `--json` writes `<repo>/.glia/graph/flows/`.
- `glia build <repo>` writes the `.glia/graph/` layout that the MCP server and Python bindings load. The CLI does not need it.
- **`GLIA_NO_PERSIST=1`** is for probing a repo you must not write to: set it on every call. It stops glia rewriting a stale layout (`glia merge --gmap`). It does not stop the parse cache or the flows files above. On such a repo, use the `--diff` forms and `flows --features --json` instead: `git -C <repo> diff | GLIA_NO_PERSIST=1 glia diff-impact <repo> --diff - --json`.

## Worked examples

Every example below was run from the root of the repo its comment names. Most are glia's test fixtures (`bench/substrate-gap/fixtures/<name>`). The rest are small git repos built to show a change or a rule. Outputs are trimmed: `…` marks dropped fields and rows.

- `xstack-go-http`: `client/client.go` `FetchUsers` does `http.Get(".../users")`, and `server/main.go` serves `GET /users` with chi.
- `loop`: `orders/` emits `order.placed`, `billing/` handles it and emits `payment.settled`, and `orders/` handles that. The change adds `audit()` to `billing/billing.ts` and calls it from `settle()`.

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

# xstack-go-http
glia blast-radius . client::client::FetchUsers --direction forward --json
{"seeds":[{"query":"client::client::FetchUsers","kind":"FUNCTION","file":"client/client.go","line":8,…}],"unresolved":[],
 "results":[{"qname":"endpoint:GET:/users","kind":"ENDPOINT","reason":"CALLS","depth":1,"score":0.267,"live":false,"file":"client/client.go","line":9,"seed":"client::client::FetchUsers"},
            {"qname":"GET /users","kind":"ROUTE","reason":"HTTP_CALLS","depth":2,"file":"server/main.go","line":13,…},
            {"qname":"server::main::listUsers","reason":"HANDLED_BY","depth":3,"file":"server/main.go","line":9,…}],"absence":null}

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

# loop; kinds are event_loop, call_loop, possible_loop and import_cycle (--kind event|import|all)
glia cycles . --json
[{"kind":"event_loop","tier":"derived","services":["billing","orders"],"channels":["order.placed","payment.settled"],"size":9,"members":[…],
  "witness":[{"from_qname":"event_emit:order.placed","to_qname":"event_handle:order.placed","category":"EVENT_FLOWS","channel":"order.placed","file":"orders/orders.ts","line":3},…]}]

# a repo with web/ and services/api/ (a pyproject.toml each) whose web/app.py imports and calls services/api/internal.py;
# .glia/overlay.toml has a [[constraint]] kind = "forbid_edge" from "web" to "services/api", categories IMPORTS + CALLS; exit 1
glia check . --json
{"rules":1,"checked":1,"unchecked":[],"errors":[],"violations":[{"rule_id":"web-no-api-internals","rule_kind":"forbid_edge","decl":".glia/overlay.toml:3","tier":"fact","count":2,
  "evidence":[{"from_qname":"web::app","to_qname":"services::api::internal","category":"IMPORTS","file":"web/app.py","line":1,"emitter":"graph:imports"},…]}]}

# xstack-go-http
glia coverage . --json
[{"language":"*","edge_category":"HTTP_CALLS","note":"URLs built dynamically (string concat / variables / base-url config) may not pair to a route","verify":"grep the path literal or base URL","edges_found":1},
 {"language":"go","edge_category":"IMPLEMENTS","note":"implicit interface satisfaction is inferred … from method NAMES …","verify":"check the method signatures and receivers against the interface","edges_found":0},…]

# xstack-go-http; "suggest" names the .glia/overlay.toml section that could repair the row
glia gaps . --json
{"counts":{"dead_symbol":1,"unpaired_endpoint":0,"tag_only_queue":0,…},"skipped":[],
 "rows":[{"category":"dead_symbol","qname":"client::client::FetchUsers","file":"client/client.go","line":8,"detail":"no entrypoint reaches it; no incoming call or use","suggest":"entrypoints","tier":"heuristic"}]}

# docs-adr-decision: the Decision section of doc/adr/0001-use-flask-for-orders.md names list_orders
glia docs-for . list_orders --json
{"results":[{"qname":"docs::doc::adr::0001-use-flask-for-orders::decision","kind":"DOC_SECTION","file":"doc/adr/0001-use-flask-for-orders.md","line":11,…}],"absence":null}
```

Declaring rules, edges glia misses and extra entry points in `.glia/overlay.toml`: https://github.com/James-Chahwan/glia/blob/main/docs/overlay.md

## Install

1. Put `glia` on PATH: in a glia checkout, `cargo build --release -p glia-cli`, then `cp target/release/glia ~/.local/bin/`. Check it with `glia --version`.
2. Copy this file to `~/.claude/skills/glia/SKILL.md` (every project) or to `<repo>/.claude/skills/glia/SKILL.md` (one repo).

For an always-on MCP server over the same graph, see repo-graph: https://github.com/James-Chahwan/repo-graph
