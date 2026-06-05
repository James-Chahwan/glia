#!/usr/bin/env bash
# lens_bias_smoke.sh — alpha sweep for lens-attention-bias on marshmallow.
#
# Runs the diagnostic lens-attention-bias bin (already shipped, never used
# in cycle) with target_qname=`_bind_to_schema` across alpha ∈ {0.1, 0.3,
# 0.5, 1.0, 2.0} and emits a comparison JSON: baseline-vs-injected output
# byte-diff per alpha. This is the substrate measurement BEFORE we wire
# attention bias into the real inference pipeline — establishes whether
# the lever even moves output before committing to cb_eval integration.
#
# Output: bench/lens/cycle/lens_bias_alpha_sweep.jsonl

set -u
SMOKE_DIR=/home/ivy/Code/glia/bench/lens/cycle/lens-bias-smoke
mkdir -p "${SMOKE_DIR}"

MODEL="${MODEL:-7b-q4}"
GGUF="${GGUF:-/home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q4_k_m.gguf}"
TOKENIZER=/home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json

# Reuse cycle 2.0's prepared prefix + suffix for marshmallow.
INSTANCE_DIR=$(ls -td /home/ivy/Code/glia/bench/latent/out/inst-marshmallow-code__marshmallow-1359-*-cycle-2.0-gpu-14b 2>/dev/null | head -1)
if [[ -z "${INSTANCE_DIR}" || ! -d "${INSTANCE_DIR}" ]]; then
  echo "ERROR: no marshmallow workdir from cycle 2.0; run cycle first" >&2
  exit 1
fi

PREFIX="${INSTANCE_DIR}/prefix.txt"
SUFFIX="${INSTANCE_DIR}/suffix.txt"

echo "=== lens-bias smoke ==="
echo "  model: ${MODEL}"
echo "  gguf:  ${GGUF}"
echo "  src:   ${INSTANCE_DIR}"

# Run for each alpha; collect per-alpha JSON.
OUT="${SMOKE_DIR}/lens_bias_alpha_sweep.jsonl"
> "${OUT}"

for ALPHA in 0.1 0.3 0.5 1.0 2.0; do
  echo "  alpha=${ALPHA}"
  /home/ivy/Code/glia/bench/lens/target/release/lens-attention-bias \
    --weights "${GGUF}" \
    --tokenizer "${TOKENIZER}" \
    --prefix "${PREFIX}" \
    --suffix "${SUFFIX}" \
    --target-qname "_bind_to_schema" \
    --inject-layers "25:28" \
    --inject-positions "23:26" \
    --alpha "${ALPHA}" \
    --max-new 64 \
    --out "${SMOKE_DIR}/alpha_${ALPHA}.json" 2>&1 | tail -5
  cat "${SMOKE_DIR}/alpha_${ALPHA}.json" >> "${OUT}"
  echo "" >> "${OUT}"
done

echo
echo "=== alpha sweep complete: ${OUT} ==="
python3 -c "
import json
recs = [json.loads(l) for l in open('${OUT}') if l.strip()]
print('| alpha | base_len | injected_len | byte_diff |')
print('|---|---|---|---|')
for r in recs:
    base = r.get('baseline_output', '')
    inj = r.get('injected_output', '')
    diff = sum(1 for a, b in zip(base, inj) if a != b) + abs(len(base) - len(inj))
    print(f'| {r.get(\"alpha\"):.1f} | {len(base)} | {len(inj)} | {diff} |')
"
