#!/usr/bin/env python3
"""Per-framework coverage check.

For each cloned framework demo, verify the framework's expected node-kinds
were actually extracted. Flags silent misses (parser/extractor gaps that
the aggregate eval can't see).
"""

from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path

import repo_graph_py

NODE_KINDS = {
    1: "MODULE", 2: "CLASS", 3: "FUNCTION", 4: "METHOD", 5: "ROUTE",
    6: "PACKAGE", 7: "INTERFACE", 8: "STRUCT", 9: "ENDPOINT", 10: "ENUM",
    11: "GRPC_SERVICE", 12: "GRPC_CLIENT", 13: "QUEUE_CONSUMER",
    14: "QUEUE_PRODUCER", 15: "GRAPHQL_RESOLVER", 16: "GRAPHQL_OPERATION",
    17: "WS_HANDLER", 18: "WS_CLIENT", 19: "EVENT_HANDLER", 20: "EVENT_EMITTER",
    21: "CLI_COMMAND", 22: "CLI_INVOCATION", 23: "DATABASE", 24: "CACHE",
    25: "BLOB_STORE", 26: "SEARCH_INDEX", 27: "EMAIL_SERVICE",
    28: "COMPONENT", 29: "HOOK", 30: "SERVICE", 31: "DIRECTIVE",
    32: "PIPE", 33: "GUARD", 34: "COMPOSABLE", 35: "ATTRIBUTE",
    36: "DATA_ENTITY", 37: "CRON_JOB", 38: "CONFIG_KEY",
    39: "INFRA_RESOURCE", 40: "PACKAGE_DEP",
}

# What we EXPECT each framework's demo to surface. The check is "≥1 node of
# this kind." If a kind is in `must_have` but the demo emits zero, that's a
# silent extractor miss.
#
# `nice_to_have` is a softer signal — these often exist but aren't critical.
FRAMEWORK_EXPECTATIONS: dict[str, dict] = {
    # Go
    "chi":               {"must_have": ["ROUTE"], "lang": "Go"},
    # Rust
    "axum":              {"must_have": ["ROUTE"], "lang": "Rust"},
    # Java/Kotlin
    "spring":            {"must_have": ["ROUTE"], "lang": "Java"},
    "javalin":           {"must_have": ["ROUTE"], "lang": "Java/Kotlin"},
    # Python
    "flask":             {"must_have": ["ROUTE"], "lang": "Python"},
    "django":            {"must_have": ["ROUTE"], "lang": "Python"},
    # Ruby
    "rails-sample":      {"must_have": ["ROUTE"], "lang": "Ruby"},
    "sinatra":           {"must_have": ["ROUTE"], "lang": "Ruby"},
    # PHP
    "laravel":           {"must_have": ["ROUTE"], "lang": "PHP"},
    "symfony-demo":      {"must_have": ["ROUTE"], "lang": "PHP"},
    "slim":              {"must_have": ["ROUTE"], "lang": "PHP"},
    # TS/JS backend
    "express":           {"must_have": ["ROUTE"], "lang": "TS/JS"},
    "koa":               {"must_have": ["ROUTE"], "lang": "TS/JS"},
    "hono":              {"must_have": ["ROUTE"], "lang": "TS/JS"},
    "fastify":           {"must_have": ["ROUTE"], "lang": "TS/JS"},
    "nestjs":            {"must_have": ["ROUTE"], "lang": "TS"},
    "nextjs":            {"must_have": ["ROUTE"], "lang": "TS/JS"},
    "sveltekit-realworld": {"must_have": ["ROUTE"], "lang": "TS"},
    "hapi":              {"must_have": ["ROUTE"], "lang": "TS/JS"},
    # Frontend (component-emitting)
    "react-cra":         {"must_have": ["COMPONENT"], "lang": "React",
                          "nice_to_have": ["HOOK"]},
    "angular-realworld": {"must_have": ["COMPONENT"], "lang": "Angular",
                          "nice_to_have": ["SERVICE", "DIRECTIVE", "PIPE"]},
    "vue-create":        {"must_have": ["COMPONENT"], "lang": "Vue",
                          "nice_to_have": ["COMPOSABLE"]},
    # C#
    "aspnetcore-clean":  {"must_have": ["ROUTE"], "lang": "C#"},
}


