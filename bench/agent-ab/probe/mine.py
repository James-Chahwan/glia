#!/usr/bin/env python3
"""Scenario probe (FE.4, 2026-10-25): mine a few held-out commits per task SHAPE, to learn
which shapes an agent A/B can tell glia apart on before any pilot. Exploration, not the
registered task set: tasks/ (FE.1) stays the pinned set.

    python3 probe/mine.py [--per-shape 4] [--out probe/tasks]

Shapes:
  ripple  a definition whose name changes or whose signature moves in one file, and the
          name is edited in >= 2 other code files (the callers): the prompt names the
          definition change only; finding every caller is the task.
  xlang   the non-test code spans >= 2 languages (a contract and its users across a
          service boundary).
  symptom the commit closes a GitHub issue: the prompt is the issue alone (title + body,
          never the commit subject, which states the fix).
  plain   FE.1's own shape (3..8 files, subject as prompt): the control.

Writes probe/tasks/<label>.jsonl in tasks.py's row schema plus "shape"; run.py reads it
with --tasks-dir probe/tasks.
"""
import argparse
import json
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import tasks as T  # noqa: E402

DEF_RE = re.compile(
    r"^[-+]\s*(?:export\s+)?(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?"
    r"(?:func(?:\s*\([^)]*\))?|def|function|fn|fun|class|interface|"
    r"(?:public|private|protected|static|final|override|\s)+[\w<>\[\],.? ]+?)\s+"
    r"([A-Za-z_]\w{3,})\s*[(<]")
SKIP_NAMES = {"main", "init", "test", "setup", "self", "this", "func", "function", "return"}


def diff_text(gd, parent, commit, path, old):
    return T.git(gd, "diff", "--no-ext-diff", "--no-textconv", "-M", "-U0", parent, commit,
                 "--", *([old] if old else []), path).stdout.decode("utf-8", errors="replace")


def ripple(gd, parent, commit, code):
    """The best (name, defining file, caller files) or None."""
    texts = {f[1]: diff_text(gd, parent, commit, f[1], f[2]) for f in code}
    best = None
    for path, text in texts.items():
        names = {m.group(1) for line in text.splitlines() if line[:1] in "+-"
                 for m in [DEF_RE.match(line)] if m and m.group(1).lower() not in SKIP_NAMES}
        for name in names:
            word = re.compile(rf"\b{re.escape(name)}\b")
            callers = sorted(p for p, t in texts.items() if p != path and any(
                line[:1] in "+-" and word.search(line) for line in t.splitlines()))
            if len(callers) >= 2 and (best is None or len(callers) > len(best[2])):
                best = (name, path, callers)
    return best


