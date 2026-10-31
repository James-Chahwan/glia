# Docs as a second graph: the doc-tie measurement (GP.9)

The one run made under `RULE.md` (committed first, 90e18828). Verdict:

```
[doc-ties] verdict=FAIL glia r10=0.2551 mrr=0.4566 drops=0 snapshots=6
```

The bridge never lowers a row (0 drops on 6 snapshots), and it does not recover the glia rows FD.79
lowered: glia single `ANS:impact+doctie` r10 0.25506484794793205 (floor 0.2594) and mrr
0.4566185848793213 (floor 0.4614), against `ANS:impact` r10 0.25506484794793205 and mrr
0.4566185853254562. By the rule GP.10 (NEW.f4o.9b) is not built, and the rank gate's four `accepted`
rows (glia/single/ANS:impact/{r10,mrr}, glia/single/ANS:impact@sel/{r10,mrr}) stay; James rules
between keeping them and option B (section-level ties from a history snapshot that records hunks).

## The run

- rank_eval release, built twice from one `git archive` of 90e18828 (the RULE commit) under the packet's
  cache dir: "before" with that commit's rank_eval.rs, "after" with GP.9's (the only file that
  differs). Each snapshot: its `snapshots.json` flags + `scripts/rank-gate.sh flags` (`--round1 on --tune
  on --pick base,d85 --round2 on --round3 on --r3-perq on --ans on --max-commits 300 --max-seeds 5 --boot
  1000`) + `--engine-parity on`, no `--sem`, `GLIA_NO_PERSIST=1`, on the gate's copies.
- Wall time, after run: glia 693 s, samples 140 s, shell 61 s, commons-lang 53 s, commons-io 49 s,
  websocket 21 s (two at a time).

## No existing row moved

- Every `perq` / `perq_shapes` column of the before run (1564 columns on glia, 1540 on each other
  snapshot, r10 and mrr) is bit-identical in the after run, `ids` and `commit_of` equal, and the ranker
  list of the before run is a prefix of the after run's: the after run adds exactly `ANS:impact+doctie`
  and `ANS:impact.files+doctie`, to single, k2, k3 and half (not find). `metrics` equal for every before
  ranker; `paired`, `round1` and `round3` only gain the two rankers' entries (every before entry present
  unchanged).
- `--gate-check bench/rank-gate/gate.json` of each after run against its before run (trimmed by the
  same gate.json), every snapshot:
  `[rank-gate] verdict label=<l> rows=98 drops=0 gains=0 new=0 drift=0 missing=0 changed=0
  correction=holm alpha=0.05` and `[rank-gate] contrasts: ok - 9 pairs, 0 below default`.
- Every `[rank-parity] label=<l> shape=.. ranking=..` line of every after run reads `key_mismatch=0
  order_mismatch=0` (24 per snapshot), and `[rank-parity] files .. equal=true not_code_equal=true`.

## Per snapshot

| snapshot | commits | test commits | queries (single) | docs | ties | hubs | bridged queries | appended queries |
|---|---|---|---|---|---|---|---|---|
| glia | 1180 | 300 | 1253 | 11 | 20 | 0 | 17 | 8 |
| shell | 759 | 216 | 641 | 2 | 0 | 0 | 0 | 0 |
| websocket | 285 | 107 | 278 | 1 | 1 | 0 | 0 | 0 |
| samples | 1626 | 298 | 1111 | 64 | 85 | 0 | 58 | 47 |
| commons-lang | 5275 | 300 | 863 | 2 | 0 | 0 | 0 | 0 |
| commons-io | 2290 | 300 | 937 | 2 | 0 | 0 | 0 | 0 |

From the `[doc-ties] label=..` markers: docs = doc files of the build (a DOC_SECTION in an md / mdx /
markdown / rst file); ties = distinct (doc, file) ties over every test commit's prefix; bridged / appended
= seeded queries (all four shapes) whose bridge reached a file / appended one the walk did not reach.

