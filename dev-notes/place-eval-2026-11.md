# Placement, measured (GB.4, item 14a of next-leap-0.5.6.md)

The question (§F item 14): the rankings can only name existing files, and a change often adds
files. On held-out commits that add files, can a ranker name the directory a new file lands in, and
the existing file it was patterned on, better than plain controls? If one does on the gate repos,
it ships as an answer in a follow-up wave of this leap (FC.22); if not, this report closes item 14.

Harness: `engine/examples/rank_eval.rs`, `--place on` (per repo; needs `--tune on --pick
base,d85`) and `--place-judge <run.json>,..` (the cross-repo rule). Repos: the six public rank-gate
snapshots of `bench/rank-gate/snapshots.json` (glia's pinned v0.5.2, pop-os/shell,
gorilla/websocket, flutter/samples, apache/commons-lang, apache/commons-io). No private run feeds
this report.

## Method

- **Query set.** rank_eval's leak-free test commits (the newest 300 test-eligible commits; the
  eligibility rule is unchanged) whose `git log --name-status --no-renames` lists at least one
  added (A) file with a candidate extension (an extension some candidate file of the snapshot has;
  extension-less files are out). Renames are not additions. A root commit has no parent and is
  skipped (counted).
- **Seeds and shapes.** Seeds are the commit's modified (M) candidate files that the snapshot
  holds, ordered by the replay's seed hash: k1 (the first), k2 (the first two, when it modifies at
  least two), half (the first half, when it modifies at least four). A commit modifying none is
  the find shape: its subject's text alone. One query per commit and shape.
- **Universe.** The directories of the parent revision's candidate-extension files (`git ls-tree`
  at the first parent), each scored on its own files only: a file's score counts toward its
  IMMEDIATE directory, never its ancestors (summing every level would put the root and `src/`
  above every leaf by construction).
- **Directory truth.** The immediate directory of each added file. A directory with no
  parent-revision candidate file is new: the file is scored on its nearest ancestor that has one,
  and the commit is counted `new_dir` (kept in the pooled numbers).
- **Sibling truth.** Per added file, the parent-revision candidate file with the highest
  line-set Jaccard (trimmed non-empty lines) to the new file's content at the commit; ties go to
  a file in the new file's own directory, then path order. No sibling when the best Jaccard is
  below 0.2 (counted).
- **Directory rankers** (file scores summed per immediate directory; ties to the directory path):
  `Dstr:P1` (P1: undirected PPR at damping 0.5 from the seeds' nodes, max-pooled per file),
  `Dstr:PT*` (the picked base,d85 PPR, sum-pooled), `Dco` (C0: the seeds' co-change over strictly
  older commits), `Dtxt` (T1s: BM25 of the subject over strictly older commits' messages), `Dmm`
  (the mean of the min-maxed Dstr:P1, Dco and Dtxt directory scores) and `Dsel` (r3_select's online
  pick among those five, per shape, on dir hit@3 summed over every earlier test commit).
- **Controls.** `Cseed`: the seeds' directories by seed count, then depth (deepest first), then
  the rest by file count (the find shape has no seed, so Cseed is Cpop there). `Cpop`: directories
  by candidate file count. `Crand`: a seeded random order (hash of commit and directory).
- **Sibling rankers.** Each directory ranker's top-3 directories' files, ordered by the same
  file scores (Dmm: the mean of the min-maxed P1, C0 and T1s file scores), the first 5. Control
  `Csib`: the seeds' directories' files (every file for the find shape), most recently modified
  first (the last commit before the test commit that added or modified it), the first 5.
- **Metrics.** Dir hit@1 / hit@3 (any truth directory in the top k), sibling recall@5 (the share
  of a query's sibled added files whose sibling is in the top 5; over the queries with a sibling),
  per repo and shape and all shapes pooled, each candidate's paired difference to the control
  with its commit-cluster bootstrap 95% CI (1000 draws, rank_eval's `paired`).
- **Leakage.** Co-change and BM25 read only commits strictly older than the test commit. The
  graph is the snapshot's (`--rev`), as for every rank_eval ranker: it already holds the added
  file and anything added later, so PPR may route through a node the parent did not have. That
  favours `Dstr`, never the controls.

## Pass rule

Pass rule (GB.4, committed before the run). Query set: rank_eval's leak-free test commits
(eligibility unchanged) that add at least one candidate-extension file; one query per commit and
seed shape (k1 / k2 / half over the commit's modified candidate files, the find shape when it
modifies none). Directory truth: the immediate directory of each added file, or its nearest
ancestor holding a parent-revision candidate file when the directory is new (`new_dir`). The
directory answer PASSES when one non-control ranker R (the same R on every repo) beats the best
control (Cseed unless Cpop has the higher all-shape mean on that repo) on dir hit@3, all shapes
pooled, with the commit-cluster bootstrap 95% CI of R minus the control above 0 on at least 4 of
the 6 public gate repos, and its CI is not wholly below 0 on any. The sibling answer PASSES under
the same rule on sibling recall@5 against Csib, over the queries with a sibling. A repo with fewer
than 30 added-file commits is `thin` and counts as neither. Verdict line: `Placement: PASS
(<ranker>[, sibling])` / `Placement: FAIL (<reason>)`.

(The 4-of-6 threshold is the RE-VERIFY f7f9a495 correction to the spec's 3-of-4; the rule text is
`PLACE_RULE` in rank_eval.rs, printed above every table the harness writes.)

## Run

For each snapshot `<label>@<rev12>` of the rank gate (`bash scripts/rank-gate.sh fetch`'s copies):

```text
GLIA_NO_PERSIST=1 target/release/examples/rank_eval --repo $GLIA_RANK_GATE_DIR/<label>@<rev12> \
    --git $GLIA_RANK_GATE_DIR/<label>.git --rev <rev> --label <label> <snapshot flags> \
    $(bash scripts/rank-gate.sh flags) --place on --place-md <out>/<label>.place.md --out <out>/<label>.json
target/release/examples/rank_eval --place-judge <out>/glia.json,<out>/shell.json,.. --place-md <out>/place.md
```

Results: pending the run (this section is filled from the judge's output in the next commit).
