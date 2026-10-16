#!/usr/bin/env python3
"""pyo3 surface, py/src/find.rs (LD.2): `find` is the one name / qname
lookup (find_node, find_nodes_by_qname and resolve_signal are gone), and
`find` / `resolve` return the LD.8a envelope as a native dict. LD.6: every
row carries a bool `live`, read off one liveness walk per graph (PyGraph caches
it, so only the first answer prints the `[live] profile=` walk line). DD.9:
`find(..., collapse=True)` folds a FACT / DERIVED identity class (a package two
repos declare) to one row carrying `identity`; `collapse=False` is the raw rows.
Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, fixture_repo, node_ids, params, rg, stderr_of

FIND_KEYS = ["id", "qname", "name", "kind", "live", "file", "line", "match"]
IDENTITY_KEYS = ["tier", "rules", "diverged", "also_in"]
ALSO_IN_KEYS = ["repo", "qname", "file", "line"]


def manifest(repo: str, extra: str) -> str:
    return ('{"name": "%s", "dependencies": {"react": "^18.2.0", "%s": "1.0.0"}}\n'
            % (repo, extra))
RESOLVE_KEYS = ["id", "qname", "name", "kind", "score", "live", "file", "line"]


def main() -> int:
    c = Checks("find")
    for gone in ("resolve_signal", "find_node", "find_nodes_by_qname"):
        c.check(f"PyGraph.{gone} removed", not hasattr(rg.PyGraph, gone))
    c.check("find signature",
            params(rg.PyGraph.find) == [("query", None), ("top_k", 20), ("kinds", None), ("scope", None),
                                        ("collapse", True)],
            params(rg.PyGraph.find))

    with tempfile.TemporaryDirectory(prefix="glia-surface-find-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        a, err = stderr_of(lambda: g.find("helper"))
        c.check("first answer walks liveness once", err.count("[live] profile=") == 1, err[-400:])
        c.check("find live marker", "[live] annotate surface=find rows=" in err, err[-400:])
        _, err = stderr_of(lambda: g.find("main"))
        c.check("later answers reuse the cached live set", "[live] profile=" not in err, err[-400:])
        ids = node_ids(g)

        c.check("find -> dict", type(a) is dict, type(a))
        c.check("find envelope keys", list(a) == ["results", "absence"], list(a))
        rows = a["results"]
        c.check("find top row", rows and rows[0]["qname"] == "app::helper", rows[:1])
        c.check("find record keys in engine field order", rows and list(rows[0]) == FIND_KEYS,
                rows and list(rows[0]))
        c.check("find ids are ints of the graph",
                all(type(r["id"]) is int and r["id"] in ids for r in rows))
        c.check("find rows carry a bool live", rows and all(type(r["live"]) is bool for r in rows))
        c.check("helper is live (main calls it)", rows and rows[0]["live"] is True, rows[:1])
        c.check("find hit has no absence", a["absence"] is None, a["absence"])
        one = g.find("helper", top_k=1)["results"]
        c.check("top_k=1 keeps one row", len(one) == 1 and one[0] == rows[0], one)

        funcs = g.find("step", top_k=0, kinds=["function"])["results"]
        c.check("kinds filter (any case)", funcs and all(r["kind"] == "FUNCTION" for r in funcs),
                funcs[:3])
        c.check("top_k=0 keeps every match", len(funcs) == 24, len(funcs))
        c.raises("unknown kind name", ValueError, lambda: g.find("helper", kinds=["FUNCTOIN"]),
                 "FUNCTION")

        none = g.find("zzqqxx")
        c.check("no match: empty results", none["results"] == [], none["results"])
        ab = none["absence"]
        c.check("no match: absence dict", type(ab) is dict and ab.get("reason") == "no_match", ab)
        c.check("absence counts unparsed files",
                type(ab) is dict and ab.get("unparsed_files") == len(g.parse_errors), ab)

        r = g.resolve("app.py", kind="diff")
        c.check("resolve -> dict", type(r) is dict, type(r))
        c.check("resolve envelope keys", type(r) is dict and list(r) == ["results", "absence"],
                type(r) is dict and list(r))
        recs = r["results"] if type(r) is dict else []
        c.check("resolve finds app.py's nodes", len(recs) > 0, recs[:2])
        c.check("resolve record keys", recs and list(recs[0]) == RESOLVE_KEYS, recs and list(recs[0]))
        c.check("resolve ids are ints of the graph",
                all(type(x["id"]) is int and x["id"] in ids for x in recs))
        live = {x["qname"]: x["live"] for x in recs}
        c.check("resolve rows carry a bool live", recs and all(type(x["live"]) is bool for x in recs))
        c.check("resolve live flags: main / helper live, module and step_0 dead",
                (live.get("app::main"), live.get("app::helper"), live.get("app"), live.get("app::step_0"))
                == (True, True, False, False), live)
        c.check("a row outside any identity class has no identity key",
                all("identity" not in r for r in rows), rows[:1])

    # DD.9: a package both repos declare is one identity class (package_name).
    with tempfile.TemporaryDirectory(prefix="glia-surface-find-identity-") as tmp:
        root = pathlib.Path(tmp)
        for repo, extra in (("web", "left-pad"), ("admin", "typescript")):
            (root / repo).mkdir()
            (root / repo / "package.json").write_text(manifest(repo, extra))
            (root / repo / "index.js").write_text("export const x = 1;\n")
        g = rg.generate_many([str(root / "web"), str(root / "admin")])
        a, err = stderr_of(lambda: g.find("package:npm:react"))
        c.check("collapse marker", "[identity] collapse surface=find classes=" in err, err[-400:])
        rows = a["results"]
        c.check("collapse: one row per class", len(rows) == 1, rows)
        row = rows[0] if rows else {}
        c.check("collapsed row keys: FIND_KEYS + identity", list(row) == FIND_KEYS + ["identity"], list(row))
        ident = row.get("identity") or {}
        c.check("identity keys", list(ident) == IDENTITY_KEYS, ident)
        c.check("identity tier / rules", (ident.get("tier"), ident.get("rules")) == ("fact", ["package_name"]),
                ident)
        also = ident.get("also_in") or []
        c.check("also_in: the other repo", len(also) == 1 and list(also[0]) == ALSO_IN_KEYS
                and also[0]["repo"] in ("web", "admin") and also[0]["qname"] == "package:npm:react",
                also)
        raw = g.find("package:npm:react", collapse=False)["results"]
        c.check("collapse=False: one row per member", len(raw) == 2, raw)
        c.check("collapse=False rows carry no identity", all("identity" not in r for r in raw), raw)
        c.check("the collapsed row is the best-ranked member", raw and row.get("id") == raw[0]["id"],
                (row.get("id"), raw[:1]))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
