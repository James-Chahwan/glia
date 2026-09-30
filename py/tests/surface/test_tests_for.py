#!/usr/bin/env python3
"""pyo3 surface, py/src/tests_for.rs (LE.3b, CC.9a): `PyGraph.tests_for(qnames,
depth=6, scope=None, module_level=True, limit=None, signals=True)`,
`PyGraph.tests_for_diff(diff_text, ...)` and the module function
`tests_for_rev(repo_path, base="HEAD", ...)` return the tests to run for a
change as a native dict {seeds, tests, omitted, test_files, untested,
unresolved, absence}, every row tiered fact / derived / heuristic, carrying
its signals and co-change confidence, and located on a 1-based line; an
engine error raises ValueError. `tests_for_rev` needs a `git` binary. Shared
helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

SERVICE = ('def price(order):\n    return sum(i["p"] for i in order["items"])\n\n\n'
           'def place(order):\n    total = price(order)\n    return {"total": total}\n')
FILES = {
    "shop/orders/service.py": SERVICE,
    "shop/orders/audit.py": ("from shop.orders.service import place\n\n\n"
                             "def audited_place(order):\n    return place(order)\n"),
    "shop/tests/test_service.py": ("from shop.orders.service import price\n\n\n"
                                   'def test_price():\n    assert price({"items": [{"p": 2}]}) == 2\n'),
    "shop/tests/test_audit.py": ("from shop.orders.audit import audited_place\n\n\n"
                                 'def test_audited_place():\n    assert audited_place({"items": []})["total"] == 0\n'),
}
PRICE = "shop::orders::service::price"
KEYS = ["seeds", "tests", "omitted", "test_files", "untested", "unresolved", "absence"]
ROW_KEYS = ["qname", "name", "kind", "file", "line", "tier", "reason", "depth", "covers", "path",
            "signals", "cochange_permille"]
SHAPE = [("shop::tests::test_service::test_price", "fact", 1),
         ("shop::tests::test_audit::test_audited_place", "derived", 3),
         ("shop::tests::test_service", "heuristic", 2)]
TWO_FILES = ["shop/tests/test_audit.py", "shop/tests/test_service.py"]
MARKER = "[tests-for] seeds=1 tests=3 fact=1 derived=1 heuristic=1 untested=0 files=2"
SIGNALS = "[tests-for] signals failed_last_run=0 on_failing_trace=0 cochange=0 cochange_only=0 omitted=0"
DIFF = ("--- a/shop/orders/service.py\n+++ b/shop/orders/service.py\n@@ -1,2 +1,2 @@\n"
        " def price(order):\n"
        '-    return sum(i["p"] for i in order["items"])\n'
        '+    return sum(i["p"] * 1 for i in order["items"])\n')
KW = [("depth", 6), ("scope", None), ("module_level", True), ("limit", None), ("signals", True)]


def shape(d) -> list:
    return [(t.get("qname"), t.get("tier"), t.get("depth")) for t in d.get("tests", [])] \
        if type(d) is dict else d


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    subprocess.run(["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def main() -> int:
    c = Checks("tests_for")
    method = getattr(rg.PyGraph, "tests_for", None)
    diff_method = getattr(rg.PyGraph, "tests_for_diff", None)
    fn = getattr(rg, "tests_for_rev", None)
    c.check("PyGraph.tests_for exists", method is not None)
    c.check("PyGraph.tests_for_diff exists", diff_method is not None)
    c.check("tests_for_rev exists", fn is not None)
    if method is None or diff_method is None or fn is None:
        return c.done()
    c.check("tests_for params", params(method) == [("qnames", None), *KW], params(method))
    c.check("tests_for_diff params", params(diff_method) == [("diff_text", None), *KW], params(diff_method))
    c.check("tests_for_rev params", params(fn) == [("repo_path", None), ("base", "HEAD"), *KW], params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-tests-for-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "shop"
        for rel, text in FILES.items():
            (top / rel).parent.mkdir(parents=True, exist_ok=True)
            (top / rel).write_text(text)
        g = rg.generate(str(top))

        d, err = stderr_of(lambda: g.tests_for([PRICE]))
        c.check("tests_for -> dict in field order", type(d) is dict and list(d) == KEYS,
                type(d) is dict and list(d))
        c.check("rows: fact, derived, heuristic", shape(d) == SHAPE, shape(d))
        c.check("row keys in engine field order", type(d) is dict and d["tests"] and list(d["tests"][0]) == ROW_KEYS,
                type(d) is dict and d.get("tests"))
        first = d["tests"][0] if type(d) is dict and d.get("tests") else {}
        c.check("fact row located, 1-based", (first.get("file"), first.get("line"))
                == ("shop/tests/test_service.py", 4), first)
        c.check("witness hops are [qname, category]", first.get("path") == [[PRICE, "TESTS"]], first)
        c.check("test files", type(d) is dict and d.get("test_files") == TWO_FILES, d)
        c.check("no absence", type(d) is dict and d.get("absence") is None, d)
        c.check("engine marker", MARKER in err, err[-400:])
        c.check("signals marker", SIGNALS in err, err[-400:])
        c.check("no signal on any row", type(d) is dict and all(t.get("signals") == [] and t.get("cochange_permille")
                                                                  is None for t in d.get("tests", [])), d)
        c.check("nothing omitted", type(d) is dict and d.get("omitted") == 0, d)

        c.check("bare name, no module level",
                shape(g.tests_for(["price"], module_level=False)) == SHAPE[:2])
        c.check("depth bound", shape(g.tests_for([PRICE], depth=2, module_level=False)) == SHAPE[:1])
        c.check("scope", shape(g.tests_for([PRICE], scope="shop/tests/test_audit.py")) == SHAPE[1:2])
        top1 = g.tests_for([PRICE], limit=1)
        c.check("limit keeps the first row", shape(top1) == SHAPE[:1] and top1.get("omitted") == 2
                and top1.get("test_files") == ["shop/tests/test_service.py"], top1)
        c.raises("limit=0 raises", ValueError, lambda: g.tests_for([PRICE], limit=0), "limit of 0")
        plain, err = stderr_of(lambda: g.tests_for([PRICE], signals=False))
        c.check("signals=False: same rows, no signals line", shape(plain) == SHAPE
                and "[tests-for] signals" not in err, err[-400:])
        nobody = g.tests_for(["no_such_symbol"])
        c.check("unknown name -> unresolved + absence",
                nobody.get("unresolved") == ["no_such_symbol"]
                and (nobody.get("absence") or {}).get("reason") == "unknown_symbol", nobody)
        c.raises("empty qnames raises", ValueError, lambda: g.tests_for([]), "no seed")

        dd = g.tests_for_diff(DIFF)
        c.check("diff mode equals qname mode", shape(dd) == SHAPE and dd.get("seeds") == [PRICE], dd)

        git(top, "init", "-q")
        git(top, "add", "-A")
        git(top, "commit", "-q", "-m", "shop")
        (top / "shop/orders/service.py").write_text(SERVICE.replace('i["p"] for', 'i["p"] * 1 for'))
        r, err = stderr_of(lambda: rg.tests_for_rev(str(top)))
        c.check("rev mode equals qname mode", shape(r) == SHAPE and r.get("seeds") == [PRICE], r)
        c.check("rev mode marker", MARKER in err, err[-400:])
        c.raises("unknown rev raises", ValueError, lambda: rg.tests_for_rev(str(top), base="no-such-rev"),
                 "no-such-rev")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
