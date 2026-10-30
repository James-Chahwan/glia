#!/usr/bin/env bash
# cohort-gate.sh - the 0.5.4 cohort gate (FH.12): the outlier cohort (bench/cohort/, FH.1, no model), the
# boundary health of each cohort repo (`glia health`, FH.2) and the Sibyl cover (bench/sibyl/, FH.11), all
# run with the tree's glia and judged against the committed baseline in bench/cohort/results/baseline/.
# The rank gate (scripts/rank-gate.sh) blocks a ranking regression; this one blocks glia going blind on a
# real repo or a known client / server combination.
#
#   bash scripts/cohort-gate.sh check [opts]       build glia, run everything, compare with the baseline
#   bash scripts/cohort-gate.sh baseline [opts]    run everything, then write bench/cohort/results/baseline/
#                                                  (a declared re-baseline: James's or a close-out's call)
#   bash scripts/cohort-gate.sh ratchet            the last green full check's results over the baseline
#                                                  (HEAD must match; held while any decline is in tolerance)
#   bash scripts/cohort-gate.sh run [opts]         run everything into the scratch dir, compare nothing
#   bash scripts/cohort-gate.sh compare <base> <new> [--only l,..]   compare two results dirs (read-only)
#   bash scripts/cohort-gate.sh selftest           the compare rules on seeded results (no glia, no network)
#
#   opts: --glia <bin>    the glia under test (default: `cargo build --release -p glia-cli` in the target dir)
#         --only <l,..>   only these cohort repos (a partial check: its marker says so; never ratcheted)
#         --jobs <n>      workers for the cohort, the health builds and the Sibyl cases (default 4)
#
# What is compared (`compare`), base -> new, per repo and per cell; a drop fails, a gain is listed:
#   - cohort health (summary.json repos.<label>.health): client_reach / route_reach down more than the
#     tolerance (0.05) or lost, a new blind flag, a new probe error;
#   - boundary health (boundary.jsonl, `glia health --json` at the pinned sha): the verdict, the parse
#     verdict and each mechanism's verdict worse (OK < DEGRADED < BLIND), each mechanism's client / server
#     reach down more than the tolerance or lost, an app turned library, a new error;
#   - every cohort task check (summary.json tasks: locate, rest, gate = the client named, clear, names) that
#     passed and does not now;
#   - every Sibyl matrix cell and chain cell level (full > partial > none) worse, and every Sibyl case check
#     (pair, drift_s, clear_c, client) that passed and does not now (one glia writes the same bytes twice).
#   A reach down by at most the tolerance is a `decline`: it passes, is listed, and holds the ratchet.
#   A changed corpus (tasks_sha256, the repo set, the Sibyl seed / transports / case ids) is drift: exit 2.
#
# Before a run: `cohort.py build-tasks --check` (the corpus re-derives offline from the bare clones); after
# a full run: `cohort.py check` on the scratch results (every stored check re-derives). Clones are reused
# from ${GLIA_COHORT_DIR:-~/.cache/glia-cohort}; scratch lives in ${GLIA_COHORT_GATE_DIR:-~/.cache/glia-cohort-gate}
# (run/results/baseline/ is the new results dir, same layout as the baseline; sibyl/ the rendered cases).
# GLIA_COHORT_GATE_SKIP=<reason>: print `check: skipped - <reason>` and exit 3 (a close-out that cannot
# afford the run records it, never a silent pass).
#
# Last line of `check`, gated on by dev-notes/wave-runner/closeout.py:
#   [cohort-gate] check: ok - <repos> repos, <tasks> tasks, <cells> cells, <gains> gains, 0 drops
#   [cohort-gate] check: FAILED (<n> drops: <key> <old>-><new>, ..)                     exit 1
#   [cohort-gate] check: FAILED (<reason>)                                               exit 2
#   [cohort-gate] check: skipped - <reason>                                              exit 3
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CB=$HERE/bench/cohort
BASE=$CB/results/baseline
DIR=${GLIA_COHORT_GATE_DIR:-$HOME/.cache/glia-cohort-gate}
RUN=$DIR/run
NEW=$RUN/results/baseline
TARGET=${CARGO_TARGET_DIR:-$HERE/target}
TOL=0.05
say() { echo "[cohort-gate] $*"; }

