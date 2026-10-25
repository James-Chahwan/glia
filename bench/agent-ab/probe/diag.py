#!/usr/bin/env python3
"""Why glia did or did not change a session (FE.4 follow-up, 2026-10-26): reads the stored
stream-json transcripts of runs and their task rows, and says per glia-arm session where
the chain broke.

    python3 probe/diag.py <run_id>=<tasks_dir> [...] [--json out.jsonl]

Per session: skill loaded (the Skill tool) and the turn it loaded on, the context the
skill added (the next turn's input growth), glia commands, the bytes glia printed, whether
a glia answer named a truth file (and which), whether the agent opened / edited each truth
file and whether it first saw it in a glia answer or in a grep / find / git result, turns,
tokens. Then one verdict per session:

  no_skill      the skill never loaded and glia never ran
  skill_only    the skill loaded, glia never ran (pure cost)
  glia_missed   glia ran, no answer named a truth file
  glia_late     a glia answer named a truth file the agent had already seen elsewhere
  glia_found    a glia answer named a truth file first
"""
import argparse
import json
import os
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

D = Path(os.environ.get("GLIA_AGENT_AB_DIR", "~/.cache/glia-agent-ab")).expanduser()


def tasks_of(tdir):
    out = {}
    for f in Path(tdir).glob("*.jsonl"):
        for line in f.read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                out[r["id"]] = r
    return out


def text_of(content):
    if isinstance(content, str):
        return content
    return "".join(x.get("text", "") for x in content or [] if isinstance(x, dict))


def truth_files(row):
    return [c["path"] for c in row["truth"]["code"]]


def mentions(text, path):
    base = os.path.basename(path)
    return path in text or (len(base) > 6 and base in text)


def session(path, row):
    truth = truth_files(row)
    uses, seen_first, opened, edited = {}, {}, set(), set()
    turn, skill_turn, glia_cmds, glia_bytes, ctx = 0, None, [], 0, []
    glia_named = set()
    for line in open(path):
        e = json.loads(line)
        if e.get("type") == "assistant":
            turn += 1
            u = e["message"].get("usage") or {}
            ctx.append(u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0)
                       + u.get("cache_creation_input_tokens", 0))
            for c in e["message"]["content"]:
                if c.get("type") != "tool_use":
                    continue
                inp = c.get("input") or {}
                if c["name"] == "Skill" and inp.get("skill", "").startswith("glia"):
                    skill_turn = skill_turn or turn
                cmd = inp.get("command", "")
                is_glia = c["name"] == "Bash" and re.search(r"(^|[;&|(\s])glia\s", cmd)
                if is_glia:
                    glia_cmds.append(re.search(r"glia\s+(--?\S+\s+)*(\S+)", cmd).group(2)
                                     if re.search(r"glia\s+(--?\S+\s+)*(\S+)", cmd) else "?")
                uses[c["id"]] = ("glia" if is_glia else c["name"], cmd or json.dumps(inp))
                for t in truth:
                    target = inp.get("file_path", "") + " " + cmd
                    if c["name"] in ("Read",) and target.strip().endswith(t):
                        opened.add(t)
                    if c["name"] in ("Edit", "Write", "MultiEdit") and inp.get("file_path", "").endswith(t):
                        edited.add(t)
                    if c["name"] == "Bash" and t in cmd and re.search(r"\b(cat|sed|head|less|nl)\b", cmd):
                        opened.add(t)
        elif e.get("type") == "user" and isinstance(e["message"].get("content"), list):
            for x in e["message"]["content"]:
                if x.get("type") != "tool_result" or x.get("tool_use_id") not in uses:
                    continue
                kind, _ = uses[x["tool_use_id"]]
                out = text_of(x.get("content"))
                if kind == "glia":
                    glia_bytes += len(out.encode())
                for t in truth:
                    if mentions(out, t):
                        seen_first.setdefault(t, kind)
                        if kind == "glia":
                            glia_named.add(t)
    skill_ctx = None
    if skill_turn and skill_turn < len(ctx):
        skill_ctx = ctx[skill_turn] - ctx[skill_turn - 1]
    if not glia_cmds:
        verdict = "skill_only" if skill_turn else "no_skill"
    elif not glia_named:
        verdict = "glia_missed"
    elif any(seen_first.get(t) == "glia" for t in glia_named):
        verdict = "glia_found"
    else:
        verdict = "glia_late"
    return {"turns": turn, "skill_turn": skill_turn, "skill_ctx_tokens": skill_ctx,
            "glia_cmds": glia_cmds, "glia_bytes": glia_bytes, "truth": len(truth),
            "glia_named": sorted(glia_named), "first_seen": seen_first,
            "opened": sorted(opened), "edited": sorted(edited),
            "tokens_in": sum(ctx), "verdict": verdict}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+", help="<run_id>=<tasks_dir>")
    ap.add_argument("--json")
    a = ap.parse_args()
    rows = []
    for spec in a.runs:
        run_id, tdir = spec.split("=", 1)
        tasks = tasks_of(tdir)
        for p in sorted((D / "sessions" / run_id).glob("*/*/*.jsonl")):
            if p.name.endswith(".prep.log") or not p.stem.isdigit():
                continue
            tid, arm = p.parent.parent.name, p.parent.name
            if tid not in tasks:
                continue
            r = session(p, tasks[tid])
            r.update(run=run_id, task=tid, arm=arm, shape=tasks[tid].get("shape", "fe1"))
            rows.append(r)
    by = defaultdict(Counter)
    for r in rows:
        if r["arm"] != "none":
            by[(r["run"], r["shape"])][r["verdict"]] += 1
    print("run/shape                      sessions  " + "  ".join(
        f"{v:>11}" for v in ("no_skill", "skill_only", "glia_missed", "glia_late", "glia_found")))
    for k in sorted(by):
        c = by[k]
        print(f"{k[0] + '/' + k[1]:30} {sum(c.values()):8}  " + "  ".join(
            f"{c[v]:>11}" for v in ("no_skill", "skill_only", "glia_missed", "glia_late",
                                    "glia_found")))
    g = [r for r in rows if r["arm"] != "none"]
    sk = [r["skill_ctx_tokens"] for r in g if r["skill_ctx_tokens"]]
    if sk:
        print(f"skill context added: median {sorted(sk)[len(sk) // 2]} tokens over {len(sk)} loads; "
              f"glia output median {sorted(r['glia_bytes'] for r in g)[len(g) // 2]} bytes/session")
    cmds = Counter(c for r in g for c in r["glia_cmds"])
    print("glia commands:", dict(cmds.most_common()))
    first = Counter(v for r in rows for v in r["first_seen"].values())
    print("where a truth file was first seen (all arms):", dict(first.most_common()))
    if a.json:
        Path(a.json).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in rows))


if __name__ == "__main__":
    main()
