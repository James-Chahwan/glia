#!/usr/bin/env python3
"""pyo3 surface, py/src/targets.rs (HT.7): the build-target domain.

`targets_sync(repo, bazel="bazel", workspace="", expr="//...:*",
from_file=None)` writes one bazel workspace's targets to
`<repo>/.glia/targets-snapshot/` (here from saved query outputs, so no bazel
runs) and returns a native dict {workspace, targets, rules, source_files,
external_labels, skipped, bazel_version}; it raises ValueError when the sync
fails. `PyGraph.targets_owners(files)` -> [{file, owners}],
`targets_deps` / `targets_rdeps(label, depth=3, workspace=None)` -> {results,
absence} and `targets_affected(files=None, base="HEAD")` -> {changed, owners,
targets, tests, unowned, caveats} answer from that snapshot for the graph's
first repo root, every row tier "fact"; ValueError with no snapshot. Each
call prints `[targets] surface=py cmd=..` once.

The repo is bazelbuild/examples' cpp-tutorial/stage3 and go-tutorial/stage3
(engine/tests/data/targets/, Apache-2.0, see its README) copied into a temp
dir as workspaces `cpp` and `go`, synced from the raw query outputs in
snapshots/tests/data/bazel/ (`/WORKSPACE` rewritten to the copy's workspace
dir). Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

TOP = pathlib.Path(__file__).resolve().parents[3]
SUMMARY_KEYS = ["workspace", "targets", "rules", "source_files", "external_labels", "skipped",
                "bazel_version"]
AFFECTED_KEYS = ["changed", "owners", "targets", "tests", "unowned", "caveats"]


def surface_lines(err: str) -> list[str]:
    return [l for l in err.splitlines() if l.startswith("[targets] surface=py ")]


def main() -> int:
    c = Checks("targets")
    c.check("targets_sync signature",
            params(rg.targets_sync) == [("repo", None), ("bazel", "bazel"), ("workspace", ""),
                                        ("expr", "//...:*"), ("from_file", None)],
            params(rg.targets_sync))
    c.check("targets_owners signature", params(rg.PyGraph.targets_owners) == [("files", None)],
            params(rg.PyGraph.targets_owners))
    for name in ("targets_deps", "targets_rdeps"):
        fn = getattr(rg.PyGraph, name)
        c.check(f"{name} signature",
                params(fn) == [("label", None), ("depth", 3), ("workspace", None)], params(fn))
    c.check("targets_affected signature",
            params(rg.PyGraph.targets_affected) == [("files", None), ("base", "HEAD")],
            params(rg.PyGraph.targets_affected))

    with tempfile.TemporaryDirectory(prefix="glia-surface-targets-") as tmp:
        repo = pathlib.Path(tmp) / "repo"
        for ws in ("cpp", "go"):
            shutil.copytree(TOP / "engine/tests/data/targets" / ws, repo / ws)

        g = rg.generate(str(repo))
        c.raises("no snapshot -> ValueError naming targets_sync", ValueError,
                 lambda: g.targets_owners(["cpp/lib/hello-time.h"]), "targets_sync")
        c.raises("a failed sync -> ValueError", ValueError,
                 lambda: rg.targets_sync(str(repo)), "no bazel workspace")

        for ws in ("cpp", "go"):
            raw = (TOP / f"snapshots/tests/data/bazel/{ws}-stage3.jsonl").read_text()
            out = pathlib.Path(tmp) / f"{ws}.jsonl"
            out.write_text(raw.replace("/WORKSPACE", str((repo / ws).resolve())))
            s, err = stderr_of(lambda: rg.targets_sync(str(repo), workspace=ws, from_file=str(out)))
            c.check(f"sync {ws} -> summary dict", type(s) is dict and list(s) == SUMMARY_KEYS, s)
            c.check(f"sync {ws} from file", s.get("workspace") == ws
                    and s.get("bazel_version") == "from-file", s)
            c.check(f"sync {ws} surface line",
                    surface_lines(err) == [f"[targets] surface=py cmd=sync workspace={ws} "
                                           f"rows={s.get('targets')}"], err)

        owners, err = stderr_of(lambda: g.targets_owners(["cpp/lib/hello-time.h", "nope.cc"]))
        c.check("owners -> [{file, owners}]", [o["file"] for o in owners]
                == ["cpp/lib/hello-time.h", "nope.cc"], owners)
        first = owners[0]["owners"][0] if owners and owners[0]["owners"] else {}
        c.check("owner row", (first.get("label"), first.get("via"), first.get("file"),
                              first.get("line"), first.get("tier"))
                == ("//lib:hello-time", "hdrs", "cpp/lib/BUILD", 3, "fact"), first)
        c.check("no owner for an unknown file", owners[1]["owners"] == [], owners)
        c.check("owners surface line", surface_lines(err)
                == ["[targets] surface=py cmd=owners files=2 rows=1"], err)

        d, err = stderr_of(lambda: g.targets_deps("//main:hello-world", depth=1))
        c.check("deps -> {results, absence}", d.get("absence") is None
                and [r["label"] for r in d.get("results", [])]
                == ["//lib:hello-time", "//main:hello-greet", "//main:hello-world.cc",
                    "@rules_cc//:link_extra_lib"], d)
        c.check("deps marker", any(l == "[targets] answer kind=deps rows=4 tests=0"
                                   for l in err.splitlines()), err)
        r = g.targets_rdeps("//fortune", depth=1)
        c.check("rdeps", [x["label"] for x in r.get("results", [])]
                == ["//:print_fortune", "//fortune:fortune_test"], r)
        n = g.targets_rdeps("//fortune:fortune", workspace="cpp")
        c.check("another workspace's label is an absence",
                n.get("results") == [] and (n.get("absence") or {}).get("reason") == "unknown_symbol", n)

        a = g.targets_affected(["go/fortune/fortune.go"])
        c.check("affected -> dict in engine field order", list(a) == AFFECTED_KEYS, a)
        c.check("affected reaches the test", [t["label"] for t in a["tests"]]
                == ["//fortune:fortune_test"] and len(a["targets"]) == 3, a)
        b = g.targets_affected(["go/fortune/BUILD"])
        c.check("a BUILD change carries the caveat", b["caveats"]
                == ["BUILD files changed; re-run glia targets sync for exact owners"], b)
        c.raises("affected vs an unknown base -> ValueError", ValueError,
                 lambda: g.targets_affected(base="no-such-rev"))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
