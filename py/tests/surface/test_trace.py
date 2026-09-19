#!/usr/bin/env python3
"""pyo3 surface, py/src/trace.rs (LD.2): `cross_stack_trace` returns a
native list of hop dicts (LD.4a later makes it a dict and updates this file).
Shared helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, rg

KEYS = ["depth", "mechanism", "cross_service", "from_qname", "to_qname", "to_kind", "to_file", "to_line"]


def main() -> int:
    c = Checks("trace")
    with tempfile.TemporaryDirectory(prefix="glia-surface-trace-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        t = g.cross_stack_trace("app::main")
        c.check("cross_stack_trace -> list", type(t) is list, type(t))
        hops = t if type(t) is list else []
        c.check("main -> helper hop", any(h.get("to_qname") == "app::helper" for h in hops), hops[:3])
        c.check("hop keys in engine field order", hops and list(hops[0]) == KEYS, hops and list(hops[0]))
        c.raises("unknown feature", ValueError, lambda: g.cross_stack_trace("no::such::thing"))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
