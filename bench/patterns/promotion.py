#!/usr/bin/env python3
"""The promotion criterion for `glia patterns` (CC.12a), written and executable.

Pattern conformance (LE.7a / LE.7b) ships EXPERIMENTAL: the CLI refuses to run
without --experimental, the pyo3 names carry `_experimental` and every report
says `experimental: true`. This script is the one place that decides whether it
may drop that: it measures three real repos against five FIXED criteria and
records the numbers, pass or fail. The thresholds below are the criterion; they
are never tuned to make a run pass, and changing one is a reviewed diff with its
reason in the commit.

  C1  at least MIN_JUDGED_REPOS of the repos have >= 1 judged population
      (the sighted-only verdict, CA.5b).
  C2  in every judged population the blind count (the population's `blind`
      list: handlers whose signature is `handler>(no effect)`) is at most
      MAX_BLIND_PCT percent of the population's size.
  C3  `handler>(no effect)` is never a convention (the engine enforces it;
      this asserts it on every population and every divergence).
  C4  every hop of every divergence's path is a real edge: per distinct hop,
      `glia why <copy> <from> <to> --category <CAT> --json` exits 0 with
      `found: true` (a backward IMPLEMENTS hop is asked the way the edge runs,
      implementation -> interface method).
  C5  two runs of `glia patterns <copy> --experimental --json` print
      byte-identical JSON.

promote = C1 and C2 and C3 and C4 and C5.

The repos are git-archive copies at recorded SHAs (REPOS; bumping one is a
reviewed diff), extracted into a temporary directory, never the live checkout;
every glia call runs with GLIA_NO_PERSIST=1. The script is stdlib only and never
imports glia_py (the wheel's generate() writes into the repo it builds). Run it
with ~/.venvs/glia-leap/bin/python for uniformity with the other bench scripts.

    promotion.py [--glia target/debug/glia] [--out bench/patterns/promotion-0.5.1.json]
                 [--repos-root ~/Code] [--work-dir DIR]
    promotion.py --self-test

Exit 0 iff promote, 1 when a criterion fails, 2 on a measurement error (a glia
call failing, the engine's marker disagreeing with its JSON). /tmp may be a RAM
disk: pass --work-dir (or TMPDIR) to keep the copies on disk.

The engine's fired_on line, parsed and cross-checked against the JSON once per
repo per run:
  [patterns] experimental populations=<P> judged=<J> handlers=<H> divergences=<D> skipped_small=<S> role_sources edge=<E> kind=<K> name=<N> blind=<B> group_by=<g>
This script's, on stderr, once per repo and once for the verdict
(grep `^\\[promotion\\] `):
  [promotion] repo=<name> sha=<sha> judged=<J> blind=<b> -> C1=<pass|fail> C2=.. C3=.. C4=.. C5=.. promote=<true|false>
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# The criterion. Never tuned to pass (module docs).
MIN_JUDGED_REPOS = 2
MAX_BLIND_PCT = 25
NO_EFFECT = "handler>(no effect)"
TOP_SIGNATURES = 5

# (name, sha): a git-archive copy of <repos-root>/<name> at <sha>.
REPOS = [
    ("quokka-stack", "a77d4cb"),
    ("lapse", "c0ece02"),
    ("Kina", "a448fb3"),
]

# 0.5.0's line ends at `name=<N>`; CA.5b added `blind=` and `group_by=`.
MARKER = re.compile(
    r"^\[patterns\] experimental populations=(\d+) judged=(\d+) handlers=(\d+) "
    r"divergences=(\d+) skipped_small=(\d+) role_sources edge=\d+ kind=\d+ name=\d+"
    r"(?: blind=(\d+))?(?: group_by=\S+)?$",
    re.M,
)


class MeasureError(Exception):
    """A measurement that cannot be trusted: exit 2, nothing recorded."""


# ---- the criterion: pure functions over measured repos ----------------------


def blind_of(pop):
    """The population's blind count: its `blind` list (CA.5b), or on a pre-CA
    report its `handler>(no effect)` signature count (the same members)."""
    if "blind" in pop:
        return len(pop["blind"])
    return sum(n for sig, n in pop["signatures"] if sig == NO_EFFECT)


def label(name, pop):
    pkg = pop.get("package")
    return f"{name}/{pop['service']}" + (f" / {pkg}" if pkg else "")


def hop_key(hop):
    return (hop["from_qname"], hop["to_qname"], hop["category"], hop["backward"])


def divergence_hops(report):
    """Every distinct hop of every divergence's path, in first-seen order."""
    seen = {}
    for d in report["divergences"]:
        for h in d["path"]:
            seen.setdefault(hop_key(h), h)
    return list(seen.values())


