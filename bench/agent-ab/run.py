#!/usr/bin/env python3
"""The agent before / after session driver (FE.2): every FE.1 task run as a
headless Claude Code session under three arms (`none`, `g052`, `g053`), each in
its own leak-free checkout of the task commit's parent.

    python3 run.py binaries              # build each glia arm's release binary once
    python3 run.py isolation             # one cheap session per arm, checks the init event
    python3 run.py pilot [--run-id <id>] # first task of each repo x every arm x 1 rep
    python3 run.py run --run-id <id>     # the whole matrix (resumes; budget stop)
    python3 run.py status [--run-id <id>]

FH.13's probe sets (bench/cohort/tasks/agent/<set>/: set.json, repos.json, rows):

    python3 run.py build-set <set dir> [--check]   # materialise / re-derive the rows
    python3 run.py --set <set dir> pilot|run|status [--only <label>,..] ...
    python3 run.py --set <set dir> report --run-id <id> [--run-id ..] [--out-md ..]
                   [--out-json ..] [--out-sessions ..] [--out-probe ..]

Everything is written under ${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab} (the
FE.1 bare clones' directory), never in the glia tree; a set run reads the cohort's
bare clones (${GLIA_COHORT_DIR:-~/.cache/glia-cohort}) and writes its split refs
(refs/agent-ab/split/..) there. Markers on stdout: `[agent-ab] binary ..`,
`[agent-ab] isolation ..`, `[agent-ab] session ..`, `[agent-ab] pilot ..`,
`[agent-ab] budget stop ..`, `[agent-ab] set ..`, `[agent-ab] score ..`,
`[agent-ab] diag verdicts ..`. Stdlib only.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import shlex
import shutil
import signal
import statistics
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tasks  # noqa: E402

HERE = tasks.HERE
GLIA_REPO = HERE.parent.parent
DENIED = ("glia", "mcp-repo-graph", "repo-graph")
GLIA_PACKAGES = ("glia-cli", "glia-py", "glia_py", "mcp-repo-graph")
INSTALLERS = frozenset({"uvx", "pipx", "pip", "pip3", "uv"})
WRAPPERS = frozenset({"env", "time", "command", "exec", "nohup", "sudo"})
OPENERS = frozenset({"cat", "head", "tail", "less", "bat"})
GREPS = frozenset({"rg", "grep", "egrep", "fgrep"})
NET_WORDS = frozenset({"gh", "curl", "wget"})
NET_GIT = frozenset({"clone", "fetch", "pull", "ls-remote"})
GLIA_GROUPS = frozenset({"history", "docs", "timeline", "cell", "cache", "overlay", "scip",
                         "tests", "hook"})
SEPARATORS = frozenset({"|", "||", "&&", ";", "&", "|&", ";;"})
REDIRECTS = frozenset({">", ">>", "<", "<<", "<<<", ">&", "&>", "<&", ">|"})
SKIP_DIRS = frozenset({".glia", ".claude", ".git"})
ASSIGN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
STAMP_RE = re.compile(r"\(build ([^)]+)\)")
RL_PHRASES = ("rate limit", "usage limit", "rate_limit", "resets at", "too many requests", "429",
              "overloaded")
RL_ATTEMPTS, RL_DEFAULT_WAIT_S, RL_MAX_WAIT_S = 5, 900, 6 * 3600
PREP_TIMEOUT_S = 3600
ISOLATION_PROMPT = "Reply with the word ok."
# FH.13's probe sets: where they live, where their rows come from, the split refs
SETS_DIR = GLIA_REPO / "bench" / "cohort" / "tasks" / "agent"
SET_SOURCES = {"cohort": GLIA_REPO / "bench" / "cohort" / "tasks", "agent-ab": HERE / "tasks"}
SET_METRICS = ("missed", "first_edit", "across")
SPLIT_REF, SPLIT_DEPTH = "refs/agent-ab/split/", 400
DRIFT_RE = re.compile(r"^The latest commit on this branch \(HEAD\) changed the server: (.*)\. "
                      r"The (.+) were not updated with it and no longer match the server\. ",
                      re.S)
SPLIT_DRIFT_PROMPT = ("The latest commit in {changed} (beside this repository) changed the "
                      "server: {subject}. The {names} were not updated with it and no longer "
                      "match the server. Update the client code so it works with the server as "
                      "it is now.")
EDIT_TOOLS = frozenset({"Edit", "Write", "MultiEdit"})
VERIFY_SUBS = frozenset({"diff-impact", "contract-breaks", "review"})
PROBE_PAIRS = (("g054", "none"), ("g054", "g053"), ("g053", "none"))
DIAG_VERDICTS = ("no_skill", "skill_only", "glia_missed", "glia_late", "glia_found")
sleep = time.sleep  # the tests replace it


class RunError(Exception):
    pass


class Stop(Exception):
    """A clean, resumable stop (budget or rate limits)."""


# --- small helpers -----------------------------------------------------------
def sh(args, cwd=None, env=None, check=True):
    p = subprocess.run([str(a) for a in args], cwd=cwd, env=env, capture_output=True, text=True)
    if check and p.returncode != 0:
        raise RunError(f"{' '.join(str(a) for a in args[:4])}: {p.stderr.strip()[:400]}")
    return p


def sha256(data):
    return hashlib.sha256(data if isinstance(data, bytes) else data.encode()).hexdigest()


def read_json(path):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return None


def write_json(path, obj):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, indent=1, sort_keys=True) + "\n")


def read_rows(path):
    rows = []
    if Path(path).exists():
        for line in Path(path).read_text().splitlines():
            if line.strip():
                rows.append(json.loads(line))
    return rows


def log(msg):
    print(msg, flush=True)


def load_config(path):
    cfg = json.loads(Path(path).read_text())
    for key in ("model", "claude_version", "runs", "max_turns", "timeout_s", "max_usd", "arms",
                "disallowed_tools", "glia_threads"):
        if key not in cfg:
            raise RunError(f"{path}: missing {key}")
    if "none" not in cfg["arms"] or any(k != "none" and not v.get("rev")
                                        for k, v in cfg["arms"].items()):
        raise RunError(f"{path}: arms must hold `none` and a `rev` for every glia arm")
    return cfg


def glia_arms(cfg):
    return [a for a, spec in cfg["arms"].items() if spec.get("rev")]


def pinned_config(cfg):
    """The config fields a resumed run must not change (max_usd may be raised)."""
    return sha256(json.dumps({k: v for k, v in cfg.items() if k != "max_usd"}, sort_keys=True))


# --- tasks and the prompt ----------------------------------------------------
def load_tasks(repos_path, tasks_dir):
    """[(repo, row)] in file order: repos.json order, then each file's line order."""
    out, shas = [], {}
    for repo in tasks.load_repos(repos_path):
        path = Path(tasks_dir) / f"{repo['label']}.jsonl"
        data = path.read_bytes()
        shas[repo["label"]] = sha256(data)
        out += [(repo, json.loads(line)) for line in data.decode().splitlines() if line.strip()]
    return out, shas


def repo_name(repo):
    m = tasks.GITHUB_RE.match(repo["url"])
    return f"{m.group(1)}/{m.group(2)}" if m else repo["label"]


def task_text(row):
    text = row["prompt_subject"]
    issue = row.get("issue")
    if issue and row.get("issue_fetch") == "ok":
        text += f"\n\nIssue #{issue['number']}: {issue['title']}\n{issue['body']}"
    return text


def render_prompt(template, repo, row, glia_note=None):
    """The session prompt; a glia arm's ends with the config's `glia_prompt` (the first
    pilot's glia arms never called glia, so the arm is glia offered, not glia installed)."""
    text = template.replace("{repo}", repo_name(repo)).replace("{task}", task_text(row))
    return f"{text}\n\n{glia_note}" if glia_note else text


def load_template():
    return (HERE / "prompt.txt").read_text().rstrip("\n")


# --- binaries ----------------------------------------------------------------
def resolve_rev(glia_repo, rev):
    p = sh(["git", "-C", glia_repo, "rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}"],
           check=False)
    if p.returncode != 0:
        raise RunError(f"refused: rev {rev!r} does not resolve to a commit in {glia_repo}")
    return p.stdout.strip()


def build_stamp(version):
    m = STAMP_RE.search(version or "")
    return m.group(1) if m else version


