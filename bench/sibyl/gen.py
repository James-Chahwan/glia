#!/usr/bin/env python3
"""The Sibyl matrix, synthetic side (FH.11a): the axes a client <-> server exchange varies on,
a deterministic pairwise cover of them, and the per-case repos rendered from bench/sibyl/templates/
with their expected answers by construction.

  gen.py cover  [--transports rest] [--seed N] [--json]
  gen.py render --out DIR [--transports rest] [--case ID]... [--trees]

`cover` prints the case list and `[sibyl] cover cases=<n> pairs=<p>/<t> transports=<..>` on
stderr. `render` writes, per case, DIR/<id>/expected.json and each repo of the case as a git
repository with three commits, P (base), S (the server renamed the field) and C (the client
followed), each also at refs/sibyl/<state>; `--trees` writes plain DIR/<id>/<state>/<repo>/
trees instead. No glia runs here: run.py (FH.11b) builds and grades the rendered cases.

`evm` (FH.11c) was an OPT-IN transport until run.py's matrix-cell tables (`MECH` by transport,
`LANG` by client / server value) gained its rows; FH.23 added `MECH["evm"]`, `LANG["go"]` and
`LANG["solidity"]` and moved "evm" into `AXES["transport"]` in the same commit, so the default
cover, run.py's default run and the cohort gate (scripts/cohort-gate.sh) hold the evm cases.
`OPT_IN_TRANSPORTS` stays, empty, for a transport whose run.py rows are not written yet.

A transport module may declare `VALUES`, the client / server values its templates render (evm:
clients ts and go, server solidity). A value only one transport renders (`go`, `solidity`)
joins an axis only when a selected transport declares it, so a cover without evm is the cover
FH.11a / FH.11b wrote, case for case; `unrendered()` turns each (transport, value) a module does
not render into a forbidden pair beside the UNEXPRESSIBLE ones."""

import argparse
import json
import os
import random
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import templates  # noqa: E402
from templates.common import norm_path  # noqa: E402,F401  (run.py reads it from here)

SEED = 54

AXES = {
    "client": ["ts", "dart", "kotlin", "swift", "python"],
    "server": ["go-gin", "ts-nest", "ts-express", "java-spring", "python-fastapi",
               "rust-axum", "csharp-aspnet"],
    "transport": ["rest", "grpc", "graphql", "queue", "websocket", "evm"],
    # How a side builds its path / channel name: a literal, a constant prefix, an enum segment.
    "wiring_source": ["literal", "const", "enum-prefix"],
    # How the client reaches the transport: at the use site, through a hand-written helper, or
    # through a generated SDK.
    "wiring_call": ["direct", "wrapper", "generated-sdk"],
    "packaging": ["single", "workspace", "multi-repo"],
    # The client's model of the payload.
    "contract": ["declared-type", "schema-object", "plain-literal", "generated"],
}

# Pairs no stack of the matrix can express idiomatically; the cover never asks for them.
UNEXPRESSIBLE = [
    (("wiring_call", "generated-sdk"), ("wiring_source", "enum-prefix"),
     "SDK generators write a literal operation path under a base-path constant, never an enum"),
    (("wiring_call", "generated-sdk"), ("contract", "declared-type"),
     "an SDK generator writes its own models: a generated SDK's contract is generated"),
    (("wiring_call", "generated-sdk"), ("contract", "schema-object"),
     "an SDK generator writes its own models: a generated SDK's contract is generated"),
    (("wiring_call", "generated-sdk"), ("contract", "plain-literal"),
     "an SDK generator writes its own models: a generated SDK's contract is generated"),
    (("server", "java-spring"), ("wiring_source", "enum-prefix"),
     "Spring mapping annotations take compile-time constant strings; an enum constant is none"),
    (("server", "csharp-aspnet"), ("wiring_source", "enum-prefix"),
     "ASP.NET route attributes take constant strings; an enum member's name is none"),
    (("server", "rust-axum"), ("wiring_source", "enum-prefix"),
     "axum routes take &str; Rust code names a route prefix with a const, not an enum"),
    (("transport", "grpc"), ("wiring_call", "direct"),
     "a gRPC call goes through a stub; there is no hand-written request at the use site"),
    (("transport", "grpc"), ("wiring_source", "enum-prefix"),
     "a gRPC method path is /package.Service/Method from the .proto; there is no URL prefix"),
    (("transport", "graphql"), ("wiring_source", "enum-prefix"),
     "GraphQL posts every operation to one endpoint; there is no resource segment to prefix"),
    (("transport", "grpc"), ("contract", "plain-literal"),
     "protobuf messages are declared in the .proto and generated, never plain literals"),
    (("transport", "grpc"), ("contract", "schema-object"),
     "protobuf messages are declared in the .proto and generated, never schema objects"),
    (("transport", "grpc"), ("server", "ts-express"),
     "Express serves HTTP/1 routes; a Node gRPC server is grpc-js, another stack"),
    (("transport", "grpc"), ("server", "python-fastapi"),
     "FastAPI serves HTTP routes; a Python gRPC server is grpcio, another stack"),
    (("transport", "queue"), ("client", "dart"),
     "a Flutter app does not hold a broker connection; it reaches queues through a server"),
    (("transport", "queue"), ("client", "swift"),
     "an iOS app does not hold a broker connection; it reaches queues through a server"),
]


