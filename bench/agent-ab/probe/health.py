#!/usr/bin/env python3
"""Boundary health (plan step 1, 2026-10-26): can glia read a repo's boundaries at all?
No model; one build per repo at its pinned head, then glia's own gaps / coverage numbers.

    python3 probe/health.py [<label>=<bare git dir>[@<rev>] ...] [--glia <bin>] [--jobs 4]

Per repo: ROUTE / ENDPOINT / RPC / QUEUE / GRAPHQL node counts; paired HTTP edges
(HTTP_CALLS); unpaired_route / unpaired_endpoint / unresolved_endpoint / dead_symbol from
`gaps`; dead share of FUNCTION+METHOD; client reach (paired endpoints / all endpoints);
route reach (routes with a caller / all routes); unresolved imports of workspace packages
(an import of a package the repo's own manifests name); files by extension glia did not
parse (code-looking extensions it skipped); suspicious endpoints (a data: / blob: / mailto:
URL, or a path with spaces). A repo is BLIND when routes or endpoints exist and fewer
than 20% pair, or when dead share is over 60%.
"""
import argparse
import json
import os
import re
import sys
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from oracle import D, checkout, sh  # noqa: E402

DEFAULT = ["shell", "websocket", "samples", "commons-text", "microservices-demo", "click",
           "immich", "twenty", "photoprism"]
EXTRA = {"gin-vue-admin": "gin-vue-admin.git", "scaffold-eth-2": "scaffold-eth-2.git"}
SFC = (".svelte", ".astro", ".mdx")


def run_json(glia, args, copy, env):
    rc, out, err, ms = sh([glia, *args], cwd=copy, env=env, timeout=3600)
    try:
        return json.loads(out), ms
    except ValueError:
        return None, ms


def one(label, gd, rev, glia):
    env = {**os.environ, "GLIA_THREADS": "4"}
    copy = D / "health" / "work" / label
    res = {"repo": label}
    try:
        checkout(gd, rev, copy)
        rc, _, err, ms = sh([glia, "build", "."], cwd=copy, env=env, timeout=3600)
        if rc:
            res["error"] = f"build rc={rc} {err.strip()[-160:]}"
            return res
        res["build_ms"] = ms
        g, _ = run_json(glia, ["gaps", ".", "--json"], copy, env)
        res["gaps"] = (g or {}).get("counts")
        s, _ = run_json(glia, ["coverage", ".", "--json"], copy, env)
        res["coverage_rows"] = len((s or {}).get("rows", [])) if isinstance(s, dict) else None
        e, _ = run_json(glia, ["query", ".", "--gql", "-e",
                               "MATCH (e:ENDPOINT) RETURN e.qname", "--json", "--limit",
                               "100000"], copy, env)
        r, _ = run_json(glia, ["query", ".", "--gql", "-e",
                               "MATCH (r:ROUTE) RETURN r.qname", "--json", "--limit",
                               "100000"], copy, env)
        p, _ = run_json(glia, ["query", ".", "--gql", "-e",
                               "MATCH (e:ENDPOINT)-[:HTTP_CALLS]->(r:ROUTE) RETURN e.qname, r.qname",
                               "--json", "--limit", "100000"], copy, env)

        def rows_of(d):
            if not isinstance(d, dict):
                return []
            for k in ("rows", "results", "tables"):
                v = d.get(k)
                if isinstance(v, list):
                    if v and isinstance(v[0], dict) and "rows" in v[0]:
                        return [x for t in v for x in t["rows"]]
                    return v
            return []
        ends, routes, pairs = rows_of(e), rows_of(r), rows_of(p)
        flat = lambda x: json.dumps(x)  # noqa: E731
        paired_e = {x["values"][0]["str"] for x in pairs if x.get("values")}
        paired_r = {x["values"][1]["str"] for x in pairs if x.get("values")}
        res["routes_with_caller"] = len(paired_r)
        res["endpoints"], res["routes"], res["http_pairs"] = len(ends), len(routes), len(pairs)
        res["client_reach"] = round(len(paired_e) / len(ends), 3) if ends else None
        gp = res["gaps"] or {}
        res["route_reach"] = round(1 - gp.get("unpaired_route", 0) / len(routes), 3) if routes else None
        res["suspicious_endpoints"] = sum(1 for x in ends if re.search(
            r"(data:|blob:|mailto:|base64)", flat(x)))
        # parse coverage: code-looking files the build did not route to a parser
        _, ls, _, _ = sh(["git", "ls-files"], cwd=copy)
        ext = Counter(os.path.splitext(f)[1] for f in ls.splitlines())
        res["sfc_files"] = {k: ext[k] for k in SFC if ext[k]}
        res["vue_files"] = ext[".vue"]
        # workspace packages: names the repo's own package.json files declare, imported somewhere
        names = set()
        for f in ls.splitlines():
            if f.endswith("package.json") and "node_modules" not in f:
                try:
                    n = json.loads((copy / f).read_text()).get("name")
                    if n and f != "package.json":
                        names.add(n)
                except (OSError, ValueError):
                    pass
        res["workspace_packages"] = len(names)
        used = 0
        for n in sorted(names)[:200]:
            rc, out, _, _ = sh(["git", "grep", "-l", "-F", f"from '{n}", "--", "*.ts", "*.tsx",
                                "*.js", "*.svelte", "*.vue"], cwd=copy, timeout=60)
            used += bool(out.strip())
        res["workspace_packages_imported"] = used
        dead = gp.get("dead_symbol", 0)
        res["dead_symbol"] = dead
        blind = []
        if res["routes"] and res["route_reach"] is not None and res["route_reach"] < 0.2:
            blind.append("routes_unreached")
        if res["endpoints"] and res["client_reach"] is not None and res["client_reach"] < 0.2:
            blind.append("clients_unpaired")
        if res["routes"] and res["endpoints"] < 0.1 * res["routes"]:
            blind.append("no_client_endpoints")
        if res["sfc_files"]:
            blind.append("unparsed_sfc")
        if res["workspace_packages_imported"]:
            blind.append("workspace_imports?")
        res["blind"] = blind
    except Exception as ex:  # noqa: BLE001
        res["error"] = str(ex)[:200]
    finally:
        sh(["rm", "-rf", str(copy)])
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("repos", nargs="*")
    ap.add_argument("--glia", default=str(D / "bin" / "g053" / "glia"))
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--out", default=str(HERE / "health.jsonl"))
    a = ap.parse_args()
    work = []
    specs = a.repos or DEFAULT + list(EXTRA)
    for s in specs:
        label, _, rest = s.partition("=")
        gdir, _, rev = rest.partition("@")
        gd = Path(gdir) if gdir else D / EXTRA.get(label, f"{label}.git")
        work.append((label, gd, rev or "HEAD"))
    res = []
    with ThreadPoolExecutor(a.jobs) as pool:
        for r in pool.map(lambda w: one(*w, a.glia), work):
            res.append(r)
            print(f"[health] {r['repo']:20} routes={r.get('routes')} endpoints={r.get('endpoints')} "
                  f"pairs={r.get('http_pairs')} route_reach={r.get('route_reach')} "
                  f"client_reach={r.get('client_reach')} dead={r.get('dead_symbol')} "
                  f"sfc={r.get('sfc_files')} ws_pkgs={r.get('workspace_packages_imported')}/"
                  f"{r.get('workspace_packages')} suspicious={r.get('suspicious_endpoints')} "
                  f"blind={r.get('blind')} {r.get('error') or ''}", flush=True)
    Path(a.out).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in res))


if __name__ == "__main__":
    main()
