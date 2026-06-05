# Comprehensive uplift plan — post-cycle-0.5

Written 2026-05-21 ~16:30 after cycle 0.5 A's negative result + 0.5 D in flight. James asked for "mostly uplifts" — concrete buildable features that broadly improve the loop, not narrow experiments. Prioritised by uplift-per-dev-hour, batched into work bundles.

## What we know so far (as of cycle 0.5 A)

- iter5 graph-derived directive PASSes marshmallow gold (1/7 in cycle 0.3 — but only marshmallow has AttributeError + traceback)
- Bare rule-emphasis prose does NOT work (cycle 0.5 A FAILed identically to baseline)
- Cycle 0.4 autoregressive lens proves divergence happens at generated-token position 24, layer 25-27 is the decision band
- 3/7 cycle 0.3 instances have NO traceback at all — directive falls back to inert
- 1/7 has wrong-fix even after correct targeting (matplotlib-22711 IndexError); the bug's root cause isn't at the traceback line
- 1/7 NO-DIFF (matplotlib-22835 raise-redirect over-correction)
- 1/7 NO-RUN (sklearn-10508 F2P didn't execute — pytest spec issue)

## Bundle A — unblock measurement (~1-2h, must-do before any cycle 0.5+ matters)

These aren't experiments; they're fixing instruments that produce false signals.

| # | Task | File | Effort |
|---|---|---|---|
| A1 | Fix sklearn-10508 NO-RUN — eval_specs.py's pytest command for sklearn doesn't actually execute the F2P. Debug + patch. | `bench/latent/out/eval_specs.py` | 30-60 min |
| A2 | Diagnose matplotlib-22835 NO-DIFF — read its out.txt, see WHY model emitted nothing. Likely raise-redirect directive over-corrected; if so, soften the directive (don't say "do not modify the raise" — say "fix in this caller instead"). | `projection-text/src/bin/synth_traceback_target.rs` | 30 min |
| A3 | Lens generate-mode with --injection — currently lens skips the JSON pool during generation. Real-pipeline mirror requires the pool. Adapt `forward_generate` to splice pool embeds before generation kicks off. | `bench/lens/src/runtime.rs` | 60-90 min |

Without A1+A2, cycle 0.5/0.6 results have 2 unreliable cells out of 7 — can't tell if a change helped sklearn/mat-22835 or just didn't break them.

## Bundle B — graph-side directive extensions (~2-4h, biggest uplift on cycle 0.3 FAILed instances)

The directive's named-target structure works on traceback instances. To help the OTHER 3/7 + wrong-fix:

| # | Task | File | Effort | Helps |
|---|---|---|---|---|
| B1 | **test_patch.patch → directive** — the F2P test bodies EXPRESS what the fix should do. Parse the test_patch, extract test functions, identify the symbols they assert on, derive a "the test expects `<symbol>` to do `<behaviour>`" directive line. New Rust bin `synth_test_expectation.rs` in projection-text/src/bin/. | new | 2-3h | no-traceback instances (django, pytest, sphinx); maybe wrong-fix (mat-22711) |
| B2 | **API-surface mention extraction (for no-traceback issues)** — when issue has prose like "calling `foo.bar(x=1)` raises", extract `foo.bar` mention + match against graph qnames. Same shape as traceback-driven directive but driven by prose mentions. | `synth_traceback_target.rs` extension | 1-2h | no-traceback instances where issue has explicit symbol names |
| B3 | **Walk-to-condition for raise-line targets** — current raise-redirect walks to caller. Better: walk to the BRANCH POINT that should prevent the raise. Requires control-flow analysis on the graph. Hard; defer to a later bundle. | new | 4-6h | matplotlib-22835 + similar |
| B4 | **Multi-channel directive composition** — currently synth_traceback_target is monolithic. Compose multiple synth bins (traceback + test-expectation + api-mention + ...) into a single directive that picks the strongest signal per instance. | new orchestrator binary | 2-3h | all instances; force-multiplier |

## Bundle C — latent mechanism upgrade (~6-10h, surgical lever the lens identified)

The cycle 0.4 lens identified the decision band at layer 25-27, generated-token position 23-25. Real latent injection would be a far more surgical lever than text channels.

| # | Task | File | Effort |
|---|---|---|---|
| C1 | **Per-token pool embed (replace mean-pool)** — emit each pool entry's full token sequence as embeds, not mean-pooled. Pool entries become multi-token segments the model can attend to. Replaces `run_llama_pathB.py`'s mean-pool step. | `bench/lens/` extension OR new bin | 4-6h |
| C2 | **Marker tokens around pool entries** — surround each pool entry's tokens with `[POOL_START_<id>]` ... `[POOL_END_<id>]` markers so the model can attend to bounded segments. | piggy-backs on C1 | 1-2h |
| C3 | **Direct attention biasing at L25-27** — modify K/V at the decision-band layers to attend more heavily to specific suffix tokens. Requires extending the llama.cpp cb_eval hook with WRITE access (not just read). | requires C++ patch — out of overnight scope | 1-2 days |

Bundle C is the "real glia" lever the project keeps gesturing at. C1 alone is ~6h Rust — a full focused day. Worth it once the cheaper bundles plateau.

## Bundle D — diagnostic instrumentation (~3-5h, multiplier for all other bundles)

| # | Task | File | Effort |
|---|---|---|---|
| D1 | **lens-batch generate-mode** — currently lens-batch only does prompt-position. Extend to generate-mode for cross-instance autoregressive lens runs. Tells us at which generated token EACH instance commits to its trajectory. | `bench/lens/src/bin/lens-batch.rs` | 1-2h |
| D2 | **Attention-bloat-ratio metric** — % of pool tokens receiving non-trivial attention from the model's generated tokens. Already in James's original directive as a secondary reward signal. Requires attention extraction via cb_eval on attn_out-N tensors. | `bench/lens/` extension | 2-3h |
| D3 | **Layer-of-decision metric** — at which layer does the model's final-layer top-1 stabilize? Per generated-token. Already computable from cycle 0.4 lens JSONL with a small Python script. | `bench/lens/src/bin/lens-analyze.rs` | 30-60 min |
| D4 | **Auto holdout-validation hook** — when a cycle achieves 3+ PASS on working set, automatically run the same pipeline ONCE on the 10-instance holdout. Confirms generalization without ad-hoc trigger. | `cycle/run_cycle_loop.sh` extension | 30 min |

## Bundle E — model + corpus expansion (slow but cheap dev-time)

| # | Task | Effort |
|---|---|---|
| E1 | Run cycle 0.3 on 14B Q4 (already on disk at `~/Models/qwen2.5-coder-14b-gguf/`) — see if model size unlocks instances 7B can't | 30 min setup + ~6h inference on 7 instances |
| E2 | Run cycle 0.3 with `deepseek-r1-distill-qwen-7b` — reasoning-capable model might use the directive differently | 30 min setup + ~6h inference |
| E3 | Expand to SWE-bench Lite test split (300 instances) for the no-traceback signal experiment once B1/B2 land — measures whether the new directive channels generalize across the corpus | many hours, defer until working set + holdout look clean |

## Recommended attack order (biggest uplift first)

1. **Bundle A (must-do)** — unblock measurement; ~2h. Without A, every cycle 0.5+ has unreliable data.
2. **Bundle B1 (test_patch directive)** — biggest single uplift on no-traceback instances; ~3h. Tests directly whether multi-channel composition is the lever beyond traceback parsing.
3. **Bundle D1+D3 (lens-batch generate-mode + layer-of-decision)** — diagnostic tooling required before bundle C is worth investing in; ~2-3h together.
4. **Bundle B2+B4 (api-mention + composition)** — finish the text-channel exploration; ~3-4h.
5. **Bundle C1+C2 (per-token pool embed + markers)** — the real latent upgrade; ~6h. Only attempt after bundles A/B prove text channels have plateaued.

Total: A + B1 + D1/D3 + B2/B4 + C1/C2 = ~15-20h. Two focused days of dev work.

## Cycle-numbering map for this plan

| Cycle | Bundle | Outcome |
|---|---|---|
| 0.5 | text-channel experiments (A done, D in-flight) | reframe valid: graph-derived structure > rule-emphasis |
| 0.6 | Bundle A | reliable 7-instance measurement |
| 0.7 | Bundle B1 + B4 | test-patch + composed directive; run on 7 |
| 0.8 | Bundle D1+D3 | diagnostic-ready for latent work |
| 0.9 | Bundle C1+C2 | real latent injection |
| 1.0 | re-run on holdout 10 + maybe expand to lite-test | confirm generalization |

## What NOT to do (force-multipliers I'm explicitly skipping)

- **Hand-tuning per-instance directives** — won't generalize; James already rejected this framing
- **JSON pool experiments past the cycle 0.2 L2 negative result** — confirmed dead channel
- **Slice-1.5 lens on more conditions** — cycle 0.4 showed it's the wrong instrument
- **Bare rule-emphasis variants** — cycle 0.5 A confirmed they don't work
- **Adding more candidates without diagnostics** — bundle D before bundle C
