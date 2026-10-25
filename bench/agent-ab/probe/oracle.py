#!/usr/bin/env python3
"""The no-model oracle (FE.4 follow-up, 2026-10-26): for every task, what glia's own best
answer finds against what a scripted grep finds, before any agent session is spent.

    python3 probe/oracle.py <tasks_dir>=<repos.json> [...] [--glia <bin>] [--jobs 6]
                            [--calibrate] [--out probe/oracle.jsonl]

Per task, at its start commit (the row's `parent`), two questions, each answered by glia
and by grep and scored as recall@10 (and the rank of the first truth file) over the truth
files the question can still find:

  L  locate: from the prompt text alone. glia: `find --files <text>`; grep: the prompt's
     words and identifiers (>= 4 chars, no stopwords) through `git grep -l -i -w`, files
     ranked by distinct words matched, then hits.
  R  rest: given the core of the change, the other files. The seed is the server half
     for a drift task (already HEAD; glia `diff-impact --base HEAD~1`), else the truth
     file with the most changed lines, applied to the work tree from the task commit
     (glia `diff-impact`). grep: the identifiers on the seed diff's changed lines that
     appear on only one side (renames, removals, additions), then every changed-line
     identifier, through `git grep -l -w`; seed files excluded on both sides.

Also recorded: answer bytes and milliseconds, glia errors, and for a drift task the
`contract-breaks --base HEAD~1` row count. Stdlib only; checkouts under
${GLIA_AGENT_AB_DIR}/oracle/.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import tasks as T  # noqa: E402

D = Path(os.environ.get("GLIA_AGENT_AB_DIR", "~/.cache/glia-agent-ab")).expanduser()
STOP = set("""this that with from into when then than they them their there these those have
has had were been being will would should could about after before under over also only
just some such each every other more most make made does done doing change changes changed
update updated updates fix fixed fixes issue issues add added adds remove removed removes
use used uses using allow allows support supports supported work works working server
client code file files function method class value values make sure when what which while
where your need needs latest commit branch head were longer match matches below github
resolve test tests version""".split())
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]{3,}")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*[A-Z_][A-Za-z0-9_]*|[a-z][a-z0-9]+(?:_[a-z0-9]+)+")


def sh(cmd, cwd=None, env=None, timeout=1800, inp=None):
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, timeout=timeout,
                           input=inp)
        return p.returncode, p.stdout.decode("utf-8", "replace"), \
            p.stderr.decode("utf-8", "replace"), int((time.monotonic() - t0) * 1000)
    except subprocess.TimeoutExpired:
        return 124, "", "timeout", int((time.monotonic() - t0) * 1000)


def checkout(gd, rev, copy):
    if copy.exists():
        shutil.rmtree(copy)
    copy.mkdir(parents=True)
    for cmd in (["git", "init", "-q"], ["git", "fetch", "-q", "--no-tags", str(gd), rev],
                ["git", "checkout", "-q", "--detach", "FETCH_HEAD"]):
        rc, _, err, _ = sh(cmd, cwd=copy)
        if rc:
            raise RuntimeError(f"{' '.join(cmd)}: {err.strip()}")


def recall(ranked, truth, k=10):
    truth = set(truth)
    if not truth:
        return None, None
    top = ranked[:k]
    first = next((i + 1 for i, f in enumerate(ranked) if f in truth), None)
    return len(truth & set(top)) / len(truth), first


def grep_rank(copy, words, exclude):
    score = {}
    for w in words:
        rc, out, _, _ = sh(["git", "grep", "-l", "-i", "-w", "-I", w, "--", ".",
                            ":!*.lock", ":!*lock.json", ":!*.min.js"], cwd=copy, timeout=120)
        for f in out.splitlines():
            if f and f not in exclude:
                s = score.setdefault(f, [0, 0])
                s[0] += 1
    return [f for f, _ in sorted(score.items(), key=lambda kv: (-kv[1][0], kv[0]))]


def prompt_words(text):
    seen, out = set(), []
    for w in WORD.findall(text):
        lw = w.lower()
        if lw not in STOP and lw not in seen:
            seen.add(lw)
            out.append(w)
    return out[:25]


def diff_idents(diff):
    minus, plus = set(), set()
    for line in diff.splitlines():
        if line.startswith("-") and not line.startswith("---"):
            minus |= set(IDENT.findall(line[1:]))
        elif line.startswith("+") and not line.startswith("+++"):
            plus |= set(IDENT.findall(line[1:]))
    one_side = sorted((minus ^ plus), key=lambda s: (-len(s), s))
    both = sorted((minus & plus), key=lambda s: (-len(s), s))
    return (one_side + both)[:25]


def glia_json(glia, args, copy, env):
    rc, out, err, ms = sh([glia, *args], cwd=copy, env=env)
    try:
        return json.loads(out), len(out.encode()), ms, None
    except ValueError:
        return None, len(out.encode()), ms, f"rc={rc} {err.strip().splitlines()[-1:]}"


def one(task, repo, glia, calibrate):
    row = task
    rid = row["id"]
    gd = D / f"{repo['label']}.git"
    copy = D / "oracle" / "work" / rid
    env = {**os.environ, "GLIA_THREADS": "4"}
    env.pop("GLIA_NO_PERSIST", None)
    res = {"task": rid, "repo": repo["label"], "shape": row.get("shape", "fe1"),
           "truth": [c["path"] for c in row["truth"]["code"]]}
    try:
        checkout(gd, row["parent"], copy)
        t0 = time.monotonic()
        steps = [["history", "sync", "."], ["build", "."]] + \
            ([["history", "calibrate", "."]] if calibrate else [])
        for st in steps:
            rc, _, err, _ = sh([glia, *st], cwd=copy, env=env, timeout=3600)
            if rc:
                res["prep_error"] = f"{' '.join(st)}: rc={rc} {err.strip()[-200:]}"
                return res
        res["prep_ms"] = int((time.monotonic() - t0) * 1000)
        truth = res["truth"]
        # L: locate from the prompt
        text = row["prompt_subject"] + (" " + row["issue"]["title"] + " " + row["issue"]["body"]
                                        if row.get("issue") else "")
        if row.get("shape") == "drift":
            text = row["subject"]
        d, nb, ms, err = glia_json(glia, ["find", "--files", ".", text[:500], "--json",
                                          "--top-k", "20"], copy, env)
        g_files = [r["file"] for r in (d or {}).get("results", [])]
        w = prompt_words(text)
        t1 = time.monotonic()
        r_files = grep_rank(copy, w, set())
        res["L"] = {"glia": recall(g_files, truth), "grep": recall(r_files, truth),
                    "glia_bytes": nb, "glia_ms": ms, "glia_err": err,
                    "grep_ms": int((time.monotonic() - t1) * 1000), "words": w[:10],
                    "glia_top": g_files[:5], "grep_top": r_files[:5]}
        # R: the rest of the change, given its core
        if row.get("shape") == "drift":
            seed = set(row["probe"]["server_files"])
            _, diff, _, _ = sh(["git", "diff", "HEAD~1", "HEAD"], cwd=copy)
            base = ["--base", "HEAD~1"]
        else:
            core = max(row["truth"]["code"], key=lambda c: sum(h[1] for h in c["hunks"]) or 1)
            seed = {core["path"]}
            rc, blob, err, _ = sh(["git", "--git-dir", str(gd), "show",
                                   f"{row['commit']}:{core['path']}"])
            if rc:
                res["R"] = {"error": f"seed: {err.strip()[:120]}"}
                return res
            p = copy / core["path"]
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(blob)
            _, diff, _, _ = sh(["git", "diff"], cwd=copy)
            base = []
        rest = [f for f in truth if f not in seed]
        d, nb, ms, err = glia_json(glia, ["diff-impact", ".", *base, "--json", "--top-files",
                                          "20"], copy, env)
        g_files = [f["file"] for f in ((d or {}).get("impact") or {}).get("files", [])
                   if f["file"] not in seed]
        ids = diff_idents(diff)
        t1 = time.monotonic()
        r_files = grep_rank(copy, ids, seed)
        res["R"] = {"rest": len(rest), "glia": recall(g_files, rest), "grep": recall(r_files, rest),
                    "glia_bytes": nb, "glia_ms": ms, "glia_err": err,
                    "grep_ms": int((time.monotonic() - t1) * 1000), "idents": ids[:10],
                    "glia_top": g_files[:5], "grep_top": r_files[:5]}
        if row.get("shape") == "drift":
            d, nb, ms, err = glia_json(glia, ["contract-breaks", ".", "--base", "HEAD~1", "--json"],
                                       copy, env)
            rows = 0
            if isinstance(d, dict):
                rows = sum(len(v) for v in d.values() if isinstance(v, list))
            res["contract_breaks"] = {"rows": rows, "ms": ms, "err": err}
    except Exception as e:  # noqa: BLE001 - one task's failure is a row, not a stop
        res["error"] = str(e)[:300]
    finally:
        shutil.rmtree(copy, ignore_errors=True)
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("sets", nargs="+", help="<tasks_dir>=<repos.json>")
    ap.add_argument("--glia", default=str(D / "bin" / "g053" / "glia"))
    ap.add_argument("--jobs", type=int, default=6)
    ap.add_argument("--calibrate", action="store_true")
    ap.add_argument("--only", help="regex over task ids")
    ap.add_argument("--out", default=str(HERE / "oracle.jsonl"))
    a = ap.parse_args()
    work = []
    for spec in a.sets:
        tdir, repos = spec.split("=", 1)
        by = {r["label"]: r for r in T.load_repos(Path(repos))}
        for f in sorted(Path(tdir).glob("*.jsonl")):
            for line in f.read_text().splitlines():
                if line.strip():
                    row = json.loads(line)
                    if a.only and not re.search(a.only, row["id"]):
                        continue
                    work.append((row, by[row["repo"]]))
    print(f"[oracle] tasks={len(work)} jobs={a.jobs} glia={a.glia}", flush=True)
    out = []
    with ThreadPoolExecutor(a.jobs) as pool:
        for res in pool.map(lambda w: one(w[0], w[1], a.glia, a.calibrate), work):
            out.append(res)
            L, R = res.get("L", {}), res.get("R", {})
            print(f"[oracle] {res['task']:28} {res['shape']:7} "
                  f"L glia={L.get('glia')} grep={L.get('grep')} | "
                  f"R glia={R.get('glia')} grep={R.get('grep')} rest={R.get('rest')}"
                  f"{' ERR ' + str(res.get('error') or res.get('prep_error')) if res.get('error') or res.get('prep_error') else ''}",
                  flush=True)
    Path(a.out).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in out))


if __name__ == "__main__":
    main()
