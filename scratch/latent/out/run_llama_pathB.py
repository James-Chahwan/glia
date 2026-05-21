#!/usr/bin/env python3
"""Path B with llama.cpp — text + pooled-summary embeddings injected via batch.embd.

Mirrors candle's `examples/run_pathB.rs` layout:
  prefix_text_tokens → embed → [Tp, D]
  per summary: tokenize → embed → mean-pool → [1, D]
  suffix_text_tokens → embed → [Ts, D]
  concat all → llama_batch.embd → decode → greedy sample

Usage: run_llama_pathB.py <gguf> <prefix_file> <suffix_file> <summaries_json> <out_file>
env: MAX_NEW=400 N_CTX=8192 N_THREADS=auto DRY_RUN=1
"""
import os, sys, time, json, ctypes
import numpy as np
import gguf
from llama_cpp import (
    Llama, llama_batch_init, llama_batch_free, llama_decode,
    llama_get_logits_ith, llama_n_embd,
)
import llama_cpp

if len(sys.argv) < 6:
    print("usage: run_llama_pathB.py <gguf> <prefix> <suffix> <summaries_json> <out>", file=sys.stderr)
    sys.exit(2)

gguf_path, prefix_path, suffix_path, summaries_path, out_path = sys.argv[1:6]
max_new = int(os.environ.get("MAX_NEW", "400"))
n_ctx = int(os.environ.get("N_CTX", "8192"))
n_threads = int(os.environ.get("N_THREADS", str(os.cpu_count() or 8)))
dry_run = os.environ.get("DRY_RUN") == "1"
no_inject = os.environ.get("NO_INJECT") == "1"
# B1 (cycle 0.6 spitball): replace mean-pool with per-token embed sequence.
# Each summary becomes a multi-token attendable segment instead of a single
# blurry mean-pooled vector. Default off to preserve cycle 0.7 reproducibility.
per_token_pool = os.environ.get("PER_TOKEN_POOL") == "1"
# B2: wrap each pool entry with marker tokens so the model can attend to
# bounded segments. Markers are literal text tokenized fresh — no reserved-
# ID allocation needed. Requires PER_TOKEN_POOL=1 to have meaning.
pool_markers = os.environ.get("POOL_MARKERS") == "1"

prefix = open(prefix_path).read()
suffix = open(suffix_path).read()
summaries = json.load(open(summaries_path))
print(f"[pathB-llama] prefix {len(prefix)}c, suffix {len(suffix)}c, {len(summaries)} summaries", file=sys.stderr)

# --- Step 1: dequantize token_embd table once ---
t0 = time.time()
print(f"[pathB-llama] loading token_embd from gguf...", file=sys.stderr)
r = gguf.GGUFReader(gguf_path)
emb_t = next(t for t in r.tensors if t.name == "token_embd.weight")
emb_table = gguf.dequantize(emb_t.data, emb_t.tensor_type).reshape(emb_t.shape[1], emb_t.shape[0])
del r
print(f"[pathB-llama] embed table {emb_table.shape} f32 in {time.time()-t0:.1f}s", file=sys.stderr)

# --- Step 2: load model ---
t0 = time.time()
llm = Llama(
    model_path=gguf_path,
    n_ctx=n_ctx,
    n_threads=n_threads,
    n_gpu_layers=0,
    n_batch=n_ctx,  # batch must hold full prefill
    verbose=False,
    logits_all=False,
)
print(f"[pathB-llama] model loaded in {time.time()-t0:.1f}s", file=sys.stderr)

D = llama_n_embd(llm._model.model)
assert D == emb_table.shape[1], f"D mismatch: model {D} vs table {emb_table.shape[1]}"

def lookup(token_ids):
    """token IDs → [T, D] f32 via embed table lookup."""
    return emb_table[np.asarray(token_ids, dtype=np.int64)]

# --- Step 3: tokenize + embed prefix/suffix ---
prefix_ids = llm.tokenize(prefix.encode(), add_bos=False, special=True)
suffix_ids = llm.tokenize(suffix.encode(), add_bos=False, special=True)
prefix_emb = lookup(prefix_ids)  # [Tp, D]
suffix_emb = lookup(suffix_ids)  # [Ts, D]
print(f"[pathB-llama] prefix {len(prefix_ids)} tok, suffix {len(suffix_ids)} tok", file=sys.stderr)

# --- Step 4: per-summary embedding ---
# Default (legacy): mean-pool each summary to [1, D] — the cycle 0.2 L2
# result showed this collapses ~all directive content to "vibes-only" and
# the score field is ignored downstream.
# PER_TOKEN_POOL=1: keep the full [t, D] embed sequence for each summary,
# turning each into an attendable multi-token segment.
# POOL_MARKERS=1 (requires PER_TOKEN_POOL=1): prepend / append literal
# `[POOL_<i>_START]` / `[POOL_<i>_END]` text markers around each entry so
# the model can attend to bounded pool spans.
node_vecs = []
if no_inject:
    print(f"[pathB-llama] NO_INJECT=1 — skipping per-summary embed ({len(summaries)} summaries ignored)", file=sys.stderr)
