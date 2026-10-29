#!/usr/bin/env python3
"""pyo3 surface, py/src/blast.rs (LD.2, many seeds LD.5): `blast_radius`
takes one qname or a list of them and returns a native dict
`{seeds, unresolved, results, absence}`, compact (FH.8: null, empty and
default fields left out; rows and seeds cut to `top_k`, default 25 since
FH.8c, with `truncated`; `full=True` keeps every one). FH.8c: the default
answer is the overview (`areas`, `truncated.next`, slim rows naming their
seed by `seed_index`), `offset=` pages. Shared helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, node_ids, params, rg, stderr_of

TOP = ["seeds", "unresolved", "results", "files", "areas", "selection", "absence", "truncated"]
KEYS = ["id", "qname", "name", "kind", "reason", "depth", "score", "live", "file", "line", "seed"]
# FH.8c: a slim row (the default answer) writes `seed_index` for `seed` and
# leaves out a `name` that is the qname's last `::` segment.
SLIM_KEYS = ["id", "qname", "name", "kind", "reason", "depth", "score", "live", "file", "line",
             "seed", "seed_index"]
AREA_KEYS = ["area", "rows", "files", "top_file", "scope"]


def seed_of(answer: dict, row: dict) -> str:
    """A row's seed qname: `seed`, or (a slim row) `seeds[seed_index]`."""
    if "seed_index" in row:
        return answer["seeds"][row["seed_index"]]["qname"]
    return row.get("seed", "")
# FH.8: `query` equal to the qname and an empty `linked_seeds` are left out.
SEED_KEYS = ["id", "qname", "kind", "file", "line"]


def cache_marks(err: str, holder: str) -> list[str]:
    """The `index=<hit|built>` of every `[rank-cache] holder=<holder>` line (EA.7)."""
    return [w for line in err.splitlines() if line.startswith(f"[rank-cache] holder={holder} ")
            for w in line.split() if w.startswith("index=")]


