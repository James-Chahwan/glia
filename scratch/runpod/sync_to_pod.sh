#!/usr/bin/env bash
# Local-side sync to a Runpod pod. Pushes the glia repo + scratch data
# (swe-work + datasets + models) to the pod's /home/ivy/* mirror layout.
#
# Usage:
#   POD_HOST=root@<pod-ip>  POD_PORT=22  ./sync_to_pod.sh
# or:
#   ./sync_to_pod.sh root@<pod-ip>:22
#
# First run: full transfer (~10-15 GB). Subsequent: rsync deltas only,
# fast. Models + swe-work skip on a warm persistent volume.

set -euo pipefail

POD_HOST="${1:-${POD_HOST:-}}"
POD_PORT="${POD_PORT:-22}"
if [[ -z "${POD_HOST}" ]]; then
  echo "usage: POD_HOST=root@<ip> POD_PORT=22 $0    (or: $0 root@<ip>:22)" >&2
  exit 1
fi
# Allow "host:port" inline form.
if [[ "${POD_HOST}" == *:* ]]; then
  POD_PORT="${POD_HOST##*:}"
  POD_HOST="${POD_HOST%:*}"
fi

SSH_OPTS="-p ${POD_PORT} -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30"
RSYNC_SSH="ssh ${SSH_OPTS}"
RSYNC_OPTS="-avz --partial --progress -e \"${RSYNC_SSH}\""

echo "=== sync to ${POD_HOST}:${POD_PORT} ==="

# 1. Glia working tree (skip target/, build artifacts — they're rebuilt on pod).
echo
echo "--- /home/ivy/Code/glia ---"
ssh ${SSH_OPTS} "${POD_HOST}" "mkdir -p /home/ivy/Code"
rsync -avz --partial --progress \
  -e "${RSYNC_SSH}" \
  --exclude 'target/' \
  --exclude '*.gguf' \
  --exclude '.git/objects/pack/*.pack' \
  --exclude 'scratch/latent/out/inst-*' \
  --exclude 'scratch/lens/cycle/*-stderr.log' \
  --exclude 'scratch/3d-viewer/target/' \
  --exclude 'scratch/tui-viewer/target/' \
  /home/ivy/Code/glia/ \
  "${POD_HOST}:/home/ivy/Code/glia/"

# 2. SWE-bench Lite parquets (small, ~3MB total).
echo
echo "--- /home/ivy/Datasets/swe-bench-lite ---"
ssh ${SSH_OPTS} "${POD_HOST}" "mkdir -p /home/ivy/Datasets/swe-bench-lite/data"
rsync -avz --partial --progress \
  -e "${RSYNC_SSH}" \
  /home/ivy/Datasets/swe-bench-lite/data/ \
  "${POD_HOST}:/home/ivy/Datasets/swe-bench-lite/data/"

# 3. Models (~5GB GGUF + ~10MB tokenizer). Slow on first run.
echo
echo "--- /home/ivy/Models/qwen2.5-coder-7b-gguf  (~5GB) ---"
ssh ${SSH_OPTS} "${POD_HOST}" "mkdir -p /home/ivy/Models/qwen2.5-coder-7b-gguf"
rsync -avz --partial --progress \
  -e "${RSYNC_SSH}" \
  /home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q4_k_m.gguf \
  "${POD_HOST}:/home/ivy/Models/qwen2.5-coder-7b-gguf/"

echo
echo "--- /home/ivy/Models/qwen2.5-coder-tokenizer  (~10MB) ---"
ssh ${SSH_OPTS} "${POD_HOST}" "mkdir -p /home/ivy/Models/qwen2.5-coder-tokenizer"
rsync -avz --partial --progress \
  -e "${RSYNC_SSH}" \
  /home/ivy/Models/qwen2.5-coder-tokenizer/ \
  "${POD_HOST}:/home/ivy/Models/qwen2.5-coder-tokenizer/"

# 4. SWE-bench working repos (CRITICAL). The cycle_loop_set 7 repos +
# holdout 10 repos = ~17 repos. Each ~50-500MB checked out.
# Use --include filters to send ONLY the loop-set + holdout-set repos
# (not the full ~/Code/swe-work which may have hundreds).
echo
echo "--- /home/ivy/swe-work  (loop_set + holdout only) ---"
LOOP_INSTANCES=$(python3 -c "
import json
loop = json.load(open('/home/ivy/Code/glia/scratch/lens/manifests/cycle_loop_set.json'))
holdout = json.load(open('/home/ivy/Code/glia/scratch/lens/manifests/holdout.json'))
for x in loop + holdout:
    print(x['instance_id'])
")
ssh ${SSH_OPTS} "${POD_HOST}" "mkdir -p /home/ivy/swe-work"
for INST in ${LOOP_INSTANCES}; do
  if [[ -d /home/ivy/swe-work/${INST} ]]; then
    echo "  -> ${INST}"
    rsync -avz --partial \
      -e "${RSYNC_SSH}" \
      --exclude '__pycache__/' \
      --exclude '*.pyc' \
      --exclude '.git/objects/pack/*.pack' \
      "/home/ivy/swe-work/${INST}/" \
      "${POD_HOST}:/home/ivy/swe-work/${INST}/"
  fi
done

echo
echo "=== sync done ==="
echo "Next on the pod:  bash /home/ivy/Code/glia/scratch/runpod/setup.sh"
