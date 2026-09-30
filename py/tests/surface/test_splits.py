#!/usr/bin/env python3
"""pyo3 surface, py/src/splits.rs (CD.2d): `splits(scope=None, parts=2,
quotient="module", source=None, sink=None, min_share=0.1, seed=42)` returns
the splits answer `{mode, quotient, units, parts, cut_weight,
global_min_weight, balanced, cut_edges_total, cut_edges, arch, shared_writes,
cycles, tier, absence}` as a native dict, keys in engine field order. The
argument errors `glia splits` exits 2 on raise ValueError before the engine
runs: `parts` outside 2..=8, `source` without `sink` (or the reverse),
`min_share` outside [0, 0.5]; an unknown quotient is an absence `no_match`.
The fixture is CD.2b / CD.2c's acceptance tree (engine/tests/splits.rs): two
packages of four modules, every function calling both functions of every
other module in its package, joined by `orders.api.checkout ->
billing.charge.charge` and `billing.charge.refund -> orders.repo.reopen`,
`util/fmt.py` hanging off `orders.cart.total`, and the sqlite table `orders`
written from both sides. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ANSWER_KEYS = ["mode", "quotient", "units", "parts", "cut_weight", "global_min_weight", "balanced",
               "cut_edges_total", "cut_edges", "arch", "shared_writes", "cycles", "tier", "absence"]
PART_KEYS = ["id", "nodes", "units", "label", "modules", "services", "entries", "top_members"]
CUT_KEYS = ["from_qname", "to_qname", "category", "from_part", "to_part", "weight", "file", "line",
            "basis"]
ARCH_KEYS = ["part", "services", "verdict"]
WRITE_KEYS = ["entity", "kind", "parts", "modes", "writers", "writers_total", "tier"]
CYCLE_KEYS = ["parts", "witness", "tier"]

ORDERS = [("api", ["create", "checkout"]), ("cart", ["add_item", "total"]),
          ("repo", ["save", "reopen"]), ("stock", ["reserve", "release"])]
BILLING = [("charge", ["charge", "refund"]), ("invoice", ["issue", "void"]),
           ("ledger", ["post", "balance"]), ("report", ["summary", "export"])]
CROSSING = [(("orders", "api", "checkout"), ("billing", "charge", "charge")),
            (("billing", "charge", "refund"), ("orders", "repo", "reopen")),
            (("orders", "cart", "total"), ("util", "fmt", "money"))]
SQL = [("orders", "repo", "save", "INSERT INTO orders (id) VALUES (1)"),
       ("billing", "ledger", "post", "UPDATE orders SET paid = 1 WHERE id = 1"),
       ("billing", "report", "summary", "SELECT * FROM ledger")]


def module_py(pkg: str, package, module: str) -> str:
    """CD.2b's `module_py`: sorted imports, then the two functions."""
    fns = dict(package)[module]
    imports: dict[tuple[str, str], set[str]] = {}
    bodies = []
    for f in fns:
        body = []
        for other, ofns in package:
            if other == module:
                continue
            imports.setdefault((pkg, other), set()).update(ofns)
            body += [f"    {c}()" for c in ofns]
        for (fp, fm, ff), (tp, tm, tf) in CROSSING:
            if (fp, fm, ff) == (pkg, module, f):
                imports.setdefault((tp, tm), set()).add(tf)
                body.append(f"    {tf}()")
        for sp, sm, sf, sql in SQL:
            if (sp, sm, sf) == (pkg, module, f):
                body.append(f'    conn.execute("{sql}")')
        bodies.append(body)
    src = "".join(f"from {p}.{m} import {', '.join(sorted(n))}\n" for (p, m), n in sorted(imports.items()))
    for f, body in zip(fns, bodies):
        src += f"\n\ndef {f}():\n" + "".join(line + "\n" for line in body)
    return src


