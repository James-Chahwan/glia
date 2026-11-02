# Edge precision, the 0.5.7 proof run (HP.5)

The release's precision claim, measured against the rule committed before any sample was drawn
(`bench/edge-audit/RULE-0.5.7.md`, commit 558fa992). Numbers come from
`bench/edge-audit/report.json` / `report.md`, derived by `bash scripts/edge-audit.sh report`
(`--panel all`, the default since HP.5) from the committed samples and verdicts; `report --check`
exits 0. Every repo is public.

```
[edge-audit] report strata=39 judged=902 unsure=2 under_floor=9 insufficient=1 kappa=1.000 ci_method=stratified_wilson
[edge-audit] rule 0.5.7 verdict=FAIL under_floor=ACCESSES_DATA|fact,CONTAINS|fact,EVENT_FLOWS|fact,HANDLED_BY|fact,HAS_ATTRIBUTE|fact,INHERITS_FROM|fact,NAVIGATES_TO|derived,NAVIGATES_TO|fact,TESTS|derived fact=0.993 derived=0.878 heuristic=0.990 kappa=1.000
[edge-audit] rule 0.5.7 rejudge independent=44 self=59
```

**Verdict: FAIL by the committed rule.** Every one of the 9 under-floor strata is under floor
because of the held-out panel's verdicts, and those verdicts describe edges the release
candidate mostly no longer writes: 26 of the held-out panel's 28 wrong edges are in no stratum of
the candidate's build (below). The fit panel, drawn at the candidate, has one stratum under
floor (DOCUMENTS|derived, 0.833) and every tier at or above its floor. The rule reads the
held-out panel as HC.3 judged it, so the verdict stands; a re-measurement on a held-out panel
re-drawn at the candidate is a follow-up with its own rule, not a re-run of this one.

## What moved

- **The self row.** `snapshots.json` pins glia at the release candidate
  558fa99229faf16d61d5b25fd5a83017951bfa1a (0.5.7 wave 9 landed, every graph-changing packet
  this run depends on: HF.1, HF.3, HF.4, HF.5, HT.4-HT.8, HP.4, HC.1, the HQ group). Its skips
  gain `engine/src/rules/reflexion_parity.rs`, the parity harness against a private
  repository's report: its function names are that repository's, and one of its edges was
  drawn into READS_CONFIG|fact (the id left; the next key took its place). The other four fit
  rows stay pinned.
- **The panel.** The fit panel is the five 0.5.6 rows. gin-vue-admin and zora-protocol are in
  the held-out panel already (HC.2), and a name is listed once, so they count here through
  `report --panel all` with the held-out panel's other five repos (mealie, chatwoot, squidex,
  tolgee, hoppscotch): 12 repos, post-stratified per panel (README, "The 0.5.7 proof run").
  The held-out sample is not re-drawn: it belongs to the calibration (`calibrate --check` and
  the confidence table are unchanged, `[edge-audit] calibration check: ok`).
- **The population.** `org_packages` left `exclusion()`: `excluded=` now counts only `skip`
  (glia 17674, every other row 0). The release candidate writes no `resolver:org_packages` edge
  on any of the 12 trees (a census, below), so no org-package stratum exists to sample. TRACES
  and GOAL_UNIT stay excluded: the goal layer is 0.5.8's and has no emitter in 0.5.7.
- **The sampler.** `edge_audit` was built from a `git archive` of the candidate plus this
  packet's `edge_audit.rs` (an example, not hashed), in an isolated target dir; every tree was
  read from its existing snapshot copy, nothing fetched.

