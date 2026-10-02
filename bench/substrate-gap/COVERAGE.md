# Coverage — 17 languages x 30 mechanisms

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

`-` is not a `?` either. `-` means the language or runtime cannot express the
mechanism at all (`matrix_vocab.NOT_APPLICABLE`, listed under "Not applicable"
below with its reason), so the cell is left out of the coverage denominator. The
list is falsifiable: a fixture that claims a `-` cell is a cell error until its
entry is deleted in the same commit.

Prose outside the generated block below is hand-maintained and survives
regeneration. (This paragraph deliberately does not quote the marker strings:
the splice partitions on the FIRST marker it finds, so a literal marker inside
the prose would make the preamble eat itself.)

<!-- BEGIN generated: matrix.py --emit -->
Engine `0.5.0` · vocabulary digest `916e9fb659ff` · schema 2

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
LEGEND ● full  ◐ partial  · none (fixture exists, nothing emitted)  ? unknown (no fixture)  - n/a (see Not applicable)  ! error
           http_cl http_sr   kafka    amqp sqs/sns  pubsub azure_s    nats   redis    mqtt   taskq    grpc graphql      ws eventbu      db    migr  config secrets   flags    cron cli_def cli_inv   calls imports injects    impl   tests service subproj
python           ●       ●       ●       ●       ●       ◐       ●       ●       ●       ●       ●       ●       ●       ●       ·       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●
go               ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ·       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●
typescript       ●       ●       ●       ●       ●       ◐       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●
java             ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ·       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●
csharp           ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ·       ●       ●       ●       ●       ●       ●       ●       ·       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●       ●
ruby             ●       ●       ·       ·       ·       ·       ?       ●       ●       ●       ●       ●       ·       ●       ·       ●       ●       ●       ·       ◐       ●       ●       ◐       ●       ●       ◐       ◐       ◐       ●       ●
php              ●       ●       ◐       ◐       ·       ·       ?       ·       ·       ·       ·       ●       ●       ·       ·       ●       ●       ◐       ·       ◐       ●       ●       ·       ●       ●       ●       ◐       ●       ●       ●
swift            ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ?       ?       ?       ?       ?       ?
c_cpp            ·       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
scala            ●       ◐       ·       ·       ·       ·       ●       ●       ◐       ●       ·       ●       ·       ·       ·       ·       ●       ◐       ●       ●       ·       ·       ·       ●       ●       ●       ●       ◐       ●       ◐
clojure          ●       ◐       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ?       ?       ?       ?       ?
dart             ●       ◐       ◐       ·       ·       ·       ?       ·       ·       ·       ·       ●       ◐       ·       ·       ·       ●       ◐       ◐       ●       ●       ·       ·       ●       ●       ◐       ●       ◐       ●       ●
elixir           ●       ●       ·       ·       ·       ·       ?       ·       ·       ·       ◐       ·       ·       ●       ·       ·       ·       ◐       ◐       ◐       ●       ·       ·       ●       ●       ·       ·       ◐       ●       ●
rust             ●       ●       ●       ·       ·       ·       ·       ·       ●       ●       ·       ●       ·       ·       ·       ●       ●       ●       ·       ◐       ·       ●       ●       ●       ●       ◐       ◐       ●       ●       ●
solidity         -       -       -       -       -       -       -       -       -       -       -       -       -       -       ●       -       -       -       -       -       -       -       -       ●       ●       ?       ●       ?       ?       ?
terraform        ?       ?       ?       ?       ?       ?       ?       ?       -       ?       -       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       -       ?       -       ?       -       -       ?       ?       ?
kotlin           ●       ●       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ?       ●       ●       ●       ●       ?       ?       ?

