#!/usr/bin/env python3
"""
Incremental-transparency guard (A1.7): the parse cache must be invisible.

`engine/tests/byte_identical.rs` proves "incremental == clean" for ONE
hand-written repo. This regrades every substrate-gap fixture -- the legacy
`fixtures/` corpus and the `matrix/<lang>/<mech>/` probes, via matrix.py's
`discover` -- three times, on a throwaway copy:

  1. cold  grade.py's own `build_graph`, untouched: today's hermetic path
  2. fill  through the parse cache; the sidecar starts empty
  3. read  through the parse cache again; every file must be REUSED

A fixture fails when pass 3 differs from pass 1 in anything grade.py scores
(node/edge counts, per-category / per-kind / per-cell recall, forbid hits) or
in a digest of the whole graph (every node, edge and cell) -- DIVERGENT -- or
when pass 3 reparsed anything, since then it never read the cache and proved
nothing -- NOT-WARM.

Grading logic is not duplicated: `grade_fixture` runs as-is with its module
global `build_graph` swapped for one call (the seam matrix.py's
`grade_with_kinds` already uses), so grade.py grows no mode switch.
Copies, never the fixture dirs: GLIA_NO_PERSIST=1 gates only the .gmap write,
not `<repo>/.ai/repo-graph/parse_cache.bin`, and a sidecar left under the
bench tree would silently de-hermeticise every later cold grade.

Usage:
  python3 incremental_check.py                   # all fixtures; exit 1 on any failure
  python3 incremental_check.py --only NAME       # basename or rel path; repeatable
  python3 incremental_check.py -v                # echo the engine's captured stderr
  python3 incremental_check.py --selftest [NAME] # negative controls, below

--selftest proves the guard can fail, on one single-dir fixture (default
py-calls). Warm the cache, then make a same-LENGTH, graph-visible edit (an
identifier's last letter; mtime restored) to the sidecar's first entry.
Honest cache: pass 3 reparses exactly that file and reads OK. Forged cache:
the pre-edit sidecar with the edited file's content hash spliced into that
entry is a false hit, and must read DIVERGENT.

Markers (stderr):
  [incremental-check] <name>: cold=Nn/Me graph=H warm=Nn/Me graph=H reused=R reparsed=P OK|DIVERGENT|NOT-WARM
  [incremental-check] <N> fixtures, <D> divergent (cold vs warm)
"""
import argparse
import contextlib
import hashlib
import json
import os
import re
import shutil
import struct
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import grade  # noqa: E402  -- sets GLIA_NO_PERSIST before the wheel loads
from matrix import discover  # noqa: E402

rg = grade.rg
COLD = grade.build_graph
SIDECAR = Path(".ai") / "repo-graph" / "parse_cache.bin"
MARK = re.compile(r"^\[incremental\] (.+): reused (\d+), reparsed (\d+), evicted \d+", re.M)
VERBOSE = False


def say(msg):
    print(f"[incremental-check] {msg}", file=sys.stderr, flush=True)


@contextlib.contextmanager
def fd2_into(sink):
    """The engine's `eprintln!` writes to fd 2 beneath `sys.stderr`, so the
    `[incremental]` marker is only capturable by swapping the descriptor."""
    sys.stderr.flush()
    saved = os.dup(2)
    with tempfile.TemporaryFile() as tmp:
        os.dup2(tmp.fileno(), 2)
        try:
            yield
        finally:
            sys.stderr.flush()
            os.dup2(saved, 2)
            os.close(saved)
            tmp.seek(0)
            sink.append(tmp.read().decode(errors="replace"))


def warm_build(fixture_dir, key):
    """grade.build_graph with the parse cache ON -- its only difference."""
    created = grade._materialize(fixture_dir, key)
    try:
        dirs = [str((fixture_dir / d).resolve()) for d in key.get("dirs", ["."])]
        if len(dirs) == 1:
            g = rg.generate(dirs[0], True)
        else:
            g = rg.generate_many(dirs, incremental=True)
        nodes = json.loads(g.nodes_json())
        return g, nodes, json.loads(g.edges_json()), {n["id"]: n for n in nodes}
    finally:
        grade._dematerialize(created)


