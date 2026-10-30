#!/usr/bin/env python3
"""The Sibyl matrix runner (FH.11b): render every case of the cover, build it with the glia under
test, and grade four things per case against its expected answer (gen.py, by construction):

  pair     at S, an edge of the case's category (HTTP_CALLS / GRPC_CALLS / GRAPHQL_CALLS /
           QUEUE_FLOWS / WS_CONNECTS / RPC_CALLS for evm) joins a node in the client's pair file to a node in one of
           the server's pair files (expected pair.server.pair_files; a REST case's route or
           handler file, where glia locates the ROUTE), and one end names the exchange's channel:
           a REST path compared with gen.norm_path, so `${…}`, `:id` and `{id}` agree (`exact`),
           or its tail on a segment boundary when glia folded no prefix (`suffix`, at least the
           resource and its parameter: `/users/{}`; the evidence records which); otherwise one of
           expected.exchange.match, regexes naming the gRPC service, the GraphQL operation or
           root field, the topic or the WebSocket path
  drift_s  at S, `contract-breaks --base HEAD~1` (the primary repo, every other repo `--with`)
           reports a breaking change
  clear_c  at C (the client followed), the same call reports no breaking change, after S broke
  client   the S answer names one of the files the client had to change (expected.drift.
           client_files: any of them), the cohort's `gate` reading

A case passes when all four do. The results group the cases by every pair of axis values (the
pass share per pair the cover holds), by axis value, by FH.10's chain cells and by the coverage
matrix's (language, mechanism) cells, each graded full / partial / none at the cohort's rule
(bench/cohort/cells.py: share >= 0.8 / >= 0.3 / below): `level` on the whole case,
`check_levels` on each check alone (a chain's `pair` level is its reach).

  python3 bench/sibyl/run.py [--glia BIN] [--transports rest,grpc,..] [--seed N] [--case ID]..
                             [--work DIR] [--out DIR] [--jobs N] [--keep] [--json]

Cases render under --work (default ~/.cache/glia-sibyl/run/), never in the repo; every glia call
runs with GLIA_NO_PERSIST=1. --out (default <work>/results/) receives results.json (no times,
so two runs of one glia on one cover are byte-identical) and results.md. Prints, on stderr,

  [sibyl] cases=<n> pairs_covered=<p>/<t> pass=<k> fail=<f> (pair=<a> drift_s=<b> clear_c=<c> client=<d>)

`p` the expressible axis pairs the run's cases hold, `t` all of them for the transports run.
Exits 0 when every case ran (whatever it scored), 2 when a glia call failed to answer."""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "cohort"))

import gen  # noqa: E402
import cells  # noqa: E402  -- bench/cohort/cells.py: level_of, the chain vocabulary

CHECKS = ("pair", "drift_s", "clear_c", "client")
DEFAULT_WORK = Path.home() / ".cache" / "glia-sibyl" / "run"

# The transport's mechanism columns in the coverage matrix: (client column, server column).
MECH = {"rest": ("http_client", "http_server"), "grpc": ("grpc", "grpc"),
        "graphql": ("graphql", "graphql"), "queue": ("kafka", "kafka"),
        "websocket": ("ws", "ws"), "evm": ("evm", "evm")}
LANG = {"ts": "typescript", "dart": "dart", "kotlin": "kotlin", "swift": "swift",
        "python": "python", "go-gin": "go", "ts-nest": "typescript",
        "ts-express": "typescript", "java-spring": "java", "python-fastapi": "python",
        "rust-axum": "rust", "csharp-aspnet": "csharp", "go": "go", "solidity": "solidity"}


def chains_of(axes):
    """The FH.10 chain cells (bench/substrate-gap/matrix_vocab.CHAINS) a case crosses, read off
    its axes: a prefix held in a constant or an enum, a hand-written wrapper between the use
    site and the call, a workspace client package between the app and the server. No case is a
    single-file component, so `sfc_service_endpoint` is never tagged here."""
    out = []
    if axes["wiring_source"] in ("const", "enum-prefix"):
        out.append("const_prefix")
    if axes["wiring_call"] == "wrapper":
        out.append("client_wrapper_endpoint")
    if axes["packaging"] == "workspace":
        out.append("workspace_sdk_route")
    return [c for c in out if c in cells.CHAIN_IDS]


def cells_of(axes):
    """The coverage-matrix cells a case exercises: the client's language x the transport's
    client column, the server's language x its server column."""
    cm, sm = MECH[axes["transport"]]
    return sorted({f"{LANG[axes['client']]}/{cm}", f"{LANG[axes['server']]}/{sm}"})


