# Docs as a second graph: the doc-tie pass rule (GP.9, committed before the run)

GP.9 measures one question: can a query-time doc-tie relation (a doc file and the code files it
co-changes with) recover the glia rank-gate rows FD.79 lowered, when it is tried as a one-hop bridge in
rank_eval's ANS:impact? This file fixes the relation, the bridge, the rankers, the run and the pass rule
BEFORE any measurement. `report.md` beside it records the one run made under this rule. The run is never
repeated to get a different verdict, and the rule is never widened after the run.

## Why

FD.79 (d22aeaad) refused lone-word, registry and placeholder doc mentions: glia's own DOCUMENTS edges fell
830 -> 344. DOCUMENTS is a carry edge (`CODE_TABLES.carry_edges`), and ANS:impact scores diff impact's rows,
which only the carry walk reaches, so a doc section bound to many symbols had joined code files that
co-change. Only the glia snapshot dropped: ANS:impact r10 0.2594 -> 0.2550, mrr 0.4614 -> 0.4566 (single
shape), recorded as gate.json `accepted` rows (2aa2dff4). No reader joins docs and code through history
today: a doc is not a rank candidate (`rank::files::NOT_CODE_EXTS`), the rank co-change index maps
candidates only, and the history stage counts doc paths `unmapped`.

Probe at the gate's glia rev (`git log --no-merges --no-renames -n 2000 9837b1ff`, re-run by GP.9): 1180
commits, 1010 with 1..30 code files, 207 of them also change a doc; under the floors below 13 docs hold 45
doc <-> code ties (docs/overlay.md 10, bench/substrate-gap/COVERAGE.md 8, bench/substrate-gap/README.md 7,
CLAUDE.md 4, ..). FD.79 removed 486 name links, so whether 45 ties recover the drop is unknown.

## The relation

- **A doc** is a path whose extension (rank_eval's `ext_of`) is `md`, `mdx`, `markdown` or `rst` and that
  holds at least one DOC_SECTION node of the build (located through its POSITION file).
- **Training commits** are the replay's own: a non-merge commit (`--no-renames`) whose candidate files
  number 1..=30 (rank_eval's `train_ok`, the co-change prefix's rule). A commit's docs are the doc paths
  its `--name-only` list names; a commit with no candidate file trains nothing, whatever docs it changes.
- **At test commit c**, over the training commits strictly older than c (the same leak-free prefix
  ANS:impact's co-change reads): `n(D)` = training commits that change doc D, `n(f)` = training commits
  that change candidate f, `n(D, f)` = training commits that change both.
- **A tie (D, f)** holds at c when `n(D, f) >= 3` and `1000 * n(D, f) / min(n(D), n(f)) >= 300`
  (integer division): LF.5b's floors (`external::history::MIN_SUPPORT`, `MIN_RATIO_PERMILLE`), not tuned.
- **A hub** is a doc tied to more than 30 files at c. A hub bridges nothing and is counted `hub`.

## The bridge

For a seeded query (shapes single, k2, k3, half) with seed files S at test commit c:

1. `docs(S)` = every non-hub doc tied at c to some s in S.
2. `B` = every file tied at c to a doc of `docs(S)`, minus S.
3. A file of B the walk already reached keeps its place: it is in diff impact's rows once, where the walk
   and the ranking put it. A file of B the walk did not reach enters as a row appended after the walk's
   rows, the appended files in the answer's own ranking order (their order in `BlastRadius::files` with
   `top_files = usize::MAX`, which ranks every candidate but the seeds).

Nothing else moves: a query whose B is empty, or whose B the walk reached entirely, scores exactly as
ANS:impact.

## The two rankers measured

Both are added to rank_eval under `--ans on` as late rows, after every existing row of every shape (after
the ENG-* rows and EG.6 / EG.7's late rows), so no existing row moves:

- **`ANS:impact+doctie`**: ANS:impact's file order (its rows rolled up to files by first appearance,
  `rows_to_keys`), then the unreached files of B in the ranking's order.
- **`ANS:impact.files+doctie`**: ANS:impact.files' order (the file roll-up) with the unreached files of B
  lifted to just after the last file that holds a row: the roll-up up to and including the last file with
  `rows > 0`, then the files of B below that point in roll-up order, then the rest in roll-up order.

Fired_on, stderr, once per run: `[doc-ties] label=<l> docs=<d> ties=<t> hubs=<h> bridged_queries=<q>`
(docs in the build; distinct (D, f) ties over every test commit; distinct docs that are a hub at some test
commit; seeded queries whose B is non-empty), followed by ` appended_queries=<a>` (queries where B held a
file the walk did not reach).

## The run

- rank_eval built in release from one committed glia tree (a `git archive` of HEAD under the packet's
  cache dir, so a sibling's working-tree change cannot reach either build), once without the two rankers
  ("before", HEAD's rank_eval.rs) and once with them ("after").
- The six snapshots of `bench/rank-gate/snapshots.json` at their pinned revs (the gate's `git archive`
  copies and bare clones), each with its own `flags` plus the gate's COMMON flags (`scripts/rank-gate.sh
  flags`) and `--engine-parity on`. No `--sem`: the find shape is not judged here.
- Before and after must agree on every existing row of every shape, bit for bit (`perq` / `perq_shapes`),
  and `--gate-check` of after against before (the after run trimmed by the gate's gate.json) reports 0
  drift and 0 changed rows; every `[rank-parity]` line reads `key_mismatch=0 order_mismatch=0`.

## The judge and the pass rule

`rank_eval --doc-ties-judge <six after-run JSONs>`, per snapshot, the gate's own test (`--gate-check`'s
rule, EG-2): per seeded shape (single, k2, k3, half) and metric (r10, mrr), new = `ANS:impact+doctie`, base
= `ANS:impact` of the same run, the paired commit-cluster bootstrap (B = 1000) of new - base; the rows
whose values differ form one family per direction, each row's one-sided p is the share of bootstrap means
on the wrong side of 0, Holm at family alpha 0.05 per snapshot, and a rejected row with |new - base| below
1e-4 is `same`. `ANS:impact.files+doctie` against `ANS:impact.files` is judged the same way and reported,
never part of the verdict.

**PASS** if and only if all three hold:

1. all six snapshots were judged;
2. on glia, shape single, `ANS:impact+doctie`'s mean r10 >= 0.2594 and mean mrr >= 0.4614 (the 0.5.4
   pre-FD.79 values; exact means compared, not rounded ones);
3. no `ANS:impact+doctie` row is a `drop` against `ANS:impact` on any of the six snapshots.

Otherwise **FAIL**. The verdict line, in report.md and on stderr:
`[doc-ties] verdict=<PASS|FAIL> glia r10=<x> mrr=<y> drops=<n> snapshots=<s>`.

On PASS, GP.10 (NEW.f4o.9b) builds the relation in the engine. On FAIL, GP.10 returns not-needed, the
gate's four `accepted` rows stay, and James rules between keeping them and option B (section-level ties
from a history snapshot that records hunks, a snapshot version bump).
