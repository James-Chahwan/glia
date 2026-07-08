#!/usr/bin/env bash
# Tier-4 doc-ingestion smoke test. Offline. Exits non-zero on any regression.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo run -q -p repo-graph-doc-sources --bin docsync -- \
    bench/doc-link/fixture-repo bench/doc-link/pages/pages.jsonl

tmp="$(mktemp)"; trap 'rm -f "$tmp"' EXIT
cargo run -q -p glia-cli -- analyze bench/doc-link/fixture-repo --format json 2>/dev/null > "$tmp"

python3 - "$tmp" <<'PY'
import json, sys, collections
d = json.load(open(sys.argv[1]))
kc = collections.Counter(n["kind_name"] for n in d["nodes"])
ec = collections.Counter(e["category"] for e in d["edges"])
byid = {n["id"]: n for n in d["nodes"]}
docs = sorted(
    (byid[e["from"]]["name"], byid[e["to"]]["kind_name"], byid[e["to"]]["name"])
    for e in d["edges"] if e["category"] == "DOCUMENTS"
)
want_docs = sorted([
    ("overview", "CLASS", "OrderService"),
    ("overview", "CLASS", "PaymentGateway"),
    ("charging", "METHOD", "charge"),
    ("charging", "METHOD", "place_order"),
    ("user-directory", "FUNCTION", "get_user"),
])
fail = []
if kc["DOC_SPACE"] != 1:    fail.append(f"DOC_SPACE={kc['DOC_SPACE']} want 1")
if kc["DOC_SECTION"] != 5:  fail.append(f"DOC_SECTION={kc['DOC_SECTION']} want 5")
if ec["CONTAINS"] != 5:     fail.append(f"CONTAINS={ec['CONTAINS']} want 5")
if ec["DOCUMENTS"] != 5:    fail.append(f"DOCUMENTS={ec['DOCUMENTS']} want 5")
if docs != want_docs:       fail.append(f"DOCUMENTS edges mismatch:\n  got  {docs}\n  want {want_docs}")
if fail:
    print("FAIL:\n  " + "\n  ".join(fail)); sys.exit(1)
print("PASS: 1 DOC_SPACE, 5 DOC_SECTION, 5 CONTAINS, 5 DOCUMENTS (all correct)")
PY
