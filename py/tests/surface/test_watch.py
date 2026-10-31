#!/usr/bin/env python3
"""pyo3 surface, py/src/watch.rs (DC.9): `glia_py.WatchSession`, one repo's
incremental build held open over `glia_engine::watch` (DC.7a-c).

- `WatchSession(repo, overlay=True, memory_budget_mb=None, with_repos=None,
  out=None)`; ValueError when `repo` is no directory. `memory_budget_mb`
  (GA.12) bounds the session's caches by the process's RSS (0: none; None
  keeps GLIA_MEMORY_BUDGET_MB's). GA.11: `with_repos` watches more repos as
  one joint build (`roots`, a read-only property, lists them in order) and
  needs `out` (ValueError before anything is read); `persist()` then writes
  the joint layout to `out`, never a repo's own `.glia/graph`.
- `apply(paths)` -> dict {seq, files: {added, modified, removed, reparsed,
  reused}, groups_rebuilt, body_only, full_rebuild, total_nodes, total_edges, counts,
  nodes, edges, phase_ms}; an unchanged file keeps `seq`, with no rows.
- `poll()` -> dict | None (None when quiet).
- `snapshot()` -> PyGraph, independent of later updates; it shares the
  session's ranking cache (EA.7, `[rank-cache] holder=watch`).
- `persist()` writes `<repo>/.glia/graph/` (manifest + parse cache);
  ValueError, writing nothing, for an `overlay=False` session.
- `memory()` -> dict {rss_kb, budget_mb, cached_groups, groups, cache_entries,
  memo_paths, evictions, memo_clears, rank: {holder, entries, hits, built}}
  (GA.12); `memory_budget_mb=1` evicts every cached group and still applies an
  edit like a clean generate.

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

KEYS = ["seq", "files", "groups_rebuilt", "body_only", "full_rebuild", "total_nodes", "total_edges",
        "counts", "nodes", "edges", "phase_ms"]
FILES_KEYS = ["added", "modified", "removed", "reparsed", "reused"]
MEMORY_KEYS = ["rss_kb", "budget_mb", "cached_groups", "groups", "cache_entries", "memo_paths",
               "evictions", "memo_clears", "rank"]
RANK_KEYS = ["holder", "entries", "hits", "built"]
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
    c.check("constructor params",
            params(rg.WatchSession) == [("repo", None), ("overlay", True), ("memory_budget_mb", None),
                                        ("with_repos", None), ("out", None)],
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
        c.check("body_only is a list of rebuilt groups (GA.4)",
                isinstance(u.get("body_only"), list)
                and set(u.get("body_only") or []) <= set(u.get("groups_rebuilt") or []), u)
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

        # GA.12: memory() and a 1 MB budget (below any process): every
        # cached group and the memos evicted, an edit still applied exactly.
        m = s.memory()
        c.check("memory() is a dict", isinstance(m, dict), type(m))
        m = m if isinstance(m, dict) else {}
        c.check("memory keys, in order", list(m) == MEMORY_KEYS, list(m))
        c.check("memory rank keys", list(m.get("rank") or {}) == RANK_KEYS, m.get("rank"))
        c.check("no budget keeps every group cached",
                m.get("budget_mb") is None and m.get("cached_groups") == m.get("groups"), m)
        tight = watch_repo(tmp, "ga12budget")
        b, err = stderr_of(lambda: rg.WatchSession(tight, memory_budget_mb=1))
        c.check("[watch] rss marker with the budget", "[watch] rss seq=0 " in err and " budget_mb=1 " in err,
                err[-800:])
        bm = b.memory()
        c.check("memory_budget_mb=1 is the budget", bm.get("budget_mb") == 1, bm)
        if bm.get("rss_kb") is not None:
            c.check("a 1 MB budget evicts every group and memo",
                    bm.get("cached_groups") == 0 and bm.get("memo_paths") == 0
                    and bm.get("evictions") == bm.get("groups"), bm)
        (pathlib.Path(tight) / "a.py").write_text(A_EDIT)
        bu = b.apply(["a.py"])
        clean = rg.generate(tight)
        c.check("a budgeted session applies an edit like a clean generate",
                bu.get("seq") == 1
                and (bu.get("total_nodes"), bu.get("total_edges")) == (clean.node_count(), clean.edge_count()),
                (bu.get("seq"), bu.get("total_nodes"), clean.node_count()))

        c.raises("WatchSession on a missing dir", ValueError,
                 lambda: rg.WatchSession(os.path.join(tmp, "missing")), "not a directory")

        # GA.11: two repos as one session.
        c.check("roots of a one-repo session", s.roots == [repo], s.roots)
        left, right = watch_repo(tmp, "ga11left"), watch_repo(tmp, "ga11right")
        before_pair = (tree(left), tree(right))
        c.raises("with_repos without out", ValueError,
                 lambda: rg.WatchSession(left, with_repos=[right]), "with_repos needs out")
        c.check("the refusal read and wrote nothing", (tree(left), tree(right)) == before_pair)
        out = os.path.join(tmp, "ga11joint")
        pair, err = stderr_of(lambda: rg.WatchSession(left, with_repos=[right], out=out))
        c.check("the joint marker",
                f"[watch] session repo={left} overlay=true repos=2 out={out} surface=py" in err, err[-600:])
        c.check("roots is both paths, in order", pair.roots == [left, right], pair.roots)
        (pathlib.Path(right) / "b.py").write_text("def b():\n    return 2\n\ndef c():\n    return b()\n")
        pu = pair.apply([os.path.join(right, "b.py")])
        c.check("an edit in the second repo is an update", pu.get("seq") == 1, pu)
        c.check("it names the file <root>:<rel>",
                (pu.get("files") or {}).get("modified") == [f"{right}:b.py"], pu.get("files"))
        pair.persist()
        c.check("persist() writes the joint layout to out",
                os.path.isfile(os.path.join(out, "manifest.json")), tree(tmp))
        c.check("no repo's own layout is written",
                not any(os.path.exists(os.path.join(r, ".glia", "graph", "manifest.json"))
                        for r in (left, right)), (tree(left), tree(right)))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
