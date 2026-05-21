#!/usr/bin/env bash
# Phase-gate smoke harness — runs the 2 historically-PASSing instances
# (marshmallow + django) and, if both PASS, runs the smallest-by-past-wall
# third instance as a stretch check. Used between every phase of the
# spike plan as a regression gate before unlocking the next phase.
#
# Why marshmallow + django: marshmallow is the only sentinel PASS across
# cycles 0.3-0.9; django flipped to PASS-but-1-regressions in cycle 0.9
# (sage validator pass-2 effect). Together they're the working baseline.
#
# Stretch instance choice when both pass: smallest-wall instance from the
# 5 historical-FAILs. From cycle 0.9 walls: sklearn-10508 (388s) <
# sphinx-10325 (452s) < matplotlib-22835 (508s) < pytest-11143 (642s) <
# matplotlib-22711 (852s). Default stretch = sklearn-10508.
#
# Usage:
#   cycle/run_smoke.sh <phase_tag> [model=7b-q4] [stretch=auto|off|<instance_id>]
# Example:
#   cycle/run_smoke.sh phase1 7b-q4
#   cycle/run_smoke.sh phase2 7b-q4 off            # skip stretch even if both PASS
#   cycle/run_smoke.sh phase3 7b-q4 sphinx-doc__sphinx-10325

set -uo pipefail

PHASE="${1:?usage: run_smoke.sh <phase_tag> [model] [stretch]}"
MODEL="${2:-7b-q4}"
STRETCH="${3:-auto}"

LENS_ROOT="/home/ivy/Code/glia/scratch/lens"
LATENT_OUT="/home/ivy/Code/glia/scratch/latent/out"

SMOKE_RESULTS="${LENS_ROOT}/cycle/smoke-${PHASE}-results.jsonl"
SMOKE_STDERR="${LENS_ROOT}/cycle/smoke-${PHASE}-stderr.log"
SMOKE_MD="${LENS_ROOT}/cycle/smoke-${PHASE}-results.md"

# Append-only logs — never overwrite a prior phase's smoke output.
mkdir -p "$(dirname "${SMOKE_RESULTS}")"
: > /tmp/_smoke_phase_${PHASE}.tmp

START_TS=$(date -Iseconds)
START_EPOCH=$(date +%s)
echo "[$(date -Iseconds)] smoke ${PHASE} START · model=${MODEL} stretch=${STRETCH}" | tee -a "${SMOKE_STDERR}"

declare -a INSTANCES=(
  "marshmallow-code__marshmallow-1359"
  "django__django-10914"
)

run_one() {
  local INST="$1"
  local I="$2"
  local N="$3"
  local T0=$(date +%s)
  echo "[$(date -Iseconds)] [${I}/${N}] smoke=${PHASE} ${INST}" | tee -a "${SMOKE_STDERR}"

  cd "${LATENT_OUT}"
  local SPLIT="test"
  if ! python3 -c "import pandas as pd, sys; df = pd.read_parquet('/home/ivy/Datasets/swe-bench-lite/data/test-00000-of-00001.parquet'); sys.exit(0 if '${INST}' in df.instance_id.values else 1)"; then
    SPLIT="dev"
  fi

  local PER_INST_RESULTS="${LATENT_OUT}/instance_results.jsonl"
  local PRE_LINES
  PRE_LINES=$(wc -l < "${PER_INST_RESULTS}" 2>/dev/null || echo 0)

  if ! python3 run_instance.py --instance-id "${INST}" --split "${SPLIT}" --model "${MODEL}" --tag "smoke-${PHASE}" >>"${SMOKE_STDERR}" 2>&1; then
    local EXITC=$?
    local T1=$(date +%s)
    echo "[$(date -Iseconds)] [${I}/${N}] ${INST} ERROR (exit ${EXITC}) wall=$((T1 - T0))s" | tee -a "${SMOKE_STDERR}"
    echo "{\"phase\": \"${PHASE}\", \"instance_id\": \"${INST}\", \"model\": \"${MODEL}\", \"status\": \"error\", \"exit_code\": ${EXITC}, \"wall_s\": $((T1 - T0))}" >> "${SMOKE_RESULTS}"
    echo "ERROR" >> /tmp/_smoke_phase_${PHASE}.tmp
    return
  fi

  local T1=$(date +%s)
  local POST_LINES
  POST_LINES=$(wc -l < "${PER_INST_RESULTS}" 2>/dev/null || echo 0)
  if [[ "${POST_LINES}" -gt "${PRE_LINES}" ]]; then
    local LAST_LINE
    LAST_LINE=$(tail -n 1 "${PER_INST_RESULTS}")
    local AUGMENTED
    AUGMENTED=$(echo "${LAST_LINE}" | python3 -c "import json,sys; o=json.loads(sys.stdin.read()); o['phase']='${PHASE}'; o['wall_s_outer']=$((T1 - T0)); print(json.dumps(o))")
    echo "${AUGMENTED}" >> "${SMOKE_RESULTS}"
    local F2P
    F2P=$(echo "${LAST_LINE}" | python3 -c "import json,sys; o=json.loads(sys.stdin.read()); print(o.get('f2p','?'))")
    case "${F2P}" in
      PASS) echo "PASS" >> /tmp/_smoke_phase_${PHASE}.tmp; echo "[$(date -Iseconds)] [${I}/${N}] ${INST} PASS wall=$((T1 - T0))s" | tee -a "${SMOKE_STDERR}";;
      PASS-but-*) echo "PASS-WITH-REGR" >> /tmp/_smoke_phase_${PHASE}.tmp; echo "[$(date -Iseconds)] [${I}/${N}] ${INST} PASS-but-regressions wall=$((T1 - T0))s" | tee -a "${SMOKE_STDERR}";;
      *) echo "FAIL" >> /tmp/_smoke_phase_${PHASE}.tmp; echo "[$(date -Iseconds)] [${I}/${N}] ${INST} FAIL (f2p=${F2P}) wall=$((T1 - T0))s" | tee -a "${SMOKE_STDERR}";;
    esac
  else
    echo "NO-RESULT" >> /tmp/_smoke_phase_${PHASE}.tmp
    echo "[$(date -Iseconds)] [${I}/${N}] ${INST} NO_RESULT_LINE wall=$((T1 - T0))s" | tee -a "${SMOKE_STDERR}"
  fi
}