def c1(repos):
    judged = [r for r in repos if r["report"]["judged"] >= 1]
    rows = "; ".join(f"{r['name']} judged={r['report']['judged']}" for r in repos)
    return (
        len(judged) >= MIN_JUDGED_REPOS,
        f"{len(judged)} of {len(repos)} repos have a judged population "
        f"(need >= {MIN_JUDGED_REPOS}): {rows}",
    )


def c2(repos):
    rows, over = [], []
    for r in repos:
        for p in r["report"]["populations"]:
            if p["status"] != "judged":
                continue
            b = blind_of(p)
            row = f"{label(r['name'], p)} blind {b}/{p['size']}"
            rows.append(row)
            if b * 100 > MAX_BLIND_PCT * p["size"]:
                over.append(row)
    head = f"blind <= {MAX_BLIND_PCT}% of size in every judged population"
    if not rows:
        return True, f"{head}: no judged population (vacuous)"
    if over:
        return False, f"{head}: {len(over)} of {len(rows)} over: " + "; ".join(over)
    return True, f"{head}: " + "; ".join(rows)


def c3(repos):
    bad = [
        label(r["name"], p)
        for r in repos
        for p in r["report"]["populations"]
        if p.get("convention") == NO_EFFECT
    ]
    bad += [
        f"{r['name']}: divergence {d['handler']}"
        for r in repos
        for d in r["report"]["divergences"]
        if d.get("convention") == NO_EFFECT
    ]
    if bad:
        return False, f"`{NO_EFFECT}` is a convention: " + "; ".join(bad)
    return True, f"`{NO_EFFECT}` is no population's convention"


def c4(repos):
    total, missing = 0, []
    for r in repos:
        checked = {hop_key(h): h for h in r["hops"]}
        for h in divergence_hops(r["report"]):
            total += 1
            got = checked.get(hop_key(h))
            if got is None or not got["found"]:
                why = "unchecked" if got is None else (got.get("error") or "not found")
                missing.append(
                    f"{r['name']}: {h['from_qname']} -[{h['category']}"
                    f"{' backward' if h['backward'] else ''}]-> {h['to_qname']} ({why})"
                )
    head = f"{total - len(missing)}/{total} divergence path hops are edges (glia why --category)"
    return not missing, head + ("" if not missing else ": " + "; ".join(missing))


def c5(repos):
    differ = [r["name"] for r in repos if not r["identical"]]
    head = f"{len(repos) - len(differ)}/{len(repos)} repos: two patterns runs byte-identical"
    return not differ, head + ("" if not differ else "; differ: " + ", ".join(differ))


CRITERIA = {"C1": c1, "C2": c2, "C3": c3, "C4": c4, "C5": c5}


def evaluate(repos):
    """{C1..C5: (pass, detail)} over the measured repos."""
    return {k: f(repos) for k, f in CRITERIA.items()}


def promoted(verdicts):
    return all(ok for ok, _ in verdicts.values())


def repo_verdicts(r):
    """One repo's share: C1 is `judged >= 1`, C2..C5 the criterion on it alone."""
    v = {k: f([r])[0] for k, f in CRITERIA.items() if k != "C1"}
    return {"C1": r["report"]["judged"] >= 1, **v}


