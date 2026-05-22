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
# B5 beam sampling: replace greedy argmax with top-k + temperature sampling
# when SAMPLE_TEMP > 0. Each invocation produces ONE candidate; the
# orchestrator (run_instance.py) loops to gather N candidates and picks the
# best by apply+test. Defaults preserve greedy behavior.
sample_temp = float(os.environ.get("SAMPLE_TEMP", "0"))
sample_top_k = int(os.environ.get("SAMPLE_TOP_K", "5"))
sample_seed = int(os.environ.get("SAMPLE_SEED", "0"))

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
n_gpu_layers = int(os.environ.get("N_GPU_LAYERS", "0"))
llm = Llama(
    model_path=gguf_path,
    n_ctx=n_ctx,
    n_threads=n_threads,
    n_gpu_layers=n_gpu_layers,
    n_batch=n_ctx,  # batch must hold full prefill
    verbose=False,
    logits_all=False,
)
print(f"[pathB-llama] n_gpu_layers={n_gpu_layers} (set N_GPU_LAYERS=99 for full GPU offload)", file=sys.stderr)
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
# P2.1 (D2 pool reshape): record per-summary position ranges in the
# concatenated prompt-embedding stream so an outer orchestrator can
# aggregate D2 attention norms per pool entry. Format: list of {idx,
# qname, token_start, token_end} (0-indexed, half-open). Token indices
# are relative to all_embeds, which is [prefix_emb | pool_entries | suffix_emb].
pool_positions = []
_prefix_tokens_len = len(prefix_ids)
_cursor = _prefix_tokens_len  # cursor walks the pool segment
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
            _entry_len = entry.shape[0]
        else:
            pooled = emb.mean(axis=0, keepdims=True)  # [1, D]
            node_vecs.append(pooled)
            pooled_count += 1
            _entry_len = 1
        pool_positions.append({
            "idx": i,
            "qname": s.get("qname"),
            "token_start": _cursor,
            "token_end": _cursor + _entry_len,
        })
        _cursor += _entry_len
        if i < 3:
            seg_tokens = node_vecs[-1].shape[0]
            print(f"[pathB-llama] node[{i}] {s.get('qname','?')} {len(sid)} tok → {seg_tokens} segment-vec(s)", file=sys.stderr)
    if per_token_pool:
        print(f"[pathB-llama] {len(node_vecs)} per-token entries, total {per_token_count} pool tokens", file=sys.stderr)
    else:
        print(f"[pathB-llama] {pooled_count} pooled node vectors", file=sys.stderr)

# --- Step 5: concat full embed sequence [T_total, D] ---
all_embeds = np.concatenate([prefix_emb] + node_vecs + [suffix_emb], axis=0).astype(np.float32)

# P6.1 (Option 10 minimal, cycle 1.1): latent prompt warping. Add a
# learned bias vector to every prompt-embedding row before decode. The
# bias is computed by train_prompt_bias.py as
# mean(prompts that PASSed) - mean(prompts that FAILed), normalized.
# Effect: nudges the prompt-embedding space toward configurations that
# historically produced PASS outcomes. Single 4096-dim vector — minimal
# scope; upgrade to D×D projection if it shows uplift.
_prompt_bias_path = os.environ.get("GLIA_PROMPT_BIAS", "")
_prompt_bias_alpha = float(os.environ.get("GLIA_PROMPT_BIAS_ALPHA", "0.1"))
if _prompt_bias_path and os.path.exists(_prompt_bias_path) and _prompt_bias_alpha != 0.0:
    try:
        bias = np.load(_prompt_bias_path).astype(np.float32)
        if bias.shape == (all_embeds.shape[1],):
            all_embeds = all_embeds + _prompt_bias_alpha * bias[None, :]
            print(f"[pathB-llama] prompt bias applied (alpha={_prompt_bias_alpha}, "
                  f"bias_norm={float(np.linalg.norm(bias)):.4f})", file=sys.stderr)
        else:
            print(f"[pathB-llama] prompt bias shape mismatch: {bias.shape} vs "
                  f"({all_embeds.shape[1]},); skipping", file=sys.stderr)
    except Exception as _e:
        print(f"[pathB-llama] prompt bias load failed: {_e}; skipping", file=sys.stderr)
