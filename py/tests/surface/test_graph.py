#!/usr/bin/env python3
"""pyo3 surface, py/src/graph.rs (LD.2): the pair-shaped accessors stay
tuples, and `save_to` / `save_to_default` are the only layout writers.
Shared helpers: test_build.py."""
from __future__ import annotations

import json
import os
import sys
import tempfile

from test_build import Checks, fixture_repo, rg, tree


def main() -> int:
    c = Checks("graph")
    with tempfile.TemporaryDirectory(prefix="glia-surface-graph-") as tmp:
        repo = fixture_repo(tmp)
        g = rg.generate(repo)
        c.check("node_count is an int", type(g.node_count()) is int and g.node_count() > 0)
        c.check("parse_errors is a list", g.parse_errors == [], g.parse_errors)
        helper = next(n for n in json.loads(g.nodes_json()) if n["qname"] == "app::helper")
        cells = g.node_cells(helper["id"])
        c.check("node_cells: (int, str) pairs",
                cells and all(type(x) is tuple and type(x[0]) is int and type(x[1]) is str for x in cells),
                cells[:2])
        nb = g.neighbours(helper["id"], "both")
        c.check("neighbours: (int, int, str) triples (LD.3c)",
                nb and all(type(x) is tuple and len(x) == 3 and type(x[0]) is int and type(x[1]) is int
                           and x[2] in ("out", "in") for x in nb), nb[:2])

        before = tree(repo)
        out = os.path.join(tmp, "layout")
        g.save_to(out)
        c.check("save_to writes the layout", os.path.exists(os.path.join(out, "manifest.json")))
        c.check("save_to leaves the repo alone", tree(repo) == before)
        g.save_to_default(repo)
        c.check("save_to_default writes <repo>/.glia/graph",
                os.path.exists(os.path.join(rg.default_gmap_dir(repo), "manifest.json")))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
