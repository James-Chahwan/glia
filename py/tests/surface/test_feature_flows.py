#!/usr/bin/env python3
"""pyo3 surface, py/src/feature_flows.rs (LG.3c): `PyGraph.feature_flows`
returns a native list of dicts {feature, services, entries, data_sources};
`PyGraph.write_feature_flows` writes `<feature>.yaml` + `index.json` and
returns {written, unchanged, removed, dir}. Built over a copy of
tests/fixtures/flows_stack (web + api). Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of, tree

FIXTURE = pathlib.Path(__file__).resolve().parents[3] / "tests" / "fixtures" / "flows_stack"


def main() -> int:
    c = Checks("feature_flows")
    c.check("feature_flows params",
            params(rg.PyGraph.feature_flows) == [("group_by", "feature"), ("depth", 6),
                                                 ("feature", None), ("scope", None)],
            params(rg.PyGraph.feature_flows))
    c.check("write_feature_flows params",
            params(rg.PyGraph.write_feature_flows) == [("out_dir", None), ("group_by", "feature"),
                                                       ("depth", 6)],
            params(rg.PyGraph.write_feature_flows))
    with tempfile.TemporaryDirectory(prefix="glia-surface-feature-flows-") as tmp:
        stack = pathlib.Path(tmp) / "stack"
        shutil.copytree(FIXTURE, stack)
        web, api = str(stack / "web"), str(stack / "api")
        g = rg.generate_many([web, api])

        flows, err = stderr_of(lambda: g.feature_flows())
        c.check("feature_flows -> list", type(flows) is list, type(flows))
        keys = [f.get("feature") for f in flows] if type(flows) is list else []
        c.check("features", keys == ["orders", "queue-orders.created"], keys)
        first = flows[0] if keys else {}
        c.check("record keys in field order",
                list(first) == ["feature", "services", "entries", "data_sources"], list(first))
        entry = (first.get("entries") or [{}])[0].get("entry", {})
        c.check("entry line is an int", type(entry.get("line")) is int, entry)
        c.check("[feature-flows] features= marker", "[feature-flows] features=2 " in err, err)
        one = g.feature_flows(feature="orders")
        c.check("feature= keeps one record", [f["feature"] for f in one] == ["orders"], one)
        by_entry = g.feature_flows(group_by="entry")
        c.check("group_by=entry keys by entry",
                "get_-api-orders" in [f["feature"] for f in by_entry], [f["feature"] for f in by_entry])
        c.raises("bad group_by raises", ValueError, lambda: g.feature_flows(group_by="service"),
                 "group_by")

        out = pathlib.Path(tmp) / "out"
        before = tree(str(stack))
        w = g.write_feature_flows(str(out))
        c.check("write -> dict in field order",
                type(w) is dict and list(w) == ["written", "unchanged", "removed", "dir"], w)
        c.check("write counts", w == {"written": 2, "unchanged": 0, "removed": 0, "dir": str(out)}, w)
        c.check("files written", (out / "orders.yaml").is_file() and (out / "index.json").is_file(),
                sorted(p.name for p in out.iterdir()) if out.exists() else None)
        again = g.write_feature_flows(str(out))
        c.check("second write unchanged", (again["written"], again["unchanged"]) == (0, 2), again)
        c.check("repos untouched", tree(str(stack)) == before)
        c.raises("walked dir refused", ValueError,
                 lambda: g.write_feature_flows(str(stack / "web" / "docs" / "flows")), "refusing")
        c.raises("two repos need out_dir", ValueError, lambda: g.write_feature_flows(), "out_dir")

        solo = rg.generate(web)
        w = solo.write_feature_flows()
        default = stack / "web" / ".glia" / "graph" / "flows"
        c.check("default dir", pathlib.Path(w["dir"]) == default and (default / "index.json").is_file(), w)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
