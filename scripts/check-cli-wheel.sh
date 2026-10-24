#!/usr/bin/env bash
# check-cli-wheel.sh - build the `glia-cli` PyPI package (cli/pyproject.toml, maturin bindings = "bin")
# and install it the ways a user would, writing nothing inside the repo (FE.5).
#
# Steps, each reported on a `[cli-wheel]` line:
#   (a) license  cli/LICENSE is a byte copy of the root LICENSE (as py/LICENSE is)
#   (b) wheel    `maturin build --release` from cli/ (as the CI's `working-directory: cli` does)
#   (c) name     exactly one glia_cli-<ver>-py3-none-*.whl, <ver> = glia-cli's Cargo version
#   (d) install  `uv pip install` into a fresh venv; `glia --version` starts `glia <ver>`
#   (e) find     `glia find <copy of tests/go_smoke> HashPassword --json` exits 0 with a row whose
#                qname ends HashPassword
#   (f) uvx      `uvx --from <wheel> glia --version` (skipped when uvx is not installed)
#   (g) sdist    `maturin sdist` from cli/ gives glia_cli-<ver>.tar.gz holding cli/Cargo.toml,
#                engine/Cargo.toml and stamp/build.rs (the path dependencies travel)
#   (w) worker   (FB.14) on x86_64 Linux / aarch64 macOS: glia-embed's `glia-embed-worker` is
#                built with `gpu-webgpu` and staged with its Dawn library in the scratch copy's
#                cli/glia_cli.data/scripts/ (+ `data = "glia_cli.data"`), as the wheels workflow
#                does; the wheel's `glia` itself is built `embed` only (cli/pyproject.toml), so
#                it links no Dawn. The build runs in a scratch COPY of the tree (target/, .git,
#                bench excluded), so the staging writes nothing inside the repo.
#   (i) embed    `glia embed <copy> --model test-hash` writes vectors; where a worker ships it is
#                installed beside `glia`, `readelf` (Linux) finds no Dawn in `glia`'s NEEDED, and
#                bge-small runs twice: with no Vulkan ICD (VK_DRIVER_FILES=/nonexistent.json) the
#                `[embed] device=` line reads `backend=cpu .. fallback=no_adapter:..` and the run
#                completes; with the host's devices it completes on whatever backend it reports.
#                Skip the two bge-small runs (they download the model once) with --no-model.
#   (h) sdist build  `uv pip install --no-binary glia-cli <sdist>` into a second venv; its
#                `glia --version` line must equal (d)'s byte for byte. glia-stamp's build.rs hashes
#                HASHED_ROOTS + Cargo.lock relative to the workspace root, so a pruned or rewritten
#                sdist prints another PARSER_STAMP (and keys every cache differently). A
#                `uvx --from glia-cli` user on a platform with no wheel builds from this sdist.
#
#   bash scripts/check-cli-wheel.sh            all steps (three release compiles: minutes)
#   bash scripts/check-cli-wheel.sh --no-sdist (a)-(f), (w), (i) only
#   bash scripts/check-cli-wheel.sh --no-model (i) without the bge-small runs
#
# Env: GLIA_ROOT (glia tree to package, default this checkout; mid-wave point it at a coherent tree,
# e.g. a `git archive HEAD` copy); CLI_WHEEL_TMP (parent of the scratch dir, default $TMPDIR or /tmp;
# put it on disk, the build trees are large); CARGO_TARGET_DIR is honoured by both release builds;
# CLI_WHEEL_KEEP=1 keeps the scratch dir. Needs maturin, uv and python3 on PATH.
# Last line:
#   [cli-wheel] check: ok wheel=<name> size_mb=<s>
#   [cli-wheel] check: FAILED (<step>)                     exit 1
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ROOT=$(cd "${GLIA_ROOT:-$HERE}" 2>/dev/null && pwd) || { echo "[cli-wheel] check: FAILED (GLIA_ROOT=${GLIA_ROOT:-} does not exist)"; exit 1; }
SDIST=1
MODEL=1
for a in "$@"; do
  case "$a" in
    --no-sdist) SDIST=0 ;;
    --no-model) MODEL=0 ;;
    *) echo "usage: $0 [--no-sdist] [--no-model]" >&2; exit 2 ;;
  esac
