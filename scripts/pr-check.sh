#!/usr/bin/env bash
# pr-check.sh - the contributor fast gate (HT.10, 0.5.7). For the paths a change touches it runs the checks those
# paths need, in minutes on a warm target/: fmt, clippy and the tests of the touched crates, the id lock
# (id_blocks::), the language seams (seams::) and the self-scan (self_scan::), the CLI and pyo3 surfaces, and the
# substrate-gap grade of the touched fixtures and the touched language's matrix row. Never the rank or cohort gate:
# those are the release's (about an hour in all); `--rank-fast` opts into the rank gate's five small snapshots.
#
#   bash scripts/pr-check.sh [--base <rev>] [--no-wheel] [--rank-fast]
#
#   --base <rev>   the change is `git diff --name-only <rev>...HEAD` plus the working tree (staged, unstaged and
#                  untracked files). Default: origin/main when it exists, else main.
#   --no-wheel     never build the glia_py wheel; the grade / matrix / pyo3-surface steps run with the glia_py
#                  already importable from GLIA_PR_CHECK_PY (default: the venv's python, else python3), which may
#                  predate the change.
#   --rank-fast    also run `scripts/rank-gate.sh check` on the five small snapshots (closeout.py's RANK_FAST,
#                  about 136 s after the snapshots are fetched; a partial check, never a ratchet).
#
# Env: GLIA_THREADS and CARGO_BUILD_JOBS pass through to every build and test; TMPDIR holds the step logs;
# GLIA_PR_CHECK_VENV (default ~/.cache/glia/pr-check/venv) is the venv the wheel is installed into (never the user
# site); GLIA_PR_CHECK_PY the python that grades under --no-wheel; GLIA_PR_CHECK_CLI_FEATURES the cargo feature
# flags for glia-cli (default: its defaults, or `--no-default-features --features embed` on a glibc older than 2.38,
# which the WebGPU prebuilt needs: cli/Cargo.toml's feature comment). GLIA_UPDATE_SURFACE / GLIA_UPDATE_GOLDEN are
# unset: the gate checks, it never rewrites a snapshot.
#
# Lines (stdout): every step prints `[pr-check] step=<name> <ok|FAILED|skipped> <s>s`, a FAILED step the tail of its
# log; the last line is `[pr-check] ok - <n> steps in <s>s (base <rev12>)` (exit 0) or `[pr-check] FAILED (<steps>)`
# (exit 1). A usage error exits 2. <n> counts the steps that ran; a skipped step says why and is not counted.
#
# A first run compiles the workspace (the engine alone is large) and, for a fixture or parser change, the wheel:
# many minutes cold. Later runs reuse target/; the wall times measured warm are in CONTRIBUTING.md.
set -uo pipefail

if ((BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 3))); then
  echo "[pr-check] needs bash >= 4.3 (this is $BASH_VERSION; on macOS: brew install bash)" >&2
  exit 2
fi
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 2
unset GLIA_UPDATE_SURFACE GLIA_UPDATE_GOLDEN

