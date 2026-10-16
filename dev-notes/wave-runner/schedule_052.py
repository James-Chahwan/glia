#!/usr/bin/env python3
"""Wave scheduler for the glia 0.5.2 packet set (dev-notes/leap-052-packets.json).

Rule (the 0.5.0 leap's, dev-notes/wave-runner/leap_schedule.py): two packets share
a wave only if their CLAIMED files (files_touched) are disjoint, because the
implementing agents edit one working tree concurrently. Wave 0 is the D0 packets,
run one at a time in `wave0_serial` order; every other packet implicitly depends on
all of them. After that, a packet is ready when every depends_on is done, and each
wave is the maximal set of ready packets, taken in id order, with pairwise-disjoint
files_touched.

The runner schedules with at most CAP packets per wave (0.5.0's largest wave was 26):
a smaller wave keeps the end-of-wave gate readable (0.5.1's schedule_052.py set the cap).
gen_wave.py / closeout.py `--release 052` read `waves()`.

Usage:
  python3 schedule_052.py                  # waves, sizes, critical path
  python3 schedule_052.py --verify         # assert LANDED waves still match
  python3 schedule_052.py --wave N         # wave N's packets, one per line (W0 = the serial D0 packets)
  python3 schedule_052.py --priority       # also run 0.5.0's order (most-unblocking first)
  python3 schedule_052.py --json           # machine-readable waves
  python3 schedule_052.py --stats          # contention table (files with 3+ claimants)
  python3 schedule_052.py --cap 26         # also schedule with at most N packets per wave
  python3 schedule_052.py --file other.json
"""
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKETS = HERE.parents[1] / "dev-notes" / "leap-052-packets.json"
CAP = 26

# Waves that have landed, by number; closeout.py --release 052 appends each one, and
# --verify then guards that re-running the scheduler reproduces history.
LANDED = {
    0: "D0.1 D0.2 D0.3 D0.4 D0.5 D0.6 D0.7 D0.8",
    1: "DA.1 DA.2 DA.3 DA.5a DB.1 DC.1 DC.2 DC.6 DD.1 DD.2 DE.1 DE.6 DE.10 DE.11 DF.1 DF.2 DF.3 DF.5 DF.7 DF.8 DF.12 DF.14 DF.17 DF.18 DF.20 DG.1",
    2: "DA.4 DA.5b DA.8 DC.3 DD.3 DD.4 DE.2 DE.7 DF.6 DF.9 DF.16 DF.22a DG.3 DG.4 DG.5 DG.6 DG.7a DG.8 DG.9 DG.10 DH.1 DH.13 DH.14 DH.15 DH.19 DH.20",
    3: "DA.6 DA.9 DA.10 DA.12 DB.7a DC.4 DD.5 DE.3 DE.9 DF.10 DF.13 DF.15 DF.22b DG.11 DH.2 DH.5 DH.6 DH.8 DH.9 DH.10 DH.11 DH.21",
    4: "DA.7 DA.11 DA.13 DC.5 DD.6 DD.8 DE.4 DE.8 DG.2 DG.7b DG.13a",
    5: "DA.14a DD.7 DD.9 DE.5 DF.4a DG.12 DG.13b DH.3 DH.7 DH.12",
    6: "DA.14b DC.7a DD.10 DF.4b DF.11 DH.4 DH.17a",
    7: "DA.15 DA.16 DA.17 DB.2a DB.7b DC.7b DD.11 DF.19 DF.21 DH.16 DH.17b",
    8: "DA.18 DA.19 DA.20 DB.2b DC.7c DD.12 DH.18",
    9: "DB.2c DC.8 DC.9 DC.10a DD.16",
    10: "DB.3a DB.4 DB.7c DC.10b DD.13",
    11: "DB.3b DB.5 DB.6 DB.9a DF.22c",
    12: "DB.3c DB.3d DB.8a DB.9b DD.14",
    13: "DB.8b DB.9c DD.15",
    14: "DZ.1 DZ.2",
}
GROUP_ORDER = {"0": 0, "A": 1, "B": 2, "C": 3, "D": 4, "E": 5, "F": 6, "G": 7, "H": 8, "Z": 9}


def id_key(pid):
    m = re.fullmatch(r"D([0-9A-Z])\.(\d+)([a-z]?)", pid)
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
    """Longest depends_on chain over the non-D0 packets (D0 is the serial wave 0)."""
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