def check_marker(line, report):
    """The engine's marker must say what its JSON says."""
    m = MARKER.search(line)
    if not m:
        raise MeasureError("no `[patterns] experimental populations=` line on stderr")
    p, j, h, d, s, b = m.groups()
    want = [
        ("populations", int(p), len(report["populations"])),
        ("judged", int(j), report["judged"]),
        ("handlers", int(h), report["handlers"]),
        ("divergences", int(d), len(report["divergences"])),
        ("skipped_small", int(s), report["skipped_small"]),
    ]
    if b is not None:
        want.append(("blind", int(b), report.get("blind")))
    for field, marker, json_value in want:
        if marker != json_value:
            raise MeasureError(f"marker {field}={marker} but JSON says {json_value}")
    return m.group(0)


# ---- the measurement --------------------------------------------------------


def glia_env():
    env = dict(os.environ)
    env["GLIA_NO_PERSIST"] = "1"
    return env


def extract(repo, sha, dest):
    if not (repo / ".git").exists():
        raise MeasureError(f"{repo} is not a git checkout")
    dest.mkdir(parents=True)
    git = subprocess.Popen(["git", "-C", str(repo), "archive", sha], stdout=subprocess.PIPE)
    tar = subprocess.run(["tar", "-x", "-C", str(dest)], stdin=git.stdout)
    git.stdout.close()
    if git.wait() != 0 or tar.returncode != 0:
        raise MeasureError(f"git archive {repo.name}@{sha} | tar failed")


def run_patterns(glia, copy):
    p = subprocess.run(
        [glia, "patterns", str(copy), "--experimental", "--json"],
        capture_output=True,
        env=glia_env(),
    )
    if p.returncode != 0:
        tail = p.stderr.decode(errors="replace").strip().splitlines()[-1:]
        raise MeasureError(f"glia patterns {copy.name} exited {p.returncode}: {tail}")
    return p.stdout, p.stderr.decode(errors="replace")


def check_hop(glia, copy, hop):
    """`glia why` the way the edge runs: a backward hop walked an IMPLEMENTS
    edge from the interface method to its implementation."""
    a, b = hop["from_qname"], hop["to_qname"]
    if hop["backward"]:
        a, b = b, a
    p = subprocess.run(
        [glia, "why", str(copy), a, b, "--category", hop["category"], "--json"],
        capture_output=True,
        env=glia_env(),
    )
    out = {k: hop[k] for k in ("from_qname", "to_qname", "category", "backward")}
    if p.returncode in (0, 1):
        found = bool(json.loads(p.stdout)["found"])
        if found != (p.returncode == 0):
            raise MeasureError(f"glia why exit {p.returncode} but found={found}")
        out["found"] = found
    else:
        err = p.stderr.decode(errors="replace").strip().splitlines()
        err = [line for line in err if not line.startswith("[")][-1:] or ["no message"]
        out["found"] = False
        out["error"] = f"why exited {p.returncode}: " + err[0].replace(str(copy), copy.name)
    return out


def measure(glia, repos_root, work, name, sha):
    copy = work / name
    extract(repos_root / name, sha, copy)
    first, err1 = run_patterns(glia, copy)
    second, err2 = run_patterns(glia, copy)
    report = json.loads(first)
    marker = check_marker(err1, report)
    again = MARKER.search(err2)
    if again is None or again.group(0) != marker:
        raise MeasureError(f"{name}: the two runs' markers differ")
    hops = [check_hop(glia, copy, h) for h in divergence_hops(report)]
    return {"name": name, "sha": sha, "report": report, "identical": first == second,
            "hops": hops, "marker": marker}


def row(r):
    """The recorded per-repo row: counts, top signatures, every population."""
    rep = r["report"]
    sigs = Counter()
    for p in rep["populations"]:
        for sig, n in p["signatures"]:
            sigs[sig] += n
    top = sorted(sigs.items(), key=lambda kv: (-kv[1], kv[0]))[:TOP_SIGNATURES]
    pops = [
        {
            "service": p["service"],
            "package": p.get("package"),
            "size": p["size"],
            "sighted": p.get("sighted", p["size"] - blind_of(p)),
            "blind": blind_of(p),
            "status": p["status"],
            "convention": p.get("convention"),
            "verdict": p.get("verdict"),
        }
        for p in rep["populations"]
    ]
    return {
        "name": r["name"],
        "sha": r["sha"],
        "populations": len(rep["populations"]),
        "judged": rep["judged"],
        "blind": rep.get("blind", sum(x["blind"] for x in pops)),
        "handlers": rep["handlers"],
        "divergences": len(rep["divergences"]),
        "top_signatures": [[s, n] for s, n in top],
        "population_detail": pops,
        "hops_checked": len(r["hops"]),
        "marker": r["marker"],
        "verdicts": {k: "pass" if ok else "fail" for k, ok in repo_verdicts(r).items()},
    }


