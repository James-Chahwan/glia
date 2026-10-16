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
version = 1                               # required: 1, or 2 to use rule / transitive / via / min_tier; any other value ignores the whole file

[walk]
skip = ["legacy/", "*.generated.ts"]      # gitignore syntax, anchored at the repo root; only adds skips
tests = ["playwright/", "qa/**/*.py"]     # test / fixture code the built-in test paths miss (ORIGIN test_fixture); only adds

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

Anchors. A `[[constraint]]`, `[[decision]]` or `[[note]]` with `anchor = "<qname>"` hangs on that
node, and an anchor that names no node orphans the stanza (it never falls back to a scope). A
`[[constraint]]` without `anchor` (an allow aside: see "Reflexion model") hangs by its scope, the
`from` of a forbid_edge and the `scope` of a no_cycle / invariant (none is the repo root): on the
PROJECT at that path, else on the deepest PROJECT above it (`from = "backend/internal/handlers"`
hangs on `project:backend`; a root PROJECT encloses every path), else, in a repo with no root
manifest, on the smallest-id MODULE under it. Where a rule hangs only stores it: `glia check`
evaluates the rule's own scopes, so a misspelt directory inside a project is reported there (`from
scope ... matches no located node`), not dropped. A `[[decision]]` without `anchor` hangs on the
PROJECT at its `scope` only. A stanza that hangs nowhere is orphaned: the build prints
`[declared] orphaned <section> id=<id> <decl> ...` on stderr and writes no cell.

`glia check` (and `PyGraph.check()`) reads each repo's overlay again and binds its
`[[constraint]]`, `[[component]]` and `[[layer]]` stanzas through the build's own binder, so a
declared rule that checks nothing is never silent. A stanza the build anchored nowhere (`orphaned:
anchor=<qname> binds no node`, `orphaned: scope=<raw> -> <path> (no PROJECT at or above it, no
MODULE under it)`, `orphaned: component=<name> (...)`), one it rejected (`rejected: <why>`, such as
a path two components claim), one this graph does not hold (`absent: ...`, a layout older than
the overlay: rebuild), and a rule whose scope or `via` facade no located node sits in, are listed
under "Rules that check nothing", each with its reason and a suggested fix (the nearest qname or
path that would anchor it, or what to declare). So are the stanzas the loader itself drops (see
"Validation"): a `[[constraint]]`, `[[component]]` or `[[layer]]` dropped at load (a version-2
key in a version-1 file, an exact path two components claim, a layer or allow naming a dropped
component, a bad kind) is a `rejected` row at its line with the loader's message (``rejected:
`transitive` need `version = 2` (this file says version = 1) (stanza dropped)``) and what to
change, listed before the other rows of its repo. A whole file the loader ignores (a TOML or
schema error, an unsupported or missing `version`) while it holds a rule stanza is one row, id
`.glia/overlay.toml`, section `overlay`, at the error's line: `rejected: <message> (rule stanzas
not read: constraint=<c> component=<m> layer=<l>)`; such a file is never "no rules declared". A
file with no rule stanza (only `[[edge]]` or `[[wrapper]]` stanzas) is not listed: its `[overlay]
error:` line and `glia gaps` report it. stderr carries `[check] overlay rejected root=<root>
whole_file=<true|false> constraint=<c> component=<m> layer=<l>` once per such repo. None of these
rows changes the exit code: `glia check` still exits 0 or 1 by its violations. `glia check
--strict` (`PyGraph.check(strict=True)`) makes each a rule error, so CI that wants every declared
rule to run exits 2 on one. Decisions and notes
are records, not rules, and are not listed (`glia gaps` reports a stale-anchor one as
`orphaned_rule`).

`origin` is accepted on `[[route_prefix]]`, `[[wrapper]]`, `[[edge]]`, `[[constraint]]` and `[[component]]`.
Stanza ids are 1-128 characters with no control characters, unique within their section.
Component and layer names are 1-118 characters (their entry ids are `component:<name>` /
`layer:<name>`, so a `[[constraint]]` id may not start with either prefix).
Every `*_arg` is a 0-based positional index, at most 8.
A `[[project]]` path that a manifest already roots keeps the detected root (`shadowed=` on the
`[roots] declared=` line), unless that manifest names no project (a `requirements.txt` or `Pipfile`
root, or one named after its directory): then the root takes the declared label and keeps its
ecosystem and manifest (`relabelled=`, DG.12). The PROJECT's ORIGIN cell records where its label
came from as `label_source`: `manifest`, `dir`, `root` or `declared` (DD.4).

## `[walk]`: skips and declared test paths

Both lists are gitignore syntax, anchored at the repo root: `legacy/` matches that directory at
any depth, `/legacy/` only at the root, `qa/**/*.py` the Python files below `qa/`, and a
`!pattern` re-includes a path against the earlier patterns of the same list. Both only add to
the built-in rules, and both are user config: applied with or without `--no-overlay`.

- `skip`: a matching path is never parsed. A skipped directory becomes a REGION node with
  ORIGIN provenance `excluded`.
- `tests`: test / fixture code the built-in test paths miss (they name directories such as
  `test`, `tests`, `__tests__`, `e2e`, `cypress`, `fixtures` and suffixes such as `_test.go`,
  `.spec.ts`; `playwright/`, `qa/`, `acceptance/`, `smoke/`, `k6/` and `perf/` are not on that
  list). The code is still parsed, and every node whose POSITION file matches carries ORIGIN
  provenance `test_fixture`, like a built-in test path: `tests-for` lists it as a test case,
  `gaps` leaves it out of `dead_symbol`, `patterns` excludes it, test-report ingest treats it
  as a fixture, and engram-export / Engram drop it by default. A pattern applies to its own
  repo's nodes only, never un-tags a path, and leaves a node that already carries an ORIGIN
  (a contract operation, a region anchor) as it is. A node with no POSITION (a package
  dependency, a queue tag) is never matched.

  A module under a declared path is also a test module for the TESTS pass: it pairs with the
  app module of the same stem (`qa/flows/cart.ts` -> `src/cart.ts`; a test affix is stripped
  first, `qa/cart.spec.ts` -> `src/cart.ts`), EVIDENCE rule `declared_name`, and its root
  functions get function-level TESTS edges to what they call there. It never pairs another
  test or declared file of its stem, and no test pairs it as the code under test. A layout
  merge (`glia merge`) does not read the overlay, so it pairs declared tests by name only, like
  any other module. The build's `[tests] module TESTS edges: <n> (snake=<s> camel=<c>
  declared=<d>)` line counts these edges as `declared`.

A build whose overlay declares `tests` prints, per repo,
`[provenance] declared tests repo=<label> patterns=<n> tagged=<d>`, where `tagged` counts the
node entries only a declared pattern tagged (modules included); the
`[provenance] test_fixture=<n> (path=<p> qname=<q>) ...` total counts them too.

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
  other than 1 or 2 ignores the **whole file**. Version 1 is the 0.5.1 schema and loads exactly as
  it did; version 2 adds the `[[constraint]]` fields `rule`, `transitive`, `via` and `min_tier`
  (next section). A 0.5.1 glia ignores a version-2 file whole (`version = 2 is not supported`).
- A stanza that parses but breaks a rule is **dropped on its own**. The rest of the file
  still applies. `glia check` lists a dropped `[[constraint]]` / `[[component]]` /
  `[[layer]]`, and a whole file ignored while it holds one, under "Rules that check nothing"
  (above), with the loader's message and a fix. The rules:
  - an `[[edge]]` category that is not an edge category name, or is `DEFINES`, `CONTAINS`
    or `CO_CHANGES`
  - a `route_prefix` that does not start with `/` or contains whitespace
  - a wrapper whose argument layout does not fit its kind (a `data_entity` wrapper without
    `name_arg`, or with a `flavor` other than `sql` / `nosql` / `graph`)
  - an unknown constraint kind, or one missing its required fields; `kind = "component"` or
    `"layer"` in a `[[constraint]]` (the model is declared only through its own sections)
  - a `rule`, `transitive = true`, a non-empty `via` or a `min_tier` in a version-1 file
    (`` `rule` need `version = 2` (this file says version = 1) ``)
  - `transitive` on any kind but `forbid_edge`; `via` without `transitive = true`, or with an
    empty path, a path that leaves the repo (`..`) or one path listed twice
  - `rule` on any kind but `invariant`, or a blank one, or one over 16,384 characters
  - a `min_tier` other than `fact` / `derived` (`heuristic` included: a heuristic violation
    never fails CI), on an `allow`, or on an `invariant` without `rule`
  - an `allow` whose `from` / `to` is not a kept `[[component]]`, names one component twice,
    or sets `scope`
  - a component with no paths, an empty path, a path that leaves the repo (`..`), or a path
    an earlier component already claims (exact-equal paths only; nested paths are legal)
  - a layer with no components, or naming a component that is not declared or already sits
    in another layer
  - a duplicate id, or a duplicate component or layer name
  - a constant name outside `[A-Za-z_][A-Za-z0-9_.]*`
  - an invalid (or empty) `[walk] skip` or `tests` pattern, which drops that pattern only
  - a project path that is the repo root or leaves it

## Rule invariants, transitive forbid and min_tier (version 2)

`version = 2` adds four `[[constraint]]` fields. Everything else in the file reads as in version 1,
and a version-1 file keeps loading unchanged; a version-1 file that sets one of the four loses that
stanza only, with an error naming the fields.

```toml
version = 2

