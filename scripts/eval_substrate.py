#!/usr/bin/env python3
"""glia v0.4.x substrate eval harness.

Walks each clone in /home/ivy/Code/glia-eval/, runs the full pipeline via
repo_graph_py, and tabulates counts by node-kind + edge-category. For
multi-service repos, the pyo3 entrypoint already merges sub-graphs under
one call; we rely on that.

Outputs a Markdown report to stdout.
"""

from __future__ import annotations

import json
import os
import sys
import time
from collections import Counter
from pathlib import Path

import repo_graph_py

# Mirror the constants in code-domain/src/lib.rs.
NODE_KINDS = {
    1: "MODULE",
    2: "CLASS",
    3: "FUNCTION",
    4: "METHOD",
    5: "ROUTE",
    6: "PACKAGE",
    7: "INTERFACE",
    8: "STRUCT",
    9: "ENDPOINT",
    10: "ENUM",
    11: "GRPC_SERVICE",
    12: "GRPC_CLIENT",
    13: "QUEUE_CONSUMER",
    14: "QUEUE_PRODUCER",
    15: "GRAPHQL_RESOLVER",
    16: "GRAPHQL_OPERATION",
    17: "WS_HANDLER",
    18: "WS_CLIENT",
    19: "EVENT_HANDLER",
    20: "EVENT_EMITTER",
    21: "CLI_COMMAND",
    22: "CLI_INVOCATION",
    23: "DATABASE",
    24: "CACHE",
    25: "BLOB_STORE",
    26: "SEARCH_INDEX",
    27: "EMAIL_SERVICE",
    28: "COMPONENT",
    29: "HOOK",
    30: "SERVICE",
    31: "DIRECTIVE",
    32: "PIPE",
    33: "GUARD",
    34: "COMPOSABLE",
    35: "ATTRIBUTE",
    36: "DATA_ENTITY",
    37: "CRON_JOB",
    38: "CONFIG_KEY",
    39: "INFRA_RESOURCE",
    40: "PACKAGE_DEP",
}

EDGE_CATEGORIES = {
    1: "DEFINES",
    2: "CONTAINS",
    3: "IMPORTS",
    4: "CALLS",
    5: "USES",
    6: "DOCUMENTS",
    7: "TESTS",
    8: "INJECTS",
    9: "HANDLED_BY",
    10: "HTTP_CALLS",
    11: "GRPC_CALLS",
    12: "QUEUE_FLOWS",
    13: "GRAPHQL_CALLS",
    14: "WS_CONNECTS",
    15: "EVENT_FLOWS",
    16: "SHARES_SCHEMA",
    17: "CLI_INVOKES",
    18: "ACCESSES_DATA",
    19: "HAS_ATTRIBUTE",
    20: "INHERITS_FROM",
    21: "RETURNS_TYPE",
    22: "SHARES_DATA_ENTITY",
    23: "SCHEDULES",
    24: "SHARES_CRON_SCHEDULE",
    25: "READS_CONFIG",
    26: "DEFINES_CONFIG",
    27: "SHARES_CONFIG",
    28: "INFRA_REFERENCES",
    29: "SHARES_INFRA_REF",
    30: "DEPENDS_ON",
    31: "SHARES_DEPENDENCY",
}

# Cross-graph categories — emitted by resolvers, never by per-file extractors.
CROSS_EDGE_CATS = {
    10, 11, 12, 13, 14, 15, 16, 17, 22, 24, 27, 29, 31,
}

# Categories specifically relevant to the v0.4.x new resolvers.
NEW_RESOLVER_CATS = {
    22: "SHARES_DATA_ENTITY",
    24: "SHARES_CRON_SCHEDULE",
    27: "SHARES_CONFIG",
    29: "SHARES_INFRA_REF",
    31: "SHARES_DEPENDENCY",
}

# v0.4.x new node kinds — substrate the new resolvers depend on.
NEW_NODE_KINDS = {
    36: "DATA_ENTITY",
    37: "CRON_JOB",
    38: "CONFIG_KEY",
    39: "INFRA_RESOURCE",
    40: "PACKAGE_DEP",
}


