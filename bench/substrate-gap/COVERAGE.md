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
Engine `0.5.5` · vocabulary digest `f31e7dc48404` · schema 3

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

Each cell reads `fixture/cohort`: the fixture grade, then the outlier cohort's grade of the same cell (`Cohort grades` below; `-` after the slash: no cohort row exercises it). The rollups count the fixture grade.

```
LEGEND ● full  ◐ partial  · none (fixture exists, nothing emitted)  ? unknown (no fixture)  - n/a (see Not applicable)  ! error
           http_cl http_sr   kafka    amqp sqs/sns  pubsub azure_s    nats   redis    mqtt   taskq    grpc graphql      ws eventbu      db    migr  config secrets   flags    cron cli_def cli_inv   calls imports injects    impl   tests service subproj
python         ●/·     ●/·     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
go             ●/·     ●/·     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/·     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
typescript     ●/·     ●/·     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
java           ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/●     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
csharp         ●/-     ●/·     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
ruby           ●/-     ●/·     ●/-     ●/-     ●/-     ●/-     ?/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
php            ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ?/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
swift          ●/-     ●/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ●/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-
c_cpp          ●/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ●/◐     ●/-     ?/-     ?/-     ?/-     ?/-     ?/-
scala          ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
clojure        ●/-     ●/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ●/-     ●/-     ?/-     ?/-     ?/-     ?/-     ?/-
dart           ●/·     ●/·     ●/-     ●/-     ●/-     ●/-     ?/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
elixir         ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ?/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
rust           ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-     ●/◐     ●/-     ●/-     ●/-     ●/-     ●/-     ●/-
solidity       -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     ●/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     -/-     ●/-     ●/-     ?/-     ●/-     ?/-     ?/-     ?/-
terraform      ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     -/-     ?/-     -/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     -/-     ?/-     -/-     ?/-     -/-     -/-     ?/-     ?/-     ?/-
kotlin         ●/-     ●/·     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ●/◐     ●/-     ●/-     ●/-     ?/-     ?/-     ?/-
lua            ●/-     ●/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ?/-     ●/-     ?/-     ?/-     ?/-     ?/-     ●/-     ●/-     ●/-     ?/-     ●/-     ●/-     ?/-     ?/-

PER-MECHANISM  across 18 languages:
  http_client  ● 16  ◐ 0   · 0   ? 1   - 1   ! 0
  http_server  ● 15  ◐ 0   · 0   ? 2   - 1   ! 0
  kafka        ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  amqp         ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  sqs_sns      ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  pubsub       ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  azure_sb     ● 7   ◐ 0   · 0   ? 10  - 1   ! 0
  nats         ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  redis        ● 11  ◐ 0   · 0   ? 5   - 2   ! 0
  mqtt         ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  taskq        ● 11  ◐ 0   · 0   ? 5   - 2   ! 0
  grpc         ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  graphql      ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  ws           ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  eventbus     ● 12  ◐ 0   · 0   ? 6   - 0   ! 0
  db           ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  migrations   ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  config       ● 12  ◐ 0   · 0   ? 5   - 1   ! 0
  secrets      ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  flags        ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  cron         ● 11  ◐ 0   · 0   ? 6   - 1   ! 0
  cli_def      ● 11  ◐ 0   · 0   ? 5   - 2   ! 0
  cli_inv      ● 12  ◐ 0   · 0   ? 5   - 1   ! 0
  calls        ● 17  ◐ 0   · 0   ? 0   - 1   ! 0
  imports      ● 16  ◐ 0   · 0   ? 2   - 0   ! 0
  injects      ● 12  ◐ 0   · 0   ? 5   - 1   ! 0
  impl         ● 14  ◐ 0   · 0   ? 3   - 1   ! 0
  tests        ● 12  ◐ 0   · 0   ? 6   - 0   ! 0
  service      ● 11  ◐ 0   · 0   ? 7   - 0   ! 0
  subproject   ● 11  ◐ 0   · 0   ? 7   - 0   ! 0

PER-LANGUAGE  across 30 mechanisms:
  python       ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  go           ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  typescript   ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  java         ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  csharp       ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  ruby         ● 29  ◐ 0   · 0   ? 1   - 0   ! 0
  php          ● 29  ◐ 0   · 0   ? 1   - 0   ! 0
  swift        ● 3   ◐ 0   · 0   ? 27  - 0   ! 0
  c_cpp        ● 3   ◐ 0   · 0   ? 27  - 0   ! 0
  scala        ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  clojure      ● 4   ◐ 0   · 0   ? 26  - 0   ! 0
  dart         ● 29  ◐ 0   · 0   ? 1   - 0   ! 0
  elixir       ● 29  ◐ 0   · 0   ? 1   - 0   ! 0
  rust         ● 30  ◐ 0   · 0   ? 0   - 0   ! 0
  solidity     ● 4   ◐ 0   · 0   ? 4   - 22  ! 0
  terraform    ● 0   ◐ 0   · 0   ? 24  - 6   ! 0
  kotlin       ● 6   ◐ 0   · 0   ? 24  - 0   ! 0
  lua          ● 8   ◐ 0   · 0   ? 22  - 0   ! 0
```

