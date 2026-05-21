#!/usr/bin/env bash
# Runpod pod-side bootstrap. Runs ON THE POD (not locally). Assumes:
# - A6000 48GB CUDA-ready image (Runpod's "pytorch" or "runpod/base" template)
# - 100GB persistent volume mounted at /workspace OR using ephemeral
# - /home/ivy structure to be created so glia's hardcoded absolute paths work
#   identically to the laptop (no path-rewriting needed)
#
# Usage:
#   curl -fsSL <gist> | bash       OR
#   scp setup.sh root@<pod>:/root/ && ssh root@<pod> 'bash /root/setup.sh'
#
# This script is idempotent: rerunning on a warm pod skips installed steps.

set -euo pipefail

echo "=== glia runpod bootstrap ==="
echo "host: $(hostname)  date: $(date -Iseconds)"
echo

# 1. Sanity: GPU + CUDA
if ! command -v nvidia-smi >/dev/null 2>&1; then
  echo "ERROR: nvidia-smi not found — is this a GPU pod?"
  exit 1
fi
nvidia-smi --query-gpu=name,memory.total --format=csv
nvcc --version 2>/dev/null || echo "WARN: nvcc not found; will install CUDA toolkit"

# 2. OS packages
echo
echo "=== apt install build essentials ==="
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
  build-essential cmake git curl ca-certificates pkg-config \
  python3-dev python3-pip python3-venv \
  libssl-dev libffi-dev \
  rsync openssh-client \
  patch

# 3. Rust toolchain (rustup)
if ! command -v cargo >/dev/null 2>&1; then
  echo
  echo "=== installing Rust via rustup ==="
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
export PATH="$HOME/.cargo/bin:$PATH"
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
rustc --version

# 4. Make /home/ivy paths so glia's hardcoded absolutes work unmodified.
# All glia bins assume /home/ivy/Code/glia, /home/ivy/swe-work,
# /home/ivy/Models, /home/ivy/Datasets.  Use a single user dir on the pod.
echo
echo "=== creating /home/ivy path layout ==="
mkdir -p /home/ivy/Code
mkdir -p /home/ivy/swe-work
mkdir -p /home/ivy/Datasets/swe-bench-lite/data
mkdir -p /home/ivy/Models/qwen2.5-coder-7b-gguf
mkdir -p /home/ivy/Models/qwen2.5-coder-tokenizer
# Persist the layout on the persistent volume if mounted.
if [[ -d /workspace ]]; then
  echo "  /workspace mounted — symlinking heavy dirs"
  for d in swe-work Datasets Models; do
    if [[ ! -L /home/ivy/$d && -d /workspace/$d ]]; then
      rmdir /home/ivy/$d 2>/dev/null || true
      ln -sf /workspace/$d /home/ivy/$d
    fi
  done
fi
ls -la /home/ivy/

# 5. Python deps
echo
echo "=== installing Python deps ==="
pip3 install --upgrade pip
pip3 install --quiet \
  numpy pandas pyarrow \
  gguf \
  tokenizers \
  regex \
  swebench

# llama-cpp-python with CUDA. The CMAKE_ARGS env triggers the CUDA build.
echo
echo "=== installing llama-cpp-python with CUDA backend ==="
CMAKE_ARGS="-DGGML_CUDA=on -DLLAMA_CUDA_FORCE_MMQ=ON" \
  pip3 install --quiet --upgrade --force-reinstall --no-cache-dir llama-cpp-python

# 6. Pull glia source. Expects rsync from laptop to /home/ivy/Code/glia OR
# git clone from a remote. The laptop-side sync script handles rsync; this
# script just verifies the tree is present and builds it.
echo
if [[ ! -d /home/ivy/Code/glia ]]; then
  echo "ERROR: /home/ivy/Code/glia not found. Run sync_to_pod.sh on the laptop first."
  exit 2
fi
echo "=== glia tree present: /home/ivy/Code/glia ==="
ls /home/ivy/Code/glia/ | head -10

# 7. Cargo build — projection-text bins (synth_*) + glia-lens with CUDA real backend.
echo
echo "=== cargo build glia bins (release, --features driver) ==="
cd /home/ivy/Code/glia
cargo build --release -p repo-graph-projection-text --features driver 2>&1 | tail -3

echo
echo "=== cargo build glia-lens with CUDA backend (release, --features cuda) ==="
cd /home/ivy/Code/glia/scratch/lens
cargo build --release --features cuda 2>&1 | tail -5

# 8. Verify the build produced what we need.
echo
echo "=== verifying built binaries ==="
ls -la /home/ivy/Code/glia/target/release/synth_directive
ls -la /home/ivy/Code/glia/target/release/synth_traceback_target
ls -la /home/ivy/Code/glia/target/release/synth_test_expectation
ls -la /home/ivy/Code/glia/target/release/synth_test_assertion
ls -la /home/ivy/Code/glia/target/release/synth_runtime_type
ls -la /home/ivy/Code/glia/target/release/synth_pr_hint
ls -la /home/ivy/Code/glia/target/release/synth_check
ls -la /home/ivy/Code/glia/target/release/synth_validator
ls -la /home/ivy/Code/glia/scratch/lens/target/release/lens
ls -la /home/ivy/Code/glia/scratch/lens/target/release/lens-attention
ls -la /home/ivy/Code/glia/scratch/lens/target/release/lens-attention-bias

# 9. Data sanity.
echo
echo "=== data sanity ==="
ls /home/ivy/Datasets/swe-bench-lite/data/*.parquet 2>/dev/null | head -3 || echo "WARN: parquets missing"
ls /home/ivy/Models/qwen2.5-coder-7b-gguf/*.gguf 2>/dev/null | head -3 || echo "WARN: GGUF missing"
ls /home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json 2>/dev/null || echo "WARN: tokenizer missing"

echo
echo "=== bootstrap done ==="
echo
echo "Next: cd /home/ivy/Code/glia/scratch/lens && ./cycle/run_smoke.sh allfeat-gpu 7b-q4"
echo "with env: GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_LENS_TRACE=1 GLIA_COMPOSITIONAL=1"
