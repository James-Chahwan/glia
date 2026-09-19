#!/usr/bin/env bash
# Message-contracts (A12) smoke test. Offline. Exits non-zero on any drift.
#
# Asserts the REPORT, not just the cell: `glia contracts` over a two-service
# Go/NATS stack must return exactly one match row and one mismatch row, and the
# pyo3 surface (`PyGraph.contracts()`) must return the same rows, so the two
# transports cannot drift apart.
set -euo pipefail
cd "$(dirname "$0")/../.."

tmp="$(mktemp)"; err="$(mktemp)"; py="$(mktemp)"; pyerr="$(mktemp)"
trap 'rm -f "$tmp" "$err" "$py" "$pyerr"' EXIT
if ! GLIA_NO_PERSIST=1 cargo run -q -p glia-cli -- contracts bench/message-contracts/svc \
    --with bench/message-contracts/worker --json 2>"$err" > "$tmp"; then
    echo "FAIL: \`glia contracts\` did not run (build error or non-zero exit):"
    sed -n '1,40p' "$err"; exit 1
fi

# The engine's own marker (engine/src/answers.rs::message_contracts). stderr is
# captured so the verdict COUNTS are asserted from the engine, not only from
# the JSON this script re-derives them from.
marker="$(grep -F '[contracts] topics=' "$err" || true)"
if [ -z "$marker" ]; then
    echo "FAIL: no [contracts] topics= marker on stderr"; sed -n '1,40p' "$err"; exit 1
fi
echo "$marker"
case "$marker" in
    "[contracts] topics=2 match=1 mismatch=1 unknown=0 literal=2 tag=0 rows=2") ;;
    *) echo "FAIL: marker want topics=2 match=1 mismatch=1 unknown=0 literal=2 tag=0 rows=2"; exit 1 ;;
esac

python3 - "$tmp" <<'PY'
import json, sys, collections
rows = json.load(open(sys.argv[1]))
st = collections.Counter(r["status"] for r in rows)
seen = sorted(
    (r["topic"], r["status"], r["confidence"],
     (r["producer"] or {}).get("message_type"),
     (r["consumer"] or {}).get("message_type"))
    for r in rows)
want = sorted([
    ("orders",    "match",    "strong", "OrderCreated",    "OrderCreated"),
    ("shipments", "mismatch", "strong", "ShipmentCreated", "ShipmentDispatched"),
])
where = sorted(
    (r["topic"], (r["producer"] or {}).get("file"), (r["consumer"] or {}).get("file"))
    for r in rows)
want_where = sorted([
    ("orders",    "publisher.go", "consumer.go"),
    ("shipments", "shipping.go",  "shipping_worker.go"),
])
fail = []
if st["match"] != 1:    fail.append(f"match={st['match']} want 1")
if st["mismatch"] != 1: fail.append(f"mismatch={st['mismatch']} want 1")
if st["unknown"] != 0:  fail.append(f"unknown={st['unknown']} want 0 (an unpaired or tag row appeared)")
if any(r["topic_is_tag"] for r in rows): fail.append("a literal topic was flagged as a framework tag")
if any(r["pattern"] for r in rows):      fail.append("an exact-topic pair was flagged as a pattern match")
for r in rows:
    p, c = r["producer"] or {}, r["consumer"] or {}
    if p.get("repo_id") == c.get("repo_id"):
        fail.append(f"{r['topic']}: producer and consumer share a repo_id — the pair must cross svc -> worker")
if seen != want:        fail.append(f"rows mismatch:\n  got  {seen}\n  want {want}")
if where != want_where: fail.append(f"locations mismatch:\n  got  {where}\n  want {want_where}")
if fail:
    print("FAIL:\n  " + "\n  ".join(fail)); sys.exit(1)
print("PASS: 1 match (orders/OrderCreated), 1 mismatch (shipments/ShipmentCreated vs ShipmentDispatched)")
PY

# pyo3 surface. Imports the INSTALLED `glia_py` wheel, never the working tree —
# after a Rust change, rebuild it first (`cargo clean -p glia-engine -p
# glia-py` before `maturin build`, or maturin can repackage a stale .so).
# A missing wheel or a wheel without contracts() FAILS: skipping would be a
# dead gate. $PYTHON picks the interpreter that has the wheel (the 0.5.0 leap
# grades in ~/.venvs/glia-leap: PYTHON=~/.venvs/glia-leap/bin/python).
GLIA_NO_PERSIST=1 "${PYTHON:-python3}" - "$tmp" > "$py" 2>"$pyerr" <<'PY' || { cat "$py"; sed -n '1,20p' "$pyerr"; exit 1; }
import json, sys
try:
    import glia_py as rg
except ImportError as e:
    print(f"FAIL: glia_py is not importable ({e}); build and install the wheel"); sys.exit(1)
g = rg.generate_many(["bench/message-contracts/svc", "bench/message-contracts/worker"])
if not hasattr(g, "contracts"):
    print("FAIL: the installed wheel predates PyGraph.contracts() (A12.3); rebuild it"); sys.exit(1)

def proj(rows):
    return sorted(
        (r["topic"], r["status"], r["confidence"], r["topic_is_tag"],
         (r["producer"] or {}).get("message_type"),
         (r["consumer"] or {}).get("message_type"))
        for r in rows)

cli = proj(json.load(open(sys.argv[1])))
rows = g.contracts()
if not isinstance(rows, list):
    print(f"FAIL: PyGraph.contracts() returned {type(rows).__name__}, want a list of dicts (LD.2); rebuild the wheel"); sys.exit(1)
got = proj(rows)
if got != cli:
    print(f"FAIL: pyo3 contracts() disagrees with the CLI:\n  pyo3 {got}\n  cli  {cli}"); sys.exit(1)
print("PASS: pyo3 contracts() agrees with the CLI")
PY
if ! grep -qF '[contracts] surface=pyo3 repos=2' "$pyerr"; then
    echo "FAIL: no '[contracts] surface=pyo3 repos=2' marker from the wheel"; sed -n '1,20p' "$pyerr"; exit 1
fi
cat "$py"
