#!/usr/bin/env bash
# cycle_21_launch.sh — kick cycle 2.1 with all edit-content levers + lens-bias.
#
# This is the validation cycle for the 5+1 levers shipped this session
# (cycles 1.1-2.0 had them implemented but NOT enabled). Compare against
# cycle 2.0 baseline to measure per-lever payoff.
#
# Sequence:
#   1. Wait for cycle 2.0-gpu-14b to finish (poll pgrep)
#   2. Run analyzers on cycle 2.0 (including the NO-RUN-bucket-fix
#      classifier — issue #6 from issues_surfacing_now)
#   3. Kick cycle 2.1 with full lever stack
#   4. Run analyzers on cycle 2.1
#   5. Generate cycle 2.0 vs 2.1 diff table → cycle_2_vs_21_diff.md
#
# Output: cycle/cycle_21_launch.log + cycle-2.1-edit-levers-attn-* files

set -u
LOG=/home/ivy/Code/glia/bench/lens/cycle/cycle_21_launch.log
cd /home/ivy/Code/glia/bench/lens
export PATH="/root/.cargo/bin:/usr/local/cuda/bin:$PATH"

START=$(date +%s)
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
  if [[ -f cycle/cycle-${cycle}-results.jsonl ]]; then
    echo "  --- results ---" >> "$LOG"
    python3 -c "
import json
with open('cycle/cycle-${cycle}-results.jsonl') as f:
    for line in f:
        o = json.loads(line)
        f2p = (o.get('f2p') or o.get('apply') or '?')[:40]
        wall = o.get('wall_s_outer') or o.get('wall_s', 0)
        print(f'    {o[\"instance_id\"]:50}  {f2p:42}  wall={int(wall):5}s')
" >> "$LOG"
  fi
}

cat > "$LOG" <<EOF
# cycle 2.1 launch — edit-content levers + lens-bias validation
Started: $(date)

## Plan
- Wait for cycle 2.0-gpu-14b (in flight)
- Re-classify 2.0 results with NO-RUN bucket fix
- Cycle 2.1: GLIA_PLAN_THEN_EDIT + GLIA_BEHAVIORAL_TARGET + GLIA_SKIP_VALIDATOR_PASS + GLIA_ATTN_INJECTION + GLIA_EXEMPLAR_FULL_HUNK + GLIA_NORMALIZE_DIFF
- Compare 2.0 vs 2.1 PASS counts + failure-mode shifts

EOF

phase "Wait for cycle 2.0-gpu-14b"
while pgrep -f "run_cycle_loop.sh 2.0-gpu-14b" >/dev/null 2>&1; do
  sleep 60
done
echo "cycle 2.0 closed" | tee -a "$LOG"

phase "Re-classify cycle 2.0 with NO-RUN bucket fix"
run_analyzers 2.0-gpu-14b

phase "Cycle 2.1 launch — full lever stack"
GLIA_TWO_PASS=1 \
GLIA_SAGE_RUNTIME=1 \
GLIA_RUNTIME_EVIDENCE=1 \
GLIA_PER_TOKEN_POOL=1 \
GLIA_SAMPLES=3 \
GLIA_LENS_TRACE=0 \
GLIA_COMPOSITIONAL=1 \
GLIA_NORMALIZE_DIFF=1 \
GLIA_EXEMPLAR_FULL_HUNK=1 \
GLIA_PLAN_THEN_EDIT=1 \
GLIA_BEHAVIORAL_TARGET=1 \
GLIA_SKIP_VALIDATOR_PASS=1 \
GLIA_ATTN_INJECTION=1 \
GLIA_ATTN_ALPHA=0.3 \
N_GPU_LAYERS=99 \
AUTO_HOLDOUT=0 \
./cycle/run_cycle_loop.sh 2.1-edit-levers-attn 14b-q4 2>&1 | tail -20 | tee -a "$LOG"

phase "Re-classify cycle 2.1"
run_analyzers 2.1-edit-levers-attn

