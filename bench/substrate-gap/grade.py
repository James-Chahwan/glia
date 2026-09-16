#!/usr/bin/env python3
"""
Substrate-gap grader — glia P1 (handoff v6, 2026-07-08).

Measures glia's *edge-extraction recall* per (framework x edge-category) against
a hand-enumerated ground-truth key. This is the recall pattern from repo-graph's
`bench/grade.py` pushed DOWN to the substrate layer: instead of grading an
agent's answer, we grade the graph's edges directly.

Thesis link (handoff v6): every (framework, edge) cell that reads 0.0 is a task
grep wins TODAY that would flip to the graph once the edge is emitted. This
harness produces the full map of those blind spots so P1 is eval-driven, not
vibes-driven.

Each fixture dir holds:
  - source file(s) for ONE framework, with KNOWN edges,
  - key.json enumerating the edges (and key nodes) that SHOULD be extracted.

key.json schema:
  {
    "framework": "ts-angular-di",         # row label in the matrix
    "language":  "typescript",
    "dirs":      ["."],                    # relative dirs; 1 => generate(),
                                           #   2+ => generate_many() (distinct
                                           #   RepoIds so cross-graph resolvers
                                           #   fire — documented substrate-eval path)
    "expect_nodes": [                      # node-extraction ground truth
      {"kind": "ENDPOINT", "name": "GET /users", "note": "..."}
    ],
    "expect_edges": [                      # edge-extraction ground truth
      {"from": "AppComponent", "to": "ApiService",
       "category": "INJECTS", "note": "constructor DI"}
    ],
    "forbid": [                            # PRECISION: must NOT be emitted
      {"kind": "ROUTE", "name": "/users"},                 # no such node
      {"kind": "ROUTE", "name": "/users", "max_nodes": 1}, # at most N such
      {"from": "Nav", "to": "GET /users", "category": "HTTP_CALLS"}  # no edge
    ],
    "expect_cells": [                      # cell-level ground truth
      {"kind": "ENDPOINT", "node": "GET /users", "cell": "POSITION",
       "contains": "app.ts"}               # "contains" is optional
    ],
    "materialize": {                       # optional: copy trackable stand-ins
      "libs/sdk/.git": "libs/sdk/_dotgit"  #   into place for the duration of
    },                                     #   the grade — git refuses to track
                                           #   a path component named `.git`,
                                           #   so a fixture that needs one ships
                                           #   `_dotgit` and names it here.
                                           #   Removed again in a finally.
    "mechanism": "http",                   # optional: matrix row/col binding
    "cells": ["typescript/http"],          # optional: "<language>/<mechanism>"
    "note": "free-form commentary"         # optional, ignored by the grader
  }

This vocabulary is FROZEN. `grade_fixture` RAISES ValueError on any
unrecognised top-level field, and on unrecognised sub-fields of the lists
above. Silent tolerance of unknown fields is exactly what would let a
precision gate ship "green" while never executing, so do NOT invent a fifth
spelling of `forbid` (no forbid_edges / expect_absent_edges /
expect_absent_nodes / expect_literals) — extend the allow-lists below and
document it in README.md.

Matching is deliberately lenient on identity (case-folded substring over name OR
qname, with `::`/`.` normalised to `/`) and STRICT on kind/category id. We are
measuring "did the edge of the right category between the right two entities get
emitted at all", not exact-qname bookkeeping.

Usage:
  python3 grade.py fixtures/py-calls            # grade one fixture (verbose)
  python3 grade.py fixtures/py-calls --dump      # also dump ALL nodes+edges
                                                 # (use this to author keys)
"""
import json
import os
import shutil
import sys
from pathlib import Path

# Never scatter .gmap dirs into the fixture folders; keep grading hermetic.
os.environ.setdefault("GLIA_NO_PERSIST", "1")
import repo_graph_py as rg  # noqa: E402

