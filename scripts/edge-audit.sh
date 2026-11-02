#!/usr/bin/env bash
# edge-audit.sh - the 0.5.4 edge-precision audit (FD.2): engine/examples/edge_audit.rs over the trees of
# bench/edge-audit/snapshots.json (glia itself at a pinned rev, three rank-gate public snapshots and,
# since FD.3d (ruling FD-2), the cohort's cross-service repo microservices-demo). Since 0.5.7 (HC.2) the
# rows marked `"panel": "heldout"` are the calibrated-confidence held-out panel (seven cohort repos the
# fit never saw), sampled into their own file and judged against the fit by the rule in
# dev-notes/confidence-gate-0.5.7.md.
#
#   bash scripts/edge-audit.sh sample              fetch the rank-gate snapshots, extract glia and the cohort rows,
#                                                  sample every fit row into bench/edge-audit/samples/audit.jsonl
#   bash scripts/edge-audit.sh sample-heldout      extract the held-out rows, sample them (n 10 per pooled stratum)
#                                                  into bench/edge-audit/samples/heldout.jsonl; the fit is not re-drawn
#   bash scripts/edge-audit.sh show <label> [--id <hex>]   print samples with live source excerpts from the snapshot copy
#   bash scripts/edge-audit.sh report [--check] [--judge <name>]   bench/edge-audit/report.{json,md} from the fit panel's
#                                                  samples + verdicts (held-out files beside them never enter it)
#   bash scripts/edge-audit.sh calibrate [--check] bench/edge-audit/calibration.{json,md}: the held-out panel against
#                                                  the fit report at FIT_REV, by the committed rule
#
# Env: GLIA_RANK_GATE_DIR (default ~/.cache/glia-rank-gate; shared with the rank gate, never deleted),
# GLIA_COHORT_DIR (default ~/.cache/glia-cohort; the cohort's bare clones, read only, never fetched here),
# CARGO_TARGET_DIR (default <this checkout>/target). GLIA_RANK_PRIVATE is unset: only public snapshots are audited.
# Every line this script prints starts `[edge-audit]`.
set -uo pipefail
unset GLIA_RANK_PRIVATE

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
AD=$HERE/bench/edge-audit
PLAN=$AD/floors.json
SNAP=$AD/snapshots.json
GATE_SNAP=$HERE/bench/rank-gate/snapshots.json
COHORT_REPOS=$HERE/bench/cohort/repos.json
SAMPLES=$AD/samples/audit.jsonl
HELDOUT=$AD/samples/heldout.jsonl
# The calibration's fit (dev-notes/confidence-gate-0.5.7.md): the 0.5.6 audit (GP.14) as committed at this
# sha, read with `git show`, never from the working tree, so a later re-audit moves no calibration number.
FIT_REV=cd541240355e9d238bebcbdf0eb8ac269ec29557
# The cohort baseline whose name-drift rows the calibration reports (information only).
COHORT_REV=9d66de3d04d98366af921af038728a3e7171e2c6
DIR=${GLIA_RANK_GATE_DIR:-$HOME/.cache/glia-rank-gate}
COHORT=${GLIA_COHORT_DIR:-$HOME/.cache/glia-cohort}
TARGET=${CARGO_TARGET_DIR:-$HERE/target}
BIN=$TARGET/release/examples/edge_audit
say() { echo "[edge-audit] $*"; }

