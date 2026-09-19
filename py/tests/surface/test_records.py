#!/usr/bin/env python3
"""pyo3 surface, py/src/records.rs (LD.2): a method whose name ends in
`_json` returns a JSON string (the bulk dumps). Shared helpers: test_build.py."""
from __future__ import annotations

import json
import sys
import tempfile

from test_build import Checks, fixture_repo, rg


def main() -> int:
    c = Checks("records")
    with tempfile.TemporaryDirectory(prefix="glia-surface-records-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        nodes, edges = g.nodes_json(), g.edges_json()
        c.check("nodes_json -> str", type(nodes) is str, type(nodes))
        c.check("edges_json -> str", type(edges) is str, type(edges))
        ns, es = json.loads(nodes), json.loads(edges)
        c.check("one record per node", len(ns) == g.node_count(), (len(ns), g.node_count()))
        c.check("node ids are ints", all(type(n["id"]) is int for n in ns))
        ids = {n["id"] for n in ns}
        c.check("edge ends are node ids",
                all(e["from"] in ids and e["to"] in ids for e in es) and len(es) > 0)
        c.check("parse_file_to_json -> str",
                type(rg.parse_file_to_json("def f():\n    pass\n", "m.py", "python")) is str)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
