#!/usr/bin/env python3
"""
Substrate-gap matrix runner — glia P1 (handoff v6).

Grades every fixture under fixtures/ and prints the blind-spot matrix:
rows = frameworks, cols = edge categories, cell = extraction recall.
A `0.00` cell is a confirmed blind spot (a task grep wins today).
A cell strictly between 0 and 1 is a PARTIAL — printed in its own section
below the matrix, because a half-fixed cell is otherwise indistinguishable
from a fully-fixed one at a glance (and was invisible before W0.4).
Precision (`forbid`) and cell-level (`expect_cells`) assertions from the
frozen key.json vocabulary are totalled under the matrix too.

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
    errors = []
    for fx in discover():
        try:
            results.append(grade_fixture(fx))
        except Exception as e:  # noqa: BLE001 — a broken fixture shouldn't sink the run
            # A raising fixture drops OUT of the matrix, so it must also be
            # reported on stdout (below) — otherwise the frozen-vocabulary
            # ValueError is muffled into a silently-missing row.
            errors.append((fx.name, f"{type(e).__name__}: {e}"))
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

    # ---- PARTIAL cells: 0 < recall < 1 (W0.4) ----
    # Kept OUT of the matrix body on purpose: the cell formatting above is
    # byte-compared across runs, so honesty about partials is additive here.
    partials = []
    for res in results:
        for c, v in sorted(res["per_category"].items()):
            if v["recall"] is not None and 0.0 < v["recall"] < 1.0:
                partials.append((res["framework"], c, v["recall"]))

    # ---- precision (forbid) + cell-level assertions (W0.4) ----
    forbid_rows = []
    for res in results:
        for r in res.get("forbid_results", []):
            if r["violated"]:
                what = (f"{r['category']} {r['from']!r} -> {r['to']!r}"
                        if "category" in r else f"{r['kind']} {r.get('name', '*')!r}")
                forbid_rows.append((res["framework"], what, r["matched"], r["max_allowed"]))
    cell_misses = []
    for res in results:
        for r in res.get("cell_recall", []):
            if not r["found"]:
                cell_misses.append((res["framework"], r["cell"], r["kind"], r["node"],
                                    r["registered"]))

    print(f"\nPARTIAL (0 < recall < 1): {len(partials)}")
    for fw, c, r in partials:
        print(f"  - {fw}: {c} {r:.2f}")
    print(f"\nFORBID VIOLATIONS: {len(forbid_rows)}")
    for fw, what, got, cap in forbid_rows:
        print(f"  - {fw}: {what}  (matched {got}, max {cap})")
    print(f"\nMISSING CELLS: {len(cell_misses)}")
    for fw, cell, kind, node, registered in cell_misses:
        tag = "" if registered else "  (cell type not in registry)"
        print(f"  - {fw}: {cell} on {kind} {node!r}{tag}")
    print(f"\nGRADER ERRORS (fixture NOT in the matrix): {len(errors)}")
    for name, msg in errors:
        print(f"  - {name}: {msg}")

    if "--no-log" not in sys.argv:
        rec = {
            "ts": datetime.now(timezone.utc).isoformat(),
            "engine": _engine_version(),
            "blind_spots": [{"framework": fw, "category": c} for fw, c in blind],
            "partial_cells": [{"framework": fw, "category": c, "recall": r}
                              for fw, c, r in partials],
            "missing_nodes": [{"framework": fw, "kind": k, "name": n} for fw, k, n in node_misses],
            "missing_cells": [{"framework": fw, "cell": cl, "kind": k, "node": nd,
                               "registered": reg}
                              for fw, cl, k, nd, reg in cell_misses],
            "forbid_violations": [{"framework": fw, "what": w, "matched": g, "max_allowed": cap}
                                  for fw, w, g, cap in forbid_rows],
            "grader_errors": [{"fixture": n, "error": m} for n, m in errors],
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
