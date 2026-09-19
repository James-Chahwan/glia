# Authoring one matrix cell

The coverage matrix is 16 languages × 30 mechanisms = **480 cells**. That is far
past hand-authoring, and six corpus packets follow this one. This file is the
per-cell recipe, so the cost per fixture is low and — more importantly —
**uniform**: six parallel authors should produce six fixtures that make the same
choices, not six conventions.

It complements [`README.md`](README.md), which owns the canonical `key.json`
vocabulary and the run/rebuild commands. Read that first; this file is only the
*procedure* and the *measured gotchas*.

> **Where cells live.** `matrix/<language>/<mechanism>/` — see
> [`matrix/README.md`](matrix/README.md) for why that tree is separate from
> `fixtures/`.

---

## The six steps

### 1. Scaffold

```bash
python3 scaffold.py <language> <mechanism>
```

It reads `matrix_vocab.py` and stamps `matrix/<lang>/<mech>/` with a `key.json`
skeleton and one source stub per dir. Four decisions the vocabulary already holds
are made for you, so you never re-derive them:

| decision | comes from | why it is not yours to guess |
|---|---|---|
| how many dirs | `cross_repo` | `grade.py` calls `generate()` for one dir and `generate_many()` for several, and **every cross-graph resolver runs in both**. The stack resolvers (HTTP, gRPC, RPC, queue, GraphQL, WebSocket, EventBus, CLI) pair inside ONE repo, so a single-dir fixture shows their edges. Only the pairwise `SHARES_*` resolvers require two separate `RepoId`s (`matrix_vocab.REPO_PAIRWISE_CATEGORIES`). Two dirs stay the default for cross-service columns because (1) it is the realistic client/server shape, (2) a single `RepoId` merges same-qname markers from both sides into one node, changing edge counts, and (3) HTTP host narrowing is keyed per repo, so a single-dir build can over-pair (`xstack-host-pairing`: 4 `HTTP_CALLS` vs 2). `cross_repo: true` ⇒ `dirs: ["client","server"]`; `false` ⇒ `dirs: ["."]`. **Consequence: a single-dir fixture is NOT a negative control for a cross-service edge — use a `forbid`.** Measured table below. |
| which node kinds | `kinds[0]` | the *intended* extraction path. Later groups are the alternative registries (see **via**, below). |
| which edge category | `categories[0]` | the routing proof. An ANCHOR mechanism (matrix_vocab `anchor: True`, today only subproject) has no routing vocabulary; its cell is graded on the anchor node, the literal and the forbid guards. The scaffold gives it `expect_edges: []` and a `max_nodes: 1` duplicate-anchor forbid instead. |
| which literal | `literal` | the identifying string that must survive into the node name/qname. Extraction without it is `partial`, not `full`. |

**Single-dir vs two-dir, measured** (installed wheel, 2026-09-19: the same
fixture built with `generate_many(dirs)` and with `generate(fixture_root)`; the
first four rows are pinned by `test_matrix.py`
`test_single_dir_pairing_matches_the_vocabulary`):

| category | pairs single-dir? | example (two-dir → single-dir edge count) |
|---|---|---|
| `HTTP_CALLS` `GRPC_CALLS` `RPC_CALLS` | yes | `xstack-go-http`, `xcut-grpc-grpc_calls`, `xcut-trpc`: 1 → 1 |
| `QUEUE_FLOWS` `WS_CONNECTS` `GRAPHQL_CALLS` `EVENT_FLOWS` | yes | `xcut-queue-queue_flows`, `xcut-websocket-ws_connects`, `xcut-graphql-graphql_calls`, `xcut-eventbus-spring`: 1 → 1 |
| `CLI_INVOKES` | yes | `xcli-invokes`: 2 → 2 |
| `SHARES_*` (all seven) | **no** — same-repo pairs are skipped | `xcut-proto-shared` `SHARES_SCHEMA`, `xdata-source-shares` `SHARES_DATA_SOURCE`, `xiac-terraform-k8s` `SHARES_INFRA_REF`, `xpoly-data-entity` `SHARES_DATA_ENTITY`: 1 → 0 |
| stack edge, different count | yes, but not the same edges | `matrix/python/amqp`, `matrix/csharp/amqp` `QUEUE_FLOWS` 2 → 1 (same-qname merge); `xstack-host-pairing` `HTTP_CALLS` 2 → 4 (host narrowing is per repo) |

