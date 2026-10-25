#!/usr/bin/env python3
"""The agent before / after task set (FE.1): held-out commits of six public
repos, each a prompt (the commit subject, or the issue it names) and its
ground truth (the files and parent-coordinate line ranges the commit changed).

    python3 tasks.py build [--repo <label>] [--n 10] [--pin]
    python3 tasks.py check [--repo <label>]

`build` writes tasks/<label>.jsonl; `check` re-derives every committed row
from the bare clones (issue text carried over, never refetched, so it runs
offline) and exits 1 naming each differing id. Both print, per repo,
`[agent-ab] tasks repo=<label> rows=<n> issues=<k> after_gate=<g> sha256=<12>`
and then `[agent-ab] tasks total=<n> repos=<r>`.

Bare clones live under ${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}; nothing
is written inside the glia tree but tasks/*.jsonl (and repos.json on --pin).
GLIA_AGENT_AB_OFFLINE=1: no clone, fetch or `gh` call. Stdlib only.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CODE_EXTS = frozenset(
    ".go .ts .tsx .js .jsx .mjs .py .java .kt .dart .rs .cs .rb .php .swift .c .cc .cpp"
    " .h .hpp .scala .ex .exs .proto .sol .vue .svelte .astro".split())
TEST_DIRS = frozenset({"test", "tests", "__tests__", "testdata"})
TEST_RES = [re.compile(p) for p in (
    r"_test\.go$", r"^test_.*\.py$", r"_test\.py$", r"^Test.*\.java$", r".*Test\.java$",
    r"_test\.dart$", r"\.(spec|test)\.[jt]sx?$")]
BOT_RE = re.compile(r"\[bot\]|dependabot|renovate|github-actions", re.I)
PREFIX_RE = re.compile(r"^[a-z]+(\([^)]*\))?!?: ")
SKIP_SUBJECT_RE = re.compile(
    r"^(bump|release|merge|revert|update dependenc|upgrade dependenc|format|lint|typo"
    r"|changelog|docs?\b|v?\d+\.\d+)", re.I)
MERGE_PR_RE = re.compile(r"^Merge pull request #(\d+) from (\S+)")
CLOSING_RE = re.compile(r"\b(?:fix(?:e[sd])?|close[sd]?|resolve[sd]?)\s+#(\d+)\b", re.I)
REF_RE = re.compile(r"(?<![\w&])#(\d+)\b")
HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+")
GITHUB_RE = re.compile(r"^https://github\.com/([^/]+)/([^/]+?)(?:\.git)?/?$")
HEX40_RE = re.compile(r"^[0-9a-f]{40}$")
# A task is structure-shaped (the pilot, 2026-10-25: 36 of 60 one-file tasks, a grep away,
# and no agent ever reached for glia): 3..8 non-test code files whose names the prompt
# subject does not give away.
MIN_FILES, MAX_FILES, MIN_LINES, MAX_LINES = 3, 8, 5, 400
MAX_ISSUE_BODY, MAX_ISSUE_TRIES, MIN_STEM = 4000, 3, 4


class TaskError(Exception):
    pass


# --- untrusted text --------------------------------------------------------
# A Python mirror of code_domain::snapshots::redact_untrusted (the Rust is
# the reference; the security gate says untrusted PR / issue / commit text is
# redacted before it is stored). Over-redaction is the safe direction.
REDACTED = "***"
SECRET_NEEDLES = ("SECRET", "PASSWORD", "PASSWD", "TOKEN", "APIKEY", "API_KEY", "PRIVATE_KEY",
                  "CREDENTIAL", "ACCESS_KEY", "SESSION_KEY", "SALT", "SIGNING")
SECRET_PREFIXES = (("sk_live_", 8), ("sk_test_", 8), ("rk_live_", 8), ("rk_test_", 8),
                   ("whsec_", 16), ("ghp_", 20), ("gho_", 20), ("ghu_", 20), ("ghs_", 20),
                   ("ghr_", 20), ("github_pat_", 20), ("glpat-", 16), ("xoxb-", 10),
                   ("xoxp-", 10), ("xoxa-", 10), ("xoxr-", 10), ("xoxs-", 10), ("xapp-", 10),
                   ("sk-", 20), ("AIza", 30), ("npm_", 30), ("pypi-", 30), ("AKIA", 16),
                   ("ASIA", 16))
_NW = r"(?<![A-Za-z0-9_])"
_REDACT = [
    (re.compile(r"-----BEGIN [^\n]*?PRIVATE KEY-----.*?(?:-----END [^\n]*?-----|\Z)", re.S),
     lambda m: REDACTED),
    (re.compile(r"([A-Za-z][A-Za-z0-9+.\-]*://)[^\s/?#@\"'<>()\[\]{}`,]*:[^\s/?#\"'<>()\[\]{}`,]*@"),
     lambda m: m.group(1) + REDACTED + "@"),
    (re.compile(_NW + "(?:" + "|".join(f"{re.escape(p)}[A-Za-z0-9_\\-]{{{n},}}"
                                        for p, n in SECRET_PREFIXES) + ")"), lambda m: REDACTED),
    (re.compile(_NW + r"eyJ[A-Za-z0-9_\-]{7,}\.[A-Za-z0-9_\-]{4,}\.[A-Za-z0-9_\-]*"),
     lambda m: REDACTED),
    (re.compile(_NW + r"((?i:bearer) +)[A-Za-z0-9\-_.+/=~]{8,}"), lambda m: m.group(1) + REDACTED),
    (re.compile(_NW + r"((?i:basic) +)(?![a-z]+(?![A-Za-z0-9+/=]))[A-Za-z0-9+/=]{8,}"),
     lambda m: m.group(1) + REDACTED),
]
_ASSIGN_RE = re.compile(_NW + r"([A-Za-z_][A-Za-z0-9_\-]*)([\"']?[ \t]*(?:=>|:=|=(?![=~])|:(?!:))"
                        r"[ \t]*)(?:([\"'`])([^\n]*?)(\3|(?=\n)|\Z)|([^\s,;&)}\]<>\"'`]+))")


def _assign(m):
    key, sep, quote, qvalue, close, bare = m.groups()
    value = qvalue if quote else bare
    if (not any(n in key.upper().replace("-", "_") for n in SECRET_NEEDLES) or not value
            or value == REDACTED):
        return m.group(0)
    return key + sep + (quote + REDACTED + close if quote else REDACTED)


def redact(text):
    for rx, sub in _REDACT:
        text = rx.sub(sub, text)
    return _ASSIGN_RE.sub(_assign, text)


# --- git -------------------------------------------------------------------
def offline():
    return os.environ.get("GLIA_AGENT_AB_OFFLINE") == "1"


def cache_dir():
    return Path(os.environ.get("GLIA_AGENT_AB_DIR") or Path.home() / ".cache" / "glia-agent-ab")


def git(gd, *args, check=True):
    p = subprocess.run(["git", "--git-dir", str(gd), "-c", "core.quotePath=false", *args],
                       capture_output=True, env={**os.environ, "LC_ALL": "C"})
    if check and p.returncode != 0:
        raise TaskError(f"git {' '.join(args[:3])}: {p.stderr.decode(errors='replace').strip()}")
    return p


def has_commit(gd, rev):
    return git(gd, "cat-file", "-e", f"{rev}^{{commit}}", check=False).returncode == 0


def ensure_clone(repo):
    gd = cache_dir() / f"{repo['label']}.git"
    if not gd.exists():
        if offline():
            raise TaskError(f"{repo['label']}: no bare clone at {gd} (offline)")
        gd.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "--bare", "--quiet", repo["url"], str(gd)], check=True)
    for rev in (repo["end_rev"], repo.get("gate_rev")):
        if rev and not has_commit(gd, rev):
            if not offline():
                git(gd, "fetch", "--quiet", repo["url"], rev, check=False)
            if not has_commit(gd, rev):
                raise TaskError(f"{repo['label']}: cannot resolve {rev} in {gd}")
    return gd


def validate(repo):
    for key in ("label", "url", "end_rev", "lang", "gate_rev", "tuned", "public"):
        if key not in repo:
            raise TaskError(f"repos.json row {repo.get('label')!r}: missing {key}")
    if repo["public"] is not True:
        raise TaskError(f"{repo['label']}: refused, not marked public: true")
    if not repo["url"].startswith("https://"):
        raise TaskError(f"{repo['label']}: refused, url is not https")
    for key in ("end_rev", "gate_rev"):
        if repo[key] is not None and not HEX40_RE.match(repo[key]):
            raise TaskError(f"{repo['label']}: {key} is not 40 hex")


# --- selection -------------------------------------------------------------
def is_test(path):
    parts = path.split("/")
    return bool(TEST_DIRS & set(parts[:-1])) or any(rx.search(parts[-1]) for rx in TEST_RES)


def is_code(path):
    return os.path.splitext(path)[1] in CODE_EXTS


def subject_ok(subject):
    stripped = PREFIX_RE.sub("", subject, count=1)
    return stripped if len(stripped.split()) >= 4 and not SKIP_SUBJECT_RE.match(stripped) else None


def log_first_parent(gd, end_rev):
    out = git(gd, "log", "--first-parent", "--format=%H%x00%P%x00%an%x00%ae%x00%cn%x00%ce%x00%at"
              "%x00%B%x1e", end_rev).stdout.decode("utf-8", errors="replace")
    for rec in out.split("\x1e"):
        rec = rec.lstrip("\n")
        if rec:
            h, parents, an, ae, cn, ce, at, body = rec.split("\x00", 7)
            yield {"commit": h, "parents": parents.split(), "people": (an, ae, cn, ce),
                   "author_time": int(at), "message": body.replace("\r\n", "\n")}


def changed_files(gd, parent, commit):
    """[(status, path, old_path, added+deleted)] for parent..commit, renames detected."""
    raw = git(gd, "diff", "--no-ext-diff", "--no-textconv", "-M", "-z", "--name-status",
              parent, commit).stdout.decode("utf-8", errors="replace").split("\0")
    num = git(gd, "diff", "--no-ext-diff", "--no-textconv", "-M", "-z", "--numstat",
              parent, commit).stdout.decode("utf-8", errors="replace").split("\0")
    lines, i = {}, 0
    while i < len(num) - 1:
        added, deleted, path = num[i].split("\t", 2)
        if path == "":  # a rename: old and new follow
            path, i = num[i + 2], i + 3
        else:
            i += 1
        lines[path] = sum(int(x) for x in (added, deleted) if x != "-")
    files, i = [], 0
    while i < len(raw) - 1:
        status = raw[i][0]
        if status in "RC":
            old, path, i = raw[i + 1], raw[i + 2], i + 3
        else:
            old, path, i = None, raw[i + 1], i + 2
        status = {"T": "M", "C": "A"}.get(status, status)
        files.append((status, path, old, lines.get(path, 0)))
    return files


def hunks(gd, parent, commit, path, old):
    out = git(gd, "diff", "--no-ext-diff", "--no-textconv", "-M", "-U0", parent, commit, "--",
              *([old] if old else []), path).stdout.decode("utf-8", errors="replace")
    res = []
    for line in out.splitlines():
        m = HUNK_RE.match(line)
        if m:
            res.append([int(m.group(1)), 1 if m.group(2) is None else int(m.group(2))])
    return res


def diff_truth(gd, parent, commit):
    """The truth / stats of parent..commit when it is a task-sized change, else None."""
    files = changed_files(gd, parent, commit)
    code = [f for f in files if is_code(f[1]) and not is_test(f[1])]
    if not MIN_FILES <= len(code) <= MAX_FILES or not any(f[0] in "MR" for f in code):
        return None
    if not MIN_LINES <= sum(f[3] for f in files if is_code(f[1])) <= MAX_LINES:
        return None
    if sum(f[0] == "R" for f in files) * 2 > len(files):
        return None
    tests = sorted(f[1] for f in files if is_test(f[1]))
    other = sorted(f[1] for f in files if not is_test(f[1]) and not is_code(f[1]))
    truth = {"code": [{"path": p, "status": s, "old_path": o, "hunks": hunks(gd, parent, commit,
                                                                              p, o)}
                      for s, p, o, _ in sorted(code, key=lambda f: f[1])],
             "tests": tests, "other": other}
    stats = {"code_files": len(code), "code_lines": sum(f[3] for f in code),
             "test_files": len(tests)}
    return truth, stats


def names_a_file(text, truth):
    """Does text name a changed code file (its stem, at least MIN_STEM chars, as a word)?"""
    low = text.lower()
    for f in truth["code"]:
        stem = os.path.splitext(os.path.basename(f["path"]))[0].lower()
        if len(stem) >= MIN_STEM and re.search(rf"(?<![a-z0-9_]){re.escape(stem)}(?![a-z0-9_])",
                                               low):
            return True
    return False


def candidate(entry):
    """(parent, raw subject, prompt subject, pr, issue-ref text) or None, rules (a), (b), (f)."""
    if any(BOT_RE.search(x) for x in entry["people"]) or not entry["parents"]:
        return None
    lines = entry["message"].split("\n")
    if len(entry["parents"]) == 1:
        prompt = subject_ok(lines[0].strip())
        return prompt and (entry["parents"][0], lines[0].strip(), prompt, None,
                           entry["message"])
    m = MERGE_PR_RE.match(lines[0])
    if not m or BOT_RE.search(m.group(2)):
        return None
    title = next((x.strip() for x in lines[1:] if x.strip()), "")
    prompt = title and subject_ok(title)
    return prompt and (entry["parents"][0], title, prompt, int(m.group(1)),
                       "\n".join(lines[1:]))


def issue_refs(text):
    seen = []
    for rx in (CLOSING_RE, REF_RE):
        for m in rx.finditer(text):
            if int(m.group(1)) not in seen:
                seen.append(int(m.group(1)))
    return seen[:MAX_ISSUE_TRIES]


def fetch_issue(url, refs):
    """(issue | None, issue_fetch) — a snapshot step at build time, never during a run."""
    gh = GITHUB_RE.match(url)
    if not refs or not gh:
        return None, "none"
    if offline():
        return None, "unavailable"
    for n in refs:
        try:
            p = subprocess.run(["gh", "api", f"repos/{gh.group(1)}/{gh.group(2)}/issues/{n}",
                                "--jq", '{title, body, pr: has("pull_request")}'],
                               capture_output=True, text=True, timeout=60)
        except (OSError, subprocess.TimeoutExpired):
            return None, "unavailable"
        if p.returncode != 0:
            if "Not Found" in p.stderr or "HTTP 404" in p.stderr or "HTTP 410" in p.stderr:
                continue
            return None, "unavailable"
        d = json.loads(p.stdout)
        title = (d.get("title") or "").strip()
        if d.get("pr") or not title:
            continue
        body = (d.get("body") or "").replace("\r\n", "\n")
        if len(body) > MAX_ISSUE_BODY:
            cut = body.rfind("\n", 0, MAX_ISSUE_BODY + 1)
            body = body[:cut if cut > 0 else MAX_ISSUE_BODY]
        return {"number": n, "title": redact(title), "body": redact(body)}, "ok"
    return None, "none"


def select(repo, gd, n, carried=None):
    """The newest n eligible rows of repo; `carried` maps id -> (issue, issue_fetch)."""
    rows, chosen = [], set()
    for entry in log_first_parent(gd, repo["end_rev"]):
        if len(rows) >= n:
            break
        c = candidate(entry)
        if not c or entry["commit"] in chosen:
            continue
        parent, subject, prompt, pr, ref_text = c
        got = diff_truth(gd, parent, entry["commit"])
        if not got or names_a_file(prompt, got[0]):
            continue
        rid = f"{repo['label']}-{entry['commit'][:12]}"
        if carried is not None:
            issue, fetched = carried.get(rid, (None, "none"))
        else:
            issue, fetched = fetch_issue(repo["url"], issue_refs(ref_text))
        if issue and names_a_file(issue["title"], got[0]):
            continue
        gate = repo["gate_rev"]
        after = None if gate is None else (entry["commit"] != gate and git(
            gd, "merge-base", "--is-ancestor", gate, entry["commit"], check=False).returncode == 0)
        chosen.add(entry["commit"])
        rows.append({"id": rid, "repo": repo["label"], "commit": entry["commit"],
                     "parent": parent, "author_time": entry["author_time"],
                     "subject": redact(subject), "prompt_subject": redact(prompt), "pr": pr,
                     "issue": issue, "issue_fetch": fetched, "after_gate_rev": after,
                     "truth": got[0], "stats": got[1]})
    return rows


# --- files -----------------------------------------------------------------
def render(rows):
    return "".join(json.dumps(r, sort_keys=True, ensure_ascii=False) + "\n" for r in rows)


def marker(label, text, rows):
    sha = hashlib.sha256(text.encode()).hexdigest()[:12]
    print(f"[agent-ab] tasks repo={label} rows={len(rows)} "
          f"issues={sum(r['issue'] is not None for r in rows)} "
          f"after_gate={sum(r['after_gate_rev'] is True for r in rows)} sha256={sha}")


def load_repos(path):
    repos = json.loads(Path(path).read_text())["repos"]
    for r in repos:
        validate(r)
    return repos


def render_repos(repos):
    rows = ",\n".join("  " + json.dumps(r) for r in repos)
    return '{\n "repos": [\n' + rows + "\n ]\n}\n"


def pin(repos, path):
    for r in repos:
        out = subprocess.run(["git", "ls-remote", r["url"], "HEAD"], capture_output=True,
                             text=True, check=True).stdout.split()
        if not out or not HEX40_RE.match(out[0]):
            raise TaskError(f"{r['label']}: ls-remote gave no HEAD")
        r["end_rev"] = out[0]
    Path(path).write_text(render_repos(repos))


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("cmd", choices=["build", "check"])
    ap.add_argument("--repo", help="one label only")
    ap.add_argument("--n", type=int, default=10, help="rows per repo (build)")
    ap.add_argument("--pin", action="store_true",
                    help="build: re-pin every end_rev to the upstream HEAD first")
    ap.add_argument("--repos", default=str(HERE / "repos.json"), help=argparse.SUPPRESS)
    ap.add_argument("--out", default=str(HERE / "tasks"), help=argparse.SUPPRESS)
    a = ap.parse_args(argv)
    try:
        repos = load_repos(a.repos)
        if a.cmd == "build" and a.pin:
            if offline():
                raise TaskError("--pin needs the network")
            pin(repos, a.repos)
        if a.repo:
            repos = [r for r in repos if r["label"] == a.repo]
            if not repos:
                raise TaskError(f"no repo labelled {a.repo!r}")
        out, total, bad = Path(a.out), 0, []
        for repo in repos:
            gd = ensure_clone(repo)
            path = out / f"{repo['label']}.jsonl"
            if a.cmd == "build":
                rows = select(repo, gd, a.n)
                text = render(rows)
                out.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            else:
                if not path.exists():
                    raise TaskError(f"{path} is missing")
                text = path.read_text()
                rows = [json.loads(x) for x in text.splitlines()]
                carried = {r["id"]: (r["issue"], r["issue_fetch"]) for r in rows}
                fresh = {r["id"]: r for r in select(repo, gd, max(len(rows), 1), carried)}
                old = {r["id"]: r for r in rows}
                for rid in sorted(set(fresh) | set(old)):
                    if fresh.get(rid) != old.get(rid):
                        bad.append(rid)
                if render(list(fresh.values())) != text and not bad:
                    bad.append(f"{repo['label']} (row order or rendering)")
            marker(repo["label"], text, rows)
            total += len(rows)
        print(f"[agent-ab] tasks total={total} repos={len(repos)}")
        for rid in bad:
            print(f"[agent-ab] check differs: {rid}", file=sys.stderr)
        return 1 if bad else 0
    except (TaskError, subprocess.CalledProcessError, OSError, ValueError) as e:
        print(f"[agent-ab] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
