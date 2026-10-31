#!/usr/bin/env python3
"""pyo3 surface, py/src/trace.rs (LD.2, LD.4a): `cross_stack_trace` returns
the LD.4a answer as a native dict `{seed, target, resolved_by, hops, paths,
truncated, absence}`: `paths` are the ranked distinct paths, `hops` the BFS
tree the pre-0.5.0 call returned as a bare list. LD.6: every hop carries a
bool `to_live`. An unknown feature is an absence, never a ValueError. LD.4b:
`entry_flows` returns every entry point's forward flow as a list of dicts, and
a feature word naming a dead end or nothing yields to the entry flow its key
names (`resolved_by == "entry_flow"`). DD.11: both take `collapse=True` last
(identity classes walk as one node; a hop into a class carries `to_identity`,
absent on the single-repo fixture here). GB.1: `concept_flows(word, depth=6,
limit=20, collapse=True)` returns the flows that touch a word as `{results,
absence}`, and a dead end found by name, or an unknown word, that no entry key
names yields to the flow that touches it (`resolved_by == "concept"`); an exact
qname's dead end stays an absence. GB.3: `concept_flows(.., semantic=None)`
last; the fixture holds no vectors, so `semantic=None` / `False` answer alike
and no row carries `similarity` (the semantic tier itself is pinned by the
engine's flows_semantic tests). Shared helpers: test_build.py."""
from __future__ import annotations

import sys
import tempfile

from test_build import Checks, fixture_repo, params, rg, stderr_of

ANSWER_KEYS = ["seed", "target", "resolved_by", "hops", "paths", "truncated", "absence"]
HOP_KEYS = ["depth", "mechanism", "cross_service", "cross_repo", "from_qname", "to_qname", "to_kind",
            "to_live", "to_file", "to_line"]
PATH_KEYS = ["rank", "hops", "cross_service_hops", "mechanisms", "length", "directed"]
FLOW_KEYS = ["key", "entry", "reach", "cross_service", "mechanisms", "services", "hops"]
CONCEPT_KEYS = ["rank", "key", "entry", "tier", "matched", "hop", "depth", "reach", "mechanisms"]


def main() -> int:
    c = Checks("trace")
    c.check("cross_stack_trace signature",
            params(rg.PyGraph.cross_stack_trace)
            == [("feature", None), ("depth", 6), ("to", None), ("max_paths", 10), ("collapse", True)],
            params(rg.PyGraph.cross_stack_trace))
    c.check("entry_flows signature", params(rg.PyGraph.entry_flows) == [("depth", 6), ("collapse", True)],
            params(rg.PyGraph.entry_flows))
    c.check("concept_flows signature",
            params(rg.PyGraph.concept_flows)
            == [("word", None), ("depth", 6), ("limit", 20), ("collapse", True), ("semantic", None)],
            params(rg.PyGraph.concept_flows))
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

        flows, err = stderr_of(lambda: g.entry_flows())
        c.check("entry_flows -> list of dicts",
                type(flows) is list and bool(flows) and all(type(f) is dict for f in flows), type(flows))
        flows = flows if type(flows) is list else []
        c.check("flow keys in engine field order", flows and list(flows[0]) == FLOW_KEYS,
                flows and list(flows[0]))
        c.check("flows fired_on marker", "[flows] entries=1 flows=1 cross_service=0 depth<=6" in err,
                err[-400:])
        main = [f for f in flows if f.get("key") == "main"]
        c.check("main's flow: entry located, one hop to helper",
                len(main) == 1 and main[0]["entry"]["qname"] == "app::main" and main[0]["entry"]["line"]
                and main[0]["reach"] == 1 and [h["to_qname"] for h in main[0]["hops"]] == ["app::helper"],
                main)
        c.check("main's flow: one service, the repo label, not cross-service",
                main and main[0]["services"] == ["ld2app"] and main[0]["cross_service"] is False
                and main[0]["mechanisms"] == ["CALLS"], main)
        c.check("flow hops are trace hops", main and list(main[0]["hops"][0]) == HOP_KEYS,
                main and list(main[0]["hops"][0]))
        c.check("depth=0 keeps no flow", g.entry_flows(depth=0) == [])

        word = g.cross_stack_trace("main_flow")
        c.check("a word naming no node yields to the entry flow its key names",
                word.get("resolved_by") == "entry_flow" and (word.get("seed") or {}).get("qname") == "app::main",
                {k: word.get(k) for k in ("resolved_by", "seed")})
        dead, err = stderr_of(lambda: g.cross_stack_trace("helper"))
        c.check("a dead end found by name yields to the flow that touches it",
                dead.get("resolved_by") == "concept" and (dead.get("seed") or {}).get("qname") == "app::main"
                and dead.get("absence") is None and bool(dead.get("paths")),
                {k: dead.get(k) for k in ("resolved_by", "seed", "absence")})
        c.check("concept trace marker",
                "[trace] seed=helper resolved_by=concept concept_tier=hop_name paths=1" in err, err[-400:])
        exact = g.cross_stack_trace("app::helper")
        c.check("an exact qname's dead end stays an absence",
                exact.get("resolved_by") == "qname" and (exact.get("absence") or {}).get("reason") == "no_edges",
                {k: exact.get(k) for k in ("resolved_by", "absence")})

        hits, err = stderr_of(lambda: g.concept_flows("helper"))
        c.check("concept_flows -> dict {results, absence}",
                type(hits) is dict and list(hits) == ["results", "absence"], type(hits))
        rows = (hits or {}).get("results") or []
        c.check("one hit: main's flow, through helper's name",
                len(rows) == 1 and rows[0]["key"] == "main" and rows[0]["tier"] == "hop_name"
                and rows[0]["matched"] == ["helper -> name helper"] and rows[0]["depth"] == 1
                and (rows[0]["hop"] or {}).get("to_qname") == "app::helper", rows)
        c.check("concept row keys in engine field order", rows and list(rows[0]) == CONCEPT_KEYS,
                rows and list(rows[0]))
        c.check("concept hop is a trace hop", rows and list(rows[0]["hop"]) == HOP_KEYS,
                rows and list(rows[0]["hop"]))
        c.check("concept fired_on marker",
                "[concept] word=helper flows=1 hits=1 tiers=hop_name:1 surface=py ms=" in err, err[-400:])
        c.check("an entry name matches at the entry tier",
                [r["tier"] for r in g.concept_flows("main")["results"]] == ["entry"])
        none = g.concept_flows("zebra")
        c.check("a word nothing touches is a no_match absence, not a raise",
                none.get("results") == [] and (none.get("absence") or {}).get("reason") == "no_match"
                and (none.get("absence") or {}).get("tier") == "fact", none)
        c.check("depth=0 walks no flow", g.concept_flows("helper", depth=0)["results"] == [])
        c.check("no sidecar: semantic=False answers as the default",
                g.concept_flows("zebra", semantic=False) == none
                and g.concept_flows("helper", semantic=False) == hits)
        c.check("no sidecar: no row carries similarity",
                all("similarity" not in r for r in rows))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
