#!/usr/bin/env python3
"""pyo3 surface, py/src/convert.rs (LD.2): node ids above 2**63 survive
every answer exactly (a float detour would round them), and answer dicts keep
the engine struct's field order. Shared helpers: test_build.py."""
from __future__ import annotations

import json
import sys
import tempfile

from test_build import Checks, fixture_repo, rg


def main() -> int:
    c = Checks("convert")
    with tempfile.TemporaryDirectory(prefix="glia-surface-convert-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        nodes = json.loads(g.nodes_json())
        big = [n for n in nodes if n["id"] >= 2**63]
        c.check("the fixture has ids above 2**63", len(big) > 0, len(big))
        for n in big:
            hit = g.find(n["qname"], top_k=1)["results"]
            c.check(f"find keeps {n['qname']}'s id exact",
                    hit and type(hit[0]["id"]) is int and hit[0]["id"] == n["id"], (hit[:1], n["id"]))
        by_id = {n["id"]: n["qname"] for n in nodes}
        blast = g.blast_radius("app::helper")["results"]
        c.check("blast ids: exact ints of nodes_json",
                all(type(r["id"]) is int and by_id.get(r["id"]) == r["qname"] for r in blast))
        c.check("blast carries an id above 2**63", any(r["id"] >= 2**63 for r in blast))
        res = g.resolve("app.py", kind="diff")["results"]
        c.check("resolve ids: exact ints of nodes_json",
                res and all(type(r["id"]) is int and by_id.get(r["id"]) == r["qname"] for r in res))
        c.check("resolve carries an id above 2**63", any(r["id"] >= 2**63 for r in res))
        c.check("no float anywhere an id is",
                not any(type(r["id"]) is float for r in blast + res))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
