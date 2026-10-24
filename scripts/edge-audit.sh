#!/usr/bin/env bash
# edge-audit.sh - the 0.5.4 edge-precision audit (FD.2): engine/examples/edge_audit.rs over the trees of
# bench/edge-audit/snapshots.json (glia itself at HEAD + three rank-gate public snapshots).
#
#   bash scripts/edge-audit.sh sample              fetch the rank-gate snapshots, extract glia@HEAD, sample all four
#                                                  into bench/edge-audit/samples/audit.jsonl
#   bash scripts/edge-audit.sh show <label> [--id <hex>]   print samples with live source excerpts from the snapshot copy
#   bash scripts/edge-audit.sh report [--check] [--judge <name>]   bench/edge-audit/report.{json,md} from samples + verdicts
#
# Env: GLIA_RANK_GATE_DIR (default ~/.cache/glia-rank-gate; shared with the rank gate, never deleted),
# CARGO_TARGET_DIR (default <this checkout>/target). GLIA_RANK_PRIVATE is unset: only public snapshots are audited.
# Every line this script prints starts `[edge-audit]`.
set -uo pipefail
unset GLIA_RANK_PRIVATE

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
AD=$HERE/bench/edge-audit
PLAN=$AD/floors.json
SNAP=$AD/snapshots.json
GATE_SNAP=$HERE/bench/rank-gate/snapshots.json
SAMPLES=$AD/samples/audit.jsonl
DIR=${GLIA_RANK_GATE_DIR:-$HOME/.cache/glia-rank-gate}
TARGET=${CARGO_TARGET_DIR:-$HERE/target}
BIN=$TARGET/release/examples/edge_audit
say() { echo "[edge-audit] $*"; }

# The audited trees as `name dir label rev skip..` lines. A rank-gate row is read by name from
# bench/rank-gate/snapshots.json (url and rev never copied) and must be a public https snapshot; the
# self row is this repository at HEAD.
rows() {
  python3 -I - "$SNAP" "$GATE_SNAP" "$DIR" "$(git -C "$HERE" rev-parse HEAD)" <<'PY'
import json, re, sys
audit = json.load(open(sys.argv[1]))["snapshots"]
gate = {r["name"]: r for r in json.load(open(sys.argv[2]))["snapshots"]}
d, head = sys.argv[3], sys.argv[4]
bad = 0
for a in audit:
    name = str(a.get("name", "?"))
    if a.get("source") == "self":
        rev, label = head, name
    else:
        g = gate.get(name)
        ok = (g is not None and g.get("public") is True and str(g.get("url", "")).startswith("https://")
              and re.fullmatch(r"[0-9a-f]{40}", str(g.get("rev", ""))) is not None)
        if not ok:
            print(f"[edge-audit] refused {name}: not a public rank-gate snapshot", file=sys.stderr)
            bad = 1
            continue
        rev, label = g["rev"], g["label"]
    print(" ".join([name, f"{d}/{name}@{rev[:12]}", label, rev] + list(a.get("skips", []))))
sys.exit(2 if bad else 0)
PY
}

build() {
  say "build edge_audit (release) in $TARGET"
  mkdir -p "$DIR/edge-audit" || { say "FAILED (mkdir $DIR/edge-audit)"; exit 2; }
  (cd "$HERE" && cargo build --release --quiet -p glia-engine --example edge_audit > "$DIR/edge-audit/build.log" 2>&1) \
    || { tail -20 "$DIR/edge-audit/build.log"; say "FAILED (build edge_audit)"; exit 2; }
}

# glia itself at HEAD as a git-archive copy beside the rank-gate copies (never the gate's own glia row).
self_copy() {
  local rev=$1 copy=$2 tmp
  [ -f "$copy/.complete" ] && return 0
  tmp=$DIR/.tmp-edge-audit-$$
  rm -rf "$tmp" "$copy" && mkdir -p "$tmp" \
    && git -C "$HERE" archive "$rev" | tar -x -C "$tmp" \
    && touch "$tmp/.complete" && mv "$tmp" "$copy" \
    || { rm -rf "$tmp"; say "FAILED (archive glia $rev)"; exit 2; }
}

do_sample() {
  local t0 r name dir label rev skips args=() s
  t0=$(date +%s)
  bash "$HERE/scripts/rank-gate.sh" fetch | sed 's/^/[edge-audit] /' \
    || { say "FAILED (rank-gate fetch)"; exit 2; }
  r=$(rows) || exit 2
  while read -r name dir label rev skips; do
    if [ "$name" = glia ] && [ ! -f "$dir/.complete" ]; then self_copy "$rev" "$dir"; fi
    [ -f "$dir/.complete" ] || { say "FAILED (no snapshot copy $dir)"; exit 2; }
    args+=(--repo "$dir" --label "$label" --rev "$rev")
    for s in $skips; do args+=(--skip "$s"); done
  done <<< "$r"
  build
  mkdir -p "$AD/samples"
  GLIA_NO_PERSIST=1 "$BIN" sample "${args[@]}" --plan "$PLAN" --out "$SAMPLES" 2> "$DIR/edge-audit/sample.log"
  local rc=$?
  grep '^\[edge-audit\]' "$DIR/edge-audit/sample.log"
  [ $rc -eq 0 ] || { tail -5 "$DIR/edge-audit/sample.log"; say "FAILED (sample, exit $rc)"; exit 2; }
  say "sample: wrote $SAMPLES s=$(( $(date +%s) - t0 ))"
}

do_show() {
  local label=${1:-} r name dir lab rev skips root=""
  [ -n "$label" ] || { echo "usage: $0 show <label> [--id <hex>]" >&2; exit 2; }
  shift
  [ -f "$SAMPLES" ] || { say "FAILED (no $SAMPLES: run sample first)"; exit 2; }
  r=$(rows) || exit 2
  while read -r name dir lab rev skips; do [ "$lab" = "$label" ] && root=$dir; done <<< "$r"
  [ -n "$root" ] || { say "FAILED (no snapshot labelled $label)"; exit 2; }
  if [ "$label" = glia ]; then
    # The self copy is the revision the samples were drawn at, read from their header.
    rev=$(python3 -I -c 'import json,sys
for l in open(sys.argv[1]):
    h = json.loads(l)
    if "stratum_sizes" in h and h["label"] == sys.argv[2]: print(h["rev"]); break' "$SAMPLES" glia)
    root=$DIR/glia@${rev:0:12}
  fi
  build
  "$BIN" show "$SAMPLES" --root "$root" --label "$label" "$@"
}

do_report() {
  build
  "$BIN" report --samples "$AD/samples/*.jsonl" --verdicts "$AD/verdicts/*.jsonl" --plan "$PLAN" \
    --out-json "$AD/report.json" --out-md "$AD/report.md" "$@"
}

case "${1:-}" in
  sample) do_sample ;;
  show) shift; do_show "$@" ;;
  report) shift; do_report "$@" ;;
  *) echo "usage: $0 sample|show <label> [--id <hex>]|report [--check] [--judge <name>]" >&2; exit 2 ;;
esac