_CAT_BY_NAME = {n: i for i, n in rg.category_names()}
_CAT_BY_ID = {i: n for i, n in rg.category_names()}
_KIND_BY_NAME = {n: i for i, n in rg.kind_names()}
_KIND_BY_ID = {i: n for i, n in rg.kind_names()}
_CELL_BY_NAME = {n: i for i, n in rg.cell_type_names()}
_CELL_BY_ID = {i: n for i, n in rg.cell_type_names()}

# ---- FROZEN key.json vocabulary (W0.4) -----------------------------------
# One spelling per concept. Anything not listed here RAISES — see module doc.
TOP_FIELDS = {
    "framework", "language", "dirs",     # identity
    "expect_nodes", "expect_edges",      # recall
    "expect_cells",                      # recall, cell level
    "forbid",                            # precision
    "materialize",                       # untrackable stand-ins (see _materialize)
    "mechanism", "cells",                # matrix binding (echoed, not graded)
    "note",                              # commentary
}
NODE_FIELDS = {"kind", "name", "note"}
EDGE_FIELDS = {"from", "to", "category", "note"}
FORBID_NODE_FIELDS = {"kind", "name", "max_nodes", "note"}
FORBID_EDGE_FIELDS = {"from", "to", "category", "note"}
CELL_FIELDS = {"kind", "node", "cell", "contains", "note"}


def _reject_unknown(fixture, obj, allowed, where="key.json field"):
    """Raise on the first (sorted => deterministic) unrecognised field."""
    for k in sorted(set(obj) - allowed):
        raise ValueError(f"{fixture}: unknown {where} {k!r}")


def _require(fixture, obj, fields, where):
    for f in fields:
        if f not in obj:
            raise ValueError(f"{fixture}: {where} is missing required field {f!r}")


def _norm(s):
    return (s or "").lower().replace("::", "/").replace(".", "/")


def _node_matches(node, pattern):
    """True if `pattern` (normalised) is a substring of the node's name or qname."""
    p = _norm(pattern)
    return p in _norm(node.get("name", "")) or p in _norm(node.get("qname", ""))


def _materialize(fixture_dir, key):
    """Copy trackable stand-ins into the untrackable names a fixture needs.

    git will not track a path component named `.git` (`git add -A` silently
    keeps only its siblings), so a fixture proving "a nested .git makes a
    REGION anchor instead of being walked into" cannot ship the file it needs.
    It ships `libs/sdk/_dotgit` and maps it here; the copy exists only for the
    duration of `build_graph` and is removed in its finally.
    """
    created = []
    spec = key.get("materialize") or {}
    root = Path(fixture_dir).resolve()
    if not isinstance(spec, dict):
        raise ValueError(f"{root.name}: materialize must be an object, got {type(spec).__name__}")
    for dest_rel, src_rel in sorted(spec.items()):
        if not isinstance(src_rel, str):
            raise ValueError(f"{root.name}: materialize source must be a string: {dest_rel}")
        dest = (root / dest_rel).resolve()
        src = (root / src_rel).resolve()
        if root not in dest.parents or root not in src.parents:
            raise ValueError(f"{root.name}: materialize path escapes fixture dir: {dest_rel}")
        if not src.exists():
            raise ValueError(f"{root.name}: materialize source missing: {src_rel}")
        if dest.exists() or dest.is_symlink():
            raise ValueError(f"{root.name}: materialize destination already exists: {dest_rel}")
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(src, dest) if src.is_dir() else shutil.copyfile(src, dest)
        created.append(dest)
    if created:
        print(f"[materialize] {len(created)} paths in {root.name}", file=sys.stderr)
    return created


def _dematerialize(created):
    """Remove every stand-in. One failure must not mask the rest: a leaked
    `.git` under fixtures/ would make the glia tree itself look like it has a
    submodule, so each removal is reported, never raised."""
    for path in reversed(created):
        try:
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink(missing_ok=True)
        except OSError as exc:  # pragma: no cover - cleanup must never mask
            print(f"[materialize] FAILED to remove {path}: {exc}", file=sys.stderr)