def sources() -> dict[str, str]:
    files = {}
    for pkg, package in (("orders", ORDERS), ("billing", BILLING)):
        files[f"{pkg}/__init__.py"] = ""
        for m, _ in package:
            files[f"{pkg}/{m}.py"] = module_py(pkg, package, m)
    files["util/__init__.py"] = ""
    files["util/fmt.py"] = "def money():\n    return 0\n"
    return files


def fixture(top: pathlib.Path) -> None:
    for rel, src in sources().items():
        path = top / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(src)


def call_line(rel: str, caller: str, callee: str) -> int:
    lines = sources()[rel].splitlines()
    start = lines.index(f"def {caller}():")
    return start + lines[start:].index(f"    {callee}()") + 1


def part_with(answer, module: str):
    return next((p for p in answer.get("parts", []) if module in p.get("modules", [])), {})


def main() -> int:
    c = Checks("splits")
    c.check("splits signature",
            params(rg.PyGraph.splits)
            == [("scope", None), ("parts", 2), ("quotient", "module"), ("source", None), ("sink", None),
                ("min_share", 0.1), ("seed", 42)],
            params(rg.PyGraph.splits))
    with tempfile.TemporaryDirectory(prefix="glia-surface-splits-") as tmp:
        top = pathlib.Path(tmp) / "splitapp"
        fixture(top)
        g = rg.generate(str(top))

        a, err = stderr_of(g.splits)
        c.check("marker", "[splits] mode=global quotient=module units=9 parts=2 " in err
                and " shared_writes=1 part_cycles=1 surface=py" in err, err[-400:])
        c.check("splits -> dict in engine field order", type(a) is dict and list(a) == ANSWER_KEYS, a)
        c.check("global, module, heuristic", (a.get("mode"), a.get("quotient"), a.get("tier"))
                == ("global", "module", "heuristic"), a)
        c.check("9 units, 2 parts, balanced", (a.get("units"), len(a.get("parts", [])), a.get("balanced"))
                == (9, 2, True), a)
        c.check("cut above the global minimum", 0 < a.get("global_min_weight", 0) < a.get("cut_weight", 0), a)
        po, pb = part_with(a, "orders::api"), part_with(a, "billing::charge")
        c.check("part keys in engine field order", list(po) == PART_KEYS, list(po))
        c.check("the seam: orders + util | billing",
                (po.get("id"), po.get("label"), po.get("nodes"), po.get("units"))
                == (0, "orders", 14, 5)
                and (pb.get("id"), pb.get("label"), pb.get("nodes"), pb.get("units")) == (1, "billing", 12, 4),
                (po, pb))
        c.check("services are [name, count] lists", pb.get("services") == [["billing", 12]], pb.get("services"))
        cuts = a.get("cut_edges", [])
        c.check("cut edge keys in engine field order", bool(cuts) and list(cuts[0]) == CUT_KEYS,
                list(cuts[0]) if cuts else cuts)
        calls = {(e["from_qname"], e["to_qname"]): e for e in cuts if e.get("category") == "CALLS"}
        checkout = calls.get(("orders::api::checkout", "billing::charge::charge"), {})
        refund = calls.get(("billing::charge::refund", "orders::repo::reopen"), {})
        c.check("both seam calls, located 1-based at their call sites",
                len(calls) == 2
                and (checkout.get("file"), checkout.get("line"), checkout.get("basis"))
                == ("orders/api.py", call_line("orders/api.py", "checkout", "charge"), "site")
                and (refund.get("file"), refund.get("line"))
                == ("billing/charge.py", call_line("billing/charge.py", "refund", "reopen")),
                calls)
        c.check("cut_edges_total counts them", a.get("cut_edges_total") == len(cuts), a.get("cut_edges_total"))
        arch = a.get("arch", [])
        c.check("arch rows", [list(r) for r in arch] == [ARCH_KEYS, ARCH_KEYS]
                and [(r["part"], r["verdict"], r["services"]) for r in arch]
                == [(0, "spans_services", ["orders", "util"]), (1, "aligned", ["billing"])], arch)
        writes = a.get("shared_writes", [])
        w = writes[0] if writes else {}
        c.check("one shared write, keys in engine field order", len(writes) == 1 and list(w) == WRITE_KEYS,
                writes)
        c.check("the orders table, written from both parts",
                (w.get("entity", {}).get("qname"), w.get("kind"), w.get("parts"), w.get("modes"), w.get("tier"))
                == ("data_entity:sql:orders", "DATA_ENTITY", [0, 1], [[0, "write"], [1, "write"]], "derived"),
                w)
        c.check("writers located, by part", [x.get("qname") for x in w.get("writers", [])]
                == ["orders::repo::save", "billing::ledger::post"] and w.get("writers_total") == 2, w)
        cycles = a.get("cycles", [])
        cy = cycles[0] if cycles else {}
        c.check("one cycle between the parts", len(cycles) == 1 and list(cy) == CYCLE_KEYS
                and cy.get("parts") == [0, 1] and cy.get("tier") == "derived"
                and [(e["from_part"], e["to_part"]) for e in cy.get("witness", [])] == [(0, 1), (1, 0)], cycles)
        c.check("absence None", a.get("absence", 1) is None, a.get("absence"))

        st, err = stderr_of(lambda: g.splits(source="orders", sink="billing"))
        c.check("st marker", "[splits] mode=st " in err and err.rstrip().endswith("surface=py"), err[-400:])
        c.check("st: part 0 is the source side", st.get("mode") == "st"
                and "orders::api" in st["parts"][0]["modules"]
                and "billing::ledger" in st["parts"][1]["modules"], st)
        by_node = g.splits(source="orders::api::create", sink="billing::ledger::post")
        c.check("st by node", by_node.get("mode") == "st" and by_node.get("absence") is None
                and "orders::api" in by_node["parts"][0]["modules"], by_node)

        three = g.splits(parts=3)
        c.check("parts=3", len(three.get("parts", [])) == 3 and three.get("cut_weight", 0) > a["cut_weight"], three)
        community = g.splits(quotient="community", seed=7, min_share=0.2)
        c.check("community quotient", community.get("quotient") == "community"
                and community.get("absence") is None, community)
        scoped = g.splits(scope="orders")
        c.check("scope", scoped.get("units") == 4, scoped)

        one = g.splits(scope="util")
        c.check("fewer than two units -> no_match absence",
                (one.get("absence") or {}).get("reason") == "no_match"
                and (one.get("absence") or {}).get("note", "").startswith("fewer than two units in scope")
                and one.get("parts") == [], one)
        unknown = g.splits(quotient="files")
        c.check("unknown quotient -> no_match absence",
                unknown.get("quotient") == "none"
                and (unknown.get("absence") or {}).get("reason") == "no_match"
                and "files" in (unknown.get("absence") or {}).get("note", ""), unknown)
        overlap = g.splits(source="orders::api", sink="orders::api::checkout")
        c.check("sides sharing a unit -> no_match absence",
                (overlap.get("absence") or {}).get("reason") == "no_match"
                and "share" in (overlap.get("absence") or {}).get("note", ""), overlap)

        c.raises("source without sink", ValueError, lambda: g.splits(source="orders"), "the sink is not set")
        c.raises("sink without source", ValueError, lambda: g.splits(sink="billing"), "the source is not set")
        c.raises("parts=9", ValueError, lambda: g.splits(parts=9), "2..=8")
        c.raises("parts=1", ValueError, lambda: g.splits(parts=1), "2..=8")
        c.raises("min_share=0.9", ValueError, lambda: g.splits(min_share=0.9), "[0, 0.5]")
        c.raises("min_share=NaN", ValueError, lambda: g.splits(min_share=float("nan")), "[0, 0.5]")
        _, err = stderr_of(lambda: c.raises("refused quietly", ValueError, lambda: g.splits(source="orders")))
        c.check("a refused call runs no engine", "[splits]" not in err, err[-400:])
        c.check("deterministic", g.splits(parts=3) == g.splits(parts=3))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
