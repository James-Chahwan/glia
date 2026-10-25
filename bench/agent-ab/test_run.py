#!/usr/bin/env python3
"""Hermetic tests for run.py — `python3 test_run.py` (pytest collects it too),
exit 0 on green. A throwaway origin repo with two task commits, bare clones
under a temp GLIA_AGENT_AB_DIR, a temp glia repo with v0.5.2 / v0.5.3 tags,
prebuilt fake `glia` binaries per arm, and a fake `claude` on PATH that emits
a canned stream-json transcript (driven by a scenario file) and logs what it
saw: the arm (from which `glia` PATH resolves), CLAUDE_CONFIG_DIR, the glia
snapshot directories and the skills of the checkout.
"""
import contextlib
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import run  # noqa: E402

GIT_ENV = {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
           "GIT_AUTHOR_NAME": "Dev", "GIT_AUTHOR_EMAIL": "dev@example.invalid",
           "GIT_COMMITTER_NAME": "Dev", "GIT_COMMITTER_EMAIL": "dev@example.invalid",
           "GLIA_AGENT_AB_OFFLINE": "1"}
os.environ.update(GIT_ENV)
VERSION = "2.1.310 (Claude Code)"

FAKE_CLAUDE = r'''#!@PY@
import json, os, pathlib, shutil, subprocess, sys, time
argv = sys.argv[1:]
if argv[:1] == ["--version"]:
    print(os.environ.get("FAKE_CLAUDE_VERSION", "@VERSION@"))
    sys.exit(0)
scen = json.loads(pathlib.Path(os.environ["FAKE_CLAUDE_SCENARIO"]).read_text())
which = shutil.which("glia") or ""
arm = pathlib.Path(which).parent.name
arm = "none" if arm == "deny" else arm
spec = dict(scen.get("default", {}))
spec.update(scen.get("by_arm", {}).get(arm, {}))
cwd = os.getcwd()
gp = subprocess.run(["glia", "--version"], capture_output=True, text=True)
cfg = os.environ.get("CLAUDE_CONFIG_DIR", "")
sk = pathlib.Path(cwd, ".claude", "skills")
skills = sorted(p.name for p in sk.iterdir()) if sk.is_dir() else []
rec = {"arm": arm, "argv": argv, "cwd": cwd, "config_dir": cfg,
       "config_entries": sorted(os.listdir(cfg)) if os.path.isdir(cfg) else None,
       "glia_rc": gp.returncode, "glia_err": gp.stderr.strip(), "glia_out": gp.stdout.strip(),
       "history": os.path.isdir(".glia/history-snapshot"), "graph": os.path.isdir(".glia/graph"),
       "calibration": os.path.isdir(".glia/calibration-snapshot"), "skills": skills,
       "claudecode": os.environ.get("CLAUDECODE"), "threads": os.environ.get("GLIA_THREADS")}
with open(os.environ["FAKE_CLAUDE_LOG"], "a") as f:
    f.write(json.dumps(rec) + "\n")
pathlib.Path(cfg, "projects").mkdir(parents=True, exist_ok=True)
def emit(e):
    print(json.dumps(e), flush=True)
init = {"type": "system", "subtype": "init", "cwd": cwd, "model": "claude-opus-5-5",
        "tools": ["Bash", "Read", "Edit"] + spec.get("extra_tools", []),
        "mcp_servers": spec.get("mcp_servers", []), "plugins": [],
        "skills": skills + ["update-config"] + spec.get("extra_skills", [])}
emit(init)
flag = pathlib.Path(cfg, "rate-limited-once")
if spec.get("rate_limit_first") and not flag.exists():
    flag.write_text("x")
    pathlib.Path(cwd, "junk.txt").write_text("partial\n")
    emit({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}})
    emit({"type": "result", "subtype": "success", "is_error": True,
          "result": "Claude AI usage limit reached"})
    sys.exit(1)
time.sleep(spec.get("sleep", 0))
for t in spec.get("tools", []):
    inp = {k: v.replace("{cwd}", cwd) for k, v in t["input"].items()}
    emit({"type": "assistant", "message": {"content": [
        {"type": "tool_use", "name": t["name"], "input": inp}]}})
for path, text in spec.get("edits", {}).items():
    pathlib.Path(cwd, path).write_text(text)
emit({"type": "result", "subtype": spec.get("subtype", "success"), "is_error": False,
      "num_turns": 4, "duration_ms": 1234, "total_cost_usd": spec.get("cost", 1.0),
      "usage": {"input_tokens": 10, "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 30, "output_tokens": 40}})
'''

