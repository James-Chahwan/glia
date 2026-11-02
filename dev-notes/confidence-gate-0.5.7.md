# The calibrated-confidence gate (0.5.7, HC.2 / HC.3 / HC.4)

Scope §2 D: a calibrated confidence per answer row, measured on the cohort (James's "crime coefficient").
Every row already carries a tier (`fact` / `derived` / `heuristic`). The 0.5.6 edge audit
(dev-notes/edge-precision-0.5.6.md, bench/edge-audit/report.json) measured each (edge category, tier)
stratum's precision on five repos: 516 judged, 27 strata, one under floor (DOCUMENTS|derived 0.742
[0.568, 0.863]); per tier fact 1.000 [0.850, 1.000], derived 0.894, heuristic 0.913. A precision earned
on five repos is a confidence for a row of ANOTHER repo only if it transfers. This gate measures the
transfer first: the same strata judged on public repos the fit never saw. HC.2 builds the tool and
commits this rule; HC.3 judges the held-out sample and appends the report; HC.4 (glia_engine::confidence,
a `precision` beside each row's tier) is built only on PASS.

## The rule (committed before any held-out id is sampled or judged)

Committed by HC.2 in this file's first commit, alone, before `edge_audit sample --panel heldout`
existed and before any held-out edge was drawn or judged. It is never tuned after a held-out verdict
exists: no re-run for a different verdict, no widened threshold, no stratum or repo dropped after the
fact. HC.3's report section quotes this commit's sha.

```
THE CONFIDENCE RULE (HC.2; committed before any held-out id is judged; never tuned):
  FIT: the 0.5.6 audit, bench/edge-audit/report.json at cd541240355e9d238bebcbdf0eb8ac269ec29557
    (GP.14: 27 strata, 516 judged), its `strata` rows read as (category, tier, correct, n = correct +
    wrong). scripts/edge-audit.sh `calibrate` reads it with `git show <that sha>:..`, never the working
    tree's report.json, so a later re-audit (HP.5) moves no number here. Information rows (below) read
    the fit's samples, verdicts and adjudication at the same sha.
  HELD-OUT panel: seven public cohort repos the fit never saw, rows `"panel": "heldout"` of
    bench/edge-audit/snapshots.json, read by label from bench/cohort/repos.json (public, https, pinned
    40-hex) and archived from the cohort's bare clones:
      mealie         0b2801bfb42b  Python (FastAPI / Pydantic) + Vue
      chatwoot       3564ec1e961f  Ruby (Rails) + Vue
      squidex        f828dd717edc  C# (ASP.NET Core) + Angular
      tolgee         43e6654422b1  Kotlin (Spring) + TypeScript (React)
      hoppscotch     63273f8e599e  TypeScript (NestJS GraphQL) + Vue
      gin-vue-admin  f7f1009c98e6  Go (gin) + Vue
      zora-protocol  32c2931a444b  Solidity + TypeScript (viem)
    (the six of the accepted default, chosen by language spread and build size, twenty / medusa /
    immich left out for build cost; zora-protocol joins by the integrator's ruling, one panel and one
    judging run serving HC.3 and HP.5). No overlay is read: every held-out tree is a `git archive`
    copy holding no `.glia/`.
  SAMPLE: exactly the fit's sampler (engine/examples/edge_audit.rs `sample`): the same population
    (every per-language and cross edge of a `generate_one` build), the same exclusions (traces,
    goal_unit, org_packages, unjudgeable), the same strata (category, tier as `glia why` prints it),
    the same key (xxh64 over seed, label, category, from, to, evidence rule, file, line) and seed
    `glia-edge-audit-0.5.4`; n = 10 per pooled (category, tier) stratum over the seven repos
    (structural categories too), split across repos in proportion to their stratum sizes with at
    least one per repo holding it (largest remainder, ties by label); a pooled stratum of at most 10
    edges is censused. Written to bench/edge-audit/samples/heldout.jsonl, each header marked
    `"panel":"heldout"`; the fit's samples are never re-drawn by it.
  JUDGING (HC.3): RUBRIC.md, one `{id, verdict, reason, judge}` per id in verdicts/heldout.jsonl; the
    judge never reads calibration.json, report.json's strata or the fit numbers above before judging.
    GP.13's matched blind re-judge (every first wrong plus as many correct, at least 20) by a second
    judge that has not read the first verdicts, appended to verdicts/rejudge.jsonl; disagreements
    adjudicated in adjudication.jsonl. Each id's verdict is resolved as the fit report resolves its own
    (first verdict, adjudication's `final` where the re-judge disagrees, `unsure` when unadjudicated).
  PER STRATUM h: fit (k_fit, n_fit) from the FIT row; held-out (k_held, n_held) with n_held = correct
    + wrong over the held-out ids of h (unsure excluded, counted). p = k / n. Each side's interval is the
    Wilson 95% score interval (z = 1.959964, edge_audit's `wilson`) at (k, n), a census stratum
    included (the panel's repos are a sample of repos, so a census of their edges is still a sample of
    the edges glia writes). N_h = the held-out pooled population of h (the samples headers'
    stratum_sizes summed over the seven repos).
  K: the strata with n_held >= 10 and a fit p (n_fit >= 1), sorted by (category, tier).
  R1 (the floor holds): for at least 90% of K (covered * 10 >= 9 * |K|, integers), the held-out Wilson
    upper bound >= the fit's Wilson lower bound: the confidence glia would print is not contradicted.
    covered = the strata of K meeting it.
  R2 (calibration error): cal_error = sum over h in K of w_h * |p_fit,h - p_held,h| <= 0.05, with
    w_h = N_h / sum over K of N_h (the held-out population share inside K).
  R3: |K| >= 15; else INSUFFICIENT, which is a FAIL.
  VERDICT: PASS iff R1 and R2 and R3; INSUFFICIENT when R3 fails (whatever R1 and R2 read); else FAIL.
  ON PASS: HC.4 ships, its precision table re-estimated on fit + held-out pooled (per stratum, the
    judged ids of both panels).
  ON FAIL / INSUFFICIENT: nothing reaches an answer (HC.4 commits nothing and returns not-needed quoting
    the verdict line); the report lists the strata of K whose R1 failed and those whose
    |p_fit - p_held| is largest, and every held-out stratum under its tier floor (fact 0.95, derived
    0.90, heuristic 0.70) becomes a dev-notes/precision-backlog.json entry with a public fixture.
  INFORMATION ROWS (reported, never gated):
    - sub-strata: the same comparison per (category, tier, emitter, rule) where both sides hold
      n >= 10 (the fit side from its samples and verdicts at the FIT sha);
    - per held-out repo: edges, sampled, judged, wrong, unsure and its stratified estimate;
    - the held-out panel's per-tier stratified estimate and its re-judge agreement (Cohen's kappa);
    - the cohort's answer-level precision for the name-drift rows, which edges cannot measure:
      over the drift tasks of bench/cohort/results/baseline/tasks.jsonl at
      9d66de3d04d98366af921af038728a3e7171e2c6, flagged files that are truth files / flagged files
      (S.names.flagged_truth / S.names.flagged), overall and per repo, with its Wilson interval.
  MARKERS (stderr): `[edge-audit] calibration strata=<|K|> fit_n=<fit judged over K> heldout_n=<held-out
    judged over K> covered=<c>/<|K|> cal_error=<e>` and
    `CONFIDENCE: R1=<yes|no> (<c>/<|K|>) R2=<yes|no> (<e>) R3=<yes|no> (<|K|>) -> PASS|FAIL|INSUFFICIENT`.
```

## How to run it

```
bash scripts/edge-audit.sh sample-heldout     # samples/heldout.jsonl from the seven held-out copies
bash scripts/edge-audit.sh show <label>       # a held-out label's samples with live excerpts
#   judge: verdicts/heldout.jsonl (first), verdicts/rejudge.jsonl (appended matched re-judge)
bash scripts/edge-audit.sh calibrate          # bench/edge-audit/calibration.{json,md}, the two markers
bash scripts/edge-audit.sh calibrate --check  # exits 1 when the committed calibration is not derived
bash scripts/edge-audit.sh report --check     # the fit report: held-out rows never enter it
```

HC.3 appends its report (calibration.md's tables, the marker, kappa and the CONFIDENCE line) below this
section.
