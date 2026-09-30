#!/usr/bin/env python3
"""pyo3 surface, py/src/timeline.rs (CD.5d): the time-travel graph as three
module functions over CD.5c's four-commit history (f -> g, + h, git mv, f
drops g).

- `timeline_build(repo_path, revs=20, head="HEAD")` -> dict {revs, skipped,
  nodes, edges, edge_spans, closed, moves, written}; writes
  `<repo>/.glia/graph/timeline.gmap` unless GLIA_NO_PERSIST=1.
- `timeline_history(repo_path, qname, category=None)` -> dict {results,
  absence}, each row {category, direction, other_qname, other_kind, since,
  since_window_start, until, file, line, tier}.
- `timeline_as_of(repo_path, rev)` -> dict {rev, nodes, edges, by_category}.

ValueError for an argument naming nothing (revs outside 1..=200, an unknown
rev), RuntimeError for a failed build or a missing sidecar. Needs a `git`
binary. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

C1 = "def f():\n    return g()\n\n\ndef g():\n    return 1\n"
C2 = "def f():\n    g()\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n"
C4 = "def f():\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n"
BUILT_KEYS = ["revs", "skipped", "nodes", "edges", "edge_spans", "closed", "moves", "written"]
REV_KEYS = ["index", "sha", "time", "subject"]
ROW_KEYS = ["category", "direction", "other_qname", "other_kind", "since", "since_window_start",
            "until", "file", "line", "tier"]
AS_OF_KEYS = ["rev", "nodes", "edges", "by_category"]


def git(top: pathlib.Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    return subprocess.run(
        ["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
         "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
        env=env, check=True, capture_output=True, text=True).stdout.strip()


def four_commits(top: pathlib.Path) -> None:
    (top / "app").mkdir(parents=True)
    git(top, "init", "-q")
    for rel, text, msg in (("app/a.py", C1, "f calls g"), ("app/a.py", C2, "f calls h")):
        (top / rel).write_text(text)
        git(top, "add", "-A")
        git(top, "commit", "-q", "-m", msg)
    git(top, "mv", "app/a.py", "app/b.py")
    git(top, "commit", "-q", "-m", "move a to b")
    (top / "app" / "b.py").write_text(C4)
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "f drops g")


def main() -> int:
    c = Checks("timeline")
    fns = {n: getattr(rg, n, None) for n in ("timeline_build", "timeline_history", "timeline_as_of")}
    for name, fn in fns.items():
        c.check(f"{name} exists", fn is not None)
    if any(fn is None for fn in fns.values()):
        return c.done()
    c.check("timeline_build params",
            params(rg.timeline_build) == [("repo_path", None), ("revs", 20), ("head", "HEAD")],
            params(rg.timeline_build))
    c.check("timeline_history params",
            params(rg.timeline_history) == [("repo_path", None), ("qname", None), ("category", None)],
            params(rg.timeline_history))
    c.check("timeline_as_of params",
            params(rg.timeline_as_of) == [("repo_path", None), ("rev", None)], params(rg.timeline_as_of))

    with tempfile.TemporaryDirectory(prefix="glia-surface-timeline-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "repo"
        four_commits(top)
        repo = str(top)

        c.raises("history before a build raises", RuntimeError,
                 lambda: rg.timeline_history(repo, "f"), "glia timeline build")
        c.raises("as_of before a build raises", RuntimeError, lambda: rg.timeline_as_of(repo, "0"),
                 "glia timeline build")
        c.raises("revs=0 raises", ValueError, lambda: rg.timeline_build(repo, revs=0), "1..=200")
        c.raises("revs=201 raises", ValueError, lambda: rg.timeline_build(repo, revs=201), "1..=200")
        c.raises("unknown head raises", RuntimeError,
                 lambda: rg.timeline_build(repo, head="no-such-rev"), "unknown rev no-such-rev")

        b, err = stderr_of(lambda: rg.timeline_build(repo, revs=4))
        c.check("build -> dict", type(b) is dict, type(b))
        c.check("build keys in field order", type(b) is dict and list(b) == BUILT_KEYS,
                type(b) is dict and list(b))
        revs = b.get("revs", []) if type(b) is dict else []
        c.check("4 revs, oldest first",
                [r.get("subject") for r in revs] == ["f calls g", "f calls h", "move a to b", "f drops g"], revs)
        c.check("rev keys", all(list(r) == REV_KEYS for r in revs), revs[:1])
        c.check("rev times are ints", all(type(r.get("time")) is int for r in revs), revs[:1])
        sidecar = top / ".glia" / "graph" / "timeline.gmap"
        c.check("sidecar written", type(b) is dict and b.get("written") == str(sidecar) and sidecar.is_file(), b)
        c.check("engine build marker", "[timeline] repo=" in err and " built=4 " in err, err[-400:])
        c.check("surface build marker", "[timeline] surface=pyo3 build rows=4" in err, err[-400:])

        h, err = stderr_of(lambda: rg.timeline_history(repo, "f"))
        c.check("history -> dict {results, absence}", type(h) is dict and list(h) == ["results", "absence"],
                type(h) is dict and list(h))
        rows = h.get("results", []) if type(h) is dict else []
        c.check("row keys in field order", all(list(r) == ROW_KEYS for r in rows), rows[:1])
        g = next((r for r in rows if r.get("category") == "CALLS" and r.get("direction") == "out"
                  and str(r.get("other_qname")).endswith("::g")), None)
        c.check("f -> g closes at rev 3",
                g is not None and (g.get("until") or {}).get("index") == 3 and g.get("since_window_start") is True,
                g)
        c.check("f -> g other end located 1-based",
                g is not None and (g.get("file"), g.get("line")) == ("app/b.py", 5), g)
        hh = next((r for r in rows if r.get("category") == "CALLS" and r.get("direction") == "out"
                   and str(r.get("other_qname")).endswith("::h")), None)
        c.check("f -> h still present", hh is not None and hh.get("until") is None, hh)
        c.check("surface history marker", f"[timeline] surface=pyo3 history rows={len(rows)}" in err, err[-400:])

        e = rg.timeline_history(repo, "f", category="HTTP_CALLS")
        c.check("an empty answer carries its absence",
                type(e) is dict and e.get("results") == [] and (e.get("absence") or {}).get("reason") == "no_edges", e)

        a, err = stderr_of(lambda: rg.timeline_as_of(repo, "1"))
        c.check("as_of -> dict", type(a) is dict and list(a) == AS_OF_KEYS, type(a) is dict and list(a))
        c.check("as_of rev 1", type(a) is dict and (a.get("rev") or {}).get("index") == 1, a)
        c.check("as_of CALLS at rev 1", type(a) is dict and (a.get("by_category") or {}).get("CALLS", 0) >= 2, a)
        c.check("surface as_of marker", "[timeline] surface=pyo3 as_of rows=" in err, err[-400:])
        sha = revs[1].get("sha", "") if len(revs) > 1 else ""
        a2 = rg.timeline_as_of(repo, sha[:7])
        c.check("as_of by sha prefix", type(a2) is dict and (a2.get("rev") or {}).get("index") == 1, a2)
        c.raises("unknown rev raises", ValueError, lambda: rg.timeline_as_of(repo, "9"), "no rev `9`")

        plain = pathlib.Path(tmp) / "plain"
        plain.mkdir()
        (plain / "app.py").write_text("def f():\n    return 1\n")
        c.raises("outside git raises", RuntimeError, lambda: rg.timeline_build(str(plain)), "not a git work tree")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
