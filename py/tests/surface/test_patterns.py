#!/usr/bin/env python3
"""pyo3 surface, py/src/patterns.rs (LE.7b, EXPERIMENTAL):
`PyGraph.patterns_experimental(min_support=5, min_share=75, scope=None)` and
the module function `patterns_vs_rev_experimental(repo_path, base="HEAD",
min_support=5, min_share=75, scope=None)` return the pattern-conformance
report as a native dict {experimental, delta_mode, handlers, judged,
skipped_small, excluded, role_sources, populations, divergences}: route
handlers grouped per service, the most frequent role chain as a population's
convention, each handler off it a located DIVERGENCE (lines 1-based). Both
names carry `experimental` and no unsuffixed alias exists. A min_share above
100 or an engine error raises ValueError. The rev function needs a `git`
binary. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

# (name, gin registration, service fn or None for a direct call, repository fn)
HANDLERS = [
    ("GetUserHandler", 'GET("/users/:id"', "GetUser", "FindUser"),
    ("CreateUserHandler", 'POST("/users"', "CreateUser", "InsertUser"),
    ("GetOrderHandler", 'GET("/orders/:id"', "GetOrder", "FindOrder"),
    ("ListProductsHandler", 'GET("/products"', "ListProducts", "AllProducts"),
    ("CreatePaymentHandler", 'POST("/payments"', "CreatePayment", "InsertPayment"),
    ("RawOrderHandler", 'POST("/orders"', None, "SaveOrder"),
]
CONVENTION = "handler>service>repository>db"
DIRECT_SIGNATURE = "handler>repository>db"
DIRECT = "handlers::handlers::RawOrderHandler"
KEYS = ["experimental", "delta_mode", "handlers", "judged", "skipped_small", "excluded", "role_sources",
        "populations", "divergences"]
POP_KEYS = ["service", "role", "size", "status", "convention", "matching", "verdict", "signatures",
            "role_sources", "exceptions"]
DIV_KEYS = ["verdict", "tier", "service", "handler", "file", "line", "route_method", "route_path", "signature",
            "convention", "matching", "population", "path", "role_sources"]
KW = [("min_support", 5), ("min_share", 75), ("scope", None)]
GRAPH_MARKER = "[patterns] experimental surface=pyo3 mode=graph"
DELTA_MARKER = "[patterns] experimental surface=pyo3 mode=delta"


def handlers_go(n: int) -> str:
    hs = HANDLERS[:n]
    s = 'package handlers\n\nimport (\n\t"net/http"\n\n'
    if any(svc is None for _, _, svc, _ in hs):
        s += '\t"example.com/shop/repository"\n'
    s += '\t"example.com/shop/service"\n\t"github.com/gin-gonic/gin"\n)\n\nfunc Register(r *gin.Engine) {\n'
    s += "".join(f"\tr.{route}, {name})\n" for name, route, _, _ in hs)
    s += "}\n"
    for name, _, svc, repo in hs:
        callee = f"service.{svc}" if svc else f"repository.{repo}"
        s += f'\nfunc {name}(c *gin.Context) {{\n\tv := {callee}(c.Param("id"))\n\tc.JSON(http.StatusOK, v)\n}}\n'
    return s


def write_shop(top: pathlib.Path, n: int) -> None:
    hs = HANDLERS[:n]
    service = 'package service\n\nimport "example.com/shop/repository"\n'
    repository = 'package repository\n\nimport "database/sql"\n\nvar db *sql.DB\n'
    for i, (_, _, svc, repo) in enumerate(hs):
        if svc:
            service += f"\nfunc {svc}(id string) string {{\n\treturn repository.{repo}(id)\n}}\n"
        repository += (f'\nfunc {repo}(id string) string {{\n'
                       f'\tdb.Exec("INSERT INTO t{i} (v) VALUES ($1)", id)\n\treturn id\n}}\n')
    for rel, text in [("go.mod", "module example.com/shop\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.1\n"),
                      ("handlers/handlers.go", handlers_go(n)),
                      ("service/service.go", service),
                      ("repository/repository.go", repository)]:
        p = top / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)


def divergent(r) -> list:
    return [d.get("handler") for d in r.get("divergences", [])] if type(r) is dict else r


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    subprocess.run(["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def main() -> int:
    c = Checks("patterns")
    method = getattr(rg.PyGraph, "patterns_experimental", None)
    fn = getattr(rg, "patterns_vs_rev_experimental", None)
    c.check("PyGraph.patterns_experimental exists", method is not None)
    c.check("patterns_vs_rev_experimental exists", fn is not None)
    c.check("no unsuffixed alias", not hasattr(rg.PyGraph, "patterns") and not hasattr(rg, "patterns_vs_rev"))
    if method is None or fn is None:
        return c.done()
    c.check("patterns_experimental params", params(method) == KW, params(method))
    c.check("patterns_vs_rev_experimental params",
            params(fn) == [("repo_path", None), ("base", "HEAD"), *KW], params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-patterns-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "shop"
        write_shop(top, 6)
        g = rg.generate(str(top))

        r, err = stderr_of(lambda: g.patterns_experimental())
        c.check("patterns_experimental -> dict in field order", type(r) is dict and list(r) == KEYS,
                type(r) is dict and list(r))
        r = r if type(r) is dict else {}
        c.check("experimental is True", r.get("experimental") is True, r.get("experimental"))
        c.check("whole-graph mode", r.get("delta_mode") is False)
        c.check("counts", (r.get("handlers"), r.get("judged"), r.get("skipped_small")) == (6, 1, 0), r)
        c.check("nothing excluded", r.get("excluded") == {}, r.get("excluded"))
        pops = r.get("populations", [])
        pop = pops[0] if pops else {}
        c.check("population keys in engine field order", list(pop) == POP_KEYS, list(pop))
        c.check("population judged 5/6",
                (pop.get("service"), pop.get("status"), pop.get("verdict"), pop.get("convention"))
                == ("handlers", "judged", "5/6", CONVENTION), pop)
        c.check("signatures are pairs",
                pop.get("signatures") == [[CONVENTION, 5], [DIRECT_SIGNATURE, 1]], pop.get("signatures"))
        c.check("one divergence", divergent(r) == [DIRECT], divergent(r))
        d = (r.get("divergences") or [{}])[0]
        c.check("divergence keys in engine field order", list(d) == DIV_KEYS, list(d))
        line = handlers_go(6).splitlines().index("func RawOrderHandler(c *gin.Context) {") + 1
        c.check("divergence located (1-based)",
                (d.get("file"), d.get("line"), d.get("route_method"), d.get("route_path"))
                == ("handlers/handlers.go", line, "POST", "/orders"), d)
        c.check("divergence is heuristic",
                (d.get("verdict"), d.get("tier"), d.get("signature")) == ("DIVERGENCE", "heuristic", DIRECT_SIGNATURE), d)
        hops = [(h.get("from_qname"), h.get("category")) for h in d.get("path", [])]
        c.check("path hops", hops == [(DIRECT, "CALLS"), ("repository::repository::SaveOrder", "ACCESSES_DATA")], hops)
        c.check("surface marker", GRAPH_MARKER in err, err[-400:])
        c.check("engine marker", "[patterns] experimental populations=1 judged=1 handlers=6 divergences=1" in err,
                err[-400:])

        small = g.patterns_experimental(min_support=7)
        c.check("min_support: too small", small["skipped_small"] == 1
                and small["populations"][0]["status"] == "too_small" and not small["divergences"], small)
        split = g.patterns_experimental(min_share=90)
        c.check("min_share: no convention",
                split["populations"][0]["status"] == "no_convention" and not split["divergences"], split)
        scoped = g.patterns_experimental(scope="service")
        c.check("scope excludes handlers outside it",
                scoped["excluded"] == {"out_of_scope": 6} and scoped["handlers"] == 0, scoped)
        c.raises("min_share above 100 raises", ValueError, lambda: g.patterns_experimental(min_share=101), "101")

        rev_top = pathlib.Path(tmp) / "revshop"
        write_shop(rev_top, 5)
        git(rev_top, "init", "-q")
        git(rev_top, "add", "-A")
        git(rev_top, "commit", "-q", "-m", "five layered handlers")
        write_shop(rev_top, 6)
        rv, err = stderr_of(lambda: rg.patterns_vs_rev_experimental(str(rev_top)))
        c.check("rev mode -> dict in field order", type(rv) is dict and list(rv) == KEYS, type(rv) is dict and list(rv))
        rv = rv if type(rv) is dict else {}
        c.check("rev mode is delta mode", rv.get("experimental") is True and rv.get("delta_mode") is True, rv)
        c.check("rev mode lists the added divergence", divergent(rv) == [DIRECT], divergent(rv))
        c.check("rev mode conventions from the working tree",
                (rv.get("populations") or [{}])[0].get("verdict") == "5/6", rv.get("populations"))
        c.check("rev surface marker", DELTA_MARKER in err, err[-400:])
        c.raises("unknown rev raises", ValueError,
                 lambda: rg.patterns_vs_rev_experimental(str(rev_top), base="no-such-rev"), "no-such-rev")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
