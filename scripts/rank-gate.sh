#!/usr/bin/env bash
# rank-gate.sh - the 0.5.3 rank gate (EG.3): engine/examples/rank_eval.rs on pinned public snapshots,
# judged against the committed baselines in bench/rank-gate/baseline/ (EG.2's --gate-check).
#
#   bash scripts/rank-gate.sh fetch               bare clones + git-archive copies of bench/rank-gate/snapshots.json
#   bash scripts/rank-gate.sh run                 build rank_eval (release), run every snapshot, trim to gate.json
#   bash scripts/rank-gate.sh check               fetch + run + --gate-check against baseline/0.5.2 AND baseline/current
#   bash scripts/rank-gate.sh baseline <release>  run, then write the trims to baseline/<release>/ (a declared re-baseline)
#   bash scripts/rank-gate.sh baseline-add        merge the last check's `new` gated rankers into baseline/0.5.2 and current
#   bash scripts/rank-gate.sh ratchet             copy the last green check's trims over baseline/current (HEAD must match)
#   bash scripts/rank-gate.sh coverage            every scope item of gate.json `items` is measured or exempt
#
# Env: GLIA_RANK_GATE_DIR (default ~/.cache/glia-rank-gate: the clones, the copies and run/), CARGO_TARGET_DIR
# (where rank_eval is built; default <this checkout>/target). GLIA_RANK_PRIVATE is unset: the gate runs public
# modes only, and only https snapshots marked `"public": true` are ever fetched.
# Last line of `check`, gated on by dev-notes/wave-runner/closeout.py:
#   [rank-gate] check: ok - <rows> rows, <gains> gains, 0 drops on <n> snapshots
#   [rank-gate] check: FAILED (<k> drops: <label>/<shape>/<ranker>/<metric>, ..)      exit 1
#   [rank-gate] check: FAILED (drift <label>/<shape>, ..)                             exit 2
#   [rank-gate] check: FAILED (<other reason>)                                        exit 2
set -uo pipefail
unset GLIA_RANK_PRIVATE

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
GD=$HERE/bench/rank-gate
SNAP=$GD/snapshots.json
GATE=$GD/gate.json
DIR=${GLIA_RANK_GATE_DIR:-$HOME/.cache/glia-rank-gate}
RUN=$DIR/run
TARGET=${CARGO_TARGET_DIR:-$HERE/target}
BIN=$TARGET/release/examples/rank_eval
COMMON=(--round1 on --tune on --pick base,d85 --round2 on --round3 on --r3-perq on --ans on
        --max-commits 300 --max-seeds 5 --boot 1000)
say() { echo "[rank-gate] $*"; }

mkdir -p "$DIR" "$RUN" || { say "check: FAILED (mkdir $DIR)"; exit 2; }

# The snapshot rows as `name url rev label flags..` lines; a row that is not a public https snapshot
# is refused (exit 2), so no private repo or local path ever enters the gate.
rows() {
  python3 -I - "$SNAP" <<'PY'
import json, re, sys
rows = json.load(open(sys.argv[1]))["snapshots"]
bad = 0
for r in rows:
    name = str(r.get("name", "?"))
    ok = (r.get("public") is True and str(r.get("url", "")).startswith("https://")
          and re.fullmatch(r"[0-9a-f]{40}", str(r.get("rev", ""))) is not None
          and re.fullmatch(r"[A-Za-z0-9._-]+", name) is not None
          and re.fullmatch(r"[A-Za-z0-9._-]+", str(r.get("label", ""))) is not None)
    if not ok:
        print(f"[rank-gate] refused {name}: not a public https snapshot", file=sys.stderr)
        bad = 1
for r in rows:
    print(" ".join([r["name"], r["url"], r["rev"], r["label"]] + list(r.get("flags", []))))
sys.exit(2 if bad else 0)
PY
}
ROWS=$(rows) || exit 2