def graph_digest(g, nodes, edges):
    rows = sorted(json.dumps([n, sorted(g.node_cells(n["id"]), key=repr)], sort_keys=True)
                  for n in nodes)
    rows += sorted(json.dumps(e, sort_keys=True) for e in edges)
    return hashlib.sha256("\n".join(rows).encode()).hexdigest()[:12]


def grade_pass(work, build):
    """One `grade_fixture` call built by `build`: (signature, engine stderr)."""
    log, digests = [], []

    def spy(fixture_dir, key):
        with fd2_into(log):
            out = build(fixture_dir, key)
        digests.append(graph_digest(*out[:3]))
        return out

    grade.build_graph = spy
    try:
        res = grade.grade_fixture(work)
    finally:
        grade.build_graph = COLD
    return {
        "nodes": res["node_count"], "edges": res["edge_count"],
        "per_category": res["per_category"], "node_by_kind": res["node_by_kind"],
        "per_cell": res["per_cell"],
        "forbid": [r["matched"] for r in res["forbid_results"]],
        "graph": digests[0],
    }, "".join(log)


def verdict(name, cold, warm, log, ndirs, want_reparsed=0):
    marks = MARK.findall(log)
    reused = sum(int(m[1]) for m in marks)
    reparsed = sum(int(m[2]) for m in marks)
    diffs = [k for k in cold if cold[k] != warm[k]]
    state = ("DIVERGENT" if diffs else
             "NOT-WARM" if len(marks) != ndirs or reparsed != want_reparsed else "OK")
    say(f"{name}: cold={cold['nodes']}n/{cold['edges']}e graph={cold['graph']} "
        f"warm={warm['nodes']}n/{warm['edges']}e graph={warm['graph']} "
        f"reused={reused} reparsed={reparsed} {state}")
    for k in diffs:
        say(f"    {k}: cold={cold[k]} warm={warm[k]}")
    if VERBOSE or state != "OK":
        sys.stderr.write(log)
    return state


def rel(p):
    return str(p.relative_to(HERE))


def has_many_incremental():
    """A1.4 added `generate_many(..., incremental=)`; an older wheel lacks it."""
    try:
        rg.generate_many([], incremental=True)
    except TypeError:
        return False
    except ValueError:  # "no graphs produced from 0 paths": the kwarg exists
        return True
    return True


def check(src, work, many_ok):
    ndirs = len(json.loads((src / "key.json").read_text()).get("dirs", ["."]))
    if ndirs > 1 and not many_ok:
        say(f"{rel(src)}: SKIP (installed generate_many has no incremental=, needs A1.4)")
        return "SKIP"
    shutil.copytree(src, work)
    cold, _ = grade_pass(work, COLD)
    grade_pass(work, warm_build)  # fill: the sidecar is written
    warm, log = grade_pass(work, warm_build)  # read: the sidecar is consulted
    return verdict(rel(src), cold, warm, log, ndirs)


def first_entry(blob):
    """(key, offset of its u64 content_hash) in a bincode-1 `ParseCache`:
    stamp, repo_canonical, go_prefix, entry count, then the first key."""
    off = 0

    def text():
        nonlocal off
        (n,) = struct.unpack_from("<Q", blob, off)
        off += 8 + n
        return blob[off - n:off].decode()

    text(), text(), text()
    off += 8
    return text(), off


