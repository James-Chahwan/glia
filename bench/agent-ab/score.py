#!/usr/bin/env python3
"""The agent before / after scorer (FE.3): each FE.2 session scored against what
its task commit changed, then per-task means, paired arm differences with a
task-cluster bootstrap, the markdown report and baseline.json.

    python3 score.py score <run_id> [--out-md <f>] [--out-json <f>] [--out-sessions <f>]
    python3 score.py score --from-metrics <sessions metrics jsonl> [--out-md ..] [--out-json ..]
    python3 score.py compare <old baseline.json> <new baseline.json>

`score <run_id>` reads ${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}/runs/<id>/
(meta.json, sessions.jsonl), the patches under DIR/patches/<id>/ and the task rows
in tasks/. `--out-sessions` writes the per-session metric rows (no transcript,
patch or path outside the task repo), and `--from-metrics` scores from such a
file alone, so a committed metrics file reproduces the committed baseline byte
for byte. Markers on stdout: `[agent-ab] score sessions=<n> excluded=<k>
tasks=<t>` and, per arm and metric, `[agent-ab] compare arm=<a> metric=<m>
diff=<d> ci=[lo,hi] verdict=<v>`. Stdlib only.
"""
import argparse
import json
import random
import statistics
import sys
import time
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run  # noqa: E402
import tasks  # noqa: E402

HERE = tasks.HERE
EXCLUDED = ("contaminated", "network", "prep_failed", "error")
SKIP_PREFIXES = (".glia/", ".claude/")
# Each scored metric and its good direction (+1 higher is better, -1 lower).
DIRECTION = {"solved": 1, "file_recall": 1, "file_precision": 1, "line_overlap": 1,
             "opened": -1, "first_truth_open": -1, "tokens_in": -1, "tokens_out": -1,
             "usd": -1, "turns": -1, "wall_s": -1}
METRICS = list(DIRECTION) + ["glia_calls"]  # glia_calls is reported, never judged
QUALITY = ("solved", "file_recall", "line_overlap")
PAIRS = (("g052", "none"), ("g053", "none"), ("g053", "g052"))
RESAMPLES, SEED, WIDEN, MIN_SPLIT = 2000, 20261023, 3, 5
SCHEMA = 1


class ScoreError(Exception):
    pass


def fmt(x):
    if x is None:
        return "-"
    r = round(x, 3)
    return f"{(r if r != 0 else 0.0):.3f}"


def rnd(x):
    return None if x is None else (round(x, 6) if round(x, 6) != 0 else 0.0)


# --- one patch ---------------------------------------------------------------
def _unquote(p):
    if p.startswith('"') and p.endswith('"'):
        p = p[1:-1].encode("latin-1", "backslashreplace").decode("unicode_escape")
        p = p.encode("latin-1", "replace").decode("utf-8", "replace")
    return p


def parse_patch(text):
    """{path: {"status", "old", "hunks": [[s, l]]}} for a `git diff -U0 HEAD` text.
    A file's key is its new path (its old path when deleted); hunks are `@@ -s,l`."""
    files, cur = {}, None

    def close():
        if cur is not None:
            old, new = cur["old"], cur["new"]
            status = "A" if old is None else "D" if new is None else (
                "R" if old != new else "M")
            files[new if new is not None else old] = {"status": status, "old": old,
                                                      "hunks": cur["hunks"]}

    for line in text.splitlines():
        if line.startswith("diff --git "):
            close()
            rest = line[len("diff --git "):]
            a, _, b = rest.partition(" b/") if not rest.startswith('"') else (rest, "", "")
            a = _unquote(a)[2:] if a.startswith(("a/", '"a/')) else a
            cur = {"old": a or None, "new": _unquote(b) if b else a or None, "hunks": []}
        elif cur is None:
            continue
        elif line.startswith("--- "):
            p = _unquote(line[4:].rstrip("\t"))
            cur["old"] = None if p == "/dev/null" else p[2:] if p.startswith("a/") else p
        elif line.startswith("+++ "):
            p = _unquote(line[4:].rstrip("\t"))
            cur["new"] = None if p == "/dev/null" else p[2:] if p.startswith("b/") else p
        elif line.startswith("rename from "):
            cur["old"] = _unquote(line[len("rename from "):])
        elif line.startswith("rename to "):
            cur["new"] = _unquote(line[len("rename to "):])
        elif line.startswith("new file mode"):
            cur["old"] = None
        elif line.startswith("deleted file mode"):
            cur["new"] = None
        else:
            m = tasks.HUNK_RE.match(line)
            if m:
                cur["hunks"].append([int(m.group(1)),
                                     1 if m.group(2) is None else int(m.group(2))])
    close()
    return files