phase "cycle 2.0 vs 2.1 comparison"
python3 <<'PY' >> "$LOG"
import json
from pathlib import Path
DIR = Path("/home/ivy/Code/glia/bench/lens/cycle")

def load(c):
    fp = DIR / f"cycle-{c}-results.jsonl"
    if not fp.exists():
        return {}
    out = {}
    for line in fp.open():
        o = json.loads(line)
        out[o["instance_id"]] = o
    return out

c20 = load("2.0-gpu-14b")
c21 = load("2.1-edit-levers-attn")
ids = sorted(set(c20) | set(c21))

print("### cycle 2.0 vs cycle 2.1 — per-instance verdict change")
print()
print("| Instance | 2.0 | 2.1 | Δ |")
print("|---|---|---|---|")
def label(o):
    if not o:
        return "—"
    f = o.get("f2p") or o.get("apply", "?")
    if "PASS-but" in f: return "P+r"
    if f == "PASS": return "PASS"
    if "NO-DIFF" in f: return "ND"
    if "NO-RUN" in f: return "NR"
    if "APPLY-FAIL" in f or f == "APPLY-FAIL": return "AF"
    if "FAIL" in f: return "FAIL"
    return f[:6]

for iid in ids:
    a, b = label(c20.get(iid)), label(c21.get(iid))
    arrow = "→" if a != b else "="
    print(f"| {iid} | {a} | {b} | {arrow} |")

print()
print("### Aggregate")
def count(d):
    c = {"PASS":0, "P+r":0, "FAIL":0, "ND":0, "NR":0, "AF":0}
    for o in d.values():
        l = label(o)
        c[l] = c.get(l, 0) + 1
    return c
c20c = count(c20); c21c = count(c21)
print(f"  2.0: {c20c}")
print(f"  2.1: {c21c}")
PY

#
# Phase 4 — queue follow-up cycles to keep GPU busy
#
phase "Cycle 2.2 — alpha sweep on marshmallow + 2 hard instances (lens-bias only)"
# Test the attention-bias lever in isolation across alpha {0.1, 0.3, 0.5,
# 1.0}. Run on marshmallow (stable PASS for reference) + pytest + sphinx
# (the two FAIL-but-applied instances where attention bias might unstick
# edit-content). 3 instances × 4 alphas = 12 inference runs.
for ALPHA in 0.1 0.3 0.5 1.0; do
  for INST in marshmallow-code__marshmallow-1359 pytest-dev__pytest-11143 sphinx-doc__sphinx-10325; do
    SPLIT="dev"
    [[ "$INST" == "marshmallow-code__marshmallow-1359" ]] && SPLIT="dev" || SPLIT="test"
    TAG="2.2-alpha-${ALPHA}"
    rm -rf /home/ivy/Code/glia/bench/latent/out/inst-${INST}-14b-q4-${TAG} 2>/dev/null
    echo "=== alpha=${ALPHA} inst=${INST} ==="
    GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
    GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_COMPOSITIONAL=1 \
    GLIA_NORMALIZE_DIFF=1 GLIA_EXEMPLAR_FULL_HUNK=1 \
    GLIA_ATTN_INJECTION=1 GLIA_ATTN_ALPHA="$ALPHA" \
    N_GPU_LAYERS=99 \
    python3 /home/ivy/Code/glia/bench/latent/out/run_instance.py \
      --instance-id "$INST" --split "$SPLIT" --model 14b-q4 --tag "$TAG" 2>&1 | tail -3 >> "$LOG"
  done
done

phase "Cycle 2.3 — lens-bias smoke (alpha sweep on prefix/suffix only, no full pipeline)"
bash /home/ivy/Code/glia/bench/lens/cycle/lens_bias_smoke.sh 2>&1 | tail -20 >> "$LOG"

phase "DONE"
echo "Total wall: $(( ($(date +%s) - START) / 60 )) min" | tee -a "$LOG"
