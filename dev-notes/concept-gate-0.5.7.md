# The concept-search semantic gate, re-run (0.5.7, HF.2)

Why a re-run: HF.1 (f70f3957, FLOWS.1 in the 0.5.7 plan) made declared entries flow entries: a node
carrying an ENTRYPOINT cell (a host start, a `__main__` guard, a script MODULE, a host callback or plugin
unit, an overlay `[entrypoints]` declaration) now seeds `trace::entries`, as it already seeded liveness.
GB.6's truth and its S / Clex / Creach rankers are all built over the head's entry flows
(`entry_flows_with_live` / `concept_flows_with_live` in rank_eval's concept harness), so HF.1 changes the
gate's input. dev-notes/next-leap-0.5.7.md §3 asks for the semantic tier to be judged again on the flows
glia now has, under GB.6's rule as committed. dev-notes/concept-gate-0.5.6.md (the 0.5.6 verdict,
`CONCEPT: C1=yes (4/5) C2=yes (4/5) -> PASS`) is left as it was.

Produced by `GLIA_CONCEPT_REPORT=<this file> bash scripts/embedding-gate.sh concept-verdict` (rank_eval
`--sem on --concept on`, `--gate-pair .. --gate-shape concept`), on the same nine-repo panel at the same pinned
revs and test windows, reusing the cached bge-small node vectors. Every number below is an aggregate over a
public repository. Model: `bge-small`.

## The pass rule (GB.6's, unchanged)

Rule committed in e333904c (0.5.6), unchanged; not re-tuned. This file, holding the copy below, is committed
before the re-run. The block is a verbatim copy of dev-notes/concept-gate-0.5.6.md's rule block.