```
[edge-audit] sample label=glia edges=168997 strata=16 sampled=258 unjudgeable=164 excluded=17674 seed=glia-edge-audit-0.5.4
[edge-audit] sample label=shell edges=2500 strata=8 sampled=15 unjudgeable=0 excluded=0 seed=glia-edge-audit-0.5.4
[edge-audit] sample label=websocket edges=1021 strata=9 sampled=30 unjudgeable=0 excluded=0 seed=glia-edge-audit-0.5.4
[edge-audit] sample label=samples edges=30752 strata=16 sampled=77 unjudgeable=13 excluded=0 seed=glia-edge-audit-0.5.4
[edge-audit] sample label=microservices-demo edges=4883 strata=20 sampled=161 unjudgeable=97 excluded=0 seed=glia-edge-audit-0.5.4
```

  The fit sample: 541 ids (516 before), 432 kept with their row and verdict (153 kept rows
  differ only in `rev`, 6 in the target's line), 84 dropped (their verdicts left with them), 109
  fresh. The held-out panel's seven `sample label=` lines are HC.2's (`samples/heldout.jsonl`,
  363 ids).
- **Judging.** HP.5 judged the 109 fresh ids under `RUBRIC.md` from source: 104 correct, 5
  wrong, 0 unsure (74 structure, 35 mechanism). All 5 wrong are glia DOCUMENTS|derived.
- **The re-judge.** GP.13's matched rule asked for 16 more ids (structure 6, mechanism 10, the 5
  wrong among them). They were re-judged by `HP.5x`: a separate `claude -p` process per snapshot
  (glia, samples, microservices-demo), its working directory the snapshot copy, tools Read / Grep
  / Glob only, given RUBRIC.md and `show`'s excerpts and never a first verdict. 16 / 16 agree;
  `adjudication.jsonl` stays empty.

### Independence

| re-judge | pairs | self pairs |
|---|---:|---:|
| FD.3c | 28 | 0 |
| GP.14r | 3 | 3 |
| HC.3r | 56 | 56 |
| HP.5x | 16 | 0 |

This run adds 16 pairs, 0 of them self pairs. The rule's kappa is over the 44 independent pairs:
44 / 44 agree, expected agreement 0.799, kappa 1.000. The 59 inherited self pairs (HC.3 re-judging
its own held-out ids, GP.14 three of its own) are counted and stay out of it. Families: mechanism
1.000, held-out 1.000 (self pairs), structure undefined (both judges said `correct` only), overall
1.000 over 103 pairs.

## Tiers, 0.5.4 -> 0.5.6 -> 0.5.7

| tier | floor | 0.5.4 (FD.3c) | 0.5.6 (GP.14) | 0.5.7 fit panel | 0.5.7 claim (`--panel all`) |
|---|---:|---|---|---|---|
| fact | 0.95 | 0.998 [0.995, 1.000] n 324 | 1.000 [0.850, 1.000] n 314 | 1.000 [0.846, 1.000] n 313 | 0.993 [0.987, 0.996] n 511 |
| derived | 0.90 | 0.599 [0.497, 0.700] n 128 | 0.894 [0.818, 0.941] n 148 | 0.936 [0.869, 0.971] n 177 | **0.878** [0.698, 0.957] n 277 |
| heuristic | 0.70 | 0.912 [0.826, 0.997] n 51 | 0.913 [0.792, 0.967] n 54 | 1.000 [0.900, 1.000] n 51 | 0.990 [0.950, 0.998] n 114 |