# Transports gen.py covers and renders only when asked by name (see the module doc): none since
# FH.23 moved evm into AXES["transport"].
OPT_IN_TRANSPORTS = []
ALL_TRANSPORTS = AXES["transport"] + OPT_IN_TRANSPORTS
# The axes a template module may narrow with `VALUES`.
VALUE_AXES = ("client", "server")


def values_of(transport, axis):
    """The `axis` values `transport`'s templates render: its module's VALUES, else AXES'."""
    mod = templates.TRANSPORTS[transport]
    return list(getattr(mod, "VALUES", {}).get(axis) or AXES[axis])


def all_values(axis):
    """Every value of `axis` some transport renders: AXES' first, then each module's extras in
    ALL_TRANSPORTS order."""
    if axis == "transport":
        return list(ALL_TRANSPORTS)
    out = list(AXES[axis])
    for t in ALL_TRANSPORTS:
        out += [v for v in values_of(t, axis) if v not in out]
    return out


def unrendered():
    """((transport, t), (axis, v), why) for every client / server value some transport renders
    and `t`'s templates do not."""
    rows = []
    for t in ALL_TRANSPORTS:
        for axis in VALUE_AXES:
            mine = values_of(t, axis)
            rows += [(("transport", t), (axis, v), f"the {t} templates render no {axis} {v}")
                     for v in all_values(axis) if v not in mine]
    return rows


def unexpressible():
    rows = list(UNEXPRESSIBLE)
    for mod in templates.TRANSPORTS.values():
        rows += list(getattr(mod, "UNEXPRESSIBLE", []))
    return rows + unrendered()


def axes_for(transports):
    bad = [t for t in transports if t not in ALL_TRANSPORTS]
    if bad or not transports:
        raise SystemExit(f"[sibyl] unknown transports {bad or transports}; "
                         f"known: {', '.join(ALL_TRANSPORTS)}")
    out = {}
    for k in AXES:
        if k == "transport":
            out[k] = sorted(transports, key=ALL_TRANSPORTS.index)
        elif k in VALUE_AXES:
            held = {v for t in transports for v in values_of(t, k)}
            out[k] = [v for v in all_values(k) if v in held]
        else:
            out[k] = list(AXES[k])
    return out