FAKE_GLIA = r'''#!@PY@
import os, sys
a = sys.argv[1:]
if a == ["--version"]:
    print("@VERSION@")
    sys.exit(0)
if os.environ.get("FAKE_GLIA_FAIL") and os.environ["FAKE_GLIA_FAIL"] in a:
    sys.exit(1)
dirs = {("history", "sync"): "history-snapshot", ("build",): "graph",
        ("history", "calibrate"): "calibration-snapshot"}
for k, v in dirs.items():
    if tuple(a[:len(k)]) == k:
        os.makedirs(os.path.join(a[len(k)], ".glia", v), exist_ok=True)
'''

BASE = {"a.go": "package a\n\nfunc A() int { return 1 }\n",
        "b.go": "package a\n\nfunc B() int { return 2 }\n",
        "sub/c.go": "package sub\n\nfunc C() int { return 3 }\n"}

NORMAL = [
    {"name": "Read", "input": {"file_path": "{cwd}/a.go"}},
    {"name": "Bash", "input": {"command": "cd sub && cat c.go ../b.go missing.go | grep x"}},
    {"name": "Read", "input": {"file_path": "{cwd}/.glia/graph/manifest.json"}},
    {"name": "Bash", "input": {"command": "sed -n '1,5p' a.go"}},
    {"name": "Edit", "input": {"file_path": "{cwd}/a.go"}},
]
GLIA_CALL = {"name": "Bash", "input": {"command": "GLIA_THREADS=1 glia find . A --json"}}


def git(cwd, *args):
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, text=True,
                          capture_output=True).stdout.strip()


def script(path, body, **subs):
    for k, v in subs.items():
        body = body.replace(k, v)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o755)


