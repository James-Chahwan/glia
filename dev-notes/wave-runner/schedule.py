#!/usr/bin/env python3
"""Deterministic wave scheduler for the review-2026-09-15 programme.

Reads dev-notes/wave-packets.json and emits file-disjoint waves. Two packets
may share a wave only if their CLAIMED files are disjoint — agents edit one
working tree concurrently, so a shared file is a guaranteed conflict.

`engine/src/lib.rs` and `graph/src/lib.rs` were split in wave 0 (75b518d,
82a2ec0). Packet specs still name the pre-split files, so their claims are
re-mapped: engine by the per-packet map W0.1 reported, graph by the old
line-range -> module map W0.2 reported.

Lived in /tmp for waves 1-5 and was lost at a session boundary; committed here
so the programme is resumable. Must reproduce the waves that already landed —
run `schedule.py --verify` before trusting it for the next one.

Usage:
  python3 schedule.py            # print every wave
  python3 schedule.py --wave N   # print wave N's packets, one per line
  python3 schedule.py --verify   # assert W1-W5 match what was committed
  python3 schedule.py --json     # waves + claims as JSON
"""
import json
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKETS = ROOT / "dev-notes" / "wave-packets.json"

# Replaced by wave 0's frozen key.json vocabulary (8461a1c).
SUPERSEDED = {"A2.0", "A3.0", "A5.0", "A11.6", "A12.0", "A15.1"}
# Merged into A13.7 with mandatory redaction.
DELETED = {"A11.3"}
# Batch C is deferred to a second programme, except A7.0 which two MATRIX
# packets depend on.
KEEP_BATCHES = {"S1", "A", "B", "MATRIX"}
PULLED_FORWARD = {"A7.0"}

ENGINE_MAP = {
    "A1.1": "lib route", "A1.4": "build route", "A2.1": "extract", "A2.6": "coverage",
    "A2.8": "answers extract passes", "A3.4": "extract route", "A3.5": "extract",
    "A3.6": "answers", "A5.1": "build route", "A5.2": "answers build extract",
    "A5.3": "answers extract", "A5.8": "answers extract route", "A6.7": "passes",
    "A6.8": "build extract", "A7.0": "build route", "A7.8": "answers", "A8.1": "build walk",
    "A8.2": "walk", "A8.3": "answers", "A8.4": "build walk", "A8.5": "build walk",
    "A8.6": "answers", "A9.2": "answers build coverage lib walk", "A10.1": "build route",
    "A10.2": "passes", "A10.3": "route", "A10.4": "extract route", "A10.5": "route",
    "A10.6": "extract route", "A10.7": "build lib", "A10.8": "route walk",
    "A10.9": "extract", "A10.10": "build", "A11.1": "build extract route",
    "A11.2": "route", "A12.1": "build route", "A12.2": "answers build coverage",
    "A13.8": "extract route", "A13.9": "extract route walk",
    "A13.16": "extract route walk", "A14.1": "build coverage walk",
    "A14.2": "build coverage extract", "A16.3": "passes", "A16.4": "build route",
}

GRAPH_RANGES = [
    ((28, 77), "types"), ((79, 256), "build"), ((258, 530), "imports"),
    ((532, 809), "calls"), ((811, 1103), "merged"),
    ((1105, 1111), "resolvers/mod"), ((1336, 1345), "resolvers/mod"),
    ((1379, 1382), "resolvers/mod"), ((1930, 1957), "resolvers/mod"),
    ((1113, 1334), "resolvers/http"), ((1347, 1403), "resolvers/grpc"),
    ((1405, 1459), "resolvers/queue"), ((1461, 1500), "resolvers/graphql"),
    ((1502, 1542), "resolvers/websocket"), ((1544, 1573), "resolvers/eventbus"),
    ((1575, 1629), "resolvers/shared_schema"), ((1887, 1897), "resolvers/shared_schema"),
    ((1631, 1682), "resolvers/db"), ((1684, 1732), "resolvers/cron"),
    ((1734, 1782), "resolvers/config"), ((1784, 1833), "resolvers/iac"),
    ((1835, 1885), "resolvers/package"), ((1899, 1928), "resolvers/cli"),
    ((1959, 2133), "traversal"), ((2135, 2283), "blast"),
    ((2285, 2363), "activation"), ((2365, 2651), "signal"),
]

