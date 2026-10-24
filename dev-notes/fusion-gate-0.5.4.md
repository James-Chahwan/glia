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
## Preview (three unseen public repos; never the verdict)

| repo | language | rev | test commits | queries | first test sha | last test sha | prior window | shared with prior |
|---|---|---|---:|---:|---|---|---|---:|
| httpx | python | b5addb64f016 | 300 | 300 | f9d18a87585b | ae1b9f66238f | none (never in a panel) | 0 |
| sinatra | ruby | cb22afd7902b | 300 | 300 | 36e5b26d35da | 0c88089be766 | none (never in a panel) | 0 |
| plug | elixir | a735b147fa74 | 300 | 300 | fc29f089f7ad | 73404f851852 | none (never in a panel) | 0 |

Device per repo (FB.14's `[embed] device=` line of each run):

- httpx: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- sinatra: `(no [embed] device= line: every vector came from the cache)`
- plug: `(no [embed] device= line: every vector came from the cache)`

### Rule rows

R1 (FUSE[T1s,SEMp:t2] - T1s, lo > 0 on >= 3 of 3, hi >= 0 on all): lo > 0 on 3, hi >= 0 on 3 -> yes. R2 (LX[IDR,FUSE[T1s,SEMp:t2]] - LX[IDR,T1s], lo > 0 on >= 2 of 3, hi >= 0 on all): lo > 0 on 3, hi >= 0 on 3 -> yes.

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| httpx | FUSE[T1s,SEMp:t2] | T1s | 0.7064 | 0.6034 | +0.1030 | [+0.0794, +0.1307] |
| sinatra | FUSE[T1s,SEMp:t2] | T1s | 0.7278 | 0.6215 | +0.1063 | [+0.0752, +0.1390] |
| plug | FUSE[T1s,SEMp:t2] | T1s | 0.7967 | 0.6835 | +0.1132 | [+0.0823, +0.1468] |
| httpx | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.7064 | 0.6034 | +0.1030 | [+0.0794, +0.1307] |
| sinatra | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.7278 | 0.6215 | +0.1063 | [+0.0752, +0.1390] |
| plug | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.7967 | 0.6835 | +0.1132 | [+0.0823, +0.1468] |

### Information rows (never the verdict)

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| httpx | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.7064 | 0.5999 | +0.1065 | [+0.0816, +0.1341] |
| sinatra | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.7278 | 0.6504 | +0.0774 | [+0.0368, +0.1179] |
| plug | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.7967 | 0.7082 | +0.0885 | [+0.0551, +0.1213] |
| httpx | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.7064 | 0.6546 | +0.0518 | [+0.0204, +0.0820] |
| sinatra | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.7278 | 0.6372 | +0.0907 | [+0.0631, +0.1206] |
| plug | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.7967 | 0.7799 | +0.0168 | [-0.0080, +0.0419] |
| httpx | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.7064 | 0.7064 | +0.0000 | [+0.0000, +0.0000] |
| sinatra | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.7278 | 0.7278 | +0.0000 | [+0.0000, +0.0000] |
| plug | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.7967 | 0.7967 | +0.0000 | [+0.0000, +0.0000] |

### Every FB.6 pair on these windows (information)

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| httpx | SEM:t2 | SEM:t1 | 0.6316 | 0.5781 | +0.0535 | [+0.0286, +0.0775] |
| sinatra | SEM:t2 | SEM:t1 | 0.6336 | 0.6042 | +0.0294 | [+0.0024, +0.0557] |
| plug | SEM:t2 | SEM:t1 | 0.7723 | 0.7126 | +0.0596 | [+0.0338, +0.0853] |
| httpx | SEMp:t1 | SEM:t1 | 0.6147 | 0.5781 | +0.0366 | [+0.0155, +0.0580] |
| sinatra | SEMp:t1 | SEM:t1 | 0.5894 | 0.6042 | -0.0148 | [-0.0358, +0.0030] |
| plug | SEMp:t1 | SEM:t1 | 0.7438 | 0.7126 | +0.0312 | [+0.0135, +0.0486] |
| httpx | SEM:t1 | SEMrand:t1 | 0.5781 | 0.2825 | +0.2957 | [+0.2521, +0.3435] |
| sinatra | SEM:t1 | SEMrand:t1 | 0.6042 | 0.1668 | +0.4374 | [+0.3888, +0.4894] |
| plug | SEM:t1 | SEMrand:t1 | 0.7126 | 0.2843 | +0.4283 | [+0.3780, +0.4770] |
| httpx | SEM:t1 | T1s | 0.5781 | 0.6034 | -0.0252 | [-0.0647, +0.0181] |
| sinatra | SEM:t1 | T1s | 0.6042 | 0.6215 | -0.0173 | [-0.0609, +0.0321] |
| plug | SEM:t1 | T1s | 0.7126 | 0.6835 | +0.0291 | [-0.0153, +0.0730] |
| httpx | FUSE[T1s,SEM:t1] | T1s | 0.6790 | 0.6034 | +0.0757 | [+0.0521, +0.1007] |
| sinatra | FUSE[T1s,SEM:t1] | T1s | 0.7004 | 0.6215 | +0.0789 | [+0.0478, +0.1137] |
| plug | FUSE[T1s,SEM:t1] | T1s | 0.7695 | 0.6835 | +0.0860 | [+0.0558, +0.1166] |
| httpx | SEM:t1 | ANS:find | 0.5781 | 0.2517 | +0.3265 | [+0.2778, +0.3750] |
| sinatra | SEM:t1 | ANS:find | 0.6042 | 0.1713 | +0.4329 | [+0.3808, +0.4832] |
| plug | SEM:t1 | ANS:find | 0.7126 | 0.1686 | +0.5440 | [+0.4949, +0.5898] |
| httpx | SEM:t1 | LX[IDR,T1s] | 0.5781 | 0.6034 | -0.0252 | [-0.0647, +0.0181] |
| sinatra | SEM:t1 | LX[IDR,T1s] | 0.6042 | 0.6215 | -0.0173 | [-0.0609, +0.0321] |
| plug | SEM:t1 | LX[IDR,T1s] | 0.7126 | 0.6835 | +0.0291 | [-0.0153, +0.0730] |
| httpx | SEM:t1 | SELf2t | 0.5781 | 0.5999 | -0.0218 | [-0.0607, +0.0201] |
| sinatra | SEM:t1 | SELf2t | 0.6042 | 0.6504 | -0.0462 | [-0.0902, +0.0016] |
| plug | SEM:t1 | SELf2t | 0.7126 | 0.7082 | +0.0044 | [-0.0413, +0.0509] |
| httpx | SEM:t1 | ANS:find_files | 0.5781 | 0.6107 | -0.0325 | [-0.0721, +0.0111] |
| sinatra | SEM:t1 | ANS:find_files | 0.6042 | 0.6315 | -0.0273 | [-0.0715, +0.0222] |
| plug | SEM:t1 | ANS:find_files | 0.7126 | 0.6727 | +0.0399 | [-0.0059, +0.0820] |
| httpx | SEM:t1 | ANS:find_files@sel | 0.5781 | 0.6050 | -0.0269 | [-0.0652, +0.0149] |
| sinatra | SEM:t1 | ANS:find_files@sel | 0.6042 | 0.6605 | -0.0563 | [-0.1007, -0.0092] |
| plug | SEM:t1 | ANS:find_files@sel | 0.7126 | 0.7088 | +0.0039 | [-0.0413, +0.0495] |
| httpx | SEM:t2 | SEMrand:t1 | 0.6316 | 0.2825 | +0.3491 | [+0.3053, +0.3961] |
| sinatra | SEM:t2 | SEMrand:t1 | 0.6336 | 0.1668 | +0.4668 | [+0.4197, +0.5200] |
| plug | SEM:t2 | SEMrand:t1 | 0.7723 | 0.2843 | +0.4879 | [+0.4390, +0.5350] |
| httpx | SEM:t2 | T1s | 0.6316 | 0.6034 | +0.0282 | [-0.0138, +0.0740] |
| sinatra | SEM:t2 | T1s | 0.6336 | 0.6215 | +0.0121 | [-0.0315, +0.0588] |
| plug | SEM:t2 | T1s | 0.7723 | 0.6835 | +0.0887 | [+0.0454, +0.1314] |
| httpx | FUSE[T1s,SEM:t2] | T1s | 0.6995 | 0.6034 | +0.0962 | [+0.0717, +0.1232] |
| sinatra | FUSE[T1s,SEM:t2] | T1s | 0.7159 | 0.6215 | +0.0943 | [+0.0602, +0.1280] |
| plug | FUSE[T1s,SEM:t2] | T1s | 0.7916 | 0.6835 | +0.1081 | [+0.0772, +0.1407] |
| httpx | SEM:t2 | ANS:find | 0.6316 | 0.2517 | +0.3799 | [+0.3336, +0.4273] |
| sinatra | SEM:t2 | ANS:find | 0.6336 | 0.1713 | +0.4623 | [+0.4149, +0.5119] |
| plug | SEM:t2 | ANS:find | 0.7723 | 0.1686 | +0.6037 | [+0.5575, +0.6489] |
| httpx | SEM:t2 | LX[IDR,T1s] | 0.6316 | 0.6034 | +0.0282 | [-0.0138, +0.0740] |
| sinatra | SEM:t2 | LX[IDR,T1s] | 0.6336 | 0.6215 | +0.0121 | [-0.0315, +0.0588] |
| plug | SEM:t2 | LX[IDR,T1s] | 0.7723 | 0.6835 | +0.0887 | [+0.0454, +0.1314] |
| httpx | SEM:t2 | SELf2t | 0.6316 | 0.5999 | +0.0317 | [-0.0097, +0.0771] |
| sinatra | SEM:t2 | SELf2t | 0.6336 | 0.6504 | -0.0168 | [-0.0612, +0.0324] |
| plug | SEM:t2 | SELf2t | 0.7723 | 0.7082 | +0.0641 | [+0.0193, +0.1079] |
| httpx | SEM:t2 | ANS:find_files | 0.6316 | 0.6107 | +0.0209 | [-0.0212, +0.0673] |
| sinatra | SEM:t2 | ANS:find_files | 0.6336 | 0.6315 | +0.0021 | [-0.0404, +0.0498] |
| plug | SEM:t2 | ANS:find_files | 0.7723 | 0.6727 | +0.0995 | [+0.0568, +0.1403] |
| httpx | SEM:t2 | ANS:find_files@sel | 0.6316 | 0.6050 | +0.0266 | [-0.0149, +0.0713] |
| sinatra | SEM:t2 | ANS:find_files@sel | 0.6336 | 0.6605 | -0.0268 | [-0.0717, +0.0229] |
| plug | SEM:t2 | ANS:find_files@sel | 0.7723 | 0.7088 | +0.0635 | [+0.0193, +0.1072] |
| httpx | SEMp:t1 | SEMrand:t1 | 0.6147 | 0.2825 | +0.3323 | [+0.2899, +0.3802] |
| sinatra | SEMp:t1 | SEMrand:t1 | 0.5894 | 0.1668 | +0.4226 | [+0.3755, +0.4714] |
| plug | SEMp:t1 | SEMrand:t1 | 0.7438 | 0.2843 | +0.4595 | [+0.4093, +0.5075] |
| httpx | SEMp:t1 | T1s | 0.6147 | 0.6034 | +0.0114 | [-0.0231, +0.0483] |
| sinatra | SEMp:t1 | T1s | 0.5894 | 0.6215 | -0.0321 | [-0.0754, +0.0146] |
| plug | SEMp:t1 | T1s | 0.7438 | 0.6835 | +0.0603 | [+0.0152, +0.1033] |
| httpx | FUSE[T1s,SEMp:t1] | T1s | 0.6902 | 0.6034 | +0.0869 | [+0.0654, +0.1108] |
| sinatra | FUSE[T1s,SEMp:t1] | T1s | 0.7046 | 0.6215 | +0.0830 | [+0.0530, +0.1131] |
| plug | FUSE[T1s,SEMp:t1] | T1s | 0.7796 | 0.6835 | +0.0961 | [+0.0633, +0.1288] |
| httpx | SEMp:t1 | ANS:find | 0.6147 | 0.2517 | +0.3630 | [+0.3142, +0.4098] |
| sinatra | SEMp:t1 | ANS:find | 0.5894 | 0.1713 | +0.4181 | [+0.3674, +0.4660] |
| plug | SEMp:t1 | ANS:find | 0.7438 | 0.1686 | +0.5752 | [+0.5254, +0.6193] |
| httpx | SEMp:t1 | LX[IDR,T1s] | 0.6147 | 0.6034 | +0.0114 | [-0.0231, +0.0483] |
| sinatra | SEMp:t1 | LX[IDR,T1s] | 0.5894 | 0.6215 | -0.0321 | [-0.0754, +0.0146] |
| plug | SEMp:t1 | LX[IDR,T1s] | 0.7438 | 0.6835 | +0.0603 | [+0.0152, +0.1033] |
| httpx | SEMp:t1 | SELf2t | 0.6147 | 0.5999 | +0.0148 | [-0.0205, +0.0522] |
| sinatra | SEMp:t1 | SELf2t | 0.5894 | 0.6504 | -0.0610 | [-0.1059, -0.0131] |
| plug | SEMp:t1 | SELf2t | 0.7438 | 0.7082 | +0.0356 | [-0.0084, +0.0807] |
| httpx | SEMp:t1 | ANS:find_files | 0.6147 | 0.6107 | +0.0041 | [-0.0312, +0.0428] |
| sinatra | SEMp:t1 | ANS:find_files | 0.5894 | 0.6315 | -0.0421 | [-0.0844, +0.0062] |
| plug | SEMp:t1 | ANS:find_files | 0.7438 | 0.6727 | +0.0711 | [+0.0265, +0.1132] |
| httpx | SEMp:t1 | ANS:find_files@sel | 0.6147 | 0.6050 | +0.0097 | [-0.0251, +0.0478] |
| sinatra | SEMp:t1 | ANS:find_files@sel | 0.5894 | 0.6605 | -0.0711 | [-0.1142, -0.0231] |
| plug | SEMp:t1 | ANS:find_files@sel | 0.7438 | 0.7088 | +0.0351 | [-0.0089, +0.0802] |
| httpx | SEMp:t2 | SEMrand:t1 | 0.6546 | 0.2825 | +0.3721 | [+0.3311, +0.4189] |
| sinatra | SEMp:t2 | SEMrand:t1 | 0.6372 | 0.1668 | +0.4704 | [+0.4198, +0.5236] |
| plug | SEMp:t2 | SEMrand:t1 | 0.7799 | 0.2843 | +0.4956 | [+0.4471, +0.5437] |
| httpx | SEMp:t2 | T1s | 0.6546 | 0.6034 | +0.0512 | [+0.0170, +0.0921] |
| sinatra | SEMp:t2 | T1s | 0.6372 | 0.6215 | +0.0156 | [-0.0296, +0.0603] |
| plug | SEMp:t2 | T1s | 0.7799 | 0.6835 | +0.0964 | [+0.0542, +0.1395] |
| httpx | FUSE[T1s,SEMp:t2] | T1s | 0.7064 | 0.6034 | +0.1030 | [+0.0794, +0.1307] |
| sinatra | FUSE[T1s,SEMp:t2] | T1s | 0.7278 | 0.6215 | +0.1063 | [+0.0752, +0.1390] |
| plug | FUSE[T1s,SEMp:t2] | T1s | 0.7967 | 0.6835 | +0.1132 | [+0.0823, +0.1468] |
| httpx | SEMp:t2 | ANS:find | 0.6546 | 0.2517 | +0.4029 | [+0.3554, +0.4491] |
| sinatra | SEMp:t2 | ANS:find | 0.6372 | 0.1713 | +0.4658 | [+0.4152, +0.5154] |
| plug | SEMp:t2 | ANS:find | 0.7799 | 0.1686 | +0.6113 | [+0.5671, +0.6563] |
| httpx | SEMp:t2 | LX[IDR,T1s] | 0.6546 | 0.6034 | +0.0512 | [+0.0170, +0.0921] |
| sinatra | SEMp:t2 | LX[IDR,T1s] | 0.6372 | 0.6215 | +0.0156 | [-0.0296, +0.0603] |
| plug | SEMp:t2 | LX[IDR,T1s] | 0.7799 | 0.6835 | +0.0964 | [+0.0542, +0.1395] |
| httpx | SEMp:t2 | SELf2t | 0.6546 | 0.5999 | +0.0546 | [+0.0189, +0.0954] |
| sinatra | SEMp:t2 | SELf2t | 0.6372 | 0.6504 | -0.0132 | [-0.0611, +0.0340] |
| plug | SEMp:t2 | SELf2t | 0.7799 | 0.7082 | +0.0717 | [+0.0286, +0.1172] |
| httpx | SEMp:t2 | ANS:find_files | 0.6546 | 0.6107 | +0.0439 | [+0.0081, +0.0872] |
| sinatra | SEMp:t2 | ANS:find_files | 0.6372 | 0.6315 | +0.0057 | [-0.0406, +0.0507] |
| plug | SEMp:t2 | ANS:find_files | 0.7799 | 0.6727 | +0.1072 | [+0.0636, +0.1481] |
| httpx | SEMp:t2 | ANS:find_files@sel | 0.6546 | 0.6050 | +0.0496 | [+0.0132, +0.0912] |
| sinatra | SEMp:t2 | ANS:find_files@sel | 0.6372 | 0.6605 | -0.0233 | [-0.0704, +0.0233] |
| plug | SEMp:t2 | ANS:find_files@sel | 0.7799 | 0.7088 | +0.0711 | [+0.0293, +0.1160] |

PREVIEW (not the verdict): R1=yes (3/3) R2=yes (3/3) -> PASS
<!-- /fusion-gate:preview -->

Preview notes: sinatra's and plug's vectors were embedded on the same GPU (`[embed] device=NVIDIA GeForce
GTX 1660 SUPER backend=webgpu .. fallback=none`) in the run's first attempt, which was stopped before any
pair was computed when pallets/flask (264 test-eligible commits, under the packet's 300) was replaced by
encode/httpx (commit ebfbb21a); this run read them from the cache. IDR moves no recall@10 on any of the three
(F2 - F1 = 0 exactly, LX[IDR,T1s] r@10 = T1s r@10 on each), so on this panel R2 reads the same numbers as
R1.

<!-- fusion-gate:verdict -->
## Verdict

Not run yet: the final step at the end of 0.5.4 pins the verdict rows of bench/rank-gate/fusion-preview.json
and runs `bash scripts/embedding-gate.sh fusion-verdict`.
<!-- /fusion-gate:verdict -->

VERDICT: pending (the end-of-0.5.4 run; the PREVIEW is never a verdict)
