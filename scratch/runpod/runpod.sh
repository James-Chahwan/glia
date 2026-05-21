#!/usr/bin/env bash
# runpod.sh — one-shot end-to-end deployer.
# Wraps sync_to_pod.sh + remote setup.sh + optional mount.sh + optional smoke launch.
#
# Usage:
#   POD_HOST=root@<ip>  POD_PORT=22                                  \
#   ACTIONS=sync,setup,mount,smoke                                    \
#     ./runpod.sh
#
# Or terse:
#   ./runpod.sh root@<ip>:22                  # default ACTIONS=sync,setup
#   ACTIONS=sync,setup,smoke ./runpod.sh root@<ip>:22
#
# ACTIONS comma-separated, run in order:
#   sync       laptop → pod rsync (code, parquets, GGUF, swe-work loop+holdout)
#   setup      run setup.sh on the pod (rust + python deps + cargo builds)
#   mount      local SSHFS mount at /mnt/runpod
#   smoke      kick smoke-allfeat-gpu (marshmallow + django) on the pod
#
# Env knobs:
#   POD_HOST   ssh target, e.g. root@<ip>
#   POD_PORT   ssh port (default 22)
#   ACTIONS    comma-separated phases (default: sync,setup)
#   SMOKE_TAG  override the smoke run's --tag (default: allfeat-gpu)

set -euo pipefail

POD_ARG="${1:-${POD_HOST:-}}"
POD_PORT="${POD_PORT:-22}"
if [[ -z "${POD_ARG}" ]]; then
  echo "usage: POD_HOST=root@<ip> POD_PORT=22 ACTIONS=sync,setup,smoke $0" >&2
  echo "   or: $0 root@<ip>:22" >&2
  exit 1
fi
if [[ "${POD_ARG}" == *:* ]]; then
  POD_PORT="${POD_ARG##*:}"
  POD_HOST="${POD_ARG%:*}"
else
  POD_HOST="${POD_ARG}"
fi

ACTIONS="${ACTIONS:-sync,setup}"
SMOKE_TAG="${SMOKE_TAG:-allfeat-gpu}"

HERE="$(dirname "$(readlink -f "$0")")"
SSH_OPTS="-p ${POD_PORT} -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30"

echo "=== runpod.sh ==="
echo "pod:      ${POD_HOST}:${POD_PORT}"
echo "actions:  ${ACTIONS}"
echo

# Pre-flight: verify ssh works.
echo "--- ssh sanity check ---"
if ! ssh ${SSH_OPTS} "${POD_HOST}" 'echo "connected as $(whoami)@$(hostname)"; nvidia-smi --query-gpu=name,memory.total --format=csv,noheader 2>/dev/null || echo "WARN: no GPU?"' ; then
  echo "ERROR: ssh to ${POD_HOST}:${POD_PORT} failed" >&2
  exit 2
fi
echo

# Pre-flight: ensure rsync exists on the pod (chicken-and-egg — needed
# BEFORE the sync phase runs). Fresh Runpod pytorch images don't include
# rsync by default. Idempotent — apt-get install will be a no-op when
# rsync is already present.
echo "--- bootstrap: ensure rsync on pod ---"
ssh ${SSH_OPTS} "${POD_HOST}" 'command -v rsync >/dev/null 2>&1 || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq rsync)'
echo

run_action() {
  local action="$1"
  case "${action}" in
    sync)
      echo "=== ACTION: sync ==="
      POD_HOST="${POD_HOST}" POD_PORT="${POD_PORT}" bash "${HERE}/sync_to_pod.sh"
      ;;
    setup)
      echo "=== ACTION: setup (running setup.sh on pod) ==="
      # setup.sh is now on the pod (synced). Run it there.
      ssh ${SSH_OPTS} "${POD_HOST}" 'bash /home/ivy/Code/glia/scratch/runpod/setup.sh'
      ;;
    mount)
      echo "=== ACTION: mount (laptop SSHFS) ==="
      POD_HOST="${POD_HOST}" POD_PORT="${POD_PORT}" bash "${HERE}/mount.sh"
      ;;
    smoke)
      echo "=== ACTION: smoke (launch full-feature smoke on pod) ==="
      ssh ${SSH_OPTS} "${POD_HOST}" "cd /home/ivy/Code/glia/scratch/lens && \
        GLIA_TWO_PASS=1 \
        GLIA_SAGE_RUNTIME=1 \
        GLIA_RUNTIME_EVIDENCE=1 \
        GLIA_PER_TOKEN_POOL=1 \
        GLIA_SAMPLES=3 \
        GLIA_LENS_TRACE=1 \
        GLIA_COMPOSITIONAL=1 \
        ./cycle/run_smoke.sh ${SMOKE_TAG} 7b-q4 2>&1 | tail -100"
      ;;
    *)
      echo "WARN: unknown action '${action}' (skipping)" >&2
      ;;
  esac
  echo
}

IFS=',' read -ra ACT_ARRAY <<< "${ACTIONS}"
for a in "${ACT_ARRAY[@]}"; do
  run_action "$(echo "$a" | xargs)"   # xargs trims whitespace
done

echo "=== runpod.sh done ==="
case ",${ACTIONS}," in
  *,setup,*)
    echo "Next: ACTIONS=smoke ./runpod.sh ${POD_HOST}:${POD_PORT}    # to kick the first cycle"
    ;;
esac