def counted(path):
    return (path and not path.startswith(SKIP_PREFIXES) and tasks.is_code(path)
            and not tasks.is_test(path))


def _overlaps(s, l, agent):
    lo, hi = s, s + max(l, 1)
    return any(lo < a + max(b, 1) + WIDEN and a - WIDEN < hi for a, b in agent)


def session_metrics(row, task, patch_text):
    """Step 1's metrics of one scored session."""
    code = task["truth"]["code"]
    truth = {c["old_path"] if c["status"] == "D" and c.get("old_path") else c["path"]
             for c in code}
    # The agent works at the parent, where a renamed file still has its old name.
    open_truth = truth | {c["old_path"] for c in code if c["status"] == "R" and c["old_path"]}
    files = parse_patch(patch_text)
    edited = {p for p in files if counted(p)}
    hit = edited & truth
    agent_hunks = {}
    for p, f in files.items():
        if f["old"] is not None:
            agent_hunks.setdefault(f["old"], []).extend(f["hunks"])
    truth_hunks = [(c["path"], s, l) for c in code if c["status"] == "M" for s, l in c["hunks"]]
    overlap = (sum(_overlaps(s, l, agent_hunks.get(p, [])) for p, s, l in truth_hunks)
               / len(truth_hunks)) if truth_hunks else None
    opened = row.get("files_opened") or []
    first = next((i + 1 for i, p in enumerate(opened) if p in open_truth), None)
    usage = row.get("usage") or {}
    return {"solved": 1.0 if truth <= edited and len(edited - truth) <= max(2, len(truth))
            else 0.0,
            "file_recall": len(hit) / len(truth),
            "file_precision": len(hit) / len(edited) if edited else 0.0,
            "line_overlap": overlap, "first_truth_open": first, "opened": len(opened),
            "tokens_in": (usage.get("input_tokens", 0) + usage.get("cache_creation_input_tokens", 0)
                          + usage.get("cache_read_input_tokens", 0)),
            "tokens_out": usage.get("output_tokens", 0),
            "usd": float(row.get("total_cost_usd") or 0.0), "turns": row.get("num_turns") or 0,
            "wall_s": (row.get("wall_ms") or 0) / 1000.0,
            "glia_calls": len(row.get("glia_calls") or [])}