def build_binary(d, glia_repo, arm, rev):
    sha = resolve_rev(glia_repo, rev)
    bindir = d / "bin" / arm
    binary, meta_path = bindir / "glia", bindir / "meta.json"
    meta = read_json(meta_path)
    if not (binary.is_file() and meta and meta.get("sha") == sha):
        wt, target = d / f"glia@{sha[:12]}", d / f"target@{sha[:12]}"
        try:
            if not wt.exists():
                sh(["git", "-C", glia_repo, "worktree", "add", "--detach", "--quiet", wt, sha])
            env = {**os.environ, "CARGO_TARGET_DIR": str(target), "CARGO_INCREMENTAL": "0"}
            p = subprocess.run(["cargo", "build", "--release", "-p", "glia-cli"], cwd=wt, env=env)
            if p.returncode != 0:
                raise RunError(f"arm {arm}: cargo build at {sha[:12]} exited {p.returncode}")
            bindir.mkdir(parents=True, exist_ok=True)
            shutil.copy2(target / "release" / "glia", binary)
        finally:
            shutil.rmtree(target, ignore_errors=True)
            if wt.exists():
                sh(["git", "-C", glia_repo, "worktree", "remove", "--force", wt], check=False)
        version = sh([binary, "--version"]).stdout.strip()
        meta = {"rev": rev, "sha": sha, "version": version}
        write_json(meta_path, meta)
    # the whole skill directory at the arm's rev: SKILL.md and the pages it points to
    skill_dir = d / "skills" / arm / "glia"
    shutil.rmtree(skill_dir, ignore_errors=True)
    skill_dir.mkdir(parents=True, exist_ok=True)
    names = sh(["git", "-C", glia_repo, "ls-tree", "--name-only", f"{sha}:skills/glia"]).stdout
    for name in names.split():
        page = sh(["git", "-C", glia_repo, "show", f"{sha}:skills/glia/{name}"]).stdout
        (skill_dir / name).write_text(page)
    log(f"[agent-ab] binary arm={arm} sha={sha[:12]} version={meta['version']}")
    return meta


def check_stamps(metas):
    seen = {}
    for arm, meta in metas.items():
        stamp = build_stamp(meta["version"])
        if stamp in seen:
            raise RunError(f"refused: arms {seen[stamp]} and {arm} report the same build "
                           f"stamp {stamp}")
        seen[stamp] = arm


def cmd_binaries(a, cfg, d):
    metas = {arm: build_binary(d, a.glia_repo, arm, cfg["arms"][arm]["rev"])
             for arm in glia_arms(cfg)}
    check_stamps(metas)
    return 0


def arm_metas(cfg, d, glia_repo):
    """Each glia arm's {rev, sha, version}, verified against its rev and skill."""
    metas = {}
    for arm in glia_arms(cfg):
        meta = read_json(d / "bin" / arm / "meta.json")
        sha = resolve_rev(glia_repo, cfg["arms"][arm]["rev"])
        if not ((d / "bin" / arm / "glia").is_file() and meta and meta.get("sha") == sha
                and (d / "skills" / arm / "glia" / "SKILL.md").is_file()):
            raise RunError(f"refused: arm {arm} has no binary / skill at {sha[:12]}: "
                           f"run `python3 run.py binaries` first")
        metas[arm] = meta
    check_stamps(metas)
    return metas


# --- the environment ---------------------------------------------------------
def auth_kind(environ):
    """Ruling FE-3: subscription usage only. A set ANTHROPIC_API_KEY (metered billing)
    refuses the run, even beside a token."""
    if environ.get("ANTHROPIC_API_KEY"):
        raise RunError("refused: ANTHROPIC_API_KEY is set; the agent A/B runs on the "
                       "subscription only (ruling FE-3): unset it and set "
                       "CLAUDE_CODE_OAUTH_TOKEN (`claude setup-token`)")
    if environ.get("CLAUDE_CODE_OAUTH_TOKEN"):
        return "oauth_token"
    raise RunError("refused: set CLAUDE_CODE_OAUTH_TOKEN (`claude setup-token`, "
                   "subscription) in the environment")


def usage_windows(environ):
    """The subscription's window shares from the statusline's rate-limits file:
    (updated, five_hour %, seven_day %), or None when it is absent or unreadable."""
    path = Path(environ.get("GLIA_RATE_LIMITS")
                or Path(environ.get("HOME", "~")).expanduser() / ".claude" / "rate-limits.json")
    lim = read_json(path)
    try:
        return (lim["updated"], lim["five_hour"]["used_percentage"],
                lim["seven_day"]["used_percentage"])
    except (TypeError, KeyError):
        return None


def usage_line(before, after, sessions, total):
    """The pilot's window share and its full-run estimate (ruling FE-3). The file is
    rewritten only when an interactive session's statusline renders, so a file not
    refreshed since the pilot started reads `unknown`."""
    if before is None or after is None:
        return "usage=unknown reason=no_rate_limits_file"
    if after[0] <= before[0]:
        return f"usage=unknown reason=rate_limits_not_refreshed updated={after[0]}"
    d5, d7 = after[1] - before[1], after[2] - before[2]
    per = d7 / sessions if sessions else 0.0
    return (f"five_hour_pct={before[1]}->{after[1]} seven_day_pct={before[2]}->{after[2]} "
            f"est_full_seven_day_pct={per * total:.0f} est_full_weeks={per * total / 100:.1f}")


def check_claude_version(cfg):
    p = sh(["claude", "--version"], check=False)
    got = p.stdout.strip().splitlines()[0] if p.stdout.strip() else ""
    if got != cfg["claude_version"]:
        raise RunError(f"refused: `claude --version` is {got!r}, config pins "
                       f"{cfg['claude_version']!r}")


def write_deny(d):
    deny = d / "bin" / "deny"
    deny.mkdir(parents=True, exist_ok=True)
    for name in DENIED:
        p = deny / name
        p.write_text(f'#!/bin/sh\necho "{name}: command not found" >&2\nexit 127\n')
        p.chmod(0o755)
    return deny


def fresh_config_dir(d, run_id):
    path = d / "claude-config" / run_id
    path.mkdir(parents=True, exist_ok=True)
    return path


def session_env(d, cfg, arm, config_dir, base=None):
    """os.environ with the caller's Claude Code session state removed; identical for
    every arm but PATH's first entry."""
    env = dict(os.environ if base is None else base)
    for key in list(env):
        if (key in ("CLAUDECODE", "CLAUDE_CONFIG_DIR", "GLIA_RANK_PRIVATE")
                or (key.startswith("CLAUDE_CODE_") and key != "CLAUDE_CODE_OAUTH_TOKEN")):
            del env[key]
    path = [str(d / "bin" / "deny"), env.get("PATH", "")]
    if cfg["arms"][arm].get("rev"):
        path.insert(0, str(d / "bin" / arm))
    env.update(PATH=os.pathsep.join(path), CLAUDE_CONFIG_DIR=str(config_dir),
               GLIA_THREADS=str(cfg["glia_threads"]))
    return env


# --- the checkout and the glia prep ------------------------------------------
def checkout(d, label, parent, commit, copy, gd=None):
    gd = gd or d / f"{label}.git"
    if not gd.is_dir():
        raise RunError(f"{label}: no bare clone at {gd} (run tasks.py build)")
    fetch_into(gd, parent, copy)
    if sh(["git", "-C", copy, "cat-file", "-e", f"{commit}^{{commit}}"],
          check=False).returncode == 0:
        raise RunError(f"leak: {copy} holds the task commit {commit[:12]}; run aborted")


def fetch_into(gd, rev, copy):
    """A fresh repo at copy whose `main` is rev, fetched alone from the bare clone gd: no
    remote, `.glia/` and `.claude/` excluded."""
    sh(["git", "--git-dir", gd, "config", "uploadpack.allowAnySHA1InWant", "true"])
    if copy.exists():
        shutil.rmtree(copy)
    copy.mkdir(parents=True)
    sh(["git", "init", "-q", copy])
    sh(["git", "-C", copy, "fetch", "-q", "--no-tags", gd, rev])
    sh(["git", "-C", copy, "checkout", "-q", "--detach", "FETCH_HEAD"])
    sh(["git", "-C", copy, "branch", "-q", "-f", "main", "FETCH_HEAD"])
    sh(["git", "-C", copy, "checkout", "-q", "main"])
    with open(copy / ".git" / "info" / "exclude", "a") as f:
        f.write(".glia/\n.claude/\n")


def install_skill(d, arm, copy):
    dst = copy / ".claude" / "skills" / "glia"
    shutil.rmtree(dst, ignore_errors=True)
    shutil.copytree(d / "skills" / arm / "glia", dst)


