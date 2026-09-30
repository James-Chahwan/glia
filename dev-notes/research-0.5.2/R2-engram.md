# R2 — Engram PPR memory (0.5.2 bet C)

Research only. The Engram repo was read, never written: another session is editing it now. Its HEAD is `b59f858`
("feat: glia v6 read side - Documents, line anchors, GmapDiff apply", 2026-09-30 19:16), with uncommitted edits in
`crates/engram-live/src/lib.rs` and `crates/engram-mcp/src/main.rs`. glia is at `2170ff8`.

## 1. Engram's recall today

`SchemaStore::read` (crates/engram/src/store.rs:1706) works in five steps.

1. **Route** to ONE entry cell: the cell holding the fact that best overlaps the cue, with a marginal-entry floor
   (candidates at store.rs:1770, entry chosen at :1828-1884, floor at :1903-1935).
2. **Hop 0.** Seed only the entry cell's facts, each with `overlap(cue, fact) × salience` (store.rs:1941-1952).
3. **Hops 1..=SPREAD_HOPS (= 2)** (const at store.rs:154, doc at :146-153; loop at :1971-1987). Each wavefront adds
   `act × decay_for(kind) × weight` to every neighbour. `decay_for` (store.rs:183) sets per-kind conductance:
   Contains / Implements / Extends 0.7, Calls / Causes 0.5, Imports / DependsOn 0.3, Returns / Documents 0.4,
   Cooccurs 0.5, the memory relations 0.6. Adjacency is undirected (both directions pushed) and sorted, so it is
   deterministic (store.rs:992-1015). It is rebuilt and sorted on every read. Spread is NOT normalised by degree.
4. **Hub penalty** at rank time: `(mean_degree / max(degree, mean_degree))²` (store.rs:~2001-2056, the square at
   :2051). The code comment says why: "spread amplifies a hub's activation 5-10× while penalty cuts it 0.2-0.5×".
5. Rank by activation, id tie-break, truncate to k.

The code calls this "HippoRAG-style spreading activation" (store.rs:152). HippoRAG itself is PPR, not bounded
spreading.

## 2. The boundary rule

- Engram CLAUDE.md:24 and :30: `engram-core` is "the *only* surface glia and engram share — `.gmap` format in,
  `Content::Symbol` facts out, nothing else crosses".
- crates/engram-core/Cargo.toml repeats it: "This crate is the ONLY thing glia and engram share".
- Engram depends on no glia crate. glia's `engram-export` is the one bridge. It is an excluded crate that
  path-depends on `../Engram` engram-core (scripts/check-engram-export.sh).
- Consequence: Engram cannot call glia's `activate`. Its PPR is a copy (~120 LOC), or glia supplies inputs through
  the gmap or through side files.
- Engram CLAUDE.md:61: "Routing must be deterministic ... If routing or rank order drifts run-to-run, organ
  ablations are not clean". That holds for any PPR copy too.

## 3. What glia provides today

- **Domain-free PPR** in `glia-activation`:
  - `activate` (activation/src/lib.rs:145) → `ppr_vector` (:166-282): power iteration with dangling mass returned
    to the personalisation vector.
  - `damping` 0.5 is the propagate probability, so restart is 1-d = 0.5 (:74-101, default :94).
  - Direction Forward / Backward / Undirected (:51-60, applied :196-215, Undirected at :208). Per-category
    weights (:80).
  - Seeds are UNIFORM: `1/seed_count` (:220-233, `seed_weight` at :228). There is no weighted personalisation.
  - Degree-based specificity is applied after PPR (activation/src/plan.rs:154-197).
  - `ActivationPlan` hooks sit around it (plan.rs).
  - The `ppr_vector` doc warns that the plan oracle compares f64 bits (lib.rs:161-165), so the arithmetic order is
    frozen.