def fired_on(r, promote):
    v = repo_verdicts(r)
    marks = " ".join(f"{k}={'pass' if v[k] else 'fail'}" for k in CRITERIA)
    return (f"[promotion] repo={r['name']} sha={r['sha']} judged={r['report']['judged']} "
            f"blind={row(r)['blind']} -> {marks} promote={str(promote).lower()}")


def record(glia_version, repos):
    verdicts = evaluate(repos)
    return {
        "glia": glia_version,
        "repos": [row(r) for r in repos],
        "criteria": {k: {"pass": ok, "detail": d} for k, (ok, d) in verdicts.items()},
        "promote": promoted(verdicts),
    }


def run(args):
    glia = str(Path(args.glia).resolve())
    ver = subprocess.run([glia, "--version"], capture_output=True, text=True)
    if ver.returncode != 0:
        raise MeasureError(f"{glia} --version exited {ver.returncode}")
    repos_root = Path(args.repos_root).expanduser()
    with tempfile.TemporaryDirectory(prefix="glia-promotion-", dir=args.work_dir) as tmp:
        repos = [measure(glia, repos_root, Path(tmp), name, sha) for name, sha in REPOS]
    rec = record(ver.stdout.strip(), repos)
    for r in repos:
        print(fired_on(r, rec["promote"]), file=sys.stderr)
    print("[promotion] verdict "
          + " ".join(f"{k}={'pass' if c['pass'] else 'fail'}" for k, c in rec["criteria"].items())
          + f" promote={str(rec['promote']).lower()}", file=sys.stderr)
    Path(args.out).write_text(json.dumps(rec, indent=2, sort_keys=True) + "\n")
    return 0 if rec["promote"] else 1


# ---- self-test over canned reports ------------------------------------------


def _pop(service, size, blind, status, convention, sigs):
    return {"service": service, "package": None, "size": size, "sighted": size - blind,
            "status": status, "convention": convention, "signatures": sigs,
            "blind": [{"handler": f"{service}::blind{i}"} for i in range(blind)]}


def _hop(a, b, cat="CALLS", backward=False):
    return {"from_qname": a, "to_qname": b, "category": cat, "backward": backward}


def _repo(name, pops, divs=(), missing=(), identical=True):
    report = {"populations": list(pops), "divergences": list(divs),
              "judged": sum(p["status"] == "judged" for p in pops),
              "handlers": sum(p["size"] for p in pops), "skipped_small": 0,
              "blind": sum(len(p["blind"]) for p in pops)}
    hops = [dict(h, found=hop_key(h) not in missing) for h in divergence_hops(report)]
    return {"name": name, "sha": "0000000", "report": report, "identical": identical,
            "hops": hops, "marker": ""}


def _div(handler, sig, conv, hops):
    return {"handler": handler, "signature": sig, "convention": conv, "path": hops}


