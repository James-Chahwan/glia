#!/usr/bin/env bash
# auto8h.sh — 8-hour autonomous learning sequence on Runpod A40.
#
# Designed to fire AFTER cycle 1.2-gpu-14b is already running. Polls for
# its completion, then sequences:
#   cycle 1.3-gpu-32b      (~150 min)  — model scaling answer
#   cycle 1.4-no-channels  (~75 min)   — channel ablation: traceback only
#   cycle 1.5-no-priors    (~75 min)   — channel ablation: file_priors off
#   1.6-beam8              (~30 min)   — beam scaling on hard instances
#   1.7-qwen3-moe-smoke    (~10 min)   — alt architecture
#   final summary          (~5 min)    — cross-cycle comparison matrix
#
# All output piped to cycle/auto8h-index.md for next-session ingest.

set -u
cd /home/ivy/Code/glia/scratch/lens
export PATH="/root/.cargo/bin:/usr/local/cuda/bin:$PATH"

LOG=/home/ivy/Code/glia/scratch/lens/cycle/auto8h-index.md
START=$(date +%s)

# Initialize log
cat > "$LOG" <<EOF
# auto8h — autonomous learning sequence
Started: $(date)

## Plan
- 1.3-gpu-32b: 32B Q4 baseline (model scaling)
- 1.4-no-channels: 14B + GLIA_DIRECTIVE_LEGACY=1 (channel ablation)
- 1.5-no-priors: 14B + file_priors/exemplars disabled
- 1.6-beam8: 14B GLIA_SAMPLES=8 on hard instances
- 1.7-qwen3-moe-smoke: alt arch smoke
- summary: cross-cycle matrix

EOF

phase() {
  local now elapsed
  now=$(date +%s)
  elapsed=$(( (now - START) / 60 ))
  echo
  echo "======================================================================"
  echo "=== [+${elapsed}min] $1"
  echo "======================================================================"
  echo "" >> "$LOG"
  echo "## [+${elapsed}min] $1" >> "$LOG"
}

run_analyzers() {
  local cycle=$1
  echo "Analyzers on $cycle:" >> "$LOG"
  for tool in failure_mode_classifier channel_attribution gold_patch_oracle solution_curve; do
    echo "  --- $tool ---" >> "$LOG"
    python3 scripts/$tool.py --cycle "$cycle" 2>&1 | tail -8 >> "$LOG"
  done
  # Append results.jsonl summary
  if [[ -f cycle/cycle-${cycle}-results.jsonl ]]; then
    echo "  --- raw results ---" >> "$LOG"
    python3 -c "
import json
with open('cycle/cycle-${cycle}-results.jsonl') as f:
    for line in f:
        o = json.loads(line)
        f2p = o.get('f2p') or o.get('apply', 'UNKNOWN')
        wall = o.get('wall_s_outer') or o.get('wall_s', 0)
        print(f'    {o[\"instance_id\"]:50}  {f2p[:32]:32}  wall={int(wall):4}s')
" >> "$LOG"
  fi
}

#
# Phase 0 — wait for the in-flight cycle 1.2-gpu-14b
#
phase "Wait for cycle 1.2-gpu-14b"
while pgrep -f "run_cycle_loop.sh 1.2-gpu-14b" >/dev/null 2>&1; do
  sleep 60
done
echo "cycle 1.2-gpu-14b complete" | tee -a "$LOG"
run_analyzers 1.2-gpu-14b

#
# Phase 1 — cycle 1.3-gpu-32b (model scaling answer)
#
phase "Setup + cycle 1.3-gpu-32b"
mkdir -p /home/ivy/Models/qwen2.5-coder-32b-gguf
if [[ ! -f /home/ivy/Models/qwen2.5-coder-32b-gguf/qwen2.5-coder-32b-instruct-q4_k_m.gguf ]]; then
  echo "Downloading 32B Q4..." | tee -a "$LOG"
  hf download Qwen/Qwen2.5-Coder-32B-Instruct-GGUF qwen2.5-coder-32b-instruct-q4_k_m.gguf \
    --local-dir /home/ivy/Models/qwen2.5-coder-32b-gguf 2>&1 | tail -3 | tee -a "$LOG"
fi
# 32B + 65K context needs ~46GB VRAM — cap N_CTX even for big pools
GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_LENS_TRACE=0 \
GLIA_COMPOSITIONAL=1 N_GPU_LAYERS=99 AUTO_HOLDOUT=0 \
GLIA_N_CTX_MAX=65536 \
./cycle/run_cycle_loop.sh 1.3-gpu-32b 32b-q4 2>&1 | tail -20 | tee -a "$LOG"
run_analyzers 1.3-gpu-32b

#
# Phase 2 — cycle 1.4-no-channels (channel ablation: legacy only)
#
phase "Cycle 1.4-no-channels (LEGACY mode, 14B)"
GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_LENS_TRACE=0 \
GLIA_COMPOSITIONAL=1 N_GPU_LAYERS=99 AUTO_HOLDOUT=0 \
GLIA_DIRECTIVE_LEGACY=1 \
./cycle/run_cycle_loop.sh 1.4-no-channels 14b-q4 2>&1 | tail -20 | tee -a "$LOG"
run_analyzers 1.4-no-channels