- **The v6 gmap contract** (engram-core lib.rs:43 `GMAP_FORMAT_VERSION = 6`). Typed edges carry an optional
  `weight` (GmapEdge, lib.rs:439-455). glia's `CATEGORY_MAP` (engram-export/src/lib.rs:304) maps every glia
  category to one EdgeKind and weight. There are move-stable `identity_hint`s and an incremental `GmapDiff`
  (glia-v6-landed.md §1).
- **v6 store sizes** as seeded (glia-v6-landed.md:630-631): quokka-stack 2,605 facts / 25,328 edges, Kina 3,278 /
  33,225, glia 11,898 / 121,106.
- **Not in v6** (glia-v6-landed.md §6, "Pending / not in v6"):
  - import classification;
  - confidence-scaled Documents weights (every doc-linker edge is 0.3);
  - per-tag NatSpec anchors;
  - Solidity constructor NatSpec;
  - move + rename + edit pairing;
  - identity across a full export without `--since`;
  - file MODULEs sharing a public type's qname;
  - every cell beyond CODE / DOC / POSITION / ORIGIN / IMPORTS / DOC_TAGS. TEST, FAIL, ENTRYPOINT, ROLE,
    EVIDENCE, COVERAGE and ACCESS_MODE are not exported, and "mapping any of them is a later contract ask".

  None of these blocks PPR. ENTRYPOINT (24) and TEST (7) would be useful seed priors later (v7).

## 4. HippoRAG and HippoRAG 2

- **HippoRAG** (NeurIPS 2024, arXiv 2405.14831):
  - Query entities are linked to KG nodes by embedding similarity and seed PPR.
  - Each seed is scaled by node specificity `s_i = |P_i|^-1`, where P_i is the passages the node came from.
  - Restart 0.5. Passage score = PPR vector × node-passage matrix.
  - Ablation, MuSiQue R@2/R@5: PPR 40.9/51.9; "query nodes & neighbors" (a one-hop spread) 25.4/38.5; "query
    nodes only" 37.1/41.0. **Neighbour expansion did worse than no expansion; PPR did best.**
  - glia's 2026-04-17 assessment (mempalace `reference_hipporag_assessment.md`) notes the failure mode: 24% of
    MuSiQue errors were diffuse PPR activation.
- **HippoRAG 2** (ICML 2025, arXiv 2502.14802):
  - Phrase nodes plus **passage nodes**, joined by `contains` context edges, plus synonym edges at sim > 0.8.
  - Query-to-triple linking; an LLM "recognition memory" filter over the top-5 triples; at most 5 phrase seeds,
    reset = their mean triple score.
  - **All passage nodes are seeded** with dense-similarity reset × a weight factor of 0.05. Damping 0.5.
  - Ablation, R@5: without passage nodes, MuSiQue 74.7 → 63.7 and HotpotQA 96.3 → 88.9. Query-to-triple beats
    NER-to-node by +12.5 on average.

Mapping onto Engram:

| HippoRAG 2 | Engram |
|---|---|
| phrase nodes | facts (`Content::Symbol` / Proposition) |
| passage nodes | concept cells (each groups facts; `contains` = cell membership) |
| relation edges | typed fact-edges, conductance = `decay_for × weight` |
| phrase seeds weighted by match score | top-N facts across ALL cells by `overlap × salience` |
| passage seeds | every cell by best-fact overlap × a small factor |

Engram today seeds one cell and spreads 2 hops unnormalised. PPR normalises by out-weight, which is the principled
form of the hub penalty in store.rs:~2001-2056.

## 5. Design options

- **C1. Engram-internal PPR, glia unchanged.** Engram copies ~120 LOC of `ppr_vector` and adds weighted seeds.
  Nothing crosses. The copy can drift from glia's arithmetic silently.
- **C2. C1 plus glia-side support that crosses only as files: RECOMMENDED.**
  - glia adds weighted personalisation to `activation`, a golden-vector generator and a non-circular eval-set
    exporter, both in `engram-export`.
  - Engram's copy asserts it matches glia's scores on the three stores; the eval sets decide the default.
  - No contract change.
