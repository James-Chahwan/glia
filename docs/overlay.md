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
| `[[constraint]]`, `[[decision]]`, `[[note]]`, `[[component]]`, `[[layer]]` | declared knowledge -> CONSTRAINT / DECISION / CONV cells | always | still applied |
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

[[wrapper]]                               # NewCollection[Room](c, db, "rooms") -> DATA_ENTITY data_entity:nosql:rooms
call = "NewCollection"
kind = "data_entity"
flavor = "nosql"                          # sql | nosql | graph; default nosql
name_arg = 2                              # required: the argument holding the table / collection name

[[edge]]
from = "web::src::report::loadReport"
to = "api::report::build_report"
category = "CALLS"
note = "called through a runtime-built URL"
origin = "human"

[[constraint]]
id = "web-no-db"
kind = "forbid_edge"                      # forbid_edge (needs from + to) | no_cycle | invariant (needs text) | allow
from = "web"
to = "services/api"
categories = ["CALLS"]                    # optional; edge category names

[[component]]                             # the reflexion model: see "Reflexion model" below
name = "web"
paths = ["web"]                           # repo-relative paths or project labels; one path, one component
[[component]]
name = "api"
paths = ["services/api"]
text = "orders and payments"              # optional

[[layer]]                                 # layers rank in file order, top first
name = "ui"
components = ["web"]
strict = false                            # true: may use the NEXT layer only

[[constraint]]
id = "web-uses-api"
kind = "allow"                            # from / to are [[component]] names
from = "web"
to = "api"

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

`origin` is accepted on `[[route_prefix]]`, `[[wrapper]]`, `[[edge]]`, `[[constraint]]` and `[[component]]`.
Stanza ids are 1-128 characters with no control characters, unique within their section.
Component and layer names are 1-118 characters (their entry ids are `component:<name>` /
`layer:<name>`, so a `[[constraint]]` id may not start with either prefix).
Every `*_arg` is a 0-based positional index, at most 8.

## `[[wrapper]]`: call sites of a project's own helpers