class World:
    """A temp cache dir, toy repos, a glia repo with two tags and fake binaries."""

    def __init__(self, tmp, runs=1, timeout_s=60, g053_stamp="0.5.3+pbbbb"):
        self.tmp = Path(tmp)
        self.cache = self.tmp / "cache"
        origin = self.tmp / "origin"
        origin.mkdir()
        git(origin, "init", "-q", "-b", "main")
        self.c = []
        for i, files in enumerate([BASE, {"a.go": BASE["a.go"] + "// one\n"},
                                   {"b.go": BASE["b.go"] + "// two\n"}]):
            for p, text in files.items():
                (origin / p).parent.mkdir(parents=True, exist_ok=True)
                (origin / p).write_text(text)
            git(origin, "add", "-A")
            git(origin, "commit", "-q", "-m", f"commit {i} changes things")
            self.c.append(git(origin, "rev-parse", "HEAD"))
        self.cache.mkdir()
        for label in ("toy", "toy2"):
            subprocess.run(["git", "clone", "-q", "--bare", str(origin),
                            str(self.cache / f"{label}.git")], check=True)
        repos = [{"label": lb, "url": f"https://github.com/example/{lb}.git", "end_rev": self.c[2],
                  "lang": "go", "gate_rev": None, "tuned": False, "public": True}
                 for lb in ("toy", "toy2")]
        (self.tmp / "repos.json").write_text(json.dumps({"repos": repos}))
        tdir = self.tmp / "tasks"
        tdir.mkdir()
        row = lambda label, k, issue=None: {  # noqa: E731
            "id": f"{label}-{self.c[k][:12]}", "commit": self.c[k], "parent": self.c[k - 1],
            "prompt_subject": f"change number {k} of the toy",
            "issue": issue, "issue_fetch": "ok" if issue else "none"}
        (tdir / "toy.jsonl").write_text(
            json.dumps(row("toy", 2, {"number": 7, "title": "B is wrong", "body": "fix B"}))
            + "\n" + json.dumps(row("toy", 1)) + "\n")
        (tdir / "toy2.jsonl").write_text(json.dumps(row("toy2", 2)) + "\n")
        gr = self.tmp / "glia"
        gr.mkdir()
        git(gr, "init", "-q", "-b", "main")
        self.sha = {}
        for arm, tag in (("g052", "v0.5.2"), ("g053", "v0.5.3")):
            (gr / "skills" / "glia").mkdir(parents=True, exist_ok=True)
            (gr / "skills" / "glia" / "SKILL.md").write_text(f"---\nname: glia\n---\n{tag}\n")
            git(gr, "add", "-A")
            git(gr, "commit", "-q", "-m", tag)
            git(gr, "tag", tag)
            self.sha[arm] = git(gr, "rev-parse", "HEAD")
        self.glia_repo = gr
        for arm, ver in (("g052", "glia 0.5.2 (build 0.5.2+paaaa)"),
                         ("g053", f"glia 0.5.3 (build {g053_stamp})")):
            script(self.cache / "bin" / arm / "glia", FAKE_GLIA,
                   **{"@PY@": sys.executable, "@VERSION@": ver})
            (self.cache / "bin" / arm / "meta.json").write_text(json.dumps(
                {"rev": "v0.5.2" if arm == "g052" else "v0.5.3", "sha": self.sha[arm],
                 "version": ver}))
        fakebin = self.tmp / "fakebin"
        script(fakebin / "claude", FAKE_CLAUDE,
               **{"@PY@": sys.executable, "@VERSION@": VERSION})
        cfg = json.loads((run.HERE / "config.json").read_text())
        cfg.update(runs=runs, timeout_s=timeout_s, claude_version=VERSION)
        self.config = self.tmp / "config.json"
        self.config.write_text(json.dumps(cfg))
        home = self.tmp / "home"
        (home / ".claude" / "skills" / "engram").mkdir(parents=True)
        self.log = self.tmp / "claude.log"
        self.scenario = self.tmp / "scenario.json"
        self.set_scenario({"default": {"tools": NORMAL, "edits": {"a.go": "package a\n"}},
                           "by_arm": {"g052": {"tools": NORMAL + [GLIA_CALL]},
                                      "g053": {"tools": NORMAL + [GLIA_CALL]}}})
        self.env = {"GLIA_AGENT_AB_DIR": str(self.cache), "HOME": str(home),
                    "PATH": f"{fakebin}{os.pathsep}{os.environ['PATH']}",
                    "CLAUDE_CODE_OAUTH_TOKEN": "test-token", "CLAUDECODE": "1",
                    "FAKE_CLAUDE_SCENARIO": str(self.scenario), "FAKE_CLAUDE_LOG": str(self.log),
                    "ANTHROPIC_API_KEY": None, "CLAUDE_CONFIG_DIR": None, "GLIA_RATE_LIMITS": None,
                    "FAKE_CLAUDE_VERSION": None, "FAKE_GLIA_FAIL": None}

    def set_scenario(self, scen):
        self.scenario.write_text(json.dumps(scen))

    def main(self, *argv, **env):
        saved = {k: os.environ.get(k) for k in {**self.env, **env}}
        for k, v in {**self.env, **env}.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        out = io.StringIO()
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                rc = run.main(["--config", str(self.config),
                               "--repos", str(self.tmp / "repos.json"),
                               "--tasks-dir", str(self.tmp / "tasks"),
                               "--glia-repo", str(self.glia_repo), *argv])
        finally:
            for k, v in saved.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
        return rc, out.getvalue()

    def calls(self):
        return [json.loads(x) for x in self.log.read_text().splitlines()] \
            if self.log.exists() else []

    def rows(self, run_id):
        return run.read_rows(self.cache / "runs" / run_id / "sessions.jsonl")

    def ready(self):
        rc, out = self.main("binaries")
        assert rc == 0, out
        rc, out = self.main("isolation")
        assert rc == 0, out
        self.log.unlink()
        return out


