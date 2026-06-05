#!/usr/bin/env bash
# Cycle driver. Iterates the working-set manifest, runs run_instance.py per
# instance, appends per-instance result to results_history.jsonl + cycle log.
#
# usage:
#   cycle/run_cycle.sh <cycle_number> [model]
#
# Defaults: model = 7b-q4
#
# Hard rules enforced here:
#   - Holdout instances cannot appear in working set (asserted at start).
#   - Each instance result gets a `cycle: N` field injected for cross-cycle tracking.
#   - Sequential — no parallel inference (Q4 7B context is too big for parallel on 64GB).
#   - One instance failing does NOT abort the cycle; logged + continue.

set -uo pipefail

CYCLE="${1:?usage: run_cycle.sh <cycle_number> [model]}"
MODEL="${2:-7b-q4}"

LENS_ROOT="/home/ivy/Code/glia/bench/lens"
LATENT_OUT="/home/ivy/Code/glia/bench/latent/out"
WORKING="${LENS_ROOT}/manifests/working_set.json"
HOLDOUT="${LENS_ROOT}/manifests/holdout.json"
RESULTS_HISTORY="${LENS_ROOT}/cycle/results_history.jsonl"
PER_CYCLE_RESULTS="${LENS_ROOT}/cycle/cycle-${CYCLE}-results.jsonl"
CYCLE_LOG="${LENS_ROOT}/cycle/cycle_log.md"

START_TS=$(date -Iseconds)
START_EPOCH=$(date +%s)

echo "[$(date -Iseconds)] cycle ${CYCLE} START · model=${MODEL}" >&2

# Sacred-holdout assertion: working_set and holdout are disjoint by instance_id.
if python3 - <<PY
import json, sys
w = set(x['instance_id'] for x in json.load(open('${WORKING}')))
h = set(x['instance_id'] for x in json.load(open('${HOLDOUT}')))
overlap = w & h
if overlap:
    print(f'HOLDOUT VIOLATION: {sorted(overlap)}', file=sys.stderr)
    sys.exit(1)
print(f'sets OK · working={len(w)} holdout={len(h)} overlap=0', file=sys.stderr)
PY
then : ; else
  echo "[$(date -Iseconds)] cycle ${CYCLE} ABORT: holdout violation" >&2
  exit 2
fi

INSTANCES=$(python3 -c "import json; [print(x['instance_id']) for x in json.load(open('${WORKING}'))]")
N=$(echo "${INSTANCES}" | wc -l)
echo "[$(date -Iseconds)] cycle ${CYCLE} N=${N} instances" >&2

i=0
PASS=0
FAIL=0
ERROR=0
for INST in ${INSTANCES}; do
  i=$((i + 1))
  T0=$(date +%s)
  echo "[$(date -Iseconds)] [${i}/${N}] cycle=${CYCLE} model=${MODEL} ${INST}" >&2
  TAG="cycle-${CYCLE}"
  # run_instance.py appends to bench/latent/out/instance_results.jsonl.
  # We tail off the LAST line and inject cycle/model context.
  PER_INST_RESULTS="${LATENT_OUT}/instance_results.jsonl"
  PRE_LINES=$(wc -l < "${PER_INST_RESULTS}" 2>/dev/null || echo 0)
  # Pick split: try test first, then dev.
  SPLIT="test"
  python3 - <<PY
import pandas as pd
df = pd.read_parquet('/home/ivy/Datasets/swe-bench-lite/data/test-00000-of-00001.parquet')
if '${INST}' not in df.instance_id.values:
    raise SystemExit(1)
