# Edge-precision audit (0.5.4, FD.2)

How often is an edge the graph writes actually true? `bench/substrate-gap` grades hand-keyed
fixtures (recall, plus forbid rows); this audit samples the edges a real build writes and has
a judge check each one against the source. It is the precision baseline 0.5.5 measures
against, and every stratum under its floor becomes a packet in the FD precision group.

## What is measured

- **Population.** Every edge of every per-language graph plus the cross edges of a
  `generate_one` build of each tree in `snapshots.json`: glia itself at the audit commit
  (`source: self`, extracted with `git archive` to `$GLIA_RANK_GATE_DIR/glia@<rev12>`; its
  skips drop the fixture trees `bench/`, `dev-notes/`, `tests/` and `engine/tests/data/`, so
  glia's stratum measures glia's code) and three rank-gate public snapshots, read by name
  from `bench/rank-gate/snapshots.json` (pop-os/shell TypeScript, gorilla/websocket Go,
  flutter/samples Dart). No private repository enters a committed file.
- **Out of the population**, counted per reason in each sample header and the marker's
  `excluded=`: a `--skip` prefix on the from-node's file (`skip`); every TRACES edge
  (`traces`), every edge touching a GOAL_UNIT node (`goal_unit`) and every edge emitted by
  `resolver:org_packages` (`org_packages`): those 0.5.4 layers are gated by their own audits,
  so this one stays the baseline of the 0.5.3 edge vocabulary. An edge with neither both ends
  located nor an evidence file cannot be judged against source: it is counted per stratum as
  `unjudgeable` and never sampled.
- **Strata.** (edge category, evidence tier), the tier `glia why` prints
  (`glia_engine::why::edge_tier`, the reading `check` and `glia query` share).
- **Sample.** Per pooled stratum, `n_per_stratum` (30) edges, `n_structural` (10) for the
  structural categories in `floors.json`. The n is split across repos in proportion to their
  stratum sizes, each repo holding the stratum taking at least one (largest remainder, ties by
  label), so a stratum spread over four repos can hold one or two more than n; a stratum of at
  most n edges is censused. Inside a repo the sample is the edges with the smallest xxh64 key
  over (seed, label, category, from qname, to qname, evidence rule, file, line): reruns on one
  tree write identical bytes. Two copies of one edge (one key) are one candidate.

## Reproduce

```
bash scripts/edge-audit.sh sample            # fetch, extract glia@HEAD, write samples/audit.jsonl
bash scripts/edge-audit.sh show glia | less  # each sample with live source excerpts
#   judge: write verdicts/<judge>.jsonl, one {"id","verdict","reason","judge"} per line (RUBRIC.md)
bash scripts/edge-audit.sh report            # report.json + report.md
bash scripts/edge-audit.sh report --check    # exits 1 when the committed report is not derived
```

`samples/audit.jsonl` holds one header line per label (`stratum_sizes`, `unjudgeable`,
`excluded`, `edges`) and then that label's rows sorted by (category, tier, key): `id`, `label`,
`rev`, `category`, `tier`, `tier_note`, `emitter`, `rule`, `site {file, line}`, `from` / `to
{qname, kind, file, line}`; lines are 1-based. No source text is ever written: `show` reads it
from the snapshot copy.

## The estimator

Per pooled stratum h: N_h (the stratum's edges over all repos), n_h = correct + wrong (unsure
excluded and counted), p_h = correct / n_h with a Wilson 95% interval (z = 1.959964). Per
category and per tier the estimate is stratified over the judged strata:
p = sum W_h p_h, W_h = N_h / N, var = sum W_h^2 p_h (1 - p_h) / n_h (1 - n_h / N_h), interval
p +- 1.96 sqrt(var) clipped to [0, 1]; a census stratum (n_h = N_h) adds no variance. The
per-repo tables are the same restricted to one repo's samples and sizes: reported, not gated.

When several judges rule on one id (FD.3c's blind re-judge) the report uses their shared
verdict and `unsure` when they disagree; `--judge <name>` reads one judge only. A verdict whose
id is in no samples file, or two verdicts from one judge on one id, is an error (exit 2).

## Statuses

- `under floor`: p_h below the tier's floor in `floors.json` (fact 0.95, derived 0.90,
  heuristic 0.70). Every `under_floor` entry in report.json is a precision-group packet.
- `insufficient`: fewer than 10 judged rows after unsure, unless the stratum is a census.
- `noisy` (a flag): unsure is over 10% of the stratum's verdicts.
- `unjudged`: no verdict yet.
