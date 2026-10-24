#!/usr/bin/env python3
"""Hermetic tests for tasks.py — `python3 test_tasks.py` (pytest collects it
too), exit 0 on green. A throwaway git repo with scripted commits: exactly the
2-file fix and the PR merge are tasks; bots, a 9-file refactor, docs-only,
test-only, rename-heavy and 3-word-subject commits are not.
"""
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import tasks  # noqa: E402

ENV = {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
       "GIT_AUTHOR_NAME": "Dev", "GIT_AUTHOR_EMAIL": "dev@example.invalid",
       "GIT_COMMITTER_NAME": "Dev", "GIT_COMMITTER_EMAIL": "dev@example.invalid",
       "GLIA_AGENT_AB_OFFLINE": "1"}
os.environ.update(ENV)
TEN = "".join(f"line{i}\n" for i in range(1, 11))


class Repo:
    def __init__(self, root):
        self.root, self.t = Path(root), 1_700_000_000
        self.root.mkdir(parents=True)
        self.git("init", "-q", "-b", "main")

    def git(self, *args, **env):
        return subprocess.run(["git", "-C", str(self.root), *args], check=True, text=True,
                              capture_output=True, env={**os.environ, **env}).stdout.strip()

    def write(self, files):
        for path, text in files.items():
            p = self.root / path
            if text is None:
                p.unlink()
            else:
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text)

    def commit(self, message, files=None, **env):
        self.write(files or {})
        self.t += 100
        when = f"@{self.t} +0000"
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message, GIT_AUTHOR_DATE=when, GIT_COMMITTER_DATE=when,
                 **env)
        return self.git("rev-parse", "HEAD")


def scripted(root):
    """The scripted history; returns {name: sha}."""
    r, s = Repo(root), {}
    base = {f"src/f{i}.go": TEN for i in range(9)}
    base.update({"src/a.go": TEN, "src/b.go": TEN, "src/c.go": TEN, "src/a_test.go": TEN,
                 "README.md": "readme\n", "src/m1.go": TEN, "src/m2.go": TEN, "src/m3.go": TEN})
    s["base"] = r.commit("Initial import of the frame reader", base)
    a = TEN.replace("line3\n", "LINE3\n").replace("line7\n", "line7\n" + "new\n" * 5)
    s["fix"] = r.commit("fix(frames): handle empty frames in the reader\n\nFixes #12 by "
                        "skipping the zero-length payload.",
                        {"src/a.go": a, "src/b.go": TEN.replace("line1\n", "first\n"),
                         "src/a_test.go": TEN + "case\n"})
    s["bot"] = r.commit("Bump golang.org/x/net from 1.0.0 to 1.0.1 in the deps",
                        {"src/b.go": TEN.replace("line2\n", "two\n" * 6)},
                        GIT_AUTHOR_NAME="dependabot[bot]",
                        GIT_AUTHOR_EMAIL="49699333+dependabot[bot]@users.noreply.github.com")
    s["refactor"] = r.commit("Refactor every handler into one package",
                             {f"src/f{i}.go": TEN.replace("line5\n", "five\n") for i in range(9)})
    s["docs"] = r.commit("Describe the new framing rules in the README",
                         {"README.md": "readme\n" + "more docs\n" * 10})
    s["tests"] = r.commit("Add more coverage for frame parsing",
                          {"src/a_test.go": TEN + "case\n" * 8})
    moved = {f"src/m{i}.go": None for i in (1, 2, 3)}
    moved.update({f"internal/m{i}.go": TEN for i in (1, 2, 3)})
    moved["src/c.go"] = TEN.replace("line9\n", "nine\n" * 6)
    s["renames"] = r.commit("Move the frame helpers to internal", moved)
    s["short"] = r.commit("Fix frame bug", {"src/c.go": TEN.replace("line1\n", "one\n" * 6)})
    r.git("checkout", "-q", "-b", "x/y")
    c_now = (r.root / "src/c.go").read_text()
    s["branch"] = r.commit("wip", {"src/c.go": c_now.replace("line4\n", "four\n"),
                                   "src/d.go": "package src\n" + "var X = 1\n" * 4})
    r.git("checkout", "-q", "main")
    r.t += 100
    when = f"@{r.t} +0000"
    r.git("merge", "-q", "--no-ff", "x/y", "-m",
          "Merge pull request #7 from x/y\n\nHandle close frames from the peer",
          GIT_AUTHOR_DATE=when, GIT_COMMITTER_DATE=when)
    s["merge"] = r.git("rev-parse", "HEAD")
    return s