#
# Phase 3 — cycle 1.5-no-priors (file_priors + exemplars disabled)
#
phase "Cycle 1.5-no-priors (priors+exemplars disabled, 14B)"
# Move dirs out of the way so the prefix builder finds nothing to inject.
mv /home/ivy/Code/glia/scratch/latent/out/file_priors /home/ivy/Code/glia/scratch/latent/out/file_priors.disabled 2>/dev/null || true
mv /home/ivy/Code/glia/scratch/latent/out/exemplars /home/ivy/Code/glia/scratch/latent/out/exemplars.disabled 2>/dev/null || true
GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_LENS_TRACE=0 \
GLIA_COMPOSITIONAL=1 N_GPU_LAYERS=99 AUTO_HOLDOUT=0 \
./cycle/run_cycle_loop.sh 1.5-no-priors 14b-q4 2>&1 | tail -20 | tee -a "$LOG"
mv /home/ivy/Code/glia/scratch/latent/out/file_priors.disabled /home/ivy/Code/glia/scratch/latent/out/file_priors 2>/dev/null || true
mv /home/ivy/Code/glia/scratch/latent/out/exemplars.disabled /home/ivy/Code/glia/scratch/latent/out/exemplars 2>/dev/null || true
run_analyzers 1.5-no-priors

#
# Phase 4 — 1.6-beam8 on hard instances (14B)
#
phase "1.6-beam8 on hard instances (pytest + sphinx + matplotlib-22835)"
for INST in pytest-dev__pytest-11143 sphinx-doc__sphinx-10325 matplotlib__matplotlib-22835; do
  SPLIT="test"
  rm -rf /home/ivy/Code/glia/scratch/latent/out/inst-${INST}-14b-q4-1.6-beam8
  echo "=== $INST ===" >> "$LOG"
  GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
  GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=8 GLIA_LENS_TRACE=0 \
  GLIA_COMPOSITIONAL=1 N_GPU_LAYERS=99 \
  python3 /home/ivy/Code/glia/scratch/latent/out/run_instance.py \
    --instance-id "$INST" --split "$SPLIT" --model 14b-q4 --tag 1.6-beam8 \
    2>&1 | tail -8 >> "$LOG"
done

#
# Phase 5 — Qwen3-30B-A3B MoE smoke (optional; skip if download too slow)
#
phase "Qwen3-30B-A3B MoE smoke (marsh + pytest only, 2 instances)"
mkdir -p /home/ivy/Models/qwen3-30b-a3b-gguf
if [[ ! -f /home/ivy/Models/qwen3-30b-a3b-gguf/qwen3-30b-a3b-instruct-q4_k_m.gguf ]]; then
  hf download unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF \
    "Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf" \
    --local-dir /home/ivy/Models/qwen3-30b-a3b-gguf 2>&1 | tail -2 | tee -a "$LOG" || true
fi
# Add model alias on the fly via env override if needed. For now smoke skipped
# if path doesn't resolve (avoid taking down auto8h on a download failure).
echo "qwen3-moe smoke deferred — needs model alias added to MODELS dict" >> "$LOG"

#
# Phase 6 — Cross-cycle summary
#
phase "Final cross-cycle comparison"
python3 <<'PY' >> "$LOG" 2>&1
import json, os
from pathlib import Path
CYCLES_DIR = Path("/home/ivy/Code/glia/scratch/lens/cycle")
CYCLES = ["1.1-gpu", "1.2-gpu-14b", "1.3-gpu-32b", "1.4-no-channels", "1.5-no-priors"]
table = {}  # instance -> {cycle: f2p_label}
for c in CYCLES:
    fp = CYCLES_DIR / f"cycle-{c}-results.jsonl"
    if not fp.exists():
        continue
    for line in fp.open():
        o = json.loads(line)
        iid = o["instance_id"]
        f2p = (o.get("f2p") or o.get("apply") or "?")
        # Compact label
        if "PASS-but" in f2p: lab = "P+r"
        elif f2p == "PASS": lab = "PASS"
        elif f2p == "NO-DIFF": lab = "ND"
        elif f2p == "APPLY-FAIL": lab = "AF"
        elif f2p == "NO-RUN" or "NO-RUN" in f2p: lab = "NR"
        elif "FAIL" in f2p: lab = "FAIL"
        else: lab = f2p[:6]
        table.setdefault(iid, {})[c] = lab

print("\n### Cross-cycle PASS matrix\n")
print("| Instance | " + " | ".join(CYCLES) + " |")
print("|" + "---|" * (len(CYCLES) + 1))
for iid in sorted(table.keys()):
    row = " | ".join(table[iid].get(c, "-") for c in CYCLES)
    print(f"| {iid} | {row} |")

print("\n### Counts per cycle\n")
print("| Cycle | PASS | PASS+regr | FAIL | NO-DIFF | NO-RUN | APPLY-FAIL |")
print("|---|---|---|---|---|---|---|")
for c in CYCLES:
    fp = CYCLES_DIR / f"cycle-{c}-results.jsonl"
    if not fp.exists():
        continue
    counts = {"PASS":0, "P+r":0, "FAIL":0, "ND":0, "NR":0, "AF":0}
    for line in fp.open():
        o = json.loads(line)
        f2p = (o.get("f2p") or o.get("apply") or "?")
        if "PASS-but" in f2p: counts["P+r"] += 1
        elif f2p == "PASS": counts["PASS"] += 1
        elif f2p == "NO-DIFF": counts["ND"] += 1
        elif f2p == "APPLY-FAIL": counts["AF"] += 1
        elif f2p == "NO-RUN" or "NO-RUN" in f2p: counts["NR"] += 1
        elif "FAIL" in f2p: counts["FAIL"] += 1
    print(f"| {c} | {counts['PASS']} | {counts['P+r']} | {counts['FAIL']} | {counts['ND']} | {counts['NR']} | {counts['AF']} |")
PY

phase "DONE"
echo "Total wall: $(( ($(date +%s) - START) / 60 )) min" | tee -a "$LOG"
