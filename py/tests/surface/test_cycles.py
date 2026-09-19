#!/usr/bin/env python3
"""pyo3 surface, py/src/cycles.rs (LE.6b): `cycles(kind="all", scope=None)`
returns the cross-service loops and import cycles as a native list of dicts;
an unknown kind raises ValueError. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

# orders/ emits order.placed, billing/ handles it and emits payment.settled,
# orders/ handles that and re-places the order. No manifests: LB.8b drops
# in-process EVENT_FLOWS between two manifest projects.
ORDERS = ('import { EventEmitter } from "events";\nexport const bus = new EventEmitter();\n'
          'export function placeOrder(o) { bus.emit("order.placed", o); }\n'
          'export function registerOrderHandlers() { bus.on("payment.settled", (p) => { retryOrder(p); }); }\n'
          'function retryOrder(p) { placeOrder(p); }\n')
BILLING = ('import { EventEmitter } from "events";\nexport const bus = new EventEmitter();\n'
           'export function registerBillingHandlers() { bus.on("order.placed", (o) => { settle(o); }); }\n'
           'function settle(o) { bus.emit("payment.settled", o); }\n')
PY_A = "from pkg.b import f\n\n\ndef g():\n    return 1\n"
PY_B = "from pkg.a import g\n\n\ndef f():\n    return 2\n"

ROW_KEYS = ["kind", "tier", "services", "mechanisms", "channels", "size", "members", "witness", "note"]
HOP_KEYS = ["from_qname", "to_qname", "category", "channel", "file", "line"]


def main() -> int:
    c = Checks("cycles")
    c.check("cycles signature", params(rg.PyGraph.cycles) == [("kind", "all"), ("scope", None)],
            params(rg.PyGraph.cycles))
    with tempfile.TemporaryDirectory(prefix="glia-surface-cycles-") as tmp:
        root = pathlib.Path(tmp) / "shop"
        for rel, src in (("orders/app.ts", ORDERS), ("billing/app.ts", BILLING),
                         ("pkg/__init__.py", ""), ("pkg/a.py", PY_A), ("pkg/b.py", PY_B)):
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(src)
        g = rg.generate(str(root))

        rows, err = stderr_of(g.cycles)
        c.check("marker", "[cycles] event_loops=1 call_loops=0 possible=0 import_cycles=1 "
                "(sccs=2 nodes=11)" in err, err[-400:])
        c.check("cycles -> list", type(rows) is list, type(rows))
        c.check("kinds in order", [r.get("kind") for r in rows] == ["event_loop", "import_cycle"],
                [r.get("kind") for r in rows])
        loop = rows[0] if rows else {}
        c.check("row keys in engine field order", list(loop) == ROW_KEYS, list(loop))
        c.check("event loop spans both services", loop.get("services") == ["billing", "orders"], loop)
        c.check("channels", loop.get("channels") == ["order.placed", "payment.settled"], loop)
        w = loop.get("witness", [])
        c.check("9-hop closed witness",
                len(w) == 9 and all(w[i]["to_qname"] == w[(i + 1) % 9]["from_qname"] for i in range(9)), w)
        c.check("hop keys in engine field order", w and list(w[0]) == HOP_KEYS, w and list(w[0]))
        c.check("hops located, int lines", all(type(h.get("line")) is int and h.get("file") for h in w), w)
        c.check("size is an int, note None", type(loop.get("size")) is int and loop.get("note", 1) is None,
                loop)
        imp = rows[1] if len(rows) > 1 else {}
        c.check("import cycle", imp.get("members") == ["pkg::a", "pkg::b"]
                and {(h["file"], h["line"]) for h in imp.get("witness", [])}
                == {("pkg/a.py", 1), ("pkg/b.py", 1)}, imp)

        c.check("kind=import", [r["kind"] for r in g.cycles(kind="import")] == ["import_cycle"])
        c.check("kind=event", [r["kind"] for r in g.cycles("event")] == ["event_loop"])
        c.check("scope drops a partial loop", g.cycles(scope="orders") == [], g.cycles(scope="orders"))
        c.check("scope keeps the import cycle",
                [r["kind"] for r in g.cycles(scope="pkg")] == ["import_cycle"])
        c.raises("unknown kind", ValueError, lambda: g.cycles(kind="loops"), "loops")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
