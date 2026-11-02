# Edge precision, the 0.5.7 proof run: the pass rule (HP.5)

Committed alone, before any 0.5.7 sample is drawn or judged. Nothing in 0.5.7 is built on the
verdict: it is the release's precision claim (README, `docs/dpg.md`) and an input to the
calibrated-confidence work. `floors.json`, `RUBRIC.md`, the sampler's key, seed and n are
unchanged by this run.

## The panel the claim reads

`bash scripts/edge-audit.sh report` runs `edge_audit report --panel all`:

- **The fit panel** (`samples/audit.jsonl`): the five 0.5.6 rows, glia at the 0.5.7 release
  candidate (the commit after the last graph-changing packet this run depends on, its sha in
  `snapshots.json`), shell, websocket, samples and microservices-demo at their pinned revs,
  re-drawn with `org_packages` edges back in the population (rules `org_import` / `org_call` /
  `org_ref` are ordinary IMPORTS / CALLS / USES strata; TRACES and GOAL_UNIT stay excluded: the
  goal layer is 0.5.8's and writes no edge in 0.5.7).
- **The held-out panel** (`samples/heldout.jsonl`): mealie, chatwoot, squidex, tolgee,
  hoppscotch, gin-vue-admin and zora-protocol, as HC.2 sampled them and HC.3 judged them
  (commit 6cbcf536). gin-vue-admin (HTTP, data, config) and zora-protocol (EVM, workspace
  packages) are the panel's widening; they stay held-out rows (a name is listed once), so their
  judged ids count here without being judged twice. The held-out sample is not re-drawn: its
  files belong to the calibration, which reads them as committed.

The two panels were drawn with different n (30 and 10 per pooled stratum), so they are
post-stratified: a pooled stratum h has one part per panel g with N_hg (that panel's edges of
h), n_hg judged and p_hg = correct / n_hg; p_h = sum_g (N_hg / N_h') p_hg over its judged parts
(N_h' their N). A tier's and a category's estimate is the stratified estimate over every judged
(stratum, panel) part, `ci_method=stratified_wilson`. Statuses as the README states them
(`under floor`: p_h below its tier's floor; `insufficient`: under 10 judged and not a census).

## The rule

**PASS** iff all four hold:

1. `report.json` `under_floor` is empty;
2. each tier's stratified p is at or above its floor in `floors.json` (fact 0.95, derived
   0.90, heuristic 0.70);
3. Cohen's kappa over the **independent** re-judge pairs of the panel is >= 0.80, over at
   least 20 such pairs;
4. the re-judge this run adds is independent: GP.13's matched rule picks the ids (every first
   `wrong` of a family plus as many `correct`, at least 20), and every id this run re-judges is
   re-judged by a different agent from its first judge, blinded to the first verdict. A pair is
   a self pair when the re-judge's name, less one trailing `r`, is its first judge's name (the
   names GP.14r / GP.14 and HC.3r / HC.3 already carry); the report counts self pairs, and this
   run adds none. Inherited self pairs (an earlier packet re-judging its own ids) stay in the
   files, are counted, and never enter condition 3.

Otherwise **FAIL**, listing each under-floor stratum. On FAIL each under-floor stratum's wrong
cluster is written up with a public fixture idea as a precision-backlog follow-up, as 0.5.6 did.

The verdict is computed, never typed: `report` prints, when this file sits beside the plan,

```
[edge-audit] rule 0.5.7 verdict=<PASS|FAIL> under_floor=<list|none> fact=<p> derived=<p> heuristic=<p> kappa=<k>
[edge-audit] rule 0.5.7 rejudge independent=<i> self=<s>
```

the thresholds read from `floors.json`, `kappa` the independent pairs' kappa.
