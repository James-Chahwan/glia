#!/usr/bin/env python3
"""pyo3 surface, py/src/why.rs (LE.5): `why(from_qname, to_qname,
category=None)` answers every edge from one node to another with its emitter,
rule, call site and confidence, tiered fact / derived / heuristic, as a native
dict; with no direct edge the witness path and the LD.8a absence. An unknown
node or category raises ValueError. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, node_ids, params, rg, stderr_of

SHOP = (
    "def price(o):\n    return o\n\n\n"
    "def place(o):\n    return price(o)\n\n\n"
    "def checkout(o):\n    return place(o)\n"
)

ANSWER_KEYS = ["found", "edges", "path", "from_nodes", "to_nodes", "note", "absence"]
ROW_KEYS = ["from_id", "from_qname", "to_id", "to_qname", "category", "confidence", "tier",
            "emitter", "rule", "basis", "site", "cross_repo", "note"]


def main() -> int:
    c = Checks("why")
    c.check("why signature",
            params(rg.PyGraph.why) == [("from_qname", None), ("to_qname", None), ("category", None)],
            params(rg.PyGraph.why))
    with tempfile.TemporaryDirectory(prefix="glia-surface-why-") as tmp:
        root = pathlib.Path(tmp) / "repo"
        (root / "shop").mkdir(parents=True)
        (root / "shop" / "a.py").write_text(SHOP)
        g = rg.generate(str(root))

        a, err = stderr_of(lambda: g.why("shop::a::place", "shop::a::price"))
        c.check("marker",
                "[why] edges=1 path_hops=0 found=true tiers fact=1 derived=0 heuristic=0" in err, err[-400:])
        c.check("why -> dict", type(a) is dict, type(a))
        c.check("answer keys in engine field order", type(a) is dict and list(a) == ANSWER_KEYS,
                type(a) is dict and list(a))
        rows = a["edges"] if type(a) is dict else []
        c.check("one CALLS row", [r["category"] for r in rows] == ["CALLS"], rows)
        c.check("row keys in engine field order", rows and list(rows[0]) == ROW_KEYS, rows and list(rows[0]))
        r = rows[0] if rows else {}
        c.check("a resolved call is a fact", r.get("tier") == "fact" and r.get("confidence") == "strong", r)
        c.check("emitter names the stage",
                str(r.get("emitter", "")).startswith(("graph:", "parser:")) and r.get("basis") == "site", r)
        c.check("site is the call, 1-based", r.get("site") == {"file": "shop/a.py", "line": 6}, r)
        ids = node_ids(g)
        c.check("ids are ints of the graph",
                type(r.get("from_id")) is int and r.get("from_id") in ids and r.get("to_id") in ids, r)
        c.check("found, no path, no absence",
                a.get("found") is True and a.get("path") == [] and a.get("absence") is None, a)
        c.check("sides resolved", a.get("from_nodes") == ["shop::a::place"]
                and a.get("to_nodes") == ["shop::a::price"], a)

        dotted = g.why("shop.a.place", "shop.a.price", category="calls")
        c.check("dotted path and any-case category", len(dotted["edges"]) == 1, dotted)

        miss = g.why("shop::a::checkout", "shop::a::price")
        c.check("no direct edge: found False", miss["found"] is False and miss["edges"] == [], miss)
        c.check("witness path of 2 explained hops",
                [(h["from_qname"], h["to_qname"], h["category"], h["site"]["line"]) for h in miss["path"]]
                == [("shop::a::checkout", "shop::a::place", "CALLS", 10),
                    ("shop::a::place", "shop::a::price", "CALLS", 6)], miss["path"])
        ab = miss["absence"]
        c.check("no_edges FACT absence",
                type(ab) is dict and (ab.get("tier"), ab.get("reason")) == ("FACT", "no_edges"), ab)
        c.check("absence counts unparsed files",
                type(ab) is dict and ab.get("unparsed_files") == len(g.parse_errors), ab)

        c.raises("unknown node", ValueError, lambda: g.why("shop::a::nope", "shop::a::price"),
                 "no node with qname/name `shop::a::nope`")
        c.raises("unknown category", ValueError,
                 lambda: g.why("shop::a::place", "shop::a::price", category="CALS"),
                 "unknown edge category `CALS`")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