def prep(d, cfg, arm, copy, env, log_path, roots=None):
    """history sync, build (and calibrate for an arm that sets it) in each repo root
    (the copy, or every part of a split workspace), then the skill into the copy (the
    agent's start); outside the agent's clock. Returns (prep_ms, ok)."""
    steps = [["history", "sync"], ["build"]]
    if cfg["arms"][arm].get("calibrate"):
        steps.append(["history", "calibrate"])
    t0, ok = time.monotonic(), True
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with open(log_path, "wb") as out:
        for root in roots or [copy]:
            for step in steps:
                try:
                    p = subprocess.run([str(d / "bin" / arm / "glia"), *step, str(root)],
                                       cwd=root, env=env, stdout=out, stderr=subprocess.STDOUT,
                                       stdin=subprocess.DEVNULL, timeout=PREP_TIMEOUT_S)
                    ok = p.returncode == 0
                except subprocess.TimeoutExpired:
                    ok = False
                if not ok:
                    break
            if not ok:
                break
    if ok:
        install_skill(d, arm, copy)
    return int((time.monotonic() - t0) * 1000), ok


# --- one Claude Code session -------------------------------------------------
def claude_cmd(cfg, prompt, max_turns, add_dir=None):
    cmd = ["claude", "-p", prompt, "--model", cfg["model"], "--output-format", "stream-json",
           "--verbose", "--mcp-config", str(HERE / "empty-mcp.json"), "--strict-mcp-config",
           "--setting-sources", "project", "--permission-mode", "bypassPermissions",
           "--max-turns", str(max_turns)]
    if cfg.get("disabled_plugins"):
        # Claude Code's own built-in plugins load under any CLAUDE_CONFIG_DIR; the
        # isolation check refuses every plugin, so the run turns them off
        cmd += ["--settings", json.dumps(
            {"enabledPlugins": {p: False for p in cfg["disabled_plugins"]}}, sort_keys=True)]
    if add_dir:  # a split workspace: the sibling repositories, every arm alike
        cmd += ["--add-dir", str(add_dir)]
    if cfg["disallowed_tools"]:
        cmd += ["--disallowedTools", *cfg["disallowed_tools"]]
    return cmd


