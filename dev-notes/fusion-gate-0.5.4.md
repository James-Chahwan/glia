# The fusion gate (0.5.4, FB.6b)

FB.6 (dev-notes/embedding-gate-0.5.4.md, `VERDICT: FAIL`) judged the semantic tier alone and it failed
R2 (shell -0.0887 [-0.129, -0.048]). Its information rows suggested the blend: FUSE[T1s,SEMp:t2] - T1s had
CI lo > 0 on 6 of 6. Those rows are seen data, so they fixed this gate's configuration and are never its
evidence (ruling FB-6-FAIL, James 2026-10-25). This gate judges the blended form on data FB.6 did not score,
in two steps (James 2026-10-25, "Can we try on 3 repos quickly unseen as long as it's looking good we can
test at the end with everything"): a PREVIEW on three public repos no panel has seen, then the VERDICT at
the end of 0.5.4 on commits newer than every window already scored. Produced by
`bash scripts/embedding-gate.sh fusion-preview` / `fusion-verdict` (rank_eval `--sem on`, `--gate-pair`;
the panels are pinned in bench/rank-gate/fusion-preview.json). Every number is an aggregate over a public
repository. Model: `bge-small`.

## The pass rule (committed with the harness, before any held-out run)

```
THE FUSION RULE (FB.6b; committed before any held-out run; strictness as FB-5, James 2026-10-24 "Strict"):
  Fixed from FB.6's (seen) information rows, before any held-out run, and never tuned: recipe t2, the
  BGE-prefixed query (SEMp:t2), FUSE[X,Y] = 0.5 minmax(X) + 0.5 minmax(Y) exactly as FB.6 defines it.
  Candidates: F1 = FUSE[T1s,SEMp:t2]; F2 = LX[IDR,FUSE[T1s,SEMp:t2]] (the shipped find --files order
  LX[IDR,T1s] with its BM25 key replaced by F1: ids still route first). Baselines: T1s, LX[IDR,T1s]
  (shipped, EB.8), SELf2t (its calibrated pick, EC.13).
  Metric: recall@10 in rank_eval's find shape. Every a - b is rank_eval `--gate-pair`'s paired
  commit-cluster bootstrap 95% CI (B = 1000) on the panel's test commits only. n = the panel's repos.
  R1: F1 - T1s has CI lo > 0 on >= ceil(5n/6) repos and CI hi >= 0 on all n.
  R2: F2 - LX[IDR,T1s] has CI lo > 0 on >= ceil(n/2) repos and CI hi >= 0 on all n.
  (n = 6 reads exactly as the packet: R1 5 of 6, R2 3 of 6; the preview's n = 3: R1 3 of 3, R2 2 of 3;
  the verdict's n = 9: R1 8 of 9, R2 5 of 9.) PASS = R1 and R2; anything else is FAIL.
  Information rows only: F2 - SELf2t, F1 - SEMp:t2, F2 - F1, and every FB.6 pair re-run on these windows.
  PREVIEW (fusion-preview): the three public repos of bench/rank-gate/fusion-preview.json, never in any
  gate panel, each in a language no other panel repo is written in, test window rank_eval's default
  (the newest 300 test-eligible commits): its R1 / R2 lines are labelled PREVIEW and are never the verdict.
  VERDICT (fusion-verdict, the final step at the end of 0.5.4): the six snapshots of snapshots.json at
  newer pinned revs plus the three preview repos at newer revs than the preview's, each run with
  `--test-after <after>` (FB.6's rev, or the preview's rev): every test commit is newer than <after>, so
  no test commit of FB.6 or of the preview is scored again (the report lists each window's first / last
  sha and the intersection with the prior window, which must be 0, else the judge refuses).
  Device: the default (FB.14: the GPU worker, CPU fallback), recorded per repo from its `[embed] device=`
  line. FB.6's cached node vectors (CPU) are reused; vectors agree across devices to < 4e-7 and the
  spike's rankings matched exactly, so a mixed-device panel is valid.
  ON PASS the integrator re-specs FB.7a / FB.7b / FB.7c / FB.8 in fused form (find --files and find's file
  tier rank by F2; pack's fallback seeds from F2's files; the rank gate measures the shipped fused find)
  and schedules them. ON FAIL they stay out of 0.5.4 and glia embed ships as a sidecar no answer reads.

```

<!-- fusion-gate:preview -->
## Preview

Not run yet.
<!-- /fusion-gate:preview -->

<!-- fusion-gate:verdict -->
## Verdict

Not run yet: the final step at the end of 0.5.4 pins the verdict rows of bench/rank-gate/fusion-preview.json
and runs `bash scripts/embedding-gate.sh fusion-verdict`.
<!-- /fusion-gate:verdict -->

VERDICT: pending (the end-of-0.5.4 run; the PREVIEW is never a verdict)
