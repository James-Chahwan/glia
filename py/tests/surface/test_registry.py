#!/usr/bin/env python3
"""pyo3 surface, py/src/registry.rs: the id -> name tables and the build
identity. LD.6: `entry_kinds()` is the code domain's entrypoint kinds, in table
order - the set a consumer derives its entry tiering from instead of keeping a
copy. Shared helpers: test_build.py."""
from __future__ import annotations

import sys

from test_build import Checks, rg

ENTRY_IDS = [5, 11, 47, 48, 13, 15, 17, 19, 21, 37, 28]


def main() -> int:
    c = Checks("registry")
    kinds = rg.entry_kinds()
    c.check("entry_kinds -> list of (int, str)",
            type(kinds) is list and all(type(k) is tuple and type(k[0]) is int and type(k[1]) is str
                                        for k in kinds), kinds)
    c.check("entry_kinds holds QUEUE_CONSUMER", (13, "QUEUE_CONSUMER") in kinds, kinds)
    c.check("entry_kinds ids in table order", [i for i, _ in kinds] == ENTRY_IDS, kinds)
    names = dict(rg.kind_names())
    c.check("entry_kinds names are kind_names", all(names.get(i) == n for i, n in kinds), kinds)
    c.check("version / build_stamp agree", rg.build_stamp().startswith(rg.version() + "+p"),
            (rg.version(), rg.build_stamp()))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