usage() {
  sed -n '7,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

# ---- the path table: the single source of which step a path needs ---------------------------------------------
# <glob>|<steps>. Every row whose glob matches a changed path adds its steps (rows are not first-match). Besides
# these, a path inside a workspace crate adds that crate's clippy and tests, and fmt always runs.
#   ids        cargo test -p glia-engine --test engine_tests id_blocks::   (the id lock, T9.1)
#   seams      cargo test -p glia-engine --test engine_tests seams::       (the language seams, T9.3)
#   self_scan  cargo test -p glia-engine --test engine_tests self_scan::   (no phantom edge from glia's own tree)
#   cli_surface / readme_surface / skill_surface   the CLI snapshots, README `## CLI`, the skills' invocations
#   py_surface the pyo3 surface over the wheel (py/check_api_surface.py, py/tests/surface/test_*.py)
#   grade      bench/substrate-gap/grade.py on each touched fixture / matrix cell / pattern cell (needs the wheel)
#   matrix     bench/substrate-gap/matrix.py --language <lang> --fail-on-error (needs the wheel)
#   bench      bench/substrate-gap/test_grade.py, test_matrix.py, test_run.py (needs the wheel)
#   workspace  clippy and tests of every workspace crate (a root manifest or lint config moved)
#   engram     scripts/check-engram-export.sh (only with ../Engram checked out beside glia)
#   self       bash -n (and shellcheck, when installed) on this script
PATH_TABLE=(
  "core/*|ids"
  "code-domain/*|ids"
  "build-domain/*|ids"
  "activation/*|ids"
  "parsers/*|seams self_scan"
  "graph/*|seams self_scan"
  "engine/*|seams self_scan"
  "cli/*|cli_surface readme_surface"
  "README.md|readme_surface"
  "skills/*|skill_surface"
  "py/*|py_surface"
  "bench/substrate-gap/fixtures/*/*|grade"
  "bench/substrate-gap/matrix/*/*/*|grade matrix"
  "bench/substrate-gap/patterns/*/*/*|grade matrix"
  "parsers/code/*/*|matrix"
  "bench/substrate-gap/*.py|bench"
  "Cargo.toml|workspace"
  "Cargo.lock|workspace"
  "clippy.toml|workspace"
  "rustfmt.toml|workspace"
  "rust-toolchain*|workspace"
  "engram-export/*|engram"
  "scripts/pr-check.sh|self"
)
# Information only, never run here: the release gates a change on these paths will run at the close-out. Mirrors
# dev-notes/wave-runner/closeout.py's RANKED_PATHS and COHORT_PATHS (prefixes).
RANK_PATHS=(core/ code-domain/ graph/ parsers/ engine/ activation/ bench/rank-gate/ scripts/rank-gate.sh)
COHORT_PATHS=(engine/ graph/ parsers/ code-domain/ bench/cohort/ bench/sibyl/ scripts/cohort-gate.sh)
RANK_FAST="shell,websocket,samples,commons-lang,commons-io"   # closeout.py's RANK_FAST

# ---- arguments -----------------------------------------------------------------------------------------------------
BASE="" NO_WHEEL=0 RANK=0
while (($#)); do
  case "$1" in
    --base) (($# >= 2)) || { echo "[pr-check] --base needs a revision" >&2; usage; }; BASE=$2; shift 2 ;;
    --base=*) BASE=${1#--base=}; shift ;;
    --no-wheel) NO_WHEEL=1; shift ;;
    --rank-fast) RANK=1; shift ;;
    -h|--help) usage ;;
    *) echo "[pr-check] unknown argument: $1" >&2; usage ;;
  esac
done
if [[ -z $BASE ]]; then
  if git rev-parse -q --verify origin/main >/dev/null; then BASE=origin/main; else BASE=main; fi
fi
BASE_SHA=$(git rev-parse -q --verify "$BASE^{commit}") || { echo "[pr-check] not a revision: $BASE" >&2; exit 2; }
BASE12=${BASE_SHA:0:12}

T0=$SECONDS
LOGDIR=$(mktemp -d "${TMPDIR:-/tmp}/glia-pr-check.XXXXXX") || { echo "[pr-check] cannot create a log dir" >&2; exit 2; }
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
VENV=${GLIA_PR_CHECK_VENV:-$HOME/.cache/glia/pr-check/venv}

# ---- the change ----------------------------------------------------------------------------------------------------
CHANGED=()
while IFS= read -r p; do [[ -n $p ]] && CHANGED+=("$p"); done < <(
  { git diff --name-only "$BASE_SHA...HEAD"; git diff --name-only HEAD; git ls-files --others --exclude-standard; } |
    LC_ALL=C sort -u)

declare -A WANT=() CRATES=() FIXTURES=() LANGS=()
want() { local s; for s in "$@"; do WANT[$s]=1; done; }

# The workspace crate owning a path: the nearest ancestor directory holding a Cargo.toml with a `name`, below the
# root (bench/ and engram-export/ are excluded from the workspace and never crates here).
crate_of() {
  local d
  d=$(dirname "$1")
  while [[ $d != . && $d != / ]]; do
    if [[ -f $d/Cargo.toml ]]; then
      case "$d" in bench|bench/*|engram-export|engram-export/*) return 1 ;; esac
      sed -n '/^\[package\]/,/^\[/{s/^name *= *"\([^"]*\)".*/\1/p}' "$d/Cargo.toml" | head -n1
      return 0
    fi
    d=$(dirname "$d")
  done
  return 1
}

# The substrate-gap matrix row a touched path belongs to, if any (matrix_vocab.LANGUAGES spelling).
lang_of_parser() {
  case "$1" in
    react|angular|vue) echo typescript ;;
    docs|extractors) echo all ;;     # cross-cutting: every row
    *) [[ -d bench/substrate-gap/matrix/$1 ]] || grep -q "\"$1\"" bench/substrate-gap/matrix_vocab.py && echo "$1" ;;
  esac
}