# Phase 1: required-two
i=0
for INST in "${INSTANCES[@]}"; do
  i=$((i + 1))
  run_one "${INST}" "${i}" "2+stretch"
done

# Stretch decision: count clean PASSes from required-two.
CLEAN_PASS=$(grep -c '^PASS$' /tmp/_smoke_phase_${PHASE}.tmp || true)

STRETCH_ID=""
case "${STRETCH}" in
  off) :;;
  auto)
    # Auto: only run stretch if both required-two were clean PASS.
    if [[ "${CLEAN_PASS}" -ge 2 ]]; then
      STRETCH_ID="scikit-learn__scikit-learn-10508"
    fi
    ;;
  *) STRETCH_ID="${STRETCH}";;
esac

if [[ -n "${STRETCH_ID}" ]]; then
  echo "[$(date -Iseconds)] smoke ${PHASE} stretch=${STRETCH_ID}" | tee -a "${SMOKE_STDERR}"
  run_one "${STRETCH_ID}" "3" "3"
else
  if [[ "${STRETCH}" == "auto" ]]; then
    echo "[$(date -Iseconds)] smoke ${PHASE} stretch skipped (clean_pass=${CLEAN_PASS} < 2)" | tee -a "${SMOKE_STDERR}"
  fi
fi

END_EPOCH=$(date +%s)
TOTAL=$((END_EPOCH - START_EPOCH))

# Final counts (re-scan tmp).
P=$(grep -c '^PASS$' /tmp/_smoke_phase_${PHASE}.tmp || true)
PR=$(grep -c '^PASS-WITH-REGR$' /tmp/_smoke_phase_${PHASE}.tmp || true)
F=$(grep -c '^FAIL$' /tmp/_smoke_phase_${PHASE}.tmp || true)
E=$(grep -c '^ERROR$' /tmp/_smoke_phase_${PHASE}.tmp || true)
NR=$(grep -c '^NO-RESULT$' /tmp/_smoke_phase_${PHASE}.tmp || true)

echo "[$(date -Iseconds)] smoke ${PHASE} DONE · clean_pass=${P} pass_with_regr=${PR} fail=${F} error=${E} no_result=${NR} total=${TOTAL}s" | tee -a "${SMOKE_STDERR}"

# Append a structured per-phase summary to the smoke markdown.
{
  echo ""
  echo "## smoke ${PHASE} — $(date -Iseconds)"
  echo ""
  echo "- model: \`${MODEL}\`"
  echo "- duration: ${TOTAL}s"
  echo "- clean PASS: ${P} / 2 (required) + ${STRETCH_ID:+1 stretch}"
  echo "- PASS-with-regressions: ${PR}"
  echo "- FAIL: ${F}  ·  ERROR: ${E}  ·  NO-RESULT: ${NR}"
  echo ""
  echo "Results JSONL: \`smoke-${PHASE}-results.jsonl\`"
  echo "Stderr log: \`smoke-${PHASE}-stderr.log\`"
} >> "${SMOKE_MD}"

# Cleanup tmp.
rm -f /tmp/_smoke_phase_${PHASE}.tmp

# Exit nonzero if marshmallow regressed (the sentinel) — caller can detect.
if grep -q '"instance_id": "marshmallow-code__marshmallow-1359".*"f2p": "PASS"' "${SMOKE_RESULTS}"; then
  exit 0
fi
exit 3  # marshmallow regression — gate failed
