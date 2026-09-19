#!/usr/bin/env python3
"""pyo3 surface, py/src/diff_impact.rs (LE.2): `PyGraph.diff_impact(diff_text,
direction="both", depth=4, top_k=None, live_only=False, scope=None)` and the
module function `diff_impact_vs_rev(repo_path, base="HEAD", ...)` return what
a change affects as a native dict {base, changed, edges_added, edges_removed,
impact, unresolved_diff_files}: the changed nodes, each marked as a seed or
not, and ONE blast radius around the seeds whose rows each name the seed that
reached them; lines are 1-based; a bad direction or an engine error raises
ValueError. `diff_impact_vs_rev` needs a `git` binary. Shared helpers:
test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

A_PY = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n"
A_PY_EDITED = A_PY.replace("return o\n", "return o or 0\n", 1)
A_PY_NO_CALL = A_PY.replace("return price(o)\n", "return o\n")
B_PY = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n"
PRICE = "shop::a::price"
PLACE = "shop::a::place"
CHECKOUT = "shop::b::checkout"
KEYS = ["base", "changed", "edges_added", "edges_removed", "impact", "unresolved_diff_files"]
CHANGED_KEYS = ["id", "qname", "kind", "file", "line", "change", "seed"]
IMPACT_KEYS = ["seeds", "unresolved", "results", "absence"]
ROWS = [(PLACE, 1, PRICE), (CHECKOUT, 2, PRICE)]
DIFF = ("--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,2 +1,2 @@\n"
        " def price(o):\n"
        "-    return o\n"
        "+    return o or 0\n")
DELETION = ("--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,3 +1,2 @@\n"
            " def price(o):\n"
            "-    o = o\n"
            "     return o\n")
KW = [("direction", "both"), ("depth", 4), ("top_k", None), ("live_only", False), ("scope", None)]
DIFF_MARKER = "[diff-impact] mode=diff base=- changed=1 seeds=1 impact=2 edges +0 -0 unresolved_files=0"
REV_MARKER = "[diff-impact] mode=rev base=HEAD changed=2 seeds=1 impact=2 edges +0 -0 unresolved_files=0"


def rows(d) -> list:
    if type(d) is not dict:
        return d
    return sorted((r.get("qname"), r.get("depth"), r.get("seed"))
                  for r in d.get("impact", {}).get("results", []))


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    subprocess.run(["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def main() -> int:
    c = Checks("diff_impact")
    method = getattr(rg.PyGraph, "diff_impact", None)
    fn = getattr(rg, "diff_impact_vs_rev", None)
    c.check("PyGraph.diff_impact exists", method is not None)
    c.check("diff_impact_vs_rev exists", fn is not None)
    if method is None or fn is None:
        return c.done()
    c.check("diff_impact params", params(method) == [("diff_text", None), *KW], params(method))
    c.check("diff_impact_vs_rev params", params(fn) == [("repo_path", None), ("base", "HEAD"), *KW], params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-diff-impact-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "shop"
        (top / "shop").mkdir(parents=True)
        (top / "shop/a.py").write_text(A_PY_EDITED)
        (top / "shop/b.py").write_text(B_PY)
        g = rg.generate(str(top))

        d, err = stderr_of(lambda: g.diff_impact(DIFF, direction="backward"))
        c.check("diff_impact -> dict in field order", type(d) is dict and list(d) == KEYS,
                type(d) is dict and list(d))
        c.check("pasted mode has no base", type(d) is dict and d.get("base") is None, d)
        changed = d.get("changed", []) if type(d) is dict else []
        c.check("changed row keys in engine field order", bool(changed) and list(changed[0]) == CHANGED_KEYS,
                changed)
        first = changed[0] if changed else {}
        c.check("the edited function is a seed",
                (first.get("qname"), first.get("change"), first.get("seed"), first.get("line"))
                == (PRICE, "diff_hit", True, 1), first)
        impact = d.get("impact", {}) if type(d) is dict else {}
        c.check("impact is blast_radius's dict", list(impact) == IMPACT_KEYS, list(impact))
        c.check("rows attributed to the seed", rows(d) == ROWS, rows(d))
        ids = [r.get("id") for r in impact.get("results", [])]
        c.check("ids are ints", bool(ids) and all(type(i) is int for i in ids), ids)
        c.check("engine marker", DIFF_MARKER in err, err[-400:])

        c.check("depth bound", rows(g.diff_impact(DIFF, direction="backward", depth=1)) == ROWS[:1])
        c.check("top_k", len(g.diff_impact(DIFF, direction="backward", top_k=1)["impact"]["results"]) == 1)
        c.check("scope", rows(g.diff_impact(DIFF, direction="backward", scope="shop/b.py")) == ROWS[1:])
        gone = g.diff_impact(DELETION)
        c.check("deletion-only hunk names its file",
                gone.get("unresolved_diff_files") == ["shop/a.py"] and not gone.get("changed")
                and (gone.get("impact", {}).get("absence") or {}).get("reason") == "no_match", gone)
        c.raises("bad direction raises", ValueError, lambda: g.diff_impact(DIFF, direction="sideways"),
                 "sideways")

        (top / "shop/a.py").write_text(A_PY)
        git(top, "init", "-q")
        git(top, "add", "-A")
        git(top, "commit", "-q", "-m", "shop")
        (top / "shop/a.py").write_text(A_PY_EDITED)
        r, err = stderr_of(lambda: rg.diff_impact_vs_rev(str(top), direction="backward"))
        c.check("rev mode names its base", type(r) is dict and r.get("base") == "HEAD", r)
        c.check("rev mode equals pasted mode", rows(r) == ROWS, rows(r))
        c.check("rev mode marker", REV_MARKER in err, err[-400:])

        (top / "shop/a.py").write_text(A_PY_NO_CALL)
        lost = rg.diff_impact_vs_rev(str(top))
        removed = [(e.get("category"), e.get("from_qname"), e.get("to_qname")) for e in lost.get("edges_removed", [])]
        c.check("a removed call is an edge row", removed == [("CALLS", PLACE, PRICE)], removed)
        c.check("its caller is still in radius", (CHECKOUT, 1, PLACE) in rows(lost), rows(lost))
        c.raises("unknown rev raises", ValueError, lambda: rg.diff_impact_vs_rev(str(top), base="no-such-rev"),
                 "no-such-rev")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