@contextlib.contextmanager
def world(**kw):
    with tempfile.TemporaryDirectory() as tmp:
        sleeps = []
        old = run.sleep
        run.sleep = sleeps.append
        try:
            w = World(tmp, **kw)
            w.sleeps = sleeps
            yield w
        finally:
            run.sleep = old


# --- tests -------------------------------------------------------------------
def test_segments_and_violations():
    seg = run.segments("cd x && FOO=1 env BAR=2 glia find . X --json | head -3; (grep -rn y .)")
    assert seg == [("cd", ["x"]), ("glia", ["find", ".", "X", "--json"]), ("head", ["-3"]),
                   ("grep", ["-rn", "y", "."])], seg
    assert run.segments("echo 'a|b' 2>&1 > out.txt") == [("echo", ["a|b", "2"])]

    def ev(*cmds, tools=()):
        uses = [{"type": "tool_use", "name": "Bash", "input": {"command": c}} for c in cmds]
        uses += [{"type": "tool_use", "name": t, "input": {}} for t in tools]
        return [{"type": "assistant", "message": {"content": uses}}]

    c, n = run.transcript_violations(ev("uvx --from glia-cli glia find ."), False)
    assert c and not n, (c, n)
    c, _ = run.transcript_violations(ev("glia find . X"), False)
    assert c
    c, _ = run.transcript_violations(ev("cd /x && /usr/local/bin/glia find ."), True)
    assert c
    c, _ = run.transcript_violations(ev("glia find . X", "python -m pip install glia-py"), True)
    assert c == ["python -m pip install glia-py"], c
    c, _ = run.transcript_violations(ev("glia find . X --json"), True)
    assert not c
    for cmd in ("git -C . fetch origin", "curl https://example.invalid", "git remote add o u",
                "gh pr view 1", "wget x", "git pull"):
        assert run.transcript_violations(ev(cmd), True)[1], cmd
    assert run.transcript_violations(ev(tools=["WebFetch"]), False)[1]
    assert run.transcript_violations(ev("git log -3", "git status"), False) == ([], [])


def test_prompt_with_issue():
    repo = {"label": "toy", "url": "https://github.com/example/toy.git"}
    row = {"prompt_subject": "Fix the thing", "issue": {"number": 7, "title": "T", "body": "B"},
           "issue_fetch": "ok"}
    p = run.render_prompt(run.load_template(), repo, row)
    assert p.startswith("You are working in a checkout of the example/toy repository. "
                        "Make this change:\n\nFix the thing\n\nIssue #7: T\nB\n\nEdit the files"), p
    assert p.endswith("reply with the list of files you changed."), p
    row["issue_fetch"] = "unavailable"
    assert "Issue #7" not in run.render_prompt("{task}", repo, row)
    assert run.render_prompt("{task}", repo, row, "Use glia.") == "Fix the thing\n\nUse glia."


def test_checkout_is_leak_free():
    with world() as w:
        copy = w.tmp / "copy"
        run.checkout(w.cache, "toy", w.c[1], w.c[2], copy)
        assert git(copy, "rev-parse", "HEAD") == w.c[1]
        assert git(copy, "symbolic-ref", "--short", "HEAD") == "main"
        r = subprocess.run(["git", "-C", str(copy), "cat-file", "-e", w.c[2]], capture_output=True)
        assert r.returncode != 0
        assert git(copy, "remote") == ""
        assert (copy / ".git" / "info" / "exclude").read_text().endswith(".glia/\n.claude/\n")


