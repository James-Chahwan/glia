# Edge precision, 0.5.6 re-run (GP.14)

The 0.5.4 audit (FD.2 - FD.3d, report FD.3c, `dev-notes/edge-precision-0.5.4.md`) found 8
strata under floor. FD.73-FD.81 (0.5.4 W25 / W26), FD.76 (834bee60) and the 0.5.6 GD packets
fixed their mechanisms. This re-run measures them again on a fresh sample. Numbers come from
`bench/edge-audit/report.json` / `report.md`, derived by `bash scripts/edge-audit.sh report`
from the committed sample and verdicts (`report --check` exits 0). Every repo is public.

```
[edge-audit] report strata=27 judged=516 unsure=0 under_floor=1 insufficient=0 kappa=1.000 ci_method=stratified_wilson
```

## What moved

- **The self row.** `bench/edge-audit/snapshots.json` pins glia at
  ad5dd83d3aae3da209290dafe21a08d319558663 (0.5.6 wave 3 landed: after GP.13's stratified
  re-judge and Wilson CI, and after GP.15's rename of the private-repo-named rank_eval
  function). The other four rows stay pinned (shell 9616931a, websocket dcea2f08, samples
  291ba126, microservices-demo 38e7348e). The old rev's sample row that named the renamed
  function is gone: `git grep -c` of that name over `bench/edge-audit` is 0.
- **The sampler.** The `edge_audit` example was built from a `git archive` of that same
  commit (an isolated target dir), so the sample is HEAD's build, not a working tree with
  sibling packets' edits. Every tree was read from its existing snapshot copy; nothing was
  fetched.
- **The sample.** 516 samples (503 before): 360 ids kept their row and verdict, 143 dropped
  out (their verdicts left with them), 156 are fresh. Every pinned row's key is independent
  of the panel, so a kept id is the same edge. A glia id moves whenever its line does, so
  most of glia's ids are fresh.
- **Judging.** The 156 fresh ids were judged under `RUBRIC.md` by GP.14 (66 structure, 90
  mechanism): 148 correct, 8 wrong, 0 unsure.
- **The re-judge.** GP.13's matched rule (every first `wrong` of a family plus as many
  `correct`, at least 20) asked for 40 ids; 21 already had FD.3c's re-judge row, and GP.14r
  re-judged the other 19. Agreement 54 / 54, 0 disagreements, so `adjudication.jsonl` stays
  empty. Kappa is defined for both families now: mechanism 1.000 (expected agreement 0.528),
  structure 1.000 (0.835), overall 1.000.

### Blinding

- **Accidental unblinding = 0 ids.** No first verdict of an earlier judge was read before
  GP.14 judged or re-judged: the verdict files were read only for their category counts, the
  judge names and the line format (verdict and reason masked). The 0.5.4 write-up was read
  after the re-judge was written.
- **What the judge knew.** The packet text names the 8 under-floor strata with their 0.5.4 p
  and the C1 mechanism (pubspec `environment:`), and the judge read the old report's method
  and agreement paragraphs (not its strata or mechanism text) before judging.
- **16 of the 19 re-judged ids were GP.14's own first verdicts** (all 10 mechanism ids, 6
  structure ids). The same agent re-judged them from source, so those 16 pairs measure
  self-consistency, not independent agreement. The mechanism family's 8 wrong / wrong pairs
  are all of that kind. The structure family's 3 wrong / wrong pairs are blind: FD.3a / FD.3d
  judged them first, GP.14r re-judged them without reading those verdicts. The matched rule
  itself tells a re-judge its set is rich in wrong verdicts.

## The 0.5.4 under-floor strata, before and after

"Fresh" is the ids this re-draw added; "gone" means the 0.5.4 wrong edge is in no stratum
of HEAD's build: a census of every stratum of all five trees (`n_per_stratum` set past every
stratum, written to a scratch file, never committed) holds no edge with that wrong row's
(label, category, from qname, to qname), at any line, rule or tier. A stratum leaves the list
only when its p reaches the floor on fresh ids.

