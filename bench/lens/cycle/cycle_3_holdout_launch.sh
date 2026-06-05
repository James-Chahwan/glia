#!/usr/bin/env bash
# cycle_3_holdout_launch.sh — generalization test on the 10-instance holdout.
#
# Pod has been idle since cycle 2.2 ended. Time to test whether the validated
# lever stack (plan-then-edit prose + behavioral target + skip-validator +
# sage-runtime + substrate pins) generalizes to instances NOT in cycle_loop_set.
#
# Holdout (10 instances, all cloned + ready):
#   pydata__xarray-4094 + xarray-4248
#   pallets__flask-4045 + flask-4992
#   astropy__astropy-14995
#   django__django-16139
#   matplotlib__matplotlib-25498
#   pytest-dev__pytest-7373
#   psf__requests-3362
#   sympy__sympy-12454
#
# Levers active (matches cycle 2.1 / 2.2 winning config):
#   GLIA_PLAN_THEN_EDIT=1, GLIA_BEHAVIORAL_TARGET=1, GLIA_SKIP_VALIDATOR_PASS=1
#   GLIA_NORMALIZE_DIFF=1, GLIA_EXEMPLAR_FULL_HUNK=1
#   GLIA_TWO_PASS=1, GLIA_SAGE_RUNTIME=1, GLIA_RUNTIME_EVIDENCE=1
#   GLIA_PER_TOKEN_POOL=1, GLIA_SAMPLES=3, GLIA_COMPOSITIONAL=1
#   N_GPU_LAYERS=99
#
# Lens-bias DISABLED — SIGABRTs at runtime; would just waste GPU.
#
# Estimated wall: ~70-90 min (10 instances × ~7-9 min average — varies by
# repo size + whether sage-runtime fires).
#
# Output: cycle-3.0-holdout-* artifacts + cycle_3_holdout_launch.log

set -u
LOG=/home/ivy/Code/glia/bench/lens/cycle/cycle_3_holdout_launch.log
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

cat > "$LOG" <<EOF
# Cycle 3.0 holdout — generalization test
Started: $(date)

## Lever stack (validated cycle 2.1/2.2 config)
- GLIA_PLAN_THEN_EDIT=1 (prose-framed suffix)
- GLIA_BEHAVIORAL_TARGET=1
- GLIA_SKIP_VALIDATOR_PASS=1
- GLIA_NORMALIZE_DIFF=1
- GLIA_EXEMPLAR_FULL_HUNK=1
- A2 protected qnames (auto from directive)
- Baseline: GLIA_TWO_PASS=1, GLIA_SAGE_RUNTIME=1, GLIA_RUNTIME_EVIDENCE=1
- GLIA_PER_TOKEN_POOL=1, GLIA_SAMPLES=3, GLIA_COMPOSITIONAL=1
- N_GPU_LAYERS=99
- Lens-bias DISABLED (SIGABRT runtime issue)

## Instances (10 holdout, all pre-cloned)
$(python3 -c "import json; [print(f'- {x[\"instance_id\"]}') for x in json.load(open('/home/ivy/Code/glia/bench/lens/manifests/holdout.json'))]")

EOF

phase "Cycle 3.0 holdout"
for INST in $(python3 -c "import json; [print(x['instance_id']) for x in json.load(open('/home/ivy/Code/glia/bench/lens/manifests/holdout.json'))]"); do
  # Determine split — most are test, some may be dev
  SPLIT="test"
  if python3 -c "
import pandas as pd, sys
df = pd.read_parquet('/home/ivy/Datasets/swe-bench-lite/data/dev-00000-of-00001.parquet')
sys.exit(0 if '$INST' in df.instance_id.values else 1)
"; then
    SPLIT="dev"
  fi
  rm -rf /home/ivy/Code/glia/bench/latent/out/inst-${INST}-14b-q4-3.0-holdout 2>/dev/null
  echo "=== ${INST} (split=${SPLIT}) ===" | tee -a "$LOG"
  time GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
       GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_LENS_TRACE=0 \
       GLIA_COMPOSITIONAL=1 GLIA_NORMALIZE_DIFF=1 GLIA_EXEMPLAR_FULL_HUNK=1 \
       GLIA_PLAN_THEN_EDIT=1 GLIA_BEHAVIORAL_TARGET=1 \
       GLIA_SKIP_VALIDATOR_PASS=1 \
       N_GPU_LAYERS=99 \
   python3 /home/ivy/Code/glia/bench/latent/out/run_instance.py \
     --instance-id "$INST" --split "$SPLIT" --model 14b-q4 --tag 3.0-holdout 2>&1 | tail -6 | tee -a "$LOG"
done

phase "Cycle 3.0 analyzers"
for tool in failure_mode_classifier channel_attribution gold_patch_oracle solution_curve; do
  echo "--- $tool ---" >> "$LOG"
  python3 scripts/$tool.py --cycle 3.0-holdout 2>&1 | tail -8 >> "$LOG"
done

phase "Cycle 3.0 results table"
if [[ -f cycle/cycle-3.0-holdout-results.jsonl ]]; then
  python3 <<'PY' >> "$LOG"
import json
n_pass = n_pr = n_fail = n_nd = n_nr = n_af = 0
print("\n### Per-instance results\n")
print("| Instance | Wall | Result |")
print("|---|---|---|")
with open("cycle/cycle-3.0-holdout-results.jsonl") as f:
    for line in f:
        o = json.loads(line)
        iid = o["instance_id"]
        f2p = (o.get("f2p") or o.get("apply") or "?")[:40]
        wall = int(o.get("wall_s_outer") or o.get("wall_s", 0))
        print(f"| {iid} | {wall}s | {f2p} |")
        if "PASS-but" in f2p: n_pr += 1
        elif f2p == "PASS": n_pass += 1
        elif "NO-DIFF" in f2p: n_nd += 1
        elif "NO-RUN" in f2p: n_nr += 1
        elif "APPLY-FAIL" in f2p: n_af += 1
        elif "FAIL" in f2p: n_fail += 1
print()
print(f"### Aggregate")
print(f"- PASS strict: {n_pass}")
print(f"- PASS+regr: {n_pr}")
print(f"- FAIL: {n_fail}")
print(f"- NO-DIFF: {n_nd}")
print(f"- NO-RUN: {n_nr}")
print(f"- APPLY-FAIL: {n_af}")
print(f"- TOTAL: {n_pass + n_pr + n_fail + n_nd + n_nr + n_af}")
PY
fi

phase "DONE"
echo "Total wall: $(( ($(date +%s) - START) / 60 )) min" | tee -a "$LOG"
