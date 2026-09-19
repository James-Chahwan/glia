# Coverage — 16 languages x 30 mechanisms

**Generated. Do not hand-edit the block below.** Run

```
python3 bench/substrate-gap/matrix.py --emit
```

before committing ANY change to extraction, resolution or the matrix
vocabulary, and commit the regenerated `COVERAGE.md` + `results-latest.json` in
the same commit. `python3 bench/substrate-gap/matrix.py --check` regenerates
both in memory, prints every cell whose level moved, and exits 1 on drift. It is
a pre-commit discipline, not a CI job: the only workflow in this repo
(.github/workflows/wheels-py.yml) builds wheels.

This file supersedes the hand-made table in
`dev-notes/review-2026-09-15-coverage-and-issues.md` section 3. That table was a
human reading a fixture and writing a glyph; every glyph here is DERIVED from
four assertions grade.py already grades (extract / literal / route / forbid) —
see `matrix.py`'s module docstring for the level rule.

`?` is not a weaker `·`. `·` means a fixture exists for the cell and nothing of
the right kind was emitted — a MEASURED blind spot. `?` means no fixture claims
the cell, so no claim is made in either direction. The review's blanks could not
tell those apart, which is what made them unfalsifiable.

Prose outside the generated block below is hand-maintained and survives
regeneration. (This paragraph deliberately does not quote the marker strings:
the splice partitions on the FIRST marker it finds, so a literal marker inside
the prose would make the preamble eat itself.)

<!-- BEGIN generated: matrix.py --emit -->
Engine `0.4.18` · vocabulary digest `916e9fb659ff` · schema 1