# The audited trees of one panel (`fit`, `heldout` or `all`) as `name source panel dir label rev skip..`
# lines, source one of self / rank-gate / cohort, panel `fit` (a row without `panel`) or `heldout`.
# A rank-gate row is read by name from bench/rank-gate/snapshots.json and a cohort row by label from
# bench/cohort/repos.json (url and rev never copied); either must be a public https snapshot pinned at
# a 40-hex commit. The self row is this repository at its `rev`: a 40-hex commit, or HEAD when absent
# or "HEAD". A name may appear once.
rows() {
  python3 -I - "$SNAP" "$GATE_SNAP" "$COHORT_REPOS" "$DIR" "$(git -C "$HERE" rev-parse HEAD)" "${1:-fit}" <<'PY'
import json, re, sys
audit = json.load(open(sys.argv[1]))["snapshots"]
gate = {r["name"]: r for r in json.load(open(sys.argv[2]))["snapshots"]}
cohort = {r["label"]: r for r in json.load(open(sys.argv[3]))["repos"]}
d, head, want = sys.argv[4], sys.argv[5], sys.argv[6]
hex40 = lambda v: re.fullmatch(r"[0-9a-f]{40}", str(v)) is not None
bad = 0
seen = set()
for a in audit:
    name, source = str(a.get("name", "?")), a.get("source")
    panel = a.get("panel", "fit")
    if panel not in ("fit", "heldout") or name in seen:
        why = f"panel {panel!r} is not fit|heldout" if name not in seen else "listed twice"
        print(f"[edge-audit] refused {name}: {why}", file=sys.stderr)
        bad = 1
        continue
    seen.add(name)
    if want != "all" and panel != want:
        continue
    if source == "self":
        rev = a.get("rev", "HEAD")
        if rev == "HEAD":
            rev = head
        if not hex40(rev):
            print(f"[edge-audit] refused {name}: self rev {rev!r} is neither HEAD nor a 40-hex commit", file=sys.stderr)
            bad = 1
            continue
        label = name
    elif source == "cohort":
        c = cohort.get(name)
        if not (c is not None and c.get("public") is True and str(c.get("url", "")).startswith("https://")
                and hex40(c.get("sha", ""))):
            print(f"[edge-audit] refused {name}: not a public cohort repo", file=sys.stderr)
            bad = 1
            continue
        rev, label = c["sha"], c["label"]
    elif source == "rank-gate":
        g = gate.get(name)
        if not (g is not None and g.get("public") is True and str(g.get("url", "")).startswith("https://")
                and hex40(g.get("rev", ""))):
            print(f"[edge-audit] refused {name}: not a public rank-gate snapshot", file=sys.stderr)
            bad = 1
            continue
        rev, label = g["rev"], g["label"]
    else:
        print(f"[edge-audit] refused {name}: unknown source {source!r}", file=sys.stderr)
        bad = 1
        continue
    print(" ".join([name, source, panel, f"{d}/{name}@{rev[:12]}", label, rev] + list(a.get("skips", []))))
sys.exit(2 if bad else 0)
PY
}

build() {
  say "build edge_audit (release) in $TARGET"
  mkdir -p "$DIR/edge-audit" || { say "FAILED (mkdir $DIR/edge-audit)"; exit 2; }
  (cd "$HERE" && cargo build --release --quiet -p glia-engine --example edge_audit > "$DIR/edge-audit/build.log" 2>&1) \
    || { tail -20 "$DIR/edge-audit/build.log"; say "FAILED (build edge_audit)"; exit 2; }
}

# A git-archive copy of `rev` from the git dir `src` at `copy`, beside the rank-gate copies (never the
# gate's own glia row): glia itself (src = this checkout) or a cohort repo (src = its bare clone).
archive_copy() {
  local src=$1 rev=$2 copy=$3 what=$4 tmp
  [ -f "$copy/.complete" ] && return 0
  tmp=$DIR/.tmp-edge-audit-$$
  rm -rf "$tmp" "$copy" && mkdir -p "$tmp" \
    && git -C "$src" archive "$rev" | tar -x -C "$tmp" \
    && touch "$tmp/.complete" && mv "$tmp" "$copy" \
    || { rm -rf "$tmp"; say "FAILED (archive $what $rev)"; exit 2; }
}

# Extract one panel's copies and sample them into `out` (`--panel heldout` for the held-out panel).
sample_panel() {
  local panel=$1 out=$2 t0 r name source pn dir label rev skips args=() s gd
  t0=$(date +%s)
  r=$(rows "$panel") || exit 2
  [ -n "$r" ] || { say "FAILED (no $panel rows in $SNAP)"; exit 2; }
  if grep -q '^[^ ]* rank-gate ' <<< "$r"; then
    bash "$HERE/scripts/rank-gate.sh" fetch | sed 's/^/[edge-audit] /' \
      || { say "FAILED (rank-gate fetch)"; exit 2; }
  fi
  while read -r name source pn dir label rev skips; do
    case "$source" in
      self) archive_copy "$HERE" "$rev" "$dir" "$name" ;;
      cohort)
        gd=$COHORT/$name.git
        [ -f "$dir/.complete" ] || git -C "$gd" cat-file -e "$rev^{commit}" 2>/dev/null \
          || { say "FAILED (no cohort clone of $name at $rev in $gd: run python3 bench/cohort/cohort.py to fetch it)"; exit 2; }
        archive_copy "$gd" "$rev" "$dir" "$name" ;;
    esac
    [ -f "$dir/.complete" ] || { say "FAILED (no snapshot copy $dir)"; exit 2; }
    args+=(--repo "$dir" --label "$label" --rev "$rev")
    for s in $skips; do args+=(--skip "$s"); done
  done <<< "$r"
  build
  mkdir -p "$AD/samples"
  GLIA_NO_PERSIST=1 "$BIN" sample "${args[@]}" --plan "$PLAN" --panel "$panel" --out "$out" \
    2> "$DIR/edge-audit/sample-$panel.log"
  local rc=$?
  grep '^\[edge-audit\]' "$DIR/edge-audit/sample-$panel.log"
  [ $rc -eq 0 ] || { tail -5 "$DIR/edge-audit/sample-$panel.log"; say "FAILED (sample, exit $rc)"; exit 2; }
  say "sample: wrote $out panel=$panel s=$(( $(date +%s) - t0 ))"
}