else:
    mode = "per-token" if per_token_pool else "mean-pool"
    markers_label = " +markers" if (per_token_pool and pool_markers) else ""
    print(f"[pathB-llama] pool mode: {mode}{markers_label}", file=sys.stderr)
    pooled_count = 0
    per_token_count = 0
    for i, s in enumerate(summaries):
        body = s.get("summary", "").strip()
        if not body:
            continue
        sid = llm.tokenize(body.encode(), add_bos=False, special=False)
        emb = lookup(sid)  # [t, D]
        if per_token_pool:
            if pool_markers:
                start_ids = llm.tokenize(f"[POOL_{i}_START]".encode(), add_bos=False, special=False)
                end_ids = llm.tokenize(f"[POOL_{i}_END]".encode(), add_bos=False, special=False)
                start_emb = lookup(start_ids)
                end_emb = lookup(end_ids)
                entry = np.concatenate([start_emb, emb, end_emb], axis=0)
            else:
                entry = emb
            node_vecs.append(entry)
            per_token_count += entry.shape[0]
        else:
            pooled = emb.mean(axis=0, keepdims=True)  # [1, D]
            node_vecs.append(pooled)
            pooled_count += 1
        if i < 3:
            seg_tokens = node_vecs[-1].shape[0]
            print(f"[pathB-llama] node[{i}] {s.get('qname','?')} {len(sid)} tok → {seg_tokens} segment-vec(s)", file=sys.stderr)
    if per_token_pool:
        print(f"[pathB-llama] {len(node_vecs)} per-token entries, total {per_token_count} pool tokens", file=sys.stderr)
    else:
        print(f"[pathB-llama] {pooled_count} pooled node vectors", file=sys.stderr)

# --- Step 5: concat full embed sequence [T_total, D] ---
all_embeds = np.concatenate([prefix_emb] + node_vecs + [suffix_emb], axis=0).astype(np.float32)
T_total = all_embeds.shape[0]
print(f"[pathB-llama] full embed [{T_total}, {D}] = prefix {len(prefix_ids)} + nodes {len(node_vecs)} + suffix {len(suffix_ids)}", file=sys.stderr)
if T_total > n_ctx:
    print(f"[pathB-llama] WARN T_total {T_total} > n_ctx {n_ctx}; truncating prefix", file=sys.stderr)
    sys.exit(3)

if dry_run:
    print(f"[pathB-llama] DRY_RUN — exit before decode", file=sys.stderr)
    sys.exit(0)

# --- Step 6: build batch with embd populated, decode ---
ctx = llm._ctx.ctx
batch = llama_batch_init(T_total, D, 1)

# Fill batch.embd with [T_total, D] flattened C-order
embd_flat = all_embeds.flatten()
ctypes.memmove(batch.embd, embd_flat.ctypes.data, embd_flat.nbytes)

# Fill positions, seq_ids, logits flags
for i in range(T_total):
    batch.pos[i] = i
    batch.n_seq_id[i] = 1
    batch.seq_id[i][0] = 0
    batch.logits[i] = 1 if i == T_total - 1 else 0
batch.n_tokens = T_total

t0 = time.time()
rc = llama_decode(ctx, batch)
if rc != 0:
    print(f"[pathB-llama] llama_decode prefill rc={rc}", file=sys.stderr)
    llama_batch_free(batch)
    sys.exit(4)
prefill_s = time.time() - t0
print(f"[pathB-llama] prefill {T_total} tok in {prefill_s:.1f}s ({T_total/prefill_s:.1f} tok/s)", file=sys.stderr)

# --- Step 7: greedy sample loop ---
eos_ids = {llm._model.token_eos(), 151645, 151643}  # generic eos + Qwen <|im_end|>, <|endoftext|>

# First token from last position of prefill
logits_p = llama_get_logits_ith(ctx, T_total - 1)
n_vocab = llm._model.n_vocab()
first_logits = np.frombuffer(
    (ctypes.c_float * n_vocab).from_address(ctypes.addressof(logits_p.contents)),
    dtype=np.float32, count=n_vocab,
).copy()
next_id = int(first_logits.argmax())
collected = [next_id]

# Reuse a small batch for incremental decode
inc = llama_batch_init(1, 0, 1)
inc.n_tokens = 1
inc.pos[0] = T_total
inc.n_seq_id[0] = 1
inc.seq_id[0][0] = 0
inc.logits[0] = 1

t1 = time.time()
for step in range(1, max_new):
    if next_id in eos_ids:
        print(f"[pathB-llama] eos at step {step}", file=sys.stderr)
        break
    inc.token[0] = next_id
    inc.pos[0] = T_total + step - 1
    rc = llama_decode(ctx, inc)
    if rc != 0:
        print(f"[pathB-llama] decode step {step} rc={rc}", file=sys.stderr)
        break
    lp = llama_get_logits_ith(ctx, 0)
    arr = np.frombuffer(
        (ctypes.c_float * n_vocab).from_address(ctypes.addressof(lp.contents)),
        dtype=np.float32, count=n_vocab,
    )
    next_id = int(arr.argmax())
    collected.append(next_id)
    if (step + 1) % 32 == 0:
        elapsed = time.time() - t1
        print(f"[pathB-llama] {step+1} tok @ {(step+1)/elapsed:.1f} tok/s", file=sys.stderr)

elapsed = time.time() - t1
print(f"[pathB-llama] gen done {len(collected)} tok in {elapsed:.1f}s ({len(collected)/elapsed:.1f} tok/s)", file=sys.stderr)

llama_batch_free(batch)
llama_batch_free(inc)

text = llm.detokenize(collected).decode("utf-8", errors="replace")
with open(out_path, "w") as f:
    f.write(text)
print(f"[pathB-llama] wrote {out_path}", file=sys.stderr)