class Cover:
    """Greedy IPOG (Lei et al., In-Parameter-Order) over `axes` with forbidden pairs. Axes are
    taken largest first (ties in AXES order); a tie between equally good values is broken by a
    Random seeded with `seed`, so the cover is a function of (axes, forbidden pairs, seed)."""

    def __init__(self, axes, forbid, seed=SEED):
        self.axes = axes
        self.order = sorted(axes, key=lambda k: (-len(axes[k]), list(AXES).index(k)))
        self.rank = {n: i for i, n in enumerate(self.order)}
        self.forbid = set()
        for a, b, _why in forbid:
            if a[0] in axes and b[0] in axes and a[1] in axes[a[0]] and b[1] in axes[b[0]]:
                self.forbid.add(self.pair(a, b))
        self.rng = random.Random(seed)

    def pair(self, a, b):
        return (a, b) if self.rank[a[0]] < self.rank[b[0]] else (b, a)

    def pairs_of(self, t):
        items = sorted(t.items(), key=lambda kv: self.rank[kv[0]])
        return {(items[i], items[j]) for i in range(len(items))
                for j in range(i + 1, len(items))}

    def consistent(self, t):
        return not (self.pairs_of(t) & self.forbid)

    def completable(self, t):
        """Whether the partial case `t` extends to a full case with no forbidden pair."""
        if not self.consistent(t):
            return False
        free = [n for n in self.order if n not in t]

        def go(i, cur):
            if i == len(free):
                return True
            n = free[i]
            for v in self.axes[n]:
                if all(self.pair((n, v), (m, x)) not in self.forbid for m, x in cur.items()):
                    cur[n] = v
                    if go(i + 1, cur):
                        return True
                    del cur[n]
            return False

        return go(0, dict(t))

    def required(self):
        """(every pair a full valid case can hold, the pairs only an interaction rules out)."""
        req, implied = set(), set()
        for i, a in enumerate(self.order):
            for b in self.order[i + 1:]:
                for x in self.axes[a]:
                    for y in self.axes[b]:
                        p = ((a, x), (b, y))
                        if p in self.forbid:
                            continue
                        (req if self.completable({a: x, b: y}) else implied).add(p)
        return req, implied

    def build(self):
        req, implied = self.required()
        uncovered = set(req)
        p0, p1 = self.order[0], self.order[1]
        tests = [{p0: x, p1: y} for x in self.axes[p0] for y in self.axes[p1]
                 if self.completable({p0: x, p1: y})]
        for t in tests:
            uncovered -= self.pairs_of(t)
        for k in range(2, len(self.order)):
            pk = self.order[k]
            for t in tests:  # horizontal growth
                best, score = [], -1
                for v in self.axes[pk]:
                    cand = dict(t, **{pk: v})
                    if not self.completable(cand):
                        continue
                    n = sum(1 for p in t if self.pair((p, t[p]), (pk, v)) in uncovered)
                    if n > score:
                        best, score = [v], n
                    elif n == score:
                        best.append(v)
                if best:
                    t[pk] = self.rng.choice(best)
                    uncovered -= self.pairs_of(t)
            for p in sorted(x for x in uncovered if x[1][0] == pk):  # vertical growth
                if p not in uncovered:
                    continue
                (a, x), (_, v) = p
                for t in tests:
                    if (a not in t or pk not in t) and t.get(a, x) == x and t.get(pk, v) == v:
                        cand = dict(t, **{a: x, pk: v})
                        if self.completable(cand):
                            t.update(cand)
                            break
                else:
                    t = {a: x, pk: v}
                    tests.append(t)
                uncovered -= self.pairs_of(t)
        for t in tests:  # the don't-care slots
            for n in self.order:
                if n not in t:
                    ok = [v for v in self.axes[n] if self.completable(dict(t, **{n: v}))]
                    t[n] = self.rng.choice(ok)
        if uncovered:
            left = sorted(uncovered)
            raise RuntimeError(f"[sibyl] cover left {len(left)} pairs: {left[:5]}")
        return tests, req, implied


def cover(transports=("rest",), seed=SEED, quiet=False):
    """The case list: [{"id", "axes"}], in construction order. Prints the marker."""
    axes = axes_for(list(transports))
    c = Cover(axes, unexpressible(), seed)
    tests, req, implied = c.build()
    cases = []
    for i, t in enumerate(tests, 1):
        ax = {k: t[k] for k in AXES}
        cases.append({"id": f"{ax['transport']}-{i:03d}-{ax['client']}-{ax['server']}",
                      "axes": ax})
    held = set()
    for t in tests:
        held |= c.pairs_of(t) & req
    if not quiet:
        print(f"[sibyl] cover cases={len(cases)} pairs={len(held)}/{len(req)} "
              f"transports={','.join(axes['transport'])}"
              + (f" implied_unexpressible={len(implied)}" if implied else ""),
              file=sys.stderr)
    return cases


