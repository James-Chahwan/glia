# glia 0.5.5 — proof, plus the tracks moved out of 0.5.4 (scope seed)

**Born 2026-10-30.** James cut 0.5.4 to what was built plus two small tracks (*"Nah we can change to option 3 to round of
0.5.4 now right ?"*, superseding the 2026-10-24 ruling that 0.5.4 ships only after every track). 0.5.4 ships T0 T1 T2 T3
T5 T10 (+FD.31) T11 TS TH and FH.11c. Everything below moves here.

## 1. Tracks moved from 0.5.4 (already specced)

The packets are in `dev-notes/leap-054-packets.json` with their corrections in `leap-054-corrections.json`; the track
lists are in `dev-notes/wave-runner/tracks_054.py`. They were specced against 2026-10-23's tree, so every one needs the
re-verify pass (`dev-notes/reverify_054.md`'s method) against the shipped 0.5.4 before a wave, and a new id prefix and
`--release 055` schedule.

| track | what | packets | LOC at spec |
|---|---|---|---|
| T4 | per-language precision and recall residuals (0.5.4 plan §D, §E, F.1) | 53 | ~15,870 |
| T6 | goal-layer connector (§G) and the clarification queue (§C) | 21 | ~8,000 |
| T7 | concept search over flows (F.6), PR / issue text as intent (F.15), where new code goes (F.14) | 7 | ~2,670 |
| T8 | Stack Graphs-style incremental resolution, watch's body-only fast path, multi-repo watch, the watch memory budget (F.3) | 19 | ~7,080 |
| T9 | build-target graphs (bazel), the first non-code domain | 6 | ~2,180 |

The moved tracks' empty wave-0 slots ship in 0.5.4 untouched, reserved here (James 2026-10-30: kept rather than removed,
since 0.5.5 starts straight away): build-domain/src/lib.rs, code-domain/src/goal.rs, engine/src/clarify/{answer,questions}.rs,
engine/src/goal/{check,drift,extract,impact,join,resolve,why}.rs, engine/src/targets.rs, parsers/build/bazel/src/lib.rs,
py/src/{clarify,goal,targets}.rs. The registry ids 0.5.4's F0.1 allocated for T6 (GOAL_UNIT 50, TRACES 38, UNIT 26) stay
reserved too. 0.5.5's packets fill these slots; no new wave-0 is needed for them.

## 2. From the Sibyl cohort follow-up (2026-10-26, option 1: "C + D go to 0.5.5")

- **C: per-repo calibrated inference as a repair loop**: detect -> hypothesise a mechanism from evidence (the overlay
  loop, an LLM fallback) -> verify by numbers (pairing rises, the drift replay) -> absorb (a per-repo record; promoted to
  an engine rule once it holds on several cohort repos). Record: `dev-notes/agent-ab-probe-0.5.4.md` §5.
- **D: calibrated confidence per row**, measured on the cohort ("crime coefficient").

## 3. Proof (the original 0.5.5 row)

Precision rate on real repos (T3's audit in 0.5.4 gives the baseline), agent before / after (0.5.4's harness and FH.13's
probes), big-repo per-phase timings, calibrated confidence, `decide/` a-e, `glia judge`. Candidate after it: a RunPod
rerun of the SWE-bench Lite / latent-injection arms (James 2026-10-23).

## 4. Raised in conversation, not yet specced

- A Claude Code plugin packaging skills/, shaped by FH.13's agent-usage data (James 2026-10-24; ruled after 0.5.4).
- Native GitHub-release binaries / multi-platform install beyond the PyPI `glia-cli` wheels (2026-10-24; undecided).
- An image-embedding space for visual domains (2026-10-24; arrives with the first visual domain; VectorRow already fits).
- The DPG explainer's future directions (LLM uplift of new graph features, biology-inspired ideas; 2026-10-24): only in
  a chat paste so far; commit it as docs/dpg.md.
- Close-out speed: per-step timestamps, then a faster rank gate (about 14 of a close-out's ~20 minutes).