for p in "${CHANGED[@]}"; do
  for row in "${PATH_TABLE[@]}"; do
    glob=${row%%|*}
    # shellcheck disable=SC2053,SC2086 # the table's globs are patterns, its steps words, on purpose
    [[ $p == $glob ]] && want ${row#*|}
  done
  if c=$(crate_of "$p") && [[ -n $c ]]; then CRATES[$c]=1; fi
  case "$p" in
    bench/substrate-gap/fixtures/*/*) FIXTURES[fixtures/$(cut -d/ -f4 <<<"$p")]=1 ;;
    bench/substrate-gap/matrix/*/*/*|bench/substrate-gap/patterns/*/*/*)
      IFS=/ read -r _ _ kind lang cell _ <<<"$p"; FIXTURES[$kind/$lang/$cell]=1; LANGS[$lang]=1 ;;
    parsers/code/*/*)
      l=$(lang_of_parser "$(cut -d/ -f3 <<<"$p")"); [[ -n $l ]] && LANGS[$l]=1 ;;
  esac
done
[[ -n ${WANT[workspace]:-} ]] && CRATES=()   # every crate, as one workspace run

# A touched fixture's matrix row comes from its key.json "cells" (a legacy fixture declares none).
for f in "${!FIXTURES[@]}"; do
  [[ $f == fixtures/* && -f bench/substrate-gap/$f/key.json ]] || continue
  while IFS= read -r l; do [[ -n $l ]] && LANGS[$l]=1; done < <(python3 -I -c '
import json, sys
for c in json.load(open(sys.argv[1])).get("cells") or []:
    print(str(c).split("/")[0])' "bench/substrate-gap/$f/key.json" 2>/dev/null)
done
((${#FIXTURES[@]})) && WANT[grade]=1
((${#LANGS[@]})) && WANT[matrix]=1
if ((${#FIXTURES[@]} + ${#LANGS[@]})) || [[ -n ${WANT[bench]:-} || -n ${WANT[py_surface]:-} ]]; then
  WANT[wheel]=1
fi

# ---- glia-cli's features -------------------------------------------------------------------------------------------
CLI_FLAGS=()
if [[ -n ${GLIA_PR_CHECK_CLI_FEATURES+set} ]]; then
  read -r -a CLI_FLAGS <<<"$GLIA_PR_CHECK_CLI_FEATURES"
elif v=$(getconf GNU_LIBC_VERSION 2>/dev/null); then
  v=${v#glibc }; maj=${v%%.*}; min=${v#*.}; min=${min%%.*}
  if ((maj < 2 || (maj == 2 && min < 38))); then
    CLI_FLAGS=(--no-default-features --features embed)
    echo "[pr-check] glibc $v < 2.38: glia-cli builds with ${CLI_FLAGS[*]} (no WebGPU worker)"
  fi
fi

sorted_keys() {   # sorted_keys <assoc array name>: its keys, one per line, in byte order (never hash order)
  local -n arr=$1
  local k
  for k in "${!arr[@]}"; do printf '%s\n' "$k"; done | LC_ALL=C sort
}

# ---- the step runner -----------------------------------------------------------------------------------------------
RAN=0
FAILED=()
step() {   # step <name> <cmd..>: run, log, time, report
  local name=$1 t=$SECONDS rc log
  shift
  log="$LOGDIR/${name//[^A-Za-z0-9_.-]/_}.log"
  "$@" >"$log" 2>&1
  rc=$?
  RAN=$((RAN + 1))
  if ((rc == 0)); then
    echo "[pr-check] step=$name ok $((SECONDS - t))s"
  else
    FAILED+=("$name")
    tail -n 40 "$log" | sed 's/^/    /'
    echo "[pr-check] step=$name FAILED $((SECONDS - t))s (rc=$rc, log $log)"
  fi
  return "$rc"
}
skip() { echo "[pr-check] step=$1 skipped 0s ($2)"; }

echo "[pr-check] base $BASE ($BASE12): ${#CHANGED[@]} changed paths, logs in $LOGDIR (kept only on failure)"
cargo_steps=(workspace ids seams self_scan cli_surface readme_surface skill_surface py_surface wheel)
needs_cargo=${#CRATES[@]}
for s in "${cargo_steps[@]}"; do [[ -n ${WANT[$s]:-} ]] && needs_cargo=1; done
((needs_cargo == 0)) || [[ -d $TARGET_DIR/debug/deps ]] ||
  echo "[pr-check] no warm $TARGET_DIR/debug: this first run compiles the workspace (the engine alone is large)"

# fmt: always.
step fmt cargo fmt --all --check

[[ -n ${WANT[self]:-} ]] && step self bash -c 'bash -n scripts/pr-check.sh &&
  { ! command -v shellcheck >/dev/null || shellcheck scripts/pr-check.sh; }'

# clippy and the tests of the touched crates. glia-cli takes its own invocation when its features differ.
CRATE_LIST=()
mapfile -t CRATE_LIST < <(sorted_keys CRATES)
if [[ -n ${WANT[workspace]:-} ]]; then
  step clippy cargo clippy --workspace --all-targets -- -D warnings
  step tests:workspace cargo test --workspace
elif ((${#CRATE_LIST[@]})); then
  others=() has_cli=0
  for c in "${CRATE_LIST[@]}"; do
    if [[ $c == glia-cli ]]; then has_cli=1; else others+=(-p "$c"); fi
  done
  clippy() {
    if ((${#others[@]})); then cargo clippy "${others[@]}" --all-targets -- -D warnings || return; fi
    if ((has_cli)); then cargo clippy -p glia-cli "${CLI_FLAGS[@]}" --all-targets -- -D warnings; fi
  }
  step clippy clippy
  for c in "${CRATE_LIST[@]}"; do
    if [[ $c == glia-cli ]]; then step "tests:$c" cargo test -p glia-cli "${CLI_FLAGS[@]}"
    else step "tests:$c" cargo test -p "$c"; fi
  done
fi
# A whole crate test run already holds a filtered step: say so instead of running it twice.
covered() {   # covered <crate>: true when that crate's whole suite ran above
  [[ -n ${WANT[workspace]:-} || -n ${CRATES[$1]:-} ]]
}
for s in ids seams self_scan; do
  [[ -n ${WANT[$s]:-} ]] || continue
  filter=$s; [[ $s == ids ]] && filter=id_blocks
  if covered glia-engine; then skip "$s" "in tests:glia-engine"
  else step "$s" cargo test -p glia-engine --test engine_tests "$filter::"; fi
done
if [[ -n ${WANT[cli_surface]:-} ]]; then
  if covered glia-cli; then skip cli_surface "in tests:glia-cli"
  else step cli_surface cargo test -p glia-cli "${CLI_FLAGS[@]}" --bin glia cli_surface; fi
fi
for s in readme_surface skill_surface; do
  [[ -n ${WANT[$s]:-} ]] || continue
  if covered glia-cli; then skip "$s" "in tests:glia-cli"
  else step "$s" cargo test -p glia-cli "${CLI_FLAGS[@]}" --test "$s"; fi
done
if [[ -n ${WANT[py_surface]:-} ]] && ! covered glia-py; then
  step "tests:glia-py" cargo test -p glia-py --test api_surface
fi

# ---- the wheel and the substrate-gap bench -------------------------------------------------------------------------
PY=""
wheel() {
  local out="$LOGDIR/wheels" cdylib deps newer
  command -v maturin >/dev/null || { echo "maturin not found: pip install maturin (or pass --no-wheel)"; return 1; }
  [[ -x $VENV/bin/python ]] || python3 -m venv "$VENV" || return
  # The dev profile: compiled once beside the tests' build, quick to relink; the fixtures are small.
  maturin build -m py/Cargo.toml -o "$out" || return
  # maturin can repackage a stale cdylib (bench/substrate-gap/README.md): a glia-py source newer than the built
  # library means the relink was skipped, so clean the two crates once and build again.
  cdylib=$(ls -t "$TARGET_DIR"/debug/libglia_py.{so,dylib} 2>/dev/null | head -n1)
  deps=$(cargo tree -p glia-py -e normal --prefix none 2>/dev/null | grep -o "($ROOT/[^)]*)" | tr -d '()' |
    sed 's#$#/src#' | sort -u)
  if [[ -n $cdylib && -n $deps ]]; then
    # shellcheck disable=SC2086 # one path per word; src/ only: a test file is never in the cdylib
    newer=$(find $deps -name '*.rs' -newer "$cdylib" -print -quit 2>/dev/null)
    if [[ -n $newer ]]; then
      echo "stale cdylib ($newer is newer): cargo clean -p glia-engine -p glia-py, then build again"
      cargo clean -p glia-engine -p glia-py && maturin build -m py/Cargo.toml -o "$out" || return
    fi
  fi
  "$VENV/bin/python" -m pip install -q --force-reinstall --no-deps "$(ls -t "$out"/glia_py-*.whl | head -n1)" || return
  "$VENV/bin/python" -c 'import glia_py; print("glia_py", glia_py.build_stamp())'
}
if [[ -n ${WANT[wheel]:-} ]]; then
  if ((NO_WHEEL)); then
    PY=${GLIA_PR_CHECK_PY:-}
    [[ -z $PY && -x $VENV/bin/python ]] && PY=$VENV/bin/python
    PY=${PY:-python3}
    skip wheel "--no-wheel: grading with $PY's installed glia_py, which may predate this change"
    if ! "$PY" -c 'import glia_py' >/dev/null 2>&1; then
      echo "[pr-check] $PY cannot import glia_py: drop --no-wheel or set GLIA_PR_CHECK_PY"
      FAILED+=(wheel)
      PY=""
    fi
  elif step wheel wheel; then
    PY=$VENV/bin/python
  fi
fi
bench_step() {   # bench_step <name> <cmd..>: a step that needs the wheel's python
  if [[ -z $PY ]]; then skip "$1" "no glia_py (wheel step)"; else step "$@"; fi
}
grade() {   # grade <fixture dir>: grade.py's exit is the verdict; a missed expectation is reported, never a failure
  local out rc
  out=$(cd bench/substrate-gap && "$PY" grade.py "$1" 2>&1); rc=$?
  printf '%s\n' "$out"
  ((rc == 0)) || return "$rc"
  echo "pr-check: $(grep -c '^    XX ' <<<"$out") expectations not met"
}
if [[ -n ${WANT[grade]:-} ]]; then
  mapfile -t FIX_LIST < <(sorted_keys FIXTURES)
  for f in "${FIX_LIST[@]}"; do
    if [[ ! -f bench/substrate-gap/$f/key.json ]]; then skip "grade:$f" "no key.json (removed)"; continue; fi
    bench_step "grade:$f" grade "$f" || continue
    miss=$(grep -o '^pr-check: [0-9]* expectations' "$LOGDIR/grade_${f//[^A-Za-z0-9_.-]/_}.log" | grep -o '[0-9]*')
    if [[ -n $miss && $miss != 0 ]]; then
      echo "[pr-check] grade $f: $miss expectations not met (shown as blind or partial; a fixture glia fails is" \
        "still a contribution, CONTRIBUTING.md)"
    fi
  done
fi
if [[ -n ${WANT[matrix]:-} ]]; then
  mapfile -t LANG_LIST < <(sorted_keys LANGS)
  # matrix.py grades every declared cell whatever the filter, so two rows cost one run of the whole grid.
  if ((${#LANG_LIST[@]} == 1)) && [[ ${LANG_LIST[0]} != all ]]; then
    bench_step "matrix:${LANG_LIST[0]}" bash -c 'cd bench/substrate-gap && "$0" matrix.py --language "$1" --fail-on-error' \
      "$PY" "${LANG_LIST[0]}"
  elif ((${#LANG_LIST[@]})); then
    bench_step matrix:all bash -c 'cd bench/substrate-gap && "$0" matrix.py --fail-on-error' "$PY"
  fi
fi
if [[ -n ${WANT[bench]:-} ]]; then
  bench_step bench bash -c 'cd bench/substrate-gap && for t in test_grade.py test_matrix.py test_run.py; do
    "$0" "$t" || exit; done' "$PY"
fi
if [[ -n ${WANT[py_surface]:-} ]]; then
  bench_step py_surface bash -c '"$0" py/check_api_surface.py || exit
    for t in py/tests/surface/test_*.py; do "$0" "$t" || exit; done' "$PY"
fi

# ---- the out-of-workspace consumer and the opt-in rank gate --------------------------------------------------------
if [[ -n ${WANT[engram]:-} ]]; then
  if [[ -d $ROOT/../Engram ]]; then step engram bash scripts/check-engram-export.sh
  else skip engram "no ../Engram checkout beside glia"; fi
fi
if ((RANK)); then
  step rank_fast env GLIA_RANK_GATE_ONLY="$RANK_FAST" GLIA_RANK_GATE_JOBS="${GLIA_RANK_GATE_JOBS:-3}" \
    bash scripts/rank-gate.sh check
fi

# ---- the release gates, as information -----------------------------------------------------------------------------
release=()
for gate in rank cohort; do
  declare -n prefixes=${gate^^}_PATHS
  for p in "${CHANGED[@]}"; do
    for pre in "${prefixes[@]}"; do
      if [[ $p == "$pre"* ]]; then release+=("$gate"); break 2; fi
    done
  done
  unset -n prefixes
done
if ((${#release[@]})); then
  echo "[pr-check] release gates this change will run: $(IFS=,; echo "${release[*]}" | sed 's/,/, /g')" \
    "(at the release close-out, never here)"
fi

if ((${#FAILED[@]})); then
  echo "[pr-check] FAILED ($(IFS=,; echo "${FAILED[*]}" | sed 's/,/, /g'))"
  exit 1
fi
rm -rf "$LOGDIR"
echo "[pr-check] ok - $RAN steps in $((SECONDS - T0))s (base $BASE12)"
exit 0