def selftest(name):
    src = HERE / name if (HERE / name / "key.json").exists() else HERE / "fixtures" / name
    if len(json.loads((src / "key.json").read_text()).get("dirs", ["."])) != 1:
        raise SystemExit(f"selftest needs a single-dir fixture: {name}")
    with tempfile.TemporaryDirectory(prefix="glia-incremental-selftest-") as tmp:
        work = Path(tmp).resolve() / src.name
        shutil.copytree(src, work)
        cold_a, _ = grade_pass(work, COLD)
        grade_pass(work, warm_build)
        side = work / SIDECAR
        stale = side.read_bytes()
        path, at = first_entry(stale)
        f = work / path
        st, text = f.stat(), f.read_text()
        m = re.search(r"\b(?:def|class|func|function|fn)\s+([A-Za-z_]\w{2,})", text)
        if m is None:
            raise SystemExit(f"selftest: no identifier to edit in {path}")
        old = m.group(1)
        new = old[:-1] + ("q" if old[-1] != "q" else "z")
        edited = re.sub(rf"\b{old}\b", new, text)
        if len(edited) != len(text):
            raise SystemExit("selftest: edit changed the file length")
        f.write_text(edited)
        os.utime(f, ns=(st.st_atime_ns, st.st_mtime_ns))
        say(f"selftest {rel(src)}: edited {path} {old}->{new} (same length, mtime kept)")
        cold_b, _ = grade_pass(work, COLD)  # generate(.., False) purges the sidecar
        if cold_b["graph"] == cold_a["graph"]:
            raise SystemExit("selftest: the edit is not graph-visible; pick another fixture")
        side.write_bytes(stale)
        warm, log = grade_pass(work, warm_build)
        honest = verdict(f"selftest {rel(src)} honest-miss", cold_b, warm, log, 1, want_reparsed=1)
        fresh = side.read_bytes()
        fresh_path, fresh_at = first_entry(fresh)
        if fresh_path != path:
            raise SystemExit(f"selftest: first entry moved {path} -> {fresh_path}")
        side.write_bytes(stale[:at] + fresh[fresh_at:fresh_at + 8] + stale[at + 8:])
        warm, log = grade_pass(work, warm_build)
        forged = verdict(f"selftest {rel(src)} forged-hit", cold_b, warm, log, 1)
    ok = honest == "OK" and forged == "DIVERGENT"
    say(f"selftest {'PASS' if ok else 'FAIL'} (honest miss {honest}, forged hit {forged}; "
        f"want OK, DIVERGENT)")
    return 0 if ok else 1


def main():
    global VERBOSE
    ap = argparse.ArgumentParser(description="cold vs warm regrade of every fixture")
    ap.add_argument("--only", action="append", default=[], metavar="NAME")
    ap.add_argument("--selftest", nargs="?", const="py-calls", metavar="NAME")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    VERBOSE = args.verbose
    if args.selftest:
        return selftest(args.selftest)
    fixtures = [p.parent for p in discover(HERE)]
    if args.only:
        fixtures = [f for f in fixtures if f.name in args.only or rel(f) in args.only]
    many_ok = has_many_incremental()
    tally = Counter()
    t0 = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="glia-incremental-check-") as tmp:
        for i, src in enumerate(fixtures):
            try:
                tally[check(src, Path(tmp).resolve() / f"{i:03d}" / src.name, many_ok)] += 1
            except Exception as e:  # noqa: BLE001 -- report every fixture, then fail
                say(f"{rel(src)}: ERROR {type(e).__name__}: {e}")
                tally["ERROR"] += 1
    leaked = sorted(rel(p) for p in HERE.rglob("parse_cache.bin"))
    checked = tally["OK"] + tally["DIVERGENT"] + tally["NOT-WARM"]
    say(f"{checked} fixtures, {tally['DIVERGENT']} divergent (cold vs warm)")
    say(f"not-warm {tally['NOT-WARM']}, skipped {tally['SKIP']}, errors {tally['ERROR']}, "
        f"leaked sidecars {len(leaked)}, wall {time.monotonic() - t0:.1f}s")
    for p in leaked:
        say(f"    leaked: {p}")
    return 1 if tally["DIVERGENT"] + tally["NOT-WARM"] + tally["ERROR"] + len(leaked) else 0


if __name__ == "__main__":
    sys.exit(main())
