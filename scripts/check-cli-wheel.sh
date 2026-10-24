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
#   (h) sdist build  `uv pip install --no-binary glia-cli <sdist>` into a second venv; its
#                `glia --version` line must equal (d)'s byte for byte. glia-stamp's build.rs hashes
#                HASHED_ROOTS + Cargo.lock relative to the workspace root, so a pruned or rewritten
#                sdist prints another PARSER_STAMP (and keys every cache differently). A
#                `uvx --from glia-cli` user on a platform with no wheel builds from this sdist.
#
#   bash scripts/check-cli-wheel.sh            all steps (two release compiles: minutes)
#   bash scripts/check-cli-wheel.sh --no-sdist (a)-(f) only
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
for a in "$@"; do
  case "$a" in
    --no-sdist) SDIST=0 ;;
    *) echo "usage: $0 [--no-sdist]" >&2; exit 2 ;;
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

# (b) the wheel.
mkdir -p "$T/dist"
if ! (cd "$ROOT/cli" && maturin build --release --out "$T/dist") >"$LOG" 2>&1; then
  tail -30 "$LOG"
  fail "wheel: maturin build"
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
  if ! (cd "$ROOT/cli" && maturin sdist --out "$T/dist") >>"$LOG" 2>&1; then
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
