#!/usr/bin/env python3
"""pyo3 surface, py/src/gaps.rs (LD.2, LF.2c): `PyGraph.gaps(top_k=None,
category=None)` returns a native dict {counts, skipped, rows}; the module
function `overlay_delta(repo_paths, incremental=False)` returns a native dict
{rules, edges_without, edges_with, added_by_category, orphans_without,
orphans_with}. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

CLIENT_TS = ("export function request(method: string, path: string) {\n"
             "  return fetch(path, { method });\n}\n\n"
             "export async function loadUsers() {\n  return request('GET', '/users');\n}\n")
API_PY = ("from flask import Flask\n\napp = Flask(__name__)\n\n\n"
          "@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n"
          "@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n")
PAIR = ("version = 1\n\n[[edge]]\nfrom = \"endpoint:GET:<unresolved>\"\n"
        "to = \"GET /users\"\ncategory = \"HTTP_CALLS\"\n")


def main() -> int:
    c = Checks("gaps")
    c.check("gaps signature", params(rg.PyGraph.gaps) == [("top_k", None), ("category", None)],
            params(rg.PyGraph.gaps))
    c.check("overlay_delta signature",
            rg.overlay_delta.__text_signature__ == "(repo_paths, incremental=False)",
            rg.overlay_delta.__text_signature__)
    with tempfile.TemporaryDirectory(prefix="glia-surface-gaps-") as tmp:
        root = pathlib.Path(tmp) / "r1"
        (root / "web" / "src").mkdir(parents=True)
        (root / "api").mkdir()
        (root / "web" / "src" / "client.ts").write_text(CLIENT_TS)
        (root / "api" / "app.py").write_text(API_PY)

        g = rg.generate(str(root))
        rep, err = stderr_of(g.gaps)
        c.check("gaps -> dict", type(rep) is dict, type(rep))
        c.check("keys in field order",
                type(rep) is dict and list(rep) == ["counts", "skipped", "rows"],
                type(rep) is dict and list(rep))
        rows = rep.get("rows", []) if type(rep) is dict else []
        unresolved = [r for r in rows if r.get("category") == "unresolved_endpoint"]
        c.check("one unresolved sink, owner named",
                [(r.get("qname"), r.get("detail")) for r in unresolved]
                == [("endpoint:GET:<unresolved>", "owner=web::src::client::request")], unresolved)
        c.check("row keys in field order",
                bool(rows) and list(rows[0]) == ["category", "qname", "kind", "file", "line",
                                                 "detail", "suggest", "tier"],
                rows[:1])
        c.check("located line is an int", bool(unresolved) and type(unresolved[0].get("line")) is int,
                unresolved)
        c.check("root categories computed", rep.get("skipped") == [], rep.get("skipped"))
        c.check("fired_on marker", "[gaps] rows=4 (" in err and " surface=py" in err, err[-400:])

        cut = g.gaps(top_k=1, category="unpaired_route")
        c.check("top_k + category cut rows, not counts",
                [r.get("qname") for r in cut.get("rows", [])] == ["GET /users"]
                and cut.get("counts", {}).get("unpaired_route") == 2, cut)
        c.raises("unknown category", ValueError, lambda: g.gaps(category="nope"),
                 "unknown gaps category")

        (root / "web" / ".glia").mkdir()
        (root / "web" / ".glia" / "overlay.toml").write_text(PAIR)
        d, err = stderr_of(lambda: rg.overlay_delta([str(root / "web"), str(root / "api")]))
        c.check("overlay_delta -> dict", type(d) is dict, type(d))
        c.check("delta keys in field order",
                type(d) is dict and list(d) == ["rules", "edges_without", "edges_with",
                                                "added_by_category", "orphans_without",
                                                "orphans_with"],
                type(d) is dict and list(d))
        c.check("orphans fall 1 -> 0",
                type(d) is dict and (d.get("orphans_without"), d.get("orphans_with")) == (1, 0), d)
        c.check("HTTP_CALLS +1", type(d) is dict and d.get("added_by_category") == {"HTTP_CALLS": 1}, d)
        c.check("accept-loop marker", "[overlay] 1 rules, +1 edges, orphans 1→0" in err, err[-400:])
        c.raises("no repo paths", ValueError, lambda: rg.overlay_delta([]), "no repo paths")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