def launch(cmd, cwd, env, out_path, timeout_s):
    """Stream stdout to out_path; returns (events, returncode, timed_out, wall_ms, stderr)."""
    out_path.parent.mkdir(parents=True, exist_ok=True)
    err_path = out_path.with_suffix(".stderr")
    t0, timed_out = time.monotonic(), False
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        p = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=out, stderr=err,
                             stdin=subprocess.DEVNULL, start_new_session=True)
        try:
            p.wait(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            timed_out = True
            for sig, grace in ((signal.SIGTERM, 10), (signal.SIGKILL, None)):
                try:
                    os.killpg(p.pid, sig)
                except ProcessLookupError:
                    break
                try:
                    p.wait(timeout=grace)
                    break
                except subprocess.TimeoutExpired:
                    continue
    wall_ms = int((time.monotonic() - t0) * 1000)
    events = []
    for line in out_path.read_text(errors="replace").splitlines():
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if isinstance(e, dict):
            events.append(e)
    return events, p.returncode, timed_out, wall_ms, err_path.read_text(errors="replace")


def first(events, type_, subtype=None):
    return next((e for e in events if e.get("type") == type_
                 and (subtype is None or e.get("subtype") == subtype)), None)


def result_event(events):
    return next((e for e in reversed(events) if e.get("type") == "result"), None)


def rate_limited(events, returncode, stderr):
    """(limited, resets_at): a result whose error text reads as a rate limit, or no
    result at all with a rejected rate-limit event / rate-limit stderr."""
    resets = None
    rejected = False
    for e in events:
        if e.get("type") == "rate_limit_event":
            info = e.get("rate_limit_info") or {}
            resets = info.get("resetsAt") or resets
            rejected = rejected or info.get("status") == "rejected"
    looks = lambda t: any(p in (t or "").lower() for p in RL_PHRASES)  # noqa: E731
    result = result_event(events)
    if result is not None:
        return bool(result.get("is_error") and looks(str(result.get("result")))), resets
    return bool(returncode != 0 and (rejected or looks(stderr))), resets


def run_with_waits(cmd, cwd, env, out_path, timeout_s, reset=None):
    """launch, sleeping out rate limits (until the reset, else 900 s; at most 6 h) for
    up to RL_ATTEMPTS attempts; raises Stop when every attempt was rate limited."""
    for attempt in range(1, RL_ATTEMPTS + 1):
        events, rc, timed_out, wall_ms, stderr = launch(cmd, cwd, env, out_path, timeout_s)
        limited, resets = rate_limited(events, rc, stderr)
        if not limited:
            return events, rc, timed_out, wall_ms, attempt
        now = time.time()
        wait = (resets - now + 30) if isinstance(resets, (int, float)) and resets > now \
            else RL_DEFAULT_WAIT_S
        wait = max(60, min(wait, RL_MAX_WAIT_S))
        log(f"[agent-ab] rate limited attempt={attempt}/{RL_ATTEMPTS} sleep_s={int(wait)}")
        if attempt == RL_ATTEMPTS:
            break
        sleep(wait)
        if reset:
            reset()
    raise Stop(f"rate limited {RL_ATTEMPTS} times")


# --- reading the transcript --------------------------------------------------
def tool_uses(events):
    for e in events:
        if e.get("type") != "assistant":
            continue
        for b in (e.get("message") or {}).get("content") or []:
            if isinstance(b, dict) and b.get("type") == "tool_use":
                yield b.get("name") or "", b.get("input") or {}


def _lines(command):
    """Split on newlines outside quotes."""
    out, cur, quote, esc = [], [], None, False
    for ch in command:
        if esc:
            cur.append(ch)
            esc = False
        elif ch == "\\" and quote != "'":
            cur.append(ch)
            esc = True
        elif quote:
            cur.append(ch)
            quote = None if ch == quote else quote
        elif ch in "'\"":
            cur.append(ch)
            quote = ch
        elif ch == "\n":
            out.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    out.append("".join(cur))
    return out


def _tokens(line):
    try:
        lex = shlex.shlex(line, posix=True, punctuation_chars=True)
        lex.whitespace_split = True
        return list(lex)
    except ValueError:
        return re.findall(r"\|\||&&|[|;&]|[^\s|;&]+", line)


def segments(command):
    """[(word, args)] per simple command: `VAR=v` prefixes, wrappers (env, time, ..),
    subshell parentheses and redirections dropped."""
    out = []
    for line in _lines(command or ""):
        cur = []
        for tok in _tokens(line) + [";"]:
            if tok in SEPARATORS:
                words = []
                skip = False
                for t in cur:
                    if skip:
                        skip = False
                    elif t in REDIRECTS or (t and set(t) <= set("<>&|") and t not in ("(", ")")):
                        skip = True
                    elif t not in ("(", ")", "{", "}", "!"):
                        words.append(t)
                while words and (ASSIGN_RE.match(words[0]) or words[0] in WRAPPERS):
                    words.pop(0)
                if words:
                    out.append((words[0], words[1:]))
                cur = []
            else:
                cur.append(tok)
    return out


def bash_commands(events):
    return [str(inp.get("command") or "") for name, inp in tool_uses(events) if name == "Bash"]


def _installs_glia(word, args):
    named = any(p in a for a in args for p in GLIA_PACKAGES)
    return named and (word in INSTALLERS or any(a in INSTALLERS for a in args[:3]))


def _git_sub(args):
    i = 0
    while i < len(args) and args[i].startswith("-"):
        i += 2 if args[i] in ("-C", "-c", "--git-dir", "--work-tree", "--namespace") else 1
    return args[i:]


def transcript_violations(events, glia_arm):
    """(contaminated reasons, network reasons) from the session's tool calls."""
    contaminated, network = [], []
    for name, _ in tool_uses(events):
        if name in ("WebFetch", "WebSearch"):
            network.append(name)
    for command in bash_commands(events):
        for word, args in segments(command):
            base = os.path.basename(word)
            if not glia_arm and (word in DENIED or ("/" in word and base in DENIED)):
                contaminated.append(f"control ran {word}")
            if glia_arm and word != "glia" and "/" in word and base == "glia":
                contaminated.append(f"glia arm ran {word}")
            if glia_arm and word == "cargo" and args[:1] == ["run"]:
                contaminated.append("glia arm ran cargo run")
            if _installs_glia(word, args):
                contaminated.append(f"{word} {' '.join(args[:4])}")
            sub = _git_sub(args) if word == "git" else []
            if word in NET_WORDS or (sub[:1] and sub[0] in NET_GIT) or sub[:2] == ["remote", "add"]:
                network.append(" ".join([word, *args[:3]]))
    return contaminated, network


def init_violations(init, glia_arm, disallowed, user_skills):
    if init is None:
        return ["no init event"]
    v = []
    if init.get("mcp_servers"):
        v.append(f"mcp_servers={[s.get('name') for s in init['mcp_servers']]}")
    tools = init.get("tools") or []
    v += [f"tool {t}" for t in tools if t.startswith("mcp__") or t in disallowed]
    if init.get("plugins"):
        v.append(f"plugins={init['plugins']}")
    skills = init.get("skills") or []
    if glia_arm and "glia" not in skills:
        v.append("glia skill missing")
    if not glia_arm:
        v += [f"skill {s}" for s in skills if s.split(":")[-1].startswith("glia")]
    v += [f"plugin skill {s}" for s in skills if ":" in s]
    leaked = sorted(set(skills) & set(user_skills) - ({"glia"} if glia_arm else set()))
    v += [f"user skill {s}" for s in leaked]
    return v


def user_skill_names():
    root = Path.home() / ".claude" / "skills"
    return sorted(p.name for p in root.iterdir() if p.is_dir()) if root.is_dir() else []


def _rel(copy, cwd, path, depth=0):
    """path, repo-relative to copy, or None outside it or under a skipped dir; depth 1
    reads a split workspace, whose first component is the part (the repo)."""
    if not path or path.startswith("-"):
        return None
    ap = os.path.normpath(os.path.join(cwd, os.path.expanduser(path)))
    rp = os.path.relpath(ap, copy)
    if rp == "." or rp.startswith(".."):
        return None
    if SKIP_DIRS & set(rp.split(os.sep)[:depth + 1]):
        return None
    return rp.replace(os.sep, "/")


def _open_args(word, args):
    """The file arguments of cat / head / tail / less / bat / sed -n."""
    if word == "sed":
        if "-n" not in args:
            return []
        rest = [a for a in args if not a.startswith("-")]
        return rest if any(a.startswith("-e") for a in args) else rest[1:]
    files, skip = [], False
    for a in args:
        if skip:
            skip = False
        elif a in ("-n", "-c", "--lines", "--bytes", "-r", "--line-range", "-l", "--language"):
            skip = True
        elif not a.startswith("-") and not a.lstrip("+").isdigit():
            files.append(a)
    return files


def tracked_files(copy, parts=None):
    """The committed paths of the copy, or of every part of a split workspace as
    `<part>/<path>`."""
    if not parts:
        return set(sh(["git", "-C", copy, "ls-tree", "-r", "--name-only", "HEAD"],
                      check=False).stdout.splitlines())
    return {f"{name}/{p}" for name in parts
            for p in sh(["git", "-C", Path(copy) / name, "ls-tree", "-r", "--name-only",
                         "HEAD"], check=False).stdout.splitlines()}


def session_metrics(events, copy, start=None, parts=None):
    """files_opened (repo-relative; a split workspace's as `<part>/<path>`, the agent
    starting in part `start`), tool / glia / grep calls."""
    copy = os.path.realpath(copy)
    home = os.path.join(copy, start) if start else copy
    depth = 1 if parts else 0
    tracked = None
    opened, tool_calls, glia_calls, grep_calls = [], Counter(), [], 0

    def add(rel):
        if rel and rel not in opened:
            opened.append(rel)

    for name, inp in tool_uses(events):
        tool_calls[name] += 1
        if name == "Grep":
            grep_calls += 1
        elif name == "Read":
            add(_rel(copy, home, str(inp.get("file_path") or ""), depth))
        elif name == "Bash":
            cwd = home
            for word, args in segments(str(inp.get("command") or "")):
                if word == "cd" and args:
                    cwd = os.path.normpath(os.path.join(cwd, os.path.expanduser(args[0])))
                elif word in GREPS or (word == "git" and _git_sub(args)[:1] == ["grep"]):
                    grep_calls += 1
                elif word == "glia":
                    pos = [a for a in args if not a.startswith("-")]
                    sub = pos[:2] if pos[:1] and pos[0] in GLIA_GROUPS else pos[:1]
                    flags = []
                    for a in args:
                        f = a.split("=", 1)[0]
                        if a.startswith("-") and f not in flags:
                            flags.append(f)
                    glia_calls.append({"subcommand": " ".join(sub), "flags": flags})
                elif word in OPENERS or word == "sed":
                    for f in _open_args(word, args):
                        rel = _rel(copy, cwd, f, depth)
                        if rel is None:
                            continue
                        if tracked is None:
                            tracked = tracked_files(copy, parts)
                        if os.path.isfile(os.path.join(copy, rel)) or rel in tracked:
                            add(rel)
    return {"files_opened": opened, "tool_calls": dict(sorted(tool_calls.items())),
            "glia_calls": glia_calls, "grep_calls": grep_calls}


def write_patch(copy, path, parts=None):
    """The agent's change as one -U0 diff against HEAD; a split workspace's is every
    part's diff in part order, its paths prefixed `<part>/`."""
    diff = b""
    for name in parts or [None]:
        repo = Path(copy) / name if name else Path(copy)
        prefix = f"{name}/" if name else ""
        sh(["git", "-C", repo, "add", "-A", "-N", "."], check=False)
        diff += subprocess.run(["git", "-C", str(repo), "diff", "--no-color", "-U0",
                                f"--src-prefix=a/{prefix}", f"--dst-prefix=b/{prefix}", "HEAD"],
                               capture_output=True).stdout
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(diff)
    return {"path": str(path), "sha256": sha256(diff), "bytes": len(diff)}


def reset_copy(copy, parts=None):
    for name in parts or [None]:
        repo = Path(copy) / name if name else Path(copy)
        sh(["git", "-C", repo, "reset", "-q", "--hard"], check=False)
        sh(["git", "-C", repo, "clean", "-fdq"], check=False)


# --- probe sets (FH.13) --------------------------------------------------------
def _module(name, path):
    """A sibling harness module by path (the cohort runner, the frozen diag probe),
    loaded once and only when a set needs it."""
    if name not in sys.modules:
        spec = importlib.util.spec_from_file_location(name, path)
        mod = importlib.util.module_from_spec(spec)
        sys.modules[name] = mod
        spec.loader.exec_module(mod)
    return sys.modules[name]


def cohort_mod():
    return _module("cohort", GLIA_REPO / "bench" / "cohort" / "cohort.py")


def load_set(set_dir):
    """set.json: name, metric, tasks [{from, id}], note / glia_note (prompt text with
    {base} {start} {repos} {with}), workspace {label: {parts: {name: prefix}, rest}}."""
    path = Path(set_dir) / "set.json"
    if not path.is_file():
        raise RunError(f"{set_dir}: no set.json")
    data = path.read_bytes()
    spec = json.loads(data)
    for key in ("name", "metric", "tasks"):
        if key not in spec:
            raise RunError(f"{path}: missing {key}")
    if spec["metric"] not in SET_METRICS:
        raise RunError(f"{path}: metric {spec['metric']!r} is none of {', '.join(SET_METRICS)}")
    for t in spec["tasks"]:
        if t.get("from") not in SET_SOURCES or not t.get("id"):
            raise RunError(f"{path}: a task needs `from` ({', '.join(SET_SOURCES)}) and `id`")
    for label, ws in (spec.get("workspace") or {}).items():
        if not ws.get("parts") or not ws.get("rest") or ws["rest"] in ws["parts"] \
                or any(not p.endswith("/") for p in ws["parts"].values()):
            raise RunError(f"{path}: workspace {label} needs parts (prefixes ending in /) "
                           "and a rest name that is no part")
    return {**spec, "dir": Path(set_dir), "sha256": sha256(data)}


def part_of(path, ws):
    """(part, path inside it): the longest part prefix, else the rest part."""
    for name, prefix in sorted(ws["parts"].items(), key=lambda kv: (-len(kv[1]), kv[0])):
        if path.startswith(prefix):
            return name, path[len(prefix):]
    return ws["rest"], path


def ws_path(path, ws):
    if path is None:
        return None
    name, rel = part_of(path, ws)
    return f"{name}/{rel}"


def _listing(paths, sep=", ", last=" and "):
    paths = list(paths)
    return paths[0] if len(paths) == 1 else sep.join(paths[:-1]) + last + paths[-1]


def split_row(row, ws):
    """A monorepo task row in split-workspace coordinates: every truth / probe path is
    `<part>/<path>`, the agent starts in the part holding most truth code files (ties
    by name), and a drift prompt names the repositories the server change is in."""
    out = json.loads(json.dumps(row))
    for c in out["truth"]["code"]:
        c["path"], c["old_path"] = ws_path(c["path"], ws), ws_path(c.get("old_path"), ws)
    for key in ("tests", "other"):
        out["truth"][key] = sorted(ws_path(p, ws) for p in out["truth"][key])
    probe = out.get("probe") or {}
    for key in ("client_files", "server_files"):
        if key in probe:
            probe[key] = sorted(ws_path(p, ws) for p in probe[key])
    counts = Counter(c["path"].split("/", 1)[0] for c in out["truth"]["code"])
    start = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]
    changed = sorted({p.split("/", 1)[0] for p in probe.get("server_files", [])} - {start})
    out["workspace"] = {"parts": ws["parts"], "rest": ws["rest"], "start": start,
                        "changed": changed}
    if out.get("shape") == "drift":
        m = DRIFT_RE.match(out["prompt_subject"])
        if not m or not changed:
            raise RunError(f"{row['id']}: a split drift row needs the drift prompt and a "
                           "server part other than the start")
        out["prompt_subject"] = SPLIT_DRIFT_PROMPT.format(
            changed=_listing(f"../{n}" for n in changed), subject=m.group(1), names=m.group(2))
    return out