done

fail() { echo "[cli-wheel] check: FAILED ($1)"; exit 1; }
for tool in maturin uv python3; do
  command -v "$tool" >/dev/null 2>&1 || fail "missing tool: $tool"
done

PARENT=${CLI_WHEEL_TMP:-${TMPDIR:-/tmp}}
mkdir -p "$PARENT" || fail "cannot create $PARENT"
T=$(mktemp -d "$PARENT/glia-cli-wheel.XXXXXX") || fail "mktemp under $PARENT"
cleanup() { if [ "${CLI_WHEEL_KEEP:-0}" = 1 ]; then echo "[cli-wheel] kept $T"; else rm -rf "$T"; fi; }
trap cleanup EXIT
LOG=$T/build.log
echo "[cli-wheel] root=$ROOT scratch=$T"

# (a) the license travels as a byte copy.
cmp -s "$ROOT/LICENSE" "$ROOT/cli/LICENSE" || fail "license: cli/LICENSE differs from LICENSE"
echo "[cli-wheel] license: cli/LICENSE == LICENSE"

VER=$(cd "$ROOT" && cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="glia-cli"))') \
  || fail "version: cargo metadata found no glia-cli package"
echo "[cli-wheel] version: glia-cli $VER"

# The scratch copy every build below runs in (the staging writes into its cli/).
SRC=$T/src
mkdir -p "$SRC"
tar -C "$ROOT" --exclude=./target --exclude=./.git --exclude=./bench --exclude='./*/target' \
  --exclude=./engram-export/target -cf - . | tar -x -C "$SRC" || fail "copy: the tree into $SRC"
TARGET_DIR=${CARGO_TARGET_DIR:-$SRC/target}

# (w) the WebGPU embedding worker, on the platforms ORT ships a WebGPU build for.
WORKER=0
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) DAWN=libwebgpu_dawn.so; WORKER=1 ;;
  Darwin-arm64) DAWN=libwebgpu_dawn.dylib; WORKER=1 ;;
esac
if [ "$WORKER" = 1 ]; then
  if ! (cd "$SRC" && cargo build --release --locked -p glia-embed --features gpu-webgpu --bin glia-embed-worker) >"$LOG" 2>&1; then
    tail -30 "$LOG"
    fail "worker: cargo build glia-embed-worker"
  fi
  mkdir -p "$SRC/cli/glia_cli.data/scripts"
  cp "$TARGET_DIR/release/glia-embed-worker" "$SRC/cli/glia_cli.data/scripts/" || fail "worker: copy the worker"
  strip "$SRC/cli/glia_cli.data/scripts/glia-embed-worker" 2>/dev/null || true
  cp -L "$TARGET_DIR/release/$DAWN" "$SRC/cli/glia_cli.data/scripts/" || fail "worker: copy $DAWN"
  cp "$SRC/cli/pyproject.toml" "$T/pyproject.clean"
  echo 'data = "glia_cli.data"' >>"$SRC/cli/pyproject.toml"
  echo "[cli-wheel] worker: staged glia-embed-worker + $DAWN"
else
  echo "[cli-wheel] worker: none on $(uname -s)-$(uname -m) (ORT ships no WebGPU build): the CPU"
fi

# (b) the wheel.
mkdir -p "$T/dist"
if ! (cd "$SRC/cli" && maturin build --release --out "$T/dist") >>"$LOG" 2>&1; then
  tail -30 "$LOG"
  fail "wheel: maturin build"
