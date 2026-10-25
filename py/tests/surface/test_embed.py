#!/usr/bin/env python3
"""pyo3 surface, py/src/embed.rs (FB.5c).

`embed(repo_path, model="bge-small", recipe=None, replace=False, limit=None,
check=False)` brings `<repo>/.glia/vectors.jsonl` up to date with the repo's
graph and returns a native dict {repo, model, recipe, dims, nodes, cached,
reused, embedded, ambiguous, deferred, stale_dropped, foreign_kept,
rows_written, ms}. A second call on an unchanged repo embeds nothing;
`check=True` counts the work and writes nothing; an unknown model or recipe
raises ValueError before any build, and so does `bge-small` in a wheel built
without the `embed` feature. Every check here uses the deterministic
`test-hash` model, so no model is downloaded.

Plain python3, no pytest (the house runner: dev-notes/wave-runner/closeout.py
runs every py/tests/surface/test_*.py over the installed wheel); `test_embed`
lets pytest collect it too. Shared helpers: test_build.py.
"""
from __future__ import annotations

import json
import pathlib
import sys
import tempfile

from test_build import Checks, fixture_repo, params, rg, stderr_of, tree

SIGNATURE = [("repo_path", None), ("model", "bge-small"), ("recipe", None),
             ("replace", False), ("limit", None), ("check", False)]
REPORT_KEYS = {"repo", "model", "recipe", "dims", "nodes", "cached", "reused", "embedded",
               "ambiguous", "deferred", "stale_dropped", "foreign_kept", "rows_written", "ms"}


def main() -> int:
    c = Checks("embed")
    c.check("embed signature", params(rg.embed) == SIGNATURE, params(rg.embed))

    with tempfile.TemporaryDirectory(prefix="glia-surface-embed-") as tmp:
        repo = fixture_repo(tmp, "fb5capp")
        vectors = pathlib.Path(repo) / ".glia" / "vectors.jsonl"

        before = tree(repo)
        r, err = stderr_of(lambda: rg.embed(repo, model="test-hash", check=True))
        c.check("check returns a dict", type(r) is dict and set(r) == REPORT_KEYS, r)
        c.check("check counts the work", type(r) is dict and r.get("embedded", 0) > 0
                and r.get("rows_written") == 0, r)
        c.check("check writes nothing", tree(repo) == before, tree(repo))
        c.check("check marker", " check=true " in err and err.rstrip().endswith("surface=py"),
                err[-400:])

        first, err = stderr_of(lambda: rg.embed(repo, model="test-hash"))
        c.check("first call embeds", type(first) is dict and first.get("embedded", 0) > 0, first)
        c.check("first call writes the sidecar", vectors.is_file())
        c.check("rows are the test model's",
                vectors.is_file() and {json.loads(line)["model"] for line in
                                       vectors.read_text().splitlines()} == {"glia-test-hash-v1/64"})
        c.check("report names the model and recipe",
                (first.get("model"), first.get("recipe"), first.get("dims"))
                == ("glia-test-hash-v1/64", "t2", 64), first)
        c.check("marker names the surface",
                "[embed] repo=fb5capp model=glia-test-hash-v1/64 recipe=t2 " in err
                and " surface=py" in err, err[-400:])
        body = vectors.read_text() if vectors.is_file() else ""

        second = rg.embed(repo, model="test-hash")
        c.check("second call embeds nothing", second.get("embedded") == 0
                and second.get("cached") == first.get("embedded"), second)
        c.check("second call keeps the sidecar byte for byte",
                vectors.is_file() and vectors.read_text() == body)

        snap = tree(repo)
        c.raises("unknown model raises", ValueError, lambda: rg.embed(repo, model="nope"),
                 "unknown embedding model `nope`")
        c.raises("unknown recipe raises", ValueError,
                 lambda: rg.embed(repo, model="test-hash", recipe="t9"),
                 "unknown embed recipe `t9`")
        c.check("a refusal writes nothing", tree(repo) == snap)
        c.raises("not a directory raises", ValueError,
                 lambda: rg.embed(str(pathlib.Path(tmp) / "absent"), model="test-hash"),
                 "is not a directory")

        other = fixture_repo(tmp, "fb5climit")
        lim = rg.embed(other, model="test-hash", limit=1, recipe="t1")
        c.check("limit defers the rest", lim.get("embedded") == 1 and lim.get("deferred", 0) > 0
                and lim.get("recipe") == "t1", lim)
    return c.done()


def test_embed() -> None:
    assert main() == 0


if __name__ == "__main__":
    sys.exit(main())