COVERAGE OF THE COVERAGE: 354/512 applicable cells have a fixture (69.1%) — 354 full, 0 partial, 0 none, 158 unknown, 0 error, 28 n/a.

`legacy_only` (fixtures with no `cells`, graded by run.py only): 315

## Cells routed via an alternative mechanism

A `●` here does not mean the intended path fired — it means SOME path did. These cells resolved through their fallback registry.

| cell | level | via | primary |
|---|---|---|---|
| `csharp/azure_sb` | ● full | `queue` | `eventbus` |
| `csharp/mqtt` | ● full | `queue` | `eventbus` |
| `csharp/pubsub` | ● full | `queue` | `eventbus` |
| `csharp/redis` | ● full | `queue` | `eventbus` |
| `csharp/sqs_sns` | ● full | `queue` | `eventbus` |
| `dart/mqtt` | ● full | `queue` | `eventbus` |
| `dart/pubsub` | ● full | `queue` | `eventbus` |
| `dart/redis` | ● full | `queue` | `eventbus` |
| `dart/sqs_sns` | ● full | `queue` | `eventbus` |
| `elixir/mqtt` | ● full | `queue` | `eventbus` |
| `elixir/pubsub` | ● full | `queue` | `eventbus` |
| `elixir/redis` | ● full | `queue` | `eventbus` |
| `elixir/sqs_sns` | ● full | `queue` | `eventbus` |
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
| `php/mqtt` | ● full | `queue` | `eventbus` |
| `php/pubsub` | ● full | `queue` | `eventbus` |
| `php/redis` | ● full | `queue` | `eventbus` |
| `php/sqs_sns` | ● full | `queue` | `eventbus` |
| `python/azure_sb` | ● full | `queue` | `eventbus` |
| `python/mqtt` | ● full | `queue` | `eventbus` |
| `python/pubsub` | ● full | `queue` | `eventbus` |
| `python/redis` | ● full | `queue` | `eventbus` |
| `python/sqs_sns` | ● full | `queue` | `eventbus` |
| `ruby/mqtt` | ● full | `queue` | `eventbus` |
| `ruby/pubsub` | ● full | `queue` | `eventbus` |
| `ruby/redis` | ● full | `queue` | `eventbus` |
| `ruby/sqs_sns` | ● full | `queue` | `eventbus` |
| `rust/azure_sb` | ● full | `queue` | `eventbus` |
| `rust/mqtt` | ● full | `queue` | `eventbus` |
| `rust/pubsub` | ● full | `queue` | `eventbus` |
| `rust/redis` | ● full | `queue` | `eventbus` |
| `rust/sqs_sns` | ● full | `queue` | `eventbus` |
| `scala/azure_sb` | ● full | `queue` | `eventbus` |
| `scala/mqtt` | ● full | `queue` | `eventbus` |
| `scala/pubsub` | ● full | `queue` | `eventbus` |
| `scala/redis` | ● full | `queue` | `eventbus` |
| `scala/sqs_sns` | ● full | `queue` | `eventbus` |
| `typescript/azure_sb` | ● full | `queue` | `eventbus` |
| `typescript/mqtt` | ● full | `queue` | `eventbus` |
| `typescript/pubsub` | ● full | `queue` | `eventbus` |
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

## Use patterns

The second axis (0.5.5): how code is USED rather than how services talk. Graded by the same rules as a mechanism, filed under `patterns/<language>/<pattern>/`, never counted in the totals above (`matrix_vocab.PATTERNS`).

