#!/usr/bin/env python3
"""pyo3 surface, py/src/contract_breaks.rs (CC.8b): the module function
`contract_breaks_vs_rev(repo_path, base="HEAD", avro="backward",
breaking_only=False)` builds the git rev `base` and the working tree of the
repo at `repo_path` and returns the engine's contract breaks as a native dict
{base, schemas, orphaned_clients, breaking, absence}: every contract paired old
-> new and judged by its format's evolution rules, one `changes` entry per
declared difference, lines 1-based; `breaking` counts the breaking schema rows
plus the orphaned clients. A bad `avro` mode or an engine error raises
ValueError. Needs a `git` binary. Shared helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

ORDERS_V1 = (
    "openapi: 3.0.0\n"
    "info:\n"
    "  title: orders\n"
    "  version: \"1\"\n"
    "paths:\n"
    "  /orders/{id}:\n"
    "    get:\n"
    "      operationId: getOrder\n"
    "      responses:\n"
    "        \"200\":\n"
    "          description: ok\n"
    "          content:\n"
    "            application/json:\n"
    "              schema:\n"
    "                type: object\n"
    "                properties:\n"
    "                  id:\n"
    "                    type: string\n"
    "                  total:\n"
    "                    type: number\n"
)
TOTAL = "                  total:\n                    type: number\n"
CURRENCY = "                  currency:\n                    type: string\n"
KEYS = ["base", "schemas", "orphaned_clients", "breaking", "absence"]
ROW_KEYS = ["kind", "key", "format", "before", "after", "status", "change", "tier", "note", "changes"]
CHANGE_KEYS = ["section", "field", "change", "producer", "consumer", "rule", "breaking"]
SIDE_KEYS = ["repo_id", "qname", "format", "file", "line"]
KW = [("repo_path", None), ("base", "HEAD"), ("avro", "backward"), ("breaking_only", False)]
MARKER = ("[contract-breaks] base=HEAD pairs=1 breaking=1 compatible=0 unknown=0 "
          "removed=0 added=0 orphaned_clients=0")


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    subprocess.run(["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def main() -> int:
    c = Checks("contract_breaks")
    fn = getattr(rg, "contract_breaks_vs_rev", None)
    c.check("contract_breaks_vs_rev exists", fn is not None)
    if fn is None:
        return c.done()
    c.check("contract_breaks_vs_rev params", params(fn) == KW, params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-contract-breaks-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        top = pathlib.Path(tmp) / "orders"
        top.mkdir()
        spec = top / "openapi.yaml"
        spec.write_text(ORDERS_V1)
        git(top, "init", "-q")
        git(top, "add", "-A")
        git(top, "commit", "-q", "-m", "v1")

        spec.write_text(ORDERS_V1.replace(TOTAL, ""))
        d, err = stderr_of(lambda: rg.contract_breaks_vs_rev(str(top)))
        c.check("-> dict in engine field order", type(d) is dict and list(d) == KEYS, type(d) is dict and list(d))
        d = d if type(d) is dict else {}
        c.check("base as given", d.get("base") == "HEAD", d.get("base"))
        c.check("breaking == 1", d.get("breaking") == 1, d)
        c.check("no absence on an answer", d.get("absence") is None, d.get("absence"))
        rows = d.get("schemas") or [{}]
        row = rows[0]
        c.check("one schema row", len(rows) == 1, rows)
        c.check("schema row keys in engine field order", list(row) == ROW_KEYS, list(row))
        c.check("the op is breaking",
                (row.get("kind"), row.get("key"), row.get("status"), row.get("change"), row.get("tier"))
                == ("operation", "GET /orders/{id}", "breaking", "modified", "fact"), row)
        after = row.get("after") or {}
        c.check("side keys in engine field order", list(after) == SIDE_KEYS, list(after))
        c.check("located on the op's 1-based line", (after.get("file"), after.get("line")) == ("openapi.yaml", 7),
                after)
        c.check("repo_id is an int", type(after.get("repo_id")) is int, after)
        change = (row.get("changes") or [{}])[0]
        c.check("change keys in engine field order", list(change) == CHANGE_KEYS, list(change))
        c.check("the removed response field",
                (change.get("section"), change.get("field"), change.get("producer"), change.get("consumer"),
                 change.get("rule"), change.get("breaking"))
                == ("response:200", "total", "number", None, "response_field_removed", True), change)
        c.check("no orphaned client", d.get("orphaned_clients") == [], d.get("orphaned_clients"))
        c.check("engine marker", MARKER in err, err[-400:])

        spec.write_text(ORDERS_V1 + CURRENCY)
        ok = rg.contract_breaks_vs_rev(str(top), avro="full")
        statuses = [r.get("status") for r in ok.get("schemas", [])]
        c.check("a compatible change is not breaking", ok.get("breaking") == 0 and statuses == ["compatible"], ok)
        only = rg.contract_breaks_vs_rev(str(top), breaking_only=True)
        c.check("breaking_only drops the compatible row",
                only.get("schemas") == [] and (only.get("absence") or {}).get("reason") == "no_match", only)

        spec.write_text(ORDERS_V1)
        clean = rg.contract_breaks_vs_rev(str(top), base="HEAD")
        c.check("a clean tree lists nothing and says why",
                clean.get("schemas") == [] and clean.get("breaking") == 0
                and (clean.get("absence") or {}).get("tier") == "FACT", clean)

        c.raises("bad avro mode raises", ValueError, lambda: rg.contract_breaks_vs_rev(str(top), avro="x"),
                 "unknown avro mode `x`")
        c.raises("unknown rev raises", ValueError,
                 lambda: rg.contract_breaks_vs_rev(str(top), base="no-such-rev"), "no-such-rev")
        plain = pathlib.Path(tmp) / "plain"
        plain.mkdir()
        c.raises("not a git work tree raises", ValueError, lambda: rg.contract_breaks_vs_rev(str(plain)),
                 "not a git work tree")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
