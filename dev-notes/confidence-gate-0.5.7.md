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


## Report (HC.3)

The rule above is the text of commit 9e94d4e215e34ab9739be393641ccb5373b93ab6 ("plan(confidence): the
calibrated-confidence pass rule, committed before any held-out edge is sampled or judged (HC.2)"); this file
had no other commit before this report, so the rule block reads exactly as committed. Every number below is
`bench/edge-audit/calibration.{json,md}`, derived by `bash scripts/edge-audit.sh calibrate` from the committed
samples and verdicts (`calibrate --check` and the fit's `report --check` both exit 0).

### What ran

- **Sample.** `bash scripts/edge-audit.sh sample-heldout`, no override: the seven copies under
  `~/.cache/glia-rank-gate/<label>@<rev12>`, archived from the cohort's bare clones (nothing fetched); the
  `edge_audit` release example built from the main tree at 221f9c84 with no uncommitted Rust in it; 67 s.
  363 samples over 38 pooled strata (`samples/heldout.jsonl`, each header `"panel":"heldout"`):

  ```
  [edge-audit] sample label=mealie edges=32208 strata=22 sampled=53 unjudgeable=13 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  [edge-audit] sample label=chatwoot edges=34628 strata=23 sampled=50 unjudgeable=38 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  [edge-audit] sample label=squidex edges=63218 strata=23 sampled=69 unjudgeable=33 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  [edge-audit] sample label=tolgee edges=81556 strata=23 sampled=50 unjudgeable=34 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  [edge-audit] sample label=hoppscotch edges=35428 strata=25 sampled=33 unjudgeable=25 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  [edge-audit] sample label=gin-vue-admin edges=9123 strata=20 sampled=46 unjudgeable=11 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  [edge-audit] sample label=zora-protocol edges=23222 strata=22 sampled=62 unjudgeable=17 excluded=0 seed=glia-edge-audit-0.5.4 panel=heldout
  ```

  A second `sample-heldout` after the judging, at a later HEAD (ae76e579, two sibling commits on), wrote the
  same bytes (`cmp` clean).
- **First verdicts.** All 363 ids judged under `RUBRIC.md` by HC.3 (`verdicts/heldout.jsonl`, one family):
  333 correct, 28 wrong, 2 unsure. Each judged from the `show` excerpts plus the files they name (route tables,
  base classes, imports, `using` lines, templates, manifests) in the held-out copies.
- **Re-judge.** GP.13's matched set for the family: all 28 wrong ids plus the 28 smallest-key correct ids, 56
  in all, re-judged by HC.3r from the excerpts and the source and appended to `verdicts/rejudge.jsonl`. 0
  disagreements, so `adjudication.jsonl` is unchanged (empty).
- **Calibrate.**

  ```
  [edge-audit] calibration strata=25 fit_n=496 heldout_n=263 covered=25/25 cal_error=0.0218
  CONFIDENCE: R1=yes (25/25) R2=yes (0.0218) R3=yes (25) -> PASS
  ```

### Blinding

- `calibration.json` did not exist before the judging; `report.json`, `report.md` and the fit's verdict files
  were not opened before the last held-out verdict was written.
- **Exposure before judging, stated:** the packet text, CLAUDE.md and this file's preamble quote the fit's
  per-tier precision (fact 1.000, derived 0.894, heuristic 0.913) and DOCUMENTS|derived 0.742, and one grep of
  `dev-notes/edge-precision-0.5.6.md` (read for the GP.14 judging procedure) printed IMPORTS|heuristic's p
  0.900. No other fit stratum's number was seen before judging.
- **The re-judge is not independent.** A workflow subagent has no second agent to hand the set to, so HC.3r is
  the HC.3 agent re-reading source with the first verdicts closed: all 56 pairs measure self-consistency, as 16
  of GP.14's 19 pairs did. The matched rule also tells the re-judge its set is rich in wrong verdicts. Kappa
  1.000 below is that self-consistency, not inter-judge agreement.

### Strata

| stratum | N held-out | fit k/n | fit p [lo, hi] | held-out k/n | held-out p [lo, hi] | in K | R1 holds | abs diff | w |
|---|---:|---:|---|---:|---|---|---|---:|---:|
| ACCESSES_DATA\|fact | 3570 | 5/5 | 1.000 [0.566, 1.000] | 7/10 | 0.700 [0.397, 0.892] | yes | yes | 0.300 | 0.014 |
| CALLS\|derived | 6 | 0/0 | - [-, -] | 6/6 | 1.000 [0.610, 1.000] | no | - | - | - |
| CALLS\|fact | 85320 | 31/31 | 1.000 [0.890, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.324 |
| CONTAINS\|fact | 3422 | 10/10 | 1.000 [0.723, 1.000] | 12/13 | 0.923 [0.667, 0.986] | yes | yes | 0.077 | 0.013 |
| DEFINES_CONFIG\|fact | 718 | 30/30 | 1.000 [0.886, 1.000] | 11/11 | 1.000 [0.741, 1.000] | yes | yes | 0.000 | 0.003 |
| DEFINES\|fact | 93422 | 12/12 | 1.000 [0.757, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.355 |
| DEPENDS_ON\|fact | 2574 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.010 |
| DOCUMENTS\|derived | 764 | 23/31 | 0.742 [0.568, 0.863] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.258 | 0.003 |
| EVENT_FLOWS\|derived | 14 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| EVENT_FLOWS\|fact | 288 | 0/0 | - [-, -] | 2/8 | 0.250 [0.071, 0.591] | no | - | - | - |
| EVENT_FLOWS\|heuristic | 3 | 0/0 | - [-, -] | 3/3 | 1.000 [0.439, 1.000] | no | - | - | - |
| GRPC_CALLS\|derived | 0 | 18/18 | 1.000 [0.824, 1.000] | 0/0 | - [-, -] | no | - | - | - |
| HANDLED_BY\|derived | 12 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.000 |
| HANDLED_BY\|fact | 4494 | 30/30 | 1.000 [0.886, 1.000] | 9/12 | 0.750 [0.468, 0.911] | yes | yes | 0.250 | 0.017 |
| HANDLED_BY\|heuristic | 155 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| HAS_ATTRIBUTE\|fact | 4054 | 11/11 | 1.000 [0.741, 1.000] | 11/12 | 0.917 [0.646, 0.985] | yes | yes | 0.083 | 0.015 |
| HTTP_CALLS\|derived | 935 | 7/7 | 1.000 [0.646, 1.000] | 10/11 | 0.909 [0.623, 0.984] | yes | yes | 0.091 | 0.004 |
| IMPLEMENTS\|derived | 263 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.001 |
| IMPLEMENTS\|fact | 3145 | 30/30 | 1.000 [0.886, 1.000] | 11/11 | 1.000 [0.741, 1.000] | yes | yes | 0.000 | 0.012 |
| IMPLEMENTS\|heuristic | 833 | 14/14 | 1.000 [0.785, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.003 |
| IMPORTS\|fact | 16584 | 31/31 | 1.000 [0.890, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.063 |
| IMPORTS\|heuristic | 14618 | 27/30 | 0.900 [0.744, 0.965] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.100 | 0.056 |
| INHERITS_FROM\|fact | 1983 | 30/30 | 1.000 [0.886, 1.000] | 10/11 | 0.909 [0.623, 0.984] | yes | yes | 0.091 | 0.007 |
| INHERITS_FROM\|heuristic | 1778 | 8/8 | 1.000 [0.676, 1.000] | 9/10 | 0.900 [0.596, 0.982] | yes | yes | 0.100 | 0.007 |
| INJECTS\|fact | 3018 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| INJECTS\|heuristic | 588 | 2/2 | 1.000 [0.342, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.002 |
| NAVIGATES_TO\|derived | 40 | 0/0 | - [-, -] | 2/10 | 0.200 [0.057, 0.510] | no | - | - | - |
| NAVIGATES_TO\|fact | 24 | 0/0 | - [-, -] | 9/10 | 0.900 [0.596, 0.982] | no | - | - | - |
| READS_CONFIG\|fact | 633 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.002 |
| RETURNS_TYPE\|fact | 457 | 1/1 | 1.000 [0.206, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.002 |
| RPC_CALLS\|derived | 8 | 0/0 | - [-, -] | 8/8 | 1.000 [0.676, 1.000] | no | - | - | - |
| RPC_CALLS\|heuristic | 32 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| SAME_AS\|derived | 240 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| SCHEDULES\|fact | 23 | 1/1 | 1.000 [0.206, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.000 |
| TESTS\|derived | 5053 | 30/30 | 1.000 [0.886, 1.000] | 10/12 | 0.833 [0.552, 0.953] | yes | yes | 0.167 | 0.019 |
| TESTS\|fact | 2327 | 1/1 | 1.000 [0.206, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.009 |
| USES\|derived | 1 | 0/0 | - [-, -] | 1/1 | 1.000 [0.206, 1.000] | no | - | - | - |
| USES\|fact | 15820 | 31/31 | 1.000 [0.890, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.060 |
| WS_CONNECTS\|derived | 2 | 2/2 | 1.000 [0.342, 1.000] | 2/2 | 1.000 [0.342, 1.000] | no | - | - | - |

Not transferred (R1 fails): none.
Largest gaps in K: ACCESSES_DATA|fact, DOCUMENTS|derived, HANDLED_BY|fact, TESTS|derived, IMPORTS|heuristic.
Held-out strata under their tier floor: ACCESSES_DATA|fact, CONTAINS|fact, EVENT_FLOWS|fact, HANDLED_BY|fact, HAS_ATTRIBUTE|fact, INHERITS_FROM|fact, NAVIGATES_TO|derived, NAVIGATES_TO|fact, TESTS|derived.

### Per tier (information)

| tier | fit n | fit p [lo, hi] | held-out n | held-out p [lo, hi] |
|---|---:|---|---:|---|
| derived | 148 | 0.894 [0.818, 0.941] | 100 | 0.869 [0.664, 0.957] |
| fact | 314 | 1.000 [0.850, 1.000] | 198 | 0.987 [0.978, 0.992] |
| heuristic | 54 | 0.913 [0.792, 0.967] | 63 | 0.990 [0.950, 0.998] |

### Per held-out repo (information)

| repo | edges | unjudgeable | sampled | judged | wrong | unsure | p [lo, hi] |
|---|---:|---:|---:|---:|---:|---:|---|
| chatwoot | 34628 | 38 | 50 | 50 | 3 | 0 | 0.971 [0.914, 0.991] |
| gin-vue-admin | 9123 | 11 | 46 | 46 | 0 | 0 | 1.000 [0.248, 1.000] |
| hoppscotch | 35428 | 25 | 33 | 33 | 1 | 0 | 0.996 [0.261, 0.999] |
| mealie | 32208 | 13 | 53 | 53 | 1 | 0 | 0.987 [0.933, 0.998] |
| squidex | 63218 | 33 | 69 | 69 | 13 | 0 | 0.971 [0.949, 0.984] |
| tolgee | 81556 | 34 | 50 | 50 | 2 | 0 | 0.995 [0.979, 0.999] |
| zora-protocol | 23222 | 17 | 62 | 60 | 8 | 2 | 0.947 [0.874, 0.978] |

### Sub-strata by emitter / rule (information: both sides n >= 10)

| sub-stratum | fit k/n | fit p [lo, hi] | held-out k/n | held-out p [lo, hi] | R1 holds | abs diff |
|---|---:|---|---:|---|---|---:|
| DEPENDS_ON\|fact\|extractor:manifest/- | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.000 |
| IMPLEMENTS\|heuristic\|graph:refs/global_unique | 14/14 | 1.000 [0.785, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.000 |
| IMPORTS\|heuristic\|graph:imports/tail_unique | 27/30 | 0.900 [0.744, 0.965] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.100 |
| READS_CONFIG\|fact\|extractor:config/- | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.000 |
| TESTS\|derived\|pass:tests/name_match | 18/18 | 1.000 [0.824, 1.000] | 8/10 | 0.800 [0.490, 0.943] | yes | 0.200 |

### Re-judge agreement (information)

Overall: 1.000 (56 re-judged of 56 expected, 0 missing); disagreements 0, adjudicated 0, unadjudicated 0.
- heldout: 1.000 (56 re-judged of 56 expected)
Kappa 1.000 (raw agreement 1.000, expected 0.500; 28 correct/correct, 28 wrong/wrong).

### Name-drift answer precision (information: the cohort's names probe)

Flagged files that are truth files / flagged files, over the drift tasks whose start ran the names probe.

| repo | held-out | tasks | flagged | truth | p [lo, hi] |
|---|---|---:|---:|---:|---|
| chatwoot | yes | 4 | 111 | 0 | 0.000 [0.000, 0.034] |
| documenso | no | 4 | 2 | 1 | 0.500 [0.095, 0.905] |
| gin-vue-admin | yes | 4 | 0 | 0 | - [-, -] |
| hoppscotch | yes | 4 | 105 | 2 | 0.019 [0.005, 0.067] |
| immich | no | 6 | 292 | 2 | 0.007 [0.002, 0.025] |
| mealie | yes | 4 | 78 | 0 | 0.000 [0.000, 0.047] |
| medusa | no | 4 | 144 | 0 | 0.000 [0.000, 0.026] |
| microservices-demo | no | 4 | 86 | 3 | 0.035 [0.012, 0.098] |
| photoprism | no | 4 | 10 | 0 | 0.000 [0.000, 0.278] |
| plane | no | 4 | 20 | 0 | 0.000 [0.000, 0.161] |
| prefect | no | 4 | 0 | 0 | - [-, -] |
| samples | no | 3 | 231 | 0 | 0.000 [0.000, 0.016] |
| scaffold-eth-2 | no | 4 | 17 | 3 | 0.176 [0.062, 0.410] |
| squidex | yes | 4 | 373 | 0 | 0.000 [0.000, 0.010] |
| tolgee | yes | 4 | 0 | 0 | - [-, -] |
| twenty | no | 4 | 17 | 0 | 0.000 [0.000, 0.184] |
| zora-protocol | yes | 4 | 38 | 1 | 0.026 [0.005, 0.135] |
| all | no | 69 | 1524 | 12 | 0.008 [0.004, 0.014] |

### Reading

- **PASS on all three conditions.** Every stratum of K (25) keeps its held-out Wilson upper bound at or above
  the fit's lower bound, the held-out-weighted error is 0.0218 against 0.05, and |K| is 25 against 15. HC.4 is
  released by this verdict (its precision table pooled over fit + held-out, per the rule).
- **What K does not see.** 13 held-out strata are outside K, 12 with no fit row at all (CALLS|derived,
  EVENT_FLOWS|derived / fact / heuristic, HANDLED_BY|heuristic, INJECTS|fact, NAVIGATES_TO|derived / fact,
  RPC_CALLS|derived / heuristic, SAME_AS|derived, USES|derived) and WS_CONNECTS|derived with 2 judged on each
  side. The two worst held-out strata, NAVIGATES_TO|derived 0.200 [0.057, 0.510] and EVENT_FLOWS|fact 0.250
  [0.071, 0.591], are both among them: the gate measured no transfer for them, and a pooled confidence there is
  the held-out panel's alone.
- **Where the weight sits.** DEFINES|fact (w 0.355), CALLS|fact (0.324), IMPORTS|fact (0.063), USES|fact (0.060)
  and IMPORTS|heuristic (0.056) carry 0.858 of R2's weight; the first four read 10/10 held-out and a perfect fit.
  The largest gaps (ACCESSES_DATA|fact 0.300, DOCUMENTS|derived 0.258, HANDLED_BY|fact 0.250, TESTS|derived 0.167)
  sit in strata of weight 0.003 to 0.019. R1 at n 10 is a wide test: ACCESSES_DATA|fact's 7/10 has an upper bound
  of 0.892 against the fit's lower bound of 0.566 (5/5).
- **DOCUMENTS|derived**, the 0.5.6 audit's one under-floor stratum, reads 10/10 on the held-out panel (1.000
  [0.723, 1.000]); five of its ten samples are OpenAPI / swag contract links (`pass:contract_link`) and five the
  doc linker's mentions (`pass:doclink`), each judged to refer to its target itself (see Limits for that reading).

### Held-out strata under their tier floor (precision-backlog follow-ups)

Nine strata read under floor on the held-out side. Each mechanism cluster below needs a public fixture before
it becomes a packet (`dev-notes/precision-backlog.json` is not this packet's file; the entries are listed in
HC.3's result for the integrator to file):

1. **ACCESSES_DATA|fact 7/10 (floor 0.95).** `extractor:data_sources` marks a test module as reading a store it
   only imports and mocks: a NestJS spec driving `mockDeep<PrismaService>()` (hoppscotch), an RSpec job spec that
   only expects `perform_later` (chatwoot, the store named in the file name), a Kotlin test round-tripping a
   Redisson Kryo codec in memory (tolgee). Fixture: a test importing a DB / cache client type and mocking it
   writes no ACCESSES_DATA.
2. **CONTAINS|fact 12/13.** `extractor:queues` reads a STOMP-over-SockJS client's `.subscribe(CHANNEL, cb)` as a
   redis pubsub QUEUE_CONSUMER (tolgee `WebsocketClient.test.ts`). Fixture: a `@stomp/stompjs` client subscribe
   mints no queue consumer.
3. **EVENT_FLOWS|fact 2/8 (no fit row).** `parser:solidity`: an `emit E(..)` of an event declared by an inherited
   interface or another file (IZoraSparks1155Managed, IERC721 in the same file, IZoraCreator1155 / IERC7572,
   IERC20Minter, IProtocolRewards; a qualified `emit IComments.Commented`) ends at an unresolved `(unknown:..)`
   placeholder, 6 wrong; 2 unsure where the event is OpenZeppelin's or forge-std's, outside the snapshot.
   Fixture: `contract C is I { function f() { emit E(); } }` with `event E` in `interface I` (and in another
   file) flows to I's E.
4. **HANDLED_BY|fact 9/12.** (a) C#: `[HttpGet]` with the template in a separate `[Route("apps/{app}/..")]`
   attribute mints a `GET /` route handled by the action (squidex SchemasController). (b) Solidity: two
   same-named contracts or interfaces in two files (two `ZoraCreator1155PreminterTest` test contracts; the
   coins and shared-contracts copies of `INonfungiblePositionManager`) share one `evm:` procedure, and the
   HANDLED_BY whose evidence is one file's function binds the other file's. Fixtures: an ASP.NET action with
   `[HttpGet]` + `[Route("x/{id}")]` routes `GET /x/{id}` only; two same-named Solidity interfaces each bind
   their own member.
5. **HAS_ATTRIBUTE|fact 11/12.** The Python parser mints a class ATTRIBUTE from a `self.repos` read of a
   property the base class declares (mealie RecipeCommentsController). Fixture: a subclass reading an inherited
   `@property` declares no attribute of its own.
6. **INHERITS_FROM|fact 10/11.** C#: the generic base `DomainObject<Team>` (Squidex.Infrastructure.Commands)
   binds by `module_symbol` to the class's own namespace PACKAGE `..Teams.DomainObject`. The same shape is the
   one INHERITS_FROM|heuristic wrong (`Initial : Migration`, EF Core's base, bound by `global_unique` to the
   `Contents.Migration` namespace; that stratum's 0.900 is above its 0.70 floor). Fixture: a base-type ref
   never binds a PACKAGE (namespace) node.
7. **NAVIGATES_TO|derived 2/10 (no fit row).** `graph:nav` rule `param` binds a literal `navigate(['/404'])` to a
   one-segment parameter page (`/:schemaName`, `/:ruleId`) whose real path is nested under parent routes, while
   the `'**'` catch-all serves it; 8 wrong, all squidex guards and their specs (the specs only verify a mock
   router). Fixture: Angular routes with a nested `:id` child and a `'**'` route; `navigate(['/404'])` binds no
   parameter page.
8. **NAVIGATES_TO|fact 9/10 (no fit row).** A nested Vue-router child `{ path: '', redirect: 'list' }` is
   recorded as `page:/ -> page:/list`, the parent path dropped (chatwoot custom-roles routes). Fixture: a child
   redirect keeps its parent prefix.
9. **TESTS|derived 10/12 (floor 0.90).** `pass:tests` rule `name_match` pairs file stems across unrelated
   directories: `spec/controllers/stripe/callbacks_controller_spec.rb` -> `app/controllers/google/
   callbacks_controller.rb` while `app/controllers/stripe/callbacks_controller.rb` exists (chatwoot);
   `MongoDb/Domain/Contents/ExtensionsTests.cs` -> `Squidex.Data.Tests.CodeGenerator/Extensions.cs` while the
   test aliases `Contents.Operations.Extensions` (squidex). Fixture: two same-stem modules, the spec's own
   directory's twin wins.

One more wrong edge sits in a stratum above floor: HTTP_CALLS|derived 10/11 (0.909, floor 0.90), squidex's
`GET /api/apps/{name}` paired by rule `any` to an `ANY /apps/{app}/` route whose template carries only the
PUT and DELETE actions, while `[HttpGet][Route("apps/{app}")]` exists.

### Limits

- One model family judged the fit and the held-out panel, and one agent judged and re-judged the held-out
  panel (above).
- **The DOCUMENTS reading.** A doc section was judged to document its target when its text refers to that same
  declaration and says something about it (an instruction, a changelog entry, a finding), even where the
  section covers more; a mention of a same-named different thing is wrong. Three of the five `pass:doclink`
  verdicts rest on that reading (mealie's AGENTS.md settings rule, squidex's CHANGELOG "e.g. `ErrorDto`", zora's
  protocol-sdk CHANGELOG `httpClient`). Under a stricter "the section's subject" reading DOCUMENTS|derived reads
  7/10 (upper bound 0.892 against the fit's lower bound 0.568, weight 0.003): R1 still holds there and R2 moves
  by under 0.001, so the verdict does not depend on it.
- n 10 per stratum: every held-out interval is wide, so R1 is a weak test for any stratum short of a large miss.
- The panel is seven public repos chosen by language spread and build size; twenty / medusa / immich are left
  out for build cost.

CONFIDENCE: R1=yes (25/25) R2=yes (0.0218) R3=yes (25) -> PASS
