#!/usr/bin/env python3
"""The agent before / after session driver (FE.2): every FE.1 task run as a
headless Claude Code session under three arms (`none`, `g052`, `g053`), each in
its own leak-free checkout of the task commit's parent.

    python3 run.py binaries              # build each glia arm's release binary once
    python3 run.py isolation             # one cheap session per arm, checks the init event
    python3 run.py pilot [--run-id <id>] # first task of each repo x every arm x 1 rep
    python3 run.py run --run-id <id>     # the whole matrix (resumes; budget stop)
    python3 run.py status [--run-id <id>]

Everything is written under ${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab} (the
FE.1 bare clones' directory), never in the glia tree. Markers on stdout:
`[agent-ab] binary ..`, `[agent-ab] isolation ..`, `[agent-ab] session ..`,
`[agent-ab] pilot ..`, `[agent-ab] budget stop ..`. Stdlib only.
"""
import argparse
import hashlib
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
    skill = sh(["git", "-C", glia_repo, "show", f"{sha}:skills/glia/SKILL.md"]).stdout
    skill_path = d / "skills" / arm / "glia" / "SKILL.md"
    skill_path.parent.mkdir(parents=True, exist_ok=True)
    skill_path.write_text(skill)
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
def checkout(d, label, parent, commit, copy):
    gd = d / f"{label}.git"
    if not gd.is_dir():
        raise RunError(f"{label}: no bare clone at {gd} (run tasks.py build)")
    sh(["git", "--git-dir", gd, "config", "uploadpack.allowAnySHA1InWant", "true"])
    if copy.exists():
        shutil.rmtree(copy)
    copy.mkdir(parents=True)
    sh(["git", "init", "-q", copy])
    sh(["git", "-C", copy, "fetch", "-q", "--no-tags", gd, parent])
    sh(["git", "-C", copy, "checkout", "-q", "--detach", "FETCH_HEAD"])
    sh(["git", "-C", copy, "branch", "-q", "-f", "main", "FETCH_HEAD"])
    sh(["git", "-C", copy, "checkout", "-q", "main"])
    with open(copy / ".git" / "info" / "exclude", "a") as f:
        f.write(".glia/\n.claude/\n")
    if sh(["git", "-C", copy, "cat-file", "-e", f"{commit}^{{commit}}"],
          check=False).returncode == 0:
        raise RunError(f"leak: {copy} holds the task commit {commit[:12]}; run aborted")


def install_skill(d, arm, copy):
    dst = copy / ".claude" / "skills" / "glia" / "SKILL.md"
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(d / "skills" / arm / "glia" / "SKILL.md", dst)


def prep(d, cfg, arm, copy, env, log_path):
    """history sync, build (and calibrate for an arm that sets it); outside the
    agent's clock. Returns (prep_ms, ok)."""
    steps = [["history", "sync"], ["build"]]
    if cfg["arms"][arm].get("calibrate"):
        steps.append(["history", "calibrate"])
    t0, ok = time.monotonic(), True
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with open(log_path, "wb") as out:
        for step in steps:
            try:
                p = subprocess.run([str(d / "bin" / arm / "glia"), *step, str(copy)], cwd=copy,
                                   env=env, stdout=out, stderr=subprocess.STDOUT,
                                   stdin=subprocess.DEVNULL, timeout=PREP_TIMEOUT_S)
                ok = p.returncode == 0
            except subprocess.TimeoutExpired:
                ok = False
            if not ok:
                break
    if ok:
        install_skill(d, arm, copy)
    return int((time.monotonic() - t0) * 1000), ok


# --- one Claude Code session -------------------------------------------------
def claude_cmd(cfg, prompt, max_turns):
    cmd = ["claude", "-p", prompt, "--model", cfg["model"], "--output-format", "stream-json",
           "--verbose", "--mcp-config", str(HERE / "empty-mcp.json"), "--strict-mcp-config",
           "--setting-sources", "project", "--permission-mode", "bypassPermissions",
           "--max-turns", str(max_turns)]
    if cfg.get("disabled_plugins"):
        # Claude Code's own built-in plugins load under any CLAUDE_CONFIG_DIR; the
        # isolation check refuses every plugin, so the run turns them off
        cmd += ["--settings", json.dumps(
            {"enabledPlugins": {p: False for p in cfg["disabled_plugins"]}}, sort_keys=True)]
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


