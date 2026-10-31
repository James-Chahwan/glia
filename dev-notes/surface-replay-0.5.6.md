# Surface replay (GA.1): sizing the body-only watch fast path

GA.1 is the measured experiment (R2-watch section 12, spike S4) that decides which languages get
a body-only fast-path packet in 0.5.6 (FA.3 the seam, FA.5 Rust, FA.6 TypeScript family, FA.7 Go)
and whether FA.24 (copy-free reuse of unchanged groups) is built. The pass rules below were
written down before any number was read. Nothing here changes a build or an answer.

## The model

Not literal github/stack-graphs (archived, and it would replace every builder with a per-language
TSG DSL). Per file, a PARTIAL is:

- its SURFACE: the declarations (node ids, kinds, qnames, nav parents and children), the imports,
  the heritage refs and in-file heritage edges, field and return types, method signatures,
  properties, MODULE-scope local types (package / module-level variables) and the surface
  NavFacts (`glia_engine::cache::surface_class`); and
- its BODY stubs: call sites, non-heritage refs, fn-scope local types, the body NavFacts and the
  node cells.

STITCHING is the builder's per-file resolution of the body stubs against the group's surface
index (the SymbolTable plus the language index: RustIndex, GoPackages, ...). A body-only edit
leaves every other file's stitching valid, so a fast path re-stitches only the edited files.

`glia_engine::cache::surface_fingerprint` hashes the surface without any row;
`surface_fingerprint_strict` adds the import, heritage and fact rows and every node's POSITION
cell (the declaration rows). Rows are out of the loose surface because an IMPORTS / INHERITS_FROM
edge's EVIDENCE row is the edited file's own output, re-derived when that file is re-stitched,
and no other file's resolution reads it; every body edit shifts the rows below it. A fast path
that cannot re-derive line-carrying outputs would need the strict one.

An edit is body-only (loose) when the loose surface is equal before and after and the file holds
no body fact another file's resolution reads: a Go `MountArg` flows into the callee's parameter
through the caller's own CALLS edge and re-keys the callee's ROUTEs, wherever they live.

## Method

`engine/examples/surface_replay.rs`, release build. For the last 200 first-parent commits of each
repo at its pin, every modified (`git diff --name-status -M`, status `M`) source file the walk
reads (no hard-skipped or always-collapsed directory: `.git`, `node_modules`, `vendor`, `build`,
`dist`, `target`, ..) is parsed at `<c>^` and `<c>` with `glia_engine::parse_one` and
fingerprinted both ways. Then, on a `git archive` copy of the pin, a `WatchSession` times three
body-only updates per build group with edits (one comment line inserted at the top of the
group's most edited file, removed, inserted again), keeping the updates whose `groups_rebuilt`
is exactly that group, and takes the median of every `phase_ms` field. Each repo runs twice: at
the shipped default (every core) and at `GLIA_THREADS=1`.

`parse_one` is the pre-graft parse. The post-cache grafts (const folds, RPC needles, the EVM
ABI join, Angular templates, WS bases) can add nodes or cells to a parse after it, so the
body-only share is an upper bound for a file a graft touches; the grafts already re-run on every
update (the `grafts` phase), outside the group build a fast path would cut.

PARSER_STAMP: the binary was built from HEAD `cb9334e5` plus GA.1's two files (an isolated
`git archive` tree), so the parsers are 0.5.5's.

## Pass rules (fixed before the run)

- **Fast path, per language.** A language PASSES when, on at least one measured repo, in the
  run at the shipped default thread count: edits >= 30, body_only_loose >= 0.40, group_ms
  (`phase_ms.language_build`, release build) >= 250, build_share = group_ms / update_ms >= 0.25,
  and both controls hold. The `GLIA_THREADS=1` run is reported beside it and decides nothing
  (it overstates the pooled TS / Python group, FH.21).
- **FA.24 (the clone cut).** PASSES when clone_ms >= 50 and clone_ms >= 0.20 x update_ms on at
  least one measured repo (default threads).
