#!/usr/bin/env python3
"""
Substrate-gap matrix runner — glia P1 (handoff v6).

Grades every fixture under fixtures/ and prints the blind-spot matrix:
rows = frameworks, cols = edge categories, cell = extraction recall.
A `0.00` cell is a confirmed blind spot (a task grep wins today).

Also appends a timestamped JSONL record to results.jsonl (append-only, per
house style) so the map is diffable across sessions and after each P1 fix.

Usage:
  python3 run.py                 # grade all fixtures, print matrix
  python3 run.py --no-log        # don't append to results.jsonl
"""
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from grade import grade_fixture  # noqa: E402

FIXTURES = HERE / "fixtures"
RESULTS = HERE / "results.jsonl"


def discover():
    return sorted(p for p in FIXTURES.iterdir() if (p / "key.json").exists())


def main():
    results = []
    for fx in discover():
        try:
            results.append(grade_fixture(fx))
        except Exception as e:  # noqa: BLE001 — a broken fixture shouldn't sink the run
            print(f"!! {fx.name}: {type(e).__name__}: {e}", file=sys.stderr)

    # union of categories seen, in registry order
    cats = []
    for res in results:
        for c in res["per_category"]:
            if c not in cats:
                cats.append(c)
    cats.sort()

    # ---- matrix ----
    fw_w = max([len(r["framework"]) for r in results] + [9])
    header = "framework".ljust(fw_w) + "  " + "  ".join(c[:10].rjust(10) for c in cats)
    print("\n" + header)
    print("-" * len(header))
    blind = []
    for res in results:
        row = res["framework"].ljust(fw_w) + "  "
        cells = []
        for c in cats:
            v = res["per_category"].get(c)
            if v is None:
                cells.append("·".rjust(10))
            else:
                cells.append(f"{v['recall']:.2f}".rjust(10))
                if v["recall"] == 0.0:
                    blind.append((res["framework"], c))
        print(row + "  ".join(cells))
    print("-" * len(header))

    # ---- node-extraction misses (attribution: node gap vs edge gap) ----
    node_misses = []
    for res in results:
        for r in res["node_recall"]:
            if not r["found"]:
                node_misses.append((res["framework"], r["kind"], r["name"]))

    print(f"\nBLIND SPOTS (recall 0.00): {len(blind)}")
    for fw, c in blind:
        print(f"  - {fw}: {c}")
    if node_misses:
        print(f"\nMISSING NODES ({len(node_misses)}):")
        for fw, k, n in node_misses:
            print(f"  - {fw}: {k} {n!r}")

    if "--no-log" not in sys.argv:
        rec = {
            "ts": datetime.now(timezone.utc).isoformat(),
            "engine": _engine_version(),
            "blind_spots": [{"framework": fw, "category": c} for fw, c in blind],
            "missing_nodes": [{"framework": fw, "kind": k, "name": n} for fw, k, n in node_misses],
            "results": results,
        }
        with RESULTS.open("a") as f:
            f.write(json.dumps(rec) + "\n")
        print(f"\nappended -> {RESULTS.relative_to(HERE.parent.parent)}")


def _engine_version():
    try:
        import repo_graph_py as rg
        return rg.version()
    except Exception:  # noqa: BLE001
        return "unknown"


if __name__ == "__main__":
    main()