- **C3. glia precomputes per-node signals into the gmap** (a specificity analogue: how many files reference a
  symbol, entrypoint flag, global PageRank). A v7 contract bump with a coordinated re-seed. Useful only once C2
  shows PPR needs a better seed prior.
- **C4. glia exports top-k PPR neighbourhoods as weighted Cooccurs edges.** Rejected: static, large (k edges per
  node), and blind to Engram's Path-B experience edges, which glia never sees.
- **C5. Engram depends on `glia-activation` directly.** Rejected under the current rule (Engram CLAUDE.md:30) and
  the cross-repo path-dep rule. Only James can change the boundary.

## 6. Recommendation

**C2.**

- The bet is decided by measurement on Engram's own data, and C2 is the cheapest way to make that measurement
  honest: a golden check that the copy is exact, plus GT that is not the spreading design's own shape.
- It keeps the boundary and needs no contract bump.
- It is the part glia owns. The read-path change is Engram's session's work (four repos, four sessions: glia acts
  on glia only).

## 7. Evaluation plan (on Engram's own recall tests)

**Stores.** The three v6 exports: quokka-stack (2,800 nodes / 4,977 edges), Kina (3,482 / 7,770) and glia
(18,788 / 56,092) (glia-v6-landed.md:28-31), seeded through `engram-mcp`.

**Arms.** Two things change at once, so the arms separate them:

| arm | what |
|---|---|
| 0 | control: `SPREAD_HOPS = 0` (lexical only) |
| 1 | today: 2-hop spread, entry-cell seeding, hub penalty |
| 2 | PPR with entry-cell seeding (isolates the propagation change) |
| 3 | PPR with global top-N seeding (isolates the seeding change) |
| 4 | PPR + cells as passage nodes (HippoRAG 2 shape) |

Damping grid {0.3, 0.5, 0.7} on arms 2-4, rendered as one grid for James to pick from.

**Ground truth.** Non-circular sets decide; the existing tests gate:

- **G1, feature flows.** glia LG.3a `feature_flows` (engine/src/feature_flows.rs): query = feature key (`auth`,
  `orders`), GT = the feature's step nodes (carry-edge BFS from entries across services), in gmap key space, after
  Engram's drop set.
- **G2, held-out co-change.** From `.glia/history-snapshot` commits (LF.5, the git-history snapshot): seed = one
  symbol changed in a held-out commit, GT = the other symbols it changed. The store is built WITHOUT CO_CHANGES
  edges (CATEGORY_MAP sends them to Cooccurs 0.3), or G2 leaks.
- **G3, tests (sanity).** Query = a test function, GT = its FACT / DERIVED `tests_for` rows.
- **Existing, reported but not deciding:** `examples/shootout.rs`. Its GT is the 2-hop neighbourhood by
  construction (shootout.rs:10-19, `GT_HOPS = 2` at :39, :244-253), so it rewards `SPREAD_HOPS = 2`.
- **Existing gates, must stay green:** `tests/scenarios.rs` (the 2-hop cron-job assembly at :79, :132-136),
  `tests/litmus.rs`, `tests/ablations.rs`.
- **quokka_recall** (engram-live/examples/quokka_recall.rs:78, 9 queries, PASS / FAIL / N/A): needs VOYAGE_API_KEY
  and costs pennies.
- **The lex + hv batteries.** Their scripts are gone from /tmp (glia-v6-landed.md:632-633); re-create them as a
  committed script first.

**Metrics.** recall@8, recall@25, MRR, NOISE-HIT rate on absent cues (quokka billing / calendar), p50 / p95 read
latency, determinism (3 runs → byte-identical prime text). Output is JSONL per (store, arm, query) plus an ASCII
table, appended to a committed results file with the run history.