[[constraint]]
id = "web-not-into-api"
kind = "forbid_edge"
from = "web"
to = "services/api"
transitive = true                         # a path through any third scope counts, not only a direct edge
via = ["services/api/public"]             # facades: a path may enter `to` here
categories = ["CALLS", "HTTP_CALLS"]
min_tier = "fact"                         # fact | derived (default derived)

[[constraint]]
id = "routes-are-handled"
kind = "invariant"
scope = "services/api"
text = "every route has a handler"
rule = 'violation(r) :- unhandled_route(r).'   # Datalog over the graph and the prelude
min_tier = "derived"
```

- `rule` (on an `invariant`; `text` stays required as the statement a person reads): Datalog text
  whose `violation/N` rows are the rule's violations. It is evaluated at `glia check` time over
  the graph plus the predicate prelude (`docs/rules.md`), never at load: the loader checks only
  that it is a non-blank string of at most 16,384 characters, so a CONSTRAINT cell never depends
  on the rules parser. A name the rule uses that this glia does not have (an unknown kind,
  category or predicate) or a syntax error is a rule error at check time: `glia check` lists it
  in its errors and exits 2, and `glia gaps` reports a `rotted_rule` row. An invariant without
  `rule` is a statement only: listed as unchecked.
- `transitive` (on a `forbid_edge`): a path from `from` to `to` through any third scope counts.
  The first node of `to` on a path is the hit. `via` names facade scopes (paths or project
  labels, resolved like `from` / `to`) the walk never passes through, so a path that enters `to`
  through a facade is allowed. `IMPORTS` chains over the module import graph, separately from the
  symbol categories, unless the rule names both `IMPORTS` and `CALLS`. One violation per hit,
  with its shortest best-tier path as the witness.
- `min_tier` (on a `forbid_edge`, a `no_cycle`, or an `invariant` with `rule`): `fact` or
  `derived`, default `derived`. A violation whose tier is weaker is reported and does not fail
  CI. A `heuristic` violation never fails CI under any `min_tier` (0.5.2 Q2), so `heuristic` is
  not a value.

A worked example. `web/` is a TypeScript app and `services/api/` a Python service whose
`services/api/public/` package is its published surface. Under the first stanza above, a path from
`web` into `services/api/public/` is allowed, and the walk never continues past the facade. A path
from `web` whose first node under `services/api/` is anywhere else - an `HTTP_CALLS` hop straight
into a handler in `services/api/internal/`, or a chain of calls through a third scope such as
`shared/` - is one violation per such node, the path its witness. The `HTTP_CALLS` hop is
resolver-paired (DERIVED), so under `min_tier = "fact"` that violation is listed and leaves the exit
code at 0; a path of `CALLS` edges the parsers bound (FACT) fails CI.

Storage. The fields go on the rule's CONSTRAINT entry only when set: `"transitive":true`,
`"via":[resolved]` beside `"via_raw":[as written]`, `"rule"` and `"min_tier"`. A version-1
stanza's entry is byte-identical to what 0.5.1 stored.

Rule text is declared only in this file (0.5.2 Q9). The cell API (`glia cell set`, `glia_py`'s
`set_cell` / `write_cell`, a hand-edited `.glia/cells.jsonl` row) refuses a CONSTRAINT entry that
carries a `rule` key, whatever its `source` says: `` `rule` (Datalog text) is declared only in
.glia/overlay.toml [[constraint]] (0.5.2 Q9); the cell API cannot write one ``. A rule a model
proposes goes through the overlay file like any other stanza, written by `glia overlay accept`
(section "Rules through the loop").

`glia check` judges all three: a transitive `forbid_edge` as `forbid_path` violations (DB.3b), an
`invariant` with `rule` through the rules layer (DB.3a), each violation blocking only at or above
its `min_tier`. Only an `invariant` without `rule` is listed as unchecked (the `unchecked=` count
of its `[check]` line); `[check] gate blocking=<b> advisory=<a>` counts the gate.

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

glia never calls a model. Three commands run the loop that writes this file, and the model step
between them (reading the code, writing stanzas) is an agent's or a person's. The `glia-overlay`
skill (`skills/glia-overlay/SKILL.md`; copy it to `~/.claude/skills/glia-overlay/`) gives an agent
the loop's commands and its rules.

1. **Gaps.** `glia gaps <repo>` lists the blind spots: unpaired endpoints and routes, `<unresolved>`
   endpoint sinks, tag-only queues, dead-flagged symbols, and overlay rules that no longer
   bind or that this glia can no longer read. Each row suggests a section to fill, and carries an `id` (`gap:<16 hex>`) that stays
   the same across rebuilds: it is keyed by the node, stanza or sidecar row, never by a line
   or an ordinal, so a stanza can name the gap it targets and a re-run shows it gone.
2. **Propose.** `glia overlay propose <repo>` is the work list: the rows a stanza could close
   (every category but `wrapped_sink` unless `--category` narrows it), each with the source lines
   around it (`--snippet-lines`, default 3) and, on a `suspected_edge` row, a paste-ready `[[edge]]`
   draft. Its `guide` points each `suggest` value at its section of this file. It writes nothing.
3. **Candidate.** The model step writes a candidate file holding only `[constants]`,
   `[entrypoints]`, `[[route_prefix]]`, `[[wrapper]]`, `[[edge]]` and `[[constraint]]` rules
   (section "Rules through the loop", below), plus an optional `version`, each stanza under
   `# gap: <id>` comment lines naming the rows it targets (above a constant's key, or above a
   pattern's item in a multi-line `qnames` array). A gap line holds ids only. The links are
   comments, so a candidate is plain overlay text, and it is all-valid or refused. A candidate with
   no `version` is checked as version 2. Keep it outside `.glia/`, whose files are fingerprinted
   build inputs.