# --- reading a run -------------------------------------------------------------
def metrics_from_run(run_id, d, repos_path, tasks_dir):
    """(meta, metric rows) of DIR/runs/<run_id>, step 1 over every session row."""
    run_dir = d / "runs" / run_id
    meta = run.read_json(run_dir / "meta.json")
    if meta is None:
        raise ScoreError(f"no run meta at {run_dir / 'meta.json'}")
    task_list, shas = run.load_tasks(repos_path, tasks_dir)
    if shas != meta.get("tasks"):
        changed = sorted(k for k in set(shas) | set(meta.get("tasks") or {})
                         if shas.get(k) != (meta.get("tasks") or {}).get(k))
        raise ScoreError(f"refused: task files differ from run {run_id}'s ({', '.join(changed)})")
    by_id = {t["id"]: (repo, t) for repo, t in task_list}
    stamp = time.gmtime((run_dir / "meta.json").stat().st_mtime)
    head = {"kind": "meta", "run_id": run_id, "date": time.strftime("%Y-%m-%d", stamp),
            "model": meta.get("model"), "claude_version": meta.get("claude_version"),
            "glia": {a: m["sha"] for a, m in sorted((meta.get("arms") or {}).items())},
            "tasks": {f"{k}.jsonl": v for k, v in sorted(shas.items())},
            "prompt_sha256": meta.get("prompt"), "config_sha256": meta.get("config")}
    rows = []
    for r in run.read_rows(run_dir / "sessions.jsonl"):
        if r["task_id"] not in by_id:
            raise ScoreError(f"session row names an unknown task {r['task_id']}")
        repo, task = by_id[r["task_id"]]
        out = {"kind": "session", "task_id": r["task_id"], "repo": repo["label"],
               "arm": r["arm"], "rep": r["rep"], "terminated": r.get("terminated"),
               "tuned": bool(repo.get("tuned")), "after_gate_rev": task.get("after_gate_rev"),
               "patch_sha256": None, "metrics": None, "glia_subcommands": []}
        if r.get("terminated") not in EXCLUDED:
            key = f"{r['task_id']}.{r['arm']}.{r['rep']}"
            path = d / "patches" / run_id / f"{key}.diff"
            data = path.read_bytes() if path.is_file() else b""
            want = (r.get("patch") or {}).get("sha256")
            if want is not None and run.sha256(data) != want:
                raise ScoreError(f"patch {path} does not match its session row's sha256")
            out["patch_sha256"] = run.sha256(data)
            out["metrics"] = session_metrics(r, task, data.decode("utf-8", "replace"))
            out["glia_subcommands"] = [c.get("subcommand") or "" for c in r.get("glia_calls")
                                       or []]
        rows.append(out)
    rows.sort(key=lambda x: (x["task_id"], x["arm"], x["rep"]))
    return head, rows


def write_metrics(path, head, rows):
    lines = [json.dumps(head, sort_keys=True)] + [json.dumps(r, sort_keys=True) for r in rows]
    Path(path).write_text("\n".join(lines) + "\n")


def read_metrics(path):
    rows = run.read_rows(path)
    heads = [r for r in rows if r.get("kind") == "meta"]
    if len(heads) != 1:
        raise ScoreError(f"{path}: expected one meta line, found {len(heads)}")
    return heads[0], [r for r in rows if r.get("kind") == "session"]


# --- aggregation -------------------------------------------------------------
def mean(xs):
    xs = [x for x in xs if x is not None]
    return sum(xs) / len(xs) if xs else None


def task_means(rows):
    """{(task_id, arm): {"repo", "tuned", "after_gate_rev", "n", "metrics"}} over scored reps."""
    groups = {}
    for r in rows:
        if r["metrics"] is not None:
            groups.setdefault((r["task_id"], r["arm"]), []).append(r)
    out = {}
    for key, rs in groups.items():
        out[key] = {"repo": rs[0]["repo"], "tuned": rs[0]["tuned"],
                    "after_gate_rev": rs[0]["after_gate_rev"], "n": len(rs),
                    "metrics": {m: mean([r["metrics"][m] for r in rs]) for m in METRICS}}
    return out


def bootstrap(diffs):
    """(diff, lo, hi): the mean paired difference and its task-cluster 95% percentile CI."""
    n = len(diffs)
    rng = random.Random(SEED)
    stats = sorted(sum(diffs[rng.randrange(n)] for _ in range(n)) / n for _ in range(RESAMPLES))
    return sum(diffs) / n, stats[int(0.025 * RESAMPLES)], stats[int(0.975 * RESAMPLES) - 1]


def verdict(metric, lo, hi, n):
    if n < 2:  # one task gives no interval
        return "n.s."
    good = DIRECTION[metric]
    if lo > 0 or hi < 0:
        return "better" if (lo > 0) == (good > 0) else "worse"
    return "n.s."