def main() -> int:
    c = Checks("blast")
    with tempfile.TemporaryDirectory(prefix="glia-surface-blast-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        ids = node_ids(g)
        c.check("signature", params(rg.PyGraph.blast_radius) == [
            ("qnames", None), ("direction", "both"), ("depth", 4), ("top_k", None),
            ("live_only", False), ("scope", None), ("collapse", True), ("ranking", None),
            ("cochange", True), ("top_files", 10), ("full", False), ("offset", 0)],
                params(rg.PyGraph.blast_radius))

        a = g.blast_radius("app::helper")
        c.check("blast_radius -> dict", type(a) is dict, type(a))
        a = a if type(a) is dict else {}
        c.check("answer keys in engine field order", list(a) == [k for k in TOP if k in a]
                and {"seeds", "results", "selection"} <= set(a), list(a))
        b = a.get("results") or []
        c.check("blast_radius reaches the callers", any(r.get("qname") == "app::main" for r in b),
                [r.get("qname") for r in b][:5])
        c.check("slim record keys in engine field order", b and all(
            list(r) == [k for k in SLIM_KEYS if k in r] and ("seed" in r) != ("seed_index" in r)
            for r in b), b and list(b[0]))
        c.check("a slim row leaves out a name that repeats the qname's tail",
                all(r.get("name") != r["qname"].split("::")[-1] for r in b), b[:2])
        c.check("ids are ints of the graph", all(type(r["id"]) is int and r["id"] in ids for r in b))
        c.check("score is a float", all(type(r["score"]) is float for r in b))
        c.check("line is an int (1-based) or None",
                all(r["line"] is None or (type(r["line"]) is int and r["line"] >= 1) for r in b))
        c.check("one seed owns every row", all(seed_of(a, r) == "app::helper" for r in b))
        areas = a.get("areas") or []
        c.check("the overview maps its rows by area", areas
                and all(list(x) == [k for k in AREA_KEYS if k in x] for x in areas)
                and sum(x["rows"] for x in areas) == len(b) + (a.get("truncated") or {}).get("results", 0),
                areas)
        seeds = a.get("seeds") or []
        c.check("seed keys in engine field order", seeds and list(seeds[0]) == SEED_KEYS,
                seeds and list(seeds[0]))
        c.check("the seed is located", seeds and seeds[0]["line"] == 4 and seeds[0]["id"] in ids, seeds)
        c.check("a found answer: no absence, nothing unresolved (both left out)",
                "absence" not in a and "unresolved" not in a, list(a))
        c.check("a str is a one-item list", g.blast_radius(["app::helper"]) == a)

        # Two seeds, backward: main calls helper, so helper's one-hop
        # upstream seed is main, and main is never a row.
        (m, err) = stderr_of(lambda: g.blast_radius(["app::helper", "app::main"], direction="backward"))
        c.check("seeds in query order", [s["qname"] for s in m["seeds"]] == ["app::helper", "app::main"],
                m["seeds"])
        c.check("linked_seeds", m["seeds"][0]["linked_seeds"] == ["app::main"]
                and "linked_seeds" not in m["seeds"][1], m["seeds"])
        c.check("a seed is never a row", all(r["qname"] not in ("app::helper", "app::main")
                                             for r in m["results"]))
        c.check("rows attributed to their seed", m["results"]
                and all(seed_of(m, r) == "app::helper" for r in m["results"]))
        c.check("[blast] marker", "[blast] seeds=2 unresolved=0 reached=24 linked_seeds=1 walk=Backward" in err,
                err)

        u = g.blast_radius("no::such::thing")
        c.check("unknown qname is an absence, not an error",
                u["results"] == [] and u["unresolved"] == ["no::such::thing"]
                and (u["absence"] or {}).get("reason") == "unknown_symbol", u)
        mixed = g.blast_radius(["app::helper", "nope"])
        c.check("mixed: the rest still answer", mixed["unresolved"] == ["nope"] and mixed["results"]
                and "absence" not in mixed, mixed["unresolved"])
        c.check("a query that is not the qname is kept",
                g.blast_radius("helper")["seeds"][0].get("query") == "helper")

        # FH.8: the row cap. top_k cuts rows and seeds after the ranking and
        # `truncated` counts the cut; full=True keeps every row and refuses a
        # top_k; every call prints its size.
        (whole, err) = stderr_of(lambda: g.blast_radius("app::helper", full=True))
        n = len(whole["results"])
        c.check("full keeps every row, nothing truncated, no areas",
                n == len(b) + (a.get("truncated") or {}).get("results", 0)
                and "truncated" not in whole and "areas" not in whole, (n, len(b), list(whole)))
        c.check("full rows are whole", whole["results"] and all(list(r) == [k for k in KEYS if k in r]
                                                                and "seed" in r and "name" in r
                                                                for r in whole["results"]),
                whole["results"][:1])
        c.check("[answer-size] marker",
                f"[answer-size] surface=blast_radius rows={n} shown={n} offset=0 areas=0 bytes=" in err,
                err[-400:])
        one = g.blast_radius("app::helper", top_k=1)
        c.check("top_k=1 keeps the first row", [r["qname"] for r in one["results"]]
                == [r["qname"] for r in whole["results"][:1]], one["results"])
        t1 = one.get("truncated") or {}
        c.check("truncated counts the cut and names the next page",
                (t1.get("results"), t1.get("seeds"), (t1.get("next") or {}).get("offset")) == (n - 1, 0, 1),
                t1)
        nxt = g.blast_radius("app::helper", top_k=1, offset=1)
        c.check("offset=1 is the second row", [r["id"] for r in nxt["results"]]
                == [r["id"] for r in whole["results"][1:2]], nxt["results"])
        c.check("truncated.offset counts the skipped rows", (nxt.get("truncated") or {}).get("offset") == 1,
                nxt.get("truncated"))
        c.raises("offset with full", ValueError, lambda: g.blast_radius("app::helper", offset=1, full=True),
                 "full keeps every row")
        two = g.blast_radius(["app::helper", "app::main"], top_k=1)
        c.check("seeds cap at top_k", len(two["seeds"]) == 1 and two["truncated"]["seeds"] == 1,
                two.get("truncated"))
        c.raises("top_k with full", ValueError, lambda: g.blast_radius("app::helper", top_k=3, full=True),
                 "full keeps every row")
        c.raises("qnames of the wrong type", TypeError, lambda: g.blast_radius(5), "str or a list of str")
        c.raises("bad direction", ValueError, lambda: g.blast_radius("app::helper", "sideways"),
                 "forward|backward|both")

        # EA.7: the ranking keywords, last; the PyGraph holds its ranking
        # indexes (built by the first call above), so a repeated call reads
        # every index warm and answers identically.
        c.raises("bad ranking", ValueError, lambda: g.blast_radius("app::helper", ranking="x"),
                 "unknown ranking `x`: expected one of ppr, ppr-wide, bfs")
        (first, err1) = stderr_of(lambda: g.blast_radius("app::helper", top_k=50))
        (again, err2) = stderr_of(lambda: g.blast_radius("app::helper", top_k=50))
        c.check("a warm index answers identically", first == again)
        c.check("[rank-cache] holder=pygraph .. index=hit on the second call",
                cache_marks(err2, "pygraph") and set(cache_marks(err2, "pygraph")) == {"index=hit"},
                (cache_marks(err1, "pygraph"), cache_marks(err2, "pygraph")))
        c.check("top_files cuts files", len(g.blast_radius("app::helper", top_files=1).get("files", [])) <= 1)
        # EC.8: ranking=None applies the repo's calibration (none here: the
        # fallback); a given ranking is explicit.
        sel = first.get("selection") or {}
        c.check("ranking=None: SELh0's fallback", (sel.get("set"), sel.get("pick"), sel.get("source"))
                == ("SELh0", "H10", "fallback"), sel)
        exp = g.blast_radius("app::helper", ranking="ppr").get("selection") or {}
        c.check("a given ranking is explicit", exp.get("source") == "explicit", exp)
        c.check("no seed: no selection", "selection" not in u, list(u))
        for ranking in ("ppr-wide", "bfs"):
            r = g.blast_radius("app::helper", ranking=ranking, cochange=False)
            c.check(f"ranking={ranking} keeps the closure",
                    sorted(x["qname"] for x in r["results"]) == sorted(x["qname"] for x in first["results"]))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
