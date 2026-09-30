#!/usr/bin/env python3
"""pyo3 surface, py/src/gaps.rs (LD.2, LF.2c): `PyGraph.gaps(top_k=None,
category=None)` returns a native dict {counts, skipped, rows}; the module
function `overlay_delta(repo_paths, incremental=False)` returns a native dict
{rules, edges_without, edges_with, added_by_category, orphans_without,
orphans_with, without, with, nodes_added_by_kind, verdict} (CE.3a); a row
carries a stable `id` first, and a `suspected_edge` row (CD.3b) a `draft` last:
the paste-ready `# gap: <id>` + `[[edge]]` stanza (CD.3c). Shared helpers:
test_build.py."""
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

# CD.3b's suspected_edges fixture (engine/tests/suspected_edges.rs): an Angular
# client whose list() / count() pair through the HTTP resolver, so (ENDPOINT,
# HTTP_CALLS, ROUTE) is learned, and whose markAllRead() posts through a
# class-field `${environment.apiUrl}` base no resolver pairs; an Express API
# serving the read-all routes behind a `/:tenant` path parameter, which the
# resolver's mount-segment fold (CB.23) never strips.
SUSPECTED = {
    "web/src/notifications.service.ts": (
        "import { Injectable } from '@angular/core';\n"
        "import { HttpClient } from '@angular/common/http';\n"
        "import { environment } from '../environments/environment';\n\n"
        "@Injectable({ providedIn: 'root' })\n"
        "export class NotificationsApi {\n"
        "  private readonly base = `${environment.apiUrl}/notifications`;\n"
        "  constructor(private http: HttpClient) {}\n\n"
        "  markAllRead() {\n    return this.http.post(`${this.base}/read-all`, {});\n  }\n\n"
        "  list() {\n    return this.http.get('/notifications');\n  }\n\n"
        "  count() {\n    return this.http.get('/notifications/count');\n  }\n}\n"),
    "web/environments/environment.ts": (
        "export const environment = { production: false, apiUrl: 'http://localhost:8080/api' };\n"),
    "web/package.json": ('{"name":"web","dependencies":{"@angular/core":"17.0.0",'
                         '"@angular/common":"17.0.0"}}\n'),
    "api/src/routes.ts": (
        "import express from 'express';\nconst router = express.Router();\n\n"
        "function markAll(req, res) { res.json({}); }\n"
        "function getOne(req, res) { res.json({}); }\n"
        "function listAll(req, res) { res.json([]); }\n"
        "function countAll(req, res) { res.json(0); }\n\n"
        "router.post('/:tenant/notifications/read-all', markAll);\n"
        "router.get('/:tenant/notifications/read-all', getOne);\n"
        "router.get('/notifications', listAll);\n"
        "router.get('/notifications/count', countAll);\n\n"
        "export default router;\n"),
    "api/package.json": '{"name":"api","dependencies":{"express":"4.18.0"}}\n',
}
ORPHAN = "endpoint:POST:${…}/read-all @web"


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
                bool(rows) and list(rows[0]) == ["id", "category", "qname", "kind", "file", "line",
                                                 "detail", "suggest", "tier"],
                rows[:1])
        c.check("no draft off suspected_edge", all("draft" not in r for r in rows),
                [r for r in rows if "draft" in r])
        c.check("row ids are gap:<16 hex> and unique",
                bool(rows) and all(str(r.get("id", "")).startswith("gap:")
                                   and len(r["id"]) == 20 for r in rows)
                and len({r.get("id") for r in rows}) == len(rows),
                [r.get("id") for r in rows])
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
                                                "orphans_with", "without", "with",
                                                "nodes_added_by_kind", "verdict"],
                type(d) is dict and list(d))
        c.check("counts keys in field order",
                type(d) is dict and type(d.get("with")) is dict
                and list(d["with"]) == ["nodes_by_kind", "edges_by_category", "gaps_by_category"],
                type(d) is dict and d.get("with"))
        c.check("verdict keep", type(d) is dict and d.get("verdict") == "keep", d)
        c.check("orphans fall 1 -> 0",
                type(d) is dict and (d.get("orphans_without"), d.get("orphans_with")) == (1, 0), d)
        c.check("HTTP_CALLS +1", type(d) is dict and d.get("added_by_category") == {"HTTP_CALLS": 1}, d)
        c.check("accept-loop marker", "[overlay] 1 rules, +1 edges, orphans 1→0, gaps 4→2, verdict=keep" in err, err[-400:])
        c.raises("no repo paths", ValueError, lambda: rg.overlay_delta([]), "no repo paths")

    with tempfile.TemporaryDirectory(prefix="glia-surface-gaps-suspected-") as tmp:
        root = pathlib.Path(tmp) / "notif"
        for rel, body in SUSPECTED.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(body)
        g = rg.generate(str(root))
        rep, err = stderr_of(lambda: g.gaps(category="suspected_edge"))
        rows = rep.get("rows", []) if type(rep) is dict else []
        c.check("one suspected row: the orphan",
                [(r.get("category"), r.get("qname")) for r in rows]
                == [("suspected_edge", ORPHAN)], rows)
        row = rows[0] if rows else {}
        draft = row.get("draft")
        c.check("suspected row keys in field order, draft last",
                list(row) == ["id", "category", "qname", "kind", "file", "line", "detail",
                              "suggest", "tier", "draft"], list(row))
        c.check("draft is a str: `# gap: <id>`, then [[edge]]",
                type(draft) is str and draft.startswith(f"# gap: {row.get('id')}\n")
                and draft.splitlines()[1:2] == ["[[edge]]"], draft)
        c.check("draft pairs the orphan with the POST route",
                type(draft) is str
                and f'from = "{ORPHAN}"' in draft.splitlines()
                and 'to = "POST /:tenant/notifications/read-all @api"' in draft.splitlines()
                and 'category = "HTTP_CALLS"' in draft.splitlines()
                and not any(line.startswith("gap = ") for line in draft.splitlines()), draft)
        c.check("suspected marker", " suspected_edge=1 " in err and " surface=py" in err,
                err[-400:])
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
