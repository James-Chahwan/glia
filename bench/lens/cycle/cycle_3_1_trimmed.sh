#!/usr/bin/env bash
# cycle_3_1_trimmed.sh — fast iteration re-run of all 10 holdout instances.
#
# Cycle 3.0 took 2h36 (5-31min/instance) with full stack at 0 PASS / 9 done.
# Cycle 3.1 cuts the wall multiplier:
#   - GLIA_SAMPLES=1 (was 3): 3× faster, no beam ensemble
#   - GLIA_TWO_PASS=0 (was 1): drop validator pass-2 (still does sage-runtime)
#   - Drops GLIA_EXEMPLAR_FULL_HUNK and GLIA_BEHAVIORAL_TARGET (minor)
# Keeps:
#   - GLIA_PLAN_THEN_EDIT=1 (the load-bearing lever for targeting)
#   - GLIA_SAGE_RUNTIME=1 (high-value retry on FAIL/APPLY-FAIL)
#   - GLIA_RUNTIME_EVIDENCE=1
#   - GLIA_PER_TOKEN_POOL=1 (graph context)
#   - GLIA_COMPOSITIONAL=1 (per-hunk fallback)
#   - GLIA_NORMALIZE_DIFF=1
#   - GLIA_SKIP_VALIDATOR_PASS=1
#   - File_priors hint in plan prompt + A2 broader regex + ast.parse check
#     (cycle 3.0 fixes still active)
#
# NEW this cycle:
#   - Stale-state cleanup: `git reset --hard $base_commit` (cycle 3.0 holdout
#     flask-4992 TESTPATCH-FAIL fix)
#
# Goal: confirm if the trimmed stack shifts any failure mode vs cycle 3.0.
# Expect: ~7-12 min/instance × 10 = 70-120 min wall.

set -u
LOG=/home/ivy/Code/glia/bench/lens/cycle/cycle_3_1_trimmed.log
cd /home/ivy/Code/glia/bench/lens
export PATH="/root/.cargo/bin:/usr/local/cuda/bin:$PATH"

START=$(date +%s)

cat > "$LOG" <<EOF
# Cycle 3.1 — trimmed-stack holdout re-run
Started: $(date)

## Trimmed stack
- GLIA_SAMPLES=1 (was 3 in cycle 3.0)
- GLIA_TWO_PASS=0 (validator pass-2 OFF)
- GLIA_PLAN_THEN_EDIT=1
- GLIA_SAGE_RUNTIME=1 + GLIA_RUNTIME_EVIDENCE=1
- GLIA_PER_TOKEN_POOL=1
- GLIA_COMPOSITIONAL=1
- GLIA_NORMALIZE_DIFF=1
- GLIA_SKIP_VALIDATOR_PASS=1
- N_GPU_LAYERS=99

## New fix
- run_instance.py uses `git reset --hard \$base_commit` (cleaner state)

EOF

for INST in $(python3 -c "import json; [print(x['instance_id']) for x in json.load(open('/home/ivy/Code/glia/bench/lens/manifests/holdout.json'))]"); do
  SPLIT="test"
  if python3 -c "
import pandas as pd, sys
df = pd.read_parquet('/home/ivy/Datasets/swe-bench-lite/data/dev-00000-of-00001.parquet')
sys.exit(0 if '$INST' in df.instance_id.values else 1)
"; then
    SPLIT="dev"
  fi
  rm -rf /home/ivy/Code/glia/bench/latent/out/inst-${INST}-14b-q4-3.1-trim 2>/dev/null
  echo "=== ${INST} (split=${SPLIT}) ===" | tee -a "$LOG"
  T0=$(date +%s)
  GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
       GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=1 GLIA_LENS_TRACE=0 \
       GLIA_COMPOSITIONAL=1 GLIA_NORMALIZE_DIFF=1 \
       GLIA_PLAN_THEN_EDIT=1 \
       GLIA_SKIP_VALIDATOR_PASS=1 \
       N_GPU_LAYERS=99 \
   python3 /home/ivy/Code/glia/bench/latent/out/run_instance.py \
     --instance-id "$INST" --split "$SPLIT" --model 14b-q4 --tag 3.1-trim 2>&1 | tail -6 | tee -a "$LOG"
  T1=$(date +%s)
  echo "wall: $((T1-T0))s" | tee -a "$LOG"
done

echo "" >> "$LOG"
echo "## Results table" >> "$LOG"
if [[ -f cycle/cycle-3.1-trim-results.jsonl ]]; then
  python3 <<'PY' >> "$LOG"
import json
n_pass = n_pr = n_fail = n_nd = n_nr = n_af = n_tp = 0
print("\n| Instance | Wall | Apply | F2P |")
print("|---|---|---|---|")
with open("cycle/cycle-3.1-trim-results.jsonl") as f:
    for line in f:
        o = json.loads(line)
        iid = o["instance_id"]
        f2p = (o.get("f2p") or "")[:40]
        apply = o.get("apply", "?")
        wall = int(o.get("wall_s_outer") or o.get("wall_s", 0))
        print(f"| {iid} | {wall}s | {apply} | {f2p} |")
        if "PASS-but" in f2p: n_pr += 1
        elif f2p == "PASS": n_pass += 1
        elif "NO-DIFF" in f2p: n_nd += 1
        elif "NO-RUN" in f2p: n_nr += 1
        elif apply == "APPLY-FAIL": n_af += 1
        elif apply == "TESTPATCH-FAIL": n_tp += 1
        elif "FAIL" in f2p: n_fail += 1
print()
print(f"PASS strict: {n_pass} | PASS+regr: {n_pr} | FAIL: {n_fail}")
print(f"NO-DIFF: {n_nd} | NO-RUN: {n_nr} | APPLY-FAIL: {n_af} | TESTPATCH-FAIL: {n_tp}")
PY
fi

echo "" >> "$LOG"
echo "Total wall: $(( ($(date +%s) - START) / 60 )) min" | tee -a "$LOG"