def test_refusals():
    with world() as w:
        assert w.main("binaries")[0] == 0
        rc, out = w.main("isolation", CLAUDE_CODE_OAUTH_TOKEN=None)
        assert rc == 2 and "refused: set CLAUDE_CODE_OAUTH_TOKEN" in out, out
        rc, out = w.main("isolation", ANTHROPIC_API_KEY="sk-test")
        assert rc == 2 and "ANTHROPIC_API_KEY is set" in out and "FE-3" in out, out
        rc, out = w.main("isolation", ANTHROPIC_API_KEY="sk-test", CLAUDE_CODE_OAUTH_TOKEN=None)
        assert rc == 2 and "ANTHROPIC_API_KEY is set" in out, out
        rc, out = w.main("isolation", FAKE_CLAUDE_VERSION="9.9.9 (Claude Code)")
        assert rc == 2 and "claude --version" in out, out
        rc, out = w.main("pilot")
        assert rc == 2 and "isolation" in out, out
        assert not w.calls()


def test_binaries_skip_and_same_stamp():
    with world() as w:
        rc, out = w.main("binaries")
        assert rc == 0, out
        assert f"[agent-ab] binary arm=g052 sha={w.sha['g052'][:12]} version=glia 0.5.2 " \
               "(build 0.5.2+paaaa)" in out, out
        assert "arm=g053" in out and "target@" not in "".join(os.listdir(w.cache))
        assert (w.cache / "skills" / "g053" / "glia" / "SKILL.md").read_text().endswith("v0.5.3\n")
    with world(g053_stamp="0.5.2+paaaa") as w:
        rc, out = w.main("binaries")
        assert rc == 2 and "same build stamp" in out, out


def test_isolation_fail_blocks_run():
    with world() as w:
        assert w.main("binaries")[0] == 0
        w.set_scenario({"by_arm": {"none": {"extra_skills": ["glia-helper", "engram"]}}})
        rc, out = w.main("isolation")
        assert rc == 1, out
        assert "[agent-ab] isolation arm=none mcp=0" in out and out.count(" ok\n") == 2, out
        assert "skill glia-helper" in out and "user skill engram" in out, out
        assert w.main("pilot")[0] == 2


