# glia-lens — slice 1: logit lens MVP

One concrete buildable thing. Per-layer top-K predictions on one Qwen forward pass, with and without a latent injection, emitted as JSONL + 2D PNG + terminal ASCII. See the approved plan at `~/.claude/plans/abstract-rolling-hellman.md`.

## What it is

Glia's job here is to be the 2D-and-text substrate for the logit lens.
Three coordinated outputs that cross-check each other:

1. **JSONL** — the canonical record. Schema mirrors neuropil's `RecordedEvent` so the slice-2 panel can ingest it directly.
2. **PNG** — 2D timeline rendered via the `plotters` crate. No 3D, no Bevy, no graphics-engine work.
3. **Terminal ASCII** — Unicode bar chart + summary table to stdout. The cheapest diagnostic; runs anywhere.

If any of the three disagrees about top-1-per-layer, one renderer has a bug. That's the whole point of having three.

## Why no candle / mistral.rs / patched llama.cpp

Investigated in Phase 1a (see plan):

- **candle** — already in `scratch/latent/`, ~7× slower than llama.cpp on this hardware per root `Cargo.toml` comment. User directive: llama.cpp only.
- **mistral.rs** — built on candle, no per-layer hooks. Same problem.
- **llama.cpp patched fork** — Phase 1b fallback. Avoided because llama.cpp already has the hook we need.

llama.cpp already exposes `ggml_backend_sched_eval_callback` (`cb_eval`) on `llama_context_params`. The `eval-callback` example in the llama.cpp tree (`common/debug.cpp::common_debug_cb_eval`) is the canonical pattern. `llama-cpp-sys-2` is the raw bindgen-generated Rust crate that exposes this field. No C++ patching, no fork.

For Qwen2 the per-layer residual stream tensor is named `l_out-{N}` (verified in `src/models/qwen2.cpp`). Filter on that regex inside the callback.

## How to build

```
cd scratch/lens
cargo build --release            # CPU (AVX2 / Ryzen 3700X-3800X)
cargo build --release --features cuda  # if 1660 Super can fit Q4 7B in 6GB
```

First build pulls in `llama-cpp-sys-2` which runs CMake on llama.cpp sources. Allow ~10 min on first compile; subsequent rebuilds are incremental.

## How to run

```
cargo run --release -- \
    --weights ~/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q4_k_m.gguf \
    --tokenizer ~/Models/qwen2.5-coder-tokenizer/tokenizer.json \
    --prefix    ../latent/out/marshmallow-1359-pathB.prefix \
    --suffix    ../latent/out/marshmallow-1359-pathB.suffix \
    --injection ../latent/out/inst-marshmallow-code__marshmallow-1359-7b-q4/summaries-aplus.json \
    --output-position 0 \
    --top-k 5 \
    --out out/marshmallow-1359-lens.jsonl \
    --render-png out/marshmallow-1359-lens.png \
    --render-ascii
```

## How to interpret a result

The JSONL has one line per `(layer, run, position)` tuple. Each line is one snapshot of "what would the model predict if the forward pass stopped at layer K?"

- **Signal A — injection acts early.** With-injection gold token enters top-3 by layer ≤14; baseline doesn't until layer ≥24. The lever pulls early; slice 2 should emphasise early-layer attention.
- **Signal B — injection acts late.** Both runs converge to gold by similar depth, but with-injection prob is higher in the last 8 layers. Slice 2 should emphasise late-layer MLP feature analysis.
- **Signal C — no per-layer effect.** KL stays low across all layers. The lens isn't the right instrument; pivot slice 2 to attention-flow rendering.

The outcome of slice 1 is *which signal we see*, not a binary pass/fail.

## Files

```
src/main.rs           orchestration + clap CLI
src/runtime.rs        unsafe llama.cpp FFI wrapper, cb_eval hook
src/lens.rs           per-layer norm + unembed + softmax + top-K + gold-rank + KL
src/jsonl.rs          RecordedEvent::LensStep schema
src/render_png.rs     plotters 2-panel (baseline | with-injection over layer axis)
src/render_ascii.rs   Unicode bar chart + summary table
```

## Hard constraints

- No Bevy, no 3D, no graphics-engine work. That's slice 2+ in neuropil.
- No Python in the runtime loop.
- llama.cpp via `cb_eval`. No vendored fork.
- One injection case (marshmallow-1359). Multi-case is slice 1.5+.