def source_row(sources, src, tid):
    label = tid.rsplit("-", 1)[0]
    path = Path(sources[src]) / f"{label}.jsonl"
    for row in read_rows(path):
        if row["id"] == tid:
            return label, row
    raise RunError(f"{tid}: not in {path}")


def set_repo_row(label, sources):
    """The set's repos.json row: the agent-ab row when FE.1 has the repo (its end_rev and
    tuned flag), else the cohort row in agent-ab's schema."""
    ab = {r["label"]: r for r in tasks.load_repos(Path(sources["agent-ab"]).parent / "repos.json")}
    if label in ab:
        return ab[label]
    co = {r["label"]: r for r in cohort_mod().load_repos(
        Path(sources["cohort"]).parent / "repos.json")}
    if label not in co:
        raise RunError(f"{label}: in neither repos.json")
    r = co[label]
    return {"label": label, "url": r["url"], "end_rev": r["sha"], "lang": r["lang"],
            "gate_rev": None, "tuned": False, "public": True}


def build_set(set_dir, check=False, sources=None):
    """Write (or, with check, compare) the set's repos.json and <label>.jsonl from its
    set.json and the source rows. Returns 0, or 1 on a difference under check."""
    spec, sources = load_set(set_dir), sources or SET_SOURCES
    rows = {}
    for t in spec["tasks"]:
        label, row = source_row(sources, t["from"], t["id"])
        ws = (spec.get("workspace") or {}).get(label)
        rows.setdefault(label, []).append(split_row(row, ws) if ws else row)
    files = {"repos.json": tasks.render_repos([set_repo_row(lb, sources) for lb in rows])}
    for label, rs in rows.items():
        files[f"{label}.jsonl"] = "".join(json.dumps(r, sort_keys=True) + "\n" for r in rs)
    stale = sorted(p.name for p in Path(set_dir).glob("*.jsonl") if p.name not in files)
    diff = [n for n, text in files.items()
            if not (Path(set_dir) / n).is_file() or (Path(set_dir) / n).read_text() != text]
    if check:
        for n in diff + stale:
            log(f"[agent-ab] set {spec['name']} differs: {n}")
    else:
        for n in diff:
            (Path(set_dir) / n).write_text(files[n])
        for n in stale:
            (Path(set_dir) / n).unlink()
    n = sum(len(rs) for rs in rows.values())
    log(f"[agent-ab] set name={spec['name']} metric={spec['metric']} tasks={n} "
        f"repos={len(rows)} split={sum(1 for rs in rows.values() for r in rs if 'workspace' in r)}"
        f" sha256={sha256(''.join(files[k] for k in sorted(files)).encode())[:12]}")
    return 1 if check and (diff or stale) else 0


def fill_note(text, row):
    ws = row.get("workspace")
    names = sorted(set(ws["parts"]) | {ws["rest"]}) if ws else []
    others = [n for n in names if ws and n != ws["start"]]
    return (text.replace("{base}", "HEAD~1" if row.get("shape") == "drift" else "HEAD")
            .replace("{start}", ws["start"] if ws else ".")
            .replace("{repos}", _listing(f"../{n}" for n in names) if names else ".")
            .replace("{with}", " ".join(f"--with ../{n}" for n in others)))


def session_prompt(template, cfg, spec, repo, row, glia_arm):
    """The task prompt, the set's note (every arm), then for a glia arm the config's
    glia_prompt and the set's glia_note. With no set it is FE.2's prompt."""
    notes = [fill_note(spec["note"], row)] if spec and spec.get("note") else []
    if glia_arm:
        notes += [cfg["glia_prompt"]] if cfg.get("glia_prompt") else []
        notes += [fill_note(spec["glia_note"], row)] if spec and spec.get("glia_note") else []
    return "\n\n".join([render_prompt(template, repo, row)] + notes)


def git_out(gd, *args, env=None, inp=None, check=True):
    p = subprocess.run(["git", "--git-dir", str(gd), "-c", "commit.gpgSign=false", *args],
                       capture_output=True, input=inp, env={**os.environ, "LC_ALL": "C",
                                                           **(env or {})})
    if check and p.returncode:
        raise RunError(f"git {' '.join(args[:2])}: {p.stderr.decode(errors='replace').strip()}")
    return p.stdout


def tree_without(gd, tree, excludes, memo):
    """tree with every excluded path (`a/b/`) removed; an emptied subtree is dropped."""
    key = (tree, tuple(sorted(excludes)))
    if key in memo:
        return memo[key]
    heads = {}
    for e in excludes:
        first, _, rest = e.strip("/").partition("/")
        heads.setdefault(first, []).append(rest)
    entries = []
    for ent in git_out(gd, "ls-tree", "-z", tree).split(b"\0"):
        if not ent:
            continue
        meta, name = ent.split(b"\t", 1)
        mode, kind, sha = meta.decode().split()
        subs = heads.get(name.decode(errors="surrogateescape"))
        if subs is not None:
            if "" in subs:
                continue
            if kind == "tree":
                sha = tree_without(gd, sha, subs, memo)
                if sha is None:
                    continue
        entries.append(f"{mode} {kind} {sha}\t".encode() + name)
    out = git_out(gd, "mktree", "-z", inp=b"".join(e + b"\0" for e in entries)).decode().strip() \
        if entries else None
    memo[key] = out
    return out


def part_tree(gd, rev, prefix, exclude, memo):
    """The part's tree at rev: the prefix's subtree, or for the rest part the root minus
    every part prefix. None when it does not exist there."""
    if prefix:
        p = git_out(gd, "rev-parse", "--verify", "-q", f"{rev}:{prefix.rstrip('/')}",
                    check=False).decode().strip()
        return p if p and git_out(gd, "cat-file", "-t", p).decode().strip() == "tree" else None
    root = git_out(gd, "rev-parse", f"{rev}^{{tree}}").decode().strip()
    return tree_without(gd, root, exclude, memo)


def split_ref(gd, task_id, name, rev, prefix, exclude):
    """The part's history as its own repo: the last SPLIT_DEPTH first-parent commits of
    rev touching it, each re-committed with the part's tree and the original author,
    date and message (committer = author), chained oldest first; the tip's tree is the
    part at rev. Written once to refs/agent-ab/split/<task>/<part> (deterministic, so a
    rebuild gives the same sha). None when the part does not exist at rev."""
    ref = f"{SPLIT_REF}{task_id}/{name}"
    have = git_out(gd, "rev-parse", "--verify", "-q", ref, check=False).decode().strip()
    if have:
        return have
    memo = {}
    want = part_tree(gd, rev, prefix, exclude, memo)
    if want is None:
        return None
    spec = [prefix] if prefix else ["."] + [f":(exclude){e}" for e in exclude]
    log_ = git_out(gd, "log", "-z", "--first-parent", f"--max-count={SPLIT_DEPTH}",
                   "--date=raw", "--format=%H%x01%an%x01%ae%x01%ad%x01%B", rev, "--", *spec)
    commits = [c.split(b"\x01", 4) for c in log_.split(b"\0") if c.strip()]
    if not commits or commits[0][0].decode() != git_out(gd, "rev-parse", rev).decode().strip():
        head = git_out(gd, "log", "-z", "-1", "--date=raw",
                       "--format=%H%x01%an%x01%ae%x01%ad%x01%B", rev)
        commits.insert(0, head.split(b"\0")[0].split(b"\x01", 4))
    trees = {}
    if prefix:  # one batch lookup of the prefix's subtree in every commit
        query = "".join(f"{c[0].decode()}:{prefix.rstrip('/')}\n" for c in commits).encode()
        for c, line in zip(commits, git_out(gd, "cat-file", "--batch-check=%(objectname) "
                                            "%(objecttype)", inp=query).decode().splitlines()):
            oid, _, kind = line.partition(" ")
            trees[c[0]] = oid if kind == "tree" else None
    tip, last = None, None
    for c in reversed(commits):
        sha, an, ae, ad, msg = c
        tree = trees[sha] if prefix else part_tree(gd, sha.decode(), prefix, exclude, memo)
        if tree is None or tree == last:
            continue
        who = {"GIT_AUTHOR_NAME": an.decode(errors="replace"),
               "GIT_AUTHOR_EMAIL": ae.decode(errors="replace"),
               "GIT_AUTHOR_DATE": ad.decode(), "GIT_COMMITTER_NAME": an.decode(errors="replace"),
               "GIT_COMMITTER_EMAIL": ae.decode(errors="replace"),
               "GIT_COMMITTER_DATE": ad.decode()}
        tip = git_out(gd, "commit-tree", tree, *(["-p", tip] if tip else []), env=who,
                      inp=msg).decode().strip()
        last = tree
    if last != want:
        raise RunError(f"{task_id}: split {name} ends at another tree than {rev[:12]}'s")
    git_out(gd, "update-ref", ref, tip)
    return tip


