#!/usr/bin/env python3
"""pyo3 surface, py/src/pack.rs (CC.4c): `pack(query, budget=8000,
bytes_per_token=3.7, seeds=5, candidates=200, preset=None, scope=None)` and
`pack_ids(node_ids, budget=8000, bytes_per_token=3.7, candidates=200,
preset=None, scope=None)` return the whole context pack `{query, text,
budget_tokens, used_tokens, bytes, bytes_per_token, candidates, nodes,
dropped, rerenders, absence}` as a native dict, keys in engine field order;
`bytes_per_token` outside 1.0..=20.0 raises ValueError. The fixture is CC.4b's
acceptance tree (engine/tests/pack.rs): `shop/a.py` `price` and `place`
(which calls it), `shop/b.py` `checkout` (which calls `place`), an unrelated
`shop/util.py`. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

PACK_KEYS = ["query", "text", "budget_tokens", "used_tokens", "bytes", "bytes_per_token",
             "candidates", "nodes", "dropped", "rerenders", "absence"]
NODE_KEYS = ["id", "qname", "kind", "file", "line", "fidelity", "tokens", "rank", "tier",
             "reason", "matched"]

A_PY = '''def price(o):
    """Price an order.

    Sums the line totals, then takes the discount off.
    """
    total = 0
    for line in o.lines:
        total = total + line.qty * line.unit
    if o.discount:
        total = total - o.discount
    total = round(total, 2)
    return total


def place(o):
    return price(o)
'''

B_PY = '''from shop.a import place


def checkout(o):
    return place(o)
'''

UTIL_PY = '''def format_report():
    rows = []
    rows.append("report")
    return "\\n".join(rows)
'''


def fixture(top: pathlib.Path) -> None:
    files = {
        "shop/a.py": A_PY,
        "shop/b.py": B_PY,
        "shop/util.py": UTIL_PY,
        "pyproject.toml": '[project]\nname = "shop"\nversion = "0.1.0"\n',
    }
    for rel, src in files.items():
        path = top / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(src)


def main() -> int:
    c = Checks("pack")
    c.check("pack signature",
            params(rg.PyGraph.pack)
            == [("query", None), ("budget", 8000), ("bytes_per_token", 3.7), ("seeds", 5),
                ("candidates", 200), ("preset", None), ("scope", None)],
            params(rg.PyGraph.pack))
    c.check("pack_ids signature",
            params(rg.PyGraph.pack_ids)
            == [("node_ids", None), ("budget", 8000), ("bytes_per_token", 3.7),
                ("candidates", 200), ("preset", None), ("scope", None)],
            params(rg.PyGraph.pack_ids))
    with tempfile.TemporaryDirectory(prefix="glia-surface-pack-") as tmp:
        top = pathlib.Path(tmp) / "shopapp"
        fixture(top)
        g = rg.generate(str(top))

        p, err = stderr_of(lambda: g.pack("price", budget=100000))
        c.check("marker", "[pack] query=price seeds=1 candidates=" in err
                and "/100000 bytes=" in err and " bpt=3.7 rerenders=0" in err, err[-400:])
        c.check("pack -> dict in engine field order", type(p) is dict and list(p) == PACK_KEYS, p)
        nodes = p.get("nodes", [])
        first = nodes[0] if nodes else {}
        c.check("node keys in engine field order", list(first) == NODE_KEYS, list(first))
        c.check("nodes[0] is the seed, full",
                (first.get("qname"), first.get("fidelity"), first.get("reason"), first.get("tier"),
                 first.get("matched")) == ("shop::a::price", "full", "seed", "fact", "exact_name"),
                first)
        c.check("seed located 1-based", (first.get("kind"), first.get("file"), first.get("line"))
                == ("FUNCTION", "shop/a.py", 1), first)
        c.check("id is an int", isinstance(first.get("id"), int), first.get("id"))
        text = p.get("text", "")
        c.check("text is the pack", text.startswith("# context for price\n")
                and "### shop::a::price (FUNCTION shop/a.py:1-" in text, text[:200])
        c.check("counts", (p.get("budget_tokens"), p.get("bytes"), p.get("bytes_per_token"), p.get("rerenders"))
                == (100000, len(text.encode()), "3.7", 0), p)
        c.check("dropped = candidates - packed", p.get("dropped") == p.get("candidates", 0) - len(nodes), p)
        c.check("neighbours derived", all(n["tier"] == "derived" and n["matched"] is None
                                          for n in nodes if n["reason"] == "neighbour"), nodes)
        c.check("absence None", p.get("absence", 1) is None, p.get("absence"))

        price = first.get("id")
        by_id, err = stderr_of(lambda: g.pack_ids([price], budget=100000))
        c.check("pack_ids -> dict in engine field order", list(by_id) == PACK_KEYS, by_id)
        seed = by_id["nodes"][0] if by_id.get("nodes") else {}
        c.check("pack_ids seeds that id",
                (seed.get("id"), seed.get("reason"), seed.get("tier"), seed.get("matched"))
                == (price, "seed", "fact", None), seed)
        c.check("pack_ids query is the seed qnames", by_id.get("query") == "shop::a::price", by_id.get("query"))
        c.check("pack_ids marker", "[pack] query=shop::a::price seeds=1 " in err, err[-400:])

        rate = g.pack("price", bytes_per_token=4.04, candidates=2, preset="repair", scope="shop")
        c.check("bytes_per_token rounded to tenths", rate["bytes_per_token"] == "4.0", rate["bytes_per_token"])
        c.check("candidates cap", rate["candidates"] <= 2, rate["candidates"])
        c.check("default budget", rate["budget_tokens"] == 8000, rate["budget_tokens"])
        c.raises("bytes_per_token below 1.0", ValueError, lambda: g.pack("price", bytes_per_token=0.5),
                 "between 1.0 and 20.0")
        c.raises("bytes_per_token above 20.0", ValueError,
                 lambda: g.pack_ids([price], bytes_per_token=20.5), "between 1.0 and 20.0")

        none = g.pack("zzz_nothing")
        c.check("no match -> absence no_match, empty pack",
                (none.get("absence") or {}).get("reason") == "no_match"
                and none["nodes"] == [] and none["text"] == "" and none["used_tokens"] == 0, none)
        missing = g.pack_ids([1, 2])
        c.check("unheld ids -> absence no_match", (missing.get("absence") or {}).get("reason") == "no_match",
                missing)
        tiny = g.pack("price", budget=1)
        c.check("budget too small -> absence", (tiny.get("absence") or {}).get("reason") == "budget_too_small",
                tiny)
        c.check("deterministic", g.pack("price", budget=300) == g.pack("price", budget=300))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