| stratum | floor | p 0.5.4 (N) | p 0.5.6 (N) | 95% CI | fresh ids: correct / judged | 0.5.4 wrong edges gone | left the list |
|---|---:|---:|---:|---|---:|---:|---|
| ACCESSES_DATA\|fact | 0.95 | 0.467 (15, census) | 1.000 (5, census) | [0.566, 1.000] | 0 / 0 (census) | 8 / 8 | yes: a census of HEAD's whole stratum |
| DEFINES_CONFIG\|fact | 0.95 | 0.667 (239) | 1.000 (166) | [0.886, 1.000] | 10 / 10 | 10 / 10 | yes |
| DEPENDS_ON\|fact | 0.95 | 0.933 (1316) | 1.000 (1228) | [0.886, 1.000] | 5 / 5 | 2 / 2 | yes |
| DOCUMENTS\|derived | 0.90 | 0.387 (893) | 0.742 (418) | [0.568, 0.863] | 21 / 29 (0.724) | 19 / 19 | **no** |
| HANDLED_BY\|fact | 0.95 | 0.833 (50) | 1.000 (59) | [0.886, 1.000] | 11 / 11 | 5 / 5 | yes |
| IMPLEMENTS\|derived | 0.90 | 0.800 (489) | 1.000 (321) | [0.886, 1.000] | 6 / 6 | 6 / 6 | yes |
| IMPLEMENTS\|fact | 0.95 | 0.933 (205) | 1.000 (540) | [0.886, 1.000] | 19 / 19 | 2 / 2 | yes |
| READS_CONFIG\|fact | 0.95 | 0.767 (162) | 1.000 (145) | [0.886, 1.000] | 11 / 11 | 7 / 7 | yes |

`report.json` `under_floor` is now `["DOCUMENTS|derived"]` (8 before). Per stratum:

- **ACCESSES_DATA|fact (A1, A2).** The stratum is a census, so the fresh-id rule reads the
  whole population: HEAD's build writes 5 ACCESSES_DATA|fact edges (3 microservices-demo, 2
  samples), all correct, and none of the 15 of 0.5.4. A1's 5 samples edges (JNI descriptors,
  `Redistribution`) are gone on a pinned tree, so the fix is FD.73's; A2's 3 glia edges are
  gone while their sites (`CREATE TABLE orders` in `engine/tests/parallel_build.rs`, the
  `query_gql` strings) are still in the tree, so the fix is FD.74's. Closed. The census
  is small (5 rows, Wilson [0.566, 1.000]); a later panel with a database-heavy repo would
  measure the stratum better than this one can.
- **DEFINES_CONFIG|fact (C1).** All 10 pubspec `environment:` rows are gone on the pinned
  samples tree (FD.76): no DEFINES_CONFIG|fact verdict is wrong, and no sampled row is a
  pubspec `environment:` key. N fell 239 -> 166. Closed.
- **DEPENDS_ON|fact (M1).** The two Cargo `[[test]]` `name =` rows are gone while glia's
  manifests still hold `[[test]]` tables (8 in engine/Cargo.toml), so the manifest reader's
  target-table fix (FD.77, GD.37) holds. Closed.
- **DOCUMENTS|derived (D1, D2, D3).** All 19 wrong edges of 0.5.4 are gone, and N fell
  893 -> 418 (FD.79's doclink precision and FD.78's single symlink walk). On a pinned tree
  that is the fix (4 of them: samples 2, microservices-demo 1, shell 1); on glia the docs
  were also rewritten since 0df1bbbb (FZ.1), so the 15 glia ones are not all a fix's.
  p rose 0.387 -> 0.742, but 8 of the 29 fresh ids are wrong: the stratum stays under floor,
  with new clusters (below).
- **HANDLED_BY|fact (H1, H2).** The 4 `anchor` rows to the registering function and the
  unregistered health `Check` are gone on the pinned microservices-demo tree (FD.80); the
  fresh rows bind the impl (`DummyEmailService.SendOrderConfirmation`,
  `HipsterShopServer.ChargeServiceHandler`). Closed.
- **IMPLEMENTS|derived (G1).** The 6 cross-module Go pairs are gone on the pinned tree
  (FD.81); the 6 fresh genproto pairs are inside one module. Closed.
- **IMPLEMENTS|fact (R1).** Both MODULE -IMPLEMENTS-> rows are gone while `impl PathGraph
  for MergedGraph` and the blanket `impl<T: HostScoped> HostScoped for &T` are still in the
  tree (FD.11); the fresh sample holds the method-level pair `MergedGraph::levels ->
  PathGraph::levels` instead. Closed.
- **READS_CONFIG|fact (E1, E2).** The 5 `os.environ[X] = ..` writes and the 2 vault path
  literals are gone while py/tests still writes `os.environ[..]` (FD.75). Closed.

Per tier: derived 0.599 -> 0.894 [0.818, 0.941] (still below 0.90 because of DOCUMENTS),
fact 0.998 -> 1.000 [0.850, 1.000], heuristic 0.912 -> 0.913 [0.792, 0.967]. The fact tier's
interval is the stratified Wilson's (GP.13), no longer the plug-in's.

## The 0.5.4 strata at floor that f4p's packets fix

