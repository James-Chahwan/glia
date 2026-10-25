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
  VERDICT (fusion-verdict; panel ruling FB-6b-PANEL, James 2026-10-25, before any verdict run): nine
  repos, the `verdict` rows of fusion-preview.json. Two are FB.6 snapshots with a full forward window
  (commons-lang, commons-io: `--test-after <FB.6's rev>`, every test commit newer than it; the report
  lists each window's first / last sha and the intersection with the prior window, which must be 0, else
  the judge refuses). Seven are fresh (`"fresh": true`, `after` "-"): public repos never in any gate
  panel, each with >= 300 test-eligible commits, pinned at their default branch's head, test window
  rank_eval's default. The other four FB.6 snapshots (24..88 commits after their revs) and the three
  preview repos (0 after theirs) are out: too few held-out commits for a 300-commit window.
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
## Verdict (two forward windows and seven fresh public repos)

| repo | language | rev | test commits | queries | first test sha | last test sha | prior window | shared with prior |
|---|---|---|---:|---:|---|---|---|---:|
| commons-lang | java | 4b1d8cb83c26 | 300 | 300 | ae061818e5bb | 4b1d8cb83c26 | 300 commits, 346aae3b3288 .. a410aab65159 | 0 |
| commons-io | java | f9d4c7567f91 | 300 | 300 | bd706ae09ea8 | e4e4fbdc3791 | 300 commits, 073bda909983 .. 8db20da2ea53 | 0 |
| guzzle | php | 89f429318d52 | 300 | 300 | acd0bcded670 | 1e0396c29269 | none (fresh: never in a panel) | 0 |
| zod | typescript | afdac65e0f80 | 300 | 300 | 2849df8907b0 | 680d19f7d1e0 | none (fresh: never in a panel) | 0 |
| okhttp | kotlin | c3179301fc7d | 300 | 300 | b926a030c628 | fb34d4da7462 | none (fresh: never in a panel) | 0 |
| sttp | scala | d06f987834af | 150 | 150 | 6fba8d10266e | 12c7fed697fa | none (fresh: never in a panel) | 0 |
| AutoMapper | csharp | f0ecba7a97d1 | 300 | 300 | 25afaf7e6dc6 | fec985c44d16 | none (fresh: never in a panel) | 0 |
| jq | c | bd56a745624a | 102 | 102 | 154d0ee4a824 | 852c4d609611 | none (fresh: never in a panel) | 0 |
| Alamofire | swift | bda9ed57d729 | 232 | 232 | c20b5602a3d5 | 3494b7c1d4e0 | none (fresh: never in a panel) | 0 |