4. **Try.** `glia overlay try <repo> --candidate <file>` builds the tree with the overlay as it is
   (base), with every candidate stanza merged in (with) and, per graph stanza, with every stanza
   but that one. It reports each node kind, edge category and gap category that moved, and per stanza
   (`wrapper#1`, `edge#2`, `constants.GATEWAY`, `entrypoints#1`, `constraint#1`) its marginal effect (with, minus
   the build without it), the linked gaps it `closes` (closed by it alone: back when it is left
   out; the report's `closed` also counts gaps several stanzas close together) and a verdict. A
   `[[constraint]]` stanza is judged by its rule on with, and a declared rule's violations moving
   is part of the verdict ("Judging a rule", below). An
   overlay never changes a parse, so every build reuses the cached parses; `builds` counts the
   distinct overlay texts, at most 2 + the graph stanzas. `--no-leave-one-out` builds base and with
   only: the totals and the verdict, no per-stanza rows. It writes only the parse cache.
5. **Accept.** `glia overlay accept <repo> --candidate <file> --only <stanza>` (repeatable; default
   every stanza) is the only writer of `.glia/overlay.toml`. It merges the chosen stanzas with their
   `# gap:` lines and keeps the file's own comments and layout, refuses any result the loader would
   not load as validated, and writes atomically. A constant or pattern the file already holds, or a
   rule with the same id and the same body, is a duplicate and writes nothing. A rule whose id the
   file already declares with another body is refused, and so is a rule `glia check` could not
   evaluate. `--dry-run` prints the diff and writes nothing. Exit 0 when written (or a dry run, or
   nothing changed), 1 when the result would not validate (nothing written: a constant pinned to
   another value and a colliding rule id included), 2 on a usage or build error.
6. **Prune rot.** A rule that `gaps` reports as an `orphaned_rule` (it binds nothing: the code was
   renamed or deleted), a `redundant_rule` (the extractor now emits the same edge) or a
   `rotted_rule` (a `[[constraint]]` whose `rule` this glia can no longer read: a syntax error, or
   a node kind, edge category or predicate it does not have; `detail` quotes the first rule error
   with its line and column in the rule text) is removed by its gap id:
   `glia overlay accept <repo> --remove <gap id>`. The row's stanza is found by its
   identity (an `[[edge]]`'s from / to / category, a `[[constraint]]` / `[[decision]]` / `[[note]]`
   id, an `[entrypoints]` pattern), never by a line, and every stanza with that identity goes.
   `--candidate` and `--remove` combine, so a renamed rule is replaced in one write. A
   `[[wrapper]]`, `[[route_prefix]]` or constant is never a rule row: a person removes one that
   stopped doing anything.

The verdict. `try` (per stanza and for the whole candidate) and `glia gaps <repo> --overlay-delta`
(for the whole file) judge by one rule. The overlay *improved* the graph when some gap category
fell, or some node kind or edge category other than `DEFINES` / `CONTAINS` rose; it *regressed* it
when some gap category rose. `keep` = improved and not regressed, `review` = both (a person
decides), `drop` = not improved. A stanza that closes a gap it links, and raises no gap category, is
a `keep`. A candidate with rules is judged by both parts: this graph rule, and its rules checked
by `glia check` ("Judging a rule", below). Accept only what the verdict keeps. The verdict
measures; it cannot tell whether an added edge is right, which is why a stanza is written from the
code it cites.

`glia gaps <repo> --overlay-delta` builds without and then with the overlay, counts each build
whole (nodes by kind, edges by category, gaps by category for the categories that need no repo
root) and prints `[overlay] N rules, +M edges, orphans K→J, gaps G0→G1, verdict=<keep|review|drop>`.
`[entrypoints]` is user config that `--no-overlay` still applies, so this delta does not see it;
`try` does.

Each step prints one stderr line: `[overlay] propose repo=<label> rows=<n> ...`,
`[overlay] candidate file=<path> stanzas=<n> (route_prefix=<a> wrapper=<b> edge=<c> constants=<d> entrypoints=<e> constraint=<k>) gap_links=<g> errors=<x>`,
`[overlay] try repo=<label> stanzas=<n> builds=<b> verdicts keep=<k> review=<r> drop=<d> ...` and
`[overlay] accept repo=<label> added=<a> (route_prefix=<p> wrapper=<w> edge=<e> constants=<c> entrypoints=<n> constraint=<k>) removed=<r> duplicates=<u> ... dry_run=<bool> ...`.

On every build that finds the file, stderr carries
`[overlay] loaded .glia/overlay.toml repo=<label> version=1 (walk=N project=N entrypoints=N constants=N route_prefix=N wrapper=N edge=N constraint=N decision=N note=N component=N layer=N) errors=E`.
The counts are skip plus tests patterns for `walk`, qname patterns for `entrypoints`, keys for
`constants`, and stanzas for every other section.

From Python (`glia_py`, CK.1): `overlay_propose(repo_paths, categories=None, top_k=20,
snippet_lines=3)`, `overlay_try(repo_paths, candidate, leave_one_out=True)` and
`overlay_accept(repo_path, candidate=None, only=None, remove=None, dry_run=False)` run the same
three steps and return the `--json` objects as dicts. The first of `repo_paths` is the primary
repo, whose `.glia/overlay.toml` the loop reads; the rest merge in, as `--with` does. `candidate`
is the candidate's TOML text, not a path. A refusal raises `ValueError` with the engine's message.
The markers end `surface=py`, and the candidate marker says `file=-`. They write what the
commands write: `overlay_propose` nothing, `overlay_try` only the primary repo's parse cache (one
repo), `overlay_accept` only `.glia/overlay.toml`.

