#!/usr/bin/env python3
"""pyo3 surface, py/src/delta.rs (LE.1c): the module function
`graph_delta(repo_path, base="HEAD")` builds the git rev `base` and the working
tree of the repo at `repo_path` and returns their graph delta as a native dict
{base, files, counts, nodes, edges}, every row located on a 1-based line; it
raises ValueError on a git or build failure. It saves the parse-cache sidecar,
never a layout. Needs a `git` binary. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

A_PY = "def place(o):\n    return price(o)\n\n\ndef price(o):\n    return o\n"
B_PY = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n"
A_PY_AUDIT = ("def place(o):\n    audit(o)\n    return price(o)\n\n\n"
              "def price(o):\n    return o\n\n\ndef audit(o):\n    return o\n")
KEYS = ["base", "files", "counts", "nodes", "edges"]


def git(top: pathlib.Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    return subprocess.run(
        ["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
         "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
        env=env, check=True, capture_output=True, text=True).stdout.strip()


def shop(top: pathlib.Path) -> None:
    """A committed two-file python repo: `place` calls `price`."""
    (top / "shop").mkdir(parents=True)
    (top / "shop" / "a.py").write_text(A_PY)
    (top / "shop" / "b.py").write_text(B_PY)
    git(top, "init", "-q")
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "shop")


def main() -> int:
    c = Checks("delta")
    fn = getattr(rg, "graph_delta", None)
    c.check("graph_delta exists", fn is not None)
    if fn is None:
        return c.done()
    c.check("graph_delta params", params(fn) == [("repo_path", None), ("base", "HEAD")], params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-delta-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "shop"
        shop(top)

        d, err = stderr_of(lambda: rg.graph_delta(str(top)))
        c.check("clean tree -> dict", type(d) is dict, type(d))
        c.check("keys in field order", type(d) is dict and list(d) == KEYS, type(d) is dict and list(d))
        c.check("clean tree is an empty delta",
                type(d) is dict and d.get("nodes") == [] and d.get("edges") == [], d)
        c.check("engine marker", "[delta] base=HEAD" in err, err[-400:])
        c.check("surface marker, clean", "[delta] surface=pyo3 rows=0" in err, err[-400:])

        (top / "shop" / "a.py").write_text(A_PY_AUDIT)
        d, err = stderr_of(lambda: rg.graph_delta(str(top), base="HEAD"))
        edges = d.get("edges", []) if type(d) is dict else []
        nodes = d.get("nodes", []) if type(d) is dict else []
        call = next((e for e in edges if e.get("change") == "added" and e.get("category") == "CALLS"
                     and str(e.get("to_qname", "")).endswith("::audit")), None)
        c.check("added call reported", call is not None, edges)
        c.check("call site 1-based",
                call is not None and (call.get("site_file"), call.get("site_line")) == ("shop/a.py", 2), call)
        audit = next((n for n in nodes if n.get("change") == "added"
                      and str(n.get("qname", "")).endswith("::audit")), None)
        c.check("added function located",
                audit is not None and (audit.get("kind"), audit.get("file"), audit.get("line"))
                == ("FUNCTION", "shop/a.py", 10), audit)
        c.check("node ids are ints", all(type(n.get("id")) is int for n in nodes), nodes[:2])
        c.check("surface marker counts the rows",
                f"[delta] surface=pyo3 rows={len(nodes) + len(edges)}" in err, err[-400:])
        graph_dir = top / ".glia" / "graph"
        c.check("parse cache saved", (graph_dir / "parse_cache.bin").is_file())
        c.check("no layout written", not (graph_dir / "manifest.json").exists())
        # (`git` strips its output, so the porcelain ` M` loses its space.)
        status = git(top, "status", "--porcelain", "--untracked-files=all")
        c.check("the sidecar is self-ignored: git status shows the edit only",
                status == "M shop/a.py", status)

        c.raises("unknown rev raises", ValueError, lambda: rg.graph_delta(str(top), base="no-such-rev"),
                 "unknown rev no-such-rev")
        plain = pathlib.Path(tmp) / "plain"
        plain.mkdir()
        (plain / "app.py").write_text("def f():\n    return 1\n")
        c.raises("outside git raises", ValueError, lambda: rg.graph_delta(str(plain)),
                 "not a git work tree")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