do_fetch() {
  exec 8>"$DIR/.fetch.lock"
  flock 8   # the clones and copies are shared by every agent: one fetch at a time
  local name url rev label flags bare copy tmp
  while read -r name url rev label flags; do
    bare=$DIR/$name.git
    copy=$DIR/$name@${rev:0:12}
    if [ -f "$copy/.complete" ] && git -C "$bare" cat-file -e "$rev^{commit}" 2>/dev/null; then
      say "fetch $name: cached"
      continue
    fi
    if [ ! -d "$bare" ]; then
      git clone --bare --quiet "$url" "$bare.part" && mv "$bare.part" "$bare" \
        || { rm -rf "$bare.part"; say "check: FAILED (fetch $name)"; exit 2; }
    fi
    if ! git -C "$bare" cat-file -e "$rev^{commit}" 2>/dev/null; then
      git -C "$bare" fetch --quiet origin || { say "check: FAILED (fetch $name)"; exit 2; }
      git -C "$bare" cat-file -e "$rev^{commit}" 2>/dev/null || { say "check: FAILED (fetch $name: no commit $rev)"; exit 2; }
    fi
    if [ ! -f "$copy/.complete" ]; then
      # The directory name <name>@<rev12> is part of the build identity (a copy with no .git gets
      # RepoId dir:<basename>): never rename it.
      tmp=$DIR/.tmp-$name-$$
      rm -rf "$tmp" "$copy" && mkdir -p "$tmp" \
        && git -C "$bare" archive "$rev" | tar -x -C "$tmp" \
        && touch "$tmp/.complete" && mv "$tmp" "$copy" \
        || { rm -rf "$tmp"; say "check: FAILED (fetch $name: archive)"; exit 2; }
    fi
    say "fetch $name: $copy"
  done <<< "$ROWS"
  exec 8>&-
}

do_run() {
  local t0 name url rev label flags q
  say "build rank_eval (release) in $TARGET"
  (cd "$HERE" && cargo build --release --quiet -p glia-engine --example rank_eval > "$RUN/build.log" 2>&1) \
    || { tail -20 "$RUN/build.log"; say "check: FAILED (build rank_eval)"; exit 2; }
  export GLIA_RANK_GATE_HEAD
  GLIA_RANK_GATE_HEAD=$(git -C "$HERE" rev-parse HEAD)
  while read -r name url rev label flags; do
    t0=$(date +%s)
    # cwd = run/: anything rank_eval writes by default lands there, never in the repo.
    # shellcheck disable=SC2086
    (cd "$RUN" && GLIA_NO_PERSIST=1 "$BIN" --repo "$DIR/$name@${rev:0:12}" --git "$DIR/$name.git" --rev "$rev" \
        --label "$label" $flags "${COMMON[@]}" --out "$RUN/$label.json" --md "$RUN/$label.md" \
        --r1md "$RUN/$label.r1.md" --r3md "$RUN/$label.r3.md" 2> "$RUN/$label.log") \
      || { tail -5 "$RUN/$label.log"; say "check: FAILED (run $label)"; exit 2; }
    (cd "$RUN" && "$BIN" --gate-trim "$GATE" --gate-new "$RUN/$label.json" --out "$RUN/$label.trim.json" 2> "$RUN/$label.trim.log") \
      || { cat "$RUN/$label.trim.log"; say "check: FAILED (trim $label)"; exit 2; }
    q=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["stats"].get("queries", "?"))' "$RUN/$label.json")
    say "run label=$label queries=$q s=$(( $(date +%s) - t0 ))"
  done <<< "$ROWS"
}

labels() { while read -r _ _ _ label _; do echo "$label"; done <<< "$ROWS"; }

