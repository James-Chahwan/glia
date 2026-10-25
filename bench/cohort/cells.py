#!/usr/bin/env python3
"""The cohort's matrix cells (FH.10): which (language, mechanism) cells and which chains each
stored cohort probe row exercises, and how the cohort grades them.

    python3 cells.py [--release <r>] [--cell <lang>/<mech> | --chain <id>] [--json]

It reads only committed files (results/<release>/{meta,health,tasks}.json*, tasks/<label>.jsonl
for a drift task's server half, repos.json for the sides), never a checkout and never glia, so
substrate-gap's matrix_emit can grade the cohort column inside `matrix.py --emit / --check` and
the emitted artefacts stay byte-identical across processes. Stdlib only.

TAGGING (a heuristic over paths and the stored evidence; a mis-tag shows up as a disagreement
to inspect, never as a silent pass):

  drift    the client half is the row's truth files: each code file's language crossed with
           the client mechanism of every boundary kind its stored evidence holds (ENDPOINT ->
           http_client, GRAPHQL_OPERATION -> graphql, ...). When no truth file holds one, the
           mechanism is http_client if the repo's health counted any ROUTE or ENDPOINT, else
           `boundary` (a created column: the boundary is one glia does not model). The server
           half is the task's `probe.server_files` (tests and non-code out, only paths under a
           server prefix or the generated-path rule): each language crossed with the server
           counterpart (http_client -> http_server, the symmetric columns map to themselves).
  ripple / symptom   each code truth file's language x `calls` (the rest check is a
           diff-impact walk over the callers).

  chains (drift rows only; the rows are matrix_vocab.CHAINS), from the changed paths
  (client truth + server half):
    const_prefix             a changed code file whose name says it is a constant / enum table
                             (const, constants, enum, keys, endpoints, paths): a route prefix
                             or base URL held in a constant
    client_wrapper_endpoint  a non-SFC client truth file in an api / service / request / http /
                             client / fetch wrapper position (a directory or name token)
    workspace_sdk_route      a changed path in an SDK package (a segment naming `sdk`) or a
                             `packages/<..client|services..>` workspace package
    sfc_service_endpoint     a client truth file that is a single-file component (.vue,
                             .svelte, .astro)

  `.vue` scores on the typescript row (glia's Vue parser stacks on TypeScript, and the vue
  fixtures claim typescript cells); `.svelte` / `.astro` have no parser and are rows of their
  own; `.proto` is a schema, no row.

GRADING. A row's relevant checks are rest, gate, clear and names for a drift row, rest for the
others. The row passes when at least one relevant check ran (pass or miss) and none missed;
any miss class counts (noisy too); a row whose relevant checks are all n/a / error is no
observation. A cell's cohort level is the share of its observed rows that passed: full >= 0.8,
partial >= 0.3, none below, `n/a` when no observed row exercises it. Row-level: every cell a
row tags shares the row's verdict. A tagged cell outside the matrix vocabulary is CREATED when
at least one of its rows missed (graded by the same rule, listed); one only ever passed is
reported in `outside_vocab_passing` and creates nothing.
"""
import argparse
import json
import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "substrate-gap"))
import cohort as C  # noqa: E402
import matrix_vocab as vocab  # noqa: E402  -- stdlib only, never imports glia_py at load

RESULTS = HERE / "results"
TASKS = HERE / "tasks"
REPOS = HERE / "repos.json"
RULE = {"full": 0.8, "partial": 0.3}
NA = "n/a"