Columns, left to right (the review's own abbreviations):

- `http_cl` — **http_client** (http)
- `http_sr` — **http_server** (http)
- `kafka` — **kafka** (messaging)
- `amqp` — **amqp** (messaging)
- `sqs/sns` — **sqs_sns** (messaging)
- `pubsub` — **pubsub** (messaging)
- `azure_s` — **azure_sb** (messaging)
- `nats` — **nats** (messaging)
- `redis` — **redis** (messaging)
- `mqtt` — **mqtt** (messaging)
- `taskq` — **taskq** (messaging)
- `grpc` — **grpc** (rpc)
- `graphql` — **graphql** (rpc)
- `ws` — **ws** (streaming)
- `eventbu` — **eventbus** (messaging)
- `db` — **db** (data)
- `migr` — **migrations** (data)
- `config` — **config** (config)
- `secrets` — **secrets** (config)
- `flags` — **flags** (config)
- `cron` — **cron** (schedule)
- `cli_def` — **cli_def** (cli)
- `cli_inv` — **cli_inv** (cli)
- `calls` — **calls** (intra)
- `imports` — **imports** (intra)
- `injects` — **injects** (intra)
- `impl` — **impl** (intra)
- `tests` — **tests** (intra)
- `service` — **service** (topology)
- `subproj` — **subproject** (topology)

```
LEGEND ● full  ◐ partial  · none (fixture exists, nothing emitted)  ? unknown (no fixture)  ! error
           http_cl http_sr   kafka    amqp sqs/sns  pubsub azure_s    nats   redis    mqtt   taskq    grpc graphql      ws eventbu      db    migr  config secrets   flags    cron cli_def cli_inv   calls imports injects    impl   tests service subproj
python           ●       ●       ●       ◐       ●       ◐       ●       ●       ●       ●       ·       ●       ◐       ●       ?       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ?
go               ●       ●       ◐       ·       ·       ?       ?       ●       ·       ●       ?       ●       ?       ●       ?       ●       ?       ?       ?       ●       ●       ●       ●       ●       ●       ●       ●       ◐       ●       ●
typescript       ●       ●       ●       ◐       ?       ◐       ?       ●       ●       ●       ◐       ●       ●       ◐       ●       ?       ?       ?       ●       ●       ?       ●       ?       ●       ●       ●       ●       ●       ●       ●
java             ●       ●       ●       ●       ◐       ?       ?       ?       ·       ?       ?       ●       ·       ●       ●       ●       ●       ?       ?       ?       ●       ●       ?       ●       ●       ●       ●       ◐       ●       ●
csharp           ●       ●       ●       ◐       ?       ?       ●       ?       ●       ?       ?       ●       ?       ●       ?       ◐       ?       ?       ·       ?       ●       ●       ?       ●       ●       ●       ●       ◐       ●       ?
ruby             ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ●       ?       ●       ●       ?       ?       ?       ●       ●       ?       ●       ●       ◐       ?       ◐       ●       ?
php              ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ●       ?       ●       ?       ◐       ●       ?
swift            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?
c_cpp            ·       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
scala            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ●       ●       ?       ●       ?
clojure          ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
dart             ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ◐       ●       ?       ●       ?
elixir           ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?       ●       ?       ?       ●       ●       ?       ?       ?       ●       ?
rust             ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ●       ●       ?       ◐       ◐       ●       ?
solidity         ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ●       ?       ?       ?
terraform        ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?

PER-MECHANISM  across 16 languages:
  http_client  ● 13  ◐ 0   · 1   ? 2   ! 0
  http_server  ● 9   ◐ 4   · 0   ? 3   ! 0
  kafka        ● 4   ◐ 1   · 0   ? 11  ! 0
  amqp         ● 1   ◐ 3   · 1   ? 11  ! 0
  sqs_sns      ● 1   ◐ 1   · 1   ? 13  ! 0
  pubsub       ● 0   ◐ 2   · 0   ? 14  ! 0
  azure_sb     ● 2   ◐ 0   · 0   ? 14  ! 0
  nats         ● 3   ◐ 0   · 0   ? 13  ! 0
  redis        ● 3   ◐ 0   · 2   ? 11  ! 0
  mqtt         ● 3   ◐ 0   · 0   ? 13  ! 0
  taskq        ● 1   ◐ 1   · 1   ? 13  ! 0
  grpc         ● 5   ◐ 0   · 0   ? 11  ! 0
  graphql      ● 1   ◐ 1   · 1   ? 13  ! 0
  ws           ● 6   ◐ 1   · 0   ? 9   ! 0
  eventbus     ● 3   ◐ 0   · 0   ? 13  ! 0
  db           ● 4   ◐ 1   · 0   ? 11  ! 0
  migrations   ● 3   ◐ 0   · 0   ? 13  ! 0
  config       ● 1   ◐ 0   · 0   ? 15  ! 0
  secrets      ● 2   ◐ 0   · 1   ? 13  ! 0
  flags        ● 3   ◐ 0   · 0   ? 13  ! 0
  cron         ● 7   ◐ 0   · 0   ? 9   ! 0
  cli_def      ● 8   ◐ 0   · 0   ? 8   ! 0
  cli_inv      ● 2   ◐ 0   · 0   ? 14  ! 0
  calls        ● 15  ◐ 0   · 0   ? 1   ! 0
  imports      ● 13  ◐ 0   · 0   ? 3   ! 0
  injects      ● 7   ◐ 2   · 0   ? 7   ! 0
  impl         ● 8   ◐ 1   · 0   ? 7   ! 0
  tests        ● 2   ◐ 6   · 0   ? 8   ! 0
  service      ● 11  ◐ 0   · 0   ? 5   ! 0
  subproject   ● 3   ◐ 0   · 0   ? 13  ! 0

PER-LANGUAGE  across 30 mechanisms:
  python       ● 24  ◐ 3   · 1   ? 2   ! 0
  go           ● 17  ◐ 2   · 3   ? 8   ! 0
  typescript   ● 19  ◐ 4   · 0   ? 7   ! 0
  java         ● 17  ◐ 2   · 2   ? 9   ! 0
  csharp       ● 14  ◐ 3   · 1   ? 12  ! 0
  ruby         ● 11  ◐ 2   · 0   ? 17  ! 0
  php          ● 7   ◐ 1   · 0   ? 22  ! 0
  swift        ● 2   ◐ 1   · 0   ? 27  ! 0
  c_cpp        ● 2   ◐ 0   · 1   ? 27  ! 0
  scala        ● 6   ◐ 1   · 0   ? 23  ! 0
  clojure      ● 3   ◐ 1   · 0   ? 26  ! 0
  dart         ● 5   ◐ 2   · 0   ? 23  ! 0
  elixir       ● 7   ◐ 0   · 0   ? 23  ! 0
  rust         ● 6   ◐ 2   · 0   ? 22  ! 0
  solidity     ● 4   ◐ 0   · 0   ? 26  ! 0
  terraform    ● 0   ◐ 0   · 0   ? 30  ! 0
```

COVERAGE OF THE COVERAGE: 176/480 cells have a fixture (36.7%) — 144 full, 24 partial, 8 none, 304 unknown, 0 error.

`legacy_only` (fixtures with no `cells`, graded by run.py only): 155

## Cells routed via an alternative mechanism

A `●` here does not mean the intended path fired — it means SOME path did. These cells resolved through their fallback registry.

| cell | level | via | primary |
|---|---|---|---|
| `csharp/azure_sb` | ● full | `queue` | `eventbus` |
| `csharp/redis` | ● full | `queue` | `eventbus` |
| `go/mqtt` | ● full | `queue` | `eventbus` |
| `java/sqs_sns` | ◐ partial | `queue` | `eventbus` |
| `python/azure_sb` | ● full | `queue` | `eventbus` |
| `python/mqtt` | ● full | `queue` | `eventbus` |
| `python/pubsub` | ◐ partial | `queue` | `eventbus` |
| `python/redis` | ● full | `queue` | `eventbus` |
| `python/sqs_sns` | ● full | `queue` | `eventbus` |
| `typescript/mqtt` | ● full | `queue` | `eventbus` |
| `typescript/pubsub` | ◐ partial | `queue` | `eventbus` |
| `typescript/redis` | ● full | `queue` | `eventbus` |

## Cell errors

_None._
<!-- END generated -->
