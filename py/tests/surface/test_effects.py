#!/usr/bin/env python3
"""pyo3 surface, py/src/effects.rs (LE.4d): `PyGraph.effects(qnames, depth=8,
classes=None, writes_only=False, cross_service=False, scope=None)` answers the
effect sinks downstream of the named nodes (DB read / write with the SQL verb,
queue produce, outbound HTTP call, ...) as a native dict {seeds, effects,
counts, writes, unresolved, absence}, each row with a 1-based located witness
path; more than 64 names or an unknown class raises ValueError. Shared
helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ORDERS = """import { Kafka } from 'kafkajs';
import axios from 'axios';
import { Pool } from 'pg';
const pool = new Pool();
const kafka = new Kafka({ clientId: 'x', brokers: ['b'] });
const producer = kafka.producer();

export async function saveOrder(order) {
  await pool.query('INSERT INTO orders (id) VALUES ($1)', [order.id]);
  return order;
}

export async function loadOrder(id) {
  const r = await pool.query('SELECT * FROM orders WHERE id = $1', [id]);
  return r.rows[0];
}

export async function publishOrder(order) {
  await producer.send({ topic: 'orders', messages: [{ value: JSON.stringify(order) }] });
}

export async function notify(order) {
  const url = process.env.NOTIFY_URL;
  await axios.post('/api/notify', order);
}

export async function placeOrder(order) {
  await saveOrder(order);
  await publishOrder(order);
  await notify(order);
}

export function label(order) {
  return 'order ' + order.id;
}
"""
PLACE = "svc::orders::placeOrder"
KEYS = ["seeds", "effects", "counts", "writes", "unresolved", "absence"]
ROW_KEYS = ["class", "qname", "name", "kind", "file", "line", "mode", "depth", "seed", "via_config",
            "services_crossed", "downstream", "path", "tier", "external_hosts"]
HOP_KEYS = ["from_qname", "to_qname", "category", "site_file", "site_line"]
SHAPE = [("db", "data_entity:sql:orders", "write", 2),
         ("queue_produce", "queue_producer:orders", None, 2),
         ("http_call", "endpoint:POST:/api/notify", None, 2)]
MARKER = ("[effects] seeds=1 reached=7 effects=3 (db=1 queue_produce=1 http_call=1 event_emit=0 other=0) "
          "writes=3 config_seeds=0")
KW = [("depth", 8), ("classes", None), ("writes_only", False), ("cross_service", False), ("scope", None)]


def shape(d) -> list:
    return [(r.get("class"), r.get("qname"), r.get("mode"), r.get("depth")) for r in d.get("effects", [])] \
        if type(d) is dict else d


def main() -> int:
    c = Checks("effects")
    method = getattr(rg.PyGraph, "effects", None)
    c.check("effects signature", method is not None and params(method) == [("qnames", None), *KW],
            method and params(method))
    with tempfile.TemporaryDirectory(prefix="glia-surface-effects-") as tmp:
        root = pathlib.Path(tmp) / "repo"
        (root / "svc").mkdir(parents=True)
        (root / "svc" / "orders.ts").write_text(ORDERS)
        g = rg.generate(str(root))

        a, err = stderr_of(lambda: g.effects([PLACE]))
        c.check("marker", MARKER in err, err[-400:])
        c.check("effects -> dict", type(a) is dict, type(a))
        c.check("answer keys in engine field order", type(a) is dict and list(a) == KEYS,
                type(a) is dict and list(a))
        c.check("three effects, class order", shape(a) == SHAPE, shape(a))
        rows = a.get("effects", []) if type(a) is dict else []
        c.check("row keys in engine field order", rows and list(rows[0]) == ROW_KEYS, rows and list(rows[0]))
        db = rows[0] if rows else {}
        path = db.get("path", [])
        c.check("hop keys in engine field order", path and list(path[0]) == HOP_KEYS, path)
        c.check("witness path from the seed",
                [(h["from_qname"], h["category"], h["to_qname"]) for h in path]
                == [(PLACE, "CALLS", "svc::orders::saveOrder"),
                    ("svc::orders::saveOrder", "ACCESSES_DATA", "data_entity:sql:orders")], path)
        c.check("hop site is the call, 1-based",
                path and (path[0]["site_file"], path[0]["site_line"]) == ("svc/orders.ts", 28), path)
        c.check("derived, seed named, no config", (db.get("tier"), db.get("seed"), db.get("via_config"))
                == ("derived", PLACE, None), db)
        c.check("counts every class", type(a) is dict and a["counts"].get("db") == 1
                and a["counts"].get("event_emit") == 0, a.get("counts") if type(a) is dict else a)
        c.check("writes, no absence", type(a) is dict and a["writes"] == 3 and a["absence"] is None, a)

        read = g.effects(["svc::orders::loadOrder"])
        c.check("a SELECT is a read", shape(read) == [("db", "data_entity:sql:orders", "read", 1)], shape(read))
        wo = g.effects(["svc::orders::loadOrder"], writes_only=True)
        c.check("writes_only drops the read",
                wo["effects"] == [] and (wo["absence"] or {}).get("reason") == "no_match", wo)
        only = g.effects([PLACE], classes=["http_call"])
        c.check("class filter", [r["class"] for r in only["effects"]] == ["http_call"], only)
        cfg = g.effects(["config:env:NOTIFY_URL"])
        c.check("config key seeds from its reader",
                [(r["class"], r["seed"], r["via_config"]) for r in cfg["effects"]]
                == [("http_call", "svc::orders::notify", "config:env:NOTIFY_URL")], cfg)

        none = g.effects(["svc::orders::label"])
        ab = none["absence"]
        c.check("a pure helper: no_edges FACT absence",
                none["effects"] == [] and type(ab) is dict
                and (ab.get("tier"), ab.get("reason")) == ("FACT", "no_edges"), none)
        c.check("absence counts unparsed files",
                type(ab) is dict and ab.get("unparsed_files") == len(g.parse_errors), ab)
        miss = g.effects(["svc::orders::nope"])
        c.check("unknown name: unresolved + unknown_symbol",
                miss["unresolved"] == ["svc::orders::nope"]
                and (miss["absence"] or {}).get("reason") == "unknown_symbol", miss)

        c.raises("65 names", ValueError, lambda: g.effects([f"n{i}" for i in range(65)]),
                 "effects: 65 seed names, more than the 64")
        c.raises("unknown class", ValueError, lambda: g.effects([PLACE], classes=["disk"]),
                 "unknown effect class `disk`")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
