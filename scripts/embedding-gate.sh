#!/usr/bin/env bash
# embedding-gate.sh - the 0.5.4 embedding gate (FB.6, seed item 13): engine/examples/rank_eval.rs's
# semantic find-shape rankers on the rank gate's pinned public snapshots, judged by a rule fixed
# before the first bge-small run, written to dev-notes/embedding-gate-0.5.4.md.
#
#   bash scripts/embedding-gate.sh            fetch (rank-gate.sh fetch), build rank_eval --features sem-eval,
#                                             run every snapshot with --sem on, pair, judge, write the report
#   bash scripts/embedding-gate.sh judge      re-judge the last runs (no build, no run)
#
# Env: GLIA_RANK_GATE_DIR (default ~/.cache/glia-rank-gate: the shared clones and copies; this script writes
# only sem/ (the vector cache) and sem-run/ there), GLIA_SEM_TARGET (where rank_eval is built with the model;
# default <this checkout>/target/sem-eval, apart from the rank gate's binary), GLIA_SEM_MODEL (bge-small;
# test-hash only for a smoke run: its report is never the gate's), GLIA_SEM_ONLY (comma-separated labels: a
# smoke run on those snapshots only, never the gate's report), GLIA_SEM_REPORT (default
# dev-notes/embedding-gate-0.5.4.md for the full bge-small panel, else sem-run-<model>/report.md),
# FASTEMBED_CACHE_DIR (default ~/.cache/glia/fastembed; never a repo dir).
# GLIA_RANK_PRIVATE is unset and --private never passed: public snapshots and public rows only.
#
# THE PASS RULE (fixed before any bge-small run; FB-5, James 2026-10-24, "Strict: R1 5/6, R2 3/6"):
#   Panel: the six public snapshots of bench/rank-gate/snapshots.json. Metric: recall@10 in rank_eval's find
#   shape (the query is the test commit's subject; no seed file). Every comparison a - b is rank_eval
#   `--gate-pair`'s paired commit-cluster bootstrap 95% CI (B = 1000).
#   Recipe r* = t2 iff SEM:t2 - SEM:t1 has CI lo > 0 on >= 3 of 6 snapshots and CI hi >= 0 on all 6; else t1.
#   Query form: prefixed (bge) iff SEMp:t1 - SEM:t1 has CI lo > 0 on >= 3 of 6 and CI hi >= 0 on all 6; else
#   raw (Engram's form). The selected ranker S* is SEM:r* (raw) or SEMp:r* (bge).
#   R1 signal: S* - SEMrand:t1 has CI lo > 0 on >= 5 of 6.
#   R2 beats BM25 alone: S* - T1s has CI lo > 0 on >= 3 of 6 and CI hi >= 0 on all 6 (S* alone: the shipped
#   tier is its own rows, never fused into another order, so no fused ranker carries the verdict).
#   PASS = R1 and R2; anything else is FAIL.
#   Selection guard: R1 / R2 are also stated for SEM:t1 raw (the pre-registered default); a PASS that holds
#   for S* but not for SEM:t1 raw is written with a `selected-only` note line above `VERDICT: PASS`.
#   Information rows, never the verdict: FUSE[T1s,S*] - T1s, S* - ANS:find (the lexical node find),
#   S* - LX[IDR,T1s] (the shipped find --files order, EB.8), S* - SELf2t (its calibrated pick, EC.13),
#   S* - ANS:find_files and S* - ANS:find_files@sel (the engine's own find --files).
#   On FAIL, glia embed ships as a sidecar no answer reads (item 13), and the integrator drops FB.7a / FB.7b /
#   FB.7c / FB.8 / FB.9 from the schedule.
#
# Fired_on: rank_eval's `[sem-eval] label=<l> recipe=<r> nodes=<n> embedded=<e> cached=<c> queries=<q> ms=<t>`
# and `[rank-gate] pair label=<l> shape=find a=<a> b=<b> metric=r10 diff=.. ci=[..]` (in sem-run/<label>.*.log),
# then this script's last line:
#   [embedding-gate] verdict=PASS|FAIL recipe=<r*> prefix=<raw|bge>
set -uo pipefail
unset GLIA_RANK_PRIVATE

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SNAP=$HERE/bench/rank-gate/snapshots.json
DIR=${GLIA_RANK_GATE_DIR:-$HOME/.cache/glia-rank-gate}
SEM=$DIR/sem
TARGET=${GLIA_SEM_TARGET:-$HERE/target/sem-eval}
BIN=$TARGET/release/examples/rank_eval
MODEL=${GLIA_SEM_MODEL:-bge-small}
ONLY=${GLIA_SEM_ONLY:-}
if [ "$MODEL" = bge-small ] && [ -z "$ONLY" ]; then
  SRUN=$DIR/sem-run
  REPORT=${GLIA_SEM_REPORT:-$HERE/dev-notes/embedding-gate-0.5.4.md}