```
THE CONCEPT RULE (GB.6; committed before any panel run; strictness as FB-5 / FB.6b, never tuned here):
  Panel: the nine `verdict` rows of bench/rank-gate/fusion-preview.json at their pinned revs and test
  windows (FB.6b's panel, ruling FB-6b-PANEL): commons-lang and commons-io with `--test-after` their FB.6
  rev (the forward windows), the seven fresh repos with rank_eval's default window (the newest 300
  test-eligible commits). The run is fusion-verdict's exactly (scripts/rank-gate.sh's COMMON flags,
  `--sem on --sem-model bge-small`, FB.6's cached vectors in $GLIA_RANK_GATE_DIR/sem) plus `--concept on`.
  GB.3's configuration is fixed and never tuned here (it was never run on these repos for flows).
  The head build: one graph per repo at its pinned rev; its entry flows are `trace::entry_flows_with_live`
  at the CLI's defaults (`flows --concept`): depth DEFAULT_DEPTH, identity collapse with the repo labels.
  Query per test commit c: the subject of c's message, redacted as EB.1 stores it (`redact_untrusted`).
  Lexical hit: GB.1's `concept_flows` (semantic None, the CLI defaults: depth DEFAULT_DEPTH, limit 20,
  collapse with labels) returns a row. Such a query is counted (`lexical_hit`) and never scored: the
  semantic tier fires only when no lexical tier matches any flow.
  Truth per remaining query: the head's entry flows with at least one node (the entry or a hop) located
  in a file c changed (c's changed paths in the head's candidate file space, the find shape's truth
  files). A query with no truth flow is dropped and counted (`dropped_no_truth`); the rest are the
  repo's concept queries.
  Rankers, each a flow order cut at k = 5:
    S      GB.3's semantic tier: the flows through the files find --files' fused order
           LX[IDR,FUSE[T1s,SEMp:t2]] (FIND_FILES_FUSED, the uncalibrated pick) ranks with a `sem`
           score, each flow at its least (file rank, depth) node, ranked by (file rank, depth, reach
           desc, key, entry qname, flow position): trace.rs `semantic_ranked`'s mapping, ported into
           rank_eval because the shipped call reads the history from disk. Its file order is
           `find::find_files_with` over c's leak-free history, exactly as ANS:find_files (FB.7c): the
           history-sync window with c and every newer commit excluded, ids routed at c's parent.
           Port parity: per concept query, the port fed by the shipped call's own `find_files` (no
           history: the snapshot copies hold none) is compared row for row (key, entry qname) with
           the shipped `concept_flows(semantic = the run's SemanticSource, rank_cache = a RankCache)`;
           any mismatch on any repo and the judge refuses (no verdict).
    Clex   the same mapping fed by LX[IDR,T1s] (FIND_FILES_FUSION: find_files_with with semantic None,
           the same history and pick): every row it ranks (idr > 0 or text > 0), every step of S
           except the similarity.
    Creach every head flow by (reach desc, key, entry qname, flow position): no query.
    Crand  every head flow by a seeded hash of (c's sha, flow position): information only.
  Metric: flow hit@5 (any of the first 5 rows is a truth flow). Information rows: hit@1 and
  precision@5 (truth flows among the first 5, over 5). Every a - b is rank_eval `--gate-pair`'s paired
  commit-cluster bootstrap 95% CI (B = 1000) on that repo's concept queries (one query per commit).
  A repo with fewer than 30 concept queries is reported and left out; n = the repos left.
  C1: S - Clex has CI lo > 0 on >= ceil(n/2) repos and CI hi >= 0 on all n.
  C2: S - Creach has CI lo > 0 on >= ceil(n/2) repos and CI hi >= 0 on all n.
  PASS = C1 and C2 with n >= 5. n < 5 is INSUFFICIENT and reads as FAIL. Anything else is FAIL.
  Information rows only, never the verdict: S - Crand and Clex - Creach (hit@5); S - Clex and
  S - Creach on hit@1 and precision@5.
  Leak: the graph, its flows and the node vectors are the head snapshot's (the same leak class as FB.6
  and FB.6b); the text history, the id routing and the query are leak-free per commit.
  ON PASS nothing in the answers changes and GZ.1 cites this report.
  ON FAIL (or INSUFFICIENT) the tier goes to the backlog, not away for good: the integrator writes the
  packet that takes it out of 0.5.6 (ConceptTier::Semantic, ConceptHit.similarity, ConceptOptions /
  TraceOptions.{semantic, rank_cache}, resolved_by `concept_semantic`, `flows` / `trace --no-semantic`
  and `concept_flows(semantic=)`; concept search is lexical only, GB.1's answer byte for byte; the GZ.2
  handoff entry from GB.3 is withdrawn) and adds a dev-notes/precision-backlog.json entry naming this
  report, GB.3's commits (aec37667, 9bb8ca3d) and the rule rows it missed, so a later release can retry.
```

How the rule's ON FAIL reads in 0.5.7 (HF.2's spec; ruling: default accepted): the tier goes to the backlog
exactly as written (the removal packet takes ConceptTier::Semantic, ConceptHit.similarity, ConceptOptions /
TraceOptions.{semantic, rank_cache}, resolved_by `concept_semantic`, `flows` / `trace --no-semantic` and
`concept_flows(semantic=)` out; concept search lexical only), with a dev-notes/precision-backlog.json entry
naming both reports, GB.3's commits (aec37667, 9bb8ca3d) and the missed rule rows. HF.1 is NOT reverted on a
FAIL: declared entries are a correctness fix to the flow set, and the gate judges the tier on the flows glia
really has. ON PASS nothing ships differently and the release docs cite this report.

Fired_on: rank_eval's `[concept-eval] label=<l> test=<t> lexical_hit=<h> concept_queries=<q>
dropped_no_truth=<d> flows=<f> ms=<ms>` once per repo (and `[concept-eval] parity label=<l> queries=<q>
mismatch=<m>`), `[rank-gate] pair label=<l> shape=concept a=<a> b=<b> metric=hit5 diff=.. ci=[..]` per
pair, then the judge's `CONCEPT: C1=<yes|no> (<k>/<n>) C2=<yes|no> (<k>/<n>) -> PASS|FAIL|INSUFFICIENT`.
Then `[concept-gate] verdict=<PASS|FAIL|INSUFFICIENT> report=dev-notes/concept-gate-0.5.7.md`.

## Report

<!-- concept-gate:verdict -->
### Panel

| repo | language | rev | test commits | lexical hit | concept queries | dropped (no truth) | no parent | flows | port parity (queries / mismatches) | in n (>= 30) | first test sha | last test sha |
|---|---|---|---:|---:|---:|---:|---:|---:|---|---|---|---|
| commons-lang | java | 4b1d8cb83c26 | 300 | 1 | 251 | 48 | 0 | 1132 | 251 / 0 | yes | ae061818e5bb | 4b1d8cb83c26 |
| commons-io | java | f9d4c7567f91 | 300 | 33 | 242 | 25 | 0 | 1257 | 242 / 0 | yes | f03363e242e1 | e4e4fbdc3791 |
| guzzle | php | 89f429318d52 | 300 | 0 | 0 | 300 | 0 | 0 | 0 / 0 | no | acd0bcded670 | 1e0396c29269 |
| zod | typescript | afdac65e0f80 | 300 | 0 | 17 | 283 | 0 | 28 | 17 / 0 | no | 86d4dad5bc27 | 680d19f7d1e0 |
| okhttp | kotlin | c3179301fc7d | 300 | 0 | 168 | 132 | 0 | 195 | 168 / 0 | yes | b926a030c628 | fb34d4da7462 |
| sttp | scala | d06f987834af | 155 | 0 | 12 | 143 | 0 | 3 | 12 / 0 | no | 6fba8d10266e | 12c7fed697fa |
| AutoMapper | csharp | f0ecba7a97d1 | 300 | 0 | 0 | 300 | 0 | 0 | 0 / 0 | no | 25afaf7e6dc6 | fec985c44d16 |
| jq | c | bd56a745624a | 102 | 2 | 83 | 17 | 0 | 3 | 83 / 0 | yes | 154d0ee4a824 | 852c4d609611 |
| Alamofire | swift | bda9ed57d729 | 232 | 0 | 213 | 18 | 1 | 452 | 213 / 0 | yes | c20b5602a3d5 | 3494b7c1d4e0 |

n = 5 (repos with >= 30 concept queries); left out: guzzle, zod, sttp, AutoMapper.

Device per repo (FB.14's `[embed] device=` line of each run):

- commons-lang: `(no [embed] device= line: every vector came from the cache)`
- commons-io: `(no [embed] device= line: every vector came from the cache)`
- guzzle: `(no [embed] device= line: every vector came from the cache)`
- zod: `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04 fallback=none batch=8`
- okhttp: `(no [embed] device= line: every vector came from the cache)`
- sttp: `(no [embed] device= line: every vector came from the cache)`
- AutoMapper: `(no [embed] device= line: every vector came from the cache)`
- jq: `(no [embed] device= line: every vector came from the cache)`
- Alamofire: `(no [embed] device= line: every vector came from the cache)`

### Means per ranker (concept queries; k = 5)

| repo | ranker | hit@1 | hit@5 | p@5 |
|---|---|---:|---:|---:|
| commons-lang | S | 0.5697 | 0.6056 | 0.4869 |
| commons-lang | Clex | 0.5060 | 0.5259 | 0.4143 |
| commons-lang | Creach | 0.1355 | 0.1355 | 0.1355 |
| commons-lang | Crand | 0.0398 | 0.1355 | 0.0327 |
| commons-io | S | 0.4876 | 0.5289 | 0.4587 |
| commons-io | Clex | 0.3967 | 0.4421 | 0.3818 |
| commons-io | Creach | 0.1529 | 0.3058 | 0.1603 |
| commons-io | Crand | 0.0413 | 0.1736 | 0.0405 |
| guzzle | S | 0.0000 | 0.0000 | 0.0000 |
| guzzle | Clex | 0.0000 | 0.0000 | 0.0000 |
| guzzle | Creach | 0.0000 | 0.0000 | 0.0000 |
| guzzle | Crand | 0.0000 | 0.0000 | 0.0000 |
| zod | S | 0.4706 | 0.5882 | 0.2471 |
| zod | Clex | 0.2353 | 0.2941 | 0.0824 |
| zod | Creach | 0.1176 | 0.5294 | 0.2118 |
| zod | Crand | 0.1176 | 0.4706 | 0.1529 |
| okhttp | S | 0.4226 | 0.5595 | 0.3619 |
| okhttp | Clex | 0.3155 | 0.3929 | 0.2679 |
| okhttp | Creach | 0.2560 | 0.3333 | 0.2095 |
| okhttp | Crand | 0.0952 | 0.2738 | 0.0762 |
| sttp | S | 0.8333 | 1.0000 | 0.3167 |
| sttp | Clex | 0.5000 | 0.5000 | 0.1500 |
| sttp | Creach | 0.5833 | 1.0000 | 0.3167 |
| sttp | Crand | 0.6667 | 1.0000 | 0.3167 |
| AutoMapper | S | 0.0000 | 0.0000 | 0.0000 |
| AutoMapper | Clex | 0.0000 | 0.0000 | 0.0000 |
| AutoMapper | Creach | 0.0000 | 0.0000 | 0.0000 |
| AutoMapper | Crand | 0.0000 | 0.0000 | 0.0000 |
| jq | S | 1.0000 | 1.0000 | 0.3639 |
| jq | Clex | 0.9639 | 0.9639 | 0.3518 |
| jq | Creach | 1.0000 | 1.0000 | 0.3639 |
| jq | Crand | 0.6627 | 1.0000 | 0.3639 |
| Alamofire | S | 0.5540 | 0.5775 | 0.5455 |
| Alamofire | Clex | 0.4977 | 0.5211 | 0.4892 |
| Alamofire | Creach | 0.1737 | 0.1737 | 0.1737 |
| Alamofire | Crand | 0.1831 | 0.5211 | 0.1897 |

### Rule rows (flow hit@5)

C1 (S - Clex, CI lo > 0 on >= 3 of 5, hi >= 0 on all 5): lo > 0 on 4, hi >= 0 on 5 -> yes. C2 (S - Creach, the same): lo > 0 on 4, hi >= 0 on 5 -> yes. Only the repos in n count; the others are listed.

| repo | in n | metric | a | b | a | b | a - b | 95% CI |
|---|---|---|---|---|---:|---:|---:|---|
| commons-lang | yes | hit5 | S | Clex | 0.6056 | 0.5259 | +0.0797 | [+0.0438, +0.1195] |
| commons-io | yes | hit5 | S | Clex | 0.5289 | 0.4421 | +0.0868 | [+0.0496, +0.1281] |
| guzzle | no | hit5 | S | Clex | - | - | - | (no concept queries) |
| zod | no | hit5 | S | Clex | 0.5882 | 0.2941 | +0.2941 | [+0.0588, +0.5294] |
| okhttp | yes | hit5 | S | Clex | 0.5595 | 0.3929 | +0.1667 | [+0.1012, +0.2321] |
| sttp | no | hit5 | S | Clex | 1.0000 | 0.5000 | +0.5000 | [+0.2500, +0.8333] |
| AutoMapper | no | hit5 | S | Clex | - | - | - | (no concept queries) |
| jq | yes | hit5 | S | Clex | 1.0000 | 0.9639 | +0.0361 | [+0.0000, +0.0843] |
| Alamofire | yes | hit5 | S | Clex | 0.5775 | 0.5211 | +0.0563 | [+0.0141, +0.1080] |
| commons-lang | yes | hit5 | S | Creach | 0.6056 | 0.1355 | +0.4701 | [+0.3904, +0.5418] |
| commons-io | yes | hit5 | S | Creach | 0.5289 | 0.3058 | +0.2231 | [+0.1529, +0.3017] |
| guzzle | no | hit5 | S | Creach | - | - | - | (no concept queries) |
| zod | no | hit5 | S | Creach | 0.5882 | 0.5294 | +0.0588 | [-0.2941, +0.4118] |
| okhttp | yes | hit5 | S | Creach | 0.5595 | 0.3333 | +0.2262 | [+0.1548, +0.3036] |
| sttp | no | hit5 | S | Creach | 1.0000 | 1.0000 | +0.0000 | [+0.0000, +0.0000] |
| AutoMapper | no | hit5 | S | Creach | - | - | - | (no concept queries) |
| jq | yes | hit5 | S | Creach | 1.0000 | 1.0000 | +0.0000 | [+0.0000, +0.0000] |
| Alamofire | yes | hit5 | S | Creach | 0.5775 | 0.1737 | +0.4038 | [+0.3333, +0.4789] |

### Information rows (never the verdict)

| repo | in n | metric | a | b | a | b | a - b | 95% CI |
|---|---|---|---|---|---:|---:|---:|---|
| commons-lang | yes | hit5 | S | Crand | 0.6056 | 0.1355 | +0.4701 | [+0.3944, +0.5458] |
| commons-io | yes | hit5 | S | Crand | 0.5289 | 0.1736 | +0.3554 | [+0.2810, +0.4380] |
| guzzle | no | hit5 | S | Crand | - | - | - | (no concept queries) |
| zod | no | hit5 | S | Crand | 0.5882 | 0.4706 | +0.1176 | [-0.2353, +0.4706] |
| okhttp | yes | hit5 | S | Crand | 0.5595 | 0.2738 | +0.2857 | [+0.2024, +0.3690] |
| sttp | no | hit5 | S | Crand | 1.0000 | 1.0000 | +0.0000 | [+0.0000, +0.0000] |
| AutoMapper | no | hit5 | S | Crand | - | - | - | (no concept queries) |
| jq | yes | hit5 | S | Crand | 1.0000 | 1.0000 | +0.0000 | [+0.0000, +0.0000] |
| Alamofire | yes | hit5 | S | Crand | 0.5775 | 0.5211 | +0.0563 | [-0.0282, +0.1408] |
| commons-lang | yes | hit5 | Clex | Creach | 0.5259 | 0.1355 | +0.3904 | [+0.3147, +0.4701] |
| commons-io | yes | hit5 | Clex | Creach | 0.4421 | 0.3058 | +0.1364 | [+0.0702, +0.2107] |
| guzzle | no | hit5 | Clex | Creach | - | - | - | (no concept queries) |
| zod | no | hit5 | Clex | Creach | 0.2941 | 0.5294 | -0.2353 | [-0.5882, +0.1176] |
| okhttp | yes | hit5 | Clex | Creach | 0.3929 | 0.3333 | +0.0595 | [-0.0119, +0.1369] |
| sttp | no | hit5 | Clex | Creach | 0.5000 | 1.0000 | -0.5000 | [-0.8333, -0.2500] |
| AutoMapper | no | hit5 | Clex | Creach | - | - | - | (no concept queries) |
| jq | yes | hit5 | Clex | Creach | 0.9639 | 1.0000 | -0.0361 | [-0.0843, +0.0000] |
| Alamofire | yes | hit5 | Clex | Creach | 0.5211 | 0.1737 | +0.3474 | [+0.2723, +0.4272] |
| commons-lang | yes | hit1 | S | Clex | 0.5697 | 0.5060 | +0.0637 | [+0.0279, +0.1036] |
| commons-io | yes | hit1 | S | Clex | 0.4876 | 0.3967 | +0.0909 | [+0.0537, +0.1322] |
| guzzle | no | hit1 | S | Clex | - | - | - | (no concept queries) |
| zod | no | hit1 | S | Clex | 0.4706 | 0.2353 | +0.2353 | [+0.0000, +0.4706] |
| okhttp | yes | hit1 | S | Clex | 0.4226 | 0.3155 | +0.1071 | [+0.0536, +0.1667] |
| sttp | no | hit1 | S | Clex | 0.8333 | 0.5000 | +0.3333 | [+0.0833, +0.5833] |
| AutoMapper | no | hit1 | S | Clex | - | - | - | (no concept queries) |
| jq | yes | hit1 | S | Clex | 1.0000 | 0.9639 | +0.0361 | [+0.0000, +0.0843] |
| Alamofire | yes | hit1 | S | Clex | 0.5540 | 0.4977 | +0.0563 | [+0.0094, +0.1080] |
| commons-lang | yes | hit1 | S | Creach | 0.5697 | 0.1355 | +0.4343 | [+0.3546, +0.5100] |
| commons-io | yes | hit1 | S | Creach | 0.4876 | 0.1529 | +0.3347 | [+0.2686, +0.4050] |
| guzzle | no | hit1 | S | Creach | - | - | - | (no concept queries) |
| zod | no | hit1 | S | Creach | 0.4706 | 0.1176 | +0.3529 | [+0.0588, +0.6471] |
| okhttp | yes | hit1 | S | Creach | 0.4226 | 0.2560 | +0.1667 | [+0.0893, +0.2560] |
| sttp | no | hit1 | S | Creach | 0.8333 | 0.5833 | +0.2500 | [-0.0833, +0.5833] |
| AutoMapper | no | hit1 | S | Creach | - | - | - | (no concept queries) |
| jq | yes | hit1 | S | Creach | 1.0000 | 1.0000 | +0.0000 | [+0.0000, +0.0000] |
| Alamofire | yes | hit1 | S | Creach | 0.5540 | 0.1737 | +0.3803 | [+0.3099, +0.4554] |
| commons-lang | yes | p5 | S | Clex | 0.4869 | 0.4143 | +0.0725 | [+0.0422, +0.1044] |
| commons-io | yes | p5 | S | Clex | 0.4587 | 0.3818 | +0.0769 | [+0.0446, +0.1132] |
| guzzle | no | p5 | S | Clex | - | - | - | (no concept queries) |
| zod | no | p5 | S | Clex | 0.2471 | 0.0824 | +0.1647 | [+0.0353, +0.3294] |
| okhttp | yes | p5 | S | Clex | 0.3619 | 0.2679 | +0.0940 | [+0.0548, +0.1357] |
| sttp | no | p5 | S | Clex | 0.3167 | 0.1500 | +0.1667 | [+0.0667, +0.2667] |
| AutoMapper | no | p5 | S | Clex | - | - | - | (no concept queries) |
| jq | yes | p5 | S | Clex | 0.3639 | 0.3518 | +0.0120 | [+0.0024, +0.0265] |
| Alamofire | yes | p5 | S | Clex | 0.5455 | 0.4892 | +0.0563 | [+0.0150, +0.1023] |
| commons-lang | yes | p5 | S | Creach | 0.4869 | 0.1355 | +0.3514 | [+0.2797, +0.4231] |
| commons-io | yes | p5 | S | Creach | 0.4587 | 0.1603 | +0.2983 | [+0.2405, +0.3645] |
| guzzle | no | p5 | S | Creach | - | - | - | (no concept queries) |
| zod | no | p5 | S | Creach | 0.2471 | 0.2118 | +0.0353 | [-0.0941, +0.1647] |
| okhttp | yes | p5 | S | Creach | 0.3619 | 0.2095 | +0.1524 | [+0.0940, +0.2095] |
| sttp | no | p5 | S | Creach | 0.3167 | 0.3167 | +0.0000 | [+0.0000, +0.0000] |
| AutoMapper | no | p5 | S | Creach | - | - | - | (no concept queries) |
| jq | yes | p5 | S | Creach | 0.3639 | 0.3639 | +0.0000 | [+0.0000, +0.0000] |
| Alamofire | yes | p5 | S | Creach | 0.5455 | 0.1737 | +0.3718 | [+0.3052, +0.4488] |

CONCEPT: C1=yes (4/5) C2=yes (4/5) -> PASS
<!-- /concept-gate:verdict -->

## Compared with 0.5.6

The 0.5.6 numbers are dev-notes/concept-gate-0.5.6.md's (run from 86a74bda); the 0.5.7 numbers are this
re-run's, from ec83d3df, which holds HF.1 (f70f3957, declared entries seed entry flows). `entries` and
`declared` are the build's `[flows] entries=` and HF.1's `[flows] declared entries=<d> new=<n>` lines in each
repo's run log (no `declared` line = 0). "Rule rows" are C1 (S - Clex, hit@5) and C2 (S - Creach, hit@5).

| repo | in n | entries 0.5.6 -> 0.5.7 | declared (new) | flows 0.5.6 -> 0.5.7 | concept queries 0.5.6 -> 0.5.7 | C1 row (S - Clex) | C2 row (S - Creach) |
|---|---|---|---|---|---|---|---|
| commons-lang | yes | 5196 -> 5196 | 0 | 1132 -> 1132 | 251 -> 251 | identical: +0.0797 [+0.0438, +0.1195] | identical: +0.4701 [+0.3904, +0.5418] |
| commons-io | yes | 3252 -> 3252 | 0 | 1257 -> 1257 | 242 -> 242 | identical: +0.0868 [+0.0496, +0.1281] | identical: +0.2231 [+0.1529, +0.3017] |
| okhttp | yes | 316 -> 316 | 0 | 195 -> 195 | 168 -> 168 | identical: +0.1667 [+0.1012, +0.2321] | identical: +0.2262 [+0.1548, +0.3036] |
| jq | yes | 3 -> 3 | 0 | 3 -> 3 | 83 -> 83 | identical: +0.0361 [+0.0000, +0.0843] | identical: +0.0000 [+0.0000, +0.0000] |
| Alamofire | yes | 778 -> 764 | 0 | 452 -> 452 | 213 -> 213 | identical: +0.0563 [+0.0141, +0.1080] | identical: +0.4038 [+0.3333, +0.4789] |
| zod | no | 41 -> 55 | 1 (1) | 15 -> 28 | 14 -> 17 | +0.1429 [+0.0000, +0.3571] -> +0.2941 [+0.0588, +0.5294] | +0.2857 [-0.1429, +0.6429] -> +0.0588 [-0.2941, +0.4118] |
| sttp | no | 58 -> 58 | 0 | 3 -> 3 | 12 -> 12 | identical: +0.5000 [+0.2500, +0.8333] | identical: +0.0000 [+0.0000, +0.0000] |
| guzzle | no | 0 -> 0 | 0 | 0 -> 0 | 0 -> 0 | (no concept queries) | (no concept queries) |
| AutoMapper | no | 16 -> 16 | 0 | 0 -> 0 | 0 -> 0 | (no concept queries) | (no concept queries) |

The verdict section differs from 0.5.6's only in zod's rows (every table) and three device lines (commons-io,
sttp and Alamofire now read every vector from the cache). The five repos in n have the same flows, the same
concept queries, the same means and the same rule and information rows, digit for digit; n stays 5 and the
verdict line is the same, `CONCEPT: C1=yes (4/5) C2=yes (4/5) -> PASS`.

- HF.1 moved one panel repo's entry set: zod, one `main_guard` declared entry (`[flows] declared entries=1
  new=1 sources=(config=0 main_guard=1 host=0 script=0 other=0)`). Its flow count matches the HF.2 probe of
  v0.5.6 (28), so the declared entry adds no flow of its own. The rest of zod's move from 15 to 28 flows
  (41 -> 55 entries) is later 0.5.6 TypeScript work landed after 86a74bda (GD.38 / GD.39, the TSX grammar
  and JSX components), as the probe read. zod stays out of n (17 concept queries < 30).
- Alamofire's entries went 778 -> 764 (not HF.1: `declared=0`; Swift work landed after 86a74bda) with the
  same 452 flows and the same rows.
- The panel's Java, Kotlin, C and Swift repos (the five in n) hold no declared entry, so HF.1 leaves their
  input unchanged: the re-run confirms the probe's prediction on the whole tree at ec83d3df.

### Notes on the run

- `GLIA_CONCEPT_REPORT=<this file> bash scripts/embedding-gate.sh concept-verdict` on 2026-11-02, from a
  `git archive` of ec83d3df (this file's rule commit, on top of HF.1) in an isolated tree, rank_eval built
  release with `--features sem-eval`, the GPU worker beside it; 10 min 15 s wall for the build and the nine
  repos (22 s to 92 s per repo). Port parity held on every concept query of every repo (mismatch=0).
- Only zod embedded anything (its new node texts, on `webgpu`); every other repo's vectors came from
  $GLIA_RANK_GATE_DIR/sem.

VERDICT: PASS