# ------------------------------------------------------------------------------------- glia

class Glia:
    def __init__(self, binary):
        self.binary = str(binary)
        self.env = {**os.environ, "GLIA_NO_PERSIST": "1",
                    "GLIA_THREADS": os.environ.get("GLIA_THREADS", "2")}

    def version(self):
        r = subprocess.run([self.binary, "--version"], capture_output=True, text=True)
        if r.returncode != 0:
            raise SystemExit(f"[sibyl] {self.binary} --version failed: {r.stderr.strip()}")
        return r.stdout.strip()

    def json(self, args, cwd, ok=(0, 1)):
        """(parsed stdout, error text): the answer of `glia <args> --json`, or why there is none."""
        r = subprocess.run([self.binary, *args, "--json"], capture_output=True, text=True,
                           cwd=cwd, env=self.env, timeout=600)
        if r.returncode not in ok:
            return None, f"exit {r.returncode}: {r.stderr.strip()[-400:]}"
        try:
            return json.loads(r.stdout), None
        except json.JSONDecodeError as e:
            return None, f"unparsable answer ({e}): {r.stdout[:200]}"


def git_checkout(repo, state):
    subprocess.run(["git", "-C", str(repo), "checkout", "-q", "--detach", f"refs/sibyl/{state}"],
                   check=True, capture_output=True)


def files_in(obj, acc):
    """Every `file` string anywhere in an answer (the cohort's gate.py reading)."""
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


PAIR_RULE = ('out(A, B, FA, FB, RA, RB) :- edge(A, B, "{cat}"), file(A, FA), file(B, FB), '
             'repo(A, RA), repo(B, RB).')
PATH_IN_QNAME = re.compile(r"(?:^|[\s:])(/[^\s]*)")


def channel_of(qname):
    """The path a route / endpoint qname carries (`GET /api/v1/users/:id @svc`,
    `endpoint:GET:/users/${…} @pkg`), or None."""
    m = PATH_IN_QNAME.search(qname)
    return m.group(1) if m else None


def channel_match(exp, qa, qb):
    """`exact` when one end names the exchange's channel, `suffix` when a REST end names only
    its tail on a segment boundary (a prefix glia did not fold: `/users/{}` of
    `/api/v1/users/{}`, at least the resource and its parameter), else None."""
    ex = exp["exchange"]
    if exp["transport"] != "rest":
        return "exact" if any(re.search(rx, q, re.I) for q in (qa, qb)
                              for rx in ex["match"]) else None
    want = ex["path_norm"]
    got = [gen.norm_path(channel_of(q)) for q in (qa, qb) if channel_of(q)]
    if want in got:
        return "exact"
    if any(len(g.strip("/").split("/")) >= 2 and want.endswith(g) and g.startswith("/")
           for g in got):
        return "suffix"
    return None


def grade_pair(glia, exp, primary, others, repo_index):
    cat = exp["pair"]["category"]
    d, err = glia.json(["query", str(primary), *sum((["--with", str(o)] for o in others), []),
                        "-e", PAIR_RULE.format(cat=cat)], cwd=primary)
    if err:
        return {"ok": False, "error": err}
    client = exp["pair"]["client"]
    cside = (f"repo#{repo_index[client['repo']]}", client["file"])
    srv = exp["pair"]["server"]
    sfiles = {(f"repo#{repo_index[srv['repo']]}", f)
              for f in (srv.get("pair_files") or [srv["route_file"], srv["handler_file"]])}
    edges, hits = [], []
    for row in d.get("results") or []:
        v = [x.get("str") if "str" in x else x.get("node", {}).get("qname") for x in row["values"]]
        qa, qb, fa, fb, ra, rb = v
        edges.append([qa, qb, f"{ra}:{fa}", f"{rb}:{fb}"])
        ends = {(ra, fa), (rb, fb)}
        how = channel_match(exp, qa, qb)
        if cside in ends and ends & sfiles and how:
            hits.append((0 if how == "exact" else 1, qa, qb, how))
    out = {"ok": bool(hits), "edges": len(edges)}
    if hits:
        _, qa, qb, how = min(hits)
        out.update(edge=[qa, qb], channel=how)
    else:
        out["seen"] = sorted(edges)[:6]
    return out


