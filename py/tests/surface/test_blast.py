#!/usr/bin/env python3
"""pyo3 surface, py/src/blast.rs (LD.2, many seeds LD.5): `blast_radius`
takes one qname or a list of them and returns a native dict
`{seeds, unresolved, results, absence}`. Shared helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, node_ids, params, rg, stderr_of

TOP = ["seeds", "unresolved", "results", "absence"]
KEYS = ["id", "qname", "name", "kind", "reason", "depth", "score", "live", "file", "line", "seed"]
SEED_KEYS = ["query", "id", "qname", "kind", "file", "line", "linked_seeds"]


def main() -> int:
    c = Checks("blast")
    with tempfile.TemporaryDirectory(prefix="glia-surface-blast-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        ids = node_ids(g)
        c.check("signature", params(rg.PyGraph.blast_radius) == [
            ("qnames", None), ("direction", "both"), ("depth", 4), ("top_k", None),
            ("live_only", False), ("scope", None)], params(rg.PyGraph.blast_radius))

        a = g.blast_radius("app::helper")
        c.check("blast_radius -> dict", type(a) is dict, type(a))
        a = a if type(a) is dict else {}
        c.check("answer keys in engine field order", list(a) == TOP, list(a))
        b = a.get("results") or []
        c.check("blast_radius reaches the callers", any(r.get("qname") == "app::main" for r in b),
                [r.get("qname") for r in b][:5])
        c.check("record keys in engine field order", b and list(b[0]) == KEYS, b and list(b[0]))
        c.check("ids are ints of the graph", all(type(r["id"]) is int and r["id"] in ids for r in b))
        c.check("score is a float", all(type(r["score"]) is float for r in b))
        c.check("line is an int (1-based) or None",
                all(r["line"] is None or (type(r["line"]) is int and r["line"] >= 1) for r in b))
        c.check("one seed owns every row", all(r["seed"] == "app::helper" for r in b))
        seeds = a.get("seeds") or []
        c.check("seed keys in engine field order", seeds and list(seeds[0]) == SEED_KEYS,
                seeds and list(seeds[0]))
        c.check("the seed is located", seeds and seeds[0]["line"] == 4 and seeds[0]["id"] in ids, seeds)
        c.check("a found answer: no absence, nothing unresolved",
                a.get("absence") is None and a.get("unresolved") == [], (a.get("absence"), a.get("unresolved")))
        c.check("a str is a one-item list", g.blast_radius(["app::helper"]) == a)

        # Two seeds, backward: main calls helper, so helper's one-hop
        # upstream seed is main, and main is never a row.
        (m, err) = stderr_of(lambda: g.blast_radius(["app::helper", "app::main"], direction="backward"))
        c.check("seeds in query order", [s["qname"] for s in m["seeds"]] == ["app::helper", "app::main"],
                m["seeds"])
        c.check("linked_seeds", m["seeds"][0]["linked_seeds"] == ["app::main"]
                and m["seeds"][1]["linked_seeds"] == [], m["seeds"])
        c.check("a seed is never a row", all(r["qname"] not in ("app::helper", "app::main")
                                             for r in m["results"]))
        c.check("rows attributed to their seed", m["results"]
                and all(r["seed"] == "app::helper" for r in m["results"]))
        c.check("[blast] marker", "[blast] seeds=2 unresolved=0 reached=24 linked_seeds=1 walk=Backward" in err,
                err)

        u = g.blast_radius("no::such::thing")
        c.check("unknown qname is an absence, not an error",
                u["results"] == [] and u["unresolved"] == ["no::such::thing"]
                and (u["absence"] or {}).get("reason") == "unknown_symbol", u)
        mixed = g.blast_radius(["app::helper", "nope"])
        c.check("mixed: the rest still answer", mixed["unresolved"] == ["nope"] and mixed["results"]
                and mixed["absence"] is None, mixed["unresolved"])
        c.raises("qnames of the wrong type", TypeError, lambda: g.blast_radius(5), "str or a list of str")
        c.raises("bad direction", ValueError, lambda: g.blast_radius("app::helper", "sideways"),
                 "forward|backward|both")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
