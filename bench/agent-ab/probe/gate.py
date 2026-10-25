#!/usr/bin/env python3
"""Gate probe (FE.4 follow-up, 2026-10-26), no model: does glia's PR gate see a real
cross-stack drift, and does it clear once the authors' client fix is in?

    python3 probe/gate.py [--tasks probe/drift-tasks] [--glia <bin>] [--jobs 4] [--out ..]

For each drift task: at S (the server half; the clients stale) and at C (the real commit),
`contract-breaks --base HEAD~1 --json` and `review --base HEAD~1 --json`. A good gate:
flags at S, names a client truth file (a site the agent must change), folds one change
into one row, and at C reports fewer or no breaking rows. Recorded per state: breaking /
advisory / models / orphaned_clients counts, distinct (section, field, change) triples
(how many real changes the rows are), the files the rows name, which truth files they
name, review's blocking flag, bytes and ms.
"""
import argparse
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from oracle import D, checkout, sh  # noqa: E402


def files_in(obj, acc):
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == "file" and isinstance(v, str):
                acc.add(v)
            else:
                files_in(v, acc)
    elif isinstance(obj, list):
        for v in obj:
            files_in(v, acc)
    return acc


def changes_in(rows):
    out = set()
    for r in rows:
        for c in r.get("changes") or []:
            out.add((c.get("section"), c.get("field"), c.get("change")))
        if not r.get("changes"):
            out.add((r.get("kind"), r.get("key"), r.get("change")))
    return out


def state(glia, copy, truth, env):
    res = {}
    rc, _, err, ms = sh([glia, "build", "."], cwd=copy, env=env, timeout=3600)
    if rc:
        return {"error": f"build rc={rc} {err.strip()[-150:]}"}
    rc, out, err, ms = sh([glia, "contract-breaks", ".", "--base", "HEAD~1", "--json"],
                          cwd=copy, env=env)
    try:
        d = json.loads(out)
        rows = [r for k in ("schemas", "models", "orphaned_clients") for r in d.get(k) or []]
        named = files_in(rows, set())
        res["cb"] = {"breaking": d.get("breaking"), "advisory": d.get("advisory"),
                     "schemas": len(d.get("schemas") or []), "models": len(d.get("models") or []),
                     "orphaned_clients": len(d.get("orphaned_clients") or []),
                     "distinct_changes": len(changes_in(rows)), "files_named": len(named),
                     "truth_named": sorted(t for t in truth if t in named),
                     "bytes": len(out.encode()), "ms": ms, "rc": rc}
    except ValueError:
        res["cb"] = {"error": f"rc={rc} {err.strip()[-150:]}"}
    rc, out, err, ms = sh([glia, "review", ".", "--base", "HEAD~1", "--json"], cwd=copy, env=env)
    try:
        d = json.loads(out)
        named = files_in(d, set())
        res["review"] = {"rc": rc, "truth_named": sorted(t for t in truth if t in named),
                         "files_named": len(named), "bytes": len(out.encode()), "ms": ms,
                         "counts": d.get("counts")}
    except ValueError:
        res["review"] = {"error": f"rc={rc} {err.strip()[-150:]}"}
    return res


def one(row, glia):
    gd = D / f"{row['repo']}.git"
    env = {**os.environ, "GLIA_THREADS": "4"}
    truth = [c["path"] for c in row["truth"]["code"]]
    out = {"task": row["id"], "truth": truth}
    for name, rev in (("S", row["parent"]), ("C", row["commit"])):
        copy = D / "gate" / "work" / f"{row['id']}-{name}"
        try:
            checkout(gd, rev, copy)
            out[name] = state(glia, copy, truth, env)
        except Exception as e:  # noqa: BLE001
            out[name] = {"error": str(e)[:200]}
        finally:
            sh(["rm", "-rf", str(copy)])
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", default=str(HERE / "drift-tasks"))
    ap.add_argument("--glia", default=str(D / "bin" / "g053" / "glia"))
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--only")
    ap.add_argument("--out", default=str(HERE / "gate.jsonl"))
    a = ap.parse_args()
    rows = [json.loads(l) for f in sorted(Path(a.tasks).glob("*.jsonl"))
            for l in f.read_text().splitlines() if l.strip()]
    if a.only:
        rows = [r for r in rows if re.search(a.only, r["id"])]
    res = []
    with ThreadPoolExecutor(a.jobs) as pool:
        for r in pool.map(lambda r: one(r, a.glia), rows):
            res.append(r)
            s, c = r.get("S", {}).get("cb", {}), r.get("C", {}).get("cb", {})
            print(f"[gate] {r['task']:26} S breaking={s.get('breaking')} distinct={s.get('distinct_changes')} "
                  f"models={s.get('models')} truth_named={len(s.get('truth_named', []))}/{len(r['truth'])} "
                  f"review_truth={len(r.get('S', {}).get('review', {}).get('truth_named', []))} | "
                  f"C breaking={c.get('breaking')} {s.get('error') or c.get('error') or ''}", flush=True)
    Path(a.out).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in res))


if __name__ == "__main__":
    main()