A wrapper stanza names a callee (`call`: a bare or dotted name such as `request`,
`api.request` or `NewCollection`). Every call site of it mints the node a direct call
to the framework would have minted, with an edge from the function that holds the site
(the file's module when no function does).

| `kind` | identity from | mints |
|---|---|---|
| `http` | `method_arg` or a fixed `method`, plus `path_arg`; or `receiver = true` (`api.get('/x')`: the member verb is the method, the path is `path_arg`, default 0) | the client ENDPOINT, `owner -CALLS-> endpoint` |
| `queue_producer` / `queue_consumer` | `topic_arg`, optional `broker` | `queue_producer:<topic>` / `queue_consumer:<topic>`, `owner -USES-> producer` / `consumer -HANDLED_BY-> owner` |
| `data_entity` | `name_arg` (required), `flavor` = `sql` / `nosql` / `graph` (default `nosql`) | DATA_ENTITY `data_entity:<flavor>:<name>`, `owner -ACCESSES_DATA-> entity` |

`data_entity` is for a project-local constructor that every repository goes through,
where the table or collection name is an argument and the database driver call inside
the constructor sees only a variable:

```go
func NewCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {
	return &Collection[T]{inner: client.Database(database).Collection(name)}
}

func NewChatPreviewRepository(client *mongo.Client, database string) *ChatPreviewRepository {
	return &ChatPreviewRepository{collection: NewCollection[ChatPreview](client, database, "chat_previews")}
}
```

With `call = "NewCollection"`, `kind = "data_entity"`, `name_arg = 2`, the call site mints
`data_entity:nosql:chat_previews` with an ACCESSES_DATA edge from
`NewChatPreviewRepository`. The qname is the one the data-entity extractor gives a direct
`.Collection("chat_previews")` call, so a wrapper site and a driver call collapse onto one
node, and the cross-service DB resolver pairs it like any other entity. A `sql` wrapper
works the same way (`Table("orders")` -> `data_entity:sql:orders`).

In Go you do not need the stanza for `NewCollection`: a function that hands its own
parameter to a driver collection call (`.Collection(name)`, `.GetCollection(name)`, ...) is
inferred as a `data_entity` wrapper with no overlay file, at that parameter's index, and so
is a function that hands its own parameter on to an inferred wrapper
(`NewNamedCollection(c, db, name)` calling `NewCollection[T](c, db, name)`, up to three
hops). Its call sites mint exactly what the stanza above would, except for provenance:
the node's ORIGIN is
`{"provenance":"inferred:wrapper","rule":"inferred:NewCollection","def":"<file>:<line>"}`
(the wrapper's `func` line), the node and edge are medium confidence, and the edge's
evidence emitter is `pass:inferred_wrapper`, which `glia why` ranks as derived. A wrapper
name shorter than 3 characters, or one that two Go functions or methods share, is not
inferred. Inference is read from the code, so it runs under `--no-overlay` too. A stanza
that names the same `call` shadows the inferred wrapper, so a repo that already declares
it builds exactly as before. Every build that finds a candidate prints one line:
`[wrappers] inferred repo=<label> wrappers=<w> (direct=<d> forwarding=<f>) sites=<n> minted=<m> duplicate=<u> skipped_nonliteral=<x> skipped_comment=<c> skipped_invalid=<i> shadowed_by_overlay=<s> skipped_ambiguous=<a> skipped_unparsed=<p>`.
Wrappers in other languages still need the stanza.

Rules shared by every kind:

- A generic argument list between the name and the paren is stepped over:
  `NewCollection[ChatPreview](`, `create<User>(`, `api.get<Order[]>(`. It must follow the
  name directly (no space), stay on one line and close within 256 bytes, so `a < b && c > (d)`
  is never a call.
- The definition of the callee is not a call site (`func NewCollection[T any](...)`,
  `function request(...)`, a typed parameter list that opens a body).
- A call in a comment is counted as `skipped_comment` and mints nothing.
- The identity argument must be exactly one string literal (a Ruby / Elixir `:symbol`
  also works for a method, a topic or an entity name). Anything else (a variable,
  a concatenation, a `${x}` placeholder in a topic or entity name) is `skipped_nonliteral`.
- A `data_entity` name must be at most 128 bytes with no whitespace, control character
  or `/`; otherwise the site is `skipped_invalid`.

Every build whose overlay keeps a wrapper stanza prints one line:
`[overlay] wrappers repo=<label> stanzas=<s> sites=<n> minted=<m> duplicate=<d> skipped_nonliteral=<x> skipped_comment=<c> skipped_invalid=<i> (http=<h> queue_producer=<p> queue_consumer=<q> data_entity=<e> receiver=<r>)`.
A `duplicate` is a site whose node and owner edge the file already holds (an extractor
caught the same call, or two stanzas match one call).

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
  - a wrapper whose argument layout does not fit its kind (a `data_entity` wrapper without
    `name_arg`, or with a `flavor` other than `sql` / `nosql` / `graph`)
  - an unknown constraint kind, or one missing its required fields; `kind = "component"` or
    `"layer"` in a `[[constraint]]` (the model is declared only through its own sections)
  - an `allow` whose `from` / `to` is not a kept `[[component]]`, names one component twice,
    or sets `scope`
  - a component with no paths, an empty path, a path that leaves the repo (`..`), or a path
    an earlier component already claims (exact-equal paths only; nested paths are legal)
  - a layer with no components, or naming a component that is not declared or already sits
    in another layer
  - a duplicate id, or a duplicate component or layer name
  - a constant name outside `[A-Za-z_][A-Za-z0-9_.]*`
  - an invalid skip pattern
  - a project path that is the repo root or leaves it

## Reflexion model

`glia check` rules (`forbid_edge`, `no_cycle`) judge one pair of scopes at a time. A reflexion
model (Murphy, Notkin and Sullivan) states the architecture once: the components, the code each
owns, and the dependencies allowed between them. The checker then reports what the code does
against that model.

- `[[component]]` maps a `name` to `paths`: repo-relative paths (`.` is the repo root) or
  project labels. A file belongs to the component with the longest path above it, so
  `web/admin` can be carved out of `web`. Optional: `text`, `origin`, `anchor`.
- `[[layer]]` groups components. Layers rank in file order, top first. A component may use a
  component of any lower layer; with `strict = true`, only of the next layer down. A use
  within one layer, or upward, is not allowed.
- `[[constraint]] kind = "allow"` permits `from` -> `to` between two components. `categories`
  (edge category names) narrows the edges the model checks, as on `forbid_edge`.

Storage. Each stanza is a CONSTRAINT entry (source `overlay`), so a loaded `.gmap` can be
checked without the file: a component is `{"id":"component:<name>","kind":"component","name",
"paths":[resolved],"paths_raw":[..],...}` (paths resolved like a rule's scope, so a label is
stored as its path), a layer `{"id":"layer:<name>","kind":"layer","name","rank","components",
"strict"}` (rank 0 is the top), and an allow keeps `from` / `to` as component names. Two paths
that resolve to one directory (a label and its path) count as one path claimed twice, and the
build rejects the later component.

Anchors. A component with `anchor` hangs on that qname. Otherwise it hangs on the PROJECT at its
first path, else the repo root's PROJECT, else (a repo with no root manifest) the smallest-id
MODULE under its first path. A layer, and an allow without `anchor`, hang on the repo root's
PROJECT, else wherever their first named component hung (the layer's first component, the
allow's `from`).

Evaluation (`glia check`, CC.5b). Every edge in the checked categories between two different
components is a dependency. An allowed dependency is a convergence. A dependency the model does
not allow is a divergence, reported as a violation with located evidence. An allow with no
dependency behind it is an absence: a FACT about the graph as built, with the coverage caveats of
the checked categories, never a violation. A model with components only (no layer, no allow) is
open: the dependency matrix is reported as observed and nothing diverges. Files that no
component owns are counted as unmapped.

A build with a model prints one line per repo:
`[declared] model repo=<label> components=<c> layers=<l> allows=<a> anchored=<n> orphaned=<o>`,
and a detail line for each stanza that did not anchor or was rejected. Component paths resolve
at build time, like rule scopes: rebuild after adding a project.

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
`[overlay] loaded .glia/overlay.toml repo=<label> version=1 (walk=N project=N entrypoints=N constants=N route_prefix=N wrapper=N edge=N constraint=N decision=N note=N component=N layer=N) errors=E`.
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

## SCIP snapshot

A compiler-grade SCIP index is a build input the same way: read once by a CLI step, stored as a
snapshot, never read by the build itself. Run the language's indexer (scip-python,
scip-typescript, scip-java, scip-go, rust-analyzer's `scip`), then
`glia scip import <repo> <index.scip> [--prefix <dir>]`. The import decodes the index (a
hand-written protobuf reader: no protobuf dependency, fields the schema adds later are skipped
and counted, one document in memory at a time, at most 256 MiB each) and writes
`.glia/scip-snapshot/` (`documents.jsonl`, `symbols.jsonl`, `meta.json`), replacing the previous
one. Nothing is written when the index cannot be decoded, or when no document of it was kept.

What the snapshot holds is read against the repo's own source at import, never at build: the
identifier text at every definition (the build binds a definition by position and checks it by
that name), whether each reference is followed by a call paren (SCIP has no call role), and a hash
of every document's bytes. Offsets are converted in the document's position encoding (UTF-8,
UTF-16 or UTF-32 code units; unspecified reads as UTF-16). Document-local symbols and forward
definitions are dropped, and so is any document whose path leaves the repo. Document paths are
relative to `--prefix` (a directory in the repo, `.` for its root) when given, else to the
index's `project_root`; a root outside the repo (an index made in a container or on another
machine) reads them as repo-relative, with one `[scip] warning:` line.

The build never runs an indexer, so re-index and re-import when the code moves. Staleness is per
document: a document whose file no longer hashes to the bytes the import read is skipped whole,
so an index never places facts on code edited since; the other documents still apply. What the
build makes of the snapshot:

- references bound to a definition become `CALLS` (a call of a function or method, rule
  `call_site`) or `USES` (`callable_ref`, `reference`, `reference_write`), emitter
  `scip:<tool>`, tier FACT (CE.1d). An edge glia already has between the same nodes keeps its
  category: when the index would classify it differently, glia's classification stands.
- a name-only or unlocated glia edge from the parser, extractor or graph stages that the index
  confirms is re-stamped `scip:<tool>` with rule `confirms:<old emitter>[/<old rule>]` and tier
  FACT; resolver and pass edges are never re-stamped (a merge recomputes them). The index's
  `is_implementation` relationships add the `IMPLEMENTS` / `INHERITS_FROM` edges glia lacks
  (rule `implementation`), and a heuristic edge the index contradicts is counted and printed,
  never removed (CE.1e).

Delta caveat: `glia delta` and `diff-impact` build the base rev with the same untracked snapshot,
and the per-document hash check skips every file that differs from the indexed bytes, so a file
changed between the rev and the working tree carries SCIP edges only on the side the index was
made for. Those edges then show as added (or removed) in the delta.

The snapshot is local and regenerable: its directory holds a `.gitignore` of `*`, so it ignores
itself even in a repo whose `.glia/.gitignore` predates it, and the import creates
`.glia/.gitignore` when it is absent. Exits 0 on a written snapshot, 1 when the index cannot be
decoded or nothing was kept, 2 on a usage error.

The import prints `[scip] decoded index=<file> documents=N occurrences=N symbols=N
external_symbols=N unknown_fields=N malformed_ranges=N` after a clean decode, then `[scip] import
repo=<label> tool=<tool>@<version> documents=N skipped=N defs=N refs=N calls=N symbols=N locals=N
forward=N bad_ranges=N encoding_unspecified=N surface=cli` on stderr (one line each), and a build
that reads the snapshot prints `[scip] ingest repo=<label> ...` and `[scip] confirm repo=<label>
...`.
