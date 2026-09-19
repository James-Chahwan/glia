#!/usr/bin/env python3
"""pyo3 surface, py/src/layout.rs (LD.2): with `generate` no longer
persisting, the load path is generate -> save_to_default -> load_from_gmap,
and the loaded graph answers like the fresh one. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import sys
import tempfile

from test_build import Checks, fixture_repo, params, rg


def main() -> int:
    c = Checks("layout")
    c.check("load_from_gmap signature (LC.8)",
            params(rg.load_from_gmap) == [("dir", None), ("repo_path", None), ("rebuild", True)],
            params(rg.load_from_gmap))
    with tempfile.TemporaryDirectory(prefix="glia-surface-layout-") as tmp:
        repo = fixture_repo(tmp)
        g = rg.generate(repo)
        d = rg.default_gmap_dir(repo)
        c.check("generate wrote no layout", not os.path.exists(d))
        c.check("no layout: stale", rg.is_stale(d, repo) is True)
        g.save_to_default(repo)
        c.check("saved: fresh", rg.is_stale(d, repo) is False)
        loaded = rg.load_from_gmap(d)
        c.check("loaded is a PyGraph", type(loaded) is rg.PyGraph, type(loaded))
        c.check("same node count", loaded.node_count() == g.node_count())
        c.check("same find answer", loaded.find("helper") == g.find("helper"))
        c.check("same blast answer", loaded.blast_radius("app::helper") == g.blast_radius("app::helper"))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
