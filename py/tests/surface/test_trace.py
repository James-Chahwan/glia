#!/usr/bin/env python3
"""pyo3 surface, py/src/trace.rs (LD.2, LD.4a): `cross_stack_trace` returns
the LD.4a answer as a native dict `{seed, target, resolved_by, hops, paths,
truncated, absence}`: `paths` are the ranked distinct paths, `hops` the BFS
tree the pre-0.5.0 call returned as a bare list. LD.6: every hop carries a
bool `to_live`. An unknown feature is an absence, never a ValueError. Shared
helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, params, rg, stderr_of

ANSWER_KEYS = ["seed", "target", "resolved_by", "hops", "paths", "truncated", "absence"]
HOP_KEYS = ["depth", "mechanism", "cross_service", "cross_repo", "from_qname", "to_qname", "to_kind",
            "to_live", "to_file", "to_line"]
PATH_KEYS = ["rank", "hops", "cross_service_hops", "mechanisms", "length", "directed"]


def main() -> int:
    c = Checks("trace")
    c.check("cross_stack_trace signature",
            params(rg.PyGraph.cross_stack_trace)
            == [("feature", None), ("depth", 6), ("to", None), ("max_paths", 10)],
            params(rg.PyGraph.cross_stack_trace))
    with tempfile.TemporaryDirectory(prefix="glia-surface-trace-") as tmp:
        g = rg.generate(fixture_repo(tmp))
        t, err = stderr_of(lambda: g.cross_stack_trace("app::main"))
        c.check("cross_stack_trace -> dict", type(t) is dict, type(t))
        t = t if type(t) is dict else {}
        c.check("answer keys in engine field order", list(t) == ANSWER_KEYS, list(t))
        c.check("fired_on marker", "[trace] seed=app::main resolved_by=qname paths=" in err, err[-400:])
        c.check("resolved by qname", t.get("resolved_by") == "qname", t.get("resolved_by"))
        c.check("seed located", (t.get("seed") or {}).get("qname") == "app::main", t.get("seed"))
        hops = t.get("hops") or []
        c.check("main -> helper hop", any(h.get("to_qname") == "app::helper" for h in hops), hops[:3])
        c.check("hop keys in engine field order", hops and list(hops[0]) == HOP_KEYS, hops and list(hops[0]))
        c.check("hops carry a bool to_live", hops and all(type(h["to_live"]) is bool for h in hops))
        c.check("main -> helper lands on a live node",
                any(h.get("to_qname") == "app::helper" and h.get("to_live") is True for h in hops), hops[:3])
        paths = t.get("paths") or []
        c.check("a ranked path", paths and paths[0]["rank"] == 1, paths[:1])
        c.check("path keys in engine field order", paths and list(paths[0]) == PATH_KEYS,
                paths and list(paths[0]))
        c.check("path ends at helper",
                paths and paths[0]["hops"] and paths[0]["hops"][-1]["to_qname"] == "app::helper", paths[:1])
        c.check("a found answer has no absence", t.get("absence") is None, t.get("absence"))

        two = g.cross_stack_trace("app::main", to="app::helper")
        c.check("two-node target located", (two.get("target") or {}).get("qname") == "app::helper",
                two.get("target"))
        c.check("two-node directed path",
                len(two["paths"]) == 1 and two["paths"][0]["directed"] is True
                and two["paths"][0]["length"] == 1, two["paths"])
        back = g.cross_stack_trace("app::helper", to="app::main")
        c.check("reverse is the undirected fallback",
                len(back["paths"]) == 1 and back["paths"][0]["directed"] is False, back["paths"])
        c.check("max_paths=1 keeps one path", len(g.cross_stack_trace("app::main", max_paths=1)["paths"]) <= 1)

        gone = g.cross_stack_trace("no::such::thing")
        c.check("unknown feature is an absence, not a raise",
                (gone.get("absence") or {}).get("reason") == "unknown_symbol", gone.get("absence"))
        c.check("unknown feature resolves by none",
                gone.get("resolved_by") == "none" and gone.get("seed") is None and gone.get("paths") == [],
                gone)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