def breaking_state(glia, primary, others, client_files):
    d, err = glia.json(["contract-breaks", str(primary), "--base", "HEAD~1",
                        *sum((["--with", str(o)] for o in others), [])], cwd=primary)
    if err:
        return {"error": err}
    rows = [r for k in ("schemas", "models", "orphaned_clients") for r in d.get(k) or []]
    named = files_in(rows, set())
    return {"breaking": d.get("breaking", 0), "advisory": d.get("advisory", 0),
            "rows": len(rows), "client_named": sorted(f for f in client_files if f in named)}


def run_case(glia, case, work):
    """Render, check out S then C, build and grade. Returns the case's result row."""
    states, exp = gen.render_case(case)
    cd = work / case["id"]
    if cd.exists():
        shutil.rmtree(cd)
    cd.mkdir(parents=True)
    gen.write_git(cd, states, exp["states"])
    order = [exp["primary"], *exp["with"]]
    repo_index = {r: i for i, r in enumerate(order)}
    primary, others = cd / exp["primary"], [cd / r for r in exp["with"]]
    client_files = sorted({c["file"] for c in exp["drift"]["client_files"]})
    row = {"id": case["id"], "axes": case["axes"], "chains": chains_of(case["axes"]),
           "cells": cells_of(case["axes"]), "checks": {}, "evidence": {}}
    errors = []
    for state in ("S", "C"):
        for r in order:
            git_checkout(cd / r, state)
        if state == "S":
            pair = grade_pair(glia, exp, primary, others, repo_index)
            row["evidence"]["pair"] = pair
            if "error" in pair:
                errors.append(f"pair: {pair['error']}")
        cb = breaking_state(glia, primary, others, client_files)
        row["evidence"][f"contract_breaks_{state}"] = cb
        if "error" in cb:
            errors.append(f"contract-breaks at {state}: {cb['error']}")
    s, c = row["evidence"]["contract_breaks_S"], row["evidence"]["contract_breaks_C"]
    row["checks"] = {
        "pair": bool(row["evidence"]["pair"].get("ok")),
        "drift_s": bool(s.get("breaking")),
        "clear_c": bool(s.get("breaking")) and "error" not in c and not c.get("breaking"),
        "client": bool(s.get("client_named")),
    }
    row["pass"] = all(row["checks"].values())
    if errors:
        row["errors"] = errors
    return row


# --------------------------------------------------------------------------------- reporting

def share(passed, n):
    return round(passed / n, 3) if n else None


def summarise(rows, transports, seed):
    axes = gen.axes_for(transports)
    cov = gen.Cover(axes, gen.unexpressible(), seed)
    req, _ = cov.required()
    held, by_pair = set(), {}
    for r in rows:
        for p in cov.pairs_of(r["axes"]) & req:
            held.add(p)
            by_pair.setdefault(p, []).append(r)
    pairs = []
    for p in sorted(by_pair):
        rs = by_pair[p]
        k = sum(1 for r in rs if r["pass"])
        pairs.append({"a": list(p[0]), "b": list(p[1]), "cases": len(rs), "passed": k,
                      "share": share(k, len(rs)),
                      "checks": {c: sum(1 for r in rs if r["checks"][c]) for c in CHECKS}})

    def group(key_fn):
        acc = {}
        for r in rows:
            for k in key_fn(r):
                acc.setdefault(k, []).append(r)
        out = {}
        for k in sorted(acc):
            rs = acc[k]
            n = sum(1 for r in rs if r["pass"])
            checks = {c: sum(1 for r in rs if r["checks"][c]) for c in CHECKS}
            out[k] = {"cases": len(rs), "passed": n, "share": share(n, len(rs)),
                      "level": cells.level_of(n, len(rs)), "checks": checks,
                      "check_levels": {c: cells.level_of(v, len(rs)) for c, v in checks.items()},
                      "ids": [r["id"] for r in rs]}
        return out

    values = group(lambda r: [f"{a}={v}" for a, v in r["axes"].items()])
    chains = group(lambda r: r["chains"])
    for cid in cells.CHAIN_IDS:
        chains.setdefault(cid, {"cases": 0, "passed": 0, "share": None,
                                "level": cells.level_of(0, 0), "checks": {},
                                "check_levels": {}, "ids": []})
    return {"pairs_covered": len(held), "pairs_total": len(req), "pairs": pairs,
            "values": values, "chains": dict(sorted(chains.items())),
            "cells": group(lambda r: r["cells"])}


