#!/usr/bin/env python3
"""Wave scheduler for the glia 0.5.1 packet set (dev-notes/leap-051-packets.json).

Rule (the 0.5.0 leap's, dev-notes/wave-runner/leap_schedule.py): two packets share
a wave only if their CLAIMED files (files_touched) are disjoint, because the
implementing agents edit one working tree concurrently. Wave 0 is the C0 packets,
run one at a time in `wave0_serial` order; every other packet implicitly depends on
all of them. After that, a packet is ready when every depends_on is done, and each
wave is the maximal set of ready packets, taken in id order, with pairwise-disjoint
files_touched.

The runner schedules with at most CAP packets per wave (0.5.0's largest wave was 26):
the spec run measured 12 waves with and without the cap, and a smaller wave keeps the
end-of-wave gate readable. gen_wave.py / closeout.py `--release 051` read `waves()`.

Usage:
  python3 schedule_051.py                  # waves, sizes, critical path
  python3 schedule_051.py --verify         # assert LANDED waves still match
  python3 schedule_051.py --wave N         # wave N's packets, one per line (W0 = the serial C0 packets)
  python3 schedule_051.py --priority       # also run 0.5.0's order (most-unblocking first)
  python3 schedule_051.py --json           # machine-readable waves
  python3 schedule_051.py --stats          # contention table (files with 3+ claimants)
  python3 schedule_051.py --cap 26         # also schedule with at most N packets per wave
  python3 schedule_051.py --file other.json
"""
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKETS = HERE.parents[1] / "dev-notes" / "leap-051-packets.json"
CAP = 26

# Waves that have landed, by number; closeout.py --release 051 appends each one, and
# --verify then guards that re-running the scheduler reproduces history.
LANDED = {
    0: "C0.3 C0.1 C0.6 C0.7 C0.2 C0.4 C0.5",
    1: "CA.1 CA.6a CA.7 CA.8 CB.2 CB.4 CB.5 CB.6 CB.9 CB.10 CB.12 CB.13 CB.16 CC.1 CC.2 CC.3 CC.5a CC.8a CD.1a CD.3a CD.4a CD.5a CD.6a CD.7b CE.1a CE.4a",
    2: "CA.2a CA.4 CA.6b CA.9 CB.14 CB.17 CB.19 CC.5b CC.6a CC.8b CC.9a CC.10a CC.11a CD.1b CD.2a CD.4b CD.4d CE.1b CE.4b CF.1 CF.2a CF.2b CF.3 CF.4a CF.4b CF.5a",
    3: "CA.2b CB.1 CB.21 CC.5c CC.6b CC.8c CC.9b CC.10b CC.11b CD.1c CD.4e CD.7a CD.7c CE.1c CE.4c CF.5b CF.6a CF.6b CF.7a CF.7b CF.7c CF.8a CF.8b CF.8c CF.9a CF.9b",
    4: "CA.3a CB.7 CB.22 CB.24 CC.4a CC.11c CD.1d CD.4c CD.5b CE.1d CE.3a CE.4d CF.9c CF.10a CF.10b CF.10c CF.11a CF.11b CF.11c CF.13a CG.1 CG.2a CG.2b CG.4a",
    5: "CA.3b CB.3a CC.4b CC.7a CD.1e CD.2b CD.3b CD.5c CE.1e CE.2a CE.4e CF.13b CG.3",
    6: "CA.5a CB.3b CB.20 CC.4c CC.7b CD.2c CD.3c CD.5d CE.2b CE.3b CE.4f CG.4b",
    7: "CA.5b CB.11 CB.15 CC.7c CD.2d CE.2c CE.3c",
    8: "CB.18 CB.23 CC.12a CD.4f CE.2d CE.3d",
    9: "CB.25 CC.12b CE.2e CE.3e",
    10: "CB.26 CE.3f",
    11: "CZ.1 CZ.2",
    12: "C0.9 CH.1 CH.5a CI.1 CJ.1a CJ.2 CJ.3 CJ.4 CK.2 CL.5a CL.11",
    13: "CH.1b CI.5 CJ.1b CK.1 CK.3 CL.1 CL.5b CL.8 CL.10",
    14: "CH.1c CH.2 CI.3 CJ.1c CL.2 CL.6a",
    15: "CH.3a CI.2a CL.3 CL.6b CL.7a CL.9",
    16: "CH.3b CI.2b CL.7b",
    17: "CH.3c CH.5b CI.4 CL.4",
    18: "CH.4 CH.5c CI.6",
}
GROUP_ORDER = {"0": 0, "A": 1, "B": 2, "C": 3, "D": 4, "E": 5, "F": 6, "Z": 9}


def id_key(pid):
    m = re.fullmatch(r"C([0-9A-Z])\.(\d+)([a-z]?)", pid)
    if not m:
        raise SystemExit(f"bad packet id {pid!r}")
    return (GROUP_ORDER.get(m.group(1), 8), m.group(1), int(m.group(2)), m.group(3))


def load(path):
    doc = json.loads(Path(path).read_text())
    P = {p["id"]: p for p in doc["packets"]}
    serial = list(doc.get("wave0_serial", []))
    claims = {i: set(p["files_touched"]) for i, p in P.items()}
    deps = {i: set(p.get("depends_on") or []) for i, p in P.items()}
    missing = sorted({(i, d) for i, ds in deps.items() for d in ds if d not in P})
    if missing:
        raise SystemExit(f"depends_on names no packet: {missing[:10]}")
    for i in P:
        if i not in serial:
            deps[i] |= set(serial)
    return P, claims, deps, serial


