#!/usr/bin/env bash
# pod_pull_and_setup.sh — runs ON THE POD, pulls everything from internet.
#
# This is the rewrite of the laptop-rsync architecture. Instead of uploading
# 5GB GGUF + ~15GB swe-work repos over a home connection, the pod uses its
# datacenter-grade bandwidth to pull each piece from its actual origin:
#
#   - glia code:    git clone github.com/James-Chahwan/glia (~50MB, ~30s)
#   - Qwen model:   huggingface-cli download (~5GB, ~30-60s @ gigabit)
#   - tokenizer:    huggingface-cli download (~10MB)
#   - parquets:     huggingface-cli download princeton-nlp/SWE-bench_Lite (~3MB)
#   - swe-work:     git clone each upstream repo + checkout base_commit
#                   (~17 repos × ~50-200MB each, can parallelize)
#
# Only thing that can't come from the internet: uncommitted Phase 1-6 work.
# Caller MUST git push the cycle 1.1 branch to GitHub before this script runs
# (or pass GLIA_REF=main if working from main).
#
# Usage (laptop-side trigger):
#   scp pod_pull_and_setup.sh root@<pod>:/root/
#   ssh root@<pod> 'GLIA_REPO=git@github.com:James-Chahwan/glia.git \
#                   GLIA_REF=overnight-loop-cycle-0 \
#                   bash /root/pod_pull_and_setup.sh'

set -euo pipefail

GLIA_REPO="${GLIA_REPO:-https://github.com/James-Chahwan/glia.git}"
GLIA_REF="${GLIA_REF:-overnight-loop-cycle-0}"
MODEL_REPO="${MODEL_REPO:-Qwen/Qwen2.5-Coder-7B-Instruct-GGUF}"
MODEL_FILE="${MODEL_FILE:-qwen2.5-coder-7b-instruct-q4_k_m.gguf}"
TOKENIZER_REPO="${TOKENIZER_REPO:-Qwen/Qwen2.5-Coder-7B-Instruct}"
SWEBENCH_DATASET="${SWEBENCH_DATASET:-princeton-nlp/SWE-bench_Lite}"

echo "=== pod_pull_and_setup ==="
echo "glia:       ${GLIA_REPO} @ ${GLIA_REF}"
echo "model:      ${MODEL_REPO}/${MODEL_FILE}"
echo "tokenizer:  ${TOKENIZER_REPO}"
echo "swe-bench:  ${SWEBENCH_DATASET}"
echo

# 0. GPU sanity.
nvidia-smi --query-gpu=name,memory.total --format=csv,noheader

# 1. OS packages (rsync, build-essential, cmake, git, python venv, etc).
echo
echo "=== apt install ==="
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
  build-essential cmake git curl ca-certificates pkg-config \
  python3-dev python3-pip python3-venv \
  libssl-dev libffi-dev patch

# 2. /home/ivy path layout (glia hardcodes absolute paths).
echo
echo "=== /home/ivy layout ==="
mkdir -p /home/ivy/Code /home/ivy/swe-work \
         /home/ivy/Datasets/swe-bench-lite/data \
         /home/ivy/Models/qwen2.5-coder-7b-gguf \
         /home/ivy/Models/qwen2.5-coder-tokenizer
if [[ -d /workspace ]]; then
  echo "  /workspace is mounted — symlinking heavy dirs to persistent volume"
  for d in swe-work Datasets Models; do
    if [[ ! -L /home/ivy/$d ]]; then
      mkdir -p "/workspace/$d"
      rm -rf "/home/ivy/$d"
      ln -sf "/workspace/$d" "/home/ivy/$d"
    fi
  done
fi
ls -la /home/ivy/

# 3. Rust toolchain.
if ! command -v cargo >/dev/null 2>&1; then
  echo
  echo "=== installing rust via rustup ==="
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
export PATH="$HOME/.cargo/bin:$PATH"
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
rustc --version

# 4. Python deps + huggingface CLI for pulls.
echo
echo "=== Python deps ==="
pip3 install --quiet --upgrade pip
pip3 install --quiet \
  numpy pandas pyarrow \
  gguf \
  tokenizers \
  regex \
  swebench \
  huggingface_hub

# 5. llama-cpp-python with CUDA.
echo
echo "=== llama-cpp-python with CUDA ==="
CMAKE_ARGS="-DGGML_CUDA=on -DLLAMA_CUDA_FORCE_MMQ=ON" \
  pip3 install --quiet --upgrade --force-reinstall --no-cache-dir llama-cpp-python

# 6. Pull glia source from GitHub.
echo
echo "=== git clone glia ==="
if [[ -d /home/ivy/Code/glia/.git ]]; then
  echo "  glia repo already present — fetching latest ${GLIA_REF}"
  cd /home/ivy/Code/glia
  git fetch origin
  git checkout "${GLIA_REF}"
  git pull --ff-only origin "${GLIA_REF}"
else
  git clone "${GLIA_REPO}" /home/ivy/Code/glia
  cd /home/ivy/Code/glia
  git checkout "${GLIA_REF}"
fi
git log --oneline -3

# 7. Pull Qwen GGUF from HuggingFace.
echo
echo "=== huggingface download: ${MODEL_REPO}/${MODEL_FILE} ==="
mkdir -p /home/ivy/Models/qwen2.5-coder-7b-gguf
if [[ ! -f /home/ivy/Models/qwen2.5-coder-7b-gguf/${MODEL_FILE} ]]; then
  huggingface-cli download "${MODEL_REPO}" "${MODEL_FILE}" \
    --local-dir /home/ivy/Models/qwen2.5-coder-7b-gguf \
    --local-dir-use-symlinks False