def checkout_workspace(gd, row, rev, copy):
    """Every part of a split row's monorepo at rev, each its own repo with its own history
    under copy/<part>/; returns the part names checked out (sorted)."""
    ws = row["workspace"]
    if copy.exists():
        shutil.rmtree(copy)
    copy.mkdir(parents=True)
    names = []
    for name in sorted(set(ws["parts"]) | {ws["rest"]}):
        prefix = ws["parts"].get(name)
        exclude = [] if prefix else sorted(ws["parts"].values())
        tip = split_ref(gd, row["id"], name, rev, prefix, exclude)
        if tip is None:
            continue
        fetch_into(gd, tip, copy / name)
        if sh(["git", "-C", copy / name, "cat-file", "-e", f"{row['commit']}^{{commit}}"],
              check=False).returncode == 0:
            raise RunError(f"leak: {copy / name} holds the task commit; run aborted")
        names.append(name)
    if ws["start"] not in names:
        raise RunError(f"{row['id']}: the start part {ws['start']} does not exist at {rev[:12]}")
    return names


def set_clone(label):
    gd = cohort_mod().cohort_dir() / f"{label}.git"
    if not gd.is_dir():
        raise RunError(f"{label}: no cohort clone at {gd} (run bench/cohort/cohort.py "
                       "build-tasks first)")
    return gd


def start_rev(gd, row):
    """A set row's start: a drift row's synthetic S (rebuilt into the cohort clone when its
    ref is missing), else its parent."""
    if row.get("shape") == "drift":
        try:
            return cohort_mod().checkout_rev(gd, row)
        except cohort_mod().CohortError as e:
            raise RunError(f"{row['id']}: {e}") from e
    return row["parent"]


# --- the run -----------------------------------------------------------------
def schedule(task_list, arms, runs, only=None):
    """(repo, row, arm, rep): rep, then task in file order, then arms rotated by the
    task's index so time-of-day drift spreads over arms."""
    out = []
    for rep in range(1, runs + 1):
        for i, (repo, row) in enumerate(task_list):
            if only is not None and row["id"] not in only:
                continue
            k = i % len(arms)
            out += [(repo, row, arm, rep) for arm in arms[k:] + arms[:k]]
    return out


def key_of(row):
    return f"{row['task_id']}.{row['arm']}.{row['rep']}"


def empty_row():
    return {"num_turns": 0, "duration_ms": 0, "wall_ms": 0, "total_cost_usd": 0.0,
            "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0,
                      "cache_read_input_tokens": 0, "output_tokens": 0},
            "tool_calls": {}, "glia_calls": [], "files_opened": [], "grep_calls": 0,
            "patch": None, "model": None, "violations": [], "attempts": 0}


def terminated_from(result, timed_out):
    if timed_out:
        return "timeout"
    if result is None:
        return "error"
    if result.get("subtype") == "error_max_turns":
        return "max_turns"
    if result.get("subtype") == "success" and not result.get("is_error"):
        return "end"
    return "error"


def one_session(ctx, repo, task, arm, rep):
    d, cfg, run_id = ctx["dir"], ctx["cfg"], ctx["run_id"]
    glia_arm = bool(cfg["arms"][arm].get("rev"))
    key = f"{task['id']}.{arm}.{rep}"
    copy = (d / "work" / run_id / task["id"] / arm / str(rep)).resolve()
    row = {"run_id": run_id, "task_id": task["id"], "repo": repo["label"], "arm": arm,
           "rep": rep, "claude_version": cfg["claude_version"], "auth": ctx["auth"],
           "glia_sha": ctx["metas"][arm]["sha"] if glia_arm else None, "prep_ms": 0,
           **empty_row()}
    env = session_env(d, cfg, arm, ctx["config_dir"])
    spec, parts, start = ctx.get("set"), None, None
    try:
        if spec is None:
            checkout(d, repo["label"], task["parent"], task["commit"], copy)
        else:
            gd = set_clone(repo["label"])
            rev = start_rev(gd, task)
            if task.get("workspace"):
                parts = checkout_workspace(gd, task, rev, copy)
                start = task["workspace"]["start"]
            else:
                checkout(d, repo["label"], rev, task["commit"], copy, gd=gd)
        home = copy / start if start else copy
        if glia_arm:
            prep_log = d / "sessions" / run_id / task["id"] / arm / f"{rep}.prep.log"
            row["prep_ms"], ok = prep(d, cfg, arm, home, env, prep_log,
                                      roots=[copy / n for n in parts] if parts else None)
            if not ok:
                row["terminated"] = "prep_failed"
                return row
        prompt = session_prompt(ctx["template"], cfg, spec, repo, task, glia_arm)
        out = d / "sessions" / run_id / task["id"] / arm / f"{rep}.jsonl"
        events, _, timed_out, wall_ms, attempts = run_with_waits(
            claude_cmd(cfg, prompt, cfg["max_turns"], add_dir=copy if parts else None), home,
            env, out, cfg["timeout_s"], reset=lambda: reset_copy(copy, parts))
        init, result = first(events, "system", "init"), result_event(events)
        row.update(wall_ms=wall_ms, attempts=attempts, model=(init or {}).get("model"))
        if result is not None:
            usage = result.get("usage") or {}
            row.update(num_turns=result.get("num_turns") or 0,
                       duration_ms=result.get("duration_ms") or 0,
                       total_cost_usd=float(result.get("total_cost_usd") or 0.0),
                       usage={k: int(usage.get(k) or 0) for k in row["usage"]})
        row.update(session_metrics(events, copy, start, parts))
        row["patch"] = write_patch(copy, d / "patches" / run_id / f"{key}.diff", parts)
        bad_init = init_violations(init, glia_arm, cfg["disallowed_tools"], ctx["user_skills"])
        contaminated, network = transcript_violations(events, glia_arm)
        row["violations"] = bad_init + contaminated + network
        row["terminated"] = ("contaminated" if bad_init or contaminated else
                             "network" if network else terminated_from(result, timed_out))
        return row
    finally:
        if not ctx["keep"] and copy.exists():
            shutil.rmtree(copy, ignore_errors=True)


def open_run(a, cfg, d, run_id):
    check_claude_version(cfg)
    auth = auth_kind(os.environ)
    metas = arm_metas(cfg, d, a.glia_repo)
    iso = read_json(d / "isolation.json") or {}
    if not (iso.get("ok") and iso.get("claude_version") == cfg["claude_version"]
            and iso.get("model") == cfg["model"]
            and iso.get("arms") == {arm: m["sha"] for arm, m in metas.items()}):
        raise RunError("refused: no passing `python3 run.py isolation` for this claude "
                       "version, model and these binaries")
    task_list, shas = load_tasks(a.repos, a.tasks_dir)
    spec = load_set(a.set) if a.set else None
    if getattr(a, "only", None):
        keep = set(a.only.split(","))
        unknown = sorted(keep - set(shas))
        if unknown:
            raise RunError(f"--only names no repo of the task set: {', '.join(unknown)}")
        task_list = [(r, t) for r, t in task_list if r["label"] in keep]
        shas = {k: v for k, v in shas.items() if k in keep}
    template = load_template()
    pins = {"config": pinned_config(cfg), "prompt": sha256(template), "tasks": shas,
            "claude_version": cfg["claude_version"], "model": cfg["model"],
            "arms": {arm: {"sha": m["sha"], "version": m["version"]} for arm, m in metas.items()}}
    if spec is not None:
        pins["set"] = spec["sha256"]
    run_dir = d / "runs" / run_id
    meta = read_json(run_dir / "meta.json")
    if meta is None:
        write_json(run_dir / "meta.json", pins)
    elif meta != pins:
        changed = sorted(k for k in pins if pins[k] != meta.get(k))
        raise RunError(f"refused: run {run_id} was started with other pins ({', '.join(changed)})")
    write_deny(d)
    log(f"[agent-ab] run id={run_id} auth={auth} tasks={len(task_list)} arms={len(cfg['arms'])}")
    ctx = {"dir": d, "cfg": cfg, "run_id": run_id, "auth": auth, "metas": metas,
           "template": template, "config_dir": fresh_config_dir(d, run_id), "set": spec,
           "user_skills": user_skill_names(), "keep": a.keep,
           "rows": run_dir / "sessions.jsonl"}
    return ctx, task_list


