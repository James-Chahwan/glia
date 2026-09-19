# `.glia/overlay.toml` reference

One checked-in, reviewable file that feeds a glia build what static extraction cannot see.
The build stays deterministic: glia never calls a model; it applies this file the same way on every build.
Schema and loader: `code-domain/src/glia_config.rs`.

## Where it lives

Only `<repo root>/.glia/overlay.toml` is read. glia never walks `.glia/`, so a nested
`.glia/overlay.toml` in a sub-project is ignored. Address sub-projects from the root file
with `scope` / `path` (a project label or a repo-relative path; `.` or `""` means the whole repo).
Check the file in. It is an input, not a cache.

## Three kinds of content

| sections | kind | applied | `--no-overlay` |
|---|---|---|---|
| `[walk]`, `[[project]]`, `[entrypoints]` | user config | always | still applied |
| `[[constraint]]`, `[[decision]]`, `[[note]]` | declared knowledge -> CONSTRAINT / DECISION / CONV cells | always | still applied |
| `[constants]`, `[[route_prefix]]`, `[[wrapper]]`, `[[edge]]` | overlay: inference written by a model or a person | by default | skipped |

Provenance: every node, edge or cell the overlay creates carries an ORIGIN cell. An `origin = "llm"`
stanza (the default) gets `overlay:llm` and `Weak` confidence; an `origin = "human"` stanza gets
`overlay:human` and `Medium`. Nothing from the overlay is ever `Strong`. Primitives can hide
overlay results, and `--no-overlay` builds without them.

## Example

```toml
version = 1                               # required; any other value ignores the whole file

[walk]
skip = ["legacy/", "*.generated.ts"]      # gitignore syntax, anchored at the repo root; only adds skips

[[project]]
path = "tools/migrator"                   # a sub-project root that has no manifest
label = "migrator"                        # optional; defaults to the directory name

[entrypoints]
qnames = ["app::jobs::nightly_rollup", "app::cli::*"]   # exact qname, or <prefix>::* for its descendants

[constants]
GATEWAY = "/orders-svc"                   # pins `${GATEWAY}` to this literal (secret-shaped names are refused)
"api.base_url" = "/api"                   # quote keys that contain a dot

[[route_prefix]]
scope = "orders"                          # every ROUTE in this project is also mounted at prefix + path
prefix = "/orders-svc"

[[wrapper]]                               # request('GET', '/users') -> ENDPOINT GET /users
call = "request"
kind = "http"
method_arg = 0                            # or a fixed `method = "GET"`: exactly one of the two
path_arg = 1

[[wrapper]]                               # api.get('/orders') -> ENDPOINT GET /orders
call = "api"
kind = "http"
receiver = true                           # the member verb is the method; path_arg defaults to 0

[[wrapper]]
call = "publish"
kind = "queue_producer"                   # or queue_consumer
topic_arg = 0
broker = "nats"                           # optional framework tag
languages = ["typescript"]                # optional: engine language names; empty = all

[[edge]]
from = "web::src::report::loadReport"
to = "api::report::build_report"
category = "CALLS"
note = "called through a runtime-built URL"
origin = "human"

[[constraint]]
id = "web-no-db"
kind = "forbid_edge"                      # forbid_edge (needs from + to) | no_cycle | invariant (needs text)
from = "web"
to = "services/api"
categories = ["CALLS"]                    # optional; edge category names

[[decision]]
id = "adr-7"
scope = "services/api"                    # or anchor = "<qname>"
title = "Charges are idempotent"          # title or text is required
status = "accepted"

[[note]]
anchor = "services::api::app::charge"
text = "retries are safe"                 # 1..=4096 chars; id defaults to note#<n>
by = "james"
```

`origin` is accepted on `[[route_prefix]]`, `[[wrapper]]`, `[[edge]]` and `[[constraint]]`.
Stanza ids are 1-128 characters with no control characters, unique within their section.
Every `*_arg` is a 0-based positional index, at most 8.

## Validation

Every table rejects unknown keys, so a typo is an error, not a silently ignored line. Errors
never fail a build. Each one is printed once as `[overlay] error: .glia/overlay.toml:<line>: <message>`.

- A TOML syntax error, an unknown key, a wrong type, a missing required field, or a `version`
  other than 1 ignores the **whole file**.
- A stanza that parses but breaks a rule is **dropped on its own**. The rest of the file
  still applies. The rules:
  - an `[[edge]]` category that is not an edge category name, or is `DEFINES`, `CONTAINS`
    or `CO_CHANGES`
  - a `route_prefix` that does not start with `/` or contains whitespace
  - a wrapper whose argument layout does not fit its kind
  - an unknown constraint kind, or one missing its required fields
  - a duplicate id
  - a constant name outside `[A-Za-z_][A-Za-z0-9_.]*`
  - an invalid skip pattern
  - a project path that is the repo root or leaves it

## What the overlay cannot do

- It cannot add a node kind, edge category or cell type. Only locked registry ids exist.
- It cannot declare structural edges (`DEFINES`, `CONTAINS`) or history edges (`CO_CHANGES`;
  those come from the history snapshot, below).
- It cannot un-skip a directory the walk hard-skips, or un-collapse a collapsed region.
- It cannot bypass secret redaction: a pinned constant goes through the same secret and
  value gates as one read from source.

## The loop: gaps -> overlay -> overlay delta

1. `glia gaps <repo>` lists the blind spots: unpaired endpoints and routes, `<unresolved>`
   endpoint sinks, tag-only queues, dead-flagged symbols, and overlay rules that no longer
   bind. Each row suggests a section to fill.
2. An agent or a person writes stanzas for those rows into `.glia/overlay.toml` (repo-graph
   hands the gaps to its agent and writes the file back).
3. `glia gaps <repo> --overlay-delta` builds with and without the overlay and prints
   `[overlay] N rules, +M edges, orphans K→J`. Keep the change only if pairings rise.
   Rules that `gaps` reports as orphaned or redundant (the extractor has caught up) should
   be removed.

On every build that finds the file, stderr carries
`[overlay] loaded .glia/overlay.toml repo=<label> version=1 (walk=N project=N entrypoints=N constants=N route_prefix=N wrapper=N edge=N constraint=N decision=N note=N) errors=E`.
The counts are skip patterns for `walk`, qname patterns for `entrypoints`, keys for
`constants`, and stanzas for every other section.

## History snapshot

Git history is a build input too, but not an overlay section. `glia history sync <repo>`
(pyo3: `history_sync(repo_path)`) reads the repo's local git (no fetch, no remote, no author
or committer identity) and writes `.glia/history-snapshot/` (`commits.jsonl`, `blame.jsonl`,
`meta.json`), replacing the previous one. The next build ingests it: churn ATTN cells on
MODULE nodes, blame-recency ATTN on symbols (with `--blame`), and heuristic `CO_CHANGES` edges
between modules that change together. The build itself never runs git, so re-sync when HEAD
moves. Flags: `--max-commits N` (default 2000), `--since <date>` (passed to `git log --since`),
`--blame`, `--blame-max-files N` (default 300). The snapshot is local and regenerable: when
`.glia/.gitignore` is absent, the sync creates one that lists it.

The sync prints `[history] sync repo=<label> head=<12 hex> commits=N files=N renames=N binary=N
blame_files=N runs=N window=max:N[,since:<date>] surface=cli|pyo3` on stderr (one line), and a
build that reads the snapshot prints `[history] ingest repo=<label> ...`.