CMD=${1:-}
[ $# -gt 0 ] && shift
GLIA="" ONLY="" JOBS=4 POS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --glia) GLIA=${2:-}; shift 2 ;;
    --only) ONLY=${2:-}; shift 2 ;;
    --jobs) JOBS=${2:-}; shift 2 ;;
    *) POS+=("$1"); shift ;;
  esac
done
[[ "$JOBS" =~ ^[1-9][0-9]?$ ]] || { say "$CMD: FAILED (--jobs takes 1..99)"; exit 2; }
[[ -z "$ONLY" || "$ONLY" =~ ^[A-Za-z0-9._,-]+$ ]] || { say "$CMD: FAILED (--only takes labels)"; exit 2; }

case "$DIR" in
  "$HERE"|"$HERE"/*) say "$CMD: FAILED (GLIA_COHORT_GATE_DIR is inside this checkout)"; exit 2 ;;
esac

# --- compare ------------------------------------------------------------------------------------------
# compare <base> <new> <out.json> <only>: prints the drop / gain / decline lines and the verdict line.
do_compare() {
  python3 -I - "$1" "$2" "$3" "$4" "$TOL" "${5:-check}" <<'PY'
import json, os, sys
base_d, new_d, out, only, tol, verb = sys.argv[1:7]
tol = float(tol)
only = [x for x in only.split(",") if x]
LEVEL = {"none": 0, "partial": 1, "full": 2}
VERDICT = {"OK": 0, "DEGRADED": 1, "BLIND": 2}
drops, gains, declines, drift = [], [], [], []


def load(d, rel, required=True):
    """A results file; `compare` (not `check`) reads a base made before a section existed (a cohort
    results dir with no boundary.jsonl or sibyl/) and skips that section."""
    required = required or verb == "check"
    p = os.path.join(d, rel)
    try:
        if rel.endswith(".jsonl"):
            return [json.loads(x) for x in open(p) if x.strip()]
        return json.load(open(p))
    except (OSError, ValueError) as e:
        if required:
            drift.append(f"unreadable {os.path.basename(d.rstrip('/'))}/{rel}: {e.__class__.__name__}")
        else:
            print(f"[cohort-gate] {verb}: {rel} not in {d}, section skipped")
        return None


def num(x):
    return round(float(x), 3) if isinstance(x, (int, float)) and not isinstance(x, bool) else None


def reach(key, b, n):
    b, n = num(b), num(n)
    if b is None:
        return
    if n is None:
        # 0.0 -> none: a side with nothing paired lost all its producers / consumers (FH.26 leaves
        # test-file producers out); nothing paired was lost, so it is no drop
        (declines if b == 0 else drops).append(f"{key} {b}->none")
        return
    d = round(n - b, 3)
    if d < -tol - 1e-9:
        drops.append(f"{key} {b}->{n}")
    elif d < 0:
        declines.append(f"{key} {b}->{n}")
    elif d > 0:
        gains.append(f"{key} {b}->{n}")


def rank(key, b, n, order):
    if b not in order or n not in order:
        return
    worse = order[n] < order[b] if order is LEVEL else order[n] > order[b]
    if worse:
        drops.append(f"{key} {b}->{n}")
    elif order[n] != order[b]:
        gains.append(f"{key} {b}->{n}")


def status(key, b, n):
    if b == "pass" and n != "pass":
        drops.append(f"{key} pass->{n}")
    elif n == "pass" and b != "pass":
        gains.append(f"{key} {b}->pass")


def task_repo(tid):
    return tid.rsplit("-", 1)[0]


# --- the cohort: corpus, per repo health, per task check
bm, nm = load(base_d, "meta.json"), load(new_d, "meta.json")
bs, ns = load(base_d, "summary.json"), load(new_d, "summary.json")
repos = tasks = cells = 0
if bm and nm and bm.get("tasks_sha256") != nm.get("tasks_sha256"):
    drift.append(f"corpus moved: tasks_sha256 {bm.get('tasks_sha256')} -> {nm.get('tasks_sha256')}")
if bs and ns and not drift:
    labels = sorted(bs["repos"])
    if sorted(ns["repos"]) != labels:
        drift.append("corpus moved: the repo set differs")
    for lab in only:
        if lab not in labels:
            drift.append(f"--only names no cohort repo: {lab}")
    judged = [lab for lab in labels if not only or lab in only]
    for lab in judged:
        if drift:
            break
        repos += 1
        bh, nh = bs["repos"][lab]["health"], ns["repos"][lab]["health"]
        for k in ("client_reach", "route_reach"):
            reach(f"{lab}/cohort/{k}", bh.get(k), nh.get(k))
        for f in sorted(set(nh.get("blind") or []) - set(bh.get("blind") or [])):
            drops.append(f"{lab}/cohort/blind +{f}")
        for f in sorted(set(bh.get("blind") or []) - set(nh.get("blind") or [])):
            gains.append(f"{lab}/cohort/blind -{f}")
        if nh.get("error") and not bh.get("error"):
            drops.append(f"{lab}/cohort/error {str(nh['error'])[:60]}")
    for tid in sorted(bs["tasks"]):
        if task_repo(tid) not in judged or drift:
            continue
        nt = ns["tasks"].get(tid)
        if nt is None:
            drift.append(f"missing task {tid}")
            continue
        tasks += 1
        for c in sorted(bs["tasks"][tid]):
            status(f"{tid}/{c}", bs["tasks"][tid][c], nt.get(c))
    extra = sorted(t for t in ns["tasks"] if t not in bs["tasks"] and task_repo(t) in judged)
    if extra:
        drift.append(f"corpus moved: {len(extra)} tasks not in the base ({extra[0]}, ..)")

# --- boundary health (`glia health`), per repo and mechanism
bb, nb = load(base_d, "boundary.jsonl", False), load(new_d, "boundary.jsonl", False)
if bb is not None and nb is not None and not drift:
    nby = {r["repo"]: r for r in nb}
    for b in bb:
        lab = b["repo"]
        if only and lab not in only:
            continue
        n = nby.get(lab)
        if n is None:
            drift.append(f"missing boundary row {lab}")
            continue
        if n.get("error"):
            if not b.get("error"):
                drops.append(f"{lab}/health/error {str(n['error'])[:60]}")
            continue
        if b.get("error"):
            gains.append(f"{lab}/health/error cleared")
            continue
        rank(f"{lab}/health/verdict", b["verdict"], n["verdict"], VERDICT)
        rank(f"{lab}/health/parse", b["parse"]["verdict"], n["parse"]["verdict"], VERDICT)
        if b["role"] == "app" and n["role"] == "library":
            drops.append(f"{lab}/health/role app->library")
        for m, bmh in sorted(b["mechanisms"].items()):
            nmh = n["mechanisms"].get(m)
            if nmh is None:
                drops.append(f"{lab}/{m} row lost")
                continue
            rank(f"{lab}/{m}/verdict", bmh["verdict"], nmh["verdict"], VERDICT)
            for k in ("client_reach", "server_reach"):
                reach(f"{lab}/{m}/{k}", bmh.get(k), nmh.get(k))

# --- the Sibyl cover: cell and chain levels, per case checks
bsy, nsy = load(base_d, "sibyl/results.json", False), load(new_d, "sibyl/results.json", False)
cases = 0
if bsy and nsy and not drift:
    for k in ("seed", "transports", "checks"):
        if bsy["meta"].get(k) != nsy["meta"].get(k):
            drift.append(f"sibyl corpus moved: {k}")
    bc = {r["id"]: r for r in bsy["cases"]}
    nc = {r["id"]: r for r in nsy["cases"]}
    if sorted(bc) != sorted(nc):
        drift.append("sibyl corpus moved: the case ids differ")
    for sect in ("cells", "chains"):
        for k, v in sorted(bsy["summary"][sect].items()):
            if not v.get("cases"):
                continue
            cells += 1
            rank(f"sibyl/{sect[:-1]}/{k}", v["level"], nsy["summary"][sect].get(k, {}).get("level", "none"),
                 LEVEL)
    for cid in sorted(bc):
        if cid not in nc:
            continue
        cases += 1
        for c, v in sorted(bc[cid]["checks"].items()):
            nv = nc[cid]["checks"].get(c)
            if v and not nv:
                drops.append(f"sibyl/case/{cid}/{c} pass->fail")
            elif nv and not v:
                gains.append(f"sibyl/case/{cid}/{c} fail->pass")

for x in declines:
    print(f"[cohort-gate] decline {x} (within {tol})")
for x in gains:
    print(f"[cohort-gate] gain {x}")
for x in drops:
    print(f"[cohort-gate] drop {x}")
partial = f" (partial: only {','.join(only)})" if only else ""
json.dump({"ok": not (drops or drift), "only": only, "repos": repos, "tasks": tasks, "cells": cells,
           "cases": cases, "gains": gains, "drops": drops, "declines": declines, "drift": drift,
           "tolerance": tol}, open(out, "w"), indent=1, sort_keys=True)
if drift:
    print(f"[cohort-gate] {verb}: FAILED ({'; '.join(drift[:6])}{'; ..' if len(drift) > 6 else ''})")
    sys.exit(2)
if drops:
    shown = ", ".join(drops[:12]) + (", .." if len(drops) > 12 else "")
    print(f"[cohort-gate] {verb}: FAILED ({len(drops)} drops: {shown}){partial}")
    sys.exit(1)
print(f"[cohort-gate] {verb}: ok - {repos} repos, {tasks} tasks, {cells} cells, {len(gains)} gains, "
      f"0 drops{partial}")
PY
}

# --- the run --------------------------------------------------------------------------------------------
build_glia() {
  if [ -n "$GLIA" ]; then
    BIN=$(cd "$(dirname "$GLIA")" 2>/dev/null && pwd)/$(basename "$GLIA")
    [ -x "$BIN" ] || { say "$CMD: FAILED (no glia at $GLIA)"; exit 2; }
    return
  fi
  say "build glia (release) in $TARGET"
  (cd "$HERE" && cargo build --release --quiet -p glia-cli > "$DIR/build.log" 2>&1) \
    || { tail -20 "$DIR/build.log"; say "$CMD: FAILED (build glia)"; exit 2; }
  BIN=$TARGET/release/glia
}

# boundary health: `glia health --json` per cohort repo at its pinned sha, reduced to the compared fields.
run_boundary() {
  python3 -I - "$CB/repos.json" "$BIN" "$JOBS" "$ONLY" "$DIR/work" "$NEW/boundary.jsonl" <<'PY'
import json, os, shutil, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
repos_json, binary, jobs, only, work, out = sys.argv[1:7]
repos = json.load(open(repos_json))["repos"]
only = [x for x in only.split(",") if x]
clones = os.environ.get("GLIA_COHORT_DIR") or os.path.expanduser("~/.cache/glia-cohort")
env = {**os.environ, "GLIA_NO_PERSIST": "1", "GLIA_THREADS": "4", "LC_ALL": "C"}


def run(cmd, cwd=None, timeout=3600):
    p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout, p.stderr


def reduce(h):
    return {"role": h["role"], "verdict": h["verdict"], "reason": h["reason"],
            "parse": {k: h["parse"][k] for k in ("verdict", "reason", "unparsed_files", "parsed_files")},
            "mechanisms": {m["mechanism"]: {k: m[k] for k in (
                "verdict", "reason", "producers", "consumers", "pairs", "client_reach", "server_reach")}
                for m in h["mechanisms"]},
            "workspace_unresolved": {w["package"]: len(w["importers"]) for w in h["workspace_unresolved"]},
            "suspicious": h["suspicious"]["count"],
            "dead_share": h["dead"]["share"]}


def one(r):
    lab = r["label"]
    copy = os.path.join(work, lab)   # the directory name is the repo's label (no remote, as cohort.py's)
    row = {"repo": lab}
    try:
        shutil.rmtree(copy, ignore_errors=True)
        os.makedirs(work, exist_ok=True)
        for cmd, cwd in ((["git", "clone", "-q", "--shared", "--no-checkout",
                           os.path.join(clones, f"{lab}.git"), copy], None),
                         (["git", "checkout", "-q", "--detach", r["sha"]], copy),
                         (["git", "remote", "remove", "origin"], copy)):
            rc, _, err = run(cmd, cwd=cwd, timeout=1800)
            if rc:
                raise RuntimeError(f"{' '.join(cmd[:3])}: {err.strip()[-160:]}")
        rc, so, se = run([binary, "health", copy, "--json"])
        line = next((l for l in se.splitlines() if l.startswith("[health] repo=")), None)
        if rc or line is None:
            raise RuntimeError(f"glia health rc={rc}: {se.strip()[-160:]}")
        row.update(reduce(json.loads(so)))
        print(f"[cohort-gate] health {lab}: {line}", flush=True)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as e:
        row["error"] = str(e)[:200]
        print(f"[cohort-gate] health {lab}: error {row['error']}", flush=True)
    finally:
        shutil.rmtree(copy, ignore_errors=True)
    return row


todo = [r for r in repos if not only or r["label"] in only]
with ThreadPoolExecutor(max_workers=int(jobs)) as pool:
    rows = list(pool.map(one, todo))
with open(out, "w") as f:
    for r in rows:
        f.write(json.dumps(r, sort_keys=True) + "\n")
sys.exit(1 if any("error" in r for r in rows) else 0)
PY
}

do_run() {
  local t0 only_args=() rc
  t0=$(date +%s)
  mkdir -p "$DIR" || { say "$CMD: FAILED (mkdir $DIR)"; exit 2; }
  build_glia
  [ -n "$ONLY" ] && only_args=(--only "$ONLY")
  # The corpus first: every task row re-derives offline from the bare clones (FH.1a's handoff).
  python3 "$CB/cohort.py" build-tasks --check "${only_args[@]}" > "$DIR/build-tasks.log" 2>&1 \
    || { tail -5 "$DIR/build-tasks.log"; say "$CMD: FAILED (corpus: cohort.py build-tasks --check)"; exit 2; }
  rm -rf "$RUN" && mkdir -p "$NEW" || { say "$CMD: FAILED (mkdir $NEW)"; exit 2; }
  GLIA_COHORT_GATE_HEAD=$(git -C "$HERE" rev-parse HEAD)
  say "run glia=$("$BIN" --version) head=${GLIA_COHORT_GATE_HEAD:0:12} jobs=$JOBS${ONLY:+ only=$ONLY}"
  python3 "$CB/cohort.py" run --release baseline --results "$RUN/results" --glia "$BIN" --jobs "$JOBS" \
      "${only_args[@]}" > "$RUN/cohort.log" 2>&1 \
    || { tail -5 "$RUN/cohort.log"; say "$CMD: FAILED (cohort.py run)"; exit 2; }
  grep -E '^\[cohort\] run (release|errors)=' "$RUN/cohort.log"
  if [ -z "$ONLY" ]; then
    python3 "$CB/cohort.py" check --release baseline --results "$RUN/results" > "$RUN/cohort-check.log" 2>&1 \
      || { tail -5 "$RUN/cohort-check.log"; say "$CMD: FAILED (cohort.py check: the stored rows do not re-derive)"; exit 2; }
  fi
  run_boundary > "$RUN/boundary.log" 2>&1
  rc=$?
  grep -E '^\[cohort-gate\] health ' "$RUN/boundary.log"
  [ "$rc" = 0 ] || { say "$CMD: FAILED (glia health errored, see $RUN/boundary.log)"; exit 2; }
  python3 "$HERE/bench/sibyl/run.py" --glia "$BIN" --work "$DIR/sibyl" --out "$NEW/sibyl" --jobs "$JOBS" \
      > "$RUN/sibyl.log" 2>&1 \
    || { tail -5 "$RUN/sibyl.log"; say "$CMD: FAILED (sibyl run.py: a glia call did not answer)"; exit 2; }
  grep -E '^\[sibyl\] cases=' "$RUN/sibyl.log"
  # glia_head is this checkout's HEAD at the run; glia_source says whether that tree built the glia
  # (`built`) or --glia handed one in (`given`: glia names the binary's build stamp, not its head).
  python3 -I - "$NEW/gate.json" "$("$BIN" --version)" "$GLIA_COHORT_GATE_HEAD" "$ONLY" "$TOL" \
      "$([ -n "$GLIA" ] && echo given || echo built)" <<'PY'
import json, sys
out, glia, head, only, tol, source = sys.argv[1:7]
json.dump({"glia": glia, "glia_head": head, "glia_source": source, "only": [x for x in only.split(",") if x],
           "tolerance": {"reach": float(tol)}}, open(out, "w"), indent=1, sort_keys=True)
open(out, "a").write("\n")
PY
  say "run done s=$(( $(date +%s) - t0 ))"
}

do_check() {
  [ -f "$BASE/summary.json" ] || { say "check: FAILED (no baseline at bench/cohort/results/baseline/)"; exit 2; }
  rm -f "$DIR/last-check.json"
  do_run
  do_compare "$BASE" "$NEW" "$RUN/compare.json" "$ONLY" check
  local rc=$?
  python3 -I - "$RUN/compare.json" "$DIR/last-check.json" "$GLIA_COHORT_GATE_HEAD" <<'PY'
import json, sys
c = json.load(open(sys.argv[1]))
c["glia_head"] = sys.argv[3]
json.dump(c, open(sys.argv[2], "w"), indent=1, sort_keys=True)
PY
  exit $rc
}

# The baseline files: cohort.py's results dir plus boundary.jsonl, sibyl/ and gate.json.
FILES=(meta.json health.jsonl tasks.jsonl misses.jsonl summary.json report.md boundary.jsonl
       sibyl/results.json sibyl/results.md gate.json)
copy_baseline() {
  local f
  rm -rf "$BASE.part" && mkdir -p "$BASE.part/sibyl" || exit 2
  for f in "${FILES[@]}"; do cp "$NEW/$f" "$BASE.part/$f" || { rm -rf "$BASE.part"; exit 2; }; done
  rm -rf "$BASE" && mv "$BASE.part" "$BASE" || exit 2
  # the committed copy re-derives where it lies
  python3 "$CB/cohort.py" check --release baseline > "$DIR/baseline-check.log" 2>&1 \
    || { tail -5 "$DIR/baseline-check.log"; say "$CMD: FAILED (cohort.py check --release baseline)"; exit 2; }
}

do_baseline() {
  [ -z "$ONLY" ] || { say "baseline: FAILED (a baseline covers the whole cohort: no --only)"; exit 2; }
  do_run
  copy_baseline
  say "baseline: ok - $(python3 -I -c 'import json,sys; s=json.load(open(sys.argv[1])); print(len(s["repos"]), "repos,", len(s["tasks"]), "tasks")' "$BASE/summary.json") -> bench/cohort/results/baseline/ (head ${GLIA_COHORT_GATE_HEAD:0:12})"
}

do_ratchet() {
  python3 -I - "$DIR/last-check.json" "$(git -C "$HERE" rev-parse HEAD)" "$BASE" "$NEW" "${FILES[@]}" <<'PY'
import json, os, sys
last, head, base, new, files = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]
try:
    c = json.load(open(last))
except (OSError, ValueError):
    print("[cohort-gate] ratchet: FAILED (no last check recorded)"); sys.exit(2)
if not c.get("ok"):
    print("[cohort-gate] ratchet: FAILED (the last check was not ok)"); sys.exit(2)
if c.get("only"):
    print("[cohort-gate] ratchet: FAILED (the last check was partial)"); sys.exit(2)
if c.get("glia_head") != head:
    print(f"[cohort-gate] ratchet: FAILED (the last check ran at {c.get('glia_head')}, HEAD is {head})"); sys.exit(2)
if c.get("declines"):
    # a decline inside the tolerance passes the check, but ratcheting it would let reach creep down
    print(f"[cohort-gate] ratchet: held - {len(c['declines'])} declines within tolerance "
          f"({', '.join(c['declines'][:4])}); baseline kept"); sys.exit(0)


def untimed(v):
    """v without its wall times: the runner's rows carry `ms`, `build_ms`, `glia_ms`, `grep_ms`, which
    move between two runs of one glia, so a ratchet comparing them would move the baseline every time."""
    if isinstance(v, dict):
        return {k: untimed(x) for k, x in v.items() if k != "ms" and not k.endswith("_ms")}
    if isinstance(v, list):
        return [untimed(x) for x in v]
    return v


def comparable(d, f):
    p = os.path.join(d, f)
    try:
        text = open(p).read()
    except OSError:
        return None
    if f.endswith(".md"):
        return [l for l in text.splitlines() if "glia " not in l]
    if f.endswith(".jsonl"):
        return [untimed(json.loads(x)) for x in text.splitlines() if x.strip()]
    v = untimed(json.loads(text))
    for k in ("glia", "glia_head", "glia_source"):
        v.pop(k, None)
    if "meta" in v:
        v["meta"].pop("glia", None)
    return v


moved = [f for f in files if comparable(base, f) != comparable(new, f)]
print(f"[cohort-gate] ratchet: {'moved' if moved else 'kept'} - {len(moved)} of {len(files)} files differ"
      f"{': ' + ', '.join(moved) if moved else ''}")
sys.exit(10 if moved else 0)
PY
  local rc=$?
  if [ "$rc" = 10 ]; then copy_baseline; say "ratchet: ok - bench/cohort/results/baseline/ moved to head $(git -C "$HERE" rev-parse --short HEAD)"; exit 0; fi
  exit $rc
}

# --- selftest: the compare rules on seeded results ------------------------------------------------------
do_selftest() {
  local st=$DIR/selftest fails=0 n=0
  rm -rf "$st" && mkdir -p "$st/base/sibyl" || exit 2
  python3 -I - "$st/base" <<'PY'
import json, os, sys
d = sys.argv[1]
w = lambda rel, v: json.dump(v, open(os.path.join(d, rel), "w"), indent=1, sort_keys=True)
w("meta.json", {"glia": "glia x", "release": "baseline", "tasks_sha256": "abc"})
health = lambda cr, rr, blind: {"client_reach": cr, "route_reach": rr, "blind": blind, "error": None}
w("summary.json", {"glia": "glia x", "release": "baseline", "repos": {
    "immich": {"health": health(0.4, 0.7, [])}, "gin-vue-admin": {"health": health(0.9, 0.8, [])}},
    "tasks": {"immich-aaaaaaaaaaaa": {"locate": "pass", "rest": "miss", "gate": "pass", "clear": "pass",
                                      "names": "n/a"},
              "gin-vue-admin-bbbbbbbbbbbb": {"locate": "pass", "rest": "pass", "gate": "n/a",
                                             "clear": "n/a", "names": "n/a"}}})
mech = lambda v, cr, sr: {"verdict": v, "reason": "paired", "producers": 50, "consumers": 40, "pairs": 30,
                          "client_reach": cr, "server_reach": sr}
row = lambda lab, h: {"repo": lab, "role": "app", "verdict": "OK", "reason": "ok",
                      "parse": {"verdict": "OK", "reason": "parsed", "unparsed_files": 0, "parsed_files": 9},
                      "mechanisms": {"http": h, "grpc": mech("OK", None, None)},
                      "workspace_unresolved": {}, "suspicious": 0, "dead_share": 0.1}
with open(os.path.join(d, "boundary.jsonl"), "w") as f:
    for lab, h in (("immich", mech("OK", 0.712, 0.65)), ("gin-vue-admin", mech("OK", 0.9, 0.8))):
        f.write(json.dumps(row(lab, h), sort_keys=True) + "\n")
lvl = lambda l, n: {"cases": n, "passed": n, "level": l}
w("sibyl/results.json", {"meta": {"glia": "glia x", "seed": 1, "transports": ["rest"], "checks": ["pair"]},
    "summary": {"cells": {"typescript/http_client": lvl("full", 4), "dart/ws": lvl("n/a", 0)},
                "chains": {"const_prefix": lvl("partial", 2)}},
    "cases": [{"id": "rest-001", "checks": {"pair": True, "drift_s": False}}]})
PY
  # expect <name> <rc> <needle> <python mutation of the new dir, d = its path>
  expect() {
    local name=$1 want=$2 needle=$3 mut=$4 out rc
    n=$((n + 1))
    rm -rf "$st/new" && cp -r "$st/base" "$st/new"
    python3 -I - "$st/new" "$mut" <<'PY' || { echo "selftest $name: mutation failed"; exit 2; }
import json, os, sys
d, mut = sys.argv[1], sys.argv[2]
def edit(rel, fn):
    p = os.path.join(d, rel)
    if rel.endswith(".jsonl"):
        rows = [json.loads(x) for x in open(p)]
        fn(rows)
        open(p, "w").write("".join(json.dumps(r) + "\n" for r in rows))
    else:
        v = json.load(open(p)); fn(v); json.dump(v, open(p, "w"))
exec(mut)
PY
    out=$(do_compare "$st/base" "$st/new" "$st/out.json" "${ONLY_T:-}" check)
    rc=$?
    if [ "$rc" = "$want" ] && grep -qF -- "$needle" <<< "$out"; then
      echo "selftest $name: ok"
    else
      fails=$((fails + 1)); echo "selftest $name: FAILED (rc=$rc want $want, needle '$needle')"; echo "$out" | tail -3
    fi
  }
  expect identical 0 "check: ok - 2 repos, 2 tasks, 2 cells, 0 gains, 0 drops" "pass"
  expect http-reach 1 "immich/http/client_reach 0.712->0.1" \
    "edit('boundary.jsonl', lambda rs: rs[0]['mechanisms']['http'].update(client_reach=0.1))"
  expect http-verdict 1 "immich/http/verdict OK->BLIND" \
    "edit('boundary.jsonl', lambda rs: rs[0]['mechanisms']['http'].update(verdict='BLIND'))"
  expect reach-lost 1 "immich/http/server_reach 0.65->none" \
    "edit('boundary.jsonl', lambda rs: rs[0]['mechanisms']['http'].update(server_reach=None))"
  expect decline 0 "decline immich/cohort/client_reach 0.4->0.36" \
    "edit('summary.json', lambda s: s['repos']['immich']['health'].update(client_reach=0.36))"
  expect cohort-reach 1 "immich/cohort/client_reach 0.4->0.3" \
    "edit('summary.json', lambda s: s['repos']['immich']['health'].update(client_reach=0.3))"
  expect blind-flag 1 "gin-vue-admin/cohort/blind +clients_unpaired" \
    "edit('summary.json', lambda s: s['repos']['gin-vue-admin']['health'].update(blind=['clients_unpaired']))"
  expect task 1 "immich-aaaaaaaaaaaa/gate pass->miss" \
    "edit('summary.json', lambda s: s['tasks']['immich-aaaaaaaaaaaa'].update(gate='miss'))"
  expect task-gain 0 "1 gains, 0 drops" \
    "edit('summary.json', lambda s: s['tasks']['immich-aaaaaaaaaaaa'].update(rest='pass'))"
  expect cell 1 "sibyl/cell/typescript/http_client full->partial" \
    "edit('sibyl/results.json', lambda v: v['summary']['cells']['typescript/http_client'].update(level='partial'))"
  expect chain-gain 0 "gain sibyl/chain/const_prefix partial->full" \
    "edit('sibyl/results.json', lambda v: v['summary']['chains']['const_prefix'].update(level='full'))"
  expect case 1 "sibyl/case/rest-001/pair pass->fail" \
    "edit('sibyl/results.json', lambda v: v['cases'][0]['checks'].update(pair=False))"
  expect corpus 2 "corpus moved: tasks_sha256 abc -> xyz" \
    "edit('meta.json', lambda m: m.update(tasks_sha256='xyz'))"
  expect missing-task 2 "missing task gin-vue-admin-bbbbbbbbbbbb" \
    "edit('summary.json', lambda s: s['tasks'].pop('gin-vue-admin-bbbbbbbbbbbb'))"
  ONLY_T=immich expect only 0 "check: ok - 1 repos, 1 tasks, 2 cells, 0 gains, 0 drops (partial: only immich)" \
    "edit('summary.json', lambda s: s['tasks']['gin-vue-admin-bbbbbbbbbbbb'].update(locate='miss'))"
  rm -rf "$st"
  if [ "$fails" = 0 ]; then say "selftest: ok - $n cases"; exit 0; fi
  say "selftest: FAILED ($fails of $n cases)"; exit 1
}

lock() { mkdir -p "$DIR" && exec 9>"$DIR/.run.lock" && flock 9; }   # run/ and the work dirs are shared

case "$CMD" in
  check)
    # before the lock: a skip never waits on another run
    if [ -n "${GLIA_COHORT_GATE_SKIP:-}" ]; then say "check: skipped - $GLIA_COHORT_GATE_SKIP"; exit 3; fi
    lock; do_check ;;
  baseline) lock; do_baseline ;;
  ratchet) lock; do_ratchet ;;
  run) lock; do_run ;;
  compare)
    [ "${#POS[@]}" = 2 ] || { say "compare: FAILED (usage: compare <base> <new> [--only l,..])"; exit 2; }
    mkdir -p "$DIR" && do_compare "${POS[0]}" "${POS[1]}" "$DIR/compare.json" "$ONLY" compare ;;
  selftest) do_selftest ;;
  *) echo "usage: $0 check|baseline|ratchet|run [--glia <bin>] [--only <l,..>] [--jobs <n>] | compare <base> <new> | selftest" >&2
     exit 2 ;;
esac
