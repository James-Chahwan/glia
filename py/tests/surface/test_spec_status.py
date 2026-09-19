#!/usr/bin/env python3
"""pyo3 surface, py/src/spec_status.rs (LD.2, LE.9b): `spec_status(feature=None)`
returns a native dict {rows, by_feature, governed_services, ungoverned_routes}.
Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

FEATURE = "name: Orders\nbackend_routes:\n  - GET /orders\n  - POST /orders\n"
APP = ("from flask import Flask\n\napp = Flask(__name__)\n\n\n"
       "@app.get(\"/orders\")\ndef list_orders():\n    return []\n\n\n"
       "@app.get(\"/health\")\ndef health():\n    return {}\n")


def main() -> int:
    c = Checks("spec_status")
    c.check("signature", params(rg.PyGraph.spec_status) == [("feature", None)],
            params(rg.PyGraph.spec_status))
    with tempfile.TemporaryDirectory(prefix="glia-surface-spec-status-") as tmp:
        root = pathlib.Path(tmp) / "shop"
        (root / "features" / "orders").mkdir(parents=True)
        (root / "app").mkdir()
        (root / "features" / "orders" / "feature.yaml").write_text(FEATURE)
        (root / "app" / "main.py").write_text(APP)
        g = rg.generate(str(root))
        s, err = stderr_of(g.spec_status)
        c.check("spec_status -> dict", type(s) is dict, type(s))
        c.check("keys in field order",
                type(s) is dict and list(s) == ["rows", "by_feature", "governed_services",
                                                "ungoverned_routes"],
                type(s) is dict and list(s))
        rows = s.get("rows", []) if type(s) is dict else []
        c.check("statuses reach Python",
                [(r.get("status"), r.get("path")) for r in rows]
                == [("implemented", "/orders"), ("declared_missing", "/orders"),
                    ("undeclared", "/health")], rows)
        c.check("undeclared row has no feature", rows[-1:] and rows[-1].get("feature") is None, rows)
        route = rows[0].get("route") if rows else None
        c.check("located line is an int", type(route) is dict and type(route.get("line")) is int, route)
        c.check("id is an int", type(route) is dict and type(route.get("id")) is int, route)
        c.check("by_feature tally", s.get("by_feature", {}).get("orders")
                == {"declared": 2, "implemented": 1, "declared_missing": 1}, s.get("by_feature"))
        c.check("fired_on marker", "[sdd] spec_status features=1 declared=2 implemented=1 "
                "declared_missing=1 undeclared=1 ungoverned=0" in err, err[-400:])
        only = g.spec_status(feature="nope")
        c.check("feature filter", only.get("rows") == [] and only.get("by_feature") == {}, only)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