def langs(code):
    ext = {os.path.splitext(f[1])[1] for f in code}
    fam = {".h": ".c", ".hpp": ".cc", ".cpp": ".cc", ".tsx": ".ts", ".jsx": ".js", ".mjs": ".js",
           ".exs": ".ex", ".kt": ".java"}
    return sorted({fam.get(e, e) for e in ext})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-shape", type=int, default=4)
    ap.add_argument("--out", default=str(Path(__file__).resolve().parent / "tasks"))
    ap.add_argument("--pick", action="append", default=[],
                    help="shape=subject-prefix: hand-pick instead of round-robin (repeatable)")
    a = ap.parse_args()
    repos = T.load_repos(T.HERE / "repos.json")
    pinned = {json.loads(line)["commit"] for f in (T.HERE / "tasks").glob("*.jsonl")
              for line in f.read_text().splitlines()}
    found = {s: [] for s in ("ripple", "xlang", "symptom", "plain")}
    for repo in repos:
        gd = T.ensure_clone(repo)
        per_repo = {s: 0 for s in found}
        for e in T.log_first_parent(gd, repo["end_rev"]):
            if all(per_repo[s] >= 2 or len(found[s]) >= a.per_shape * 3 for s in found):
                break
            c = T.candidate(e)
            if not c or e["commit"] in pinned:
                continue
            parent, subject, prompt, pr, ref_text = c
            files = T.changed_files(gd, parent, e["commit"])
            code = [f for f in files if T.is_code(f[1]) and not T.is_test(f[1])]
            lines = sum(f[3] for f in files if T.is_code(f[1]))
            if not 2 <= len(code) <= 12 or not 5 <= lines <= 600 or \
                    not any(f[0] in "MR" for f in code):
                continue
            truth = {"code": [{"path": p, "status": s, "old_path": o,
                               "hunks": T.hunks(gd, parent, e["commit"], p, o)}
                              for s, p, o, _ in sorted(code, key=lambda f: f[1])],
                     "tests": sorted(f[1] for f in files if T.is_test(f[1])),
                     "other": sorted(f[1] for f in files
                                     if not T.is_test(f[1]) and not T.is_code(f[1]))}
            stats = {"code_files": len(code), "code_lines": sum(f[3] for f in code),
                     "test_files": len(truth["tests"])}
            shape, extra, issue, fetched = None, {}, None, "none"
            r = ripple(gd, parent, e["commit"], code)
            if r and per_repo["ripple"] < 2 and len(found["ripple"]) < a.per_shape * 3:
                shape, extra = "ripple", {"symbol": r[0], "defined_in": r[1], "callers": r[2]}
            elif len(langs(code)) >= 2 and per_repo["xlang"] < 2 and \
                    len(found["xlang"]) < a.per_shape * 3:
                shape, extra = "xlang", {"langs": langs(code)}
            elif per_repo["symptom"] < 2 and len(found["symptom"]) < a.per_shape * 3 and \
                    T.CLOSING_RE.search(ref_text):
                issue, fetched = T.fetch_issue(repo["url"], T.issue_refs(ref_text))
                if issue and not T.names_a_file(issue["title"], truth):
                    shape = "symptom"
            elif 3 <= len(code) <= 8 and per_repo["plain"] < 2 and not \
                    T.names_a_file(prompt, truth) and len(found["plain"]) < a.per_shape * 3:
                shape = "plain"
            if not shape:
                continue
            per_repo[shape] += 1
            found[shape].append({
                "id": f"{repo['label']}-{e['commit'][:12]}", "repo": repo["label"],
                "commit": e["commit"], "parent": parent, "author_time": e["author_time"],
                "subject": T.redact(subject),
                "prompt_subject": ("Resolve the GitHub issue below." if shape == "symptom"
                                   else T.redact(prompt)),
                "pr": pr, "issue": issue, "issue_fetch": fetched, "after_gate_rev": None,
                "truth": truth, "stats": stats, "shape": shape, "probe": extra})
            print(f"[probe] {shape:7} {repo['label']:18} files={len(code)} "
                  f"{json.dumps(extra)[:90]} | {prompt[:70]}", flush=True)
    # a.per_shape per shape, spread over repos (round-robin by repo)
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    rows = {}
    if a.pick:
        want = [x.split("=", 1) for x in a.pick]
        for shape, pre in want:
            hit = [it for it in found[shape] if pre in it["subject"]]
            if len(hit) != 1:
                sys.exit(f"pick {shape}={pre!r} matched {len(hit)}")
            rows.setdefault(hit[0]["repo"], []).append(hit[0])
        found = {}
    for shape, items in found.items():
        by_repo = {}
        for it in items:
            by_repo.setdefault(it["repo"], []).append(it)
        picked = []
        while len(picked) < a.per_shape and any(by_repo.values()):
            for k in list(by_repo):
                if by_repo[k] and len(picked) < a.per_shape:
                    picked.append(by_repo[k].pop(0))
        for it in picked:
            rows.setdefault(it["repo"], []).append(it)
        print(f"[probe] shape={shape} picked={len(picked)} of {len(items)}")
    for f in out.glob("*.jsonl"):
        f.unlink()
    for label, items in rows.items():
        (out / f"{label}.jsonl").write_text(
            "".join(json.dumps(r, sort_keys=True) + "\n" for r in items))


if __name__ == "__main__":
    main()