def self_test():
    div = _div("turps::h9", "handler>db", "handler>rpc_call",
               [_hop("turps::h9", "turps::Repo::Get"),
                _hop("turps::Repo::Get", "data_entity:mongo:users", "ACCESSES_DATA")])
    missing_hop = _hop("turps::h9", "turps::Repo::Get")

    def judged_set(quokka_blind, lapse_conv="handler>db", missing=(), identical=True):
        return [
            _repo("quokka-stack", [_pop("turps", 10, quokka_blind, "judged", "handler>rpc_call",
                                        [["handler>rpc_call", 7], ["handler>db", 1]])], [div],
                  missing=missing),
            _repo("lapse", [_pop("lapse", 20, 4, "judged", lapse_conv, [["handler>db", 16]])]),
            _repo("Kina", [_pop("backend", 35, 5, "no_convention", None,
                                [["handler>service>db", 23], [NO_EFFECT, 5]])],
                  identical=identical),
        ]

    # The HEAD numbers the spec measured at 2170ff8 (pre-CA): 0 judged in 3.
    head = [
        _repo("quokka-stack", [_pop("turps", 59, 58, "no_convention", None,
                                    [[NO_EFFECT, 58], ["handler>rpc_call", 1]])]),
        _repo("lapse", [_pop("lapse", 65, 42, "no_convention", None,
                             [[NO_EFFECT, 42], ["handler>db", 17],
                              ["handler>repository>db", 5], ["handler>http_call", 1]])]),
        _repo("Kina", [_pop("backend", 35, 5, "no_convention", None,
                            [["handler>service>db", 23],
                             ["handler>service>repository>service>db", 6],
                             [NO_EFFECT, 5], ["handler>repository>db", 1]])]),
    ]
    cases = [
        ("HEAD baseline: 0 judged in 3 -> C1 fails", head,
         {"C1": False, "C2": True, "C3": True, "C4": True, "C5": True}, False),
        ("two judged, blind 20% -> promote", judged_set(2),
         {"C1": True, "C2": True, "C3": True, "C4": True, "C5": True}, True),
        ("blind 40% -> C2 fails", judged_set(4),
         {"C1": True, "C2": False, "C3": True, "C4": True, "C5": True}, False),
        ("no-effect convention, a missing hop, a differing run -> C3 C4 C5 fail",
         judged_set(2, lapse_conv=NO_EFFECT, missing={hop_key(missing_hop)}, identical=False),
         {"C1": True, "C2": True, "C3": False, "C4": False, "C5": False}, False),
    ]
    failures = 0
    for title, repos, want, want_promote in cases:
        v = evaluate(repos)
        got = {k: ok for k, (ok, _) in v.items()}
        ok = got == want and promoted(v) == want_promote
        failures += not ok
        print(f"[promotion] self-test {'ok' if ok else 'FAIL'}: {title}", file=sys.stderr)
        if not ok:
            for k, (passed, detail) in v.items():
                print(f"    {k} {passed} (want {want[k]}): {detail}", file=sys.stderr)
    if "0 of 3 repos" not in evaluate(head)["C1"][1]:
        failures += 1
        print("[promotion] self-test FAIL: C1 detail does not say `0 of 3 repos`", file=sys.stderr)

    # The marker cross-check, on the 0.5.0 and the CA.5b line.
    rep = judged_set(2)[0]["report"]
    for line, bad in [
        ("[patterns] experimental populations=1 judged=1 handlers=10 divergences=1 "
         "skipped_small=0 role_sources edge=1 kind=2 name=0 blind=2 group_by=service", False),
        ("[patterns] experimental populations=1 judged=1 handlers=10 divergences=1 "
         "skipped_small=0 role_sources edge=1 kind=2 name=0", False),
        ("[patterns] experimental populations=1 judged=0 handlers=10 divergences=1 "
         "skipped_small=0 role_sources edge=1 kind=2 name=0 blind=2 group_by=service", True),
    ]:
        try:
            check_marker(line, rep)
            raised = False
        except MeasureError:
            raised = True
        if raised != bad:
            failures += 1
            print(f"[promotion] self-test FAIL: marker cross-check on {line!r}", file=sys.stderr)
    print(f"[promotion] self-test {'passed' if not failures else f'{failures} failed'}",
          file=sys.stderr)
    return 0 if not failures else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--glia", default=str(ROOT / "target" / "debug" / "glia"))
    ap.add_argument("--out", default=str(ROOT / "bench" / "patterns" / "promotion-0.5.1.json"))
    ap.add_argument("--repos-root", default=str(ROOT.parent),
                    help="directory holding the checkouts named in REPOS (default: glia's parent)")
    ap.add_argument("--work-dir", default=None,
                    help="where the temporary copies go (default: TMPDIR)")
    ap.add_argument("--self-test", action="store_true",
                    help="evaluate the criterion over canned reports; no glia call")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    try:
        return run(args)
    except MeasureError as e:
        print(f"[promotion] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
