#!/usr/bin/env python3
"""watchdog.py - is anything we started stuck, crawling or filling the disk?

James 2026-10-26: "we gotta stop letting this happen ... long running dead tasks or tasks that are
running so slow we might wanna intervene". A rebuild sat 57 minutes with every linker blocked on a
full /home and nobody looked. Run this whenever a background job passes ~2x its expected wall time,
and before a wave launch or a long run.

    python3 dev-notes/wave-runner/watchdog.py            # one report
    python3 dev-notes/wave-runner/watchdog.py --sample 10   # CPU measured over 10 s (default 5)

Reports free space on /home, the load, and every build / bench process (cargo, rustc, linkers, glia,
rank_eval, the cohort / closeout / bench scripts) with its age, its CPU over the sample and its state.
Flags, one line each:
  STUCK   older than 5 min and under 1% CPU over the sample (blocked: disk, lock, network, deadlock)
  IOWAIT  in uninterruptible sleep (D): usually the disk
  DISK    /home under 60 GB free (a wave HOLDs there), ALARM under 20 GB
Exit 0 when nothing is flagged, 1 otherwise.
"""
import argparse
import os
import shutil
import time
from pathlib import Path

WATCH = ("cargo", "rustc", "ld.mold", "ld", "collect2", "cc", "glia", "glia-head", "glia-seed",
         "rank_eval", "maturin")
WATCH_ARGS = ("cohort.py", "cohort-gate.sh", "rank-gate.sh", "closeout.py", "grade.py", "run.py",
              "sibyl/run.py", "agent-ab")
TICK = os.sysconf("SC_CLK_TCK")


def procs():
    out = {}
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            stat = (d / "stat").read_text()
            cmd = (d / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace").strip()
        except OSError:
            continue
        comm = stat[stat.index("(") + 1:stat.rindex(")")]
        f = stat[stat.rindex(")") + 2:].split()
        if comm not in WATCH and not any(w in cmd for w in WATCH_ARGS):
            continue
        out[int(d.name)] = {"comm": comm, "cmd": cmd, "state": f[0],
                            "cpu": int(f[11]) + int(f[12]), "start": int(f[19])}
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=float, default=5.0)
    a = ap.parse_args()
    uptime = float(Path("/proc/uptime").read_text().split()[0])
    a0 = procs()
    time.sleep(a.sample)
    a1 = procs()
    flags = []
    free = shutil.disk_usage(Path.home()).free / 2**30
    load = os.getloadavg()[0]
    print(f"[watchdog] disk {free:.0f} GB free on /home | load {load:.1f} on {os.cpu_count()} cpus | "
          f"{len(a1)} watched processes")
    if free < 20:
        flags.append(f"ALARM disk: {free:.1f} GB free on /home - stop builds, clean target/")
    elif free < 60:
        flags.append(f"DISK {free:.0f} GB free on /home (a wave HOLDs under 60)")
    for pid, p in sorted(a1.items(), key=lambda kv: kv[1]["start"]):
        age = uptime - p["start"] / TICK
        cpu = (p["cpu"] - a0[pid]["cpu"]) / TICK / a.sample * 100 if pid in a0 else float("nan")
        line = f"pid={pid} {p['comm']} age={age / 60:.0f}m cpu={cpu:.0f}% state={p['state']} {p['cmd'][:110]}"
        print("  " + line)
        if p["state"] == "D":
            flags.append("IOWAIT " + line)
        elif age > 300 and cpu < 1 and p["comm"] in ("rustc", "ld.mold", "ld", "glia", "rank_eval"):
            flags.append("STUCK " + line)
    for f in flags:
        print("[watchdog] " + f)
    print(f"[watchdog] {'ok' if not flags else f'{len(flags)} flagged'}")
    raise SystemExit(1 if flags else 0)


if __name__ == "__main__":
    main()