# extension -> matrix row; the rows are matrix_vocab.LANGUAGES spellings
EXT_LANG = {
    ".py": "python", ".go": "go",
    ".ts": "typescript", ".tsx": "typescript", ".js": "typescript", ".jsx": "typescript",
    ".mjs": "typescript", ".cjs": "typescript", ".vue": "typescript",
    ".java": "java", ".cs": "csharp", ".rb": "ruby", ".php": "php", ".swift": "swift",
    ".c": "c_cpp", ".cc": "c_cpp", ".cpp": "c_cpp", ".h": "c_cpp", ".hpp": "c_cpp",
    ".scala": "scala", ".clj": "clojure", ".cljs": "clojure", ".cljc": "clojure",
    ".dart": "dart", ".ex": "elixir", ".exs": "elixir", ".rs": "rust", ".sol": "solidity",
    ".tf": "terraform", ".kt": "kotlin", ".kts": "kotlin",
    # no parser: rows of their own, so a miss on one creates its cell
    ".svelte": "svelte", ".astro": "astro",
}
SFC_EXTS = (".vue", ".svelte", ".astro")

# a client boundary node kind -> (client column, server column)
KIND_MECH = {
    "ENDPOINT": ("http_client", "http_server"),
    "GRPC_CLIENT": ("grpc", "grpc"),
    "GRAPHQL_OPERATION": ("graphql", "graphql"),
    "WS_CLIENT": ("ws", "ws"),
    "RPC_CALL": ("rpc", "rpc"),
    "QUEUE_PRODUCER": ("queue", "queue"),
    "EVENT_EMITTER": ("eventbus", "eventbus"),
}
SERVER_OF = {c: s for c, s in KIND_MECH.values()}
SERVER_OF["boundary"] = "boundary"

DRIFT_CHECKS = ("rest", "gate", "clear", "names")
OTHER_CHECKS = ("rest",)

# the chain rows are the matrix vocabulary's (matrix_vocab.CHAINS)
CHAINS = vocab.CHAINS
CHAIN_IDS = vocab.CHAIN_IDS

CONST_TOKENS = {"const", "consts", "constant", "constants", "enum", "enums", "keys",
                "endpoints", "paths"}
WRAPPER_TOKENS = {"api", "apis", "request", "requests", "http", "client", "clients",
                  "service", "services", "fetch", "fetcher"}
WORKSPACE_TOKENS = {"client", "services", "sdk"}
TEST_EXTRA = re.compile(r"(^|/)(e2e|spec|specs)/|[-.](spec|test|e2e-spec)\.[a-z]+$")


def level_of(passed, rows):
    """The cohort level of `passed` of `rows` observed rows (RULE), `n/a` with none."""
    if not rows:
        return NA
    share = passed / rows
    return "full" if share >= RULE["full"] else "partial" if share >= RULE["partial"] \
        else "none"


def release_key(name):
    """Version order for results/<release>/ names: 0.5.4-pre < 0.5.4 < 0.5.10."""
    head, _, suffix = str(name).partition("-")
    nums = tuple(int(x) if x.isdigit() else -1 for x in head.split("."))
    return (nums, 0 if suffix else 1, suffix)


def default_release(results=RESULTS):
    """The newest results/<release>/ holding a tasks.jsonl, or None."""
    root = Path(results)
    found = [d.name for d in root.iterdir() if (d / "tasks.jsonl").exists()] \
        if root.is_dir() else []
    return max(found, key=release_key) if found else None


def language_of(path):
    """The matrix row a file scores on, or None for a file no row parses."""
    return EXT_LANG.get(os.path.splitext(path)[1].lower())


def is_test(path):
    return C.T.is_test(path) or bool(TEST_EXTRA.search(path))


def _tokens(segment):
    return set(t for t in re.split(r"[-_.]", segment.lower()) if t)


def _stem(path):
    return os.path.splitext(os.path.basename(path))[0]


def chains_of(client, server):
    """The chain ids a drift row crosses, from its client truth and server-half paths."""
    changed = [p for p in list(client) + list(server) if language_of(p)]
    out = set()
    if any(_tokens(_stem(p)) & CONST_TOKENS for p in changed):
        out.add("const_prefix")
    for p in client:
        if not language_of(p):
            continue
        if p.lower().endswith(SFC_EXTS):
            out.add("sfc_service_endpoint")
            continue
        parts = p.split("/")
        if any(_tokens(s) & WRAPPER_TOKENS for s in parts[:-1]) \
                or _tokens(_stem(p)) & WRAPPER_TOKENS:
            out.add("client_wrapper_endpoint")
    for p in list(client) + list(server):
        parts = p.split("/")
        for i, seg in enumerate(parts[:-1]):
            if "sdk" in seg.lower() or (i and parts[i - 1] == "packages"
                                        and _tokens(seg) & WORKSPACE_TOKENS):
                out.add("workspace_sdk_route")
    return sorted(out)


