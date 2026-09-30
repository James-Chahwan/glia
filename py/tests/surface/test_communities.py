#!/usr/bin/env python3
"""pyo3 surface, py/src/communities.rs (CD.1e): `communities(scope=None,
seed=42, resolution=1.0, top=30, members=10, method=None)` returns the
communities answer `{method, seed, resolution, modularity, total, nodes,
isolated, communities, absence}` as a native dict, keys in engine field order;
an unknown method is an absence `no_match`, a resolution that is not a finite
number above 0 raises ValueError. The fixture is CD.1d's acceptance tree
(engine/tests/communities.rs): two packages, each a ring of six functions with
chords, `pkg_a::core::f3` calling `pkg_b::core::g1`, `main` in pkg_a. Shared
helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ANSWER_KEYS = ["method", "seed", "resolution", "modularity", "total", "nodes", "isolated",
               "communities", "absence"]
COMMUNITY_KEYS = ["id", "size", "label", "tier", "cohesion", "files", "kinds", "top_members",
                  "entries", "services", "sinks", "links"]
MEMBER_KEYS = ["qname", "kind", "file", "line", "weight"]
ENTRY_KEYS = ["id", "name", "qname", "kind", "file", "line"]
LINK_KEYS = ["to", "weight", "edges", "categories"]


def core_py(p: str, imp: str | None) -> str:
    s = f"from {imp}.core import g1\n\n" if imp else ""
    for i in range(6):
        extra = "\n    g1()" if p == "f" and i == 3 else ""
        s += f"\ndef {p}{i}():\n    {p}{(i + 1) % 6}()\n    {p}{(i + 2) % 6}(){extra}\n\n"
    if p == "f":
        s += "\ndef main():\n    f0()\n"
    return s


def fixture(top: pathlib.Path) -> None:
    files = {
        "pkg_a/__init__.py": "",
        "pkg_a/core.py": core_py("f", "pkg_b"),
        "pkg_b/__init__.py": "",
        "pkg_b/core.py": core_py("g", None),
    }
    for rel, src in files.items():
        path = top / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(src)


def main_line() -> int:
    return core_py("f", "pkg_b").splitlines().index("def main():") + 1


def labels(answer) -> list[str]:
    return [c.get("label") for c in answer.get("communities", [])]


def main() -> int:
    c = Checks("communities")
    c.check("communities signature",
            params(rg.PyGraph.communities)
            == [("scope", None), ("seed", 42), ("resolution", 1.0), ("top", 30), ("members", 10),
                ("method", None)],
            params(rg.PyGraph.communities))
    with tempfile.TemporaryDirectory(prefix="glia-surface-communities-") as tmp:
        top = pathlib.Path(tmp) / "twopkgs"
        fixture(top)
        g = rg.generate(str(top))

        a, err = stderr_of(g.communities)
        markers = [line for line in err.splitlines() if line.startswith("[communities] ")]
        c.check("one marker", len(markers) == 1, err[-600:])
        marker = markers[0] if markers else ""
        c.check("marker names leiden and surface=py",
                marker.startswith("[communities] method=leiden nodes=17 ")
                and marker.endswith(" seed=42 surface=py"), marker)
        c.check("communities -> dict in engine field order", type(a) is dict and list(a) == ANSWER_KEYS, a)
        c.check("header values", (a.get("method"), a.get("seed"), a.get("resolution"), a.get("total"),
                                  a.get("nodes"), a.get("isolated")) == ("leiden", 42, 1.0, 2, 17, 2), a)
        c.check("modularity is a float", isinstance(a.get("modularity"), float) and a["modularity"] > 0.3,
                a.get("modularity"))
        comms = a.get("communities", [])
        c.check("communities is a list of dicts", type(comms) is list and all(type(x) is dict for x in comms),
                comms)
        c.check("two communities by size", labels(a) == ["pkg_a::core", "pkg_b::core"]
                and [x.get("size") for x in comms] == [8, 7] and [x.get("id") for x in comms] == [0, 1],
                labels(a))
        first = comms[0] if comms else {}
        c.check("community keys in engine field order", list(first) == COMMUNITY_KEYS, list(first))
        c.check("tier heuristic", all(x.get("tier") == "heuristic" for x in comms), comms)
        c.check("kinds are [name, count] lists", first.get("kinds") == [["FUNCTION", 7], ["MODULE", 1]],
                first.get("kinds"))
        c.check("services", first.get("services") == [["pkg_a", 8]], first.get("services"))
        c.check("no sinks", first.get("sinks") == [], first.get("sinks"))
        members = first.get("top_members", [])
        head = members[0] if members else {}
        c.check("member keys in engine field order", list(head) == MEMBER_KEYS, list(head))
        c.check("heaviest member located 1-based",
                (head.get("qname"), head.get("kind"), head.get("file"), head.get("line"))
                == ("pkg_a::core::f0", "FUNCTION", "pkg_a/core.py", 4), head)
        c.check("every member listed at the default", len(members) == 8, members)
        entries = first.get("entries", [])
        entry = entries[0] if entries else {}
        c.check("entry keys in engine field order", list(entry) == ENTRY_KEYS, list(entry))
        c.check("main is the entry", [e.get("qname") for e in entries] == ["pkg_a::core::main"]
                and entry.get("line") == main_line() and isinstance(entry.get("id"), int), entries)
        links = first.get("links", [])
        link = links[0] if links else {}
        c.check("link keys in engine field order", list(link) == LINK_KEYS, list(link))
        c.check("one link of the call and the import",
                (len(links), link.get("to"), link.get("edges"), link.get("categories"))
                == (1, 1, 2, [["CALLS", 1], ["IMPORTS", 1]]), links)
        c.check("absence None", a.get("absence", 1) is None, a.get("absence"))

        cut = g.communities(top=1, members=3)
        c.check("top / members cut", labels(cut) == ["pkg_a::core"] and cut["total"] == 2
                and len(cut["communities"][0]["top_members"]) == 3, cut)
        lpa = g.communities(method="lpa", seed=7, resolution=0.5)
        c.check("method / seed / resolution reach the engine",
                (lpa["method"], lpa["seed"], lpa["resolution"], lpa["total"])
                == ("label_propagation", 7, 0.5, 2), lpa)
        scoped = g.communities(scope="pkg_b")
        c.check("scope", labels(scoped) == ["pkg_b::core"]
                and (scoped["nodes"], scoped["isolated"]) == (8, 1), scoped)
        unknown = g.communities(method="louvain")
        why = unknown.get("absence") or {}
        c.check("unknown method -> no_match absence",
                unknown.get("method") == "none" and unknown.get("communities") == []
                and why.get("reason") == "no_match" and "louvain" in why.get("note", ""), unknown)
        empty = g.communities(scope="no_such_dir")
        c.check("empty scope -> no_edges absence",
                (empty.get("absence") or {}).get("reason") == "no_edges", empty)
        for bad in (0.0, -1.0, float("nan"), float("inf")):
            c.raises(f"resolution {bad} raises", ValueError, lambda bad=bad: g.communities(resolution=bad),
                     "resolution must be a finite number > 0")
        c.check("deterministic", g.communities(top=0, members=0) == g.communities(top=0, members=0))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