| cluster | stratum | packet | at ad5dd83d | evidence |
|---|---|---|---|---|
| I1 | IMPORTS\|heuristic, `tail_unique` kind-blind | GD.34 (NEW.f4p.5) | **open** (GD.34 not landed) | all 3 wrong edges are still sampled at the same key: Go `"encoding/json"` -> `json.go`, Java `java.util.ArrayList` -> an EVENT_EMITTER, C# `using static Grpc.Health.V1.Health` -> the GRPC_SERVER; p 0.900 (floor 0.70) |
| I2 | IMPORTS\|fact, a `use` in an inline `mod` | GD.35 (NEW.f4p.6, aa4dde8d) | closed | the code-domain `node_kind` row is gone; HEAD writes `code-domain::src::lib::endpoint -> ..::node_kind` from the inline mod; IMPORTS\|fact 31 / 31 |
| T1 | TESTS\|derived, `name_match` across packages | GD.36 (NEW.f4p.7, 4844ddfe) | closed | websocket's root `server_test` -> `examples::echo::server` and the flutter drive host pair are gone; TESTS\|derived 30 / 30 |
| R1 | IMPLEMENTS\|fact, a non-local self type | FD.11 | closed | above |

## New wrong clusters (DOCUMENTS|derived, 8 of 29 fresh, all doclink rule `unique`)

FD.79 now refuses a lone dictionary word; the 8 wrong rows are identifier-shaped tokens that
name something other than the symbol they bind. Each is a follow-up for the precision
backlog with a public fixture:

- **D4. A backticked token that names a key, a rule or a config kind (5, glia).** A dict key
  (`name_breaks` in the Python 0.5.4 notes; `empty_scopes` in the overlay try report), a cell
  key (`abi_ref` in the EVM hit cell), an evidence rule name (`receiver_return`, a TS CALLS
  rule, bound to C++ `CppScope::receiver_return`) and an overlay `[[constraint]]` kind
  (`forbid_edge`, bound to check.rs's private fn) each bind the one function of that name.
  Follow-up: **"doclink: a token in a rule / key / kind position is no symbol mention: after
  `rule`, `key`, `kind`, `gains`, a `[[..]]` stanza list or inside a JSON-shaped `{..}` span,
  and never a private (non-`pub`) fn from a doc outside its crate"** (`engine/src/passes.rs`).
  Fixture: a README with "rule `receiver_return`", "the dict gains `name_breaks`" and
  "`[[constraint]]` kinds: `forbid_edge`" beside a Rust crate defining `pub fn
  name_breaks`, `fn receiver_return` and `fn forbid_edge`. Forbid those three DOCUMENTS
  edges; expect DOCUMENTS to a `pub fn` the README describes by a qualified path.
- **D5. A Python API doc binds a same-named Rust symbol (2, glia).** py/README.md's
  `review_vs_rev` is the pyo3 function `review_vs_rev_py` (renamed by `#[pyo3(name =
  ..)]`), and its `resolve_signal` is a removed Python method; both bind the engine / graph
  Rust symbol. Follow-up: **"doclink: a doc under a pyo3 crate binds the crate's Python-visible
  names (`#[pyo3(name)]`, `#[pyfunction]`) first and never a Rust symbol of another crate by
  a bare name"** (`engine/src/passes.rs`, the pyo3 name read in `parsers/code/rust/src/lib.rs`).
  Fixture: `pkg/README.md` naming `do_thing` beside `pkg/src/lib.rs` with `#[pyfunction]
  #[pyo3(name = "do_thing")] fn do_thing_py` and `core/src/lib.rs` with `pub fn do_thing`.
  Expect DOCUMENTS -> `do_thing_py`; forbid -> `core::do_thing`.
- **D6. A key or UI word in a code span (1, shell).** "`Alt` + `Tab`" binds the TS `interface
  Tab`. Follow-up: **"doclink: a code span joined to another by `+` (a key chord) names a
  key, never a symbol"** (`engine/src/passes.rs`). Fixture: a README section "switch with
  `Alt` + `Tab`" beside `interface Tab {}` in src/stack.ts. Forbid that DOCUMENTS edge.

No fresh id outside DOCUMENTS|derived is wrong. IMPORTS|heuristic's 3 wrong verdicts are
I1's, all kept ids.

## Limits

- The panel is the 0.5.4 panel: one cross-service repo, no database-heavy or config-heavy
  service repo, so ACCESSES_DATA (5 edges), SCHEDULES (1) and HTTP_CALLS (7) are censuses
  too small to say much.
- The 15 glia DOCUMENTS edges that left did so while the docs they came from were rewritten;
  the pinned rows are the cleaner evidence for FD.79 / FD.78.
- One judging procedure (the same model family under RUBRIC.md) ran both passes, and 16 of
  the 19 new re-judge pairs are one agent agreeing with itself.