def check_acyclic(deps):
    state = {}

    def visit(i, stack):
        s = state.get(i)
        if s == 1:
            cyc = stack[stack.index(i):] + [i]
            raise SystemExit("dependency cycle: " + " -> ".join(cyc))
        if s == 2:
            return
        state[i] = 1
        for d in sorted(deps[i]):
            visit(d, stack + [i])
        state[i] = 2

    for i in sorted(deps, key=id_key):
        visit(i, [])


def schedule(P, claims, deps, serial, priority=False, cap=0):
    unblocks = Counter(d for ds in deps.values() for d in ds)
    waves = [list(serial)]
    done = set(serial)
    left = set(P) - done
    while left:
        ready = [i for i in left if deps[i] <= done]
        if not ready:
            raise SystemExit(f"deadlock: {sorted(left, key=id_key)[:12]}")
        if priority:
            ready.sort(key=lambda i: (-unblocks[i], -len(claims[i]), id_key(i)))
        else:
            ready.sort(key=id_key)
        used, wave = set(), []
        for i in ready:
            if claims[i] & used:
                continue
            if cap and len(wave) >= cap:
                break
            wave.append(i)
            used |= claims[i]
        waves.append(sorted(wave, key=id_key))
        done |= set(wave)
        left -= set(wave)
    return waves


def critical_path(P, deps, serial):
    """Longest depends_on chain over the non-C0 packets (C0 is the serial wave 0)."""
    memo = {}

    def depth(i):
        if i not in memo:
            best = (0, [])
            for d in deps[i]:
                if d in serial:
                    continue
                cand = depth(d)
                if cand[0] > best[0] or (cand[0] == best[0] and cand[1] < best[1]):
                    best = cand
            memo[i] = (best[0] + 1, best[1] + [i])
        return memo[i]

    return max((depth(i) for i in P if i not in serial), key=lambda t: (t[0], [id_key(x) for x in t[1]]))


def waves(path=PACKETS):
    """(P, claims, waves) exactly as the runner schedules them."""
    P, claims, deps, serial = load(path)
    check_acyclic(deps)
    return P, claims, schedule(P, claims, deps, serial, cap=CAP)


def main(argv):
    path = PACKETS
    if "--file" in argv:
        path = Path(argv[argv.index("--file") + 1])
    P, claims, deps, serial = load(path)
    check_acyclic(deps)
    waves = schedule(P, claims, deps, serial, cap=CAP)
    if "--verify" in argv:
        bad = 0
        for n, want in LANDED.items():
            if waves[n] != want.split():
                bad += 1
                print(f"W{n} MISMATCH\n  landed:    {want}\n  scheduler: {' '.join(waves[n])}")
            else:
                print(f"W{n} ok ({len(waves[n])} packets)")
        print("VERIFIED" if not bad else f"{bad} waves differ")
        return 1 if bad else 0
    if "--wave" in argv:
        print("\n".join(waves[int(argv[argv.index("--wave") + 1])]))
        return 0
    n_len, chain = critical_path(P, deps, serial)
    loc = {i: P[i]["loc"] for i in P}
    if "--json" in argv:
        print(json.dumps({"waves": waves, "critical_path": chain}, indent=1))
        return 0
    print(f"packets {len(P)}  LOC {sum(loc.values())}  wave0 (serial) {len(serial)}: {' '.join(serial)}")
    print(f"WAVES {len(waves)} (W0 serial + {len(waves) - 1} parallel, at most {CAP} packets per wave)")
    for n, w in enumerate(waves):
        tag = "W0 serial" if n == 0 else f"W{n}"
        print(f"{tag:>9} {len(w):>3} pkts {sum(loc[i] for i in w):>6} LOC  {' '.join(w)}")
    print(f"CRITICAL PATH (depends_on, after W0): {n_len} packets: {' -> '.join(chain)}")
    print(f"  lower bound on waves: 1 (W0) + {n_len}; file contention adds {len(waves) - 1 - n_len}")
    if "--priority" in argv:
        pw = schedule(P, claims, deps, serial, priority=True)
        print(f"PRIORITY ORDER (0.5.0 leap_schedule: most-unblocking first): {len(pw)} waves")
        for n, w in enumerate(pw):
            if n:
                print(f"{'W' + str(n):>9} {len(w):>3} pkts  {' '.join(w)}")
    if "--cap" in argv:
        cap = int(argv[argv.index("--cap") + 1])
        cw = schedule(P, claims, deps, serial, cap=cap)
        print(f"WITH A CAP OF {cap} PACKETS PER WAVE: {len(cw)} waves ({' '.join(str(len(w)) for w in cw[1:])})")
    if "--stats" in argv:
        c = defaultdict(list)
        for i in P:
            for f in claims[i]:
                c[f].append(i)
        print("CONTENTION (files with 3+ claimants):")
        for f, ids in sorted(c.items(), key=lambda kv: (-len(kv[1]), kv[0])):
            if len(ids) >= 3:
                print(f"  {len(ids):>2} {f}: {' '.join(sorted(ids, key=id_key))}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