The scaffolder **refuses to overwrite** an existing cell without `--force`,
because `--force` destroys an authored key and its baseline.

### 2. Write the smallest source a real repo would contain

Budget: **≤ 2 files per dir, ≤ 20 lines per file**. Use the real library's
canonical import and call form. Two hard rules, both **measured on 2026-09-16
against the installed 0.4.18 wheel** — they are the difference between a corpus
that measures production and one that measures a flattering toy.

#### (a) Include the real import line — many needles are *gated* on it

Broad needles carry a framework signal list, and in a fixture this small the
import is the only place that word appears. Measured, identical apart from line 1:

```python
from kafka import Consumer          #  <-- the only occurrence of "kafka"
consumer = Consumer({})
consumer.subscribe(["orders"])
```
```
[QUEUE_CONSUMER] 'orders'   qname='queue_consumer:orders'      # extracted
```

Delete that import and **nothing at all is emitted**:

```
XX  QUEUE_CONSUMER 'orders'                                     # blind
```

A fixture that omits the import measures a cell as blind where production would
extract. That is not a conservative error — it is a false blind spot, and it
would send someone to fix an extractor that works.

#### (b) Use the library's canonical casing

The **gate** is case-*insensitive* (`queues.rs` lowercases a copy of the source
before testing signals — `using Confluent.Kafka;` passes a `["kafka"]` gate
today). The **needles are not**: C# `.Subscribe(` is not JS `.subscribe(`, and
Go's `nc.Publish` is not `nc.publish`. Lowercasing an API name to make a fixture
pass measures a language that does not exist.

### 3. Dump reality

```bash
python3 grade.py matrix/<lang>/<mech> --dump
```

This is the authoring loop, exactly as `README.md` recommends: write source, dump
reality, write the key against it. `--dump` prints every node and edge the graph
actually emitted, with `path=` — which step 5 needs.

**Dump the *unwritten* stub first, before you type any source.** The extractors
scan **text, not an AST**, so they do not skip comments — anything call-shaped in
a comment is extracted as if it were code. The scaffolder's banner is prose-only
for exactly this reason, but a bare vendor word cannot always be avoided: a
freshly-scaffolded `matrix/python/redis/` emits

```
[CACHE       ] 'redis'   qname='data_source:redis'   path=None
[ACCESSES_DATA ] 'consumer' -> 'redis'
```

purely from the word `redis` in `# FIXTURE: python/redis.`. That is your true
zero. Record it, and do not mistake it for extraction you earned. The same trap
bites the *source* you are about to write: a commented-out call still counts.

### 4. Write the key against the *intended* graph — and record the failing baseline

Write what the graph **should** emit, not what it does. **A new cell is supposed
to fail.** Record its `0.00` before writing any fix:

```bash
python3 grade.py matrix/<lang>/<mech> > /tmp/<cell>_baseline.txt
```

A fixture that passes the moment you write it proves nothing — it means either
the cell was already covered (a fine finding: report it, do not fix the number)
or the key was written against the dump instead of against the intent. `0.00 →
0.50` is a reportable half-fix; `run.py` prints a PARTIAL section for exactly
this.

### 5. Assert the literal — and mind the two identity gotchas

The literal assertion **is** an `expect_nodes` entry whose `name` is the literal:
the matcher is a case-folded substring over name **or** qname. There is no
`expect_literals` field; inventing one makes the fixture raise and vanish from
the matrix.

**Gotcha 1 — the ROUTE identity shape is not portable.** Measured across the
committed fixtures:

