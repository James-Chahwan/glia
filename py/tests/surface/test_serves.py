#!/usr/bin/env python3
"""pyo3 surface, py/src/serves.rs (LD.8b): `serves(channel, mechanism="auto")`
answers who serves an HTTP `METHOD /path` or a queue topic with the LD.8a
envelope {results, absence} as a native dict; an unknown mechanism raises
ValueError. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

API = ("from flask import Flask\nfrom kafka import KafkaProducer\n\napp = Flask(__name__)\n"
       "producer = KafkaProducer()\n\n\ndef publish(order):\n    producer.send('orders', order)\n\n\n"
       "@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n"
       "    publish(order)\n    return order\n")
WORKER = ("from kafka import KafkaConsumer\n\nconsumer = KafkaConsumer('orders')\n\n\n"
          "def handle():\n    for msg in consumer:\n        print(msg)\n")

SERVER_KEYS = ["id", "qname", "kind", "file", "line", "live", "match", "confidence", "handlers",
               "external_hosts"]
LOCATED_KEYS = ["id", "name", "qname", "kind", "file", "line"]


def main() -> int:
    c = Checks("serves")
    c.check("serves signature", params(rg.PyGraph.serves) == [("channel", None), ("mechanism", "auto")],
            params(rg.PyGraph.serves))
    with tempfile.TemporaryDirectory(prefix="glia-surface-serves-") as tmp:
        root = pathlib.Path(tmp)
        for repo, name, src in (("api", "app.py", API), ("worker", "consume.py", WORKER)):
            (root / repo).mkdir()
            (root / repo / name).write_text(src)
        g = rg.generate_many([str(root / "api"), str(root / "worker")])

        a, err = stderr_of(lambda: g.serves("POST /orders"))
        c.check("marker", "[serves] mechanism=http channel='POST /orders' servers=1 match=exact" in err,
                err[-400:])
        c.check("serves -> dict", type(a) is dict, type(a))
        c.check("envelope keys", type(a) is dict and list(a) == ["results", "absence"],
                type(a) is dict and list(a))
        rows = a["results"] if type(a) is dict else []
        c.check("one server", len(rows) == 1, rows)
        row = rows[0] if rows else {}
        c.check("server keys in engine field order", list(row) == SERVER_KEYS, list(row))
        c.check("the route, on the exact tier", (row.get("kind"), row.get("match")) == ("ROUTE", "exact"), row)
        # No client calls the route in this build, so a pass may demote it:
        # pin the vocabulary, not the value.
        c.check("confidence is a tier name", row.get("confidence") in ("strong", "medium", "weak"), row)
        c.check("id is an int, live a bool", type(row.get("id")) is int and type(row.get("live")) is bool,
                row)
        hs = row.get("handlers", [])
        c.check("handler located", [h.get("qname") for h in hs] == ["app::create_order"]
                and type(hs[0].get("line")) is int, hs)
        c.check("handler keys", hs and list(hs[0]) == LOCATED_KEYS, hs and list(hs[0]))
        c.check("no absence", a.get("absence", 1) is None if type(a) is dict else False, a)

        miss = g.serves("DELETE /orders")
        ab = miss["absence"]
        c.check("unserved verb: empty results", miss["results"] == [], miss["results"])
        c.check("unserved verb: unserved_channel FACT",
                type(ab) is dict and (ab.get("tier"), ab.get("reason")) == ("FACT", "unserved_channel"), ab)
        c.check("near miss names the POST route",
                type(ab) is dict and any(s.split(" @")[0] == "POST /orders" for s in ab.get("suggestions", [])),
                ab)
        c.check("absence counts unparsed files",
                type(ab) is dict and ab.get("unparsed_files") == len(g.parse_errors), ab)

        q = g.serves("orders", mechanism="queue")["results"]
        c.check("topic served by its consumer",
                [(r["qname"].split(" @")[0], r["kind"]) for r in q] == [("queue_consumer:orders", "QUEUE_CONSUMER")],
                q)
        pay = g.serves("payments")["absence"]
        c.check("auto reads a bare word as a topic",
                type(pay) is dict and pay.get("mechanisms") == ["QUEUE_FLOWS"], pay)
        c.raises("unknown mechanism", ValueError, lambda: g.serves("orders", mechanism="smtp"), "smtp")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
