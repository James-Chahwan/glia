# Edge-precision audit (0.5.4, FD.2)

How often is an edge the graph writes actually true? `bench/substrate-gap` grades hand-keyed
fixtures (recall, plus forbid rows); this audit samples the edges a real build writes and has
a judge check each one against the source. It is the precision baseline 0.5.6 measures
against, and every stratum under its floor becomes a packet in the FD precision group.

## What is measured

- **Population.** Every edge of every per-language graph plus the cross edges of a
  `generate_one` build of each tree in `snapshots.json`, five rows:
  - glia itself (`source: self`), extracted with `git archive` to
    `$GLIA_RANK_GATE_DIR/glia@<rev12>`. Its `rev` is pinned at the 0.5.7 release candidate
    558fa99229faf16d61d5b25fd5a83017951bfa1a (HP.5; FD.3a drew at 0df1bbbb, GP.14 at
    ad5dd83d), so a re-draw keeps glia's sampled rows and their verdicts wherever an edge's
    site did not move (a self row without `rev`, or `"rev": "HEAD"`, audits HEAD). Its skips
    drop the fixture trees `bench/`, `dev-notes/`, `tests/` and `engine/tests/data/`, so
    glia's stratum measures glia's code, and (since HP.5) `engine/src/rules/reflexion_parity.rs`,
    the parity harness against a private repository's report, whose names are that
    repository's.
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
  (`traces`) and every edge touching a GOAL_UNIT node (`goal_unit`): the goal layer is
  0.5.8's and writes no edge in 0.5.7. Org-package routing (`resolver:org_packages`, rules
  `org_import` / `org_call` / `org_ref`) is in the population since the 0.5.7 proof run
  (HP.5): its edges are ordinary IMPORTS / CALLS / USES strata. An edge with neither both ends
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

## The held-out panel and the calibration (0.5.7, HC.2)

A precision measured on the fit panel above is a confidence for a row of another repo only if it
transfers. The calibrated-confidence gate (`dev-notes/confidence-gate-0.5.7.md`, its rule committed
before any held-out id was sampled or judged) measures that on repos the fit never saw.

- **Panel.** The `snapshots.json` rows marked `"panel": "heldout"`: seven public cohort repos (mealie,
  chatwoot, squidex, tolgee, hoppscotch, gin-vue-admin, zora-protocol), read by label from
  `bench/cohort/repos.json` and archived from the cohort's bare clones like any cohort row. A row
  without `panel` is the fit; a name appears once.
- **Sample.** `bash scripts/edge-audit.sh sample-heldout` runs `edge_audit sample --panel heldout`: the
  fit's population, exclusions, strata, key and seed, n 10 per pooled (category, tier) stratum
  (structural categories too; a pooled stratum of at most 10 is censused), into
  `samples/heldout.jsonl`, each header carrying `"panel":"heldout"`. The fit's `samples/audit.jsonl`
  is never re-drawn by it, and two runs write the same bytes.
- **Judging.** As the fit: `show <label>` (it picks the held-out file for a held-out label), first
  verdicts in `verdicts/heldout.jsonl`, the matched blind re-judge appended to `verdicts/rejudge.jsonl`,
  disagreements in `adjudication.jsonl`.
- **Panels never mix.** `report --panel fit` reads only the fit panel's samples and the verdicts of their ids
  (a verdict or adjudication row of a held-out id is known and skipped), so `report --check` holds
  with held-out files beside the fit's; `calibrate` reads only the held-out panel's.
- **Calibration.** `bash scripts/edge-audit.sh calibrate [--check]` runs `edge_audit report
  --calibration`: the fit is `report.json` and the fit's samples, verdicts and adjudication as committed
  at `FIT_REV` (cd541240, the 0.5.6 audit; extracted with `git show`, so a later re-audit moves no
  calibration number). Per stratum it compares the fit's (correct, judged) with the held-out's, both
  with Wilson 95% intervals; K is the strata with >= 10 held-out judged ids and a fit p; R1 (the
  held-out upper bound reaches the fit's lower bound on >= 90% of K), R2 (the held-out-weighted mean
  |p_fit - p_held| <= 0.05) and R3 (|K| >= 15, else INSUFFICIENT) give the verdict. Information rows:
  the comparison per (category, tier, emitter, rule) where both sides hold n >= 10, the per-repo and
  per-tier held-out estimates, the held-out re-judge's kappa, and the cohort's name-drift answer
  precision (flagged files that are truth files / flagged files, `bench/cohort/results/baseline/
  tasks.jsonl` at `COHORT_REV`). It writes `calibration.json` / `calibration.md` and prints
  `[edge-audit] calibration strata=<k> fit_n=<a> heldout_n=<b> covered=<c>/<k> cal_error=<e>` and
  `CONFIDENCE: R1=<yes|no> (<c>/<k>) R2=<yes|no> (<e>) R3=<yes|no> (<k>) -> PASS|FAIL|INSUFFICIENT`.

## The 0.5.7 proof run (HP.5): both panels, one rule

`report` reads `--panel all` by default since HP.5 (`--panel fit` is the fit report above,
`--panel heldout` the held-out alone): the fit samples (re-drawn at the release candidate) and
the held-out samples (as HC.3 judged them, never re-drawn here: they are the calibration's), each
pooled stratum post-stratified per panel, since the two panels were drawn at different n. A
pooled stratum's p is the stratified estimate over its judged panel parts (weights N_hg over
their N) with that estimate's interval; a tier's or category's estimate is stratified over every
judged (stratum, panel) part. report.json gains `panel: "all"`, each stratum's `panels` and the
agreement's `independence`; report.md a "Strata by panel" table. `calibrate` is unchanged.

`RULE-0.5.7.md`, committed before the sample was drawn, is the run's pass rule. When it sits
beside the plan, `report --panel all` prints the computed verdict and the re-judge's independence:

```
[edge-audit] rule 0.5.7 verdict=<PASS|FAIL> under_floor=<list|none> fact=<p> derived=<p> heuristic=<p> kappa=<k>
[edge-audit] rule 0.5.7 rejudge independent=<i> self=<s>
```

A re-judge pair is a self pair when its re-judge name, less one trailing `r`, is its first
judge's (GP.14r / GP.14, HC.3r / HC.3); the rule's kappa is over the independent pairs only.
HP.5's re-judge (`HP.5x`) is a separate, blinded `claude -p` process per snapshot. The write-up
is `dev-notes/edge-precision-0.5.7.md`.