## Rules through the loop

A rule that an agent proposes reaches `.glia/overlay.toml` the way any other stanza does: a
candidate, `glia overlay try`, then `glia overlay accept`. A candidate may hold these
`[[constraint]]` kinds, each with an optional `min_tier`:

- `forbid_edge`, with `transitive` and `via` if it needs them;
- `no_cycle`;
- `invariant` with a `rule`.

Each rule stanza has the handle `constraint#<n>` (its position among the candidate's rules), so
`--only constraint#2` picks one. Its `id` is its identity in the file.

Two kinds are refused in a candidate, and both still load from a file a person writes by hand:

- `kind = "allow"`: it waives a reflexion divergence, and that is a person's decision. (It also
  names `[[component]]`s that a candidate cannot declare.)
- An `invariant` without `rule`: `glia check` never evaluates prose, so `try` has nothing to judge.

`[[component]]`, `[[layer]]`, `[[decision]]` and `[[note]]` are never candidate sections.

Version. The file moves to `version = 2` only when a written rule sets `rule`, `transitive`, `via`
or `min_tier`. The move happens in place: `version = 1 # schema` becomes `version = 2 # schema`, and
nothing else in the file moves. A file with no `version` key never gets one, and accept never
lowers a version. A new file starts from `version = 2` when its first rule needs it, and from
`version = 1` otherwise, so a 0.5.1 glia can still read it. The move is refused when version 2
would also load a stanza the old version drops (typically a hand-written `rule` in a version-1
file): `the merged .glia/overlay.toml would also load constraint=1 stanza(s) that version = 1 drops
(...)`, and `accept` exits 1 with nothing written. Raise the version by hand, check those stanzas,
then accept again.