| framework | node name | qname |
|---|---|---|
| Go / chi | `GET /users` | same — one node per method since LB.11a (was `route:/users`, no method) |
| Express / Next / Nest / SvelteKit / Hapi / Bun (ts_routes) | `GET /users` | same — one node per method since LB.11b (was `route:/users`, name `/users`); `.all(`, `@All(`, Hapi `*`, a Pages Router default export and a Bun single-value route are `ANY` |
| Spring | `GET /users/{id}` | same |
| Rails | `GET /users/:id` | same |
| ASP.NET | `ANY api/users` | same — method present, **no leading slash** |

So assert the **path** against name\|qname via `expect_nodes`, and the **method**
separately via `expect_cells` on `ROUTE_METHOD` (verified satisfiable against
`fixtures/java-spring-http`). The scaffolder stamps that cell for you on
`http_server`.

**Gotcha 2 — some synthetic nodes carry no POSITION.** The cross-cutting
extractors mint some nodes without a span: `ENDPOINT` and the `ROUTE`s of the
bare-verb server parsers (Spring, Rails, ASP.NET, ... — located only through
their handler) dump `path=None`, while AST entities (`MODULE`, `FUNCTION`,
`CLASS`) carry one. `ROUTE` is no longer POSITION-less everywhere: Go routes
(LA.32a) and ts_routes routes (LB.11b, Express / Next / Nest / SvelteKit /
Hapi / Bun) carry a POSITION per registration, and the queue markers
(`QUEUE_PRODUCER` / `QUEUE_CONSUMER`) now dump the file of their call site.
`expect_cells` is optional and the scaffolder leaves it **empty** on purpose:
add a `POSITION` gate only where `--dump` shows a real path. A blanket
POSITION assertion would cap every messaging/http/rpc cell at partial
forever.

### 6. Forbid the phantoms the fixture provokes

A cell is not honest until its *precision* is pinned too. Write the smallest real
source, then look at the dump for nodes that should not be there.

**Worked example, measured.** A paho-mqtt subscriber:

```python
import paho.mqtt.client as mqtt

client = mqtt.Client()
client.connect("broker", 1883)
client.subscribe("sensors/temp")
```

dumps:

```
[GRAPHQL_OPERATION] 'client.subscribe'   qname='graphql_op:client.subscribe' path=None
[EVENT_HANDLER    ] 'sensors/temp'       qname='event_handle:sensors/temp'   path=None
```

`graphql.rs` lists `"client.subscribe("` among its operation patterns with no
language or library gate, so an MQTT file mints a phantom GraphQL operation.
Measured at 0.4.18; LA.26 gated the operation needles, so this forbid row now
guards that fix. The mqtt fixture therefore carries

```json
"forbid": [{"kind": "GRAPHQL_OPERATION", "name": "client.subscribe"}]
```

and the row stays as the regression guard. Note
also the `EVENT_HANDLER`: mqtt's vocabulary lists `EVENT_*` before `QUEUE_*`, so
`resolve_via` records that the eventbus path fired rather than grading it as if
the intended queue path had.

---

## Conventions

- **Naming.** The directory *is* the cell: `matrix/<language>/<mechanism>/`,
  spelled exactly as `matrix_vocab.py` spells them (`normalize_language` accepts
  `ts`/`js`/`c++`/`c#` and raises on anything that is not one of the 16 rows).
- **One fixture, one cell.** `cells` holds a single `"<lang>/<mech>"`. The
  exception is a genuinely cross-language fixture — a TypeScript client calling a
  Go server — which declares both cells and tags each assertion with the `cell`
  it belongs to. Do not use one fixture to cover two cells of the same language.
- **Size.** ≤ 2 files per dir, ≤ 20 lines per file. If a mechanism cannot be
  shown in that budget, that is worth reporting; it usually means the needle
  needs more context than a real call site provides.
- **Frozen vocabulary.** `framework`, `language`, `dirs`, `expect_nodes`,
  `expect_edges`, `expect_cells`, `forbid`, `materialize`, `mechanism`, `cells`,
  `note` — and nothing else. `grade.py` raises `ValueError` on any other
  top-level field, and a raising fixture drops out of the matrix entirely.