def _rel(copy, cwd, path):
    if not path or path.startswith("-"):
        return None
    ap = os.path.normpath(os.path.join(cwd, os.path.expanduser(path)))
    rp = os.path.relpath(ap, copy)
    if rp == "." or rp.startswith(".."):
        return None
    if rp.split(os.sep)[0] in SKIP_DIRS:
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


def session_metrics(events, copy):
    copy = os.path.realpath(copy)
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
            add(_rel(copy, copy, str(inp.get("file_path") or "")))
        elif name == "Bash":
            cwd = copy
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
                        rel = _rel(copy, cwd, f)
                        if rel is None:
                            continue
                        if tracked is None:
                            tracked = set(sh(["git", "-C", copy, "ls-tree", "-r", "--name-only",
                                              "HEAD"], check=False).stdout.splitlines())
                        if os.path.isfile(os.path.join(copy, rel)) or rel in tracked:
                            add(rel)
    return {"files_opened": opened, "tool_calls": dict(sorted(tool_calls.items())),
            "glia_calls": glia_calls, "grep_calls": grep_calls}


def write_patch(copy, path):
    sh(["git", "-C", copy, "add", "-A", "-N", "."], check=False)
    diff = subprocess.run(["git", "-C", str(copy), "diff", "--no-color", "-U0", "HEAD"],
                          capture_output=True).stdout
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(diff)
    return {"path": str(path), "sha256": sha256(diff), "bytes": len(diff)}


def reset_copy(copy):
    sh(["git", "-C", copy, "reset", "-q", "--hard"], check=False)
    sh(["git", "-C", copy, "clean", "-fdq"], check=False)


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
    try:
        checkout(d, repo["label"], task["parent"], task["commit"], copy)
        if glia_arm:
            prep_log = d / "sessions" / run_id / task["id"] / arm / f"{rep}.prep.log"
            row["prep_ms"], ok = prep(d, cfg, arm, copy, env, prep_log)
            if not ok:
                row["terminated"] = "prep_failed"
                return row
        prompt = render_prompt(ctx["template"], repo, task,
                               cfg.get("glia_prompt") if glia_arm else None)
        out = d / "sessions" / run_id / task["id"] / arm / f"{rep}.jsonl"
        events, _, timed_out, wall_ms, attempts = run_with_waits(
            claude_cmd(cfg, prompt, cfg["max_turns"]), copy, env, out, cfg["timeout_s"],
            reset=lambda: reset_copy(copy))
        init, result = first(events, "system", "init"), result_event(events)
        row.update(wall_ms=wall_ms, attempts=attempts, model=(init or {}).get("model"))
        if result is not None:
            usage = result.get("usage") or {}
            row.update(num_turns=result.get("num_turns") or 0,
                       duration_ms=result.get("duration_ms") or 0,
                       total_cost_usd=float(result.get("total_cost_usd") or 0.0),
                       usage={k: int(usage.get(k) or 0) for k in row["usage"]})
        row.update(session_metrics(events, copy))
        row["patch"] = write_patch(copy, d / "patches" / run_id / f"{key}.diff")
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
    template = load_template()
    pins = {"config": pinned_config(cfg), "prompt": sha256(template), "tasks": shas,
            "claude_version": cfg["claude_version"], "model": cfg["model"],
            "arms": {arm: {"sha": m["sha"], "version": m["version"]} for arm, m in metas.items()}}
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
           "template": template, "config_dir": fresh_config_dir(d, run_id),
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
    ap.add_argument("--repos", default=str(HERE / "repos.json"))
    ap.add_argument("--tasks-dir", default=str(HERE / "tasks"))
    ap.add_argument("--glia-repo", default=str(GLIA_REPO))
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("binaries")
    sub.add_parser("isolation")
    for name in ("pilot", "run"):
        p = sub.add_parser(name)
        p.add_argument("--run-id", required=name == "run")
        p.add_argument("--keep", action="store_true", help="keep each session's checkout")
        p.add_argument("--max-usd", type=float, help="override config max_usd")
    sub.add_parser("status").add_argument("--run-id")
    a = ap.parse_args(argv)
    try:
        cfg = load_config(a.config)
        d = tasks.cache_dir().resolve()
        d.mkdir(parents=True, exist_ok=True)
        if a.cmd == "binaries":
            return cmd_binaries(a, cfg, d)
        if a.cmd == "isolation":
            return cmd_isolation(a, cfg, d)
        if a.cmd in ("pilot", "run"):
            return cmd_run(a, cfg, d, pilot=a.cmd == "pilot")
        return cmd_status(a, cfg, d)
    except (RunError, tasks.TaskError) as e:
        print(f"[agent-ab] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
