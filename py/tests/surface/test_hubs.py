#!/usr/bin/env python3
"""pyo3 surface, py/src/hubs.rs (CD.4c): `hubs(scope=None, top=20,
category=None, min_degree=5, include_tests=False)` returns the hubs answer
`{fan_in, fan_out, cross_service, nodes, edges, p99_in, p99_out, absence}` as a
native dict, keys in engine field order; an unknown category is an absence
`no_match`, not an error. The fixture is CD.4b's acceptance tree
(engine/tests/hubs.rs): `log` called from 12 functions across svc_a / svc_b,
`main` calling 10 steps, 30 tests calling `log`. Shared helpers:
test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ANSWER_KEYS = ["fan_in", "fan_out", "cross_service", "nodes", "edges", "p99_in", "p99_out", "absence"]
ROW_KEYS = ["qname", "kind", "file", "line", "label", "fan_in", "fan_out", "by_category",
            "caller_services", "callee_services", "authority", "hub", "live", "tier"]


def handlers(prefix: str) -> str:
    return "from util.log import log\n\n" + "".join(
        f"\ndef {prefix}_handle_{i}():\n    return log(\"{prefix}{i}\")\n\n" for i in range(1, 7))


def fixture(top: pathlib.Path) -> None:
    steps = "".join(f"def step_{i}():\n    return {i}\n\n\n" for i in range(1, 11))
    names = [f"step_{i}" for i in range(1, 11)]
    main = (f"from svc_a.steps import {', '.join(names)}\n\n\ndef main():\n"
            + "".join(f"    {n}()\n" for n in names))
    tests = "from util.log import log\n\n" + "".join(
        f"\ndef test_log_{i:02}():\n    assert log(\"t{i}\") == \"t{i}\"\n\n" for i in range(1, 31))
    files = {
        "util/log.py": "def log(msg):\n    return msg\n",
        "svc_a/handlers.py": handlers("a"),
        "svc_b/handlers.py": handlers("b"),
        "svc_a/steps.py": steps,
        "svc_a/main.py": main,
        "tests/test_log.py": tests,
    }
    for rel, src in files.items():
        path = top / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(src)


def qnames(rows) -> list[str]:
    return [r.get("qname") for r in rows]


def main() -> int:
    c = Checks("hubs")
    c.check("hubs signature",
            params(rg.PyGraph.hubs)
            == [("scope", None), ("top", 20), ("category", None), ("min_degree", 5), ("include_tests", False)],
            params(rg.PyGraph.hubs))
    with tempfile.TemporaryDirectory(prefix="glia-surface-hubs-") as tmp:
        top = pathlib.Path(tmp) / "hubapp"
        fixture(top)
        g = rg.generate(str(top))

        h, err = stderr_of(g.hubs)
        c.check("marker", "[hubs] nodes=" in err and " hits_iters=20 surface=py" in err, err[-400:])
        c.check("hubs -> dict in engine field order", type(h) is dict and list(h) == ANSWER_KEYS, h)
        fan_in = h.get("fan_in", [])
        log = fan_in[0] if fan_in else {}
        c.check("row keys in engine field order", list(log) == ROW_KEYS, list(log))
        c.check("fan_in[0] is log", (log.get("qname"), log.get("label"), log.get("fan_in"), log.get("fan_out"))
                == ("util::log::log", "utility", 12, 0), log)
        c.check("log located 1-based", (log.get("file"), log.get("line"), log.get("kind"))
                == ("util/log.py", 1, "FUNCTION"), log)
        c.check("by_category is [name, in, out] lists", log.get("by_category") == [["CALLS", 12, 0]],
                log.get("by_category"))
        c.check("caller services", log.get("caller_services") == ["svc_a", "svc_b"]
                and log.get("callee_services") == [], log)
        c.check("HITS scores are floats", isinstance(log.get("authority"), float)
                and log.get("authority", 0) > 0 and log.get("hub") == 0.0, log)
        c.check("tier derived", log.get("tier") == "derived", log)
        c.check("no test node", not any(q.startswith("tests::") for q in qnames(fan_in)), qnames(fan_in))
        fan_out = h.get("fan_out", [])
        main_row = fan_out[0] if fan_out else {}
        c.check("fan_out[0] is main", (main_row.get("qname"), main_row.get("label"), main_row.get("fan_out"),
                                       main_row.get("live")) == ("svc_a::main::main", "orchestrator", 10, True),
                main_row)
        c.check("log joins two services", "util::log::log" in qnames(h.get("cross_service", [])),
                qnames(h.get("cross_service", [])))
        c.check("counts and thresholds are ints",
                all(isinstance(h.get(k), int) for k in ("nodes", "edges", "p99_in", "p99_out")), h)
        c.check("absence None", h.get("absence", 1) is None, h.get("absence"))

        with_tests = g.hubs(include_tests=True)
        log_t = next((r for r in with_tests["fan_in"] if r["qname"] == "util::log::log"), {})
        c.check("include_tests counts the test calls", log_t.get("fan_in") == 42, log_t)
        c.check("category CALLS", g.hubs(category="CALLS")["fan_in"] == fan_in)
        c.check("category case ignored", g.hubs(category="calls")["fan_in"] == fan_in)
        cut = g.hubs(include_tests=True, top=1)
        c.check("top cuts each list", all(len(cut[k]) <= 1 for k in ("fan_in", "fan_out", "cross_service")), cut)
        scoped = g.hubs(scope="svc_a")
        c.check("scope keeps rows under it", "svc_a::main::main" in qnames(scoped["fan_out"])
                and "util::log::log" not in qnames(scoped["fan_in"]), scoped)
        unknown = g.hubs(category="NO_SUCH")
        c.check("unknown category -> no_match absence",
                (unknown.get("absence") or {}).get("reason") == "no_match"
                and "NO_SUCH" in (unknown.get("absence") or {}).get("note", "")
                and unknown["fan_in"] == [], unknown)
        none = g.hubs(min_degree=50, scope="svc_b")
        c.check("no rows -> no_match absence", (none.get("absence") or {}).get("reason") == "no_match", none)
        c.check("deterministic", g.hubs(include_tests=True, top=0) == g.hubs(include_tests=True, top=0))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
