"""
Per-repo test command + log parser dispatch, sourced from the official
swebench harness so we get authoritative behaviour for django (runtests.py),
sympy (bin/test), pytest-self, etc. without reimplementing.

The official harness runs inside Docker + conda; we run in-process. So we
import the *spec* (test_cmd, directives, parser) but skip the install /
conda activation lines — the caller is responsible for ensuring the repo
is importable (pip install -e .).

Public surface:
  - get_test_command(instance) -> (cmd_str, directives, parse_fn)
  - evaluate_log(repo, log, f2p_ids, p2p_ids) -> dict
"""
from __future__ import annotations

import re

from swebench.harness.constants import MAP_REPO_VERSION_TO_SPECS, TestStatus
from swebench.harness.log_parsers import MAP_REPO_TO_PARSER
from swebench.harness.test_spec.python import get_test_directives


# Patch parse_log_django: official parser drops the test method name when the
# test has a docstring (django --verbosity 2 prints "test_x (mod.Class)" on
# line N, "docstring ... ok" on N+1). We track the most recent line that
# matches `test_x (path.Class)` and use it as the id when the next line ends
# with " ... ok"/"... FAIL"/"... ERROR".
_DJANGO_TEST_HEADER = re.compile(r"^(test_\w+\s+\([\w.]+\))\s*$")


def _patched_parse_log_django(log: str, test_spec=None) -> dict[str, str]:
    PASSED = TestStatus.PASSED.value
    FAILED = TestStatus.FAILED.value
    ERROR = TestStatus.ERROR.value
    SKIPPED = TestStatus.SKIPPED.value
    out = {}
    pending_id = None
    for raw in log.split("\n"):
        line = raw.strip()
        m = _DJANGO_TEST_HEADER.match(line)
        if m:
            pending_id = m.group(1)
            continue
        # Single-line form: "test_x (path.Class) ... ok"
        m2 = re.match(r"^(test_\w+\s+\([\w.]+\))\s+\.\.\.\s+(ok|OK|FAIL|ERROR|skipped.*)$", line)
        if m2:
            tid = m2.group(1)
            verdict = m2.group(2)
            if verdict in ("ok", "OK"):
                out[tid] = PASSED
            elif verdict.startswith("skipped"):
                out[tid] = SKIPPED
            elif verdict == "FAIL":
                out[tid] = FAILED
            else:
                out[tid] = ERROR
            pending_id = None
            continue
        # Multi-line form: pending_id was set on previous line; this line is
        # "<docstring> ... ok/FAIL/ERROR/skipped".
        if pending_id is not None:
            if line.endswith(" ... ok") or line.endswith(" ... OK"):
                out[pending_id] = PASSED
                pending_id = None
            elif line.endswith(" ... FAIL"):
                out[pending_id] = FAILED
                pending_id = None
            elif line.endswith(" ... ERROR"):
                out[pending_id] = ERROR
                pending_id = None
            elif " ... skipped" in line:
                out[pending_id] = SKIPPED
                pending_id = None
        # FAIL: / ERROR: blocks (django prints these in summary)
        if line.startswith("FAIL: "):
            tid = line[len("FAIL: "):].split()[0]
            # Recover full id `test_x (path.Class)` if rest of line has it
            rest = line[len("FAIL: "):].strip()
            full = re.match(r"^(test_\w+\s+\([\w.]+\))", rest)
            if full:
                out[full.group(1)] = FAILED
            else:
                out[tid] = FAILED
        elif line.startswith("ERROR: "):
            rest = line[len("ERROR: "):].strip()
            full = re.match(r"^(test_\w+\s+\([\w.]+\))", rest)
            if full:
                out[full.group(1)] = ERROR
    return out


# Replace in dispatch table
MAP_REPO_TO_PARSER = dict(MAP_REPO_TO_PARSER)
MAP_REPO_TO_PARSER["django/django"] = _patched_parse_log_django


def get_test_command(instance: dict) -> tuple[str, list[str], callable]:
    """Return (test_cmd, directives, parse_fn) for an instance.

    test_cmd: shell command string from spec (e.g. "pytest -rA",
              "./tests/runtests.py --verbosity 2 --settings=test_sqlite --parallel 1")
    directives: per-repo list of test-file arguments (django dotted, others paths)
    parse_fn: log parser returning {test_name: TestStatus}
    """
    repo = instance["repo"]
    version = instance["version"]
    spec = MAP_REPO_VERSION_TO_SPECS[repo][version]
    cmd = spec["test_cmd"]
    directives = get_test_directives(instance)
    parse_fn = MAP_REPO_TO_PARSER[repo]
    return cmd, directives, parse_fn


_PASS_VALUES = {TestStatus.PASSED.value, TestStatus.XFAIL.value}


def evaluate_log(repo: str, log: str, f2p_ids: list[str], p2p_ids: list[str] | None = None) -> dict:
    """Parse log with the repo's parser, return dict of F2P/P2P pass counts.

    Returns:
        {
          "f2p_pass": int, "f2p_fail": int, "f2p_missing": [ids not in log],
          "p2p_pass": int, "p2p_fail": int, "p2p_missing": [ids not in log],
          "f2p_status": {id: status},
          "all_pass": bool — true iff every f2p id passed and no p2p regressed
        }
    """
    parse_fn = MAP_REPO_TO_PARSER[repo]
    status_map = parse_fn(log, None)  # parsers don't use test_spec arg

    p2p_ids = p2p_ids or []

    def _classify(ids: list[str]) -> tuple[int, int, list[str], dict]:
        passed, failed, missing = 0, 0, []
        per_id = {}
        for tid in ids:
            s = status_map.get(tid)
            per_id[tid] = s
            if s is None:
                missing.append(tid)
            elif s in _PASS_VALUES:
                passed += 1
            else:
                failed += 1
        return passed, failed, missing, per_id

    f2p_pass, f2p_fail, f2p_missing, f2p_status = _classify(f2p_ids)
    p2p_pass, p2p_fail, p2p_missing, _ = _classify(p2p_ids)

    all_pass = (
        f2p_pass == len(f2p_ids)
        and f2p_fail == 0
        and len(f2p_missing) == 0
        and p2p_fail == 0
    )
    return {
        "f2p_pass": f2p_pass,
        "f2p_fail": f2p_fail,
        "f2p_missing": f2p_missing,
        "f2p_status": f2p_status,
        "p2p_pass": p2p_pass,
        "p2p_fail": p2p_fail,
        "p2p_missing": p2p_missing,
        "all_pass": all_pass,
    }
