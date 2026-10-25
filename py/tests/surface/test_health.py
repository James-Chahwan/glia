#!/usr/bin/env python3
"""pyo3 surface, py/src/health.rs (FH.2b): `PyGraph.health()` returns the
`glia health` report `{repos, role, role_basis, verdict, reason, mechanisms,
parse, workspace_unresolved, suspicious, dead}` as a native dict, keys in
engine field order, and prints the engine's `[health] repo=<label> ..
surface=py` line once per call. The repos are written into temp dirs: a
TypeScript library with one unparsed `.marko` and one `.mdx`, and a Flask
app with a route nothing calls (the server side seen, no client). Shared
helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

HEALTH_KEYS = ["repos", "role", "role_basis", "verdict", "reason", "mechanisms", "parse",
               "workspace_unresolved", "suspicious", "dead"]
MECH_KEYS = ["mechanism", "producers", "external_producers", "consumers", "pairs",
             "paired_producers", "reached_consumers", "client_reach", "server_reach",
             "verdict", "reason", "reach", "measure"]
PARSE_KEYS = ["parsed_files", "unparsed", "docs_unparsed", "unparsed_files",
              "docs_unparsed_files", "verdict", "reason", "measure"]
MECHANISMS = ["http", "grpc", "rpc", "graphql", "queue", "ws"]


def write(root: pathlib.Path, rel: str, body: str) -> None:
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def main() -> int:
    c = Checks("health")
    c.check("health signature", params(rg.PyGraph.health) == [], params(rg.PyGraph.health))
    with tempfile.TemporaryDirectory(prefix="glia-surface-health-") as tmp:
        lib = pathlib.Path(tmp) / "strutil"
        write(lib, "src/index.ts", "export function pad(s: string): string {\n  return ' ' + s;\n}\n")
        write(lib, "ui/App.marko", "<p>x</p>\n")
        write(lib, "docs/guide.mdx", "# Guide\n")
        g = rg.generate(str(lib))

        h, err = stderr_of(g.health)
        markers = [l for l in err.splitlines() if l.startswith("[health] ")]
        c.check("one marker, surface=py",
                len(markers) == 1 and "repo=strutil role=library" in markers[0]
                and markers[0].endswith("verdict=OK surface=py"), markers)
        c.check("report -> dict in engine field order", type(h) is dict and list(h) == HEALTH_KEYS, h)
        c.check("library, OK", (h.get("repos"), h.get("role"), h.get("verdict"), h.get("role_basis"))
                == (["strutil"], "library", "OK", []), h)
        mechs = h.get("mechanisms", [])
        c.check("six mechanisms in table order", [m.get("mechanism") for m in mechs] == MECHANISMS, mechs)
        c.check("mechanism keys in engine field order", mechs and list(mechs[0]) == MECH_KEYS, mechs)
        c.check("a library is never judged on reach",
                all((m["verdict"], m["reason"]) == ("OK", "library") for m in mechs), mechs)
        parse = h.get("parse", {})
        c.check("parse keys in engine field order", list(parse) == PARSE_KEYS, parse)
        c.check("marko unparsed, mdx apart",
                (parse.get("unparsed"), parse.get("docs_unparsed"), parse.get("unparsed_files"),
                 parse.get("docs_unparsed_files")) == ({"marko": 1}, {"mdx": 1}, 1, 1), parse)
        c.check("suspicious / dead shapes",
                list(h.get("suspicious", {})) == ["count", "examples"]
                and list(h.get("dead", {})) == ["dead_symbols", "symbols", "share"], h)
        c.check("deterministic", g.health() == g.health())

        app = pathlib.Path(tmp) / "svc"
        body = "from flask import Flask\n\napp = Flask(__name__)\n"
        for i in range(12):
            body += f"\n\n@app.route('/items/{i}')\ndef item_{i}():\n    return 'ok'\n"
        write(app, "app.py", body)
        a = rg.generate(str(app)).health()
        http = next((m for m in a.get("mechanisms", []) if m.get("mechanism") == "http"), {})
        c.check("routes and no client: app, http BLIND no_clients",
                (a.get("role"), http.get("verdict"), http.get("reason"), http.get("consumers"),
                 http.get("producers")) == ("app", "BLIND", "no_clients", 12, 0), http)
        c.check("overall verdict is the worst row",
                a.get("verdict") == "BLIND" and a.get("reason", "").startswith("http: no_clients"), a)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
