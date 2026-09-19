#!/usr/bin/env python3
"""pyo3 surface, py/src/contracts.rs (LD.2): `contracts` returns a native
list of row dicts; `repo_id` / `node_id` are exact Python ints.
Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, fixture_repo, node_ids, rg, stderr_of

PUBLISHER = ('package svc\n\nimport "github.com/nats-io/nats.go"\n\n'
             'func Publish(nc *nats.Conn) error {\n\treturn nc.Publish("orders", nil)\n}\n')
ROW = ["topic", "topic_is_tag", "pattern", "producer", "consumer", "status", "confidence", "note"]


def main() -> int:
    c = Checks("contracts")
    with tempfile.TemporaryDirectory(prefix="glia-surface-contracts-") as tmp:
        empty = rg.generate(fixture_repo(tmp)).contracts()
        c.check("no queue: an empty list", empty == [] and type(empty) is list, empty)

        svc = pathlib.Path(tmp) / "svc"
        svc.mkdir()
        (svc / "publisher.go").write_text(PUBLISHER)
        g = rg.generate(str(svc))
        rows, err = stderr_of(g.contracts)
        c.check("marker", "[contracts] surface=pyo3 repos=1" in err, err[-300:])
        c.check("contracts -> list", type(rows) is list, type(rows))
        rows = rows if type(rows) is list else []
        c.check("one row per topic", [r.get("topic") for r in rows] == ["orders"], rows)
        c.check("row keys in engine field order", rows and list(rows[0]) == ROW, rows and list(rows[0]))
        p = rows[0]["producer"] if rows else None
        c.check("producer side is a dict", type(p) is dict, p)
        c.check("one-sided topic: consumer is None", rows and rows[0]["consumer"] is None)
        if type(p) is dict:
            c.check("node_id is an exact int of the graph",
                    type(p["node_id"]) is int and p["node_id"] in node_ids(g), p["node_id"])
            c.check("repo_id is an int", type(p["repo_id"]) is int, p["repo_id"])
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