do_check() {
  local label base out rc verdicts=()
  rm -f "$RUN/last-check.json"
  do_fetch
  do_run
  for label in $(labels); do
    for base in 0.5.2 current; do
      out=$RUN/$label.$base.verdict
      [ -f "$GD/baseline/$base/$label.json" ] || { say "check: FAILED (no baseline $base/$label.json)"; exit 2; }
      (cd "$RUN" && "$BIN" --gate-check "$GATE" --gate-base "$GD/baseline/$base/$label.json" --gate-new "$RUN/$label.json" \
          --boot 1000 --out "$out.json" --md "$out.md" 2> "$out.log")
      rc=$?
      grep -E '^\[rank-gate\] (verdict|drift|missing|new) ' "$out.log" | sed "s/^\[rank-gate\] /[rank-gate] base=$base /"
      [ -f "$out.json" ] || { tail -5 "$out.log"; say "check: FAILED (check $label vs $base, exit $rc)"; exit 2; }
      verdicts+=("$base=$out.json:$rc")
    done
  done
  python3 -I - "$RUN/last-check.json" "$GLIA_RANK_GATE_HEAD" "${verdicts[@]}" <<'PY'
import json, sys
dest, head, items = sys.argv[1], sys.argv[2], sys.argv[3:]
rows = gains = 0
drops, drift, missing, other, labels = [], [], [], [], []
gain_rows, new = set(), {}
for it in items:
    base, rest = it.split("=", 1)
    path, rc = rest.rsplit(":", 1)
    v = json.load(open(path))
    lab = v["label"]
    if lab not in labels:
        labels.append(lab)
    for shape, rk in v["rows"].items():
        for ranker, ms in rk.items():
            for metric, r in ms.items():
                rows += 1
                key = f"{lab}/{shape}/{ranker}/{metric}"
                if r["verdict"] == "drop" and key not in drops:
                    drops.append(key)
                if r["verdict"] == "gain":
                    gain_rows.add(key)
    for d in v["drift"]:
        k = f"{lab}/{d.replace(':', '/')}"
        if k not in drift:
            drift.append(k)
    for m in v["missing"]:
        k = f"{lab}/{m.replace(':', '/')}"
        if k not in missing:
            missing.append(k)
    for n in v["new"]:
        new.setdefault(lab, [])
        if n not in new[lab]:
            new[lab].append(n)
    if rc not in ("0", "1", "2"):
        other.append(f"{lab} vs {base} exit {rc}")
ok = not (drops or drift or missing or other)
json.dump({"ok": ok, "glia_head": head, "labels": labels, "rows": rows, "gains": len(gain_rows),
           "drops": drops, "drift": drift, "missing": missing, "new": new}, open(dest, "w"), indent=1, sort_keys=True)
if drift:
    print(f"[rank-gate] check: FAILED (drift {', '.join(drift)})"); sys.exit(2)
if missing or other:
    print(f"[rank-gate] check: FAILED (missing {', '.join(missing + other)})"); sys.exit(2)
if drops:
    print(f"[rank-gate] check: FAILED ({len(drops)} drops: {', '.join(drops)})"); sys.exit(1)
print(f"[rank-gate] check: ok - {rows} rows, {len(gain_rows)} gains, 0 drops on {len(labels)} snapshots")
PY
  exit $?
}