def test_pilot_then_resume():
    with world() as w:
        out = w.ready()
        for arm in ("none", "g052", "g053"):
            assert f"[agent-ab] isolation arm={arm} mcp=0 " in out, out
        rc, out = w.main("pilot", "--run-id", "p1")
        assert rc == 0, out
        calls = w.calls()
        # pilot = first task of each repo (toy[0], toy2[0]) x 3 arms; toy2[0] is index 2
        assert [c["arm"] for c in calls] == ["none", "g052", "g053", "g053", "none", "g052"], calls
        assert ("[agent-ab] pilot sessions=6 usd=6.00 median_usd=1.0000 est_full_usd=9.00"
                in out), out
        assert "usage=unknown reason=no_rate_limits_file" in out, out
        first = calls[0]
        assert first["config_dir"] == str(w.cache / "claude-config" / "p1")
        assert first["config_entries"] == [], first
        assert first["claudecode"] is None and first["threads"] == "4"
        for c in calls:
            glia_arm = c["arm"] != "none"
            assert c["history"] == c["graph"] == glia_arm, c
            assert c["calibration"] == (c["arm"] == "g053"), c
            assert c["skills"] == (["glia"] if glia_arm else []), c
            if glia_arm:
                assert c["glia_rc"] == 0 and c["glia_out"].startswith("glia 0.5."), c
            else:
                assert c["glia_rc"] == 127 and c["glia_err"] == "glia: command not found", c
            argv = c["argv"]
            assert argv[argv.index("-p") + 1].endswith(
                "\n\nA glia graph of this repository is already built. Start with the `glia` "
                "skill: use it to find where the change belongs and what it affects (definitions, "
                "callers, implementations, the files a task is about) before you read or edit "
                "code, and again whenever you need to know what else a change touches.") == glia_arm
            cfg = json.loads((run.HERE / "config.json").read_text())
            assert argv[argv.index("--disallowedTools") + 1:] == cfg["disallowed_tools"], argv
            assert {"Agent", "Task", "Workflow", "WebFetch", "RemoteTrigger"} <= \
                set(cfg["disallowed_tools"]), cfg
            assert json.loads(argv[argv.index("--settings") + 1]) == {"enabledPlugins": {
                p: False for p in cfg["disabled_plugins"]}}, argv
            for flag in ("--strict-mcp-config", "--verbose"):
                assert flag in argv
            assert argv[argv.index("--setting-sources") + 1] == "project"
            assert argv[argv.index("--output-format") + 1] == "stream-json"
        assert "Issue #7: B is wrong\nfix B" in calls[0]["argv"][1]
        rows = w.rows("p1")
        assert len(rows) == 6
        r = next(x for x in rows if x["arm"] == "g052")
        assert r["terminated"] == "end", r
        assert r["files_opened"] == ["a.go", "sub/c.go", "b.go"], r["files_opened"]
        assert r["glia_calls"] == [{"subcommand": "find", "flags": ["--json"]}], r
        assert r["grep_calls"] == 1 and r["tool_calls"] == {"Bash": 3, "Edit": 1, "Read": 2}, r
        assert r["glia_sha"] == w.sha["g052"] and r["auth"] == "oauth_token" and r["prep_ms"] >= 0
        assert r["usage"] == {"input_tokens": 10, "cache_creation_input_tokens": 20,
                              "cache_read_input_tokens": 30, "output_tokens": 40}
        assert r["model"] == "claude-opus-5-5" and r["num_turns"] == 4
        diff = Path(r["patch"]["path"]).read_bytes()
        assert r["patch"]["sha256"] == hashlib.sha256(diff).hexdigest()
        assert b"-func A() int { return 1 }" in diff and b".glia" not in diff, diff
        c = next(x for x in rows if x["arm"] == "none")
        assert c["terminated"] == "end" and c["glia_sha"] is None and c["glia_calls"] == []
        assert not (w.cache / "work" / "p1" / r["task_id"] / "g052" / "1").exists()
        assert "[agent-ab] session task=toy-" in out and "glia_calls=1 terminated=end" in out
        # resume: the full matrix adds only toy[1] x 3 arms (index 1 -> g052, g053, none)
        rc, out = w.main("run", "--run-id", "p1")
        assert rc == 0, out
        assert [c["arm"] for c in w.calls()[6:]] == ["g052", "g053", "none"]
        assert len(w.rows("p1")) == 9
        rc, out = w.main("run", "--run-id", "p1")
        assert rc == 0 and len(w.calls()) == 9
        rc, out = w.main("status", "--run-id", "p1")
        assert "[agent-ab] status run=p1 sessions=9 usd=9.00" in out, out
        assert "[agent-ab] status arm=g053 sessions=3 end=3" in out, out


def test_contamination_and_network():
    with world() as w:
        w.ready()
        bash = lambda c: {"tools": [{"name": "Bash", "input": {"command": c}}]}  # noqa: E731
        w.set_scenario({"by_arm": {"none": bash("uvx --from glia-cli glia find ."),
                                   "g052": bash("/opt/x/bin/glia find ."),
                                   "g053": bash("git fetch origin && curl x")}})
        assert w.main("pilot", "--run-id", "c1")[0] == 0
        by = {r["arm"]: r["terminated"] for r in w.rows("c1")}
        assert by == {"none": "contaminated", "g052": "contaminated", "g053": "network"}, by
        w.set_scenario({"default": {"extra_tools": ["mcp__repo-graph__find"]}})
        assert w.main("pilot", "--run-id", "c2")[0] == 0
        rows = w.rows("c2")
        assert {r["terminated"] for r in rows} == {"contaminated"}
        assert "tool mcp__repo-graph__find" in rows[0]["violations"]