T_total = all_embeds.shape[0]
print(f"[pathB-llama] full embed [{T_total}, {D}] = prefix {len(prefix_ids)} + nodes {len(node_vecs)} + suffix {len(suffix_ids)}", file=sys.stderr)
if T_total > n_ctx:
    # Top-K pool cap (cycle 1.2-gpu-14b mid-flight finding): matplotlib-22711
    # has 1156 entries → 141K tokens, exceeds Qwen's 131K native ctx by ~10K.
    # Drop pool entries until the total fits. Prefix + suffix are load-
    # bearing — never truncate those.
    #
    # A2 protected-set (cycle 2.0 finding): cycle 1.2 + cycle 2.0 evidence
    # both show matplotlib-22711 emerging from pool-cap with NO-DIFF — the
    # gold qname is presumably in the dropped tail. Build a protected set
    # from GLIA_PROTECTED_QNAMES (comma-separated qnames) — entries whose
    # qname matches NEVER get dropped. run_instance.py sets this from the
    # directive's PRIMARY target block.
    protected = set()
    _protected_env = os.environ.get("GLIA_PROTECTED_QNAMES", "")
    if _protected_env:
        for q in _protected_env.split(","):
            q = q.strip()
            if q:
                protected.add(q)
    overflow = T_total - n_ctx
    # Build (drop-order-priority, idx, token_len) tuples for each pool slot.
    # Lower priority = drop first. Protected entries get priority +1M
    # (effectively never dropped unless absolutely required).
    pool_meta = []
    for i, v in enumerate(node_vecs):
        qn = pool_positions[i].get("qname", "") if i < len(pool_positions) else ""
        is_protected = qn in protected or any(qn.endswith("::" + p.split("::")[-1])
                                              for p in protected if "::" in p)
        # Priority: tail entries drop first (low priority = high idx); protected
        # gets a +1M priority bump.
        priority = i + (1_000_000 if is_protected else 0)
        pool_meta.append((priority, i, v.shape[0], is_protected))
    # Sort ascending by priority: lowest-priority (tail, non-protected) at front.
    pool_meta.sort(key=lambda m: m[0])

    # Drop from front (lowest priority) until overflow cleared.
    dropped_set = set()
    dropped_tokens = 0
    n_protected_kept = sum(1 for m in pool_meta if m[3])
    for prio, idx, tok_len, prot in pool_meta:
        if prot:
            # Don't touch protected entries even if we run over.
            continue
        if dropped_tokens >= overflow + 256:
            break
        dropped_set.add(idx)
        dropped_tokens += tok_len

    if dropped_set:
        kept_indices = [i for i in range(len(node_vecs)) if i not in dropped_set]
        # Rebuild concat with the kept pool subset.
        node_vecs = [node_vecs[i] for i in kept_indices]
        if pool_positions:
            pool_positions = [pool_positions[i] for i in kept_indices]
        all_embeds = np.concatenate([prefix_emb] + node_vecs + [suffix_emb], axis=0).astype(np.float32)
        T_total = all_embeds.shape[0]
        print(f"[pathB-llama] pool capped: dropped {len(dropped_set)} entries "
              f"({dropped_tokens} tokens); protected={n_protected_kept} kept; "
              f"new T_total={T_total}",
              file=sys.stderr)
        # Log first 3 dropped qnames so we can see if gold is being culled
        # (it shouldn't, with the protected-set filter active).
        sample_dropped = [pool_positions_orig := None]  # placeholder
        # Note: pool_positions has already been trimmed above. Read from
        # node_vecs context: log indices instead.
        sample = sorted(dropped_set)[:3]
        print(f"[pathB-llama] pool capped: sample dropped idx={sample}",
              file=sys.stderr)
    if T_total > n_ctx:
        print(f"[pathB-llama] ERROR T_total {T_total} still > n_ctx {n_ctx} after pool cap; "
              f"prefix+suffix alone ({len(prefix_ids)+len(suffix_ids)} tok) exceeds ctx",
              file=sys.stderr)
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

# --- Step 7: sample loop (greedy by default; top-k + temperature when SAMPLE_TEMP > 0) ---
eos_ids = {llm._model.token_eos(), 151645, 151643}  # generic eos + Qwen <|im_end|>, <|endoftext|>

# B5 beam-sampling RNG. Seeded so callers can reproduce a given candidate by
# passing the same SAMPLE_SEED. Default 0 → llama.cpp's fresh RNG via numpy.
_rng = np.random.default_rng(sample_seed if sample_seed else None)

def _sample_next(logits: np.ndarray) -> int:
    """Pick the next token from a logits vector. When SAMPLE_TEMP <= 0
    returns greedy argmax (cycle 0.7/0.8/0.9 behavior). Otherwise:
    keep top-K logits, scale by 1/temperature, softmax, multinomial draw.
    """
    if sample_temp <= 0.0:
        return int(logits.argmax())
    # Top-K filter: indices of K highest-logit tokens.
    k = max(1, min(sample_top_k, logits.shape[0]))
    top_idx = np.argpartition(-logits, k - 1)[:k]
    top_logits = logits[top_idx] / sample_temp
    # Softmax (subtract max for numerical stability).
    top_logits -= top_logits.max()
    probs = np.exp(top_logits)
    probs /= probs.sum()
    choice = _rng.choice(k, p=probs)
    return int(top_idx[choice])

# First token from last position of prefill
logits_p = llama_get_logits_ith(ctx, T_total - 1)
n_vocab = llm._model.n_vocab()
first_logits = np.frombuffer(
    (ctypes.c_float * n_vocab).from_address(ctypes.addressof(logits_p.contents)),
    dtype=np.float32, count=n_vocab,
).copy()
next_id = _sample_next(first_logits)
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
    next_id = _sample_next(arr)
    collected.append(next_id)
    if (step + 1) % 32 == 0:
        elapsed = time.time() - t1
        print(f"[pathB-llama] {step+1} tok @ {(step+1)/elapsed:.1f} tok/s", file=sys.stderr)

elapsed = time.time() - t1
print(f"[pathB-llama] gen done {len(collected)} tok in {elapsed:.1f}s ({len(collected)/elapsed:.1f} tok/s)", file=sys.stderr)

llama_batch_free(batch)
llama_batch_free(inc)

text = llm.detokenize(collected).decode("utf-8", errors="replace")
# P2.1: emit pool position ranges so an orchestrator can correlate per-
# token attention norms (from lens-attention bin) with per-summary identity.
# Path: alongside out_path, replacing .txt with .pool_positions.json.
try:
    _pos_path = os.path.splitext(out_path)[0] + ".pool_positions.json"
    with open(_pos_path, "w") as f:
        json.dump({
            "prefix_token_count": _prefix_tokens_len,
            "pool_token_start": _prefix_tokens_len,
            "pool_token_end": _cursor,
            "suffix_token_count": len(suffix_ids),
            "entries": pool_positions,
        }, f)
    print(f"[pathB-llama] wrote pool_positions: {_pos_path}", file=sys.stderr)
except Exception as _e:
    print(f"[pathB-llama] pool_positions write failed: {_e}", file=sys.stderr)

with open(out_path, "w") as f:
    f.write(text)
print(f"[pathB-llama] wrote {out_path}", file=sys.stderr)
