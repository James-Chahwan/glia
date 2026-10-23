# Ranking eval, second run: vertical (upper-layer) links (2026-10-18)

James, 2026-10-18: *"changes do have direct edges in my head ... imagine graphs stacked upon themselves and vertically ...
information about the reason for the change is not in the code domain"*. The hypothesis: files that change together are
siblings under the same requirement / spec / law (a vertical edge through an upper layer), not neighbours in the code graph.

Same harness, dataset, queries, metrics and paired commit-cluster bootstrap as the first run (`engine/examples/rank_eval.rs`,
`--vertical kina|glia|quokka`). Every first-run ranker reproduces: 68 / 69 / 68 table rows identical on Kina / quokka-stack / glia.

## Bottom line

1. **Requirement ids are real vertical edges, but weak ones.** A shared id cited in code (V1) beats its random-id control by
   +0.064 recall@10 on Kina and +0.166 on glia, so the gain is shared requirements, not "files that cite anything".
   - Beyond structure the gain is small. On Kina, V1 blended into PPR (HP25[V1]) adds +0.014 [+0.002, +0.026] over P1;
     adding the files that docs name (V1m) gives +0.038 [+0.023, +0.053]. On glia it adds +0.034 [+0.022, +0.048].
   - Co-change still does as well. On Kina, H10 (co-change, then P1) beats HP25[V1m] by 0.015 [-0.006, +0.038], not
     significant. On glia, HP50[V1] edges H10 by +0.011 [+0.000, +0.024].
   - Beyond intent text there is nothing on Kina: HR25[V1] is -0.010 [-0.019, -0.002] against H9a50[P1], HR25[V1m] is
     -0.003. On glia it is small: +0.018 [+0.009, +0.027].
2. **Every hop up the stack loses signal.**
   - Kina: spec (V2) is below requirement (V1) by 0.049 [0.025, 0.075]. Law (V3) is below spec by a further 0.031, and
     sits exactly on its own random control (+0.000 [-0.003, +0.003]).
   - glia: packet family < packet, and wave / batch < family. James's rulings (V4) are no better than the same map with the
     packets permuted (+0.004 [-0.013, +0.019]).
   - Adding the rulings to the packets (V14) lowers recall@10 by 0.013 [0.006, 0.019].
3. **The one strong vertical edge is a plan that names files.**
   - On glia, a pending packet's `files_touched` at the parent revision names both the seed and the co-changed file in
     94% of co-change pairs, against 0.07% of other pairs.
   - Alone it reaches recall@10 0.890 (P1 0.294, H9a50[P1] 0.404); HRlex[V2mP] reaches 0.910.
   - This measures glia's agent-wave process (a spec run writes each packet's file list, then a wave edits exactly those
     files). Kina's open tasks name the touched files for 10.5% of seeds; quokka's feature docs add +0.061 [+0.004, +0.120].
4. **Intent text routed through the layer beats intent text alone.**
   - When the commit message names an id, sending that id down to the files that cite it or that its doc rows name (VM)
     and blending with H9 gives HM50.
   - Kina: +0.039 [+0.019, +0.060] over H9a50[P1] on all queries, +0.082 [+0.045, +0.123] on the 47% whose message cites an
     id. This is the largest honest gain in Kina.
   - glia: VM alone reaches 0.831, because the packet named in the message carries its own file list (point 3).

So the answer to "do changes have direct edges through an upper layer": yes, measurably, at the most specific node (the
requirement / task / packet id) and above all where that node names its files. The "why" layers (spec, law, ruling) carry
no measurable predictive signal for which files change together.

## Method (additions only)

**Leak-free layers.** For each test commit, the upper layer is read at its parent revision:
- code with one `git grep <parent>` per commit;
- Kina docs: Kina's tracked `*.md` at the parent (minus `.specify/`, `.claude/`, `node_modules/`), plus, once c643306
  (2026-09-24) moved the specs into the wiki, the wiki commit current at the parent's committer time (the wiki is its own
  git repo; 36 of 199 test commits are after the move);
- glia: the packet JSONs, schedules (LANDED waves), plan docs and corrections at the parent (`git ls-tree` + blob cache);
- quokka-stack: `features/`, `docs/`, `.ai/` and `*.md` at the parent.