def load(results=RESULTS, tasks=TASKS, repos=REPOS, release=None):
    """The stored rows of one release, or None when there is none. Pure reads."""
    release = release or default_release(results)
    if release is None:
        return None
    d = Path(results) / release
    if not (d / "tasks.jsonl").exists():
        return None
    meta = json.loads((d / "meta.json").read_text()) if (d / "meta.json").exists() else {}
    health = {h["repo"]: h for h in C.read_rows(d / "health.jsonl")}
    try:
        repo_rows = json.loads(Path(repos).read_text())["repos"]
    except (OSError, ValueError, KeyError):
        repo_rows = []
    task_rows = {}
    for label in sorted({r["repo"] for r in C.read_rows(d / "tasks.jsonl")}):
        for t in C.read_rows(Path(tasks) / f"{label}.jsonl"):
            task_rows[t["id"]] = t
    return {"release": release, "meta": meta, "health": health,
            "repos": {r["label"]: r for r in repo_rows},
            "rows": C.read_rows(d / "tasks.jsonl"), "tasks": task_rows}


def _verdict(row):
    """(passed, miss classes) over the row's relevant checks; passed None = no observation."""
    want = DRIFT_CHECKS if row["shape"] == "drift" else OTHER_CHECKS
    ran = [c for c in row.get("checks") or [] if c["check"] in want
           and c["status"] in ("pass", "miss")]
    classes = {}
    for c in ran:
        if c["status"] == "miss":
            k = c.get("class") or "unclassified"
            classes[k] = classes.get(k, 0) + 1
    return (None if not ran else not classes), dict(sorted(classes.items()))


def _server_side(repo, files):
    """The server-half code files a drift row's server mechanism scores on."""
    sides = (repo or {}).get("sides") or {}
    prefixes = tuple(sides.get("server") or ())
    gen = C.gen_rule(repo) if repo else None
    out = []
    for p in files:
        if not language_of(p) or is_test(p):
            continue
        if prefixes and not p.startswith(prefixes) and not (gen and re.search(gen, p)):
            continue
        out.append(p)
    return out


def tag_row(row, data):
    """One stored probe row -> its cells (sorted (lang, mech) pairs), chains and verdict."""
    passed, classes = _verdict(row)
    cells, chains = set(), []
    truth = [p for p in row.get("truth") or [] if language_of(p)]
    if row["shape"] == "drift":
        ev = (row.get("S") or {}).get("evidence") or {}
        kinds = sorted({k for p in truth for k in (ev.get(p) or {}).get("boundary") or []
                        if k in KIND_MECH})
        h = data["health"].get(row["repo"]) or {}
        mechs = [KIND_MECH[k][0] for k in kinds] or (
            ["http_client"] if (h.get("routes") or h.get("endpoints")) else ["boundary"])
        task = data["tasks"].get(row["task"]) or {}
        server_all = sorted((task.get("probe") or {}).get("server_files") or [])
        server = _server_side(data["repos"].get(row["repo"]), server_all)
        for p in truth:
            cells |= {(language_of(p), m) for m in mechs}
        for p in server:
            cells |= {(language_of(p), SERVER_OF[m]) for m in mechs}
        chains = chains_of(truth, [p for p in server_all if not is_test(p)])
    else:
        cells = {(language_of(p), "calls") for p in truth}
    return {"task": row["task"], "repo": row["repo"], "shape": row["shape"],
            "cells": sorted(cells), "chains": chains, "passed": passed, "classes": classes}


def tag_rows(data):
    return sorted((tag_row(r, data) for r in data["rows"]), key=lambda t: t["task"])


