#!/usr/bin/env python3
"""pyo3 surface, py/src/cells.rs (LF.1b): the cell write API. A write lands in
the repo's `.glia` sidecar, in its default layout while that is fresh, and in
the live PyGraph, so a fresh generate() and a warm load_from_gmap() both show
it. Also `PyGraph.save_to(dir, repo_path=...)` (py/src/graph.rs). Shared
helpers: test_build.py."""
from __future__ import annotations

import json
import os
import sys
import tempfile

from test_build import Checks, fixture_repo, params, rg, stderr_of

CONV_ID = next(i for i, n in rg.cell_type_names() if n == "CONV")
DECISION_ID = next(i for i, n in rg.cell_type_names() if n == "DECISION")


def node_id(g, qname: str) -> int:
    return next(n["id"] for n in json.loads(g.nodes_json()) if n["qname"] == qname)


def cell(g, qname: str, type_id: int) -> str | None:
    return next((p for t, p in g.node_cells(node_id(g, qname)) if t == type_id), None)


def main() -> int:
    c = Checks("cells")
    c.check("write_cell signature",
            [p for p, _ in params(rg.write_cell)]
            == ["repo_path", "qname", "cell_type", "payload", "kind", "model", "dims"],
            params(rg.write_cell))
    c.check("remove_cell default source",
            params(rg.remove_cell)[-1] == ("source", "api"), params(rg.remove_cell))
    c.check("save_to takes repo_path",
            params(rg.PyGraph.save_to) == [("dir", None), ("repo_path", None)],
            params(rg.PyGraph.save_to))
    with tempfile.TemporaryDirectory(prefix="glia-surface-cells-") as tmp:
        repo = fixture_repo(tmp)
        g = rg.generate(repo)
        d = rg.default_gmap_dir(repo)
        g.save_to_default(repo)

        out, err = stderr_of(lambda: g.set_cell("app::helper", "CONV", '{"text": "x"}'))
        c.check("set_cell returns a dict", type(out) is dict, type(out))
        c.check("set_cell: first CONV id", out.get("entry_id") == "000001", out)
        c.check("set_cell: written through", out.get("write_through") == "applied", out)
        c.check("set_cell: bound", out.get("target") == "bound", out)
        c.check("set_cell marker",
                "[cells] write qname=app::helper cell=CONV id=000001 rows=1 target=bound "
                "write_through=applied surface=pyo3" in err, err)
        c.check("sidecar written", os.path.isfile(os.path.join(repo, ".glia", "cells.jsonl")))
        c.check("layout stays fresh", rg.is_stale(d, repo) is False)
        live = cell(g, "app::helper", CONV_ID)
        c.check("live graph carries the CONV", live is not None and '"text":"x"' in live, live)
        c.check("a fresh generate() shows the CONV", cell(rg.generate(repo), "app::helper", CONV_ID) == live)
        c.check("a warm load shows the CONV", cell(rg.load_from_gmap(d), "app::helper", CONV_ID) == live)

        vec = b"\x00\x00\x80\x3f"
        out = g.set_cell("app::helper", "VECTOR", vec, dims=1)
        c.check("vector write", out.get("cell") == "VECTOR" and out.get("entry_id") is None, out)
        c.check("node_cell_bytes", g.node_cell_bytes(node_id(g, "app::helper"), "VECTOR") == vec)
        c.check("node_cell_bytes: none", g.node_cell_bytes(node_id(g, "app::helper"), "DECISION") is None)

        c.check("remove_cell", g.remove_cell("app::helper", "CONV", "000001") is True)
        c.check("removed from the live graph", cell(g, "app::helper", CONV_ID) is None)
        c.check("remove_cell again", g.remove_cell("app::helper", "CONV", "000001") is False)

        out = rg.write_cell(repo, "app::main", "DECISION", '{"id": "d1", "title": "Keep it"}')
        c.check("module write_cell", out.get("entry_id") == "d1", out)
        c.check("module write reaches the next build", cell(rg.generate(repo), "app::main", DECISION_ID) is not None)
        c.check("module remove_cell", rg.remove_cell(repo, "app::main", "DECISION", "d1") is True)

        c.raises("CODE is not writable", ValueError,
                 lambda: g.set_cell("app::helper", "CODE", "{}"), "WRITABLE")
        c.raises("payload type", TypeError, lambda: g.set_cell("app::helper", "CONV", 7), "payload")

        out_dir = os.path.join(tmp, "custom")
        g.save_to(out_dir, repo_path=repo)
        c.check("save_to(repo_path) is fresh", rg.is_stale(out_dir, repo) is False)
        with open(os.path.join(repo, ".glia", "cells.jsonl"), "a") as f:
            f.write('{"qname":"app::main","cell":"CONV","entry":{"source":"api","id":"9","text":"hand"}}\n')
        c.check("save_to(repo_path) sees a sidecar edit", rg.is_stale(out_dir, repo) is True)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