def paired(tm, a, b, keep=lambda info: True):
    """{metric: {diff, lo, hi, verdict, n}} for arm a minus arm b over shared tasks."""
    tids = sorted({t for t, arm in tm if arm == a and (t, b) in tm and keep(tm[(t, a)])})
    out = {}
    for m in DIRECTION:
        diffs = [tm[(t, a)]["metrics"][m] - tm[(t, b)]["metrics"][m] for t in tids
                 if tm[(t, a)]["metrics"][m] is not None and tm[(t, b)]["metrics"][m] is not None]
        if not diffs:
            out[m] = {"diff": None, "lo": None, "hi": None, "verdict": "n.s.", "n": 0}
            continue
        d, lo, hi = bootstrap(diffs)
        out[m] = {"diff": rnd(d), "lo": rnd(lo), "hi": rnd(hi),
                  "verdict": verdict(m, lo, hi, len(diffs)), "n": len(diffs)}
    return out


def subsets(tm):
    """[(name, predicate)]: pooled, tuned / untuned, the after_gate_rev split when both
    sides hold >= 5 tasks, then each repo."""
    out = [("pooled", lambda i: True), ("tuned", lambda i: i["tuned"]),
           ("untuned", lambda i: not i["tuned"])]
    sides = (True, False)
    if all(len({t for (t, _), i in tm.items() if i["after_gate_rev"] is side}) >= MIN_SPLIT
           for side in sides):
        out += [(f"after_gate={str(side).lower()}", lambda i, s=side: i["after_gate_rev"] is s)
                for side in sides]
    for repo in sorted({i["repo"] for i in tm.values()}):
        out.append((f"repo={repo}", lambda i, r=repo: i["repo"] == r))
    return out


def reading(pair_metrics):
    """The stated reading rule over one pair's verdicts."""
    v = {m: pair_metrics[m]["verdict"] for m in pair_metrics}
    if any(v[m] == "worse" for m in QUALITY):
        return "HURTS"
    if v["solved"] == "better" or v["file_recall"] == "better":
        return "HELPS"
    if all(v[m] == "n.s." for m in QUALITY) and "better" in (v["tokens_in"], v["usd"]):
        return "HELPS"
    return "NO MEASURED EFFECT"


def arm_means(tm, arm, keep=lambda info: True):
    return {m: mean([i["metrics"][m] for (t, a), i in tm.items() if a == arm and keep(i)])
            for m in METRICS}


def score(head, rows):
    """(baseline dict, markdown text, marker line)."""
    tm = task_means(rows)
    arms = sorted({a for _, a in tm})
    pairs = [(a, b) for a, b in PAIRS if a in arms and b in arms]
    subs = subsets(tm)
    by_subset = {name: {f"{a}-{b}": paired(tm, a, b, keep) for a, b in pairs}
                 for name, keep in subs}
    baseline = {
        "schema": SCHEMA, "run_id": head["run_id"],
        "fingerprints": {"model": head["model"], "claude_version": head["claude_version"],
                         "glia": head["glia"], "tasks": head["tasks"],
                         "prompt_sha256": head["prompt_sha256"],
                         "config_sha256": head["config_sha256"]},
        "per_task": [{"task_id": t, "arm": a, "n": i["n"],
                      "metrics": {m: rnd(i["metrics"][m]) for m in METRICS}}
                     for (t, a), i in sorted(tm.items())],
        "pooled": {a: {m: rnd(v) for m, v in arm_means(tm, a).items()} for a in arms},
        "pairs": {p: {m: {k: v[k] for k in ("diff", "lo", "hi", "verdict")}
                      for m, v in ms.items()} for p, ms in by_subset["pooled"].items()},
        "pairs_by_subset": by_subset,
    }
    scored = [r for r in rows if r["metrics"] is not None]
    excluded = Counter(r["terminated"] for r in rows if r["metrics"] is None)
    marker = (f"[agent-ab] score sessions={len(scored)} excluded={sum(excluded.values())} "
              f"tasks={len({r['task_id'] for r in scored})}")
    return baseline, render_md(head, rows, tm, arms, by_subset, subs, excluded), marker


