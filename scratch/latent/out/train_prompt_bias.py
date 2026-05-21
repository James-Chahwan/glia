#!/usr/bin/env python3
"""train_prompt_bias — P6.1 (Option 10 minimal, cycle 1.1).

Trains a single 4096-dim bias vector to add to prompt embeddings before
decode. The bias = mean(prompt_embeds | F2P PASS) - mean(prompt_embeds |
F2P FAIL), L2-normalized. Applied as `prompt + alpha * bias` to nudge
the prompt-embedding space toward configurations that historically
produced PASS outcomes.

Inputs:
  - scratch/latent/out/instance_results.jsonl  (per-instance PASS/FAIL)
  - scratch/latent/out/inst-<id>-*/prefix.txt   (the prompt that generated
                                                  that result)

Output:
  - scratch/latent/out/prompt_bias.npy   shape [n_embd=4096], float32

Approach: cheap proxy. Computes per-instance prompt embedding as the
MEAN of token-embedding-table rows over prefix tokens (same dequant path
run_llama_pathB.py uses, no forward pass). Aggregates by F2P verdict.

Why minimal: proof-of-concept. If the bias produces measurable PASS
uplift in cycle 1.1, upgrade to D×D projection trained on validator
verdicts (Option 10 full).

Usage:
  python train_prompt_bias.py --gguf <path> --out scratch/latent/out/prompt_bias.npy
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import gguf
import numpy as np
from llama_cpp import Llama

LATENT_OUT = Path("/home/ivy/Code/glia/scratch/latent/out")
DEFAULT_GGUF = Path("/home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q4_k_m.gguf")


def _dequant_token_embed(gguf_path: Path) -> np.ndarray:
    """Load + dequantize the token_embd.weight tensor. Uses the same gguf
    dequant path run_llama_pathB.py uses — handles Q4_K_M / Q8_0 / F32
    via `gguf.dequantize`. Returns shape [n_vocab, n_embd]."""
    r = gguf.GGUFReader(str(gguf_path))
    emb_t = next(t for t in r.tensors if t.name == "token_embd.weight")
    # GGUF tensor.shape is [n_embd, n_vocab]; after dequantize+reshape
    # we want [n_vocab, n_embd] for row-indexed lookup.
    arr = gguf.dequantize(emb_t.data, emb_t.tensor_type)
    arr = arr.reshape(emb_t.shape[1], emb_t.shape[0])
    return arr.astype(np.float32, copy=False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gguf", default=str(DEFAULT_GGUF))
    ap.add_argument("--results", default=str(LATENT_OUT / "instance_results.jsonl"))
    ap.add_argument("--out", default=str(LATENT_OUT / "prompt_bias.npy"))
    ap.add_argument("--min-each", type=int, default=2,
                    help="minimum PASS and FAIL counts to produce a bias (else abort).")
    args = ap.parse_args()

    results_path = Path(args.results)
    if not results_path.exists():
        print(f"results file missing: {results_path}", file=sys.stderr)
        sys.exit(1)

    # Build label map: instance_id-tag → PASS/FAIL.
    by_inst: dict[tuple[str, str], str] = {}
    for line in results_path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            o = json.loads(line)
        except json.JSONDecodeError:
            continue
        inst_id = o.get("instance_id")
        if not inst_id:
            continue
        f2p = o.get("f2p") or ""
        if f2p == "PASS":
            label = "PASS"
        elif f2p.startswith("FAIL"):
            label = "FAIL"
        else:
            continue  # skip APPLY-FAIL / NO-RUN — ambiguous signal
        # Use latest per instance.
        by_inst[(inst_id, "any")] = label

    # Discover inst-*/prefix.txt directories.
    pass_prefixes: list[Path] = []
    fail_prefixes: list[Path] = []
    for inst_dir in sorted(LATENT_OUT.glob("inst-*")):
        prefix = inst_dir / "prefix.txt"
        if not prefix.exists():
            continue
        # Extract instance_id from dir name: inst-<id>-<model>-<tag>
        # The model tag is something like "7b-q4" or has dashes. Split
        # carefully: try matching against by_inst keys.
        name = inst_dir.name[len("inst-"):]
        # Try progressive prefixes until one matches a known instance.
        matched_inst_id: str | None = None
        for sep_idx in range(len(name), 0, -1):
            cand = name[:sep_idx]
            if (cand, "any") in by_inst:
                matched_inst_id = cand
                break
        if not matched_inst_id:
            continue
        label = by_inst[(matched_inst_id, "any")]
        if label == "PASS":
            pass_prefixes.append(prefix)
        else:
            fail_prefixes.append(prefix)

    print(f"train data: {len(pass_prefixes)} PASS, {len(fail_prefixes)} FAIL", file=sys.stderr)
    if len(pass_prefixes) < args.min_each or len(fail_prefixes) < args.min_each:
        print(f"insufficient data (min_each={args.min_each}); aborting bias train", file=sys.stderr)
        sys.exit(2)

    # Load tokenizer + embed table.
    print(f"loading gguf: {args.gguf}", file=sys.stderr)
    llm = Llama(model_path=str(args.gguf), embedding=False, vocab_only=False,
                logits_all=False, verbose=False)
    embed_table = _dequant_token_embed(Path(args.gguf))
    n_embd = embed_table.shape[1]
    print(f"embed_table: vocab={embed_table.shape[0]} n_embd={n_embd}", file=sys.stderr)

    def _mean_prompt_embed(prefix_path: Path) -> np.ndarray | None:
        try:
            text = prefix_path.read_text()
            ids = llm.tokenize(text.encode(), add_bos=False, special=True)
            # Clip to first 4K tokens to bound memory.
            ids = ids[:4096]
            if not ids:
                return None
            rows = embed_table[np.asarray(ids, dtype=np.int64)]
            return rows.mean(axis=0).astype(np.float32)
        except Exception as e:
            print(f"  embed failed for {prefix_path}: {e}", file=sys.stderr)
            return None

    pass_vecs = [v for v in (_mean_prompt_embed(p) for p in pass_prefixes) if v is not None]
    fail_vecs = [v for v in (_mean_prompt_embed(p) for p in fail_prefixes) if v is not None]
    if len(pass_vecs) < args.min_each or len(fail_vecs) < args.min_each:
        print(f"after embed: PASS={len(pass_vecs)} FAIL={len(fail_vecs)}; aborting", file=sys.stderr)
        sys.exit(3)

    pass_mean = np.mean(pass_vecs, axis=0)
    fail_mean = np.mean(fail_vecs, axis=0)
    bias = (pass_mean - fail_mean).astype(np.float32)
    norm = np.linalg.norm(bias)
    if norm > 0:
        bias = bias / norm
    np.save(args.out, bias)
    print(f"wrote bias: {args.out} (norm-before-normalize={norm:.4f}, shape={bias.shape})", file=sys.stderr)


if __name__ == "__main__":
    main()
