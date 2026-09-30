#!/usr/bin/env python3
"""pyo3 surface, py/src/duplicate_flows.rs (CD.4f): `duplicate_flows(scope=None,
depth=6, threshold=0.8, min_size=3, include_tests=False, keep_hubs=False)`
returns the duplicate-flow answer `{entries, flows, hubs_ignored, candidates,
oversized_buckets, groups, absence}` as a native dict, keys in engine field
order; a threshold outside (0, 1] raises ValueError before the engine runs.
The fixture is CD.4e's acceptance tree (engine/tests/duplicate_flows.rs, no
log hub): GET /orders and GET /v2/orders stacked on one Flask handler, POST
/orders and PUT /orders/<id> sharing 14 of 18 nodes, GET /health alone, and
`tests/test_orders.py` calling the list handler. Shared helpers:
test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ANSWER_KEYS = ["entries", "flows", "hubs_ignored", "candidates", "oversized_buckets", "groups", "absence"]
GROUP_KEYS = ["kind", "tier", "entries", "jaccard", "shared", "union", "differing", "services"]
ROW_KEYS = ["id", "name", "qname", "kind", "file", "line"]
HELPERS = ["validate", "price", "tax", "discount", "stock", "reserve", "ship_date", "currency",
           "rounding", "fraud_check", "ledger_line", "audit_row", "receipt", "totals"]


def views_py() -> str:
    s = "from flask import Flask\n\nfrom app import repo\n\napp = Flask(__name__)\n\n\n"
    s += ("@app.route('/orders')\n@app.route('/v2/orders')\ndef list_orders():\n"
          "    rows = repo.find()\n    return {'rows': rows, 'n': repo.count()}\n\n\n")
    for h in HELPERS + ["audit_create", "audit_update"]:
        s += f"def {h}(x):\n    return x\n\n\n"
    for route, method, name, audit in [("/orders", "POST", "create_order", "audit_create"),
                                       ("/orders/<id>", "PUT", "update_order", "audit_update")]:
        s += f"@app.route('{route}', methods=['{method}'])\ndef {name}():\n    x = {{}}\n"
        s += "".join(f"    x = {h}(x)\n" for h in HELPERS)
        s += f"    return {audit}(x)\n\n\n"
    return s + "@app.route('/health')\ndef health():\n    return 'ok'\n"


def fixture(top: pathlib.Path) -> None:
    files = {
        "app/__init__.py": "",
        "app/views.py": views_py(),
        "app/repo.py": "def find():\n    return []\n\n\ndef count():\n    return 0\n",
        "tests/test_orders.py": "from app.views import list_orders\n\n\ndef test_list():\n    list_orders()\n",
    }
    for rel, src in files.items():
        path = top / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(src)


def line_of(prefix: str) -> int:
    return next(i for i, l in enumerate(views_py().splitlines(), 1) if l.startswith(prefix))


def qnames(rows) -> list[str]:
    return [r.get("qname") for r in rows]


def main() -> int:
    c = Checks("duplicate_flows")
    c.check("duplicate_flows signature",
            params(rg.PyGraph.duplicate_flows)
            == [("scope", None), ("depth", 6), ("threshold", 0.8), ("min_size", 3),
                ("include_tests", False), ("keep_hubs", False)],
            params(rg.PyGraph.duplicate_flows))
    with tempfile.TemporaryDirectory(prefix="glia-surface-dupflows-") as tmp:
        top = pathlib.Path(tmp) / "dupapp"
        fixture(top)
        g = rg.generate(str(top))

        d, err = stderr_of(lambda: g.duplicate_flows(threshold=0.7))
        c.check("marker", "[dupflows] entries=5 flows=4 exact_groups=1 near_groups=1 " in err
                and " threshold=0.7 surface=py" in err, err[-400:])
        c.check("duplicate_flows -> dict in engine field order", type(d) is dict and list(d) == ANSWER_KEYS, d)
        c.check("counts are ints", all(isinstance(d.get(k), int) for k in ANSWER_KEYS[:5]), d)
        c.check("entries / flows / hubs", (d.get("entries"), d.get("flows"), d.get("hubs_ignored")) == (5, 4, 0), d)
        groups = d.get("groups", [])
        c.check("two groups", len(groups) == 2, groups)
        exact = groups[0] if groups else {}
        c.check("group keys in engine field order", list(exact) == GROUP_KEYS, list(exact))
        c.check("exact group first", (exact.get("kind"), exact.get("tier")) == ("exact", "derived"), exact)
        c.check("exact group names both /orders routes",
                qnames(exact.get("entries", [])) == ["GET /orders", "GET /v2/orders"], exact)
        row = (exact.get("entries") or [{}])[0]
        c.check("entry row keys", list(row) == ROW_KEYS, list(row))
        c.check("entry located 1-based", (row.get("file"), row.get("line"), row.get("kind"))
                == ("app/views.py", line_of("def list_orders"), "ROUTE"), row)
        c.check("exact counts", (exact.get("jaccard"), exact.get("shared"), exact.get("union"), exact.get("differing"))
                == (1.0, 3, 3, []), exact)
        c.check("services", exact.get("services") == ["app"], exact.get("services"))
        near = groups[1] if len(groups) > 1 else {}
        c.check("near group", (near.get("kind"), near.get("tier")) == ("near", "heuristic")
                and qnames(near.get("entries", [])) == ["POST /orders", "PUT /orders/<id>"], near)
        c.check("near jaccard 14/18", near.get("jaccard") == 14 / 18
                and (near.get("shared"), near.get("union")) == (14, 18), near)
        c.check("near differs by the private helpers and handlers",
                sorted(qnames(near.get("differing", []))) == sorted(
                    f"app::views::{n}" for n in ["audit_create", "audit_update", "create_order", "update_order"]),
                near.get("differing"))
        c.check("absence None", d.get("absence", 1) is None, d.get("absence"))

        default = g.duplicate_flows()
        c.check("default threshold 0.8 keeps only the exact group",
                [x["kind"] for x in default["groups"]] == ["exact"], default["groups"])
        with_tests = g.duplicate_flows(include_tests=True)
        c.check("include_tests adds the test entry",
                with_tests["entries"] == 6 and qnames(with_tests["groups"][0]["entries"])
                == ["GET /orders", "GET /v2/orders", "tests::test_orders::test_list"], with_tests)
        c.check("min_size reaches the engine", g.duplicate_flows(min_size=4)["flows"] == 2)
        c.check("depth reaches the engine", g.duplicate_flows(depth=1, min_size=1)["flows"] == 5)
        c.check("keep_hubs reaches the engine", g.duplicate_flows(keep_hubs=True)["hubs_ignored"] == 0)
        none = g.duplicate_flows(scope="tests")
        c.check("no groups -> no_match absence",
                none["groups"] == [] and (none.get("absence") or {}).get("reason") == "no_match"
                and (none.get("absence") or {}).get("tier") == "FACT", none)
        for bad in (1.5, 0.0, -0.2, float("nan")):
            c.raises(f"threshold={bad}", ValueError, lambda bad=bad: g.duplicate_flows(threshold=bad),
                     "threshold must be in (0, 1]")
        _, err = stderr_of(lambda: c.raises("refused quietly", ValueError,
                                            lambda: g.duplicate_flows(threshold=2.0)))
        c.check("a refused threshold runs no engine", "[dupflows]" not in err, err[-400:])
        c.check("threshold=1.0 accepted", [x["kind"] for x in g.duplicate_flows(threshold=1.0)["groups"]] == ["exact"])
        c.check("deterministic", g.duplicate_flows(threshold=0.7) == g.duplicate_flows(threshold=0.7))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