def _grade(obs):
    """obs: [(repo, task, passed, classes)] observed rows of one cell or chain."""
    passed = sum(1 for o in obs if o[2])
    repos, classes = {}, {}
    for repo, _, ok, cls in obs:
        r = repos.setdefault(repo, [0, 0])
        r[0] += bool(ok)
        r[1] += 1
        for k, n in cls.items():
            classes[k] = classes.get(k, 0) + n
    return {"level": level_of(passed, len(obs)), "passed": passed, "rows": len(obs),
            "repos": {k: repos[k] for k in sorted(repos)},
            "classes": {k: classes[k] for k in sorted(classes)}}


def grade(tagged, languages, mechanisms):
    """Grade the tagged rows: grid cells, created cells and chain cells."""
    grid = {(lg, m) for lg in languages for m in mechanisms}
    per_cell, per_chain = {}, {}
    for t in tagged:
        if t["passed"] is None:
            continue
        o = (t["repo"], t["task"], t["passed"], t["classes"])
        for c in t["cells"]:
            per_cell.setdefault(c, []).append(o)
        for ch in t["chains"]:
            per_chain.setdefault(ch, []).append(o)
    cells, created, passing = {}, [], []
    for (lg, m) in sorted(per_cell):
        obs = per_cell[(lg, m)]
        label = f"{lg}/{m}"
        inside = (lg, m) in grid
        if not inside and all(o[2] for o in obs):
            passing.append(label)
            continue
        if not inside:
            created.append(label)
        cells[label] = {**_grade(obs), "created": not inside}
    chains = {}
    for ch in CHAINS:
        obs = per_chain.get(ch["id"])
        if obs:
            chains[f"chain/{ch['id']}"] = {**_grade(obs), "label": ch["label"],
                                           "description": ch["description"]}
    return cells, chains, created, passing


def cohort(results=RESULTS, tasks=TASKS, repos=REPOS, languages=(), mechanisms=(),
           release=None):
    """The cohort column of one release, or None when no results exist."""
    data = load(results, tasks, repos, release)
    if data is None:
        return None
    tagged = tag_rows(data)
    cells, chains, created, passing = grade(tagged, languages, mechanisms)
    meta = data["meta"]
    return {"release": data["release"], "glia": meta.get("glia"),
            "tasks_sha256": meta.get("tasks_sha256"), "rule": dict(RULE),
            "rows": len(tagged), "observed": sum(t["passed"] is not None for t in tagged),
            "cells": cells, "chains": chains, "created": created,
            "outside_vocab_passing": passing}


def main(argv=None):
    ap = argparse.ArgumentParser(description="the cohort's matrix cells")
    ap.add_argument("--release", help="results/<release>/ (default: the newest)")
    ap.add_argument("--cell", help="'<lang>/<mech>': the rows that grade it")
    ap.add_argument("--chain", help="a chain id: the rows that grade it")
    ap.add_argument("--json", action="store_true", help="every tagged row as JSON lines")
    a = ap.parse_args(argv)
    data = load(release=a.release)
    if data is None:
        print("[cohort-cells] no results", file=sys.stderr)
        return 1
    tagged = tag_rows(data)
    if a.json:
        for t in tagged:
            print(json.dumps({**t, "cells": [f"{x}/{y}" for x, y in t["cells"]]},
                             sort_keys=True))
    elif a.cell or a.chain:
        for t in tagged:
            hit = a.chain in t["chains"] if a.chain else \
                a.cell in [f"{x}/{y}" for x, y in t["cells"]]
            if hit:
                verdict = {True: "pass", False: "miss", None: "n/a"}[t["passed"]]
                print(f"{t['task']}  {t['shape']:<7} {verdict:<4} "
                      f"{json.dumps(t['classes'], sort_keys=True)}")
    n_cells = len({c for t in tagged for c in t["cells"]})
    n_chains = len({c for t in tagged for c in t["chains"]})
    print(f"[cohort-cells] release={data['release']} rows={len(tagged)} "
          f"observed={sum(t['passed'] is not None for t in tagged)} cells={n_cells} "
          f"chains={n_chains}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