# --- the report --------------------------------------------------------------
RULE = """\
Decided before the numbers. An arm HELPS an agent when, pooled, `solved` or `file_recall` is
`better` and no quality metric (`solved`, `file_recall`, `line_overlap`) is `worse`; or when every
quality metric is `n.s.` and `tokens_in` or `usd` is `better`. It HURTS when a quality metric is
`worse`. Otherwise NO MEASURED EFFECT. A verdict is `better` / `worse` when the 95% task-cluster
bootstrap interval (2,000 resamples, seed 20261023) of the paired per-task difference excludes 0
in the metric's good direction (higher: solved, file_recall, file_precision, line_overlap; lower:
opened, first_truth_open, tokens_in, tokens_out, usd, turns, wall_s), else `n.s.`. The g053-g052
line (0.5.3 rankings against 0.5.2) uses the same rule, pooled and on the untuned subset
(commons-text, microservices-demo, click); where the two disagree, the pooled effect rests on the
repos 0.5.3 was tuned on. Success is file-level against one commit's diff, so a valid alternative
fix in another file counts as a miss; `line_overlap` and `file_recall` show partial credit."""


def table(header, rows):
    out = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return out


def pair_table(ms):
    return table(["pair", "metric", "n", "diff", "95% CI", "verdict"],
                 [[p, m, v["n"], fmt(v["diff"]), f"[{fmt(v['lo'])}, {fmt(v['hi'])}]",
                   v["verdict"]] for p, mm in ms.items() for m, v in mm.items()])


def render_md(head, rows, tm, arms, by_subset, subs, excluded):
    scored = [r for r in rows if r["metrics"] is not None]
    L = [f"# Agent before / after: {head['run_id']}", "",
         f"- date: {head['date']}", f"- run_id: {head['run_id']}", f"- model: {head['model']}",
         f"- claude_version: {head['claude_version']}"]
    L += [f"- glia {a}: {s}" for a, s in sorted(head["glia"].items())]
    L += [f"- tasks {f}: {s}" for f, s in sorted(head["tasks"].items())]
    L += [f"- prompt sha256: {head['prompt_sha256']}", f"- config sha256: {head['config_sha256']}",
          f"- sessions scored: {len(scored)} over {len({r['task_id'] for r in scored})} tasks",
          "- sessions excluded: " + (", ".join(f"{k} {v}" for k, v in sorted(excluded.items()))
                                     or "none"), "", "## Reading rule", "", RULE, ""]
    L += ["## Outcome", ""]
    for p in by_subset["pooled"]:
        L.append(f"- {p} pooled: {reading(by_subset['pooled'][p])}")
    if "g053-g052" in by_subset["pooled"]:
        pooled = reading(by_subset["pooled"]["g053-g052"])
        untuned = reading(by_subset["untuned"]["g053-g052"])
        L.append(f"- g053-g052 untuned: {untuned}")
        if pooled != untuned:
            L.append("- the pooled g053-g052 effect rests on the repos 0.5.3 was tuned on")
    for name, keep in subs:
        L += ["", f"## {name}", ""]
        L += table(["arm"] + METRICS,
                   [[a] + [fmt(v) for v in arm_means(tm, a, keep).values()] for a in arms])
        L += [""] + pair_table(by_subset[name])
        L += [""] + [f"- {p}: {reading(ms)}" for p, ms in by_subset[name].items()]
    L += ["", "## Cost", ""]
    cost = []
    for a in arms:
        ms = [r["metrics"] for r in scored if r["arm"] == a]
        cost.append([a, len(ms), fmt(sum(m["usd"] for m in ms)),
                     fmt(statistics.median(m["usd"] for m in ms)),
                     sum(m["tokens_in"] for m in ms), fmt(statistics.median(m["tokens_in"]
                                                                            for m in ms)),
                     sum(m["tokens_out"] for m in ms), fmt(sum(m["wall_s"] for m in ms)),
                     fmt(statistics.median(m["wall_s"] for m in ms))])
    L += table(["arm", "sessions", "usd sum", "usd median", "tokens_in sum", "tokens_in median",
                "tokens_out sum", "wall_s sum", "wall_s median"], cost)
    L += ["", "## glia usage", ""]
    glia_rows = [r for r in scored if r["arm"] != "none"]
    for a in sorted({r["arm"] for r in glia_rows}):
        rs = [r for r in glia_rows if r["arm"] == a]
        used = sum(1 for r in rs if r["glia_subcommands"])
        hist = Counter(s for r in rs for s in r["glia_subcommands"])
        L.append(f"- {a}: {used}/{len(rs)} sessions ({fmt(used / len(rs))}) made >= 1 glia call; "
                 + (", ".join(f"`{k or '(none)'}` {v}" for k, v in
                              sorted(hist.items(), key=lambda kv: (-kv[1], kv[0])))
                    or "no calls"))
    return "\n".join(L) + "\n"


