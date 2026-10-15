#!/usr/bin/env python3
"""RSS over repeated rebuilds in one process (DE.10, the engine RSS creep).

Mimics repo-graph's MCP `_build_graph`: `generate(repo, incremental=True)`,
`save_to_default`, swap the live graph (dropping the old one), optionally a
`malloc_trim(0)` of the harness's own, then read VmRSS. Each config runs in a
fresh child process so every run starts from the same heap and the same cold
parse cache; the parent collects the rows.

    ~/.venvs/glia-leap/bin/python bench/rss/rss_loop.py <repo copy> \
        --rebuilds 15 --config glibc --config trim [--label glia@<rev>]

Configs: glibc (nothing), trim (the harness trims after the swap, repo-graph's
wrapper pattern), notrim (GLIA_MALLOC_TRIM=0, the engine's opt-out), arena2
(MALLOC_ARENA_MAX=2), arena2-trim, threads1 (GLIA_THREADS=1), threads1-trim.
`--preload <lib.so>` and `--env K=V` compose an allocator run (name it with
`--name`). The installed glia_py builds; its own drops trim on glia >= 0.5.2,
counted per run as `engine_trims` from the `[memory] malloc_trim` lines.

fired_on, on stdout: `[rss] <repo>:<config> rebuild=<i> rss_mb=<m> hwm_mb=<h>
secs=<s> drop_secs=<d>` per rebuild (`drop_secs`: the swap, i.e. freeing the
old graph plus any trim, engine or harness). Rows append to bench/rss/results.jsonl (`--no-log` to
skip): one `rebuild` row per rebuild and one `run` row per config; the table
printed last is every `run` row in the file. Refuses a repo under ~/Code or
this checkout: the wheel's generate and save_to_default write `.glia/` into it,
so point it at a `git archive` copy.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
RESULTS = HERE / "results.jsonl"
CONFIGS = {
    "glibc": ({}, False), "trim": ({}, True), "notrim": ({"GLIA_MALLOC_TRIM": "0"}, False),
    "arena2": ({"MALLOC_ARENA_MAX": "2"}, False), "arena2-trim": ({"MALLOC_ARENA_MAX": "2"}, True),
    "threads1": ({"GLIA_THREADS": "1"}, False), "threads1-trim": ({"GLIA_THREADS": "1"}, True),
}
LINE = re.compile(r"^\[rss\] \S+ rebuild=(\d+) rss_mb=([\d.]+) hwm_mb=([\d.]+) secs=([\d.]+) drop_secs=([\d.]+)$")


def status_mb(field: str) -> float:
    for line in pathlib.Path("/proc/self/status").read_text().splitlines():
        if line.startswith(field + ":"):
            return round(int(line.split()[1]) / 1024, 1)
    return -1.0


def child(repo: str, rebuilds: int, trim: bool, tag: str) -> int:
    import ctypes
    import glia_py
    malloc_trim = ctypes.CDLL(None).malloc_trim if trim else None
    live = None
    for i in range(1, rebuilds + 1):
        t0 = time.perf_counter()
        pg = glia_py.generate(repo, incremental=True)
        try:
            pg.save_to_default(repo)
        except ValueError as e:
            print(f"[rss] save_to_default failed: {e}", file=sys.stderr)
        t1 = time.perf_counter()
        live, pg = pg, None  # the swap: the old graph drops here
        if malloc_trim is not None:
            malloc_trim(0)
        t2 = time.perf_counter()
        print(f"[rss] {tag} rebuild={i} rss_mb={status_mb('VmRSS')} hwm_mb={status_mb('VmHWM')} "
              f"secs={t2 - t0:.3f} drop_secs={t2 - t1:.3f}", flush=True)
    return 0 if live is not None else 1


def refuse(repo: pathlib.Path) -> str | None:
    for root in (pathlib.Path.home() / "Code", HERE.parents[1]):
        if repo == root.resolve() or root.resolve() in repo.parents:
            return f"{repo} is under {root}: run on a `git archive` copy elsewhere"
    return None if repo.is_dir() else f"{repo} is not a directory"


def run_config(a, repo: pathlib.Path, name: str, env_add: dict, trim: bool, preload: str | None) -> dict:
    import glia_py
    shutil.rmtree(glia_py.default_gmap_dir(str(repo)), ignore_errors=True)  # a cold rebuild 1
    env = {**os.environ, **env_add}
    env.pop("GLIA_NO_PERSIST", None)  # repo-graph's server persists every rebuild
    if preload:
        env["LD_PRELOAD"] = preload
    tag = f"{a.label}:{name}"
    cmd = [sys.executable, __file__, str(repo), "--child", "--rebuilds", str(a.rebuilds), "--tag", tag]
    load_start = os.getloadavg()[0]
    p = subprocess.run(cmd + (["--harness-trim"] if trim else []), env=env, capture_output=True, text=True)
    rows = []
    for line in p.stdout.splitlines():
        m = LINE.match(line)
        if m:
            print(line, flush=True)
            rows.append({"rebuild": int(m[1]), "rss_mb": float(m[2]), "hwm_mb": float(m[3]),
                         "secs": float(m[4]), "drop_secs": float(m[5])})
    if p.returncode != 0 or len(rows) != a.rebuilds:
        sys.exit(f"[rss] {tag}: child exit {p.returncode}, {len(rows)} rows\n{p.stderr[-2000:]}")
    later = rows[1:] or rows[:1]
    now = datetime.datetime.now().replace(microsecond=0)
    return {"kind": "run", "run_id": f"{now.isoformat()} {tag}", "date": now.date().isoformat(), "source": a.source,
            "repo": a.label, "config": name, "rebuilds": a.rebuilds,
            "first_mb": rows[0]["rss_mb"], "last_mb": rows[-1]["rss_mb"],
            "delta_mb": round(rows[-1]["rss_mb"] - rows[0]["rss_mb"], 1),
            "peak_hwm_mb": max(r["hwm_mb"] for r in rows),
            "mean_secs_2_n": round(sum(r["secs"] for r in later) / len(later), 3),
            "mean_drop_secs_2_n": round(sum(r["drop_secs"] for r in later) / len(later), 4),
            "load_1m": [round(load_start, 1), round(os.getloadavg()[0], 1)],
            "engine_trims": p.stderr.count("[memory] malloc_trim site=pygraph_drop"),
            "glia_py": glia_version(), "env": env_add, "preload": preload,
            "harness_trim": trim, "cores": os.cpu_count(), "rows": rows}


def glia_version() -> str:
    from importlib.metadata import version
    return version("glia-py")


def table(path: pathlib.Path) -> None:
    runs = [r for r in map(json.loads, path.read_text().splitlines()) if r.get("kind") == "run"]
    head = ("date", "source", "repo", "config", "n", "first", "last", "delta", "s/rebuild", "drop s",
            "eng.trims", "load")
    num = lambda v, f: "-" if v is None else format(v, f)  # noqa: E731
    cells = [head] + [(r["date"], r["source"], r["repo"], r["config"], str(r.get("rebuilds") or "?"),
                       f"{r['first_mb']:.0f}", f"{r['last_mb']:.0f}", f"{r['last_mb'] - r['first_mb']:+.0f}",
                       num(r.get("mean_secs_2_n"), ".2f"), num(r.get("mean_drop_secs_2_n"), ".3f"),
                       str(r.get("engine_trims", "-")),
                       "/".join(num(x, ".0f") for x in r.get("load_1m") or [None])) for r in runs]
    widths = [max(len(c[i]) for c in cells) for i in range(len(head))]
    for i, c in enumerate(cells):
        print("  ".join(v.ljust(w) for v, w in zip(c, widths)).rstrip())
        if i == 0:
            print("  ".join("-" * w for w in widths))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("repo")
    ap.add_argument("--rebuilds", type=int, default=15)
    ap.add_argument("--config", action="append", choices=sorted(CONFIGS))
    ap.add_argument("--name", help="config name for a --preload / --env run")
    ap.add_argument("--preload", help="LD_PRELOAD an allocator .so for the --name run")
    ap.add_argument("--env", action="append", default=[], help="K=V for the --name run")
    ap.add_argument("--label", help="repo label in rows, e.g. glia@4439808 (default: dir name)")
    ap.add_argument("--source", default="rss_loop", help="who measured the rows")
    ap.add_argument("--no-log", action="store_true", help=f"do not append to {RESULTS.name}")
    ap.add_argument("--child", action="store_true", help=argparse.SUPPRESS)
    ap.add_argument("--harness-trim", action="store_true", help=argparse.SUPPRESS)
    ap.add_argument("--tag", help=argparse.SUPPRESS)
    a = ap.parse_args()
    if a.child:
        return child(a.repo, a.rebuilds, a.harness_trim, a.tag)
    repo = pathlib.Path(a.repo).resolve()
    if (why := refuse(repo)) is not None:
        sys.exit(f"[rss] refused: {why}")
    a.label = a.label or repo.name
    plan = [(c, *CONFIGS[c], None) for c in a.config or []]
    if a.name:
        plan.append((a.name, dict(kv.split("=", 1) for kv in a.env), False, a.preload))
    if not plan:
        sys.exit("[rss] nothing to run: give --config or --name")
    for name, env_add, trim, preload in plan:
        run = run_config(a, repo, name, env_add, trim, preload)
        print(f"[rss] {a.label}:{name} first={run['first_mb']} last={run['last_mb']} "
              f"delta={run['delta_mb']:+} mean_secs={run['mean_secs_2_n']} "
              f"mean_drop_secs={run['mean_drop_secs_2_n']} engine_trims={run['engine_trims']}")
        if not a.no_log:
            rows = run.pop("rows")
            with RESULTS.open("a") as f:
                for r in rows:
                    f.write(json.dumps({"kind": "rebuild", "run_id": run["run_id"], **r}) + "\n")
                f.write(json.dumps(run) + "\n")
    if RESULTS.exists():
        table(RESULTS)
    return 0


if __name__ == "__main__":
    sys.exit(main())
