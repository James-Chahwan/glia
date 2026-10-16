#!/usr/bin/env python3
"""pyo3 surface, py/src/check.rs (LE.8): `check()` evaluates the declared
`[[constraint]]` rules and returns the report as one native dict. Shared
helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

# web/ imports and calls into services/api/; services/api/a.py and b.py
# import each other. Two manifests, so every rule anchors on a PROJECT.
FILES = (
    ("web/pyproject.toml", '[project]\nname = "web"\n'),
    ("services/api/pyproject.toml", '[project]\nname = "api"\n'),
    ("web/app.py", "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n"),
    ("services/api/internal.py", "def charge(o):\n    return o\n"),
    ("services/api/a.py", "from services.api.b import g\n\n\ndef f():\n    return g()\n"),
    ("services/api/b.py", "from services.api.a import f\n\n\ndef g():\n    return 1\n"),
    (".glia/overlay.toml", 'version = 1\n\n[[constraint]]\nid = "web-no-api-internals"\n'
     'kind = "forbid_edge"\nfrom = "web"\nto = "services/api"\ncategories = ["IMPORTS", "CALLS"]\n\n'
     '[[constraint]]\nid = "api-acyclic"\nkind = "no_cycle"\nscope = "services/api"\n\n'
     '[[constraint]]\nid = "prose"\nkind = "invariant"\nscope = "services/api"\n'
     'text = "charges are idempotent"\n'),
)

# DB.3a: a version-2 invariant whose Datalog `rule` names every pkg/a.py
# FUNCTION nothing calls; b.py calls `used`, so `orphan` is the one row.
RULE_FILES = (
    ("pkg/pyproject.toml", '[project]\nname = "pkg"\n'),
    ("pkg/a.py", "def used():\n    return 1\n\n\ndef orphan():\n    return 2\n"),
    ("pkg/b.py", "from pkg.a import used\n\n\ndef main():\n    return used()\n"),
    (".glia/overlay.toml", 'version = 2\n\n[[constraint]]\nid = "a-all-called"\nkind = "invariant"\n'
     'scope = "pkg"\ntext = "every pkg/a.py function is called"\n'
     'rule = "violation(f) :- node(f, \\"FUNCTION\\"), in_scope(f, \\"pkg/a.py\\"), !called(f). '
     'called(f) :- edge(_, f, \\"CALLS\\")."\n'),
)

REPORT_KEYS = ["rules", "checked", "unchecked", "errors", "violations", "reflexion"]
VIOLATION_KEYS = ["rule_id", "rule_kind", "decl", "severity", "tier", "blocking", "count", "evidence"]
RULE_VIOLATION_KEYS = VIOLATION_KEYS + ["subject", "file", "line", "caveats"]
EDGE_KEYS = ["from_qname", "to_qname", "category", "file", "line", "emitter", "tier", "note"]


def write_tree(root: pathlib.Path, files) -> None:
    for rel, src in files:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(src)


def main() -> int:
    c = Checks("check")
    c.check("check signature", params(rg.PyGraph.check) == [], params(rg.PyGraph.check))
    with tempfile.TemporaryDirectory(prefix="glia-surface-check-") as tmp:
        root = pathlib.Path(tmp) / "shop"
        write_tree(root, FILES)
        g = rg.generate(str(root))

        report, err = stderr_of(g.check)
        c.check("marker", "[check] rules=3 checked=2 violations=2 (forbid_edge=1 no_cycle=1) "
                "unchecked=1 errors=0" in err, err[-400:])
        c.check("tiers marker (CC.3)", "[check] tiers fact=1 derived=1 heuristic=0" in err, err[-400:])
        c.check("gate marker (DB.3a)", "[check] gate blocking=2 advisory=0" in err, err[-400:])
        c.check("check -> dict", type(report) is dict, type(report))
        c.check("report keys in engine field order", list(report) == REPORT_KEYS, list(report))
        c.check("counts are ints", type(report.get("rules")) is int and report.get("rules") == 3
                and report.get("checked") == 2, report)
        c.check("invariant unchecked", report.get("unchecked") == ["prose"], report.get("unchecked"))
        c.check("no errors", report.get("errors") == [], report.get("errors"))
        c.check("no reflexion model without a component (CC.5b)",
                "reflexion" in report and report["reflexion"] is None, report.get("reflexion"))
        rows = report.get("violations", [])
        c.check("violations sorted by rule id",
                [v.get("rule_id") for v in rows] == ["api-acyclic", "web-no-api-internals"],
                [v.get("rule_id") for v in rows])
        cyc = rows[0] if rows else {}
        c.check("violation keys in engine field order", list(cyc) == VIOLATION_KEYS, list(cyc))
        c.check("cycle tier derived", cyc.get("tier") == "derived" and cyc.get("severity") == "VIOLATION",
                cyc)
        c.check("derived and fact violations block (DB.3a)",
                [v.get("blocking") for v in rows] == [True, True], [v.get("blocking") for v in rows])
        c.check("cycle decl", cyc.get("decl") == ".glia/overlay.toml:10", cyc.get("decl"))
        w = cyc.get("evidence", [])
        c.check("2-hop witness a -> b -> a",
                [(h["from_qname"], h["to_qname"]) for h in w]
                == [("services::api::a", "services::api::b"), ("services::api::b", "services::api::a")], w)
        c.check("edge keys in engine field order", w and list(w[0]) == EDGE_KEYS, w and list(w[0]))
        fe = rows[1] if len(rows) > 1 else {}
        c.check("forbid tier fact, count 2", fe.get("tier") == "fact" and fe.get("count") == 2, fe)
        c.check("forbidden edges located, 1-based int lines",
                [(e["category"], e["file"], e["line"]) for e in fe.get("evidence", [])]
                == [("IMPORTS", "web/app.py", 1), ("CALLS", "web/app.py", 5)], fe.get("evidence"))
        c.check("emitters named", all(e.get("emitter", "").startswith("graph:")
                                      for e in fe.get("evidence", [])), fe.get("evidence"))
        c.check("evidence rows carry why's tier (CC.3)",
                [(e.get("tier"), e.get("note")) for e in fe.get("evidence", [])] == [("fact", None), ("fact", None)]
                and [h.get("tier") for h in w] == ["fact", "fact"], (fe.get("evidence"), w))

    with tempfile.TemporaryDirectory(prefix="glia-surface-check-rule-") as tmp:
        root = pathlib.Path(tmp) / "lib"
        write_tree(root, RULE_FILES)
        g = rg.generate(str(root))
        report, err = stderr_of(g.check)
        c.check("rule marker (DB.3a)",
                "[rules] check invariant=1 rows=1 transitive=0 hits=0 (imports=0 symbols=0 mixed=0)" in err,
                err[-600:])
        c.check("rule invariant checked", (report.get("rules"), report.get("checked"), report.get("unchecked"),
                                           report.get("errors")) == (1, 1, [], []), report)
        rows = report.get("violations", [])
        c.check("one rule violation", [v.get("rule_kind") for v in rows] == ["rule"], rows)
        rv = rows[0] if rows else {}
        c.check("rule violation keys in engine field order", list(rv) == RULE_VIOLATION_KEYS, list(rv))
        c.check("rule subject is the uncalled function's qname", rv.get("subject") == ["pkg::a::orphan"], rv)
        c.check("rule violation located, 1-based int line",
                rv.get("file") == "pkg/a.py" and type(rv.get("line")) is int and rv.get("line") >= 1, rv)
        c.check("a fact row blocks", rv.get("tier") == "fact" and rv.get("blocking") is True, rv)
        c.check("no edge derives it", rv.get("evidence") == [] and rv.get("count") == 0, rv)
        c.check("the negation's CALLS caveats (Q5)",
                bool(rv.get("caveats")) and all(n.get("edge_category") in ("CALLS", "*")
                                                for n in rv.get("caveats", [])), rv.get("caveats"))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