def drive(ctx, plan, max_usd):
    done = {key_of(r) for r in read_rows(ctx["rows"])}
    for repo, task, arm, rep in plan:
        if f"{task['id']}.{arm}.{rep}" in done:
            continue
        rows = read_rows(ctx["rows"])
        costs = [r["total_cost_usd"] for r in rows]
        spent = sum(costs)
        if spent + (statistics.median(costs) if costs else 0.0) > max_usd:
            log(f"[agent-ab] budget stop spent={spent:.2f}")
            return
        row = one_session(ctx, repo, task, arm, rep)
        with open(ctx["rows"], "a") as f:
            f.write(json.dumps(row, sort_keys=True) + "\n")
        log(f"[agent-ab] session task={task['id']} arm={arm} rep={rep} turns={row['num_turns']} "
            f"usd={row['total_cost_usd']:.4f} opened={len(row['files_opened'])} "
            f"glia_calls={len(row['glia_calls'])} terminated={row['terminated']}")


def cmd_run(a, cfg, d, pilot=False):
    run_id = a.run_id or time.strftime("pilot-%Y%m%d-%H%M%S")
    ctx, task_list = open_run(a, cfg, d, run_id)
    arms = list(cfg["arms"])
    only = None
    if pilot:
        only, seen = set(), set()
        for repo, task in task_list:
            if repo["label"] not in seen:
                seen.add(repo["label"])
                only.add(task["id"])
    plan = schedule(task_list, arms, 1 if pilot else cfg["runs"], only)
    max_usd = cfg["max_usd"] if a.max_usd is None else a.max_usd
    usage_before = usage_windows(os.environ)
    try:
        drive(ctx, plan, max_usd)
    except Stop as e:
        log(f"[agent-ab] rate-limit stop: {e}; resume with `python3 run.py run --run-id {run_id}`")
        return 3
    if pilot:
        rows = [r for r in read_rows(ctx["rows"]) if r["rep"] == 1 and r["task_id"] in only]
        costs = [r["total_cost_usd"] for r in rows]
        walls = [r["wall_ms"] + r["prep_ms"] for r in rows]
        med = statistics.median(costs) if costs else 0.0
        total = len(task_list) * len(arms) * cfg["runs"]
        hours = (statistics.median(walls) if walls else 0) * total / 3.6e6
        log(f"[agent-ab] pilot sessions={len(rows)} usd={sum(costs):.2f} median_usd={med:.4f} "
            f"est_full_usd={med * total:.2f} est_hours={hours:.1f} "
            f"{usage_line(usage_before, usage_windows(os.environ), len(rows), total)}")
        log(f"[agent-ab] full run: python3 run.py run --run-id {run_id}")
    return 0


# --- the probe report (FH.13) ----------------------------------------------------
def probe_metrics(events, task, patch_text, copy, score):
    """What a probe set reads beyond score.py, from one session's transcript and patch:
    truth code files missed (not in the patch), the turn and the cumulative input tokens
    at the first Edit / Write / MultiEdit of each truth file (a file changed only through
    Bash is not counted as edited here; the patch still counts it for `missed`), and
    whether a verifying glia command (diff-impact, contract-breaks, review) ran after
    the agent's first edit."""
    ws = task.get("workspace")
    depth, home = (1, os.path.join(copy, ws["start"])) if ws else (0, copy)
    truth = {c["old_path"] if c["status"] == "D" and c.get("old_path") else c["path"]
             for c in task["truth"]["code"]}
    edited = {p for p in score.parse_patch(patch_text) if score.counted(p)}
    turn = tokens = 0
    first, first_edit, verified = {}, None, False
    for e in events:
        if e.get("type") != "assistant":
            continue
        turn += 1
        u = (e.get("message") or {}).get("usage") or {}
        tokens += sum(int(u.get(k) or 0) for k in ("input_tokens", "cache_read_input_tokens",
                                                     "cache_creation_input_tokens"))
        for b in (e.get("message") or {}).get("content") or []:
            if not isinstance(b, dict) or b.get("type") != "tool_use":
                continue
            inp = b.get("input") or {}
            if b.get("name") in EDIT_TOOLS:
                rel = _rel(copy, home, str(inp.get("file_path") or ""), depth)
                if rel:
                    first_edit = first_edit or turn
                    if rel in truth:
                        first.setdefault(rel, (turn, tokens))
            elif b.get("name") == "Bash" and first_edit is not None:
                for word, args in segments(str(inp.get("command") or "")):
                    pos = [x for x in args if not x.startswith("-")]
                    if word == "glia" and pos[:1] and pos[0] in VERIFY_SUBS:
                        verified = True
    done = len(first) == len(truth)
    return {"truth": len(truth), "missed": len(truth - edited), "edited_truth": len(first),
            "all_edited": done,
            "all_edited_turn": max(t for t, _ in first.values()) if done else None,
            "all_edited_tokens": max(k for _, k in first.values()) if done else None,
            "first_edit_turn_mean": (sum(t for t, _ in first.values()) / len(first)
                                     if first else None),
            "verified": verified}


def _run_meta(d, run_id):
    meta = read_json(d / "runs" / run_id / "meta.json")
    if meta is None:
        raise RunError(f"no run meta at {d / 'runs' / run_id / 'meta.json'}")
    return meta


def merge_heads(heads):
    """One score meta line over several run ids of one set (e.g. one per repo): the same
    model, Claude Code version, binaries, prompt and config, task files merged."""
    for key in ("model", "claude_version", "glia", "prompt_sha256", "config_sha256"):
        if len({json.dumps(h[key], sort_keys=True) for h in heads}) > 1:
            raise RunError(f"refused: the runs differ in {key}")
    tasks_ = {}
    for h in heads:
        tasks_.update(h["tasks"])
    return {**heads[0], "run_id": "+".join(h["run_id"] for h in heads),
            "date": max(h["date"] for h in heads), "tasks": dict(sorted(tasks_.items()))}


def cmd_report(a, cfg, d):
    score = _module("score", HERE / "score.py")
    diag = _module("diag", HERE / "probe" / "diag.py")
    spec = load_set(a.set) if a.set else None
    task_list, _ = load_tasks(a.repos, a.tasks_dir)
    by_id = {t["id"]: t for _, t in task_list}
    all_repos = json.loads(Path(a.repos).read_text())["repos"]
    heads, rows, probe = [], [], []
    tmp = d / "tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    for rid in a.run_id:
        labels = set(_run_meta(d, rid).get("tasks") or {})
        # a run made with --only pins its own repos: score it against those alone
        repos_path = tmp / f"repos.{rid}.json"
        repos_path.write_text(json.dumps({"repos": [r for r in all_repos
                                                    if r["label"] in labels]}))
        head, rs = score.metrics_from_run(rid, d, repos_path, a.tasks_dir)
        heads.append(head)
        rows += rs
        for r in read_rows(d / "runs" / rid / "sessions.jsonl"):
            if r.get("terminated") in score.EXCLUDED:
                continue
            key = f"{r['task_id']}.{r['arm']}.{r['rep']}"
            tpath = d / "sessions" / rid / r["task_id"] / r["arm"] / f"{r['rep']}.jsonl"
            events = read_rows(tpath) if tpath.is_file() else []
            ppath = d / "patches" / rid / f"{key}.diff"
            patch = ppath.read_bytes().decode("utf-8", "replace") if ppath.is_file() else ""
            copy = str((d / "work" / rid / r["task_id"] / r["arm"] / str(r["rep"])).resolve())
            task = by_id[r["task_id"]]
            row = {"run_id": rid, "task_id": r["task_id"], "repo": r["repo"], "arm": r["arm"],
                   "rep": r["rep"], **probe_metrics(events, task, patch, copy, score),
                   "verdict": (diag.session(tpath, task)["verdict"]
                               if r["arm"] != "none" and tpath.is_file() else None)}
            probe.append(row)
    rows.sort(key=lambda x: (x["task_id"], x["arm"], x["rep"]))
    probe.sort(key=lambda x: (x["task_id"], x["arm"], x["rep"]))
    head = merge_heads(heads)
    baseline, _, marker = score.score(head, rows)
    if a.out_sessions:
        score.write_metrics(a.out_sessions, head, rows)
    if a.out_json:
        Path(a.out_json).write_text(json.dumps(baseline, indent=1, sort_keys=True) + "\n")
    if a.out_probe:
        Path(a.out_probe).write_text("".join(json.dumps(p, sort_keys=True) + "\n"
                                             for p in probe))
    verdicts = Counter(p["verdict"] for p in probe if p["verdict"])
    diag_line = "[agent-ab] diag verdicts " + " ".join(f"{v}={verdicts[v]}"
                                                       for v in DIAG_VERDICTS)
    md = render_probe_md(score, spec, head, rows, probe)
    if a.out_md:
        Path(a.out_md).write_text(md)
    else:
        print(md, end="")
    log(marker)
    log(diag_line)
    return 0