- **Packets.** FA.3 (Python, the per-file stitching seam the others reuse) is built when Python
  passes OR any other language passes; FA.5 Rust, FA.6 TypeScript family and FA.7 Go each only on
  their own PASS; FA.24 only on its clone rule's PASS. The passing languages are ordered by
  saving_ms = body_only_loose x group_ms descending (an UPPER bound: the fast path still
  stitches the edited files and re-fills the group's evidence), ties by language name.

## Controls

- **Null control.** Per commit, up to 20 unmodified source files (walked, not in the commit's
  diff) are parsed twice and fingerprinted both ways; any inequality fails the run and no
  verdict is printed.
- **Positive control.** Every modified file whose loose surface held although its diff adds or
  removes a top-level (column 0) declaration line (`def` / `class` / `fn` / `func` / `function`
  / `interface` / `struct` / `impl` / `trait` / `type`, after `export` / `pub` / `async` / ..
  modifiers) is listed with an explanation: `moved` (the same declaration lines removed and
  re-added), `no_node` (the declared name is no node's name before or after: the parser mints
  nothing a resolution could read), `same_name` (every changed declaration keeps its name; what
  changed is parameters, annotations or formatting the language's surface does not record), or
  `unexplained`, which fails the controls for that language.

## Runs

Release `surface_replay` from HEAD `cb9334e5` + GA.1, 2026-10-31, 16 cores, sibling agents running
(load average 7-25, recorded per row: the same phase moved up to 2x between quiet and loaded runs
in the spec pass's probe, so treat a row near a bar as near it). 200 first-parent commits each:

| repo | pin | language groups with edits | edits outside the walk |
|---|---|---|---|
| twenty | `8fd53a835fd5` | typescript | 5 |
| photoprism | `70e20b5e033f` | go, typescript, python | 4 |
| prefect | `fa93ba256927` | python, typescript | 0 |
| glia | `v0.5.5` (`70e4d08b6e5b`) | rust, python | 17 |

pop-os/shell, gorilla/websocket and flask were dropped (ruling: their groups build far under the
250 ms bar); grpc-go was not needed.

## Controls

| repo | null control (files parsed twice) | null failures | positive-control hits | explained as | unexplained |
|---|---|---|---|---|---|
| twenty | 4000 | 0 | 12 | same_name 8, no_node 4 | 0 |
| photoprism | 4000 | 0 | 14 | same_name 14 | 0 |
| prefect | 4000 | 0 | 3 | same_name 1, no_node 2 | 0 |
| glia | 4000 | 0 | 3 | same_name 3 | 0 |

Both controls hold on every row (`controls=ok`). What the explained hits are: a Go FUNCTION whose
result list grew (`*T` -> `(*T, error)`: the first result, the one return type a resolution
reads, kept; Go records a signature only for a METHOD, and the implicit-IMPLEMENTS compare reads
only those), Go / Python / JS parameter lists grown or renamed, a Rust `fn` -> `pub(crate) fn`
(no resolution reads visibility), TS object-type aliases whose fields changed (an INTERFACE node's
SCHEMA_FIELDS cell: read by the models / contract passes in the update's tail, which re-run every
update, never by a group's stitching), and TS union type aliases that mint no node (`no_node`).

## Results

Default threads (the shipped configuration; the verdict reads this run). saving_ms =
body_only_loose x group_ms, projected_update_ms = update_ms - saving_ms (the floor with the fast
path). tail = resolve + post + finalize. All times are medians of 3 updates, ms.

| repo | lang | edits | body_only loose | strict | cross-dep edits | group_ms | grafts | tail | delta | clone_ms | update_ms | build_share | saving_ms | projected update | load | controls | verdict | FA.24 clone |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| twenty | typescript | 1937 | 0.458 | 0.200 | 0 | 3132 | 1719 | 2588 | 1068 | 495 | 10784 | 0.290 | 1436 | 9348 | 10.63 | ok | **PASS** | FAIL |
| photoprism | go | 1548 | 0.463 | 0.207 | 16 | 3722 | 256 | 254 | 231 | 141 | 4952 | 0.751 | 1721 | 3231 | 11.51 | ok | **PASS** | FAIL |
| photoprism | python | 1 | 0.000 | 0.000 | 0 | 71 | 469 | 450 | 49 | 187 | 1933 | 0.037 | 0 | 1933 | 22.52 | ok | FAIL | FAIL |
| photoprism | typescript | 178 | 0.539 | 0.270 | 0 | 138 | 661 | 498 | 123 | 246 | 2197 | 0.063 | 74 | 2123 | 24.64 | ok | FAIL | FAIL |
| prefect | python | 345 | 0.426 | 0.183 | 0 | 1447 | 1131 | 462 | 225 | 158 | 3718 | 0.389 | 616 | 3102 | 6.98 | ok | **PASS** | FAIL |
| prefect | typescript | 169 | 0.278 | 0.083 | 0 | 95 | 1128 | 514 | 73 | 134 | 2340 | 0.041 | 26 | 2314 | 8.33 | ok | FAIL | FAIL |
| glia | python | 127 | 0.693 | 0.268 | 0 | 58 | 133 | 551 | 52 | 150 | 1249 | 0.046 | 40 | 1209 | 10.66 | ok | FAIL | FAIL |
| glia | rust | 424 | 0.316 | 0.125 | 0 | 814 | 183 | 451 | 228 | 163 | 2143 | 0.380 | 257 | 1885 | 11.18 | ok | FAIL | FAIL |

`GLIA_THREADS=1` (reported, decides nothing):

| repo | lang | group_ms | grafts | tail | delta | clone_ms | update_ms | build_share | saving_ms | load | rule at 1 thread |
|---|---|---|---|---|---|---|---|---|---|---|---|
| twenty | typescript | 3397 | 1988 | 3128 | 1004 | 629 | 13954 | 0.243 | 1558 | 12.51 | FAIL (build_share) |
| photoprism | go | 4123 | 254 | 270 | 248 | 134 | 5501 | 0.750 | 1907 | 15.92 | PASS |
| photoprism | python | 60 | 246 | 256 | 32 | 116 | 1052 | 0.057 | 0 | 13.78 | FAIL |
| photoprism | typescript | 96 | 247 | 262 | 40 | 110 | 1125 | 0.085 | 52 | 13.78 | FAIL |
| prefect | python | 1933 | 1310 | 784 | 321 | 172 | 5026 | 0.385 | 824 | 12.69 | PASS |
| prefect | typescript | 193 | 1154 | 693 | 62 | 137 | 2919 | 0.066 | 54 | 11.92 | FAIL |
| glia | python | 99 | 194 | 447 | 48 | 152 | 1394 | 0.071 | 69 | 13.36 | FAIL |
| glia | rust | 985 | 204 | 421 | 250 | 160 | 2657 | 0.371 | 311 | 12.09 | FAIL (body_only) |

The body-only shares are identical in both runs (the replay does not depend on threads).

### What moves the loose surface (diagnostic, not a verdict input)

Per edit whose loose surface moved, the parts that moved (`surface_moved_by` in the JSON; an edit
counts once per part). The commonest single cause is the import list:

| repo | lang | edits | loose equal | surface moved by imports ONLY | by any import change |
|---|---|---|---|---|---|
| twenty | typescript | 1937 | 888 | 554 (0.286) | 879 |
| photoprism | go | 1548 | 722 | 212 (0.137) | 524 |
| prefect | python | 345 | 147 | 31 (0.090) | 128 |
| glia | rust | 424 | 134 | 68 (0.160) | 160 |

The rest is new or removed declarations (`nodes:+FUNCTION` / `+METHOD` / `+STATE_VAR` with their
`nav_tree`), Go method signatures and return types, TS module-level local types and re-export
facts. One photoprism Python edit moved only a class's `children_of` order (a `self.assertIn`
first used earlier in a body reorders the class's attribute children); kept as surface, since a
by-name lookup that takes the first of two same-named children reads that order.

Imports stay surface in this measurement, as the spec defines it, because other files do read
them: Go's call hook types a `pkg.T` field through the declaring struct's file imports (DT.5j),
a TS barrel's `export { x }` of an import (`ExportLocal`) resolves through the barrel's imports,
Python resolves `from m import b` through m's own imports, and Rust's paths read `pub use`. A
fast-path packet that narrows the import surface to what is read across files (Rust `pub use`,
TS re-exported locals, Go files declaring qualified field types, Python every import) would move
the imports-only edits into the body-only share: up to +0.286 TS, +0.160 Rust, +0.137 Go,
+0.090 Python. That is a sensitivity, not a verdict: Rust stays FAIL under the committed rule.

Two caveats on the shares. They are UPPER bounds for any file a post-cache graft adds nodes to
(`parse_one` is the pre-graft parse); the grafts re-run every update anyway. And the projected
saving is an upper bound too: the fast path still stitches the edited files and re-fills the
group's evidence.

## Verdicts

- **Go: PASS** (photoprism: 1548 edits, body-only 0.463, group 3722 ms, build_share 0.751).
- **TypeScript family: PASS** (twenty: 1937 edits, body-only 0.458, group 3132 ms, build_share
  0.290 at default threads; 0.243 at `GLIA_THREADS=1`, under the bar, so this pass is the
  narrowest of the three and rests on the shipped thread default).
- **Python: PASS** (prefect: 345 edits, body-only 0.426, group 1447 ms, build_share 0.389).
- **Rust: FAIL** (glia: body-only 0.316 < 0.40; group 814 ms and build_share 0.380 would pass).
- **FA.24 (clone cut): FAIL.** The largest clone share is 0.120 (glia python, 150 / 1249 ms;
  twenty's 495 ms is 0.046 of its update): no row reaches clone_ms >= 0.20 x update_ms.

## The packets this report fixes

Ordered by saving_ms (default threads), descending:

1. **FA.7 Go** (photoprism saving <= 1721 ms of 4952). Read its row again after NEW.t8.1 (the
   pooled Go call hook) lands: that packet alone may cut the group, and with it the saving.
2. **FA.6 TypeScript family** (twenty saving <= 1436 ms of 10784; its update is dominated by the
   tail 2588, grafts 1719 and delta 1068, which the fast path does not touch).
3. **FA.3 Python** (prefect saving <= 616 ms of 3718), the per-file stitching seam; built first,
   as the others reuse it, though its own saving is the smallest of the three.

Not built: **FA.5 Rust** (its own FAIL) and **FA.24** (its clone rule's FAIL); each reports
not-needed quoting its verdict line above.

## Artefacts

The per-run JSON (rows, controls, every positive-control hit, every edit with the surface parts
it moved) lives outside the repo, under the packet's scratch directory; only these aggregate
numbers are committed. Reproduce one row:

```
cargo build --release -p glia-engine --example surface_replay
git -C ~/.cache/glia-cohort/prefect.git rev-parse fa93ba2569274e61a5dd9d39c5b91824475a8521
target/release/examples/surface_replay --repo ~/.cache/glia-cohort/prefect.git --label prefect \
  --rev fa93ba2569274e61a5dd9d39c5b91824475a8521 --commits 200 --work <scratch> --out <json>
```