def build_graph(fixture_dir, key):
    created = _materialize(fixture_dir, key)
    try:
        dirs = [str((fixture_dir / d).resolve()) for d in key.get("dirs", ["."])]
        if len(dirs) == 1:
            g = rg.generate(dirs[0], False)  # non-incremental => hermetic per run
        else:
            g = rg.generate_many(dirs)  # distinct RepoIds => cross resolvers fire
        nodes = json.loads(g.nodes_json())
        edges = json.loads(g.edges_json())
        by_id = {n["id"]: n for n in nodes}
        return g, nodes, edges, by_id
    finally:
        # Cells are read off the in-memory graph after this returns, so the
        # stand-ins are safe to drop here even on the _dump / expect_cells path.
        _dematerialize(created)


def _node_matches_exact(node, pattern):
    """STRICT identity: the normalised pattern IS the node's name or qname.

    Precision gates must not use the recall gates' substring matcher. Leniency
    makes a recall gate generous (it can only turn a miss into a hit), but it
    makes a precision gate FALSE-POSITIVE: `forbid {to: "UserController"}` would
    also match the method `UserController::UserController::getUser`, because the
    forbidden name is an ancestor segment of a perfectly legitimate target. That
    fired on java-spring-composed in wave 2 and accused a correct graph.
    """
    p = _norm(pattern)
    return p == _norm(node.get("name", "")) or p == _norm(node.get("qname", ""))


def _grade_forbid(fixture, key, nodes, edges, by_id):
    """PRECISION gate: things that must NOT be emitted.

    Strict identity (`_node_matches_exact`), NOT the recall gates' substring
    matcher — see that function for why. Kind/category ids are strict here as
    they are everywhere.
    """
    results = []
    for i, exp in enumerate(key.get("forbid", [])):
        where = f"forbid[{i}]"
        if {"from", "to", "category"} & set(exp):  # --- edge form ---
            _reject_unknown(fixture, exp, FORBID_EDGE_FIELDS, f"key.json field in {where}")
            _require(fixture, exp, ("from", "to", "category"), where)
            cat_id = _CAT_BY_NAME.get(exp["category"])
            if cat_id is None:
                raise ValueError(f"{fixture}: unknown category {exp['category']!r}")
            count = 0
            for e in edges:
                if e["category"] != cat_id:
                    continue
                fr, to = by_id.get(e["from"]), by_id.get(e["to"])
                if fr and to and _node_matches_exact(fr, exp["from"]) and _node_matches_exact(to, exp["to"]):
                    count += 1
            limit = 0
        else:  # --- node form ---
            _reject_unknown(fixture, exp, FORBID_NODE_FIELDS, f"key.json field in {where}")
            _require(fixture, exp, ("kind",), where)
            kind_id = _KIND_BY_NAME.get(exp["kind"])
            if kind_id is None:
                raise ValueError(f"{fixture}: unknown kind {exp['kind']!r}")
            name = exp.get("name")
            count = sum(
                1 for n in nodes
                if n["kind"] == kind_id and (name is None or _node_matches_exact(n, name))
            )
            limit = int(exp.get("max_nodes", 0))
        results.append({**exp, "matched": count, "max_allowed": limit,
                        "violated": count > limit})
    return results


def _grade_cells(fixture, key, g, nodes):
    """Cell-level recall: node of `kind` matching `node` carries cell `cell`.

    An unregistered cell NAME scores a miss rather than raising, so a 0.00
    baseline is recordable for a cell type a later packet still has to add.
    `contains` is a case-folded plain substring of the cell payload (NOT the
    `_norm` identity matcher — payloads are literals, not qnames).
    """
    results = []
    for i, exp in enumerate(key.get("expect_cells", [])):
        where = f"expect_cells[{i}]"
        _reject_unknown(fixture, exp, CELL_FIELDS, f"key.json field in {where}")
        _require(fixture, exp, ("kind", "node", "cell"), where)
        kind_id = _KIND_BY_NAME.get(exp["kind"])
        if kind_id is None:
            raise ValueError(f"{fixture}: unknown kind {exp['kind']!r}")
        cell_id = _CELL_BY_NAME.get(exp["cell"])
        cands = [n for n in nodes
                 if n["kind"] == kind_id and _node_matches(n, exp["node"])]
        want = (exp.get("contains") or "").casefold()
        hit = False
        if cell_id is not None:
            for n in cands:
                for ct, payload in g.node_cells(n["id"]):
                    if ct == cell_id and (not want or want in (payload or "").casefold()):
                        hit = True
                        break
                if hit:
                    break
        results.append({**exp, "found": hit, "candidates": len(cands),
                        "registered": cell_id is not None})
    return results