# --- compare -----------------------------------------------------------------
def compare(old, new, out=print):
    for field in ("tasks", "prompt_sha256"):
        if old["fingerprints"].get(field) != new["fingerprints"].get(field):
            raise ScoreError(f"refused: fingerprints.{field} differs")
    def per(b):
        return {(r["task_id"], r["arm"]): r["metrics"] for r in b["per_task"]}
    po, pn = per(old), per(new)
    arms = sorted({a for _, a in po} & {a for _, a in pn})
    for arm in arms:
        tids = sorted({t for t, a in po if a == arm and (t, arm) in pn})
        for m in DIRECTION:
            diffs = [pn[(t, arm)][m] - po[(t, arm)][m] for t in tids
                     if pn[(t, arm)].get(m) is not None and po[(t, arm)].get(m) is not None]
            if not diffs:  # neither run measured it on a shared task (no truth file opened)
                continue
            d, lo, hi = bootstrap(diffs)
            v = verdict(m, lo, hi, len(diffs))
            out(f"[agent-ab] compare arm={arm} metric={m} diff={fmt(d)} "
                f"ci=[{fmt(lo)},{fmt(hi)}] verdict={v}")


# --- cli ---------------------------------------------------------------------
def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--repos", default=str(HERE / "repos.json"))
    ap.add_argument("--tasks-dir", default=str(HERE / "tasks"))
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("score")
    s.add_argument("run_id", nargs="?")
    s.add_argument("--from-metrics")
    s.add_argument("--out-md")
    s.add_argument("--out-json")
    s.add_argument("--out-sessions")
    c = sub.add_parser("compare")
    c.add_argument("old")
    c.add_argument("new")
    a = ap.parse_args(argv)
    try:
        if a.cmd == "compare":
            compare(json.loads(Path(a.old).read_text()), json.loads(Path(a.new).read_text()))
            return 0
        if bool(a.run_id) == bool(a.from_metrics):
            raise ScoreError("give exactly one of <run_id> or --from-metrics")
        if a.from_metrics:
            head, rows = read_metrics(a.from_metrics)
        else:
            head, rows = metrics_from_run(a.run_id, tasks.cache_dir().resolve(), a.repos,
                                          a.tasks_dir)
        if a.out_sessions:
            write_metrics(a.out_sessions, head, rows)
        baseline, md, marker = score(head, rows)
        if a.out_json:
            Path(a.out_json).write_text(json.dumps(baseline, indent=1, sort_keys=True) + "\n")
        if a.out_md:
            Path(a.out_md).write_text(md)
        print(marker)
        return 0
    except (ScoreError, run.RunError, tasks.TaskError, OSError, ValueError, KeyError) as e:
        print(f"[agent-ab] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