def eval_one(path: Path) -> dict:
    """Generate graph for one path, return per-kind counts."""
    try:
        g = repo_graph_py.generate(str(path))
    except Exception as exc:
        return {"error": str(exc)}
    nodes = json.loads(g.nodes_json())
    by_kind = Counter(n["kind"] for n in nodes)
    return {
        "node_total": len(nodes),
        "by_kind": {NODE_KINDS.get(k, f"unk:{k}"): c for k, c in by_kind.items()},
    }


def main():
    root = Path("/home/ivy/Code/glia-eval/frameworks")
    if not root.is_dir():
        print(f"missing: {root}", file=sys.stderr)
        sys.exit(1)

    rows = []
    for name, expect in FRAMEWORK_EXPECTATIONS.items():
        path = root / name
        if not path.is_dir():
            rows.append({"name": name, "status": "MISSING_CLONE",
                         "must_have": expect["must_have"]})
            continue
        result = eval_one(path)
        if "error" in result:
            rows.append({"name": name, "status": "PARSE_ERROR",
                         "error": result["error"]})
            continue
        misses = [k for k in expect["must_have"] if result["by_kind"].get(k, 0) == 0]
        nice_misses = [k for k in expect.get("nice_to_have", [])
                       if result["by_kind"].get(k, 0) == 0]
        rows.append({
            "name": name,
            "lang": expect["lang"],
            "node_total": result["node_total"],
            "must_have": expect["must_have"],
            "nice_to_have": expect.get("nice_to_have", []),
            "by_kind": result["by_kind"],
            "misses": misses,
            "nice_misses": nice_misses,
            "status": "MISS" if misses else ("SOFT_MISS" if nice_misses else "PASS"),
        })

    # Render table.
    print("# Framework coverage check")
    print()
    print("| Framework | Lang | Status | Nodes | Must-have | Hits |")
    print("|---|---|---|---|---|---|")
    for r in rows:
        if r["status"] in ("MISSING_CLONE", "PARSE_ERROR"):
            print(f"| {r['name']} | — | **{r['status']}** | — | — | — |")
            continue
        must_hits = ", ".join(
            f"{k}={r['by_kind'].get(k, 0)}" for k in r["must_have"]
        )
        status_emoji = {
            "PASS": "✓",
            "SOFT_MISS": "soft-miss",
            "MISS": "**MISS**",
        }[r["status"]]
        print(
            f"| {r['name']} | {r['lang']} | {status_emoji} | "
            f"{r['node_total']:,} | {', '.join(r['must_have'])} | {must_hits} |"
        )
    print()

    # Detail section for misses.
    misses = [r for r in rows if r.get("status") == "MISS"]
    if misses:
        print("## Hard misses (extractor gaps)")
        print()
        for r in misses:
            print(f"### {r['name']} ({r['lang']})")
            print(f"- Missing: {', '.join(r['misses'])}")
            print(f"- Got: {dict(sorted(r['by_kind'].items(), key=lambda kv: -kv[1])[:8])}")
            print()

    soft = [r for r in rows if r.get("status") == "SOFT_MISS"]
    if soft:
        print("## Soft misses (nice-to-have gaps)")
        print()
        for r in soft:
            nice_hits = ", ".join(
                f"{k}={r['by_kind'].get(k, 0)}" for k in r.get("nice_to_have", [])
            )
            print(f"- **{r['name']}** ({r['lang']}): {nice_hits}")
        print()

    # Summary.
    n_pass = sum(1 for r in rows if r.get("status") == "PASS")
    n_soft = sum(1 for r in rows if r.get("status") == "SOFT_MISS")
    n_miss = sum(1 for r in rows if r.get("status") == "MISS")
    n_skip = sum(1 for r in rows if r.get("status") in ("MISSING_CLONE", "PARSE_ERROR"))
    print(f"## Summary: {n_pass} pass, {n_soft} soft-miss, **{n_miss} miss**, {n_skip} skipped")


if __name__ == "__main__":
    main()
