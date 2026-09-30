#!/usr/bin/env python3
"""pyo3 surface, py/src/cochange.rs (CC.11c): `PyGraph.cochange(files,
min_confidence=0.3, min_support=3, top=20, unlinked_only=False)` and the module
function `cochange_vs_rev(repo_path, base="HEAD", ...)` return the co-change
suggestions `{query_files, unmapped, rows, absence}` as a native dict: the
files that usually change with the query in git history, each with a
directional confidence, its support, the antecedent file(s), the static link
and tier heuristic. `min_confidence` outside [0, 1] and an unknown rev raise
ValueError. The history is real: a git repo whose commits are synced with
`history_sync` before the build, so this needs a `git` binary. Shared helpers:
test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ADMIN, REPORT, PAGE = "svc/admin.py", "svc/report.py", "web/page.ts"
SOURCES = {
    ADMIN: "from svc.report import format_report\n\n\ndef admin_summary(rows):\n    return format_report(rows)\n",
    REPORT: 'def format_report(rows):\n    return ", ".join(rows)\n',
    PAGE: "export function renderPage(title: string): string {\n  return title;\n}\n",
}
# After a README-only first commit: admin 12 commits, 3 with report (report's
# only 3), 4 with page (page's only 4), 5 alone. admin imports report; nothing
# links page to admin.
PLAN = [[ADMIN, REPORT]] * 3 + [[ADMIN, PAGE]] * 4 + [[ADMIN]] * 5
KEYS = ["query_files", "unmapped", "rows", "absence"]
ROW_KEYS = ["file", "module_qname", "antecedent", "support", "antecedent_commits", "confidence_permille",
            "link", "source", "tier", "note"]
KW = [("min_confidence", 0.3), ("min_support", 3), ("top", 20), ("unlinked_only", False)]
MARKER = ("[cochange-suggest] query_files=1 unmapped=0 candidates=1 rows=1 unlinked=0 "
          "source=multi antecedents=0 commits_scanned=13")
IDENTITY = {
    "GIT_AUTHOR_NAME": "Cochange Fixture",
    "GIT_AUTHOR_EMAIL": "cochange.fixture@identity.invalid",
    "GIT_COMMITTER_NAME": "Cochange Fixture",
    "GIT_COMMITTER_EMAIL": "cochange.fixture@identity.invalid",
}


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    env.update(IDENTITY)
    subprocess.run(["git", "-c", "commit.gpgsign=false", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def history_repo(top: pathlib.Path) -> None:
    """The acceptance tree committed as PLAN says; each file is created by
    the first commit that touches it."""
    top.mkdir(parents=True)
    git(top, "init", "-q", "-b", "main")
    (top / "README.md").write_text("# demo\n")
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "readme")
    for k, paths in enumerate(PLAN, 1):
        for rel in paths:
            path = top / rel
            if path.exists():
                with open(path, "a") as f:
                    f.write(f"# edit {k}\n" if rel.endswith(".py") else f"// edit {k}\n")
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(SOURCES[rel])
        git(top, "add", "-A")
        git(top, "commit", "-q", "-m", f"commit {k}")


def rows(d) -> list:
    return [(r.get("file"), r.get("confidence_permille"), r.get("link")) for r in d.get("rows", [])] \
        if type(d) is dict else d


def main() -> int:
    c = Checks("cochange")
    method = getattr(rg.PyGraph, "cochange", None)
    fn = getattr(rg, "cochange_vs_rev", None)
    c.check("PyGraph.cochange exists", method is not None)
    c.check("cochange_vs_rev exists", fn is not None)
    if method is None or fn is None:
        return c.done()
    c.check("cochange params", params(method) == [("files", None), *KW], params(method))
    c.check("cochange_vs_rev params", params(fn) == [("repo_path", None), ("base", "HEAD"), *KW], params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-cochange-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "shop"
        history_repo(top)

        bare = rg.generate(str(top))
        d = bare.cochange([ADMIN])
        c.check("no snapshot -> no_history absence", d.get("rows") == []
                and (d.get("absence") or {}).get("reason") == "no_history", d)
        c.check("absence names history sync", "glia history sync" in (d.get("absence") or {}).get("note", ""),
                d.get("absence"))

        rg.history_sync(str(top))
        g = rg.generate(str(top))
        d, err = stderr_of(lambda: g.cochange([REPORT]))
        c.check("cochange -> dict in engine field order", type(d) is dict and list(d) == KEYS,
                type(d) is dict and list(d))
        c.check("rows[0] is svc/admin.py", type(d) is dict and d.get("rows")
                and d["rows"][0].get("file") == ADMIN, d)
        first = d["rows"][0] if type(d) is dict and d.get("rows") else {}
        c.check("row keys in engine field order", list(first) == ROW_KEYS, list(first))
        c.check("report -> admin 3/3, linked, pairwise",
                (first.get("support"), first.get("antecedent_commits"), first.get("confidence_permille"),
                 first.get("antecedent"), first.get("link"), first.get("source"), first.get("tier"),
                 first.get("module_qname"), first.get("note"))
                == (3, 3, 1000, [REPORT], "direct", "pairwise", "heuristic", "svc::admin", None), first)
        c.check("no absence", type(d) is dict and d.get("absence", 1) is None, d)
        c.check("engine marker", MARKER in err, err[-400:])

        a = g.cochange([ADMIN])
        c.check("admin -> page 4/12, unlinked; admin -> report 3/12 under the floor",
                rows(a) == [(PAGE, 333, "none")], a)
        c.check("unlinked row carries the note", a.get("rows") and "blind spot" in (a["rows"][0].get("note") or ""), a)
        c.check("unlinked_only lists web/page.ts only", rows(g.cochange([ADMIN], unlinked_only=True))
                == [(PAGE, 333, "none")])
        c.check("min_confidence=0.25 adds the linked converse",
                rows(g.cochange([ADMIN], min_confidence=0.25)) == [(PAGE, 333, "none"), (REPORT, 250, "direct")])
        c.check("top=1", rows(g.cochange([ADMIN], min_confidence=0.25, top=1)) == [(PAGE, 333, "none")])
        miss = g.cochange([REPORT], min_support=4)
        c.check("floors -> no_match", miss.get("rows") == []
                and (miss.get("absence") or {}).get("reason") == "no_match"
                and "min_support=4" in miss["absence"].get("note", ""), miss)
        u = g.cochange([REPORT, "README.md"])
        c.check("unmapped query file listed", u.get("unmapped") == ["README.md"] and rows(u)[0][0] == ADMIN, u)
        c.raises("min_confidence > 1 raises", ValueError, lambda: g.cochange([REPORT], min_confidence=1.5), "[0, 1]")
        c.raises("min_confidence < 0 raises", ValueError, lambda: g.cochange([REPORT], min_confidence=-0.1),
                 "[0, 1]")

        clean = rg.cochange_vs_rev(str(top))
        c.check("clean tree -> no_match, no change", clean.get("query_files") == []
                and "no change against HEAD" in (clean.get("absence") or {}).get("note", ""), clean)
        (top / REPORT).write_text(SOURCES[REPORT] + "# working tree\n")
        r, err = stderr_of(lambda: rg.cochange_vs_rev(str(top)))
        c.check("rev mode queries the change", r.get("query_files") == [REPORT]
                and rows(r) == [(ADMIN, 1000, "direct")], r)
        c.check("rev mode marker", MARKER in err, err[-400:])
        c.raises("unknown rev raises", ValueError, lambda: rg.cochange_vs_rev(str(top), base="no-such-rev"),
                 "no-such-rev")
        c.raises("min_confidence NaN raises", ValueError,
                 lambda: rg.cochange_vs_rev(str(top), min_confidence=float("nan")), "[0, 1]")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