- `h_cmd` — **host_command**: Neovim nvim_create_user_command, a Maki register_tool, a VS Code registerCommand: the host's command -> the script's handler
- `h_evt` — **host_event**: Neovim autocmds, WoW RegisterEvent + OnEvent, AwesomeWM connect_signal: a host event name -> the script's callback
- `h_cb` — **host_callback**: LOVE love.load / update / draw, Defold update(self, dt): the host calls the function by its name, so it is an entry (ENTRYPOINT cell), never dead
- `cfg_tbl` — **config_table**: configuration as code the host reads: WezTerm's returned table, vim.opt
- `embed` — **embedded_script**: another language's code or config runs the script: nginx content_by_lua_file (a location ROUTE HANDLED_BY the module), a Redis EVAL / register_script
- `script` — **script_entry**: a standalone script run from a shell (`#!/usr/bin/env lua`, `arg`): its MODULE is an entry
- `proc` — **process_entry**: Dockerfile ENTRYPOINT / CMD, compose command, k8s command / args, Procfile, systemd ExecStart: the host that starts the process -> the code it starts

```
              h_cmd    h_evt     h_cb  cfg_tbl    embed   script     proc
python            ●        ?        ?        ?        ?        ●        ◐
go                ?        ?        ?        ?        ?        ?        ◐
typescript        ?        ?        ?        ?        ?        ?        ◐
java              ?        ?        ?        ?        ?        ?        ◐
csharp            ?        ?        ?        ?        ?        ?        ◐
ruby              ?        ?        ?        ?        ?        ?        ?
php               ?        ?        ?        ?        ?        ?        ?
swift             ?        ?        ?        ?        ?        ?        ?
c_cpp             ?        ?        ?        ?        ?        ?        ?
scala             ?        ?        ?        ?        ?        ?        ?
clojure           ?        ?        ?        ?        ?        ?        ?
dart              ?        ?        ?        ?        ?        ?        ?
elixir            ?        ?        ?        ?        ?        ?        ?
rust              ?        ?        ?        ?        ?        ?        ◐
solidity          ?        ?        ?        ?        ?        ?        ?
terraform         ?        ?        ?        ?        ?        ?        ?
kotlin            ?        ?        ?        ?        ?        ?        ?
lua               ●        ·        ◐        ●        ◐        ◐        ?
```

USE PATTERNS: 14/126 cells have a fixture — 4 full, 9 partial, 1 none, 112 unknown, 0 error.

## Cohort grades

Release `0.5.4-pre` (`glia 0.5.3 (build 0.5.3+pdc42bbace0d19e35)`, tasks digest `810577329035`): 195 of 200 probe rows observed (bench/cohort/cells.py tags each with the cells and chains it exercises). A cell is full when >= 80% of its rows passed, partial at >= 30%, none below. 22 grid cells graded (1 full, 9 partial, 12 none), 0 of them with no fixture; 21 disagree with their fixture grade; 4 chain cells graded; 5 cells created by a miss.

### Disagreements

Both grades exist and differ: the work list (README: reading a disagreement).