do_baseline() {
  local rel=${1:-} label
  [[ "$rel" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { say "baseline: FAILED (usage: baseline <release>, e.g. 0.5.2)"; exit 2; }
  do_fetch
  do_run
  mkdir -p "$GD/baseline/$rel"
  for label in $(labels); do cp "$RUN/$label.trim.json" "$GD/baseline/$rel/$label.json" || exit 2; done
  say "baseline: ok - $(labels | wc -l) snapshots -> bench/rank-gate/baseline/$rel/ (head $GLIA_RANK_GATE_HEAD)"
}

# The last check, refused unless it was green and made at this HEAD.
last_green() {
  python3 -I - "$RUN/last-check.json" "$(git -C "$HERE" rev-parse HEAD)" "$1" <<'PY'
import json, sys
try:
    c = json.load(open(sys.argv[1]))
except (OSError, ValueError):
    print(f"[rank-gate] {sys.argv[3]}: FAILED (no last check recorded)"); sys.exit(2)
if not c.get("ok"):
    print(f"[rank-gate] {sys.argv[3]}: FAILED (the last check was not ok)"); sys.exit(2)
if c.get("glia_head") != sys.argv[2]:
    print(f"[rank-gate] {sys.argv[3]}: FAILED (the last check ran at {c.get('glia_head')}, HEAD is {sys.argv[2]})"); sys.exit(2)
PY
}

do_ratchet() {
  last_green ratchet || exit 2
  python3 -I - "$RUN" "$GD/baseline/current" $(labels) <<'PY'
import json, os, shutil, sys
run, cur, labels = sys.argv[1], sys.argv[2], sys.argv[3:]
moved = []
for lab in labels:
    src, dst = os.path.join(run, f"{lab}.trim.json"), os.path.join(cur, f"{lab}.json")
    new = json.load(open(src))
    try:
        old = json.load(open(dst))
    except (OSError, ValueError):
        old = None
    # glia_head moves every commit: only a moved value (or a new column) rewrites the file.
    strip = lambda d: {k: v for k, v in d.items() if k != "glia_head"} if d else d
    if strip(old) != strip(new):
        shutil.copyfile(src, dst)
        moved.append(lab)
print(f"[rank-gate] ratchet: ok - {len(moved)} of {len(labels)} baselines moved{': ' + ', '.join(moved) if moved else ''}")
PY
}

do_baseline_add() {
  last_green baseline-add || exit 2
  python3 -I - "$RUN" "$GD/baseline" <<'PY'
import json, os, sys
run, bdir = sys.argv[1], sys.argv[2]
c = json.load(open(os.path.join(run, "last-check.json")))
added = 0
for lab, news in sorted(c.get("new", {}).items()):
    trim = json.load(open(os.path.join(run, f"{lab}.trim.json")))
    for rel in ("0.5.2", "current"):
        path = os.path.join(bdir, rel, f"{lab}.json")
        base = json.load(open(path))
        before = added
        for n in news:
            shape, ranker = n.split(":", 1)
            t, b = trim["shapes"][shape], base["shapes"][shape]
            if t["ids"] != b["ids"] or t["commit_of"] != b["commit_of"]:
                print(f"[rank-gate] baseline-add: FAILED (ids differ {rel}/{lab}/{shape})"); sys.exit(2)
            for m, cols in t.items():
                if m in ("ids", "commit_of") or ranker not in cols:
                    continue
                if ranker not in b.setdefault(m, {}):   # adds columns only, never rewrites one
                    b[m][ranker] = cols[ranker]
                    added += 1
        if added == before:
            continue
        # rank_eval's trim format: compact, keys sorted (serde_json over BTreeMaps)
        with open(path, "w") as f:
            json.dump(base, f, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
print(f"[rank-gate] baseline-add: ok - {added} columns added")
PY
}

do_coverage() {
  python3 -I - "$GATE" <<'PY'
import json, sys
g = json.load(open(sys.argv[1]))
gated = {r for rs in g["rankers"].values() for r in rs}
items = g.get("items", {})
measured, exempt, unmeasured = 0, 0, []
for k in sorted(set(items) | {str(i) for i in range(1, 12)}, key=lambda s: (len(s), s)):
    v = items.get(k)
    if isinstance(v, dict) and v.get("exempt"):
        exempt += 1
    elif isinstance(v, list) and v and all(r in gated for r in v):
        measured += 1
    else:
        unmeasured.append(k)
if unmeasured:
    print(f"[rank-gate] coverage: FAILED (unmeasured items: {','.join(unmeasured)})"); sys.exit(1)
print(f"[rank-gate] coverage: ok - {measured} items measured, {exempt} exempt")
PY
}

case "${1:-}" in
  fetch) do_fetch ;;
  run) exec 9>"$DIR/.run.lock"; flock 9; do_fetch; do_run ;;
  check)
    exec 9>"$DIR/.run.lock"; flock 9   # run/ is shared: one run at a time
    do_check ;;
  baseline) exec 9>"$DIR/.run.lock"; flock 9; do_baseline "${2:-}" ;;
  baseline-add) exec 9>"$DIR/.run.lock"; flock 9; do_baseline_add ;;
  ratchet) exec 9>"$DIR/.run.lock"; flock 9; do_ratchet ;;
  coverage) do_coverage ;;
  *) echo "usage: $0 fetch|run|check|baseline <release>|baseline-add|ratchet|coverage" >&2; exit 2 ;;
esac