PY
  if [[ $? -ne 0 ]]; then SPLIT="dev"; fi

  cd "${LATENT_OUT}"
  if ! python3 run_instance.py --instance-id "${INST}" --split "${SPLIT}" --model "${MODEL}" --tag "${TAG}" >>"${LENS_ROOT}/cycle/cycle-${CYCLE}-stdout.log" 2>>"${LENS_ROOT}/cycle/cycle-${CYCLE}-stderr.log"; then
    EXITC=$?
    T1=$(date +%s)
    echo "[$(date -Iseconds)] [${i}/${N}] ${INST} ERROR (exit ${EXITC}) wall=$((T1 - T0))s" >&2
    ERROR=$((ERROR + 1))
    echo "{\"cycle\": ${CYCLE}, \"instance_id\": \"${INST}\", \"model\": \"${MODEL}\", \"status\": \"error\", \"exit_code\": ${EXITC}, \"wall_s\": $((T1 - T0))}" >> "${PER_CYCLE_RESULTS}"
    echo "{\"cycle\": ${CYCLE}, \"instance_id\": \"${INST}\", \"model\": \"${MODEL}\", \"status\": \"error\", \"exit_code\": ${EXITC}, \"wall_s\": $((T1 - T0))}" >> "${RESULTS_HISTORY}"
    continue
  fi
  T1=$(date +%s)
  POST_LINES=$(wc -l < "${PER_INST_RESULTS}" 2>/dev/null || echo 0)
  if [[ "${POST_LINES}" -gt "${PRE_LINES}" ]]; then
    LAST_LINE=$(tail -n 1 "${PER_INST_RESULTS}")
    # Augment with cycle tag.
    AUGMENTED=$(echo "${LAST_LINE}" | python3 -c "import json,sys; o=json.loads(sys.stdin.read()); o['cycle']=${CYCLE}; o['wall_s_outer']=$((T1 - T0)); print(json.dumps(o))")
    echo "${AUGMENTED}" >> "${PER_CYCLE_RESULTS}"
    echo "${AUGMENTED}" >> "${RESULTS_HISTORY}"
    # Quick PASS/FAIL detection from the JSON.
    F2P=$(echo "${LAST_LINE}" | python3 -c "import json,sys; o=json.loads(sys.stdin.read()); print(o.get('f2p','?'))")
    case "${F2P}" in
      PASS) PASS=$((PASS + 1)); echo "[$(date -Iseconds)] [${i}/${N}] ${INST} PASS  wall=$((T1 - T0))s" >&2 ;;
      *)    FAIL=$((FAIL + 1)); echo "[$(date -Iseconds)] [${i}/${N}] ${INST} ${F2P}  wall=$((T1 - T0))s" >&2 ;;
    esac
  else
    ERROR=$((ERROR + 1))
    echo "[$(date -Iseconds)] [${i}/${N}] ${INST} NO_RESULT_LINE wall=$((T1 - T0))s" >&2
    echo "{\"cycle\": ${CYCLE}, \"instance_id\": \"${INST}\", \"model\": \"${MODEL}\", \"status\": \"no_result_line\", \"wall_s\": $((T1 - T0))}" >> "${PER_CYCLE_RESULTS}"
    echo "{\"cycle\": ${CYCLE}, \"instance_id\": \"${INST}\", \"model\": \"${MODEL}\", \"status\": \"no_result_line\", \"wall_s\": $((T1 - T0))}" >> "${RESULTS_HISTORY}"
  fi
done

END_TS=$(date -Iseconds)
END_EPOCH=$(date +%s)
TOTAL=$((END_EPOCH - START_EPOCH))

echo "[$(date -Iseconds)] cycle ${CYCLE} DONE · pass=${PASS} fail=${FAIL} error=${ERROR} total=${TOTAL}s" >&2

# Append cycle summary block to top of cycle_log.md (insert after the first
# matching header line; awk is the lightest tool that does in-place "insert
# after first matching pattern").
SUMMARY=$(cat <<EOF

## cycle ${CYCLE} — ${MODEL} · ${START_TS} → ${END_TS}

- N=${N} working-set instances
- pass=${PASS} / fail=${FAIL} / error=${ERROR}
- total wall=${TOTAL}s (avg per instance: $((TOTAL / N))s)
- per-cycle results: \`cycle/cycle-${CYCLE}-results.jsonl\`

EOF
)
# Insert after line 1 (the H1 header).
python3 - <<PY
p = '${CYCLE_LOG}'
text = open(p).read()
lines = text.split('\n')
out = []
inserted = False
for ln in lines:
    out.append(ln)
    if not inserted and ln.startswith('# '):
        out.append('''${SUMMARY}''')
        inserted = True
open(p, 'w').write('\n'.join(out))
PY

exit 0
