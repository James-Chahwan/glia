#!/usr/bin/env bash
# Local-side SSHFS mount of the pod's /home/ivy tree. After running this,
# you can `cd /mnt/runpod/Code/glia` locally and edit files as if they
# were on disk — changes flow through to the pod live. Build + cycle
# execution still happens on the pod (via ssh ... commands).
#
# Usage:
#   POD_HOST=root@<pod-ip>  POD_PORT=22  ./mount.sh
#   ./mount.sh root@<pod-ip>:22
#
# Unmount:
#   fusermount -u /mnt/runpod
#   (or)  sudo umount /mnt/runpod

set -euo pipefail

POD_HOST="${1:-${POD_HOST:-}}"
POD_PORT="${POD_PORT:-22}"
if [[ -z "${POD_HOST}" ]]; then
  echo "usage: POD_HOST=root@<ip> POD_PORT=22 $0   (or: $0 root@<ip>:22)" >&2
  exit 1
fi
if [[ "${POD_HOST}" == *:* ]]; then
  POD_PORT="${POD_HOST##*:}"
  POD_HOST="${POD_HOST%:*}"
fi

MOUNTPOINT="${MOUNTPOINT:-/mnt/runpod}"

# 1. Ensure sshfs is installed.
if ! command -v sshfs >/dev/null 2>&1; then
  echo "sshfs not installed. Install:"
  echo "  Fedora:   sudo dnf install fuse-sshfs"
  echo "  Ubuntu:   sudo apt install sshfs"
  echo "  macOS:    brew install --cask macfuse  &&  brew install gromgit/fuse/sshfs-mac"
  exit 1
fi

# 2. Make mountpoint.
if [[ ! -d "${MOUNTPOINT}" ]]; then
  sudo mkdir -p "${MOUNTPOINT}"
  sudo chown "${USER}" "${MOUNTPOINT}"
fi

# 3. Unmount if already mounted (clean reattach).
if mountpoint -q "${MOUNTPOINT}" 2>/dev/null; then
  echo "remounting ${MOUNTPOINT} (was mounted)"
  fusermount -u "${MOUNTPOINT}" 2>/dev/null || sudo umount "${MOUNTPOINT}"
fi

# 4. Mount.
echo "mounting ${POD_HOST}:/home/ivy → ${MOUNTPOINT}"
sshfs -p "${POD_PORT}" \
  -o reconnect,ServerAliveInterval=30,ServerAliveCountMax=3 \
  -o cache=yes,kernel_cache,large_read \
  -o follow_symlinks \
  "${POD_HOST}:/home/ivy" \
  "${MOUNTPOINT}"

echo
echo "mounted. Browse via:"
echo "  ls ${MOUNTPOINT}/"
echo "  ls ${MOUNTPOINT}/Code/glia/"
echo
echo "Run cycles on the pod (sshfs is fs-only, not exec):"
echo "  ssh -p ${POD_PORT} ${POD_HOST} 'cd /home/ivy/Code/glia/scratch/lens && GLIA_TWO_PASS=1 ... ./cycle/run_smoke.sh allfeat-gpu 7b-q4'"
echo
echo "Unmount when done:"
echo "  fusermount -u ${MOUNTPOINT}"
