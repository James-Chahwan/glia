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
Engine `0.4.18` · vocabulary digest `a3672b018b49` · schema 1

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
python           ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ●       ·       ●       ●       ·       ?       ◐       ·       ●       ●       ?       ●       ●       ?       ?
go               ●       ●       ●       ?       ?       ?       ?       ●       ?       ?       ?       ●       ?       ●       ?       ●       ?       ?       ?       ·       ◐       ●       ●       ●       ●       ?       ?       ?       ?       ·
typescript       ●       ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ◐       ●       ?       ?       ?       ●       ·       ?       ?       ?       ●       ●       ●       ?       ●       ?       ·
java             ●       ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ·       ?       ?       ?       ?       ?       ?       ●       ●       ●       ●       ?       ?       ·
csharp           ●       ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ·       ?       ?       ?       ?       ?       ?       ·       ?       ?       ?       ?       ●       ●       ●       ●       ?       ?       ?
ruby             ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ●       ·       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
php              ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?
swift            ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?
c_cpp            ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
scala            ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ●       ?       ?       ?
clojure          ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
dart             ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?
elixir           ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
rust             ?       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
solidity         ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ●       ?       ?       ?
terraform        ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?

PER-MECHANISM  across 16 languages:
  http_client  ● 11  ◐ 0   · 0   ? 5   ! 0
  http_server  ● 9   ◐ 0   · 0   ? 7   ! 0
  kafka        ● 4   ◐ 0   · 0   ? 12  ! 0
  amqp         ● 0   ◐ 0   · 0   ? 16  ! 0
  sqs_sns      ● 0   ◐ 0   · 0   ? 16  ! 0
  pubsub       ● 0   ◐ 0   · 0   ? 16  ! 0
  azure_sb     ● 0   ◐ 0   · 0   ? 16  ! 0
  nats         ● 1   ◐ 0   · 0   ? 15  ! 0
  redis        ● 0   ◐ 0   · 0   ? 16  ! 0
  mqtt         ● 0   ◐ 0   · 0   ? 16  ! 0
  taskq        ● 2   ◐ 0   · 0   ? 14  ! 0
  grpc         ● 1   ◐ 0   · 1   ? 14  ! 0
  graphql      ● 1   ◐ 0   · 0   ? 15  ! 0
  ws           ● 1   ◐ 1   · 0   ? 14  ! 0
  eventbus     ● 3   ◐ 0   · 0   ? 13  ! 0
  db           ● 4   ◐ 0   · 0   ? 12  ! 0
  migrations   ● 0   ◐ 0   · 3   ? 13  ! 0
  config       ● 1   ◐ 0   · 0   ? 15  ! 0
  secrets      ● 2   ◐ 0   · 1   ? 13  ! 0
  flags        ● 0   ◐ 0   · 3   ? 13  ! 0
  cron         ● 0   ◐ 1   · 0   ? 15  ! 0
  cli_def      ● 1   ◐ 1   · 0   ? 14  ! 0
  cli_inv      ● 1   ◐ 0   · 1   ? 14  ! 0
  calls        ● 14  ◐ 0   · 0   ? 2   ! 0
  imports      ● 12  ◐ 0   · 0   ? 4   ! 0
  injects      ● 3   ◐ 0   · 0   ? 13  ! 0
  impl         ● 5   ◐ 0   · 0   ? 11  ! 0
  tests        ● 2   ◐ 0   · 0   ? 14  ! 0
  service      ● 0   ◐ 0   · 0   ? 16  ! 0
  subproject   ● 0   ◐ 0   · 3   ? 13  ! 0

PER-LANGUAGE  across 30 mechanisms:
  python       ● 10  ◐ 1   · 3   ? 16  ! 0
  go           ● 11  ◐ 1   · 2   ? 16  ! 0
  typescript   ● 10  ◐ 1   · 2   ? 17  ! 0
  java         ● 9   ◐ 0   · 2   ? 19  ! 0
  csharp       ● 7   ◐ 0   · 2   ? 21  ! 0
  ruby         ● 6   ◐ 0   · 1   ? 23  ! 0
  php          ● 3   ◐ 0   · 0   ? 27  ! 0
  swift        ● 2   ◐ 0   · 0   ? 28  ! 0
  c_cpp        ● 2   ◐ 0   · 0   ? 28  ! 0
  scala        ● 4   ◐ 0   · 0   ? 26  ! 0
  clojure      ● 2   ◐ 0   · 0   ? 28  ! 0
  dart         ● 2   ◐ 0   · 0   ? 28  ! 0
  elixir       ● 4   ◐ 0   · 0   ? 26  ! 0
  rust         ● 3   ◐ 0   · 0   ? 27  ! 0
  solidity     ● 3   ◐ 0   · 0   ? 27  ! 0
  terraform    ● 0   ◐ 0   · 0   ? 30  ! 0
```

COVERAGE OF THE COVERAGE: 93/480 cells have a fixture (19.4%) — 78 full, 3 partial, 12 none, 387 unknown, 0 error.

`legacy_only` (fixtures with no `cells`, graded by run.py only): 20

## Cells routed via an alternative mechanism

A `●` here does not mean the intended path fired — it means SOME path did. These cells resolved through their fallback registry.

_None: every covered cell resolved through its primary path._

## Cell errors

_None._
<!-- END generated -->