Why it moves nothing: a file the bridge reaches is either already one of diff impact's rows (the depth-4
carry walk reaches it, so it keeps its place) or is appended after every row. On glia 17 queries bridged
and 8 appended a file, yet no glia r10 value changed (changed=0 in every shape) and single mrr moved by
-4.5e-10 (an appended non-target file pushes a target that was ranked below every row one place down): the appended files land below the rows the walk already
ranks. On samples the single-shape r10 gains +0.0011 (p_gain 0.046, not rejected by Holm, `same`). The relation is also sparser than the spec's probe: the probe counted 13 docs and 45
ties over the full 2000-commit log with every `.md` path a doc, while the rule's doc is a file the build
ingests as DOC_SECTIONs (the DocRule scopes: repo root, docs/, project roots, ADRs, ..), so
bench/substrate-gap/COVERAGE.md and README.md, two of the probe's top three, are not docs here, and
each test commit sees only its own prefix.

## The judge

`rank_eval --doc-ties-judge <the six after runs> --boot 1000` (one run; the table is its `--md`):

new - base, commit-cluster bootstrap 95% CI (B = 1000); Holm per snapshot over the changed rows, one-sided, family alpha 0.05.

### glia

| shape | ranker - base | metric | base | new | diff [CI] | verdict |
|---|---|---|---|---|---|---|
| single | ANS:impact+doctie - ANS:impact | r10 | 0.2551 | 0.2551 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact+doctie - ANS:impact | mrr | 0.4566 | 0.4566 | -0.0000 [-0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | r10 | 0.3096 | 0.3096 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | mrr | 0.4630 | 0.4630 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | r10 | 0.2925 | 0.2925 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | mrr | 0.4308 | 0.4308 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | r10 | 0.3237 | 0.3237 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | mrr | 0.4621 | 0.4621 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.3196 | 0.3196 | +0.0000 [+0.0000, +0.0000] | same (info) |
| single | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5113 | 0.5113 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.3488 | 0.3488 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.4869 | 0.4869 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.3249 | 0.3249 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.4409 | 0.4409 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.3600 | 0.3600 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.4740 | 0.4740 | +0.0000 [+0.0000, +0.0000] | same (info) |

### shell

| shape | ranker - base | metric | base | new | diff [CI] | verdict |
|---|---|---|---|---|---|---|
| single | ANS:impact+doctie - ANS:impact | r10 | 0.6363 | 0.6363 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact+doctie - ANS:impact | mrr | 0.6567 | 0.6567 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | r10 | 0.7109 | 0.7109 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | mrr | 0.5878 | 0.5878 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | r10 | 0.7059 | 0.7059 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | mrr | 0.6029 | 0.6029 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | r10 | 0.6693 | 0.6693 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | mrr | 0.6884 | 0.6884 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.7384 | 0.7384 | +0.0000 [+0.0000, +0.0000] | same (info) |
| single | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.7152 | 0.7152 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.7534 | 0.7534 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6124 | 0.6124 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.7900 | 0.7900 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6151 | 0.6151 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.6919 | 0.6919 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6945 | 0.6945 | +0.0000 [+0.0000, +0.0000] | same (info) |

### websocket

| shape | ranker - base | metric | base | new | diff [CI] | verdict |
|---|---|---|---|---|---|---|
| single | ANS:impact+doctie - ANS:impact | r10 | 0.7417 | 0.7417 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact+doctie - ANS:impact | mrr | 0.6570 | 0.6570 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | r10 | 0.6474 | 0.6474 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | mrr | 0.5211 | 0.5211 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | r10 | 0.7032 | 0.7032 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | mrr | 0.5944 | 0.5944 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | r10 | 0.6684 | 0.6684 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | mrr | 0.6349 | 0.6349 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.7707 | 0.7707 | +0.0000 [+0.0000, +0.0000] | same (info) |
| single | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6902 | 0.6902 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.6317 | 0.6317 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5312 | 0.5312 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.7420 | 0.7420 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5592 | 0.5592 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.6889 | 0.6889 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6765 | 0.6765 | +0.0000 [+0.0000, +0.0000] | same (info) |

### samples

| shape | ranker - base | metric | base | new | diff [CI] | verdict |
|---|---|---|---|---|---|---|
| single | ANS:impact+doctie - ANS:impact | r10 | 0.3616 | 0.3627 | +0.0011 [+0.0000, +0.0029] | same |
| single | ANS:impact+doctie - ANS:impact | mrr | 0.4834 | 0.4834 | -0.0000 [-0.0000, -0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | r10 | 0.4796 | 0.4796 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | mrr | 0.5457 | 0.5457 | -0.0000 [-0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | r10 | 0.4902 | 0.4902 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | mrr | 0.5771 | 0.5771 | -0.0000 [-0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | r10 | 0.5469 | 0.5469 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | mrr | 0.6330 | 0.6330 | -0.0000 [-0.0000, +0.0000] | same |
| single | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.5280 | 0.5280 | +0.0000 [+0.0000, +0.0000] | same (info) |
| single | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5959 | 0.5959 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.5921 | 0.5921 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6370 | 0.6370 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.5742 | 0.5742 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6295 | 0.6295 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.6295 | 0.6295 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6857 | 0.6857 | +0.0000 [+0.0000, +0.0000] | same (info) |

### commons-lang

| shape | ranker - base | metric | base | new | diff [CI] | verdict |
|---|---|---|---|---|---|---|
| single | ANS:impact+doctie - ANS:impact | r10 | 0.2775 | 0.2775 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact+doctie - ANS:impact | mrr | 0.3287 | 0.3287 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | r10 | 0.3065 | 0.3065 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | mrr | 0.4079 | 0.4079 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | r10 | 0.3034 | 0.3034 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | mrr | 0.4304 | 0.4304 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | r10 | 0.3274 | 0.3274 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | mrr | 0.4343 | 0.4343 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.5789 | 0.5789 | +0.0000 [+0.0000, +0.0000] | same (info) |
| single | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.6017 | 0.6017 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.5023 | 0.5023 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5616 | 0.5616 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.4963 | 0.4963 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5346 | 0.5346 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.4874 | 0.4874 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5598 | 0.5598 | +0.0000 [+0.0000, +0.0000] | same (info) |

### commons-io

| shape | ranker - base | metric | base | new | diff [CI] | verdict |
|---|---|---|---|---|---|---|
| single | ANS:impact+doctie - ANS:impact | r10 | 0.2867 | 0.2867 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact+doctie - ANS:impact | mrr | 0.3744 | 0.3744 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | r10 | 0.3316 | 0.3316 | +0.0000 [+0.0000, +0.0000] | same |
| k2 | ANS:impact+doctie - ANS:impact | mrr | 0.3859 | 0.3859 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | r10 | 0.3062 | 0.3062 | +0.0000 [+0.0000, +0.0000] | same |
| k3 | ANS:impact+doctie - ANS:impact | mrr | 0.4083 | 0.4083 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | r10 | 0.3214 | 0.3214 | +0.0000 [+0.0000, +0.0000] | same |
| half | ANS:impact+doctie - ANS:impact | mrr | 0.4458 | 0.4458 | +0.0000 [+0.0000, +0.0000] | same |
| single | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.4935 | 0.4935 | +0.0000 [+0.0000, +0.0000] | same (info) |
| single | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5752 | 0.5752 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.4696 | 0.4696 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k2 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.4768 | 0.4768 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.4594 | 0.4594 | +0.0000 [+0.0000, +0.0000] | same (info) |
| k3 | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5339 | 0.5339 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | r10 | 0.4632 | 0.4632 | +0.0000 [+0.0000, +0.0000] | same (info) |
| half | ANS:impact.files+doctie - ANS:impact.files | mrr | 0.5639 | 0.5639 | +0.0000 [+0.0000, +0.0000] | same (info) |

[doc-ties] verdict=FAIL glia r10=0.2551 mrr=0.4566 drops=0 snapshots=6
