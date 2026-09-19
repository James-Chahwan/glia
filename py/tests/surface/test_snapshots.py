#!/usr/bin/env python3
"""pyo3 surface, py/src/snapshots.rs (LF.5d): the module function
`history_sync(repo_path, max_commits=2000, since=None, blame=False,
blame_max_files=300)` reads a repo's local git, writes
`<repo>/.glia/history-snapshot/` and returns a native dict {head, commits,
files, renames, binary, blame_files, runs}; it raises ValueError when the sync
fails. The next generate() ingests the snapshot (CO_CHANGES). Needs a `git`
binary. Shared helpers: test_build.py."""
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, rg, stderr_of

T0, DAY = 1_767_225_600, 86_400  # 2026-01-01T00:00:00Z
IDENTITY = {
    "GIT_AUTHOR_NAME": "Quillon Identitymarker",
    "GIT_AUTHOR_EMAIL": "quillon.author@identity.invalid",
    "GIT_COMMITTER_NAME": "Pemberly Committertoken",
    "GIT_COMMITTER_EMAIL": "pemberly.committer@identity.invalid",
}
SNAPSHOT_FILES = ["commits.jsonl", "blame.jsonl", "meta.json"]


def git(top: pathlib.Path, *args: str, t: int = T0) -> str:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    env.update(IDENTITY, GIT_AUTHOR_DATE=f"@{t} +0000", GIT_COMMITTER_DATE=f"@{t} +0000")
    return subprocess.run(["git", "-C", str(top), *args], env=env, check=True,
                          capture_output=True, text=True).stdout.strip()


def probe_g1(top: pathlib.Path) -> None:
    """svc/a.py and svc/b.py change together in 4 commits, then svc/c.py is
    renamed: 5 commits, 3 files, 1 rename."""
    (top / "svc").mkdir(parents=True)
    git(top, "init", "-q", "-b", "main")
    for name, body in (("a", 1), ("b", 2), ("c", 3)):
        (top / "svc" / f"{name}.py").write_text(f"def {name}():\n    return {body}\n")
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "init")
    for i in range(1, 4):
        for name in ("a", "b"):
            with open(top / "svc" / f"{name}.py", "a") as f:
                f.write(f"# edit {i}\n")
        git(top, "add", "-A", t=T0 + i * DAY)
        git(top, "commit", "-q", "-m", f"co-change {i}", t=T0 + i * DAY)
    git(top, "mv", "svc/c.py", "svc/c2.py", t=T0 + 4 * DAY)
    git(top, "commit", "-q", "-m", "rename c", t=T0 + 4 * DAY)


def main() -> int:
    c = Checks("snapshots")
    c.check("history_sync signature",
            getattr(rg, "history_sync", None) is not None
            and rg.history_sync.__text_signature__
            == "(repo_path, max_commits=2000, since=None, blame=False, blame_max_files=300)",
            getattr(getattr(rg, "history_sync", None), "__text_signature__", "missing"))
    if getattr(rg, "history_sync", None) is None:
        return c.done()

    with tempfile.TemporaryDirectory(prefix="glia-surface-snapshots-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "g1"
        probe_g1(top)
        head = git(top, "rev-parse", "HEAD")
        snap = top / ".glia" / "history-snapshot"

        s, err = stderr_of(lambda: rg.history_sync(str(top)))
        c.check("history_sync -> dict", type(s) is dict, type(s))
        c.check("keys in field order",
                type(s) is dict and list(s) == ["head", "commits", "files", "renames", "binary",
                                                "blame_files", "runs"],
                type(s) is dict and list(s))
        c.check("summary counts",
                type(s) is dict and (s.get("head"), s.get("commits"), s.get("files"), s.get("renames"))
                == (head, 5, 3, 1), s)
        c.check("snapshot written", all((snap / f).is_file() for f in SNAPSHOT_FILES),
                sorted(p.name for p in snap.glob("*")) if snap.is_dir() else "no snapshot dir")
        c.check("fired_on marker",
                f"head={head[:12]} commits=5 files=3 renames=1 binary=0 blame_files=0 runs=0 "
                "window=max:2000 surface=pyo3" in err, err[-400:])

        s, err = stderr_of(lambda: rg.history_sync(str(top), max_commits=2, blame=True, blame_max_files=2))
        c.check("options reach the capture",
                type(s) is dict and (s.get("commits"), s.get("blame_files")) == (2, 2), s)
        c.check("blame rows written",
                len((snap / "blame.jsonl").read_text().splitlines()) == 2 if snap.is_dir() else False)
        c.check("window in the marker", " window=max:2 surface=pyo3" in err, err[-400:])

        rg.history_sync(str(top))
        g, err = stderr_of(lambda: rg.generate(str(top)))
        c.check("generate() ingests the snapshot", "[history] ingest repo=" in err, err[-400:])
        co_changes = next((i for i, n in rg.category_names() if n == "CO_CHANGES"), None)
        qnames = {n["id"]: n["qname"] for n in json.loads(g.nodes_json())}
        pairs = [(qnames.get(e["from"]), qnames.get(e["to"])) for e in json.loads(g.edges_json())
                 if e["category"] == co_changes]
        c.check("CO_CHANGES for the co-changed pair", pairs == [("svc::a", "svc::b")], pairs)

        plain = pathlib.Path(tmp) / "plain"
        plain.mkdir()
        (plain / "app.py").write_text("def f():\n    return 1\n")
        c.raises("outside git raises", ValueError, lambda: rg.history_sync(str(plain)),
                 "is not inside a git work tree")
        c.check("a failed sync writes nothing", not (plain / ".glia").exists())
        c.raises("empty window raises", ValueError, lambda: rg.history_sync(str(top), max_commits=0),
                 "max_commits must be at least 1")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
