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
Engine `0.4.18` · vocabulary digest `82fbaf1daf5c` · schema 1

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
python           ●       ●       ●       ◐       ●       ◐       ●       ●       ●       ●       ·       ●       ◐       ●       ?       ●       ·       ●       ●       ·       ?       ◐       ◐       ●       ●       ●       ●       ●       ●       ?
go               ●       ●       ◐       ·       ·       ?       ?       ●       ·       ●       ?       ●       ?       ●       ?       ●       ?       ?       ?       ·       ◐       ●       ●       ●       ●       ●       ?       ◐       ●       ●
typescript       ●       ●       ●       ◐       ?       ◐       ?       ●       ●       ●       ◐       ●       ●       ◐       ●       ?       ?       ?       ●       ·       ?       ?       ?       ●       ●       ●       ◐       ●       ●       ●
java             ●       ●       ●       ●       ◐       ?       ?       ?       ·       ?       ?       ●       ·       ●       ●       ●       ·       ?       ?       ?       ?       ●       ?       ●       ●       ●       ●       ◐       ●       ●
csharp           ●       ●       ●       ◐       ?       ?       ●       ?       ●       ?       ?       ●       ?       ●       ?       ?       ?       ?       ·       ?       ?       ●       ?       ●       ●       ●       ●       ◐       ●       ?
ruby             ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ●       ·       ?       ?       ?       ?       ●       ?       ●       ●       ◐       ?       ◐       ●       ?
php              ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ●       ?       ●       ?       ◐       ●       ?
swift            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?
c_cpp            ·       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
scala            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ●       ●       ?       ●       ?
clojure          ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
dart             ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ◐       ◐       ?       ●       ?
elixir           ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ●       ?
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
  ws           ● 4   ◐ 1   · 0   ? 11  ! 0
  eventbus     ● 3   ◐ 0   · 0   ? 13  ! 0
  db           ● 4   ◐ 0   · 0   ? 12  ! 0
  migrations   ● 0   ◐ 0   · 3   ? 13  ! 0
  config       ● 1   ◐ 0   · 0   ? 15  ! 0
  secrets      ● 2   ◐ 0   · 1   ? 13  ! 0
  flags        ● 0   ◐ 0   · 3   ? 13  ! 0
  cron         ● 0   ◐ 1   · 0   ? 15  ! 0
  cli_def      ● 6   ◐ 1   · 0   ? 9   ! 0
  cli_inv      ● 1   ◐ 1   · 0   ? 14  ! 0
  calls        ● 15  ◐ 0   · 0   ? 1   ! 0
  imports      ● 13  ◐ 0   · 0   ? 3   ! 0
  injects      ● 7   ◐ 2   · 0   ? 7   ! 0
  impl         ● 5   ◐ 3   · 0   ? 8   ! 0
  tests        ● 2   ◐ 6   · 0   ? 8   ! 0
  service      ● 11  ◐ 0   · 0   ? 5   ! 0
  subproject   ● 3   ◐ 0   · 0   ? 13  ! 0

PER-LANGUAGE  across 30 mechanisms:
  python       ● 19  ◐ 5   · 3   ? 3   ! 0
  go           ● 14  ◐ 3   · 4   ? 9   ! 0
  typescript   ● 16  ◐ 5   · 1   ? 8   ! 0
  java         ● 15  ◐ 2   · 3   ? 10  ! 0
  csharp       ● 13  ◐ 2   · 1   ? 14  ! 0
  ruby         ● 8   ◐ 2   · 1   ? 19  ! 0
  php          ● 6   ◐ 1   · 0   ? 23  ! 0
  swift        ● 2   ◐ 1   · 0   ? 27  ! 0
  c_cpp        ● 2   ◐ 0   · 1   ? 27  ! 0
  scala        ● 6   ◐ 1   · 0   ? 23  ! 0
  clojure      ● 3   ◐ 1   · 0   ? 26  ! 0
  dart         ● 4   ◐ 3   · 0   ? 23  ! 0
  elixir       ● 5   ◐ 0   · 0   ? 25  ! 0
  rust         ● 6   ◐ 2   · 0   ? 22  ! 0
  solidity     ● 4   ◐ 0   · 0   ? 26  ! 0
  terraform    ● 0   ◐ 0   · 0   ? 30  ! 0
```

COVERAGE OF THE COVERAGE: 165/480 cells have a fixture (34.4%) — 123 full, 28 partial, 14 none, 315 unknown, 0 error.

`legacy_only` (fixtures with no `cells`, graded by run.py only): 105

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
