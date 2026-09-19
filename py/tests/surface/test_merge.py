#!/usr/bin/env python3
"""pyo3 surface, py/src/merge.rs (LC.10c): `merge_gmaps(dirs, out=None)`
merges pre-built layouts into the graph one build of every repo gives, without
their sources. Two repos built and saved separately (generate ->
save_to_default) merge into a PyGraph holding both, with the HTTP link only
the union has; `out` writes a layout load_from_gmap reads back. Shared
helpers: test_build.py."""
from __future__ import annotations

import json
import os
import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

API = (
    "from flask import Flask\n\napp = Flask(__name__)\n\n\n"
    '@app.route("/users")\ndef list_users():\n    return []\n'
)
WEB = (
    "export async function loadUsers() {\n"
    '  const res = await fetch("/users");\n  return res.json();\n}\n'
)


def repo(tmp: str, name: str, file: str, body: str) -> str:
    root = pathlib.Path(tmp) / name
    root.mkdir()
    (root / file).write_text(body)
    return str(root)


def main() -> int:
    c = Checks("merge")
    c.check("merge_gmaps signature", params(rg.merge_gmaps) == [("dirs", None), ("out", None)],
            params(rg.merge_gmaps))
    with tempfile.TemporaryDirectory(prefix="glia-surface-merge-") as tmp:
        api = repo(tmp, "lc10capi", "app.py", API)
        web = repo(tmp, "lc10cweb", "client.ts", WEB)
        for r in (api, web):
            rg.generate(r).save_to_default(r)
        dirs = [rg.default_gmap_dir(api), rg.default_gmap_dir(web)]
        joint = rg.generate_many([api, web])

        g, err = stderr_of(lambda: rg.merge_gmaps(dirs))
        c.check("marker", "[merge] members=2 (gmap=2 repo=0) repos=2 " in err, err[-400:])
        c.check("returns a PyGraph", type(g) is rg.PyGraph, type(g))
        c.check("same nodes as the joint build", g.node_count() == joint.node_count(),
                (g.node_count(), joint.node_count()))
        c.check("the union's cross edges", g.cross_edge_count() == joint.cross_edge_count() > 0,
                (g.cross_edge_count(), joint.cross_edge_count()))
        repos = {s["repo"] for s in g.service_map()["services"]}
        c.check("service_map names both repos", {"lc10capi", "lc10cweb"} <= repos, repos)

        out = os.path.join(tmp, "merged")
        _, err = stderr_of(lambda: rg.merge_gmaps(dirs, out=out))
        c.check("out marker", "[merge] wrote " in err and "writer=py" in err, err[-400:])
        manifest = json.loads(pathlib.Path(out, "manifest.json").read_text())
        c.check("out records the members (named by basename, .glia/graph stripped)",
                [(m["name"], m["source"]) for m in manifest.get("members", [])]
                == [("lc10capi", "gmap"), ("lc10cweb", "gmap")], manifest.get("members"))
        c.check("out loads back", rg.load_from_gmap(out).node_count() == joint.node_count())

        c.raises("no dirs", ValueError, lambda: rg.merge_gmaps([]), "merge_gmaps: no members")
        c.raises("missing layout", ValueError,
                 lambda: rg.merge_gmaps([os.path.join(tmp, "gone")]), "member 'gone'")
        c.raises("one name twice", ValueError, lambda: rg.merge_gmaps([dirs[0], dirs[0]]),
                 "two members are named 'lc10capi'")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