def grade_fixture(fixture_dir):
    fixture_dir = Path(fixture_dir)
    key = json.loads((fixture_dir / "key.json").read_text())
    fixture = fixture_dir.name
    # FROZEN vocabulary: an unknown field is a dead gate, so refuse to run.
    _reject_unknown(fixture, key, TOP_FIELDS)
    g, nodes, edges, by_id = build_graph(fixture_dir, key)

    # ---- node-level extraction recall (per kind) ----
    node_results = []
    for i, exp in enumerate(key.get("expect_nodes", [])):
        _reject_unknown(fixture, exp, NODE_FIELDS, f"key.json field in expect_nodes[{i}]")
        _require(fixture, exp, ("kind", "name"), f"expect_nodes[{i}]")
        kind_id = _KIND_BY_NAME.get(exp["kind"])
        if kind_id is None:
            raise ValueError(f"{fixture_dir.name}: unknown kind {exp['kind']!r}")
        hit = any(n["kind"] == kind_id and _node_matches(n, exp["name"]) for n in nodes)
        node_results.append({**exp, "found": hit})

    # ---- edge-level extraction recall (per category) ----
    edge_results = []
    for i, exp in enumerate(key.get("expect_edges", [])):
        _reject_unknown(fixture, exp, EDGE_FIELDS, f"key.json field in expect_edges[{i}]")
        _require(fixture, exp, ("from", "to", "category"), f"expect_edges[{i}]")
        cat_id = _CAT_BY_NAME.get(exp["category"])
        if cat_id is None:
            raise ValueError(f"{fixture_dir.name}: unknown category {exp['category']!r}")
        hit = False
        for e in edges:
            if e["category"] != cat_id:
                continue
            fr, to = by_id.get(e["from"]), by_id.get(e["to"])
            if fr and to and _node_matches(fr, exp["from"]) and _node_matches(to, exp["to"]):
                hit = True
                break
        edge_results.append({**exp, "found": hit})

    per_cat = {}
    for r in edge_results:
        c = r["category"]
        slot = per_cat.setdefault(c, {"expected": 0, "found": 0})
        slot["expected"] += 1
        slot["found"] += 1 if r["found"] else 0
    for slot in per_cat.values():
        slot["recall"] = slot["found"] / slot["expected"] if slot["expected"] else None

    node_by_kind = {}
    for r in node_results:
        k = r["kind"]
        slot = node_by_kind.setdefault(k, {"expected": 0, "found": 0})
        slot["expected"] += 1
        slot["found"] += 1 if r["found"] else 0
    for slot in node_by_kind.values():
        slot["recall"] = slot["found"] / slot["expected"] if slot["expected"] else None

    # ---- precision gate + cell-level recall (frozen vocabulary) ----
    forbid_results = _grade_forbid(fixture, key, nodes, edges, by_id)
    cell_results = _grade_cells(fixture, key, g, nodes)
    per_cell = {}
    for r in cell_results:
        slot = per_cell.setdefault(r["cell"], {"expected": 0, "found": 0})
        slot["expected"] += 1
        slot["found"] += 1 if r["found"] else 0
    for slot in per_cell.values():
        slot["recall"] = slot["found"] / slot["expected"] if slot["expected"] else None

    return {
        "framework": key.get("framework", fixture_dir.name),
        "language": key.get("language", ""),
        "node_count": len(nodes),
        "edge_count": len(edges),
        "node_recall": node_results,
        "edge_recall": edge_results,
        "per_category": per_cat,
        "node_by_kind": node_by_kind,
        # --- W0.4 frozen-vocabulary additions ---
        "cell_recall": cell_results,
        "per_cell": per_cell,
        "forbid_results": forbid_results,
        "forbid_violations": sum(1 for r in forbid_results if r["violated"]),
        # echoed for the derived (language x mechanism) matrix; not graded here
        "mechanism": key.get("mechanism", ""),
        "cells": key.get("cells", []),
    }