else
  # a smoke run: its own run dir and report, never the gate's
  SRUN=$DIR/sem-run-$MODEL${ONLY:+-smoke}
  REPORT=${GLIA_SEM_REPORT:-$SRUN/report.md}
fi
export FASTEMBED_CACHE_DIR=${FASTEMBED_CACHE_DIR:-$HOME/.cache/glia/fastembed}
say() { echo "[embedding-gate] $*"; }
fail() { say "FAILED ($*)"; exit 2; }

case "$FASTEMBED_CACHE_DIR" in
  "$HERE"|"$HERE"/*) fail "FASTEMBED_CACHE_DIR is inside this checkout" ;;
esac

# The pairs every snapshot is judged on (a,b = a - b), the forms S* can take included.
PAIRS="SEM:t2,SEM:t1;SEMp:t1,SEM:t1"
for s in SEM:t1 SEM:t2 SEMp:t1 SEMp:t2; do
  PAIRS="$PAIRS;$s,SEMrand:t1;$s,T1s;FUSE[T1s,$s],T1s;$s,ANS:find;$s,LX[IDR,T1s];$s,SELf2t;$s,ANS:find_files;$s,ANS:find_files@sel"
done

# The snapshot rows (`name url rev label flags..`), with rank-gate.sh's refusal rule.
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
        print(f"[embedding-gate] refused {name}: not a public https snapshot", file=sys.stderr)
        bad = 1
for r in rows:
    print(" ".join([r["name"], r["url"], r["rev"], r["label"]] + list(r.get("flags", []))))
sys.exit(2 if bad else 0)
PY
}
ROWS=$(rows) || exit 2
if [ -n "$ONLY" ]; then
  ROWS=$(while read -r name url rev label flags; do
    case ",$ONLY," in *",$label,"*) echo "$name $url $rev $label $flags" ;; esac
  done <<< "$ROWS")
  [ -n "$ROWS" ] || fail "GLIA_SEM_ONLY=$ONLY names no snapshot"
fi

do_run() {
  local name url rev label flags t0 common=()
  bash "$HERE/scripts/rank-gate.sh" fetch || fail "fetch"
  mapfile -t common < <(bash "$HERE/scripts/rank-gate.sh" flags)
  [ "${#common[@]}" -gt 0 ] || fail "rank-gate.sh flags printed nothing"
  mkdir -p "$SEM" "$SRUN" || fail "mkdir $SRUN"
  say "build rank_eval (release, --features sem-eval) in $TARGET"
  (cd "$HERE" && CARGO_TARGET_DIR=$TARGET cargo build --release --quiet -p glia-engine --example rank_eval \
      --features sem-eval > "$SRUN/build.log" 2>&1) || { tail -20 "$SRUN/build.log"; fail "build rank_eval"; }
  export GLIA_RANK_GATE_HEAD
  GLIA_RANK_GATE_HEAD=$(git -C "$HERE" rev-parse HEAD 2>/dev/null || echo unknown)
  while read -r name url rev label flags; do
    t0=$(date +%s)
    # cwd = sem-run/: anything rank_eval writes by default lands there, never in the repo.
    # shellcheck disable=SC2086
    (cd "$SRUN" && GLIA_NO_PERSIST=1 "$BIN" --repo "$DIR/$name@${rev:0:12}" --git "$DIR/$name.git" --rev "$rev" \
        --label "$label" $flags "${common[@]}" --sem on --sem-model "$MODEL" --sem-cache "$SEM" \
        --out "$SRUN/$label.json" --md "$SRUN/$label.md" --r1md "$SRUN/$label.r1.md" \
        --r3md "$SRUN/$label.r3.md" 2> "$SRUN/$label.log") \
      || { tail -5 "$SRUN/$label.log"; fail "run $label"; }
    grep '^\[sem-eval\]' "$SRUN/$label.log" | grep -v ' progress ' || true
    say "run label=$label s=$(( $(date +%s) - t0 ))"
  done <<< "$ROWS"
}

do_pairs() {
  local label
  while read -r _ _ _ label _; do
    "$BIN" --gate-pair "$PAIRS" --gate-new "$SRUN/$label.json" --gate-shape find --boot 1000 \
        2> "$SRUN/$label.pairs.log" || { cat "$SRUN/$label.pairs.log"; fail "pairs $label"; }
  done <<< "$ROWS"
}

do_judge() {
  local labels
  labels=$(while read -r _ _ _ label _; do echo "$label"; done <<< "$ROWS")
  python3 -I - "$SRUN" "$SEM" "$REPORT" "$MODEL" "$HERE/scripts/embedding-gate.sh" $labels <<'PY'
import json, os, re, sys

srun, sem_dir, report, model, script = sys.argv[1:6]
labels = sys.argv[6:]
pat = re.compile(r"^\[rank-gate\] pair label=(\S+) shape=find a=(\S+) b=(\S+) metric=r10 diff=(\S+) "
                 r"ci=\[(\S+),(\S+)\] a_r10=(\S+) b_r10=(\S+) queries=(\d+) commits=(\d+)$")
rows = {}   # (label, a, b) -> (diff, lo, hi, a_r10, b_r10, queries, commits)
for l in labels:
    for line in open(os.path.join(srun, f"{l}.pairs.log")):
        m = pat.match(line.strip())
        if m:
            rows[(m[1], m[2], m[3])] = tuple(float(m[i]) for i in range(4, 9)) + (int(m[9]), int(m[10]))
n = len(labels)

def row(l, a, b):
    k = (l, a, b)
    if k not in rows:
        print(f"[embedding-gate] FAILED (no pair {a} - {b} for {l})")
        sys.exit(2)
    return rows[k]

def count(a, b, side):
    return sum(1 for l in labels if (row(l, a, b)[1] > 0 if side == "lo" else row(l, a, b)[2] >= 0))

def wins(a, b, k):   # CI lo > 0 on >= k and CI hi >= 0 on all n
    return count(a, b, "lo") >= k and count(a, b, "hi") == n

recipe = "t2" if wins("SEM:t2", "SEM:t1", 3) else "t1"
prefix = "bge" if wins("SEMp:t1", "SEM:t1", 3) else "raw"
star = ("SEMp:" if prefix == "bge" else "SEM:") + recipe

def r1(s):
    return count(s, "SEMrand:t1", "lo") >= 5

def r2(s):
    return wins(s, "T1s", 3)

ok = r1(star) and r2(star)
guard = r1("SEM:t1") and r2("SEM:t1")
verdict = "PASS" if ok else "FAIL"

# the rule, verbatim from the script's header
rule = []
on = False
for line in open(script):
    if line.startswith("# THE PASS RULE"):
        on = True
    elif on and line.startswith("# Fired_on"):
        break
    if on:
        rule.append(line[2:].rstrip("\n") if line.startswith("# ") else line[1:].rstrip("\n"))

def f4(x):
    return f"{x:+.4f}"

def ci(r):
    return f"[{r[1]:+.4f}, {r[2]:+.4f}]"

out = []
out.append("# The embedding gate (0.5.4, FB.6)")
out.append("")
out.append("Seed item 13: the semantic tier reaches an answer only if it beats BM25 alone on the rank gate's")
out.append("public snapshots; otherwise `glia embed` ships as a sidecar no answer reads. Produced by")
out.append("`bash scripts/embedding-gate.sh` (rank_eval `--sem on`, `--gate-pair`); every number below is an")
out.append(f"aggregate over a public repository. Model: `{model}`.")
out.append("")
out.append("## The pass rule (committed with the harness, before the first bge-small run)")
out.append("")
out.append("```")
out.extend(rule)
out.append("```")
out.append("")
out.append("## Rankers")
out.append("")
out.append("Find shape, one query per test commit (its subject). SEM:<r>: per file the max cosine of the raw")
out.append("subject's vector over its EMBED_KINDS nodes' recipe-r vectors (`semantic::embed_text`); SEMp:<r>")
out.append("the same with the BGE query prefix; SEMrand:t1 the raw subject over a fixed random unit vector per")
out.append("node (h64(qname)-seeded); FUSE[T1s,X] = 0.5 minmax(T1s) + 0.5 minmax(X). T1s is BM25 over the")
out.append("strictly older commit messages (the find bank's BM25-alone).")
out.append("")
out.append("## Panel")
out.append("")
out.append("| repo | queries | commits | t1 nodes | t2 nodes | node embed ms (first run) | query embed ms (first run) | t2 index load ms (this run) | this run embedded (t1 / t2 / queries) |")
out.append("|---|---:|---:|---:|---:|---:|---:|---:|---|")
for l in labels:
    d = json.load(open(os.path.join(srun, f"{l}.json")))
    s = d.get("sem", {})
    rec = s.get("recipes", {})
    q = s.get("queries", {})
    slug = re.sub(r"[^A-Za-z0-9]", "-", model)
    tpath = os.path.join(sem_dir, f"{l}-{slug}.times.json")
    embedded_now = sum(rec.get(r, {}).get("embedded", 0) for r in ("t1", "t2")) + q.get("embedded", 0)
    if embedded_now > 0 or not os.path.exists(tpath):
        with open(tpath, "w") as fh:
            json.dump(s, fh, sort_keys=True)
    first = json.load(open(tpath))
    frec, fq = first.get("recipes", {}), first.get("queries", {})
    node_ms = sum(frec.get(r, {}).get("embed_ms", 0) for r in ("t1", "t2"))
    t2 = rec.get("t2", {})
    load_ms = t2.get("ms", 0) - t2.get("embed_ms", 0)
    any_pair = row(l, "SEM:t1", "T1s")
    out.append(f"| {l} | {any_pair[5]} | {any_pair[6]} | {rec.get('t1', {}).get('nodes', 0)} | "
               f"{t2.get('nodes', 0)} | {node_ms} | {fq.get('ms', 0)} | {load_ms} | "
               f"{rec.get('t1', {}).get('embedded', 0)} / {t2.get('embedded', 0)} / {q.get('embedded', 0)} |")
out.append("")

def table(title, pairs, note):
    out.append(f"## {title}")
    out.append("")
    if note:
        out.append(note)
        out.append("")
    out.append("| repo | a | b | a r@10 | b r@10 | a - b | 95% CI |")
    out.append("|---|---|---|---:|---:|---:|---|")
    for a, b in pairs:
        for l in labels:
            r = row(l, a, b)
            out.append(f"| {l} | {a} | {b} | {r[3]:.4f} | {r[4]:.4f} | {f4(r[0])} | {ci(r)} |")
    out.append("")

table("Selection: recipe and query form", [("SEM:t2", "SEM:t1"), ("SEMp:t1", "SEM:t1")],
      f"SEM:t2 - SEM:t1: lo > 0 on {count('SEM:t2', 'SEM:t1', 'lo')} of {n}, hi >= 0 on "
      f"{count('SEM:t2', 'SEM:t1', 'hi')} of {n} -> r* = {recipe}. SEMp:t1 - SEM:t1: lo > 0 on "
      f"{count('SEMp:t1', 'SEM:t1', 'lo')} of {n}, hi >= 0 on {count('SEMp:t1', 'SEM:t1', 'hi')} of {n} -> "
      f"query form {prefix}. S* = {star}.")
verdict_pairs = [(star, "SEMrand:t1"), (star, "T1s")]
if star != "SEM:t1":
    verdict_pairs += [("SEM:t1", "SEMrand:t1"), ("SEM:t1", "T1s")]
table("Verdict rows", verdict_pairs,
      f"R1 ({star} - SEMrand:t1, lo > 0 on >= 5 of {n}): lo > 0 on {count(star, 'SEMrand:t1', 'lo')} -> "
      f"{'yes' if r1(star) else 'no'}. R2 ({star} - T1s, lo > 0 on >= 3 of {n}, hi >= 0 on all): lo > 0 on "
      f"{count(star, 'T1s', 'lo')}, hi >= 0 on {count(star, 'T1s', 'hi')} -> {'yes' if r2(star) else 'no'}. "
      f"Guard, SEM:t1 raw: R1 {'yes' if r1('SEM:t1') else 'no'} (lo > 0 on {count('SEM:t1', 'SEMrand:t1', 'lo')}), "
      f"R2 {'yes' if r2('SEM:t1') else 'no'} (lo > 0 on {count('SEM:t1', 'T1s', 'lo')}, hi >= 0 on "
      f"{count('SEM:t1', 'T1s', 'hi')}).")
table("Information rows (never the verdict)",
      [(f"FUSE[T1s,{star}]", "T1s"), (star, "ANS:find"), (star, "LX[IDR,T1s]"), (star, "SELf2t"),
       (star, "ANS:find_files"), (star, "ANS:find_files@sel")], "")
others = [s for s in ("SEM:t1", "SEM:t2", "SEMp:t1", "SEMp:t2") if s != star]
table("Every other form (information)",
      [p for s in others for p in ((s, "SEMrand:t1"), (s, "T1s"), (f"FUSE[T1s,{s}]", "T1s"))], "")

out.append("## Verdict")
out.append("")
out.append(f"recipe r* = {recipe}; query form = {prefix}; S* = {star}; R1 = {'yes' if r1(star) else 'no'}; "
           f"R2 = {'yes' if r2(star) else 'no'}.")
out.append("")
if ok:
    out.append("The semantic tier may reach find / flow search / pack's fallback (FB.7a-FB.9), as its own rows")
    out.append("beside the lexical answer, never fused into another order and never deciding presence.")
    if recipe == "t2":
        out.append("The measured default recipe is t2: engine/src/semantic/text.rs DEFAULT_RECIPE = T2.")
    out.append(f"The shipped query form is {'BGE-prefixed' if prefix == 'bge' else 'raw'}.")
    out.append("")
    if not guard:
        out.append("selected-only: the PASS holds for the selected S* but not for SEM:t1 raw (the pre-registered default).")
else:
    out.append("glia embed ships as a sidecar no answer reads (item 13): the integrator drops FB.7a / FB.7b /")
    out.append("FB.7c / FB.8 / FB.9 from the schedule.")
    out.append("")
out.append(f"VERDICT: {verdict}")
with open(report, "w") as fh:
    fh.write("\n".join(out) + "\n")
print(f"[embedding-gate] report {report}")
print(f"[embedding-gate] verdict={verdict} recipe={recipe} prefix={prefix}")
PY
}

case "${1:-}" in
  "") do_run; do_pairs; do_judge ;;
  judge) do_pairs; do_judge ;;
  *) echo "usage: $0 [judge]" >&2; exit 2 ;;
esac
