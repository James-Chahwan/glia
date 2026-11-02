# Contributing to glia

Thank you for looking. glia is not yet set up to merge pull requests directly, and it is better to say so up front.

## Why, for now

Some things in glia are the way they are because it has not reached a PR-friendly stage yet, not because
contributions are unwelcome:

- **Releases land as big leaps.** A release can bring hundreds of commits at once (a repo-wide format, new modules,
  moved code), so a branch opened against one release is usually far behind by the next.
- **Graph ids are allocated by hand.** Node kinds, edge categories and cell types are numbered in
  `code-domain/src/lib.rs`, and a number is permanent once a `.gmap` is written with it. Two branches that each take
  "the next free id" collide, and one of them has to be renumbered. Since 0.5.7 every id sits in a numbered block, like
  a port range (block = the id's high 16 bits, 65,536 ids each; `core/src/ids.rs`): `0x0000` code, `0x0001` core
  (shared), `0x0002` build targets, `0x0003` goal (reserved), and `0x8000`-`0xFFFE` private. `core/ids.lock` lists every
  allocated id and only grows; `engine/tests/id_blocks.rs` fails on an id that moved, an id two domains share or a
  private id in a shipped table.
- **The gates are heavy.** A release runs the full test suite, a rank gate on six pinned public snapshots and a cohort
  gate on 21 public repos, about an hour in all. Those are release-only; a pull request runs the fast gate below.
- **The graph work is moving.** The next releases add more graph layers, stacking and new domains, so the shape a
  contribution would plug into is still changing.

## What works today

Five **seams** are documented in [docs/seams.md](docs/seams.md), each with the exact files and symbols it touches and
the check that proves it complete: a fixture, a matrix or pattern cell, a host-table row (with its fixture), a language
parser crate and a domain crate. The first three can be merged as pull requests once their checks pass; a parser or a
domain is read as a design (below). `engine/tests/seams.rs` fails when a language misses one of its seam points.

- **Issues**: a bug, a missing language or framework, a repo glia reads wrong, a use case. A short example repo helps
  most.
- **Example fixtures**: a directory under `bench/substrate-gap/fixtures/` (source files plus a `key.json` naming the
  nodes and edges glia should find; `bench/substrate-gap/README.md`) changes no engine code and can be merged almost any
  time. A fixture glia fails is a useful contribution in itself.
- **Matrix and pattern cells**: `bench/substrate-gap/matrix/<language>/<mechanism>/` (stamped by
  `bench/substrate-gap/scaffold.py`) and `bench/substrate-gap/patterns/<language>/<pattern>/`
  ([docs/seams.md](docs/seams.md#2-a-matrix-or-pattern-cell)).
- **Host-table rows**: one `code_domain::hosts::HOSTS` row with the fixture that shows it, when the row fits the
  table's existing shapes ([docs/seams.md](docs/seams.md#3-a-host-table-row)).
- **Code PRs are read as designs.** Everything outside those seams, a language parser and a domain crate included. A
  pull request is read (never run unreviewed), and the idea is rebuilt into the next
  release on top of the current tree, with you credited as `Co-authored-by` on the commits that use it. glia 0.5.5's Lua
  support came this way, from glia PR #3.
- **Take ids from the private range** in a PR: number from `0x8000_0001` up (or use no new id), and say in the issue
  what you need. A shipped table never holds a private id, so the release that merges the idea promotes them into the
  domain's block; a renumber is safe only because a private id never shipped.
- **Start a big feature as an issue** (a new language, a new kind of edge), so it can be lined up with a release.

## Before you open a PR

```bash
bash scripts/pr-check.sh                 # against origin/main (else main)
bash scripts/pr-check.sh --base <rev>    # against another base
```

It reads the paths your change touches (`git diff --name-only <base>...HEAD` plus the working tree) and runs only the
checks those paths need; the path table at the top of the script is the one list of which path needs which step:

- always `cargo fmt --all --check`;
- for each touched workspace crate, `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo test -p <crate>`;
- `core/`, `code-domain/`, `build-domain/`, `activation/`: the id lock (`engine_tests id_blocks::`);
- `parsers/`, `graph/`, `engine/`: the language seams (`engine_tests seams::`) and the self-scan (`self_scan::`);
- `cli/`, `README.md`, `skills/`: the CLI surface snapshots, the README `## CLI` usage lines, the skills' invocations;
- `py/`: the pyo3 surface, over a freshly built wheel;
- a fixture, a matrix or pattern cell, or a language parser crate: the `glia_py` wheel built into a venv
  (`~/.cache/glia/pr-check/venv`, `GLIA_PR_CHECK_VENV`; never your user site), `grade.py` on each touched fixture or
  cell, and `matrix.py --language <lang> --fail-on-error` for its row. A missed expectation is reported, not failed:
  a fixture glia does not read yet is still a contribution. `--no-wheel` skips the build and grades with a `glia_py`
  you already have (`GLIA_PR_CHECK_PY`).

Every step prints `[pr-check] step=<name> <ok|FAILED|skipped> <s>s`; the last line is
`[pr-check] ok - <n> steps in <s>s (base <rev12>)` (exit 0) or `[pr-check] FAILED (<steps>)` (exit 1). It respects
`GLIA_THREADS`, `CARGO_BUILD_JOBS` and `TMPDIR`, needs bash 4.3 or newer, and on a glibc older than 2.38 builds `glia-cli`
with `--no-default-features --features embed` (the WebGPU prebuilt needs 2.38).

Measured warm (target/ already built, 16 cores, `CARGO_BUILD_JOBS=8`, `CARGO_INCREMENTAL=0`), at 0.5.7: a docs-only
change 6 s (fmt); a fixture's `key.json` 30 to 60 s (fmt, the wheel relinked and installed, the grade, the matrix
row); a one-line Lua parser change 132 s (fmt 5, clippy 1, the crate's tests 1, `seams::` 49 and `self_scan::` 22 for
relinking the engine's test binary, the wheel 50, the matrix row 4). A first run compiles the workspace and, for a
fixture or parser change, the wheel: the parser change took 199 s from an empty target/ on that machine, and takes
longer on a laptop.

**Release-only, never run by pr-check:** the full workspace test suite (pr-check runs it only when a root
`Cargo.toml`, `Cargo.lock` or lint config moved), the rank gate (`scripts/rank-gate.sh`, six
pinned snapshots) and the cohort gate (`scripts/cohort-gate.sh`, 21 repos). pr-check prints which of them your change
will meet at the release (`release gates this change will run: rank, cohort`). `--rank-fast` opts into the rank gate's
five small snapshots, a few minutes after they are fetched; it never writes a baseline.

## Licence

glia is under the [Glia Software License v0.1](./LICENSE) (PolyForm Noncommercial 1.0.0 plus a worker-protection
overlay). A contribution is accepted under the same licence.
