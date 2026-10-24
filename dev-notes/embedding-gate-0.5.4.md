# The embedding gate (0.5.4, FB.6)

Seed item 13: the semantic tier reaches an answer only if it beats BM25 alone on the rank gate's
public snapshots; otherwise `glia embed` ships as a sidecar no answer reads. Produced by
`bash scripts/embedding-gate.sh` (rank_eval `--sem on`, `--gate-pair`); every number below is an
aggregate over a public repository.

## The pass rule (committed with the harness, before the first bge-small run)

```
THE PASS RULE (fixed before any bge-small run; FB-5, James 2026-10-24, "Strict: R1 5/6, R2 3/6"):
  Panel: the six public snapshots of bench/rank-gate/snapshots.json. Metric: recall@10 in rank_eval's find
  shape (the query is the test commit's subject; no seed file). Every comparison a - b is rank_eval
  `--gate-pair`'s paired commit-cluster bootstrap 95% CI (B = 1000).
  Recipe r* = t2 iff SEM:t2 - SEM:t1 has CI lo > 0 on >= 3 of 6 snapshots and CI hi >= 0 on all 6; else t1.
  Query form: prefixed (bge) iff SEMp:t1 - SEM:t1 has CI lo > 0 on >= 3 of 6 and CI hi >= 0 on all 6; else
  raw (Engram's form). The selected ranker S* is SEM:r* (raw) or SEMp:r* (bge).
  R1 signal: S* - SEMrand:t1 has CI lo > 0 on >= 5 of 6.
  R2 beats BM25 alone: S* - T1s has CI lo > 0 on >= 3 of 6 and CI hi >= 0 on all 6 (S* alone: the shipped
  tier is its own rows, never fused into another order, so no fused ranker carries the verdict).
  PASS = R1 and R2; anything else is FAIL.
  Selection guard: R1 / R2 are also stated for SEM:t1 raw (the pre-registered default); a PASS that holds
  for S* but not for SEM:t1 raw is written with a `selected-only` note line above `VERDICT: PASS`.
  Information rows, never the verdict: FUSE[T1s,S*] - T1s, S* - ANS:find (the lexical node find),
  S* - LX[IDR,T1s] (the shipped find --files order, EB.8), S* - SELf2t (its calibrated pick, EC.13),
  S* - ANS:find_files and S* - ANS:find_files@sel (the engine's own find --files).
  On FAIL, glia embed ships as a sidecar no answer reads (item 13), and the integrator drops FB.7a / FB.7b /
  FB.7c / FB.8 / FB.9 from the schedule.

```

## Status

Not yet run: this commit fixes the rule and the harness. The bge-small run replaces this file
with the measured rows and ends it in the verdict line.