def _dump(fixture_dir):
    key = json.loads((Path(fixture_dir) / "key.json").read_text())
    _, nodes, edges, by_id = build_graph(Path(fixture_dir), key)
    print(f"\n=== NODES ({len(nodes)}) ===")
    for n in sorted(nodes, key=lambda n: (n["kind"], n.get("qname", ""))):
        kn = _KIND_BY_ID.get(n["kind"], n["kind"])
        print(f"  [{kn:<12}] {n.get('name','')!r:<28} qname={n.get('qname','')!r} path={n.get('path')}")
    print(f"\n=== EDGES ({len(edges)}) ===")
    for e in sorted(edges, key=lambda e: e["category"]):
        cn = _CAT_BY_ID.get(e["category"], e["category"])
        fr = by_id.get(e["from"], {})
        to = by_id.get(e["to"], {})
        print(f"  [{cn:<14}] {fr.get('name','?')!r} -> {to.get('name','?')!r}")


def _print_verbose(res):
    print(f"\n### {res['framework']} ({res['language']})  "
          f"nodes={res['node_count']} edges={res['edge_count']}")
    if res["node_recall"]:
        print("  nodes:")
        for r in res["node_recall"]:
            mark = "OK " if r["found"] else "XX "
            print(f"    {mark} {r['kind']:<10} {r['name']!r}  — {r.get('note','')}")
    if res["edge_recall"]:
        print("  edges:")
        for r in res["edge_recall"]:
            mark = "OK " if r["found"] else "XX "
            print(f"    {mark} {r['category']:<14} {r['from']!r} -> {r['to']!r}  — {r.get('note','')}")
    if res["cell_recall"]:
        print("  cells:")
        for r in res["cell_recall"]:
            mark = "OK " if r["found"] else "XX "
            extra = "" if r["registered"] else "  (UNREGISTERED cell type)"
            sub_ = f" contains {r['contains']!r}" if r.get("contains") else ""
            print(f"    {mark} {r['cell']:<14} {r['kind']} {r['node']!r}{sub_}"
                  f"  — {r.get('note','')}{extra}")
    if res["forbid_results"]:
        print("  forbid:")
        for r in res["forbid_results"]:
            mark = "XX " if r["violated"] else "OK "
            what = (f"{r['category']:<14} {r['from']!r} -> {r['to']!r}"
                    if "category" in r else
                    f"{r['kind']:<14} {r.get('name', '*')!r}")
            print(f"    {mark} {what}  matched={r['matched']} "
                  f"max={r['max_allowed']}  — {r.get('note','')}")
        print(f"  FORBID VIOLATIONS: {res['forbid_violations']}")
    print("  per-category recall:")
    for cat, v in sorted(res["per_category"].items()):
        pct = "n/a" if v["recall"] is None else f"{v['recall']:.2f}"
        flag = ""
        if v["recall"] == 0.0:
            flag = "  <-- BLIND SPOT"
        elif v["recall"] is not None and v["recall"] < 1.0:
            flag = "  <-- PARTIAL"
        print(f"    {cat:<16} {v['found']}/{v['expected']}  ({pct}){flag}")
    if res["per_cell"]:
        print("  per-cell recall:")
        for cell, v in sorted(res["per_cell"].items()):
            pct = "n/a" if v["recall"] is None else f"{v['recall']:.2f}"
            print(f"    {cell:<16} {v['found']}/{v['expected']}  ({pct})")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if not args:
        print(__doc__)
        sys.exit(2)
    target = args[0]
    if "--dump" in sys.argv:
        _dump(target)
    _print_verbose(grade_fixture(target))