Device per repo (FB.14's `[embed] device=` line of each run):

- commons-lang: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- commons-io: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- guzzle: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- zod: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- okhttp: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- sttp: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- AutoMapper: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- jq: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- Alamofire: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`

### Rule rows

R1 (FUSE[T1s,SEMp:t2] - T1s, lo > 0 on >= 8 of 9, hi >= 0 on all): lo > 0 on 8, hi >= 0 on 9 -> yes. R2 (LX[IDR,FUSE[T1s,SEMp:t2]] - LX[IDR,T1s], lo > 0 on >= 5 of 9, hi >= 0 on all): lo > 0 on 8, hi >= 0 on 9 -> yes.

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| commons-lang | FUSE[T1s,SEMp:t2] | T1s | 0.7704 | 0.6317 | +0.1387 | [+0.1102, +0.1724] |
| commons-io | FUSE[T1s,SEMp:t2] | T1s | 0.5151 | 0.4229 | +0.0922 | [+0.0659, +0.1203] |
| guzzle | FUSE[T1s,SEMp:t2] | T1s | 0.7847 | 0.7454 | +0.0393 | [+0.0132, +0.0666] |
| zod | FUSE[T1s,SEMp:t2] | T1s | 0.6419 | 0.5407 | +0.1012 | [+0.0716, +0.1309] |
| okhttp | FUSE[T1s,SEMp:t2] | T1s | 0.3975 | 0.2624 | +0.1351 | [+0.1078, +0.1639] |
| sttp | FUSE[T1s,SEMp:t2] | T1s | 0.3542 | 0.1775 | +0.1768 | [+0.1329, +0.2293] |
| AutoMapper | FUSE[T1s,SEMp:t2] | T1s | 0.3994 | 0.3650 | +0.0344 | [+0.0082, +0.0611] |
| jq | FUSE[T1s,SEMp:t2] | T1s | 0.7586 | 0.6482 | +0.1104 | [+0.0451, +0.1827] |
| Alamofire | FUSE[T1s,SEMp:t2] | T1s | 0.5047 | 0.5209 | -0.0161 | [-0.0510, +0.0166] |
| commons-lang | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.7704 | 0.6317 | +0.1387 | [+0.1102, +0.1724] |
| commons-io | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.5151 | 0.4229 | +0.0922 | [+0.0659, +0.1203] |
| guzzle | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.7847 | 0.7454 | +0.0393 | [+0.0132, +0.0666] |
| zod | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.6419 | 0.5407 | +0.1012 | [+0.0716, +0.1309] |
| okhttp | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.3975 | 0.2624 | +0.1351 | [+0.1078, +0.1639] |
| sttp | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.3542 | 0.1775 | +0.1768 | [+0.1329, +0.2293] |
| AutoMapper | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.3994 | 0.3650 | +0.0344 | [+0.0082, +0.0611] |
| jq | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.7586 | 0.6482 | +0.1104 | [+0.0451, +0.1827] |
| Alamofire | LX[IDR,FUSE[T1s,SEMp:t2]] | LX[IDR,T1s] | 0.5047 | 0.5209 | -0.0161 | [-0.0510, +0.0166] |

### Information rows (never the verdict)

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| commons-lang | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.7704 | 0.6291 | +0.1413 | [+0.1106, +0.1764] |
| commons-io | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.5151 | 0.4879 | +0.0271 | [-0.0174, +0.0674] |
| guzzle | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.7847 | 0.7360 | +0.0487 | [+0.0241, +0.0802] |
| zod | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.6419 | 0.5248 | +0.1171 | [+0.0862, +0.1479] |
| okhttp | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.3975 | 0.2940 | +0.1035 | [+0.0740, +0.1366] |
| sttp | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.3542 | 0.2636 | +0.0906 | [+0.0263, +0.1576] |
| AutoMapper | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.3994 | 0.4516 | -0.0521 | [-0.0870, -0.0152] |
| jq | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.7586 | 0.6912 | +0.0674 | [+0.0070, +0.1318] |
| Alamofire | LX[IDR,FUSE[T1s,SEMp:t2]] | SELf2t | 0.5047 | 0.5511 | -0.0464 | [-0.0867, -0.0104] |
| commons-lang | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.7704 | 0.7548 | +0.0156 | [-0.0080, +0.0398] |
| commons-io | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.5151 | 0.4982 | +0.0169 | [-0.0149, +0.0468] |
| guzzle | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.7847 | 0.6014 | +0.1833 | [+0.1563, +0.2114] |
| zod | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.6419 | 0.4754 | +0.1665 | [+0.1341, +0.2001] |
| okhttp | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.3975 | 0.3920 | +0.0055 | [-0.0304, +0.0451] |
| sttp | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.3542 | 0.4391 | -0.0849 | [-0.1478, -0.0194] |
| AutoMapper | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.3994 | 0.2722 | +0.1272 | [+0.0969, +0.1581] |
| jq | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.7586 | 0.6854 | +0.0732 | [+0.0222, +0.1229] |
| Alamofire | FUSE[T1s,SEMp:t2] | SEMp:t2 | 0.5047 | 0.3186 | +0.1861 | [+0.1469, +0.2281] |
| commons-lang | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.7704 | 0.7704 | +0.0000 | [+0.0000, +0.0000] |
| commons-io | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.5151 | 0.5151 | +0.0000 | [+0.0000, +0.0000] |
| guzzle | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.7847 | 0.7847 | +0.0000 | [+0.0000, +0.0000] |
| zod | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.6419 | 0.6419 | +0.0000 | [+0.0000, +0.0000] |
| okhttp | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.3975 | 0.3975 | +0.0000 | [+0.0000, +0.0000] |
| sttp | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.3542 | 0.3542 | +0.0000 | [+0.0000, +0.0000] |
| AutoMapper | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.3994 | 0.3994 | +0.0000 | [+0.0000, +0.0000] |
| jq | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.7586 | 0.7586 | +0.0000 | [+0.0000, +0.0000] |
| Alamofire | LX[IDR,FUSE[T1s,SEMp:t2]] | FUSE[T1s,SEMp:t2] | 0.5047 | 0.5047 | +0.0000 | [+0.0000, +0.0000] |

### Every FB.6 pair on these windows (information)

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| commons-lang | SEM:t2 | SEM:t1 | 0.7595 | 0.6902 | +0.0693 | [+0.0472, +0.0920] |
| commons-io | SEM:t2 | SEM:t1 | 0.4930 | 0.4576 | +0.0354 | [+0.0174, +0.0553] |
| guzzle | SEM:t2 | SEM:t1 | 0.5949 | 0.5506 | +0.0444 | [+0.0155, +0.0707] |
| zod | SEM:t2 | SEM:t1 | 0.4613 | 0.3885 | +0.0728 | [+0.0430, +0.1038] |
| okhttp | SEM:t2 | SEM:t1 | 0.3682 | 0.3416 | +0.0266 | [+0.0051, +0.0460] |
| sttp | SEM:t2 | SEM:t1 | 0.4140 | 0.3446 | +0.0694 | [+0.0355, +0.1016] |
| AutoMapper | SEM:t2 | SEM:t1 | 0.2512 | 0.2449 | +0.0064 | [-0.0133, +0.0268] |
| jq | SEM:t2 | SEM:t1 | 0.6743 | 0.5725 | +0.1017 | [+0.0541, +0.1514] |
| Alamofire | SEM:t2 | SEM:t1 | 0.3084 | 0.2515 | +0.0569 | [+0.0299, +0.0862] |
| commons-lang | SEMp:t1 | SEM:t1 | 0.7052 | 0.6902 | +0.0150 | [+0.0018, +0.0298] |
| commons-io | SEMp:t1 | SEM:t1 | 0.4751 | 0.4576 | +0.0175 | [+0.0019, +0.0351] |
| guzzle | SEMp:t1 | SEM:t1 | 0.5407 | 0.5506 | -0.0099 | [-0.0335, +0.0124] |
| zod | SEMp:t1 | SEM:t1 | 0.4126 | 0.3885 | +0.0241 | [+0.0022, +0.0459] |
| okhttp | SEMp:t1 | SEM:t1 | 0.3654 | 0.3416 | +0.0238 | [+0.0085, +0.0406] |
| sttp | SEMp:t1 | SEM:t1 | 0.3696 | 0.3446 | +0.0250 | [+0.0009, +0.0519] |
| AutoMapper | SEMp:t1 | SEM:t1 | 0.2515 | 0.2449 | +0.0067 | [-0.0085, +0.0216] |
| jq | SEMp:t1 | SEM:t1 | 0.5640 | 0.5725 | -0.0086 | [-0.0415, +0.0217] |
| Alamofire | SEMp:t1 | SEM:t1 | 0.2545 | 0.2515 | +0.0030 | [-0.0149, +0.0237] |
| commons-lang | SEM:t1 | SEMrand:t1 | 0.6902 | 0.0403 | +0.6499 | [+0.6031, +0.6961] |
| commons-io | SEM:t1 | SEMrand:t1 | 0.4576 | 0.0420 | +0.4156 | [+0.3653, +0.4660] |
| guzzle | SEM:t1 | SEMrand:t1 | 0.5506 | 0.3081 | +0.2425 | [+0.1967, +0.2906] |
| zod | SEM:t1 | SEMrand:t1 | 0.3885 | 0.1544 | +0.2341 | [+0.1961, +0.2734] |
| okhttp | SEM:t1 | SEMrand:t1 | 0.3416 | 0.0328 | +0.3088 | [+0.2720, +0.3484] |
| sttp | SEM:t1 | SEMrand:t1 | 0.3446 | 0.0395 | +0.3052 | [+0.2446, +0.3671] |
| AutoMapper | SEM:t1 | SEMrand:t1 | 0.2449 | 0.0403 | +0.2046 | [+0.1756, +0.2371] |
| jq | SEM:t1 | SEMrand:t1 | 0.5725 | 0.4321 | +0.1404 | [+0.0677, +0.2159] |
| Alamofire | SEM:t1 | SEMrand:t1 | 0.2515 | 0.1515 | +0.1000 | [+0.0500, +0.1470] |
| commons-lang | SEM:t1 | T1s | 0.6902 | 0.6317 | +0.0585 | [+0.0123, +0.1027] |
| commons-io | SEM:t1 | T1s | 0.4576 | 0.4229 | +0.0347 | [-0.0065, +0.0780] |
| guzzle | SEM:t1 | T1s | 0.5506 | 0.7454 | -0.1948 | [-0.2354, -0.1540] |
| zod | SEM:t1 | T1s | 0.3885 | 0.5407 | -0.1522 | [-0.1982, -0.1080] |
| okhttp | SEM:t1 | T1s | 0.3416 | 0.2624 | +0.0792 | [+0.0313, +0.1270] |
| sttp | SEM:t1 | T1s | 0.3446 | 0.1775 | +0.1672 | [+0.0972, +0.2494] |
| AutoMapper | SEM:t1 | T1s | 0.2449 | 0.3650 | -0.1201 | [-0.1580, -0.0784] |
| jq | SEM:t1 | T1s | 0.5725 | 0.6482 | -0.0756 | [-0.1518, +0.0052] |
| Alamofire | SEM:t1 | T1s | 0.2515 | 0.5209 | -0.2694 | [-0.3312, -0.2078] |
| commons-lang | FUSE[T1s,SEM:t1] | T1s | 0.7474 | 0.6317 | +0.1157 | [+0.0875, +0.1465] |
| commons-io | FUSE[T1s,SEM:t1] | T1s | 0.5045 | 0.4229 | +0.0816 | [+0.0548, +0.1101] |
| guzzle | FUSE[T1s,SEM:t1] | T1s | 0.7506 | 0.7454 | +0.0052 | [-0.0217, +0.0331] |
| zod | FUSE[T1s,SEM:t1] | T1s | 0.5950 | 0.5407 | +0.0543 | [+0.0264, +0.0838] |
| okhttp | FUSE[T1s,SEM:t1] | T1s | 0.3653 | 0.2624 | +0.1029 | [+0.0775, +0.1301] |
| sttp | FUSE[T1s,SEM:t1] | T1s | 0.3127 | 0.1775 | +0.1352 | [+0.0923, +0.1836] |
| AutoMapper | FUSE[T1s,SEM:t1] | T1s | 0.3790 | 0.3650 | +0.0140 | [-0.0108, +0.0387] |
| jq | FUSE[T1s,SEM:t1] | T1s | 0.7018 | 0.6482 | +0.0536 | [-0.0096, +0.1188] |
| Alamofire | FUSE[T1s,SEM:t1] | T1s | 0.4910 | 0.5209 | -0.0298 | [-0.0655, +0.0032] |
| commons-lang | SEM:t1 | ANS:find | 0.6902 | 0.0333 | +0.6569 | [+0.6111, +0.7020] |
| commons-io | SEM:t1 | ANS:find | 0.4576 | 0.0189 | +0.4387 | [+0.3902, +0.4872] |
| guzzle | SEM:t1 | ANS:find | 0.5506 | 0.1504 | +0.4002 | [+0.3503, +0.4481] |
| zod | SEM:t1 | ANS:find | 0.3885 | 0.0018 | +0.3867 | [+0.3525, +0.4224] |
| okhttp | SEM:t1 | ANS:find | 0.3416 | 0.0239 | +0.3177 | [+0.2804, +0.3590] |
| sttp | SEM:t1 | ANS:find | 0.3446 | 0.0061 | +0.3385 | [+0.2824, +0.3984] |
| AutoMapper | SEM:t1 | ANS:find | 0.2449 | 0.0261 | +0.2188 | [+0.1877, +0.2514] |
| jq | SEM:t1 | ANS:find | 0.5725 | 0.2186 | +0.3540 | [+0.2894, +0.4211] |
| Alamofire | SEM:t1 | ANS:find | 0.2515 | 0.1213 | +0.1301 | [+0.0816, +0.1779] |
| commons-lang | SEM:t1 | LX[IDR,T1s] | 0.6902 | 0.6317 | +0.0585 | [+0.0123, +0.1027] |
| commons-io | SEM:t1 | LX[IDR,T1s] | 0.4576 | 0.4229 | +0.0347 | [-0.0065, +0.0780] |
| guzzle | SEM:t1 | LX[IDR,T1s] | 0.5506 | 0.7454 | -0.1948 | [-0.2354, -0.1540] |
| zod | SEM:t1 | LX[IDR,T1s] | 0.3885 | 0.5407 | -0.1522 | [-0.1982, -0.1080] |
| okhttp | SEM:t1 | LX[IDR,T1s] | 0.3416 | 0.2624 | +0.0792 | [+0.0313, +0.1270] |
| sttp | SEM:t1 | LX[IDR,T1s] | 0.3446 | 0.1775 | +0.1672 | [+0.0972, +0.2494] |
| AutoMapper | SEM:t1 | LX[IDR,T1s] | 0.2449 | 0.3650 | -0.1201 | [-0.1580, -0.0784] |
| jq | SEM:t1 | LX[IDR,T1s] | 0.5725 | 0.6482 | -0.0756 | [-0.1518, +0.0052] |
| Alamofire | SEM:t1 | LX[IDR,T1s] | 0.2515 | 0.5209 | -0.2694 | [-0.3312, -0.2078] |
| commons-lang | SEM:t1 | SELf2t | 0.6902 | 0.6291 | +0.0611 | [+0.0129, +0.1076] |
| commons-io | SEM:t1 | SELf2t | 0.4576 | 0.4879 | -0.0304 | [-0.0816, +0.0221] |
| guzzle | SEM:t1 | SELf2t | 0.5506 | 0.7360 | -0.1854 | [-0.2262, -0.1440] |
| zod | SEM:t1 | SELf2t | 0.3885 | 0.5248 | -0.1363 | [-0.1820, -0.0899] |
| okhttp | SEM:t1 | SELf2t | 0.3416 | 0.2940 | +0.0476 | [-0.0023, +0.0988] |
| sttp | SEM:t1 | SELf2t | 0.3446 | 0.2636 | +0.0810 | [-0.0030, +0.1743] |
| AutoMapper | SEM:t1 | SELf2t | 0.2449 | 0.4516 | -0.2067 | [-0.2486, -0.1607] |
| jq | SEM:t1 | SELf2t | 0.5725 | 0.6912 | -0.1186 | [-0.1905, -0.0417] |
| Alamofire | SEM:t1 | SELf2t | 0.2515 | 0.5511 | -0.2997 | [-0.3646, -0.2369] |
| commons-lang | SEM:t1 | ANS:find_files | 0.6902 | 0.5475 | +0.1427 | [+0.0950, +0.1962] |
| commons-io | SEM:t1 | ANS:find_files | 0.4576 | 0.3949 | +0.0627 | [+0.0162, +0.1094] |
| guzzle | SEM:t1 | ANS:find_files | 0.5506 | 0.7395 | -0.1890 | [-0.2305, -0.1461] |
| zod | SEM:t1 | ANS:find_files | 0.3885 | 0.5378 | -0.1493 | [-0.1976, -0.1035] |
| okhttp | SEM:t1 | ANS:find_files | 0.3416 | 0.2442 | +0.0974 | [+0.0503, +0.1456] |
| sttp | SEM:t1 | ANS:find_files | 0.3446 | 0.1684 | +0.1762 | [+0.1055, +0.2576] |
| AutoMapper | SEM:t1 | ANS:find_files | 0.2449 | 0.3619 | -0.1170 | [-0.1578, -0.0731] |
| jq | SEM:t1 | ANS:find_files | 0.5725 | 0.6259 | -0.0534 | [-0.1312, +0.0229] |
| Alamofire | SEM:t1 | ANS:find_files | 0.2515 | 0.5200 | -0.2685 | [-0.3305, -0.2067] |
| commons-lang | SEM:t1 | ANS:find_files@sel | 0.6902 | 0.5583 | +0.1319 | [+0.0822, +0.1859] |
| commons-io | SEM:t1 | ANS:find_files@sel | 0.4576 | 0.4718 | -0.0142 | [-0.0686, +0.0427] |
| guzzle | SEM:t1 | ANS:find_files@sel | 0.5506 | 0.7339 | -0.1833 | [-0.2235, -0.1409] |
| zod | SEM:t1 | ANS:find_files@sel | 0.3885 | 0.5236 | -0.1350 | [-0.1820, -0.0889] |
| okhttp | SEM:t1 | ANS:find_files@sel | 0.3416 | 0.2876 | +0.0541 | [+0.0032, +0.1055] |
| sttp | SEM:t1 | ANS:find_files@sel | 0.3446 | 0.2604 | +0.0842 | [+0.0021, +0.1742] |
| AutoMapper | SEM:t1 | ANS:find_files@sel | 0.2449 | 0.4394 | -0.1946 | [-0.2406, -0.1463] |
| jq | SEM:t1 | ANS:find_files@sel | 0.5725 | 0.6741 | -0.1016 | [-0.1753, -0.0257] |
| Alamofire | SEM:t1 | ANS:find_files@sel | 0.2515 | 0.5492 | -0.2978 | [-0.3610, -0.2359] |
| commons-lang | SEM:t2 | SEMrand:t1 | 0.7595 | 0.0403 | +0.7191 | [+0.6700, +0.7677] |
| commons-io | SEM:t2 | SEMrand:t1 | 0.4930 | 0.0420 | +0.4510 | [+0.4001, +0.5041] |
| guzzle | SEM:t2 | SEMrand:t1 | 0.5949 | 0.3081 | +0.2868 | [+0.2415, +0.3349] |
| zod | SEM:t2 | SEMrand:t1 | 0.4613 | 0.1544 | +0.3068 | [+0.2691, +0.3469] |
| okhttp | SEM:t2 | SEMrand:t1 | 0.3682 | 0.0328 | +0.3354 | [+0.2936, +0.3754] |
| sttp | SEM:t2 | SEMrand:t1 | 0.4140 | 0.0395 | +0.3746 | [+0.3120, +0.4467] |
| AutoMapper | SEM:t2 | SEMrand:t1 | 0.2512 | 0.0403 | +0.2109 | [+0.1795, +0.2465] |
| jq | SEM:t2 | SEMrand:t1 | 0.6743 | 0.4321 | +0.2421 | [+0.1688, +0.3199] |
| Alamofire | SEM:t2 | SEMrand:t1 | 0.3084 | 0.1515 | +0.1569 | [+0.1066, +0.2024] |
| commons-lang | SEM:t2 | T1s | 0.7595 | 0.6317 | +0.1278 | [+0.0871, +0.1700] |
| commons-io | SEM:t2 | T1s | 0.4930 | 0.4229 | +0.0701 | [+0.0286, +0.1131] |
| guzzle | SEM:t2 | T1s | 0.5949 | 0.7454 | -0.1504 | [-0.1880, -0.1111] |
| zod | SEM:t2 | T1s | 0.4613 | 0.5407 | -0.0794 | [-0.1251, -0.0353] |
| okhttp | SEM:t2 | T1s | 0.3682 | 0.2624 | +0.1058 | [+0.0561, +0.1572] |
| sttp | SEM:t2 | T1s | 0.4140 | 0.1775 | +0.2366 | [+0.1596, +0.3205] |
| AutoMapper | SEM:t2 | T1s | 0.2512 | 0.3650 | -0.1138 | [-0.1552, -0.0687] |
| jq | SEM:t2 | T1s | 0.6743 | 0.6482 | +0.0261 | [-0.0558, +0.1218] |
| Alamofire | SEM:t2 | T1s | 0.3084 | 0.5209 | -0.2125 | [-0.2697, -0.1568] |
| commons-lang | FUSE[T1s,SEM:t2] | T1s | 0.7656 | 0.6317 | +0.1339 | [+0.1068, +0.1659] |
| commons-io | FUSE[T1s,SEM:t2] | T1s | 0.5006 | 0.4229 | +0.0777 | [+0.0530, +0.1066] |
| guzzle | FUSE[T1s,SEM:t2] | T1s | 0.7882 | 0.7454 | +0.0428 | [+0.0178, +0.0699] |
| zod | FUSE[T1s,SEM:t2] | T1s | 0.6206 | 0.5407 | +0.0799 | [+0.0528, +0.1108] |
| okhttp | FUSE[T1s,SEM:t2] | T1s | 0.3869 | 0.2624 | +0.1245 | [+0.0966, +0.1531] |
| sttp | FUSE[T1s,SEM:t2] | T1s | 0.3424 | 0.1775 | +0.1650 | [+0.1195, +0.2175] |
| AutoMapper | FUSE[T1s,SEM:t2] | T1s | 0.3924 | 0.3650 | +0.0274 | [+0.0020, +0.0521] |
| jq | FUSE[T1s,SEM:t2] | T1s | 0.7543 | 0.6482 | +0.1062 | [+0.0454, +0.1758] |
| Alamofire | FUSE[T1s,SEM:t2] | T1s | 0.5209 | 0.5209 | +0.0000 | [-0.0347, +0.0339] |
| commons-lang | SEM:t2 | ANS:find | 0.7595 | 0.0333 | +0.7261 | [+0.6812, +0.7724] |
| commons-io | SEM:t2 | ANS:find | 0.4930 | 0.0189 | +0.4741 | [+0.4243, +0.5265] |
| guzzle | SEM:t2 | ANS:find | 0.5949 | 0.1504 | +0.4446 | [+0.3994, +0.4925] |
| zod | SEM:t2 | ANS:find | 0.4613 | 0.0018 | +0.4594 | [+0.4287, +0.4946] |
| okhttp | SEM:t2 | ANS:find | 0.3682 | 0.0239 | +0.3443 | [+0.3039, +0.3867] |
| sttp | SEM:t2 | ANS:find | 0.4140 | 0.0061 | +0.4079 | [+0.3474, +0.4789] |
| AutoMapper | SEM:t2 | ANS:find | 0.2512 | 0.0261 | +0.2251 | [+0.1932, +0.2621] |
| jq | SEM:t2 | ANS:find | 0.6743 | 0.2186 | +0.4557 | [+0.3913, +0.5258] |
| Alamofire | SEM:t2 | ANS:find | 0.3084 | 0.1213 | +0.1870 | [+0.1365, +0.2361] |
| commons-lang | SEM:t2 | LX[IDR,T1s] | 0.7595 | 0.6317 | +0.1278 | [+0.0871, +0.1700] |
| commons-io | SEM:t2 | LX[IDR,T1s] | 0.4930 | 0.4229 | +0.0701 | [+0.0286, +0.1131] |
| guzzle | SEM:t2 | LX[IDR,T1s] | 0.5949 | 0.7454 | -0.1504 | [-0.1880, -0.1111] |
| zod | SEM:t2 | LX[IDR,T1s] | 0.4613 | 0.5407 | -0.0794 | [-0.1251, -0.0353] |
| okhttp | SEM:t2 | LX[IDR,T1s] | 0.3682 | 0.2624 | +0.1058 | [+0.0561, +0.1572] |
| sttp | SEM:t2 | LX[IDR,T1s] | 0.4140 | 0.1775 | +0.2366 | [+0.1596, +0.3205] |
| AutoMapper | SEM:t2 | LX[IDR,T1s] | 0.2512 | 0.3650 | -0.1138 | [-0.1552, -0.0687] |
| jq | SEM:t2 | LX[IDR,T1s] | 0.6743 | 0.6482 | +0.0261 | [-0.0558, +0.1218] |
| Alamofire | SEM:t2 | LX[IDR,T1s] | 0.3084 | 0.5209 | -0.2125 | [-0.2697, -0.1568] |
| commons-lang | SEM:t2 | SELf2t | 0.7595 | 0.6291 | +0.1304 | [+0.0880, +0.1766] |
| commons-io | SEM:t2 | SELf2t | 0.4930 | 0.4879 | +0.0050 | [-0.0448, +0.0573] |
| guzzle | SEM:t2 | SELf2t | 0.5949 | 0.7360 | -0.1410 | [-0.1770, -0.1027] |
| zod | SEM:t2 | SELf2t | 0.4613 | 0.5248 | -0.0635 | [-0.1110, -0.0188] |
| okhttp | SEM:t2 | SELf2t | 0.3682 | 0.2940 | +0.0742 | [+0.0202, +0.1268] |
| sttp | SEM:t2 | SELf2t | 0.4140 | 0.2636 | +0.1504 | [+0.0571, +0.2473] |
| AutoMapper | SEM:t2 | SELf2t | 0.2512 | 0.4516 | -0.2003 | [-0.2436, -0.1493] |
| jq | SEM:t2 | SELf2t | 0.6743 | 0.6912 | -0.0169 | [-0.0949, +0.0722] |
| Alamofire | SEM:t2 | SELf2t | 0.3084 | 0.5511 | -0.2428 | [-0.3038, -0.1827] |
| commons-lang | SEM:t2 | ANS:find_files | 0.7595 | 0.5475 | +0.2120 | [+0.1655, +0.2598] |
| commons-io | SEM:t2 | ANS:find_files | 0.4930 | 0.3949 | +0.0981 | [+0.0540, +0.1479] |
| guzzle | SEM:t2 | ANS:find_files | 0.5949 | 0.7395 | -0.1446 | [-0.1821, -0.1054] |
| zod | SEM:t2 | ANS:find_files | 0.4613 | 0.5378 | -0.0765 | [-0.1227, -0.0333] |
| okhttp | SEM:t2 | ANS:find_files | 0.3682 | 0.2442 | +0.1240 | [+0.0734, +0.1742] |
| sttp | SEM:t2 | ANS:find_files | 0.4140 | 0.1684 | +0.2456 | [+0.1667, +0.3307] |
| AutoMapper | SEM:t2 | ANS:find_files | 0.2512 | 0.3619 | -0.1107 | [-0.1550, -0.0640] |
| jq | SEM:t2 | ANS:find_files | 0.6743 | 0.6259 | +0.0484 | [-0.0356, +0.1387] |
| Alamofire | SEM:t2 | ANS:find_files | 0.3084 | 0.5200 | -0.2116 | [-0.2702, -0.1537] |
| commons-lang | SEM:t2 | ANS:find_files@sel | 0.7595 | 0.5583 | +0.2012 | [+0.1527, +0.2521] |
| commons-io | SEM:t2 | ANS:find_files@sel | 0.4930 | 0.4718 | +0.0212 | [-0.0322, +0.0762] |
| guzzle | SEM:t2 | ANS:find_files@sel | 0.5949 | 0.7339 | -0.1390 | [-0.1765, -0.1012] |
| zod | SEM:t2 | ANS:find_files@sel | 0.4613 | 0.5236 | -0.0623 | [-0.1092, -0.0184] |
| okhttp | SEM:t2 | ANS:find_files@sel | 0.3682 | 0.2876 | +0.0806 | [+0.0280, +0.1325] |
| sttp | SEM:t2 | ANS:find_files@sel | 0.4140 | 0.2604 | +0.1536 | [+0.0643, +0.2480] |
| AutoMapper | SEM:t2 | ANS:find_files@sel | 0.2512 | 0.4394 | -0.1882 | [-0.2382, -0.1380] |
| jq | SEM:t2 | ANS:find_files@sel | 0.6743 | 0.6741 | +0.0002 | [-0.0775, +0.0915] |
| Alamofire | SEM:t2 | ANS:find_files@sel | 0.3084 | 0.5492 | -0.2409 | [-0.3022, -0.1805] |
| commons-lang | SEMp:t1 | SEMrand:t1 | 0.7052 | 0.0403 | +0.6649 | [+0.6135, +0.7094] |
| commons-io | SEMp:t1 | SEMrand:t1 | 0.4751 | 0.0420 | +0.4331 | [+0.3814, +0.4838] |
| guzzle | SEMp:t1 | SEMrand:t1 | 0.5407 | 0.3081 | +0.2326 | [+0.1864, +0.2831] |
| zod | SEMp:t1 | SEMrand:t1 | 0.4126 | 0.1544 | +0.2582 | [+0.2207, +0.2969] |
| okhttp | SEMp:t1 | SEMrand:t1 | 0.3654 | 0.0328 | +0.3326 | [+0.2926, +0.3718] |
| sttp | SEMp:t1 | SEMrand:t1 | 0.3696 | 0.0395 | +0.3302 | [+0.2702, +0.3958] |
| AutoMapper | SEMp:t1 | SEMrand:t1 | 0.2515 | 0.0403 | +0.2113 | [+0.1832, +0.2423] |
| jq | SEMp:t1 | SEMrand:t1 | 0.5640 | 0.4321 | +0.1318 | [+0.0577, +0.2092] |
| Alamofire | SEMp:t1 | SEMrand:t1 | 0.2545 | 0.1515 | +0.1030 | [+0.0543, +0.1535] |
| commons-lang | SEMp:t1 | T1s | 0.7052 | 0.6317 | +0.0735 | [+0.0288, +0.1165] |
| commons-io | SEMp:t1 | T1s | 0.4751 | 0.4229 | +0.0522 | [+0.0093, +0.0954] |
| guzzle | SEMp:t1 | T1s | 0.5407 | 0.7454 | -0.2047 | [-0.2449, -0.1669] |
| zod | SEMp:t1 | T1s | 0.4126 | 0.5407 | -0.1281 | [-0.1740, -0.0859] |
| okhttp | SEMp:t1 | T1s | 0.3654 | 0.2624 | +0.1030 | [+0.0561, +0.1515] |
| sttp | SEMp:t1 | T1s | 0.3696 | 0.1775 | +0.1922 | [+0.1233, +0.2670] |
| AutoMapper | SEMp:t1 | T1s | 0.2515 | 0.3650 | -0.1135 | [-0.1509, -0.0730] |
| jq | SEMp:t1 | T1s | 0.5640 | 0.6482 | -0.0842 | [-0.1588, -0.0046] |
| Alamofire | SEMp:t1 | T1s | 0.2545 | 0.5209 | -0.2664 | [-0.3296, -0.2062] |
| commons-lang | FUSE[T1s,SEMp:t1] | T1s | 0.7604 | 0.6317 | +0.1287 | [+0.1005, +0.1602] |
| commons-io | FUSE[T1s,SEMp:t1] | T1s | 0.5069 | 0.4229 | +0.0840 | [+0.0595, +0.1108] |
| guzzle | FUSE[T1s,SEMp:t1] | T1s | 0.7569 | 0.7454 | +0.0115 | [-0.0169, +0.0396] |
| zod | FUSE[T1s,SEMp:t1] | T1s | 0.5990 | 0.5407 | +0.0583 | [+0.0289, +0.0873] |
| okhttp | FUSE[T1s,SEMp:t1] | T1s | 0.3759 | 0.2624 | +0.1135 | [+0.0869, +0.1426] |
| sttp | FUSE[T1s,SEMp:t1] | T1s | 0.3293 | 0.1775 | +0.1519 | [+0.1086, +0.2004] |
| AutoMapper | FUSE[T1s,SEMp:t1] | T1s | 0.3888 | 0.3650 | +0.0238 | [-0.0016, +0.0501] |
| jq | FUSE[T1s,SEMp:t1] | T1s | 0.7117 | 0.6482 | +0.0635 | [+0.0042, +0.1272] |
| Alamofire | FUSE[T1s,SEMp:t1] | T1s | 0.4904 | 0.5209 | -0.0305 | [-0.0692, +0.0027] |
| commons-lang | SEMp:t1 | ANS:find | 0.7052 | 0.0333 | +0.6719 | [+0.6254, +0.7153] |
| commons-io | SEMp:t1 | ANS:find | 0.4751 | 0.0189 | +0.4562 | [+0.4074, +0.5049] |
| guzzle | SEMp:t1 | ANS:find | 0.5407 | 0.1504 | +0.3903 | [+0.3458, +0.4372] |
| zod | SEMp:t1 | ANS:find | 0.4126 | 0.0018 | +0.4108 | [+0.3780, +0.4453] |
| okhttp | SEMp:t1 | ANS:find | 0.3654 | 0.0239 | +0.3415 | [+0.3026, +0.3813] |
| sttp | SEMp:t1 | ANS:find | 0.3696 | 0.0061 | +0.3635 | [+0.3056, +0.4275] |
| AutoMapper | SEMp:t1 | ANS:find | 0.2515 | 0.0261 | +0.2255 | [+0.1949, +0.2581] |
| jq | SEMp:t1 | ANS:find | 0.5640 | 0.2186 | +0.3454 | [+0.2773, +0.4136] |
| Alamofire | SEMp:t1 | ANS:find | 0.2545 | 0.1213 | +0.1331 | [+0.0827, +0.1835] |
| commons-lang | SEMp:t1 | LX[IDR,T1s] | 0.7052 | 0.6317 | +0.0735 | [+0.0288, +0.1165] |
| commons-io | SEMp:t1 | LX[IDR,T1s] | 0.4751 | 0.4229 | +0.0522 | [+0.0093, +0.0954] |
| guzzle | SEMp:t1 | LX[IDR,T1s] | 0.5407 | 0.7454 | -0.2047 | [-0.2449, -0.1669] |
| zod | SEMp:t1 | LX[IDR,T1s] | 0.4126 | 0.5407 | -0.1281 | [-0.1740, -0.0859] |
| okhttp | SEMp:t1 | LX[IDR,T1s] | 0.3654 | 0.2624 | +0.1030 | [+0.0561, +0.1515] |
| sttp | SEMp:t1 | LX[IDR,T1s] | 0.3696 | 0.1775 | +0.1922 | [+0.1233, +0.2670] |
| AutoMapper | SEMp:t1 | LX[IDR,T1s] | 0.2515 | 0.3650 | -0.1135 | [-0.1509, -0.0730] |
| jq | SEMp:t1 | LX[IDR,T1s] | 0.5640 | 0.6482 | -0.0842 | [-0.1588, -0.0046] |
| Alamofire | SEMp:t1 | LX[IDR,T1s] | 0.2545 | 0.5209 | -0.2664 | [-0.3296, -0.2062] |
| commons-lang | SEMp:t1 | SELf2t | 0.7052 | 0.6291 | +0.0761 | [+0.0293, +0.1231] |
| commons-io | SEMp:t1 | SELf2t | 0.4751 | 0.4879 | -0.0129 | [-0.0628, +0.0393] |
| guzzle | SEMp:t1 | SELf2t | 0.5407 | 0.7360 | -0.1953 | [-0.2354, -0.1538] |
| zod | SEMp:t1 | SELf2t | 0.4126 | 0.5248 | -0.1122 | [-0.1571, -0.0679] |
| okhttp | SEMp:t1 | SELf2t | 0.3654 | 0.2940 | +0.0715 | [+0.0218, +0.1214] |
| sttp | SEMp:t1 | SELf2t | 0.3696 | 0.2636 | +0.1060 | [+0.0245, +0.1964] |
| AutoMapper | SEMp:t1 | SELf2t | 0.2515 | 0.4516 | -0.2000 | [-0.2406, -0.1567] |
| jq | SEMp:t1 | SELf2t | 0.5640 | 0.6912 | -0.1272 | [-0.1980, -0.0529] |
| Alamofire | SEMp:t1 | SELf2t | 0.2545 | 0.5511 | -0.2967 | [-0.3603, -0.2338] |
| commons-lang | SEMp:t1 | ANS:find_files | 0.7052 | 0.5475 | +0.1577 | [+0.1083, +0.2107] |
| commons-io | SEMp:t1 | ANS:find_files | 0.4751 | 0.3949 | +0.0802 | [+0.0354, +0.1256] |
| guzzle | SEMp:t1 | ANS:find_files | 0.5407 | 0.7395 | -0.1988 | [-0.2393, -0.1610] |
| zod | SEMp:t1 | ANS:find_files | 0.4126 | 0.5378 | -0.1252 | [-0.1717, -0.0822] |
| okhttp | SEMp:t1 | ANS:find_files | 0.3654 | 0.2442 | +0.1212 | [+0.0734, +0.1722] |
| sttp | SEMp:t1 | ANS:find_files | 0.3696 | 0.1684 | +0.2012 | [+0.1325, +0.2818] |
| AutoMapper | SEMp:t1 | ANS:find_files | 0.2515 | 0.3619 | -0.1104 | [-0.1501, -0.0664] |
| jq | SEMp:t1 | ANS:find_files | 0.5640 | 0.6259 | -0.0619 | [-0.1356, +0.0152] |
| Alamofire | SEMp:t1 | ANS:find_files | 0.2545 | 0.5200 | -0.2655 | [-0.3299, -0.2040] |
| commons-lang | SEMp:t1 | ANS:find_files@sel | 0.7052 | 0.5583 | +0.1469 | [+0.0954, +0.1999] |
| commons-io | SEMp:t1 | ANS:find_files@sel | 0.4751 | 0.4718 | +0.0033 | [-0.0496, +0.0596] |
| guzzle | SEMp:t1 | ANS:find_files@sel | 0.5407 | 0.7339 | -0.1932 | [-0.2324, -0.1521] |
| zod | SEMp:t1 | ANS:find_files@sel | 0.4126 | 0.5236 | -0.1109 | [-0.1574, -0.0661] |
| okhttp | SEMp:t1 | ANS:find_files@sel | 0.3654 | 0.2876 | +0.0779 | [+0.0267, +0.1288] |
| sttp | SEMp:t1 | ANS:find_files@sel | 0.3696 | 0.2604 | +0.1093 | [+0.0263, +0.1956] |
| AutoMapper | SEMp:t1 | ANS:find_files@sel | 0.2515 | 0.4394 | -0.1879 | [-0.2329, -0.1413] |
| jq | SEMp:t1 | ANS:find_files@sel | 0.5640 | 0.6741 | -0.1101 | [-0.1810, -0.0320] |
| Alamofire | SEMp:t1 | ANS:find_files@sel | 0.2545 | 0.5492 | -0.2948 | [-0.3576, -0.2310] |
| commons-lang | SEMp:t2 | SEMrand:t1 | 0.7548 | 0.0403 | +0.7145 | [+0.6631, +0.7607] |
| commons-io | SEMp:t2 | SEMrand:t1 | 0.4982 | 0.0420 | +0.4562 | [+0.4044, +0.5096] |
| guzzle | SEMp:t2 | SEMrand:t1 | 0.6014 | 0.3081 | +0.2933 | [+0.2475, +0.3394] |
| zod | SEMp:t2 | SEMrand:t1 | 0.4754 | 0.1544 | +0.3210 | [+0.2846, +0.3586] |
| okhttp | SEMp:t2 | SEMrand:t1 | 0.3920 | 0.0328 | +0.3592 | [+0.3192, +0.4006] |
| sttp | SEMp:t2 | SEMrand:t1 | 0.4391 | 0.0395 | +0.3996 | [+0.3324, +0.4659] |
| AutoMapper | SEMp:t2 | SEMrand:t1 | 0.2722 | 0.0403 | +0.2320 | [+0.2002, +0.2685] |
| jq | SEMp:t2 | SEMrand:t1 | 0.6854 | 0.4321 | +0.2532 | [+0.1849, +0.3300] |
| Alamofire | SEMp:t2 | SEMrand:t1 | 0.3186 | 0.1515 | +0.1672 | [+0.1181, +0.2126] |
| commons-lang | SEMp:t2 | T1s | 0.7548 | 0.6317 | +0.1231 | [+0.0852, +0.1654] |
| commons-io | SEMp:t2 | T1s | 0.4982 | 0.4229 | +0.0753 | [+0.0343, +0.1216] |
| guzzle | SEMp:t2 | T1s | 0.6014 | 0.7454 | -0.1440 | [-0.1801, -0.1028] |
| zod | SEMp:t2 | T1s | 0.4754 | 0.5407 | -0.0653 | [-0.1131, -0.0219] |
| okhttp | SEMp:t2 | T1s | 0.3920 | 0.2624 | +0.1296 | [+0.0782, +0.1805] |
| sttp | SEMp:t2 | T1s | 0.4391 | 0.1775 | +0.2616 | [+0.1858, +0.3466] |
| AutoMapper | SEMp:t2 | T1s | 0.2722 | 0.3650 | -0.0928 | [-0.1326, -0.0504] |
| jq | SEMp:t2 | T1s | 0.6854 | 0.6482 | +0.0372 | [-0.0446, +0.1252] |
| Alamofire | SEMp:t2 | T1s | 0.3186 | 0.5209 | -0.2023 | [-0.2665, -0.1440] |
| commons-lang | FUSE[T1s,SEMp:t2] | T1s | 0.7704 | 0.6317 | +0.1387 | [+0.1102, +0.1724] |
| commons-io | FUSE[T1s,SEMp:t2] | T1s | 0.5151 | 0.4229 | +0.0922 | [+0.0659, +0.1203] |
| guzzle | FUSE[T1s,SEMp:t2] | T1s | 0.7847 | 0.7454 | +0.0393 | [+0.0132, +0.0666] |
| zod | FUSE[T1s,SEMp:t2] | T1s | 0.6419 | 0.5407 | +0.1012 | [+0.0716, +0.1309] |
| okhttp | FUSE[T1s,SEMp:t2] | T1s | 0.3975 | 0.2624 | +0.1351 | [+0.1078, +0.1639] |
| sttp | FUSE[T1s,SEMp:t2] | T1s | 0.3542 | 0.1775 | +0.1768 | [+0.1329, +0.2293] |
| AutoMapper | FUSE[T1s,SEMp:t2] | T1s | 0.3994 | 0.3650 | +0.0344 | [+0.0082, +0.0611] |
| jq | FUSE[T1s,SEMp:t2] | T1s | 0.7586 | 0.6482 | +0.1104 | [+0.0451, +0.1827] |
| Alamofire | FUSE[T1s,SEMp:t2] | T1s | 0.5047 | 0.5209 | -0.0161 | [-0.0510, +0.0166] |
| commons-lang | SEMp:t2 | ANS:find | 0.7548 | 0.0333 | +0.7215 | [+0.6771, +0.7671] |
| commons-io | SEMp:t2 | ANS:find | 0.4982 | 0.0189 | +0.4793 | [+0.4277, +0.5306] |
| guzzle | SEMp:t2 | ANS:find | 0.6014 | 0.1504 | +0.4510 | [+0.4051, +0.4984] |
| zod | SEMp:t2 | ANS:find | 0.4754 | 0.0018 | +0.4736 | [+0.4423, +0.5067] |
| okhttp | SEMp:t2 | ANS:find | 0.3920 | 0.0239 | +0.3681 | [+0.3285, +0.4119] |
| sttp | SEMp:t2 | ANS:find | 0.4391 | 0.0061 | +0.4330 | [+0.3657, +0.5037] |
| AutoMapper | SEMp:t2 | ANS:find | 0.2722 | 0.0261 | +0.2462 | [+0.2140, +0.2820] |
| jq | SEMp:t2 | ANS:find | 0.6854 | 0.2186 | +0.4668 | [+0.4029, +0.5335] |
| Alamofire | SEMp:t2 | ANS:find | 0.3186 | 0.1213 | +0.1973 | [+0.1458, +0.2460] |
| commons-lang | SEMp:t2 | LX[IDR,T1s] | 0.7548 | 0.6317 | +0.1231 | [+0.0852, +0.1654] |
| commons-io | SEMp:t2 | LX[IDR,T1s] | 0.4982 | 0.4229 | +0.0753 | [+0.0343, +0.1216] |
| guzzle | SEMp:t2 | LX[IDR,T1s] | 0.6014 | 0.7454 | -0.1440 | [-0.1801, -0.1028] |
| zod | SEMp:t2 | LX[IDR,T1s] | 0.4754 | 0.5407 | -0.0653 | [-0.1131, -0.0219] |
| okhttp | SEMp:t2 | LX[IDR,T1s] | 0.3920 | 0.2624 | +0.1296 | [+0.0782, +0.1805] |
| sttp | SEMp:t2 | LX[IDR,T1s] | 0.4391 | 0.1775 | +0.2616 | [+0.1858, +0.3466] |
| AutoMapper | SEMp:t2 | LX[IDR,T1s] | 0.2722 | 0.3650 | -0.0928 | [-0.1326, -0.0504] |
| jq | SEMp:t2 | LX[IDR,T1s] | 0.6854 | 0.6482 | +0.0372 | [-0.0446, +0.1252] |
| Alamofire | SEMp:t2 | LX[IDR,T1s] | 0.3186 | 0.5209 | -0.2023 | [-0.2665, -0.1440] |
| commons-lang | SEMp:t2 | SELf2t | 0.7548 | 0.6291 | +0.1257 | [+0.0841, +0.1704] |
| commons-io | SEMp:t2 | SELf2t | 0.4982 | 0.4879 | +0.0102 | [-0.0397, +0.0642] |
| guzzle | SEMp:t2 | SELf2t | 0.6014 | 0.7360 | -0.1346 | [-0.1706, -0.0933] |
| zod | SEMp:t2 | SELf2t | 0.4754 | 0.5248 | -0.0494 | [-0.0972, -0.0045] |
| okhttp | SEMp:t2 | SELf2t | 0.3920 | 0.2940 | +0.0981 | [+0.0459, +0.1497] |
| sttp | SEMp:t2 | SELf2t | 0.4391 | 0.2636 | +0.1755 | [+0.0827, +0.2700] |
| AutoMapper | SEMp:t2 | SELf2t | 0.2722 | 0.4516 | -0.1793 | [-0.2231, -0.1293] |
| jq | SEMp:t2 | SELf2t | 0.6854 | 0.6912 | -0.0058 | [-0.0853, +0.0812] |
| Alamofire | SEMp:t2 | SELf2t | 0.3186 | 0.5511 | -0.2325 | [-0.2975, -0.1720] |
| commons-lang | SEMp:t2 | ANS:find_files | 0.7548 | 0.5475 | +0.2073 | [+0.1640, +0.2546] |
| commons-io | SEMp:t2 | ANS:find_files | 0.4982 | 0.3949 | +0.1033 | [+0.0566, +0.1507] |
| guzzle | SEMp:t2 | ANS:find_files | 0.6014 | 0.7395 | -0.1381 | [-0.1749, -0.0970] |
| zod | SEMp:t2 | ANS:find_files | 0.4754 | 0.5378 | -0.0624 | [-0.1088, -0.0195] |
| okhttp | SEMp:t2 | ANS:find_files | 0.3920 | 0.2442 | +0.1478 | [+0.0973, +0.1991] |
| sttp | SEMp:t2 | ANS:find_files | 0.4391 | 0.1684 | +0.2707 | [+0.1932, +0.3581] |
| AutoMapper | SEMp:t2 | ANS:find_files | 0.2722 | 0.3619 | -0.0897 | [-0.1317, -0.0444] |
| jq | SEMp:t2 | ANS:find_files | 0.6854 | 0.6259 | +0.0595 | [-0.0237, +0.1479] |
| Alamofire | SEMp:t2 | ANS:find_files | 0.3186 | 0.5200 | -0.2014 | [-0.2636, -0.1417] |
| commons-lang | SEMp:t2 | ANS:find_files@sel | 0.7548 | 0.5583 | +0.1965 | [+0.1483, +0.2477] |
| commons-io | SEMp:t2 | ANS:find_files@sel | 0.4982 | 0.4718 | +0.0264 | [-0.0279, +0.0831] |
| guzzle | SEMp:t2 | ANS:find_files@sel | 0.6014 | 0.7339 | -0.1325 | [-0.1697, -0.0882] |
| zod | SEMp:t2 | ANS:find_files@sel | 0.4754 | 0.5236 | -0.0481 | [-0.0959, -0.0039] |
| okhttp | SEMp:t2 | ANS:find_files@sel | 0.3920 | 0.2876 | +0.1045 | [+0.0528, +0.1567] |
| sttp | SEMp:t2 | ANS:find_files@sel | 0.4391 | 0.2604 | +0.1787 | [+0.0891, +0.2722] |
| AutoMapper | SEMp:t2 | ANS:find_files@sel | 0.2722 | 0.4394 | -0.1672 | [-0.2134, -0.1171] |
| jq | SEMp:t2 | ANS:find_files@sel | 0.6854 | 0.6741 | +0.0113 | [-0.0636, +0.0980] |
| Alamofire | SEMp:t2 | ANS:find_files@sel | 0.3186 | 0.5492 | -0.2306 | [-0.2945, -0.1697] |

Verdict rows: R1=yes (8/9) R2=yes (8/9) -> PASS
<!-- /fusion-gate:verdict -->

VERDICT: PASS
