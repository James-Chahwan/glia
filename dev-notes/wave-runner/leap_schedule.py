#!/usr/bin/env python3
"""Deterministic wave scheduler for the 0.5.0 leap (dev-notes/next-leap-0.5.0.md).

Same rule as the 09-16 programme's schedule.py: two packets share a wave only if
their CLAIMED files are disjoint, because implementing agents edit one working
tree concurrently. Inputs, all committed:

  dev-notes/leap-packets.json   leap packets (L0 + LA..LG), the Batch C
                                re-verification rows, the dependency patch and
                                the wave-0 split remaps
  dev-notes/wave-packets.json   the Batch C specs the re-verification refers to

Batch C packets are claimed by their re-verified `files_touched_now`, never by
the stale 09-16 `files_touched`. A packet whose file is split by a wave-0 packet
claims the new modules named in that split's remap and depends on the split.

Usage:
  python3 leap_schedule.py            # print every wave
  python3 leap_schedule.py --wave N   # wave N's packets, one per line (W0 = the serial splits)
  python3 leap_schedule.py --json     # waves + claims + deps as JSON
  python3 leap_schedule.py --stats    # contention and critical-path summary
  python3 leap_schedule.py --verify   # assert LANDED waves still match
"""
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEAP = ROOT / "dev-notes" / "leap-packets.json"
PROGRAMME = ROOT / "dev-notes" / "wave-packets.json"

# Waves that have landed, by number. Empty until the leap starts; --verify
# then guards that re-running the scheduler reproduces history.
LANDED = {
    0: "L0.1 L0.3 L0.2 L0.6 L0.4 L0.5",
}

ITEM_RE = re.compile(r"^(L[A-G]\.\d+)$")


def load():
    leap = json.loads(LEAP.read_text())
    prog = {p["id"]: p for p in json.loads(PROGRAMME.read_text())["packets"]}
    P, claims, deps = {}, {}, {}
    for p in leap["packets"]:
        P[p["id"]] = p
        claims[p["id"]] = {f.strip() for f in p["files_touched"]}
        deps[p["id"]] = {d.strip() for d in p.get("depends_on") or []}
    for row in leap["batch_c"]:
        pid = row["packet"]
        if row["verdict"] == "not-needed":
            continue
        spec = dict(prog[pid])
        spec["loc"] = row.get("loc_now") or spec["loc"]
        P[pid] = spec
        claims[pid] = {f.strip() for f in row["files_touched_now"]}
        deps[pid] = {d.strip() for d in (spec.get("depends_on") or []) + (row.get("depends_on_add") or [])}
    patch = leap.get("deps_patch", {})
    for pid, add in patch.get("add_depends_on", {}).items():
        if pid in deps:
            deps[pid] |= set(add)
    for pid, rm in patch.get("remove_depends_on", {}).items():
        if pid in deps:
            deps[pid] -= set(rm)
    # Wave-0 splits: {split_packet_id: [{"file": "cli/src/main.rs", "remap": {pid: [new files]}}, ...]}.
    # A remapped claimant claims the new modules instead of the split file (an
    # empty list means it no longer touches the file at all); every claimant,
    # remapped or not, waits for the split.
    for split_id, entries in leap.get("splits", {}).items():
        for sp in entries if isinstance(entries, list) else [entries]:
            for pid, files in sp["remap"].items():
                if pid in claims and pid != split_id and sp["file"] in claims[pid]:
                    claims[pid].discard(sp["file"])
                    claims[pid] |= set(files)
                    deps[pid].add(split_id)
            for pid in claims:
                if pid != split_id and sp["file"] in claims[pid]:
                    deps[pid].add(split_id)
    serial = leap.get("wave0_serial", [])
    for pid in claims:
        if pid not in serial:
            deps[pid] |= set(serial)
    return P, claims, expand(P, deps), set(leap.get("exclusive", [])), serial


def expand(P, deps):
    """Item ids (LB.1) mean every packet of that item (LB.1, LB.1a, LB.1b ...).
    Ids that are not packets of this leap (programme packets that landed) are
    satisfied and dropped."""
    by_item = defaultdict(set)
    for pid in P:
        m = re.match(r"^(L[A-G0]\.\d+)", pid)
        if m:
            by_item[m.group(1)].add(pid)
    out = {}
    for pid, ds in deps.items():
        resolved = set()
        for d in ds:
            if d in P:
                resolved.add(d)
            elif ITEM_RE.match(d) and by_item.get(d):
                resolved |= by_item[d]
        resolved.discard(pid)
        out[pid] = resolved
    return out


def schedule(P, C, D, X=frozenset(), serial=()):
    """X: packets that must run alone in their wave (a change every sibling
    would compile against half-done, e.g. LC.2's core::Edge). serial: wave 0,
    run one packet at a time in the given order before everything else; it is
    returned as waves[0] and printed as W0."""
    unblocks = Counter(d for ds in D.values() for d in ds)
    waves, done, left = [list(serial)], set(serial), set(P) - set(serial)
    while left:
        ready = sorted((i for i in left if D[i] <= done),
                       key=lambda i: (-unblocks[i], -len(C[i]), i))
        if not ready:
            raise SystemExit(f"deadlock (dependency cycle or missing packet): {sorted(left)[:12]}")
        used, wave = set(), []
        for i in ready:
            if C[i] & used:
                continue
            if i in X and wave:
                continue
            if wave and wave[0] in X:
                break
            wave.append(i)
            used |= C[i]
            if i in X:
                break
        waves.append(wave)
        done |= set(wave)
        left -= set(wave)
    return waves


def critical_path(P, D):
    memo = {}

    def depth(i):
        if i not in memo:
            memo[i] = 1 + max((depth(d) for d in D[i]), default=0)
        return memo[i]
    return max((depth(i) for i in P), default=0)


def main():
    P, C, D, X, serial = load()
    waves = schedule(P, C, D, X, serial)
    args = sys.argv[1:]
    if "--verify" in args:
        bad = 0
        for n, want in LANDED.items():
            got = waves[n]
            if got != want.split():
                bad += 1
                print(f"W{n} MISMATCH\n  landed:    {want}\n  scheduler: {' '.join(got)}")
            else:
                print(f"W{n} ok ({len(got)} packets)")
        print("VERIFIED" if not bad else f"{bad} waves differ")
        sys.exit(1 if bad else 0)
    if "--wave" in args:
        for pid in waves[int(args[args.index("--wave") + 1])]:
            print(pid)
        return
    if "--json" in args:
        print(json.dumps({"waves": waves, "claims": {k: sorted(v) for k, v in C.items()},
                          "deps": {k: sorted(v) for k, v in D.items()}}))
        return
    if "--stats" in args:
        hot = Counter(f for c in C.values() for f in c)
        print(f"packets {len(P)}  LOC {sum(P[i].get('loc', 0) for i in P)}  waves W0..W{len(waves) - 1} "
              f"(wave 0 serial: {len(waves[0])} packets)  "
              f"dependency depth {critical_path(P, D)}")
        print("files claimed by more than 4 packets (each forces that many waves):")
        for f, n in hot.most_common():
            if n <= 4:
                break
            print(f"  {n:>3}  {f}")
        return
    for n, w in enumerate(waves):
        loc = sum(P[i].get("loc", 0) for i in w)
        tag = (" (landed)" if n in LANDED else "") + (" SERIAL" if n == 0 else "")
        print(f"W{n:<2} ({len(w):>2}, ~{loc:>5} LOC){tag}: {' '.join(w)}")


if __name__ == "__main__":
    main()