PER-MECHANISM  across 17 languages:
  http_client  ● 14  ◐ 0   · 1   ? 1   - 1   ! 0
  http_server  ● 10  ◐ 4   · 0   ? 2   - 1   ! 0
  kafka        ● 6   ◐ 2   · 3   ? 5   - 1   ! 0
  amqp         ● 5   ◐ 1   · 5   ? 5   - 1   ! 0
  sqs_sns      ● 5   ◐ 0   · 6   ? 5   - 1   ! 0
  pubsub       ● 3   ◐ 2   · 6   ? 5   - 1   ! 0
  azure_sb     ● 6   ◐ 0   · 1   ? 9   - 1   ! 0
  nats         ● 7   ◐ 0   · 4   ? 5   - 1   ! 0
  redis        ● 7   ◐ 1   · 3   ? 4   - 2   ! 0
  mqtt         ● 8   ◐ 0   · 3   ? 5   - 1   ! 0
  taskq        ● 3   ◐ 1   · 7   ? 4   - 2   ! 0
  grpc         ● 10  ◐ 0   · 1   ? 5   - 1   ! 0
  graphql      ● 6   ◐ 1   · 4   ? 5   - 1   ! 0
  ws           ● 7   ◐ 0   · 4   ? 5   - 1   ! 0
  eventbus     ● 5   ◐ 0   · 7   ? 5   - 0   ! 0
  db           ● 8   ◐ 0   · 3   ? 5   - 1   ! 0
  migrations   ● 10  ◐ 0   · 1   ? 5   - 1   ! 0
  config       ● 7   ◐ 4   · 0   ? 5   - 1   ! 0
  secrets      ● 5   ◐ 2   · 4   ? 5   - 1   ! 0
  flags        ● 7   ◐ 4   · 0   ? 5   - 1   ! 0
  cron         ● 9   ◐ 0   · 2   ? 5   - 1   ! 0
  cli_def      ● 8   ◐ 0   · 3   ? 4   - 2   ! 0
  cli_inv      ● 6   ◐ 1   · 4   ? 5   - 1   ! 0
  calls        ● 16  ◐ 0   · 0   ? 0   - 1   ! 0
  imports      ● 15  ◐ 0   · 0   ? 2   - 0   ! 0
  injects      ● 8   ◐ 3   · 1   ? 4   - 1   ! 0
  impl         ● 9   ◐ 3   · 1   ? 3   - 1   ! 0
  tests        ● 7   ◐ 4   · 0   ? 6   - 0   ! 0
  service      ● 11  ◐ 0   · 0   ? 6   - 0   ! 0
  subproject   ● 10  ◐ 1   · 0   ? 6   - 0   ! 0

PER-LANGUAGE  across 30 mechanisms:
  python       ● 28  ◐ 1   · 1   ? 0   - 0   ! 0
  go           ● 29  ◐ 0   · 1   ? 0   - 0   ! 0
  typescript   ● 29  ◐ 1   · 0   ? 0   - 0   ! 0
  java         ● 29  ◐ 0   · 1   ? 0   - 0   ! 0
  csharp       ● 28  ◐ 0   · 2   ? 0   - 0   ! 0
  ruby         ● 17  ◐ 5   · 7   ? 1   - 0   ! 0
  php          ● 14  ◐ 5   · 10  ? 1   - 0   ! 0
  swift        ● 2   ◐ 1   · 0   ? 27  - 0   ! 0
  c_cpp        ● 2   ◐ 0   · 1   ? 27  - 0   ! 0
  scala        ● 13  ◐ 5   · 12  ? 0   - 0   ! 0
  clojure      ● 3   ◐ 1   · 0   ? 26  - 0   ! 0
  dart         ● 10  ◐ 7   · 12  ? 1   - 0   ! 0
  elixir       ● 8   ◐ 5   · 16  ? 1   - 0   ! 0
  rust         ● 16  ◐ 3   · 11  ? 0   - 0   ! 0
  solidity     ● 4   ◐ 0   · 0   ? 4   - 22  ! 0
  terraform    ● 0   ◐ 0   · 0   ? 24  - 6   ! 0
  kotlin       ● 6   ◐ 0   · 0   ? 24  - 0   ! 0