else
  echo "  model already present"
fi
ls -lh /home/ivy/Models/qwen2.5-coder-7b-gguf/

# 8. Pull tokenizer.
echo
echo "=== huggingface download: ${TOKENIZER_REPO}/tokenizer.json ==="
if [[ ! -f /home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json ]]; then
  huggingface-cli download "${TOKENIZER_REPO}" tokenizer.json \
    --local-dir /home/ivy/Models/qwen2.5-coder-tokenizer \
    --local-dir-use-symlinks False
fi
ls -lh /home/ivy/Models/qwen2.5-coder-tokenizer/

# 9. Pull SWE-bench Lite parquets.
echo
echo "=== huggingface download: ${SWEBENCH_DATASET} parquets ==="
if [[ ! -f /home/ivy/Datasets/swe-bench-lite/data/test-00000-of-00001.parquet ]]; then
  huggingface-cli download "${SWEBENCH_DATASET}" \
    "data/dev-00000-of-00001.parquet" "data/test-00000-of-00001.parquet" \
    --repo-type dataset \
    --local-dir /home/ivy/Datasets/swe-bench-lite \
    --local-dir-use-symlinks False
fi
ls -lh /home/ivy/Datasets/swe-bench-lite/data/

# 10. Clone swe-work repos at base_commit from upstream GitHub. The
# cycle_loop_set + holdout manifests are now in the glia tree we just
# pulled. Parses parquet to get (repo, base_commit) per instance.
echo
echo "=== cloning swe-work repos ==="
python3 <<'PY'
import json, os, subprocess
import pandas as pd
from pathlib import Path

GLIA = Path("/home/ivy/Code/glia")
LOOP = GLIA / "scratch/lens/manifests/cycle_loop_set.json"
HOLDOUT = GLIA / "scratch/lens/manifests/holdout.json"
PARQS = list(Path("/home/ivy/Datasets/swe-bench-lite/data").glob("*.parquet"))

instances = []
for path in [LOOP, HOLDOUT]:
    if path.exists():
        instances.extend(json.loads(path.read_text()))

want_ids = [x["instance_id"] for x in instances]
print(f"  want {len(want_ids)} instances")

df = pd.concat([pd.read_parquet(p) for p in PARQS], ignore_index=True)
df = df[df.instance_id.isin(want_ids)]
print(f"  matched {len(df)} rows in parquet")

for _, row in df.iterrows():
    inst_id = row.instance_id
    repo = row.repo
    base = row.base_commit
    dest = Path("/home/ivy/swe-work") / inst_id
    if (dest / ".git").exists():
        # Already cloned — fast-forward / checkout base_commit.
        subprocess.run(["git", "-C", str(dest), "fetch", "--quiet", "origin"], check=False)
        subprocess.run(["git", "-C", str(dest), "checkout", "--quiet", base], check=False)
        print(f"  [exists] {inst_id} @ {base[:8]}")
        continue
    dest.parent.mkdir(parents=True, exist_ok=True)
    url = f"https://github.com/{repo}.git"
    print(f"  clone {repo} → {dest} @ {base[:8]}")
    # Shallow-ish clone: full history takes too long for sympy etc.
    # We'll do a full clone since `git checkout <commit>` needs it.
    r = subprocess.run(["git", "clone", "--quiet", url, str(dest)], check=False)
    if r.returncode == 0:
        subprocess.run(["git", "-C", str(dest), "checkout", "--quiet", base], check=False)
    else:
        print(f"    CLONE FAILED rc={r.returncode}")

print("  swe-work clone phase done")
PY

# 11. Cargo builds.
echo
echo "=== cargo build (release, --features driver) ==="
cd /home/ivy/Code/glia
cargo build --release -p repo-graph-projection-text --features driver 2>&1 | tail -3

echo
echo "=== cargo build glia-lens (release, --features cuda) ==="
cd /home/ivy/Code/glia/scratch/lens
cargo build --release --features cuda 2>&1 | tail -5

# 12. Verification.
echo
echo "=== built binaries ==="
ls /home/ivy/Code/glia/target/release/synth_* 2>/dev/null | head -20
ls /home/ivy/Code/glia/scratch/lens/target/release/lens* 2>/dev/null | head -5

# 13. Status report.
echo
echo "=== STATUS ==="
echo "  GPU:      $(nvidia-smi --query-gpu=name --format=csv,noheader)"
echo "  GGUF:     $(ls -lh /home/ivy/Models/qwen2.5-coder-7b-gguf/*.gguf 2>/dev/null | awk '{print $5}')"
echo "  parquets: $(ls /home/ivy/Datasets/swe-bench-lite/data/*.parquet 2>/dev/null | wc -l) found"
echo "  swe-work: $(ls -d /home/ivy/swe-work/*/ 2>/dev/null | wc -l) repos"
echo
echo "Next: kick first smoke:"
echo "  cd /home/ivy/Code/glia/scratch/lens && \\"
echo "  GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \\"
echo "  GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_LENS_TRACE=1 GLIA_COMPOSITIONAL=1 \\"
echo "  ./cycle/run_smoke.sh allfeat-gpu 7b-q4"
