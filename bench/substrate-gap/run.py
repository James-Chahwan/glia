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
That file is gitignored and machine-local, so it proves nothing to anyone else.

The COMMITTED proof of record is legacy-latest.json — this runner's view (per
fixture edge recall + the five summary sections), written ONLY on `--emit` and
verified by `--check`. It is deliberately NOT rewritten on every run: agents
grade constantly for baselines, and a committed file that changed on every run
would dirty the tree for every sibling mid-wave. It is deterministic (no
timestamp, no build stamp, sort_keys) so `git diff` is the signal. The
language x mechanism grid is a DIFFERENT view with a different schema, owned by
matrix.py (results-latest.json + COVERAGE.md); the two never share a file.

Usage:
  python3 run.py                 # grade all fixtures, print matrix
  python3 run.py --no-log        # don't append to results.jsonl
  python3 run.py --no-log --emit   # rewrite the committed legacy-latest.json
  python3 run.py --no-log --check  # exit 1, naming each change, if it is stale

Default stdout is unchanged by --emit/--check; their markers go to stderr and
drift lines are appended to stdout under their own header.
"""
import argparse
import json
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from grade import grade_fixture  # noqa: E402

FIXTURES = HERE / "fixtures"
RESULTS = HERE / "results.jsonl"
LEGACY_PATH = HERE / "legacy-latest.json"
# A string, not results-latest.json's integer `schema: 1`, so neither file can
# be mistaken for the other by a reader keying on `schema`.
LEGACY_SCHEMA = "substrate-gap-legacy/1"
LEGACY_VIEW = ("run.py: per-fixture edge recall over fixtures/* plus its summary "
               "sections; the language x mechanism grid is results-latest.json")
SUMMARY_KEYS = ("blind_spots", "missing_nodes", "partial_cells", "forbid_violations",
                "missing_cells", "grader_errors")


def discover():
    return sorted(p for p in FIXTURES.iterdir() if (p / "key.json").exists())


def main(argv=None):
    ap = argparse.ArgumentParser(description="grade fixtures/* and print the blind-spot matrix")
    ap.add_argument("--no-log", action="store_true", help="don't append to results.jsonl")
    ap.add_argument("--emit", action="store_true",
                    help=f"rewrite the committed {LEGACY_PATH.name}")
    ap.add_argument("--check", action="store_true",
                    help=f"exit 1 if the committed {LEGACY_PATH.name} is stale")
    args = ap.parse_args(argv)

    graded = []  # (fixture dir name, result) — `framework` is NOT unique per fixture
    errors = []
    fixture_count = 0
    for fx in discover():
        fixture_count += 1
        try:
            graded.append((fx.name, grade_fixture(fx)))
        except Exception as e:  # noqa: BLE001 — a broken fixture shouldn't sink the run
            # A raising fixture drops OUT of the matrix, so it must also be
            # reported on stdout (below) — otherwise the frozen-vocabulary
            # ValueError is muffled into a silently-missing row.
            errors.append((fx.name, f"{type(e).__name__}: {e}"))
            print(f"!! {fx.name}: {type(e).__name__}: {e}", file=sys.stderr)
    results = [res for _, res in graded]

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

    # Each summary line is spelled ONCE and shared by stdout and the committed
    # snapshot, so the two can never disagree about what a line says.
    lines = {"blind_spots": [f"{fw}: {c}" for fw, c in blind],
             "missing_nodes": [f"{fw}: {k} {n!r}" for fw, k, n in node_misses]}
    print(f"\nBLIND SPOTS (recall 0.00): {len(blind)}")
    for line in lines["blind_spots"]:
        print(f"  - {line}")
    if node_misses:
        print(f"\nMISSING NODES ({len(node_misses)}):")
        for line in lines["missing_nodes"]:
            print(f"  - {line}")

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

    lines["partial_cells"] = [f"{fw}: {c} {r:.2f}" for fw, c, r in partials]
    lines["forbid_violations"] = [f"{fw}: {what}  (matched {got}, max {cap})"
                                  for fw, what, got, cap in forbid_rows]
    lines["missing_cells"] = [
        f"{fw}: {cell} on {kind} {node!r}"
        + ("" if registered else "  (cell type not in registry)")
        for fw, cell, kind, node, registered in cell_misses]
    lines["grader_errors"] = [f"{name}: {msg}" for name, msg in errors]
    for key, title in (("partial_cells", "PARTIAL (0 < recall < 1)"),
                       ("forbid_violations", "FORBID VIOLATIONS"),
                       ("missing_cells", "MISSING CELLS"),
                       ("grader_errors", "GRADER ERRORS (fixture NOT in the matrix)")):
        print(f"\n{title}: {len(lines[key])}")
        for line in lines[key]:
            print(f"  - {line}")

    if not args.no_log:
        rec = {
            "ts": datetime.now(timezone.utc).isoformat(),
            "engine": _engine_version(),
            "build_stamp": _build_stamp(),
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

    if not (args.emit or args.check):
        return 0
    payload = legacy_payload(fixture_count, graded, lines, _engine_version())
    tail = (f"{payload['fixtures']} fixtures, {len(lines['blind_spots'])} blind spots, "
            f"engine {payload['engine']}")
    if args.emit:
        if not graded:
            # Every fixture raised (no wheel?): writing would wipe the proof.
            print(f"[substrate-gap] refusing to write {LEGACY_PATH.name} — "
                  f"0 of {fixture_count} fixtures graded", file=sys.stderr)
            return 1
        LEGACY_PATH.write_text(render_legacy(payload), encoding="utf-8")
        print(f"[substrate-gap] wrote {LEGACY_PATH.name} — {tail}", file=sys.stderr)
    if args.check:
        drift = check_legacy(payload)
        if drift:
            print(f"\nLEGACY SNAPSHOT DRIFT ({LEGACY_PATH.name}): {len(drift)}")
            for line in drift:
                print(f"  - {line}")
            print(f"[substrate-gap] check: DRIFT {len(drift)} change(s) in "
                  f"{LEGACY_PATH.name} — re-run with --emit if intended", file=sys.stderr)
            return 1
        print(f"[substrate-gap] check: OK {LEGACY_PATH.name} matches a fresh run ({tail})",
              file=sys.stderr)
    return 0


# ---------------------------------------------------------------------------
# legacy-latest.json — the committed, deterministic view of this runner
# ---------------------------------------------------------------------------

def legacy_payload(fixture_count, graded, lines, engine):
    """The committed object. Pure: same inputs -> same bytes.

    `recall` is keyed by fixture DIRECTORY (framework names repeat across
    fixtures) and carries every per-category recall, so a sub-1.0 cell is
    visible here even though only 0.00 cells reach `blind_spots`.
    No timestamp and no build stamp: either would drift on every emit/rebuild
    without any recall moving.
    """
    return {
        "schema": LEGACY_SCHEMA,
        "view": LEGACY_VIEW,
        "engine": engine,
        "fixtures": fixture_count,
        "summary": {k: list(lines.get(k, [])) for k in SUMMARY_KEYS},
        "recall": {
            name: {
                "framework": res["framework"],
                "language": res["language"],
                "per_category": {
                    c: (None if v["recall"] is None else round(v["recall"], 4))
                    for c, v in res["per_category"].items()},
            }
            for name, res in graded
        },
    }


def render_legacy(payload):
    return json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


_ABSENT = "(absent)"


def _flatten(obj, prefix=""):
    """Dotted leaf paths. Lists are leaves (they only ever hold summary lines)."""
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            out.update(_flatten(v, f"{prefix}.{k}" if prefix else str(k)))
        if not obj and prefix:
            out[prefix] = {}
        return out
    return {prefix: obj}


def _show(v):
    return v if v is _ABSENT else json.dumps(v, sort_keys=True, ensure_ascii=False)


def legacy_drift(committed, payload):
    """One line per change, `<path>: <committed> -> <measured>`.

    A summary list reports members as `+ line` / `- line` so a new blind spot
    reads as itself, not as a re-dump of the whole list.
    """
    old = _flatten(committed) if isinstance(committed, dict) else {}
    new = _flatten(payload)
    out = []
    for path in sorted(set(old) | set(new)):
        was, now = old.get(path, _ABSENT), new.get(path, _ABSENT)
        if was == now:
            continue
        if isinstance(was, list) and isinstance(now, list):
            a, b = Counter(map(_show, was)), Counter(map(_show, now))
            moved = [f"{path}: - {x}" for x in sorted((a - b).elements())]
            moved += [f"{path}: + {x}" for x in sorted((b - a).elements())]
            out.extend(moved or [f"{path}: same lines, order changed"])
        else:
            out.append(f"{path}: {_show(was)} -> {_show(now)}")
    return out


def check_legacy(payload, path=LEGACY_PATH):
    """Drift lines; an empty list means the committed bytes are current."""
    try:
        raw = Path(path).read_text(encoding="utf-8")
    except FileNotFoundError:
        return [f"{Path(path).name}: {_ABSENT} — run `run.py --no-log --emit`"]
    try:
        committed = json.loads(raw)
    except json.JSONDecodeError as exc:
        return [f"{Path(path).name}: not valid JSON ({exc})"]
    drift = legacy_drift(committed, payload)
    if not drift and raw != render_legacy(payload):
        drift = [f"{Path(path).name}: bytes differ (no value changed) — re-run --emit"]
    return drift


def _engine_version():
    try:
        import glia_py as rg
        return rg.version()
    except Exception:  # noqa: BLE001
        return "unknown"


def _build_stamp():
    """`<release>+p<16 hex>` of the INSTALLED wheel. Two records with the same
    `engine` but different stamps graded different parser code; the same stamp
    across a claimed rebuild means maturin repackaged a stale .so."""
    try:
        import glia_py as rg
        return rg.build_stamp()
    except Exception:  # noqa: BLE001 — older wheel without the symbol
        return "unknown"


if __name__ == "__main__":
    sys.exit(main())