def eval_repo(path: Path) -> dict:
    """Run the full pipeline on `path` and return summary stats."""
    t0 = time.time()
    try:
        graph = repo_graph_py.generate(str(path))
    except Exception as exc:
        return {"path": str(path), "error": str(exc), "elapsed_ms": 0}
    elapsed_ms = int((time.time() - t0) * 1000)

    nodes = json.loads(graph.nodes_json())
    edges = json.loads(graph.edges_json())

    node_count_by_kind = Counter(n["kind"] for n in nodes)
    edge_count_by_cat = Counter(e["category"] for e in edges)

    return {
        "path": str(path),
        "elapsed_ms": elapsed_ms,
        "node_total": len(nodes),
        "edge_total": len(edges),
        "cross_edge_total": graph.cross_edge_count(),
        "node_count_by_kind": dict(node_count_by_kind),
        "edge_count_by_cat": dict(edge_count_by_cat),
    }


def fmt_kind_table(counts: Counter, name_map: dict[int, str], header: str) -> str:
    """Render a counter as a Markdown table sorted by count desc."""
    rows = sorted(counts.items(), key=lambda kv: -kv[1])
    if not rows:
        return f"_(no {header.lower()})_"
    lines = [f"| {header} | Count |", "|---|---|"]
    for kind_id, count in rows:
        name = name_map.get(kind_id, f"unknown:{kind_id}")
        lines.append(f"| {name} | {count} |")
    return "\n".join(lines)


def report(results: list[dict], merged_summary: dict | None = None) -> str:
    """Render the final Markdown report."""
    out = ["# glia v0.4.x substrate eval", ""]
    out.append(f"**Repos scanned:** {len(results)}  ")
    successful = [r for r in results if "error" not in r]
    out.append(f"**Successful:** {len(successful)}  ")
    out.append(
        f"**Total nodes (per-repo):** {sum(r['node_total'] for r in successful):,}  "
    )
    out.append(
        f"**Total edges (per-repo):** {sum(r['edge_total'] for r in successful):,}  "
    )
    total_elapsed_s = sum(r["elapsed_ms"] for r in successful) / 1000
    out.append(f"**Per-repo elapsed:** {total_elapsed_s:.1f}s")
    if merged_summary is not None:
        out.append(f"**Merged elapsed:** {merged_summary['elapsed_ms']/1000:.1f}s")
        out.append(f"**Merged cross-edges:** {merged_summary['cross_edge_total']:,}")
    out.append("")

    # Aggregate counts (per-repo summed).
    agg_kinds = Counter()
    agg_cats = Counter()
    for r in successful:
        agg_kinds.update({int(k): v for k, v in r["node_count_by_kind"].items()})
        agg_cats.update({int(k): v for k, v in r["edge_count_by_cat"].items()})

    out.append("## Aggregate node-kind counts (per-repo summed)")
    out.append("")
    out.append(fmt_kind_table(agg_kinds, NODE_KINDS, "Node kind"))
    out.append("")

    out.append("## Aggregate edge-category counts (per-repo summed)")
    out.append("")
    out.append(fmt_kind_table(agg_cats, EDGE_CATEGORIES, "Edge category"))
    out.append("")

    # Merged cross-graph stats — ALL resolvers' actual cross-repo emissions.
    if merged_summary is not None:
        merged_cats = Counter(
            {int(k): v for k, v in merged_summary["edge_count_by_cat"].items()}
        )
        out.append("## Cross-graph edges from merged resolver pass")
        out.append("")
        cross_only = Counter(
            {cat_id: c for cat_id, c in merged_cats.items() if cat_id in CROSS_EDGE_CATS}
        )
        out.append(fmt_kind_table(cross_only, EDGE_CATEGORIES, "Resolver edge category"))
        out.append("")

    # New v0.4.x resolver coverage matrix.
    out.append("## v0.4.x new resolver coverage")
    out.append("")
    out.append(
        "| Resolver | Substrate node kind | Nodes emitted | Cross-edges (merged) |"
    )
    out.append("|---|---|---|---|")
    merged_cats = (
        Counter({int(k): v for k, v in merged_summary["edge_count_by_cat"].items()})
        if merged_summary
        else Counter()
    )
    rows = [
        ("DbResolver", 36, 22),
        ("CronResolver", 37, 24),
        ("ConfigResolver", 38, 27),
        ("IacResolver", 39, 29),
        ("PackageResolver", 40, 31),
    ]
    for name, kind_id, cross_cat_id in rows:
        n = agg_kinds.get(kind_id, 0)
        e = merged_cats.get(cross_cat_id, 0)
        out.append(f"| {name} | {NEW_NODE_KINDS[kind_id]} | {n:,} | {e:,} |")
    out.append("")

    # Per-repo summary.
    out.append("## Per-repo")
    out.append("")
    out.append(
        "| Repo | Nodes | Edges | Cross-edges | Elapsed (ms) |"
    )
    out.append("|---|---|---|---|---|")
    for r in sorted(results, key=lambda x: x.get("node_total", 0), reverse=True):
        if "error" in r:
            out.append(
                f"| {Path(r['path']).name} | _error_ | — | — | — |"
            )
            continue
        out.append(
            f"| {Path(r['path']).name} | "
            f"{r['node_total']:,} | "
            f"{r['edge_total']:,} | "
            f"{r['cross_edge_total']:,} | "
            f"{r['elapsed_ms']:,} |"
        )
    out.append("")

    # Errors.
    errors = [r for r in results if "error" in r]
    if errors:
        out.append("## Errors")
        out.append("")
        for r in errors:
            out.append(f"- `{Path(r['path']).name}`: {r['error']}")
        out.append("")

    return "\n".join(out)


