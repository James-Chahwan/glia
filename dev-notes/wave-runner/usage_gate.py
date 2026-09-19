#!/usr/bin/env python3
"""Should the next wave start now, given the 5-hour usage limit?

Claude Code passes the account's rate limits to the status line on every
refresh; ~/.claude/statusline-command.sh persists them to
~/.claude/rate-limits.json. This reads that file.

    python3 usage_gate.py                    # GO / HOLD / UNKNOWN for the next wave
    python3 usage_gate.py --start W7         # ...and record the 5h % at launch
    python3 usage_gate.py --end W7           # record the 5h % after close-out
    python3 usage_gate.py --max-pct 70 --near-reset-min 45

Rule (James, 2026-09-19: don't start a wave at ~80% with more than about an hour
left before the reset): HOLD when the 5-hour usage is at or above --max-pct, or
when usage plus the measured cost of an average wave would pass 100% — unless
the window resets within --near-reset-min minutes, in which case a limit hit
only pauses the wave briefly. The measured cost comes from usage-log.jsonl
(--start / --end pairs); until a wave has been measured, only --max-pct applies.

Exit codes: 0 GO, 2 HOLD, 3 UNKNOWN (no data yet — open a Claude Code session
so the status line renders once). The numbers are as fresh as the last status
line render; other sessions on the account can have used more since.
"""
import argparse
import json
import sys
import time
from pathlib import Path

LIMITS = Path.home() / ".claude" / "rate-limits.json"
LOG = Path(__file__).resolve().parent / "usage-log.jsonl"


def load_limits():
    try:
        return json.loads(LIMITS.read_text())
    except (OSError, ValueError):
        return None


def wave_cost():
    """Average 5h-% a wave consumed, from --start / --end pairs in one window."""
    try:
        rows = [json.loads(l) for l in LOG.read_text().splitlines() if l.strip()]
    except OSError:
        return None, 0
    starts = {r["wave"]: r for r in rows if r.get("event") == "start"}
    costs = []
    for r in rows:
        s = starts.get(r["wave"]) if r.get("event") == "end" else None
        # Only pairs inside one 5h window are comparable (the counter resets):
        # the end reading must come before the window the start saw reset.
        if s and s.get("resets_at") and r["ts"] < int(s["resets_at"]) and r["pct"] >= s["pct"]:
            costs.append(r["pct"] - s["pct"])
    return (sum(costs) / len(costs), len(costs)) if costs else (None, 0)


def fmt_mins(m):
    return f"{m // 60}h{m % 60:02d}m" if m >= 60 else f"{m}m"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--max-pct", type=float, default=80.0)
    ap.add_argument("--near-reset-min", type=int, default=60)
    ap.add_argument("--start", metavar="WAVE")
    ap.add_argument("--end", metavar="WAVE")
    a = ap.parse_args()

    lim = load_limits()
    five = (lim or {}).get("five_hour") or {}
    pct, resets = five.get("used_percentage"), five.get("resets_at")
    if pct is None:
        print(f"UNKNOWN: no 5-hour usage in {LIMITS} yet (open a Claude Code session so the status line renders)")
        sys.exit(3)
    now = int(time.time())
    age = now - int(lim.get("updated", now))
    if resets and int(resets) <= now:
        # The window this reading belongs to has already reset: a fresh window.
        print(f"(last reading {pct:.0f}% was for a window that reset {fmt_mins((now - int(resets)) // 60)} ago; treating usage as 0%)")
        pct, resets = 0, None
    mins_left = max(0, (int(resets) - now) // 60) if resets else None
    week = (lim.get("seven_day") or {}).get("used_percentage")

    if a.start or a.end:
        with LOG.open("a") as f:
            f.write(json.dumps({"wave": a.start or a.end, "event": "start" if a.start else "end",
                                "pct": pct, "resets_at": resets, "ts": now}) + "\n")

    cost, n = wave_cost()
    reset_txt = (f"resets in {fmt_mins(mins_left)} (at {time.strftime('%H:%M', time.localtime(int(resets)))})"
                 if mins_left is not None else "reset time unknown")
    stale = f"; data is {fmt_mins(age // 60)} old" if age >= 600 else ""
    cost_txt = f"; an average wave costs {cost:.0f}% ({n} measured)" if cost is not None else "; no wave measured yet"
    week_txt = f"; 7-day {week:.0f}%" if week is not None else ""
    print(f"5-hour usage {pct:.0f}%, {reset_txt}{cost_txt}{week_txt}{stale}")

    near_reset = mins_left is not None and mins_left <= a.near_reset_min
    too_high = pct >= a.max_pct
    wont_fit = cost is not None and pct + cost >= 100
    if (too_high or wont_fit) and not near_reset:
        why = f"at or above {a.max_pct:.0f}%" if too_high else f"{pct:.0f}% + ~{cost:.0f}% for a wave would pass 100%"
        wait = f"; start after the reset in {fmt_mins(mins_left)}" if mins_left is not None else ""
        print(f"HOLD: {why}{wait}")
        sys.exit(2)
    if week is not None and week >= 90:
        print("GO (warning: 7-day usage is at or above 90%)")
    else:
        print("GO" + (" (the window resets soon, so a limit hit only pauses the wave briefly)" if (too_high or wont_fit) else ""))
    sys.exit(0)


if __name__ == "__main__":
    main()