Ids. Give each rule a fresh id. A rule whose id and body match one already in the file is a
duplicate. Bodies are compared by value, so formatting, key order and comments do not count. A
rule with that id and another body is refused (`constraint "<id>" is already declared in
.glia/overlay.toml with another body`, exit 1): a candidate adds rules and never edits one.

Readable rules. Accept refuses a rule that `glia check` cannot read: one that does not validate
against the code schema and the prelude (`rule "<id>" does not validate at <line>:<column>:
<message>`, the check `rotted_rule` uses), or one that defines no `violation` predicate. Nothing
is written. `try` builds such a stanza anyway and judges it. A rule whose scope holds no node is
not refused: accept never builds, the rule parses, and `glia check` then lists it under "Rules
that check nothing" (`try` judges it `checks_nothing`).

Origin. An unmarked stanza is `origin = "llm"`, the loader's default, so a rule a model wrote is
marked as inference without any extra field.

glia drafts no rule. `propose` lists no rule rows: a rule states intent, and the graph holds only
what the code is. A rule glia could mine from structure would only restate the code as law, so the
evidence of intent stays prose (an unchecked invariant, an ADR or `[[decision]]`, the person's
request), and the model step translates it.

### Judging a rule

`try` runs `glia check` once on every build whose graph declares a rule (the file's, the
candidate's, a cell-API rule) and keeps each violation as a hit keyed by its rule id, its rule
kind and its identity within the rule: a `forbid_edge` or a divergence by evidence row (`<from>
-[<category>]-> <to>`, plus one `<n> row(s) past check::MAX_EVIDENCE` hit for the rows check
cuts), a `rule` or `forbid_path` violation by its subject, a `no_cycle` by its witness's qnames
and member count. Two builds compare by those keys, never by a count, so a stanza that hides one
violation and adds another moves two hits. A hit takes its row's tier and location, and blocks
when `glia check` would exit 1 on it (its tier at or above the rule's `min_tier`). A build that
declares no rule is not checked, and a rule-free try prints and serialises exactly as it did
before rules.

A `[[constraint]]` stanza only adds a rule, so it costs no build: it is judged on with alone, and
its status is the first that applies:

| status | when |
|---|---|
| `duplicate` | the overlay as it is already declares its id (the same body, or a cell-API rule) |
| `error` | `glia check` reports an error for it on with (its message, on one line) |
| `orphaned` | with declares no rule of its id: an `anchor` that names no node, or a scope with no PROJECT at or above it and no MODULE under it; `glia check` lists it under "Rules that check nothing" |
| `checks_nothing` | with's check lists it as one that checks nothing: a `from`, `to`, `via` or no_cycle `scope` it names holds no located node (its `reason` and `fix` are `glia check`'s, which lists it under "Rules that check nothing" and fails CI on it only with `--strict`) |
| `violated` | with holds a hit of it |
| `holds` | otherwise |

Its empty scopes are the scopes its `rule`'s positive `in_scope(_, "s")` and `under(_, "p")`
literals name that hold no node on with: such a rule may hold vacuously. A negated literal never
counts (`!in_scope(f, "legacy")` holds for every node).

| verdict | a `[[constraint]]` stanza |
|---|---|
| `keep` | holds with no empty scope: true of the code as it is, and `glia check` keeps it true |
| `review` | holds on an empty scope (a misspelt scope, or one meant to stay empty), checks nothing (a misspelt scope, or one meant to fill later), or violated (the rule or the code is wrong; a blocking violation would make `glia check` exit 1, a heuristic one never does) |
| `drop` | error (`glia check` would exit 2), orphaned (it checks nothing; `glia check` lists it) or duplicate (adds nothing) |

A graph stanza keeps its graph verdict, except that `keep` becomes `review` when the build without
it and the build with it differ in a declared rule's hits, or a rule errors only with it: an
overlay stanza that adds or hides a rule's violation needs a person, as an `allow` does.

The candidate's verdict has two parts. The graph part is the graph rule on base and with, `keep`
demoted to `review` when base -> with moves a hit (or an error) of a rule the candidate does not
newly declare. The rule part is `keep` when every `[[constraint]]` stanza keeps, `drop` when every
one drops, and `review` otherwise; a candidate with no `[[constraint]]` stanza has none. The
verdict is the graph part when there is no rule part; otherwise `review` when either part is
review, `keep` when either keeps, and `drop` when both drop. A rule-only candidate's graph part is
`drop` (nothing in the graph moved), so its verdict is its rule part. The rule part needs only
base and with, so `--no-leave-one-out` judges it too, and `builds` stays at most 2 + the graph
stanzas.

The text form shows the rule part when there is one. The stanza table gains a `rules` column (a
rule stanza's status, `<n> violation(s) (<tiers>; <b> blocking)` when violated; a graph stanza's
moved hits as `+<added> -<resolved> (<b> blocking)`), and a `rules (glia check on the build with
every stanza):` block under it gives each such stanza's verdict and reason with up to five located
hits (`+` on with, `-` gone with it; `--json` lists up to 20). The totals gain a `rules:` line (the
statuses, then `violations +<a> -<r> (<b> blocking)` from base to with and the rules that start to
error), the one place `--no-leave-one-out` names the statuses, and the verdict line reads
`verdict: <v> - graph part <g> (<why>), rule part <r>`. In `--json` and from Python the report
gains `rules` (`{delta, statuses, graph_verdict, rule_verdict}`), a rule stanza `rule` (`{rule_id,
status, violations, tiers, blocking, hits}`, plus `error`, `reason` / `fix` (a `checks_nothing`
rule: `glia check`'s) and `empty_scopes` when set) and a graph
stanza that moves hits `rules` (`{added, added_total, resolved, resolved_total, blocking,
errors}`).

When a try has a rule part, a second stderr line follows `[overlay] try ...`:
`[overlay] try-rules repo=<label> rules=<R> (holds=<h> violated=<v> error=<e> orphaned=<o> duplicate=<u> checks_nothing=<n>) empty_scopes=<s> violations +<a> -<r> blocking=<b> errors=<n> surface=<cli|py|engine>`,
` checks_nothing=<n>` only when `n` > 0, `R` the candidate's `[[constraint]]` stanzas by status,
`s` those that hold on an empty scope, and the rest the base -> with hits. Each rule-bearing build
also prints check's own `[check]` line (with ` unanchored=<n>` and a `[check] declared not
checked:` line when a rule checks nothing; a try reads no overlay file, so those count only the
empty scopes).

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

- a definition binds the innermost glia function, method, type, attribute or state variable
  whose span holds its line and whose name is exactly the identifier the import read there; a
  definition that binds nothing is counted `unbound`, and a symbol whose definitions bind two
  different nodes is counted `ambiguous` and never used.
- references to a bound definition become `CALLS` (a call of a function or method, rule `call_site`)
  or `USES` (`callable_ref` for a function or method used as a value, else `reference`,
  `reference_write` on a write), emitter `scip:<tool>` (the indexer's name, lowercased, other
  characters than `a-z0-9_-` as `_`), tier FACT (CE.1d), from the innermost function or method
  holding the reference, else the innermost type, else the file's module. Import references add
  nothing (the parser's `IMPORTS` are already FACT), and a node's reference to itself adds nothing
  unless it is a call (recursion, kept; the rest counted `self_refs`). One edge per (from, to,
  category), located at its first reference. An edge glia already has between the same nodes keeps
  its category: the same category counts as `confirmed`, and when glia joins the two by any other
  category (`DEFINES` and `INHERITS_FROM` included, and a heritage edge the index's relationship
  just added) its classification stands and nothing is added (`category_differs`).
- the index's `is_implementation` relationships between two bound definitions add the heritage
  edges glia lacks, before the references are read (so a class header's reference to its base adds
  no `USES` beside the heritage edge): `IMPLEMENTS` for a method implementing a method, or a class,
  struct or enum implementing an interface; `INHERITS_FROM` for any other pair of types (an
  interface extending an interface included, glia's own shape); rule `implementation`, located at
  the implementing definition. A relationship between other kinds (an attribute implementing an
  interface property) adds nothing and is counted `other_kinds`; one whose pair glia already joins
  by the other heritage category counts `category_differs`.
- a glia edge the index confirms (a reference or relationship on the same nodes and category) keeps
  its evidence when it is already a located FACT (`confirmed_fact`), and when its emitter is not
  the parser, extractor or graph stage (`confirmed_other`: a merge recomputes resolver and pass
  edges by emitter, so a re-stamped one would come back twice). A name-only, below-Strong inferred
  or unlocated one (tier HEURISTIC or DERIVED) is re-stamped `scip:<tool>` with rule
  `confirms:<old emitter>[/<old rule>]`, at its own site when the index binds its target there,
  else at the index's, confidence Strong, tier FACT (`upgraded`); `why` notes `confirmed by a SCIP
  index (<tool>) at <file>:<line>; first emitted by <old emitter> (<old rule>)` (CE.1e).
- a heuristic `CALLS` / `USES` / `INHERITS_FROM` / `IMPLEMENTS` edge whose site the index binds to
  another node, and not to the edge's target (which the index must know), is counted
  `contradicted` and printed, never removed or changed (CE.1e).

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
that reads the snapshot prints `[scip] ingest repo=<label> tool=<tool> documents=N stale=N defs=N
bound=N unbound=N ambiguous=N refs=N imports=N unowned=N added=N (calls=N uses=N) confirmed=N
category_differs=N self_refs=N` and `[scip] confirm repo=<label> upgraded=N confirmed_fact=N
confirmed_other=N relationships=N added_implements=N added_inherits=N contradicted=N
other_kinds=N`, then at most 20 `[scip] contradicts <from> -[<CATEGORY> <emitter>/<rule>]-> <to>
at <file>:<line>; index says <qname>` lines, sorted by file and line.
