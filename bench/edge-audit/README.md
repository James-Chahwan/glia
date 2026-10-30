# Edge-precision audit (0.5.4, FD.2)

How often is an edge the graph writes actually true? `bench/substrate-gap` grades hand-keyed
fixtures (recall, plus forbid rows); this audit samples the edges a real build writes and has
a judge check each one against the source. It is the precision baseline 0.5.6 measures
against, and every stratum under its floor becomes a packet in the FD precision group.

## What is measured

- **Population.** Every edge of every per-language graph plus the cross edges of a
  `generate_one` build of each tree in `snapshots.json`, five rows:
  - glia itself (`source: self`), extracted with `git archive` to
    `$GLIA_RANK_GATE_DIR/glia@<rev12>`. Its `rev` is pinned at
    0df1bbbb89b7276e298b301d6c29bdf628d39d53, the commit FD.3a drew its sample at, so a
    re-draw keeps glia's sampled rows and FD.3a's verdicts on them (a self row without `rev`,
    or `"rev": "HEAD"`, audits HEAD). Its skips drop the fixture trees `bench/`,
    `dev-notes/`, `tests/` and `engine/tests/data/`, so glia's stratum measures glia's code.
  - three rank-gate public snapshots (`source: rank-gate`), read by name from
    `bench/rank-gate/snapshots.json`: pop-os/shell TypeScript, gorilla/websocket Go,
    flutter/samples Dart.
  - microservices-demo (`source: cohort`, FD.3d, ruling FD-2), read by label from
    `bench/cohort/repos.json` (public, https, pinned 38e7348e) and archived from the cohort's
    bare clone `$GLIA_COHORT_DIR/<label>.git` (default `~/.cache/glia-cohort`; this script
    never fetches: `python3 bench/cohort/cohort.py` does). Eleven services in five languages
    joined by gRPC. It supplies the cross-service pairing edges, glia's distinguishing ones:
    every GRPC_CALLS (18) and every derived HANDLED_BY (17) in the panel is its; the other
    four rows hold two WS_CONNECTS and no pairing edge else, and no row holds an HTTP_CALLS
    (the demo's only HTTP server, the Go frontend, has no in-repo HTTP client). It skips
    nothing: its generated genproto stubs are edges a build writes like any other.

  No private repository enters a committed file; a rank-gate or cohort row that is not public,
  https and pinned at a 40-hex commit is refused.
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
bash scripts/edge-audit.sh sample            # fetch, extract glia@<rev> + cohort rows, write samples/audit.jsonl
bash scripts/edge-audit.sh show glia | less  # each sample with live source excerpts
#   judge: write verdicts/<judge>.jsonl, one {"id","verdict","reason","judge"} per line (RUBRIC.md)
bash scripts/edge-audit.sh report            # report.json + report.md
bash scripts/edge-audit.sh report --check    # exits 1 when the committed report is not derived
```

Adding a row re-draws the sample: a stratum's n is re-split across repos, so a smaller repo can
lose an id (its verdict leaves with it) while every id still sampled keeps its row byte for byte
(the key does not depend on the panel). FD.3d's re-draw kept 250 of FD.3a's 263 structure
verdicts, dropped 13 and judged the 49 new structure samples (all microservices-demo).

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

A sample's verdict is its first judge's (several first judges on one id: their shared verdict,
`unsure` when they disagree). FD.3c's blind re-judge (`verdicts/rejudge.jsonl`, a fixed subset)
only measures agreement: where it agrees nothing changes, and where it disagrees the
`adjudication.jsonl` row's `final` replaces the first verdict (a disagreement with no
adjudication row reads `unsure`). Cohen's kappa is reported per family (a first-verdict file) and
overall, and a family under the low-agreement threshold is flagged provisional. `--judge <name>`
reads one judge only. A verdict whose id is in no samples file, two verdicts from one judge on
one id, a re-judged id with no first verdict, or an adjudication row naming no real disagreement
is an error (exit 2). The report prints `[edge-audit] report strata=<s> judged=<j> unsure=<u>
under_floor=<f> insufficient=<i> kappa=<k>` on stderr.

## Statuses

- `under floor`: p_h below the tier's floor in `floors.json` (fact 0.95, derived 0.90,
  heuristic 0.70). Every `under_floor` entry in report.json is a precision-group packet.
- `insufficient`: fewer than 10 judged rows after unsure, unless the stratum is a census.
- `noisy` (a flag): unsure is over 10% of the stratum's verdicts.
- `unjudged`: no verdict yet.
