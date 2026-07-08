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
    ]
  }

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
import sys
from pathlib import Path

# Never scatter .gmap dirs into the fixture folders; keep grading hermetic.
os.environ.setdefault("GLIA_NO_PERSIST", "1")
import repo_graph_py as rg  # noqa: E402

_CAT_BY_NAME = {n: i for i, n in rg.category_names()}
_CAT_BY_ID = {i: n for i, n in rg.category_names()}
_KIND_BY_NAME = {n: i for i, n in rg.kind_names()}
_KIND_BY_ID = {i: n for i, n in rg.kind_names()}


def _norm(s):
    return (s or "").lower().replace("::", "/").replace(".", "/")


def _node_matches(node, pattern):
    """True if `pattern` (normalised) is a substring of the node's name or qname."""
    p = _norm(pattern)
    return p in _norm(node.get("name", "")) or p in _norm(node.get("qname", ""))


def build_graph(fixture_dir, key):
    dirs = [str((fixture_dir / d).resolve()) for d in key.get("dirs", ["."])]
    if len(dirs) == 1:
        g = rg.generate(dirs[0], False)  # non-incremental => hermetic per run
    else:
        g = rg.generate_many(dirs)  # distinct RepoIds => cross resolvers fire
    nodes = json.loads(g.nodes_json())
    edges = json.loads(g.edges_json())
    by_id = {n["id"]: n for n in nodes}
    return nodes, edges, by_id


def grade_fixture(fixture_dir):
    fixture_dir = Path(fixture_dir)
    key = json.loads((fixture_dir / "key.json").read_text())
    nodes, edges, by_id = build_graph(fixture_dir, key)

    # ---- node-level extraction recall (per kind) ----
    node_results = []
    for exp in key.get("expect_nodes", []):
        kind_id = _KIND_BY_NAME.get(exp["kind"])
        if kind_id is None:
            raise ValueError(f"{fixture_dir.name}: unknown kind {exp['kind']!r}")
        hit = any(n["kind"] == kind_id and _node_matches(n, exp["name"]) for n in nodes)
        node_results.append({**exp, "found": hit})

    # ---- edge-level extraction recall (per category) ----
    edge_results = []
    for exp in key.get("expect_edges", []):
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

    return {
        "framework": key.get("framework", fixture_dir.name),
        "language": key.get("language", ""),
        "node_count": len(nodes),
        "edge_count": len(edges),
        "node_recall": node_results,
        "edge_recall": edge_results,
        "per_category": per_cat,
        "node_by_kind": node_by_kind,
    }


def _dump(fixture_dir):
    key = json.loads((Path(fixture_dir) / "key.json").read_text())
    nodes, edges, by_id = build_graph(Path(fixture_dir), key)
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
    print("  per-category recall:")
    for cat, v in sorted(res["per_category"].items()):
        pct = "n/a" if v["recall"] is None else f"{v['recall']:.2f}"
        flag = "  <-- BLIND SPOT" if v["recall"] == 0.0 else ""
        print(f"    {cat:<16} {v['found']}/{v['expected']}  ({pct}){flag}")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if not args:
        print(__doc__)
        sys.exit(2)
    target = args[0]
    if "--dump" in sys.argv:
        _dump(target)
    _print_verbose(grade_fixture(target))