**Proposed decision rule (James rules):** PPR becomes the default if an arm beats arm 1 on recall@8 for G1 and G2 on
≥2 of the 3 stores, with no drop in quokka_recall PASS and no scenario / litmus regression. Otherwise it ships behind
`recall_mode = ppr`.

**Latency check.** glia's store has 121,106 edges, so power iteration is ~2E × ≤50 iterations ≈ 12M multiply-adds per
read: milliseconds in release. The per-read `adjacency()` rebuild-and-sort (store.rs:992-1015) costs more than PPR;
a CSR cached until the next write helps both arms.

## 8. Engram contract version impact

- **C2: none.** GMAP stays 6. Eval sets and golden vectors are side files, not gmap fields.
- If bet B (cross-repo identity) adds SAME_AS, glia needs one `CATEGORY_MAP` row. Mapping to an existing EdgeKind
  needs no bump; a new EdgeKind is a v7 ask.
- **C3** (per-node signals such as specificity, ENTRYPOINT, TEST) would be v7, a coordinated bump under Engram's
  single-version rule, with a full re-seed.
- 0.5.1's CD-7 (CODE cells as spans) changes how engram-export reads CODE text, not the contract, unless the contract
  is also moved to spans.

## 9. Components (LOC, crate)

glia side (this bet's packets):

- **Weighted personalisation** in activation (`activate_weighted(nodes, edges, seeds: &[(NodeId, f64)], cfg)`),
  normalised seed mass. The uniform path must stay bit-identical to `ppr_vector` (lib.rs:161-165), with tests.
  glia can use it too (`resolve` weighting stack frames by depth). 80-150 LOC, `glia-activation`.
- **Golden-vector generator** `engram-export/examples/ppr_golden.rs`: load an `.engram-gmap`, map EdgeKind → weight
  exactly as Engram's `decay_for × weight`, undirected, run glia's weighted PPR for sampled seed sets, and write
  seeds plus top-k scores as JSONL. Engram's copy asserts against it. 150-250 LOC, `engram-export` (excluded
  workspace).
- **Eval-set exporter** `engram-export/examples/recall_sets.rs` (G1 / G2 / G3 in gmap key space, drop-set aware,
  CO_CHANGES-free variant). 250-450 LOC, `engram-export`.
- **Handoff doc** to the Engram session (arms, protocol, decision rule, golden check), in LG.14's shape. Doc only.

glia total: **480-850 LOC** (§4 said 400-900).

Engram side, for reference; Engram's session builds it (1,030-1,900; §4 said 1.2-2k):

- A `RecallMode` flag plus a PPR read path over a sorted CSR in f64. 150-250 LOC.
- Global top-N seeding plus cells as passage nodes. 150-300 LOC.
- MCP / config flags. 80-150 LOC.
- A recall eval harness over the eval sets. 350-600 LOC.
- The committed battery script. 100-200 LOC.
- Determinism and golden conformance tests. 200-400 LOC.

## 10. 0.5.1 packets it needs first

- **Gate: none from 0.5.1.** The gate is Engram applying v6, which is landing now (b59f858 plus uncommitted edits).
  Packets wait for that session to finish.
- **CD-7 FORMAT_VERSION 3.** engram-export reads CODE cells; CODE-as-spans changes the exporter, and the eval-set and
  golden tools sit on the same exporter.
- **CC-7 hotspots.** Its "global PageRank via `activate`, one adjacency index" helper is reusable for a later C3
  specificity signal.
- **LF.5 history snapshot** (already landed): G2's input. **LG.3a `feature_flows`** (landed): G1's input.
- **0.5.2 bet B (SAME_AS)** if it lands in the same release: one `CATEGORY_MAP` row.

## 11. Open questions only James can answer

1. Stay with "Engram copies PPR, glia supplies harness and eval sets" (C2), or loosen the boundary so Engram depends
   on `glia-activation` (C5)?
2. The decision rule: what gain, on which GT, makes PPR the default rather than a flag?
3. Seeding semantics: may recall seed across all cells (HippoRAG 2), or must single entry-cell routing stay (arm 2
   only)? It changes what "routing" means in Engram's organ model.
