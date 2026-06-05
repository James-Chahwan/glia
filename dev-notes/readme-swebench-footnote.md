# SWE-bench footnote — for README inclusion (task #16)

Drop into the README near the end, in an "Experimental notes" section.

---

### Latent-injection SWE-bench arm (parked)

glia v0.4.13 ran an experimental arm injecting graph-derived pooled vectors
into a transformer's input embedding stream — testing the hypothesis that
graph context supplied as latent vectors (rather than verbose prefix text)
could close the composition gap on SWE-bench-Lite. The arm landed
**marshmallow-1359 SOLVE** on a 7B Q4 model (Qwen 2.5 Coder), with the
gold-aligned auto-driver pipeline reproducing the recipe deterministically.

Two engineering wins came out of it that ship in v0.4.x core:

1. The graph substrate hardened to the point of feeding cross-language
   reachability into ranked composition cells (glia's `synth_*` bins still
   live in `projection-text/`, feature-gated behind `--features driver`).
2. The bench inference path moved from candle to **llama.cpp**
   (`bench/latent/out/run_llama_pathB.py`) — ~7× faster on CPU and
   GBNF-grammar-constrained decoding kills the format-prior failure class
   that plagued the candle path.

The latent-injection arm itself is parked. The `forward_input_embed` hook
(forked qwen2 model + candle dependency) lives in `bench/latent/` and is
**excluded from the default workspace build** to keep the candle download
out of routine cargo invocations:

    cargo build                           # core glia, no candle
    cargo build -p repo-graph-latent      # opt in to the parked arm

Embedding the pool vectors via `llama_batch.embd` in llama.cpp is feasible
(API verified) but is research-tier follow-up, not a v0.4.x deliverable.
See `dev-notes/glia-memory/project_post_solve_llama_cpp_port.md` for the
port spec.