def render_probe_md(score, spec, head, rows, probe):
    """One set's section of the probe report: score.py's per-arm table and the probe
    pairs under its rule, the set's own metric, the diag verdict counts."""
    tm = score.task_means(rows)
    arms = sorted({a for _, a in tm})
    scored = [r for r in rows if r["metrics"] is not None]
    excluded = Counter(r["terminated"] for r in rows if r["metrics"] is None)
    name = spec["name"] if spec else "agent-ab"
    L = [f"### {name}", "",
         f"- runs: {head['run_id']}; model {head['model']}; {head['claude_version']}",
         f"- metric: {spec['metric'] if spec else 'score.py'}",
         f"- sessions scored: {len(scored)} over {len({r['task_id'] for r in scored})} tasks; "
         "excluded: " + (", ".join(f"{k} {v}" for k, v in sorted(excluded.items())) or "none"),
         ""]
    L += score.table(["arm"] + score.METRICS,
                     [[a] + [score.fmt(v) for v in score.arm_means(tm, a).values()]
                      for a in arms])
    pairs = {f"{x}-{y}": score.paired(tm, x, y) for x, y in PROBE_PAIRS
             if x in arms and y in arms}
    if pairs:
        L += [""] + score.pair_table(pairs) + [""]
        L += [f"- {p}: {score.reading(ms)}" for p, ms in pairs.items()]
    L += ["", "| arm | sessions | truth files | missed (mean) | all truth edited | "
          "turn to all edited (median) | tokens to all edited (median) | "
          "first-edit turn (mean) | verified after editing |", "|" + "---|" * 9]
    for a in arms:
        ps = [p for p in probe if p["arm"] == a]
        done = [p for p in ps if p["all_edited"]]
        med = (lambda xs: score.fmt(statistics.median(xs)) if xs else "-")
        L.append(f"| {a} | {len(ps)} | {sum(p['truth'] for p in ps)} | "
                 f"{score.fmt(score.mean([p['missed'] for p in ps]))} | "
                 f"{len(done)}/{len(ps)} | {med([p['all_edited_turn'] for p in done])} | "
                 f"{med([p['all_edited_tokens'] for p in done])} | "
                 f"{score.fmt(score.mean([p['first_edit_turn_mean'] for p in ps]))} | "
                 + (f"{sum(p['verified'] for p in ps)}/{len(ps)} |" if a != "none" else "- |"))
    task_missed = {}
    for p in probe:
        task_missed.setdefault((p["task_id"], p["arm"]), []).append(p["missed"])
    for x, y in PROBE_PAIRS:
        tids = sorted({t for t, a in task_missed if a == x and (t, y) in task_missed})
        if not tids:
            continue
        diffs = [score.mean(task_missed[(t, x)]) - score.mean(task_missed[(t, y)]) for t in tids]
        dd, lo, hi = score.bootstrap(diffs)
        v = ("n.s." if len(diffs) < 2 or lo <= 0 <= hi else "better" if hi < 0 else "worse")
        L.append(f"- missed {x}-{y}: diff {score.fmt(dd)} [{score.fmt(lo)}, {score.fmt(hi)}] "
                 f"over {len(diffs)} tasks: {v}")
    L += ["", "| arm | " + " | ".join(DIAG_VERDICTS) + " |", "|" + "---|" * 6]
    for a in arms:
        if a == "none":
            continue
        c = Counter(p["verdict"] for p in probe if p["arm"] == a and p["verdict"])
        L.append(f"| {a} | " + " | ".join(str(c[v]) for v in DIAG_VERDICTS) + " |")
    return "\n".join(L) + "\n"


def cmd_isolation(a, cfg, d):
    check_claude_version(cfg)
    auth = auth_kind(os.environ)
    metas = arm_metas(cfg, d, a.glia_repo)
    write_deny(d)
    stamp = time.strftime("isolation-%Y%m%d-%H%M%S")
    config_dir = fresh_config_dir(d, stamp)
    user_skills, all_ok = user_skill_names(), True
    for arm in cfg["arms"]:
        glia_arm = bool(cfg["arms"][arm].get("rev"))
        copy = (d / "work" / stamp / arm).resolve()
        copy.mkdir(parents=True)
        sh(["git", "init", "-q", copy])
        if glia_arm:
            install_skill(d, arm, copy)
        env = session_env(d, cfg, arm, config_dir)
        events = run_with_waits(claude_cmd(cfg, ISOLATION_PROMPT, 1), copy, env,
                                d / "sessions" / stamp / f"{arm}.jsonl", cfg["timeout_s"])[0]
        init, result = first(events, "system", "init"), result_event(events)
        v = init_violations(init, glia_arm, cfg["disallowed_tools"], user_skills)
        if result is None or result.get("is_error"):
            v.append("no successful result")
        ok = not v
        all_ok = all_ok and ok
        skills = ",".join((init or {}).get("skills") or [])
        log(f"[agent-ab] isolation arm={arm} mcp={len((init or {}).get('mcp_servers') or [])} "
            f"skills={skills} {'ok' if ok else 'FAIL'}")
        for reason in v:
            log(f"[agent-ab] isolation arm={arm} violation: {reason}")
        shutil.rmtree(copy, ignore_errors=True)
    write_json(d / "isolation.json", {"ok": all_ok, "claude_version": cfg["claude_version"],
                                      "model": cfg["model"], "auth": auth,
                                      "arms": {arm: m["sha"] for arm, m in metas.items()}})
    return 0 if all_ok else 1


def cmd_status(a, cfg, d):
    runs = sorted((d / "runs").glob("*/sessions.jsonl"), key=lambda p: p.stat().st_mtime) \
        if (d / "runs").is_dir() else []
    path = d / "runs" / a.run_id / "sessions.jsonl" if a.run_id else (runs[-1] if runs else None)
    if path is None or not path.exists():
        log("[agent-ab] status: no run")
        return 1
    rows = read_rows(path)
    log(f"[agent-ab] status run={path.parent.name} sessions={len(rows)} "
        f"usd={sum(r['total_cost_usd'] for r in rows):.2f}")
    for arm in cfg["arms"]:
        counts = Counter(r["terminated"] for r in rows if r["arm"] == arm)
        detail = " ".join(f"{k}={v}" for k, v in sorted(counts.items()))
        log(f"[agent-ab] status arm={arm} sessions={sum(counts.values())} {detail}".rstrip())
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--config", default=str(HERE / "config.json"))
    ap.add_argument("--repos", default=None, help="default: repos.json, or the set's")
    ap.add_argument("--tasks-dir", default=None, help="default: tasks/, or the set's dir")
    ap.add_argument("--glia-repo", default=str(GLIA_REPO))
    ap.add_argument("--set", help="a probe set dir (bench/cohort/tasks/agent/<set>)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("binaries")
    sub.add_parser("isolation")
    for name in ("pilot", "run"):
        p = sub.add_parser(name)
        p.add_argument("--run-id", required=name == "run")
        p.add_argument("--keep", action="store_true", help="keep each session's checkout")
        p.add_argument("--max-usd", type=float, help="override config max_usd")
        p.add_argument("--only", help="these repo labels only (comma-separated): one run id "
                       "per repo runs repos in parallel processes")
    sub.add_parser("status").add_argument("--run-id")
    b = sub.add_parser("build-set")
    b.add_argument("set_dir")
    b.add_argument("--check", action="store_true", help="re-derive and compare; exit 1 on drift")
    r = sub.add_parser("report")
    r.add_argument("--run-id", action="append", required=True)
    for flag in ("--out-md", "--out-json", "--out-sessions", "--out-probe"):
        r.add_argument(flag)
    a = ap.parse_args(argv)
    base = Path(a.set) if a.set else HERE
    a.repos = a.repos or str(base / "repos.json")
    a.tasks_dir = a.tasks_dir or str(base if a.set else HERE / "tasks")
    try:
        if a.cmd == "build-set":
            return build_set(a.set_dir, check=a.check)
        cfg = load_config(a.config)
        d = tasks.cache_dir().resolve()
        d.mkdir(parents=True, exist_ok=True)
        if a.cmd == "binaries":
            return cmd_binaries(a, cfg, d)
        if a.cmd == "isolation":
            return cmd_isolation(a, cfg, d)
        if a.cmd in ("pilot", "run"):
            return cmd_run(a, cfg, d, pilot=a.cmd == "pilot")
        if a.cmd == "report":
            return cmd_report(a, cfg, d)
        return cmd_status(a, cfg, d)
    except (RunError, tasks.TaskError) as e:
        print(f"[agent-ab] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
