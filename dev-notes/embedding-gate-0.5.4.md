# The embedding gate (0.5.4, FB.6)

Seed item 13: the semantic tier reaches an answer only if it beats BM25 alone on the rank gate's
public snapshots; otherwise `glia embed` ships as a sidecar no answer reads. Produced by
`bash scripts/embedding-gate.sh` (rank_eval `--sem on`, `--gate-pair`); every number below is an
aggregate over a public repository. Model: `bge-small`.

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

## Rankers

Find shape, one query per test commit (its subject). SEM:<r>: per file the max cosine of the raw
subject's vector over its EMBED_KINDS nodes' recipe-r vectors (`semantic::embed_text`); SEMp:<r>
the same with the BGE query prefix; SEMrand:t1 the raw subject over a fixed random unit vector per
node (h64(qname)-seeded); FUSE[T1s,X] = 0.5 minmax(T1s) + 0.5 minmax(X). T1s is BM25 over the
strictly older commit messages (the find bank's BM25-alone).

## Panel

| repo | queries | commits | t1 nodes | t2 nodes | node embed ms (first run) | query embed ms (first run) | t2 index load ms (this run) | this run embedded (t1 / t2 / queries) |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| glia | 300 | 300 | 32738 | 32738 | 3214427 | 12343 | 413 | 32026 / 32738 / 600 |
| shell | 216 | 216 | 889 | 889 | 57314 | 1328 | 4 | 887 / 889 / 428 |
| websocket | 107 | 107 | 416 | 416 | 31336 | 871 | 1 | 416 / 416 / 210 |
| samples | 298 | 298 | 8692 | 8692 | 514959 | 5314 | 79 | 8458 / 8692 / 596 |
| commons-lang | 300 | 300 | 7079 | 7079 | 807474 | 3915 | 56 | 7049 / 7079 / 586 |
| commons-io | 300 | 300 | 3596 | 3596 | 328402 | 4065 | 24 | 3554 / 3596 / 550 |

## Selection: recipe and query form

SEM:t2 - SEM:t1: lo > 0 on 2 of 6, hi >= 0 on 6 of 6 -> r* = t1. SEMp:t1 - SEM:t1: lo > 0 on 3 of 6, hi >= 0 on 6 of 6 -> query form bge. S* = SEMp:t1.

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| glia | SEM:t2 | SEM:t1 | 0.5840 | 0.5342 | +0.0499 | [+0.0282, +0.0727] |
| shell | SEM:t2 | SEM:t1 | 0.7307 | 0.6939 | +0.0367 | [+0.0102, +0.0662] |
| websocket | SEM:t2 | SEM:t1 | 0.7576 | 0.7737 | -0.0160 | [-0.0491, +0.0197] |
| samples | SEM:t2 | SEM:t1 | 0.3491 | 0.3492 | -0.0001 | [-0.0244, +0.0241] |
| commons-lang | SEM:t2 | SEM:t1 | 0.5733 | 0.5562 | +0.0171 | [-0.0076, +0.0380] |
| commons-io | SEM:t2 | SEM:t1 | 0.4432 | 0.4298 | +0.0134 | [-0.0102, +0.0389] |
| glia | SEMp:t1 | SEM:t1 | 0.5556 | 0.5342 | +0.0215 | [+0.0048, +0.0385] |
| shell | SEMp:t1 | SEM:t1 | 0.7138 | 0.6939 | +0.0199 | [-0.0001, +0.0424] |
| websocket | SEMp:t1 | SEM:t1 | 0.7946 | 0.7737 | +0.0209 | [-0.0062, +0.0479] |
| samples | SEMp:t1 | SEM:t1 | 0.3577 | 0.3492 | +0.0085 | [-0.0054, +0.0241] |
| commons-lang | SEMp:t1 | SEM:t1 | 0.5775 | 0.5562 | +0.0213 | [+0.0049, +0.0364] |
| commons-io | SEMp:t1 | SEM:t1 | 0.4511 | 0.4298 | +0.0212 | [+0.0027, +0.0432] |

## Verdict rows

R1 (SEMp:t1 - SEMrand:t1, lo > 0 on >= 5 of 6): lo > 0 on 6 -> yes. R2 (SEMp:t1 - T1s, lo > 0 on >= 3 of 6, hi >= 0 on all): lo > 0 on 3, hi >= 0 on 5 -> no. Guard, SEM:t1 raw: R1 yes (lo > 0 on 6), R2 no (lo > 0 on 2, hi >= 0 on 5).

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| glia | SEMp:t1 | SEMrand:t1 | 0.5556 | 0.0193 | +0.5364 | [+0.5066, +0.5657] |
| shell | SEMp:t1 | SEMrand:t1 | 0.7138 | 0.2991 | +0.4147 | [+0.3638, +0.4643] |
| websocket | SEMp:t1 | SEMrand:t1 | 0.7946 | 0.4568 | +0.3378 | [+0.2530, +0.4132] |
| samples | SEMp:t1 | SEMrand:t1 | 0.3577 | 0.0092 | +0.3485 | [+0.3100, +0.3900] |
| commons-lang | SEMp:t1 | SEMrand:t1 | 0.5775 | 0.0679 | +0.5097 | [+0.4562, +0.5625] |
| commons-io | SEMp:t1 | SEMrand:t1 | 0.4511 | 0.0945 | +0.3565 | [+0.3069, +0.4056] |
| glia | SEMp:t1 | T1s | 0.5556 | 0.3366 | +0.2190 | [+0.1854, +0.2545] |
| shell | SEMp:t1 | T1s | 0.7138 | 0.8025 | -0.0887 | [-0.1290, -0.0475] |
| websocket | SEMp:t1 | T1s | 0.7946 | 0.7389 | +0.0556 | [-0.0254, +0.1376] |
| samples | SEMp:t1 | T1s | 0.3577 | 0.2995 | +0.0581 | [+0.0139, +0.1031] |
| commons-lang | SEMp:t1 | T1s | 0.5775 | 0.5537 | +0.0238 | [-0.0231, +0.0685] |
| commons-io | SEMp:t1 | T1s | 0.4511 | 0.3950 | +0.0561 | [+0.0014, +0.1117] |
| glia | SEM:t1 | SEMrand:t1 | 0.5342 | 0.0193 | +0.5149 | [+0.4864, +0.5434] |
| shell | SEM:t1 | SEMrand:t1 | 0.6939 | 0.2991 | +0.3949 | [+0.3451, +0.4464] |
| websocket | SEM:t1 | SEMrand:t1 | 0.7737 | 0.4568 | +0.3169 | [+0.2369, +0.3910] |
| samples | SEM:t1 | SEMrand:t1 | 0.3492 | 0.0092 | +0.3400 | [+0.3022, +0.3795] |
| commons-lang | SEM:t1 | SEMrand:t1 | 0.5562 | 0.0679 | +0.4884 | [+0.4370, +0.5433] |
| commons-io | SEM:t1 | SEMrand:t1 | 0.4298 | 0.0945 | +0.3353 | [+0.2851, +0.3839] |
| glia | SEM:t1 | T1s | 0.5342 | 0.3366 | +0.1975 | [+0.1632, +0.2339] |
| shell | SEM:t1 | T1s | 0.6939 | 0.8025 | -0.1086 | [-0.1477, -0.0688] |
| websocket | SEM:t1 | T1s | 0.7737 | 0.7389 | +0.0347 | [-0.0490, +0.1157] |
| samples | SEM:t1 | T1s | 0.3492 | 0.2995 | +0.0496 | [+0.0044, +0.0948] |
| commons-lang | SEM:t1 | T1s | 0.5562 | 0.5537 | +0.0025 | [-0.0421, +0.0500] |
| commons-io | SEM:t1 | T1s | 0.4298 | 0.3950 | +0.0348 | [-0.0226, +0.0897] |

## Information rows (never the verdict)

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| glia | FUSE[T1s,SEMp:t1] | T1s | 0.4545 | 0.3366 | +0.1178 | [+0.0971, +0.1428] |
| shell | FUSE[T1s,SEMp:t1] | T1s | 0.8151 | 0.8025 | +0.0126 | [-0.0167, +0.0423] |
| websocket | FUSE[T1s,SEMp:t1] | T1s | 0.8528 | 0.7389 | +0.1138 | [+0.0480, +0.1822] |
| samples | FUSE[T1s,SEMp:t1] | T1s | 0.3876 | 0.2995 | +0.0880 | [+0.0652, +0.1119] |
| commons-lang | FUSE[T1s,SEMp:t1] | T1s | 0.6123 | 0.5537 | +0.0585 | [+0.0337, +0.0842] |
| commons-io | FUSE[T1s,SEMp:t1] | T1s | 0.5034 | 0.3950 | +0.1084 | [+0.0722, +0.1457] |
| glia | SEMp:t1 | ANS:find | 0.5556 | 0.0036 | +0.5521 | [+0.5232, +0.5811] |
| shell | SEMp:t1 | ANS:find | 0.7138 | 0.1315 | +0.5824 | [+0.5368, +0.6266] |
| websocket | SEMp:t1 | ANS:find | 0.7946 | 0.5604 | +0.2342 | [+0.1459, +0.3295] |
| samples | SEMp:t1 | ANS:find | 0.3577 | 0.0176 | +0.3401 | [+0.3008, +0.3824] |
| commons-lang | SEMp:t1 | ANS:find | 0.5775 | 0.0562 | +0.5213 | [+0.4678, +0.5742] |
| commons-io | SEMp:t1 | ANS:find | 0.4511 | 0.0282 | +0.4229 | [+0.3703, +0.4723] |
| glia | SEMp:t1 | LX[IDR,T1s] | 0.5556 | 0.9315 | -0.3759 | [-0.4055, -0.3453] |
| shell | SEMp:t1 | LX[IDR,T1s] | 0.7138 | 0.8025 | -0.0887 | [-0.1290, -0.0475] |
| websocket | SEMp:t1 | LX[IDR,T1s] | 0.7946 | 0.7389 | +0.0556 | [-0.0254, +0.1376] |
| samples | SEMp:t1 | LX[IDR,T1s] | 0.3577 | 0.2995 | +0.0581 | [+0.0139, +0.1031] |
| commons-lang | SEMp:t1 | LX[IDR,T1s] | 0.5775 | 0.5537 | +0.0238 | [-0.0231, +0.0685] |
| commons-io | SEMp:t1 | LX[IDR,T1s] | 0.4511 | 0.3950 | +0.0561 | [+0.0014, +0.1117] |
| glia | SEMp:t1 | SELf2t | 0.5556 | 0.9315 | -0.3758 | [-0.4050, -0.3451] |
| shell | SEMp:t1 | SELf2t | 0.7138 | 0.7998 | -0.0860 | [-0.1285, -0.0437] |
| websocket | SEMp:t1 | SELf2t | 0.7946 | 0.7417 | +0.0529 | [-0.0303, +0.1379] |
| samples | SEMp:t1 | SELf2t | 0.3577 | 0.3083 | +0.0494 | [+0.0026, +0.0976] |
| commons-lang | SEMp:t1 | SELf2t | 0.5775 | 0.5974 | -0.0198 | [-0.0688, +0.0277] |
| commons-io | SEMp:t1 | SELf2t | 0.4511 | 0.4079 | +0.0432 | [-0.0142, +0.1000] |
| glia | SEMp:t1 | ANS:find_files | 0.5556 | 0.9298 | -0.3741 | [-0.4043, -0.3434] |
| shell | SEMp:t1 | ANS:find_files | 0.7138 | 0.7931 | -0.0793 | [-0.1204, -0.0365] |
| websocket | SEMp:t1 | ANS:find_files | 0.7946 | 0.7221 | +0.0724 | [-0.0097, +0.1532] |
| samples | SEMp:t1 | ANS:find_files | 0.3577 | 0.2973 | +0.0603 | [+0.0156, +0.1052] |
| commons-lang | SEMp:t1 | ANS:find_files | 0.5775 | 0.5147 | +0.0629 | [+0.0151, +0.1100] |
| commons-io | SEMp:t1 | ANS:find_files | 0.4511 | 0.3685 | +0.0825 | [+0.0294, +0.1355] |
| glia | SEMp:t1 | ANS:find_files@sel | 0.5556 | 0.9263 | -0.3707 | [-0.4007, -0.3383] |
| shell | SEMp:t1 | ANS:find_files@sel | 0.7138 | 0.7970 | -0.0832 | [-0.1252, -0.0394] |
| websocket | SEMp:t1 | ANS:find_files@sel | 0.7946 | 0.7294 | +0.0651 | [-0.0201, +0.1506] |
| samples | SEMp:t1 | ANS:find_files@sel | 0.3577 | 0.3074 | +0.0503 | [+0.0039, +0.0992] |
| commons-lang | SEMp:t1 | ANS:find_files@sel | 0.5775 | 0.5595 | +0.0180 | [-0.0338, +0.0719] |
| commons-io | SEMp:t1 | ANS:find_files@sel | 0.4511 | 0.4104 | +0.0406 | [-0.0174, +0.0979] |

## Every other form (information)

| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |
|---|---|---|---:|---:|---:|---|
| glia | SEM:t1 | SEMrand:t1 | 0.5342 | 0.0193 | +0.5149 | [+0.4864, +0.5434] |
| shell | SEM:t1 | SEMrand:t1 | 0.6939 | 0.2991 | +0.3949 | [+0.3451, +0.4464] |
| websocket | SEM:t1 | SEMrand:t1 | 0.7737 | 0.4568 | +0.3169 | [+0.2369, +0.3910] |
| samples | SEM:t1 | SEMrand:t1 | 0.3492 | 0.0092 | +0.3400 | [+0.3022, +0.3795] |
| commons-lang | SEM:t1 | SEMrand:t1 | 0.5562 | 0.0679 | +0.4884 | [+0.4370, +0.5433] |
| commons-io | SEM:t1 | SEMrand:t1 | 0.4298 | 0.0945 | +0.3353 | [+0.2851, +0.3839] |
| glia | SEM:t1 | T1s | 0.5342 | 0.3366 | +0.1975 | [+0.1632, +0.2339] |
| shell | SEM:t1 | T1s | 0.6939 | 0.8025 | -0.1086 | [-0.1477, -0.0688] |
| websocket | SEM:t1 | T1s | 0.7737 | 0.7389 | +0.0347 | [-0.0490, +0.1157] |
| samples | SEM:t1 | T1s | 0.3492 | 0.2995 | +0.0496 | [+0.0044, +0.0948] |
| commons-lang | SEM:t1 | T1s | 0.5562 | 0.5537 | +0.0025 | [-0.0421, +0.0500] |
| commons-io | SEM:t1 | T1s | 0.4298 | 0.3950 | +0.0348 | [-0.0226, +0.0897] |
| glia | FUSE[T1s,SEM:t1] | T1s | 0.4529 | 0.3366 | +0.1162 | [+0.0959, +0.1414] |
| shell | FUSE[T1s,SEM:t1] | T1s | 0.8101 | 0.8025 | +0.0075 | [-0.0217, +0.0360] |
| websocket | FUSE[T1s,SEM:t1] | T1s | 0.8576 | 0.7389 | +0.1186 | [+0.0537, +0.1870] |
| samples | FUSE[T1s,SEM:t1] | T1s | 0.3873 | 0.2995 | +0.0878 | [+0.0636, +0.1120] |
| commons-lang | FUSE[T1s,SEM:t1] | T1s | 0.6131 | 0.5537 | +0.0594 | [+0.0354, +0.0832] |
| commons-io | FUSE[T1s,SEM:t1] | T1s | 0.4967 | 0.3950 | +0.1017 | [+0.0653, +0.1395] |
| glia | SEM:t2 | SEMrand:t1 | 0.5840 | 0.0193 | +0.5647 | [+0.5330, +0.5972] |
| shell | SEM:t2 | SEMrand:t1 | 0.7307 | 0.2991 | +0.4316 | [+0.3764, +0.4826] |
| websocket | SEM:t2 | SEMrand:t1 | 0.7576 | 0.4568 | +0.3009 | [+0.2187, +0.3827] |
| samples | SEM:t2 | SEMrand:t1 | 0.3491 | 0.0092 | +0.3399 | [+0.3050, +0.3763] |
| commons-lang | SEM:t2 | SEMrand:t1 | 0.5733 | 0.0679 | +0.5054 | [+0.4507, +0.5600] |
| commons-io | SEM:t2 | SEMrand:t1 | 0.4432 | 0.0945 | +0.3487 | [+0.2991, +0.3988] |
| glia | SEM:t2 | T1s | 0.5840 | 0.3366 | +0.2474 | [+0.2104, +0.2877] |
| shell | SEM:t2 | T1s | 0.7307 | 0.8025 | -0.0718 | [-0.1171, -0.0319] |
| websocket | SEM:t2 | T1s | 0.7576 | 0.7389 | +0.0187 | [-0.0705, +0.1078] |
| samples | SEM:t2 | T1s | 0.3491 | 0.2995 | +0.0496 | [+0.0059, +0.0921] |
| commons-lang | SEM:t2 | T1s | 0.5733 | 0.5537 | +0.0196 | [-0.0274, +0.0646] |
| commons-io | SEM:t2 | T1s | 0.4432 | 0.3950 | +0.0482 | [-0.0128, +0.1036] |
| glia | FUSE[T1s,SEM:t2] | T1s | 0.4695 | 0.3366 | +0.1328 | [+0.1103, +0.1575] |
| shell | FUSE[T1s,SEM:t2] | T1s | 0.8384 | 0.8025 | +0.0359 | [+0.0058, +0.0643] |
| websocket | FUSE[T1s,SEM:t2] | T1s | 0.8403 | 0.7389 | +0.1013 | [+0.0264, +0.1719] |
| samples | FUSE[T1s,SEM:t2] | T1s | 0.3780 | 0.2995 | +0.0785 | [+0.0573, +0.1005] |
| commons-lang | FUSE[T1s,SEM:t2] | T1s | 0.6161 | 0.5537 | +0.0623 | [+0.0371, +0.0883] |
| commons-io | FUSE[T1s,SEM:t2] | T1s | 0.5021 | 0.3950 | +0.1071 | [+0.0698, +0.1424] |
| glia | SEMp:t2 | SEMrand:t1 | 0.5897 | 0.0193 | +0.5705 | [+0.5382, +0.6031] |
| shell | SEMp:t2 | SEMrand:t1 | 0.7394 | 0.2991 | +0.4403 | [+0.3904, +0.4908] |
| websocket | SEMp:t2 | SEMrand:t1 | 0.7520 | 0.4568 | +0.2952 | [+0.2072, +0.3789] |
| samples | SEMp:t2 | SEMrand:t1 | 0.3814 | 0.0092 | +0.3723 | [+0.3322, +0.4093] |
| commons-lang | SEMp:t2 | SEMrand:t1 | 0.5819 | 0.0679 | +0.5140 | [+0.4586, +0.5680] |
| commons-io | SEMp:t2 | SEMrand:t1 | 0.4649 | 0.0945 | +0.3704 | [+0.3205, +0.4209] |
| glia | SEMp:t2 | T1s | 0.5897 | 0.3366 | +0.2531 | [+0.2178, +0.2919] |
| shell | SEMp:t2 | T1s | 0.7394 | 0.8025 | -0.0631 | [-0.1065, -0.0204] |
| websocket | SEMp:t2 | T1s | 0.7520 | 0.7389 | +0.0130 | [-0.0746, +0.1027] |
| samples | SEMp:t2 | T1s | 0.3814 | 0.2995 | +0.0819 | [+0.0387, +0.1240] |
| commons-lang | SEMp:t2 | T1s | 0.5819 | 0.5537 | +0.0282 | [-0.0213, +0.0716] |
| commons-io | SEMp:t2 | T1s | 0.4649 | 0.3950 | +0.0699 | [+0.0132, +0.1237] |
| glia | FUSE[T1s,SEMp:t2] | T1s | 0.4821 | 0.3366 | +0.1454 | [+0.1242, +0.1704] |
| shell | FUSE[T1s,SEMp:t2] | T1s | 0.8447 | 0.8025 | +0.0422 | [+0.0122, +0.0718] |
| websocket | FUSE[T1s,SEMp:t2] | T1s | 0.8213 | 0.7389 | +0.0823 | [+0.0153, +0.1531] |
| samples | FUSE[T1s,SEMp:t2] | T1s | 0.3874 | 0.2995 | +0.0879 | [+0.0659, +0.1103] |
| commons-lang | FUSE[T1s,SEMp:t2] | T1s | 0.6194 | 0.5537 | +0.0657 | [+0.0415, +0.0905] |
| commons-io | FUSE[T1s,SEMp:t2] | T1s | 0.5121 | 0.3950 | +0.1171 | [+0.0788, +0.1533] |

## Verdict

recipe r* = t1; query form = bge; S* = SEMp:t1; R1 = yes; R2 = no.

glia embed ships as a sidecar no answer reads (item 13): the integrator drops FB.7a / FB.7b /
FB.7c / FB.8 / FB.9 from the schedule.

VERDICT: FAIL