| cell | fixture | cohort | passed | repos | missed checks |
|---|---|---|---|---|---|
| `c_cpp/calls` | ● full | ◐ partial | 1/2 | immich 1/2 | glia_blind 1 |
| `csharp/calls` | ● full | ◐ partial | 4/6 | microservices-demo 1/1, squidex 3/5 | glia_blind 1, glia_wrong 1 |
| `csharp/http_server` | ● full | · none | 0/4 | squidex 0/4 | glia_blind 8, glia_wrong 2, noisy 2 |
| `dart/calls` | ● full | ◐ partial | 7/13 | immich 2/5, samples 5/8 | glia_blind 4, glia_wrong 1, noisy 1 |
| `dart/http_client` | ● full | · none | 0/5 | immich 0/2, samples 0/3 | glia_blind 10, glia_wrong 3 |
| `dart/http_server` | ● full | · none | 0/4 | immich 0/1, samples 0/3 | glia_blind 10, glia_wrong 2 |
| `go/calls` | ● full | ◐ partial | 8/13 | gin-vue-admin 2/3, microservices-demo 2/2, photoprism 3/4, websocket 1/4 | glia_wrong 1, noisy 4 |
| `go/grpc` | ● full | · none | 0/1 | microservices-demo 0/1 | glia_wrong 2 |
| `go/http_client` | ● full | · none | 0/3 | microservices-demo 0/3 | glia_wrong 3 |
| `go/http_server` | ● full | · none | 0/10 | gin-vue-admin 0/4, microservices-demo 0/3, photoprism 0/3 | glia_blind 9, glia_wrong 10, noisy 3 |
| `kotlin/calls` | ● full | ◐ partial | 5/8 | immich 1/2, tolgee 4/6 | glia_blind 2, noisy 1 |
| `kotlin/http_server` | ● full | · none | 0/4 | tolgee 0/4 | glia_blind 9 |
| `python/calls` | ● full | ◐ partial | 15/21 | click 7/8, mealie 2/3, microservices-demo 1/1, plane 4/5, prefect 1/4 | glia_blind 1, noisy 5 |
| `python/http_client` | ● full | · none | 0/2 | prefect 0/2 | glia_wrong 4, noisy 1 |
| `python/http_server` | ● full | · none | 0/10 | mealie 0/3, microservices-demo 0/2, plane 0/2, prefect 0/3 | glia_blind 15, glia_wrong 6, noisy 1 |
| `ruby/calls` | ● full | ◐ partial | 5/7 | chatwoot 5/7 | glia_blind 1, noisy 1 |
| `ruby/http_server` | ● full | · none | 0/4 | chatwoot 0/4 | glia_blind 10 |
| `rust/calls` | ● full | ◐ partial | 3/5 | hoppscotch 2/3, immich 1/2 | glia_blind 2 |
| `typescript/calls` | ● full | ◐ partial | 39/68 | chatwoot 1/3, documenso 4/8, gin-vue-admin 1/2, hoppscotch 4/7, immich 1/3, mealie 0/2, medusa 2/8, microservices-demo 3/3, scaffold-eth-2 4/5, shell 7/8, squidex 3/5, tolgee 2/2, twenty 3/8, zora-protocol 4/4 | glia_blind 16, glia_wrong 2, noisy 11 |
| `typescript/http_client` | ● full | · none | 0/49 | chatwoot 0/4, documenso 0/1, gin-vue-admin 0/4, hoppscotch 0/4, immich 0/1, mealie 0/3, medusa 0/4, photoprism 0/3, plane 0/4, prefect 0/2, scaffold-eth-2 0/3, squidex 0/4, tolgee 0/4, twenty 0/4, zora-protocol 0/4 | glia_blind 92, glia_wrong 26, noisy 7 |
| `typescript/http_server` | ● full | · none | 0/40 | documenso 0/1, hoppscotch 0/4, immich 0/6, mealie 0/1, medusa 0/4, microservices-demo 0/2, plane 0/2, prefect 0/4, scaffold-eth-2 0/3, squidex 0/1, tolgee 0/4, twenty 0/4, zora-protocol 0/4 | glia_blind 67, glia_wrong 25, noisy 5 |

### Cells with no fixture the cohort grades

No fixture claims the cell; real repos exercise it.

_None._

### Chain cells

Where mechanisms chain (bench/cohort/cells.py CHAINS); graded by the cohort, fixture side by generated repos.

| cell | fixture | cohort | passed | repos | missed checks |
|---|---|---|---|---|---|
| `chain/client_wrapper_endpoint` | ? unknown | · none | 0/15 | gin-vue-admin 0/1, immich 0/3, mealie 0/1, photoprism 0/1, plane 0/1, prefect 0/3, twenty 0/1, zora-protocol 0/4 | glia_blind 19, glia_wrong 15, noisy 2 |
| `chain/const_prefix` | ? unknown | · none | 0/2 | immich 0/1, samples 0/1 | glia_blind 1, glia_wrong 3 |
| `chain/sfc_service_endpoint` | ? unknown | · none | 0/17 | chatwoot 0/3, gin-vue-admin 0/3, hoppscotch 0/2, immich 0/3, mealie 0/3, photoprism 0/3 | glia_blind 35, glia_wrong 9, noisy 2 |
| `chain/workspace_sdk_route` | ? unknown | · none | 0/13 | immich 0/6, medusa 0/2, plane 0/1, twenty 0/1, zora-protocol 0/3 | glia_blind 24, glia_wrong 9 |

### Cells the cohort created

A cohort miss on a cell outside the vocabulary (a language glia has no parser for, a boundary no column models).

| cell | fixture | cohort | passed | repos | missed checks |
|---|---|---|---|---|---|
| `go/queue` | ? unknown | · none | 0/1 | photoprism 0/1 | glia_blind 2, glia_wrong 1 |
| `svelte/calls` | ? unknown | · none | 0/1 | immich 0/1 | glia_blind 1 |
| `svelte/http_client` | ? unknown | · none | 0/3 | immich 0/3 | glia_blind 9 |
| `typescript/queue` | ? unknown | · none | 0/1 | photoprism 0/1 | glia_blind 2, glia_wrong 1 |
| `typescript/rpc` | ? unknown | · none | 0/3 | documenso 0/3 | glia_blind 7, glia_wrong 2 |
<!-- END generated -->
