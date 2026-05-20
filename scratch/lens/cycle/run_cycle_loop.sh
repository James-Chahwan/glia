#!/usr/bin/env bash
# Cycle 0.X driver: runs the 7-instance fixed loop set + appends results
# to cycle/results_history.jsonl with a `cycle` tag.
#
# usage:
#   cycle/run_cycle_loop.sh <cycle_tag> [model]
# example:
#   cycle/run_cycle_loop.sh 0.3 7b-q4
#
# The 7-instance loop set is at scratch/lens/manifests/cycle_loop_set.json.
# marshmallow is the regression sentinel — first in the list. Others
# follow in fixed order so per-cycle diffs are stable.

set -uo pipefail

CYCLE="${1:?usage: run_cycle_loop.sh <cycle_tag> [model]}"
MODEL="${2:-7b-q4}"

LENS_ROOT="/home/ivy/Code/glia/scratch/lens"
LATENT_OUT="/home/ivy/Code/glia/scratch/latent/out"
LOOP_SET="${LENS_ROOT}/manifests/cycle_loop_set.json"
HOLDOUT="${LENS_ROOT}/manifests/holdout.json"
RESULTS_HISTORY="${LENS_ROOT}/cycle/results_history.jsonl"
PER_CYCLE_RESULTS="${LENS_ROOT}/cycle/cycle-${CYCLE}-results.jsonl"
STDERR_LOG="${LENS_ROOT}/cycle/cycle-${CYCLE}-stderr.log"

START_TS=$(date -Iseconds)
START_EPOCH=$(date +%s)

echo "[$(date -Iseconds)] cycle ${CYCLE} START · model=${MODEL}" | tee -a "${STDERR_LOG}"

# Holdout-violation assertion: loop set must not intersect holdout.
if ! python3 - <<PY
import json, sys
l = set(x['instance_id'] for x in json.load(open('${LOOP_SET}')))
h = set(x['instance_id'] for x in json.load(open('${HOLDOUT}')))
overlap = l & h
if overlap:
    print(f'HOLDOUT VIOLATION: {sorted(overlap)}', file=sys.stderr)
    sys.exit(1)
print(f'sets OK · loop={len(l)} holdout={len(h)} overlap=0', file=sys.stderr)
PY
then
  echo "[$(date -Iseconds)] cycle ${CYCLE} ABORT: holdout violation" | tee -a "${STDERR_LOG}"
  exit 2
fi

INSTANCES=$(python3 -c "import json; [print(x['instance_id']) for x in json.load(open('${LOOP_SET}'))]")
N=$(echo "${INSTANCES}" | wc -l)
echo "[$(date -Iseconds)] cycle ${CYCLE} N=${N} instances" | tee -a "${STDERR_LOG}"

i=0
PASS=0
FAIL=0
ERROR=0
for INST in ${INSTANCES}; do
  i=$((i + 1))
  T0=$(date +%s)
  echo "[$(date -Iseconds)] [${i}/${N}] cycle=${CYCLE} model=${MODEL} ${INST}" | tee -a "${STDERR_LOG}"
  TAG="cycle-${CYCLE}"
  # Determine split by trying test first.
  cd "${LATENT_OUT}"
  SPLIT="test"
  python3 - <<PY
import pandas as pd, sys
df = pd.read_parquet('/home/ivy/Datasets/swe-bench-lite/data/test-00000-of-00001.parquet')
sys.exit(0 if '${INST}' in df.instance_id.values else 1)
PY
  if [[ $? -ne 0 ]]; then SPLIT="dev"; fi

  PER_INST_RESULTS="${LATENT_OUT}/instance_results.jsonl"
  PRE_LINES=$(wc -l < "${PER_INST_RESULTS}" 2>/dev/null || echo 0)

  if ! python3 run_instance.py --instance-id "${INST}" --split "${SPLIT}" --model "${MODEL}" --tag "${TAG}" >>"${STDERR_LOG}" 2>&1; then
    EXITC=$?
    T1=$(date +%s)
    echo "[$(date -Iseconds)] [${i}/${N}] ${INST} ERROR (exit ${EXITC}) wall=$((T1 - T0))s" | tee -a "${STDERR_LOG}"
    ERROR=$((ERROR + 1))
    echo "{\"cycle\": \"${CYCLE}\", \"instance_id\": \"${INST}\", \"model\": \"${MODEL}\", \"status\": \"error\", \"exit_code\": ${EXITC}, \"wall_s\": $((T1 - T0))}" | tee -a "${PER_CYCLE_RESULTS}" >> "${RESULTS_HISTORY}"
    continue
  fi
  T1=$(date +%s)
  POST_LINES=$(wc -l < "${PER_INST_RESULTS}" 2>/dev/null || echo 0)
  if [[ "${POST_LINES}" -gt "${PRE_LINES}" ]]; then
    LAST_LINE=$(tail -n 1 "${PER_INST_RESULTS}")
    AUGMENTED=$(echo "${LAST_LINE}" | python3 -c "import json,sys; o=json.loads(sys.stdin.read()); o['cycle']='${CYCLE}'; o['wall_s_outer']=$((T1 - T0)); print(json.dumps(o))")
    echo "${AUGMENTED}" | tee -a "${PER_CYCLE_RESULTS}" >> "${RESULTS_HISTORY}"
    F2P=$(echo "${LAST_LINE}" | python3 -c "import json,sys; o=json.loads(sys.stdin.read()); print(o.get('f2p','?'))")
    case "${F2P}" in
      PASS|*"PASS"*) PASS=$((PASS + 1)); STATUS=PASS ;;
      *) FAIL=$((FAIL + 1)); STATUS=FAIL ;;
    esac
    echo "[$(date -Iseconds)] [${i}/${N}] ${INST} ${STATUS} wall=$((T1 - T0))s" | tee -a "${STDERR_LOG}"
  else
    ERROR=$((ERROR + 1))
    echo "[$(date -Iseconds)] [${i}/${N}] ${INST} NO_RESULT_LINE wall=$((T1 - T0))s" | tee -a "${STDERR_LOG}"
  fi
done

END_TS=$(date -Iseconds)
TOTAL=$(($(date +%s) - START_EPOCH))

echo "[$(date -Iseconds)] cycle ${CYCLE} DONE · pass=${PASS} fail=${FAIL} error=${ERROR} total=${TOTAL}s" | tee -a "${STDERR_LOG}"

# Append cycle summary to cycle_log.md (insert after H1).
CYCLE_LOG="${LENS_ROOT}/cycle/cycle_log.md"
python3 - <<PY
p = '${CYCLE_LOG}'
text = open(p).read()
lines = text.split('\n')
summary = '''
## cycle ${CYCLE} — ${MODEL} · ${START_TS} → ${END_TS}

- N=${N} loop-set instances
- pass=${PASS} / fail=${FAIL} / error=${ERROR}
- total wall=${TOTAL}s
- per-cycle results: cycle/cycle-${CYCLE}-results.jsonl
'''
out = []
inserted = False
for ln in lines:
    out.append(ln)
    if not inserted and ln.startswith('# '):
        out.append(summary)
        inserted = True
open(p, 'w').write('\n'.join(out))
PY