# What actually landed, by wave. --verify checks the scheduler reproduces it.
LANDED = {
    1: "A15.2 A2.1 A4.0 A1.1 A9.1 A8.3 A8.0 A5.6 A6.1 A4.11 A4.13 A6.10 A12.5 A13.6 A16.3 A13.7 A16.6 A6.9",
    2: "A15.3 A15.5 A5.1 A2.2 A4.1 A4.8 A4.5 A1.2 A16.7 A4.4 A4.12 A4.7 A16.1 A16.5 A1.3",
    3: "A15.4 A10.1 A2.3 A4.6 A15.6 A4.3 A13.3 A4.9 A16.2 A1.5",
    4: "A9.2 A3.4 A15.11 A2.5 A5.7 A13.5 A4.10 A4.2",
    5: "A8.1 A3.1 A9.4 A2.8 A15.12 A9.3",
    6: "A1.4 A10.2 A10.9 A15.7 A8.2 A2.6 A9.6 A2.7",
    7: "A10.5 A5.2 A15.8 A2.4 A3.2 A5.5 A1.6 A1.7",
    8: "A7.0 A15.10 A5.4 A2.9 A9.5",
}


def _graph_module(line):
    for (lo, hi), mod in GRAPH_RANGES:
        if lo <= line <= hi:
            return f"graph/src/{mod}.rs"
    return None


def load():
    data = json.loads(PACKETS.read_text())
    return {p["id"]: p for p in data["packets"]}


def programme(P):
    return {
        i for i, p in P.items()
        if (p["batch"] in KEEP_BATCHES or i in PULLED_FORWARD)
        and i not in SUPERSEDED and i not in DELETED
    }


def claims(pid, P):
    p, out = P[pid], set()
    for raw in p["files_touched"]:
        f = raw.strip().lstrip("/").replace(str(ROOT) + "/", "")
        if f == "engine/src/lib.rs" and pid in ENGINE_MAP:
            out |= {f"engine/src/{m}.rs" for m in ENGINE_MAP[pid].split()}
        elif f == "graph/src/lib.rs":
            lines = [int(n) for ep in p.get("entry_points", [])
                     for n in re.findall(r"graph(?:/src)?(?:/lib\.rs)?:(\d{2,4})", ep)]
            mods = {m for m in map(_graph_module, lines) if m}
            out |= mods or {f}
        else:
            out.add(f)
    return out


def schedule(P):
    prog = programme(P)
    C = {i: claims(i, P) for i in prog}
    deps = {i: {d.strip() for d in (P[i].get("depends_on") or []) if d.strip() in prog}
            for i in prog}
    unblocks = Counter(d for ds in deps.values() for d in ds)
    waves, done, left = [], set(), set(prog)
    while left:
        ready = sorted((i for i in left if deps[i] <= done),
                       key=lambda i: (-unblocks[i], -len(C[i]), i))
        if not ready:
            raise SystemExit(f"deadlock: {sorted(left)[:10]}")
        used, wave = set(), []
        for i in ready:
            if C[i] & used:
                continue
            wave.append(i)
            used |= C[i]
        waves.append(wave)
        done |= set(wave)
        left -= set(wave)
    return waves, C


def main():
    P = load()
    waves, C = schedule(P)
    args = sys.argv[1:]
    if "--verify" in args:
        bad = 0
        for n, want in LANDED.items():
            got = waves[n - 1]
            if got != want.split():
                bad += 1
                print(f"W{n} MISMATCH\n  landed:    {want}\n  scheduler: {' '.join(got)}")
            else:
                print(f"W{n} ok ({len(got)} packets)")
        print("VERIFIED" if not bad else f"{bad} waves differ")
        sys.exit(1 if bad else 0)
    if "--wave" in args:
        for pid in waves[int(args[args.index("--wave") + 1]) - 1]:
            print(pid)
        return
    if "--json" in args:
        print(json.dumps({"waves": waves, "claims": {k: sorted(v) for k, v in C.items()}}))
        return
    for n, w in enumerate(waves, 1):
        tag = " (landed)" if n in LANDED else ""
        print(f"W{n:<2} ({len(w):>2}){tag}: {' '.join(w)}")


if __name__ == "__main__":
    main()