**Kina vocabulary** (derived from definition lines: the first token of a heading, a table row's first cell, or a list /
checkbox item, at each parent; listed here at the newest test commit's parent):
- 20 hyphenated prefixes: spec-kit's FR and SC and 18 project prefixes (names kept private);
- spec-kit's hyphen-less T### (tasks), R# (research), C# (analysis), D# (decisions);
- excluded look-alikes: SHA, AES, ERC, UTF, ISO, RFC, EIP, BIP, CVE, MIT, BSD, GPL, LGPL, AGPL, MPL, CC0.

**Spec scoping (Kina).** Spec-kit numbers FR-002, T014, etc. per spec, so an id defined in a spec is qualified as
`specs/<dir>|<id>`:
- by a spec number on the same line (`spec NNN`, `NNN T014`);
- else, if no global doc also defines it, by its only defining spec or the one spec the file names;
- an id a global doc defines (a plan doc's D6, a project-prefix id such as ABC-01) keeps its bare form unless its own line
  qualifies it.

Citations summed over commits: 78,538 global, 3,503 line-qualified, 1,991 global over spec, 1,127 by the file's spec,
463 by the unique spec, 286 ambiguous (kept as `*|id`), 392 bare and ambiguous (dropped).

**glia vocabulary.** Packet-id prefixes of the packets defined at the parent (A1-A16, C0, CA-CL, CZ, D0, DA-DH, DT, DZ,
L0, LA-LG), from the four `*packets.json` files.

**Rankers.** Each is score(f) = sum over upper-layer nodes shared by the seed and f of idf(node), idf = ln(1 + files /
files linking the node).

| id | upper-layer node a file links to |
|---|---|
| V1 | requirement / packet id the file cites |
| V2 (Kina) | spec unit (`specs/<dir>`, else the defining doc) of its ids, plus direct `spec NNN` citations |
| V2fam / V2bat / V2wav (glia) | packet family (DT.1h -> DT.1) / batch / LANDED wave at the parent |
| V3 (Kina) | laws its spec units name; V3d adds laws the code names directly |
| V2m | doc unit (or the id a doc line defines) whose text names the file's path; glia: the packets whose `files_touched` list it |
| V2mP / V2mD | V2m split: glia, packets with no LANDED wave yet vs landed; Kina, open `- [ ]` task lines vs the rest |
| V1m | V1 and V2m together (one token space: requirement nodes reached from either direction) |
| V4 / V4p (glia) | rulings that trace to its cited packets: all sources / plan docs only (no corrections) |
| V14 (glia) | its packets and their rulings |
| V2w / V3w / V2mw (Kina) | V2 / V3 / V2m with the wiki at HEAD (2026-10-18) as the doc layer: a snapshot, LEAKY, flagged |
| VM | the test commit's message ids, routed through V1m's nodes (seed-independent) |

Law keys are capitalised runs ending in Act / Rules / Regulations / Directive(s). They are normalised: jurisdiction words
and the year are dropped (so two jurisdictions' same-named Acts of different years are one key), and the spellings of the
regime's main instrument are unified. A key
is kept only when two doc units name it; 45 survive.

Rulings, at the newest parent, number 89 (median 3 packets, max 133):
- 25 from next-leap-0.5.2.md: 18 Q-rulings, linked through the packets that cite Q<n>, and 7 §4.2 decisions;
- 19 from tweaks-0.5.2.md: the T1-T13 rows (T<n> -> the DT.<n> family) and the numbered rulings;
- 13 from next-leap-0.5.0.md and 11 from next-leap-0.5.1.md (the decision sections);
- 21 corrections entries that quote James.

**Hybrids.**
- Impact: HP25 / HP50 = a mm(V) + (1-a) mm(P1), and HPlex = (V, P1).
- Review: HR25 / HR50 = b mm(V) + (1-b) mm(H9a50[P1]), and HRlex = (V, H9).
- Triple a T1 + b V + c P1 on the grid HTa (.5,.25,.25), HTb (.25,.5,.25), HTc (.25,.25,.5), HTd (1/3 each).
- Message route: HM25 / HM50 = b mm(VM) + (1-b) mm(H9), and HMlex = (VM, H9).

**Controls.**
- V0: each file's ids are replaced by the same number of ids drawn uniformly from the ids cited at that revision
  (deterministic).
- V0df[X]: the same for every signal X, but drawn in proportion to how many files link each node (keeps popularity).
- V4perm / V4pperm / V14perm: the ruling -> packet map under a random permutation of the defined packets.

**Coverage.** A query is covered when its seed has at least one node of the signal at the parent. Every metric is
reported on ALL and COVERED queries.

## Results

### Kina (764 queries, 199 test commits)

58% of (seed, co-changed file) pairs share no direct code edge (any category).

| signal | covered queries | P(link given co-change) | P(link given co-change, no code edge) | P(link given other pair) | lift |
|---|---|---|---|---|---|
| V1 (seed cites an id) | 369 (48.3%) | 0.161 | 0.073 | 0.0137 | 11.8 |
| V2 | 371 (48.6%) | 0.255 | 0.155 | 0.0556 | 4.6 |
| V3 | 269 (35.2%) | 0.131 | 0.087 | 0.0226 | 5.8 |
| V2m | 514 (67.3%) | 0.332 | 0.237 | 0.0701 | 4.7 |
| V1m | 569 (74.5%) | 0.400 | 0.272 | 0.0780 | 5.1 |
| V2mP (open tasks) | 80 (10.5%) | 0.020 | 0.015 | 0.0004 | 46.7 |
| V2mw (leaky snapshot) | 645 (84.4%) | 0.571 | 0.429 | 0.2916 | 2.0 |
| VM (message cites an id) | 362 (47.4%), 89 of 199 commits | - | - | - | - |

ALL queries:

| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |
|---|---|---|---|---|---|
| B0 | 0.324 | 0.442 | 0.551 | 0.485 | 0.409 |
| P1 | 0.303 | 0.426 | 0.539 | 0.476 | 0.399 |
| C0 | 0.252 | 0.321 | 0.377 | 0.392 | 0.294 |
| H10 | 0.364 | 0.479 | 0.594 | 0.580 | 0.470 |
| T1 | 0.343 | 0.477 | 0.573 | 0.510 | 0.428 |
| H9a50[P1] | 0.385 | 0.531 | 0.646 | 0.567 | 0.487 |
| V0 | 0.112 | 0.155 | 0.203 | 0.190 | 0.138 |
| V1 | 0.161 | 0.219 | 0.271 | 0.253 | 0.194 |
| V2 | 0.113 | 0.170 | 0.239 | 0.238 | 0.165 |
| V3 | 0.097 | 0.139 | 0.215 | 0.183 | 0.136 |
| V3d | 0.104 | 0.142 | 0.238 | 0.202 | 0.150 |
| V2m | 0.174 | 0.229 | 0.292 | 0.303 | 0.221 |
| V1m | 0.227 | 0.301 | 0.381 | 0.364 | 0.284 |
| V2w / V3w (leaky) | 0.122 / 0.093 | 0.175 / 0.146 | 0.243 / 0.222 | 0.218 / 0.179 | 0.161 / 0.137 |
| HP25[V1] | 0.321 | 0.439 | 0.560 | 0.493 | 0.417 |
| HP25[V1m] | 0.342 | 0.463 | 0.564 | 0.508 | 0.429 |
| HP50[V1m] | 0.342 | 0.458 | 0.573 | 0.557 | 0.447 |
| HR25[V1] | 0.391 | 0.521 | 0.645 | 0.569 | 0.487 |
| HR25[V1m] | 0.398 | 0.528 | 0.650 | 0.580 | 0.494 |
| HTa[V1m] | 0.378 | 0.531 | 0.643 | 0.553 | 0.479 |
| VM | 0.197 | 0.236 | 0.267 | 0.302 | 0.221 |
| HM50 | **0.417** | **0.571** | **0.675** | **0.591** | **0.514** |

COVERED by V1 (369 queries):

| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |
|---|---|---|---|---|---|
| P1 | 0.279 | 0.432 | 0.561 | 0.434 | 0.378 |
| H10 | 0.375 | 0.513 | 0.644 | 0.572 | 0.478 |
| T1 | 0.467 | 0.639 | 0.755 | 0.595 | 0.551 |
| H9a50[P1] | 0.439 | 0.630 | 0.759 | 0.592 | 0.546 |
| V0 / V0df[V1] | 0.161 / 0.171 | 0.238 / 0.258 | 0.313 / 0.353 | 0.284 / 0.278 | 0.212 / 0.223 |
| V1 | 0.262 | 0.371 | 0.453 | 0.414 | 0.330 |
| HP25[V1] / HP50[V1] | 0.317 / 0.313 | 0.460 / 0.463 | 0.604 / 0.626 | 0.468 / 0.509 | 0.415 / 0.434 |
| HR25[V1] / HTa[V1] | 0.452 / 0.466 | 0.609 / 0.621 | 0.756 / 0.769 | 0.596 / 0.611 | 0.546 / 0.559 |

COVERED by VM (362 queries): H9a50[P1] 0.372 / 0.545 / 0.677 / 0.577 / 0.507; HM50 0.441 / 0.628 / 0.739 / 0.627 / 0.563.

Paired recall@10 (95% commit-cluster bootstrap):

| comparison | ALL | COVERED |
|---|---|---|
| V1 - V0 | +0.064 [+0.043, +0.089] | +0.133 [+0.088, +0.179] |
| V1 - V0df[V1] | +0.055 [+0.031, +0.081] | +0.113 [+0.069, +0.158] |
| HP25[V1] - P1 | +0.014 [+0.002, +0.026] | +0.028 [+0.004, +0.052] |
| HP25[V1m] - P1 | +0.038 [+0.023, +0.053] | +0.050 [+0.030, +0.071] (V1m-covered, 569) |
| HP25[V1m] - H10 | -0.015 [-0.038, +0.006] | -0.011 [-0.039, +0.015] |
| HR25[V1] - H9a50[P1] | -0.010 [-0.019, -0.002] | -0.021 [-0.039, -0.004] |
| HR25[V1m] - H9a50[P1] | -0.003 [-0.015, +0.008] | -0.005 [-0.020, +0.012] |
| V2 - V1 | -0.049 [-0.075, -0.025] | -0.103 [-0.155, -0.050] |
| V3 - V2 | -0.031 [-0.043, -0.018] | -0.047 [-0.082, -0.015] |
| V3 - V0df[V3] | +0.000 [-0.003, +0.003] | +0.000 [-0.007, +0.009] |
| V3d - V0df[V3d] | -0.004 [-0.008, -0.000] | -0.010 [-0.021, -0.001] |
| HP25[V2mw] - P1 (leaky) | +0.027 [+0.011, +0.043] | +0.032 [+0.014, +0.051] |
| HR25[V2mw] - H9 (leaky) | +0.003 [-0.009, +0.015] | +0.003 [-0.012, +0.019] |
| HM50 - H9a50[P1] | +0.039 [+0.019, +0.060] | +0.082 [+0.045, +0.123] |

MRR: HP50[V1m] - P1 +0.080 [+0.053, +0.110]; HM50 - H9 +0.024 [-0.015, +0.067] (all) and +0.050 [-0.038, +0.134]
(covered), both not significant.

### glia (1253 queries, 300 test commits)

80% of co-change pairs share no direct code edge.

| signal | covered queries | P(link given co-change) | P(link given co-change, no code edge) | P(link given other pair) | lift |
|---|---|---|---|---|---|
| V1 (seed cites a packet) | 654 (52.2%) | 0.189 | 0.127 | 0.0092 | 20.6 |
| V2fam | 654 (52.2%) | 0.208 | 0.144 | 0.0136 | 15.3 |
| V2wav | 616 (49.2%) | 0.213 | 0.158 | 0.0464 | 4.6 |
| V2m (files_touched) | 1249 (99.7%) | 0.953 | 0.945 | 0.0207 | 46 |
| V2mP (pending packets) | 1211 (96.6%) | 0.942 | 0.936 | 0.0007 | 1269 |
| V2mD (landed packets) | 651 (52.0%) | 0.145 | 0.091 | 0.0201 | 7.2 |
| V4 (rulings) | 599 (47.8%) | 0.227 | 0.175 | 0.0716 | 3.2 |
| V4p (plan-doc rulings) | 399 (31.8%) | 0.073 | 0.053 | 0.0048 | 15.3 |
| VM (message cites a packet) | 1248 (99.6%), 298 of 300 commits | - | - | - | - |

ALL queries:

| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |
|---|---|---|---|---|---|
| B0 | 0.186 | 0.255 | 0.321 | 0.356 | 0.245 |
| P1 | 0.223 | 0.294 | 0.364 | 0.416 | 0.295 |
| C0 | 0.134 | 0.163 | 0.185 | 0.238 | 0.163 |
| H10 | 0.261 | 0.321 | 0.381 | 0.511 | 0.343 |
| T1 | 0.265 | 0.324 | 0.392 | 0.532 | 0.351 |
| H9a50[P1] | 0.307 | 0.404 | 0.486 | 0.570 | 0.413 |
| V0 | 0.006 | 0.014 | 0.024 | 0.024 | 0.014 |
| V1 | 0.126 | 0.180 | 0.218 | 0.192 | 0.161 |
| V2fam / V2bat / V2wav | 0.111 / 0.065 / 0.061 | 0.164 / 0.098 / 0.093 | 0.210 / 0.141 / 0.131 | 0.174 / 0.125 / 0.127 | 0.148 / 0.094 / 0.091 |
| V4 / V4perm | 0.034 / 0.030 | 0.057 / 0.053 | 0.088 / 0.072 | 0.079 / 0.060 | 0.058 / 0.046 |
| V4p / V4pperm | 0.028 / 0.025 | 0.047 / 0.039 | 0.068 / 0.053 | 0.059 / 0.046 | 0.046 / 0.035 |
| V14 / V14perm | 0.119 / 0.119 | 0.167 / 0.168 | 0.203 / 0.200 | 0.189 / 0.183 | 0.152 / 0.150 |
| HP25[V1] / HP50[V1] | 0.244 / 0.246 | 0.328 / 0.332 | 0.402 / 0.397 | 0.442 / 0.480 | 0.324 / 0.338 |
| HR25[V1] / HTc[V1] | 0.321 / 0.323 | 0.422 / 0.425 | 0.501 / 0.500 | 0.581 / 0.575 | 0.428 / 0.428 |
| HP25[V4] / HR25[V4] | 0.222 / 0.308 | 0.292 / 0.395 | 0.350 / 0.481 | 0.417 / 0.570 | 0.291 / 0.412 |
| V2m | 0.617 | 0.768 | 0.846 | 0.825 | 0.789 |
| V2mP / V2mD | 0.749 / 0.118 | 0.890 / 0.152 | 0.936 / 0.177 | 0.920 / 0.212 | 0.915 / 0.149 |
| HRlex[V2mP] | **0.763** | **0.910** | **0.961** | **0.942** | **0.935** |
| VM / HMlex | 0.595 / 0.598 | 0.831 / 0.833 | 0.950 / 0.955 | 0.793 / 0.807 | 0.811 / 0.818 |

COVERED by V1 (654 queries): P1 0.216 / 0.319 / 0.423 / 0.337 / 0.286; H9a50[P1] 0.304 / 0.426 / 0.527 / 0.556 / 0.419;
V1 0.241 / 0.341 / 0.413 / 0.365 / 0.306; HP50[V1] 0.260 / 0.393 / 0.486 / 0.460 / 0.369; HR25[V1] 0.330 / 0.460 /
0.555 / 0.576 / 0.446.

Paired recall@10:

| comparison | ALL | COVERED |
|---|---|---|
| V1 - V0 | +0.166 [+0.140, +0.194] | +0.317 [+0.280, +0.360] |
| HP25[V1] - P1 | +0.034 [+0.022, +0.048] | +0.065 [+0.042, +0.090] |
| HP50[V1] - H10 | +0.011 [+0.000, +0.024] | +0.023 [+0.002, +0.044] |
| HR25[V1] - H9a50[P1] | +0.018 [+0.009, +0.027] | +0.034 [+0.017, +0.050] |
| V2fam - V1 | -0.016 [-0.026, -0.006] | -0.030 [-0.051, -0.012] |
| V2wav - V1 | -0.087 [-0.106, -0.069] | -0.161 [-0.193, -0.130] |
| V4 - V4perm (ruling map shuffled) | +0.004 [-0.013, +0.019] | (biased: the permutation loses coverage) |
| V4p - V4pperm | +0.008 [-0.008, +0.023] | (biased, as above) |
| V4 - V2fam | -0.107 [-0.129, -0.087] | -0.192 [-0.225, -0.156] |
| V14 - V1 (rulings added to packets) | -0.013 [-0.019, -0.006] | -0.024 [-0.037, -0.013] |
| HP25[V4] - P1 | -0.002 [-0.011, +0.007] | -0.004 [-0.022, +0.016] |
| HR25[V4] - H9a50[P1] | -0.009 [-0.015, -0.003] | -0.019 [-0.033, -0.007] |
| V2m - H9a50[P1] | +0.363 [+0.329, +0.400] | +0.363 (99.7% covered) |
| V2mP - V2mD | +0.738 [+0.705, +0.770] | +0.741 [+0.706, +0.773] |
| HRlex[V2mP] - H9a50[P1] | +0.506 [+0.476, +0.536] | +0.523 [+0.495, +0.552] |
| VM - H9a50[P1] | +0.427 [+0.391, +0.465] | +0.432 [+0.397, +0.470] |

MRR: V4 - V4perm +0.019 [+0.003, +0.036], a small positive; the recall@10 difference is not significant.

### quokka-stack (110 queries, 40 test commits; small)

No requirement ids in code, so V0 / V1 / V2 / V3 do not apply. The third pattern is doc -> code: the `features/<f>/`
docs (and `docs/`, `.ai/`) name file paths.
- V2m covers 80.9% of queries and links 60.9% of co-change pairs vs 24.3% of other pairs (lift 2.5).
- V2m minus its control: +0.327 [+0.202, +0.475].
- HP25[V2m] minus P1: +0.061 [+0.004, +0.120].
- HR25[V2m] minus H9a50[P1]: +0.035 [+0.000, +0.070].
- Best blend (selected on test): HR50[V2m] 0.710 on the covered 89 queries, +0.069 [-0.002, +0.132] over H9.

## Verdict

- **Beyond structure:** yes, a little.
  - Kina: requirement ids add +0.014 recall@10 over PPR (+0.028 where the seed cites one), and +0.038 with the files the
    docs name.
  - glia: cited packets add +0.034 (+0.065 covered).
  - That is the same size as, or smaller than, co-change as a second key (Kina's H10 is ahead, glia's is level).
- **Beyond intent text:** no on Kina (-0.010 to -0.001); +0.018 on glia.
- **Where it fails:**
  - Coverage: half the seeds cite no id at the parent revision.
  - Coarse upper nodes: one spec and two plan docs each reach about 90 of the roughly 160 citing files, and the main
    law key reaches 113. A shared spec or law therefore says only that both files sit in the same broad area.
  - Spec-kit's per-spec numbering: FR-002 exists in every spec, so ids need scoping.
- **Rulings and law (the "why"):** no predictive signal.
  - V4 equals its permutation control, and adding rulings to packets lowers recall.
  - V3 equals its random control.
  - They explain a change; they do not predict its files.
- **The exception that matters:** a plan that names files.
  - glia's pending packets predict 89% of co-changed files at rank 10.
  - In review, routing the message's own ids through the layer adds +0.04 (Kina, all) to +0.08 (Kina, covered) over the
    BM25 intent blend.
- **The 84-93% claim:**
  - Here 58% (Kina) and 80% (glia) of co-change pairs have no direct code edge.
  - Requirement ids join 7% (Kina) and 13% (glia) of those no-edge pairs.
  - Doc-named files bring Kina to 27%, and glia's packet file lists to 95%.

## What it implies for glia

1. **A requirement / task layer earns its place as first-class nodes with vertical edges.** Keep the edges at the most
   specific node (the requirement / task / packet id), not at spec, law or ruling granularity. Two edge kinds carry the
   signal:
   - code cites an id (comment ids, scoped per spec);
   - a doc row or task names a file (`files_touched`, task lines, feature docs).
2. **Ranking.**
   - impact: P1 + 0.25 of the vertical score (V1m-style). Expect +0.01 to +0.04 recall@10 and up to +0.08 MRR. It is not
     a replacement for co-change as the second key.
   - review: when the PR / commit text names an id, route that id through the layer (VM / HM50). That is the measured win
     (+0.04 to +0.08 on Kina). A seed-based vertical term on top of H9 adds nothing.
3. **Planned work names its files.** A pending plan item (packet `files_touched`, an open task with paths) is a
   near-oracle for the files a change will touch, wherever the process writes it down. Ingesting plan items as nodes with
   NAMES_FILE edges, and marking them pending or done, is the highest-value piece. Its strength is process-dependent
   (glia's agent waves), so measure it per repo.
4. **The "why" layers serve explanation, not ranking.** Laws, rulings and spec-level grouping should feed `why`-style
   answers and provenance (which ruling or law a change serves), not the impact or review ranking.
5. **Ingestion needs scoping rules.** Spec-kit ids are numbered per spec. A global id that shares its name with a spec id
   (D6) must stay global unless its line names the spec. Law names need normalisation, and a key must be named by at least
   two doc units, or sentence fragments become laws.

## Methodology caveats

1. **glia's file-list results measure its own process.** A spec run writes each packet's `files_touched` before an agent
   wave edits exactly those files. V2m / V2mP / VM on glia are near-oracles for that reason, not evidence that requirements
   predict files in general.
2. **Kina's post-move doc layer is paired by timestamp.** Kina and its wiki are paired by the parent's committer time
   across two repositories (36 test commits).
3. **The `w` variants leak.** They read the wiki at 2026-10-18, after every test commit, so they are upper bounds.
4. **Heuristic extraction.**
   - Definitions come from line shape: the first token of a heading, table cell or list item.
   - Law keys come from capitalised runs. No section-level law keys were built.
   - Two jurisdictions' same-named Acts share one key.
5. **The message is the intent.** VM, like T1, uses the commit message, which is written after the change.
6. **Multiple comparisons.** About 170 vertical rankers per repo. HP25 / HR25 are the fixed rows; the "best blend" rows
   are selected on the test set.
7. **Controls.**
   - V0 draws from the ids in use at the revision, not from every defined id.
   - The permutation controls lose coverage on the real signal's covered subset, so only their ALL rows are fair.
8. **First-run caveats still apply:** the snapshot graph serves the structural rankers, whole-file seeds, and quokka's
   small sample.

## Reproduce

```sh
cargo build --release -p glia-engine --example rank_eval
GLIA_NO_PERSIST=1 target/release/examples/rank_eval --repo <git-archive copy> --git <live repo> --rev <sha> --label <name> \
    --out <name>.json --md <name>.md --vertical kina|glia|quokka [--wiki <Kina/.wiki>] --vmd <name>_vertical.md
# GLIA_RANK_VDUMP=<file> also writes the newest test commit's layer (file -> nodes per signal) for inspection.
```

Run times: Kina 15 s, quokka-stack 1 s, glia 68 s (13.7 s of it reading the layers), 16 threads.

## Headline tables

Generated by the run; the full per-repo tables are in `ranking-eval-vertical-2026-10-18-tables.md`.

### Kina

| signal | covered queries | P(link given co-change) | P(link given co-change, no code edge) | P(link given other pair) | lift |
|---|---|---|---|---|---|
| V1 | 369 (48.3%) | 0.161 | 0.073 | 0.0137 | 11.8 |
| V1m | 569 (74.5%) | 0.400 | 0.272 | 0.0780 | 5.1 |
| V2 | 371 (48.6%) | 0.255 | 0.155 | 0.0556 | 4.6 |
| V2m | 514 (67.3%) | 0.332 | 0.237 | 0.0701 | 4.7 |
| V2mD | 495 (64.8%) | 0.326 | 0.232 | 0.0701 | 4.7 |
| V2mP | 80 (10.5%) | 0.020 | 0.015 | 0.0004 | 46.7 |
| V2mw | 645 (84.4%) | 0.571 | 0.429 | 0.2916 | 2.0 |
| V2w | 371 (48.6%) | 0.254 | 0.154 | 0.0557 | 4.6 |
| V3 | 269 (35.2%) | 0.131 | 0.087 | 0.0226 | 5.8 |
| V3d | 291 (38.1%) | 0.157 | 0.097 | 0.0254 | 6.2 |
| V3w | 316 (41.4%) | 0.184 | 0.113 | 0.0331 | 5.6 |
| VM (the commit message's ids) | 362 (47.4%) | - | - | - | - |

ALL queries (764):

| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |
|---|---|---|---|---|---|
| B0 | 0.324 | 0.442 | 0.551 | 0.485 | 0.409 |
| P1 | 0.303 | 0.426 | 0.539 | 0.476 | 0.399 |
| C0 | 0.252 | 0.321 | 0.377 | 0.392 | 0.294 |
| H10 | 0.364 | 0.479 | 0.594 | 0.580 | 0.470 |
| T1 | 0.343 | 0.477 | 0.573 | 0.510 | 0.428 |
| H9a50[P1] | 0.385 | 0.531 | 0.646 | 0.567 | 0.487 |
| V0 | 0.112 | 0.155 | 0.203 | 0.190 | 0.138 |
| V1 | 0.161 | 0.219 | 0.271 | 0.253 | 0.194 |
| V1m | 0.227 | 0.301 | 0.381 | 0.364 | 0.284 |
| V2 | 0.113 | 0.170 | 0.239 | 0.238 | 0.165 |
| V2m | 0.174 | 0.229 | 0.292 | 0.303 | 0.221 |
| V2mD | 0.164 | 0.220 | 0.281 | 0.285 | 0.209 |
| V2mP | 0.091 | 0.099 | 0.127 | 0.147 | 0.093 |
| V2mw | 0.205 | 0.297 | 0.392 | 0.385 | 0.292 |
| V2w | 0.122 | 0.175 | 0.243 | 0.218 | 0.161 |
| V3 | 0.097 | 0.139 | 0.215 | 0.183 | 0.136 |
| V3d | 0.104 | 0.142 | 0.238 | 0.202 | 0.150 |
| V3w | 0.093 | 0.146 | 0.222 | 0.179 | 0.137 |
| VM | 0.197 | 0.236 | 0.267 | 0.302 | 0.221 |

Per signal X, recall@10 (MRR in brackets). Impact = HP25[X] = .25 mm(X) + .75 mm(P1); review = HR25[X] = .25 mm(X) + .75 mm(H9a50[P1]); CIs are paired commit-cluster bootstrap 95%.

| X | subset | n | X | own control V0df[X] | X minus control | P1 | HP25[X] | HP25 minus P1 | H9a50[P1] | HR25[X] | HR25 minus H9 | best blend (selected on test) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| V1 | ALL | 764 | 0.219 (0.253) | 0.165 (0.188) | +0.055 [+0.031, +0.081] | 0.426 (0.476) | 0.439 (0.493) | +0.014 [+0.002, +0.026] | 0.531 (0.567) | 0.521 (0.569) | -0.010 [-0.019, -0.002] | HR25[V1] 0.521, -0.010 [-0.019, -0.002] vs H9a50[P1] |
| V1 | covered | 369 | 0.371 (0.414) | 0.258 (0.278) | +0.113 [+0.069, +0.158] | 0.432 (0.434) | 0.460 (0.468) | +0.028 [+0.004, +0.052] | 0.630 (0.592) | 0.609 (0.596) | -0.021 [-0.039, -0.004] | HTa[V1] 0.621, -0.009 [-0.027, +0.008] vs H9a50[P1] |
| V1m | ALL | 764 | 0.301 (0.364) | 0.215 (0.251) | +0.086 [+0.060, +0.114] | 0.426 (0.476) | 0.463 (0.508) | +0.038 [+0.023, +0.053] | 0.531 (0.567) | 0.528 (0.580) | -0.003 [-0.015, +0.008] | HTa[V1m] 0.531, -0.001 [-0.016, +0.016] vs H9a50[P1] |
| V1m | covered | 569 | 0.378 (0.455) | 0.262 (0.303) | +0.116 [+0.082, +0.151] | 0.428 (0.445) | 0.478 (0.489) | +0.050 [+0.030, +0.071] | 0.566 (0.564) | 0.562 (0.582) | -0.005 [-0.020, +0.012] | HTa[V1m] 0.573, +0.006 [-0.013, +0.026] vs H9a50[P1] |
| V2 | ALL | 764 | 0.170 (0.238) | 0.156 (0.225) | +0.014 [+0.002, +0.026] | 0.426 (0.476) | 0.440 (0.487) | +0.014 [-0.001, +0.030] | 0.531 (0.567) | 0.520 (0.567) | -0.011 [-0.022, -0.001] | HR25[V2] 0.520, -0.011 [-0.022, -0.001] vs H9a50[P1] |
| V2 | covered | 371 | 0.267 (0.380) | 0.238 (0.354) | +0.029 [+0.005, +0.053] | 0.431 (0.434) | 0.460 (0.455) | +0.029 [-0.002, +0.060] | 0.628 (0.591) | 0.605 (0.592) | -0.023 [-0.045, -0.002] | HR25[V2] 0.605, -0.023 [-0.045, -0.002] vs H9a50[P1] |
| V2m | ALL | 764 | 0.229 (0.303) | 0.175 (0.216) | +0.054 [+0.033, +0.075] | 0.426 (0.476) | 0.444 (0.504) | +0.019 [+0.004, +0.035] | 0.531 (0.567) | 0.529 (0.578) | -0.003 [-0.013, +0.007] | HTa[V2m] 0.530, -0.001 [-0.016, +0.015] vs H9a50[P1] |
| V2m | covered | 514 | 0.295 (0.394) | 0.214 (0.264) | +0.081 [+0.049, +0.115] | 0.400 (0.417) | 0.427 (0.459) | +0.028 [+0.004, +0.050] | 0.541 (0.543) | 0.537 (0.560) | -0.004 [-0.019, +0.011] | HTa[V2m] 0.546, +0.005 [-0.015, +0.026] vs H9a50[P1] |
| V2mD | ALL | 764 | 0.220 (0.285) | 0.183 (0.220) | +0.037 [+0.015, +0.061] | 0.426 (0.476) | 0.440 (0.498) | +0.014 [-0.000, +0.030] | 0.531 (0.567) | 0.527 (0.576) | -0.004 [-0.014, +0.006] | HR25[V2mD] 0.527, -0.004 [-0.014, +0.006] vs H9a50[P1] |
| V2mD | covered | 495 | 0.287 (0.376) | 0.230 (0.277) | +0.057 [+0.026, +0.094] | 0.399 (0.412) | 0.422 (0.446) | +0.022 [-0.000, +0.044] | 0.547 (0.546) | 0.540 (0.561) | -0.006 [-0.022, +0.009] | HTa[V2mD] 0.545, -0.001 [-0.024, +0.022] vs H9a50[P1] |
| V2mP | ALL | 764 | 0.099 (0.147) | 0.084 (0.119) | +0.015 [+0.006, +0.025] | 0.426 (0.476) | 0.430 (0.484) | +0.004 [+0.001, +0.009] | 0.531 (0.567) | 0.533 (0.570) | +0.002 [-0.000, +0.004] | HTd[V2mP] 0.534, +0.003 [-0.001, +0.007] vs H9a50[P1] |
| V2mP | covered | 80 | 0.227 (0.443) | 0.087 (0.175) | +0.140 [+0.073, +0.222] | 0.352 (0.499) | 0.393 (0.569) | +0.041 [+0.009, +0.080] | 0.439 (0.477) | 0.455 (0.503) | +0.015 [-0.000, +0.035] | HTd[V2mP] 0.466, +0.026 [-0.007, +0.065] vs H9a50[P1] |
| V2mw | ALL | 764 | 0.297 (0.385) | 0.232 (0.280) | +0.065 [+0.042, +0.090] | 0.426 (0.476) | 0.453 (0.505) | +0.027 [+0.011, +0.043] | 0.531 (0.567) | 0.534 (0.567) | +0.003 [-0.009, +0.015] | HR25[V2mw] 0.534, +0.003 [-0.009, +0.015] vs H9a50[P1] |
| V2mw | covered | 645 | 0.340 (0.442) | 0.263 (0.317) | +0.077 [+0.050, +0.106] | 0.434 (0.457) | 0.466 (0.491) | +0.032 [+0.014, +0.051] | 0.561 (0.565) | 0.564 (0.565) | +0.003 [-0.012, +0.019] | HTa[V2mw] 0.566, +0.005 [-0.012, +0.023] vs H9a50[P1] |
| V2w | ALL | 764 | 0.175 (0.218) | 0.159 (0.200) | +0.016 [+0.005, +0.028] | 0.426 (0.476) | 0.431 (0.490) | +0.005 [-0.008, +0.019] | 0.531 (0.567) | 0.518 (0.565) | -0.013 [-0.024, -0.003] | HR25[V2w] 0.518, -0.013 [-0.024, -0.003] vs H9a50[P1] |
| V2w | covered | 371 | 0.278 (0.340) | 0.245 (0.303) | +0.033 [+0.010, +0.057] | 0.431 (0.434) | 0.442 (0.463) | +0.011 [-0.019, +0.041] | 0.628 (0.591) | 0.600 (0.588) | -0.028 [-0.048, -0.008] | HR25[V2w] 0.600, -0.028 [-0.048, -0.008] vs H9a50[P1] |
| V3 | ALL | 764 | 0.139 (0.183) | 0.139 (0.183) | +0.000 [-0.003, +0.003] | 0.426 (0.476) | 0.426 (0.482) | +0.000 [-0.013, +0.014] | 0.531 (0.567) | 0.514 (0.562) | -0.018 [-0.029, -0.006] | HR25[V3] 0.514, -0.018 [-0.029, -0.006] vs H9a50[P1] |
| V3 | covered | 269 | 0.241 (0.319) | 0.241 (0.319) | +0.000 [-0.007, +0.009] | 0.380 (0.414) | 0.380 (0.429) | +0.000 [-0.042, +0.041] | 0.606 (0.573) | 0.556 (0.559) | -0.051 [-0.084, -0.018] | HR25[V3] 0.556, -0.051 [-0.084, -0.018] vs H9a50[P1] |
| V3d | ALL | 764 | 0.142 (0.202) | 0.146 (0.200) | -0.004 [-0.008, -0.000] | 0.426 (0.476) | 0.434 (0.487) | +0.008 [-0.005, +0.022] | 0.531 (0.567) | 0.513 (0.565) | -0.019 [-0.031, -0.006] | HR25[V3d] 0.513, -0.019 [-0.031, -0.006] vs H9a50[P1] |
| V3d | covered | 291 | 0.237 (0.352) | 0.247 (0.348) | -0.010 [-0.021, -0.001] | 0.388 (0.406) | 0.409 (0.435) | +0.020 [-0.015, +0.055] | 0.614 (0.579) | 0.565 (0.573) | -0.049 [-0.080, -0.017] | HR25[V3d] 0.565, -0.049 [-0.080, -0.017] vs H9a50[P1] |
| V3w | ALL | 764 | 0.146 (0.179) | 0.146 (0.163) | +0.000 [-0.010, +0.012] | 0.426 (0.476) | 0.424 (0.487) | -0.001 [-0.015, +0.012] | 0.531 (0.567) | 0.508 (0.566) | -0.024 [-0.033, -0.014] | HR25[V3w] 0.508, -0.024 [-0.033, -0.014] vs H9a50[P1] |
| V3w | covered | 316 | 0.240 (0.280) | 0.238 (0.242) | +0.001 [-0.024, +0.029] | 0.408 (0.416) | 0.405 (0.443) | -0.003 [-0.038, +0.030] | 0.623 (0.575) | 0.566 (0.574) | -0.057 [-0.083, -0.035] | HR25[V3w] 0.566, -0.057 [-0.083, -0.035] vs H9a50[P1] |
| VM | ALL | 764 | 0.236 (0.302) | - | - | 0.426 (0.476) | - | - | 0.531 (0.567) | 0.550 (0.590) | +0.019 [+0.009, +0.030] | HM50 0.571, +0.039 [+0.019, +0.060] vs H9a50[P1] |
| VM | covered | 362 | 0.426 (0.543) | - | - | 0.429 (0.494) | - | - | 0.545 (0.577) | 0.585 (0.626) | +0.040 [+0.019, +0.061] | HM50 0.628, +0.082 [+0.045, +0.123] vs H9a50[P1] |

Layer contrasts (a minus b, recall@10, ALL queries / covered subset):

| a | b | ALL | covered (n) |
|---|---|---|---|
| V2 | V1 | -0.049 [-0.075, -0.025] | -0.103 [-0.155, -0.050] (369) |
| HP25[V2] | HP25[V1] | +0.000 [-0.013, +0.014] | +0.000 [-0.030, +0.029] (369) |
| HR25[V2] | HR25[V1] | -0.001 [-0.011, +0.010] | -0.002 [-0.023, +0.019] (369) |
| V3 | V2 | -0.031 [-0.043, -0.018] | -0.047 [-0.082, -0.015] (269) |
| HP25[V3] | HP25[V2] | -0.014 [-0.024, -0.004] | -0.035 [-0.055, -0.016] (269) |
| HR25[V3] | HR25[V2] | -0.007 [-0.015, +0.001] | -0.022 [-0.042, -0.005] (269) |
| V3d | V2 | -0.027 [-0.041, -0.013] | -0.041 [-0.079, -0.004] (291) |
| HP25[V3d] | HP25[V2] | -0.006 [-0.016, +0.002] | -0.013 [-0.033, +0.004] (291) |
| HR25[V3d] | HR25[V2] | -0.007 [-0.016, +0.001] | -0.024 [-0.043, -0.005] (291) |
| V1m | V1 | +0.082 [+0.055, +0.109] | +0.039 [+0.012, +0.066] (369) |
| HP25[V1m] | HP25[V1] | +0.024 [+0.013, +0.037] | +0.022 [+0.008, +0.036] (369) |
| HR25[V1m] | HR25[V1] | +0.007 [-0.002, +0.016] | +0.002 [-0.010, +0.015] (369) |
| V2m | V1 | +0.010 [-0.032, +0.050] | +0.061 [+0.007, +0.115] (514) |
| HP25[V2m] | HP25[V1] | +0.005 [-0.011, +0.022] | +0.013 [-0.013, +0.039] (514) |
| HR25[V2m] | HR25[V1] | +0.007 [-0.003, +0.018] | +0.010 [-0.005, +0.026] (514) |
| V2w | V2 | +0.005 [-0.006, +0.017] | +0.011 [-0.013, +0.034] (371) |
| HP25[V2w] | HP25[V2] | -0.009 [-0.019, +0.001] | -0.018 [-0.038, +0.001] (371) |
| HR25[V2w] | HR25[V2] | -0.002 [-0.009, +0.004] | -0.005 [-0.017, +0.009] (371) |
| V2mP | V2mD | -0.121 [-0.153, -0.089] | -0.180 [-0.220, -0.138] (514) |
| HP25[V2mP] | HP25[V2mD] | -0.010 [-0.026, +0.005] | -0.015 [-0.039, +0.008] (514) |
| HR25[V2mP] | HR25[V2mD] | +0.006 [-0.004, +0.015] | +0.009 [-0.007, +0.024] (514) |

### glia

| signal | covered queries | P(link given co-change) | P(link given co-change, no code edge) | P(link given other pair) | lift |
|---|---|---|---|---|---|
| V1 | 654 (52.2%) | 0.189 | 0.127 | 0.0092 | 20.6 |
| V14 | 654 (52.2%) | 0.245 | 0.187 | 0.0723 | 3.4 |
| V1m | 1250 (99.8%) | 0.962 | 0.955 | 0.0282 | 34.2 |
| V2bat | 654 (52.2%) | 0.239 | 0.177 | 0.0488 | 4.9 |
| V2fam | 654 (52.2%) | 0.208 | 0.144 | 0.0136 | 15.3 |
| V2m | 1249 (99.7%) | 0.953 | 0.945 | 0.0207 | 46.0 |
| V2mD | 651 (52.0%) | 0.145 | 0.091 | 0.0201 | 7.2 |
| V2mP | 1211 (96.6%) | 0.942 | 0.936 | 0.0007 | 1268.8 |
| V2wav | 616 (49.2%) | 0.213 | 0.158 | 0.0464 | 4.6 |
| V4 | 599 (47.8%) | 0.227 | 0.175 | 0.0716 | 3.2 |
| V4p | 399 (31.8%) | 0.073 | 0.053 | 0.0048 | 15.3 |
| VM (the commit message's ids) | 1248 (99.6%) | - | - | - | - |

ALL queries (1253):

| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |
|---|---|---|---|---|---|
| B0 | 0.186 | 0.255 | 0.321 | 0.356 | 0.245 |
| P1 | 0.223 | 0.294 | 0.364 | 0.416 | 0.295 |
| C0 | 0.134 | 0.163 | 0.185 | 0.238 | 0.163 |
| H10 | 0.261 | 0.321 | 0.381 | 0.511 | 0.343 |
| T1 | 0.265 | 0.324 | 0.392 | 0.532 | 0.351 |
| H9a50[P1] | 0.307 | 0.404 | 0.486 | 0.570 | 0.413 |
| V0 | 0.006 | 0.014 | 0.024 | 0.024 | 0.014 |
| V1 | 0.126 | 0.180 | 0.218 | 0.192 | 0.161 |
| V14 | 0.119 | 0.167 | 0.203 | 0.189 | 0.152 |
| V1m | 0.556 | 0.712 | 0.813 | 0.752 | 0.731 |
| V2bat | 0.065 | 0.098 | 0.141 | 0.125 | 0.094 |
| V2fam | 0.111 | 0.164 | 0.210 | 0.174 | 0.148 |
| V2m | 0.617 | 0.768 | 0.846 | 0.825 | 0.789 |
| V2mD | 0.118 | 0.152 | 0.177 | 0.212 | 0.149 |
| V2mP | 0.749 | 0.890 | 0.936 | 0.920 | 0.915 |
| V2wav | 0.061 | 0.093 | 0.131 | 0.127 | 0.091 |
| V4 | 0.034 | 0.057 | 0.088 | 0.079 | 0.058 |
| V4p | 0.028 | 0.047 | 0.068 | 0.059 | 0.046 |
| VM | 0.595 | 0.831 | 0.950 | 0.793 | 0.811 |

Per signal X, recall@10 (MRR in brackets). Impact = HP25[X] = .25 mm(X) + .75 mm(P1); review = HR25[X] = .25 mm(X) + .75 mm(H9a50[P1]); CIs are paired commit-cluster bootstrap 95%.

| X | subset | n | X | own control V0df[X] | X minus control | P1 | HP25[X] | HP25 minus P1 | H9a50[P1] | HR25[X] | HR25 minus H9 | best blend (selected on test) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| V1 | ALL | 1253 | 0.180 (0.192) | 0.016 (0.021) | +0.164 [+0.139, +0.193] | 0.294 (0.416) | 0.328 (0.442) | +0.034 [+0.022, +0.048] | 0.404 (0.570) | 0.422 (0.581) | +0.018 [+0.009, +0.027] | HTc[V1] 0.425, +0.021 [+0.007, +0.035] vs H9a50[P1] |
| V1 | covered | 654 | 0.341 (0.365) | 0.026 (0.037) | +0.315 [+0.278, +0.358] | 0.319 (0.337) | 0.385 (0.387) | +0.065 [+0.042, +0.090] | 0.426 (0.556) | 0.460 (0.576) | +0.034 [+0.017, +0.050] | HR25[V1] 0.460, +0.034 [+0.017, +0.050] vs H9a50[P1] |
| V14 | ALL | 1253 | 0.167 (0.189) | 0.020 (0.026) | +0.147 [+0.124, +0.173] | 0.294 (0.416) | 0.323 (0.438) | +0.030 [+0.018, +0.043] | 0.404 (0.570) | 0.416 (0.579) | +0.012 [+0.004, +0.021] | HTc[V14] 0.420, +0.016 [+0.003, +0.029] vs H9a50[P1] |
| V14 | covered | 654 | 0.317 (0.360) | 0.035 (0.046) | +0.282 [+0.247, +0.320] | 0.319 (0.337) | 0.377 (0.380) | +0.057 [+0.033, +0.081] | 0.426 (0.556) | 0.449 (0.572) | +0.023 [+0.008, +0.039] | HR25[V14] 0.449, +0.023 [+0.008, +0.039] vs H9a50[P1] |
| V1m | ALL | 1253 | 0.712 (0.752) | 0.018 (0.029) | +0.693 [+0.666, +0.716] | 0.294 (0.416) | 0.658 (0.588) | +0.364 [+0.338, +0.391] | 0.404 (0.570) | 0.541 (0.673) | +0.137 [+0.121, +0.155] | HTb[V1m] 0.722, +0.318 [+0.286, +0.350] vs H9a50[P1] |
| V1m | covered | 1250 | 0.713 (0.753) | 0.018 (0.029) | +0.695 [+0.669, +0.718] | 0.294 (0.416) | 0.659 (0.588) | +0.365 [+0.339, +0.392] | 0.404 (0.570) | 0.541 (0.673) | +0.138 [+0.122, +0.155] | HTb[V1m] 0.722, +0.319 [+0.287, +0.350] vs H9a50[P1] |
| V2bat | ALL | 1253 | 0.098 (0.125) | 0.030 (0.047) | +0.068 [+0.052, +0.085] | 0.294 (0.416) | 0.299 (0.429) | +0.005 [-0.004, +0.015] | 0.404 (0.570) | 0.397 (0.572) | -0.007 [-0.014, +0.001] | HTc[V2bat] 0.401, -0.003 [-0.014, +0.009] vs H9a50[P1] |
| V2bat | covered | 654 | 0.185 (0.237) | 0.055 (0.086) | +0.130 [+0.103, +0.158] | 0.319 (0.337) | 0.329 (0.362) | +0.010 [-0.009, +0.027] | 0.426 (0.556) | 0.414 (0.559) | -0.013 [-0.027, +0.002] | HR25[V2bat] 0.414, -0.013 [-0.027, +0.002] vs H9a50[P1] |
| V2fam | ALL | 1253 | 0.164 (0.174) | 0.018 (0.023) | +0.146 [+0.123, +0.171] | 0.294 (0.416) | 0.321 (0.438) | +0.027 [+0.016, +0.039] | 0.404 (0.570) | 0.414 (0.576) | +0.010 [+0.002, +0.020] | HTc[V2fam] 0.420, +0.016 [+0.003, +0.029] vs H9a50[P1] |
| V2fam | covered | 654 | 0.311 (0.331) | 0.030 (0.042) | +0.280 [+0.246, +0.319] | 0.319 (0.337) | 0.372 (0.378) | +0.052 [+0.029, +0.073] | 0.426 (0.556) | 0.445 (0.566) | +0.019 [+0.001, +0.036] | HR25[V2fam] 0.445, +0.019 [+0.001, +0.036] vs H9a50[P1] |
| V2m | ALL | 1253 | 0.768 (0.825) | 0.026 (0.038) | +0.741 [+0.715, +0.764] | 0.294 (0.416) | 0.690 (0.607) | +0.396 [+0.371, +0.422] | 0.404 (0.570) | 0.552 (0.684) | +0.148 [+0.132, +0.166] | HRlex[V2m] 0.775, +0.371 [+0.339, +0.406] vs H9a50[P1] |
| V2m | covered | 1249 | 0.770 (0.827) | 0.026 (0.038) | +0.743 [+0.718, +0.766] | 0.294 (0.417) | 0.692 (0.608) | +0.398 [+0.372, +0.424] | 0.404 (0.570) | 0.553 (0.684) | +0.149 [+0.133, +0.167] | HRlex[V2m] 0.776, +0.372 [+0.340, +0.408] vs H9a50[P1] |
| V2mD | ALL | 1253 | 0.152 (0.212) | 0.020 (0.025) | +0.132 [+0.110, +0.157] | 0.294 (0.416) | 0.313 (0.440) | +0.020 [+0.006, +0.033] | 0.404 (0.570) | 0.408 (0.575) | +0.004 [-0.002, +0.010] | HTc[V2mD] 0.411, +0.007 [-0.004, +0.018] vs H9a50[P1] |
| V2mD | covered | 651 | 0.290 (0.405) | 0.035 (0.045) | +0.255 [+0.217, +0.292] | 0.321 (0.333) | 0.359 (0.379) | +0.038 [+0.014, +0.065] | 0.432 (0.557) | 0.439 (0.566) | +0.007 [-0.004, +0.018] | HR25[V2mD] 0.439, +0.007 [-0.004, +0.018] vs H9a50[P1] |
| V2mP | ALL | 1253 | 0.890 (0.920) | 0.056 (0.095) | +0.834 [+0.807, +0.860] | 0.294 (0.416) | 0.804 (0.651) | +0.511 [+0.484, +0.538] | 0.404 (0.570) | 0.631 (0.736) | +0.227 [+0.206, +0.250] | HRlex[V2mP] 0.910, +0.506 [+0.476, +0.536] vs H9a50[P1] |
| V2mP | covered | 1211 | 0.921 (0.951) | 0.058 (0.098) | +0.863 [+0.841, +0.885] | 0.297 (0.421) | 0.826 (0.663) | +0.529 [+0.500, +0.557] | 0.406 (0.572) | 0.640 (0.743) | +0.234 [+0.213, +0.257] | HRlex[V2mP] 0.929, +0.523 [+0.495, +0.552] vs H9a50[P1] |
| V2wav | ALL | 1253 | 0.093 (0.127) | 0.028 (0.050) | +0.065 [+0.050, +0.081] | 0.294 (0.416) | 0.297 (0.430) | +0.004 [-0.006, +0.014] | 0.404 (0.570) | 0.402 (0.576) | -0.002 [-0.008, +0.005] | HR25[V2wav] 0.402, -0.002 [-0.008, +0.005] vs H9a50[P1] |
| V2wav | covered | 616 | 0.185 (0.256) | 0.053 (0.098) | +0.132 [+0.103, +0.160] | 0.305 (0.330) | 0.312 (0.358) | +0.007 [-0.014, +0.028] | 0.428 (0.555) | 0.424 (0.567) | -0.003 [-0.016, +0.010] | HR25[V2wav] 0.424, -0.003 [-0.016, +0.010] vs H9a50[P1] |
| V4 | ALL | 1253 | 0.057 (0.079) | 0.014 (0.025) | +0.043 [+0.032, +0.056] | 0.294 (0.416) | 0.292 (0.417) | -0.002 [-0.011, +0.007] | 0.404 (0.570) | 0.395 (0.570) | -0.009 [-0.015, -0.003] | HTc[V4] 0.396, -0.008 [-0.019, +0.002] vs H9a50[P1] |
| V4 | covered | 599 | 0.115 (0.161) | 0.025 (0.049) | +0.090 [+0.068, +0.114] | 0.305 (0.328) | 0.301 (0.330) | -0.004 [-0.022, +0.016] | 0.425 (0.548) | 0.406 (0.547) | -0.019 [-0.033, -0.007] | HR25[V4] 0.406, -0.019 [-0.033, -0.007] vs H9a50[P1] |
| V4p | ALL | 1253 | 0.047 (0.059) | 0.012 (0.018) | +0.035 [+0.024, +0.047] | 0.294 (0.416) | 0.291 (0.417) | -0.002 [-0.011, +0.008] | 0.404 (0.570) | 0.398 (0.566) | -0.006 [-0.012, -0.000] | HR25[V4p] 0.398, -0.006 [-0.012, -0.000] vs H9a50[P1] |
| V4p | covered | 399 | 0.137 (0.176) | 0.027 (0.050) | +0.110 [+0.078, +0.141] | 0.284 (0.284) | 0.277 (0.287) | -0.007 [-0.039, +0.022] | 0.405 (0.522) | 0.386 (0.508) | -0.019 [-0.037, -0.002] | HR25[V4p] 0.386, -0.019 [-0.037, -0.002] vs H9a50[P1] |
| VM | ALL | 1253 | 0.831 (0.793) | - | - | 0.294 (0.416) | - | - | 0.404 (0.570) | 0.531 (0.697) | +0.127 [+0.109, +0.145] | HMlex 0.833, +0.429 [+0.393, +0.467] vs H9a50[P1] |
| VM | covered | 1248 | 0.835 (0.796) | - | - | 0.295 (0.418) | - | - | 0.402 (0.569) | 0.530 (0.697) | +0.127 [+0.109, +0.144] | HMlex 0.833, +0.431 [+0.396, +0.468] vs H9a50[P1] |

Layer contrasts (a minus b, recall@10, ALL queries / covered subset):

| a | b | ALL | covered (n) |
|---|---|---|---|
| V4 | V2fam | -0.107 [-0.129, -0.087] | -0.192 [-0.225, -0.156] (599) |
| HP25[V4] | HP25[V2fam] | -0.029 [-0.040, -0.019] | -0.056 [-0.073, -0.037] (599) |
| HR25[V4] | HR25[V2fam] | -0.019 [-0.028, -0.012] | -0.035 [-0.050, -0.021] (599) |
| V4 | V1 | -0.123 [-0.147, -0.100] | -0.226 [-0.263, -0.191] (599) |
| HP25[V4] | HP25[V1] | -0.036 [-0.048, -0.025] | -0.068 [-0.087, -0.048] (599) |
| HR25[V4] | HR25[V1] | -0.027 [-0.036, -0.019] | -0.049 [-0.064, -0.034] (599) |
| V4p | V2fam | -0.117 [-0.142, -0.095] | -0.135 [-0.166, -0.107] (399) |
| HP25[V4p] | HP25[V2fam] | -0.029 [-0.042, -0.017] | -0.043 [-0.067, -0.020] (399) |
| HR25[V4p] | HR25[V2fam] | -0.016 [-0.026, -0.008] | -0.025 [-0.039, -0.009] (399) |
| V14 | V1 | -0.013 [-0.019, -0.006] | -0.024 [-0.037, -0.013] (654) |
| HP25[V14] | HP25[V1] | -0.004 [-0.008, -0.000] | -0.008 [-0.017, -0.000] (654) |
| HR25[V14] | HR25[V1] | -0.006 [-0.009, -0.003] | -0.011 [-0.017, -0.005] (654) |
| V2fam | V1 | -0.016 [-0.026, -0.006] | -0.030 [-0.051, -0.012] (654) |
| HP25[V2fam] | HP25[V1] | -0.007 [-0.014, +0.000] | -0.013 [-0.028, -0.001] (654) |
| HR25[V2fam] | HR25[V1] | -0.008 [-0.013, -0.003] | -0.015 [-0.025, -0.006] (654) |
| V2wav | V1 | -0.087 [-0.106, -0.069] | -0.161 [-0.193, -0.130] (616) |
| HP25[V2wav] | HP25[V1] | -0.031 [-0.041, -0.021] | -0.060 [-0.078, -0.042] (616) |
| HR25[V2wav] | HR25[V1] | -0.019 [-0.026, -0.013] | -0.034 [-0.045, -0.024] (616) |
| V2m | V1 | +0.588 [+0.555, +0.618] | +0.292 [+0.258, +0.328] (654) |
| HP25[V2m] | HP25[V1] | +0.362 [+0.336, +0.389] | +0.154 [+0.131, +0.176] (654) |
| HR25[V2m] | HR25[V1] | +0.131 [+0.115, +0.146] | +0.067 [+0.053, +0.084] (654) |
| V2mP | V2mD | +0.738 [+0.705, +0.770] | +0.741 [+0.706, +0.773] (1249) |
| HP25[V2mP] | HP25[V2mD] | +0.491 [+0.463, +0.520] | +0.493 [+0.464, +0.522] (1249) |
| HR25[V2mP] | HR25[V2mD] | +0.223 [+0.202, +0.245] | +0.224 [+0.203, +0.246] (1249) |
| V4 | V4perm | +0.004 [-0.013, +0.019] | +0.012 [-0.023, +0.045] (599) |
| V4p | V4pperm | +0.008 [-0.008, +0.023] | +0.074 [+0.039, +0.111] (399) |
| V14 | V14perm | -0.000 [-0.007, +0.006] | -0.001 [-0.013, +0.010] (654) |

### quokka-stack

| signal | covered queries | P(link given co-change) | P(link given co-change, no code edge) | P(link given other pair) | lift |
|---|---|---|---|---|---|
| V2m | 89 (80.9%) | 0.609 | 0.505 | 0.2425 | 2.5 |

ALL queries (110):

| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |
|---|---|---|---|---|---|
| B0 | 0.461 | 0.594 | 0.719 | 0.470 | 0.459 |
| P1 | 0.436 | 0.615 | 0.737 | 0.458 | 0.474 |
| C0 | 0.264 | 0.275 | 0.295 | 0.273 | 0.253 |
| H10 | 0.531 | 0.636 | 0.730 | 0.602 | 0.564 |
| T1 | 0.360 | 0.410 | 0.476 | 0.406 | 0.365 |
| H9a50[P1] | 0.536 | 0.625 | 0.748 | 0.542 | 0.536 |
| V2m | 0.347 | 0.458 | 0.541 | 0.364 | 0.375 |

Per signal X, recall@10 (MRR in brackets). Impact = HP25[X] = .25 mm(X) + .75 mm(P1); review = HR25[X] = .25 mm(X) + .75 mm(H9a50[P1]); CIs are paired commit-cluster bootstrap 95%.

| X | subset | n | X | own control V0df[X] | X minus control | P1 | HP25[X] | HP25 minus P1 | H9a50[P1] | HR25[X] | HR25 minus H9 | best blend (selected on test) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| V2m | ALL | 110 | 0.458 (0.364) | 0.131 (0.091) | +0.327 [+0.202, +0.475] | 0.615 (0.458) | 0.676 (0.535) | +0.061 [+0.004, +0.120] | 0.625 (0.542) | 0.660 (0.585) | +0.035 [+0.000, +0.070] | HTc[V2m] 0.681, +0.056 [-0.001, +0.104] vs H9a50[P1] |
| V2m | covered | 89 | 0.566 (0.447) | 0.161 (0.109) | +0.404 [+0.256, +0.565] | 0.608 (0.408) | 0.683 (0.503) | +0.075 [+0.006, +0.138] | 0.641 (0.535) | 0.684 (0.588) | +0.043 [-0.002, +0.084] | HR50[V2m] 0.710, +0.069 [-0.002, +0.132] vs H9a50[P1] |

