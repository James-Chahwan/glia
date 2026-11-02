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
(filled by the run)
<!-- /concept-gate:verdict -->

VERDICT: pending (the HF.2 re-run)
