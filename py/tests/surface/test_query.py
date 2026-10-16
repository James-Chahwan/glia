#!/usr/bin/env python3
"""pyo3 surface, py/src/query.rs (DB.6): `query(program, ...)` answers a
Datalog program and `query_gql(query, ...)` a GQL read query over the graph,
each as the engine's QueryAnswer in one native dict; a parse, analysis or
budget error raises ValueError with the rendered diagnostics. Shared helpers:
test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

# DB.2a's fx1 without its overlay edge: three manifest projects; web/app.py
# `page()` calls shared.util.helper and `other()` calls
# services.api.public.facade.get; helper calls db.query; get calls db.write.
FILES = (
    ("web/pyproject.toml", '[project]\nname = "web"\n'),
    ("shared/pyproject.toml", '[project]\nname = "shared"\n'),
    ("services/api/pyproject.toml", '[project]\nname = "api"\n'),
    ("web/app.py", "from shared.util import helper\nfrom services.api.public.facade import get\n\n\n"
     "def page():\n    return helper()\n\n\ndef other():\n    return get()\n"),
    ("shared/util.py", "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n"),
    ("services/api/public/facade.py",
     "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n"),
    ("services/api/internal/db.py", "def query():\n    return 1\n\n\ndef write():\n    return 2\n"),
)

WEB_CALLS = 'x(a, b) :- calls(a, b), in_scope(a, "web").'
GQL = "MATCH (a:FUNCTION)-[:CALLS]->(b) RETURN a.qname, count(b) AS n ORDER BY n DESC"

ANSWER_KEYS = ["language", "outputs", "results", "absence"]
OUTPUT_KEYS = ["predicate", "arity", "columns", "total", "shown"]
ROW_KEYS = ["predicate", "values", "tier", "file", "line"]
NODE_KEYS = ["qname", "kind", "file", "line"]
EDGE_KEYS = ["from_qname", "to_qname", "category", "file", "line", "emitter", "tier", "note"]


def write_tree(root: pathlib.Path, files) -> None:
    for rel, src in files:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(src)


def qnames(row: dict) -> list:
    return [v.get("node", {}).get("qname") for v in row.get("values", [])]


def main() -> int:
    c = Checks("query")
    c.check("query signature",
            params(rg.PyGraph.query)
            == [("program", None), ("outputs", None), ("limit", None), ("explain", False), ("max_rows", None)],
            params(rg.PyGraph.query))
    c.check("query_gql signature",
            params(rg.PyGraph.query_gql)
            == [("query", None), ("limit", None), ("explain", False), ("max_rows", None)],
            params(rg.PyGraph.query_gql))
    with tempfile.TemporaryDirectory(prefix="glia-surface-query-") as tmp:
        root = pathlib.Path(tmp) / "fx1"
        write_tree(root, FILES)
        g = rg.generate(str(root))

        a, err = stderr_of(lambda: g.query(WEB_CALLS))
        c.check("marker", "[query] lang=datalog outputs=1 rows=2 fact=2 derived=0 heuristic=0 "
                "caveated=0 surface=py" in err, err[-600:])
        c.check("query -> dict", type(a) is dict, type(a))
        c.check("answer keys in engine field order", list(a) == ANSWER_KEYS, list(a))
        c.check("language datalog", a.get("language") == "datalog", a.get("language"))
        outs = a.get("outputs", [])
        c.check("one output x/2, columns from the head",
                outs == [{"predicate": "x", "arity": 2, "columns": ["a", "b"], "total": 2, "shown": 2}], outs)
        c.check("output keys in engine field order", outs and list(outs[0]) == OUTPUT_KEYS,
                outs and list(outs[0]))
        rows = a.get("results", [])
        c.check("2 results", len(rows) == 2, rows)
        r0 = rows[0] if rows else {}
        c.check("row keys in engine field order (no caveats, no witness)", list(r0) == ROW_KEYS, list(r0))
        c.check("rows in rendered order",
                [qnames(r) for r in rows] == [["web::app::other", "services::api::public::facade::get"],
                                              ["web::app::page", "shared::util::helper"]],
                [qnames(r) for r in rows])
        node = (r0.get("values") or [{}])[0].get("node", {})
        c.check("first value is web::app::other", node.get("qname") == "web::app::other", node)
        c.check("node value keys in engine field order", list(node) == NODE_KEYS, list(node))
        c.check("node located, 1-based int line",
                node.get("kind") == "FUNCTION" and node.get("file") == "web/app.py"
                and type(node.get("line")) is int and node.get("line") == 9, node)
        c.check("parsed calls are fact", [r.get("tier") for r in rows] == ["fact", "fact"],
                [r.get("tier") for r in rows])
        c.check("row located at its first node, int line",
                (r0.get("file"), type(r0.get("line")) is int, r0.get("line")) == ("web/app.py", True, 9), r0)
        c.check("a non-empty answer has no absence", "absence" in a and a["absence"] is None, a.get("absence"))

        # outputs= picks the predicates answered; a helper is never a default output.
        two = "x(a) :- calls(a, _), in_scope(a, \"web\").\ny(a) :- calls(_, a), in_scope(a, \"shared\")."
        both = g.query(two)
        c.check("default outputs are the sinks", [o["predicate"] for o in both.get("outputs", [])] == ["x", "y"],
                both.get("outputs"))
        only = g.query(two, outputs=["y"])
        c.check("outputs=['y'] honoured",
                [o["predicate"] for o in only.get("outputs", [])] == ["y"]
                and [qnames(r) for r in only.get("results", [])] == [["shared::util::helper"]], only)
        named = g.query(WEB_CALLS, outputs=["x"])
        c.check("outputs=['x'] honoured", [o["predicate"] for o in named.get("outputs", [])] == ["x"]
                and len(named.get("results", [])) == 2, named.get("outputs"))

        cut = g.query(WEB_CALLS, limit=1)
        c.check("limit cuts after the sort, total kept",
                (cut["outputs"][0]["total"], cut["outputs"][0]["shown"], [qnames(r) for r in cut["results"]])
                == (2, 1, [["web::app::other", "services::api::public::facade::get"]]), cut.get("outputs"))

        ex = g.query(WEB_CALLS, explain=True)
        wit = [r.get("witness", []) for r in ex.get("results", [])]
        c.check("explain: one witness edge per row", [len(w) for w in wit] == [1, 1], wit)
        w0 = wit[0][0] if wit and wit[0] else {}
        c.check("witness edge keys in engine field order", list(w0) == EDGE_KEYS, list(w0))
        c.check("witness is the located call",
                (w0.get("from_qname"), w0.get("to_qname"), w0.get("category"), w0.get("file"),
                 type(w0.get("line")) is int, w0.get("tier"))
                == ("web::app::other", "services::api::public::facade::get", "CALLS", "web/app.py", True, "fact"),
                w0)

        none = g.query('x(a) :- node(a, "ROUTE").')
        ab = none.get("absence") or {}
        c.check("empty answer: absence no_match",
                none.get("results") == [] and ab.get("reason") == "no_match", none)

        c.raises("unknown kind raises ValueError", ValueError, lambda: g.query('x(a) :- node(a, "ROUTEE").'),
                 "ROUTEE")
        c.raises("the rendered diagnostic names the source and position", ValueError,
                 lambda: g.query('x(a) :- node(a, "ROUTEE").'), "<query>:1:")
        c.raises("parse error raises ValueError", ValueError, lambda: g.query("x(a) :- "), "<query>:1:")
        c.raises("unknown output raises ValueError", ValueError,
                 lambda: g.query(WEB_CALLS, outputs=["nope"]), "nope")
        c.raises("row budget raises ValueError naming max_rows", ValueError,
                 lambda: g.query("x(a) :- node(a, _).", max_rows=1), "max_rows=<n>")
        roomy = g.query("x(a) :- node(a, _).", max_rows=1000)
        c.check("max_rows raises the budget", roomy.get("outputs", [{}])[0].get("total", 0) > 1,
                roomy.get("outputs"))

        q, err = stderr_of(lambda: g.query_gql(GQL))
        c.check("gql marker", "[query] lang=gql outputs=1 rows=4 fact=4 derived=0 heuristic=0 "
                "caveated=0 surface=py" in err, err[-600:])
        c.check("query_gql -> dict, gql", type(q) is dict and list(q) == ANSWER_KEYS
                and q.get("language") == "gql", q)
        gout = q.get("outputs", [{}])[0]
        c.check("gql columns", gout.get("predicate") == "result" and gout.get("columns") == ["a.qname", "n"],
                gout)
        grows = q.get("results", [])
        c.check("gql rows: qname strings and int counts, ORDER BY n DESC",
                [[list(v) for v in r["values"]] for r in grows] == [[["str"], ["int"]]] * 4
                and [r["values"][1]["int"] for r in grows] == [1, 1, 1, 1], grows)
        c.check("gql ties keep the rendered order",
                [r["values"][0]["str"] for r in grows]
                == ["services::api::public::facade::get", "shared::util::helper", "web::app::other",
                    "web::app::page"], grows)
        glim = g.query_gql(GQL + " LIMIT 3", limit=2)
        c.check("gql: limit= cuts after the query's LIMIT, total uncut",
                (glim["outputs"][0]["total"], glim["outputs"][0]["shown"]) == (4, 2), glim.get("outputs"))
        c.raises("gql unknown kind raises ValueError", ValueError,
                 lambda: g.query_gql("MATCH (a:ROUTEE) RETURN a"), "ROUTEE")
        c.raises("gql diagnostics name <gql>", ValueError,
                 lambda: g.query_gql("MATCH (a:FUNCTION) RETURN"), "<gql>:1:")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