def test_budget_stop_and_resume():
    with world() as w:
        w.ready()
        rc, out = w.main("run", "--run-id", "b1", "--max-usd", "2.5")
        assert rc == 0 and "[agent-ab] budget stop spent=2.00" in out, out
        assert len(w.rows("b1")) == 2 and len(w.calls()) == 2
        rc, out = w.main("run", "--run-id", "b1", "--max-usd", "100")
        assert rc == 0 and len(w.rows("b1")) == 9 and len(w.calls()) == 9, out
        keys = [run.key_of(r) for r in w.rows("b1")]
        assert len(set(keys)) == 9


def test_pins_refuse_a_changed_run():
    with world() as w:
        w.ready()
        assert w.main("pilot", "--run-id", "x1")[0] == 0
        (w.tmp / "tasks" / "toy2.jsonl").write_text(
            (w.tmp / "tasks" / "toy2.jsonl").read_text().replace("number 2", "number 3"))
        rc, out = w.main("run", "--run-id", "x1")
        assert rc == 2 and "other pins (tasks)" in out, out


def test_rate_limit_retry_resets_checkout():
    with world() as w:
        w.ready()
        w.set_scenario({"default": {"rate_limit_first": True, "tools": NORMAL,
                                    "edits": {"a.go": "package a\n"}}})
        rc, out = w.main("pilot", "--run-id", "r1")
        assert rc == 0, out
        assert w.sleeps == [900], w.sleeps
        rows = w.rows("r1")
        assert rows[0]["attempts"] == 2 and rows[0]["terminated"] == "end", rows[0]
        assert b"junk.txt" not in Path(rows[0]["patch"]["path"]).read_bytes()
        assert all(r["attempts"] == 1 for r in rows[1:])


def test_timeout_and_prep_failed():
    with world(timeout_s=1) as w:
        w.ready()
        w.set_scenario({"default": {"sleep": 30}})
        rc, out = w.main("pilot", "--run-id", "t1", FAKE_GLIA_FAIL="calibrate")
        assert rc == 0, out
        by = {(r["task_id"][:4], r["arm"]): r["terminated"] for r in w.rows("t1")}
        assert set(by.values()) == {"timeout", "prep_failed"}, by
        assert [k for k, v in by.items() if v == "prep_failed"] == [("toy-", "g053"),
                                                                   ("toy2", "g053")], by
        assert {c["arm"] for c in w.calls()} == {"none", "g052"}



def test_usage_windows_and_line():
    with tempfile.TemporaryDirectory() as t:
        f = Path(t) / "rl.json"
        env = {"GLIA_RATE_LIMITS": str(f)}
        assert run.usage_windows(env) is None
        f.write_text(json.dumps({"updated": 100, "five_hour": {"used_percentage": 10},
                                 "seven_day": {"used_percentage": 60}}))
        before = run.usage_windows(env)
        assert before == (100, 10, 60)
        f.write_text("{not json")
        assert run.usage_windows(env) is None
        home = Path(t) / "home"
        (home / ".claude").mkdir(parents=True)
        (home / ".claude" / "rate-limits.json").write_text(json.dumps(
            {"updated": 200, "five_hour": {"used_percentage": 14},
             "seven_day": {"used_percentage": 62}}))
        after = run.usage_windows({"HOME": str(home)})
        assert after == (200, 14, 62)
    # 6 pilot sessions moved 7-day by 2 points; the full run is 180 sessions
    assert run.usage_line(before, after, 6, 180) == (
        "five_hour_pct=10->14 seven_day_pct=60->62 est_full_seven_day_pct=60 "
        "est_full_weeks=0.6")
    assert run.usage_line(before, before, 6, 180) == \
        "usage=unknown reason=rate_limits_not_refreshed updated=100"
    assert run.usage_line(None, after, 6, 180) == "usage=unknown reason=no_rate_limits_file"


def main():
    failed = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except Exception:
                failed += 1
                print(f"FAIL {name}")
                traceback.print_exc()
    print(f"{'FAILED' if failed else 'passed'}: {failed} failing")
    return 1 if failed else 0


if __name__ == "__main__":
    shutil.which("git") or sys.exit("git is required")
    sys.exit(main())

