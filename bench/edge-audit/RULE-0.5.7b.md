# Edge precision, the 0.5.7 re-measure: the pass rule (HP.7)

Committed alone, before any `heldout_rc` sample is drawn or judged (James 2026-11-03: "option b
yeah pointless fail"). HP.5's `RULE-0.5.7.md` and its computed verdict
(`[edge-audit] rule 0.5.7 verdict=FAIL ..`) stay as written beside this one: this rule re-measures
the same claim on a held-out panel drawn with the release candidate's build, so the release's
claim describes the release's graph. Nothing in 0.5.7 is built on the verdict. `floors.json`,
`RUBRIC.md`, the sampler's key, seed and n are unchanged by this run.

## The panel the claim reads

`bash scripts/edge-audit.sh report` prints this rule's lines over `edge_audit report --panel
all_rc`, beside the 0.5.7 lines, which keep reading `--panel all` (fit + `heldout`) unchanged:

- **The fit panel** (`samples/audit.jsonl`), exactly as HP.5 drew it at the release candidate
  558fa992 and as its verdicts stand (commit 528c7e4c).
- **The re-drawn held-out panel `heldout_rc`** (`samples/heldout_rc.jsonl`): the same seven
  repos as HC.2's held-out panel (mealie, chatwoot, squidex, tolgee, hoppscotch, gin-vue-admin,
  zora-protocol) at their `snapshots.json` / `bench/cohort/repos.json` revs, drawn with the
  release candidate's build: `edge_audit` built from a `git archive` of
  54978e17b8dd2f9bc9aedfa0a99f4ed497bd1efc (the HEAD this run starts from, every graph-changing
  0.5.7 packet landed; its tree differs from 558fa992's, the fit's build, only under `bench/`,
  `dev-notes/` and `engine/examples/`, so both panels are one build) plus this run's
  `engine/examples/edge_audit.rs` (an example, not hashed), in an isolated target dir. The draw
  is HC.2's: seed `glia-edge-audit-0.5.4`, the same xxh64 key over (seed, label, category, from
  qname, to qname, evidence rule, file, line), n 10 per pooled (category, tier) stratum
  (structural categories too; a pooled stratum of at most 10 is censused), the current
  population (TRACES and GOAL_UNIT out, `org_packages` in, as HP.5 set it). Each header carries
  `"panel":"heldout_rc"`. A `heldout_rc` row's id is the xxh64 of the panel name and its key, so
  it never collides with a `heldout` id of the same edge; selection and row order are by the key.
- **Kept and fresh.** A `heldout_rc` row whose claim is a row HC.3 judged (the same label, rev,
  category, tier, emitter, rule, site and both ends, i.e. the same id in the `heldout` draw)
  keeps HC.3's verdict: it is copied into `verdicts/heldout_rc.jsonl` with its judge name `HC.3`
  and a `kept_from` field naming the `heldout` id. Every other row is fresh and is judged from
  source under `RUBRIC.md` by HP.7 (judge name `HP.7`), blinded to `calibration.json`,
  `report.json`'s strata and any earlier verdict of the same edge.
- **`heldout` is not read by this rule.** `samples/heldout.jsonl` and `verdicts/heldout.jsonl`
  (HC.2 / HC.3, commit 6cbcf536) stay as committed: they are the calibration's input
  (`calibrate` never reads `heldout_rc`), and the 0.5.7 lines still read them.

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

Conditions 1 and 2 read the `all_rc` report (fit + `heldout_rc`), not the committed
`report.json`, which stays the `--panel all` report. Condition 3's independent pairs for
`heldout_rc` come from HP.8, a separate packet's agent (judge name `HP.8`, appended to
`verdicts/rejudge.jsonl`): this run (HP.7) is the first judge of the fresh ids and starts no
model process, so it re-judges nothing and does not state the verdict as final.

Otherwise **FAIL**, listing each under-floor stratum. On FAIL each under-floor stratum's wrong
cluster is written up with a public fixture idea as a precision-backlog follow-up, as 0.5.6 did.

The verdict is computed, never typed: `report` prints, when this file sits beside the plan,

```
[edge-audit] rule 0.5.7b verdict=<PASS|FAIL> under_floor=<list|none> fact=<p> derived=<p> heuristic=<p> kappa=<k>
[edge-audit] rule 0.5.7b rejudge independent=<i> self=<s>
```

the thresholds read from `floors.json`, `kappa` the independent pairs' kappa.
