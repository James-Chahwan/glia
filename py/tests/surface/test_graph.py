#!/usr/bin/env python3
"""pyo3 surface, py/src/graph.rs (LD.2): the pair-shaped accessors stay
tuples, and `save_to` / `save_to_default` are the only layout writers. A
dropped PyGraph hands its freed memory back on Linux glibc (DE.10).
Shared helpers: test_build.py."""
from __future__ import annotations

import ctypes
import json
import os
import pathlib
import platform
import re
import subprocess
import sys
import tempfile

from test_build import Checks, fixture_repo, rg, stderr_of, tree

# The drop checks expect the trim on; a caller's opt-out would hide it.
os.environ.pop("GLIA_MALLOC_TRIM", None)
GLIBC = sys.platform == "linux" and platform.libc_ver()[0] == "glibc"
TRIM = re.compile(r"\[memory\] malloc_trim site=pygraph_drop rss_mb=(\d+)->(\d+)")
DROP_ONE = "import sys, glia_py\ng = glia_py.generate(sys.argv[1])\ndel g\n"


def big_repo(tmp: str) -> str:
    """40 python modules of 150 calling functions: a graph of several MB."""
    root = pathlib.Path(tmp) / "ld2big"
    root.mkdir()
    for m in range(40):
        body = "".join(f"\n\ndef f{m}_{i}(v):\n    \"\"\"Step {i} of module {m}.\"\"\"\n"
                       f"    return f{m}_{max(i - 1, 0)}(v) + {i}\n" for i in range(150))
        (root / f"mod_{m}.py").write_text(body)
    return str(root)


def drop_checks(c: Checks, tmp: str) -> None:
    repo = fixture_repo(tmp, "ld2drop")
    _, err = stderr_of(lambda: (rg.generate(repo), None)[1])
    if not GLIBC:
        c.check("no trim off Linux glibc", "[memory]" not in err, err[-300:])
        return
    c.check("a dropped graph trims (marker)", TRIM.search(err) is not None, err[-300:])
    # Drop order: the guard is PyGraph's last field, so the trim sees the
    # graph already freed. Trim the build's own garbage first, then drop.
    held = [rg.generate(big_repo(tmp))]
    ctypes.CDLL(None).malloc_trim(0)
    _, err = stderr_of(held.clear)
    m = TRIM.search(err)
    c.check("the trim runs after the graph is freed (rss_after < rss_before)",
            m is not None and int(m[2]) < int(m[1]), err[-300:])
    for value, want in ((None, True), ("0", False)):
        env = {k: v for k, v in os.environ.items() if k != "GLIA_MALLOC_TRIM"}
        if value is not None:
            env["GLIA_MALLOC_TRIM"] = value
        p = subprocess.run([sys.executable, "-c", DROP_ONE, repo], env=env, capture_output=True, text=True)
        c.check(f"GLIA_MALLOC_TRIM={value}: marker {'printed' if want else 'absent'}",
                p.returncode == 0 and (TRIM.search(p.stderr) is not None) == want, p.stderr[-300:])


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
    with tempfile.TemporaryDirectory(prefix="glia-surface-graph-drop-") as tmp:
        drop_checks(c, tmp)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