4. May test-only side files (eval sets, golden vectors) cross the boundary, or must Engram derive its GT from the
   gmap alone?
5. Spend the Voyage (hv) pennies in the eval, or run lex only?

## 12. Spikes worth running before packets

- **S1.** On a scratch COPY of Engram (never the live tree): swap the spread loop for PPR (~120 LOC) and run
  shootout, scenarios and a hand-made G1 on the three v6 exports. Get recall@25 and latency.
- **S2.** Measure shootout's circularity: recall@25 of the oracle 2-hop BFS ranking itself vs PPR on shootout's GT.
  If the oracle scores ~1.0, shootout cannot judge the bet.
- **S3.** Golden conformance: glia f64 `ppr_vector` vs an Engram-side copy on the three stores. Aim bit-exact with
  the same operation order and sorted CSR; otherwise fix a tolerance.
- **S4.** G2 leakage check: recall with and without CO_CHANGES edges in the store.

## 13. Risks

- **The wrong yardstick.** shootout's 2-hop GT favours today's design by construction. A PPR "loss" there is not
  evidence.
- **Confounding.** Seeding scope and propagation change together; without arms 2 / 3 no result is attributable.
- **Lost batteries.** The lex / hv battery scripts are gone (glia-v6-landed.md:632-633). Until they are re-created
  and committed, the v4 / v5 battery numbers cannot be reproduced.
- **Hub behaviour flips.** Degree normalisation can under-rank legitimate hubs (`AuthService`) that the squared
  penalty keeps. HippoRAG's measured failure is diffuse activation.
- **Determinism.** Engram accumulates f32 in HashMaps (store.rs:1941, :1972). A PPR copy must use sorted CSR and a
  fixed operation order or break Engram CLAUDE.md:61.
- **Concurrency with Engram's session.** Any read-path change collides with in-flight edits. glia ships only
  harness, eval sets and handoff; the Engram session makes the change.

Sources: https://arxiv.org/abs/2405.14831 · https://arxiv.org/html/2405.14831v1 · https://arxiv.org/abs/2502.14802 ·
https://arxiv.org/html/2502.14802 · https://proceedings.mlr.press/v267/gutierrez25a.html ·
https://github.com/osu-nlp-group/hipporag · Andersen, Chung & Lang, "Local Graph Partitioning using PageRank
Vectors", FOCS 2006, https://www.math.ucsd.edu/~fan/wp/localpartition.pdf (push-based approximate PPR, if
per-read cost ever matters).

## Addendum 2026-10-01 — measurements from the Engram session (engram-6c)

Engram's 480-query recall battery on quokka-stack and Kina, dev split, paired, glia 0.5.0 exports:
- Lookups: a flat dense-cosine store over each fact's FULL text (bge-small) beats Engram's current read 78 vs 54 of
  100 (26–2, p<0.0001). The whole gap is paraphrased queries (named lookups: 41 vs 40 of 44). Cause: Engram embeds
  name + qname only; flat store full text vs short text is +22 / −1. glia's leading_doc capture is the load-bearing
  signal for paraphrase retrieval — keep doc quality high.
- Relational: Engram's printed outgoing edges answer "what does X call" 32/35 and "which endpoint does X request"
  19/19; the flat store 2/35 and 8/19.
- "Who calls X": Engram 7/35, flat store 5/35. Engram does not surface incoming edges yet (next on its list). PPR over
  incoming edges is where the graph should help most; glia gaps cap caller recall directly: Go calls inside returned
  closures (0.5.1 CA.1), Go receivers typed from a call's return (CA.2a/b), TS arrow-function class fields (new).
- False Go IMPLEMENTS (name-only matching) pollute any PPR over the graph; Engram skips Go implements for now (CA.3b).
- The glia-store hybrid seed that failed twice was a Voyage connect timeout in Engram's own adapter — not glia.
