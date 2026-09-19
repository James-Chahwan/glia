#!/usr/bin/env python3
"""pyo3 surface, py/src/implementors.rs (LD.7c): `implementors(qname,
direction="down", transitive=True)` answers who implements or extends a type
(or what it implements with direction="up") with the LD.8a envelope
{results, absence} as a native dict, each row tiered FACT / DERIVED /
HEURISTIC; an unknown direction raises ValueError. Shared helpers:
test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, node_ids, params, rg, stderr_of

REPOS_CS = (
    "namespace Shop {\n    public interface IRepo { string Get(string id); }\n"
    "    public interface IUserRepo : IRepo { string ByEmail(string email); }\n"
    "    public class PgUserRepo : IUserRepo {\n"
    "        public string Get(string id) { return id; }\n"
    "        public string ByEmail(string email) { return email; }\n    }\n}\n"
)

ROW_KEYS = ["id", "qname", "name", "kind", "file", "line", "live", "relation", "depth", "via", "tier"]


def main() -> int:
    c = Checks("implementors")
    c.check("implementors signature",
            params(rg.PyGraph.implementors) == [("qname", None), ("direction", "down"), ("transitive", True)],
            params(rg.PyGraph.implementors))
    with tempfile.TemporaryDirectory(prefix="glia-surface-implementors-") as tmp:
        root = pathlib.Path(tmp) / "shop"
        root.mkdir()
        (root / "Repos.cs").write_text(REPOS_CS)
        g = rg.generate(str(root))

        a, err = stderr_of(lambda: g.implementors("Shop::IRepo"))
        c.check("marker",
                "[implementors] target=Shop::IRepo direction=Down transitive=true found=2 fact=2 derived=0 heuristic=0"
                in err, err[-400:])
        c.check("implementors -> dict", type(a) is dict, type(a))
        c.check("envelope keys", type(a) is dict and list(a) == ["results", "absence"],
                type(a) is dict and list(a))
        rows = a["results"] if type(a) is dict else []
        c.check("row keys in engine field order", rows and list(rows[0]) == ROW_KEYS, rows and list(rows[0]))
        c.check("the chain, in discovery order",
                [(r["qname"], r["depth"], r["relation"], r["via"]) for r in rows]
                == [("Shop::IUserRepo", 1, "IMPLEMENTS", None),
                    ("Shop::PgUserRepo", 2, "IMPLEMENTS", "Shop::IUserRepo")], rows)
        c.check("declared heritage is FACT", rows and rows[0]["tier"] == "FACT", rows[:1])
        ids = node_ids(g)
        c.check("ids are ints of the graph, live a bool",
                all(type(r["id"]) is int and r["id"] in ids and type(r["live"]) is bool for r in rows), rows)
        c.check("rows located, 1-based",
                [(r["file"], r["line"]) for r in rows] == [("Repos.cs", 3), ("Repos.cs", 4)], rows)
        c.check("no absence", type(a) is dict and a.get("absence", 1) is None, a)

        direct = g.implementors("Shop::IRepo", transitive=False)["results"]
        c.check("transitive=False keeps the direct level", [r["qname"] for r in direct] == ["Shop::IUserRepo"],
                direct)
        up = g.implementors("Shop::PgUserRepo", direction="up")["results"]
        c.check("direction='up' lists the supertypes",
                [r["qname"] for r in up] == ["Shop::IUserRepo", "Shop::IRepo"], up)
        meth = g.implementors("Shop::IRepo::Get")["results"]
        c.check("a method target lists its implementations through the hierarchy",
                [(r["qname"], r["kind"]) for r in meth] == [("Shop::PgUserRepo::Get", "METHOD")], meth)

        none = g.implementors("Shop::PgUserRepo")
        ab = none["absence"]
        c.check("nothing extends it: empty results", none["results"] == [], none["results"])
        c.check("no_edges FACT with the heritage mechanisms",
                type(ab) is dict and (ab.get("tier"), ab.get("reason"), ab.get("mechanisms"))
                == ("FACT", "no_edges", ["IMPLEMENTS", "INHERITS_FROM"]), ab)
        c.check("absence counts unparsed files",
                type(ab) is dict and ab.get("unparsed_files") == len(g.parse_errors), ab)
        unknown = g.implementors("IUsrRepo")["absence"]
        c.check("a typo is unknown_symbol with suggestions",
                type(unknown) is dict and unknown.get("reason") == "unknown_symbol"
                and "Shop::IUserRepo" in unknown.get("suggestions", []), unknown)
        c.raises("unknown direction", ValueError, lambda: g.implementors("Shop::IRepo", direction="sideways"),
                 "down, up")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