@contextlib.contextmanager
def fixture(gate="bot", url="https://github.com/example/demo.git", public=True):
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        s = scripted(tmp / "work")
        cache = tmp / "cache"
        subprocess.run(["git", "clone", "-q", "--bare", str(tmp / "work"),
                        str(cache / "demo.git")], check=True)
        repos = tmp / "repos.json"
        repos.write_text(json.dumps({"repos": [{
            "label": "demo", "url": url, "end_rev": s["merge"], "lang": "go",
            "gate_rev": s[gate] if gate else None, "tuned": False, "public": public}]}))
        old = os.environ.get("GLIA_AGENT_AB_DIR")
        os.environ["GLIA_AGENT_AB_DIR"] = str(cache)
        try:
            yield tmp, repos, s
        finally:
            if old is None:
                os.environ.pop("GLIA_AGENT_AB_DIR", None)
            else:
                os.environ["GLIA_AGENT_AB_DIR"] = old


def run(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = tasks.main(list(argv))
    return rc, out.getvalue(), err.getvalue()


def build(tmp, repos, name="tasks"):
    rc, out, err = run("build", "--repos", str(repos), "--out", str(tmp / name))
    assert rc == 0, err
    return (tmp / name / "demo.jsonl").read_text(), out


def test_selects_exactly_the_fix_and_the_merge():
    with fixture() as (tmp, repos, s):
        text, out = build(tmp, repos)
        rows = [json.loads(x) for x in text.splitlines()]
        assert [r["commit"] for r in rows] == [s["merge"], s["fix"]]
        merge, fix = rows
        assert merge["parent"] == s["short"] and merge["pr"] == 7
        assert merge["prompt_subject"] == merge["subject"] == "Handle close frames from the peer"
        assert merge["issue"] is None and merge["issue_fetch"] == "none"
        assert merge["truth"]["code"] == [
            {"path": "src/c.go", "status": "M", "old_path": None, "hunks": [[9, 1]]},
            {"path": "src/d.go", "status": "A", "old_path": None, "hunks": [[0, 0]]}]
        assert fix["parent"] == s["base"] and fix["pr"] is None
        assert fix["subject"] == "fix(frames): handle empty frames in the reader"
        assert fix["prompt_subject"] == "handle empty frames in the reader"
        assert fix["truth"] == {
            "code": [{"path": "src/a.go", "status": "M", "old_path": None,
                      "hunks": [[3, 1], [7, 0]]},
                     {"path": "src/b.go", "status": "M", "old_path": None, "hunks": [[1, 1]]}],
            "tests": ["src/a_test.go"], "other": []}
        assert fix["stats"] == {"code_files": 2, "code_lines": 9, "test_files": 1}
        assert fix["id"] == "demo-" + s["fix"][:12] and fix["repo"] == "demo"
        assert "[agent-ab] tasks repo=demo rows=2 issues=0 after_gate=1 sha256=" in out
        assert out.rstrip().endswith("[agent-ab] tasks total=2 repos=1")


def test_issue_fetch_is_skipped_offline():
    with fixture() as (tmp, repos, s):
        fix = json.loads(build(tmp, repos)[0].splitlines()[1])
        assert fix["issue"] is None and fix["issue_fetch"] == "unavailable"


def test_after_gate_is_strict_and_null_without_a_gate():
    with fixture(gate="fix") as (tmp, repos, s):
        rows = [json.loads(x) for x in build(tmp, repos)[0].splitlines()]
        assert [r["after_gate_rev"] for r in rows] == [True, False]
    with fixture(gate=None) as (tmp, repos, s):
        rows = [json.loads(x) for x in build(tmp, repos)[0].splitlines()]
        assert [r["after_gate_rev"] for r in rows] == [None, None]


def test_two_builds_are_byte_identical_and_check_passes():
    with fixture() as (tmp, repos, s):
        a, _ = build(tmp, repos, "one")
        b, _ = build(tmp, repos, "two")
        assert a == b and a.endswith("\n")
        rc, out, err = run("check", "--repos", str(repos), "--out", str(tmp / "one"))
        assert rc == 0, err
        assert "[agent-ab] tasks repo=demo rows=2" in out


def test_check_names_a_differing_row_and_carries_issue_text():
    with fixture() as (tmp, repos, s):
        text, _ = build(tmp, repos)
        path = tmp / "tasks" / "demo.jsonl"
        rows = [json.loads(x) for x in text.splitlines()]
        rows[1]["issue"] = {"number": 12, "title": "Empty frames", "body": "b"}
        rows[1]["issue_fetch"] = "ok"
        path.write_text(tasks.render(rows))
        assert run("check", "--repos", str(repos), "--out", str(tmp / "tasks"))[0] == 0
        rows[1]["truth"]["code"][0]["hunks"] = [[1, 1]]
        path.write_text(tasks.render(rows))
        rc, _, err = run("check", "--repos", str(repos), "--out", str(tmp / "tasks"))
        assert rc == 1 and f"check differs: demo-{s['fix'][:12]}" in err


def test_refuses_non_https_and_non_public_rows():
    for kw in ({"url": "http://example.invalid/demo.git"}, {"public": False}):
        with fixture(**kw) as (tmp, repos, s):
            rc, _, err = run("build", "--repos", str(repos), "--out", str(tmp / "t"))
            assert rc == 2 and "refused" in err
            assert not (tmp / "t").exists()


def test_classifiers():
    assert tasks.is_test("pkg/conn_test.go") and tasks.is_test("tests/x.py")
    assert tasks.is_test("src/test_cli.py") and tasks.is_test("a/FooTest.java")
    assert tasks.is_test("web/app.spec.tsx") and tasks.is_test("x/testdata/in.json")
    assert not tasks.is_test("src/contest.go") and not tasks.is_test("src/latest.py")
    assert tasks.subject_ok("feat(cli)!: add a pager for long help") == "add a pager for long help"
    for bad in ("Fix frame bug", "Docs for the new option flags", "Bump x from 1 to 2 now",
                "v1.2.3 release notes and things", "Revert the change to the reader"):
        assert tasks.subject_ok(bad) is None, bad


def test_redaction_mirrors_the_rust_rules():
    assert tasks.redact("API_TOKEN=abc123 rest") == "API_TOKEN=*** rest"
    assert tasks.redact('password: "hunter22" ok') == 'password: "***" ok'
    assert tasks.redact("Authorization: Bearer abcdefghijkl") == "Authorization: Bearer ***"
    assert tasks.redact("see https://u:pw@host/x") == "see https://***@host/x"
    assert tasks.redact("key ghp_" + "a" * 24) == "key ***"
    for prose in ("Basic authentication is fine", "token handling in the reader", "token == x"):
        assert tasks.redact(prose) == prose


def main():
    tests = [(n, f) for n, f in sorted(globals().items()) if n.startswith("test_")]
    failed = 0
    for name, fn in tests:
        try:
            fn()
            print(f"ok   {name}")
        except Exception:
            failed += 1
            print(f"FAIL {name}")
            traceback.print_exc()
    print(f"{len(tests) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