fi
# The sdist (g) is the published one: no staged worker, no `data` key (it embeds on the CPU).
if [ "$WORKER" = 1 ]; then
  cp "$T/pyproject.clean" "$SRC/cli/pyproject.toml" && rm -rf "$SRC/cli/glia_cli.data"
fi

# (c) exactly one wheel, named for the workspace version.
shopt -s nullglob
WHEELS=("$T"/dist/glia_cli-"$VER"-py3-none-*.whl)
ALL=("$T"/dist/*.whl)
shopt -u nullglob
[ "${#WHEELS[@]}" -eq 1 ] && [ "${#ALL[@]}" -eq 1 ] || fail "name: expected one glia_cli-$VER-py3-none-*.whl, found: $(ls "$T/dist")"
WHEEL=${WHEELS[0]}
NAME=$(basename "$WHEEL")
SIZE=$(python3 -c 'import os,sys; print(f"{os.path.getsize(sys.argv[1])/1048576:.1f}")' "$WHEEL")
echo "[cli-wheel] wheel: $NAME size_mb=$SIZE"

# (d) install into a fresh venv.
uv venv -q "$T/venv" >>"$LOG" 2>&1 || fail "install: uv venv"
uv pip install -q --python "$T/venv/bin/python" "$WHEEL" >>"$LOG" 2>&1 || { tail -20 "$LOG"; fail "install: uv pip install wheel"; }
[ -x "$T/venv/bin/glia" ] || fail "install: no glia executable in the venv"
VLINE=$("$T/venv/bin/glia" --version 2>/dev/null) || fail "install: glia --version exited non-zero"
case "$VLINE" in
  "glia $VER"*) echo "[cli-wheel] install: $VLINE" ;;
  *) fail "install: glia --version printed '$VLINE', expected 'glia $VER ..'" ;;
esac

# (e) a real answer from the installed binary, on a copy of the Go smoke fixture.
cp -r "$ROOT/tests/go_smoke" "$T/repo" || fail "find: copy tests/go_smoke"
if ! GLIA_NO_PERSIST=1 "$T/venv/bin/glia" find "$T/repo" HashPassword --json >"$T/find.json" 2>>"$LOG"; then
  fail "find: glia find exited non-zero"
fi
python3 - "$T/find.json" <<'PY' || fail "find: no row whose qname ends HashPassword"
import json, sys
rows = json.load(open(sys.argv[1])).get("results") or []
hits = [r["qname"] for r in rows if str(r.get("qname", "")).endswith("HashPassword")]
if not hits:
    sys.exit(1)
print(f"[cli-wheel] find: rows={len(rows)} hit={hits[0]}")
PY

# (i) embeddings from the installed wheel.
cp -r "$ROOT/tests/go_smoke" "$T/embed" || fail "embed: copy tests/go_smoke"
GLIA_NO_PERSIST=1 "$T/venv/bin/glia" embed "$T/embed" --model test-hash >>"$LOG" 2>&1 || fail "embed: glia embed --model test-hash"
[ -s "$T/embed/.glia/vectors.jsonl" ] || fail "embed: no vectors written"
echo "[cli-wheel] embed: test-hash rows=$(wc -l <"$T/embed/.glia/vectors.jsonl")"
if [ "$WORKER" = 1 ]; then
  [ -x "$T/venv/bin/glia-embed-worker" ] || fail "embed: no glia-embed-worker beside glia"
  [ -f "$T/venv/bin/$DAWN" ] || fail "embed: no $DAWN beside glia"
  if command -v readelf >/dev/null 2>&1 && readelf -d "$T/venv/bin/glia" | grep -q 'NEEDED.*webgpu_dawn'; then
    fail "embed: the wheel's glia links Dawn"
  fi
  if [ "$MODEL" = 1 ]; then
    cp -r "$ROOT/tests/go_smoke" "$T/embed_icd"
    GLIA_NO_PERSIST=1 VK_DRIVER_FILES=/nonexistent.json "$T/venv/bin/glia" embed "$T/embed_icd" --model bge-small \
      >/dev/null 2>"$T/embed_icd.err" || { tail -20 "$T/embed_icd.err"; fail "embed: bge-small without a Vulkan ICD"; }
    ICD=$(grep '^\[embed\] device=' "$T/embed_icd.err" | tail -1)
    case "$ICD" in
      *" backend=cpu "*" fallback=no_adapter:"*) echo "[cli-wheel] embed: no ICD -> $ICD" ;;
      *) tail -20 "$T/embed_icd.err"; fail "embed: no ICD gave '$ICD', expected backend=cpu fallback=no_adapter:" ;;
    esac
    cp -r "$ROOT/tests/go_smoke" "$T/embed_gpu"
    GLIA_NO_PERSIST=1 "$T/venv/bin/glia" embed "$T/embed_gpu" --model bge-small \
      >/dev/null 2>"$T/embed_gpu.err" || { tail -20 "$T/embed_gpu.err"; fail "embed: bge-small on the default device"; }
    echo "[cli-wheel] embed: default -> $(grep '^\[embed\] device=' "$T/embed_gpu.err" | tail -1)"
  else
    echo "[cli-wheel] embed: bge-small runs skipped (--no-model)"
  fi
fi

# (f) zero-install: uvx straight from the wheel file.
if command -v uvx >/dev/null 2>&1; then
  UVX_LINE=$(UV_TOOL_DIR="$T/uv-tools" uvx -q --from "$WHEEL" glia --version 2>>"$LOG") || fail "uvx: uvx --from <wheel> glia --version"
  [ "$UVX_LINE" = "$VLINE" ] || fail "uvx: printed '$UVX_LINE', expected '$VLINE'"
  echo "[cli-wheel] uvx: $UVX_LINE"
else
  echo "[cli-wheel] uvx: skipped (not installed)"
fi

if [ "$SDIST" = 1 ]; then
  # (g) the sdist carries the path dependencies.
  if ! (cd "$SRC/cli" && maturin sdist --out "$T/dist") >>"$LOG" 2>&1; then
    tail -20 "$LOG"
    fail "sdist: maturin sdist"
  fi
  SD="$T/dist/glia_cli-$VER.tar.gz"
  [ -f "$SD" ] || fail "sdist: no glia_cli-$VER.tar.gz, found: $(ls "$T/dist")"
  tar -tzf "$SD" >"$T/sdist.list" || fail "sdist: tar -tzf"
  for want in cli/Cargo.toml engine/Cargo.toml stamp/build.rs; do
    grep -q "^glia_cli-$VER/$want\$" "$T/sdist.list" || fail "sdist: $want missing from the archive"
  done
  echo "[cli-wheel] sdist: $(basename "$SD") files=$(wc -l <"$T/sdist.list") (cli, engine, stamp present)"

  # (h) building from the sdist stamps the same binary.
  uv venv -q "$T/venv2" >>"$LOG" 2>&1 || fail "sdist build: uv venv"
  if ! uv pip install -q --python "$T/venv2/bin/python" --no-binary glia-cli "$SD" >>"$LOG" 2>&1; then
    tail -30 "$LOG"
    fail "sdist build: uv pip install --no-binary"
  fi
  SLINE=$("$T/venv2/bin/glia" --version 2>/dev/null) || fail "sdist build: glia --version exited non-zero"
  if [ "$SLINE" != "$VLINE" ]; then
    echo "[cli-wheel] sdist: wheel '$VLINE' vs sdist '$SLINE'"
    fail "sdist build: version line differs"
  fi
  echo "[cli-wheel] sdist: version line identical ($SLINE)"
else
  echo "[cli-wheel] sdist: skipped (--no-sdist)"
fi

echo "[cli-wheel] check: ok wheel=$NAME size_mb=$SIZE"
