#!/usr/bin/env python3
"""Prototype of plan item B (2026-10-26): style-agnostic drift from names across a boundary.
No schema model: the names the server-side change removed, case-normalised, looked up on
the other side of the boundary glia's graph walks to. Scripted, so it can be judged on the
drift tasks before any Rust.

    python3 probe/names.py [--tasks probe/drift-tasks] [--glia <bin>] [--jobs 4] [--out ..]

Per drift task, at S (server half applied; clients stale) and at C (the real commit):
  old names   identifiers and string keys on HEAD~1..HEAD's removed lines that no added
              line keeps (normalised: lower case, `_` / `-` dropped), >= 4 chars, outside
              the language keyword / noise list
  boundary    the files glia's `diff-impact --base HEAD~1` reaches (rows and ranked files),
              minus the changed files
  flagged     boundary files that still hold an old name (as an identifier, a property, a
              key or a string, any case style)
and, as the no-graph control, every tracked code file that holds an old name. A good check
flags the truth client files at S, flags fewer at C, and the graph narrows the control.
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
sys.path.insert(0, str(HERE.parent))
from oracle import D, checkout, sh  # noqa: E402
import tasks as T  # noqa: E402

TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_\-]{3,}")
NOISE = set("""this that true false null none self string number boolean return const
static class public private protected async await function import export default from
type interface extends implements readonly optional required description example
deprecated value values field fields name names data error errors message response
request result results param params query body status code item items list option
options config index count total string_ int64 int32 float uint8 json yaml omitempty
override public void final new delete update create get set post put patch""".split())
CODE = T.CODE_EXTS | {".svelte", ".vue", ".json"}


def norm(s):
    return s.lower().replace("_", "").replace("-", "")


def old_names(diff):
    minus, plus = {}, set()
    for line in diff.splitlines():
        if line.startswith(("---", "+++")):
            continue
        toks = TOKEN.findall(line[1:])
        if line.startswith("-"):
            for t in toks:
                minus.setdefault(norm(t), t)
        elif line.startswith("+"):
            plus |= {norm(t) for t in toks}
    return {n: raw for n, raw in minus.items()
            if n not in plus and n not in NOISE and len(n) >= 4 and not n.isdigit()}


def holds(copy, path, names):
    try:
        text = (copy / path).read_text(errors="replace")
    except OSError:
        return []
    have = {norm(t) for t in TOKEN.findall(text)}
    return sorted(n for n in names if n in have)


def state(glia, copy, truth, env, changed):
    rc, _, err, _ = sh([glia, "build", "."], cwd=copy, env=env, timeout=3600)
    if rc:
        return {"error": f"build rc={rc}"}
    _, diff, _, _ = sh(["git", "diff", "HEAD~1", "HEAD"], cwd=copy)
    names = old_names(diff)
    rc, out, err, ms = sh([glia, "diff-impact", ".", "--base", "HEAD~1", "--json",
                           "--top-files", "200", "--depth", "6"], cwd=copy, env=env)
    boundary = set()
    try:
        imp = (json.loads(out).get("impact") or {})
        boundary |= {f["file"] for f in imp.get("files") or [] if f.get("file")}
        boundary |= {r["file"] for r in imp.get("results") or [] if r.get("file")}
    except ValueError:
        return {"error": f"diff-impact rc={rc} {err.strip()[-120:]}"}
    boundary -= changed
    flagged = {}
    for f in sorted(boundary):
        if os.path.splitext(f)[1] in CODE:
            hit = holds(copy, f, names)
            if hit:
                flagged[f] = hit
    _, ls, _, _ = sh(["git", "ls-files"], cwd=copy)
    control = {}
    for f in ls.splitlines():
        if f in changed or os.path.splitext(f)[1] not in CODE or T.is_test(f):
            continue
        hit = holds(copy, f, names)
        if hit:
            control[f] = hit
    t = set(truth)
    return {"names": len(names), "name_sample": sorted(names.values())[:12],
            "boundary": len(boundary), "boundary_has_truth": sorted(t & boundary),
            "flagged": len(flagged), "flagged_truth": sorted(t & set(flagged)),
            "control": len(control), "control_truth": sorted(t & set(control)),
            "truth_hits": {f: flagged.get(f) or control.get(f) for f in truth},
            "ms": ms}


def one(row, glia):
    gd = D / f"{row['repo']}.git"
    env = {**os.environ, "GLIA_THREADS": "4"}
    truth = [c["path"] for c in row["truth"]["code"]]
    changed = set(row["probe"]["server_files"])
    out = {"task": row["id"], "truth": truth}
    for name, rev in (("S", row["parent"]), ("C", row["commit"])):
        copy = D / "names" / "work" / f"{row['id']}-{name}"
        try:
            checkout(gd, rev, copy)
            ch = changed | (set(truth) if name == "C" else set())
            out[name] = state(glia, copy, truth, env, ch if name == "S" else changed)
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
    ap.add_argument("--out", default=str(HERE / "names.jsonl"))
    a = ap.parse_args()
    rows = [json.loads(l) for f in sorted(Path(a.tasks).glob("*.jsonl"))
            for l in f.read_text().splitlines() if l.strip()]
    if a.only:
        rows = [r for r in rows if re.search(a.only, r["id"])]
    res = []
    with ThreadPoolExecutor(a.jobs) as pool:
        for r in pool.map(lambda r: one(r, a.glia), rows):
            res.append(r)
            s, c = r.get("S", {}), r.get("C", {})
            print(f"[names] {r['task']:26} names={s.get('names')} boundary={s.get('boundary')} "
                  f"(truth in it {len(s.get('boundary_has_truth', []))}/{len(r['truth'])}) "
                  f"flagged={s.get('flagged')} truth={len(s.get('flagged_truth', []))} | "
                  f"control={s.get('control')} truth={len(s.get('control_truth', []))} | "
                  f"C flagged={c.get('flagged')} {s.get('error') or c.get('error') or ''}",
                  flush=True)
    Path(a.out).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in res))


if __name__ == "__main__":
    main()
