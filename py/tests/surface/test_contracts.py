#!/usr/bin/env python3
"""pyo3 surface, py/src/contracts.rs (LD.2): `contracts` returns a native
list of row dicts; `repo_id` / `node_id` are exact Python ints. LE.10d:
`contract_fields` returns the field-level diff rows the same way.
Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile

from test_build import Checks, fixture_repo, node_ids, rg, stderr_of

PUBLISHER = ('package svc\n\nimport "github.com/nats-io/nats.go"\n\n'
             'func Publish(nc *nats.Conn) error {\n\treturn nc.Publish("orders", nil)\n}\n')
ROW = ["topic", "topic_is_tag", "pattern", "producer", "consumer", "status", "confidence", "note"]
FIELD_ROW = ["pairing", "key", "producer", "consumer", "status", "tier", "note", "changes"]
FIELD_SIDE = ["repo_id", "qname", "format", "file", "line"]
CHANGE = ["section", "field", "change", "producer", "consumer", "rule", "breaking"]
# Two repos' copies of one proto message; the consumer's drifted (LE.10a).
PROTO_DRIFT = pathlib.Path(__file__).resolve().parents[3] / "bench/substrate-gap/fixtures/proto-field-drift"


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

        fields, err = stderr_of(g.contract_fields)
        c.check("contract_fields: no pairing -> an empty list", fields == [] and type(fields) is list, fields)
        c.check("contract_fields marker, always", "[contract-fields] surface=pyo3 rows=0" in err, err[-300:])

        # A copy, so the build never runs inside the committed fixture tree.
        dirs = []
        for d in ("producer", "consumer"):
            shutil.copytree(PROTO_DRIFT / d, pathlib.Path(tmp) / "drift" / d)
            dirs.append(str(pathlib.Path(tmp) / "drift" / d))
        drift = rg.generate_many(dirs)
        rows, err = stderr_of(drift.contract_fields)
        c.check("engine marker precedes the surface marker",
                err.find("[contract-fields] pairs=1 ") < err.find("[contract-fields] surface=pyo3 rows=1")
                and "[contract-fields] pairs=1 " in err, err[-400:])
        c.check("contract_fields -> list", type(rows) is list, type(rows))
        rows = rows if type(rows) is list else []
        c.check("one schema_copy row", [(r.get("pairing"), r.get("key")) for r in rows]
                == [("schema_copy", "shop.v1.OrderCreated")], rows)
        r = rows[0] if rows else {}
        c.check("field row keys in engine field order", list(r) == FIELD_ROW, list(r))
        c.check("drifted copy is breaking", r.get("status") == "breaking" and r.get("note") is None, r)
        side = r.get("producer")
        c.check("side keys in engine field order", type(side) is dict and list(side) == FIELD_SIDE, side)
        if type(side) is dict:
            c.check("side line is 1-based", side["line"] == 6, side)
            c.check("side repo_id is an exact int", type(side["repo_id"]) is int, side["repo_id"])
        drifted = [ch for ch in r.get("changes", []) if ch.get("field") == "total_cents"]
        c.check("change keys in engine field order", drifted and list(drifted[0]) == CHANGE, drifted)
        c.check("total_cents: int64 -> int32, breaking",
                drifted and (drifted[0]["producer"], drifted[0]["consumer"], drifted[0]["rule"],
                             drifted[0]["breaking"]) == ("int64", "int32", "proto_wire_type", True), drifted)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
