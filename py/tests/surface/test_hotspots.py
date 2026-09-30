#!/usr/bin/env python3
"""pyo3 surface, py/src/hotspots.rs (CC.10b): `hotspots(level="both", top=20,
min_churn=2, include_tests=False, scope=None)` returns the hotspot answer
`{modules, symbols, history_head, absence}` as a native dict; an unknown level
raises ValueError. The history is real: a git repo whose commits are synced
with `history_sync(..., blame=True)` before the build. Shared helpers:
test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

T0, DAY = 1_767_225_600, 86_400  # 2026-01-01T00:00:00Z
IDENTITY = {
    "GIT_AUTHOR_NAME": "Hotspot Fixture",
    "GIT_AUTHOR_EMAIL": "hotspot.fixture@identity.invalid",
    "GIT_COMMITTER_NAME": "Hotspot Fixture",
    "GIT_COMMITTER_EMAIL": "hotspot.fixture@identity.invalid",
}

# CC.10a's acceptance tree: `helper` is called by five functions across three
# files; scripts/once.py calls and is called by nothing.
A_PY = ("from core.util import helper\n\n\ndef get_a(x):\n    return helper(x)\n\n\n"
        "def list_a(xs):\n    return [helper(x) for x in xs]\n")
B_PY = ("from core.util import helper\n\n\ndef get_b(x):\n    return helper(x) * 3\n\n\n"
        "def list_b(xs):\n    return [helper(x) - 1 for x in xs]\n")
ONCE_PY = "def run_once(n):\n    acc = 0\n    for i in range(n):\n        acc += i\n    return acc\n"

# Commit k (1 = oldest) touches these files: once.py 8 commits, util.py 6,
# a.py 4, b.py 1 (below min_churn=2).
TOUCHES = {
    1: ["scripts/once.py", "core/util.py", "api/a.py", "api/b.py"],
    2: ["scripts/once.py", "core/util.py"],
    3: ["scripts/once.py", "core/util.py", "api/a.py"],
    4: ["scripts/once.py", "core/util.py", "api/a.py"],
    5: ["scripts/once.py", "core/util.py", "api/a.py"],
    6: ["scripts/once.py", "core/util.py"],
    7: ["scripts/once.py"],
    8: ["scripts/once.py"],
}
ROW_KEYS = ["level", "qname", "kind", "file", "line", "churn", "lines_changed", "last_change",
            "churn_rank", "centrality_rank", "ranked", "tier"]


def util_py(k: int) -> str:
    """helper's third line changes in every commit that touches util.py, so
    its span blames to two times (commit 1 and the last util.py commit)."""
    return (f"def helper(x):\n    total = x + 1\n    total = total * {k}\n    return total\n\n\n"
            "def wrap(x):\n    return helper(x) + 1\n")


def git(top: pathlib.Path, *args: str, t: int = T0) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    env.update(IDENTITY, GIT_AUTHOR_DATE=f"@{t} +0000", GIT_COMMITTER_DATE=f"@{t} +0000")
    subprocess.run(["git", "-C", str(top), *args], env=env, check=True, capture_output=True, text=True)


def history_repo(top: pathlib.Path) -> None:
    """The acceptance tree committed as TOUCHES says, one day apart."""
    top.mkdir(parents=True)
    git(top, "init", "-q", "-b", "main")
    base = {"api/a.py": A_PY, "api/b.py": B_PY, "scripts/once.py": ONCE_PY}
    for k, paths in TOUCHES.items():
        for rel in paths:
            path = top / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            if rel == "core/util.py":
                path.write_text(util_py(k))
            elif k == 1:
                path.write_text(base[rel])
            else:
                with open(path, "a") as f:
                    f.write(f"# edit {k}\n")
        git(top, "add", "-A", t=T0 + k * DAY)
        git(top, "commit", "-q", "-m", f"commit {k}", t=T0 + k * DAY)


def main() -> int:
    c = Checks("hotspots")
    c.check("hotspots signature",
            params(rg.PyGraph.hotspots)
            == [("level", "both"), ("top", 20), ("min_churn", 2), ("include_tests", False), ("scope", None)],
            params(rg.PyGraph.hotspots))
    with tempfile.TemporaryDirectory(prefix="glia-surface-hotspots-") as tmp:
        top = pathlib.Path(tmp) / "shop"
        history_repo(top)

        bare = rg.generate(str(top))
        h, err = stderr_of(bare.hotspots)
        c.check("no-history marker", "[hotspots] modules=0/0 symbols=0/0 pagerank_iterations=0 head=-" in err,
                err[-400:])
        c.check("no history -> absence", h.get("modules") == [] and h.get("history_head") is None
                and (h.get("absence") or {}).get("reason") == "no_history", h)
        c.check("absence names history sync", "glia history sync" in (h.get("absence") or {}).get("note", ""),
                h.get("absence"))

        rg.history_sync(str(top), blame=True)
        g = rg.generate(str(top))
        h, err = stderr_of(g.hotspots)
        c.check("marker", "[hotspots] modules=3/3 symbols=1/1 pagerank_iterations=" in err
                and f"head={T0 + 8 * DAY}" in err, err[-400:])
        c.check("hotspots -> dict in engine field order",
                type(h) is dict and list(h) == ["modules", "symbols", "history_head", "absence"], h)
        mods = h.get("modules", [])
        c.check("modules[0] is core::util", mods and mods[0].get("qname") == "core::util",
                [m.get("qname") for m in mods])
        c.check("module order", [m.get("qname") for m in mods] == ["core::util", "scripts::once", "api::a"],
                [m.get("qname") for m in mods])
        c.check("both ranks and the population",
                [(m["churn_rank"], m["centrality_rank"], m["ranked"]) for m in mods]
                == [(2, 1, 3), (1, 3, 3), (3, 2, 3)], mods)
        util = mods[0] if mods else {}
        c.check("row keys in engine field order", list(util) == ROW_KEYS, list(util))
        c.check("util row", (util.get("churn"), util.get("file"), util.get("line"), util.get("last_change"),
                             util.get("tier")) == (6, "core/util.py", 1, T0 + 6 * DAY, "heuristic"), util)
        syms = h.get("symbols", [])
        c.check("blamed symbol", [(s.get("qname"), s.get("churn"), s.get("line")) for s in syms]
                == [("core::util::helper", 2, 1)], syms)
        c.check("history_head is int unix seconds", h.get("history_head") == T0 + 8 * DAY, h.get("history_head"))
        c.check("absence None", h.get("absence", 1) is None, h.get("absence"))

        c.check("level=symbol", g.hotspots(level="symbol")["modules"] == []
                and len(g.hotspots("symbol")["symbols"]) == 1)
        cut = g.hotspots(level="module", top=1)
        c.check("top cuts rows, not ranks", [(m["qname"], m["ranked"]) for m in cut["modules"]]
                == [("core::util", 3)] and cut["symbols"] == [], cut)
        c.check("top=0 keeps every row", len(g.hotspots(top=0)["modules"]) == 3)
        c.check("min_churn=1 ranks b.py", g.hotspots(level="module", min_churn=1)["modules"][0]["ranked"] == 4)
        c.check("scope", [m["qname"] for m in g.hotspots(scope="api")["modules"]] == ["api::a"])
        c.check("include_tests keeps the same rows here",
                g.hotspots(include_tests=True)["modules"] == mods)
        miss = g.hotspots(min_churn=99)
        c.check("no_match", (miss.get("absence") or {}).get("reason") == "no_match"
                and miss["history_head"] == T0 + 8 * DAY, miss)
        c.raises("unknown level", ValueError, lambda: g.hotspots(level="modules"), "module, symbol, both")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
