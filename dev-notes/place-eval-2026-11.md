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

The run (2026-10-31): the binary built from 7188bb9a (the pre-registration commit) on a
git-archive copy of HEAD 12375a67, the six snapshots with the rank gate's COMMON flags (`--round1
on --tune on --pick base,d85 --round2 on --round3 on --r3-perq on --ans on --max-commits 300
--max-seeds 5 --boot 1000`, glia also `--pre glia`; no `--sem`: the place rankers read no
vectors) plus `--place on`. Every run exited 0 (glia 837 s, samples 307 s, commons-lang 166 s,
shell 125 s, commons-io 115 s, websocket 27 s; the place step itself 0.3-10.6 s). The `[place-eval]`
lines:

```text
[place-eval] repo=glia queries=229 with_sibling=117 best=Dmm dir_hit3=0.4774 control=Cseed verdict=FAIL control_hit3=0.4434 ci=[-0.0129,0.0752] shape_queries=442 ms=10628
[place-eval] repo=shell queries=29 with_sibling=9 best=Dstr:P1 dir_hit3=1.0000 control=Cseed verdict=thin control_hit3=1.0000 ci=[0.0000,0.0000] shape_queries=70 ms=344
[place-eval] repo=websocket queries=15 with_sibling=7 best=Dstr:P1 dir_hit3=1.0000 control=Cseed verdict=thin control_hit3=1.0000 ci=[0.0000,0.0000] shape_queries=23 ms=345
[place-eval] repo=samples queries=117 with_sibling=97 best=Dmm dir_hit3=0.3757 control=Cseed verdict=PASS control_hit3=0.2431 ci=[0.0773,0.1910] shape_queries=181 ms=4750
[place-eval] repo=commons-lang queries=24 with_sibling=19 best=Dstr:P1 dir_hit3=0.8444 control=Cseed verdict=thin control_hit3=0.8000 ci=[-0.0638,0.1600] shape_queries=45 ms=1444
[place-eval] repo=commons-io queries=54 with_sibling=48 best=Dmm dir_hit3=0.8193 control=Cseed verdict=PASS control_hit3=0.7349 ci=[0.0222,0.1600] shape_queries=83 ms=781
```

## Results

The tables below are the judge's output (`--place-judge`), verbatim.

### Coverage

| repo | test commits | add a candidate file | k1 | k2 | half | find | new_dir | unanchored | added files | with a sibling | below 0.2 | dir control | thin |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glia | 300 | 229 | 203 | 151 | 62 | 26 | 143 | 2 | 978 | 353 | 625 | Cseed | no |
| shell | 216 | 29 | 28 | 26 | 15 | 1 | 4 | 0 | 65 | 27 | 38 | Cseed | thin |
| websocket | 107 | 15 | 12 | 7 | 1 | 3 | 3 | 0 | 31 | 10 | 21 | Cseed | thin |
| samples | 298 | 117 | 71 | 47 | 17 | 46 | 78 | 65 | 1383 | 654 | 729 | Cseed | no |
| commons-lang | 300 | 24 | 19 | 12 | 9 | 5 | 3 | 0 | 55 | 43 | 12 | Cseed | thin |
| commons-io | 300 | 54 | 37 | 21 | 8 | 17 | 4 | 0 | 151 | 135 | 16 | Cseed | no |

### Directory hit@3, all shapes pooled (mean per query)

| ranker | glia | shell | websocket | samples | commons-lang | commons-io |
|---|---|---|---|---|---|---|
| Dstr:P1 | 0.2919 | 1.0000 | 1.0000 | 0.3149 | 0.8444 | 0.7229 |
| Dstr:PT* | 0.3167 | 1.0000 | 1.0000 | 0.3039 | 0.8222 | 0.6506 |
| Dco | 0.3439 | 1.0000 | 1.0000 | 0.3370 | 0.8222 | 0.6867 |
| Dtxt | 0.4570 | 1.0000 | 1.0000 | 0.1934 | 0.7111 | 0.6747 |
| Dmm | 0.4774 | 1.0000 | 1.0000 | 0.3757 | 0.8222 | 0.8193 |
| Cseed | 0.4434 | 1.0000 | 1.0000 | 0.2431 | 0.8000 | 0.7349 |
| Cpop | 0.3891 | 1.0000 | 1.0000 | 0.0000 | 0.4444 | 0.3133 |
| Crand | 0.0068 | 0.8429 | 0.4348 | 0.0552 | 0.2000 | 0.3373 |
| Csib | - | - | - | - | - | - |
| Dsel | 0.4615 | 1.0000 | 1.0000 | 0.3591 | 0.8222 | 0.7952 |

### Directory hit@1, all shapes pooled (mean per query)

| ranker | glia | shell | websocket | samples | commons-lang | commons-io |
|---|---|---|---|---|---|---|
| Dstr:P1 | 0.1312 | 1.0000 | 0.9565 | 0.1823 | 0.7333 | 0.5181 |
| Dstr:PT* | 0.1244 | 1.0000 | 0.9565 | 0.1878 | 0.6889 | 0.5301 |
| Dco | 0.1018 | 1.0000 | 0.9130 | 0.1768 | 0.5111 | 0.4337 |
| Dtxt | 0.1855 | 1.0000 | 0.9130 | 0.1050 | 0.3778 | 0.4337 |
| Dmm | 0.1674 | 1.0000 | 1.0000 | 0.2044 | 0.6000 | 0.5422 |
| Cseed | 0.1493 | 1.0000 | 0.9130 | 0.2155 | 0.6444 | 0.5542 |
| Cpop | 0.3484 | 1.0000 | 0.9130 | 0.0000 | 0.3111 | 0.1566 |
| Crand | 0.0000 | 0.3571 | 0.0870 | 0.0000 | 0.0667 | 0.1084 |
| Csib | - | - | - | - | - | - |
| Dsel | 0.1810 | 1.0000 | 0.9565 | 0.2044 | 0.7333 | 0.5422 |

### Sibling recall@5, all shapes pooled (mean per query)

| ranker | glia | shell | websocket | samples | commons-lang | commons-io |
|---|---|---|---|---|---|---|
| Dstr:P1 | 0.0357 | 0.2745 | 0.2727 | 0.1237 | 0.0000 | 0.0822 |
| Dstr:PT* | 0.0357 | 0.2745 | 0.2727 | 0.1099 | 0.0000 | 0.0594 |
| Dco | 0.0298 | 0.2745 | 0.1818 | 0.0794 | 0.0263 | 0.0548 |
| Dtxt | 0.0224 | 0.2745 | 0.0000 | 0.0836 | 0.0263 | 0.0890 |
| Dmm | 0.0454 | 0.2745 | 0.1818 | 0.0692 | 0.0263 | 0.0731 |
| Cseed | 0.0223 | 0.3179 | 0.0000 | 0.0845 | 0.0263 | 0.1301 |
| Cpop | 0.0089 | 0.3179 | 0.0000 | 0.0023 | 0.0000 | 0.0651 |
| Crand | 0.0134 | 0.3179 | 0.0909 | 0.0586 | 0.0000 | 0.0753 |
| Csib | 0.0402 | 0.2745 | 0.3636 | 0.1189 | 0.0658 | 0.1720 |
| Dsel | 0.0410 | 0.2745 | 0.2727 | 0.1021 | 0.0000 | 0.0753 |

### Directory: dir hit@3 minus the best control (all shapes pooled; mean [95% commit-cluster bootstrap CI])

| ranker | glia | shell | websocket | samples | commons-lang | commons-io | wins | below |
|---|---|---|---|---|---|---|---|---|
| Dstr:P1 | -0.1516 [-0.2014, -0.0986] below | +0.0000 [+0.0000, +0.0000] (thin) | +0.0000 [+0.0000, +0.0000] (thin) | +0.0718 [+0.0272, +0.1280] win | +0.0444 [-0.0638, +0.1600] (thin) | -0.0120 [-0.1250, +0.0968] | 1 | 1 |
| Dstr:PT* | -0.1267 [-0.1743, -0.0778] below | +0.0000 [+0.0000, +0.0000] (thin) | +0.0000 [+0.0000, +0.0000] (thin) | +0.0608 [+0.0173, +0.1077] win | +0.0222 [-0.0750, +0.1316] (thin) | -0.0843 [-0.1852, +0.0115] | 1 | 1 |
| Dco | -0.0995 [-0.1438, -0.0539] below | +0.0000 [+0.0000, +0.0000] (thin) | +0.0000 [+0.0000, +0.0000] (thin) | +0.0939 [+0.0258, +0.1658] win | +0.0222 [-0.1020, +0.1364] (thin) | -0.0482 [-0.1600, +0.0506] | 1 | 1 |
| Dtxt | +0.0136 [-0.0321, +0.0599] | +0.0000 [+0.0000, +0.0000] (thin) | +0.0000 [+0.0000, +0.0000] (thin) | -0.0497 [-0.1333, +0.0349] | -0.0889 [-0.2500, +0.0476] (thin) | -0.0602 [-0.2000, +0.0741] | 0 | 0 |
| Dmm | +0.0339 [-0.0129, +0.0752] | +0.0000 [+0.0000, +0.0000] (thin) | +0.0000 [+0.0000, +0.0000] (thin) | +0.1326 [+0.0773, +0.1910] win | +0.0222 [+0.0000, +0.0769] (thin) | +0.0843 [+0.0222, +0.1600] win | 2 | 0 |
| Dsel | +0.0181 [-0.0223, +0.0596] | +0.0000 [+0.0000, +0.0000] (thin) | +0.0000 [+0.0000, +0.0000] (thin) | +0.1160 [+0.0645, +0.1774] win | +0.0222 [-0.0750, +0.1277] (thin) | +0.0602 [-0.0127, +0.1375] | 1 | 0 |

The control per repo: glia Cseed, shell Cseed, websocket Cseed, samples Cseed, commons-lang Cseed, commons-io Cseed.

### Sibling: recall@5 minus Csib (all shapes pooled; mean [95% commit-cluster bootstrap CI])

| ranker | glia | shell | websocket | samples | commons-lang | commons-io | wins | below |
|---|---|---|---|---|---|---|---|---|
| Dstr:P1 | -0.0045 [-0.0280, +0.0175] | +0.0000 [+0.0000, +0.0000] (thin) | -0.0909 [-0.2727, +0.0000] (thin) | +0.0048 [-0.0514, +0.0569] | -0.0658 [-0.1429, +0.0000] (thin) | -0.0898 [-0.2107, +0.0062] | 0 | 0 |
| Dstr:PT* | -0.0045 [-0.0280, +0.0175] | +0.0000 [+0.0000, +0.0000] (thin) | -0.0909 [-0.2727, +0.0000] (thin) | -0.0090 [-0.0683, +0.0455] | -0.0658 [-0.1429, +0.0000] (thin) | -0.1126 [-0.2212, -0.0256] below | 0 | 1 |
| Dco | -0.0104 [-0.0550, +0.0374] | +0.0000 [+0.0000, +0.0000] (thin) | -0.1818 [-0.6000, +0.0000] (thin) | -0.0395 [-0.0941, +0.0121] | -0.0395 [-0.1000, +0.0000] (thin) | -0.1172 [-0.1984, -0.0456] below | 0 | 1 |
| Dtxt | -0.0178 [-0.0566, +0.0154] | +0.0000 [+0.0000, +0.0000] (thin) | -0.3636 [-0.7273, +0.0000] (thin) | -0.0353 [-0.1019, +0.0289] | -0.0395 [-0.1000, +0.0000] (thin) | -0.0829 [-0.2246, +0.0898] | 0 | 0 |
| Dmm | +0.0053 [-0.0116, +0.0243] | +0.0000 [+0.0000, +0.0000] (thin) | -0.1818 [-0.6000, +0.0000] (thin) | -0.0497 [-0.0980, -0.0073] below | -0.0395 [-0.1000, +0.0000] (thin) | -0.0989 [-0.1918, -0.0207] below | 0 | 2 |
| Dsel | +0.0008 [-0.0182, +0.0209] | +0.0000 [+0.0000, +0.0000] (thin) | -0.0909 [-0.2727, +0.0000] (thin) | -0.0168 [-0.0713, +0.0327] | -0.0658 [-0.1429, +0.0000] (thin) | -0.0966 [-0.2154, +0.0010] | 0 | 0 |

### Directory hit@3 per shape, the three non-thin repos (Dmm minus Cseed, 95% CI)

| repo | shape | queries | Cseed | Cpop | Dstr:P1 | Dtxt | Dmm | Dmm - Cseed | Dsel | Dsel's last pick |
|---|---|---|---|---|---|---|---|---|---|---|
| glia | k1 | 203 | 0.468 | 0.399 | 0.300 | 0.448 | 0.493 | +0.025 [-0.020, +0.074] | 0.478 | Dmm |
| glia | k2 | 151 | 0.497 | 0.397 | 0.318 | 0.444 | 0.457 | -0.040 [-0.086, +0.007] | 0.444 | Dmm |
| glia | half | 62 | 0.419 | 0.500 | 0.323 | 0.532 | 0.500 | +0.081 [-0.032, +0.177] | 0.484 | Dtxt |
| glia | find | 26 | 0.000 | 0.000 | 0.000 | 0.423 | 0.423 | +0.423 [+0.231, +0.615] | 0.385 | Dtxt |
| samples | k1 | 71 | 0.225 | 0.000 | 0.352 | 0.239 | 0.479 | +0.254 [+0.155, +0.366] | 0.451 | Dmm |
| samples | k2 | 47 | 0.426 | 0.000 | 0.489 | 0.255 | 0.532 | +0.106 [+0.021, +0.213] | 0.511 | Dmm |
| samples | half | 17 | 0.471 | 0.000 | 0.529 | 0.294 | 0.471 | +0.000 [+0.000, +0.000] | 0.529 | Dstr:P1 |
| samples | find | 46 | 0.000 | 0.000 | 0.000 | 0.022 | 0.022 | +0.022 [+0.000, +0.065] | 0.000 | Dtxt |
| commons-io | k1 | 37 | 0.595 | 0.243 | 0.784 | 0.622 | 0.784 | +0.189 [+0.081, +0.324] | 0.757 | Dstr:P1 |
| commons-io | k2 | 21 | 0.905 | 0.190 | 0.905 | 0.714 | 0.905 | +0.000 [-0.143, +0.143] | 0.905 | Dstr:P1 |
| commons-io | half | 8 | 1.000 | 0.125 | 1.000 | 0.750 | 1.000 | +0.000 [+0.000, +0.000] | 1.000 | Dstr:P1 |
| commons-io | find | 17 | 0.706 | 0.706 | 0.235 | 0.706 | 0.706 | +0.000 [+0.000, +0.000] | 0.647 | Dtxt |

## Reading

- **Coverage decides first.** Three of the six gate repos add a candidate file in fewer than 30
  of their test commits (shell 29, commons-lang 24, websocket 15), so they are `thin` and only
  three repos can count; the rule asks for four wins. The verdict would be FAIL on coverage
  alone, whatever the numbers.
- **Directory.** On the three that count, the best directory ranker is `Dmm` (the mean of
  min-maxed structure, co-change and text): it beats Cseed on samples (+0.133, CI [+0.077,
  +0.191]) and commons-io (+0.084, [+0.022, +0.160]) and is level on glia (+0.034, [-0.013,
  +0.075]). Structure alone (`Dstr:P1` / `Dstr:PT*`) and co-change alone lose to Cseed on glia
  (CI wholly below 0), whose parent revisions hold about 1,160 candidate directories (fixture
  trees) and 143 of 229 added-file commits open a new directory. Most of the margin is k1 on samples
  and commons-io (+0.254 and +0.189), plus glia's 26 find-shape commits (+0.423, where Cseed is
  Cpop and names nothing): with one seed, "the seed's own directory" misses where a
  multi-directory Flutter sample or a commons-io test lands, and Dmm's text and co-change parts
  name it. On k2 / half the seeds' directories already hold the answer about as often as any
  ranker finds it.
- **Sibling.** No ranker's CI against `Csib` (the seeds' directories' files, most recently
  modified first) on recall@5 is above 0 on any repo; Dmm is below it on samples and commons-io. Sibling recall is low
  for every ranker row (glia 0.02-0.05, samples 0.07-0.12, commons-io 0.05-0.17), and 64% of glia's added
  files, 53% of samples' have no parent file above Jaccard 0.2: most new files are not a copy of
  one existing file at the line level.
- **Leakage check.** The snapshot graph already holds the added files, which can only favour
  `Dstr`; Dstr:P1 is the weakest of the five rankers on glia regardless.
- `samples` has 65 `unanchored` commits (a new top-level sample directory with no ancestor
  holding a candidate file, the repo root holding none): every ranker and control misses those
  alike, so they lower every hit rate there but move no difference.

## Verdict

Placement: FAIL (directory: no ranker beats the best control on dir hit@3 on >= 4 repos without a loss, best Dmm wins 2 of 3 non-thin repos, below on 0; sibling: none beats Csib on recall@5, best Dstr:P1 wins 0 of 3 non-thin repos, below on 0)

Per the plan (§F item 14), placement does not ship: FC.22 (the follow-up answer) is dropped and
this report closes item 14. The harness stays (`--place on`, `--place-judge`), so a later leap
with a wider public panel (repos that add files in at least 30 of their test commits) can re-run
the same rule without a code change.
