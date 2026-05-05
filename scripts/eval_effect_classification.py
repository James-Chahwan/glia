#!/usr/bin/env python3
"""Effect classification trial for task #19.

Walks each repo in /home/ivy/Code/glia-eval/, classifies every FUNCTION /
METHOD / MODULE node with a coarse effect class via substring pattern
matching against the source file, and tabulates the distribution. The
question this answers: does the heuristic produce a useful signal, or is
it noisy enough that effect tags would clutter `glia analyze` without
helping reasoning?

Effect classes (mutually exclusive — first match wins, in order):
  network    — fetch/axios/requests/http.Get/...
  fs         — open()/fs.readFile/std::fs/io.ReadFile/...
  db         — db.execute/cursor.execute/.query/prisma./mongoose./...
  queue      — .lpush/.publish/producer.send/...
  time       — time.now/Date.now/datetime.now/...
  random     — random./Math.random/rand::/...
  pure       — none of the above
"""

from __future__ import annotations

import json
from collections import Counter
from pathlib import Path

import repo_graph_py

# Effect-pattern table. Order matters — first match wins per source line.
EFFECT_PATTERNS: list[tuple[str, list[str]]] = [
    ("network", [
        "fetch(", "axios.", "axios(", ".request(", "got(", "got.",
        "requests.get", "requests.post", "requests.put", "requests.delete",
        "http.Get", "http.Post", "http.Do", "httpClient.",
        "urllib", "fetch_url", ".send(",
        "WebSocket", "ws://", "wss://",
    ]),
    ("fs", [
        "open(", "fs.readFile", "fs.writeFile", "fs.createReadStream",
        "std::fs", "io.ReadFile", "io.WriteFile", "ioutil.ReadFile",
        "Path(", "pathlib.", "os.path", "shutil.",
        "File.open", "File.read", "File.write", "FileInputStream",
        "FileOutputStream",
    ]),
    ("db", [
        "db.execute", "cursor.execute", ".query(", ".findOne(", ".findMany(",
        "prisma.", "mongoose.", "Sequelize", "TypeORM",
        "sqlalchemy", "session.query", "session.commit",
        "ActiveRecord", ".find_by", ".save",
        "diesel::", "sqlx::", "gorm.",
    ]),
    ("queue", [
        ".lpush(", ".rpush(", ".blpop(", ".brpop(",
        ".publish(", ".subscribe(",
        "producer.send", "producer.produce",
        "perform_async", "perform_in",
        "@celery.task", "@shared_task", ".apply_async(",
        "channel.publish", "channel.consume",
    ]),
    ("time", [
        "time.now", "time.time(", "time.sleep",
        "Date.now", "new Date(", "datetime.now", "datetime.utcnow",
        "chrono::", "Instant::", "SystemTime::",
        "Time.now", "Time.current",
    ]),
    ("random", [
        "random.", "Math.random", "secrets.",
        "rand::", "thread_rng", "rand(", "rand_int", "uuid",
        "crypto.randomBytes",
    ]),
]


def classify_source(src: str) -> Counter:
    """Count effect-class hits across the source string. Each line contributes
    at most one effect (first matching pattern wins)."""
    hits: Counter = Counter()
    for line in src.splitlines():
        matched = None
        for cls, patterns in EFFECT_PATTERNS:
            if any(p in line for p in patterns):
                matched = cls
                break
        if matched is not None:
            hits[matched] += 1
    return hits


def eval_repo(path: Path) -> dict:
    try:
        g = repo_graph_py.generate(str(path))
    except Exception as exc:
        return {"path": str(path), "error": str(exc)}

    # Walk source files alongside the graph; for each function-bearing
    # source file, accumulate effect counts.
    by_class: Counter = Counter()
    files_classified = 0
    for f in path.rglob("*"):
        if not f.is_file() or f.name.startswith("."):
            continue
        ext = f.suffix.lower().lstrip(".")
        # Limit to languages that actually have effect-shaped operations.
        if ext not in {"py", "js", "ts", "tsx", "jsx", "go", "rs", "rb",
                       "java", "kt", "cs", "php"}:
            continue
        try:
            src = f.read_text(errors="ignore")
        except Exception:
            continue
        classified = classify_source(src)
        if classified:
            files_classified += 1
            by_class.update(classified)
    return {
        "path": str(path),
        "node_total": g.node_count(),
        "files_classified": files_classified,
        "by_class": dict(by_class),
    }


def main():
    eval_root = Path("/home/ivy/Code/glia-eval")
    repos = []
    for entry in sorted(eval_root.iterdir()):
        if not entry.is_dir() or entry.name.startswith("."):
            continue
        if entry.name == "frameworks":
            for sub in sorted(entry.iterdir()):
                if sub.is_dir():
                    repos.append(sub)
        else:
            repos.append(entry)

    rows = []
    agg: Counter = Counter()
    for repo in repos:
        r = eval_repo(repo)
        if "error" in r:
            continue
        rows.append(r)
        for cls, n in r["by_class"].items():
            agg[cls] += n

    # Aggregate distribution.
    total = sum(agg.values()) or 1
    print("# Effect classification trial — heuristic distribution")
    print()
    print(f"**Repos scanned:** {len(rows)}  ")
    print(f"**Total effect-tagged lines:** {total:,}")
    print()
    print("## Aggregate effect distribution")
    print()
    print("| Class | Lines | % |")
    print("|---|---|---|")
    for cls in ("network", "fs", "db", "queue", "time", "random"):
        n = agg.get(cls, 0)
        pct = (n / total) * 100
        print(f"| {cls} | {n:,} | {pct:.1f}% |")
    print()

    # Per-repo top hits.
    print("## Per-repo effect tags (top 10 by total)")
    print()
    print("| Repo | Tagged | network | fs | db | queue | time | random |")
    print("|---|---|---|---|---|---|---|---|")
    rows_sorted = sorted(rows, key=lambda r: -sum(r["by_class"].values()))
    for r in rows_sorted[:15]:
        c = r["by_class"]
        total_r = sum(c.values())
        print(
            f"| {Path(r['path']).name} | {total_r} | "
            f"{c.get('network',0)} | {c.get('fs',0)} | {c.get('db',0)} | "
            f"{c.get('queue',0)} | {c.get('time',0)} | {c.get('random',0)} |"
        )


if __name__ == "__main__":
    main()
