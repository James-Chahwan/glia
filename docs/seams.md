# Contribution seams

A **seam** is a place a change can plug into glia without touching the parts a release moves: the passes, the store,
the answers, the ranking. Each seam below lists what it is, the exact files and symbols it touches (by name: line
numbers go stale every release), the check that proves it is complete, and what a release promises not to move without
updating this file.

Which seams can merge as a pull request (James 2026-11-02, the 0.5.7 rulings):

| Seam | Merges as a PR? |
|---|---|
| [A fixture](#1-a-fixture) | yes, once its checks pass |
| [A matrix or pattern cell](#2-a-matrix-or-pattern-cell) | yes, once its checks pass |
| [A host-table row](#3-a-host-table-row) (with its fixture) | yes, once its checks pass, when the row fits the table's existing shapes |
| [A language parser](#4-a-language-parser) | read as a design and rebuilt into a release, with credit ([CONTRIBUTING.md](../CONTRIBUTING.md)) |
| [A domain crate](#5-a-domain-crate) | read as a design and rebuilt into a release, with credit |

Before a pull request, `bash scripts/pr-check.sh` runs every seam's checks that the change's paths need, in minutes
on a warm target/; the rank and cohort gates are the release's, never a PR's. A change outside every seam is read as a
design too. The language seam is documented here and pinned by a
completeness test, `engine/tests/seams.rs`; there is no single table of languages to edit.

## 1. A fixture

A small repo that shows one thing glia should read (or reads wrong), plus a key naming the nodes and edges it should
find. It changes no engine code.

**Touches**

- `bench/substrate-gap/fixtures/<name>/`: the source files and `key.json`, in the frozen vocabulary
  `bench/substrate-gap/README.md` defines (`expect_nodes`, `expect_edges`, `forbid`, `expect_cells`, ..). A file git
  will not track is shipped through `materialize` (same README).

**Checks**

- `bash scripts/pr-check.sh [--base <rev>]`, the one fast command: it runs, for the paths the change touches,
  the checks below and fmt / clippy / the touched crates' tests ([CONTRIBUTING.md](../CONTRIBUTING.md#before-you-open-a-pr)).
- `python3 bench/substrate-gap/grade.py fixtures/<name>` grades the one fixture (`--dump` lists everything the build
  emitted, to author the key against reality). It needs the `glia_py` wheel.
- `python3 bench/substrate-gap/run.py --no-log` grades every fixture. A fixture glia fails is still a contribution: it
  shows up as a blind spot or a partial, and the release that fixes it makes it green.
- `python3 bench/substrate-gap/test_grade.py` when the key uses an unusual field.

**What a release keeps:** the `key.json` vocabulary (`grade.py`'s `TOP_FIELDS`) only grows, and `run.py` discovers
`fixtures/*` by directory. A field that changes meaning changes this file and the README in the same commit.

## 2. A matrix or pattern cell

The coverage matrix grades every (language, mechanism) pair, and the use-pattern grid every (language, pattern) pair.
A cell is a fixture filed under the matrix's layout.

**Touches**

- A mechanism cell: `bench/substrate-gap/matrix/<language>/<mechanism>/key.json` plus its sources. Start it with
  `python3 bench/substrate-gap/scaffold.py <language> <mechanism>`, which stamps the skeleton from
  `bench/substrate-gap/matrix_vocab.py` (`LANGUAGES`, the mechanisms' kinds, categories and literal). The recipe is
  `bench/substrate-gap/AUTHORING.md`.
- A pattern cell: `bench/substrate-gap/patterns/<language>/<pattern>/key.json`, `"cells": ["<language>/<pattern>"]`,
  where `<pattern>` is one of `matrix_vocab.PATTERNS` (`host_command`, `host_event`, `host_callback`, `config_table`,
  `embedded_script`, `script_entry`, `process_entry`).
- A cell `matrix_vocab.NOT_APPLICABLE` lists as `n/a` is never authored: the same commit deletes the entry, or the
  cell grades `error`.

**Checks**

- `bash scripts/pr-check.sh [--base <rev>]`, the one fast command: it runs, for the paths the change touches,
  the checks below and fmt / clippy / the touched crates' tests ([CONTRIBUTING.md](../CONTRIBUTING.md#before-you-open-a-pr)).
- `python3 bench/substrate-gap/matrix.py --check` (exit 0) and `python3 bench/substrate-gap/test_matrix.py`.
- `python3 bench/substrate-gap/grade.py matrix/<language>/<mechanism>` (or `patterns/..`) for the one cell.
- A pull request never regenerates `bench/substrate-gap/COVERAGE.md`, `results-latest.json` or `legacy-latest.json`
  (`matrix.py --emit`, `run.py --emit`): a release does that once, after its wheel is built.

**What a release keeps:** `matrix_vocab.LANGUAGES`, the mechanism ids and `PATTERNS` ids are append-only (rows and
columns are added at the end, never renamed), so a cell's directory never moves under it.

## 3. A host-table row

`code_domain::hosts` (`code-domain/src/hosts.rs`) is the table of programs that load code and call into it: editors,
games, web frameworks, test runners, bundlers, MCP servers. A row says **what to look for**, never that something is
there: no row mints an edge by itself (James 2026-10-31, "as long as it's not a crutch"). The parser reads the
registration call or declaration in the AST, with its handler argument, gated on the host being present in that file
(its import, require, unshadowed global, manifest or layout), and only then mints the node.

**Touches**

- One `HostRow` appended to `hosts::HOSTS`: `name` (the `<host>` of `host_cmd:<host>:<name>` /
  `host_event:<host>:<event>` / `config:<host>:<key>`), `lang` (the parser that reads it), `exts`, `gate`
  (`hosts::Gate`), and whichever of `loads`, `registrations` (`hosts::Registration` with its `Callee` and `Shape`),
  `callbacks`, `config` and `manifest` the host has.
- Its fixture, in the same commit: a `bench/substrate-gap/patterns/<lang>/<pattern>/` cell (`host_command`,
  `host_event`, `host_callback`, `config_table`) whose key names the row's node and its HANDLED_BY edge (or the
  ENTRYPOINT cell of a callback), and a `forbid` for the same call with the host absent. A row without a fixture is not
  merged.
- When the language's `engine::coverage` caveat lists the hosts it reads (the Python `HANDLED_BY` row does), its text
  names the new host, and the reflexion golden is regenerated:
  `GLIA_UPDATE_GOLDEN=1 cargo test -p glia-engine --lib rules::reflexion_parity`.

A row is data only when the parser already reads its `Gate`, `Callee` and `Shape`. A host that needs a new shape (MCP
servers needed `Shape::Decorator`'s `literal_name` and parser code to read it, 0.5.7's HF.3) is parser work and is read
as a design.

**Checks**

- `bash scripts/pr-check.sh [--base <rev>]`, the one fast command: it runs, for the paths the change touches,
  the checks below and fmt / clippy / the touched crates' tests ([CONTRIBUTING.md](../CONTRIBUTING.md#before-you-open-a-pr)).
- `cargo test -p glia-code-domain hosts::` (`the_table_is_valid` runs `hosts::validate()` over every row: a duplicate
  or reserved name, a missing `lang` / `exts`, an empty or malformed gate, a callee not rooted at its gate's global, a
  callee and shape that do not go together, a manifest another row claims).
- The parser's own host tests (`cargo test -p glia-parser-<lang>`) and the fixture through `grade.py`.

**What a release keeps:** `HostRow`'s fields and the `Gate` / `Callee` / `Shape` variants only grow (each enum is
`#[non_exhaustive]`), and the qname forms above are stable.

## 4. A language parser

A tree-sitter parser crate behind the engine's routing. It is the widest seam: the touch points outside the crate are
few, but they are in hot code, so a parser is read as a design. 0.5.5's Lua (from glia PR #3, `ea57fee9`) is the
worked example; outside `parsers/code/lua/` it touched the lines below plus its fixtures and tests.

**Touches**

- `parsers/code/<lang>/`: the crate `glia-parser-<lang>`, exposing `parse_file(source, path, module_qname, repo)`
  returning a `FileParse` (nodes, `UnresolvedRef`s, `CallSite`s, `CodeNav`). It extracts; it never resolves
  (CLAUDE.md, "Parser-vs-Graph Split"). Kinds, categories and cells by their constant names in `glia_code_domain`.
- The root `Cargo.toml`: one `[workspace] members` line, `"parsers/code/<lang>"`. Its grammar crate moves
  `Cargo.lock`, which a release reads with care.
- `engine/Cargo.toml`: `glia-parser-<lang> = { path = "../parsers/code/<lang>" }`. The engine owns every parser
  dependency; `py/` lists none.
- `engine/src/extract.rs`:
  - `detect_language`: an arm from the file extension(s) to the tag;
  - `parse_one_as`: the tag's arm calling `glia_parser_<lang>::parse_file` (behind the public `parse_one`,
    `parse_one_with` and `parse_one_with_go_modules`);
  - `build_group` / `TS_FAMILY`: only when the tag joins another language's graph (the TS family builds as
    `typescript`, Kotlin joins Java's JVM graph).
- `engine/src/build/lang_build.rs`: a `build_solo` arm calling `glia_graph::build_<lang>`. A language that takes the
  `_` arm (`glia_graph::build_typescript`'s shared passes with `resolve_relative_source`) does so on purpose and is
  named in `engine/tests/seams.rs`'s `SHARED_BUILD`, as Solidity and Terraform are.
- `graph/src/build.rs`: `pub fn build_<lang>` (through the `build` glob, so it is `glia_graph::build_<lang>`), its
  passes in a child module `graph/src/build/<lang>.rs` (`mod <lang>;` in `build.rs`).
- `engine/src/health.rs`: the extension leaves `CODE_LIKE_EXTS` (the files no parser reads), and the array's length
  drops by one.
- Optional: `parsers/code/extractors/` needles the language has (Lua's `os.execute(` / `io.popen(` CLI needles in
  `cli.rs`); routes and clients are cross-cutting extractors there, never in the language crate.
- The matrix: a `matrix_vocab.LANGUAGES` row appended (and `LANGUAGE_ALIASES` for its extension), at least one
  `bench/substrate-gap/matrix/<lang>/<mechanism>/` cell, the row counts `test_matrix.py` pins.
- Tests: `engine/tests/<lang>.rs` over the language's fixtures, and the tag's rows in `engine/tests/seams.rs`
  (`EXTS`, `GROUPS`, `SAMPLES`).

PARSER_STAMP hashes `parsers/`, `engine/src/` and `graph/`, so a new parser invalidates parse caches once with no
version bump.

**Checks**

- `bash scripts/pr-check.sh [--base <rev>]`, the one fast command: it runs, for the paths the change touches,
  the checks below and fmt / clippy / the touched crates' tests ([CONTRIBUTING.md](../CONTRIBUTING.md#before-you-open-a-pr)).
- `cargo test -p glia-engine --test engine_tests seams::` prints
  `[seams] languages=<n> groups=<g> matrix_rows=<m> exempt=<e>` and fails when a tag `detect_language` returns misses
  any of: a row in the test's tables, a `parse_one` that parses a one-line sample, the parser directory, its workspace
  member and engine dependency, a build group `build_group` and `lang_build` agree on, a `matrix_vocab.LANGUAGES` row,
  a `bench/substrate-gap/matrix/<group>/` directory, an extension still in `CODE_LIKE_EXTS`, or a MODULE missing from a
  joint build of every sample. Its named `EXEMPT` list (with reasons) holds the routing tags `proto`, `graphql` and
  `avro`, which `route.rs` scans and no parser reads, and Terraform's matrix directory.
- `cargo test -p glia-parser-<lang>`, `cargo test -p glia-graph`, `cargo test -p glia-engine --test engine_tests <lang>::`.

**What a release keeps:** the symbols named above (`detect_language`, `parse_one_as`, `build_group`, `TS_FAMILY`,
`build_solo`, `glia_graph::build_<lang>`, `CODE_LIKE_EXTS`) and the `parse_file` shape. A release that renames or
moves one updates this section and `engine/tests/seams.rs` in the same commit; the test reads those symbols by name,
so a silent move fails it.

## 5. A domain crate

glia's model is a Domain-Parameterised Graph (`docs/dpg.md`): a domain brings its own node kinds, edge categories,
cell types and tables. Code is the first domain; build targets (0.5.7) the second.

**Touches**

- `<domain>/`: the domain crate (`build-domain/` is the worked example). It depends on the domain-free layers only
  (`glia-core`, `glia-activation`, `glia-store`) and holds:
  - its registries (`build-domain/src/registry.rs`): every id `glia_core::ids::id_in(<block>, n)`, `n` dense from 1;
  - its tables (`build-domain/src/profile.rs`): a `glia_activation::profile::DomainTables` (entries, carry edges,
    activation weights) and its passes;
  - its graph and store round trip (`build-domain/src/graph.rs`).
- `parsers/<domain>/<x>/`: the domain's readers (`parsers/build/bazel/`), beside `parsers/code/`.
- The root `Cargo.toml` members, and `stamp`'s hashed roots when the crate shapes a stored graph.
- **Ids from the private range.** An unshipped domain numbers in a private block, `0x8000`-`0xFFFE` (`glia_core::ids::PRIVATE`;
  `id_in(0x8000, 1)` is `0x8000_0001`), without asking. A shipped table never holds a private id: the release that
  merges the domain promotes its ids into a registered block (`core/src/ids.rs`'s table: `0x0000` code, `0x0001` core,
  `0x0002` build, `0x0003` goal reserved), adds its tables to `engine/tests/id_blocks.rs`'s `SHIPPED`, and appends its
  rows to `core/ids.lock`. A renumber from the private range is safe only because a private id never shipped.

**Checks**

- `bash scripts/pr-check.sh [--base <rev>]`, the one fast command: it runs, for the paths the change touches,
  the checks below and fmt / clippy / the touched crates' tests ([CONTRIBUTING.md](../CONTRIBUTING.md#before-you-open-a-pr)).
- `cargo test -p glia-engine --test engine_tests id_blocks::` (`[ids] blocks domains=<d> ..`): every shipped id in its
  domain's block, none in two domains, `core/ids.lock` equal to the shipped tables, the lock append-only.
- The domain crate's own round-trip test (`build-domain/tests/round_trip.rs`), and `toy-domain/`'s proof that the
  engine-free layers hold a second domain.

**What a release keeps:** an allocated id never moves (`core/ids.lock` only grows), a block's owner never changes, and
the private range is never shipped.

## What a release may still move

Everything outside the five seams: the passes and their order (`engine/src/profile.rs`'s `CODE_PASSES`), the resolvers,
the store format, the answers and their rows, the ranking. A pull request there is read as a design and rebuilt on the
release's tree, with the author credited `Co-authored-by` on the commits that use it.
