#!/usr/bin/env python3
"""pyo3 surface, py/src/watch.rs (DC.9): `glia_py.WatchSession`, one repo's
incremental build held open over `glia_engine::watch` (DC.7a-c).

- `WatchSession(repo, overlay=True)`; ValueError when `repo` is no directory.
- `apply(paths)` -> dict {seq, files: {added, modified, removed, reparsed,
  reused}, groups_rebuilt, full_rebuild, total_nodes, total_edges, counts,
  nodes, edges, phase_ms}; an unchanged file keeps `seq`, with no rows.
- `poll()` -> dict | None (None when quiet).
- `snapshot()` -> PyGraph, independent of later updates; it shares the
  session's ranking cache (EA.7, `[rank-cache] holder=watch`).
- `persist()` writes `<repo>/.glia/graph/` (manifest + parse cache);
  ValueError, writing nothing, for an `overlay=False` session.

Marker: `[watch] session repo=<repo> overlay=<true|false> surface=py`. The
repo is two Python files in a TemporaryDirectory: never a real repo. Shared
helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import sys
import tempfile
import time

from test_build import Checks, params, rg, stderr_of, tree

KEYS = ["seq", "files", "groups_rebuilt", "full_rebuild", "total_nodes", "total_edges",
        "counts", "nodes", "edges", "phase_ms"]
FILES_KEYS = ["added", "modified", "removed", "reparsed", "reused"]
A_PY = "def foo():\n    return bar()\n\ndef bar():\n    return 1\n"
# One more function: the graph gains nodes, so a snapshot's count can tell.
A_EDIT = A_PY + "\ndef baz():\n    return foo() + 2\n"
A_POLL = A_EDIT + "\ndef qux():\n    return baz()\n"


def cache_marks(err: str, holder: str) -> list[str]:
    """The `index=<hit|built>` of every `[rank-cache] holder=<holder>` line (EA.7)."""
    return [w for line in err.splitlines() if line.startswith(f"[rank-cache] holder={holder} ")
            for w in line.split() if w.startswith("index=")]


def watch_repo(tmp: str, name: str = "dc9app") -> str:
    """`<tmp>/<name>`: a.py and b.py, every mtime a minute old, so the
    session's racy rule (DC.7c) has nothing to re-check and a quiet poll does
    no work."""
    root = pathlib.Path(tmp) / name
    root.mkdir()
    (root / "a.py").write_text(A_PY)
    (root / "b.py").write_text("def b():\n    return 2\n")
    old = time.time() - 60
    for p in [*root.rglob("*"), root]:
        os.utime(p, (old, old))
    return str(root)


def main() -> int:
    c = Checks("watch")
    c.check("WatchSession is a class", isinstance(getattr(rg, "WatchSession", None), type))
    c.check("constructor params", params(rg.WatchSession) == [("repo", None), ("overlay", True)],
            params(rg.WatchSession))

    with tempfile.TemporaryDirectory(prefix="glia-surface-watch-") as tmp:
        repo = watch_repo(tmp)
        before = tree(repo)
        s, err = stderr_of(lambda: rg.WatchSession(repo))
        c.check("marker", f"[watch] session repo={repo} overlay=true surface=py" in err, err[-600:])
        c.check("engine open marker", f"[watch] open repo={repo} " in err, err[-600:])
        c.check("open writes nothing into the repo", tree(repo) == before, tree(repo))

        clean = rg.generate(repo)
        first = s.snapshot()
        c.check("snapshot is a PyGraph", type(first).__name__ == "PyGraph", type(first))
        c.check("snapshot counts equal a clean generate",
                (first.node_count(), first.edge_count()) == (clean.node_count(), clean.edge_count()),
                ((first.node_count(), first.edge_count()), (clean.node_count(), clean.edge_count())))
        c.check("snapshot is overlay-applied", first.overlay_applied is True, first.overlay_applied)
        n0 = first.node_count()
        # EA.7: a snapshot shares the session's ranking cache (holder
        # `watch`): two snapshots of one unchanged session answer
        # blast_radius identically, the second reading the PPR index warm.
        one, err1 = stderr_of(lambda: first.blast_radius("foo", top_k=50))
        two, err2 = stderr_of(lambda: s.snapshot().blast_radius("foo", top_k=50))
        c.check("two snapshots of one session answer blast_radius identically", one == two)
        c.check("the first snapshot builds the session's index",
                cache_marks(err1, "watch")[:1] == ["index=built"], cache_marks(err1, "watch"))
        c.check("the second snapshot reads it: [rank-cache] holder=watch .. index=hit",
                cache_marks(err2, "watch") and set(cache_marks(err2, "watch")) == {"index=hit"},
                cache_marks(err2, "watch"))
        c.check("poll() is None when quiet", s.poll() is None)

        (pathlib.Path(repo) / "a.py").write_text(A_EDIT)
        u = s.apply(["a.py"])
        c.check("apply returns a dict", isinstance(u, dict), type(u))
        u = u if isinstance(u, dict) else {}
        c.check("apply keys, in order", list(u) == KEYS, list(u))
        files = u.get("files") if isinstance(u.get("files"), dict) else {}
        c.check("files keys, in order", list(files) == FILES_KEYS, list(files))
        c.check("files.modified == ['a.py']", files.get("modified") == ["a.py"], files)
        c.check("seq 1 after the first update", u.get("seq") == 1, u.get("seq"))
        c.check("full_rebuild None on a body edit", u.get("full_rebuild") is None, u.get("full_rebuild"))
        c.check("the edit's rows", any(n.get("qname", "").endswith("baz") for n in u.get("nodes", [])),
                u.get("nodes"))
        c.check("counts and phase_ms are dicts",
                isinstance(u.get("counts"), dict) and isinstance(u.get("phase_ms"), dict), u)
        clean = rg.generate(repo)
        c.check("total_nodes / total_edges equal a clean generate",
                (u.get("total_nodes"), u.get("total_edges")) == (clean.node_count(), clean.edge_count()),
                ((u.get("total_nodes"), u.get("total_edges")), (clean.node_count(), clean.edge_count())))
        after = s.snapshot()
        c.check("a new snapshot sees the edit",
                (after.node_count(), after.edge_count()) == (clean.node_count(), clean.edge_count())
                and after.node_count() > first.node_count(),
                (after.node_count(), first.node_count(), clean.node_count()))
        c.check("an earlier snapshot keeps its node count",
                first.node_count() == n0 < after.node_count(), (n0, first.node_count(), after.node_count()))

        noop = s.apply([os.path.join(repo, "a.py")])
        c.check("an unchanged file is a noop: seq kept, no rows",
                noop.get("seq") == 1 and noop.get("nodes") == [] and noop.get("edges") == [], noop)
        c.raises("apply takes a list, not a str", TypeError, lambda: s.apply("a.py"))

        (pathlib.Path(repo) / "a.py").write_text(A_POLL)
        p = s.poll()
        c.check("poll() is a dict after an edit", isinstance(p, dict), p)
        p = p if isinstance(p, dict) else {}
        c.check("poll keys", list(p) == KEYS, list(p))
        c.check("poll saw a.py", p.get("files", {}).get("modified") == ["a.py"] and p.get("seq") == 2, p)
        c.check("poll() is None once caught up", s.poll() is None)
        c.check("apply / poll write nothing into the repo",
                tree(repo) == before and not os.path.exists(rg.default_gmap_dir(repo)), tree(repo))

        s.persist()
        layout = pathlib.Path(repo) / ".glia" / "graph"
        c.check("persist() writes the default layout", (layout / "manifest.json").is_file(), tree(repo))
        c.check("persist() writes the parse cache", (layout / "parse_cache.bin").is_file(), tree(repo))
        loaded = rg.load_from_gmap(rg.default_gmap_dir(repo), repo)
        c.check("load_from_gmap reads what persist wrote",
                loaded.node_count() == s.snapshot().node_count(), (loaded.node_count(),))

        other = watch_repo(tmp, "dc9bare")
        bare, err = stderr_of(lambda: rg.WatchSession(other, overlay=False))
        c.check("overlay=False marker", f"[watch] session repo={other} overlay=false surface=py" in err,
                err[-600:])
        c.check("overlay=False snapshot is marked", bare.snapshot().overlay_applied is False)
        before_refusal = tree(other)
        c.raises("persist() refuses an overlay=False session", ValueError, bare.persist, "overlay=False")
        c.check("the refusal wrote nothing", tree(other) == before_refusal, tree(other))

        c.raises("WatchSession on a missing dir", ValueError,
                 lambda: rg.WatchSession(os.path.join(tmp, "missing")), "not a directory")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