def marker(rows, summary):
    k = sum(1 for r in rows if r["pass"])
    c = {x: sum(1 for r in rows if r["checks"][x]) for x in CHECKS}
    return (f"[sibyl] cases={len(rows)} pairs_covered={summary['pairs_covered']}/"
            f"{summary['pairs_total']} pass={k} fail={len(rows) - k} (pair={c['pair']} "
            f"drift_s={c['drift_s']} clear_c={c['clear_c']} client={c['client']})")


def render_md(res):
    s = res["summary"]
    lines = [f"# Sibyl matrix - {res['meta']['glia']}", "",
             f"`{res['meta']['marker']}`", "",
             "| case | pair | drift_s | clear_c | client | pass |", "|---|---|---|---|---|---|"]
    yn = {True: "yes", False: "no"}
    for r in res["cases"]:
        lines.append(f"| {r['id']} | " + " | ".join(yn[r["checks"][c]] for c in CHECKS)
                     + f" | {yn[r['pass']]} |")
    head = ("| {} | cases | passed | level | " + " | ".join(CHECKS) + " |",
            "|---|---|---|---|" + "---|" * len(CHECKS))

    def table(title, key, rows):
        out = ["", title, "", head[0].format(key), head[1]]
        for k, v in rows.items():
            out.append(f"| {k} | {v['cases']} | {v['passed']} | {v['level']} | "
                       + " | ".join(str(v["checks"].get(c, 0)) for c in CHECKS) + " |")
        return out

    lines += table("## By axis value", "value", s["values"])
    lines += table("## Chain cells (FH.10)", "chain", s["chains"])
    lines += table("## Matrix cells", "cell", s["cells"])
    lines += ["", "## By axis pair", "", "| a | b | cases | passed | share | "
              + " | ".join(CHECKS) + " |", "|---|---|---|---|---|" + "---|" * len(CHECKS)]
    lines += [f"| {'='.join(p['a'])} | {'='.join(p['b'])} | {p['cases']} | {p['passed']} | "
              f"{p['share']} | " + " | ".join(str(p["checks"][c]) for c in CHECKS) + " |"
              for p in s["pairs"]]
    return "\n".join(lines) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--glia", default=os.environ.get("GLIA", str(ROOT / "target/debug/glia")))
    ap.add_argument("--transports", default=",".join(gen.AXES["transport"]))
    ap.add_argument("--seed", type=int, default=gen.SEED)
    ap.add_argument("--case", action="append")
    ap.add_argument("--work", default=str(DEFAULT_WORK))
    ap.add_argument("--out")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--keep", action="store_true", help="keep the rendered case repos")
    ap.add_argument("--json", action="store_true", help="print results.json on stdout")
    a = ap.parse_args(argv)
    transports = a.transports.split(",")
    glia = Glia(Path(a.glia).resolve())
    version = glia.version()
    cases = gen.cover(transports, a.seed)
    if a.case:
        missing = sorted(set(a.case) - {c["id"] for c in cases})
        if missing:
            raise SystemExit(f"[sibyl] unknown case ids: {', '.join(missing)}")
        cases = [c for c in cases if c["id"] in a.case]
    work = Path(a.work).resolve()
    if ROOT in work.parents or work == ROOT:
        raise SystemExit(f"[sibyl] --work {work} is inside the glia tree; render elsewhere")
    cases_dir = work / "cases"
    cases_dir.mkdir(parents=True, exist_ok=True)
    with ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
        rows = list(pool.map(lambda c: run_case(glia, c, cases_dir), cases))
    if not a.keep:
        for c in cases:
            shutil.rmtree(cases_dir / c["id"], ignore_errors=True)
    summary = summarise(rows, transports, a.seed)
    line = marker(rows, summary)
    res = {"meta": {"glia": version, "seed": a.seed, "transports": transports,
                    "cases": len(rows), "marker": line, "checks": list(CHECKS),
                    "rule": cells.RULE},
           "summary": summary, "cases": rows}
    out = Path(a.out).resolve() if a.out else work / "results"
    out.mkdir(parents=True, exist_ok=True)
    (out / "results.json").write_text(json.dumps(res, indent=1, sort_keys=True) + "\n")
    (out / "results.md").write_text(render_md(res))
    if a.json:
        print(json.dumps(res, indent=1, sort_keys=True))
    print(f"[sibyl] results -> {out / 'results.json'}", file=sys.stderr)
    print(line, file=sys.stderr)
    return 2 if any(r.get("errors") for r in rows) else 0


if __name__ == "__main__":
    sys.exit(main())
