---
name: glia-overlay
description: "Close glia's blind spots in a repo by writing .glia/overlay.toml stanzas: propose the gaps, write candidate stanzas from the code, measure them with glia overlay try, and accept only what the measurement keeps. Use it when glia gaps or glia coverage shows unpaired endpoints, unresolved sinks, tag-only queues, dead-flagged symbols or overlay rot."
---

# Closing glia's blind spots with the overlay

glia builds its graph from source, so it misses links the source does not spell out: a URL built at runtime, a project's own request / publish / collection helper, an entry point that the OS or a scheduler calls. `<repo>/.glia/overlay.toml` declares them. glia never calls a model: you write candidate stanzas from the code, and glia measures them. Three commands run the loop:

- `glia overlay propose` lists the gaps a stanza could close. Each gap has a stable id (`gap:<16 hex>`) and the source lines around it. It writes nothing.
- `glia overlay try` builds the repo with and without your candidate stanzas and gives each stanza a verdict. It writes only the parse cache.
- `glia overlay accept` is the only writer of `.glia/overlay.toml`. It adds the stanzas you pick and removes rotted rules by gap id.

The schema of every section is in docs/overlay.md (https://github.com/James-Chahwan/glia/blob/main/docs/overlay.md). For queries (where is X, what does X affect), use the `glia` skill.

**From Python.** The wheel runs the same three steps in-process, with the same reports as the `--json` output: `glia_py.overlay_propose(repo_paths, categories=None, top_k=20, snippet_lines=3)`, `glia_py.overlay_try(repo_paths, candidate, leave_one_out=True)` and `glia_py.overlay_accept(repo_path, candidate=None, only=None, remove=None, dry_run=False)`. `repo_paths` is a list: the first is the primary repo, whose `.glia/overlay.toml` the loop reads, and the rest are what `--with` adds. `candidate` is the candidate's TOML text, not a path. `only` and `remove` are lists of stanza handles and gap ids. A refused candidate, or an accept that would not validate, raises `ValueError` with the loader's reasons, and nothing is written. `overlay_accept` is still the only writer of `.glia/overlay.toml`.

## When to use it, and when not

Use it when `glia gaps <repo> --json` lists rows whose `suggest` is `constants`, `route_prefix`, `wrapper`, `edge`, `entrypoints` or `remove`. `glia coverage <repo> --json` names the edge kinds glia misses by language.

Many rows need no stanza. Read the code, then leave the row alone when:
- it is a `heuristic` row that is legitimate: a public API route nothing in the build calls, an exported library function, a call to a third-party host (`fetch("https://nominatim.openstreetmap.org/...")`).
- the extractor misread the code, such as a `Set.delete(id)` counted as an HTTP DELETE. No stanza fixes that. Report it to the person as a glia bug, with the `file:line`.
- the code really has no counterpart. For example, a client sends `PUT /settings/account` and the server serves only `DELETE /settings/account`. That is a bug in the code, not a blind spot. Report it.
- it is generated code (a `.pb.dart`, `*.generated.ts`) flagged dead. Ask the person whether to skip it with `[walk] skip`. That setting is theirs, and it is not a candidate section.
- it is test code flagged dead or counted as product code because it lives where glia's built-in test paths do not look (`playwright/`, `qa/`, `acceptance/`, `smoke/`, `k6/`, `perf/`). Ask the person whether to declare it in `.glia/overlay.toml`:

  ```toml
  [walk]
  tests = ["playwright/", "qa/**/*.py"]
  ```

  The patterns use gitignore syntax and are anchored at the repo root. They only add: nothing un-tags a path. Every node in a matching file gets ORIGIN provenance `test_fixture`, so `tests-for` lists it as a test case, `gaps` leaves it out of `dead_symbol`, `patterns` leaves it out, and Engram drops it by default. A build prints `[provenance] declared tests repo=<label> patterns=<n> tagged=<d>`. Like `[walk] skip`, it is the person's setting. It is never a candidate section, so `overlay try` cannot measure it, and `--overlay-delta` does not see it either: `[walk]` applies under `--no-overlay` too. It does not yet add TESTS edges.
- it is a `wrapped_sink` (informational) or an `orphaned_cell` (repaired by `glia cell ls --check --rekey`, not by the overlay).

## The loop

1. **Survey.** Run `glia gaps <repo> --json` (add `--with <repo>` for every service the links cross). Read `counts` to see how many gaps each category has.
2. **Propose.** Run `glia overlay propose <repo> --json`, narrowing it with `--category <cat>` (repeatable), `--top-k <n>` and `--snippet-lines <n>`. Each row is a `glia gaps` row plus a `snippet`. `guide` names the docs/overlay.md section for each `suggest` value.
3. **Read.** For every row you target, read its snippet, then open the file around it. Look up every qname you will write with `glia find <repo> <name> --json`, and copy the `qname` field exactly.
4. **Write the candidate** at a temp path such as `/tmp/overlay-candidate.toml` (see "The candidate file").
5. **Try it:** `glia overlay try <repo> --candidate /tmp/overlay-candidate.toml --json`. Use the same `--with` repos as step 1.
6. **Fix or drop** each stanza that is not a `keep`, and try again.
7. **Accept the kept stanzas.** Run `glia overlay accept <repo> --candidate /tmp/overlay-candidate.toml --only <stanza> --dry-run --json`, read the diff, then run the same command without `--dry-run`.
8. **Prune rot.** Run `glia overlay propose <repo> --category orphaned_rule --category redundant_rule --json`, then `glia overlay accept <repo> --remove gap:<id> --json` for each row.
9. **Confirm.** Re-run `glia gaps <repo> --json`: the ids you closed are gone. `glia gaps <repo> --overlay-delta --json` measures the whole file. It leaves out `[entrypoints]`, because entry points are user config that still applies under `--no-overlay`.

## What to write, per `suggest`

| `suggest` | write | docs/overlay.md section |
|---|---|---|
| `constants` | `[constants] NAME = "/literal"` pins a `${NAME}` base to the value the runtime resolves it to | Example |
| `route_prefix` | `[[route_prefix]]`: a project served under a gateway prefix | Example |
| `wrapper` | `[[wrapper]]`: the project's own helper, so every call site mints the sink | `[[wrapper]]`: call sites of a project's own helpers |
| `edge` | `[[edge]]` between two qnames the code joins | Example |
| `entrypoints` | `[entrypoints] qnames`: a symbol called from outside the build | Example |
| `remove` | no stanza: `glia overlay accept <repo> --remove gap:<id>` | The loop |

A row may offer two options (`route_prefix|edge`, `constants|wrapper`). Pick the one that fixes the most rows. One wrapper or constant closes every call site that goes through it, while an `[[edge]]` closes one.

Each stanza below is one shape. In a real candidate, replace every `gap:<id>` with the row's id. A `# gap:` line holds ids and nothing else: every word on it is read as a link, and a link that is not a `gap:<16 hex>` id refuses the whole candidate. Put your own remarks on a comment line above it.

```toml
[constants]
# the unpaired `${GATEWAY}/users` endpoint row; the value comes from config or deployment files, never a guess
# gap: gap:<id>
GATEWAY = "/orders-svc"

# every route of the project is also served under the gateway prefix
# gap: gap:<id>
[[route_prefix]]
scope = "orders"             # a project label (glia projects <repo>) or a repo-relative path
prefix = "/orders-svc"
origin = "llm"

# request('GET', '/users') in web/src/client.ts
# gap: gap:<id>
[[wrapper]]
call = "request"
kind = "http"
method_arg = 0               # or a fixed method = "GET"
path_arg = 1
origin = "llm"

# api.get('/orders'): the member verb is the method
# gap: gap:<id>
[[wrapper]]
call = "api"
kind = "http"
receiver = true
origin = "llm"

# a tag-only queue: publish('order.placed', body)
# gap: gap:<id>
[[wrapper]]
call = "publish"
kind = "queue_producer"      # or queue_consumer
topic_arg = 0
origin = "llm"

# NewCollection[Room](c, db, "rooms") -> data_entity:nosql:rooms; Go infers most of these by itself (then it drops)
# gap: gap:<id>
[[wrapper]]
call = "NewCollection"
kind = "data_entity"
name_arg = 2
origin = "llm"

# the ROUTE row: unpaired because the client builds its URL at runtime
# gap: gap:<id>
[[edge]]
from = "web::src::activity::ActivityService::updateActivity"
to = "PATCH /api/activity/:id @api"
category = "HTTP_CALLS"      # client function -> the ROUTE it hits: the route counts as called
note = "activity.ts:255 patches buildApiUrl(`activity/${encodeURIComponent(id)}`); the call inside the template hides the path"
origin = "llm"

[entrypoints]
qnames = [
    # started by the scheduler; `<prefix>::*` matches everything under a prefix
    # gap: gap:<id>
    "app::jobs::nightly_rollup",
]
```

- **unresolved_endpoint**: `detail` names the owner (`owner=src::client::request`). That owner is usually the wrapper, and its last qname segment is the `call`. The sink inside the wrapper remains after the wrapper is accepted, and `glia gaps` then lists it as a `wrapped_sink`.
- **suspected_edge**: the row's `draft` is a paste-ready `[[edge]]` whose `# gap:` line is already filled in. Read both ends first, then replace its `note` with your reason.
- **cochange_no_edge** is git history, not proof of coupling. Write an `[[edge]]` only when the code shows the link.

## The candidate file

- **Sections.** A candidate holds only `[constants]`, `[entrypoints]`, `[[route_prefix]]`, `[[wrapper]]` and `[[edge]]`, plus an optional `version = 1`. Any other section, or any key the loader does not know, refuses the whole candidate.
- **Gap links.** Put one or more `# gap: gap:<16 hex>` comment lines directly above each stanza's header, naming the rows it targets. For a constant, put them above its key. For an entry point, put them above its item in a multi-line `qnames` array. A gap line holds ids only. `try` reports `closes` against these links.
- **Origin.** Write `origin = "llm"` on every `[[route_prefix]]`, `[[wrapper]]` and `[[edge]]`. It gives their results Weak confidence (`origin = "human"`, a person's, gives Medium), and `glia why` ranks every overlay edge HEURISTIC. `[constants]` and `[entrypoints]` take no `origin` key.
- **Notes.** Every `[[edge]]` needs a `note` saying why: the call site's `file:line` and what hides it from glia.
- **Location.** Never put the candidate under `.glia/`, because the files there are fingerprinted build inputs. Use a temp path.

## Reading the try report

- Each stanza gets a handle (`wrapper#1`, `edge#2`, `route_prefix#1`, `constants.GATEWAY`, `entrypoints#3`) and a `verdict`:
  - `keep`: a gap category fell or the graph grew, and no gap category rose.
  - `review`: the graph improved, but a gap category rose too. A person must decide. Do not accept it yourself. Report the stanza and the category that rose.
  - `drop`: no gap category fell and the graph did not grow. The stanza binds nothing, repeats what glia already has (Go's inferred collection wrappers, say), or repeats another stanza. Fix it or leave it out.
- A stanza's `marginal` is its own effect: the build with every stanza, minus the build without this one. A stanza that only repeats another shows no effect and drops.
- A stanza's `closes` lists the linked gaps that it closed by itself. The top-level `closed` also counts gaps that several stanzas closed together.
- `keep` is a measurement, not a proof. It cannot tell whether an added edge is right, which is why you read the code first.
- `--no-leave-one-out` runs only two builds. You get the totals and the verdict but no per-stanza rows. Use it for a quick first look at a large candidate. Run the full try before you accept.
- Exit 0 is any report. Exit 2 means the candidate was refused (each loader error is listed at its line) or the build failed.

## Accepting and pruning

- Pass `--only <handle>` once for each kept stanza. Without `--only`, accept writes every stanza in the candidate.
- Run with `--dry-run` first: it prints the diff and writes nothing. A constant or pattern the file already holds counts as a duplicate and writes nothing.
- A stanza of the file whose code was renamed turns up as an `orphaned_rule`. Try the replacement first, then run one accept that does both: `glia overlay accept <repo> --candidate /tmp/overlay-candidate.toml --remove gap:<id> --json`.
- `--remove` takes only `orphaned_rule` / `redundant_rule` ids: `[[edge]]`, `[entrypoints]`, `[[constraint]]`, `[[decision]]` and `[[note]]` stanzas. If a `[[wrapper]]`, `[[route_prefix]]` or constant stops doing anything, report it. A person removes it.
- Exit codes: 0 written, dry run, or nothing changed. 1 the result would not validate (nothing written). 2 a usage, read or build error.

## Do not

- write a qname you did not see in `glia find` or `glia overlay propose` output.
- write an `[[edge]]` for each call of a whole class of calls. Write the wrapper or the constant that fixes them all.
- write a stanza for a row whose snippet and surrounding code you did not read.
- draw an `[[edge]]` from an `endpoint:<METHOD>:<unresolved>` sink. That one node pools every unresolved call of that verb in the repo.
- accept a `drop` or a `review`, or guess a constant's value.
- edit `.glia/overlay.toml` by hand as part of the loop. `accept` is its only writer: it validates the result and shows the diff.

## Cost, and what gets written

- `try` runs at most N + 2 builds for N stanzas: the file as it is, the file with every stanza, and the file with every stanza but one, once per stanza. Only distinct overlay texts are built, so a stanza the file already holds costs nothing. Parses are cached across the builds, but the post-cache phases rerun each time (wrappers, constants, resolvers, passes). Split a large candidate by category.
- `gaps`, `propose` and `find` write nothing. `try` writes only the parse cache, `<repo>/.glia/graph/parse_cache.bin` (self-gitignored), even under `GLIA_NO_PERSIST=1`. Only `accept` writes `.glia/overlay.toml`.

## Worked example

Every command below was run from the root of a copy of glia's fixture `bench/substrate-gap/fixtures/xcut-overlay-wrapper` with `web/.glia/overlay.toml` deleted. `web/src/client.ts` calls `request('GET', '/users')`, `request("POST", "/users")` and `api.get('/orders')`, and `api/app.py` (Flask) serves all three routes. Outputs are trimmed: `…` marks dropped fields and rows.

```bash
# the survey: three unpaired routes, and the sink inside request()
glia gaps web --with api --json
{"counts":{"unresolved_endpoint":1,"unpaired_route":3,"dead_symbol":4,"wrapped_sink":0,…},"skipped":[],
 "rows":[{"id":"gap:d33d5cdcf04a5343","category":"unresolved_endpoint","qname":"endpoint:GET:<unresolved>","kind":"ENDPOINT","file":"src/client.ts","line":4,"detail":"owner=src::client::request","suggest":"wrapper","tier":"fact"},
         {"id":"gap:4e0a8658155d53b0","category":"unpaired_route","qname":"GET /users","kind":"ROUTE","file":"app.py","line":7,"detail":"no incoming HTTP_CALLS; handler=app::list_users","suggest":"route_prefix|edge","tier":"heuristic"},…]}

# the work list, with source
glia overlay propose web --with api --category unresolved_endpoint --category unpaired_route --snippet-lines 2 --json
{"rows":[{"id":"gap:d33d5cdcf04a5343","category":"unresolved_endpoint","qname":"endpoint:GET:<unresolved>","detail":"owner=src::client::request","suggest":"wrapper",…,
          "snippet":{"file":"src/client.ts","start_line":2,"lines":["","export function request(method: string, path: string) {","  return fetch(path, { method });","}",""]}},
         {"id":"gap:8089976aea650fd5","category":"unpaired_route","qname":"GET /orders","file":"app.py","line":17,…},…],
 "counts":{"unpaired_route":3,"unresolved_endpoint":1},"snippets":4,"ambiguous_root":0,"guide":"Each row's `suggest` names what could close it (docs/overlay.md): …"}

# the helper the owner names, before writing `call = "request"`
glia find web request --json
{"results":[{"qname":"src::client::request","name":"request","kind":"FUNCTION","live":false,"file":"src/client.ts","line":3,"match":"exact_name",…}],"absence":null}
```

Reading `client.ts` shows two helpers: `request(method, path)` and a client object whose member verb is the method (`api.get('/orders')`). Both go in `/tmp/overlay-candidate.toml`:

```toml
# request(method, path) wraps fetch: every call site names its verb and path.
# gap: gap:d33d5cdcf04a5343
# gap: gap:4e0a8658155d53b0
# gap: gap:5905d8b3fb366ba8
[[wrapper]]
call = "request"
kind = "http"
method_arg = 0
path_arg = 1
languages = ["typescript"]
origin = "llm"

# api = makeClient(): api.get('/orders') is a GET through the project's client.
# gap: gap:8089976aea650fd5
[[wrapper]]
call = "api"
kind = "http"
receiver = true
languages = ["typescript"]
origin = "llm"
```

```bash
glia overlay try web --candidate /tmp/overlay-candidate.toml --with api --json
{"stanzas":[{"stanza":"wrapper#1","gaps":["gap:d33d5cdcf04a5343","gap:4e0a8658155d53b0","gap:5905d8b3fb366ba8"],"closes":["gap:4e0a8658155d53b0","gap:5905d8b3fb366ba8"],
             "marginal":{"nodes":{"ENDPOINT":2},"edges":{"CALLS":2,"HTTP_CALLS":2},"gaps":{"unpaired_route":-2}},"verdict":"keep"},
            {"stanza":"wrapper#2","gaps":["gap:8089976aea650fd5"],"closes":["gap:8089976aea650fd5"],
             "marginal":{"nodes":{"ENDPOINT":1},"edges":{"CALLS":1,"HTTP_CALLS":1},"gaps":{"unpaired_route":-1}},"verdict":"keep"}],
 "base":{…},"with":{…},"delta":{"nodes":{"ENDPOINT":3},"edges":{"CALLS":3,"HTTP_CALLS":3},"gaps":{"unpaired_route":-3}},
 "verdict":"keep","closed":["gap:4e0a8658155d53b0","gap:5905d8b3fb366ba8","gap:8089976aea650fd5"],"builds":4}

# both keep: accept them (run once with --dry-run first to read the diff)
glia overlay accept web --candidate /tmp/overlay-candidate.toml --only wrapper#1 --only wrapper#2 --json
{"added":{"wrapper":2},"removed":0,"duplicates":0,"file":"web/.glia/overlay.toml","dry_run":false,"written":true,
 "diff":"--- /dev/null\n+++ b/.glia/overlay.toml\n@@ -0,0 +1,22 @@\n+version = 1\n+\n+# request(method, path) wraps fetch: …\n+# gap: gap:d33d5cdcf04a5343\n…+[[wrapper]]\n+call = \"request\"\n…"}
```

`glia gaps web --with api --json` now counts `unpaired_route` at 0. The `fetch` inside `request()` is listed as a `wrapped_sink`, which is informational.

```bash
# later: src::client::loadUsers was accepted into [entrypoints] (gap:a9ecd9046857cb09), then renamed fetchUsers
# the pattern now binds nothing, so prune it by its gap id
glia overlay propose web --with api --category orphaned_rule --category redundant_rule --json
{"rows":[{"id":"gap:10babdf1b14bceb0","category":"orphaned_rule","qname":"src::client::loadUsers","kind":"entrypoint","file":".glia/overlay.toml","line":27,
          "detail":"repo=web entrypoints qname=src::client::loadUsers (no node)","suggest":"remove","tier":"fact",…}],"counts":{"orphaned_rule":1,"redundant_rule":0},…}
glia overlay accept web --remove gap:10babdf1b14bceb0 --json
{"added":{},"removed":1,"duplicates":0,"file":"web/.glia/overlay.toml","dry_run":false,"written":true,
 "diff":"--- a/.glia/overlay.toml\n+++ b/.glia/overlay.toml\n@@ -23,6 +23,4 @@\n \n [entrypoints]\n qnames = [\n-    # gap: gap:a9ecd9046857cb09\n-    \"src::client::loadUsers\",\n ]\n"}
```

## Install

1. Put `glia` on PATH. The `glia` skill's Install section shows how.
2. Copy this file to `~/.claude/skills/glia-overlay/SKILL.md` (every project) or to `<repo>/.claude/skills/glia-overlay/SKILL.md` (one repo).
