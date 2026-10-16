#!/usr/bin/env python3
"""pyo3 surface, py/src/arch.rs (LD.2): `coverage` and `project_roots`
return native lists of dicts, `service_map` a native dict. DD.14:
`service_map(collapse=True)` reads a SAME_AS class as one link per service
pair (three repos declaring react: three links); `collapse=False` is the raw
member -> center star (two links). Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, fixture_repo, params, rg, stderr_of


def same_as_links(sm) -> list[tuple[str, str, str, int, str]]:
    return [(l["from"], l["to"], l["channel"], l["count"], l["confidence"])
            for l in sm["links"] if l["mechanism"] == "SAME_AS"]


def main() -> int:
    c = Checks("arch")
    c.check("service_map signature", params(rg.PyGraph.service_map) == [("collapse", True)],
            params(rg.PyGraph.service_map))
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
        c.check("collapse=False on a class-free graph is the same answer",
                g.service_map(collapse=False) == sm)

    # DD.14: one package_name class of three repos, stored as a two-edge star.
    with tempfile.TemporaryDirectory(prefix="glia-surface-arch-identity-") as tmp:
        root = pathlib.Path(tmp)
        for repo in ("alpha", "beta", "gamma"):
            (root / repo).mkdir()
            (root / repo / "package.json").write_text(
                '{"name": "%s", "dependencies": {"react": "^18.2.0"}}\n' % repo)
            (root / repo / "index.js").write_text(
                'export function %sMain() {\n  return "%s";\n}\n' % (repo, repo))
        g = rg.generate_many([str(root / r) for r in ("alpha", "beta", "gamma")])
        sm, err = stderr_of(lambda: g.service_map())
        c.check("collapse marker", "[identity] collapse surface=arch classes=1 " in err, err[-400:])
        c.check("collapse: one SAME_AS link per service pair",
                same_as_links(sm) == [("alpha", "beta", "react", 1, "strong"),
                                      ("alpha", "gamma", "react", 1, "strong"),
                                      ("beta", "gamma", "react", 1, "strong")],
                same_as_links(sm))
        raw = g.service_map(collapse=False)
        c.check("collapse=False: the raw star, two links", len(same_as_links(raw)) == 2, same_as_links(raw))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
