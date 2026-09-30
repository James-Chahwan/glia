#!/usr/bin/env python3
"""pyo3 surface, py/src/review.rs (CC.6b): the module function
`review_vs_rev(repo_path, base="HEAD", format="dict", markdown_rows=20,
depth=4, max_tests=50, max_impact=50)` builds the git rev `base` and the
working tree of the repo at `repo_path` and returns the engine's review as a
native dict {base, counts, changed, impact, tests, edges, new_violations,
resolved_violations, check_errors, blocking} (`blocking` is True only for a
violation the base did not have), or with format="markdown" the markdown PR
report `glia review` prints, as a str. Any other format, or an engine error,
raises ValueError. Needs a `git` binary. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

MANIFESTS = {
    "web/pyproject.toml": "[project]\nname = \"web\"\n",
    "services/api/pyproject.toml": "[project]\nname = \"api\"\n",
}
INTERNAL = "def charge(o):\n    return o\n"
INTERNAL_EDITED = "def charge(o):\n    return o or 0\n"
WEB_APP_CLEAN = "def pay(o):\n    return o\n"
WEB_APP = "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n"
TEST_PAY = "from web.app import pay\n\n\ndef test_pay():\n    assert pay(1) == 1\n"
RULES = ("version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\n"
         "from = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n")
KEYS = ["base", "counts", "changed", "impact", "tests", "edges", "new_violations",
        "resolved_violations", "check_errors", "blocking"]
KW = [("repo_path", None), ("base", "HEAD"), ("format", "dict"), ("markdown_rows", 20), ("depth", 4),
      ("max_tests", 50), ("max_impact", 50)]
MARKER_TAIL = " edges +2 -0 (fact=2 derived=0 heuristic=0) violations new=1 resolved=0 blocking=true"
ROW = "| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact |"


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    subprocess.run(["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def committed(top: pathlib.Path, web_app: str) -> None:
    """CC.6a's tree with `web_app` as web/app.py, committed on main."""
    files = {**MANIFESTS, "services/api/internal.py": INTERNAL, "web/app.py": web_app,
             "tests/test_pay.py": TEST_PAY, ".glia/overlay.toml": RULES}
    for rel, text in files.items():
        p = top / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    git(top, "init", "-q")
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "clean")


def main() -> int:
    c = Checks("review")
    fn = getattr(rg, "review_vs_rev", None)
    c.check("review_vs_rev exists", fn is not None)
    if fn is None:
        return c.done()
    c.check("review_vs_rev params", params(fn) == KW, params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-review-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"

        top = pathlib.Path(tmp) / "shop"
        committed(top, WEB_APP_CLEAN)
        (top / "web/app.py").write_text(WEB_APP)
        d, err = stderr_of(lambda: rg.review_vs_rev(str(top)))
        c.check("-> dict in engine field order", type(d) is dict and list(d) == KEYS, type(d) is dict and list(d))
        d = d if type(d) is dict else {}
        c.check("base as given", d.get("base") == "HEAD", d.get("base"))
        c.check("a new violation blocks", d.get("blocking") is True, d.get("blocking"))
        v = (d.get("new_violations") or [{}])[0]
        c.check("the new violation",
                (v.get("rule_id"), v.get("rule_kind"), v.get("decl"), v.get("count"))
                == ("web-no-api-internals", "forbid_edge", ".glia/overlay.toml:3", 2), v)
        rows = [(e.get("category"), e.get("file"), e.get("line")) for e in v.get("evidence", [])]
        c.check("its rows, 1-based", rows == [("IMPORTS", "web/app.py", 1), ("CALLS", "web/app.py", 5)], rows)
        c.check("nothing resolved", d.get("resolved_violations") == [], d.get("resolved_violations"))
        edges = [(e.get("change"), e.get("category"), e.get("tier")) for e in d.get("edges", [])]
        c.check("edge rows, facts first", edges == [("added", "CALLS", "fact"), ("added", "IMPORTS", "fact")],
                edges)
        counts = d.get("counts") or {}
        c.check("counts", (counts.get("new_violations"), counts.get("edges_added"), counts.get("tests"))
                == (1, 2, 1), counts)
        marker = [line for line in err.splitlines() if line.startswith("[review] base=")]
        c.check("engine marker", len(marker) == 1 and marker[0].endswith(MARKER_TAIL), err[-600:])
        c.check("surface marker", "[review] surface=py format=dict" in err, err[-400:])

        md, err = stderr_of(lambda: rg.review_vs_rev(str(top), format="markdown"))
        c.check("markdown is a str", type(md) is str, type(md))
        md = md if type(md) is str else ""
        c.check("markdown headline", md.startswith("## glia review vs `HEAD`\n**1 new violation(s)**"), md[:200])
        c.check("markdown blocking section", "### New violations (blocking)" in md, md)
        c.check("markdown violation row", ROW in md, md)
        c.check("markdown surface marker", "[review] surface=py format=markdown" in err, err[-400:])
        cut = rg.review_vs_rev(str(top), format="markdown", markdown_rows=1)
        c.check("markdown_rows cuts the tables",
                [line for line in cut.splitlines() if line.startswith("_(")] == ["_(1 of 2)_", "_(1 of 2)_",
                                                                                 "_(1 of 3)_"], cut)
        capped = rg.review_vs_rev(str(top), max_tests=0, max_impact=0, depth=1)
        c.check("max_tests / max_impact cut the rows, not the counts",
                capped["tests"]["tests"] == [] and capped["impact"]["results"] == []
                and capped["counts"] == d.get("counts"), capped.get("counts"))

        c.raises("format=xml raises", ValueError, lambda: rg.review_vs_rev(str(top), format="xml"),
                 "unknown format `xml`: expected one of dict, markdown")
        c.raises("unknown rev raises", ValueError, lambda: rg.review_vs_rev(str(top), base="no-such-rev"),
                 "no-such-rev")

        old = pathlib.Path(tmp) / "old"
        committed(old, WEB_APP)
        (old / "services/api/internal.py").write_text(INTERNAL_EDITED)
        pre = rg.review_vs_rev(str(old))
        c.check("a pre-existing violation does not block",
                pre.get("blocking") is False and pre.get("new_violations") == []
                and pre.get("resolved_violations") == [], pre)

        plain = pathlib.Path(tmp) / "plain"
        plain.mkdir()
        c.raises("not a git work tree raises", ValueError, lambda: rg.review_vs_rev(str(plain)),
                 "not a git work tree")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
