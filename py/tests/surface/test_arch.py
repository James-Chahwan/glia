#!/usr/bin/env python3
"""pyo3 surface, py/src/arch.rs (LD.2): `coverage` and `project_roots`
return native lists of dicts, `service_map` a native dict.
Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, fixture_repo, rg


def main() -> int:
    c = Checks("arch")
    with tempfile.TemporaryDirectory(prefix="glia-surface-arch-") as tmp:
        repo = fixture_repo(tmp)
        (pathlib.Path(repo) / "pyproject.toml").write_text('[project]\nname = "ld2app"\n')
        g = rg.generate(repo)

        cov = g.coverage()
        c.check("coverage -> list", type(cov) is list, type(cov))
        cov = cov if type(cov) is list else []
        c.check("coverage notes are dicts in field order",
                all(list(n) == ["language", "edge_category", "note", "verify", "edges_found"] for n in cov),
                cov[:1])
        c.check("edges_found is an int", all(type(n["edges_found"]) is int for n in cov))

        roots = g.project_roots()
        c.check("project_roots -> list", type(roots) is list, type(roots))
        roots = roots if type(roots) is list else []
        c.check("the pyproject root is listed", any(r.get("path") == "." for r in roots), roots)
        c.check("project records in field order",
                all(list(r) == ["qname", "label", "ecosystem", "manifest", "path"] for r in roots), roots[:1])

        sm = g.service_map()
        c.check("service_map -> dict", type(sm) is dict, type(sm))
        c.check("service_map keys in field order",
                type(sm) is dict and list(sm) == ["keying", "services", "links", "self_links", "unlocated_nodes"],
                type(sm) is dict and list(sm))
        c.check("services / links are lists",
                type(sm) is dict and type(sm["services"]) is list and type(sm["links"]) is list)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