do_sample() { sample_panel fit "$SAMPLES"; }
do_sample_heldout() { sample_panel heldout "$HELDOUT"; }

do_show() {
  local label=${1:-} r name source pn dir lab rev skips root="" file=$SAMPLES
  [ -n "$label" ] || { echo "usage: $0 show <label> [--id <hex>]" >&2; exit 2; }
  shift
  r=$(rows all) || exit 2
  while read -r name source pn dir lab rev skips; do
    if [ "$lab" = "$label" ]; then
      root=$dir
      [ "$pn" = heldout ] && file=$HELDOUT
    fi
  done <<< "$r"
  [ -n "$root" ] || { say "FAILED (no snapshot labelled $label)"; exit 2; }
  [ -f "$file" ] || { say "FAILED (no $file: run sample first)"; exit 2; }
  if [ "$label" = glia ]; then
    # The self copy is the revision the samples were drawn at, read from their header.
    rev=$(python3 -I -c 'import json,sys
for l in open(sys.argv[1]):
    h = json.loads(l)
    if "stratum_sizes" in h and h["label"] == sys.argv[2]: print(h["rev"]); break' "$SAMPLES" glia)
    root=$DIR/glia@${rev:0:12}
  fi
  build
  "$BIN" show "$file" --root "$root" --label "$label" "$@"
}

do_report() {
  build
  "$BIN" report --samples "$AD/samples/*.jsonl" --verdicts "$AD/verdicts/*.jsonl" --plan "$PLAN" \
    --out-json "$AD/report.json" --out-md "$AD/report.md" "$@"
}

# The calibration's pinned inputs under $DIR/edge-audit/fit@<rev12>/: the fit report, its samples, verdicts
# and adjudication at FIT_REV, and the cohort baseline's tasks at COHORT_REV.
fit_inputs() {
  local d=$DIR/edge-audit/fit@${FIT_REV:0:12} f
  rm -rf "$d" && mkdir -p "$d/samples" "$d/verdicts" || { say >&2 "FAILED (mkdir $d)"; exit 2; }
  git -C "$HERE" show "$FIT_REV:bench/edge-audit/report.json" > "$d/report.json" \
    || { say >&2 "FAILED (no fit report at $FIT_REV)"; exit 2; }
  git -C "$HERE" show "$FIT_REV:bench/edge-audit/adjudication.jsonl" > "$d/adjudication.jsonl" \
    || { say >&2 "FAILED (no fit adjudication at $FIT_REV)"; exit 2; }
  for f in $(git -C "$HERE" ls-tree --name-only "$FIT_REV" bench/edge-audit/samples/ bench/edge-audit/verdicts/); do
    git -C "$HERE" show "$FIT_REV:$f" > "$d/$(basename "$(dirname "$f")")/$(basename "$f")" \
      || { say >&2 "FAILED (git show $FIT_REV:$f)"; exit 2; }
  done
  git -C "$HERE" show "$COHORT_REV:bench/cohort/results/baseline/tasks.jsonl" > "$d/cohort-tasks.jsonl" \
    || { say >&2 "FAILED (no cohort tasks at $COHORT_REV)"; exit 2; }
  echo "$d"
}

do_calibrate() {
  local d
  build
  d=$(fit_inputs) || exit 2
  "$BIN" report --calibration --fit "$d/report.json" --fit-samples "$d/samples/*.jsonl" \
    --fit-verdicts "$d/verdicts/*.jsonl" --fit-adjudication "$d/adjudication.jsonl" \
    --samples "$AD/samples/*.jsonl" --verdicts "$AD/verdicts/*.jsonl" --plan "$PLAN" \
    --cohort-tasks "$d/cohort-tasks.jsonl" \
    --out-json "$AD/calibration.json" --out-md "$AD/calibration.md" "$@"
}

case "${1:-}" in
  sample) do_sample ;;
  sample-heldout) do_sample_heldout ;;
  show) shift; do_show "$@" ;;
  report) shift; do_report "$@" ;;
  calibrate) shift; do_calibrate "$@" ;;
  *) echo "usage: $0 sample|sample-heldout|show <label> [--id <hex>]|report [--check] [--judge <name>]|calibrate [--check]" >&2; exit 2 ;;
esac
