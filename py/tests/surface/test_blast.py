#!/usr/bin/env python3
"""pyo3 surface, py/src/blast.rs (LD.2): `blast_radius` returns a native
list of dicts. Shared helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, node_ids, rg

KEYS = ["id", "qname", "name", "kind", "reason", "depth", "score", "live", "file", "line"]


def main() -> int:
    c = Checks("blast")
    with tempfile.TemporaryDirectory(prefix="glia-surface-blast-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        ids = node_ids(g)
        b = g.blast_radius("app::helper")
        c.check("blast_radius -> list", type(b) is list, type(b))
        b = b if type(b) is list else []
        c.check("blast_radius reaches the callers", any(r.get("qname") == "app::main" for r in b),
                [r.get("qname") for r in b][:5])
        c.check("record keys in engine field order", b and list(b[0]) == KEYS, b and list(b[0]))
        c.check("ids are ints of the graph", all(type(r["id"]) is int and r["id"] in ids for r in b))
        c.check("score is a float", all(type(r["score"]) is float for r in b))
        c.check("line is an int (1-based) or None",
                all(r["line"] is None or (type(r["line"]) is int and r["line"] >= 1) for r in b))
        c.raises("unknown qname", ValueError, lambda: g.blast_radius("no::such::thing"))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