The fit column is `report --panel fit` on the same files (information; the rule reads the last).
The claim's derived tier is under its floor through NAVIGATES_TO|derived (held-out, 2 / 10) and
TESTS|derived (held-out part 10 / 12, N 5053 of the stratum's 5267).

## Strata, 0.5.4 -> 0.5.6 -> 0.5.7

p (correct / judged, N). The 0.5.7 claim post-stratifies the fit and held-out parts.

| stratum | 0.5.4 | 0.5.6 | 0.5.7 fit | 0.5.7 claim | status |
|---|---|---|---|---|---|
| ACCESSES_DATA\|fact | 0.467 (7/15, 15) | 1.000 (5/5, 5) | 1.000 (5/5, 5) | 0.700 (12/15, 3575) | under floor |
| CALLS\|fact | 1.000 (32/32) | 1.000 (31/31) | 1.000 (31/31, 110765) | 1.000 (41/41, 196085) | ok |
| CONTAINS\|fact | 1.000 (10/10) | 1.000 (10/10) | 1.000 (10/10, 469) | 0.932 (22/23, 3891) | under floor |
| DEFINES_CONFIG\|fact | 0.667 (20/30) | 1.000 (30/30) | 1.000 (30/30, 166) | 1.000 (41/41, 884) | ok |
| DEPENDS_ON\|fact | 0.933 (28/30) | 1.000 (30/30) | 1.000 (30/30, 1228) | 1.000 (40/40, 3802) | ok |
| DOCUMENTS\|derived | 0.387 (12/31, 893) | 0.742 (23/31, 418) | **0.833 (25/30, 409)** | 0.942 (35/40, 1173) | ok (fit part under) |
| EVENT_FLOWS\|fact | - | - | - | 0.250 (2/8, 288) | insufficient, under floor |
| GRPC_CALLS\|derived | 1.000 (18/18) | 1.000 (18/18) | 1.000 (18/18, 18) | 1.000 (18/18, 18) | census |
| HANDLED_BY\|derived | 1.000 (17/17) | 1.000 (30/30, 40) | 1.000 (30/30, 51) | 1.000 (40/40, 63) | ok |
| HANDLED_BY\|fact | 0.833 (25/30) | 1.000 (30/30) | 1.000 (30/30, 59) | 0.753 (39/42, 4553) | under floor |
| HAS_ATTRIBUTE\|fact | 1.000 (11/11) | 1.000 (11/11) | 1.000 (11/11, 2374) | 0.947 (22/23, 6428) | under floor |
| HTTP_CALLS\|derived | - | 1.000 (7/7, 7) | 1.000 (7/7, 7) | 0.910 (17/18, 942) | ok |
| IMPLEMENTS\|derived | 0.800 (24/30) | 1.000 (30/30) | 1.000 (30/30, 321) | 1.000 (40/40, 584) | ok |
| IMPLEMENTS\|fact | 0.933 (28/30) | 1.000 (30/30) | 1.000 (30/30, 561) | 1.000 (41/41, 3706) | ok |
| IMPORTS\|fact | 0.968 (30/31) | 1.000 (31/31) | 1.000 (31/31, 6866) | 1.000 (41/41, 23450) | ok |
| IMPORTS\|heuristic | 0.900 (27/30) | 0.900 (27/30) | 1.000 (30/30, 151) | 1.000 (40/40, 14769) | ok |
| INHERITS_FROM\|fact | 1.000 (30/30) | 1.000 (30/30) | 1.000 (30/30, 219) | 0.918 (40/41, 2202) | under floor |
| NAVIGATES_TO\|derived | - | - | - | 0.200 (2/10, 40) | under floor |
| NAVIGATES_TO\|fact | - | - | - | 0.900 (9/10, 24) | under floor |
| READS_CONFIG\|fact | 0.767 (23/30) | 1.000 (30/30) | 1.000 (30/30, 144) | 1.000 (40/40, 777) | ok |
| RPC_CALLS\|derived / heuristic | - | - | - | 1.000 (8/8) / 1.000 (10/10) | ok |
| TESTS\|derived | 0.933 (28/30) | 1.000 (30/30) | 1.000 (30/30, 214) | 0.840 (40/42, 5267) | under floor |
| USES\|derived | - | - | 1.000 (30/30, 52) | 1.000 (31/31, 53) | ok |
| USES\|fact | 1.000 (31/31) | 1.000 (31/31) | 1.000 (31/31, 11461) | 1.000 (41/41, 27281) | ok |

The other 14 strata of the claim (SAME_AS, INJECTS, RETURNS_TYPE, SCHEDULES, the heuristic HANDLED_BY /
IMPLEMENTS / INHERITS_FROM / INJECTS / EVENT_FLOWS, CALLS|derived, EVENT_FLOWS|derived, TESTS|fact,
WS_CONNECTS, DEFINES) are at or above floor, every judged id correct but INHERITS_FROM|heuristic's
one wrong (17 / 18, 0.900, held-out); report.md has every row.

## Why the claim fails: the held-out panel predates the candidate

The held-out panel was drawn and judged at 221f9c84 (HC.2 / HC.3, before 0.5.7's precision
packets). A census of every stratum of all 12 trees at the candidate (`n_per_stratum` set past
every stratum, a scratch file, never committed):

- 45 of the 363 held-out ids are no longer drawn by any key at the candidate: all 28 first-verdict
  `wrong` ids, 15 `correct`, 2 `unsure`.
- By (label, category, from qname, to qname), at any line, rule or tier: 26 of the 28 wrong edges
  are in no stratum of the candidate's build. The 0.5.7 precision packets name these mechanisms:
  HQ.1 (a heritage base is never a namespace), HQ.3 (an emit binds the event its base declares,
  never an id no node has), HQ.5 / HQ.6 (Angular lazy child prefixes, a literal `/404`, computed
  route paths), HQ.8 (mocked store clients in tests are no data access: the three ids HQ.8's
  handoff named are gone), HQ.10 (TESTS `name_match`), HQ.13 (C# usings).
- The other 2 are zora-protocol HANDLED_BY|fact (parser:solidity): a procedure qname two
  same-named contracts share (`setUp` of two test files, an interface's `transferFrom` in two
  packages). HQ.4 now writes each copy's HANDLED_BY at that copy's own file and line, so the
  (procedure, copy) pair is still written, re-sited at the copy that declares it, which is
  HQ.4's intended edge rather than the cross-file binding HC.3 judged wrong (not re-judged here:
  the held-out sample is not re-drawn).
- All 11 of 0.5.6's committed wrong edges (8 DOCUMENTS, 3 IMPORTS `tail_unique`) are gone too.

So each under-floor stratum's held-out part is an older build's precision. Information only (not
the rule): dropping the 45 held-out ids the candidate no longer draws leaves no stratum under floor
(`[edge-audit] report strata=39 judged=859 unsure=0 under_floor=0 insufficient=4`, derived 0.992,
fact 1.000, heuristic 1.000), but that selection keeps only surviving edges and the held-out N are
still 221f9c84's, so it is no estimate.

## DOCUMENTS|derived after HP.4

HP.4 refused the three 0.5.6 wrong-mention shapes; its 8 wrong edges are gone. On the fit panel
the stratum reads 0.833 (25 / 30, [0.664, 0.927]), up from 0.742, and still under its 0.90 floor.
By doclink rule: `qualified` 8 / 8, `unique` 12 / 13, `ambiguous` 5 / 9. Four of the five wrong
rows bind through `ambiguous`, the DE.1 least-qname pick among several same-named candidates.

## The widened panel's strata

- **gin-vue-admin** (held-out, 46 judged ids, all correct): HTTP_CALLS|derived 2 / 2 of its 190,
  ACCESSES_DATA|fact 1 / 1 of 221 (gorm), READS_CONFIG / DEFINES_CONFIG 1 / 1 each, its docker
  start (HANDLED_BY|derived, resolver:host) 1 / 1. The pooled HTTP_CALLS|derived stratum is 17 / 18
  over 942 edges (the one wrong is squidex's, gone at the candidate).
- **zora-protocol** (held-out, 60 judged, stratified 0.947 [0.874, 0.978]): RPC_CALLS via
  resolver:evm 8 / 8 derived and 10 / 10 heuristic, EVENT_FLOWS via resolver:evm 3 / 3 heuristic;
  EVENT_FLOWS|fact (parser:solidity) 2 / 8 with 2 unsure, 8 of its 10 ids gone at the candidate;
  HANDLED_BY|fact 4 / 6 (the two above); its `manifest_bin` and docker starts 4 / 4.
- **org_packages**: no edge on any of the 12 trees at the candidate (glia's header counted 5
  excluded at ad5dd83d; zora-protocol's held-out header counted none).
- **The host graph** (resolver:host, 0.5.6): fit HANDLED_BY|derived `manifest_bin` 14 / 14 and
  docker 6 / 6 (microservices-demo's ENTRYPOINTs through WORKDIR / COPY), USES|derived
  `include_str` 30 / 30 (glia's tests embedding fixture files); held-out 11 / 11. 61 / 61.

## Wrong clusters (fit, 5 of 109 fresh, all glia DOCUMENTS|derived)

Each is a precision-backlog follow-up with a public fixture idea. D4 / D5 continue 0.5.6's names.

- **D4 again. A JSON field in a table's key column (1, rule `unique`).** docs/overlay.md's
  vectors.jsonl field table row `` | `text_hash` | no | .. `` binds the engine function
  `semantic::text::text_hash`. HP.4 refuses key positions in prose; the first cell of a markdown
  table row under a "field" header is one too. Fixture: a doc table `| field | .. |` naming a
  field that is also a function name, no DOCUMENTS edge.
- **D5 again. A pyo3 method name binds a crate-private Rust fn (1, `ambiguous`).** docs/overlay.md
  "pyo3 `set_cell` / `write_cell`" binds `store::container::set_cell` (pub(crate)). Fixture: a doc
  naming `set_cell` beside "pyo3" with a PyGraph method and a pub(crate) fn of that name: the
  method, or nothing.
- **D7. A leading-dot member in a code span binds a type (1, `ambiguous`).** docs/history.md
  "groups chained through `.Use`" (Go's `RouterGroup.Use`) binds the private struct
  `contract_breaks::names::Use`. Fixture: `` `.Use` `` with an in-repo struct `Use`: a token
  starting with `.` is a member access, no edge to a type.
- **D8. An example atom of another language binds a method (1, `ambiguous`).** docs/rules.md's
  Datalog example `done()` binds `IdDefs::done`. Fixture: a doc section about a query language
  whose inline example names `done()`, the only `done` a method: no edge from a bare unqualified
  method name picked by tie-break.
- **D9. A module name binds a same-named method (1, `ambiguous`).** CLAUDE.md's `ppr_index` (the
  activation module `activation/src/ppr_index.rs`) binds `RankCache::ppr_index`. Fixture: a doc
  naming `ppr_index` where a MODULE `ppr_index` and a METHOD `T::ppr_index` exist: the MODULE.

D5, D7, D8 and D9 share the mechanism: `ambiguous` takes the least-qname candidate even when the
candidates are of different kinds and none is the mention's subject (5 / 9 correct on this sample).

The held-out clusters (NAVIGATES_TO `/404` to a param page, Solidity unresolved event targets,
same-named Solidity handlers, mocked data clients, C# namespace bases, name-matched TESTS) are
HC.3's (`dev-notes/confidence-gate-0.5.7.md`); 26 of their 28 edges are gone at the candidate and
the other 2 are re-sited by HQ.4, so no held-out cluster is left open by this census. A held-out
panel re-drawn at the candidate is what would measure them.

## Limits

- The claim mixes two builds: the fit panel at the candidate, the held-out panel at 221f9c84.
  The rule reads both as committed; the census above is the measure of the gap.
- gin-vue-admin and zora-protocol enter at the held-out n (10 per pooled stratum over seven
  repos), so HTTP_CALLS (18 judged), ACCESSES_DATA (15) and the EVM strata are still thin.
- No org_packages edge exists on the panel, so their precision is still unmeasured: a panel row
  with an npm / cargo / maven org that routes through a sibling's manifest name is needed.
- The first judge (HP.5) is one agent; the independent re-judge covers the 16 ids the matched rule
  named, one fresh process per snapshot of the same model family.