def renderable(case):
    mod = templates.TRANSPORTS.get(case["axes"]["transport"])
    if mod is None:
        raise SystemExit(f"[sibyl] no templates for transport {case['axes']['transport']!r} "
                         f"(have: {', '.join(templates.TRANSPORTS)})")
    return mod


def render_case(case):
    """{state: {repo: {path: text}}} and the expected answer."""
    mod = renderable(case)
    return {s: mod.render(case, s) for s in mod.STATES}, mod.expected(case)


GIT_ENV = {
    "GIT_AUTHOR_NAME": "sibyl", "GIT_AUTHOR_EMAIL": "sibyl@example.invalid",
    "GIT_COMMITTER_NAME": "sibyl", "GIT_COMMITTER_EMAIL": "sibyl@example.invalid",
    "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
}


def write_tree(root, files):
    for rel, text in files.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)


def git(d, *args, env=None):
    subprocess.run(["git", "-C", str(d), *args], check=True, capture_output=True,
                   env={**os.environ, **GIT_ENV, **(env or {})})


def write_git(case_dir, states, notes):
    """One git repo per case repo; commits P, S, C (empty when that repo did not change, so
    HEAD~1 is the previous state in every repo) with fixed identity and dates."""
    repos = sorted({r for s in states.values() for r in s})
    for repo in repos:
        d = case_dir / repo
        d.mkdir(parents=True)
        git(d, "init", "-q", "-b", "main")
        for i, (state, by_repo) in enumerate(states.items()):
            for child in d.iterdir():
                if child.name != ".git":
                    shutil.rmtree(child) if child.is_dir() else child.unlink()
            write_tree(d, by_repo.get(repo, {}))
            git(d, "add", "-A")
            date = f"2026-01-01T00:0{i}:00+00:00"
            git(d, "commit", "-q", "--allow-empty", "--no-gpg-sign", "-m",
                f"sibyl {state}: {notes[state]}",
                env={"GIT_AUTHOR_DATE": date, "GIT_COMMITTER_DATE": date})
            git(d, "update-ref", f"refs/sibyl/{state}", "HEAD")


def cmd_cover(a):
    cases = cover(a.transports.split(","), a.seed)
    if a.json:
        print(json.dumps(cases, indent=1))
    else:
        cols = list(AXES)
        print("\t".join(["id"] + cols))
        for c in cases:
            print("\t".join([c["id"]] + [c["axes"][k] for k in cols]))
    return 0


def cmd_render(a):
    cases = cover(a.transports.split(","), a.seed)
    if a.case:
        known = {c["id"] for c in cases}
        missing = [x for x in a.case if x not in known]
        if missing:
            raise SystemExit(f"[sibyl] unknown case ids: {', '.join(missing)}")
        cases = [c for c in cases if c["id"] in a.case]
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    for c in cases:
        if (out / c["id"]).exists():
            raise SystemExit(f"[sibyl] {out / c['id']} exists; render into an empty directory")
    for c in cases:
        states, exp = render_case(c)
        cd = out / c["id"]
        cd.mkdir()
        (cd / "expected.json").write_text(json.dumps(exp, indent=1) + "\n")
        if a.trees:
            for state, by_repo in states.items():
                for repo, files in by_repo.items():
                    write_tree(cd / state / repo, files)
        else:
            write_git(cd, states, exp["states"])
    (out / "cases.json").write_text(json.dumps(cases, indent=1) + "\n")
    print(f"[sibyl] rendered cases={len(cases)} out={out} form={'trees' if a.trees else 'git'}",
          file=sys.stderr)
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("cover", "render"):
        p = sub.add_parser(name)
        p.add_argument("--transports", default="rest")
        p.add_argument("--seed", type=int, default=SEED)
        if name == "cover":
            p.add_argument("--json", action="store_true")
        else:
            p.add_argument("--out", required=True)
            p.add_argument("--case", action="append")
            p.add_argument("--trees", action="store_true")
    a = ap.parse_args(argv)
    return cmd_cover(a) if a.cmd == "cover" else cmd_render(a)


if __name__ == "__main__":
    sys.exit(main())
