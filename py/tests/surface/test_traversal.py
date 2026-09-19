#!/usr/bin/env python3
"""pyo3 surface, py/src/traversal.rs (LD.3c): `neighbours` with a direction
and a category filter, and the walks `bfs`, `predecessors`, `reachable_by`,
`shortest_path` over the merged graph. Rows are tuples of ints; `categories=None`
keeps every category (DEFINES included). Shared helpers: test_build.py.

Over LD.1's 9-line app: `app` DEFINES `helper` and `main`, `main` CALLS
`helper`. The repo dir is `ld2app` so the ids are the deterministic ones
test_build.py describes (some sit above 2**63)."""
from __future__ import annotations

import json
import pathlib
import sys
import tempfile

from test_build import Checks, node_ids, params, rg, stderr_of

APP = "import os\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n"

SIGNATURES = {
    "neighbours": [("node_id", None), ("direction", "out"), ("categories", None)],
    "bfs": [("node_id", None), ("direction", "out"), ("categories", None), ("depth", 3)],
    "predecessors": [("node_id", None), ("categories", None), ("depth", 3)],
    "reachable_by": [("sink_id", None), ("source_ids", None), ("categories", None), ("depth", 6)],
    "shortest_path": [("from_id", None), ("to_id", None), ("direction", "both"), ("categories", None),
                      ("depth", 12)],
}


def main() -> int:
    c = Checks("traversal")
    for name, want in SIGNATURES.items():
        fn = getattr(rg.PyGraph, name, None)
        c.check(f"{name} signature", fn is not None and params(fn) == want, fn and params(fn))

    cat = {n: i for i, n in rg.category_names()}
    calls, defines = cat["CALLS"], cat["DEFINES"]
    with tempfile.TemporaryDirectory(prefix="glia-surface-traversal-") as tmp:
        root = pathlib.Path(tmp) / "ld2app"
        root.mkdir()
        (root / "app.py").write_text(APP)
        g = rg.generate(str(root))
        ids = {n["qname"]: n["id"] for n in json.loads(g.nodes_json())}
        app, helper, main_ = ids["app"], ids["app::helper"], ids["app::main"]
        known = node_ids(g)

        def call(name, *args):
            fn = getattr(g, name, None)
            if fn is None:
                return None
            try:
                return fn(*args)
            except Exception as e:  # noqa: BLE001 - reported by the check that reads it
                return e

        # neighbours: both directions, every category by default.
        nb_in, err = stderr_of(lambda: call("neighbours", helper, "in"))
        c.check("neighbours(helper, 'in') sees main's CALLS and app's DEFINES",
                nb_in == [(app, defines, "in"), (main_, calls, "in")], nb_in)
        c.check("neighbours is silent", "[traverse]" not in err, err[-300:])
        c.check("neighbours(helper, 'in', [CALLS])",
                call("neighbours", helper, "in", [calls]) == [(main_, calls, "in")],
                call("neighbours", helper, "in", [calls]))
        c.check("neighbours(main) is outgoing triples",
                call("neighbours", main_) == [(helper, calls, "out")], call("neighbours", main_))
        c.check("neighbours(helper) default 'out' is empty", call("neighbours", helper) == [],
                call("neighbours", helper))
        both = call("neighbours", main_, "both")
        c.check("neighbours(main, 'both')", both == [(app, defines, "in"), (helper, calls, "out")], both)
        c.check("neighbour ids are exact ints",
                type(both) is list and all(type(r[0]) is int and r[0] in known for r in both), both)
        c.raises("direction 'sideways'", ValueError, lambda: g.neighbours(helper, "sideways"),
                 '"out", "in", "both"')
        c.raises("categories=[999]", ValueError, lambda: g.neighbours(helper, "in", [999]), "999")

        # bfs: the marker fires, rows are (id, depth, via, parent).
        walked, err = stderr_of(lambda: call("bfs", main_, "out"))
        c.check("bfs(main, 'out') reaches helper at depth 1 via CALLS",
                type(walked) is list and (helper, 1, calls, main_) in walked, walked)
        c.check("bfs marker", "[traverse] op=bfs walk=Forward seeds=1 reached=1 index_nodes=3" in err,
                err[-300:])
        c.check("bfs depth=0 reaches nothing", call("bfs", main_, "out", None, 0) == [],
                call("bfs", main_, "out", None, 0))
        c.check("bfs(helper, 'in')",
                call("bfs", helper, "in") == [(app, 1, defines, helper), (main_, 1, calls, helper)],
                call("bfs", helper, "in"))
        c.raises("bfs direction 'up'", ValueError, lambda: g.bfs(main_, "up"), "direction")

        # predecessors / reachable_by.
        preds, err = stderr_of(lambda: call("predecessors", helper, [calls]))
        c.check("predecessors(helper, [CALLS]) == [main]", preds == [main_], preds)
        c.check("predecessors marker", "[traverse] op=predecessors walk=Backward" in err, err[-300:])
        c.check("predecessors(helper) keeps every category",
                call("predecessors", helper) == [app, main_], call("predecessors", helper))
        c.raises("predecessors categories=[999]", ValueError, lambda: g.predecessors(helper, [999]), "999")
        hits, err = stderr_of(lambda: call("reachable_by", helper, [main_, app], [calls]))
        c.check("reachable_by(helper, [main, app], [CALLS]) == [main]", hits == [main_], hits)
        c.check("reachable_by marker", "[traverse] op=reachable_by walk=Backward" in err, err[-300:])
        c.check("reachable_by keeps source order",
                call("reachable_by", helper, [app, main_]) == [app, main_],
                call("reachable_by", helper, [app, main_]))

        # shortest_path.
        path, err = stderr_of(lambda: call("shortest_path", helper, main_, "both"))
        c.check("shortest_path(helper, main, 'both')", path == [(helper, None), (main_, calls)], path)
        c.check("shortest_path marker", "[traverse] op=shortest_path walk=Both" in err, err[-300:])
        c.check("shortest_path(helper, main, 'out') is None",
                call("shortest_path", helper, main_, "out") is None,
                call("shortest_path", helper, main_, "out"))
        c.check("shortest_path to itself",
                call("shortest_path", main_, main_) == [(main_, None)], call("shortest_path", main_, main_))
        c.raises("shortest_path categories=[999]", ValueError,
                 lambda: g.shortest_path(helper, main_, "both", [999]), "999")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
