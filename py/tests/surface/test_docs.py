#!/usr/bin/env python3
"""pyo3 surface, py/src/docs.rs (LD.2): `governing_docs` returns the LD.8a
envelope as a native dict. Shared helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, rg


def main() -> int:
    c = Checks("docs")
    with tempfile.TemporaryDirectory(prefix="glia-surface-docs-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        d = g.governing_docs("app::helper")
        c.check("governing_docs -> dict", type(d) is dict, type(d))
        c.check("envelope keys", type(d) is dict and list(d) == ["results", "absence"],
                type(d) is dict and list(d))
        # The fixture has no docs: the answer is an absence, not an error.
        ab = d.get("absence") if type(d) is dict else None
        c.check("no docs: absence dict", type(ab) is dict and ab.get("reason") == "no_edges", ab)
        c.check("absence counts unparsed files",
                type(ab) is dict and ab.get("unparsed_files") == len(g.parse_errors), ab)
        u = g.governing_docs("no::such::thing")
        uab = u.get("absence") if type(u) is dict else None
        c.check("unknown symbol is an absence",
                type(uab) is dict and uab.get("reason") == "unknown_symbol", uab)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
