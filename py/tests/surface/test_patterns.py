#!/usr/bin/env python3
"""pyo3 surface, py/src/patterns.rs (LE.7b, promoted by CC.12b):
`PyGraph.patterns(min_support=5, min_share=75, scope=None, group_by="service")`
and the module function `patterns_vs_rev(repo_path, base="HEAD",
min_support=5, min_share=75, scope=None, group_by="service")` return the
pattern-conformance report as a native dict {delta_mode, handlers, judged,
skipped_small, excluded, role_sources, populations, divergences, blind}: route
handlers grouped per service (or per service and directory with
group_by="package", CA.5b), the most frequent role chain of the sighted
handlers as a population's convention, each sighted handler off it a located
DIVERGENCE (lines 1-based), each blind one (`handler>(no effect)`) listed in
the population's `blind`. No key and no marker says `experimental`. The
pre-promotion names `PyGraph.patterns_experimental` / `patterns_vs_rev_experimental`
stay until 0.5.2 as aliases: same parameters, the same dict, and a
DeprecationWarning naming the new name (raised under
`warnings.simplefilter("error")`). A min_share above 100, a group_by other
than "service" / "package" or an engine error raises ValueError. The rev
function needs a `git` binary. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile
import warnings

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
KEYS = ["delta_mode", "handlers", "judged", "skipped_small", "excluded", "role_sources",
        "populations", "divergences", "blind"]
POP_KEYS = ["service", "package", "role", "size", "sighted", "status", "convention", "matching", "verdict",
            "signatures", "role_sources", "exceptions", "blind"]
DIV_KEYS = ["verdict", "tier", "service", "handler", "file", "line", "route_method", "route_path", "signature",
            "convention", "matching", "population", "path", "role_sources"]
KW = [("min_support", 5), ("min_share", 75), ("scope", None), ("group_by", "service")]
GRAPH_MARKER = "[patterns] surface=pyo3 mode=graph"
DELTA_MARKER = "[patterns] surface=pyo3 mode=delta"


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


def deprecated(fn):
    """(fn(), the DeprecationWarnings it emitted) under the default filters."""
    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always")
        out = fn()
    return out, [w for w in seen if issubclass(w.category, DeprecationWarning)]


def main() -> int:
    c = Checks("patterns")
    method = getattr(rg.PyGraph, "patterns", None)
    fn = getattr(rg, "patterns_vs_rev", None)
    old_method = getattr(rg.PyGraph, "patterns_experimental", None)
    old_fn = getattr(rg, "patterns_vs_rev_experimental", None)
    c.check("PyGraph.patterns exists", method is not None)
    c.check("patterns_vs_rev exists", fn is not None)
    c.check("PyGraph.patterns_experimental alias kept until 0.5.2", old_method is not None)
    c.check("patterns_vs_rev_experimental alias kept until 0.5.2", old_fn is not None)
    if None in (method, fn, old_method, old_fn):
        return c.done()
    rev_params = [("repo_path", None), ("base", "HEAD"), *KW]
    c.check("patterns params", params(method) == KW, params(method))
    c.check("patterns_vs_rev params", params(fn) == rev_params, params(fn))
    c.check("alias params match", params(old_method) == KW and params(old_fn) == rev_params,
            (params(old_method), params(old_fn)))

    with tempfile.TemporaryDirectory(prefix="glia-surface-patterns-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "shop"
        write_shop(top, 6)
        g = rg.generate(str(top))

        r, err = stderr_of(lambda: g.patterns())
        c.check("patterns -> dict in field order", type(r) is dict and list(r) == KEYS,
                type(r) is dict and list(r))
        r = r if type(r) is dict else {}
        c.check("no experimental key", "experimental" not in r, list(r))
        c.check("whole-graph mode", r.get("delta_mode") is False)
        c.check("counts", (r.get("handlers"), r.get("judged"), r.get("skipped_small")) == (6, 1, 0), r)
        c.check("nothing excluded", r.get("excluded") == {}, r.get("excluded"))
        pops = r.get("populations", [])
        pop = pops[0] if pops else {}
        c.check("population keys in engine field order", list(pop) == POP_KEYS, list(pop))
        c.check("population judged 5/6",
                (pop.get("service"), pop.get("status"), pop.get("verdict"), pop.get("convention"))
                == ("handlers", "judged", "5/6", CONVENTION), pop)
        c.check("population keyed by service, every handler sighted",
                (pop.get("package"), pop.get("size"), pop.get("sighted"), pop.get("blind"), r.get("blind"))
                == (None, 6, 6, [], 0), pop)
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
        c.check("engine marker", "[patterns] populations=1 judged=1 handlers=6 divergences=1" in err,
                err[-400:])
        c.check("engine marker ends in blind and group_by", " blind=0 group_by=service\n" in err, err[-400:])
        c.check("no marker says experimental", "[patterns] experimental" not in err, err[-400:])

        (old, seen), old_err = stderr_of(lambda: deprecated(lambda: g.patterns_experimental()))
        c.check("patterns_experimental returns the same dict", old == r, old)
        c.check("patterns_experimental warns once",
                len(seen) == 1 and "use patterns (removed in 0.5.2)" in str(seen[0].message),
                [str(w.message) for w in seen])
        c.check("patterns_experimental marker is the pyo3 one", GRAPH_MARKER in old_err, old_err[-400:])
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            c.raises("patterns_experimental raises DeprecationWarning under error filter", DeprecationWarning,
                     lambda: g.patterns_experimental(), "patterns_experimental is deprecated")
            c.check("patterns() does not warn under error filter", g.patterns() == r)

        by_pkg, err = stderr_of(lambda: g.patterns(group_by="package"))
        c.check("group_by=package keys every population by directory",
                [(p.get("service"), p.get("package")) for p in by_pkg.get("populations", [])]
                == [("handlers", "handlers")], by_pkg)
        c.check("group_by=package marker", " group_by=package\n" in err, err[-400:])
        c.raises("group_by other than service / package raises", ValueError,
                 lambda: g.patterns(group_by="dir"), '"service" or "package"')

        small = g.patterns(min_support=7)
        c.check("min_support: too small", small["skipped_small"] == 1
                and small["populations"][0]["status"] == "too_small" and not small["divergences"], small)
        split = g.patterns(min_share=90)
        c.check("min_share: no convention",
                split["populations"][0]["status"] == "no_convention" and not split["divergences"], split)
        scoped = g.patterns(scope="service")
        c.check("scope excludes handlers outside it",
                scoped["excluded"] == {"out_of_scope": 6} and scoped["handlers"] == 0, scoped)
        c.raises("min_share above 100 raises", ValueError, lambda: g.patterns(min_share=101), "101")

        rev_top = pathlib.Path(tmp) / "revshop"
        write_shop(rev_top, 5)
        git(rev_top, "init", "-q")
        git(rev_top, "add", "-A")
        git(rev_top, "commit", "-q", "-m", "five layered handlers")
        write_shop(rev_top, 6)
        rv, err = stderr_of(lambda: rg.patterns_vs_rev(str(rev_top)))
        c.check("rev mode -> dict in field order", type(rv) is dict and list(rv) == KEYS, type(rv) is dict and list(rv))
        rv = rv if type(rv) is dict else {}
        c.check("rev mode is delta mode", "experimental" not in rv and rv.get("delta_mode") is True, rv)
        c.check("rev mode lists the added divergence", divergent(rv) == [DIRECT], divergent(rv))
        c.check("rev mode conventions from the working tree",
                (rv.get("populations") or [{}])[0].get("verdict") == "5/6", rv.get("populations"))
        c.check("rev surface marker", DELTA_MARKER in err, err[-400:])
        c.raises("unknown rev raises", ValueError,
                 lambda: rg.patterns_vs_rev(str(rev_top), base="no-such-rev"), "no-such-rev")

        old_rv, seen = deprecated(lambda: rg.patterns_vs_rev_experimental(str(rev_top)))
        c.check("patterns_vs_rev_experimental returns the same dict", old_rv == rv, old_rv)
        c.check("patterns_vs_rev_experimental warns once",
                len(seen) == 1 and "use patterns_vs_rev (removed in 0.5.2)" in str(seen[0].message),
                [str(w.message) for w in seen])
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            c.raises("patterns_vs_rev_experimental raises DeprecationWarning under error filter",
                     DeprecationWarning, lambda: rg.patterns_vs_rev_experimental(str(rev_top)),
                     "patterns_vs_rev_experimental is deprecated")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