```

COVERAGE OF THE COVERAGE: 346/482 applicable cells have a fixture (71.8%) — 238 full, 34 partial, 74 none, 136 unknown, 0 error, 28 n/a.

`legacy_only` (fixtures with no `cells`, graded by run.py only): 186

## Cells routed via an alternative mechanism

A `●` here does not mean the intended path fired — it means SOME path did. These cells resolved through their fallback registry.

| cell | level | via | primary |
|---|---|---|---|
| `csharp/azure_sb` | ● full | `queue` | `eventbus` |
| `csharp/mqtt` | ● full | `queue` | `eventbus` |
| `csharp/pubsub` | ● full | `queue` | `eventbus` |
| `csharp/redis` | ● full | `queue` | `eventbus` |
| `csharp/sqs_sns` | ● full | `queue` | `eventbus` |
| `dart/mqtt` | · none | `queue` | `eventbus` |
| `dart/redis` | · none | `queue` | `eventbus` |
| `go/azure_sb` | ● full | `queue` | `eventbus` |
| `go/mqtt` | ● full | `queue` | `eventbus` |
| `go/pubsub` | ● full | `queue` | `eventbus` |
| `go/redis` | ● full | `queue` | `eventbus` |
| `go/sqs_sns` | ● full | `queue` | `eventbus` |
| `java/azure_sb` | ● full | `queue` | `eventbus` |
| `java/mqtt` | ● full | `queue` | `eventbus` |
| `java/pubsub` | ● full | `queue` | `eventbus` |
| `java/redis` | ● full | `queue` | `eventbus` |
| `java/sqs_sns` | ● full | `queue` | `eventbus` |
| `python/azure_sb` | ● full | `queue` | `eventbus` |
| `python/mqtt` | ● full | `queue` | `eventbus` |
| `python/pubsub` | ◐ partial | `queue` | `eventbus` |
| `python/redis` | ● full | `queue` | `eventbus` |
| `python/sqs_sns` | ● full | `queue` | `eventbus` |
| `ruby/mqtt` | ● full | `queue` | `eventbus` |
| `ruby/redis` | ● full | `queue` | `eventbus` |
| `rust/mqtt` | ● full | `queue` | `eventbus` |
| `rust/redis` | ● full | `queue` | `eventbus` |
| `scala/azure_sb` | ● full | `queue` | `eventbus` |
| `scala/mqtt` | ● full | `queue` | `eventbus` |
| `scala/redis` | ◐ partial | `queue` | `eventbus` |
| `typescript/azure_sb` | ● full | `queue` | `eventbus` |
| `typescript/mqtt` | ● full | `queue` | `eventbus` |
| `typescript/pubsub` | ◐ partial | `queue` | `eventbus` |
| `typescript/redis` | ● full | `queue` | `eventbus` |
| `typescript/sqs_sns` | ● full | `queue` | `eventbus` |

## Cell errors

_None._

## Not applicable

`-` cells: the language or runtime cannot express the mechanism (`matrix_vocab.NOT_APPLICABLE`, digest `6e1c8eedbe15`), so they leave the coverage denominator. A fixture that claims one is a cell error: delete the entry in the commit that adds it.

| cell | class | reason |
|---|---|---|
| `solidity/amqp` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/azure_sb` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/cli_def` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/cli_inv` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/config` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/cron` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/db` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/flags` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/graphql` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/grpc` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/http_client` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/http_server` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/kafka` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/migrations` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/mqtt` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/nats` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/pubsub` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/redis` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/secrets` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/sqs_sns` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/taskq` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `solidity/ws` | structural | an on-chain Solidity contract has no network, broker, filesystem, environment or process access; the off-chain tooling around it (hardhat / foundry scripts, subgraphs) is TypeScript / YAML and scores on those rows |
| `terraform/calls` | structural | Terraform has no user-defined functions; built-in and provider functions are not resolved callee qnames, and a module block is the imports column |
| `terraform/cli_def` | structural | Terraform declares no command-line interface |
| `terraform/impl` | structural | Terraform declares no types, so nothing implements or inherits |
| `terraform/injects` | structural | Terraform declares no types, so nothing is injected |
| `terraform/redis` | structural | no Terraform construct publishes to or subscribes on a Redis channel or list; ElastiCache resources declare the store only |
| `terraform/taskq` | structural | no Terraform construct names, enqueues or processes a task; a Cloud Tasks queue resource declares the queue only |
<!-- END generated -->