def collect_repo_paths(eval_root: Path) -> list[Path]:
    """Enumerate effective per-repo paths under eval_root.

    Multi-service demos are expanded so each microservice subdir becomes its
    own repo path. Without this, intra-repo services share one RepoId and
    cross-graph resolvers (which gate on `refs[i].repo != refs[j].repo`) never
    fire."""
    repos: list[Path] = []
    for entry in sorted(eval_root.iterdir()):
        if not entry.is_dir() or entry.name.startswith("."):
            continue
        if entry.name == "frameworks":
            for sub in sorted(entry.iterdir()):
                if sub.is_dir() and not sub.name.startswith("."):
                    repos.append(sub)
            continue
        # microservices-demo / bank-of-anthos: services live under src/<svc>/.
        # voting-app: services are top-level dirs (vote, worker, result, seed-data).
        src_dir = entry / "src"
        if src_dir.is_dir():
            for svc in sorted(src_dir.iterdir()):
                if svc.is_dir() and not svc.name.startswith("."):
                    repos.append(svc)
        elif entry.name == "voting-app":
            for svc_name in ("vote", "worker", "result", "seed-data"):
                p = entry / svc_name
                if p.is_dir():
                    repos.append(p)
        else:
            repos.append(entry)
    return repos


def main():
    eval_root = Path("/home/ivy/Code/glia-eval")
    if not eval_root.is_dir():
        print(f"missing eval root: {eval_root}", file=sys.stderr)
        sys.exit(1)

    repos = collect_repo_paths(eval_root)
    print(f"# scanning {len(repos)} repos individually", file=sys.stderr)
    results = []
    for repo in repos:
        print(f"  → {repo.name}", file=sys.stderr, flush=True)
        results.append(eval_repo(repo))

    # Now run a single MergedGraph over EVERY repo so cross-graph resolvers
    # actually fire. The per-repo loop above gives the per-service stats; this
    # gives the cross-service edges.
    print(f"# merging {len(repos)} repos for cross-graph eval", file=sys.stderr)
    t0 = time.time()
    try:
        merged = repo_graph_py.generate_many([str(r) for r in repos])
    except Exception as exc:
        print(f"merge failed: {exc}", file=sys.stderr)
        merged = None
    merged_elapsed_ms = int((time.time() - t0) * 1000)

    merged_summary = None
    if merged is not None:
        nodes = json.loads(merged.nodes_json())
        edges = json.loads(merged.edges_json())
        merged_summary = {
            "node_total": len(nodes),
            "edge_total": len(edges),
            "cross_edge_total": merged.cross_edge_count(),
            "node_count_by_kind": dict(Counter(n["kind"] for n in nodes)),
            "edge_count_by_cat": dict(Counter(e["category"] for e in edges)),
            "elapsed_ms": merged_elapsed_ms,
        }

    print(report(results, merged_summary))


if __name__ == "__main__":
    main()
